//! LCU (League Client Update protocol) transport.
//!
//! Read-only access to the local League Client HTTP + WebSocket API. All
//! connections are made to `127.0.0.1` only. The LCU serves a self-signed
//! certificate, so TLS verification is intentionally skipped **only** for
//! loopback connections — a custom verifier rejects any non-loopback
//! server name.
//!
//! The transport runs on a dedicated worker thread with its own tokio
//! runtime. The plugin's synchronous `poll` reads the latest [`Shared`]
//! snapshot; it never blocks on the network.
//!
//! # Security notes
//!
//! * The connection token is used for `Basic` auth. It is never logged.
//! * No endpoint is ever modified; this is strictly read-only.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{header, Uri};
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;
use tracing::{debug, error, info, warn};

use crate::detection::{LeagueConnectionInfo, LockfileLocator};
use crate::state::{queue_name, LeagueSnapshot};

/// How long to wait before re-attempting a lost connection.
pub const RECONNECT_DELAY: Duration = Duration::from_secs(2);
/// Backoff cap between reconnect attempts.
pub const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Cadence of the fallback HTTP poll while the websocket is idle.
pub const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Faster cadence used by the fallback HTTP poll during volatile pre-game
/// states (matchmaking, ready check, champion select, game start), where a
/// dodge, queue denial, or accept/cancel can change the phase in seconds.
pub const POLL_INTERVAL_VOLATILE: Duration = Duration::from_millis(500);
/// Per-request timeout for LCU HTTP GETs.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors produced by the LCU transport.
#[derive(Debug, thiserror::Error)]
pub enum LcuError {
    #[error("LCU connection failed: {0}")]
    Connection(String),
    #[error("WebSocket error: {0}")]
    WebSocket(String),
    #[error("TLS configuration error: {0}")]
    Tls(String),
}

impl LcuError {
    fn ws(e: impl std::fmt::Display) -> Self {
        LcuError::WebSocket(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Loopback-only TLS
// ---------------------------------------------------------------------------

/// A certificate verifier that accepts **only** loopback IP connections.
///
/// Any other server name is rejected, so the self-signed certificate
/// tolerance can never be used against a remote host.
#[derive(Debug)]
struct LoopbackOnlyVerifier;

impl ServerCertVerifier for LoopbackOnlyVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match server_name {
            ServerName::IpAddress(ip) => {
                let std_ip: std::net::IpAddr = (*ip).into();
                if std_ip.is_loopback() {
                    Ok(ServerCertVerified::assertion())
                } else {
                    Err(rustls::Error::General(
                        "TLS verification restricted to loopback connections".to_string(),
                    ))
                }
            }
            _ => Err(rustls::Error::General(
                "TLS verification restricted to loopback connections".to_string(),
            )),
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
        ]
    }
}

/// Builds a TLS client config that accepts self-signed certs on loopback.
fn loopback_tls_config() -> Result<rustls::ClientConfig, LcuError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| LcuError::Tls(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(LoopbackOnlyVerifier))
        .with_no_client_auth())
}

// ---------------------------------------------------------------------------
// Basic auth (base64)
// ---------------------------------------------------------------------------

/// Encodes a string as base64 (RFC 4648). Used only for the LCU WebSocket
/// `Authorization` header.
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(n >> 6) as usize & 63] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[n as usize & 63] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// The `Authorization` header value for the LCU WebSocket handshake.
fn ws_authorization(info: &LeagueConnectionInfo) -> String {
    let creds = format!("riot:{}", info.token);
    format!("Basic {}", base64_encode(creds.as_bytes()))
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Snapshot holder shared between the worker thread and the plugin's `poll`.
pub struct Shared {
    /// The latest observed state.
    pub snapshot: Mutex<LeagueSnapshot>,
    /// Champion id -> display name cache. Champion names are stable for a
    /// patch, so repeated champ-select events (hover/select cycles) reuse the
    /// cached name instead of re-fetching the game-data asset every time.
    pub champion_names: Mutex<HashMap<i32, String>>,
    /// Set to `true` to request a clean shutdown of the worker thread.
    pub shutdown: AtomicBool,
    /// Transition-based dedup for the worker's repeated warnings.
    ///
    /// Reconnect failures and per-poll data-fetch failures are logged on the
    /// first occurrence of a distinct message and stay silent while it
    /// persists, so an unreachable client (or a malformed/slow endpoint)
    /// cannot spam the log on every poll. Re-arms after a successful connect
    /// or when the client disappears.
    warnings: Mutex<WarnDedup>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            snapshot: Mutex::new(LeagueSnapshot::default()),
            champion_names: Mutex::new(HashMap::new()),
            shutdown: AtomicBool::new(false),
            warnings: Mutex::new(WarnDedup::default()),
        }
    }
}

impl Shared {
    /// Runs `f` against the current snapshot.
    ///
    /// Recovers a poisoned lock: if the worker panicked while holding the
    /// guard, the previously written snapshot is still intact and is used
    /// instead of silently dropping the write. Without this, a single panic
    /// permanently disabled every subsequent snapshot update until restart.
    fn update(&self, f: impl FnOnce(&mut LeagueSnapshot)) {
        let mut snap = self.snapshot.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut snap);
    }

    /// Marks the connection as lost and clears all observed data.
    fn set_disconnected(&self) {
        self.update(|s| {
            *s = LeagueSnapshot::default();
        });
    }

    /// Returns `true` when `message` is a new transition and the worker should
    /// log it, `false` while the identical message persists. Recovers a
    /// poisoned tracker lock like the snapshot lock: a worker panic must not
    /// permanently disable warning dedup.
    fn should_warn(&self, message: &str) -> bool {
        self.warnings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .should_warn(message)
    }

    /// Re-arms the warning tracker after a successful connection or when the
    /// client disappears, so a distinct subsequent failure logs again.
    fn warn_cleared(&self) {
        self.warnings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .cleared();
    }
}

// ---------------------------------------------------------------------------
// Worker thread
// ---------------------------------------------------------------------------

/// Spawns the LCU worker thread.
///
/// Returns `None` if the thread could not be spawned.
///
/// The thread runs a panic boundary: if the observer panics (e.g. while
/// holding the snapshot lock), the panic is caught and the observer loop is
/// restarted after a short pause instead of silently killing the thread.
/// Combined with the poisoned-lock recovery in [`Shared::update`], this lets
/// the plugin keep serving snapshots after a transient worker failure.
pub fn spawn(shared: Arc<Shared>) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("league-lcu".to_string())
        .spawn(move || {
            loop {
                if shared.shutdown.load(Ordering::Relaxed) {
                    break;
                }
                let worker_shared = shared.clone();
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                    let runtime = match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(rt) => rt,
                        Err(e) => {
                            error!(error = %e, "Failed to create League LCU runtime");
                            return;
                        }
                    };
                    runtime.block_on(run(worker_shared));
                }));
                match outcome {
                    // `run` only returns on a clean shutdown request.
                    Ok(()) => break,
                    Err(_) => {
                        error!("League LCU worker panicked; restarting observer loop");
                        // Pause before restarting so a recurring panic cannot
                        // spin the thread into a hot loop.
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            }
        })
        .ok()
}

/// Transition-based dedup for the worker's repeated warnings.
///
/// The worker thread lives outside the runtime's per-source
/// [`PollErrorTracker`], so without this guard a persistent failure logs on
/// every attempt (a stale lockfile with no live client logs
/// "LCU connection lost; reconnecting" on every reconnect attempt; a
/// malformed JSON body or best-effort fetch failure logs on every poll). This
/// tracker applies the same transition semantics as the runtime: warn on the
/// first occurrence of a distinct message, stay silent while the identical
/// message persists, warn again when the message changes, and re-arm after a
/// successful connect or once the client disappears.
#[derive(Default)]
struct WarnDedup {
    last: Option<String>,
}

