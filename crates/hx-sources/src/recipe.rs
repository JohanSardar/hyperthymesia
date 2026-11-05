//! Recipe wire model and bounded mapping of already-scrubbed source responses.

use chrono::{DateTime, Duration, Utc};
use hx_schema::{
    ConnectorInstance, ENVELOPE_VERSION, Envelope, Kind, ObservationId, ObservationTime, Operation,
    Revision, Sensitivity, SourceKey,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::request::Request;

pub const MAX_ITEM_BYTES: usize = 64 * 1024;
pub const MAX_TITLE_BYTES: usize = 1024;
pub const MAX_PAGES_PER_PULL: usize = 20;
pub const MAX_ITEMS_PER_PULL: usize = 1_000;

/// The `recipe_save` / `recipe_test` argument shape. Persistence owns the recipe ID.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub source: String,
    pub name: String,
    pub request: Request,
    pub items: String,
    pub kind: String,
    pub map: FieldMap,
    pub time_format: TimeFormat,
    #[serde(default)]
    pub cursor: Option<Cursor>,
    #[serde(default)]
    pub pages: Option<Pages>,
    #[serde(default)]
    pub ignore: Vec<String>,
    #[serde(default)]
    pub reconnect_exit_codes: Vec<i32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FieldMap {
    pub id: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeFormat {
    Rfc3339,
    UnixS,
    UnixMs,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cursor {
    pub from: CursorFrom,
    pub pointer: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorFrom {
    Item,
    Response,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pages {
    pub next: String,
    pub max: usize,
}

/// Owner settings, not recipe-controlled. The connection-time history floor
/// applies to every page of every pull, including delayed executions.
#[derive(Debug, Clone, Copy)]
pub struct MapContext<'a> {
    pub recipe_id: &'a str,
    pub source_id: &'a str,
    pub connected_at: DateTime<Utc>,
    pub history_days: u8,
    pub pulled_at: DateTime<Utc>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PullCounts {
    pub pages: usize,
    pub items_seen: usize,
    pub mapped: usize,
    pub bad_time: usize,
    pub too_old: usize,
    pub too_future: usize,
    pub long_span: usize,
    pub too_large: usize,
}

#[derive(Debug, Default)]
pub struct MappedPage {
    pub envelopes: Vec<Envelope>,
    /// Cursor after the last consumed item, including deliberately skipped items.
    /// A response cursor is returned only after consuming the complete page.
    pub cursor: Option<Value>,
    /// Value at `pages.next`; the pull runner substitutes it into the next request.
    /// Absent once this pull's page or item budget is exhausted.
    pub next_page: Option<Value>,
    pub unprocessed_items: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RecipeError {
    #[error("recipe kind must be a non-core source.* kind")]
    Kind,
    #[error("invalid recipe setting: {0}")]
    Setting(&'static str),
    #[error("items pointer must select an array")]
    Items,
    #[error("pull limit reached")]
    PullLimit,
}

impl Recipe {
    pub fn validate(&self) -> Result<(), RecipeError> {
        if !self.kind.starts_with("source.")
            || !Kind::is_valid(&self.kind)
            || hx_schema::kinds::is_core(&self.kind)
        {
            return Err(RecipeError::Kind);
        }
        if self.source.is_empty() || self.name.trim().is_empty() {
            return Err(RecipeError::Setting("source and name must not be empty"));
        }
        let pointers = std::iter::once(self.items.as_str())
            .chain(self.map.id.as_deref())
            .chain(self.map.start.as_deref())
            .chain(self.map.end.as_deref())
            .chain(self.map.title.as_deref())
            .chain(self.cursor.as_ref().map(|c| c.pointer.as_str()))
            .chain(self.pages.as_ref().map(|p| p.next.as_str()))
            .chain(self.ignore.iter().map(String::as_str));
        if self.ignore.iter().any(String::is_empty) {
            return Err(RecipeError::Setting("ignore must not select the root"));
        }
        if !pointers.into_iter().all(valid_pointer) {
            return Err(RecipeError::Setting("invalid JSON pointer"));
        }
        if let Some(pages) = &self.pages
            && !(1..=MAX_PAGES_PER_PULL).contains(&pages.max)
        {
            return Err(RecipeError::Setting("pages.max must be 1..=20"));
        }
        Ok(())
    }
}

/// Reuse one mapper for all pages in a pull; creating one per page resets limits.
pub struct PullMapper<'a> {
    recipe: &'a Recipe,
    context: MapContext<'a>,
    counts: PullCounts,
    cutoff: DateTime<Utc>,
}

impl<'a> PullMapper<'a> {
    pub fn new(recipe: &'a Recipe, context: MapContext<'a>) -> Result<Self, RecipeError> {
        recipe.validate()?;
        if context.recipe_id.is_empty()
            || context.source_id.is_empty()
            || context.recipe_id.contains('\0')
            || context.source_id.contains('\0')
            || context.history_days > 30
        {
            return Err(RecipeError::Setting(
                "recipe and source IDs required without NUL; history_days must be 0..=30",
            ));
        }
        let cutoff = context
            .connected_at
            .checked_sub_signed(Duration::days(context.history_days.into()))
            .ok_or(RecipeError::Setting("history cutoff is out of range"))?;
        Ok(Self {
            recipe,
            context,
            counts: PullCounts::default(),
            cutoff,
        })
    }

    pub fn counts(&self) -> PullCounts {
        self.counts
    }

    /// Caller must scrub the whole response with all source secrets before mapping.
    /// This module never receives or reads a source key.
    pub fn map_page(&mut self, response: &Value) -> Result<MappedPage, RecipeError> {
        let page_limit = self.recipe.pages.as_ref().map_or(1, |p| p.max);
        if self.counts.pages >= page_limit || self.counts.items_seen >= MAX_ITEMS_PER_PULL {
            return Err(RecipeError::PullLimit);
        }
        // PostgreSQL JSONB refuses NUL in both values and object keys. Normalize
        // before looking up fields so identity, hashes, cursors and payload agree.
        let response = without_nul(response);
        let items = response
            .pointer(&self.recipe.items)
            .and_then(Value::as_array)
            .ok_or(RecipeError::Items)?;
        self.counts.pages += 1;
        let consumed = items.len().min(MAX_ITEMS_PER_PULL - self.counts.items_seen);
        let mut page = MappedPage {
            unprocessed_items: consumed < items.len(),
            ..MappedPage::default()
        };
        for item in &items[..consumed] {
            self.counts.items_seen += 1;
            if let Some(cursor) = &self.recipe.cursor
                && cursor.from == CursorFrom::Item
                && let Some(value) = item.pointer(&cursor.pointer).filter(|v| !v.is_null())
            {
                page.cursor = Some(value.clone());
            }
            if serde_json::to_vec(item)
                .expect("JSON value serializes")
                .len()
                > MAX_ITEM_BYTES
            {
                self.counts.too_large += 1;
                continue;
            }
            let Some(mut time) = self.item_time(item) else {
                self.counts.bad_time += 1;
                continue;
            };
            let start = time
                .source_start
                .expect("item_time always sets source_start");
            if start < self.cutoff {
                self.counts.too_old += 1;
                continue;
            }
            if start.signed_duration_since(self.context.pulled_at) > Duration::days(1) {
                self.counts.too_future += 1;
                continue;
            }
            if time
                .source_end
                .is_some_and(|end| end - start > Duration::hours(24))
            {
                time.source_end = None;
                self.counts.long_span += 1;
            }
            let content = without_ignored(item, "", &self.recipe.ignore);
            let revision =
                Revision::digest(&serde_json::to_vec(&content).expect("JSON value serializes"));
            let native_id = self
                .recipe
                .map
                .id
                .as_ref()
                .and_then(|pointer| item.pointer(pointer))
                .and_then(item_id);
            let source_key = SourceKey::new(format!(
                "{}:{}",
                self.context.recipe_id,
                native_id.as_deref().unwrap_or(revision.as_str())
            ));
            let title = self
                .recipe
                .map
                .title
                .as_ref()
                .and_then(|p| item.pointer(p))
                .and_then(Value::as_str)
                .map(|title| &title[..title.floor_char_boundary(MAX_TITLE_BYTES)]);
            page.envelopes.push(Envelope {
                envelope_version: ENVELOPE_VERSION,
                observation_id: ObservationId::generate(),
                connector_instance: ConnectorInstance::new(format!(
                    "source:{}",
                    self.context.source_id
                )),
                source_key,
                revision,
                operation: Operation::Upsert,
                sequence: None,
                kind: Kind::new(&self.recipe.kind),
                schema_version: 1,
                time,
                device: None,
                actor: None,
                subject: None,
                resource: None,
                links: Vec::new(),
                coverage: None,
                sensitivity: Sensitivity::Restricted,
                // Keep the complete scrubbed item, including ignored fields. Expose
                // the mapped title for source normalization without overwriting data.
                payload: json!({"title":title, "item":item}),
                blobs: Vec::new(),
            });
            self.counts.mapped += 1;
        }
        if !page.unprocessed_items
            && let Some(cursor) = &self.recipe.cursor
            && cursor.from == CursorFrom::Response
        {
            page.cursor = response
                .pointer(&cursor.pointer)
                .filter(|v| !v.is_null())
                .cloned();
        }
        if self.counts.pages < page_limit && self.counts.items_seen < MAX_ITEMS_PER_PULL {
            page.next_page = self
                .recipe
                .pages
                .as_ref()
                .and_then(|p| response.pointer(&p.next))
                .filter(|v| !v.is_null() && v.as_str() != Some("") && **v != Value::Bool(false))
                .cloned();
        }
        Ok(page)
    }

    fn item_time(&self, item: &Value) -> Option<ObservationTime> {
        let start = match &self.recipe.map.start {
            Some(pointer) => parse_time(item.pointer(pointer)?, self.recipe.time_format)?,
            None => self.context.pulled_at,
        };
        let end = match &self.recipe.map.end {
            Some(pointer) => Some(parse_time(item.pointer(pointer)?, self.recipe.time_format)?),
            None => None,
        };
        if end.is_some_and(|end| end < start) {
            return None;
        }
        let mut time = ObservationTime::point(start);
        time.source_end = end;
        time.observed_at = self.context.pulled_at;
        time.resolution_ms =
            if self.recipe.map.start.is_some() && self.recipe.time_format == TimeFormat::UnixS {
                1000
            } else {
                1
            };
        Some(time)
    }
}

/// Strip NUL recursively, including keys, before producing database-bound strings.
fn without_nul(value: &Value) -> Value {
    match value {
        Value::String(s) => Value::String(s.replace('\0', "")),
        Value::Array(values) => Value::Array(values.iter().map(without_nul).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.replace('\0', ""), without_nul(value)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn item_id(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn parse_time(value: &Value, format: TimeFormat) -> Option<DateTime<Utc>> {
    if format == TimeFormat::Rfc3339 {
        return DateTime::parse_from_rfc3339(value.as_str()?)
            .ok()
            .map(|t| t.with_timezone(&Utc));
    }
    let number = value.as_i64().or_else(|| value.as_str()?.parse().ok())?;
    match format {
        TimeFormat::UnixS => DateTime::from_timestamp(number, 0),
        TimeFormat::UnixMs => DateTime::from_timestamp_millis(number),
        TimeFormat::Rfc3339 => unreachable!(),
    }
}

fn valid_pointer(pointer: &str) -> bool {
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return false;
    }
    let mut chars = pointer.chars();
    while let Some(c) = chars.next() {
        if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
            return false;
        }
    }
    true
}

/// Remove all ignored paths against original array indices (never shift indices
/// while applying later pointers), and canonicalize object order for hashing.
fn without_ignored(value: &Value, path: &str, ignore: &[String]) -> Value {
    if ignore.iter().any(|p| p == path) {
        return Value::Null;
    }
    let child_path = |key: &str| format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                let path = child_path(key);
                if !ignore.contains(&path) {
                    out.insert(key.clone(), without_ignored(value, &path, ignore));
                }
            }
            out.sort_keys();
            Value::Object(out)
        }
        Value::Array(values) => Value::Array(
            values
                .iter()
                .enumerate()
                .filter_map(|(index, value)| {
                    let path = child_path(&index.to_string());
                    (!ignore.contains(&path)).then(|| without_ignored(value, &path, ignore))
                })
                .collect(),
        ),
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use hx_schema::{Operation, Sensitivity};
    use serde_json::json;

    fn recipe() -> Recipe {
        serde_json::from_value(json!({
            "source":"01source", "name":"mail", "request":{
                "kind":"http", "method":"GET", "url":"http://127.0.0.1/messages",
                "headers":{}, "body":null
            },
            "items":"/messages", "kind":"source.email",
            "map":{"id":"/id", "start":"/sent_at", "end":null, "title":"/subject"},
            "time_format":"rfc3339", "cursor":null, "pages":null,
            "ignore":[], "reconnect_exit_codes":[2]
        }))
        .unwrap()
    }

    fn context() -> MapContext<'static> {
        let connected_at = "2026-11-05T12:00:00Z".parse().unwrap();
        MapContext {
            recipe_id: "recipe-1",
            source_id: "01source",
            connected_at,
            history_days: 1,
            pulled_at: connected_at + Duration::hours(4),
        }
    }

    fn item() -> Value {
        json!({"id":"message-1", "sent_at":"2026-11-05T08:00:00Z", "subject":"Orchids", "unread":true})
    }

    fn mapped(recipe: &Recipe, item: Value) -> Envelope {
        let page = PullMapper::new(recipe, context())
            .unwrap()
            .map_page(&json!({"messages":[item]}))
            .unwrap();
        assert_eq!(
            page.envelopes.len(),
            1,
            "valid item must become an envelope"
        );
        page.envelopes.into_iter().next().unwrap()
    }

    #[test]
    fn same_item_has_same_key_revision_and_restricted_provenance() {
        let recipe = recipe();
        let a = mapped(&recipe, item());
        let b = mapped(&recipe, item());
        assert_eq!(a.dedup_key(), b.dedup_key());
        assert_eq!(a.source_key.as_str(), "recipe-1:message-1");
        assert_eq!(a.connector_instance.as_str(), "source:01source");
        assert_eq!(a.operation, Operation::Upsert);
        assert_eq!(a.sensitivity, Sensitivity::Restricted);
        assert_eq!(a.device, None);
        assert_eq!(a.kind.as_str(), "source.email");
        assert_eq!(a.payload["title"], "Orchids");
        assert_eq!(a.payload["item"], item());
        a.validate().unwrap();
        let mut edited = item();
        edited["subject"] = json!("Roses");
        let edited = mapped(&recipe, edited);
        assert_eq!(a.source_key, edited.source_key);
        assert_ne!(a.revision, edited.revision);
    }

    #[test]
    fn ignore_keeps_revision_and_fallback_key_stable_without_losing_payload() {
        let mut recipe = recipe();
        recipe.map.id = None;
        recipe.ignore = vec![
            "/unread".into(),
            "/nested/a~1b/~0flag".into(),
            "/list/0".into(),
            "/list/1".into(),
        ];
        let mut a = item();
        a["nested"] = json!({"a/b":{"~flag":1}});
        a["list"] = json!(["volatile-a", "volatile-b", "keep"]);
        let mut b = a.clone();
        b["unread"] = json!(false);
        b["nested"]["a/b"]["~flag"] = json!(2);
        b["list"][0] = json!("changed-a");
        b["list"][1] = json!("changed-b");
        let first = mapped(&recipe, a);
        let second = mapped(&recipe, b);
        assert_eq!(first.dedup_key(), second.dedup_key());
        assert_ne!(first.payload, second.payload);
        assert!(first.source_key.as_str().starts_with("recipe-1:sha256:"));
    }

    #[test]
    fn content_hash_is_independent_of_object_field_order() {
        let mut recipe = recipe();
        recipe.map.id = None;
        let a: Value =
            serde_json::from_str(r#"{"id":1,"subject":"same","sent_at":"2026-11-05T08:00:00Z"}"#)
                .unwrap();
        let b: Value =
            serde_json::from_str(r#"{"sent_at":"2026-11-05T08:00:00Z","subject":"same","id":1}"#)
                .unwrap();
        assert_eq!(
            mapped(&recipe, a).dedup_key(),
            mapped(&recipe, b).dedup_key()
        );
    }

    #[test]
    fn core_and_non_source_kinds_are_refused_even_for_empty_responses() {
        let mut recipe = recipe();
        for kind in hx_schema::kinds::CORE_KINDS.iter().copied().chain([
            "other.event",
            "source.",
            "source.Email",
            "source..email",
        ]) {
            recipe.kind = kind.into();
            assert!(
                matches!(PullMapper::new(&recipe, context()), Err(RecipeError::Kind)),
                "accepted {kind}"
            );
        }
    }

    #[test]
    fn bad_or_missing_times_and_negative_spans_are_skipped_and_counted() {
        let mut recipe = recipe();
        recipe.map.end = Some("/end".into());
        let mut bad = item();
        bad["sent_at"] = json!("not-a-time");
        let mut missing = item();
        missing.as_object_mut().unwrap().remove("sent_at");
        let mut reverse = item();
        reverse["end"] = json!("2026-11-05T07:00:00Z");
        let mut good = item();
        good["end"] = json!("2026-11-05T09:00:00Z");
        let mut mapper = PullMapper::new(&recipe, context()).unwrap();
        let result = mapper
            .map_page(&json!({"messages":[bad, missing, reverse, good]}))
            .unwrap();
        assert_eq!(result.envelopes.len(), 1);
        assert_eq!(mapper.counts().bad_time, 3);
        assert_eq!(mapper.counts().items_seen, 4);
    }

    #[test]
    fn history_floor_bounds_total_windows_across_repeated_pulls() {
        // Independent four-hour bucket oracle, matching the ingest window contract.
        let recipe = recipe();
        let mut windows = std::collections::BTreeSet::new();
        let mut skipped = 0;
        for pull in 0..3 {
            let ctx = MapContext {
                pulled_at: context().connected_at + Duration::days(5 + pull),
                ..context()
            };
            let messages: Vec<_> = (0..365 * 6).map(|bucket| {
                json!({"id":bucket, "sent_at":(ctx.connected_at - Duration::hours(bucket * 4)).to_rfc3339()})
            }).collect();
            let mut paged = recipe.clone();
            paged.pages = Some(Pages {
                next: "/next".into(),
                max: 20,
            });
            let mut mapper = PullMapper::new(&paged, ctx).unwrap();
            // Different slices on later pulls simulate continuing through a year of mail.
            for page in messages.chunks(300).skip(pull as usize * 2).take(2) {
                for envelope in mapper
                    .map_page(&json!({"messages":page}))
                    .unwrap()
                    .envelopes
                {
                    windows.insert(
                        envelope
                            .time
                            .source_start
                            .unwrap()
                            .timestamp()
                            .div_euclid(4 * 3600),
                    );
                }
            }
            skipped += mapper.counts().too_old;
        }
        assert_eq!(windows.len(), 7, "later pulls must not admit older windows");
        assert_eq!(skipped, 1800 - 7);
    }

    #[test]
    fn future_items_and_long_spans_cannot_expand_work_without_bound() {
        let mut recipe = recipe();
        recipe.map.end = Some("/end".into());
        let ctx = context();
        let response = json!({"messages":[
            {"sent_at":(ctx.pulled_at + Duration::days(1) + Duration::milliseconds(1)).to_rfc3339(),
             "end":(ctx.pulled_at + Duration::days(2)).to_rfc3339()},
            {"sent_at":ctx.pulled_at.to_rfc3339(), "end":(ctx.pulled_at + Duration::days(730)).to_rfc3339()},
            {"sent_at":ctx.pulled_at.to_rfc3339(), "end":(ctx.pulled_at + Duration::hours(24)).to_rfc3339()},
            {"sent_at":(ctx.pulled_at + Duration::days(1)).to_rfc3339(), "end":(ctx.pulled_at + Duration::days(1)).to_rfc3339()}
        ]});
        let mut mapper = PullMapper::new(&recipe, ctx).unwrap();
        let page = mapper.map_page(&response).unwrap();
        assert_eq!(page.envelopes.len(), 3, "skip starts beyond tomorrow");
        assert_eq!(mapper.counts().too_future, 1);
        assert_eq!(mapper.counts().long_span, 1);
        assert_eq!(
            page.envelopes[0].time.source_end, None,
            "long spans become points"
        );
        assert_eq!(
            page.envelopes[1].time.source_end,
            Some(ctx.pulled_at + Duration::hours(24))
        );
        recipe.time_format = TimeFormat::UnixS;
        recipe.map.end = None;
        let mut mapper = PullMapper::new(&recipe, ctx).unwrap();
        assert!(
            mapper
                .map_page(&json!({"messages":[{"sent_at":ctx.pulled_at.timestamp_millis()}]}))
                .unwrap()
                .envelopes
                .is_empty()
        );
        assert_eq!(mapper.counts().too_future, 1);
    }

    #[test]
    fn nul_is_removed_recursively_before_identity_hashing_and_payload_copy() {
        let recipe = recipe();
        let mut dirty = item();
        dirty["id"] = json!("mes\u{0000}sage-1");
        dirty["subject"] = json!("Orch\u{0000}ids");
        dirty["nested\u{0000}"] = json!([{ "ke\u{0000}y": "val\u{0000}ue" }]);
        let mut clean = item();
        clean["nested"] = json!([{ "key":"value" }]);
        let envelope = mapped(&recipe, dirty);
        assert_eq!(
            envelope.dedup_key(),
            mapped(&recipe, clean.clone()).dedup_key()
        );
        assert_eq!(envelope.payload, json!({"title":"Orchids", "item":clean}));
        assert!(
            !serde_json::to_string(&envelope)
                .unwrap()
                .contains("\\u0000")
        );
        envelope.validate().unwrap();
    }

    #[test]
    fn owner_context_controls_source_identity() {
        let mut recipe = recipe();
        recipe.source = "agent-supplied-other-source".into();
        for source_id in ["01source", "owner-selected-source"] {
            let page = PullMapper::new(
                &recipe,
                MapContext {
                    source_id,
                    ..context()
                },
            )
            .unwrap()
            .map_page(&json!({"messages":[item()]}))
            .unwrap();
            assert_eq!(
                page.envelopes[0].connector_instance.as_str(),
                format!("source:{source_id}")
            );
        }
    }

    #[test]
    fn root_ignore_is_refused_and_copied_title_is_bounded_utf8() {
        let mut recipe = recipe();
        recipe.ignore = vec![String::new()];
        assert!(recipe.validate().is_err());
        recipe.ignore.clear();
        let mut input = item();
        input["subject"] = json!("🌻".repeat(257));
        let envelope = mapped(&recipe, input.clone());
        assert_eq!(envelope.payload["title"].as_str().unwrap().len(), 1024);
        assert_eq!(envelope.payload["item"], input);
    }

    #[test]
    fn zero_history_and_multi_page_cutoff_cannot_buy_old_windows() {
        let mut recipe = recipe();
        recipe.pages = Some(Pages {
            next: "/next".into(),
            max: 20,
        });
        let mut mapper = PullMapper::new(
            &recipe,
            MapContext {
                history_days: 0,
                ..context()
            },
        )
        .unwrap();
        for _ in 0..2 {
            let mut at_connect = item();
            at_connect["sent_at"] = json!("2026-11-05T12:00:00Z");
            assert_eq!(
                mapper
                    .map_page(&json!({"messages":[item(), at_connect]}))
                    .unwrap()
                    .envelopes
                    .len(),
                1
            );
        }
        assert_eq!(mapper.counts().too_old, 2);
    }

    #[test]
    fn null_start_uses_pull_time_and_unix_formats_map_exactly() {
        let mut recipe = recipe();
        recipe.map.start = None;
        assert_eq!(
            mapped(&recipe, item()).time.source_start,
            Some(context().pulled_at)
        );
        recipe.map.start = Some("/sent_at".into());
        for (format, value, expected, resolution) in [
            (
                TimeFormat::UnixS,
                json!(1793865600),
                "2026-11-05T08:00:00Z",
                1000,
            ),
            (
                TimeFormat::UnixMs,
                json!(1793865600123_i64),
                "2026-11-05T08:00:00.123Z",
                1,
            ),
        ] {
            recipe.time_format = format;
            let mut input = item();
            input["sent_at"] = value;
            let time = mapped(&recipe, input).time;
            assert_eq!(time.source_start, Some(expected.parse().unwrap()));
            assert_eq!(time.observed_at, context().pulled_at);
            assert_eq!(time.resolution_ms, resolution);
        }
    }

    #[test]
    fn oversized_items_are_skipped_and_counted_at_the_exact_byte_limit() {
        let mut recipe = recipe();
        recipe.map.start = None;
        let base = json!({"id":"large", "text":""});
        let overhead = serde_json::to_vec(&base).unwrap().len();
        let mut exact = base.clone();
        exact["text"] = json!("x".repeat(MAX_ITEM_BYTES - overhead));
        let mut over = exact.clone();
        over["text"] = json!("x".repeat(MAX_ITEM_BYTES - overhead + 1));
        let mut mapper = PullMapper::new(&recipe, context()).unwrap();
        assert_eq!(
            mapper
                .map_page(&json!({"messages":[exact, over]}))
                .unwrap()
                .envelopes
                .len(),
            1
        );
        assert_eq!(mapper.counts().too_large, 1);
    }

    #[test]
    fn pull_budget_is_shared_across_pages_and_resumes_at_last_consumed_item() {
        let mut recipe = recipe();
        recipe.pages = Some(Pages {
            next: "/next".into(),
            max: 20,
        });
        recipe.cursor = Some(Cursor {
            from: CursorFrom::Item,
            pointer: "/id".into(),
        });
        let batch = |start, count| {
            Value::Array(
                (start..start + count)
                    .map(|id| {
                        let mut i = item();
                        i["id"] = json!(id);
                        i
                    })
                    .collect(),
            )
        };
        let mut mapper = PullMapper::new(&recipe, context()).unwrap();
        let first = mapper
            .map_page(&json!({"messages":batch(0, 600), "next":"p2"}))
            .unwrap();
        assert_eq!(first.next_page, Some(json!("p2")));
        let last = mapper
            .map_page(&json!({"messages":batch(600, 500), "next":"p3"}))
            .unwrap();
        assert_eq!(last.envelopes.len(), 400);
        assert_eq!(last.cursor, Some(json!(999)));
        assert!(last.unprocessed_items);
        assert_eq!(last.next_page, None);
        assert_eq!(mapper.counts().items_seen, MAX_ITEMS_PER_PULL);
        assert_eq!(
            mapper.map_page(&json!({"messages":[]})).unwrap_err(),
            RecipeError::PullLimit
        );
    }

    #[test]
    fn page_budget_counts_even_empty_pages_and_response_cursor_is_not_advanced_mid_page() {
        let mut recipe = recipe();
        recipe.pages = Some(Pages {
            next: "/next".into(),
            max: 20,
        });
        recipe.cursor = Some(Cursor {
            from: CursorFrom::Response,
            pointer: "/cursor".into(),
        });
        let mut mapper = PullMapper::new(&recipe, context()).unwrap();
        for page in 1..=20 {
            let result = mapper
                .map_page(&json!({"messages":[], "next":"again", "cursor":page}))
                .unwrap();
            assert_eq!(result.cursor, Some(json!(page)));
            assert_eq!(result.next_page.is_some(), page < 20);
        }
        assert_eq!(mapper.counts().pages, MAX_PAGES_PER_PULL);
        assert_eq!(
            mapper.map_page(&json!({"messages":[]})).unwrap_err(),
            RecipeError::PullLimit
        );
        let mut mapper = PullMapper::new(&recipe, context()).unwrap();
        let result = mapper
            .map_page(&json!({"messages":vec![item(); 1001], "cursor":"past-page"}))
            .unwrap();
        assert!(result.unprocessed_items);
        assert_eq!(
            result.cursor, None,
            "response cursor would lose unconsumed items"
        );
    }

    #[test]
    fn skipped_items_consume_budget_and_advance_item_cursor() {
        let mut recipe = recipe();
        recipe.cursor = Some(Cursor {
            from: CursorFrom::Item,
            pointer: "/id".into(),
        });
        let mut bad = item();
        bad["sent_at"] = json!("bad");
        let mut mapper = PullMapper::new(&recipe, context()).unwrap();
        let result = mapper
            .map_page(&json!({"messages":vec![bad; 1001]}))
            .unwrap();
        assert!(result.envelopes.is_empty());
        assert_eq!(result.cursor, Some(json!("message-1")));
        assert_eq!(mapper.counts().bad_time, 1000);
        assert!(result.unprocessed_items);
    }

    #[test]
    fn invalid_configuration_and_items_pointer_fail_without_echoing_source_data() {
        let mut recipe = recipe();
        recipe.items = "/bad~2pointer".into();
        assert!(recipe.validate().is_err());
        recipe.items = "/messages".into();
        recipe.pages = Some(Pages {
            next: "/next".into(),
            max: 21,
        });
        assert!(recipe.validate().is_err());
        recipe.pages = None;
        assert!(
            PullMapper::new(
                &recipe,
                MapContext {
                    history_days: 31,
                    ..context()
                }
            )
            .is_err()
        );
        let mut mapper = PullMapper::new(&recipe, context()).unwrap();
        assert_eq!(
            mapper.map_page(&json!({"secret":"planted"})).unwrap_err(),
            RecipeError::Items
        );
    }
}
