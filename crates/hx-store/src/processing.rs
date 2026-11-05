//! Scheduling and interpretation persistence, with existing transaction boundaries.
use super::*;

pub struct InterpretationGuard {
    _lock: crate::locks::FileLock,
}
#[derive(Debug, sqlx::FromRow)]
pub struct SettledGeneration {
    pub generation: i64,
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
        let mut tx = self.pool.begin().await?;
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
            sqlx::query("UPDATE jobs SET state='done' WHERE output_key=$1 AND state='ready' AND NOT (payload ? 'sweep_day')")
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
        let rows=sqlx::query("SELECT start_at,end_at,generation FROM windows WHERE dirty OR (end_at<=now() AND canonical_revision IS NULL AND empty_generation IS DISTINCT FROM generation) ORDER BY start_at DESC LIMIT 1000").fetch_all(&self.pool).await?;
        let mut tx = self.pool.begin().await?;
        for r in rows {
            let w = Interval {
                start: r.get("start_at"),
                end: r.get("end_at"),
            };
            let generation: i64 = r.get("generation");
            request_window_job(&mut tx, "compile", w, generation).await?;
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
        Ok(sqlx::query_as("SELECT generation,greatest(end_at,input_changed_at+make_interval(secs=>$2::double precision)) AS ready_at FROM windows WHERE start_at=$1").bind(start).bind(settle_s).fetch_one(&self.pool).await?)
    }
    pub async fn revision_exists(
        &self,
        start: DateTime<Utc>,
        generation: i64,
        identity: &Value,
    ) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM revisions WHERE start_at=$1 AND input_generation=$2 AND interpretation->'input_manifest' @> $3)").bind(start).bind(generation).bind(identity).fetch_one(&self.pool).await?)
    }
    pub async fn interpretation_ready_at(
        &self,
        start: DateTime<Utc>,
        revision24: bool,
        settle_s: f64,
    ) -> Result<DateTime<Utc>> {
        Ok(sqlx::query_scalar(
                    "SELECT greatest(end_at+CASE WHEN $2 THEN interval '24 hours' ELSE interval '0' END,input_changed_at+make_interval(secs=>$3::double precision)) FROM windows WHERE start_at=$1",
                ).bind(start).bind(revision24).bind(settle_s).fetch_one(&self.pool).await?)
    }
    pub async fn cached_timeline(
        &self,
        start: DateTime<Utc>,
        generation: i64,
        compiler_version: &str,
    ) -> Result<Option<Value>> {
        Ok(sqlx::query_scalar("SELECT timeline FROM windows WHERE start_at=$1 AND generation=$2 AND compiled_generation=$2 AND compiler_version=$3 AND timeline IS NOT NULL AND timeline_hash IS NOT NULL").bind(start).bind(generation).bind(compiler_version).fetch_optional(&self.pool).await?)
    }
}
