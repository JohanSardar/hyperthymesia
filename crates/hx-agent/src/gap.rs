use crate::envelope::Producer;
use hx_schema::{kinds::*, *};
/// Suppression is extended only through observed time, never into an assumed future.
#[derive(Default)]
pub struct PolicyGap {
    open: Option<(String, ObservationTime, u64)>,
}
impl PolicyGap {
    pub fn observe(
        &mut self,
        excluded: bool,
        at: ObservationTime,
        p: &mut Producer,
    ) -> Option<Envelope> {
        if excluded && self.open.is_none() {
            self.open = Some((
                format!("policy-gap:{}", ulid::Ulid::generate()),
                at.clone(),
                0,
            ));
        }
        let (key, start, revision) = self.open.as_mut()?;
        let mut e = p.core(
            key.clone(),
            revision.to_string(),
            Operation::Upsert,
            CorePayload::CollectorHealth(CollectorHealth {
                collector: "focus".into(),
                status: CollectorStatus::Paused,
                last_sequence: None,
                last_observed_at: None,
                queue_age_ms: None,
                dropped: 0,
                detail: None,
            }),
        );
        *revision += 1;
        e.kind = Kind::new("policy.gap");
        e.payload = serde_json::json!({"reason":"policy_exclusion","open":excluded});
        e.time = start.clone();
        e.time.source_end = at
            .source_start
            .map(|end| start.source_start.map_or(end, |begin| end.max(begin)));
        e.time.mono_end = at.mono_start.map(|mut end| {
            if let Some(begin) = &start.mono_start {
                end.nanos = end.nanos.max(begin.nanos);
            }
            end
        });
        e.time.observed_at = at.observed_at;
        e.coverage = Some(Coverage {
            scope: "policy_exclusion".into(),
            completeness: Completeness::Complete,
        });
        if !excluded {
            self.open = None;
        }
        Some(e)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gap_revisions_cover_the_exact_observed_exclusion_interval_without_content() {
        let mut p = Producer::new("test".into()).unwrap();
        let mut gap = PolicyGap::default();
        let start = p.time();
        let first = gap.observe(true, start.clone(), &mut p).unwrap();
        let mut later = start.clone();
        later.source_start = later.source_start.map(|t| t + chrono::Duration::seconds(5));
        later.mono_start.as_mut().unwrap().nanos += 5_000_000_000;
        let update = gap.observe(true, later.clone(), &mut p).unwrap();
        let closed = gap.observe(false, later, &mut p).unwrap();
        assert_eq!(first.kind.as_str(), "policy.gap");
        assert_eq!(first.source_key, closed.source_key);
        assert_eq!(first.revision.as_str(), "0");
        assert_eq!(update.revision.as_str(), "1");
        assert_eq!(closed.revision.as_str(), "2");
        assert_eq!(closed.time.duration().unwrap().nanos, 5_000_000_000);
        assert_eq!(
            closed.payload,
            serde_json::json!({"reason":"policy_exclusion","open":false})
        );
        closed.validate().unwrap();
        assert!(gap.observe(false, p.time(), &mut p).is_none());
    }
    #[test]
    fn allowed_focus_does_not_fabricate_a_gap() {
        let mut p = Producer::new("test".into()).unwrap();
        assert!(
            PolicyGap::default()
                .observe(false, p.time(), &mut p)
                .is_none()
        );
    }
    #[test]
    fn emitted_gap_suppresses_the_excluded_interval_in_the_real_compiler() {
        let mut p = Producer::new("test".into()).unwrap();
        let start = p.time();
        let utc = start.source_start.unwrap();
        let mut focus = p.core(
            "focus:test",
            "1",
            Operation::Upsert,
            CorePayload::FocusSpan(FocusSpan {
                app_bundle_id: "synthetic.app".into(),
                app_name: "Synthetic".into(),
                window_title: None,
                pid: None,
                display: None,
            }),
        );
        focus.time = start.clone();
        focus.time.source_end = Some(utc + chrono::Duration::seconds(20));
        focus.time.mono_end = focus.time.mono_start.clone().map(|mut stamp| {
            stamp.nanos += 20_000_000_000;
            stamp
        });
        let mut gap = PolicyGap::default();
        let mut begin = start.clone();
        begin.source_start = Some(utc + chrono::Duration::seconds(5));
        begin.mono_start.as_mut().unwrap().nanos += 5_000_000_000;
        let opened = gap.observe(true, begin.clone(), &mut p).unwrap();
        let mut end = begin;
        end.source_start = Some(utc + chrono::Duration::seconds(10));
        end.mono_start.as_mut().unwrap().nanos += 5_000_000_000;
        let closed = gap.observe(false, end, &mut p).unwrap();
        let observations: Vec<_> = [focus, opened, closed]
            .into_iter()
            .enumerate()
            .map(|(id, envelope)| hx_compile::Observation {
                id: id as i64 + 1,
                received_at: envelope.time.observed_at,
                envelope,
            })
            .collect();
        let timeline = hx_compile::compile(
            &observations,
            Interval::new(utc, utc + chrono::Duration::seconds(20)).unwrap(),
        );
        assert!(timeline.coverage.iter().any(|coverage| {
            coverage
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("[focus]"))
                && coverage.state == hx_compile::CoverageState::Suppressed
                && coverage.interval
                    == Interval::new(
                        utc + chrono::Duration::seconds(5),
                        utc + chrono::Duration::seconds(10),
                    )
                    .unwrap()
        }));
    }
}
