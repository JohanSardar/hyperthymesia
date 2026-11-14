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
    Pair {
        link: String,
        /// Replace saved credentials; revoke this device's previous server token first when re-pairing.
        #[arg(long)]
        replace: bool,
    },
    Status,
    /// Recognize the foreground screen once; output text JSON only, without recording.
    ScreenText {
        /// Replay an existing image with its accessibility sample from a JSON record.
        #[arg(long)]
        replay: Option<std::path::PathBuf>,
    },
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
        Command::Pair { link, replace } => {
            tokio::runtime::Runtime::new()?.block_on(hx_agent::pair::redeem(
                &link,
                replace,
                &hx_agent::config::support_dir(),
            ))?;
            println!("Paired. Uploads will resume automatically.");
            Ok(())
        }
        Command::ScreenText { replay } => {
            let value = match replay {
                Some(path) => {
                    hx_agent::screen_text::replay(&serde_json::from_slice(&std::fs::read(path)?)?)?
                }
                None => hx_agent::screen_text::capture_once()?,
            };
            println!("{}", serde_json::to_string_pretty(&value)?);
            Ok(())
        }
        Command::Status => {
            println!("{}", serde_json::to_string_pretty(&hx_agent::status()?)?);
            Ok(())
        }
        Command::Onboard => {
            anyhow::ensure!(
                !cfg!(feature = "e2e-harness"),
                "e2e-harness cannot request permissions"
            );
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
