//! Connection pool: one live transport per profile, created lazily and reused.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use uuid::Uuid;

use super::connection::{AuthFailed, Connection, Stage, connect};
use crate::keystore::BadPassphrase;
use crate::known_hosts::KnownHosts;
use crate::profile::{AuthMethod, Profile, secret};

type Slot = Arc<tokio::sync::Mutex<Option<Arc<Connection>>>>;

#[derive(Clone)]
pub struct Pool {
    slots: Arc<Mutex<HashMap<Uuid, Slot>>>,
    known_hosts: Arc<Mutex<KnownHosts>>,
}

impl Pool {
    pub fn new(known_hosts: KnownHosts) -> Self {
        Self { slots: Arc::default(), known_hosts: Arc::new(Mutex::new(known_hosts)) }
    }

    fn slot(&self, id: Uuid) -> Slot {
        let mut slots = self.slots.lock().expect("pool lock poisoned");
        slots.entry(id).or_default().clone()
    }

    /// True if a usable connection exists (no credentials needed to open more channels).
    pub fn has_live(&self, id: Uuid) -> bool {
        let slot = self.slot(id);
        let guard = slot.try_lock();
        matches!(guard, Ok(g) if g.as_ref().is_some_and(|c| !c.is_closed()))
    }

    /// Returns the shared connection, connecting if needed. Concurrent callers for the
    /// same profile wait on the same connect instead of racing a second handshake.
    pub async fn get(&self, profile: &Profile, secret: Option<String>) -> Result<Arc<Connection>> {
        self.get_staged(profile, secret, &|_| {}).await
    }

    /// [`Self::get`], reporting progress to `on_stage` (for the loading screens).
    pub async fn get_staged(&self, profile: &Profile, secret: Option<String>, on_stage: &(dyn Fn(Stage) + Send + Sync)) -> Result<Arc<Connection>> {
        let slot = self.slot(profile.id);
        let mut guard = slot.lock().await;
        // Only the endpoint matters: comparing whole profiles (which carry `last_used`,
        // labels, colours) made every second tab of a host redo the full handshake.
        if let Some(conn) = guard.as_ref().filter(|c| !c.is_closed() && c.profile.same_endpoint(profile)) {
            return Ok(conn.clone());
        }
        match connect(profile, secret.clone(), self.known_hosts.clone(), on_stage).await {
            Ok(conn) => {
                *guard = Some(conn.clone());
                Ok(conn)
            }
            Err(e) => {
                // A rejected stored secret would otherwise be replayed forever without
                // ever prompting again.
                let rejected =
                    (profile.auth == AuthMethod::Password && e.downcast_ref::<AuthFailed>().is_some()) || e.downcast_ref::<BadPassphrase>().is_some();
                // Only forget what was actually rejected: a typo in a prompt must not wipe
                // the remembered passphrase other hosts using the same key rely on.
                let id = profile.secret_id();
                if rejected && secret.is_some() && secret::get(id) == secret {
                    let _ = secret::delete(id);
                }
                Err(e)
            }
        }
    }

    /// One-off connect + authenticate with `profile` (the host editor's "Test connection"),
    /// without touching the shared slot. Returns how long the handshake took.
    pub async fn probe(&self, profile: &Profile, secret: Option<String>, on_stage: &(dyn Fn(Stage) + Send + Sync)) -> Result<std::time::Duration> {
        let started = std::time::Instant::now();
        let conn = connect(profile, secret, self.known_hosts.clone(), on_stage).await?;
        let took = started.elapsed();
        conn.disconnect().await;
        Ok(took)
    }

    /// Disconnects once the slot is free, unless `still_unused` says a tab claimed the
    /// profile again in the meantime.
    pub async fn drop_connection(&self, id: Uuid, still_unused: impl Fn() -> bool) {
        let slot = self.slot(id);
        let mut guard = slot.lock().await;
        if !still_unused() {
            return;
        }
        if let Some(conn) = guard.take() {
            conn.disconnect().await;
        }
    }

    pub fn forget_host_key(&self, host: &str, port: u16) -> Result<()> {
        self.known_hosts.lock().expect("known_hosts lock poisoned").forget(host, port)
    }
}
