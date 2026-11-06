//! Source configuration belongs to core.sqlite. Agent-created tables alone use
//! agent.sqlite. Callers enforce owner/setup authorization and scrub secrets
//! before passing descriptions, recipes, add-ons or live payloads here.
use super::*;
use anyhow::ensure;
use hx_schema::{
    ConnectorInstance, Kind, ObservationId, ObservationTime, Operation, Revision, SourceKey,
};
use hx_sources::{
    recipe::Recipe,
    request::{MAX_HTTP_RESPONSE_BYTES, McpTransport, Request},
};
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
    pub identity_hash: String,
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
    pub name: String,
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

#[derive(Default)]
pub struct SourceConnection<'a> {
    pub probe: Option<&'a Request>,
    pub error: Option<&'a str>,
    pub cli: Option<&'a hx_sources::request::CliIdentity>,
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
        self.connect_source_with_key(name, description, history_days, None)
            .await
    }

    /// Publish setup only after its optional key is durably in the private key file.
    /// A crash before commit may leave an orphan key, never a runnable setup job.
    pub async fn connect_source_with_key(
        &self,
        name: &str,
        description: &str,
        history_days: u8,
        key: Option<&str>,
    ) -> Result<Source> {
        self.connect_source_checked(
            name,
            description,
            history_days,
            key,
            SourceConnection::default(),
        )
        .await
    }

    /// A deterministic failed preflight publishes a reconnectable source without
    /// a setup job. Authentication is fixed before any paid work is purchased.
    pub async fn connect_source_checked(
        &self,
        name: &str,
        description: &str,
        history_days: u8,
        key: Option<&str>,
        connection: SourceConnection<'_>,
    ) -> Result<Source> {
        let SourceConnection { probe, error, cli } = connection;
        ensure!(!name.trim().is_empty(), "source name required");
        ensure!(history_days <= 30, "history_days must be in 0..=30");
        let id = new_id();
        let connector = format!("source:{id}");
        let now = Ts::now();
        let mut tx = self.begin_write().await?;
        let mut source: Source = sqlx::query_as("INSERT INTO sources(id,connector_instance,name,description,history_days,connected_at) VALUES($1,$2,$3,$4,$5,$6) RETURNING *")
            .bind(&id).bind(connector).bind(name).bind(description).bind(i64::from(history_days)).bind(now).fetch_one(&mut *tx).await?;
        if let Some(cli) = cli {
            sqlx::query(
                "INSERT INTO source_cli(source_id,command,executable,home) VALUES($1,$2,$3,$4)",
            )
            .bind(&id)
            .bind(&cli.command)
            .bind(&cli.executable)
            .bind(
                cli.home
                    .to_str()
                    .ok_or_else(|| anyhow::anyhow!("invalid CLI HOME"))?,
            )
            .execute(&mut *tx)
            .await?;
        }
        if let Some(probe) = probe {
            sqlx::query("INSERT INTO source_checks(source_id,request) VALUES($1,$2)")
                .bind(&id)
                .bind(serde_json::to_value(probe)?)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(error) = error {
            sqlx::query("UPDATE sources SET status='needs_reconnect',last_error=$2 WHERE id=$1")
                .bind(&id)
                .bind(error)
                .execute(&mut *tx)
                .await?;
            source.status = "needs_reconnect".to_owned();
            source.last_error = Some(error.to_owned());
        } else {
            enqueue_setup(&mut tx, &id, 1, now).await?;
        }
        if let Some(key) = key {
            hx_sources::keys::KeyStore::new(&self.data_dir)?.write(&id, key)?;
        }
        tx.commit().await?;
        Ok(source)
    }

    pub async fn source_cli(&self, id: &str) -> Result<Option<hx_sources::request::CliIdentity>> {
        let row: Option<(String, String, String)> =
            sqlx::query_as("SELECT command,executable,home FROM source_cli WHERE source_id=$1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(
            |(command, executable, home)| hx_sources::request::CliIdentity {
                command,
                executable,
                home: home.into(),
            },
        ))
    }

    pub async fn prepare_source_request(
        &self,
        id: &str,
        request: &Request,
        host: &hx_sources::host::HostRunner,
    ) -> Result<(Request, hx_sources::host::HostRunner)> {
        request.validate_cli()?;
        let mut host = host.clone();
        let request = if let Some(cli) = self.source_cli(id).await? {
            host.home = cli.home.clone();
            cli.pin(request)?
        } else {
            request.clone()
        };
        Ok((request, host))
    }

    /// Refund exactly one fenced claim that never launched fx, and pause it
    /// durably. Reconnect reuses this job and the remaining real-launch budget.
    pub async fn pause_source_setup(
        &self,
        source: &str,
        generation: i64,
        job: i64,
        owner: &str,
        message: &str,
    ) -> Result<()> {
        let mut tx = self.begin_write().await?;
        let changed = sqlx::query("UPDATE jobs SET state='done',attempts=max(attempts-1,0),lease_owner=NULL,lease_until=NULL,last_error=$5 WHERE id=$1 AND lease_owner=$2 AND state='running' AND lease_until>$6 AND output_key=$3 AND kind='setup' AND EXISTS(SELECT 1 FROM sources WHERE id=$4 AND setup_generation=$7)")
            .bind(job).bind(owner).bind(format!("setup:{source}:{generation}")).bind(source).bind(message).bind(Ts::now()).bind(generation).execute(&mut *tx).await?.rows_affected();
        if changed == 1 {
            sqlx::query("UPDATE sources SET status='needs_reconnect',last_error=$2 WHERE id=$1 AND setup_generation=$3")
                .bind(source).bind(message).bind(generation).execute(&mut *tx).await?;
            sqlx::query("UPDATE setup_runs SET last_error=$3 WHERE source_id=$1 AND generation=$2 AND status='pending'")
                .bind(source).bind(generation).bind(message).execute(&mut *tx).await?;
        }
        tx.commit().await
    }

    pub async fn source_check(&self, id: &str) -> Result<Option<hx_sources::request::Request>> {
        let value: Option<sqlx::types::Json<hx_sources::request::Request>> =
            sqlx::query_scalar("SELECT request FROM source_checks WHERE source_id=$1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(value.map(|v| v.0))
    }

    /// Successful owner reconnect resumes recipes or the unused part of the
    /// current setup budget. Never reset attempts or create a new generation.
    pub async fn resume_source(&self, id: &str) -> Result<()> {
        let mut tx = self.begin_write().await?;
        let source: Source = sqlx::query_as("SELECT * FROM sources WHERE id=$1")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        ensure!(source.enabled, "source is off");
        let setup: Option<(String, String, i64, i64)> = sqlx::query_as("SELECT r.status,j.state,j.attempts,j.max_attempts FROM setup_runs r JOIN jobs j ON j.output_key='setup:' || r.source_id || ':' || r.generation WHERE r.source_id=$1 AND r.generation=$2")
            .bind(id).bind(source.setup_generation).fetch_optional(&mut *tx).await?;
        let (status, error) = match setup {
            None => {
                enqueue_setup(&mut tx, id, source.setup_generation, Ts::now()).await?;
                ("setting_up", None)
            }
            Some((status, _, _, _)) if status == "succeeded" => ("ready", None),
            Some((_, state, attempts, limit)) if attempts < limit => {
                if matches!(state.as_str(), "done" | "failed") {
                    sqlx::query("UPDATE jobs SET state='ready',available_at=$2,last_error=NULL,request_version=request_version+1 WHERE output_key=$1")
                        .bind(format!("setup:{id}:{}", source.setup_generation)).bind(Ts::now()).execute(&mut *tx).await?;
                }
                sqlx::query("UPDATE setup_runs SET status='pending',result=NULL,last_error=NULL,finished_at=NULL WHERE source_id=$1 AND generation=$2")
                    .bind(id).bind(source.setup_generation).execute(&mut *tx).await?;
                ("setting_up", None)
            }
            Some(_) => (
                "needs_attention",
                Some(
                    "Login restored. Setup's two attempts were used. Review the connection and use Run setup again on Agent setup.",
                ),
            ),
        };
        sqlx::query("UPDATE sources SET status=$2,last_error=$3 WHERE id=$1")
            .bind(id)
            .bind(status)
            .bind(error)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE recipe_state SET next_run_at=$2,last_error=NULL WHERE recipe_id IN (SELECT id FROM recipes WHERE source_id=$1)").bind(id).bind(Ts::now()).execute(&mut *tx).await?;
        tx.commit().await
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
        self.patch_source(id, Some(enabled), None).await
    }

    /// Rotate the file before committing switches. A crash cannot queue work;
    /// a completed file replacement is safe to repeat with the same key.
    pub async fn patch_source(
        &self,
        id: &str,
        enabled: Option<bool>,
        key: Option<&str>,
    ) -> Result<()> {
        let mut tx = self.begin_write().await?;
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sources WHERE id=$1)")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        ensure!(exists, "unknown source");
        if let Some(key) = key {
            hx_sources::keys::KeyStore::new(&self.data_dir)?.write(id, key)?;
        }
        if let Some(enabled) = enabled {
            sqlx::query("UPDATE sources SET enabled=$2 WHERE id=$1")
                .bind(id)
                .bind(enabled)
                .execute(&mut *tx)
                .await?;
            log_switch(&mut tx, "source", id, enabled).await?;
        }
        tx.commit().await?;
        if enabled == Some(false) {
            self.cancel_source_exchanges(id, None).await;
        }
        Ok(())
    }
    pub async fn recipe_exists(&self, id: &str) -> Result<bool> {
        Ok(
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM recipes WHERE id=$1)")
                .bind(id)
                .fetch_one(&self.pool)
                .await?,
        )
    }
    pub async fn addon_exists(&self, id: &str) -> Result<bool> {
        Ok(
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM addons WHERE id=$1)")
                .bind(id)
                .fetch_one(&self.pool)
                .await?,
        )
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
    /// one recipe, or an explicit no_pull). Old generations and off sources
    /// cannot activate configuration or become ready; their completion returns false.
    pub async fn finish_source_setup(
        &self,
        source: &str,
        generation: i64,
        result: Value,
        error: Option<&str>,
    ) -> Result<bool> {
        let result = stored_json(result);
        let mut tx = self.begin_write().await?;
        let updated = sqlx::query("UPDATE setup_runs SET status=$3,result=$4,last_error=$5,finished_at=$6 WHERE source_id=$1 AND generation=$2 AND status='pending' AND EXISTS(SELECT 1 FROM sources WHERE id=$1 AND setup_generation=$2 AND (enabled OR $5 IS NOT NULL))")
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
            // Every successful setup gets one immediate pull, including a
            // re-setup of an existing recipe. Off recipes remain untouched.
            sqlx::query("UPDATE recipe_state SET next_run_at=$2 WHERE recipe_id IN (SELECT id FROM recipes WHERE source_id=$1 AND enabled AND NOT owner_off)")
                .bind(source).bind(Ts::now()).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    /// Setup-only save. HTTP identity is source + method + normalized URL path;
    /// query, headers, body and display name do not create a new owner switch.
    /// A genuinely different path (or CLI argv / MCP tool) is a different
    /// pull. This is an identity boundary, not a detector for equivalent APIs.
    /// A re-save never writes enabled or owner_off, or resets the pull cursor.
    pub async fn save_recipe(&self, source: &str, recipe: &Recipe) -> Result<StoredRecipe> {
        self.save_recipe_for_setup(source, recipe, None).await
    }
    pub async fn save_recipe_for_setup(
        &self,
        source: &str,
        recipe: &Recipe,
        generation: Option<i64>,
    ) -> Result<StoredRecipe> {
        recipe.validate()?;
        let cli = self.source_cli(source).await?;
        for request in
            std::iter::once(&recipe.request).chain(recipe.detail.iter().map(|d| &d.request))
        {
            request.validate_cli()?;
            if let Some(cli) = &cli {
                cli.pin(request)?;
            }
        }
        let mut recipe = recipe.clone();
        recipe.source = source.to_owned();
        let mut identity_hash = recipe_identity(&recipe)?;
        let value = stored_json(serde_json::to_value(&recipe)?);
        let id = new_id();
        let mut tx = self.begin_write().await?;
        ensure_source_on(&mut tx, source).await?;
        ensure_setup_generation(&mut tx, source, generation).await?;
        // Older CLI rows used their display name in the hash. Compare the saved
        // requests under the current rules, retaining the row, switch and cursor.
        // If older code already made duplicates, an owner-off row wins.
        let existing: Vec<StoredRecipe> =
            sqlx::query_as("SELECT * FROM recipes WHERE source_id=$1 ORDER BY owner_off DESC,id")
                .bind(source)
                .fetch_all(&mut *tx)
                .await?;
        for saved in existing {
            if recipe_identity(&saved.recipe.0)? == identity_hash {
                identity_hash = saved.identity_hash;
                break;
            }
        }
        let row: StoredRecipe = sqlx::query_as("INSERT INTO recipes(id,source_id,name,identity_hash,recipe) VALUES($1,$2,$3,$4,$5) ON CONFLICT(source_id,identity_hash) DO UPDATE SET name=EXCLUDED.name,recipe=EXCLUDED.recipe RETURNING *")
            .bind(id).bind(source).bind(&recipe.name).bind(identity_hash).bind(value).fetch_one(&mut *tx).await?;
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

    /// Read again before each page; queued jobs are never switch authority.
    pub async fn recipe(&self, id: &str) -> Result<Option<StoredRecipe>> {
        Ok(sqlx::query_as("SELECT * FROM recipes WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// One durable slot per recipe. Repeated scheduler ticks neither multiply
    /// missed boundaries nor steal a running lease or refill retry attempts.
    pub async fn schedule_pulls(&self, now: DateTime<Utc>) -> Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("INSERT INTO jobs(output_key,kind,payload,available_at) SELECT 'pull:' || r.id,'pull',json_object('recipe',r.id),$1 FROM recipes r JOIN sources s ON s.id=r.source_id JOIN recipe_state rs ON rs.recipe_id=r.id WHERE r.enabled AND NOT r.owner_off AND s.enabled AND s.status='ready' AND rs.next_run_at<=$1 ON CONFLICT(output_key) DO UPDATE SET state='ready',attempts=0,available_at=EXCLUDED.available_at,last_error=NULL,request_version=jobs.request_version+1 WHERE jobs.state IN ('done','failed')")
            .bind(Ts::from(now)).execute(&mut *tx).await?;
        tx.commit().await
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

    /// Finish a pull atomically: a crash cannot advance its boundary without
    /// also recording reconnect/error status. Call only after ingest commits.
    pub async fn finish_recipe_pull(
        &self,
        source: &str,
        state: &RecipeState,
        at: DateTime<Utc>,
        reconnect: bool,
    ) -> Result<()> {
        let cursor = state.cursor.clone().map(stored_json);
        let mut tx = self.begin_write().await?;
        let updated = sqlx::query("UPDATE recipe_state SET cursor=$2,next_run_at=$3,last_ok_at=$4,last_error=$5 WHERE recipe_id=$1 AND EXISTS(SELECT 1 FROM recipes WHERE id=$1 AND source_id=$6)")
            .bind(&state.recipe_id).bind(cursor).bind(Ts::from(state.next_run_at)).bind(state.last_ok_at.map(Ts::from)).bind(&state.last_error).bind(source).execute(&mut *tx).await?;
        ensure!(updated.rows_affected() == 1, "unknown source recipe");
        let updated = sqlx::query("UPDATE sources SET last_pull_at=$2,last_error=$3,status=CASE WHEN $4 THEN 'needs_reconnect' ELSE status END WHERE id=$1")
            .bind(source).bind(Ts::from(at)).bind(&state.last_error).bind(reconnect).execute(&mut *tx).await?;
        ensure!(updated.rows_affected() == 1, "unknown source");
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

    /// Setup-only save. A stable name owns the switch, even when notes change.
    /// A genuinely different name is a separate add-on; we do not detect prose
    /// equivalence. Surrounding whitespace in the name is not significant.
    pub async fn save_addon(&self, source: &str, name: &str, text: &str) -> Result<Addon> {
        self.save_addon_for_setup(source, name, text, None).await
    }
    pub async fn save_addon_for_setup(
        &self,
        source: &str,
        name: &str,
        text: &str,
        generation: Option<i64>,
    ) -> Result<Addon> {
        let name = name.trim();
        ensure!(!name.is_empty(), "add-on name required");
        ensure!(
            !text.trim().is_empty() && text.len() <= 1000,
            "add-on must contain 1..=1000 bytes"
        );
        let id = new_id();
        let mut tx = self.begin_write().await?;
        ensure_source_on(&mut tx, source).await?;
        ensure_setup_generation(&mut tx, source, generation).await?;
        let existing: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM addons WHERE source_id=$1 AND name=$2)",
        )
        .bind(source)
        .bind(name)
        .fetch_one(&mut *tx)
        .await?;
        if !existing {
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM addons WHERE source_id=$1")
                .bind(source)
                .fetch_one(&mut *tx)
                .await?;
            ensure!(count < 3, "at most three add-ons per source");
        }
        let row = sqlx::query_as("INSERT INTO addons(id,source_id,name,text) VALUES($1,$2,$3,$4) ON CONFLICT(source_id,name) DO UPDATE SET text=EXCLUDED.text RETURNING *")
            .bind(id).bind(source).bind(name).bind(text).fetch_one(&mut *tx).await?;
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
        let recipe: Option<StoredRecipe> = if kind == "recipe" && !enabled {
            sqlx::query_as("SELECT * FROM recipes WHERE id=$1")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
        } else {
            None
        };
        let updated = sqlx::query(sql)
            .bind(id)
            .bind(enabled)
            .execute(&mut *tx)
            .await?;
        ensure!(updated.rows_affected() == 1, "unknown switch target");
        log_switch(&mut tx, kind, id, enabled).await?;
        tx.commit().await?;
        if let Some(recipe) = recipe {
            self.cancel_source_exchanges(
                &recipe.source_id,
                Some((id, &recipe_identity(&recipe.recipe.0)?)),
            )
            .await;
        }
        Ok(())
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
        let bytes = serde_json::to_vec(&payload)?;
        ensure!(
            bytes.len() <= MAX_HTTP_RESPONSE_BYTES,
            "live payload exceeds 8 MiB"
        );
        let revision = Revision::digest(&bytes);
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

// HTTP URL parsing resolves dot segments, supplies '/' for an empty path and
// excludes queries/fragments. Decode only unreserved ASCII and canonicalize
// other percent escapes; encoded path separators remain significant.
fn normalized_path(address: &str) -> Result<String> {
    let url = url::Url::parse(address).map_err(|_| anyhow::anyhow!("invalid recipe URL"))?;
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "HTTP recipe URL required"
    );
    let bytes = url.path().as_bytes();
    let mut out = String::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && at + 2 < bytes.len()
            && let Ok(value) = u8::from_str_radix(&url.path()[at + 1..at + 3], 16)
        {
            if value.is_ascii_alphanumeric() || b"-._~".contains(&value) {
                out.push(char::from(value));
            } else {
                use std::fmt::Write;
                write!(out, "%{value:02X}")?;
            }
            at += 3;
        } else {
            out.push(char::from(bytes[at]));
            at += 1;
        }
    }
    // `/m/` and `/m` are the same pull to an owner who switched it off.
    let trimmed = out.trim_end_matches('/');
    Ok(if trimmed.is_empty() {
        "/".into()
    } else {
        trimmed.to_owned()
    })
}

pub fn recipe_identity(recipe: &Recipe) -> Result<String> {
    let executable = |argv: &[String]| -> Result<String> {
        Ok(argv
            .first()
            .filter(|s| !s.is_empty())
            .context("recipe executable required")?
            .clone())
    };
    let identity = match &recipe.request {
        Request::Http { method, url, .. } => {
            ensure!(!method.trim().is_empty(), "recipe method required");
            json!([
                "http",
                method.trim().to_ascii_uppercase(),
                normalized_path(url)?
            ])
        }
        // Environment changes, like HTTP headers/body, retain the switch.
        // In particular ignored/overridden env vars must not bypass an off.
        // The display name, mapping and cadence are never request identity.
        Request::Cli { argv, .. } => json!(["cli", argv]),
        Request::Mcp {
            transport, tool, ..
        } => match transport {
            McpTransport::Http { url } => json!(["mcp_http", normalized_path(url)?, tool]),
            McpTransport::Stdio { argv } => json!(["mcp_stdio", executable(argv)?, tool]),
        },
    };
    Ok(hash(&serde_json::to_vec(&identity)?))
}

#[derive(Debug)]
pub struct SourceAccessChanged;
impl std::fmt::Display for SourceAccessChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("owner disabled an input source")
    }
}
impl std::error::Error for SourceAccessChanged {}

async fn ensure_setup_generation(
    tx: &mut WriteTransaction,
    source: &str,
    generation: Option<i64>,
) -> Result<()> {
    if let Some(generation) = generation {
        let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sources s JOIN setup_runs r ON r.source_id=s.id AND r.generation=s.setup_generation WHERE s.id=$1 AND s.setup_generation=$2 AND r.status='pending')")
            .bind(source).bind(generation).fetch_one(&mut **tx).await?;
        ensure!(valid, "setup superseded or finished");
    }
    Ok(())
}
