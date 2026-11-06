//! Owner-only OAuth handshake. Pending secrets and tokens stay in the key store.
use crate::{ApiResult, bad, connections::same_origin, dashboard::shell, sources};
use axum::{
    Form,
    extract::{Query, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Redirect, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use hx_sources::{
    keys::KeyStore,
    oauth::{Config, Credential},
};
use hx_store::Store;
use maud::html;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Deserialize, Default)]
pub struct QueryParams {
    pending: Option<String>,
    source: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Pending {
    hx_oauth_pending_v1: bool,
    config: Config,
    verifier: String,
    expires: DateTime<Utc>,
    source: Option<String>,
    name: String,
    description: String,
    history_days: u8,
}
fn keys(store: &Store) -> ApiResult<KeyStore> {
    KeyStore::new(&store.data_dir).map_err(|_| bad("OAuth key storage unavailable"))
}
fn private_page(content: maud::Markup) -> Response {
    (
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "same-origin"),
        ],
        shell("Connect with OAuth", content),
    )
        .into_response()
}
fn load(store: &Store, state: &str) -> ApiResult<Pending> {
    let value = keys(store)?
        .read(&format!("oauth-{state}"))
        .map_err(|_| bad("Invalid OAuth state"))?
        .ok_or_else(|| bad("OAuth link expired or already used. Start again."))?;
    let pending: Pending = serde_json::from_str(&value).map_err(|_| bad("Invalid OAuth state"))?;
    if pending.expires < Utc::now() {
        keys(store)?
            .remove(&format!("oauth-{state}"))
            .map_err(|_| bad("OAuth storage unavailable"))?;
        return Err(bad("OAuth link expired. Start again."));
    }
    Ok(pending)
}

