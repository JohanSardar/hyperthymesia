//! A compact, deterministic digest of one window for a single-call interpreter.
//!
//! Consecutive focus records of the same app become one segment, sub-second
//! flickers are folded in as counted switches, screen text is reduced to lines
//! not already shown earlier in the window, and every line names the evidence
//! ids that support it. Each segment's interval is registered as support for
//! all of its member records, so citing a segment never fails because a
//! 100 ms flicker inside it was left out.

use chrono::{DateTime, Duration, Utc};
use hx_compile::{Observation, Timeline};
use hx_schema::Interval;
use std::collections::{BTreeMap, BTreeSet};

/// Tunables in one place; defaults target a busy 4-hour window in ~150k chars.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct DigestConfig {
    /// A different app shown for less than this is a flicker, not a segment.
    pub blip_ms: i64,
    /// A same-app run splits when focus records are this far apart.
    pub split_gap_s: i64,
    /// Same-app runs also split at this length so long stretches stay readable.
    pub max_segment_s: i64,
    /// New screen text kept per segment.
    pub text_chars: usize,
    /// Distinct titles listed per segment.
    pub titles: usize,
    /// Unknown attention shorter than this is not listed as a gap.
    pub min_gap_s: i64,
}

impl Default for DigestConfig {
    fn default() -> Self {
        Self {
            blip_ms: 3_000,
            split_gap_s: 60,
            max_segment_s: 20 * 60,
            text_chars: 1_200,
            titles: 8,
            min_gap_s: 60,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Segment {
    pub interval: Interval,
    pub app: String,
    /// Every focus record merged into this segment, including flickers.
    pub evidence: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Digest {
    pub text: String,
    pub segments: Vec<Segment>,
    /// Each line's interval as printed (whole seconds) with the ids it lists.
    pub lines: Vec<(Interval, Vec<i64>)>,
}

impl Digest {
    /// Segment intervals keyed by each member record, for citation validation.
    pub fn support(&self) -> BTreeMap<i64, Vec<Interval>> {
        let mut out = BTreeMap::<i64, Vec<Interval>>::new();
        let spans = self
            .segments
            .iter()
            .map(|s| (s.interval, &s.evidence))
            .chain(self.lines.iter().map(|(i, ids)| (*i, ids)));
        for (interval, ids) in spans {
            for id in ids {
                let entry = out.entry(*id).or_default();
                if !entry.contains(&interval) {
                    entry.push(interval);
                }
            }
        }
        out
    }
}

struct Focus {
    interval: Interval,
    app: String,
    title: Option<String>,
    evidence: Vec<i64>,
}

pub fn build_digest(
    timeline: &Timeline,
    observations: &[Observation],
    config: DigestConfig,
) -> Digest {
    let window = timeline.window;
    let focus = focus_records(timeline);
    let segments = segment(&focus, config);
    let attention = attention_runs(timeline);
    let mut seen_lines = BTreeSet::new();
    let mut lines = Vec::new();
    let mut out = String::new();
    out.push_str(&format!(
        "window {} to {} (UTC). One line per stretch of foreground activity.\n\
         Format: [evidence ids] start-end app | titles | attention | input | urls, then new screen text.\n\
         Cite any listed id; the line's whole interval is supported by each of its ids.\n",
        window.start.to_rfc3339(),
        window.end.to_rfc3339()
    ));
    out.push_str(&totals(&segments, &attention, window));
    let mut gaps = attention
        .iter()
        .filter(|(i, label, _)| {
            label == "unknown" && (i.end - i.start).num_seconds() >= config.min_gap_s
        })
        .peekable();
    for s in &segments {
        while let Some((gap, _, ids)) = gaps.peek().filter(|(g, _, _)| g.start <= s.interval.start)
        {
            lines.push((shown(*gap), ids.clone()));
            out.push_str(&format!(
                "[{}] {}-{} GAP unknown coverage (capture not verified); never treat as idle or away\n",
                ids_text(ids),
                hm(gap.start),
                hm(gap.end)
            ));
            gaps.next();
        }
        lines.push((shown(s.interval), s.evidence.clone()));
        out.push_str(&segment_line(
            s,
            &focus,
            &attention,
            observations,
            config,
            &mut seen_lines,
        ));
    }
    for (gap, _, ids) in gaps {
        lines.push((shown(*gap), ids.clone()));
        out.push_str(&format!(
            "[{}] {}-{} GAP unknown coverage (capture not verified); never treat as idle or away\n",
            ids_text(ids),
            hm(gap.start),
            hm(gap.end)
        ));
    }
    Digest {
        text: out,
        segments,
        lines,
    }
}

fn focus_records(timeline: &Timeline) -> Vec<Focus> {
    let mut out: Vec<Focus> = timeline
        .lanes
        .iter()
        .filter(|l| l.id.kind == "focus")
        .flat_map(|l| &l.intervals)
        .map(|i| Focus {
            interval: i.interval,
            app: i.attrs["app_name"]
                .as_str()
                .or(i.attrs["app_bundle_id"].as_str())
                .unwrap_or("unknown app")
                .to_owned(),
            title: i.attrs["window_title"]
                .as_str()
                .map(clean_title)
                .filter(|t| !t.is_empty()),
            evidence: i.evidence.clone(),
        })
        .collect();
    out.sort_by_key(|f| (f.interval.start, f.interval.end));
    out
}

/// Window titles carry browser chrome ("- Google Chrome - Profile", memory and
/// audio badges) that changes without the page changing.
pub fn clean_title(title: &str) -> String {
    let mut t = title.to_owned();
    for marker in [
        " - Google Chrome",
        " - Safari",
        " - Arc",
        " — Mozilla Firefox",
    ] {
        if let Some(i) = t.find(marker) {
            t.truncate(i);
        }
    }
    for badge in [
        " - Audio playing",
        " - High memory usage",
        " - Memory usage",
    ] {
        if let Some(i) = t.find(badge) {
            let rest = &t[i + badge.len()..];
            // Drop the badge and any " - 803 MB" figure that follows it.
            let tail = rest
                .strip_prefix(" - ")
                .and_then(|r| r.find(" - ").map(|j| r[j..].to_owned()))
                .unwrap_or_default();
            t = format!("{}{}", &t[..i], tail);
        }
    }
    t.trim().to_owned()
}

fn segment(focus: &[Focus], config: DigestConfig) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let mut i = 0;
    while i < focus.len() {
        let f = &focus[i];
        let blip = (f.interval.end - f.interval.start).num_milliseconds() < config.blip_ms;
        if let Some(last) = out.last_mut() {
            let gap = (f.interval.start - last.interval.end).num_seconds();
            let long = (f.interval.end - last.interval.start).num_seconds() > config.max_segment_s;
            let same = f.app == last.app;
            // A flicker to another app folds in when focus returns to this app.
            let returns = blip
                && focus[i + 1..]
                    .iter()
                    .find(|n| {
                        (n.interval.end - n.interval.start).num_milliseconds() >= config.blip_ms
                    })
                    .is_some_and(|n| n.app == last.app);
            if gap <= config.split_gap_s && !long && (same || returns || (blip && gap <= 1)) {
                last.interval.end = last.interval.end.max(f.interval.end);
                last.evidence.extend(&f.evidence);
                i += 1;
                continue;
            }
        }
        out.push(Segment {
            interval: f.interval,
            app: f.app.clone(),
            evidence: f.evidence.clone(),
        });
        i += 1;
    }
    for s in &mut out {
        s.evidence.sort_unstable();
        s.evidence.dedup();
    }
    out
}

/// Adjacent attention intervals with the same label, merged.
fn attention_runs(timeline: &Timeline) -> Vec<(Interval, String, Vec<i64>)> {
    let mut runs: Vec<(Interval, String, Vec<i64>)> = Vec::new();
    let mut items: Vec<_> = timeline
        .lanes
        .iter()
        .filter(|l| l.id.kind == "attention")
        .flat_map(|l| &l.intervals)
        .collect();
    items.sort_by_key(|i| i.interval.start);
    for i in items {
        match runs.last_mut() {
            Some((iv, label, ids)) if *label == i.label && i.interval.start <= iv.end => {
                iv.end = iv.end.max(i.interval.end);
                ids.extend(&i.evidence);
            }
            _ => runs.push((i.interval, i.label.clone(), i.evidence.clone())),
        }
    }
    for (_, _, ids) in &mut runs {
        ids.sort_unstable();
        ids.dedup();
    }
    runs
}

fn totals(
    segments: &[Segment],
    attention: &[(Interval, String, Vec<i64>)],
    window: Interval,
) -> String {
    let mut apps = BTreeMap::<&str, i64>::new();
    for s in segments {
        *apps.entry(&s.app).or_default() += (s.interval.end - s.interval.start).num_seconds();
    }
    let mut apps: Vec<_> = apps.into_iter().collect();
    apps.sort_by_key(|(_, secs)| -secs);
    let mut att = BTreeMap::<&str, i64>::new();
    for (i, label, _) in attention {
        if let Some(i) = i.intersect(&window) {
            *att.entry(label).or_default() += (i.end - i.start).num_seconds();
        }
    }
    format!(
        "totals: apps {} | attention {}\n\n",
        apps.iter()
            .map(|(a, s)| format!("{a} {}", mins(*s)))
            .collect::<Vec<_>>()
            .join(", "),
        att.iter()
            .map(|(a, s)| format!("{a} {}", mins(*s)))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn segment_line(
    s: &Segment,
    focus: &[Focus],
    attention: &[(Interval, String, Vec<i64>)],
    observations: &[Observation],
    config: DigestConfig,
    seen_lines: &mut BTreeSet<String>,
) -> String {
    let members: BTreeSet<i64> = s.evidence.iter().copied().collect();
    let parts: Vec<&Focus> = focus
        .iter()
        .filter(|f| f.evidence.iter().any(|id| members.contains(id)))
        .collect();
    let mut titles: Vec<(String, DateTime<Utc>)> = Vec::new();
    let mut flickers = BTreeMap::<&str, usize>::new();
    for f in &parts {
        if f.app != s.app {
            *flickers.entry(&f.app).or_default() += 1;
            continue;
        }
        if let Some(t) = &f.title
            && !titles.iter().any(|(x, _)| x == t)
        {
            titles.push((t.clone(), f.interval.start));
        }
    }
    let shown: Vec<String> = titles
        .iter()
        .take(config.titles)
        .map(|(t, at)| format!("{} \"{}\"", hm(*at), bounded(t, 90)))
        .collect();
    let mut title_text = shown.join("; ");
    if titles.len() > config.titles {
        title_text.push_str(&format!("; +{} more", titles.len() - config.titles));
    }
    let mut att = BTreeMap::<&str, i64>::new();
    for (i, label, _) in attention {
        if let Some(o) = i.intersect(&s.interval) {
            *att.entry(label).or_default() += (o.end - o.start).num_seconds();
        }
    }
    let (mut keys, mut clicks, mut scrolls) = (0i64, 0i64, 0i64);
    let mut urls: Vec<String> = Vec::new();
    // Lines keyed by their opening so a draft growing across snapshots keeps
    // only its latest version, in order of first appearance.
    let mut text: Vec<(String, i64, String)> = Vec::new();
    for o in observations {
        let Some(at) = o.envelope.time.source_start else {
            continue;
        };
        if at < s.interval.start || at >= s.interval.end {
            continue;
        }
        let p = &o.envelope.payload;
        match o.envelope.kind.as_str() {
            "input.bucket" => {
                keys += p["keys"].as_i64().unwrap_or(0);
                clicks += p["clicks"].as_i64().unwrap_or(0);
                scrolls += p["scroll_events"].as_i64().unwrap_or(0);
            }
            "text.snapshot" => {
                let body = p["text"].as_str().unwrap_or_default();
                // Snapshots start with their window title. One taken during a flicker
                // to another app belongs to that app, not to this segment.
                let first = body
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .unwrap_or_default();
                let cleaned = clean_title(first);
                if !(titles.iter().any(|(t, _)| *t == cleaned) || first.contains(s.app.as_str())) {
                    continue;
                }
                for url in addresses(body) {
                    if !urls.contains(&url) {
                        urls.push(url);
                    }
                }
                for line in body.lines().map(str::trim) {
                    if line.chars().count() < 12
                        || line.starts_with("Address and search bar")
                        || !seen_lines.insert(line.to_owned())
                    {
                        continue;
                    }
                    let key: String = line.chars().take(48).collect();
                    if let Some(entry) = text.iter_mut().find(|(k, _, _)| *k == key) {
                        entry.1 = o.id;
                        entry.2 = line.to_owned();
                    } else if text.iter().map(|(_, _, l)| l.len()).sum::<usize>()
                        < config.text_chars
                    {
                        text.push((key, o.id, line.to_owned()));
                    }
                }
            }
            _ => {}
        }
    }
    let mut line = format!(
        "[{}] {}-{} {}",
        ids_text(&s.evidence),
        hm(s.interval.start),
        hm(s.interval.end),
        s.app
    );
    if !title_text.is_empty() {
        line.push_str(&format!(" | {title_text}"));
    }
    if !flickers.is_empty() {
        line.push_str(&format!(
            " | brief switches: {}",
            flickers
                .iter()
                .map(|(a, n)| format!("{a} x{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !att.is_empty() {
        line.push_str(&format!(
            " | {}",
            att.iter()
                .filter(|(_, secs)| **secs > 0)
                .map(|(a, secs)| format!("{a} {}", mins(*secs)))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    line.push_str(&format!(" | keys {keys} clicks {clicks} scrolls {scrolls}"));
    if !urls.is_empty() {
        line.push_str(&format!(
            " | urls {}",
            urls.iter()
                .take(6)
                .map(|u| bounded(u, 120))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    line.push('\n');
    for (_, id, l) in &text {
        line.push_str(&format!("[{id}] {}\n", bounded(l, 240)));
    }
    line
}

/// URLs Chrome exposes after its "Address and search bar" label.
fn addresses(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    while let Some(line) = lines.next() {
        let rest = line.strip_prefix("Address and search bar").map(str::trim);
        let candidate = match rest {
            Some("") => lines.next().map(str::to_owned),
            Some(r) => Some(r.to_owned()),
            None => None,
        };
        if let Some(c) = candidate
            && c.contains('.')
            && !c.contains(' ')
            && !out.contains(&c)
        {
            out.push(c);
        }
    }
    out
}

fn ids_text(ids: &[i64]) -> String {
    match ids {
        [] => String::new(),
        [one] => one.to_string(),
        [first, .., last] if ids.len() > 4 => {
            format!("{first},{},…,{last} ({} records)", ids[1], ids.len())
        }
        _ => ids.iter().map(i64::to_string).collect::<Vec<_>>().join(","),
    }
}

/// An interval truncated to the whole seconds the digest prints.
fn shown(i: Interval) -> Interval {
    let floor = |t: DateTime<Utc>| t - Duration::nanoseconds(i64::from(t.timestamp_subsec_nanos()));
    Interval {
        start: floor(i.start),
        end: floor(i.end),
    }
}

fn hm(t: DateTime<Utc>) -> String {
    t.format("%H:%M:%S").to_string()
}

fn mins(secs: i64) -> String {
    let d = Duration::seconds(secs);
    if d.num_minutes() > 0 {
        format!("{}m{:02}s", d.num_minutes(), secs % 60)
    } else {
        format!("{secs}s")
    }
}

fn bounded(text: &str, cap: usize) -> String {
    if text.chars().count() <= cap {
        text.to_owned()
    } else {
        text.chars()
            .take(cap - 1)
            .chain(std::iter::once('…'))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_drop_browser_chrome_and_badges() {
        assert_eq!(
            clean_title("X - High memory usage - 803 MB - Google Chrome - Johan (Person 1)"),
            "X"
        );
        assert_eq!(
            clean_title("Elections / X - Audio playing - Google Chrome - P"),
            "Elections / X"
        );
        assert_eq!(clean_title("T3 Code (Nightly)"), "T3 Code (Nightly)");
    }

    #[test]
    fn address_bar_urls_are_extracted() {
        assert_eq!(
            addresses("Back\nAddress and search bar\nx.com/search?q=5pm\nReload"),
            vec!["x.com/search?q=5pm".to_owned()]
        );
        assert_eq!(
            addresses("Address and search bar x.com/home"),
            vec!["x.com/home".to_owned()]
        );
    }
}
