//! Prints the interpreter digest for a window (read-only): cargo run --example digest_probe -- FROM TO
use sqlx::Row;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let window = hx_schema::Interval {
        start: a[1].parse()?,
        end: a[2].parse()?,
    };
    let pool = sqlx::PgPool::connect(&std::env::var("DATABASE_URL")?).await?;
    let rows = sqlx::query("SELECT id,received_at,envelope FROM events WHERE source_start<$2 AND coalesce(source_end,source_start)>=$1 AND kind NOT IN ('media.still','media.chunk','policy.gap') ORDER BY source_start,id")
        .bind(window.start).bind(window.end).fetch_all(&pool).await?;
    let obs: Vec<_> = rows
        .iter()
        .map(|r| hx_compile::Observation {
            id: r.get("id"),
            received_at: r.get("received_at"),
            envelope: serde_json::from_value(r.get("envelope")).unwrap(),
        })
        .collect();
    let timeline = hx_compile::compile(&obs, window);
    let d = hx_interpret::build_digest(&timeline, &obs, hx_interpret::DigestConfig::default());
    print!("{}", d.text);
    eprintln!(
        "records {} | segments {} | chars {} | ~tokens {}",
        obs.len(),
        d.segments.len(),
        d.text.len(),
        d.text.len() / 4
    );
    Ok(())
}
