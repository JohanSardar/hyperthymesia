use crate::config::support_dir;
use anyhow::{Result, ensure};
use hx_schema::CompanionEvent;
use std::io::{Read, Write};
pub const MAX_FRAME: usize = 1024 * 1024;
pub fn read_frame(reader: &mut impl Read) -> Result<Option<CompanionEvent>> {
    let mut length = [0; 4];
    let n = reader.read(&mut length[..1])?;
    if n == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut length[1..])?;
    let length = u32::from_le_bytes(length) as usize;
    ensure!(
        length > 0 && length <= MAX_FRAME,
        "invalid native message length"
    );
    let mut data = vec![0; length];
    reader.read_exact(&mut data)?;
    Ok(Some(serde_json::from_slice(&data)?))
}
pub fn reply(writer: &mut impl Write, value: &serde_json::Value) -> Result<()> {
    let data = serde_json::to_vec(value)?;
    writer.write_all(&(data.len() as u32).to_le_bytes())?;
    writer.write_all(&data)?;
    writer.flush()?;
    Ok(())
}
pub fn run() -> Result<()> {
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    while let Some(event) = read_frame(&mut input)? {
        let result = (|| -> Result<()> {
            let mut socket =
                std::os::unix::net::UnixStream::connect(support_dir().join("agent.sock"))?;
            socket.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
            socket.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
            serde_json::to_writer(&mut socket, &event)?;
            socket.write_all(b"\n")?;
            let mut ack = String::new();
            std::io::BufRead::read_line(&mut std::io::BufReader::new(socket), &mut ack)?;
            ensure!(
                serde_json::from_str::<serde_json::Value>(&ack)?["ok"] == true,
                "agent declined event"
            );
            Ok(())
        })();
        reply(
            &mut output,
            &match result {
                Ok(()) => serde_json::json!({"ok":true}),
                Err(e) => serde_json::json!({"ok":false,"error":e.to_string()}),
            },
        )?;
    }
    Ok(())
}
pub fn install(extension_id: &str) -> Result<std::path::PathBuf> {
    ensure!(
        extension_id.len() == 32 && extension_id.bytes().all(|b| (b'a'..=b'p').contains(&b)),
        "Chrome extension ID must be 32 letters a-p"
    );
    let dir = support_dir();
    std::fs::create_dir_all(&dir)?;
    // A stable launcher preserves nm-host arguments Chrome cannot put in its manifest.
    let exe = std::env::current_exe()?;
    let launcher = dir.join("nm-host");
    let quoted = exe.to_string_lossy().replace('\'', "'\\''");
    std::fs::write(&launcher, format!("#!/bin/sh\nexec '{quoted}' nm-host\n"))?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700))?;
    let manifest_dir = std::path::PathBuf::from(std::env::var_os("HOME").expect("HOME"))
        .join("Library/Application Support/Google/Chrome/NativeMessagingHosts");
    std::fs::create_dir_all(&manifest_dir)?;
    let path = manifest_dir.join("com.hyperthymesia.agent.json");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(
            &serde_json::json!({"name":"com.hyperthymesia.agent","description":"Hyperthymesia local capture agent","path":launcher,"type":"stdio","allowed_origins":[format!("chrome-extension://{extension_id}/")]}),
        )?,
    )?;
    Ok(path)
}
