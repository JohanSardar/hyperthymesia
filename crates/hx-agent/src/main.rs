use clap::{Parser, Subcommand};
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Run {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        duration_s: Option<u64>,
    },
    Status,
    Onboard,
    NmHost,
    InstallNmHost {
        #[arg(long)]
        extension_id: String,
    },
}
fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Run {
            dry_run,
            duration_s,
        } => hx_agent::run(hx_agent::RunOptions {
            dry_run,
            duration_s,
            ..Default::default()
        }),
        Command::Status => {
            println!("{}", serde_json::to_string_pretty(&hx_agent::status()?)?);
            Ok(())
        }
        Command::Onboard => {
            println!(
                "{}",
                serde_json::to_string_pretty(&hx_macos_sys::onboard())?
            );
            Ok(())
        }
        Command::NmHost => hx_agent::native_messaging::run(),
        Command::InstallNmHost { extension_id } => {
            println!(
                "{}",
                hx_agent::native_messaging::install(&extension_id)?.display()
            );
            Ok(())
        }
    }
}
