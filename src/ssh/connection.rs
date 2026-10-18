//! One authenticated SSH transport per host, shared by every terminal and SFTP pane
//! opened against it (Bitvise model: one connection, many channels).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use russh::client::{self, AuthResult, KeyboardInteractiveAuthResponse};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate, load_secret_key};
use russh::{Disconnect, Preferred};

use crate::known_hosts::{HostKeyVerdict, KnownHosts};
use crate::profile::{AuthMethod, Profile};

/// Large receive window keeps the pipe full on high-latency links.
const WINDOW_SIZE: u32 = 16 * 1024 * 1024;
/// SSH packets beyond a TCP segment buy nothing; russh warns above this.
const MAX_PACKET: u32 = 65535;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// A server that stalls mid-auth must not hold the pool slot forever.
const AUTH_TIMEOUT: Duration = Duration::from_secs(30);

/// The server rejected the credentials (as opposed to a network/protocol failure).
#[derive(Debug)]
pub struct AuthFailed(pub String);

impl std::fmt::Display for AuthFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Authentication failed ({})", self.0)
    }
}

impl std::error::Error for AuthFailed {}

/// Where a connect is, for the loading screens. Reported in order; a reused connection
/// skips straight to `Opening`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// TCP connect and key exchange.
    Connecting,
    Authenticating,
    /// Opening the shell / SFTP channel on the (possibly shared) transport.
    Opening,
    /// First directory listing (SFTP only).
    Loading,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Stage::Connecting => "Reaching server",
            Stage::Authenticating => "Authenticating",
            Stage::Opening => "Opening session",
            Stage::Loading => "Loading files",
        }
    }
}

const KEEPALIVE: Duration = Duration::from_secs(20);

pub struct ClientHandler {
    host: String,
    port: u16,
    known_hosts: Arc<Mutex<KnownHosts>>,
    verdict: Arc<Mutex<Option<(HostKeyVerdict, String)>>>,
}

impl client::Handler for ClientHandler {
    type Error = anyhow::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let fingerprint = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.fingerprint(HashAlg::Sha256),
            PublicKeyOrCertificate::Certificate(cert) => cert.public_key_fingerprint(),
        }
        .to_string();
        let verdict = self.known_hosts.lock().map_err(|_| anyhow!("known_hosts lock poisoned"))?.verify(&self.host, self.port, &fingerprint)?;
        let accepted = !matches!(verdict, HostKeyVerdict::Mismatch { .. });
        *self.verdict.lock().map_err(|_| anyhow!("verdict lock poisoned"))? = Some((verdict, fingerprint));
        Ok(accepted)
    }
}

trait CertFingerprint {
    fn public_key_fingerprint(&self) -> russh::keys::ssh_key::Fingerprint;
}

impl CertFingerprint for russh::keys::Certificate {
    fn public_key_fingerprint(&self) -> russh::keys::ssh_key::Fingerprint {
        self.public_key().fingerprint(HashAlg::Sha256)
    }
}

/// Runs `command` on its own exec channel; returns stdout (capped at `max_output` bytes)
/// and the exit status.
pub async fn exec(conn: &Connection, command: String, max_output: usize) -> Result<(Vec<u8>, Option<u32>)> {
    let mut channel = conn.handle.channel_open_session().await?;
    channel.exec(true, command).await?;
    let (mut out, mut status) = (Vec::new(), None);
    while let Some(msg) = channel.wait().await {
        match msg {
            russh::ChannelMsg::Data { data } if out.len() < max_output => out.extend_from_slice(&data),
            russh::ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
            russh::ChannelMsg::Close => break,
            _ => {}
        }
    }
    Ok((out, status))
}

pub struct Connection {
    pub handle: client::Handle<ClientHandler>,
    pub profile: Profile,
    /// Set when the host key was learned on this connect (shown to the user once).
    pub learned_fingerprint: Mutex<Option<String>>,
}

impl Connection {
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    pub async fn disconnect(&self) {
        let _ = self.handle.disconnect(Disconnect::ByApplication, "bye", "en").await;
    }
}

fn client_config() -> client::Config {
    client::Config {
        window_size: WINDOW_SIZE,
        maximum_packet_size: MAX_PACKET,
        keepalive_interval: Some(KEEPALIVE),
        keepalive_max: 3,
        nodelay: true,
        preferred: Preferred::default(),
        ..Default::default()
    }
}

