//! Scheduling and interpretation persistence, with existing transaction boundaries.
use super::*;

pub struct InterpretationGuard {
    _lock: crate::locks::FileLock,
}
#[derive(Debug, sqlx::FromRow)]
pub struct SettledGeneration {
    pub generation: i64,
    #[sqlx(try_from = "Ts")]
    pub ready_at: DateTime<Utc>,
}
impl Store {
    pub async fn schedule_interpretations(
        &self,
        w: Interval,
        generation: i64,
        state: &InterpretationState,
        settle_s: i64,
    ) -> Result<()> {
        let mut tx = self.begin_write().await?;
        if state.canonical_revision.is_none() && Utc::now() < w.end + Duration::hours(24) {
            request_interpret_job(
                &mut tx,
                w,
                generation,
                false,
                w.end
                    .max(state.input_changed_at + Duration::seconds(settle_s)),
            )
            .await?;
        }
        if state.canonical_revision.is_some() {
            // A previously deferred first run is obsolete once any cadence
            // has published. Preserve active leases and daily sweep requests.
            sqlx::query("UPDATE jobs SET state='done' WHERE output_key=$1 AND state='ready' AND NOT (json_type(payload,'$.sweep_day') IS NOT NULL)")
                        .bind(format!("interpret:{}", w.start)).execute(&mut *tx).await?;
        }
        if !state.revision24_finished
                    // Allow brief downtime around T+24h, never historical catch-up.
                    && Utc::now() < w.end + Duration::hours(48)
        {
            request_interpret_job(&mut tx, w, generation, true, w.end + Duration::hours(24))
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn schedule_compilations(&self) -> Result<()> {
        let rows=sqlx::query("SELECT start_at,end_at,generation FROM windows WHERE dirty OR (end_at<=$1 AND canonical_revision IS NULL AND empty_generation IS DISTINCT FROM generation) ORDER BY start_at DESC LIMIT 1000").bind(Ts::now()).fetch_all(&self.pool).await?;
        let mut tx = self.begin_write().await?;
        for r in rows {
            let w = Interval {
                start: DateTime::<Utc>::from(r.get::<Ts, _>("start_at")),
                end: DateTime::<Utc>::from(r.get::<Ts, _>("end_at")),
            };
            let generation: i64 = r.get("generation");
            request_window_job(&mut tx, "compile", w, generation, Ts::now()).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn try_interpretation_guard(
        &self,
        window: Interval,
    ) -> Result<Option<InterpretationGuard>> {
        Ok(self
            .try_file_lock(&format!("interpret-{}", window.start.to_rfc3339()))
            .await?
            .map(|lock| InterpretationGuard { _lock: lock }))
    }
    pub async fn settled_generation(
        &self,
        start: DateTime<Utc>,
        settle_s: f64,
    ) -> Result<SettledGeneration> {
        Ok(sqlx::query_as("SELECT generation,max(end_at,input_changed_at+$2) AS ready_at FROM windows WHERE start_at=$1").bind(Ts::from(start)).bind((settle_s * 1_000_000.0) as i64).fetch_one(&self.pool).await?)
    }
    pub async fn revision_exists(
        &self,
        start: DateTime<Utc>,
        generation: i64,
        identity: &Value,
    ) -> Result<bool> {
        let manifests: Vec<Value> = sqlx::query_scalar("SELECT interpretation->'input_manifest' FROM revisions WHERE start_at=$1 AND input_generation=$2 AND json_type(interpretation,'$.input_manifest')='object'")
            .bind(Ts::from(start)).bind(generation).fetch_all(&self.pool).await?;
        Ok(manifests
            .iter()
            .any(|stored| crate::persistence::json_contains(stored, identity)))
    }

    pub async fn interpretation_ready_at(
        &self,
        start: DateTime<Utc>,
        revision24: bool,
        settle_s: f64,
    ) -> Result<DateTime<Utc>> {
        Ok(DateTime::<Utc>::from(sqlx::query_scalar::<_, Ts>(
                    "SELECT max(end_at+CASE WHEN $2 THEN 86400000000 ELSE 0 END,input_changed_at+$3) FROM windows WHERE start_at=$1",
                ).bind(Ts::from(start)).bind(revision24).bind((settle_s * 1_000_000.0) as i64).fetch_one(&self.pool).await?))
    }
    pub async fn cached_timeline(
        &self,
        start: DateTime<Utc>,
        generation: i64,
        compiler_version: &str,
    ) -> Result<Option<Value>> {
        Ok(sqlx::query_scalar("SELECT timeline FROM windows WHERE start_at=$1 AND generation=$2 AND compiled_generation=$2 AND compiler_version=$3 AND timeline IS NOT NULL AND timeline_hash IS NOT NULL").bind(Ts::from(start)).bind(generation).bind(compiler_version).fetch_optional(&self.pool).await?)
    }
}
