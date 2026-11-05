//! Redact source secrets before data crosses a storage or disclosure boundary.

use base64::{Engine, engine::general_purpose};
use regex::{NoExpand, Regex};
use serde_json::Value;
use std::collections::BTreeSet;

pub const REDACTED: &str = "[REDACTED]";
/// Shorter credential JSON values are labels like "user" or "web". Redacting them
/// would destroy ordinary text everywhere they occur.
const MIN_CREDENTIAL_VALUE: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum ScrubError {
    #[error("source keys must contain at least 4 bytes")]
    KeyTooShort,
    #[error("could not prepare source secret scrubber")]
    Pattern,
}

// Deliberately no Debug: compiled expressions contain the secrets.
pub struct Scrubber {
    pattern: Option<Regex>,
}

impl Scrubber {
    /// Each supplied key must be at least four UTF-8 bytes. For credential JSON,
    /// string values of at least eight bytes are also protected; property names,
    /// numbers and shorter values are not treated as secrets.
    pub fn new<I, S>(secrets: I) -> Result<Self, ScrubError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut values = BTreeSet::new();
        for secret in secrets {
            let secret = secret.as_ref();
            validate_key(secret)?;
            values.insert(secret.to_owned());
            if let Ok(value) = serde_json::from_str::<Value>(secret) {
                collect_strings(&value, &mut values);
            }
        }
        // Prefer complete secrets over a shorter key that is their prefix.
        let mut values: Vec<_> = values.into_iter().collect();
        values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        let mut forms = Vec::new();
        for value in values {
            forms.push(json_pattern(&value));
            forms.push(url_pattern(&value));
            let hex: String = value.bytes().map(|byte| format!("{byte:02x}")).collect();
            forms.push(format!("(?i:{hex})"));
            for engine in [
                general_purpose::STANDARD,
                general_purpose::URL_SAFE,
                general_purpose::STANDARD_NO_PAD,
                general_purpose::URL_SAFE_NO_PAD,
            ] {
                forms.push(regex::escape(&engine.encode(&value)));
            }
            forms.push(regex::escape(&value));
        }
        let pattern = if forms.is_empty() {
            None
        } else {
            // Never expose regex errors: they can include the secret pattern.
            Some(Regex::new(&forms.join("|")).map_err(|_| ScrubError::Pattern)?)
        };
        Ok(Self { pattern })
    }

    pub fn scrub(&self, text: &str) -> String {
        match &self.pattern {
            Some(pattern) => pattern.replace_all(text, NoExpand(REDACTED)).into_owned(),
            None => text.to_owned(),
        }
    }

    /// Uses exactly the same forms as `scrub`, for rejecting saved descriptions.
    pub fn contains_secret(&self, text: &str) -> bool {
        self.pattern
            .as_ref()
            .is_some_and(|pattern| pattern.is_match(text))
    }
}

pub(crate) fn validate_key(key: &str) -> Result<(), ScrubError> {
    if key.len() < 4 {
        return Err(ScrubError::KeyTooShort);
    }
    Ok(())
}

fn collect_strings(value: &Value, strings: &mut BTreeSet<String>) {
    match value {
        Value::String(value) if value.len() >= MIN_CREDENTIAL_VALUE => {
            strings.insert(value.clone());
        }
        Value::Array(values) => {
            for value in values {
                collect_strings(value, strings);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_strings(value, strings);
            }
        }
        _ => {}
    }
}

fn url_pattern(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            let mut bytes = [0; 4];
            let encoded: String = character
                .encode_utf8(&mut bytes)
                .bytes()
                .map(|byte| format!("%{byte:02x}"))
                .collect();
            let space = if character == ' ' { "|\\+" } else { "" };
            // Accept mixed percent/literal forms and either case of hex digits.
            format!(
                "(?:(?i:{encoded}){space}|{})",
                regex::escape(&character.to_string())
            )
        })
        .collect()
}