pub async fn connect(
    profile: &Profile,
    secret: Option<String>,
    known_hosts: Arc<Mutex<KnownHosts>>,
    on_stage: &(dyn Fn(Stage) + Send + Sync),
) -> Result<Arc<Connection>> {
    on_stage(Stage::Connecting);
    let verdict = Arc::new(Mutex::new(None));
    let handler = ClientHandler { host: profile.host.clone(), port: profile.port, known_hosts, verdict: verdict.clone() };
    let addr = (profile.host.as_str(), profile.port);
    let connected = tokio::time::timeout(CONNECT_TIMEOUT, client::connect(Arc::new(client_config()), addr, handler)).await;

    let host_verdict = verdict.lock().ok().and_then(|v| v.clone());
    let mut handle = match connected {
        Err(_) => bail!("Timed out connecting to {}:{}", profile.host, profile.port),
        Ok(Err(e)) => {
            if let Some((HostKeyVerdict::Mismatch { expected }, got)) = host_verdict {
                bail!(
                    "HOST KEY CHANGED for {}:{} — expected {expected}, got {got}. \
                     Refusing to connect (possible man-in-the-middle).",
                    profile.host,
                    profile.port
                );
            }
            return Err(e).with_context(|| format!("connecting to {}:{}", profile.host, profile.port));
        }
        Ok(Ok(h)) => h,
    };

    on_stage(Stage::Authenticating);
    tokio::time::timeout(AUTH_TIMEOUT, authenticate(&mut handle, profile, secret))
        .await
        .map_err(|_| anyhow!("Timed out during authentication with {}", profile.host))??;

    let learned_fingerprint = match host_verdict {
        Some((HostKeyVerdict::Learned, fp)) => Some(fp),
        _ => None,
    };
    Ok(Arc::new(Connection { handle, profile: profile.clone(), learned_fingerprint: Mutex::new(learned_fingerprint) }))
}

async fn authenticate(handle: &mut client::Handle<ClientHandler>, profile: &Profile, secret: Option<String>) -> Result<()> {
    let user = profile.user.clone();
    match &profile.auth {
        AuthMethod::Key { path } => {
            let key = load_secret_key(path, secret.as_deref().filter(|s| !s.is_empty())).with_context(|| format!("loading private key {}", path.display()))?;
            let hash = handle.best_supported_rsa_hash().await?.flatten();
            let res = handle.authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash)).await?;
            ensure_success(res, "public key")
        }
        AuthMethod::Managed { key } => {
            let store = crate::keystore::KeyStore::load(crate::keystore::default_dir())?;
            let name = store.get(*key).map(|k| k.name.clone()).ok_or_else(|| anyhow!("The key used by this host was deleted from the key manager"))?;
            let private = store.load_private(*key, secret.as_deref()).with_context(|| format!("unlocking key “{name}”"))?;
            let hash = handle.best_supported_rsa_hash().await?.flatten();
            let res = handle.authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(private), hash)).await?;
            ensure_success(res, "public key")
        }
        AuthMethod::Password => {
            let password = secret.ok_or_else(|| anyhow!("Password required"))?;
            let res = handle.authenticate_password(user.clone(), password.clone()).await?;
            if res.success() {
                return Ok(());
            }
            // Many servers only expose keyboard-interactive for passwords.
            keyboard_interactive(handle, user, &password).await
        }
    }
}

fn ensure_success(res: AuthResult, method: &str) -> Result<()> {
    if res.success() { Ok(()) } else { Err(AuthFailed(format!("{method} rejected")).into()) }
}

async fn keyboard_interactive(handle: &mut client::Handle<ClientHandler>, user: String, password: &str) -> Result<()> {
    const MAX_ROUNDS: usize = 4;
    let mut reply = handle.authenticate_keyboard_interactive_start(user, None::<String>).await?;
    let mut answered = false;
    for _ in 0..MAX_ROUNDS {
        match reply {
            KeyboardInteractiveAuthResponse::Success => return Ok(()),
            KeyboardInteractiveAuthResponse::Failure { .. } => break,
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                // Only the password is known: send it once, to a lone prompt, and never
                // to a follow-up (one-time code) prompt.
                if prompts.len() > 1 || (answered && !prompts.is_empty()) {
                    let asked = prompts.iter().map(|p| p.prompt.trim()).collect::<Vec<_>>().join(", ");
                    bail!(AuthFailed(format!("server asked for “{asked}” (two-factor prompts aren't supported yet)")));
                }
                answered |= !prompts.is_empty();
                let answers = prompts.iter().map(|_| password.to_string()).collect();
                reply = handle.authenticate_keyboard_interactive_respond(answers).await?;
            }
        }
    }
    Err(AuthFailed("wrong username or password".into()).into())
}
