//! Screen pixels live only during recognition. Only this text payload is queued.
use hx_macos_sys::{Focus, Text};
use hx_schema::{
    Completeness,
    kinds::{TextSnapshot, TextSource},
    sha256_hex,
};
use hx_visual::OcrResult;

/// Preserve every OCR observation (including geometry/confidence) and the full AX
/// sample. Preserve repeated lines: identical names in separate messages or
/// columns still carry positional meaning.
pub fn merge(focus: &Focus, ocr: OcrResult, ax: Option<Text>) -> serde_json::Value {
    let mut text = focus.title.clone().unwrap_or_else(|| focus.name.clone());
    text.push_str("\n\nScreen text (top to bottom):\n");
    // Vision returns reading order. Keep it: sorting by x/y scrambles columns.
    for line in &ocr.lines {
        text.push_str(&line.text);
        text.push('\n');
    }
    text.push_str("\nAccessibility text (may include offscreen content):\n");
    if let Some(ax) = &ax {
        text.push_str(&ax.text);
        text.push('\n');
    }
    let mut payload = serde_json::to_value(TextSnapshot {
        surface: focus.bundle_id.clone(),
        source: TextSource::Ocr,
        text_sha256: sha256_hex(text.as_bytes()),
        text,
        completeness: Completeness::Partial,
        // AX can include offscreen content; only the OCR observations prove visibility.
        visible: None,
    })
    .expect("serializable screen text");
    payload["window_title"] = serde_json::json!(focus.title);
    payload["app_name"] = serde_json::json!(focus.name);
    payload["app_bundle_id"] = serde_json::json!(focus.bundle_id);
    payload["ocr"] = serde_json::to_value(ocr).expect("serializable OCR");
    payload["accessibility"] = match ax {
        Some(ax) => {
            serde_json::json!({"text": ax.text, "truncated": ax.truncated, "nodes": ax.nodes})
        }
        None => serde_json::Value::Null,
    };
    payload
}

/// Recognition runs on the screen collector thread, before the persistence
/// queue. Release encoded pixels before querying AX or returning a text record.
pub(crate) fn recognize(
    focus: &Focus,
    bytes: Vec<u8>,
    engine: &dyn hx_visual::OcrEngine,
    accessibility: impl FnOnce() -> Option<Text>,
) -> anyhow::Result<serde_json::Value> {
    let ocr = engine.ocr(&bytes, "image/jpeg")?;
    drop(bytes);
    Ok(merge(focus, ocr, accessibility()))
}

/// Read-only diagnostic for the exact capture/recognition path. Never opens an
/// outbox, contacts a server, requests permissions, or changes the login service.
pub fn capture_once() -> anyhow::Result<serde_json::Value> {
    use anyhow::{Context, ensure};
    use hx_visual::{OcrEngine, VisionCliEngine};
    use std::time::{Duration, Instant};
    let permissions = hx_macos_sys::permissions();
    ensure!(
        permissions.screen && permissions.accessibility,
        "Screen Recording and Accessibility must already be granted; no permission request was made"
    );
    let main_loop = hx_macos_sys::MainLoop::new().context("main thread required")?;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<_> {
            let mut focus = hx_macos_sys::frontmost().context("no foreground window")?;
            focus.title = hx_macos_sys::window_title(&focus);
            ensure!(!hx_macos_sys::secure_input(), "secure input active");
            let stream = hx_macos_sys::ScreenStream::start(&focus).map_err(anyhow::Error::msg)?;
            let start = Instant::now();
            loop {
                if let Some(frame) = stream.pop() {
                    let bytes = frame.jpeg().map_err(anyhow::Error::msg)?;
                    let hash = sha256_hex(&bytes);
                    let ocr = VisionCliEngine::default().ocr(&bytes, "image/jpeg")?;
                    drop(bytes);
                    drop(frame);
                    drop(stream);
                    let ax =
                        hx_macos_sys::window_text(&focus, 256, Duration::from_millis(150), 65536);
                    let mut current = hx_macos_sys::frontmost().context("lost focus")?;
                    current.title = hx_macos_sys::window_title(&current);
                    ensure!(
                        current == focus && !hx_macos_sys::secure_input(),
                        "focus changed during capture"
                    );
                    let mut payload = merge(&focus, ocr, ax);
                    payload["visual_sha256"] = serde_json::json!(hash);
                    return Ok(payload);
                }
                ensure!(
                    start.elapsed() < Duration::from_secs(10),
                    "no screen frame received"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        })();
        let _ = tx.send(result);
    });
    loop {
        if let Ok(result) = rx.try_recv() {
            return result;
        }
        main_loop.pump(Duration::from_millis(50));
    }
}

/// Re-read a preexisting historical screenshot without copying its image to disk.
pub fn replay(record: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    use anyhow::Context;
    use hx_visual::{OcrEngine, VisionCliEngine};
    let bytes = std::fs::read(
        record["image_path"]
            .as_str()
            .context("image_path required")?,
    )?;
    let hash = sha256_hex(&bytes);
    let ocr = VisionCliEngine::default().ocr(&bytes, "image/jpeg")?;
    drop(bytes);
    let focus = Focus {
        bundle_id: record["app_bundle_id"]
            .as_str()
            .context("app_bundle_id required")?
            .into(),
        name: record["app_name"].as_str().unwrap_or_default().into(),
        title: record["window_title"].as_str().map(str::to_owned),
        pid: 0,
        electron: false,
    };
    let ax = record["accessibility"]["text"].as_str().map(|text| Text {
        text: text.into(),
        truncated: record["accessibility"]["completeness"] == "truncated",
        nodes: 0,
    });
    let mut payload = merge(&focus, ocr, ax);
    payload["visual_sha256"] = serde_json::json!(hash);
    Ok(payload)
}