fn json_pattern(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            let mut units = [0; 2];
            let unicode: String = character
                .encode_utf16(&mut units)
                .iter()
                .map(|unit| format!(r"\u{unit:04x}"))
                .collect();
            let quoted = serde_json::to_string(&character.to_string())
                .expect("serializing a character cannot fail");
            let escaped = &quoted[1..quoted.len() - 1];
            let slash = if character == '/' { r"|\\/" } else { "" };
            format!(
                "(?:(?i:{})|{}{slash}|{})",
                regex::escape(&unicode),
                regex::escape(escaped),
                regex::escape(&character.to_string())
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every key and credential in these tests is invented.
    const KEY: &str = "demo+key/42?=\"leaf\"\n☃";

    #[test]
    fn short_credential_values_do_not_wreck_ordinary_text() {
        let scrubber =
            Scrubber::new([r#"{"type":"user","x":"a","token":"made-up-refresh-token"}"#]).unwrap();
        assert_eq!(scrubber.scrub("a user read data"), "a user read data");
        assert_eq!(scrubber.scrub("made-up-refresh-token"), REDACTED);
    }

    #[test]
    fn planted_forms_are_redacted() {
        let scrubber = Scrubber::new([KEY]).unwrap();
        let forms = [
            ("raw", KEY),
            ("base64", "ZGVtbytrZXkvNDI/PSJsZWFmIgrimIM="),
            ("base64 unpadded", "ZGVtbytrZXkvNDI/PSJsZWFmIgrimIM"),
            ("base64url", "ZGVtbytrZXkvNDI_PSJsZWFmIgrimIM="),
            ("base64url unpadded", "ZGVtbytrZXkvNDI_PSJsZWFmIgrimIM"),
            ("URL", "demo%2Bkey%2F42%3F%3D%22leaf%22%0A%E2%98%83"),
            (
                "URL lowercase",
                "demo%2bkey%2f42%3f%3d%22leaf%22%0a%e2%98%83",
            ),
            ("URL mixed", "%64emo%2bkey/42%3F=%22leaf%22%0a%E2%98%83"),
            ("hex", "64656d6f2b6b65792f34323f3d226c656166220ae29883"),
            (
                "hex uppercase",
                "64656D6F2B6B65792F34323F3D226C656166220AE29883",
            ),
            ("JSON", r#"demo+key/42?=\"leaf\"\n☃"#),
            ("JSON Unicode", r#"demo+key/42?=\"leaf\"\n\u2603"#),
            (
                "JSON mixed escapes",
                r#"\u0064emo+key\/42?=\u0022leaf\"\u000A☃"#,
            ),
        ];
        for (name, planted) in forms {
            let text = format!("before {planted} middle {planted} after");
            assert!(scrubber.contains_secret(&text), "detect {name}");
            assert_eq!(
                scrubber.scrub(&text),
                format!("before {REDACTED} middle {REDACTED} after"),
                "scrub {name}"
            );
        }
    }

    #[test]
    fn credential_json_values_are_scrubbed_recursively() {
        let credential = r#"{"client_id":"made-up-client","nested":{"token":"fake-token/42"},"values":["fake-refresh+key","xy",""],"expires":123}"#;
        let scrubber = Scrubber::new([credential]).unwrap();
        for planted in [
            credential,
            "made-up-client",
            "fake-token/42",
            "ZmFrZS10b2tlbi80Mg==",
            "fake-token%2F42",
            "66616b652d746f6b656e2f3432",
            r#"fake-token\/42"#,
            "fake-refresh+key",
        ] {
            assert_eq!(scrubber.scrub(planted), REDACTED);
        }
        assert_eq!(
            scrubber.scrub("client_id nested token expires 123"),
            "client_id nested token expires 123"
        );
    }

    #[test]
    fn short_keys_are_refused_by_byte_length() {
        for key in ["", "a", "ab", "abc", "é", "☃"] {
            assert!(matches!(Scrubber::new([key]), Err(ScrubError::KeyTooShort)));
        }
        assert_eq!(Scrubber::new(["éé"]).unwrap().scrub("éé"), REDACTED);
    }

    #[test]
    fn preserves_unrelated_text_and_redacts_overlapping_secrets_once() {
        let scrubber = Scrubber::new(["fake", "fake-long-key", "other-key"]).unwrap();
        assert_eq!(scrubber.scrub("unrelated café"), "unrelated café");
        assert_eq!(
            scrubber.scrub("fake-long-key fake other-key"),
            format!("{REDACTED} {REDACTED} {REDACTED}")
        );
        assert_eq!(
            Scrubber::new(Vec::<String>::new())
                .unwrap()
                .scrub("unchanged"),
            "unchanged"
        );
        assert!(!scrubber.contains_secret("unrelated"));
    }

    #[test]
    fn form_spaces_and_json_surrogate_pairs_are_scrubbed() {
        let scrubber = Scrubber::new(["fake space+key", "fake-🦀-key"]).unwrap();
        for planted in [
            "fake+space%2Bkey",
            "fake%20space%2bkey",
            r"fake-\ud83e\udd80-key",
            r"fake-\uD83E\uDD80-key",
        ] {
            assert_eq!(scrubber.scrub(planted), REDACTED);
        }
    }
}
