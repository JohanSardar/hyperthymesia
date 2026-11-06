//! Display causes, not database row details or credentials.
use anyhow::Error;
use std::str::FromStr;

pub fn diagnostic(error: &Error, connection: &str) -> String {
    let mut reasons = Vec::new();
    for cause in error.chain() {
        if let Some(error) = cause.downcast_ref::<sqlx::Error>() {
            if matches!(error, sqlx::Error::Migrate(_)) {
                reasons.push("schema migration failed".to_owned());
                continue;
            }
            let reason = match error {
                sqlx::Error::Database(db) => format!(
                    "database {:?} (SQLSTATE {}, table {}, constraint {})",
                    db.kind(),
                    db.code().as_deref().unwrap_or("unknown"),
                    db.table().unwrap_or("unspecified"),
                    db.constraint().unwrap_or("unspecified")
                ),
                sqlx::Error::ColumnDecode { index, .. } => {
                    format!("database column {index} could not be decoded")
                }
                sqlx::Error::Encode(_)
                | sqlx::Error::AnyDriverError(_)
                | sqlx::Error::InvalidArgument(_) => {
                    "invalid database argument or encoding".to_owned()
                }
                sqlx::Error::Decode(_) => "database value could not be decoded".to_owned(),
                sqlx::Error::Configuration(_) => {
                    "invalid database connection configuration".to_owned()
                }
                // Protocol errors (including unknown message type), connection
                // I/O, missing columns and type mismatches carry no row values.
                _ => error.to_string(),
            };
            reasons.push(reason);
            break;
        }
        if let Some(error) = cause.downcast_ref::<serde_json::Error>() {
            reasons.push(format!(
                "invalid JSON ({:?}, line {}, column {})",
                error.classify(),
                error.line(),
                error.column()
            ));
            break;
        }
        if cause.is::<sqlx::migrate::MigrateError>() {
            reasons.push("schema migration failed".to_owned());
            continue;
        }
        reasons.push(cause.to_string());
    }
    let mut message = reasons.join(": ");
    // Also redact encoded and decoded passwords when a cause includes a URI or
    // credentials. Query-string passwords are supported by the URL parser too.
    if let Ok(url) = url::Url::from_str(connection) {
        let mut safe = url.clone();
        if url.password().is_some() {
            let _ = safe.set_password(Some("REDACTED"));
        }
        let query: Vec<_> = url
            .query_pairs()
            .map(|(key, value)| {
                let secret = key == "password";
                (
                    key.into_owned(),
                    if secret {
                        "REDACTED".to_owned()
                    } else {
                        value.into_owned()
                    },
                )
            })
            .collect();
        if !query.is_empty() {
            safe.query_pairs_mut().clear().extend_pairs(query);
        }
        message = message.replace(connection, safe.as_str());
        let mut secrets = Vec::new();
        if let Some(password) = url.password() {
            secrets.push(password.to_owned());
            secrets.push(
                percent_encoding::percent_decode_str(password)
                    .decode_utf8_lossy()
                    .into_owned(),
            );
        }
        secrets.extend(
            url.query_pairs()
                .filter(|(key, _)| key == "password")
                .map(|(_, value)| value.into_owned()),
        );
        secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        for secret in secrets.into_iter().filter(|secret| !secret.is_empty()) {
            message = message.replace(&secret, "[REDACTED]");
        }
    }
    // Print control characters escaped, so a malformed protocol response or
    // path cannot forge another line of the diagnostic.
    message
        .chars()
        .flat_map(|c| {
            if c == '\n' {
                "\\n".chars().collect::<Vec<_>>()
            } else if c.is_control() {
                c.escape_default().collect()
            } else {
                vec![c]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_cause_survives_but_uri_passwords_and_control_characters_do_not() {
        let connection = "postgres://hx:p%40ssword@localhost/hx?password=query-secret";
        let error = anyhow::anyhow!("connection {connection}: p@ssword, query-secret")
            .context(sqlx::Error::Protocol("unknown message type: '\0'".into()));
        // Both a top-level protocol error and a URI-bearing context are safe.
        let text = diagnostic(&error, connection);
        assert!(text.contains("unknown message type") && text.contains("\\u{0}"));
        assert!(!text.contains('\0'));
        let error = anyhow::anyhow!("connection {connection}: p@ssword, query-secret")
            .context("source unavailable");
        let text = diagnostic(&error, connection);
        assert!(text.contains("source unavailable") && text.contains("REDACTED"));
        for secret in ["p%40ssword", "p@ssword", "query-secret"] {
            assert!(!text.contains(secret));
        }
    }
}
