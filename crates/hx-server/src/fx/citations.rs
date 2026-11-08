//! Digest repair excerpts and time support for citation admission.
use hx_schema::Interval;

/// The digest's header plus every line and unknown stretch within ten minutes of
/// `around`. Lines print `[ids] hh:mm:ss-hh:mm:ss`; following unbracketed lines
/// (screen text) belong to the line above.
pub(super) fn digest_excerpt(digest: &str, window: Interval, around: &[Interval]) -> String {
    let margin = chrono::Duration::minutes(10);
    let near = |line: &str| {
        let Some(times) = line
            .split_once("] ")
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .and_then(|t| t.split_once('-'))
        else {
            return false;
        };
        let at = |t: &str| {
            chrono::NaiveTime::parse_from_str(t, "%H:%M:%S")
                .ok()
                .map(|t| window.start.date_naive().and_time(t).and_utc())
        };
        let (Some(start), Some(mut end)) = (at(times.0), at(times.1)) else {
            return false;
        };
        if end < start {
            end += chrono::Duration::days(1);
        }
        around
            .iter()
            .any(|i| start < i.end + margin && i.start - margin < end)
    };
    let mut out = String::new();
    let mut lines = digest.lines();
    for line in lines.by_ref() {
        out.push_str(line);
        out.push('\n');
        if line.is_empty() {
            break;
        }
    }
    let mut keep = false;
    for line in lines {
        if line.starts_with('[') {
            keep = near(line);
        } else if line.starts_with("Unknown stretches") {
            out.push_str(line);
            out.push('\n');
            keep = false;
            continue;
        }
        if keep {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Time support per cited record, clipped to the window. A record supports both its
/// compiled interval and its own declared wall interval: compiled ends derive from the
/// monotonic clock, so they can differ by milliseconds from the source end that tools
/// show the model, and exact boundary matching would reject faithful citations.
pub(super) fn citation_support(
    timeline: &hx_compile::Timeline,
    observations: &[hx_compile::Observation],
    window: Interval,
) -> std::collections::BTreeMap<i64, Vec<Interval>> {
    let clip = |s: Interval| {
        if s.start == s.end {
            window.contains(s.start).then_some(s)
        } else {
            s.intersect(&window)
        }
    };
    let long_sources: std::collections::BTreeMap<_, _> = observations
        .iter()
        .filter_map(|o| {
            let start = o
                .envelope
                .time
                .source_start
                .unwrap_or(o.envelope.time.observed_at);
            let end = o.envelope.time.source_end.unwrap_or(start);
            (o.envelope
                .connector_instance
                .as_str()
                .starts_with("source:")
                && end - start > chrono::Duration::hours(4))
            .then_some((o.id, start))
        })
        .collect();
    let mut by_evidence = std::collections::BTreeMap::<i64, Vec<Interval>>::new();
    let mut add = |id: i64, s: Interval| {
        // Apply the same bound to compiled and raw support so neither path
        // can restore a long sender-controlled span (or clamp an old start).
        let s = long_sources.get(&id).map_or(s, |start| Interval {
            start: *start,
            end: *start,
        });
        if let Some(s) = clip(s) {
            let entry = by_evidence.entry(id).or_default();
            if !entry.contains(&s) {
                entry.push(s);
            }
        }
    };
    for span in timeline.lanes.iter().flat_map(|lane| &lane.intervals) {
        for id in &span.evidence {
            add(*id, span.interval);
        }
    }
    for point in &timeline.points {
        add(
            point.evidence,
            Interval {
                start: point.at,
                end: point.at,
            },
        );
    }
    for o in observations {
        // Match the broker's EvidenceItem timestamp, including sources with no
        // declared time. An omitted map entry must not borrow another citation's time.
        let at = o
            .envelope
            .time
            .source_start
            .unwrap_or(o.envelope.time.observed_at);
        add(o.id, Interval { start: at, end: at });
        if let (Some(start), Some(end)) = (o.envelope.time.source_start, o.envelope.time.source_end)
            && start <= end
        {
            add(o.id, Interval { start, end });
        }
    }
    by_evidence
}
