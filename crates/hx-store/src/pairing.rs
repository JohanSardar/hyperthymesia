//! One-use pairing credentials. Raw codes and tokens are returned once, never stored.
use crate::{Store, Ts, hash, random_secret, valid_hash};
use anyhow::{Result, ensure};
use chrono::{Duration, Utc};
use serde::Serialize;

/// Safe to display: `id` is a token hash, never the bearer secret.
#[derive(Serialize, sqlx::FromRow)]
pub struct TokenInfo {
    pub id: String,
    pub name: String,
    #[sqlx(json)]
    pub prefixes: Vec<String>,
    #[sqlx(json)]
    pub scopes: Vec<String>,
    pub revoked: bool,
}

impl Store {
    pub async fn create_pair_code(&self, name: &str) -> Result<String> {
        ensure!(!name.trim().is_empty(), "a device name is required");
        let code = random_secret()?;
        let mut tx = self.begin_write().await?;
        sqlx::query("INSERT INTO pair_codes(hash,name,expires_at) VALUES($1,$2,$3)")
            .bind(hash(code.as_bytes()))
            .bind(name)
            .bind(Ts::from(Utc::now() + Duration::minutes(15)))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(code)
    }

    /// Acquire the writer BEFORE checking validity or expiry. Both consumption
    /// and token creation commit together; a lost response still burns the code.
    pub async fn redeem_pair_code(&self, code: &str, device_id: &str) -> Result<Option<String>> {
        let mut tx = self.begin_write().await?;
        if code.len() != 64
            || !code.bytes().all(|c| c.is_ascii_hexdigit())
            || device_id.is_empty()
            || device_id.len() > 255
            || device_id.trim() != device_id
            || device_id.chars().any(char::is_control)
        {
            return Ok(None);
        }
        let code_hash = hash(code.to_ascii_lowercase().as_bytes());
        let now = Ts::now();
        let name: Option<String> = sqlx::query_scalar(
            "SELECT name FROM pair_codes WHERE hash=$1 AND used_at IS NULL AND expires_at>$2",
        )
        .bind(&code_hash)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(name) = name else { return Ok(None) };
        let prefixes: Vec<_> = [
            "macos-agent",
            "macos-media",
            "companion:browser",
            "companion:editor",
            "companion:shell",
        ]
        .into_iter()
        .map(|namespace| format!("{namespace}:{device_id}"))
        .collect();
        let secret = random_secret()?;
        let token_hash = hash(secret.as_bytes());
        sqlx::query(
            "INSERT INTO device_tokens(hash,name,prefixes,scopes) VALUES($1,$2,$3,'[\"ingest\"]')",
        )
        .bind(&token_hash)
        .bind(name)
        .bind(sqlx::types::Json(prefixes))
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE pair_codes SET used_at=$2,token_hash=$3 WHERE hash=$1")
            .bind(code_hash)
            .bind(now)
            .bind(token_hash)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(secret))
    }

    pub async fn list_tokens(&self) -> Result<Vec<TokenInfo>> {
        Ok(sqlx::query_as(
            "SELECT hash AS id,name,prefixes,scopes,revoked FROM device_tokens ORDER BY name,hash",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Use the full ID from `list_tokens`, avoiding ambiguous names or prefixes.
    pub async fn revoke_token(&self, id: &str) -> Result<bool> {
        ensure!(valid_hash(id), "use the full token ID from token list");
        let mut tx = self.begin_write().await?;
        let changed = sqlx::query("UPDATE device_tokens SET revoked=1 WHERE hash=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            == 1;
        tx.commit().await?;
        Ok(changed)
    }
}
