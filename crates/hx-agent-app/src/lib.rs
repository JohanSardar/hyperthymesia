//! Menu bar entrypoint; AppKit and Objective-C lifecycle code stay in hx-macos-sys.
pub fn run() -> anyhow::Result<()> {
    if !std::env::args_os().any(|arg| arg == "--managed") {
        return hx_agent::startup::install();
    }
    hx_agent::run(hx_agent::RunOptions {
        menu_bar: true,
        ..Default::default()
    })
}