impl WarnDedup {
    /// Returns `true` when `message` is a new transition and should be warned.
    fn should_warn(&mut self, message: &str) -> bool {
        if self.last.as_deref() == Some(message) {
            false
        } else {
            self.last = Some(message.to_string());
            true
        }
    }

    /// Re-arms after a successful connection or when the client disappears,
    /// so a subsequent distinct failure logs again.
    fn cleared(&mut self) {
        self.last = None;
    }
}

/// Main worker loop: discover the client, connect, observe, reconnect.
async fn run(shared: Arc<Shared>) {
    run_loop(shared, &LockfileLocator::new()).await;
}

/// Worker loop parameterized over the lockfile locator, so tests can point
/// the worker at a controlled lockfile.
///
/// The shared snapshot is cleared in exactly one place: when the client is no
/// longer detected (`find_and_parse` returns `None`, i.e. no lockfile or a
/// dead pid). A failed [`connect_and_observe`] while the client is still
/// detected is treated as *transient* and deliberately preserves the last
/// observed snapshot, so a brief disconnect (client restarting, a network
/// blip) does not end the presence session and reset the game timer.
async fn run_loop(shared: Arc<Shared>, locator: &LockfileLocator) {
    let mut backoff = RECONNECT_DELAY;

    loop {
        if shared.shutdown.load(Ordering::Relaxed) {
            break;
        }

        let Some(info) = locator.find_and_parse() else {
            shared.warn_cleared();
            let connected = shared
                .snapshot
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .connected;
            if !connected {
                debug!("League client not detected; waiting");
            } else {
                debug!("League client gone; clearing state");
            }
            shared.set_disconnected();
            tokio::time::sleep(RECONNECT_DELAY).await;
            continue;
        };

        match connect_and_observe(shared.as_ref(), &info).await {
            Ok(()) => {
                shared.warn_cleared();
                backoff = RECONNECT_DELAY;
            }
            Err(e) => {
                let message = e.to_string();
                if shared.should_warn(&message) {
                    warn!(error = %message, "LCU connection lost; reconnecting");
                }
                // Presence is preserved across the reconnect window: the
                // client is still detected (lockfile + live pid), so this
                // failure is transient. Clearing the snapshot here would make
                // poll() error out and end the session, resetting the game
                // timer on a mere blip.
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }

    info!("League LCU worker stopped");
}

/// Establishes a connection and observes the client until it drops.
async fn connect_and_observe(shared: &Shared, info: &LeagueConnectionInfo) -> Result<(), LcuError> {
    let base = format!("https://127.0.0.1:{}", info.port);
    let http = build_http_client()?;

    // Initial snapshot via HTTPS.
    refresh_all(shared, &http, &base, info).await?;

    // Prefer the WebSocket for real-time updates; fall back to polling.
    match observe_websocket(shared, info, &http, &base).await {
        Ok(()) => Ok(()),
        Err(e) => {
            warn!(error = %e, "WebSocket unavailable; using polling fallback");
            poll_loop(shared, &http, &base, info).await
        }
    }
}

/// Builds the HTTP client used for LCU HTTPS GETs.
fn build_http_client() -> Result<reqwest::Client, LcuError> {
    let tls = loopback_tls_config()?;
    reqwest::Client::builder()
        .use_preconfigured_tls(tls)
        .no_proxy()
        .build()
        .map_err(|e| LcuError::Connection(format!("failed to build HTTP client: {e}")))
}

/// Returns the fallback-poll cadence appropriate for a gameflow phase.
///
/// Volatile pre-game states — where a dodge, queue denial, or accept/cancel
/// can change the phase in seconds — poll at the fast cadence so the presence
/// cannot go stale. Stable states (in game, idle, lobby, post-game) use the
/// regular cadence.
fn poll_interval_for_phase(phase: &str) -> Duration {
    if phase == "GameStart" {
        // The brief loading transition into a game: a launch failure returns
        // to the client immediately, so it is treated as volatile.
        return POLL_INTERVAL_VOLATILE;
    }
    match crate::state::state_from_phase(phase) {
        crate::state::LeagueState::Matchmaking
        | crate::state::LeagueState::ReadyCheck
        | crate::state::LeagueState::ChampionSelect => POLL_INTERVAL_VOLATILE,
        _ => POLL_INTERVAL,
    }
}

/// Pure polling loop used when the WebSocket cannot be established.
async fn poll_loop(
    shared: &Shared,
    http: &reqwest::Client,
    base: &str,
    info: &LeagueConnectionInfo,
) -> Result<(), LcuError> {
    loop {
        if shared.shutdown.load(Ordering::Relaxed) {
            return Ok(());
        }
        refresh_all(shared, http, base, info).await?;
        // Adaptive cadence: poll fast while in a volatile pre-game state so
        // dodge/deny/accept transitions cannot go stale at the regular cadence.
        // This reuses the single polling loop — no second timer is created.
        let phase = shared
            .snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .phase
            .clone();
        tokio::time::sleep(poll_interval_for_phase(&phase)).await;
    }
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

/// Performs a read-only GET and returns the JSON body, if the endpoint
/// answers with a 2xx status.
///
/// `Ok(None)` means the endpoint was reachable but not applicable (404 /
/// other non-2xx). `Err` means the request itself failed (connection
/// refused, timeout, TLS failure) — i.e. the client is unreachable.
async fn get_json(
    shared: &Shared,
    http: &reqwest::Client,
    base: &str,
    token: &str,
    path: &str,
) -> Result<Option<serde_json::Value>, LcuError> {
    let url = format!("{base}{path}");
    let resp = http
        .get(&url)
        .basic_auth("riot", Some(token))
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| LcuError::Connection(format!("GET {path}: {e}")))?;

    let status = resp.status();
    if status.is_success() {
        let body = resp
            .text()
            .await
            .map_err(|e| LcuError::Connection(format!("read {path}: {e}")))?;
        return Ok(Some(parse_json_body(shared, path, &body)));
    }
    Ok(None)
}

/// Parses an LCU JSON response body.
///
/// The LCU always answers with JSON, so a body that fails to parse is
/// anomalous and worth surfacing instead of silently degrading to `Null`
/// (which drops whatever context this request carried). Shadowed
/// [`Shared::should_warn`] dedup keeps the message to one per distinct
/// failure, so a persistently misbehaving endpoint cannot spam every poll.
fn parse_json_body(shared: &Shared, path: &str, body: &str) -> serde_json::Value {
    match serde_json::from_str(body) {
        Ok(value) => value,
        Err(e) => {
            if shared.should_warn(&format!("parse-json {path}: {e}")) {
                warn!(path = %path, error = %e, "LCU response was not valid JSON");
            }
            serde_json::Value::Null
        }
    }
}

/// Fetches a best-effort endpoint, logging a warning when the request itself
/// fails (transport error).
///
/// Unlike [`get_json`], a failure here must not tear down the connection —
/// the phase probe is the authoritative liveness check. But a failed
/// best-effort fetch silently drops context (version, ranked, session,
/// champ select, lobby), which deserves a log line rather than swallowing it.
/// A `404`/non-2xx (the endpoint is simply not applicable, e.g. no session
/// active) returns `None` without warning.
async fn get_json_best_effort(
    shared: &Shared,
    http: &reqwest::Client,
    base: &str,
    token: &str,
    path: &str,
) -> Option<serde_json::Value> {
    match get_json(shared, http, base, token, path).await {
        Ok(Some(value)) => Some(value),
        Ok(None) => None,
        Err(e) => {
            if shared.should_warn(&format!("best-effort {path}: {e}")) {
                warn!(path = %path, error = %e, "Best-effort LCU fetch failed");
            }
            None
        }
    }
}

/// Fetches everything needed to rebuild a full snapshot.
async fn refresh_all(
    shared: &Shared,
    http: &reqwest::Client,
    base: &str,
    info: &LeagueConnectionInfo,
) -> Result<(), LcuError> {
    let token = info.token.clone();

    // The phase endpoint is the liveness probe: a transport error here means
    // the client is gone.
    let phase = get_json(
        shared,
        http,
        base,
        &token,
        "/lol-gameflow/v1/gameflow-phase",
    )
    .await?
    .and_then(|v| v.as_str().map(|s| s.to_string()))
    .unwrap_or_default();

    let version = get_json_best_effort(shared, http, base, &token, "/lol-patch/v1/game-version")
        .await
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .map(|s| s.split('+').next().unwrap_or(&s).to_string());

    let (tier, rank) = fetch_ranked(shared, http, base, &token).await;

    shared.update(|s| {
        s.connected = true;
        s.version = version;
        s.tier = tier;
        s.rank = rank;
        apply_phase(s, &phase);
    });

    // Gameflow session context (queue/map from `gameData`). Best-effort; the
    // endpoint 404s when the player is not in a session.
    refresh_gameflow_session(shared, http, base, &token).await;

    // Champion select context (best-effort; 404 when not in champ select).
    refresh_champ_select(shared, http, base, &token).await;

    // Lobby queue context (best-effort; 404 when not in a lobby).
    refresh_lobby(shared, http, base, &token).await;

    Ok(())
}

/// Applies a newly observed gameflow phase to the snapshot.
///
/// Clears game-scoped context that no longer applies: champion/map are kept
/// while a game is live or in the post-game review, and the queue is dropped
/// when the player is back on the client home screen so `Idle` never shows
/// stale game context.
fn apply_phase(s: &mut LeagueSnapshot, phase: &str) {
    s.phase = phase.to_string();
    if !crate::state::state_from_phase(phase).preserves_game_context() {
        s.champion_id = None;
        s.champion_name = None;
        s.map_id = None;
    }
    if phase.is_empty() || phase == "None" {
        s.queue_id = None;
        s.queue_name = None;
    }
}

/// Reads the gameflow session from `/lol-gameflow/v1/session`.
///
/// The session's `gameData` carries the canonical queue (id and display
/// name) and map while a game is live, including Practice Tool and Custom
/// games that may not pass through a normal lobby. Best-effort: a 404
/// (player not in a session) leaves the existing snapshot fields untouched.
async fn refresh_gameflow_session(
    shared: &Shared,
    http: &reqwest::Client,
    base: &str,
    token: &str,
) {
    let Some(v) = get_json_best_effort(shared, http, base, token, "/lol-gameflow/v1/session").await
    else {
        return;
    };

    let Some((queue_id, queue_name, map_id)) = parse_gameflow_session(&v) else {
        return;
    };

    shared.update(|s| {
        if let Some(q) = queue_id {
            s.queue_id = Some(q);
        }
        if let Some(n) = queue_name {
            s.queue_name = Some(n);
        }
        if let Some(m) = map_id {
            s.map_id = Some(m);
        }
    });
}

/// Parses a `/lol-gameflow/v1/session` response into queue id, queue name,
/// and map id.
///
/// The queue lives under `gameData.queue`; the map id is nested inside that
/// queue object (a top-level `gameData.mapId` is not emitted). The queue
/// name prefers the canonical local mapping and falls back to the client's
/// own display name so brand-new queues are still shown.
fn parse_gameflow_session(
    v: &serde_json::Value,
) -> Option<(Option<i32>, Option<String>, Option<i64>)> {
    let queue_id = v
        .pointer("/gameData/queue/id")
        .and_then(|x| x.as_i64())
        .map(|x| x as i32);
    let queue_name = queue_id.and_then(queue_name).or_else(|| {
        v.pointer("/gameData/queue/name")
            .and_then(|x| x.as_str())
            .map(String::from)
    });
    let map_id = v
        .pointer("/gameData/queue/mapId")
        .and_then(|x| x.as_i64())
        .or_else(|| v.pointer("/gameData/mapId").and_then(|x| x.as_i64()));
    Some((queue_id, queue_name, map_id))
}

/// Reads the ranked tier/division from `/lol-ranked/v1/current-ranked-stats`.
async fn fetch_ranked(
    shared: &Shared,
    http: &reqwest::Client,
    base: &str,
    token: &str,
) -> (Option<String>, Option<String>) {
    let Some(v) = get_json_best_effort(
        shared,
        http,
        base,
        token,
        "/lol-ranked/v1/current-ranked-stats",
    )
    .await
    else {
        return (None, None);
    };

    let clean = |value: Option<&serde_json::Value>| {
        value
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty() && *s != "NONE" && *s != "NA" && *s != "null")
            .map(|s| s.to_string())
    };

    let tier = clean(v.pointer("/queueMap/RANKED_SOLO_5x5/tier"));
    let rank = clean(v.pointer("/queueMap/RANKED_SOLO_5x5/division"));
    (tier, rank)
}

/// Reads the lobby queue id/name from `/lol-lobby/v2/lobby`.
async fn refresh_lobby(shared: &Shared, http: &reqwest::Client, base: &str, token: &str) {
    let Some(v) = get_json_best_effort(shared, http, base, token, "/lol-lobby/v2/lobby").await
    else {
        shared.update(|s| {
            // Only the pre-game lobby phases rely on this endpoint for their
            // queue. During champion select and every game phase the queue is
            // supplied by the champ-select / gameflow session, so a 404 here
            // must not wipe that context.
            let phase = crate::state::state_from_phase(&s.phase);
            let relies_on_lobby = matches!(
                phase,
                crate::state::LeagueState::Lobby
                    | crate::state::LeagueState::Matchmaking
                    | crate::state::LeagueState::ReadyCheck
            );
            if relies_on_lobby {
                s.queue_id = None;
                s.queue_name = None;
            }
        });
        return;
    };

    let queue_id = v
        .pointer("/gameConfig/queueId")
        .and_then(|x| x.as_i64())
        .map(|x| x as i32);

    shared.update(|s| apply_lobby_queue(s, queue_id));
}

/// Merges a lobby queue id into the snapshot.
///
/// The canonical mapping supplies the name; unknown ids keep any name the
/// gameflow session already provided so the queue does not flip between a
/// named mode and its raw numeric id mid-lobby.
fn apply_lobby_queue(s: &mut LeagueSnapshot, queue_id: Option<i32>) {
    s.queue_id = queue_id;
    if let Some(name) = queue_id.and_then(queue_name) {
        s.queue_name = Some(name);
    }
}

/// Reads the local player's champion from `/lol-champ-select/v1/session`.
async fn refresh_champ_select(shared: &Shared, http: &reqwest::Client, base: &str, token: &str) {
    let Some(v) =
        get_json_best_effort(shared, http, base, token, "/lol-champ-select/v1/session").await
    else {
        return;
    };

    apply_champ_select_payload(shared, http, base, token, &v).await;
}

/// Parses a champ-select session value into the local player's champion id,
/// the queue id, and the map id.
///
/// The payload shape is identical whether it comes from the HTTP endpoint or
/// a WebSocket `OnJsonApiEvent` for `/lol-champ-select/v1/session`. The local
/// player is found by matching `localPlayerCellId` inside `myTeam`.
/// `championId == 0` (nothing locked/selected yet) maps to `None`, so
/// reporting Champion Select never depends on a champion being locked.
fn parse_champ_select(v: &serde_json::Value) -> (Option<i32>, Option<i32>, Option<i64>) {
    let local_cell = v.get("localPlayerCellId").and_then(|x| x.as_i64());
    let queue_id = v.get("queueId").and_then(|x| x.as_i64()).map(|x| x as i32);
    let map_id = v.get("mapId").and_then(|x| x.as_i64());

    let champion_id = v
        .get("myTeam")
        .and_then(|t| t.as_array())
        .and_then(|team| {
            local_cell.and_then(|cell| {
                team.iter()
                    .find(|m| m.get("cellId").and_then(|c| c.as_i64()) == Some(cell))
            })
        })
        .and_then(|m| {
            m.get("championId")
                .and_then(|c| c.as_i64())
                .map(|x| x as i32)
        })
        // championId == 0 means "not locked yet".
        .filter(|&id| id != 0);

    (champion_id, queue_id, map_id)
}

/// Applies a champ-select session payload (HTTP or WebSocket) to the snapshot.
async fn apply_champ_select_payload(
    shared: &Shared,
    http: &reqwest::Client,
    base: &str,
    token: &str,
    v: &serde_json::Value,
) {
    let (champion_id, queue_id, map_id) = parse_champ_select(v);
    let champion_name = match champion_id {
        Some(id) => fetch_champion_name(shared, http, base, token, id).await,
        None => None,
    };

    shared.update(|s| {
        s.champion_id = champion_id;
        s.champion_name = champion_name;
        apply_champ_select_queue(s, queue_id);
        if let Some(m) = map_id {
            s.map_id = Some(m);
        }
    });
}

/// Merges the champ-select session queue id into the snapshot.
///
/// Unknown queue ids keep the gameflow-provided name instead of falling back
/// to the raw numeric id during champion select.
fn apply_champ_select_queue(s: &mut LeagueSnapshot, queue_id: Option<i32>) {
    if let Some(q) = queue_id {
        s.queue_id = Some(q);
        s.queue_name = queue_name(q).or_else(|| s.queue_name.clone());
    }
}

/// Resolves a champion id to its display name, using the shared cache and
/// falling back to the game data assets endpoint on a miss.
///
/// A transport failure on the fallback fetch is logged (transition-deduped
/// via [`Shared::should_warn`]) instead of silently dropping the name; a
/// plain `404` (unknown/unreleased id) returns `None` without a warning.
async fn fetch_champion_name(
    shared: &Shared,
    http: &reqwest::Client,
    base: &str,
    token: &str,
    id: i32,
) -> Option<String> {
    // Read the cache, recovering a poisoned lock (a worker panic while
    // holding it must not disable champion-name caching for the rest of the
    // run; the cache is best-effort, so a miss just refetches).
    let cached = shared
        .champion_names
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .cloned();
    if let Some(name) = cached {
        return Some(name);
    }

    let path = format!("/lol-game-data/assets/v1/champions/{id}.json");
    let name = match get_json(shared, http, base, token, &path).await {
        Ok(Some(v)) => v.get("name").and_then(|x| x.as_str()).map(String::from),
        Ok(None) => None,
        Err(e) => {
            if shared.should_warn(&format!("champion-name {path}: {e}")) {
                warn!(path = %path, error = %e, "Best-effort champion name fetch failed");
            }
            None
        }
    };

    if let Some(n) = &name {
        let mut cache = shared
            .champion_names
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        cache.insert(id, n.clone());
    }
    name
}

// ---------------------------------------------------------------------------
// WebSocket subscription
// ---------------------------------------------------------------------------

/// An `OnJsonApiEvent` payload received from the LCU websocket.
struct LcuEvent {
    uri: String,
    data: serde_json::Value,
}

/// Parses a raw websocket text frame into an event.
///
/// Malformed frames are dropped, but the drop is logged (transition-deduped
/// via [`Shared::should_warn`]) rather than silently swallowed: a client that
/// keeps sending unparseable or incomplete frames must not be hidden, yet a
/// persistent flood cannot spam the log on every frame.
fn parse_event(shared: &Shared, text: &str) -> Option<LcuEvent> {
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            if shared.should_warn(&format!("ws-frame: {e}")) {
                warn!(error = %e, "Ignoring malformed LCU websocket frame");
            }
            return None;
        }
    };
    let uri = match v.get("uri").and_then(|x| x.as_str()) {
        Some(uri) => uri.to_string(),
        None => {
            if shared.should_warn("ws-frame missing uri") {
                warn!("Ignoring LCU websocket frame without a uri");
            }
            return None;
        }
    };
    let Some(data) = v.get("data").cloned() else {
        if shared.should_warn("ws-frame missing data") {
            warn!("Ignoring LCU websocket frame without data");
        }
        return None;
    };
    Some(LcuEvent { uri, data })
}

