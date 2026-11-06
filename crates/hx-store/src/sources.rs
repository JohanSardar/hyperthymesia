//! Source configuration belongs to core.sqlite. Agent-created tables alone use
//! agent.sqlite. Callers enforce owner/setup authorization and scrub secrets
//! before passing descriptions, recipes, add-ons or live payloads here.
use super::*;
use anyhow::ensure;
use hx_schema::{
    ConnectorInstance, Kind, ObservationId, ObservationTime, Operation, Revision, SourceKey,
};
use hx_sources::recipe::Recipe;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize)]
pub struct Source {
    pub id: String,
    pub connector_instance: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub status: String,
    pub history_days: i64,
    pub connected_at: DateTime<Utc>,
    pub setup_generation: i64,
    pub last_pull_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StoredRecipe {
    pub id: String,
    pub source_id: String,
    pub name: String,
    pub request_hash: String,
    pub recipe: sqlx::types::Json<Recipe>,
    pub enabled: bool,
    pub owner_off: bool,
}

#[derive(Debug, Clone)]
pub struct RecipeState {
    pub recipe_id: String,
    pub cursor: Option<Value>,
    pub next_run_at: DateTime<Utc>,
    pub last_ok_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Addon {
    pub id: String,
    pub source_id: String,
    pub text: String,
    pub enabled: bool,
    pub owner_off: bool,
}

#[derive(Debug, Clone)]
pub struct SetupRun {
    pub source_id: String,
    pub generation: i64,
    pub status: String,
    pub result: Option<Value>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl Store {
    /// Owner-only connect. The source is enabled immediately; setup and its
    /// two-attempt job are committed together. This creates no window work.
    pub async fn connect_source(
        &self,
        name: &str,
        description: &str,
        history_days: u8,
    ) -> Result<Source> {
        ensure!(!name.trim().is_empty(), "source name required");
        ensure!(history_days <= 30, "history_days must be in 0..=30");
        let id = new_id();
        let connector = format!("source:{id}");
        let now = Ts::now();
        let mut tx = self.begin_write().await?;
        let source = sqlx::query_as("INSERT INTO sources(id,connector_instance,name,description,history_days,connected_at) VALUES($1,$2,$3,$4,$5,$6) RETURNING *")
            .bind(&id).bind(connector).bind(name).bind(description).bind(i64::from(history_days)).bind(now).fetch_one(&mut *tx).await?;
        enqueue_setup(&mut tx, &id, 1, now).await?;
        tx.commit().await?;
        Ok(source)
    }

    pub async fn sources(&self) -> Result<Vec<Source>> {
        Ok(
            sqlx::query_as("SELECT * FROM sources ORDER BY connected_at,id")
                .fetch_all(&self.pool)
                .await?,
        )
    }

    pub async fn source(&self, id: &str) -> Result<Option<Source>> {
        Ok(sqlx::query_as("SELECT * FROM sources WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn source_grants(&self) -> Result<BTreeSet<String>> {
        Ok(sqlx::query_scalar::<_, String>(
            "SELECT connector_instance FROM sources WHERE enabled ORDER BY connector_instance",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .collect())
    }

    /// Owner-only. Agent saves and setup completion never call this path.
    /// No switch queues interpretation or changes a window generation.
    pub async fn set_source_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let mut tx = self.begin_write().await?;
        let changed = sqlx::query("UPDATE sources SET enabled=$2 WHERE id=$1")
            .bind(id)
            .bind(enabled)
            .execute(&mut *tx)
            .await?;
        ensure!(changed.rows_affected() == 1, "unknown source");
        log_switch(&mut tx, "source", id, enabled).await?;
        tx.commit().await
    }

    /// Owner-only retry, with a distinct durable job identity. It cannot enable
    /// an off source. The broker must still check the switch on every call.
    pub async fn request_source_setup(&self, source: &str) -> Result<i64> {
        let mut tx = self.begin_write().await?;
        let generation = sqlx::query_scalar("UPDATE sources SET setup_generation=setup_generation+1,status='setting_up',last_error=NULL WHERE id=$1 RETURNING setup_generation")
            .bind(source).fetch_one(&mut *tx).await?;
        enqueue_setup(&mut tx, source, generation, Ts::now()).await?;
        tx.commit().await?;
        Ok(generation)
    }

    pub async fn setup_run(&self, source: &str, generation: i64) -> Result<Option<SetupRun>> {
        Ok(
            sqlx::query_as("SELECT * FROM setup_runs WHERE source_id=$1 AND generation=$2")
                .bind(source)
                .bind(generation)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// The setup worker decides whether a result counts as success (at least
    /// one recipe, or an explicit no_pull). Old generations cannot activate.
    pub async fn finish_source_setup(
        &self,
        source: &str,
        generation: i64,
        result: Value,
        error: Option<&str>,
    ) -> Result<bool> {
        let result = stored_json(result);
        let mut tx = self.begin_write().await?;
        let updated = sqlx::query("UPDATE setup_runs SET status=$3,result=$4,last_error=$5,finished_at=$6 WHERE source_id=$1 AND generation=$2 AND status='pending' AND EXISTS(SELECT 1 FROM sources WHERE id=$1 AND setup_generation=$2)")
            .bind(source).bind(generation).bind(if error.is_some() { "failed" } else { "succeeded" }).bind(result).bind(error).bind(Ts::now()).execute(&mut *tx).await?;
        if updated.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        sqlx::query("UPDATE sources SET status=$2,last_error=$3 WHERE id=$1")
            .bind(source)
            .bind(if error.is_some() {
                "needs_attention"
            } else {
                "ready"
            })
            .bind(error)
            .execute(&mut *tx)
            .await?;
        if error.is_none() {
            sqlx::query("UPDATE recipes SET enabled=1 WHERE source_id=$1 AND NOT owner_off")
                .bind(source)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE addons SET enabled=1 WHERE source_id=$1 AND NOT owner_off")
                .bind(source)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    /// Setup-only save. Identity is the request, not the agent's chosen name.
    /// A re-save never writes enabled or owner_off, or resets the pull cursor.
    pub async fn save_recipe(&self, source: &str, recipe: &Recipe) -> Result<StoredRecipe> {
        recipe.validate()?;
        let mut recipe = recipe.clone();
        recipe.source = source.to_owned();
        let request_hash = hash(&serde_json::to_vec(&serde_json::to_value(
            &recipe.request,
        )?)?);
        let value = stored_json(serde_json::to_value(&recipe)?);
        let id = new_id();
        let mut tx = self.begin_write().await?;
        ensure_source_on(&mut tx, source).await?;
        let row: StoredRecipe = sqlx::query_as("INSERT INTO recipes(id,source_id,name,request_hash,recipe) VALUES($1,$2,$3,$4,$5) ON CONFLICT(source_id,request_hash) DO UPDATE SET name=EXCLUDED.name,recipe=EXCLUDED.recipe RETURNING *")
            .bind(id).bind(source).bind(&recipe.name).bind(request_hash).bind(value).fetch_one(&mut *tx).await?;
        sqlx::query(
            "INSERT INTO recipe_state(recipe_id,next_run_at) VALUES($1,$2) ON CONFLICT DO NOTHING",
        )
        .bind(&row.id)
        .bind(Ts::now())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn recipes(&self, source: Option<&str>) -> Result<Vec<StoredRecipe>> {
        Ok(sqlx::query_as(
            "SELECT * FROM recipes WHERE ($1 IS NULL OR source_id=$1) ORDER BY source_id,id",
        )
        .bind(source)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Owner-only, and the sole path that may clear a recipe's owner_off.
    pub async fn set_recipe_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        self.set_child_enabled("recipe", id, enabled).await
    }

    pub async fn recipe_state(&self, recipe: &str) -> Result<RecipeState> {
        Ok(
            sqlx::query_as("SELECT * FROM recipe_state WHERE recipe_id=$1")
                .bind(recipe)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    /// Save only after ingest commits. Cursor JSON null differs from no cursor.
    pub async fn save_recipe_state(&self, state: &RecipeState) -> Result<()> {
        let cursor = state.cursor.clone().map(stored_json);
        let mut tx = self.begin_write().await?;
        let updated = sqlx::query("UPDATE recipe_state SET cursor=$2,next_run_at=$3,last_ok_at=$4,last_error=$5 WHERE recipe_id=$1")
            .bind(&state.recipe_id).bind(cursor).bind(Ts::from(state.next_run_at)).bind(state.last_ok_at.map(Ts::from)).bind(&state.last_error).execute(&mut *tx).await?;
        ensure!(updated.rows_affected() == 1, "unknown recipe");
        tx.commit().await
    }

    pub async fn record_source_pull(
        &self,
        source: &str,
        at: DateTime<Utc>,
        reconnect: bool,
        error: Option<&str>,
    ) -> Result<()> {
        let mut tx = self.begin_write().await?;
        let updated = sqlx::query("UPDATE sources SET last_pull_at=$2,last_error=$3,status=CASE WHEN $4 THEN 'needs_reconnect' ELSE status END WHERE id=$1")
            .bind(source).bind(Ts::from(at)).bind(error).bind(reconnect).execute(&mut *tx).await?;
        ensure!(updated.rows_affected() == 1, "unknown source");
        tx.commit().await
    }

    /// Setup-only save. Re-saving identical notes preserves their owner switch.
    pub async fn save_addon(&self, source: &str, text: &str) -> Result<Addon> {
        ensure!(
            !text.trim().is_empty() && text.len() <= 1000,
            "add-on must contain 1..=1000 bytes"
        );
        let text_hash = hash(text.as_bytes());
        let id = new_id();
        let mut tx = self.begin_write().await?;
        ensure_source_on(&mut tx, source).await?;
        let existing: Option<Addon> =
            sqlx::query_as("SELECT * FROM addons WHERE source_id=$1 AND text_hash=$2")
                .bind(source)
                .bind(&text_hash)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(row) = existing {
            tx.commit().await?;
            return Ok(row);
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM addons WHERE source_id=$1")
            .bind(source)
            .fetch_one(&mut *tx)
            .await?;
        ensure!(count < 3, "at most three add-ons per source");
        let row = sqlx::query_as(
            "INSERT INTO addons(id,source_id,text_hash,text) VALUES($1,$2,$3,$4) RETURNING *",
        )
        .bind(id)
        .bind(source)
        .bind(text_hash)
        .bind(text)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn addons(&self, source: Option<&str>) -> Result<Vec<Addon>> {
        Ok(sqlx::query_as(
            "SELECT * FROM addons WHERE ($1 IS NULL OR source_id=$1) ORDER BY source_id,id",
        )
        .bind(source)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Owner-only, and the sole path that may clear an add-on's owner_off.
    pub async fn set_addon_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        self.set_child_enabled("addon", id, enabled).await
    }

    async fn set_child_enabled(&self, kind: &str, id: &str, enabled: bool) -> Result<()> {
        let sql = match kind {
            "recipe" => "UPDATE recipes SET enabled=$2,owner_off=NOT $2 WHERE id=$1",
            "addon" => "UPDATE addons SET enabled=$2,owner_off=NOT $2 WHERE id=$1",
            _ => bail!("unknown switch kind"),
        };
        let mut tx = self.begin_write().await?;
        let updated = sqlx::query(sql)
            .bind(id)
            .bind(enabled)
            .execute(&mut *tx)
            .await?;
        ensure!(updated.rows_affected() == 1, "unknown switch target");
        log_switch(&mut tx, kind, id, enabled).await?;
        tx.commit().await
    }

    /// Run-scoped context only. The broker issues the run ULID and scrubs the
    /// payload before calling. This never ingests, schedules or changes windows.
    /// The revision hashes just the payload, independent of collection time.
    pub async fn save_live_snapshot(
        &self,
        run_token: ulid::Ulid,
        source: &str,
        payload: Value,
    ) -> Result<i64> {
        let revision = Revision::digest(&serde_json::to_vec(&payload)?);
        let now = Utc::now();
        let envelope = Envelope {
            envelope_version: hx_schema::ENVELOPE_VERSION,
            observation_id: ObservationId::generate(),
            connector_instance: ConnectorInstance::new(format!("source:{source}")),
            source_key: SourceKey::new(format!("live:{run_token}:{}", revision.as_str())),
            revision: revision.clone(),
            operation: Operation::Observe,
            sequence: None,
            kind: Kind::new("source.live"),
            schema_version: 1,
            time: ObservationTime {
                source_start: None,
                ..ObservationTime::point(now)
            },
            device: None,
            actor: None,
            subject: None,
            resource: None,
            links: vec![],
            coverage: None,
            sensitivity: Sensitivity::Restricted,
            payload,
            blobs: vec![],
        };
        envelope.validate()?;
        let envelope = stored_json(serde_json::to_value(envelope)?);
        let connector = format!("source:{source}");
        let run_token = run_token.to_string();
        let mut tx = self.begin_write().await?;
        ensure_source_on(&mut tx, source).await?;
        let inserted = tx.insert_evidence(sqlx::query_scalar("INSERT INTO live_snapshots(id,run_token,connector_instance,revision,envelope,created_at) VALUES((SELECT next_id FROM evidence_seq),$1,$2,$3,$4,$5) ON CONFLICT(run_token,connector_instance,revision) DO NOTHING RETURNING id")
            .bind(&run_token).bind(&connector).bind(revision.as_str()).bind(envelope).bind(Ts::from(now))).await?;
        let id = match inserted {
            Some(id) => id,
            None => sqlx::query_scalar("SELECT id FROM live_snapshots WHERE run_token=$1 AND connector_instance=$2 AND revision=$3")
                .bind(run_token).bind(connector).bind(revision.as_str()).fetch_one(&mut *tx).await?,
        };
        tx.commit().await?;
        Ok(id)
    }
}

fn new_id() -> String {
    ulid::Ulid::generate().to_string().to_ascii_lowercase()
}

async fn ensure_source_on(tx: &mut WriteTransaction, source: &str) -> Result<()> {
    let on: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sources WHERE id=$1 AND enabled)")
            .bind(source)
            .fetch_one(&mut **tx)
            .await?;
    ensure!(on, "source unavailable");
    Ok(())
}

async fn log_switch(tx: &mut WriteTransaction, kind: &str, id: &str, enabled: bool) -> Result<()> {
    sqlx::query(
        "INSERT INTO switch_log(target_kind,target_id,enabled,changed_at) VALUES($1,$2,$3,$4)",
    )
    .bind(kind)
    .bind(id)
    .bind(enabled)
    .bind(Ts::now())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn enqueue_setup(
    tx: &mut WriteTransaction,
    source: &str,
    generation: i64,
    now: Ts,
) -> Result<()> {
    sqlx::query("INSERT INTO setup_runs(source_id,generation,created_at) VALUES($1,$2,$3)")
        .bind(source)
        .bind(generation)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO jobs(output_key,kind,payload,available_at,max_attempts) VALUES($1,'setup',$2,$3,2)")
        .bind(format!("setup:{source}:{generation}")).bind(json!({"source":source,"setup_generation":generation})).bind(now).execute(&mut **tx).await?;
    Ok(())
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for Source {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> std::result::Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            connector_instance: row.try_get("connector_instance")?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            enabled: row.try_get("enabled")?,
            status: row.try_get("status")?,
            history_days: row.try_get("history_days")?,
            connected_at: DateTime::<Utc>::from(row.try_get::<Ts, _>("connected_at")?),
            setup_generation: row.try_get("setup_generation")?,
            last_pull_at: row
                .try_get::<Option<Ts>, _>("last_pull_at")?
                .map(DateTime::<Utc>::from),
            last_error: row.try_get("last_error")?,
        })
    }
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for RecipeState {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> std::result::Result<Self, sqlx::Error> {
        Ok(Self {
            recipe_id: row.try_get("recipe_id")?,
            cursor: row.try_get("cursor")?,
            next_run_at: DateTime::<Utc>::from(row.try_get::<Ts, _>("next_run_at")?),
            last_ok_at: row
                .try_get::<Option<Ts>, _>("last_ok_at")?
                .map(DateTime::<Utc>::from),
            last_error: row.try_get("last_error")?,
        })
    }
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for SetupRun {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> std::result::Result<Self, sqlx::Error> {
        Ok(Self {
            source_id: row.try_get("source_id")?,
            generation: row.try_get("generation")?,
            status: row.try_get("status")?,
            result: row.try_get("result")?,
            last_error: row.try_get("last_error")?,
            created_at: DateTime::<Utc>::from(row.try_get::<Ts, _>("created_at")?),
            finished_at: row
                .try_get::<Option<Ts>, _>("finished_at")?
                .map(DateTime::<Utc>::from),
        })
    }
}

/// Title first, then payload string values, with a hard UTF-8 byte bound. Keys,
/// numbers and other scalar values do not become searchable source content.
pub(crate) fn search_text(payload: &Value) -> String {
    const LIMIT: usize = 16 * 1024;
    fn append(out: &mut String, text: &str) {
        if text.is_empty() || out.len() == LIMIT {
            return;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        let mut end = text.len().min(LIMIT - out.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        out.push_str(&text[..end]);
    }
    fn visit(out: &mut String, value: &Value) {
        match value {
            Value::String(text) => append(out, text),
            Value::Array(values) => {
                for value in values {
                    visit(out, value);
                    if out.len() >= LIMIT - 3 {
                        break;
                    }
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    visit(out, value);
                    if out.len() >= LIMIT - 3 {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    let mut out = String::new();
    if let Value::Object(values) = payload {
        if let Some(Value::String(title)) = values.get("title") {
            append(&mut out, title);
        }
        for (key, value) in values {
            if key != "title" || !value.is_string() {
                visit(&mut out, value);
                if out.len() >= LIMIT - 3 {
                    break;
                }
            }
        }
    } else {
        visit(&mut out, payload);
    }
    out
}
