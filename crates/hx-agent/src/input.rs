use hx_macos_sys::Input;
use hx_schema::kinds::InputBucket;
use std::collections::BTreeMap;
const SECOND: u64 = 1_000_000_000;
pub struct Buckets {
    next: u64,
    pending: BTreeMap<u64, InputBucket>,
    position: Option<(f64, f64)>,
    pub dropped: u64,
}
impl Buckets {
    pub fn new(now: u64) -> Self {
        Self {
            next: now / SECOND * SECOND,
            pending: BTreeMap::new(),
            position: None,
            dropped: 0,
        }
    }
    pub fn reset(&mut self, now: u64) {
        self.dropped += self.pending.len() as u64;
        self.pending.clear();
        self.next = now / SECOND * SECOND;
        self.position = None;
    }
    pub fn push(&mut self, event: Input) {
        let start = event.nanos / SECOND * SECOND;
        if start < self.next || start > self.next + 4 * SECOND {
            self.dropped += 1;
            return;
        }
        let bucket = self.pending.entry(start).or_insert(InputBucket {
            bucket_ms: 1000,
            ..Default::default()
        });
        match event.kind {
            10 => bucket.keys = bucket.keys.saturating_add(1),
            1 | 3 | 25 => bucket.clicks = bucket.clicks.saturating_add(1),
            22 => bucket.scroll_events = bucket.scroll_events.saturating_add(1),
            5 | 6 | 7 | 26 | 27 => {
                if let Some((x, y)) = self.position {
                    bucket.mouse_px = bucket
                        .mouse_px
                        .saturating_add(((event.x - x).hypot(event.y - y)) as u32);
                }
                self.position = Some((event.x, event.y));
            }
            _ => {}
        }
    }
    pub fn complete(&mut self, now: u64) -> Vec<(u64, InputBucket)> {
        let end = now / SECOND * SECOND;
        if end.saturating_sub(self.next) > 5 * SECOND {
            let skipped = (end - self.next) / SECOND - 5;
            self.dropped += skipped;
            self.next += skipped * SECOND;
            self.pending.retain(|start, _| *start >= self.next);
        }
        let mut result = vec![];
        while self.next < end {
            result.push((
                self.next,
                self.pending.remove(&self.next).unwrap_or(InputBucket {
                    bucket_ms: 1000,
                    ..Default::default()
                }),
            ));
            self.next += SECOND;
        }
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn input(kind: u32, nanos: u64, x: f64, y: f64) -> Input {
        Input { kind, nanos, x, y }
    }
    #[test]
    fn fixed_buckets_keep_boundaries_and_zero_input() {
        let mut b = Buckets::new(0);
        b.push(input(10, 999_999_999, 0., 0.));
        b.push(input(1, SECOND, 0., 0.));
        let complete = b.complete(3 * SECOND);
        assert_eq!(complete.len(), 3);
        assert_eq!(complete[0].1.keys, 1);
        assert_eq!(complete[1].1.clicks, 1);
        assert_eq!(
            complete[2].1,
            InputBucket {
                bucket_ms: 1000,
                ..Default::default()
            }
        );
    }
    #[test]
    fn movement_is_aggregate_distance_and_late_input_is_explicit() {
        let mut b = Buckets::new(0);
        b.push(input(5, 1, 0., 0.));
        b.push(input(5, 2, 3., 4.));
        assert_eq!(b.complete(SECOND)[0].1.mouse_px, 5);
        b.push(input(10, 1, 0., 0.));
        assert_eq!(b.dropped, 1);
    }
    #[test]
    fn wake_discards_old_pending_buckets_with_a_counter() {
        let mut b = Buckets::new(0);
        b.push(input(10, 1, 0., 0.));
        b.reset(100 * SECOND);
        assert_eq!(b.dropped, 1);
        assert_eq!(b.complete(101 * SECOND).len(), 1);
    }
}