/// Observes the client over a WebSocket subscription.
///
/// While the socket is idle, a periodic HTTP poll keeps the snapshot fresh.
/// Returns `Ok(())` on graceful shutdown, `Err` when the connection fails.
async fn observe_websocket(
    shared: &Shared,
    info: &LeagueConnectionInfo,
    http: &reqwest::Client,
    base: &str,
) -> Result<(), LcuError> {
    let url = format!("wss://127.0.0.1:{}/", info.port);
    // Build the request via `Uri::into_client_request()` so tungstenite
    // generates the required `Sec-WebSocket-Key`/upgrade headers, then attach
    // the LCU basic-auth header.
    let uri: Uri = url.parse().map_err(LcuError::ws)?;
    let mut request = uri.into_client_request().map_err(LcuError::ws)?;
    request.headers_mut().insert(
        header::AUTHORIZATION,
        ws_authorization(info).parse().map_err(LcuError::ws)?,
    );

    // Manual TLS handshake over loopback so we can control certificate
    // verification precisely.
    let addr = (IpAddr::V4(Ipv4Addr::LOCALHOST), info.port);
    let tcp = TcpStream::connect(addr)
        .await
        .map_err(|e| LcuError::Connection(format!("TCP connect: {e}")))?;
    let server_name = ServerName::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST).into());
    let tls_connector = tokio_rustls::TlsConnector::from(Arc::new(loopback_tls_config()?));
    let tls_stream = tls_connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| LcuError::Connection(format!("TLS handshake: {e}")))?;

    let (mut ws, _) = tokio_tungstenite::client_async(request, tls_stream)
        .await
        .map_err(LcuError::ws)?;

    info!(port = info.port, "Connected to League Client WebSocket");
    shared.update(|s| s.connected = true);

    let mut last_refresh = Instant::now();

    loop {
        if shared.shutdown.load(Ordering::Relaxed) {
            return Ok(());
        }

        match tokio::time::timeout(Duration::from_secs(2), ws.next()).await {
            Ok(Some(Ok(WsMessage::Text(text)))) => {
                if let Some(event) = parse_event(shared, &text) {
                    apply_event(shared, http, base, &info.token, &event).await;
                }
            }
            Ok(Some(Ok(WsMessage::Close(_)))) => {
                return Err(LcuError::WebSocket(
                    "connection closed by client".to_string(),
                ));
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => {
                // The socket errored (client exited, reset, protocol error).
                return Err(LcuError::ws(e));
            }
            Ok(None) => {
                return Err(LcuError::WebSocket("connection closed".to_string()));
            }
            Err(_) => {
                // Idle read timeout. Fall back to a periodic HTTP poll so the
                // snapshot stays fresh even if the websocket silently misses
                // events.
                if last_refresh.elapsed() >= POLL_INTERVAL {
                    refresh_all(shared, http, base, info).await?;
                    last_refresh = Instant::now();
                }
            }
        }
    }
}