pub async fn page(
    State(store): State<Store>,
    headers: HeaderMap,
    Query(query): Query<QueryParams>,
) -> ApiResult<Response> {
    sources::owner(&store, &headers).await?;
    if let Some(state) = query.pending {
        let pending = load(&store, &state)?;
        let mut url = url::Url::parse(&pending.config.authorization_url)
            .map_err(|_| bad("Invalid OAuth endpoint"))?;
        url.query_pairs_mut().extend_pairs([
            ("client_id", pending.config.client_id.as_str()),
            ("redirect_uri", pending.config.redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", pending.config.scopes.as_str()),
            ("state", state.as_str()),
            (
                "code_challenge",
                URL_SAFE_NO_PAD
                    .encode(Sha256::digest(pending.verifier.as_bytes()))
                    .as_str(),
            ),
            ("code_challenge_method", "S256"),
            ("access_type", "offline"),
            ("prompt", "consent"),
        ]);
        return Ok(private_page(html! {main {
            h1 {"Authorize your source"}
            p {"Open this link in your own browser and approve read access. The server needs no browser."}
            p {a href=(url.as_str()) target="_blank" rel="noreferrer" {"Open authorization link"}}
            p {"If the browser cannot reach the redirect, copy its entire address after consent, including code and state, and paste it here. This uses a registered redirect, not Google's retired out-of-band flow."}
            p {"Redirect URI: " code {(pending.config.redirect_uri)}}
            form method="post" action="/connect/oauth/finish" autocomplete="off" {
                input type="hidden" name="state" value=(state);
                label for="oauth-return" {"Redirected URL"}
                input id="oauth-return" name="redirected_url" type="password" required style="width:100%";
                button type="submit" {"Finish connection"}
            }
        }}));
    }
    let reconnect = if let Some(id) = query.source {
        let source = store
            .source(&id)
            .await?
            .ok_or_else(|| bad("Unknown source"))?;
        let saved = keys(&store)?
            .read(&id)
            .map_err(|_| bad("OAuth key unavailable"))?
            .ok_or_else(|| bad("OAuth key missing. Add the connection again."))?;
        if Credential::parse(&saved)
            .map_err(|_| bad("Invalid OAuth credential"))?
            .is_none()
        {
            return Err(bad("Source does not use OAuth"));
        }
        Some(source.id)
    } else {
        None
    };
    Ok(private_page(html! {main {
        h1 {"Google APIs with your Console client"}
        p {"Enable Gmail API and Google Calendar API in your project, configure consent and add your account as a test user if the app is in testing. Paste a Desktop app or Web application OAuth client JSON from Google Cloud Console."}
        p {"For a Desktop client, use a loopback redirect such as http://127.0.0.1:8787/. On a headless server, paste the final browser address back here even if that address cannot load. For a Web client, register the exact redirect URI in Console; an HTTPS address ending /connect/oauth/callback can finish automatically in this dashboard."}
        form method="post" action="/connect/oauth" autocomplete="off" {
            @if let Some(id) = reconnect {input type="hidden" name="source" value=(id); p {"Your saved client and scopes will be reused."}}
            @else {
                label for="oauth-client" {"Console client JSON"}
                textarea id="oauth-client" name="client_json" required rows="7" style="width:100%" spellcheck="false" {}
                label for="oauth-redirect" {"Redirect URI"}
                input id="oauth-redirect" name="redirect_uri" required value="http://127.0.0.1:8787/";
                label for="oauth-name" {"Name"} input id="oauth-name" name="name" value="Google mail and calendar" required;
                label for="oauth-description" {"What is it?"}
                textarea id="oauth-description" name="description" rows="4" style="width:100%" {"My Gmail and Google Calendar. Read recent messages at https://gmail.googleapis.com and events at https://www.googleapis.com/calendar/v3. Gmail messages.list returns IDs only; use detail requests for messages.get, map internalDate as unix_ms and Subject from payload.headers. Calendar events use start.dateTime and end.dateTime; all-day dates are floating, not instants. Use history-bound filters and stable IDs."}
                label for="oauth-origins" {"API origins allowed to receive access tokens"}
                input id="oauth-origins" name="origins" value="https://gmail.googleapis.com https://www.googleapis.com" style="width:100%" required;
                label for="oauth-scopes" {"Read-only scopes"}
                input id="oauth-scopes" name="scopes" value="https://www.googleapis.com/auth/gmail.readonly https://www.googleapis.com/auth/calendar.readonly" style="width:100%" required;
                label for="oauth-history" {"History days (0–30)"} input id="oauth-history" name="history_days" type="number" min="0" max="30" value="1";
            }
            button type="submit" {"Get authorization link"}
        }
    }}))
}
#[derive(Deserialize)]
pub struct Start {
    source: Option<String>,
    client_json: Option<String>,
    redirect_uri: Option<String>,
    name: Option<String>,
    description: Option<String>,
    origins: Option<String>,
    scopes: Option<String>,
    history_days: Option<u8>,
}
pub async fn start(
    State(store): State<Store>,
    headers: HeaderMap,
    request: Result<Form<Start>, axum::extract::rejection::FormRejection>,
) -> ApiResult<Response> {
    sources::owner(&store, &headers).await?;
    same_origin(&headers)?;
    let Form(request) = request.map_err(|_| bad("Invalid OAuth form"))?;
    let (config, name, description, history_days) = if let Some(id) = &request.source {
        let source = store
            .source(id)
            .await?
            .ok_or_else(|| bad("Unknown source"))?;
        if !source.enabled {
            return Err(bad("Turn this source on before reconnecting"));
        }
        let saved = keys(&store)?
            .read(id)
            .map_err(|_| bad("OAuth key unavailable"))?
            .ok_or_else(|| bad("OAuth key missing"))?;
        let credential = Credential::parse(&saved)
            .map_err(|_| bad("Invalid OAuth credential"))?
            .ok_or_else(|| bad("Source does not use OAuth"))?;
        (
            credential.config,
            source.name,
            source.description,
            source.history_days as u8,
        )
    } else {
        let client: serde_json::Value =
            serde_json::from_str(request.client_json.as_deref().unwrap_or(""))
                .map_err(|_| bad("Invalid Console client JSON"))?;
        let client = client
            .get("installed")
            .or_else(|| client.get("web"))
            .ok_or_else(|| bad("Use a Desktop or Web Console OAuth client"))?;
        let field = |name| client[name].as_str().unwrap_or("").to_owned();
        (
            Config {
                client_id: field("client_id"),
                client_secret: field("client_secret"),
                authorization_url: field("auth_uri"),
                token_url: field("token_uri"),
                redirect_uri: request.redirect_uri.unwrap_or_default(),
                origins: request
                    .origins
                    .unwrap_or_default()
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect(),
                scopes: request.scopes.unwrap_or_default(),
            },
            request.name.unwrap_or_default(),
            request.description.unwrap_or_default(),
            request.history_days.unwrap_or(1),
        )
    };
    config
        .validate()
        .map_err(|_| bad("Invalid OAuth client, endpoint, redirect, origins or scopes"))?;
    if name.trim().is_empty()
        || name.len() > 200
        || name.chars().any(char::is_control)
        || description.len() > 16384
        || history_days > 30
    {
        return Err(bad("Invalid source name, description or history limit"));
    }
    let filter = sources::scrubber(&store, Some(&config.client_secret)).await?;
    if filter.contains_secret(&name) || filter.contains_secret(&description) {
        return Err(bad("Source description must not contain credentials"));
    }
    let random_token = || -> ApiResult<String> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| bad("OAuth randomness unavailable"))?;
        Ok(URL_SAFE_NO_PAD.encode(bytes))
    };
    let state = random_token()?;
    let verifier = random_token()?;
    let pending = Pending {
        hx_oauth_pending_v1: true,
        config,
        verifier,
        expires: Utc::now() + Duration::minutes(15),
        source: request.source,
        name,
        description,
        history_days,
    };
    keys(&store)?
        .write(
            &format!("oauth-{state}"),
            &serde_json::to_string(&pending).map_err(anyhow::Error::from)?,
        )
        .map_err(|_| bad("OAuth key storage unavailable"))?;
    Ok(Redirect::to(&format!("/connect/oauth?pending={state}")).into_response())
}
#[derive(Deserialize)]
pub struct Finish {
    state: String,
    redirected_url: String,
}
pub async fn finish(
    State(store): State<Store>,
    headers: HeaderMap,
    request: Result<Form<Finish>, axum::extract::rejection::FormRejection>,
) -> ApiResult<Response> {
    sources::owner(&store, &headers).await?;
    same_origin(&headers)?;
    let Form(request) = request.map_err(|_| bad("Invalid OAuth return"))?;
    let pending = load(&store, &request.state)?;
    let returned = url::Url::parse(&request.redirected_url)
        .map_err(|_| bad("Paste the entire redirected URL, including code and state"))?;
    let redirect =
        url::Url::parse(&pending.config.redirect_uri).map_err(|_| bad("Invalid OAuth redirect"))?;
    if returned.origin() != redirect.origin()
        || returned.path() != redirect.path()
        || returned.fragment().is_some()
    {
        return Err(bad("OAuth redirect does not match"));
    }
    let params: std::collections::BTreeMap<_, _> = returned.query_pairs().into_owned().collect();
    complete(
        &store,
        params
            .get("state")
            .filter(|s| **s == request.state)
            .ok_or_else(|| bad("OAuth state mismatch"))?,
        params
            .get("code")
            .ok_or_else(|| bad("Authorization denied or no code returned"))?,
    )
    .await
}
#[derive(Deserialize)]
pub struct Callback {
    state: Option<String>,
    code: Option<String>,
}
pub async fn callback(
    State(store): State<Store>,
    query: Result<Query<Callback>, axum::extract::rejection::QueryRejection>,
) -> ApiResult<Response> {
    // Cross-site OAuth redirects do not carry SameSite=Strict owner cookies.
    // The unguessable, short-lived state was issued only to an authenticated
    // owner and binds this one callback to its private PKCE verifier.
    let Query(query) = query.map_err(|_| bad("Invalid OAuth callback"))?;
    complete(
        &store,
        query
            .state
            .as_deref()
            .ok_or_else(|| bad("OAuth state missing"))?,
        query
            .code
            .as_deref()
            .ok_or_else(|| bad("Authorization denied or code missing"))?,
    )
    .await?;
    Ok(private_page(html! {main {
        h1 {"Connection completed"}
        p {"Automatic pulls are being set up. You can return to your dashboard."}
        a href="/connect" {"Return to connected sources"}
    }}))
}
static COMPLETE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
async fn complete(store: &Store, state: &str, code: &str) -> ApiResult<Response> {
    let _lock = COMPLETE.lock().await;
    let pending = load(store, state)?;
    if let Some(id) = &pending.source
        && !store.source(id).await?.is_some_and(|s| s.enabled)
    {
        return Err(bad("Source is off or missing"));
    }
    // Consume before exchange: a callback replay can never publish a second source.
    keys(store)?
        .remove(&format!("oauth-{state}"))
        .map_err(|_| bad("OAuth key storage unavailable"))?;
    let value = hx_sources::oauth::token(
        &pending.config,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &pending.config.redirect_uri),
            ("code_verifier", &pending.verifier),
        ],
    )
    .await
    .map_err(|_| {
        bad("OAuth exchange failed. Check the Console client and consent, then reconnect.")
    })?;
    let credential = Credential {
        hx_oauth_v1: true,
        config: pending.config,
        access_token: value["access_token"].as_str().unwrap_or("").to_owned(),
        refresh_token: value["refresh_token"]
            .as_str()
            .filter(|s| s.len() >= 4)
            .ok_or_else(|| bad("No refresh token returned. Grant offline consent and reconnect."))?
            .to_owned(),
        expires_at: hx_sources::oauth::expiry(&value)
            .map_err(|_| bad("Invalid OAuth token expiry"))?,
    };
    let key = serde_json::to_string(&credential).map_err(anyhow::Error::from)?;
    if let Some(id) = pending.source {
        store.patch_source(&id, None, Some(&key)).await?;
        store.resume_source(&id).await?;
    } else {
        store
            .connect_source_with_key(
                &pending.name,
                &pending.description,
                pending.history_days,
                Some(&key),
            )
            .await?;
    }
    Ok(Redirect::to("/connect?connected=1").into_response())
}
