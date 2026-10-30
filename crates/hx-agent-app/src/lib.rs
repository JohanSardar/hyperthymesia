//! Menu bar entrypoint; AppKit and all Objective-C lifecycle code stay in hx-macos-sys.
pub fn run() -> anyhow::Result<()> {
    hx_agent::run(hx_agent::RunOptions {
        menu_bar: true,
        ..Default::default()
    })
}