/// Handles the champ-select session being torn down (null WebSocket payload).
///
/// The session is deleted both when a game starts (champ select is torn down
/// as the match loads) and when the player dodges/cancels/leaves the queue.
/// The gameflow-phase event can arrive *after* this deletion, so the snapshot
/// phase may still read `ChampSelect` while the game is actually starting —
/// ARAM in particular tears the session down before the phase advances to
/// `GameStart`. The champion must survive that transition into the live game.
///
/// The LCU does not expose the local player's champion from an authoritative
/// in-game endpoint after champ select ends, so the champion observed in
/// champ select is the only source. It is therefore preserved whenever the
/// current phase is game-scoped ([`preserves_game_context`] — ChampionSelect,
/// InProgress, Reconnect, WaitingForStats) and cleared only when the player
/// is definitely outside a game. The non-game phases are also cleared by
/// [`apply_phase`], so this is a safety net rather than the primary clearing
/// path.
///
/// [`preserves_game_context`]: crate::state::LeagueState::preserves_game_context
fn champ_select_session_deleted(s: &mut LeagueSnapshot) {
    if !crate::state::state_from_phase(&s.phase).preserves_game_context() {
        s.champion_id = None;
        s.champion_name = None;
    }
}

/// Applies a websocket event to the shared snapshot.
async fn apply_event(
    shared: &Shared,
    http: &reqwest::Client,
    base: &str,
    token: &str,
    event: &LcuEvent,
) {
    match event.uri.as_str() {
        "/lol-gameflow/v1/gameflow-phase" => {
            if let Some(phase) = event.data.as_str() {
                let phase = phase.to_string();
                shared.update(|s| apply_phase(s, &phase));
                // Re-read the gameflow session so queue/map stay in sync with
                // the new phase (e.g. the game starts and `gameData` appears).
                refresh_gameflow_session(shared, http, base, token).await;
            }
        }
        "/lol-champ-select/v1/session" => {
            // The event payload is the full champ-select session (same shape
            // as the HTTP endpoint), so the champion/queue/map are derived
            // directly without a redundant HTTP re-fetch on every event.
            if event.data.is_null() {
                // Session torn down. Both the game-start transition and a
                // dodge/cancel/queue-leave delete it; see
                // [`champ_select_session_deleted`] for how they differ.
                shared.update(champ_select_session_deleted);
            } else {
                apply_champ_select_payload(shared, http, base, token, &event.data).await;
            }
        }
        "/lol-lobby/v2/lobby" => {
            refresh_lobby(shared, http, base, token).await;
        }
        "/lol-ranked/v1/current-ranked-stats" => {
            let (tier, rank) = fetch_ranked(shared, http, base, token).await;
            shared.update(|s| {
                s.tier = tier;
                s.rank = rank;
            });
        }
        _ => {
            debug!(uri = %event.uri, "Ignoring LCU event");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_encodes_standard_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn ws_authorization_matches_basic_auth() {
        let info = LeagueConnectionInfo {
            process: "LeagueClient".into(),
            pid: 1,
            port: 53034,
            token: "abc".into(),
            protocol: "https".into(),
        };
        // base64("riot:abc") == "cmlvdDphYmM="
        assert_eq!(ws_authorization(&info), "Basic cmlvdDphYmM=");
    }

    #[test]
    fn parse_event_extracts_uri_and_data() {
        let text = r#"{"data":"InProgress","eventType":"Update","uri":"/lol-gameflow/v1/gameflow-phase","timestamp":123}"#;
        let ev = parse_event(&Shared::default(), text).unwrap();
        assert_eq!(ev.uri, "/lol-gameflow/v1/gameflow-phase");
        assert_eq!(ev.data.as_str(), Some("InProgress"));
    }

    #[test]
    fn parse_event_ignores_malformed_frames() {
        let shared = Shared::default();
        assert!(parse_event(&shared, "not json").is_none());
        assert!(parse_event(&shared, "{\"uri\":\"/x\"}").is_none()); // missing data
    }

    #[test]
    fn loopback_tls_config_builds() {
        assert!(loopback_tls_config().is_ok());
    }

    #[test]
    fn shared_snapshot_clears_on_disconnect() {
        let shared = Shared::default();
        shared.update(|s| {
            s.connected = true;
            s.phase = "InProgress".into();
            s.champion_name = Some("Annie".into());
        });
        shared.set_disconnected();
        let snap = shared.snapshot.lock().unwrap().clone();
        assert!(!snap.connected);
        assert!(snap.champion_name.is_none());
    }

    #[test]
    fn shared_update_recovers_from_poisoned_lock() {
        let shared = Shared::default();
        shared.update(|s| {
            s.connected = true;
            s.phase = "InProgress".into();
        });

        // Simulate the worker panicking while holding the snapshot lock.
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = shared.snapshot.lock().unwrap();
            panic!("simulated worker panic");
        }));
        assert!(
            poisoned.is_err(),
            "test precondition: lock must be poisoned"
        );
        assert!(shared.snapshot.lock().is_err(), "lock is poisoned");

        // `update` must recover the poisoned lock and apply the write,
        // preserving and updating the pre-panic data instead of dropping it.
        shared.update(|s| {
            s.connected = false;
            s.phase = "Lobby".into();
        });

        // Reads must also recover the poisoned lock to observe the update.
        let snap = shared
            .snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        assert!(!snap.connected);
        assert_eq!(snap.phase, "Lobby", "write applied after poison recovery");

        // Recovery keeps working on every subsequent call, even though the
        // lock remains flagged poisoned at the API level.
        shared.update(|s| s.phase = "InProgress".into());
        assert_eq!(
            shared
                .snapshot
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .phase,
            "InProgress"
        );
    }

    #[test]
    fn transient_connect_failure_preserves_presence() {
        let shared = Arc::new(Shared::default());
        shared.update(|s| {
            s.connected = true;
            s.phase = "InProgress".into();
            s.champion_name = Some("Yasuo".into());
        });

        // A lockfile for the current (alive) process, pointing at a port with
        // nothing listening: the client is "detected" but every connect fails.
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "ph_league_transient_{}.lockfile",
            std::process::id()
        ));
        let dead_port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        std::fs::write(
            &path,
            format!(
                "LeagueClient:{}:{}:tok:https",
                std::process::id(),
                dead_port
            ),
        )
        .unwrap();
        let locator = LockfileLocator::new().with_extra([path.clone()]);

        let worker_shared = shared.clone();
        let stop_shared = shared.clone();
        let handle = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                // Stop the worker shortly after its first connect fails.
                let stopper = std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(400));
                    stop_shared.shutdown.store(true, Ordering::Relaxed);
                });
                run_loop(worker_shared, &locator).await;
                let _ = stopper.join();
            });
        });
        handle.join().unwrap();

        // The transient connect failure must not have cleared the live
        // snapshot: presence survives the reconnect window instead of the
        // plugin erroring out and ending the session.
        let snap = shared
            .snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        assert!(
            snap.connected,
            "presence preserved across transient connect failures"
        );
        assert_eq!(snap.phase, "InProgress");

        let _ = std::fs::remove_file(&path);
    }

    // -- logging on silent data loss -------------------------------------------

    /// A `Write` sink that captures bytes into a shared buffer, so tests can
    /// assert on what the worker logged.
    #[derive(Clone, Default)]
    struct CaptureWriter(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl CaptureWriter {
        fn contents(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    /// Builds a tracing subscriber that captures WARN+ events into a
    /// [`CaptureWriter`]. Returns the subscriber and its capture buffer.
    fn warn_capture() -> (impl tracing::Subscriber + 'static, CaptureWriter) {
        let writer = CaptureWriter::default();
        let writer_for_sub = writer.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || writer_for_sub.clone())
            .finish();
        (subscriber, writer)
    }

    #[test]
    fn parse_json_body_accepts_valid_json() {
        let value = parse_json_body(&Shared::default(), "/x", r#"{"a":1}"#);
        assert_eq!(value["a"], 1);
    }

    #[test]
    fn malformed_json_response_warns_and_yields_null() {
        let (subscriber, writer) = warn_capture();
        let mut parsed = serde_json::Value::Null;
        tracing::subscriber::with_default(subscriber, || {
            parsed = parse_json_body(
                &Shared::default(),
                "/lol-gameflow/v1/gameflow-phase",
                "not json{",
            );
        });

        assert_eq!(
            parsed,
            serde_json::Value::Null,
            "malformed JSON degrades to Null"
        );
        let output = writer.contents();
        assert!(
            output.contains("not valid JSON"),
            "parse failure must be logged, got: {output}"
        );
        assert!(
            output.contains("/lol-gameflow/v1/gameflow-phase"),
            "endpoint path must be included in the warning, got: {output}"
        );
    }

    #[test]
    fn best_effort_fetch_warns_on_transport_error() {
        let (subscriber, writer) = warn_capture();
        let http = build_http_client().expect("loopback TLS client builds");
        // A port with nothing listening: the request fails at the transport
        // layer (connection refused), which must be logged rather than
        // silently swallowed as missing context.
        let dead_port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let base = format!("https://127.0.0.1:{dead_port}");

        let mut result = None;
        tracing::subscriber::with_default(subscriber, || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            result = runtime.block_on(get_json_best_effort(
                &Shared::default(),
                &http,
                &base,
                "tok",
                "/lol-patch/v1/game-version",
            ));
        });

        assert!(
            result.is_none(),
            "transport failure yields None, not a panic"
        );
        let output = writer.contents();
        assert!(
            output.contains("Best-effort LCU fetch failed"),
            "transport failure must be logged, got: {output}"
        );
        assert!(
            output.contains("/lol-patch/v1/game-version"),
            "endpoint path must be included, got: {output}"
        );
    }

    // -- gameflow session parsing -----------------------------------------------

    #[test]
    fn parse_gameflow_session_extracts_queue_and_map() {
        // Shape captured live from a running Practice Tool game: the map id
        // lives inside `gameData.queue`, and there is no top-level mapId.
        let json = r#"{
            "gameId": 7944757944,
            "gameData": {
                "gameId": 7944757944,
                "queue": { "id": 3140, "name": "Multiplayer Practice Tool Custom", "mapId": 11 }
            },
            "phase": "InProgress"
        }"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let (queue_id, queue_name, map_id) = parse_gameflow_session(&v).unwrap();
        assert_eq!(queue_id, Some(3140));
        assert_eq!(
            queue_name.as_deref(),
            Some("Multiplayer Practice Tool Custom")
        );
        assert_eq!(map_id, Some(11));
    }

    #[test]
    fn parse_gameflow_session_maps_known_queue_canonically() {
        // A known queue id resolves to the canonical local name.
        let json = r#"{"gameData": { "queue": { "id": 2000 } }}"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let (queue_id, queue_name, _map_id) = parse_gameflow_session(&v).unwrap();
        assert_eq!(queue_id, Some(2000));
        assert_eq!(queue_name.as_deref(), Some("Practice Tool"));
    }

    #[test]
    fn parse_gameflow_session_missing_queue_is_safe() {
        let json = r#"{"phase": "Lobby"}"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let (queue_id, queue_name, map_id) = parse_gameflow_session(&v).unwrap();
        assert_eq!(queue_id, None);
        assert_eq!(queue_name, None);
        assert_eq!(map_id, None);
    }

    #[test]
    fn parse_gameflow_session_malformed_is_safe() {
        let v: serde_json::Value = serde_json::from_str(r#"{"gameData": "oops"}"#).unwrap();
        let parsed = parse_gameflow_session(&v);
        assert!(parsed.is_some(), "malformed gameData must not panic");
    }

    // -- champ-select session parsing -------------------------------------------

    #[test]
    fn parse_champ_select_extracts_local_player() {
        // Shape mirroring `/lol-champ-select/v1/session` (also the WebSocket
        // event payload): the local player's champion is found by matching
        // `localPlayerCellId` against `myTeam`.
        let json = r#"{
            "localPlayerCellId": 1,
            "queueId": 420,
            "mapId": 11,
            "myTeam": [
                { "cellId": 0, "championId": 0 },
                { "cellId": 1, "championId": 99 }
            ]
        }"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let (champion_id, queue_id, map_id) = parse_champ_select(&v);
        assert_eq!(champion_id, Some(99));
        assert_eq!(queue_id, Some(420));
        assert_eq!(map_id, Some(11));
    }

    #[test]
    fn parse_champ_select_ignores_unlocked_champion() {
        // championId == 0 (nothing locked) must map to None: Champion Select
        // is reported without requiring a locked champion.
        let json = r#"{
            "localPlayerCellId": 0,
            "queueId": 450,
            "mapId": 12,
            "myTeam": [ { "cellId": 0, "championId": 0 } ]
        }"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let (champion_id, queue_id, map_id) = parse_champ_select(&v);
        assert_eq!(champion_id, None);
        assert_eq!(queue_id, Some(450));
        assert_eq!(map_id, Some(12));
    }

    #[test]
    fn parse_champ_select_is_null_safe() {
        // A WebSocket deletion event delivers a null payload; parsing it must
        // not panic and must report no champion.
        let v: serde_json::Value = serde_json::Value::Null;
        let (champion_id, queue_id, map_id) = parse_champ_select(&v);
        assert_eq!(champion_id, None);
        assert_eq!(queue_id, None);
        assert_eq!(map_id, None);
    }

    // -- adaptive polling policy ------------------------------------------------

    #[test]
    fn volatile_phases_poll_fast() {
        // Matchmaking, Ready Check, Champion Select, and GameStart change in
        // seconds (dodge, deny, accept/cancel), so the fallback poll must run
        // at the fast cadence.
        for phase in ["Matchmaking", "ReadyCheck", "ChampSelect", "GameStart"] {
            assert_eq!(
                poll_interval_for_phase(phase),
                POLL_INTERVAL_VOLATILE,
                "{phase} should poll at the volatile cadence"
            );
        }
    }

    #[test]
    fn stable_phases_poll_at_regular_cadence() {
        // Lobby, InProgress, WaitingForStats, and the client home are stable
        // states; the fallback poll stays at the regular cadence.
        for phase in [
            "",
            "None",
            "Lobby",
            "InProgress",
            "Started",
            "WaitingForStats",
            "EndOfGame",
            "Reconnect",
            "TerminatedInError",
            "TotallyUnknown",
        ] {
            assert_eq!(
                poll_interval_for_phase(phase),
                POLL_INTERVAL,
                "{phase:?} should poll at the regular cadence"
            );
        }
    }

    #[test]
    fn volatile_cadence_is_faster_than_regular() {
        assert!(
            POLL_INTERVAL_VOLATILE < POLL_INTERVAL,
            "volatile cadence must be faster than the regular cadence"
        );
    }

    // -- phase application ------------------------------------------------------

    #[test]
    fn champ_select_null_preserves_champion_when_entering_game() {
        // Regression: when champ select ends (null event) as the game starts,
        // the champion must survive into the live game. The LCU does not
        // expose the local player's champion from an authoritative in-game
        // endpoint, so the champ-select value is the only source.
        let shared = Shared::default();
        shared.update(|s| {
            s.connected = true;
            s.phase = "InProgress".into();
            s.champion_id = Some(99);
            s.champion_name = Some("Lux".into());
        });

        // Simulate the champ-select session teardown event arriving while the
        // game is already live (the phase event and the session null event
        // can race; the phase may already be InProgress).
        let event = LcuEvent {
            uri: "/lol-champ-select/v1/session".to_string(),
            data: serde_json::Value::Null,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let http = build_http_client().expect("loopback TLS client builds");
        runtime.block_on(apply_event(
            &shared,
            &http,
            "https://127.0.0.1:1",
            "tok",
            &event,
        ));

        let snap = shared.snapshot.lock().unwrap().clone();
        assert_eq!(
            snap.champion_name.as_deref(),
            Some("Lux"),
            "champion must survive champ-select teardown into the live game"
        );
        assert_eq!(snap.champion_id, Some(99));
    }

    #[test]
    fn champ_select_null_clears_champion_on_dodge() {
        // A dodge/cancel/queue-leave tears down champ select while the player
        // is NOT in a game: the champion must be cleared.
        let shared = Shared::default();
        shared.update(|s| {
            s.connected = true;
            s.phase = "Lobby".into();
            s.champion_id = Some(99);
            s.champion_name = Some("Lux".into());
        });

        let event = LcuEvent {
            uri: "/lol-champ-select/v1/session".to_string(),
            data: serde_json::Value::Null,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let http = build_http_client().expect("loopback TLS client builds");
        runtime.block_on(apply_event(
            &shared,
            &http,
            "https://127.0.0.1:1",
            "tok",
            &event,
        ));

        let snap = shared.snapshot.lock().unwrap().clone();
        assert!(
            snap.champion_name.is_none(),
            "champion must be cleared on a dodge/cancel (not in a game)"
        );
        assert!(snap.champion_id.is_none());
    }

    #[test]
    fn champ_select_null_preserves_champion_when_phase_still_champ_select() {
        // ARAM regression: when champ select is torn down as the game loads,
        // the gameflow-phase event can arrive AFTER the session deletion. The
        // snapshot phase is therefore still "ChampSelect" at that moment, and
        // the champion must survive it (the phase will advance to GameStart/
        // InProgress next, which keeps the champion).
        let shared = Shared::default();
        shared.update(|s| {
            s.connected = true;
            s.phase = "ChampSelect".into();
            s.champion_id = Some(103);
            s.champion_name = Some("Ahri".into());
        });

        let event = LcuEvent {
            uri: "/lol-champ-select/v1/session".to_string(),
            data: serde_json::Value::Null,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let http = build_http_client().expect("loopback TLS client builds");
        runtime.block_on(apply_event(
            &shared,
            &http,
            "https://127.0.0.1:1",
            "tok",
            &event,
        ));

        let snap = shared.snapshot.lock().unwrap().clone();
        assert_eq!(
            snap.champion_name.as_deref(),
            Some("Ahri"),
            "champion must survive session teardown while the phase is still ChampSelect"
        );
        assert_eq!(snap.champion_id, Some(103));
    }

    #[test]
    fn aram_champion_survives_teardown_into_in_progress() {
        // End-to-end ARAM flow: the champion selected in champ select must
        // still be present once the game is InProgress, even when the session
        // deletion (null) arrives while the phase has not yet advanced past
        // ChampSelect. This is the exact event ordering observed on ARAM.
        let shared = Shared::default();
        shared.update(|s| {
            s.connected = true;
            s.phase = "ChampSelect".into();
            s.queue_id = Some(450);
            s.queue_name = Some("ARAM".into());
            s.map_id = Some(12);
            s.champion_id = Some(103);
            s.champion_name = Some("Ahri".into());
        });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let http = build_http_client().expect("loopback TLS client builds");

        // 1. Session torn down while the phase still reads ChampSelect.
        let teardown = LcuEvent {
            uri: "/lol-champ-select/v1/session".to_string(),
            data: serde_json::Value::Null,
        };
        runtime.block_on(apply_event(
            &shared,
            &http,
            "https://127.0.0.1:1",
            "tok",
            &teardown,
        ));

        // 2. The phase advances to GameStart (→ InProgress).
        let phase = LcuEvent {
            uri: "/lol-gameflow/v1/gameflow-phase".to_string(),
            data: serde_json::json!("GameStart"),
        };
        runtime.block_on(apply_event(
            &shared,
            &http,
            "https://127.0.0.1:1",
            "tok",
            &phase,
        ));

        let snap = shared.snapshot.lock().unwrap().clone();
        assert_eq!(snap.phase, "GameStart");
        assert_eq!(
            snap.champion_name.as_deref(),
            Some("Ahri"),
            "ARAM champion selected in champ select must survive into the live game"
        );
        assert_eq!(snap.champion_id, Some(103));
        assert_eq!(snap.map_id, Some(12));
        assert_eq!(snap.queue_name.as_deref(), Some("ARAM"));
    }

    #[test]
    fn apply_phase_clears_game_context_on_client_home() {
        let mut snap = LeagueSnapshot {
            connected: true,
            phase: "InProgress".into(),
            queue_id: Some(3140),
            queue_name: Some("Multiplayer Practice Tool Custom".into()),
            champion_name: Some("Annie".into()),
            map_id: Some(11),
            ..LeagueSnapshot::default()
        };
        apply_phase(&mut snap, "None");
        assert_eq!(snap.phase, "None");
        assert!(
            snap.queue_id.is_none(),
            "stale game queue must clear at client home"
        );
        assert!(snap.queue_name.is_none());
        assert!(snap.champion_name.is_none());
        assert!(snap.map_id.is_none());
    }

    #[test]
    fn apply_phase_preserves_context_during_reconnect() {
        let mut snap = LeagueSnapshot {
            connected: true,
            phase: "InProgress".into(),
            queue_id: Some(2000),
            queue_name: Some("Practice Tool".into()),
            champion_name: Some("Lux".into()),
            map_id: Some(11),
            ..LeagueSnapshot::default()
        };
        apply_phase(&mut snap, "Reconnect");
        assert_eq!(snap.phase, "Reconnect");
        assert_eq!(snap.champion_name.as_deref(), Some("Lux"));
        assert_eq!(snap.queue_id, Some(2000));
        assert_eq!(snap.map_id, Some(11));
    }

    #[test]
    fn apply_phase_preserves_post_game_context_for_waiting_stats() {
        let mut snap = LeagueSnapshot {
            connected: true,
            phase: "InProgress".into(),
            champion_name: Some("Yasuo".into()),
            map_id: Some(11),
            ..LeagueSnapshot::default()
        };
        apply_phase(&mut snap, "WaitingForStats");
        assert_eq!(snap.phase, "WaitingForStats");
        assert_eq!(
            snap.champion_name.as_deref(),
            Some("Yasuo"),
            "post-game context kept"
        );
        assert_eq!(snap.map_id, Some(11));
    }

    // -- queue name preservation -----------------------------------------------

    #[test]
    fn apply_lobby_queue_keeps_gameflow_name_for_unknown_id() {
        // Regression: the lobby refresh must not clobber the gameflow-provided
        // name for an unknown queue id (Practice Tool is 3140 in the wild).
        let mut snap = LeagueSnapshot {
            queue_id: Some(3140),
            queue_name: Some("Multiplayer Practice Tool Custom".into()),
            ..LeagueSnapshot::default()
        };
        apply_lobby_queue(&mut snap, Some(3140));
        assert_eq!(snap.queue_id, Some(3140));
        assert_eq!(
            snap.queue_name.as_deref(),
            Some("Multiplayer Practice Tool Custom"),
            "unknown-id lobby must keep the client-provided queue name"
        );
    }

    #[test]
    fn apply_lobby_queue_uses_canonical_name_for_known_id() {
        let mut snap = LeagueSnapshot::default();
        apply_lobby_queue(&mut snap, Some(2000));
        assert_eq!(snap.queue_id, Some(2000));
        assert_eq!(snap.queue_name.as_deref(), Some("Practice Tool"));
    }

    #[test]
    fn apply_lobby_queue_no_id_clears_only_queue_id() {
        let mut snap = LeagueSnapshot {
            queue_id: Some(3140),
            queue_name: Some("Multiplayer Practice Tool Custom".into()),
            ..LeagueSnapshot::default()
        };
        apply_lobby_queue(&mut snap, None);
        assert_eq!(snap.queue_id, None);
        assert_eq!(
            snap.queue_name.as_deref(),
            Some("Multiplayer Practice Tool Custom"),
            "a 404 must not erase the gameflow name either"
        );
    }

    #[test]
    fn apply_champ_select_queue_keeps_gameflow_name_for_unknown_id() {
        // Regression: champ select was overwriting queue_name with the numeric
        // fallback even when the gameflow session already named the queue.
        let mut snap = LeagueSnapshot {
            queue_id: Some(3140),
            queue_name: Some("Multiplayer Practice Tool Custom".into()),
            ..LeagueSnapshot::default()
        };
        apply_champ_select_queue(&mut snap, Some(3140));
        assert_eq!(snap.queue_id, Some(3140));
        assert_eq!(
            snap.queue_name.as_deref(),
            Some("Multiplayer Practice Tool Custom"),
            "unknown-id champ select must keep the client-provided queue name"
        );
    }

    #[test]
    fn apply_champ_select_queue_uses_canonical_name_for_known_id() {
        let mut snap = LeagueSnapshot {
            queue_name: Some("Multiplayer Practice Tool Custom".into()),
            ..LeagueSnapshot::default()
        };
        apply_champ_select_queue(&mut snap, Some(2000));
        assert_eq!(snap.queue_name.as_deref(), Some("Practice Tool"));
    }

    #[test]
    fn warn_dedup_logs_on_transition_only() {
        let mut w = WarnDedup::default();

        // First failure logs; identical repeats are silent.
        assert!(w.should_warn("connection refused"), "first failure logs");
        assert!(
            !w.should_warn("connection refused"),
            "identical repeat is silent"
        );
        assert!(
            !w.should_warn("connection refused"),
            "still silent while unchanged"
        );

        // A changed error message is a new transition and logs again.
        assert!(
            w.should_warn("TLS handshake failed"),
            "changed message logs"
        );
        assert!(!w.should_warn("TLS handshake failed"));

        // After a successful connect (or client disappearing), the tracker
        // re-arms so a fresh failure logs again.
        w.cleared();
        assert!(
            w.should_warn("connection refused"),
            "re-arm after recovery logs"
        );
        assert!(!w.should_warn("connection refused"));
    }

    #[test]
    fn shared_dedup_is_transition_based_and_rearms() {
        let shared = Shared::default();

        assert!(
            shared.should_warn("GET /x: connection refused"),
            "first logs"
        );
        assert!(
            !shared.should_warn("GET /x: connection refused"),
            "persistent is silent"
        );

        // A distinct message is its own transition even while the first
        // persists; keying by message means endpoints dedup independently.
        assert!(
            shared.should_warn("GET /y: connection refused"),
            "per-endpoint transitions"
        );
        assert!(!shared.should_warn("GET /y: connection refused"));

        // Re-arm (successful reconnect / client disappearing): the original
        // failure logs again the next time it is seen.
        shared.warn_cleared();
        assert!(shared.should_warn("GET /x: connection refused"), "re-armed");
    }

    #[test]
    fn shared_warn_cleared_recovers_from_poisoned_lock() {
        let shared = Shared::default();
        shared.should_warn("boom");

        // Simulate the worker panicking while holding the tracker lock.
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = shared.warnings.lock().unwrap();
            panic!("simulated worker panic");
        }));
        assert!(
            poisoned.is_err(),
            "test precondition: lock must be poisoned"
        );

        // Recovery must keep dedup working, not permanently disable it.
        shared.warn_cleared();
        assert!(
            shared.should_warn("boom"),
            "recovery re-arms and dedups again"
        );
        assert!(!shared.should_warn("boom"));
    }

    #[test]
    fn parse_event_warns_on_malformed_frame_and_dedups() {
        let (subscriber, writer) = warn_capture();
        let shared = Shared::default();
        tracing::subscriber::with_default(subscriber, || {
            // A malformed frame is dropped, but the drop is logged.
            assert!(parse_event(&shared, "not json").is_none());
            // Persistent malformed frames stay silent (transition-deduped).
            assert!(parse_event(&shared, "not json").is_none());
            assert!(parse_event(&shared, "not json").is_none());
        });

        let output = writer.contents();
        assert!(
            output.contains("malformed LCU websocket frame"),
            "malformed frame must be logged, got: {output}"
        );
        // Exactly one warning for the persistent failure.
        assert_eq!(
            output.matches("malformed LCU websocket frame").count(),
            1,
            "persistent malformed frame must warn once, got: {output}"
        );
    }

    #[test]
    fn parse_event_warns_on_frame_missing_uri_or_data() {
        let (subscriber, writer) = warn_capture();
        let shared = Shared::default();
        tracing::subscriber::with_default(subscriber, || {
            assert!(parse_event(&shared, "{\"data\":1}").is_none()); // missing uri
            assert!(parse_event(&shared, "{\"data\":1}").is_none()); // deduped
            assert!(parse_event(&shared, "{\"uri\":\"/x\"}").is_none()); // missing data
        });

        let output = writer.contents();
        assert!(
            output.contains("without a uri"),
            "missing uri must be logged"
        );
        assert!(
            output.contains("without data"),
            "missing data must be logged, got: {output}"
        );
    }

    #[test]
    fn champion_name_fetch_warns_on_transport_failure_and_dedups() {
        let (subscriber, writer) = warn_capture();
        let http = build_http_client().expect("loopback TLS client builds");
        let dead_port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let base = format!("https://127.0.0.1:{dead_port}");
        let shared = Arc::new(Shared::default());

        let mut names = Vec::new();
        tracing::subscriber::with_default(subscriber, || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            names = runtime.block_on(async {
                let first = fetch_champion_name(shared.as_ref(), &http, &base, "tok", 1).await;
                // A repeat of the same persistent transport failure must not
                // be logged again.
                let second = fetch_champion_name(shared.as_ref(), &http, &base, "tok", 1).await;
                vec![first, second]
            });
        });

        assert!(
            names.iter().all(Option::is_none),
            "transport failure yields None"
        );
        let output = writer.contents();
        assert!(
            output.contains("Best-effort champion name fetch failed"),
            "champion-name transport failure must be logged, got: {output}"
        );
        assert_eq!(
            output
                .matches("Best-effort champion name fetch failed")
                .count(),
            1,
            "persistent champion-name failure must warn once, got: {output}"
        );
    }
}
