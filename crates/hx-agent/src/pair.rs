//! Pairing never writes a config until redemption succeeds, and never logs secrets.
use crate::config::Config;
use anyhow::{Context, Result, ensure};
use hx_schema::protocol::{PairRequest, PairResponse};
use reqwest::Url;
use std::{path::Path, time::Duration};

fn parse_link(link: &str) -> Result<(Url, String)> {
    let link = Url::parse(link).context("invalid pairing link")?;
    ensure!(
        link.scheme() == "hyperthymesia"
            && link.host_str() == Some("pair")
            && link.username().is_empty()
            && link.password().is_none()
            && link.port().is_none()
            && ["", "/"].contains(&link.path())
            && link.fragment().is_none(),
        "invalid pairing link"
    );
    let mut server = None;
    let mut code = None;
    for (key, value) in link.query_pairs() {
        match key.as_ref() {
            "server" if server.is_none() => server = Some(value.into_owned()),
            "code" if code.is_none() => code = Some(value.into_owned()),
            _ => anyhow::bail!("invalid pairing link parameters"),
        }
    }
    let code = code.context("missing pairing code")?;
    ensure!(
        code.len() == 64 && code.bytes().all(|c| c.is_ascii_hexdigit()),
        "pairing code must be 256-bit hex"
    );
    let server =
        Url::parse(&server.context("missing pairing server")?).context("invalid pairing server")?;
    ensure!(
        server.host().is_some()
            && server.username().is_empty()
            && server.password().is_none()
            && server.query().is_none()
            && server.fragment().is_none(),
        "invalid pairing server"
    );
    let host = server.host_str().unwrap_or_default();
    let loopback = host == "localhost"
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    ensure!(
        server.scheme() == "https" || (server.scheme() == "http" && loopback),
        "pairing requires HTTPS except on a loopback host"
    );
    Ok((server, code))
}

pub async fn redeem(link: &str, replace: bool, dir: &Path) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let (server, code) = parse_link(link)?;
    std::fs::create_dir_all(dir)?;
    // Serialize CLI/menu redeems before inspecting the token or burning a code.
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(dir.join("agent.pair.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock).context("pairing already in progress")?;
    let mut config = Config::load_at(dir)?;
    ensure!(
        replace || config.token.is_empty(),
        "already paired; use --replace to send the outbox to a new server"
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = client
        .post(format!("{}/v1/pair", server.as_str().trim_end_matches('/')))
        .json(&PairRequest {
            code,
            device_id: config.device_id.clone(),
        })
        .send()
        .await?;
    ensure!(
        response.status() != reqwest::StatusCode::GONE,
        "pairing code expired or already used; mint a new link"
    );
    ensure!(
        response.status().is_success(),
        "pairing failed: {}",
        response.status()
    );
    let paired: PairResponse = response.json().await.context("invalid pairing response")?;
    ensure!(
        !paired.token.trim().is_empty(),
        "pairing returned an empty token"
    );
    // Recheck after the network round trip in case a config editor paired meanwhile.
    ensure!(
        replace || Config::load_at(dir)?.token.is_empty(),
        "already paired; use --replace to replace the configured token"
    );
    config.server_url = server.as_str().trim_end_matches('/').to_owned();
    config.token = paired.token;
    config.save_at(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn link(server: &str) -> String {
        let mut url = Url::parse("hyperthymesia://pair").unwrap();
        url.query_pairs_mut()
            .append_pair("server", server)
            .append_pair("code", &"ab".repeat(32));
        url.to_string()
    }
    #[test]
    fn link_validation_checks_authority_and_literal_loopback_without_dns() {
        for server in [
            "http://127.0.0.1:7377",
            "http://127.12.34.56",
            "http://[::1]:7377",
            "http://localhost:7377",
            "https://example.com",
        ] {
            assert!(parse_link(&link(server)).is_ok(), "{server}");
        }
        for server in [
            "http://example.com",
            "http://127.0.0.1.example.com",
            "http://localhost.example.com",
            "http://192.168.0.1",
            "http://[::]",
            "ftp://127.0.0.1",
            "https://user:secret@example.com",
            "https://example.com?secret=x",
            "https://example.com#fragment",
        ] {
            assert!(parse_link(&link(server)).is_err(), "{server}");
        }
        let valid = link("https://example.com");
        for invalid in [
            valid.replace("hyperthymesia:", "https:"),
            valid.replace("://pair?", "://other?"),
            format!("{valid}&code={}", "cd".repeat(32)),
            format!("{valid}&server=https://other.invalid"),
            format!("{valid}#fragment"),
            valid.replace(&"ab".repeat(32), "bad"),
        ] {
            assert!(parse_link(&invalid).is_err());
        }
    }
}
