//! Menu bar entrypoint; AppKit and Objective-C lifecycle code stay in hx-macos-sys.
#[cfg(not(feature = "e2e-harness"))]
pub fn run() -> anyhow::Result<()> {
    if !std::env::args_os().any(|arg| arg == "--managed") {
        match hx_agent::startup::install() {
            Ok(()) => return Ok(()),
            // Opening the app must still record, even when login startup is off.
            // The singleton socket stops a second copy if launchd did start one.
            Err(error) => eprintln!("recording without login startup: {error}"),
        }
    }
    hx_agent::run(hx_agent::RunOptions {
        menu_bar: true,
        ..Default::default()
    })
}

#[cfg(feature = "e2e-harness")]
pub fn run() -> anyhow::Result<()> {
    hx_agent::harness::main()
}
