//! Product-private finite key operation state; no new queue or secret history.
use super::*;
use crate::provider_server::ClientKeyPublication;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileKeyOperation {
    Create,
    Copy,
    Revoke,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileKeyState {
    NotCreated,
    Created,
    Revoked,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileKeyMetadata {
    pub profile: String,
    pub state: ProfileKeyState,
    pub exists: bool,
    pub epoch: u64,
    pub durability_unconfirmed: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileKeyReceipt {
    pub profile: String,
    pub request: u64,
    pub operation: ProfileKeyOperation,
    pub publication: Option<ClientKeyPublication>,
    pub result: Result<(), RuntimeError>,
}
pub struct ClientKeyCopy {
    pub profile: String,
    pub request: u64,
    pub key_epoch: u64,
    stop_epoch: u64,
    secret: Secret,
}
impl std::fmt::Debug for ClientKeyCopy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClientKeyCopy([REDACTED])")
    }
}
impl ClientKeyCopy {
    pub fn expose(&self) -> &str {
        self.secret.expose()
    }
}

#[derive(Clone, Copy)]
pub(super) struct KeyFlight {
    request: u64,
    epoch: u64,
    operation: ProfileKeyOperation,
    deliverable: bool,
}
#[derive(Default)]
pub(super) struct KeyPending {
    last_request: u64,
    flights: BTreeMap<String, KeyFlight>,
    delivery_errors: BTreeMap<String, (u64, RuntimeError)>,
}
pub(super) struct KeyState {
    epoch: u64,
    revoked: bool,
    durability_unconfirmed: bool,
}
impl Default for KeyState {
    fn default() -> Self {
        Self {
            epoch: 1,
            revoked: false,
            durability_unconfirmed: false,
        }
    }
}

const WARNING: &str = "Local key publication is effective; crash durability is unconfirmed.";
fn applied(config: &Config, profile: &str) -> bool {
    profile == "default" || config.profiles.iter().any(|row| row.id == profile)
}
fn key_path(root: &Path, config: &Config, profile: &str) -> Result<PathBuf, RuntimeError> {
    if !applied(config, profile)
        || (profile != "default" && !crate::provider_server::opaque_id(profile))
    {
        return Err(RuntimeError::InvalidAction);
    }
    Ok(root.join(if profile == "default" {
        "client-key".into()
    } else {
        format!("client-key-{profile}")
    }))
}

impl Runtime {
    /// Read-only local handoff admission; no queue, key I/O or paid work.
    pub fn profile_key_handoff_ready(&self, profile: &str) -> Result<(), RuntimeError> {
        let pending = self.pending.lock().map_err(|_| RuntimeError::Storage)?;
        if pending.batch_gate.reserved() || !pending.keys.flights.is_empty() {
            return Err(RuntimeError::Busy);
        }
        if !self
            .view
            .profile_keys
            .iter()
            .any(|row| row.profile == profile && row.exists)
        {
            return Err(RuntimeError::MissingClientKey);
        }
        Ok(())
    }
    /// Candidate for one explicit key click; only successful act enqueue claims
    /// it. This sequence is independent of connection draft/proof requests.
    pub fn next_profile_key_request(&self) -> Result<u64, RuntimeError> {
        self.pending
            .lock()
            .map_err(|_| RuntimeError::Storage)?
            .keys
            .last_request
            .checked_add(1)
            .ok_or(RuntimeError::InvalidAction)
    }
    pub(super) fn keys_reserved(&self) -> bool {
        !self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .keys
            .flights
            .is_empty()
    }
    pub(super) fn key_action(&mut self, action: Action) -> Result<(), RuntimeError> {
        let (request, profile, operation) = match action {
            Action::ProfileKey {
                request,
                profile,
                operation,
            } => (request, profile, operation),
            Action::CreateClientKey | Action::CopyClientKey | Action::RevokeClientKey => {
                let operation = match action {
                    Action::CreateClientKey => ProfileKeyOperation::Create,
                    Action::CopyClientKey => ProfileKeyOperation::Copy,
                    _ => ProfileKeyOperation::Revoke,
                };
                let request = self.next_profile_key_request()?;
                (request, "default".into(), operation)
            }
            _ => return Err(RuntimeError::InvalidAction),
        };
        if request == 0 || (profile != "default" && !crate::provider_server::opaque_id(&profile)) {
            return Err(RuntimeError::InvalidAction);
        }
        if !self
            .view
            .profile_keys
            .iter()
            .any(|row| row.profile == profile)
        {
            return Err(RuntimeError::InvalidAction);
        }
        let gate = self
            .pending
            .lock()
            .map_err(|_| RuntimeError::Storage)?
            .batch_gate
            .clone();
        if gate.reserved() {
            return Err(RuntimeError::Busy);
        }
        let mut pending = self.pending.lock().map_err(|_| RuntimeError::Storage)?;
        if request <= pending.keys.last_request {
            return Err(RuntimeError::InvalidAction);
        }
        if let Some(flight) = pending.keys.flights.get_mut(&profile) {
            // A later explicit mutation intent may not let an earlier Copy
            // escape while this producer reports Busy. It is not queued/replayed.
            if operation != ProfileKeyOperation::Copy {
                flight.deliverable = false;
            }
            return Err(RuntimeError::Busy);
        }
        if pending.keys.flights.len() >= 8 {
            return Err(RuntimeError::Busy);
        }
        let epoch = self.stop_requested.load(Ordering::Acquire);
        let delivered = self
            .view
            .connection_receipts
            .iter()
            .map(|receipt| receipt.request)
            .collect();
        self.commands
            .try_send((
                epoch,
                delivered,
                Action::ProfileKey {
                    request,
                    profile: profile.clone(),
                    operation,
                },
            ))
            .map_err(|error| match error {
                TrySendError::Full(_) => RuntimeError::Busy,
                TrySendError::Disconnected(_) => RuntimeError::WorkerUnavailable,
            })?;
        pending.keys.flights.insert(
            profile.clone(),
            KeyFlight {
                request,
                epoch,
                operation,
                deliverable: true,
            },
        );
        pending.keys.delivery_errors.remove(&profile);
        pending.keys.last_request = request;
        Ok(())
    }
    pub(super) fn consume_key_effects(
        &mut self,
        pending: &mut Pending,
        effects: Vec<Effect>,
    ) -> Vec<Effect> {
        let mut delivered = BTreeMap::new();
        let mut output = Vec::new();
        for effect in effects {
            if let Effect::CopyClientKey(copy) = effect {
                let current = self.view.profile_keys.iter().any(|row| {
                    row.profile == copy.profile && row.exists && row.epoch == copy.key_epoch
                });
                let valid = current
                    && pending
                        .keys
                        .flights
                        .get(&copy.profile)
                        .is_some_and(|flight| {
                            flight.request == copy.request
                                && flight.epoch == copy.stop_epoch
                                && flight.deliverable
                                && flight.operation == ProfileKeyOperation::Copy
                        })
                    && copy.stop_epoch == self.stop_requested.load(Ordering::Acquire);
                if valid {
                    delivered.insert(copy.profile.clone(), copy.request);
                    output.push(Effect::CopyClientKey(copy));
                } else {
                    pending.keys.delivery_errors.insert(
                        copy.profile.clone(),
                        (copy.request, RuntimeError::Cancelled),
                    );
                }
            } else {
                output.push(effect);
            }
        }
        for receipt in &mut self.view.profile_key_receipts {
            if let Some(flight) = pending
                .keys
                .flights
                .get(&receipt.profile)
                .filter(|flight| flight.request == receipt.request)
            {
                if receipt.operation == ProfileKeyOperation::Copy
                    && receipt.result.is_ok()
                    && delivered.get(&receipt.profile) != Some(&receipt.request)
                {
                    pending
                        .keys
                        .delivery_errors
                        .entry(receipt.profile.clone())
                        .or_insert((
                            receipt.request,
                            if flight.deliverable {
                                RuntimeError::Busy
                            } else {
                                RuntimeError::Cancelled
                            },
                        ));
                }
                pending.keys.flights.remove(&receipt.profile);
            }
            if let Some((request, error)) = pending.keys.delivery_errors.get(&receipt.profile)
                && *request == receipt.request
            {
                receipt.result = Err(*error);
            }
        }
        output
    }
}

fn publish_private_key(
    path: &Path,
    key: Option<&Secret>,
) -> Result<ClientKeyPublication, ServerError> {
    if let Some(key) = key {
        match minicon_platform::filesystem_publish::write_private_atomic(
            path,
            key.expose().as_bytes(),
        ) {
            Ok(()) => Ok(ClientKeyPublication::Durable),
            Err(error) if error.published() => Ok(ClientKeyPublication::DurabilityUnconfirmed),
            Err(_) => Err(ServerError::Storage),
        }
    } else {
        match minicon_platform::filesystem_publish::remove_private_file(path) {
            Ok(minicon_platform::filesystem_publish::PrivateRemoveOutcome::Removed) => {
                Ok(ClientKeyPublication::Durable)
            }
            Ok(minicon_platform::filesystem_publish::PrivateRemoveOutcome::NotFound) => {
                Ok(ClientKeyPublication::NoChange)
            }
            Err(error) if error.published() => Ok(ClientKeyPublication::DurabilityUnconfirmed),
            Err(_) => Err(ServerError::Storage),
        }
    }
}

impl Owner {
    fn prepare_key_receipt(&mut self, profile: &str) -> Result<(), RuntimeError> {
        if self.key_receipts.contains_key(profile) || self.key_receipts.len() < 8 {
            return Ok(());
        }
        let unresolved = self
            .progress
            .as_ref()
            .map(|sink| {
                sink.pending
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .keys
                    .flights
                    .clone()
            })
            .unwrap_or_default();
        // Only already-delivered historical results may be discarded. Real
        // Runtime admission bounds unresolved requests at eight before enqueue.
        self.key_receipts.retain(|id, result| {
            unresolved
                .get(id)
                .is_some_and(|flight| flight.request == result.request)
        });
        if self.key_receipts.len() >= 8 {
            Err(RuntimeError::Busy)
        } else {
            Ok(())
        }
    }
    fn private_key(&self, profile: &str) -> Option<&Secret> {
        if profile == "default" {
            self.key.as_ref()
        } else {
            self.client_keys.get(profile)
        }
    }
    pub(super) fn project_profile_keys(&mut self) {
        let ids = std::iter::once("default".to_owned())
            .chain(self.config.profiles.iter().map(|row| row.id.clone()))
            .collect::<Vec<_>>();
        self.key_states.retain(|id, _| ids.contains(id));
        for id in &ids {
            self.key_states.entry(id.clone()).or_default();
        }
        self.view.profile_keys = ids
            .iter()
            .map(|id| {
                let state = &self.key_states[id];
                let exists = self.private_key(id).is_some();
                ProfileKeyMetadata {
                    profile: id.clone(),
                    state: if exists {
                        ProfileKeyState::Created
                    } else if state.revoked {
                        ProfileKeyState::Revoked
                    } else {
                        ProfileKeyState::NotCreated
                    },
                    exists,
                    epoch: state.epoch,
                    durability_unconfirmed: state.durability_unconfirmed,
                }
            })
            .collect();
        self.view.profile_key_receipts = self.key_receipts.values().cloned().collect();
        if self
            .view
            .profile_keys
            .iter()
            .any(|row| row.durability_unconfirmed)
        {
            self.view.status = Some(WARNING.into());
        } else if self.view.status.as_deref() == Some(WARNING) {
            self.view.status = None;
        }
    }
    pub(super) fn reject_key(
        &mut self,
        request: u64,
        profile: String,
        operation: ProfileKeyOperation,
        error: RuntimeError,
    ) {
        if self.prepare_key_receipt(&profile).is_err() {
            self.view.status = Some(RuntimeError::Busy.to_string());
            return;
        }
        self.key_receipts.insert(
            profile.clone(),
            ProfileKeyReceipt {
                profile,
                request,
                operation,
                publication: None,
                result: Err(error),
            },
        );
        self.view.status = Some(error.to_string());
        self.project_profile_keys();
    }
    pub(super) fn perform_key(
        &mut self,
        request: u64,
        profile: String,
        operation: ProfileKeyOperation,
        expected_epoch: u64,
    ) -> Option<Effect> {
        self.perform_key_with(
            request,
            profile,
            operation,
            expected_epoch,
            publish_private_key,
        )
    }
    pub(super) fn perform_key_with(
        &mut self,
        request: u64,
        profile: String,
        operation: ProfileKeyOperation,
        expected_epoch: u64,
        publish: impl FnOnce(&Path, Option<&Secret>) -> Result<ClientKeyPublication, ServerError>,
    ) -> Option<Effect> {
        self.view.status = None;
        let mut publication = None;
        let result = (|| {
            if self.batch_gate.reserved() {
                return Err(RuntimeError::Busy);
            }
            if expected_epoch != self.stop_epoch.load(Ordering::Acquire) {
                return Err(RuntimeError::Cancelled);
            }
            if request == 0 || request <= self.last_key_request {
                return Err(RuntimeError::InvalidAction);
            }
            self.last_key_request = request;
            self.prepare_key_receipt(&profile)?;
            let path = key_path(&self.root, &self.config, &profile)?;
            self.key_states.entry(profile.clone()).or_default();
            if operation == ProfileKeyOperation::Copy {
                let secret = clone_secret(self.private_key(&profile))?
                    .ok_or(RuntimeError::MissingClientKey)?;
                return Ok(Some(Effect::CopyClientKey(ClientKeyCopy {
                    profile: profile.clone(),
                    request,
                    key_epoch: self.key_states[&profile].epoch,
                    stop_epoch: expected_epoch,
                    secret,
                })));
            }
            if operation == ProfileKeyOperation::Create && self.private_key(&profile).is_some() {
                return Err(RuntimeError::InvalidAction);
            }
            let key = if operation == ProfileKeyOperation::Create {
                Some(
                    Secret::new(&format!(
                        "mc_{}{}",
                        crate::provider_server::random_id().map_err(map_server)?,
                        crate::provider_server::random_id().map_err(map_server)?
                    ))
                    .map_err(|_| RuntimeError::Corrupt)?,
                )
            } else {
                None
            };
            let gateway_key = clone_secret(key.as_ref())?;
            let next_epoch = self.key_states[&profile]
                .epoch
                .checked_add(1)
                .ok_or(RuntimeError::Corrupt)?;
            let update = self
                .gateway
                .publish_client_key(&profile, gateway_key, || {
                    if expected_epoch != self.stop_epoch.load(Ordering::Acquire) {
                        return Err(ServerError::Cancelled);
                    }
                    publish(&path, key.as_ref())
                })
                .map_err(|error| {
                    if error == ServerError::InvalidConfig {
                        RuntimeError::InvalidAction
                    } else {
                        map_server(error)
                    }
                })?;
            // Everything below the published boundary is nonfallible and pure.
            if profile == "default" {
                self.key = key;
            } else if let Some(key) = key {
                self.client_keys.insert(profile.clone(), key);
            } else {
                self.client_keys.remove(&profile);
            }
            let state = self
                .key_states
                .get_mut(&profile)
                .expect("prepared key state");
            if update.changed {
                state.epoch = next_epoch;
            }
            state.revoked = operation == ProfileKeyOperation::Revoke;
            match update.publication {
                ClientKeyPublication::Durable => state.durability_unconfirmed = false,
                ClientKeyPublication::DurabilityUnconfirmed => state.durability_unconfirmed = true,
                ClientKeyPublication::NoChange => {}
            }
            publication = Some(update.publication);
            Ok(None)
        })();
        let effect = match result {
            Ok(effect) => {
                self.key_receipts.insert(
                    profile.clone(),
                    ProfileKeyReceipt {
                        profile: profile.clone(),
                        request,
                        operation,
                        publication,
                        result: Ok(()),
                    },
                );
                effect
            }
            Err(error) => {
                self.reject_key(request, profile.clone(), operation, error);
                None
            }
        };
        self.refresh_connection_view();
        effect
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn k1_poll_refuses_old_key_epoch_and_stopped_copy_without_exposing_private_bytes() {
        for stopped in [false, true] {
            let (commands, _incoming) = mpsc::sync_channel(1);
            let pending = Arc::new(Mutex::new(Pending::default()));
            let view = View {
                profile_keys: vec![ProfileKeyMetadata {
                    profile: "default".into(),
                    state: ProfileKeyState::Created,
                    exists: true,
                    epoch: if stopped { 1 } else { 2 },
                    durability_unconfirmed: false,
                }],
                profile_key_receipts: vec![ProfileKeyReceipt {
                    profile: "default".into(),
                    request: 1,
                    operation: ProfileKeyOperation::Copy,
                    publication: None,
                    result: Ok(()),
                }],
                ..View::default()
            };

            {
                let mut shared = pending.lock().unwrap();
                shared.keys.flights.insert(
                    "default".into(),
                    KeyFlight {
                        request: 1,
                        epoch: 0,
                        operation: ProfileKeyOperation::Copy,
                        deliverable: true,
                    },
                );
                shared.view = Some(view);
                shared
                    .effects
                    .push_back(Effect::CopyClientKey(ClientKeyCopy {
                        profile: "default".into(),
                        request: 1,
                        key_epoch: 1,
                        stop_epoch: 0,
                        secret: Secret::new("synthetic-not-for-delivery").unwrap(),
                    }));
            }
            let mut runtime = Runtime {
                commands,
                connection_inflight: Default::default(),
                last_connection_request: 0,
                pending,
                view: View::default(),
                exit: Arc::new(AtomicBool::new(false)),
                stop_requested: Arc::new(AtomicU64::new(if stopped { 1 } else { 0 })),
                worker: None,
            };
            assert!(
                runtime
                    .poll()
                    .into_iter()
                    .all(|effect| !matches!(effect, Effect::CopyClientKey(_))),
                "stale copy escaped actual Runtime.poll"
            );
            assert_eq!(
                runtime.view().profile_key_receipts[0].result,
                Err(RuntimeError::Cancelled)
            );
            assert!(!runtime.keys_reserved());
        }
    }
}
