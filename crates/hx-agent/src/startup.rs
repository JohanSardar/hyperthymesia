//! Per-user launch supervision. Only the app entrypoint calls `install`.
use anyhow::{Context, Result, ensure};
use std::{path::Path, process::Command};
pub const LABEL: &str = "com.hyperthymesia.agent";

pub fn launch_agent(executable: &Path) -> Result<String> {
    let executable = executable.to_str().context("app path is not UTF-8")?;
    let escaped = executable
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;");
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>{LABEL}</string>
<key>ProgramArguments</key><array><string>{escaped}</string><string>--managed</string></array>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
<key>LimitLoadToSessionType</key><string>Aqua</string>
<key>ThrottleInterval</key><integer>30</integer>
</dict></plist>
"#
    ))
}

/// Returns after handing this manual launch to launchd. The managed process runs
/// capture; this launcher never opens data or races the managed singleton socket.
pub fn install() -> Result<()> {
    ensure!(
        !cfg!(feature = "e2e-harness"),
        "e2e-harness cannot register login startup"
    );
    let exe = std::env::current_exe()?.canonicalize()?;
    let bundle = exe
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .filter(|p| p.extension().is_some_and(|ext| ext == "app"))
        .context("launch the built Hyperthymesia.app bundle")?;
    let uid = Command::new("/usr/bin/id").arg("-u").output()?;
    ensure!(uid.status.success(), "could not determine login session");
    let domain = format!("gui/{}", String::from_utf8(uid.stdout)?.trim());
    // A certificate-backed bundle can use the modern, relocation-aware service.
    // Ad-hoc builds have no TeamIdentifier and use the user LaunchAgents folder.
    let signature = Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=4"])
        .arg(bundle)
        .output()?;
    let signed = signature.status.success()
        && String::from_utf8_lossy(&signature.stderr)
            .lines()
            .any(|line| line.starts_with("TeamIdentifier=") && line != "TeamIdentifier=not set");
    let modern = signed
        && hx_macos_sys::register_launch_agent(&format!("{LABEL}.plist"))
            .map_err(anyhow::Error::msg)?;
    if !modern {
        let home = std::env::var_os("HOME").context("HOME is not set")?;
        let dir = Path::new(&home).join("Library/LaunchAgents");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{LABEL}.plist"));
        let plist = launch_agent(&exe)?;
        let changed = std::fs::read(&path).ok().as_deref() != Some(plist.as_bytes());
        if changed {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let tmp = dir.join(format!(".{LABEL}.{}.tmp", std::process::id()));
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            file.write_all(plist.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(tmp, &path)?;
        }
        let job = format!("{domain}/{LABEL}");
        let loaded = Command::new("/bin/launchctl")
            .args(["print", &job])
            .output()?
            .status
            .success();
        if changed && loaded {
            let output = Command::new("/bin/launchctl")
                .args(["bootout", &job])
                .output()?;
            ensure!(
                output.status.success(),
                "could not update login startup: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        if changed || !loaded {
            let output = Command::new("/bin/launchctl")
                .arg("bootstrap")
                .arg(&domain)
                .arg(&path)
                .output()?;
            ensure!(
                output.status.success(),
                "could not enable login startup: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    // A successful Quit leaves the job loaded but stopped. A new manual launch
    // explicitly starts it again without killing an already running instance.
    let output = Command::new("/bin/launchctl")
        .args(["kickstart", &format!("{domain}/{LABEL}")])
        .output()?;
    ensure!(
        output.status.success(),
        "could not start recording: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
