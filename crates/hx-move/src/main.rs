use clap::Parser;
use hx_move::{Options, move_postgres};
use std::{path::PathBuf, process::ExitCode};

#[derive(Parser)]
#[command(about = "Verify and copy frozen Postgres history into a new SQLite state directory")]
struct Args {
    #[arg(long, env = "HX_MOVE_POSTGRES_URL", hide_env_values = true)]
    postgres_url: String,
    /// Legacy data directory containing blobs/ (read only).
    #[arg(long)]
    source_data_dir: PathBuf,
    /// Must be absent or empty. No default.
    #[arg(long, env = "HX_STATE_DIR")]
    state_dir: PathBuf,
    /// Compatibility flag: every published blob is always fully hashed.
    #[arg(long)]
    verify_all_blobs: bool,
    /// Acknowledge an intentionally empty source database.
    #[arg(long)]
    allow_empty: bool,
}
#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    match move_postgres(&Options {
        postgres_url: args.postgres_url.clone(),
        source_data_dir: args.source_data_dir,
        state_dir: args.state_dir,
        verify_all_blobs: args.verify_all_blobs,
        allow_empty: args.allow_empty,
    })
    .await
    {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).expect("verification report")
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!(
                "Move refused: {}",
                hx_move::diagnostics::diagnostic(&error, &args.postgres_url)
            );
            ExitCode::FAILURE
        }
    }
}
