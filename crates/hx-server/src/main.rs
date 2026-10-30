use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use hx_schema::Interval;
use hx_server::processing::{self, ProviderConfig, Runner};
use hx_store::Store;
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
struct Cli {
    #[arg(
        long,
        env = "DATABASE_URL",
        default_value = "postgres://hx:hx@localhost:55432/hx"
    )]
    database_url: String,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Migrate,
    Serve {
        #[arg(long, default_value = "127.0.0.1:7377")]
        listen: SocketAddr,
        #[arg(long)]
        data_dir: PathBuf,
    },
    Token {
        #[command(subcommand)]
        command: TokenCommand,
    },
    Compile {
        #[arg(long)]
        from: DateTime<Utc>,
        #[arg(long)]
        to: DateTime<Utc>,
    },
    Interpret {
        #[arg(long)]
        from: DateTime<Utc>,
        #[arg(long)]
        to: DateTime<Utc>,
    },
    Gc {
        #[arg(long)]
        data_dir: PathBuf,
    },
}
#[derive(Subcommand)]
enum TokenCommand {
    Create {
        #[arg(long)]
        name: String,
        #[arg(long, required = true)]
        prefix: Vec<String>,
        #[arg(long, value_delimiter = ',', required = true)]
        scopes: Vec<String>,
    },
}
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    let dir = match &cli.command {
        Command::Serve { data_dir, .. } | Command::Gc { data_dir } => data_dir.clone(),
        _ => PathBuf::from("."),
    };
    let store = Store::connect(&cli.database_url, dir).await?;
    match cli.command {
        Command::Migrate => store.migrate().await?,
        Command::Token {
            command:
                TokenCommand::Create {
                    name,
                    prefix,
                    scopes,
                },
        } => println!("{}", store.create_token(&name, prefix, scopes).await?),
        Command::Gc { .. } => println!("Expired {} blobs", store.gc().await?),
        Command::Compile { from, to } => {
            for w in windows(from, to)? {
                processing::compile_window(&store, w).await?;
            }
        }
        Command::Interpret { from, to } => {
            for w in windows(from, to)? {
                processing::interpret_window(
                    &store,
                    w,
                    &processing::coordinator_provider(ProviderConfig::from_env()),
                )
                .await?;
            }
        }
        Command::Serve { listen, .. } => {
            if !listen.ip().is_loopback() {
                bail!("listen must be loopback; use a local Tailscale proxy");
            }
            let listener = tokio::net::TcpListener::bind(listen).await?;
            let runner = Runner {
                store: store.clone(),
                provider: processing::coordinator_provider(ProviderConfig::from_env()),
            };
            let worker = tokio::spawn(async move {
                let mut schedule = tokio::time::interval(std::time::Duration::from_secs(60));
                loop {
                    tokio::select! {
                        _=schedule.tick()=>{if let Err(e)=processing::schedule(&runner.store).await{tracing::error!("scheduler failed: {e:#}");}},
                        _=tokio::time::sleep(std::time::Duration::from_millis(200))=>{
                            if let Err(e)=hx_jobs::run_one(&runner.store.pool,&runner).await{tracing::error!("processing failed: {e:#}");}
                        }
                    }
                }
            });
            let result = axum::serve(listener, hx_server::router(store))
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await;
            worker.abort();
            result?;
        }
    }
    Ok(())
}
fn windows(from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<Interval>> {
    if to <= from || to - from > chrono::Duration::days(366) {
        bail!("invalid range");
    }
    let mut result = vec![];
    let mut w = hx_store::window_at(from);
    while w.start < to {
        result.push(w);
        w = hx_store::window_at(w.end);
    }
    Ok(result)
}
