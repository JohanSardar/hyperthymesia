//! Prints the digest the fx interpreter would receive for a window (read-only):
//! cargo run --example digest_probe -- FROM TO
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let window = hx_schema::Interval {
        start: a[1].parse()?,
        end: a[2].parse()?,
    };
    let scratch = tempfile::tempdir()?;
    let store = hx_store::Store::connect(&std::env::var("DATABASE_URL")?, scratch.path()).await?;
    let broker = hx_server::mcp::Broker::new(
        store,
        hx_server::disclosure::DisclosurePolicy::from_env()?,
        hx_server::processing::SensitiveEgress::from_env(),
        window,
    );
    let text = broker.digest().await?;
    print!("{text}");
    eprintln!("chars {} | ~tokens {}", text.len(), text.len() / 4);
    Ok(())
}
