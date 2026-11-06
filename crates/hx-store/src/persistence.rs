//! Shared runtime/mover representations for JSON, job windows and dependencies.
use super::*;
use std::collections::BTreeSet;

/// Serialize JSON with the legacy numeric storage representation: decimal
/// expansion of exponent-form numbers and unsigned numeric zero. Keeping that
/// text (rather than parsing/re-encoding it here) gives reads exactly one serde
/// parse, just as before, including extreme floating-point rounding behavior.
/// serde's default maps sort keys when read, independent of database key order.
pub fn stored_json(value: Value) -> String {
    fn encode(value: &Value) -> String {
        match value {
            Value::Array(values) => format!(
                "[{}]",
                values.iter().map(encode).collect::<Vec<_>>().join(",")
            ),
            Value::Object(values) => format!(
                "{{{}}}",
                values
                    .iter()
                    .map(|(key, value)| format!(
                        "{}:{}",
                        serde_json::to_string(key).expect("JSON key"),
                        encode(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Value::Number(number) => hx_schema::json::stored_number(number),
            _ => value.to_string(),
        }
    }
    encode(&value)
}

pub fn job_window_start(payload: &Value) -> Result<Option<Ts>> {
    payload
        .get("window")
        .and_then(|w| w.get("start"))
        .map(|start| serde_json::from_value::<DateTime<Utc>>(start.clone()).map(Ts::from))
        .transpose()
        .map_err(Into::into)
}

/// None identifies legacy runs without a dependency record, distinct from a
/// modern run that consulted prior context and found no usable dependencies.
pub fn revision_dependency_ids(content: &Value, run: Option<&Value>) -> Option<Vec<i64>> {
    let explicit = content
        .pointer("/input_manifest/prior_revision_ids")
        .and_then(Value::as_array);
    let calls = run.and_then(|r| r.get("calls")).and_then(Value::as_array);
    if explicit.is_none() && calls.is_none() {
        return None;
    }
    let mut ids = BTreeSet::new();
    if let Some(explicit) = explicit {
        ids.extend(explicit.iter().filter_map(Value::as_i64));
    }
    for call in calls.into_iter().flatten() {
        if call["tool"] == "prior_context" {
            ids.extend(
                call.pointer("/result/revisions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|r| r["revision_id"].as_i64()),
            );
        }
    }
    Some(ids.into_iter().collect())
}

pub async fn record_revision_dependencies(
    tx: &mut WriteTransaction,
    id: i64,
    dependencies: Option<&[i64]>,
) -> Result<()> {
    for prior in dependencies.into_iter().flatten() {
        sqlx::query("INSERT INTO revision_dependencies(revision_id,prior_revision_id) VALUES($1,$2) ON CONFLICT DO NOTHING")
            .bind(id).bind(prior).execute(&mut **tx).await?;
    }
    Ok(())
}

/// Object containment for revision identities: missing differs from JSON null,
/// extra stored fields are allowed, and every requested field participates.
pub(crate) fn json_contains(stored: &Value, requested: &Value) -> bool {
    match (stored, requested) {
        (Value::Object(a), Value::Object(b)) => b.iter().all(|(k, v)| {
            a.get(k).is_some_and(|a| {
                std::mem::discriminant(a) == std::mem::discriminant(v) && json_contains(a, v)
            })
        }),
        (Value::Array(a), Value::Array(b)) => b.iter().all(|b| {
            a.iter()
                .any(|a| a.is_array() == b.is_array() && json_contains(a, b))
        }),
        (Value::Array(a), b) if !b.is_object() => {
            a.iter().any(|a| !a.is_array() && json_contains(a, b))
        }
        (Value::Number(_), Value::Number(_)) => {
            let normalize = |v: &Value| {
                let s = stored_json(v.clone());
                if s.contains('.') {
                    s.trim_end_matches('0').trim_end_matches('.').to_owned()
                } else {
                    s
                }
            };
            normalize(stored) == normalize(requested)
        }
        _ => stored == requested,
    }
}
