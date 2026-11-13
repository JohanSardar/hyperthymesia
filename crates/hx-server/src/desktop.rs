//! Same-user desktop sign-in; pairing credentials remain ingest-only.
use anyhow::Result;
use hx_store::Store;
use serde_json::json;
use std::{io::Write, net::SocketAddr};

/// The native app can already read this owner's private state folder. Give it
/// the same reader access as guided setup, without promoting a device token.
pub async fn prepare_session(store: &Store, listen: SocketAddr) -> Result<()> {
    let path = store.data_dir.join("desktop-session.json");
    let server_url = format!("http://{listen}");
    if let Ok(bytes) = std::fs::read(&path)
        && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && value["server_url"] == server_url
        && let Some(secret) = value["token"].as_str()
        && store
            .authenticate(secret)
            .await?
            .is_some_and(|t| t.has("read_raw"))
    {
        return Ok(());
    }
    const NAME: &str = "desktop app (local owner)";
    for token in store.list_tokens().await? {
        if token.name == NAME && !token.revoked {
            store.revoke_token(&token.id).await?;
        }
    }
    let secret = store
        .create_token(
            NAME,
            ["source:", "macos-agent:", "macos-media:", "companion:"]
                .map(String::from)
                .to_vec(),
            vec!["read_raw".into(), "read_l3".into()],
        )
        .await?;
    // NamedTempFile is owner-only from creation; replacement is atomic.
    let mut file = tempfile::NamedTempFile::new_in(&store.data_dir)?;
    file.write_all(
        serde_json::to_string(&json!({"server_url":server_url,"token":secret}))?.as_bytes(),
    )?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}
