use hx_macos_sys::{Focus, Hit, Input, Text};
use std::{sync::mpsc, time::Duration};
pub enum Request {
    Text(Focus),
    Click(Focus, Input),
}
pub enum Observation {
    Text(Text),
    Click(Hit, u32),
}
pub struct Sample {
    pub focus: Focus,
    pub observation: Observation,
    pub observed_at: chrono::DateTime<chrono::Utc>,
    pub nanos: u64,
}
pub struct Worker {
    pub request: mpsc::SyncSender<Request>,
    pub samples: mpsc::Receiver<Sample>,
    title_request: mpsc::SyncSender<Focus>,
    titles: mpsc::Receiver<(i32, Option<String>)>,
    pub dropped: std::sync::Arc<std::sync::atomic::AtomicU64>,
}
impl Worker {
    /// The focused window's title. `None` means the app did not answer in time,
    /// which is not the same as a window without a title (`Some(None)`).
    pub fn title(&self, focus: &Focus, wait: Duration) -> Option<Option<String>> {
        if !hx_macos_sys::permissions().accessibility {
            return Some(None);
        }
        while self.titles.try_recv().is_ok() {}
        self.title_request.try_send(focus.clone()).ok()?;
        let (sample_pid, title) = self.titles.recv_timeout(wait).ok()?;
        (sample_pid == focus.pid).then_some(title)
    }

    pub fn start() -> Self {
        let (request, rx) = mpsc::sync_channel(16);
        let (tx, samples) = mpsc::sync_channel(16);
        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let worker_dropped = dropped.clone();
        let (title_request, title_rx) = mpsc::sync_channel::<Focus>(1);
        let (title_tx, titles) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            while let Ok(focus) = title_rx.recv() {
                if title_tx
                    .send((focus.pid, hx_macos_sys::window_title(&focus)))
                    .is_err()
                {
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            while let Ok(request) = rx.recv() {
                let focus = match &request {
                    Request::Text(f) | Request::Click(f, _) => f,
                };
                if hx_macos_sys::secure_input()
                    || hx_macos_sys::frontmost().is_none_or(|f| f.pid != focus.pid)
                {
                    continue;
                }
                let observation = match request {
                    Request::Text(ref f) => {
                        hx_macos_sys::window_text(f, 256, Duration::from_millis(150), 65536)
                            .map(Observation::Text)
                    }
                    Request::Click(_, input) => hx_macos_sys::hit_test(focus, input.x, input.y)
                        .map(|hit| {
                            Observation::Click(
                                hit,
                                ((hx_macos_sys::continuous_nanos() - input.nanos) / 1_000_000)
                                    .min(u32::MAX as u64) as u32,
                            )
                        }),
                };
                if let Some(observation) = observation
                    && tx
                        .try_send(Sample {
                            focus: focus.clone(),
                            observation,
                            observed_at: chrono::Utc::now(),
                            nanos: hx_macos_sys::continuous_nanos(),
                        })
                        .is_err()
                {
                    worker_dropped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
        });
        Self {
            request,
            samples,
            title_request,
            titles,
            dropped,
        }
    }
}
