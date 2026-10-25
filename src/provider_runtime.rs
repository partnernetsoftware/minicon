//! Process-owned Harness runtime. GUI methods only exchange bounded messages;
//! filesystem, login and listener work belongs to the background owner.

pub use crate::provider_accounts::{
    AccountEdit, AccountPreferences, AccountSummary, NetworkPolicy,
};
pub use crate::provider_auth::Provider;
pub use crate::provider_login::AttemptId;
use crate::provider_selection::{
    CatalogueCache, CatalogueRequest, ConnectionAccounts, DraftBook, SelectionError,
};
pub use crate::provider_selection::{CatalogueModel, CatalogueStatus};
mod profile_batch;
mod profile_keys;
use crate::provider_server::CatalogueResult;
use crate::{
    provider_accounts::AccountStore,
    provider_auth::Secret,
    provider_login::{LoginChallenge, LoginCoordinator, LoginEvent, LoginMetadata},
};
use profile_batch::{Batch, BatchGate, PendingReadiness, ProgressSink, Quote};
pub use profile_keys::{
    ClientKeyCopy, ProfileKeyMetadata, ProfileKeyOperation, ProfileKeyReceipt, ProfileKeyState,
};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub use crate::provider_selection::{
    ConnectionAllocationInput, ConnectionAvailability, ConnectionDraftInput, ConnectionEvidence,
    ConnectionProfileView,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionChange {
    Create,
    Edit,
    Choose,
    Remove,
    Revert,
    Discard,
    Apply,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectionEditReceipt {
    pub request: u64,
    pub profile: String,
    pub change: ConnectionChange,
    pub revision: Option<u64>,
    pub result: Result<(), RuntimeError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListenerState {
    Stopped,
    Starting,
    Listening,
    Draining,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountSaveReceipt {
    pub account: String,
    pub request: u64,
    pub result: Result<(), RuntimeError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthOutcome {
    CredentialStored,
    Cancelled,
    Failed,
}

/// Initial private metadata load, independent of operation busy/listener state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadState {
    Loading,
    Loaded,
    Unavailable(RuntimeError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountCatalogueView {
    pub account: String,
    pub status: CatalogueStatus,
    pub models: Vec<CatalogueModel>,
    pub retained_rows: usize,
    pub error: Option<RuntimeError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionCheckPhase {
    Checking,
    Verified,
    Cancelled,
    Failed,
    Applying,
    Applied,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionCheckWarning {
    ReadinessStorage,
    ReadinessStale,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionCheckScope {
    Single,
    Profile,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectionPlanView {
    pub profile: String,
    pub revision: u64,
    pub id: u64,
    pub paid_slots: Vec<u8>,
    pub error: Option<RuntimeError>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectionCheckView {
    pub scope: ConnectionCheckScope,
    pub total: u8,
    pub completed: u8,
    pub applied_revision: Option<u64>,
    pub request: u64,
    pub profile: String,
    pub revision: u64,
    pub slot: u8,
    pub phase: ConnectionCheckPhase,
    pub error: Option<RuntimeError>,
    pub checked_unix_ms: Option<u64>,
    pub warning: Option<ConnectionCheckWarning>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct View {
    pub load_state: LoadState,
    pub port: u16,
    pub actual_port: Option<u16>,
    pub accounts: Vec<AccountSummary>,
    pub account_preferences: Vec<(String, AccountPreferences)>,
    pub account_save_receipts: Vec<AccountSaveReceipt>,
    pub selected_account: Option<String>,
    pub model: String,
    pub models: Vec<String>,
    pub catalogues: Vec<AccountCatalogueView>,
    pub catalogue_bytes: usize,
    pub connection_plans: Vec<ConnectionPlanView>,
    pub connection_profile: String,
    pub connection_profiles: Vec<ConnectionProfileView>,
    pub connection_receipts: Vec<ConnectionEditReceipt>,
    pub connection_check_active: Option<ConnectionCheckView>,
    pub connection_check_last_terminal: Option<ConnectionCheckView>,
    pub listener: ListenerState,
    pub busy: bool,
    pub login_active: bool,
    pub auth_attempt: Option<AttemptId>,
    pub auth_result: Option<(AttemptId, AuthOutcome)>,
    pub local_key_exists: bool,
    pub profile_keys: Vec<ProfileKeyMetadata>,
    pub profile_key_receipts: Vec<ProfileKeyReceipt>,
    pub config_durability_unconfirmed: bool,
    pub status: Option<String>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            load_state: LoadState::Loading,
            port: 28789,
            actual_port: None,
            accounts: Vec::new(),
            account_preferences: Vec::new(),
            account_save_receipts: Vec::new(),
            selected_account: None,
            model: String::new(),
            models: Vec::new(),
            catalogues: Vec::new(),
            catalogue_bytes: 0,
            connection_plans: Vec::new(),
            connection_profile: String::new(),
            connection_profiles: Vec::new(),
            connection_receipts: Vec::new(),
            connection_check_active: None,
            connection_check_last_terminal: None,
            listener: ListenerState::Stopped,
            busy: true,
            login_active: false,
            auth_attempt: None,
            auth_result: None,
            local_key_exists: false,
            profile_keys: Vec::new(),
            profile_key_receipts: Vec::new(),
            config_durability_unconfirmed: false,
            status: None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Action {
    VerifyAndApplyConnectionProfile {
        request: u64,
        profile: String,
        expected_revision: u64,
        plan_id: u64,
        paid_slots: Vec<u8>,
    },
    ApplyConnectionProfile {
        request: u64,
        profile: String,
        expected_revision: u64,
    },
    VerifyConnectionAllocation {
        request: u64,
        profile: String,
        expected_revision: u64,
        slot: u8,
    },
    CancelConnectionVerification {
        request: u64,
    },
    RetryConnectionReadiness {
        request: u64,
    },
    FocusConnectionProfile(String),
    CreateConnectionProfile {
        request: u64,
    },
    EditConnectionProfile {
        request: u64,
        profile: String,
        expected_revision: u64,
        input: Box<ConnectionDraftInput>,
    },
    ChooseConnectionModel {
        request: u64,
        profile: String,
        expected_revision: u64,
        slot: u8,
        account: String,
        catalogue_id: String,
    },
    RemoveConnectionAllocation {
        request: u64,
        profile: String,
        expected_revision: u64,
        slot: u8,
    },
    RevertConnectionProfile {
        request: u64,
        profile: String,
        expected_revision: u64,
    },
    DiscardUnappliedProfile {
        request: u64,
        profile: String,
        expected_revision: u64,
    },

    Connect {
        provider: Provider,
        label: String,
        login: NetworkPolicy,
        service: NetworkPolicy,
    },
    Reconnect(String),
    CancelLogin,
    RefreshAccounts,
    RefreshAccountModels(String),
    #[cfg(test)]
    SelectAccount(String),
    SaveAccount {
        request: u64,
        edit: Box<AccountEdit>,
    },
    #[cfg(test)]
    SelectModel(String),
    SetPort(u16),
    Start,
    Stop,
    RemoveAccount(String),
    ProfileKey {
        request: u64,
        profile: String,
        operation: ProfileKeyOperation,
    },
    CreateClientKey,
    CopyClientKey,
    RevokeClientKey,
}

#[derive(Debug)]
pub enum Effect {
    OpenAuth {
        attempt: AttemptId,
        context: minicon_web::PageContext,
        url: Secret,
    },
    AuthFinished(AttemptId),
    CopyClientKey(ClientKeyCopy),
    DeviceChallenge {
        attempt: AttemptId,
        context: minicon_web::PageContext,
        verification_url: Secret,
        user_code: Secret,
        deadline: Instant,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeError {
    InvalidRoot,
    InvalidAction,
    Busy,
    Storage,
    Corrupt,
    MissingAccount,
    MissingModel,
    VerificationRequired,
    DraftChanged,
    PlanChanged,
    DeadlineExceeded,
    Cancelled,
    AccountChanged,
    MissingClientKey,
    NeedsLogin,
    WorkerUnavailable,
    ProviderUnavailable,
}
impl std::fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Harness: {self:?}")
    }
}
impl std::error::Error for RuntimeError {}

#[derive(Default)]
struct Pending {
    batch_gate: BatchGate,
    keys: profile_keys::KeyPending,
    view: Option<View>,
    effects: VecDeque<Effect>,
    last_view: Option<View>,
}

pub struct Runtime {
    commands: SyncSender<(u64, Vec<u64>, Action)>,
    connection_inflight: std::collections::BTreeMap<u64, String>,
    last_connection_request: u64,
    pending: Arc<Mutex<Pending>>,
    view: View,
    exit: Arc<AtomicBool>,
    stop_requested: Arc<AtomicU64>,
    worker: Option<JoinHandle<()>>,
}

impl Runtime {
    /// Only checks the injected path and starts the owner thread. No disk or
    /// network access occurs on the constructing GUI thread.
    pub fn new(
        root: PathBuf,
        metadata: LoginMetadata,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, RuntimeError> {
        if !root.is_absolute() {
            return Err(RuntimeError::InvalidRoot);
        }
        let (commands, incoming) = mpsc::sync_channel(16);
        let pending = Arc::new(Mutex::new(Pending::default()));
        let exit = Arc::new(AtomicBool::new(false));
        let stop_requested = Arc::new(AtomicU64::new(0));
        let stop_signal = stop_requested.clone();
        let outgoing = pending.clone();
        let stopping = exit.clone();
        let worker = std::thread::Builder::new()
            .name("minicon-provider-runtime".into())
            .spawn(move || {
                run_owner(
                    root,
                    metadata,
                    notify,
                    outgoing,
                    stopping,
                    stop_signal,
                    incoming,
                )
            })
            .map_err(|_| RuntimeError::WorkerUnavailable)?;
        Ok(Self {
            commands,
            connection_inflight: Default::default(),
            last_connection_request: 0,
            pending,
            view: View::default(),
            exit,
            stop_requested,
            worker: Some(worker),
        })
    }

    pub fn act(&mut self, action: Action) -> Result<(), RuntimeError> {
        if matches!(
            &action,
            Action::ProfileKey { .. }
                | Action::CreateClientKey
                | Action::CopyClientKey
                | Action::RevokeClientKey
        ) {
            return self.key_action(action);
        }
        let profile_id = |id: &str| id == "default" || crate::provider_server::opaque_id(id);
        let invalid = match &action {
            Action::VerifyAndApplyConnectionProfile {
                request,
                profile,
                plan_id,
                paid_slots,
                ..
            } => {
                *request == 0
                    || !profile_id(profile)
                    || *plan_id == 0
                    || paid_slots.is_empty()
                    || paid_slots.len() > 32
                    || paid_slots.iter().any(|slot| *slot >= 32)
                    || paid_slots
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        != paid_slots.len()
            }
            Action::ApplyConnectionProfile {
                request, profile, ..
            } => *request == 0 || !profile_id(profile),
            Action::VerifyConnectionAllocation {
                request,
                profile,
                slot,
                ..
            } => *request == 0 || !profile_id(profile) || *slot >= 32,
            Action::CancelConnectionVerification { request }
            | Action::RetryConnectionReadiness { request } => *request == 0,
            Action::FocusConnectionProfile(profile) => !profile_id(profile),
            Action::CreateConnectionProfile { request } => *request == 0,
            Action::EditConnectionProfile {
                request,
                profile,
                input,
                ..
            } => *request == 0 || !profile_id(profile) || input.bounded().is_err(),
            Action::ChooseConnectionModel {
                request,
                profile,
                slot,
                account,
                catalogue_id,
                ..
            } => {
                *request == 0
                    || !profile_id(profile)
                    || usize::from(*slot) >= 32
                    || !crate::provider_server::opaque_id(account)
                    || !crate::provider_server::safe_model(catalogue_id, false)
            }
            Action::RemoveConnectionAllocation {
                request,
                profile,
                slot,
                ..
            } => *request == 0 || !profile_id(profile) || usize::from(*slot) >= 32,
            Action::RevertConnectionProfile {
                request, profile, ..
            }
            | Action::DiscardUnappliedProfile {
                request, profile, ..
            } => *request == 0 || !profile_id(profile),
            _ => false,
        };
        if invalid {
            return Err(RuntimeError::InvalidAction);
        }
        let connection = match &action {
            Action::ApplyConnectionProfile {
                request, profile, ..
            } => Some((*request, profile.clone())),
            Action::CreateConnectionProfile { request } => Some((*request, String::new())),
            Action::EditConnectionProfile {
                request, profile, ..
            }
            | Action::ChooseConnectionModel {
                request, profile, ..
            }
            | Action::RemoveConnectionAllocation {
                request, profile, ..
            }
            | Action::RevertConnectionProfile {
                request, profile, ..
            }
            | Action::DiscardUnappliedProfile {
                request, profile, ..
            } => Some((*request, profile.clone())),
            _ => None,
        };
        let ordered_request = match &action {
            Action::VerifyConnectionAllocation { request, .. }
            | Action::VerifyAndApplyConnectionProfile { request, .. } => Some(*request),
            _ => connection.as_ref().map(|(request, _)| *request),
        };
        if ordered_request.is_some_and(|request| request <= self.last_connection_request) {
            return Err(RuntimeError::InvalidAction);
        }
        let gate = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .batch_gate
            .clone();
        match &action {
            Action::EditConnectionProfile { profile, .. }
            | Action::ChooseConnectionModel { profile, .. }
            | Action::RemoveConnectionAllocation { profile, .. }
            | Action::RevertConnectionProfile { profile, .. }
            | Action::DiscardUnappliedProfile { profile, .. } => {
                gate.invalidate_profile(profile);
            }
            _ => {}
        }
        if let Some((request, profile)) = &connection {
            if *request <= self.last_connection_request {
                return Err(RuntimeError::InvalidAction);
            }
            if self.connection_inflight.len() >= 9
                || self
                    .connection_inflight
                    .values()
                    .any(|pending| pending == profile)
            {
                return Err(RuntimeError::Busy);
            }
        }
        if matches!(action, Action::Stop) {
            gate.stop(&self.stop_requested);
            return Ok(());
        }
        if let Action::SetPort(port) = &action
            && (*port < 1024
                || !matches!(
                    self.view.listener,
                    ListenerState::Stopped | ListenerState::Error
                ))
        {
            return Err(RuntimeError::InvalidAction);
        }
        if let Action::CancelConnectionVerification { request } = &action
            && gate.reserved()
        {
            return gate.cancel_exact(*request);
        }
        // Only receipts delivered by poll are acknowledged. Merely accepting
        // a command does not grant the owner permission to drop its response.
        let delivered = self
            .view
            .connection_receipts
            .iter()
            .map(|receipt| receipt.request)
            .collect();
        let registration = if let Action::VerifyAndApplyConnectionProfile {
            request,
            profile,
            expected_revision,
            plan_id,
            ..
        } = &action
        {
            let id = profile_batch::BatchIdentity {
                request: *request,
                profile: profile.clone(),
                revision: *expected_revision,
                plan: *plan_id,
                epoch: self.stop_requested.load(Ordering::Acquire),
            };
            if self.keys_reserved() {
                return Err(RuntimeError::Busy);
            }
            gate.register(id.clone())?;
            Some(id)
        } else {
            None
        };
        let sent = self
            .commands
            .try_send((
                self.stop_requested.load(Ordering::Acquire),
                delivered,
                action,
            ))
            .map_err(|error| match error {
                TrySendError::Full(_) => RuntimeError::Busy,
                TrySendError::Disconnected(_) => RuntimeError::WorkerUnavailable,
            });
        if let Err(error) = sent {
            if let Some(id) = &registration {
                gate.withdraw(id);
            }
            return Err(error);
        }
        if let Some((request, profile)) = connection {
            self.connection_inflight.insert(request, profile);
        }
        if let Some(request) = ordered_request {
            self.last_connection_request = request;
        }
        Ok(())
    }

    /// Applies already-produced metadata/effects. No store or transport calls.
    pub fn poll(&mut self) -> Vec<Effect> {
        let pending_arc = self.pending.clone();
        let (effects, gate) = {
            let mut pending = pending_arc
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(view) = pending.view.take() {
                for receipt in &view.connection_receipts {
                    self.connection_inflight.remove(&receipt.request);
                }
                self.view = view;
            }
            let effects = pending.effects.drain(..).collect();
            let effects = self.consume_key_effects(&mut pending, effects);
            (effects, pending.batch_gate.clone())
        };
        if let Some(view) = &self.view.connection_check_last_terminal
            && view.scope == ConnectionCheckScope::Profile
        {
            gate.acknowledge(view.request);
        }
        effects
    }

    pub fn invalidate_connection_profile(&self, profile: &str) -> bool {
        let gate = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .batch_gate
            .clone();
        gate.invalidate_profile(profile)
    }

    pub fn view(&self) -> &View {
        &self.view
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        let gate = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .batch_gate
            .clone();
        gate.invalidate_all();
        self.exit.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn publish(
    pending: &Mutex<Pending>,
    notify: &Arc<dyn Fn() + Send + Sync>,
    mut view: View,
    effect: Option<Effect>,
) {
    let mut pending = pending.lock().unwrap_or_else(|error| error.into_inner());
    if effect.is_none() && pending.last_view.as_ref() == Some(&view) {
        return;
    }
    if let Some(effect) = effect {
        if pending.effects.len() < 8 {
            pending.effects.push_back(effect);
        } else {
            view.status = Some(RuntimeError::Busy.to_string());
        }
    }
    pending.last_view = Some(view.clone());
    pending.view = Some(view);
    drop(pending);
    notify();
}

fn login_event(event: LoginEvent, view: &mut View, store: &AccountStore) -> Option<Effect> {
    let attempt = event.attempt();
    match event {
        LoginEvent::Challenge {
            challenge: LoginChallenge::BrowserUrl { url },
            context: Some(context),
            ..
        } => {
            return Some(Effect::OpenAuth {
                attempt,
                context,
                url,
            });
        }
        LoginEvent::Challenge {
            challenge:
                LoginChallenge::Device {
                    verification_url,
                    user_code,
                    deadline,
                },
            context: Some(context),
            ..
        } => {
            return Some(Effect::DeviceChallenge {
                attempt,
                context,
                verification_url,
                user_code,
                deadline,
            });
        }
        LoginEvent::Challenge { context: None, .. } => {
            view.status = Some(RuntimeError::ProviderUnavailable.to_string());
            view.auth_result = Some((attempt, AuthOutcome::Failed));
        }
        LoginEvent::CredentialStored {
            account,
            completion_url,
            ..
        } => {
            // Completion is acknowledged by the host. A provider landing page
            // cannot overwrite the already committed account result.
            drop(completion_url);
            view.accounts.retain(|entry| entry.id != account.id);
            view.accounts.push(account);
            view.status =
                Some("Credential stored; Test model is required before readiness.".into());
            view.login_active = false;
            view.auth_attempt = None;
            view.auth_result = Some((attempt, AuthOutcome::CredentialStored));
            view.busy = false;
            return Some(Effect::AuthFinished(attempt));
        }
        #[cfg(test)]
        LoginEvent::Accounts { accounts, .. } => view.accounts = accounts,
        LoginEvent::LoggedOut { remote_revoked, .. } => {
            view.status = Some(
                if remote_revoked {
                    "Local account removed and remote grant revoked."
                } else {
                    "Local account removed; remote revocation is unconfirmed."
                }
                .into(),
            )
        }
        LoginEvent::Cancelled { .. } => {
            view.status = Some("Login cancelled.".into());
            view.auth_result = Some((attempt, AuthOutcome::Cancelled));
        }
        LoginEvent::Failed { error, .. } => {
            view.status = Some(error.to_string());
            view.auth_result = Some((attempt, AuthOutcome::Failed));
        }
    }
    view.login_active = false;
    view.auth_attempt = None;
    view.busy = false;
    if let Ok(accounts) = store.list() {
        view.accounts = accounts;
    }
    Some(Effect::AuthFinished(attempt))
}

#[cfg(test)]
use crate::provider_server::ModelRoute;
use crate::provider_server::{Config, ConfigPublication, Gateway, ServerError};
use minicon_platform::{
    filesystem, network_http_stream::NetworkHttpCancel, network_loopback::Server,
};
use std::path::Path;

enum OperationResult {
    Catalog(CatalogueResult),
    Probe(crate::provider_server::ProbeObservation),
}
enum OperationKind {
    Catalogue(CatalogueRequest),
    Probe {
        ticket: crate::provider_selection::ProbeTicket,
        permit: crate::provider_selection::ProbePermit,
    },
}

pub const CONFIG_DURABILITY_WARNING: &str = "Settings saved and applied; disk durability could not be confirmed. Save again when storage is available.";
struct Operation {
    cancel: Arc<NetworkHttpCancel>,
    worker: JoinHandle<Result<OperationResult, RuntimeError>>,
    account: String,
    kind: OperationKind,
}
struct Owner {
    root: PathBuf,
    store: Arc<AccountStore>,
    config: Config,
    key: Option<Secret>,
    client_keys: std::collections::BTreeMap<String, Secret>,
    key_states: std::collections::BTreeMap<String, profile_keys::KeyState>,
    key_receipts: std::collections::BTreeMap<String, ProfileKeyReceipt>,
    last_key_request: u64,
    gateway: Arc<Gateway>,
    server: Option<Server>,
    operation: Option<Operation>,
    readiness_pending: Option<PendingReadiness>,
    batch_gate: BatchGate,
    stop_epoch: Arc<AtomicU64>,
    batch: Option<Batch>,
    quotes: std::collections::BTreeMap<String, Quote>,
    quote_sequence: u64,
    progress: Option<ProgressSink>,
    catalogues: CatalogueCache,
    drafts: DraftBook,
    connection_accounts: ConnectionAccounts,
    catalogue_errors: std::collections::BTreeMap<String, RuntimeError>,
    removing: Option<(crate::provider_login::AttemptId, String)>,
    view: View,
}
impl Owner {
    fn load(root: PathBuf) -> Result<Self, RuntimeError> {
        prepare_root(&root)?;
        let config = match filesystem::read_private_bounded(
            &root.join("gateway.json"),
            crate::provider_server::CONFIG_BYTES,
        ) {
            Ok(bytes) => Config::decode_applied(&bytes).map_err(|_| RuntimeError::Corrupt)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Config::fresh()
                .and_then(Config::upgrade_applied)
                .map_err(map_server)?,
            Err(_) => return Err(RuntimeError::Storage),
        };
        config.validate().map_err(map_server)?;
        let key = read_key(&root)?;
        let store = Arc::new(
            AccountStore::new(root.join("accounts")).map_err(|_| RuntimeError::InvalidRoot)?,
        );
        let gateway = Gateway::new(store.clone(), config.clone(), clone_secret(key.as_ref())?)
            .map_err(map_server)?;
        let mut client_keys = std::collections::BTreeMap::new();
        for profile in &config.profiles {
            if let Some(key) = read_key_path(&root.join(format!("client-key-{}", profile.id)))? {
                let live = clone_secret(Some(&key))?;
                gateway
                    .set_client_key(&profile.id, live)
                    .map_err(map_server)?;
                client_keys.insert(profile.id.clone(), key);
            }
        }
        let drafts = DraftBook::from_applied(&config).map_err(map_selection)?;
        let mut owner = Self {
            root,
            store,
            config,
            key,
            client_keys,
            key_states: Default::default(),
            key_receipts: Default::default(),
            last_key_request: 0,
            gateway,
            server: None,
            operation: None,
            readiness_pending: None,
            batch_gate: BatchGate::default(),
            stop_epoch: Arc::new(AtomicU64::new(0)),
            batch: None,
            quotes: Default::default(),
            quote_sequence: 0,
            progress: None,
            catalogues: CatalogueCache::default(),
            drafts,
            connection_accounts: ConnectionAccounts::new(),
            catalogue_errors: Default::default(),
            removing: None,
            view: View::default(),
        };
        if let Err(error) = owner.refresh_view() {
            // Valid applied references remain observable even when account
            // metadata is unavailable. Config/key decode failures above still
            // fail construction; this does not manufacture usable accounts.
            owner.view.load_state = LoadState::Unavailable(error);
            owner.view.status = Some(error.to_string());
        }
        Ok(owner)
    }
    fn connection_snapshot(
        &self,
        account: &str,
    ) -> Result<crate::provider_accounts::AccountSnapshot, ConnectionAvailability> {
        use crate::provider_accounts::StoreError;
        match self.store.snapshot(account) {
            Ok(snapshot) if !snapshot.preferences.enabled => {
                Err(ConnectionAvailability::DisabledAccount)
            }
            Ok(snapshot) => Ok(snapshot),
            Err(StoreError::Missing) => Err(ConnectionAvailability::MissingAccount),
            Err(_) => Err(ConnectionAvailability::CredentialUnavailable),
        }
    }

    fn observe_connection_accounts(&mut self, metadata_available: bool) {
        let accounts = self
            .drafts
            .referenced_accounts()
            .into_iter()
            .chain(self.view.accounts.iter().map(|account| account.id.clone()))
            .collect::<std::collections::BTreeSet<_>>();
        self.connection_accounts = accounts
            .into_iter()
            .map(|account| {
                let snapshot = if metadata_available {
                    self.connection_snapshot(&account)
                } else {
                    Err(ConnectionAvailability::CredentialUnavailable)
                };
                (account, snapshot)
            })
            .collect();
    }

    fn project_connection_drafts(&mut self) {
        self.view.connection_profile = self.drafts.selected().into();
        self.view.connection_profiles = self.drafts.views(&self.connection_accounts);
        self.refresh_profile_plans();
    }

    fn connection_receipt(
        &mut self,
        request: u64,
        profile: String,
        change: ConnectionChange,
        result: Result<(), RuntimeError>,
    ) {
        let revision = self.drafts.revision(&profile);
        self.view
            .connection_receipts
            .retain(|receipt| receipt.profile != profile);
        // Runtime admits at most nine unresolved commands and acknowledges
        // delivered receipts in the next command envelope. Never evict one.
        self.view.connection_receipts.push(ConnectionEditReceipt {
            request,
            profile,
            change,
            revision,
            result,
        });
        self.project_connection_drafts();
    }

    /// Consumes connection commands. Only explicit Apply saves Config.
    /// None of these branches starts network I/O. Explicit Choose reads a
    /// bounded private account snapshot on Owner.
    fn connection_action(&mut self, action: Action) -> Option<Action> {
        let (request, profile, change, result) = match action {
            Action::ApplyConnectionProfile {
                request,
                profile,
                expected_revision,
            } => {
                let result = self.apply_connection_profile(&profile, expected_revision);
                (request, profile, ConnectionChange::Apply, result)
            }
            Action::FocusConnectionProfile(profile) => {
                if let Err(error) = self.drafts.select(&profile) {
                    self.view.status = Some(map_selection(error).to_string());
                }
                self.project_connection_drafts();
                return None;
            }
            Action::CreateConnectionProfile { request } => {
                // Entropy creates only a process draft ID, never a key/profile file.
                let created = crate::provider_server::random_id()
                    .map_err(map_server)
                    .and_then(|id| {
                        self.drafts.create_profile(&id).map_err(map_selection)?;
                        Ok(id)
                    });
                match created {
                    Ok(id) => (request, id, ConnectionChange::Create, Ok(())),
                    Err(error) => (request, String::new(), ConnectionChange::Create, Err(error)),
                }
            }
            Action::EditConnectionProfile {
                request,
                profile,
                expected_revision,
                input,
            } => {
                let result = self
                    .drafts
                    .edit_input(&profile, expected_revision, *input)
                    .map(|_| ())
                    .map_err(map_selection);
                (request, profile, ConnectionChange::Edit, result)
            }
            Action::ChooseConnectionModel {
                request,
                profile,
                expected_revision,
                slot,
                account,
                catalogue_id,
            } => {
                let result = (|| {
                    // This is bounded private snapshot reading on Owner, never
                    // catalogue HTTP or a paid inference. Cache checks Fresh
                    // plus exact current authority before freezing the row.
                    if self.drafts.revision(&profile) != Some(expected_revision) {
                        return Err(RuntimeError::DraftChanged);
                    }
                    let snapshot =
                        self.connection_snapshot(&account)
                            .map_err(|reason| match reason {
                                ConnectionAvailability::MissingAccount => {
                                    RuntimeError::MissingAccount
                                }
                                ConnectionAvailability::DisabledAccount => RuntimeError::NeedsLogin,
                                _ => RuntimeError::Storage,
                            })?;
                    let selected = self
                        .catalogues
                        .select(&snapshot, &catalogue_id)
                        .map_err(map_selection)?;
                    self.drafts
                        .choose(&profile, expected_revision, slot, selected, &catalogue_id)
                        .map_err(map_selection)?;
                    self.connection_accounts.insert(account, Ok(snapshot));
                    Ok(())
                })();
                (request, profile, ConnectionChange::Choose, result)
            }
            Action::RemoveConnectionAllocation {
                request,
                profile,
                expected_revision,
                slot,
            } => {
                let result = self
                    .drafts
                    .remove_allocation(&profile, expected_revision, slot)
                    .map(|_| ())
                    .map_err(map_selection);
                (request, profile, ConnectionChange::Remove, result)
            }
            Action::RevertConnectionProfile {
                request,
                profile,
                expected_revision,
            } => {
                let result = self
                    .drafts
                    .revert(&profile, expected_revision, &self.config)
                    .map(|_| ())
                    .map_err(map_selection);
                (request, profile, ConnectionChange::Revert, result)
            }
            Action::DiscardUnappliedProfile {
                request,
                profile,
                expected_revision,
            } => {
                let result = self
                    .drafts
                    .discard(&profile, expected_revision)
                    .map_err(map_selection);
                (request, profile, ConnectionChange::Discard, result)
            }
            other => return Some(other),
        };
        if result.is_ok()
            && self
                .view
                .connection_check_active
                .as_ref()
                .is_some_and(|active| {
                    active.profile == profile
                        && self.drafts.revision(&profile) != Some(active.revision)
                })
        {
            self.cancel_operation();
        }
        self.connection_receipt(request, profile, change, result);
        None
    }

    fn refresh_view(&mut self) -> Result<(), RuntimeError> {
        let metadata = (|| {
            let accounts = self.store.list().map_err(|_| RuntimeError::Storage)?;
            let preferences = accounts
                .iter()
                .map(|account| {
                    self.store
                        .preferences(&account.id)
                        .map(|preferences| (account.id.clone(), preferences))
                        .map_err(|_| RuntimeError::Storage)
                })
                .collect::<Result<_, _>>()?;
            Ok::<_, RuntimeError>((accounts, preferences))
        })();
        let (accounts, preferences) = match metadata {
            Ok(metadata) => metadata,
            Err(error) => {
                // Retain bounded readings, but revoke Fresh/Loading claims
                // even when registry/preferences cannot be reread. Operation
                // completion is independent of that metadata read succeeding.
                for account in self.catalogues.accounts() {
                    self.catalogues.observe(&account, None);
                    self.catalogue_errors.insert(account, error);
                }
                self.observe_connection_accounts(false);
                self.view.load_state = LoadState::Unavailable(error);
                self.refresh_connection_view();
                return Err(error);
            }
        };
        self.view
            .account_save_receipts
            .retain(|receipt| accounts.iter().any(|account| account.id == receipt.account));
        for cached in self.catalogues.accounts() {
            if !accounts.iter().any(|account| account.id == cached) {
                self.catalogues.evict(&cached);
                self.catalogue_errors.remove(&cached);
            }
        }
        self.catalogue_errors
            .retain(|id, _| accounts.iter().any(|account| &account.id == id));
        for account in &accounts {
            let snapshot = self.store.snapshot(&account.id);
            self.catalogues.observe(&account.id, snapshot.as_ref().ok());
            if let Err(error) = snapshot {
                self.catalogue_errors.insert(
                    account.id.clone(),
                    map_login(crate::provider_login::LoginError::Store(error)),
                );
            }
        }
        self.view.accounts = accounts;
        self.observe_connection_accounts(true);
        self.view.account_preferences = preferences;
        self.view.load_state = LoadState::Loaded;
        self.refresh_connection_view();
        Ok(())
    }

    fn refresh_connection_view(&mut self) {
        let provider = self
            .view
            .accounts
            .iter()
            .find(|account| Some(&account.id) == self.config.selected_account.as_ref())
            .map(|account| account.provider);
        self.view.port = self.config.port;
        self.view.selected_account = self.config.selected_account.clone();
        self.view.model = self.config.model.clone();
        self.view.models = self
            .config
            .models
            .iter()
            .filter(|route| Some(route.provider) == provider)
            .map(|route| route.id.clone())
            .collect();
        self.view.catalogues = self
            .view
            .accounts
            .iter()
            .map(|account| AccountCatalogueView {
                account: account.id.clone(),
                status: self.catalogues.status(&account.id),
                models: self.catalogues.models(&account.id),
                retained_rows: self.catalogues.stale_rows(&account.id),
                error: self.catalogue_errors.get(&account.id).copied(),
            })
            .collect();
        self.view.catalogue_bytes = self.catalogues.retained_bytes();
        self.project_connection_drafts();
        self.project_profile_keys();
        // The legacy default observation and targeted metadata share one
        // projection instead of maintaining two potentially divergent facts.
        self.view.local_key_exists = self
            .view
            .profile_keys
            .iter()
            .any(|row| row.profile == "default" && row.state == ProfileKeyState::Created);
        self.view.busy = self.operation.is_some()
            || self.view.login_active
            || self.batch.is_some()
            || self
                .view
                .connection_check_active
                .as_ref()
                .is_some_and(|view| view.scope == ConnectionCheckScope::Profile);
    }
    fn save(&mut self, config: Config) -> Result<(), RuntimeError> {
        self.save_with(config, |path, bytes| {
            match minicon_platform::filesystem_publish::write_private_atomic(path, bytes) {
                Ok(()) => Ok(ConfigPublication::Durable),
                Err(error) if error.published() => Ok(ConfigPublication::DurabilityUnconfirmed),
                Err(_) => Err(ServerError::Storage),
            }
        })
    }

    fn save_with(
        &mut self,
        config: Config,
        publish: impl FnOnce(&Path, &[u8]) -> Result<ConfigPublication, ServerError>,
    ) -> Result<(), RuntimeError> {
        let bytes = config.encode_applied().map_err(map_server)?;
        // Account reads may fail. Finish them before publication so a later
        // metadata read cannot turn an already-applied save into a false error.
        self.refresh_view()?;
        let publication = self
            .gateway
            .update_and_publish(config.clone(), || {
                publish(&self.root.join("gateway.json"), &bytes)
            })
            .map_err(map_server)?;
        self.config = config;
        self.view.config_durability_unconfirmed =
            publication == ConfigPublication::DurabilityUnconfirmed;
        self.refresh_connection_view();
        Ok(())
    }

    fn apply_connection_profile(
        &mut self,
        profile: &str,
        revision: u64,
    ) -> Result<(), RuntimeError> {
        self.apply_connection_profile_with(profile, revision, |path, bytes| {
            match minicon_platform::filesystem_publish::write_private_atomic(path, bytes) {
                Ok(()) => Ok(ConfigPublication::Durable),
                Err(error) if error.published() => Ok(ConfigPublication::DurabilityUnconfirmed),
                Err(_) => Err(ServerError::Storage),
            }
        })
    }

    fn apply_connection_profile_with(
        &mut self,
        profile: &str,
        revision: u64,
        publish: impl FnOnce(&Path, &[u8]) -> Result<ConfigPublication, ServerError>,
    ) -> Result<(), RuntimeError> {
        if self.operation.is_some() || self.view.login_active || self.batch.is_some() {
            return Err(RuntimeError::Busy);
        }
        // Reads/projection can fail; complete them before touching routing or
        // publishing bytes. Readiness is not proof and is never set by Apply.
        self.refresh_view()?;
        let prepared = self
            .drafts
            .prepare_apply(profile, revision, &self.config, &self.connection_accounts)
            .map_err(map_selection)?;
        let bytes = prepared.config.encode_applied().map_err(map_server)?;
        let publication = self
            .gateway
            .update_with_authorities(prepared.config.clone(), &prepared.authorities, || {
                publish(&self.root.join("gateway.json"), &bytes)
            })
            .map_err(map_server)?;
        self.config = prepared.config;
        self.drafts = prepared.book;
        self.view.config_durability_unconfirmed =
            publication == ConfigPublication::DurabilityUnconfirmed;
        self.refresh_connection_view();
        Ok(())
    }

    fn cancel_operation(&mut self) {
        if self.batch.is_some() {
            self.abort_batch(RuntimeError::Cancelled);
            return;
        }
        if let Some(operation) = self.operation.take() {
            if let OperationKind::Probe { permit, .. } = &operation.kind {
                permit.invalidate();
            }
            operation.cancel.cancel();
            let _ = operation.worker.join();
            match operation.kind {
                OperationKind::Catalogue(ticket) => {
                    let _ = self.catalogues.fail(&ticket);
                    self.catalogue_errors
                        .insert(operation.account, RuntimeError::Cancelled);
                }
                OperationKind::Probe { .. } => {
                    if let Some(view) = self.view.connection_check_active.take() {
                        self.reject_check(view, RuntimeError::Cancelled);
                    }
                }
            }
        }
        self.view.busy = self.view.login_active;
        self.refresh_connection_view();
    }
    fn stop(&mut self) {
        self.view.listener = ListenerState::Draining;
        self.gateway.begin_stop();
        if let Some(operation) = &self.operation {
            operation.cancel.cancel();
        }
        self.cancel_operation();
        if let Some(mut server) = self.server.take() {
            server.stop();
        }
        self.view.listener = ListenerState::Stopped;
        self.view.actual_port = None;
    }
    fn poll_operation(&mut self) {
        if !self
            .operation
            .as_ref()
            .is_some_and(|operation| operation.worker.is_finished())
        {
            return;
        }
        let operation = self.operation.take().expect("finished operation");
        let result = operation
            .worker
            .join()
            .unwrap_or(Err(RuntimeError::WorkerUnavailable));
        match operation.kind {
            OperationKind::Catalogue(ticket) => {
                let result = if operation.cancel.is_cancelled() {
                    Err(RuntimeError::Cancelled)
                } else {
                    match result {
                        Ok(OperationResult::Catalog(result)) => {
                            let store = self.store.clone();
                            store
                                .with_snapshot(&operation.account, |current| {
                                    self.catalogues.complete(
                                        &ticket,
                                        result.authority,
                                        Some(&current),
                                        result.routes,
                                        result.response_bytes,
                                    )
                                })
                                .map_err(|error| {
                                    map_login(crate::provider_login::LoginError::Store(error))
                                })
                                .and_then(|result| result.map_err(map_selection))
                        }
                        Ok(OperationResult::Probe(_)) => Err(RuntimeError::InvalidAction),
                        Err(error) => Err(error),
                    }
                };
                match result {
                    Ok(()) => {
                        self.catalogue_errors.remove(&operation.account);
                    }
                    Err(error) => {
                        let _ = self.catalogues.fail(&ticket);
                        self.catalogue_errors.insert(operation.account, error);
                    }
                }
            }
            OperationKind::Probe { ticket, permit } if self.batch.is_some() => self
                .finish_batch_check(
                    ticket,
                    permit,
                    &operation.account,
                    result,
                    operation.cancel.is_cancelled(),
                ),
            OperationKind::Probe { ticket, permit } => self.finish_check(
                ticket,
                permit,
                &operation.account,
                result,
                operation.cancel.is_cancelled(),
            ),
        }
        if let Err(error) = self.refresh_view() {
            self.view.status = Some(error.to_string());
        }
    }

    fn spawn_probe_operation(
        &self,
        ticket: crate::provider_selection::ProbeTicket,
        candidate: crate::provider_selection::FrozenCandidate,
        cancel: Arc<NetworkHttpCancel>,
        deadline: Instant,
        batch: Option<&profile_batch::BatchIdentity>,
    ) -> Result<Operation, RuntimeError> {
        let permit = crate::provider_selection::ProbePermit::new();
        if let Some(batch) = batch {
            self.batch_gate.child(
                batch,
                self.stop_epoch.load(Ordering::Acquire),
                permit.clone(),
            )?;
        }
        let worker_permit = permit.clone();
        let network = cancel.clone();
        let gateway = self.gateway.clone();
        let identity = self.config.identity.clone();
        let account = candidate.account().to_owned();
        let worker = std::thread::Builder::new()
            .name("minicon-candidate-check".into())
            .spawn(move || {
                gateway
                    .probe_candidate(&candidate, &worker_permit, &identity, &network, deadline)
                    .map(OperationResult::Probe)
                    .map_err(map_server)
            })
            .map_err(|_| RuntimeError::WorkerUnavailable)?;
        Ok(Operation {
            cancel,
            worker,
            account,
            kind: OperationKind::Probe { ticket, permit },
        })
    }

    fn accept_check_result(
        &mut self,
        ticket: crate::provider_selection::ProbeTicket,
        permit: crate::provider_selection::ProbePermit,
        account: &str,
        result: Result<OperationResult, RuntimeError>,
        cancelled: bool,
        parent: Option<(
            &profile_batch::BatchGate,
            &profile_batch::BatchIdentity,
            u64,
        )>,
    ) -> Result<crate::provider_accounts::AccountSnapshot, RuntimeError> {
        if let Some((gate, id, epoch)) = parent {
            gate.with_active(id, epoch, || Ok(()))?;
        }
        if cancelled {
            return Err(RuntimeError::Cancelled);
        }
        match result {
            Ok(OperationResult::Probe(observation)) => {
                let store = self.store.clone();
                store
                    .with_snapshot(account, |current| {
                        let accept = || {
                            self.drafts
                                .accept_probe_observation(ticket, observation, &current, &permit)
                                .map(|()| current)
                                .map_err(map_selection)
                        };
                        if let Some((gate, id, epoch)) = parent {
                            gate.with_active(id, epoch, accept)
                        } else {
                            accept()
                        }
                    })
                    .map_err(|error| map_login(crate::provider_login::LoginError::Store(error)))?
            }
            Ok(OperationResult::Catalog(_)) => Err(RuntimeError::InvalidAction),
            Err(error) => Err(error),
        }
    }

    fn set_check_terminal(&mut self, view: ConnectionCheckView) {
        if self
            .view
            .connection_check_last_terminal
            .as_ref()
            .is_some_and(|old| {
                old.request != view.request
                    && old.scope == ConnectionCheckScope::Profile
                    && self.batch_gate.protects_terminal(old.request)
            })
        {
            return;
        }
        self.view.connection_check_last_terminal = Some(view);
    }

    fn check_view(request: u64, profile: String, revision: u64, slot: u8) -> ConnectionCheckView {
        ConnectionCheckView {
            scope: ConnectionCheckScope::Single,
            total: 1,
            completed: 0,
            applied_revision: None,
            request,
            profile,
            revision,
            slot,
            phase: ConnectionCheckPhase::Checking,
            error: None,
            checked_unix_ms: None,
            warning: None,
        }
    }

    fn reject_check(&mut self, mut view: ConnectionCheckView, error: RuntimeError) {
        view.phase = if error == RuntimeError::Cancelled {
            ConnectionCheckPhase::Cancelled
        } else {
            ConnectionCheckPhase::Failed
        };
        view.error = Some(error);
        self.set_check_terminal(view);
    }

    fn begin_check(&mut self, request: u64, profile: String, revision: u64, slot: u8) {
        let view = Self::check_view(request, profile.clone(), revision, slot);
        let prepared = (|| {
            if self.operation.is_some() || self.view.login_active || self.batch_gate.reserved() {
                return Err(RuntimeError::Busy);
            }
            let account = self
                .drafts
                .probe_account(&profile, revision, slot)
                .map_err(map_selection)?
                .to_owned();
            let snapshot = self
                .store
                .snapshot(&account)
                .map_err(|error| map_login(crate::provider_login::LoginError::Store(error)))?;
            let (ticket, candidate) = self
                .drafts
                .freeze_probe(&profile, revision, slot, &snapshot)
                .map_err(map_selection)?;
            self.spawn_probe_operation(
                ticket,
                candidate,
                Arc::new(NetworkHttpCancel::new()),
                Instant::now() + Duration::from_secs(60),
                None,
            )
        })();
        match prepared {
            Ok(operation) => {
                self.operation = Some(operation);
                self.view.connection_check_active = Some(view);
                self.refresh_connection_view();
            }
            Err(error) => self.reject_check(view, error),
        }
    }

    fn cancel_check_request(&mut self, request: u64) -> Result<(), RuntimeError> {
        if !self
            .view
            .connection_check_active
            .as_ref()
            .is_some_and(|active| active.request == request)
        {
            return Err(RuntimeError::InvalidAction);
        }
        self.cancel_operation();
        Ok(())
    }

    fn retry_readiness(&mut self, request: u64) -> Result<(), RuntimeError> {
        let Some(pending) = self
            .readiness_pending
            .as_ref()
            .filter(|pending| pending.request == request)
        else {
            return Err(RuntimeError::InvalidAction);
        };
        let snapshots: Vec<_> = pending.snapshots.values().cloned().collect();
        let mut first_error = None;
        let mut stale = false;
        for snapshot in snapshots {
            match self.store.mark_ready_for_snapshot(&snapshot) {
                Ok(_) => {
                    self.readiness_pending
                        .as_mut()
                        .unwrap()
                        .snapshots
                        .remove(&snapshot.summary.id);
                }
                Err(crate::provider_accounts::ReadinessError::Persist) => {
                    first_error.get_or_insert(RuntimeError::Storage);
                }
                Err(crate::provider_accounts::ReadinessError::Authority(error)) => {
                    self.readiness_pending
                        .as_mut()
                        .unwrap()
                        .snapshots
                        .remove(&snapshot.summary.id);
                    first_error
                        .get_or_insert(map_login(crate::provider_login::LoginError::Store(error)));
                    stale = true;
                }
            }
        }
        if self
            .readiness_pending
            .as_ref()
            .is_some_and(|pending| pending.snapshots.is_empty())
        {
            self.readiness_pending = None;
        }
        if let Some(terminal) = &mut self.view.connection_check_last_terminal
            && terminal.request == request
        {
            terminal.warning = if stale {
                Some(ConnectionCheckWarning::ReadinessStale)
            } else {
                self.readiness_pending
                    .as_ref()
                    .map(|_| ConnectionCheckWarning::ReadinessStorage)
            };
            if stale && terminal.phase != ConnectionCheckPhase::Applied {
                terminal.phase = ConnectionCheckPhase::Failed;
                terminal.error = first_error;
            }
        }
        self.observe_connection_accounts(true);
        self.project_connection_drafts();
        if let Some(error) = first_error {
            Err(error)
        } else {
            self.refresh_view()
        }
    }

    fn finish_check(
        &mut self,
        ticket: crate::provider_selection::ProbeTicket,
        permit: crate::provider_selection::ProbePermit,
        account: &str,
        result: Result<OperationResult, RuntimeError>,
        cancelled: bool,
    ) {
        let Some(mut view) = self.view.connection_check_active.take() else {
            return;
        };
        let result = self.accept_check_result(ticket, permit, account, result, cancelled, None);
        match result {
            Ok(snapshot) => {
                self.connection_accounts
                    .insert(account.into(), Ok(snapshot.clone()));
                // Timestamp comes from the actual accepted proof, never a new
                // synthetic success time or account-wide readiness flag.
                view.checked_unix_ms = self
                    .drafts
                    .views(&self.connection_accounts)
                    .into_iter()
                    .find(|profile| profile.id == view.profile)
                    .and_then(|profile| {
                        profile
                            .allocations
                            .into_iter()
                            .find(|row| row.slot == view.slot)
                    })
                    .and_then(|row| row.checked_unix_ms);
                view.phase = ConnectionCheckPhase::Verified;
                view.completed = 1;
                self.readiness_pending = None;
                if let Err(error) = self.record_readiness(view.request, snapshot) {
                    view.phase = ConnectionCheckPhase::Failed;
                    view.error = Some(error);
                }
                view.warning = self
                    .readiness_pending
                    .as_ref()
                    .map(|_| ConnectionCheckWarning::ReadinessStorage);
                self.set_check_terminal(view);
            }
            Err(error) => self.reject_check(view, error),
        }
    }

    fn begin_catalogue(&mut self, account: String) -> Result<(), RuntimeError> {
        if self.operation.is_some() || self.view.login_active || self.batch.is_some() {
            return Err(RuntimeError::Busy);
        }
        let snapshot = self.store.snapshot(&account);
        self.catalogues.observe(&account, snapshot.as_ref().ok());
        let ticket = match snapshot
            .map_err(|error| map_login(crate::provider_login::LoginError::Store(error)))
            .and_then(|snapshot| self.catalogues.begin(&snapshot).map_err(map_selection))
        {
            Ok(ticket) => ticket,
            Err(error) => {
                // Unknown IDs cannot grow the bounded per-account error map.
                if self.view.accounts.iter().any(|entry| entry.id == account) {
                    self.catalogue_errors.insert(account, error);
                }
                self.refresh_connection_view();
                return Err(error);
            }
        };
        let requested = ticket.authority().clone();
        let gateway = self.gateway.clone();
        let cancel = Arc::new(NetworkHttpCancel::new());
        let network = cancel.clone();
        let selected = account.clone();
        let worker = match std::thread::Builder::new()
            .name("minicon-provider-catalogue".into())
            .spawn(move || {
                gateway
                    .catalogue(&selected, &requested, &network)
                    .map(OperationResult::Catalog)
                    .map_err(map_server)
            }) {
            Ok(worker) => worker,
            Err(_) => {
                let _ = self.catalogues.fail(&ticket);
                self.catalogue_errors
                    .insert(account, RuntimeError::WorkerUnavailable);
                self.refresh_connection_view();
                return Err(RuntimeError::WorkerUnavailable);
            }
        };
        self.catalogue_errors.remove(&account);
        self.operation = Some(Operation {
            cancel,
            worker,
            account,
            kind: OperationKind::Catalogue(ticket),
        });
        self.refresh_connection_view();
        Ok(())
    }

    fn login_event(&mut self, event: LoginEvent) -> Option<Effect> {
        if let Some((pending, account)) = &self.removing {
            let terminal = match &event {
                LoginEvent::LoggedOut {
                    attempt,
                    account_id,
                    ..
                } if attempt == pending && account_id == account => Some(true),
                LoginEvent::Cancelled { attempt } | LoginEvent::Failed { attempt, .. }
                    if attempt == pending =>
                {
                    Some(false)
                }
                _ => None,
            };
            if let Some(removed) = terminal {
                if removed
                    || self
                        .store
                        .list()
                        .is_ok_and(|accounts| accounts.iter().any(|entry| &entry.id == account))
                {
                    self.gateway.forget_account(account);
                }
                self.removing = None;
            }
        }
        let terminal = !matches!(&event, LoginEvent::Challenge { .. });
        let effect = login_event(event, &mut self.view, &self.store);
        if terminal && let Err(error) = self.refresh_view() {
            self.view.status = Some(error.to_string());
        }
        effect
    }
    fn action(
        &mut self,
        action: Action,
        login: &mut LoginCoordinator,
    ) -> Result<Option<Effect>, RuntimeError> {
        let Some(action) = self.connection_action(action) else {
            return Ok(None);
        };
        match action {
            Action::VerifyAndApplyConnectionProfile {
                request,
                profile,
                expected_revision,
                plan_id,
                paid_slots,
            } => self.begin_profile_check(
                request,
                profile,
                expected_revision,
                plan_id,
                paid_slots,
                self.stop_epoch.load(Ordering::Acquire),
            ),
            Action::VerifyConnectionAllocation {
                request,
                profile,
                expected_revision,
                slot,
            } => self.begin_check(request, profile, expected_revision, slot),
            Action::CancelConnectionVerification { request } => {
                self.cancel_check_request(request)?
            }
            Action::RetryConnectionReadiness { request } => self.retry_readiness(request)?,
            Action::Stop => self.stop(),
            Action::CancelLogin => {
                let _ = login.cancel();
            }
            Action::Connect {
                provider,
                label,
                login: login_network,
                service,
            } => {
                if self.operation.is_some() || self.batch.is_some() {
                    return Err(RuntimeError::Busy);
                }
                let preferences = AccountPreferences::pending(login_network, service)
                    .map_err(|_| RuntimeError::InvalidAction)?;
                let attempt = login
                    .start_configured(
                        provider,
                        if label.trim().is_empty() {
                            match provider {
                                Provider::DeepSeek => "DeepSeek",
                                Provider::OpenCode => "OpenCode",
                            }
                            .into()
                        } else {
                            label
                        },
                        preferences,
                    )
                    .map_err(map_login)?;
                self.view.login_active = true;
                self.view.auth_attempt = Some(attempt);
                self.view.auth_result = None;
            }
            Action::Reconnect(account) => {
                if self.operation.is_some() || self.view.login_active || self.batch.is_some() {
                    return Err(RuntimeError::Busy);
                }
                let snapshot = self
                    .store
                    .snapshot(&account)
                    .map_err(|error| map_login(crate::provider_login::LoginError::Store(error)))?;
                let attempt = login.start_reconnect(snapshot).map_err(map_login)?;
                self.view.login_active = true;
                self.view.auth_attempt = Some(attempt);
                self.view.auth_result = None;
                self.view.status = Some("Reconnect in progress. Existing account settings are retained; model verification is required after success.".into());
            }
            Action::RefreshAccounts => self.refresh_view()?,
            Action::SaveAccount { request, edit } => {
                let result = self.gateway.update_account(&edit).map_err(|error| {
                    if error == ServerError::BindingChanged {
                        RuntimeError::AccountChanged
                    } else if error == ServerError::InvalidConfig {
                        RuntimeError::InvalidAction
                    } else {
                        map_server(error)
                    }
                });
                self.view
                    .account_save_receipts
                    .retain(|receipt| receipt.account != edit.account);
                if self.view.account_save_receipts.len() >= 16 {
                    self.view.account_save_receipts.remove(0);
                }
                self.view.account_save_receipts.push(AccountSaveReceipt {
                    account: edit.account,
                    request,
                    result,
                });
                if let Err(error) = result {
                    self.refresh_view()?;
                    return Err(error);
                }
                self.view.status = Some("Account settings saved.".into());
            }
            Action::RefreshAccountModels(account) => self.begin_catalogue(account)?,
            Action::SetPort(port) => {
                if port < 1024
                    || !matches!(
                        self.view.listener,
                        ListenerState::Stopped | ListenerState::Error
                    )
                {
                    return Err(RuntimeError::InvalidAction);
                }
                let mut config = self.config.clone();
                config.port = port;
                self.save(config)?;
                self.view.listener = ListenerState::Stopped;
                self.view.actual_port = None;
            }
            #[cfg(test)]
            Action::SelectAccount(_legacy_selection) | Action::SelectModel(_legacy_selection) => {
                return Err(RuntimeError::VerificationRequired);
            }
            Action::FocusConnectionProfile(_)
            | Action::ApplyConnectionProfile { .. }
            | Action::CreateConnectionProfile { .. }
            | Action::EditConnectionProfile { .. }
            | Action::ChooseConnectionModel { .. }
            | Action::RemoveConnectionAllocation { .. }
            | Action::RevertConnectionProfile { .. }
            | Action::DiscardUnappliedProfile { .. } => return Err(RuntimeError::InvalidAction),
            Action::Start => {
                if self.server.is_some() || self.operation.is_some() || self.batch.is_some() {
                    return Err(RuntimeError::Busy);
                }
                self.view.listener = ListenerState::Starting;
                match self.gateway.start(self.config.port) {
                    Ok(server) => {
                        self.view.actual_port = Some(server.address().port());
                        self.server = Some(server);
                        self.view.listener = ListenerState::Listening;
                    }
                    Err(error) => {
                        self.view.listener = ListenerState::Error;
                        return Err(map_server(error));
                    }
                }
            }
            Action::RemoveAccount(id) => {
                if !self
                    .store
                    .list()
                    .map_err(|_| RuntimeError::Storage)?
                    .iter()
                    .any(|account| account.id == id)
                {
                    return Err(RuntimeError::MissingAccount);
                }
                let attempt = login.logout(id.clone()).map_err(|_| RuntimeError::Busy)?;
                self.removing = Some((attempt, id.clone()));
                self.gateway.disable_account(&id);
                if self
                    .operation
                    .as_ref()
                    .is_some_and(|operation| operation.account == id)
                {
                    self.cancel_operation();
                }
                self.view.login_active = true;
            }
            Action::ProfileKey {
                request,
                profile,
                operation,
            } => {
                return Ok(self.perform_key(
                    request,
                    profile,
                    operation,
                    self.stop_epoch.load(Ordering::Acquire),
                ));
            }
            Action::CreateClientKey | Action::CopyClientKey | Action::RevokeClientKey => {
                let operation = match action {
                    Action::CreateClientKey => ProfileKeyOperation::Create,
                    Action::CopyClientKey => ProfileKeyOperation::Copy,
                    _ => ProfileKeyOperation::Revoke,
                };
                let request = self
                    .last_key_request
                    .checked_add(1)
                    .ok_or(RuntimeError::InvalidAction)?;
                return Ok(self.perform_key(
                    request,
                    "default".into(),
                    operation,
                    self.stop_epoch.load(Ordering::Acquire),
                ));
            }
        }
        self.refresh_view()?;
        Ok(None)
    }
}

fn run_owner(
    root: PathBuf,
    metadata: LoginMetadata,
    notify: Arc<dyn Fn() + Send + Sync>,
    pending: Arc<Mutex<Pending>>,
    exit: Arc<AtomicBool>,
    stop_requested: Arc<AtomicU64>,
    incoming: mpsc::Receiver<(u64, Vec<u64>, Action)>,
) {
    let owner = match Owner::load(root) {
        Ok(owner) => owner,
        Err(error) => {
            publish(
                &pending,
                &notify,
                View {
                    load_state: LoadState::Unavailable(error),
                    busy: false,
                    status: Some(error.to_string()),
                    ..View::default()
                },
                None,
            );
            return;
        }
    };
    run_loaded_owner(
        owner,
        metadata,
        notify,
        pending,
        exit,
        stop_requested,
        incoming,
    );
}

fn run_loaded_owner(
    mut owner: Owner,
    metadata: LoginMetadata,
    notify: Arc<dyn Fn() + Send + Sync>,
    pending: Arc<Mutex<Pending>>,
    exit: Arc<AtomicBool>,
    stop_requested: Arc<AtomicU64>,
    incoming: mpsc::Receiver<(u64, Vec<u64>, Action)>,
) {
    owner.batch_gate = pending
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .batch_gate
        .clone();
    owner.stop_epoch = stop_requested.clone();
    owner.progress = Some(ProgressSink {
        pending: pending.clone(),
        notify: notify.clone(),
    });
    let mut login = LoginCoordinator::new(owner.store.clone(), metadata);
    publish(&pending, &notify, owner.view.clone(), None);
    let mut last_stop = 0;
    while !exit.load(Ordering::Acquire) {
        let current_stop = stop_requested.load(Ordering::Acquire);
        if current_stop != last_stop {
            owner.stop();
            last_stop = current_stop;
        }
        owner.poll_operation();
        owner.drive_batch();
        while let Some(event) = login.poll() {
            let effect = owner.login_event(event);
            publish(&pending, &notify, owner.view.clone(), effect);
        }
        match incoming.recv_timeout(Duration::from_millis(20)) {
            Ok((epoch, delivered, action)) => {
                owner
                    .view
                    .connection_receipts
                    .retain(|receipt| !delivered.contains(&receipt.request));
                let stale_network_action = matches!(
                    &action,
                    Action::Start
                        | Action::RefreshAccountModels(_)
                        | Action::VerifyConnectionAllocation { .. }
                        | Action::ApplyConnectionProfile { .. }
                        | Action::VerifyAndApplyConnectionProfile { .. }
                        | Action::ProfileKey { .. }
                );
                if epoch != stop_requested.load(Ordering::Acquire) && stale_network_action {
                    if let Action::ProfileKey {
                        request,
                        profile,
                        operation,
                    } = &action
                    {
                        owner.reject_key(
                            *request,
                            profile.clone(),
                            *operation,
                            RuntimeError::Cancelled,
                        );
                        publish(&pending, &notify, owner.view.clone(), None);
                    }
                    if let Action::VerifyAndApplyConnectionProfile {
                        request,
                        profile,
                        expected_revision,
                        paid_slots,
                        ..
                    } = &action
                    {
                        let mut view = Owner::check_view(
                            *request,
                            profile.clone(),
                            *expected_revision,
                            paid_slots.first().copied().unwrap_or(0),
                        );
                        view.scope = ConnectionCheckScope::Profile;
                        view.total = paid_slots.len() as u8;
                        owner.finish_rejected_profile(view, RuntimeError::Cancelled);
                        publish(&pending, &notify, owner.view.clone(), None);
                    }
                    if let Action::ApplyConnectionProfile {
                        request, profile, ..
                    } = &action
                    {
                        owner.connection_receipt(
                            *request,
                            profile.clone(),
                            ConnectionChange::Apply,
                            Err(RuntimeError::Cancelled),
                        );
                        publish(&pending, &notify, owner.view.clone(), None);
                    }
                    if let Action::VerifyConnectionAllocation {
                        request,
                        profile,
                        expected_revision,
                        slot,
                    } = action
                    {
                        owner.reject_check(
                            Owner::check_view(request, profile, expected_revision, slot),
                            RuntimeError::Cancelled,
                        );
                        publish(&pending, &notify, owner.view.clone(), None);
                    }
                    continue;
                }
                if !matches!(
                    &action,
                    Action::FocusConnectionProfile(_)
                        | Action::ApplyConnectionProfile { .. }
                        | Action::CreateConnectionProfile { .. }
                        | Action::EditConnectionProfile { .. }
                        | Action::ChooseConnectionModel { .. }
                        | Action::RemoveConnectionAllocation { .. }
                        | Action::RevertConnectionProfile { .. }
                        | Action::DiscardUnappliedProfile { .. }
                ) {
                    owner.view.status = None;
                }
                if let Action::ProfileKey {
                    request,
                    profile,
                    operation,
                } = action
                {
                    let effect = owner.perform_key(request, profile, operation, epoch);
                    publish(&pending, &notify, owner.view.clone(), effect);
                    continue;
                }
                let effect = match owner.action(action, &mut login) {
                    Ok(effect) => effect,
                    Err(error) => {
                        owner.view.status = Some(error.to_string());
                        None
                    }
                };
                publish(&pending, &notify, owner.view.clone(), effect);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                publish(&pending, &notify, owner.view.clone(), None);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    owner.stop();
    let _ = login.shutdown(Duration::from_secs(5));
}
fn clone_secret(secret: Option<&Secret>) -> Result<Option<Secret>, RuntimeError> {
    secret
        .map(|secret| Secret::new(secret.expose()).map_err(|_| RuntimeError::Corrupt))
        .transpose()
}
fn read_key(root: &Path) -> Result<Option<Secret>, RuntimeError> {
    read_key_path(&root.join("client-key"))
}
fn read_key_path(path: &Path) -> Result<Option<Secret>, RuntimeError> {
    match filesystem::read_private_bounded(path, 256) {
        Ok(bytes) => Secret::new(&String::from_utf8(bytes).map_err(|_| RuntimeError::Corrupt)?)
            .map(Some)
            .map_err(|_| RuntimeError::Corrupt),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(RuntimeError::Storage),
    }
}
fn prepare_root(root: &Path) -> Result<(), RuntimeError> {
    for ancestor in root.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) if filesystem::metadata_is_link_like(&metadata) => {
                return Err(RuntimeError::InvalidRoot);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(RuntimeError::Storage),
        }
    }
    std::fs::create_dir_all(root).map_err(|_| RuntimeError::Storage)?;
    filesystem::protect_private_directory(root).map_err(|_| RuntimeError::Storage)
}
fn map_login(error: crate::provider_login::LoginError) -> RuntimeError {
    use crate::{
        provider_accounts::StoreError, provider_auth::AuthError, provider_login::LoginError,
    };
    match error {
        LoginError::Busy => RuntimeError::Busy,
        LoginError::WorkerUnavailable => RuntimeError::WorkerUnavailable,
        LoginError::Store(StoreError::Missing) => RuntimeError::MissingAccount,
        LoginError::Store(StoreError::Changed) => RuntimeError::AccountChanged,
        LoginError::Store(StoreError::InvalidAccount)
        | LoginError::Auth(AuthError::InvalidInput) => RuntimeError::InvalidAction,
        LoginError::Store(_) => RuntimeError::Storage,
        _ => RuntimeError::ProviderUnavailable,
    }
}

fn map_selection(error: SelectionError) -> RuntimeError {
    match error {
        SelectionError::InvalidAuthority
        | SelectionError::AuthorityChanged
        | SelectionError::LateResult => RuntimeError::AccountChanged,
        SelectionError::Disabled => RuntimeError::NeedsLogin,
        SelectionError::AccountBound
        | SelectionError::ProfileBound
        | SelectionError::AggregateBound => RuntimeError::Busy,
        SelectionError::DraftChanged => RuntimeError::DraftChanged,
        SelectionError::MissingProfile
        | SelectionError::MissingAlias
        | SelectionError::InvalidDraft => RuntimeError::InvalidAction,
        SelectionError::CatalogueUnavailable | SelectionError::VerificationRequired => {
            RuntimeError::VerificationRequired
        }
        SelectionError::MissingRoute => RuntimeError::MissingModel,
        _ => RuntimeError::ProviderUnavailable,
    }
}

fn map_server(error: ServerError) -> RuntimeError {
    match error {
        ServerError::MissingAccount => RuntimeError::MissingAccount,
        ServerError::MissingModel => RuntimeError::MissingModel,
        ServerError::MissingKey => RuntimeError::MissingClientKey,
        ServerError::NeedsLogin => RuntimeError::NeedsLogin,
        ServerError::Busy => RuntimeError::Busy,
        ServerError::Storage => RuntimeError::Storage,
        ServerError::BindingChanged => RuntimeError::AccountChanged,
        ServerError::Cancelled => RuntimeError::Cancelled,
        _ => RuntimeError::ProviderUnavailable,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn k1_owner_batch_and_stop_gates_refuse_before_key_publication_without_replay() {
        let (root, mut owner, _) = k1_fixture();
        let before = std::fs::read(root.0.join("client-key")).unwrap();
        owner
            .batch_gate
            .register(profile_batch::BatchIdentity {
                request: 10,
                profile: "default".into(),
                revision: 1,
                plan: 1,
                epoch: 0,
            })
            .unwrap();
        let called = AtomicBool::new(false);
        owner.perform_key_with(
            1,
            "default".into(),
            ProfileKeyOperation::Revoke,
            0,
            |_, _| {
                called.store(true, Ordering::Release);
                Err(ServerError::Storage)
            },
        );
        assert_eq!(
            owner.view.profile_key_receipts[0].result,
            Err(RuntimeError::Busy)
        );
        assert!(
            !called.load(Ordering::Acquire),
            "reserved batch reached Owner key I/O"
        );
        owner.batch_gate.finish_request(10);
        owner.batch_gate.acknowledge(10);
        owner.stop_epoch.store(1, Ordering::Release);
        owner.perform_key_with(
            2,
            "default".into(),
            ProfileKeyOperation::Revoke,
            0,
            |_, _| {
                called.store(true, Ordering::Release);
                Err(ServerError::Storage)
            },
        );
        assert_eq!(
            owner.view.profile_key_receipts[0].result,
            Err(RuntimeError::Cancelled)
        );
        assert!(!called.load(Ordering::Acquire));
        assert!(
            std::fs::read(root.0.join("client-key")).unwrap() == before,
            "rejected operation replayed key mutation"
        );
        owner.perform_key(3, "default".into(), ProfileKeyOperation::Revoke, 1);
        assert!(!root.0.join("client-key").exists());
        assert!(!owner.view.local_key_exists);
        assert!(owner.server.is_none());
    }
    #[test]
    fn k1_eight_profile_backlog_and_repeated_copy_delivery_stay_bounded_and_targeted() {
        let (root, owner, _) = cs02_eight_profile_owner();
        drop(owner);
        let mut runtime = root.runtime();
        until(&mut runtime, |view| !view.busy);
        let ids = runtime
            .view()
            .profile_keys
            .iter()
            .map(|row| row.profile.clone())
            .collect::<Vec<_>>();
        assert_eq!(ids.len(), 8);
        for (index, profile) in ids.iter().enumerate() {
            runtime
                .act(Action::ProfileKey {
                    request: index as u64 + 1,
                    profile: profile.clone(),
                    operation: ProfileKeyOperation::Create,
                })
                .unwrap();
        }
        assert_eq!(
            runtime.act(Action::ProfileKey {
                request: 9,
                profile: "default".into(),
                operation: ProfileKeyOperation::Copy
            }),
            Err(RuntimeError::Busy)
        );
        until(&mut runtime, |view| {
            view.profile_keys.iter().all(|row| row.exists)
        });
        assert!(!runtime.keys_reserved());
        for round in 0..2 {
            let first = 9 + round * 8;
            for (index, profile) in ids.iter().enumerate() {
                runtime
                    .act(Action::ProfileKey {
                        request: first + index as u64,
                        profile: profile.clone(),
                        operation: ProfileKeyOperation::Copy,
                    })
                    .unwrap();
            }
            let effects = until(&mut runtime, |view| {
                view.profile_key_receipts.len() == 8
                    && view
                        .profile_key_receipts
                        .iter()
                        .all(|row| row.request >= first)
            });
            let mut copies = std::collections::BTreeMap::new();
            for effect in effects {
                if let Effect::CopyClientKey(copy) = effect {
                    assert!(
                        copies.insert(copy.profile.clone(), copy.request).is_none(),
                        "duplicate one-shot target delivery"
                    );
                }
            }
            assert_eq!(
                copies.len(),
                8,
                "actual eight-profile delivery lost an unresolved secret effect"
            );
            for (index, profile) in ids.iter().enumerate() {
                assert_eq!(copies.get(profile), Some(&(first + index as u64)));
            }
            assert_eq!(runtime.view().profile_keys.len(), 8);
            assert_eq!(runtime.view().profile_key_receipts.len(), 8);
            assert!(!runtime.keys_reserved());
            assert!(
                runtime
                    .poll()
                    .into_iter()
                    .all(|effect| !matches!(effect, Effect::CopyClientKey(_)))
            );
        }
        assert_eq!(runtime.view().listener, ListenerState::Stopped);
    }
    fn k1_fixture() -> (Root, Owner, String) {
        let (root, owner, _) = catalogue_owner();
        filesystem::write_private_atomic(&root.0.join("client-key"), b"k1-synthetic-default")
            .unwrap();
        drop(owner);
        let owner = Owner::load(root.0.clone()).unwrap();
        (root, owner, "b".repeat(32))
    }
    fn k1_meta<'a>(view: &'a View, profile: &str) -> &'a ProfileKeyMetadata {
        view.profile_keys
            .iter()
            .find(|row| row.profile == profile)
            .unwrap()
    }
    #[test]
    fn k1_targeted_custom_create_restart_copy_and_revoke_preserve_default() {
        let (root, owner, profile) = k1_fixture();
        drop(owner);
        let before = std::fs::read(root.0.join("client-key")).unwrap();
        let mut runtime = root.runtime();
        until(&mut runtime, |view| !view.busy);
        assert!(!k1_meta(runtime.view(), &profile).exists);
        runtime
            .act(Action::ProfileKey {
                request: 1,
                profile: profile.clone(),
                operation: ProfileKeyOperation::Create,
            })
            .unwrap();
        until(&mut runtime, |view| {
            view.profile_key_receipts
                .iter()
                .any(|result| result.request == 1)
        });
        let result = runtime
            .view()
            .profile_key_receipts
            .iter()
            .find(|result| result.request == 1)
            .unwrap();
        assert_eq!(
            (result.profile.as_str(), result.result),
            (profile.as_str(), Ok(()))
        );
        assert!(k1_meta(runtime.view(), &profile).exists);
        assert!(
            std::fs::read(root.0.join("client-key")).unwrap() == before,
            "custom Create changed default key bytes"
        );
        let stored =
            filesystem::read_private_bounded(&root.0.join(format!("client-key-{profile}")), 256)
                .unwrap();
        drop(runtime);
        let mut runtime = root.runtime();
        until(&mut runtime, |view| !view.busy);
        assert!(
            k1_meta(runtime.view(), &profile).exists,
            "actual private custom file must reload into metadata"
        );
        runtime
            .act(Action::ProfileKey {
                request: 2,
                profile: profile.clone(),
                operation: ProfileKeyOperation::Copy,
            })
            .unwrap();
        runtime
            .act(Action::FocusConnectionProfile("default".into()))
            .unwrap();
        let effects = until(&mut runtime, |view| {
            view.profile_key_receipts
                .iter()
                .any(|result| result.request == 2)
        });
        let copy = effects
            .into_iter()
            .find_map(|effect| {
                if let Effect::CopyClientKey(copy) = effect {
                    Some(copy)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!((copy.profile.as_str(), copy.request), (profile.as_str(), 2));
        assert!(
            copy.expose().as_bytes() == stored.as_slice(),
            "Copy returned a different profile's private bytes"
        );
        assert!(
            !format!("{copy:?}").contains(copy.expose()),
            "Secret effect Debug exposed key"
        );
        assert!(
            runtime
                .poll()
                .into_iter()
                .all(|effect| !matches!(effect, Effect::CopyClientKey(_))),
            "Copy effect was replayed"
        );
        runtime
            .act(Action::ProfileKey {
                request: 3,
                profile: profile.clone(),
                operation: ProfileKeyOperation::Revoke,
            })
            .unwrap();
        until(&mut runtime, |view| {
            view.profile_key_receipts
                .iter()
                .any(|result| result.request == 3)
        });
        assert!(!k1_meta(runtime.view(), &profile).exists);
        assert!(!root.0.join(format!("client-key-{profile}")).exists());
        assert!(
            std::fs::read(root.0.join("client-key")).unwrap() == before,
            "custom Revoke changed default bytes"
        );
        assert_eq!(runtime.view().listener, ListenerState::Stopped);
        drop(runtime);
        let mut restarted = root.runtime();
        until(&mut restarted, |view| !view.busy);
        assert!(!k1_meta(restarted.view(), &profile).exists);
        assert!(restarted.view().local_key_exists);
    }
    #[test]
    fn k1_unapplied_profile_refuses_without_creating_an_orphan_file() {
        let (root, mut owner, _) = k1_fixture();
        let draft = "f".repeat(32);
        let revision = owner.drafts.create_profile(&draft).unwrap();
        assert!(revision > 0);
        assert!(
            owner
                .perform_key(1, draft.clone(), ProfileKeyOperation::Create, 0)
                .is_none()
        );
        assert_eq!(
            owner
                .view
                .profile_key_receipts
                .iter()
                .find(|result| result.request == 1)
                .unwrap()
                .result,
            Err(RuntimeError::InvalidAction)
        );
        assert!(!root.0.join(format!("client-key-{draft}")).exists());
        assert!(owner.operation.is_none() && owner.server.is_none());
    }
    #[test]
    fn k1_failed_publication_preserves_cached_default_and_disk_before_live_change() {
        let (root, mut owner, _) = k1_fixture();
        let before = std::fs::read(root.0.join("client-key")).unwrap();
        let epoch = k1_meta(&owner.view, "default").epoch;
        let called = AtomicBool::new(false);
        owner.perform_key_with(
            1,
            "default".into(),
            ProfileKeyOperation::Revoke,
            0,
            |_, _| {
                called.store(true, Ordering::Release);
                Err(ServerError::Storage)
            },
        );
        assert!(called.load(Ordering::Acquire));
        assert_eq!(
            owner.view.profile_key_receipts[0].result,
            Err(RuntimeError::Storage)
        );
        assert!(owner.view.local_key_exists);
        assert_eq!(k1_meta(&owner.view, "default").epoch, epoch);
        assert!(
            std::fs::read(root.0.join("client-key")).unwrap() == before,
            "unpublished failure changed actual disk bytes"
        );
        let effect = owner
            .perform_key(2, "default".into(), ProfileKeyOperation::Copy, 0)
            .unwrap();
        let Effect::CopyClientKey(copy) = effect else {
            panic!("expected explicit Copy")
        };
        assert!(
            copy.expose().as_bytes() == before.as_slice(),
            "failed Revoke changed cached Copy bytes"
        );
    }
    #[test]
    fn k1_actual_remove_warning_and_absent_nochange_do_not_fake_durable_rollback_or_clear_warning()
    {
        let (root, mut owner, _) = k1_fixture();
        let old_epoch = k1_meta(&owner.view, "default").epoch;
        owner.perform_key_with(
            1,
            "default".into(),
            ProfileKeyOperation::Revoke,
            0,
            |path, key| {
                assert!(key.is_none());
                std::fs::remove_file(path).unwrap();
                Ok(crate::provider_server::ClientKeyPublication::DurabilityUnconfirmed)
            },
        );
        assert!(!root.0.join("client-key").exists());
        assert!(!owner.view.local_key_exists);
        assert!(k1_meta(&owner.view, "default").durability_unconfirmed);
        assert!(k1_meta(&owner.view, "default").epoch > old_epoch);
        let revoked_epoch = k1_meta(&owner.view, "default").epoch;
        owner.perform_key(2, "default".into(), ProfileKeyOperation::Revoke, 0);
        let receipt = owner
            .view
            .profile_key_receipts
            .iter()
            .find(|row| row.request == 2)
            .unwrap();
        assert_eq!(
            receipt.publication,
            Some(crate::provider_server::ClientKeyPublication::NoChange)
        );
        assert_eq!(k1_meta(&owner.view, "default").epoch, revoked_epoch);
        assert!(
            k1_meta(&owner.view, "default").durability_unconfirmed,
            "absent Revoke cannot invent a successful durability barrier"
        );
        assert!(
            owner
                .view
                .status
                .as_deref()
                .unwrap()
                .contains("unconfirmed")
        );
        owner.perform_key(3, "default".into(), ProfileKeyOperation::Create, 0);
        assert!(owner.view.local_key_exists);
        assert!(root.0.join("client-key").is_file());
        assert!(!k1_meta(&owner.view, "default").durability_unconfirmed);
        assert!(k1_meta(&owner.view, "default").epoch > revoked_epoch);
    }
    #[test]
    fn k1_failed_queue_send_does_not_claim_request_or_block_a_later_explicit_click() {
        let (commands, incoming) = mpsc::sync_channel(1);
        let pending = Arc::new(Mutex::new(Pending::default()));
        let view = View {
            profile_keys: vec![ProfileKeyMetadata {
                profile: "default".into(),
                state: profile_keys::ProfileKeyState::NotCreated,
                exists: false,
                epoch: 1,
                durability_unconfirmed: false,
            }],
            ..View::default()
        };
        let mut runtime = Runtime {
            commands,
            connection_inflight: Default::default(),
            last_connection_request: 0,
            pending,
            view,
            exit: Arc::new(AtomicBool::new(false)),
            stop_requested: Arc::new(AtomicU64::new(0)),
            worker: None,
        };
        runtime.act(Action::RefreshAccounts).unwrap();
        let action = || Action::ProfileKey {
            request: 1,
            profile: "default".into(),
            operation: ProfileKeyOperation::Create,
        };
        assert_eq!(runtime.act(action()), Err(RuntimeError::Busy));
        assert!(!runtime.keys_reserved());
        incoming.recv().unwrap();
        assert_eq!(
            runtime.act(action()),
            Ok(()),
            "failed send must not advance the monotonic request or reserve a slot"
        );
        assert!(runtime.keys_reserved());
        assert_eq!(
            runtime.act(Action::ProfileKey {
                request: 2,
                profile: "default".into(),
                operation: ProfileKeyOperation::Create
            }),
            Err(RuntimeError::Busy)
        );
    }
    #[test]
    fn k1_later_revoke_intent_cancels_queued_copy_without_replaying_busy_mutation() {
        let (root, owner, _) = k1_fixture();
        let before = std::fs::read(root.0.join("client-key")).unwrap();
        let (ready, wait_ready) = mpsc::channel();
        let (release, wait_release) = mpsc::channel();
        let wait = Arc::new(Mutex::new(Some(wait_release)));
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if let Some(wait) = wait.lock().unwrap().take() {
                ready.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        });
        let (commands, incoming) = mpsc::sync_channel(16);
        let pending = Arc::new(Mutex::new(Pending::default()));
        let exit = Arc::new(AtomicBool::new(false));
        let stop_requested = Arc::new(AtomicU64::new(0));
        let (output, stopping, stop) = (pending.clone(), exit.clone(), stop_requested.clone());
        let worker = std::thread::spawn(move || {
            run_loaded_owner(
                owner,
                LoginMetadata {
                    version: "synthetic".into(),
                    locale: "en".into(),
                    timezone_offset_seconds: 0,
                },
                notify,
                output,
                stopping,
                stop,
                incoming,
            )
        });
        let mut runtime = Runtime {
            commands,
            connection_inflight: Default::default(),
            last_connection_request: 0,
            pending,
            view: View::default(),
            exit,
            stop_requested,
            worker: Some(worker),
        };
        wait_ready.recv_timeout(Duration::from_secs(5)).unwrap();
        let _ = runtime.poll();
        runtime
            .act(Action::ProfileKey {
                request: 1,
                profile: "default".into(),
                operation: ProfileKeyOperation::Copy,
            })
            .unwrap();
        assert_eq!(
            runtime.act(Action::ProfileKey {
                request: 2,
                profile: "default".into(),
                operation: ProfileKeyOperation::Revoke
            }),
            Err(RuntimeError::Busy)
        );
        release.send(()).unwrap();
        let effects = until(&mut runtime, |view| {
            view.profile_key_receipts.iter().any(|row| row.request == 1)
        });
        assert!(
            effects
                .into_iter()
                .all(|effect| !matches!(effect, Effect::CopyClientKey(_))),
            "cancelled queued Copy leaked a Secret effect"
        );
        assert_eq!(
            runtime.view().profile_key_receipts[0].result,
            Err(RuntimeError::Cancelled)
        );
        assert!(runtime.view().local_key_exists);
        assert!(
            std::fs::read(root.0.join("client-key")).unwrap() == before,
            "Busy mutation was silently replayed"
        );
        runtime
            .act(Action::ProfileKey {
                request: 2,
                profile: "default".into(),
                operation: ProfileKeyOperation::Revoke,
            })
            .unwrap();
        until(&mut runtime, |view| !view.local_key_exists);
        assert!(!root.0.join("client-key").exists());
    }
    #[test]
    fn cs05_real_poll_observes_admitted_applying_while_actual_writer_is_blocked() {
        let (_root, mut owner) = cs05_one_checked_batch();
        let pending = Arc::new(Mutex::new(Pending::default()));
        pending.lock().unwrap().batch_gate = owner.batch_gate.clone();
        owner.progress = Some(ProgressSink {
            pending: pending.clone(),
            notify: Arc::new(|| {}),
        });
        let (entered, wait_entered) = mpsc::channel();
        let (release, wait_release) = mpsc::channel();
        let output = pending.clone();
        let worker = std::thread::spawn(move || {
            owner.commit_profile_check_with(|path, bytes| {
                entered.send(()).unwrap();
                wait_release.recv_timeout(Duration::from_secs(5)).unwrap();
                minicon_platform::filesystem_publish::write_private_atomic(path, bytes).unwrap();
                Ok(ConfigPublication::Durable)
            });
            let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
            publish(&output, &notify, owner.view.clone(), None);
            owner
        });
        let (commands, _incoming) = mpsc::sync_channel(1);
        let mut runtime = Runtime {
            commands,
            connection_inflight: Default::default(),
            last_connection_request: 0,
            pending,
            view: View::default(),
            exit: Arc::new(AtomicBool::new(false)),
            stop_requested: Arc::new(AtomicU64::new(0)),
            worker: None,
        };
        wait_entered.recv_timeout(Duration::from_secs(5)).unwrap();
        let _ = runtime.poll();
        assert_eq!(
            runtime
                .view
                .connection_check_active
                .as_ref()
                .map(|check| check.phase),
            Some(ConnectionCheckPhase::Applying),
            "poll must observe admission before synchronous publication returns"
        );
        assert_eq!(
            runtime.act(Action::CancelConnectionVerification { request: 30 }),
            Err(RuntimeError::InvalidAction)
        );
        release.send(()).unwrap();
        let owner = worker.join().unwrap();
        let _ = runtime.poll();
        assert_eq!(
            runtime
                .view
                .connection_check_last_terminal
                .as_ref()
                .unwrap()
                .phase,
            ConnectionCheckPhase::Applied
        );
        assert_eq!(
            runtime
                .view
                .connection_check_last_terminal
                .as_ref()
                .unwrap()
                .applied_revision,
            owner.drafts.revision("default")
        );
    }

    #[test]
    fn cs05_data_intent_between_children_prevents_second_real_post_and_partial_apply() {
        let (root, mut owner, accounts) = catalogue_owner();
        let profile = cs05_prepare_sparse(&mut owner, &accounts);
        let disk = std::fs::read(root.0.join("gateway.json")).unwrap();
        let paid = Arc::new(AtomicU64::new(0));
        let first = paid.clone();
        let second = paid.clone();
        let peer = Cs03Peer::observing(vec![
            Box::new(move |body| {
                first.fetch_add(1, Ordering::AcqRel);
                cs03_reply(body)
            }),
            Box::new(move |body| {
                second.fetch_add(1, Ordering::AcqRel);
                cs03_reply(body)
            }),
        ]);
        owner.gateway.fixture_catalogue_upstream(peer.url.clone());
        cs05_begin(&mut owner, &profile, 40);
        owner.drive_batch();
        cs03_wait_finished_without_poll(&owner);
        owner.poll_operation();
        assert_eq!(
            owner
                .view
                .connection_check_active
                .as_ref()
                .unwrap()
                .completed,
            1
        );
        assert!(owner.batch_gate.invalidate_profile(&profile));
        cs05_finish(&mut owner);
        assert_eq!(
            paid.load(Ordering::Acquire),
            1,
            "data intent dispatched another paid child"
        );
        assert_eq!(
            owner
                .view
                .connection_check_last_terminal
                .as_ref()
                .unwrap()
                .error,
            Some(RuntimeError::DraftChanged)
        );
        assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), disk);
        assert_eq!(
            owner
                .view
                .connection_plans
                .iter()
                .find(|plan| plan.profile == profile)
                .unwrap()
                .paid_slots,
            vec![7]
        );
    }
    fn cs05_one_checked_batch() -> (Root, Owner) {
        let (root, mut owner, accounts) = catalogue_owner();
        cs04_choose_synthetic_catalogue(&mut owner, "default", 0, &accounts[0], "new", "new-wire");
        let peer = Cs03Peer::new(cs03_reply);
        owner.gateway.fixture_catalogue_upstream(peer.url.clone());
        cs05_begin(&mut owner, "default", 30);
        owner.drive_batch();
        cs03_wait_finished_without_poll(&owner);
        owner.poll_operation();
        drop(peer);
        assert_eq!(
            owner
                .view
                .connection_check_active
                .as_ref()
                .unwrap()
                .completed,
            1
        );
        assert!(owner.operation.is_none());
        (root, owner)
    }
    #[test]
    fn cs05_pre_admission_cancel_prevents_actual_publication_and_keeps_successful_proof() {
        let (root, mut owner) = cs05_one_checked_batch();
        let before = std::fs::read(root.0.join("gateway.json")).unwrap();
        owner.batch_gate.cancel_exact(30).unwrap();
        let called = AtomicBool::new(false);
        owner.commit_profile_check_with(|_, _| {
            called.store(true, Ordering::Release);
            Err(ServerError::Storage)
        });
        assert!(
            !called.load(Ordering::Acquire),
            "revoked parent reached the actual publication callback"
        );
        assert_eq!(
            owner
                .view
                .connection_check_last_terminal
                .as_ref()
                .unwrap()
                .error,
            Some(RuntimeError::Cancelled)
        );
        assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), before);
        assert_eq!(
            owner
                .view
                .connection_plans
                .iter()
                .find(|plan| plan.profile == "default")
                .unwrap()
                .paid_slots,
            Vec::<u8>::new()
        );
    }
    #[test]
    fn cs05_commit_admitted_late_cancel_cannot_undo_actual_published_warning() {
        let (root, mut owner) = cs05_one_checked_batch();
        let gate = owner.batch_gate.clone();
        let called = AtomicBool::new(false);
        owner.commit_profile_check_with(|path, bytes| {
            called.store(true, Ordering::Release);
            assert_eq!(
                gate.cancel_exact(30),
                Err(RuntimeError::InvalidAction),
                "late Cancel must be rejected at real prewrite admission"
            );
            assert!(!gate.invalidate_profile("default"));
            minicon_platform::filesystem_publish::write_private_atomic(path, bytes).unwrap();
            Ok(ConfigPublication::DurabilityUnconfirmed)
        });
        assert!(called.load(Ordering::Acquire));
        let result = owner.view.connection_check_last_terminal.as_ref().unwrap();
        assert_eq!(result.phase, ConnectionCheckPhase::Applied);
        assert!(result.applied_revision.is_some());
        assert!(owner.view.config_durability_unconfirmed);
        assert_eq!(Owner::load(root.0.clone()).unwrap().config, owner.config);
    }
    #[test]
    fn cs05_unpublished_write_failure_is_failed_and_retry_is_zero_paid_plain_apply() {
        let (root, mut owner) = cs05_one_checked_batch();
        let before = std::fs::read(root.0.join("gateway.json")).unwrap();
        let called = AtomicBool::new(false);
        owner.commit_profile_check_with(|_, _| {
            called.store(true, Ordering::Release);
            Err(ServerError::Storage)
        });
        assert!(called.load(Ordering::Acquire));
        let result = owner.view.connection_check_last_terminal.as_ref().unwrap();
        assert_eq!(
            (result.phase, result.error, result.applied_revision),
            (
                ConnectionCheckPhase::Failed,
                Some(RuntimeError::Storage),
                None
            )
        );
        assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), before);
        let plan = owner
            .view
            .connection_plans
            .iter()
            .find(|plan| plan.profile == "default")
            .unwrap()
            .clone();
        assert!(plan.paid_slots.is_empty());
        assert_eq!(
            owner.apply_connection_profile("default", plan.revision),
            Ok(())
        );
        assert!(owner.operation.is_none());
    }
    fn cs05_prepare_sparse(owner: &mut Owner, accounts: &[String; 2]) -> String {
        let profile = "b".repeat(32);
        let form = owner
            .view
            .connection_profiles
            .iter()
            .find(|form| form.id == profile)
            .unwrap()
            .clone();
        let mut input = form.input;
        input.allocations.push(ConnectionAllocationInput {
            slot: 7,
            alias: "second".into(),
            account: Some(accounts[0].clone()),
            model: "second-wire".into(),
        });
        owner
            .drafts
            .edit_input(&profile, form.revision, input)
            .unwrap();
        cs04_choose_synthetic_catalogue(owner, &profile, 0, &accounts[1], "first", "first-wire");
        cs04_choose_synthetic_catalogue(owner, &profile, 7, &accounts[0], "second", "second-wire");
        profile
    }
    fn cs05_begin(owner: &mut Owner, profile: &str, request: u64) -> ConnectionPlanView {
        owner.refresh_view().unwrap();
        let plan = owner
            .view
            .connection_plans
            .iter()
            .find(|plan| plan.profile == profile)
            .unwrap()
            .clone();
        assert_eq!(
            plan.error, None,
            "a complete controlled candidate must produce a quote"
        );
        owner
            .batch_gate
            .register(profile_batch::BatchIdentity {
                request,
                profile: profile.into(),
                revision: plan.revision,
                plan: plan.id,
                epoch: owner.stop_epoch.load(Ordering::Acquire),
            })
            .unwrap();
        owner.begin_profile_check(
            request,
            profile.into(),
            plan.revision,
            plan.id,
            plan.paid_slots.clone(),
            owner.stop_epoch.load(Ordering::Acquire),
        );
        assert!(
            owner.batch.is_some(),
            "{plan:?} / {:?}",
            owner.view.connection_check_last_terminal
        );
        plan
    }
    fn cs05_finish(owner: &mut Owner) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while owner.batch.is_some() || owner.operation.is_some() {
            owner.poll_operation();
            owner.drive_batch();
            assert!(
                Instant::now() < deadline,
                "profile operation did not terminate"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn cs05_sparse_two_account_plan_orders_actual_posts_and_publishes_one_profile() {
        let (root, mut owner, accounts) = catalogue_owner();
        let profile = cs05_prepare_sparse(&mut owner, &accounts);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let peer = Cs03Peer::sequence(vec![
            Box::new(move |body| {
                seen.lock()
                    .unwrap()
                    .push(body["model"].as_str().unwrap().to_owned());
                cs03_reply(body)
            }),
            {
                let seen = requests.clone();
                Box::new(move |body| {
                    seen.lock()
                        .unwrap()
                        .push(body["model"].as_str().unwrap().to_owned());
                    cs03_reply(body)
                })
            },
        ]);
        owner.gateway.fixture_catalogue_upstream(peer.url.clone());
        let before_default = owner
            .config
            .resolve_allocation("default", None)
            .unwrap()
            .route
            .clone();
        let plan = cs05_begin(&mut owner, &profile, 10);
        assert_eq!(plan.paid_slots, vec![0, 7]);
        cs05_finish(&mut owner);
        assert_eq!(*requests.lock().unwrap(), vec!["first-wire", "second-wire"]);
        let result = owner.view.connection_check_last_terminal.as_ref().unwrap();
        assert_eq!(
            (result.scope, result.phase, result.completed, result.total),
            (
                ConnectionCheckScope::Profile,
                ConnectionCheckPhase::Applied,
                2,
                2
            )
        );
        assert!(result.applied_revision.unwrap() > plan.revision);
        assert_eq!(
            owner
                .config
                .resolve_allocation("default", None)
                .unwrap()
                .route,
            &before_default
        );
        assert_eq!(Owner::load(root.0.clone()).unwrap().config, owner.config);
        assert_eq!(owner.view.listener, ListenerState::Stopped);
        for alias in ["work", "second"] {
            assert!(matches!(
                owner
                    .config
                    .resolve_allocation(&profile, Some(alias))
                    .unwrap()
                    .evidence
                    .unwrap()
                    .origin,
                crate::provider_server::AppliedOrigin::Inference { .. }
            ));
        }
    }

    #[test]
    fn cs05_partial_failure_retains_first_real_proof_and_explicit_retry_pays_only_missing_slot() {
        let (root, mut owner, accounts) = catalogue_owner();
        let profile = cs05_prepare_sparse(&mut owner, &accounts);
        let before = std::fs::read(root.0.join("gateway.json")).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let peer = Cs03Peer::sequence(vec![
            Box::new(move |body| {
                seen.lock()
                    .unwrap()
                    .push(body["model"].as_str().unwrap().to_owned());
                cs03_reply(body)
            }),
            {
                let seen = requests.clone();
                Box::new(move |body| {
                    seen.lock()
                        .unwrap()
                        .push(body["model"].as_str().unwrap().to_owned());
                    serde_json::json!({"invalid":"not an inference response"})
                })
            },
        ]);
        owner.gateway.fixture_catalogue_upstream(peer.url.clone());
        let plan = cs05_begin(&mut owner, &profile, 11);
        cs05_finish(&mut owner);
        drop(peer);
        assert_eq!(
            owner
                .view
                .connection_check_last_terminal
                .as_ref()
                .unwrap()
                .phase,
            ConnectionCheckPhase::Failed
        );
        assert_eq!(
            owner
                .view
                .connection_check_last_terminal
                .as_ref()
                .unwrap()
                .completed,
            1
        );
        assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), before);
        assert_eq!(*requests.lock().unwrap(), vec!["first-wire", "second-wire"]);
        let next = owner
            .view
            .connection_plans
            .iter()
            .find(|p| p.profile == profile)
            .unwrap()
            .clone();
        assert_eq!(next.paid_slots, vec![7]);
        assert_ne!(next.id, plan.id);
        owner.batch_gate.acknowledge(11);
        let retry = Cs03Peer::new(|body| {
            assert_eq!(body["model"], "second-wire");
            cs03_reply(body)
        });
        owner.gateway.fixture_catalogue_upstream(retry.url.clone());
        cs05_begin(&mut owner, &profile, 12);
        cs05_finish(&mut owner);
        assert_eq!(
            owner
                .view
                .connection_check_last_terminal
                .as_ref()
                .unwrap()
                .phase,
            ConnectionCheckPhase::Applied
        );
    }

    #[test]
    fn cs05_same_count_stale_authority_quote_is_refused_before_any_paid_worker() {
        let (_root, mut owner, accounts) = catalogue_owner();
        cs04_choose_synthetic_catalogue(&mut owner, "default", 0, &accounts[0], "new", "new-wire");
        let plan = owner
            .view
            .connection_plans
            .iter()
            .find(|p| p.profile == "default")
            .unwrap()
            .clone();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        owner.gateway.fixture_catalogue_upstream(format!(
            "http://{}/candidate",
            listener.local_addr().unwrap()
        ));
        let snapshot = owner.store.snapshot(&accounts[0]).unwrap();
        let mut preferences = snapshot.preferences;
        preferences.service = NetworkPolicy::Proxy("http://127.0.0.1:9".into());
        owner
            .store
            .update_preferences(&accounts[0], "Changed transport", preferences)
            .unwrap();
        owner
            .batch_gate
            .register(profile_batch::BatchIdentity {
                request: 13,
                profile: "default".into(),
                revision: plan.revision,
                plan: plan.id,
                epoch: 0,
            })
            .unwrap();
        owner.begin_profile_check(
            13,
            "default".into(),
            plan.revision,
            plan.id,
            plan.paid_slots,
            0,
        );
        assert!(owner.operation.is_none() && owner.batch.is_none());
        assert_eq!(
            owner
                .view
                .connection_check_last_terminal
                .as_ref()
                .unwrap()
                .error,
            Some(RuntimeError::PlanChanged)
        );
        assert!(
            matches!(listener.accept(),Err(error)if error.kind()==std::io::ErrorKind::WouldBlock)
        );
        // Isolate quote identity from availability/refusal redundancy: both
        // candidates are valid and cost one call, but the caller combines the
        // current raw revision with an old quote ID. That must not reserve work.
        let (_second_root, mut second, accounts) = catalogue_owner();
        cs04_choose_synthetic_catalogue(
            &mut second,
            "default",
            0,
            &accounts[0],
            "first",
            "first-wire",
        );
        let old = second
            .view
            .connection_plans
            .iter()
            .find(|plan| plan.profile == "default")
            .unwrap()
            .clone();
        cs04_choose_synthetic_catalogue(
            &mut second,
            "default",
            0,
            &accounts[0],
            "second",
            "second-wire",
        );
        let current = second
            .view
            .connection_plans
            .iter()
            .find(|plan| plan.profile == "default")
            .unwrap()
            .clone();
        assert_eq!((old.error, current.error), (None, None));
        assert_eq!(old.paid_slots, current.paid_slots);
        assert_eq!(current.paid_slots.len(), 1);
        assert_ne!(old.id, current.id);
        assert!(current.revision > old.revision);
        second
            .batch_gate
            .register(profile_batch::BatchIdentity {
                request: 14,
                profile: "default".into(),
                revision: current.revision,
                plan: old.id,
                epoch: 0,
            })
            .unwrap();
        second.begin_profile_check(
            14,
            "default".into(),
            current.revision,
            old.id,
            current.paid_slots,
            0,
        );
        assert!(
            second.operation.is_none() && second.batch.is_none(),
            "same-count valid candidate with stale quote must not reserve paid work"
        );
        assert_eq!(
            second
                .view
                .connection_check_last_terminal
                .as_ref()
                .unwrap()
                .error,
            Some(RuntimeError::PlanChanged)
        );
    }

    #[test]
    fn cs05_zero_call_missing_legacy_uses_plain_apply_without_creating_a_check() {
        let (root, mut owner, accounts) = catalogue_owner();
        owner.store.remove(&accounts[0]).unwrap();
        owner.refresh_view().unwrap();
        let plan = owner
            .view
            .connection_plans
            .iter()
            .find(|p| p.profile == "default")
            .unwrap()
            .clone();
        assert_eq!((plan.error, plan.paid_slots.len()), (None, 0));
        // Loading a legacy file upgrades only memory. Explicit plain Apply
        // may publish v3 provenance; it must not invent inference evidence.
        let before =
            Config::decode_applied(&std::fs::read(root.0.join("gateway.json")).unwrap()).unwrap();
        assert_eq!(
            owner.apply_connection_profile("default", plan.revision),
            Ok(())
        );
        assert!(
            owner.view.connection_check_active.is_none()
                && owner.view.connection_check_last_terminal.is_none()
        );
        assert!(owner.operation.is_none());
        assert_eq!(owner.view.listener, ListenerState::Stopped);
        let bytes = std::fs::read(root.0.join("gateway.json")).unwrap();
        let disk: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(disk["version"], 3, "Apply must publish the migrated file");
        let persisted = Config::decode_applied(&bytes).unwrap();
        assert_eq!(persisted, owner.config);
        assert_eq!(persisted.port, before.port);
        assert_eq!(persisted.identity, before.identity);
        assert_eq!(persisted.selected_account, before.selected_account);
        assert_eq!(persisted.model, before.model);
        assert_eq!(persisted.models, before.models);
        assert_eq!(persisted.profiles, before.profiles);
        assert_eq!(
            persisted.applied_evidence.len(),
            before.applied_evidence.len()
        );
        for evidence in &before.applied_evidence {
            assert!(persisted.applied_evidence.contains(evidence));
        }
        let row = persisted.resolve_allocation("default", None).unwrap();
        let previous = before.resolve_allocation("default", None).unwrap();
        assert_eq!(
            (row.account, row.alias, row.route),
            (previous.account, previous.alias, previous.route)
        );
        assert!(matches!(
            row.evidence.unwrap().origin,
            crate::provider_server::AppliedOrigin::LegacyApplied { .. }
        ));
        let form = owner
            .view
            .connection_profiles
            .iter()
            .find(|form| form.id == "default")
            .unwrap();
        assert_eq!(form.allocations[0].checked_unix_ms, None);
        assert_eq!(
            form.allocations[0].availability,
            ConnectionAvailability::MissingAccount
        );
    }

    #[test]
    fn cs05_new_alias_on_retained_wire_needs_real_new_identity_proof_then_applies() {
        let (root, mut owner, _) = catalogue_owner();
        let profile = "b".repeat(32);
        let before = std::fs::read(root.0.join("gateway.json")).unwrap();
        let form = owner
            .view
            .connection_profiles
            .iter()
            .find(|form| form.id == profile)
            .unwrap()
            .clone();
        let mut input = form.input;
        input.allocations[0].alias = "renamed".into();
        input.default_alias = "renamed".into();
        let revision = owner
            .drafts
            .edit_input(&profile, form.revision, input)
            .unwrap();
        owner.refresh_view().unwrap();
        assert_eq!(
            owner.apply_connection_profile(&profile, revision),
            Err(RuntimeError::VerificationRequired)
        );
        assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), before);
        cs04_real_check(&mut owner, &profile, 0, 14);
        assert_eq!(owner.apply_connection_profile(&profile, revision), Ok(()));
        let row = owner
            .config
            .resolve_allocation(&profile, Some("renamed"))
            .unwrap();
        assert_eq!(row.route.model_id, "legacy-wire");
        assert!(matches!(
            row.evidence.unwrap().origin,
            crate::provider_server::AppliedOrigin::Inference { .. }
        ));
        assert!(
            owner
                .config
                .resolve_allocation(&profile, Some("work"))
                .is_err()
        );
    }

    #[test]
    fn cs05_pending_gate_precedes_enqueue_and_queue_full_retracts_only_its_parent() {
        let (commands, incoming) = mpsc::sync_channel(1);
        let pending = Arc::new(Mutex::new(Pending::default()));
        let mut runtime = Runtime {
            commands,
            connection_inflight: Default::default(),
            last_connection_request: 0,
            pending: pending.clone(),
            view: View::default(),
            exit: Arc::new(AtomicBool::new(false)),
            stop_requested: Arc::new(AtomicU64::new(0)),
            worker: None,
        };
        runtime.act(Action::RefreshAccounts).unwrap();
        let action = |request| Action::VerifyAndApplyConnectionProfile {
            request,
            profile: "default".into(),
            expected_revision: 1,
            plan_id: 1,
            paid_slots: vec![0],
        };
        assert_eq!(runtime.act(action(1)), Err(RuntimeError::Busy));
        let gate = pending.lock().unwrap().batch_gate.clone();
        assert!(!gate.reserved());
        assert_eq!(runtime.last_connection_request, 0);
        incoming.recv().unwrap();
        assert_eq!(runtime.act(action(1)), Ok(()));
        assert!(gate.reserved());
        assert!(runtime.invalidate_connection_profile("default"));
        let id = profile_batch::BatchIdentity {
            request: 1,
            profile: "default".into(),
            revision: 1,
            plan: 1,
            epoch: 0,
        };
        assert!(matches!(
            gate.activate(&id, Instant::now() + Duration::from_secs(3), 0),
            Err(RuntimeError::DraftChanged)
        ));
        assert_eq!(
            runtime.act(Action::CancelConnectionVerification { request: 999 }),
            Err(RuntimeError::InvalidAction)
        );
    }

    #[test]
    fn cs05_runtime_queued_batch_stop_cancels_without_post_and_fresh_explicit_batch_still_works() {
        let (_root, mut owner, accounts) = catalogue_owner();
        cs04_choose_synthetic_catalogue(&mut owner, "default", 0, &accounts[0], "new", "new-wire");
        let paid = Arc::new(AtomicU64::new(0));
        let observed = paid.clone();
        let peer = Cs03Peer::new(move |body| {
            observed.fetch_add(1, Ordering::AcqRel);
            cs03_reply(body)
        });
        owner.gateway.fixture_catalogue_upstream(peer.url.clone());
        let (ready, wait_ready) = mpsc::channel();
        let (release, wait_release) = mpsc::channel();
        let wait = Arc::new(Mutex::new(Some(wait_release)));
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if let Some(wait) = wait.lock().unwrap().take() {
                ready.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        });
        let (commands, incoming) = mpsc::sync_channel(16);
        let pending = Arc::new(Mutex::new(Pending::default()));
        let exit = Arc::new(AtomicBool::new(false));
        let stop_requested = Arc::new(AtomicU64::new(0));
        let (outgoing, stopping, stop) = (pending.clone(), exit.clone(), stop_requested.clone());
        let worker = std::thread::spawn(move || {
            run_loaded_owner(
                owner,
                LoginMetadata {
                    version: "synthetic".into(),
                    locale: "en".into(),
                    timezone_offset_seconds: 0,
                },
                notify,
                outgoing,
                stopping,
                stop,
                incoming,
            )
        });
        let mut runtime = Runtime {
            commands,
            connection_inflight: Default::default(),
            last_connection_request: 0,
            pending,
            view: View::default(),
            exit,
            stop_requested,
            worker: Some(worker),
        };
        wait_ready.recv_timeout(Duration::from_secs(5)).unwrap();
        let _ = runtime.poll();
        let plan = runtime
            .view
            .connection_plans
            .iter()
            .find(|p| p.profile == "default")
            .unwrap()
            .clone();
        let action = |request| Action::VerifyAndApplyConnectionProfile {
            request,
            profile: "default".into(),
            expected_revision: plan.revision,
            plan_id: plan.id,
            paid_slots: plan.paid_slots.clone(),
        };
        runtime.act(action(1)).unwrap();
        runtime.act(Action::Stop).unwrap();
        release.send(()).unwrap();
        until(&mut runtime, |view| {
            view.connection_check_last_terminal
                .as_ref()
                .is_some_and(|check| check.request == 1)
        });
        assert_eq!(
            paid.load(Ordering::Acquire),
            0,
            "queued batch paid after Stop"
        );
        assert_eq!(
            runtime
                .view
                .connection_check_last_terminal
                .as_ref()
                .unwrap()
                .phase,
            ConnectionCheckPhase::Cancelled
        );
        runtime.act(action(2)).unwrap();
        until(&mut runtime, |view| {
            view.connection_check_last_terminal
                .as_ref()
                .is_some_and(|check| check.request == 2)
        });
        let result = runtime
            .view
            .connection_check_last_terminal
            .as_ref()
            .unwrap();
        assert_eq!(result.phase, ConnectionCheckPhase::Applied, "{result:?}");
        assert_eq!(paid.load(Ordering::Acquire), 1);
        assert_eq!(runtime.view.listener, ListenerState::Stopped);
    }

    #[test]
    fn cs05_multiple_health_failures_are_local_and_stale_retry_cannot_undo_applied() {
        let (_root, mut owner, accounts) = catalogue_owner();
        let profile = cs05_prepare_sparse(&mut owner, &accounts);
        let peer = Cs03Peer::sequence(vec![Box::new(cs03_reply), Box::new(cs03_reply)]);
        owner.gateway.fixture_catalogue_upstream(peer.url.clone());
        let plan = cs05_begin(&mut owner, &profile, 20);
        owner.drive_batch();
        cs03_wait_finished_without_poll(&owner);
        owner.store.fixture_fail_readiness_once();
        owner.poll_operation();
        owner.drive_batch();
        cs03_wait_finished_without_poll(&owner);
        owner.store.fixture_fail_readiness_once();
        owner.poll_operation();
        assert_eq!(owner.readiness_pending.as_ref().unwrap().snapshots.len(), 2);
        owner.drive_batch();
        let before = owner.view.connection_check_last_terminal.clone().unwrap();
        assert_eq!(before.phase, ConnectionCheckPhase::Applied);
        assert!(before.applied_revision.unwrap() > plan.revision);
        owner.store.remove(&accounts[0]).unwrap();
        assert!(owner.retry_readiness(20).is_err());
        let after = owner.view.connection_check_last_terminal.as_ref().unwrap();
        assert_eq!(
            (after.phase, after.applied_revision),
            (ConnectionCheckPhase::Applied, before.applied_revision)
        );
        assert_eq!(after.warning, Some(ConnectionCheckWarning::ReadinessStale));
        assert!(owner.operation.is_none());
        assert!(owner.readiness_pending.is_none());
    }

    #[test]
    fn cs04_real_runtime_rejects_old_queued_apply_after_stop_but_accepts_new_explicit_apply() {
        let (root, mut owner, _) = catalogue_owner();
        let profile = "b".repeat(32);
        let view = owner
            .view
            .connection_profiles
            .iter()
            .find(|p| p.id == profile)
            .unwrap()
            .clone();
        let mut input = view.input;
        input.label = "Explicit new publication".into();
        let revision = owner
            .drafts
            .edit_input(&profile, view.revision, input)
            .unwrap();
        let disk = std::fs::read(root.0.join("gateway.json")).unwrap();
        owner.refresh_connection_view();
        let (ready, wait_ready) = mpsc::channel();
        let (release, wait_release) = mpsc::channel();
        let gate = Arc::new(Mutex::new(Some(wait_release)));
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if let Some(wait) = gate.lock().unwrap().take() {
                ready.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        });
        let (commands, incoming) = mpsc::sync_channel(16);
        let pending = Arc::new(Mutex::new(Pending::default()));
        let exit = Arc::new(AtomicBool::new(false));
        let stop_requested = Arc::new(AtomicU64::new(0));
        let outgoing = pending.clone();
        let stopping = exit.clone();
        let stop = stop_requested.clone();
        let worker = std::thread::spawn(move || {
            run_loaded_owner(
                owner,
                LoginMetadata {
                    version: "synthetic-court".into(),
                    locale: "en".into(),
                    timezone_offset_seconds: 0,
                },
                notify,
                outgoing,
                stopping,
                stop,
                incoming,
            )
        });
        let mut runtime = Runtime {
            commands,
            connection_inflight: Default::default(),
            last_connection_request: 0,
            pending,
            view: View::default(),
            exit,
            stop_requested,
            worker: Some(worker),
        };
        wait_ready.recv_timeout(Duration::from_secs(5)).unwrap();
        runtime
            .act(Action::ApplyConnectionProfile {
                request: 1,
                profile: profile.clone(),
                expected_revision: revision,
            })
            .unwrap();
        runtime.act(Action::Stop).unwrap();
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            runtime.poll();
            if let Some(ack) = runtime
                .view()
                .connection_receipts
                .iter()
                .find(|ack| ack.request == 1)
            {
                assert_eq!(ack.change, ConnectionChange::Apply);
                assert_eq!(ack.result, Err(RuntimeError::Cancelled));
                break;
            }
            assert!(Instant::now() < deadline, "cancelled Apply receipt missing");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), disk);
        runtime
            .act(Action::ApplyConnectionProfile {
                request: 2,
                profile: profile.clone(),
                expected_revision: revision,
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            runtime.poll();
            if let Some(ack) = runtime
                .view()
                .connection_receipts
                .iter()
                .find(|ack| ack.request == 2)
            {
                assert_eq!(ack.result, Ok(()));
                break;
            }
            assert!(
                Instant::now() < deadline,
                "new explicit Apply receipt missing"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(runtime.view().listener, ListenerState::Stopped);
        drop(runtime);
        let saved = Owner::load(root.0.clone()).unwrap();
        assert_eq!(
            saved
                .config
                .profiles
                .iter()
                .find(|p| p.id == profile)
                .unwrap()
                .label,
            "Explicit new publication"
        );
    }

    fn cs04_choose_synthetic_catalogue(
        owner: &mut Owner,
        profile: &str,
        slot: u8,
        account: &str,
        id: &str,
        model: &str,
    ) {
        let snapshot = owner.store.snapshot(account).unwrap();
        let mut route = owner.config.models[0].clone();
        route.id = id.into();
        route.model_id = model.into();
        route.url = format!("https://opencode.ai/inference/{model}/v1/chat/completions");
        let request = owner.catalogues.begin(&snapshot).unwrap();
        let bytes = serde_json::to_vec(&route).unwrap().len();
        owner
            .catalogues
            .complete(
                &request,
                crate::provider_selection::ResolvedAuthority::from_snapshot(&snapshot).unwrap(),
                Some(&snapshot),
                vec![route],
                bytes,
            )
            .unwrap();
        let route = owner.catalogues.select(&snapshot, id).unwrap();
        let revision = owner.drafts.revision(profile).unwrap();
        owner
            .drafts
            .choose(profile, revision, slot, route, id)
            .unwrap();
        owner.refresh_view().unwrap();
    }

    fn cs04_real_check(owner: &mut Owner, profile: &str, slot: u8, request: u64) {
        let peer = Cs03Peer::new(cs03_reply);
        owner.gateway.fixture_catalogue_upstream(peer.url.clone());
        let revision = owner.drafts.revision(profile).unwrap();
        owner.begin_check(request, profile.into(), revision, slot);
        finish_catalogue(owner);
        assert_eq!(
            owner
                .view
                .connection_check_last_terminal
                .as_ref()
                .unwrap()
                .phase,
            ConnectionCheckPhase::Verified
        );
        drop(peer); // Apply cannot hide a second inference at this endpoint.
    }

    #[test]
    fn cs04_plain_apply_requires_current_raw_selected_proof_and_revision_without_paying() {
        let (root, mut owner, accounts) = catalogue_owner();
        let old_config = owner.config.clone();
        let disk = std::fs::read(root.0.join("gateway.json")).unwrap();
        cs04_choose_synthetic_catalogue(
            &mut owner,
            "default",
            0,
            &accounts[0],
            "new-catalogue",
            "new-wire",
        );
        let revision = owner.drafts.revision("default").unwrap();
        assert_eq!(
            owner.apply_connection_profile("default", revision),
            Err(RuntimeError::VerificationRequired)
        );
        assert_eq!(owner.config, old_config);
        assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), disk);
        assert!(owner.operation.is_none());
        cs04_real_check(&mut owner, "default", 0, 1);
        assert_eq!(
            owner.apply_connection_profile("default", revision - 1),
            Err(RuntimeError::DraftChanged)
        );
        // An incomplete raw edit must not Apply the last valid semantic edit.
        let view = owner
            .view
            .connection_profiles
            .iter()
            .find(|p| p.id == "default")
            .unwrap()
            .clone();
        let mut incomplete = view.input.clone();
        incomplete.allocations[0].model = "unselected raw text".into();
        let revision = owner
            .drafts
            .edit_input("default", revision, incomplete)
            .unwrap();
        assert_eq!(
            owner.apply_connection_profile("default", revision),
            Err(RuntimeError::InvalidAction)
        );
        assert_eq!(owner.config, old_config);
        assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), disk);
    }

    #[test]
    fn cs04_unpublished_failure_preserves_real_proof_for_zero_paid_retry_and_exact_apply_ack() {
        let (root, mut owner, accounts) = cs02_eight_profile_owner();
        cs04_choose_synthetic_catalogue(
            &mut owner,
            "default",
            0,
            &accounts[0],
            "new-catalogue",
            "new-wire",
        );
        cs04_real_check(&mut owner, "default", 0, 1);
        let revision = owner.drafts.revision("default").unwrap();
        let proof_time = owner
            .view
            .connection_check_last_terminal
            .as_ref()
            .unwrap()
            .checked_unix_ms;
        let before = owner.config.clone();
        let disk = std::fs::read(root.0.join("gateway.json")).unwrap();
        assert_eq!(
            owner.apply_connection_profile_with("default", revision, |_, _| Err(
                ServerError::Storage
            )),
            Err(RuntimeError::Storage)
        );
        assert_eq!(owner.config, before);
        assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), disk);
        assert_eq!(owner.drafts.revision("default"), Some(revision));
        let proof = owner
            .drafts
            .views(&owner.connection_accounts)
            .into_iter()
            .find(|p| p.id == "default")
            .unwrap();
        assert_eq!(proof.allocations[0].evidence, ConnectionEvidence::Verified);
        assert_eq!(proof.allocations[0].checked_unix_ms, proof_time);
        let mut server = owner.gateway.start(owner.config.port).unwrap();
        let custom_before = catalogue_binding(server.address(), "synthetic-custom");
        let transport = |session: &str| crate::harness_gateway::GatewayTransport {
            base: format!("http://{}/v1", server.address()),
            session: session.into(),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        let target_before = transport("cs04-original")
            .binding("synthetic-default", Duration::from_secs(3))
            .unwrap();
        assert!(
            owner
                .connection_action(Action::ApplyConnectionProfile {
                    request: 2,
                    profile: "default".into(),
                    expected_revision: revision
                })
                .is_none()
        );
        let ack = owner
            .view
            .connection_receipts
            .iter()
            .find(|r| r.request == 2)
            .unwrap();
        assert_eq!((ack.change, ack.result), (ConnectionChange::Apply, Ok(())));
        assert!(ack.revision.unwrap() > revision);
        let applied = owner.config.resolve_allocation("default", None).unwrap();
        assert_eq!(applied.route.model_id, "new-wire");
        assert!(matches!(
            &applied.evidence.unwrap().origin,
            crate::provider_server::AppliedOrigin::Inference { .. }
        ));
        assert_eq!(Owner::load(root.0.clone()).unwrap().config, owner.config);
        assert!(owner.operation.is_none() && owner.server.is_none());
        assert_eq!(owner.view.listener, ListenerState::Stopped);
        assert_eq!(
            catalogue_binding(server.address(), "synthetic-custom"),
            custom_before
        );
        assert!(
            transport("cs04-original")
                .binding("synthetic-default", Duration::from_secs(3))
                .is_err()
        );
        let target_after = transport("cs04-new")
            .binding("synthetic-default", Duration::from_secs(3))
            .unwrap();
        assert_eq!(target_after.model, "new-wire");
        assert_ne!(target_after.generation, target_before.generation);
        server.stop();
    }

    #[test]
    fn cs04_ready_storage_warning_proof_applies_without_rotating_noop_sessions_or_keys() {
        let (root, mut owner, _) = cs02_eight_profile_owner();
        let peer = Cs03Peer::new(cs03_reply);
        owner.gateway.fixture_catalogue_upstream(peer.url.clone());
        let revision = owner.drafts.revision("default").unwrap();
        owner.begin_check(1, "default".into(), revision, 0);
        cs03_wait_finished_without_poll(&owner);
        owner.store.fixture_fail_readiness_once();
        owner.poll_operation();
        assert_eq!(
            owner
                .view
                .connection_check_last_terminal
                .as_ref()
                .unwrap()
                .warning,
            Some(ConnectionCheckWarning::ReadinessStorage)
        );
        drop(peer);
        let custom = "b".repeat(32);
        let key_path = root.0.join("client-key");
        let custom_key = root.0.join(format!("client-key-{custom}"));
        filesystem::write_private_atomic(&key_path, b"synthetic-default").unwrap();
        filesystem::write_private_atomic(&custom_key, b"synthetic-custom").unwrap();
        owner.key = Some(Secret::new("synthetic-default").unwrap());
        let original_model = owner.config.model.clone();
        let mut server = owner.gateway.start(owner.config.port).unwrap();
        let before_default = catalogue_binding(server.address(), "synthetic-default");
        let before_custom = catalogue_binding(server.address(), "synthetic-custom");
        owner.apply_connection_profile("default", revision).unwrap();
        assert_eq!(owner.config.model, original_model);
        assert_eq!(
            catalogue_binding(server.address(), "synthetic-default"),
            before_default
        );
        assert_eq!(
            catalogue_binding(server.address(), "synthetic-custom"),
            before_custom
        );
        let revision = owner.drafts.revision("default").unwrap();
        owner.apply_connection_profile("default", revision).unwrap();
        assert_eq!(
            catalogue_binding(server.address(), "synthetic-default"),
            before_default
        );
        assert_eq!(std::fs::read(key_path).unwrap(), b"synthetic-default");
        assert_eq!(std::fs::read(custom_key).unwrap(), b"synthetic-custom");
        assert_eq!(
            owner
                .view
                .connection_check_last_terminal
                .as_ref()
                .unwrap()
                .warning,
            Some(ConnectionCheckWarning::ReadinessStorage)
        );
        server.stop();
    }

    #[test]
    fn cs04_two_account_final_gate_refuses_changed_second_authority_before_any_publication() {
        let (root, mut owner, accounts) = catalogue_owner();
        let profile = "b".repeat(32);
        let first = owner
            .view
            .connection_profiles
            .iter()
            .find(|p| p.id == profile)
            .unwrap()
            .clone();
        let mut input = first.input;
        input.allocations.push(ConnectionAllocationInput {
            slot: 7,
            alias: "second".into(),
            account: Some(accounts[0].clone()),
            model: "next".into(),
        });
        owner
            .drafts
            .edit_input(&profile, first.revision, input)
            .unwrap();
        cs04_choose_synthetic_catalogue(
            &mut owner,
            &profile,
            7,
            &accounts[0],
            "next",
            "second-wire",
        );
        cs04_real_check(&mut owner, &profile, 0, 1);
        cs04_real_check(&mut owner, &profile, 7, 2);
        let revision = owner.drafts.revision(&profile).unwrap();
        let prepared = owner
            .drafts
            .prepare_apply(
                &profile,
                revision,
                &owner.config,
                &owner.connection_accounts,
            )
            .unwrap();
        assert_eq!(prepared.authorities.len(), 2);
        let second = prepared.authorities.last().unwrap();
        let mut preferences = second.preferences.clone();
        preferences.service_generation = "f".repeat(32);
        owner
            .store
            .update_preferences(&second.summary.id, "Still visible", preferences)
            .unwrap();
        let disk = std::fs::read(root.0.join("gateway.json")).unwrap();
        let encoded = prepared.config.encode_applied().unwrap();
        let wrote = std::sync::atomic::AtomicBool::new(false);
        assert_eq!(
            owner
                .gateway
                .update_with_authorities(prepared.config, &prepared.authorities, || {
                    wrote.store(true, Ordering::Release);
                    minicon_platform::filesystem_publish::write_private_atomic(
                        &root.0.join("gateway.json"),
                        &encoded,
                    )
                    .unwrap();
                    Ok(ConfigPublication::Durable)
                }),
            Err(ServerError::BindingChanged)
        );
        assert!(!wrote.load(Ordering::Acquire));
        assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), disk);
        assert_eq!(owner.drafts.revision(&profile), Some(revision));
        assert!(
            owner
                .drafts
                .views(&owner.connection_accounts)
                .iter()
                .find(|p| p.id == profile)
                .unwrap()
                .allocations
                .iter()
                .all(|row| row.evidence == ConnectionEvidence::Verified)
        );
    }

    #[test]
    fn cs04_published_warning_installs_complete_profile_and_preserves_other_seven_drafts() {
        let (root, mut owner, _) = cs02_eight_profile_owner();
        let profile = "b".repeat(32);
        for view in owner.view.connection_profiles.clone() {
            if view.id == profile {
                continue;
            }
            let mut input = view.input;
            input.allocations[0].model = format!("unsaved-{}", view.id);
            owner
                .drafts
                .edit_input(&view.id, view.revision, input)
                .unwrap();
        }
        let view = owner
            .drafts
            .views(&owner.connection_accounts)
            .into_iter()
            .find(|p| p.id == profile)
            .unwrap();
        let mut input = view.input;
        input.label = "Published target".into();
        let revision = owner
            .drafts
            .edit_input(&profile, view.revision, input)
            .unwrap();
        let others: Vec<_> = owner
            .drafts
            .views(&owner.connection_accounts)
            .into_iter()
            .filter(|p| p.id != profile)
            .collect();
        let registry = root.0.join("accounts/accounts.json");
        let registry_before = std::fs::read(&registry).unwrap();
        assert_eq!(
            owner.apply_connection_profile_with(&profile, revision, |path, bytes| {
                minicon_platform::filesystem_publish::write_private_atomic(path, bytes).unwrap();
                filesystem::write_private_atomic(&registry, b"synthetic-unavailable-metadata")
                    .unwrap();
                Ok(ConfigPublication::DurabilityUnconfirmed)
            }),
            Ok(())
        );
        assert!(owner.view.config_durability_unconfirmed);
        assert_eq!(
            owner
                .config
                .profiles
                .iter()
                .find(|p| p.id == profile)
                .unwrap()
                .label,
            "Published target"
        );
        assert_eq!(
            owner
                .view
                .connection_profiles
                .iter()
                .filter(|p| p.id != profile)
                .cloned()
                .collect::<Vec<_>>(),
            others
        );
        assert!(
            !owner
                .view
                .connection_profiles
                .iter()
                .find(|p| p.id == profile)
                .unwrap()
                .dirty
        );
        filesystem::write_private_atomic(&registry, &registry_before).unwrap();
        assert_eq!(Owner::load(root.0.clone()).unwrap().config, owner.config);
        assert_eq!(owner.view.listener, ListenerState::Stopped);
        assert!(owner.server.is_none());
    }

    #[test]
    fn cs04_unchanged_missing_account_legacy_reference_stays_legacy_and_does_not_create_proof() {
        let (_root, mut owner, accounts) = catalogue_owner();
        owner.store.remove(&accounts[0]).unwrap();
        let before = owner
            .config
            .resolve_allocation("default", None)
            .unwrap()
            .route
            .clone();
        let revision = owner.drafts.revision("default").unwrap();
        assert_eq!(owner.apply_connection_profile("default", revision), Ok(()));
        let applied = owner.config.resolve_allocation("default", None).unwrap();
        assert_eq!(applied.route, &before);
        assert!(matches!(
            &applied.evidence.unwrap().origin,
            crate::provider_server::AppliedOrigin::LegacyApplied { .. }
        ));
        let view = owner
            .view
            .connection_profiles
            .iter()
            .find(|p| p.id == "default")
            .unwrap();
        assert_eq!(
            view.allocations[0].evidence,
            ConnectionEvidence::LegacyApplied
        );
        assert_eq!(
            view.allocations[0].availability,
            ConnectionAvailability::MissingAccount
        );
        assert_eq!(view.allocations[0].checked_unix_ms, None);
        assert!(owner.operation.is_none());
    }
    #[test]
    fn cs03_runtime_stop_rejects_a_real_queued_old_verify_and_allows_new_explicit_verify() {
        let (_root, owner, _) = catalogue_owner();
        let paid_requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = paid_requests.clone();
        let peer = Cs03Peer::new(move |body| {
            observed.fetch_add(1, Ordering::AcqRel);
            cs03_reply(body)
        });
        owner.gateway.fixture_catalogue_upstream(peer.url.clone());
        let (ready, wait_ready) = mpsc::channel();
        let (release, wait_release) = mpsc::channel();
        let gate = Arc::new(Mutex::new(Some(wait_release)));
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if let Some(wait) = gate.lock().unwrap().take() {
                ready.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        });
        let (commands, incoming) = mpsc::sync_channel(16);
        let pending = Arc::new(Mutex::new(Pending::default()));
        let exit = Arc::new(AtomicBool::new(false));
        let stop_requested = Arc::new(AtomicU64::new(0));
        let outgoing = pending.clone();
        let stopping = exit.clone();
        let stop = stop_requested.clone();
        let worker = std::thread::spawn(move || {
            run_loaded_owner(
                owner,
                LoginMetadata {
                    version: "synthetic-court".into(),
                    locale: "en".into(),
                    timezone_offset_seconds: 0,
                },
                notify,
                outgoing,
                stopping,
                stop,
                incoming,
            )
        });
        let mut runtime = Runtime {
            commands,
            connection_inflight: Default::default(),
            last_connection_request: 0,
            pending,
            view: View::default(),
            exit,
            stop_requested,
            worker: Some(worker),
        };
        wait_ready.recv_timeout(Duration::from_secs(5)).unwrap();
        let _ = runtime.poll();
        let revision = runtime
            .view()
            .connection_profiles
            .iter()
            .find(|profile| profile.id == "default")
            .unwrap()
            .revision;
        runtime
            .act(Action::VerifyConnectionAllocation {
                request: 1,
                profile: "default".into(),
                expected_revision: revision,
                slot: 0,
            })
            .unwrap();
        runtime.act(Action::Stop).unwrap();
        release.send(()).unwrap();
        until(&mut runtime, |view| {
            view.connection_check_last_terminal
                .as_ref()
                .is_some_and(|result| result.request == 1)
        });
        // The peer serves an unexpected old POST rather than manufacturing a
        // timeout. Its real request counter is the pre-Stop intent oracle.
        assert_eq!(
            paid_requests.load(Ordering::Acquire),
            0,
            "old queued verification emitted paid inference after Stop"
        );
        let terminal = runtime
            .view()
            .connection_check_last_terminal
            .as_ref()
            .unwrap();
        assert_eq!(terminal.phase, ConnectionCheckPhase::Cancelled);
        assert_eq!(terminal.error, Some(RuntimeError::Cancelled));
        assert_eq!(
            (terminal.profile.as_str(), terminal.revision, terminal.slot),
            ("default", revision, 0)
        );
        assert!(runtime.view().connection_check_active.is_none());
        assert_eq!(runtime.view().listener, ListenerState::Stopped);
        runtime
            .act(Action::VerifyConnectionAllocation {
                request: 2,
                profile: "default".into(),
                expected_revision: revision,
                slot: 0,
            })
            .unwrap();
        until(&mut runtime, |view| {
            view.connection_check_last_terminal
                .as_ref()
                .is_some_and(|result| result.request == 2)
        });
        let terminal = runtime
            .view()
            .connection_check_last_terminal
            .as_ref()
            .unwrap();
        assert_eq!(
            terminal.phase,
            ConnectionCheckPhase::Verified,
            "{terminal:?}"
        );
        assert_eq!(paid_requests.load(Ordering::Acquire), 1);
        assert_eq!(runtime.view().listener, ListenerState::Stopped);
        assert_eq!(runtime.view().actual_port, None);
    }

    type Cs03Reply = Box<dyn FnOnce(serde_json::Value) -> serde_json::Value + Send>;
    struct Cs03Peer {
        url: String,
        stopping: Arc<AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }
    impl Cs03Peer {
        fn new(
            reply: impl FnOnce(serde_json::Value) -> serde_json::Value + Send + 'static,
        ) -> Self {
            Self::sequence(vec![Box::new(reply)])
        }
        fn sequence(replies: Vec<Cs03Reply>) -> Self {
            Self::bounded(replies, true)
        }
        fn observing(replies: Vec<Cs03Reply>) -> Self {
            Self::bounded(replies, false)
        }
        fn bounded(replies: Vec<Cs03Reply>, require_all: bool) -> Self {
            assert!(!replies.is_empty() && replies.len() <= 32);
            use std::io::{Read, Write};
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/candidate", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let stopping = Arc::new(AtomicBool::new(false));
            let stop = stopping.clone();
            let worker = std::thread::spawn(move || {
                for reply in replies {
                    let deadline = Instant::now() + Duration::from_secs(5);
                    let mut stream = loop {
                        match listener.accept() {
                            Ok((stream, _)) => break stream,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                if !require_all && stop.load(Ordering::Acquire) {
                                    return;
                                }
                                assert!(
                                    Instant::now() < deadline,
                                    "candidate fixture request did not arrive"
                                );
                                std::thread::sleep(Duration::from_millis(2));
                            }
                            Err(error) => panic!("candidate fixture accept: {error}"),
                        }
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut head = Vec::new();
                    while !head.ends_with(b"\r\n\r\n") {
                        assert!(head.len() < 16384);
                        let mut byte = [0];
                        stream.read_exact(&mut byte).unwrap();
                        head.push(byte[0]);
                    }
                    let head = String::from_utf8(head).unwrap();
                    assert!(head.starts_with("POST /candidate "));
                    let length: usize = head
                        .lines()
                        .find_map(|line| {
                            line.split_once(':')
                                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                .map(|(_, value)| value.trim().parse().unwrap())
                        })
                        .unwrap();
                    assert!(length <= 16384);
                    let mut body = vec![0; length];
                    stream.read_exact(&mut body).unwrap();
                    let body = reply(serde_json::from_slice(&body).unwrap()).to_string();
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    // Cancellation is allowed to close this actual client; the
                    // request/body assertions above must still run and be joined.
                    let _ = stream
                        .write_all(header.as_bytes())
                        .and_then(|()| stream.write_all(body.as_bytes()));
                }
            });
            Self {
                url,
                stopping,
                worker: Some(worker),
            }
        }
    }
    impl Drop for Cs03Peer {
        fn drop(&mut self) {
            self.stopping.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let result = worker.join();
                if !std::thread::panicking() {
                    assert!(result.is_ok(), "candidate fixture worker failed");
                }
            }
        }
    }
    fn cs03_reply(body: serde_json::Value) -> serde_json::Value {
        assert_eq!(body["max_tokens"], 8);
        assert_eq!(body["stream"], false);
        serde_json::json!({"id":"synthetic-probe","model":body["model"],"object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}]})
    }
    fn cs03_wait_finished_without_poll(owner: &Owner) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !owner.operation.as_ref().unwrap().worker.is_finished() {
            assert!(
                Instant::now() < deadline,
                "candidate fixture HTTP worker exceeded deadline"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn cs03_owner_verifies_all_eight_profiles_without_apply_or_listener_effects() {
        let (root, mut owner, _) = cs02_eight_profile_owner();
        let config = owner.config.clone();
        let disk = std::fs::read(root.0.join("gateway.json")).unwrap();
        let profiles = owner.view.connection_profiles.clone();
        assert_eq!(profiles.len(), 8);
        for (index, profile) in profiles.into_iter().enumerate() {
            let peer = Cs03Peer::new(cs03_reply);
            owner.gateway.fixture_catalogue_upstream(peer.url.clone());
            owner.begin_check(index as u64 + 1, profile.id.clone(), profile.revision, 0);
            assert_eq!(
                owner.view.connection_check_active.as_ref().unwrap().profile,
                profile.id
            );
            finish_catalogue(&mut owner);
            let terminal = owner.view.connection_check_last_terminal.as_ref().unwrap();
            assert_eq!(
                terminal.phase,
                ConnectionCheckPhase::Verified,
                "{terminal:?}"
            );
            assert!(terminal.checked_unix_ms.is_some_and(|time| time > 0));
            assert!(
                owner
                    .view
                    .connection_profiles
                    .iter()
                    .find(|row| row.id == profile.id)
                    .unwrap()
                    .allocations
                    .iter()
                    .all(|row| row.evidence == ConnectionEvidence::Verified)
            );
            assert_eq!(owner.config, config);
            assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), disk);
            assert!(owner.server.is_none());
            assert_eq!(owner.view.listener, ListenerState::Stopped);
        }
    }

    #[test]
    fn cs03_owner_final_acceptance_rejects_revision_aba_after_completed_real_response() {
        let (_root, mut owner, _) = catalogue_owner();
        let peer = Cs03Peer::new(cs03_reply);
        owner.gateway.fixture_catalogue_upstream(peer.url.clone());
        let profile = owner
            .view
            .connection_profiles
            .iter()
            .find(|row| row.id != "default")
            .unwrap()
            .clone();
        owner.begin_check(1, profile.id.clone(), profile.revision, 0);
        cs03_wait_finished_without_poll(&owner);
        // Independently exercise the final Owner acceptance guard: actual raw
        // book edits are complete before result consumption; this does not use
        // the action-level proactive cancellation as a substitute for the CAS.
        let mut input = profile.input.clone();
        input.label = "temporary label".into();
        let revision = owner
            .drafts
            .edit_input(&profile.id, profile.revision, input)
            .unwrap();
        owner
            .drafts
            .edit_input(&profile.id, revision, profile.input)
            .unwrap();
        owner.poll_operation();
        assert_eq!(
            owner
                .view
                .connection_check_last_terminal
                .as_ref()
                .unwrap()
                .phase,
            ConnectionCheckPhase::Failed
        );
        assert!(
            owner
                .view
                .connection_profiles
                .iter()
                .find(|row| row.id == profile.id)
                .unwrap()
                .allocations
                .iter()
                .all(|row| row.checked_unix_ms.is_none())
        );
    }

    #[test]
    fn cs03_rejected_and_stale_cancel_requests_never_overwrite_or_cancel_active_check() {
        let (_root, mut owner, _) = catalogue_owner();
        let peer = Cs03Peer::new(cs03_reply);
        owner.gateway.fixture_catalogue_upstream(peer.url.clone());
        let revision = owner.drafts.revision("default").unwrap();
        owner.begin_check(2, "default".into(), revision, 0);
        owner.begin_check(3, "default".into(), revision, 0);
        assert_eq!(
            owner.view.connection_check_active.as_ref().unwrap().request,
            2
        );
        assert_eq!(
            owner
                .view
                .connection_check_last_terminal
                .as_ref()
                .unwrap()
                .error,
            Some(RuntimeError::Busy)
        );
        assert_eq!(
            owner.cancel_check_request(1),
            Err(RuntimeError::InvalidAction)
        );
        assert_eq!(
            owner.view.connection_check_active.as_ref().unwrap().request,
            2
        );
        cs03_wait_finished_without_poll(&owner);
        owner.cancel_check_request(2).unwrap();
        assert_eq!(
            owner
                .view
                .connection_check_last_terminal
                .as_ref()
                .unwrap()
                .phase,
            ConnectionCheckPhase::Cancelled
        );
        assert!(
            owner
                .view
                .connection_profiles
                .iter()
                .find(|row| row.id == "default")
                .unwrap()
                .allocations
                .iter()
                .all(|row| row.checked_unix_ms.is_none())
        );
    }

    #[test]
    fn cs03_readiness_storage_warning_keeps_real_proof_and_retry_never_repeats_inference() {
        let (_root, mut owner, _) = catalogue_owner();
        let peer = Cs03Peer::new(cs03_reply);
        owner.gateway.fixture_catalogue_upstream(peer.url.clone());
        let revision = owner.drafts.revision("default").unwrap();
        owner.begin_check(1, "default".into(), revision, 0);
        cs03_wait_finished_without_poll(&owner);
        owner.store.fixture_fail_readiness_once();
        owner.poll_operation();
        let terminal = owner.view.connection_check_last_terminal.clone().unwrap();
        assert_eq!(terminal.phase, ConnectionCheckPhase::Verified);
        assert_eq!(
            terminal.warning,
            Some(ConnectionCheckWarning::ReadinessStorage)
        );
        assert!(
            owner
                .view
                .connection_profiles
                .iter()
                .find(|row| row.id == "default")
                .unwrap()
                .allocations
                .iter()
                .all(|row| row.evidence == ConnectionEvidence::Verified)
        );
        drop(peer); // No fixture listener remains: retry cannot hide another paid request.
        owner.retry_readiness(1).unwrap();
        let restored = owner.view.connection_check_last_terminal.as_ref().unwrap();
        assert_eq!(restored.warning, None);
        assert_eq!(restored.checked_unix_ms, terminal.checked_unix_ms);
        assert!(owner.operation.is_none());
        assert_eq!(owner.retry_readiness(1), Err(RuntimeError::InvalidAction));
    }

    use super::*;
    use std::{net::TcpListener, time::Instant};

    fn cs02_eight_profile_owner() -> (Root, Owner, [String; 2]) {
        let (root, owner, accounts) = catalogue_owner();
        let mut config = owner.config.clone();
        config.version = 2;
        config.port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let template = config.profiles[0].clone();
        for index in 1..7 {
            let mut profile = template.clone();
            profile.id = format!("{index:032x}");
            profile.label = format!("Client {index}");
            config.profiles.push(profile);
        }
        let owner = load_legacy_fixture(owner, config);
        owner
            .gateway
            .set_key(Some(Secret::new("synthetic-default").unwrap()))
            .unwrap();
        owner
            .gateway
            .set_client_key(
                &"b".repeat(32),
                Some(Secret::new("synthetic-custom").unwrap()),
            )
            .unwrap();
        (root, owner, accounts)
    }

    #[test]
    fn cs02_all_profile_draft_actions_preserve_private_bytes_and_actual_named_bindings() {
        let (root, mut owner, _) = cs02_eight_profile_owner();
        let original = filesystem::read_private_bounded(
            &root.0.join("gateway.json"),
            crate::provider_server::CONFIG_BYTES,
        )
        .unwrap();
        let config = owner.config.clone();
        let mut server = owner.gateway.start(owner.config.port).unwrap();
        let address = server.address();
        let bindings = [
            catalogue_binding(address, "synthetic-default"),
            catalogue_binding(address, "synthetic-custom"),
        ];
        let mut login = LoginCoordinator::new(
            owner.store.clone(),
            LoginMetadata {
                version: "synthetic-court".into(),
                locale: "en".into(),
                timezone_offset_seconds: 0,
            },
        );
        let profiles = owner.view.connection_profiles.clone();
        assert_eq!(profiles.len(), 8);
        owner.view.status = Some("unrelated service error".into());
        for (index, profile) in profiles.iter().enumerate() {
            let mut input = profile.input.clone();
            if profile.id == "default" {
                input.allocations[0].model = "unapplied default text".into();
            } else {
                input.label = format!("Edited client {index}");
            }
            owner
                .action(
                    Action::EditConnectionProfile {
                        request: index as u64 + 1,
                        profile: profile.id.clone(),
                        expected_revision: profile.revision,
                        input: Box::new(input.clone()),
                    },
                    &mut login,
                )
                .unwrap();
            let row = owner
                .view
                .connection_profiles
                .iter()
                .find(|view| view.id == profile.id)
                .unwrap();
            assert_eq!(row.input, input);
            assert!(row.dirty);
            let receipt = owner
                .view
                .connection_receipts
                .iter()
                .find(|receipt| receipt.profile == profile.id)
                .unwrap();
            assert_eq!(receipt.result, Ok(()));
            assert_eq!(receipt.revision, Some(row.revision));
            owner
                .action(
                    Action::FocusConnectionProfile(profile.id.clone()),
                    &mut login,
                )
                .unwrap();
            assert_eq!(owner.view.connection_profile, profile.id);
            assert_eq!(
                owner.view.status.as_deref(),
                Some("unrelated service error")
            );
            assert_eq!(owner.config, config);
            assert_eq!(
                std::fs::read(root.0.join("gateway.json")).unwrap(),
                original
            );
            assert!(owner.operation.is_none());
            assert!(owner.server.is_none());
            assert_eq!(
                [
                    catalogue_binding(address, "synthetic-default"),
                    catalogue_binding(address, "synthetic-custom")
                ],
                bindings
            );
        }
        assert_eq!(owner.view.connection_receipts.len(), 8);
        let edited = owner.view.connection_profiles.clone();
        let target = edited
            .iter()
            .find(|view| view.id != "default")
            .unwrap()
            .clone();
        owner
            .action(
                Action::RevertConnectionProfile {
                    request: 50,
                    profile: target.id.clone(),
                    expected_revision: target.revision,
                },
                &mut login,
            )
            .unwrap();
        for view in &owner.view.connection_profiles {
            if view.id == target.id {
                assert!(!view.dirty);
            } else {
                assert_eq!(Some(view), edited.iter().find(|old| old.id == view.id));
            }
        }
        assert_eq!(
            std::fs::read(root.0.join("gateway.json")).unwrap(),
            original
        );
        assert_eq!(
            [
                catalogue_binding(address, "synthetic-default"),
                catalogue_binding(address, "synthetic-custom")
            ],
            bindings
        );
        server.stop();
    }

    #[test]
    fn cs02_owner_receipts_cas_revert_and_create_failures_do_not_lose_other_form_results() {
        let (root, mut owner, _) = cs02_eight_profile_owner();
        let mut login = LoginCoordinator::new(
            owner.store.clone(),
            LoginMetadata {
                version: "synthetic-court".into(),
                locale: "en".into(),
                timezone_offset_seconds: 0,
            },
        );
        let profiles = owner.view.connection_profiles.clone();
        for (index, profile) in profiles.iter().enumerate() {
            owner
                .action(
                    Action::EditConnectionProfile {
                        request: index as u64 + 1,
                        profile: profile.id.clone(),
                        expected_revision: profile.revision,
                        input: Box::new(profile.input.clone()),
                    },
                    &mut login,
                )
                .unwrap();
        }
        owner
            .action(Action::CreateConnectionProfile { request: 9 }, &mut login)
            .unwrap();
        assert_eq!(owner.view.connection_receipts.len(), 9);
        assert!(
            owner
                .view
                .connection_receipts
                .iter()
                .any(|receipt| receipt.request == 9
                    && receipt.profile.is_empty()
                    && receipt.result == Err(RuntimeError::Busy))
        );
        for (index, profile) in profiles.iter().enumerate() {
            assert!(
                owner
                    .view
                    .connection_receipts
                    .iter()
                    .any(|receipt| receipt.request == index as u64 + 1
                        && receipt.profile == profile.id
                        && receipt.result == Ok(()))
            );
        }
        let target = owner.view.connection_profiles[0].clone();
        let mut later = target.input.clone();
        later.allocations[0].model = "later text".into();
        owner
            .action(
                Action::EditConnectionProfile {
                    request: 10,
                    profile: target.id.clone(),
                    expected_revision: target.revision - 1,
                    input: Box::new(later),
                },
                &mut login,
            )
            .unwrap();
        let failed = owner
            .view
            .connection_receipts
            .iter()
            .find(|receipt| receipt.request == 10)
            .unwrap();
        assert_eq!(failed.result, Err(RuntimeError::DraftChanged));
        assert_eq!(failed.revision, Some(target.revision));
        assert_eq!(
            owner
                .view
                .connection_profiles
                .iter()
                .find(|view| view.id == target.id)
                .unwrap(),
            &target
        );
        let disk = std::fs::read(root.0.join("gateway.json")).unwrap();
        let restarted = Owner::load(root.0.clone()).unwrap();
        assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), disk);
        assert!(
            restarted
                .view
                .connection_profiles
                .iter()
                .all(|view| !view.dirty)
        );
    }

    #[test]
    fn cs02_corrupt_credentials_remain_visible_after_real_owner_reload_and_revert() {
        let (root, mut owner, accounts) = catalogue_owner();
        let applied = owner.config.clone();
        let bytes = std::fs::read(root.0.join("gateway.json")).unwrap();
        filesystem::write_private_atomic(
            &root
                .0
                .join(format!("accounts/credentials/{}.json", accounts[0])),
            b"corrupt synthetic credential",
        )
        .unwrap();
        owner.refresh_view().unwrap();
        let default = owner
            .view
            .connection_profiles
            .iter()
            .find(|view| view.id == "default")
            .unwrap();
        assert_eq!(default.allocations.len(), 1);
        assert_eq!(
            default.allocations[0].availability,
            ConnectionAvailability::CredentialUnavailable
        );
        assert_eq!(
            default.allocations[0].evidence,
            ConnectionEvidence::LegacyApplied
        );
        let reloaded = Owner::load(root.0.clone()).unwrap();
        assert_eq!(reloaded.config, applied);
        assert_eq!(
            reloaded
                .view
                .connection_profiles
                .iter()
                .find(|view| view.id == "default")
                .unwrap()
                .allocations,
            default.allocations
        );
        owner
            .drafts
            .revert("default", default.revision, &owner.config)
            .unwrap();
        owner.project_connection_drafts();
        assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), bytes);
        assert_eq!(
            owner
                .view
                .connection_profiles
                .iter()
                .find(|view| view.id == "default")
                .unwrap()
                .allocations[0]
                .availability,
            ConnectionAvailability::CredentialUnavailable
        );
    }

    #[test]
    fn cs02_initial_unreadable_account_registry_keeps_all_applied_references_observable() {
        let (root, owner, _) = cs02_eight_profile_owner();
        let expected = owner
            .view
            .connection_profiles
            .iter()
            .map(|profile| (profile.id.clone(), profile.input.clone()))
            .collect::<Vec<_>>();
        let applied = std::fs::read(root.0.join("gateway.json")).unwrap();
        drop(owner);
        let registry_path = root.0.join("accounts/accounts.json");
        let registry = std::fs::read(&registry_path).unwrap();
        let key = b"synthetic-persistent-key";
        filesystem::write_private_atomic(&root.0.join("client-key"), key).unwrap();
        filesystem::write_private_atomic(&registry_path, b"unavailable registry").unwrap();
        let mut owner = Owner::load(root.0.clone()).unwrap();
        assert_eq!(
            owner.view.load_state,
            LoadState::Unavailable(RuntimeError::Storage)
        );
        assert!(!owner.view.busy);
        assert_eq!(
            owner
                .view
                .connection_profiles
                .iter()
                .map(|profile| (profile.id.clone(), profile.input.clone()))
                .collect::<Vec<_>>(),
            expected
        );
        assert!(owner.view.connection_profiles.iter().all(|profile| {
            profile
                .allocations
                .iter()
                .all(|row| row.availability == ConnectionAvailability::CredentialUnavailable)
        }));
        assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), applied);
        assert_eq!(std::fs::read(root.0.join("client-key")).unwrap(), key);
        assert!(owner.operation.is_none() && owner.server.is_none());
        assert!(
            owner
                .view
                .connection_profiles
                .iter()
                .flat_map(|profile| &profile.allocations)
                .all(|row| row.evidence == ConnectionEvidence::LegacyApplied
                    && row.checked_unix_ms.is_none())
        );
        let target = owner
            .view
            .connection_profiles
            .iter()
            .find(|profile| profile.id != "default")
            .unwrap()
            .clone();
        let mut input = target.input.clone();
        input.label = "Dirty while account storage is unavailable".into();
        let mut login = LoginCoordinator::new(
            owner.store.clone(),
            LoginMetadata {
                version: "synthetic-court".into(),
                locale: "en".into(),
                timezone_offset_seconds: 0,
            },
        );
        owner
            .action(
                Action::EditConnectionProfile {
                    request: 1,
                    profile: target.id.clone(),
                    expected_revision: target.revision,
                    input: Box::new(input.clone()),
                },
                &mut login,
            )
            .unwrap();
        filesystem::write_private_atomic(&registry_path, &registry).unwrap();
        owner.action(Action::RefreshAccounts, &mut login).unwrap();
        assert_eq!(owner.view.load_state, LoadState::Loaded);
        assert_eq!(
            owner
                .view
                .connection_profiles
                .iter()
                .find(|profile| profile.id == target.id)
                .unwrap()
                .input,
            input
        );
        assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), applied);
        assert_eq!(std::fs::read(root.0.join("client-key")).unwrap(), key);
        assert!(owner.operation.is_none() && owner.server.is_none());
        assert!(
            owner
                .view
                .connection_profiles
                .iter()
                .flat_map(|profile| &profile.allocations)
                .all(|row| row.evidence == ConnectionEvidence::LegacyApplied
                    && row.checked_unix_ms.is_none())
        );
    }

    #[test]
    fn cs02_receipt_delivery_survives_more_than_nine_create_discard_cycles_without_eviction() {
        let root = Root::new();
        let mut runtime = root.runtime();
        until(&mut runtime, |view| view.load_state == LoadState::Loaded);
        for index in 0..16u64 {
            let create = 2 * index + 1;
            runtime
                .act(Action::CreateConnectionProfile { request: create })
                .unwrap();
            until(&mut runtime, |view| {
                view.connection_receipts
                    .iter()
                    .any(|receipt| receipt.request == create)
            });
            let receipt = runtime
                .view()
                .connection_receipts
                .iter()
                .find(|receipt| receipt.request == create)
                .unwrap()
                .clone();
            assert_eq!(receipt.result, Ok(()));
            assert!(crate::provider_server::opaque_id(&receipt.profile));
            let discard = create + 1;
            runtime
                .act(Action::DiscardUnappliedProfile {
                    request: discard,
                    profile: receipt.profile.clone(),
                    expected_revision: receipt.revision.unwrap(),
                })
                .unwrap();
            until(&mut runtime, |view| {
                view.connection_receipts
                    .iter()
                    .any(|receipt| receipt.request == discard)
            });
            let removed = runtime
                .view()
                .connection_receipts
                .iter()
                .find(|receipt| receipt.request == discard)
                .unwrap();
            assert_eq!(removed.profile, receipt.profile);
            assert_eq!(removed.result, Ok(()));
            assert_eq!(removed.revision, None);
            assert!(runtime.view().connection_receipts.len() <= 9);
            assert_eq!(runtime.view().connection_profiles.len(), 1);
            assert!(runtime.connection_inflight.is_empty());
        }
        assert!(!root.0.join("gateway.json").exists());
    }

    #[test]
    fn cs02_nine_unconsumed_receipts_backpressure_without_losing_acknowledgements_or_stop() {
        let (root, owner, _) = cs02_eight_profile_owner();
        drop(owner);
        let mut runtime = root.runtime();
        until(&mut runtime, |view| view.load_state == LoadState::Loaded);
        let profiles = runtime.view().connection_profiles.clone();
        for (index, profile) in profiles.iter().enumerate() {
            runtime
                .act(Action::EditConnectionProfile {
                    request: index as u64 + 1,
                    profile: profile.id.clone(),
                    expected_revision: profile.revision,
                    input: Box::new(profile.input.clone()),
                })
                .unwrap();
        }
        runtime
            .act(Action::CreateConnectionProfile { request: 9 })
            .unwrap();
        // Distinct otherwise-valid request, not the same-profile guard.
        assert_eq!(
            runtime.act(Action::EditConnectionProfile {
                request: 10,
                profile: "f".repeat(32),
                expected_revision: 0,
                input: Box::new(profiles[0].input.clone())
            }),
            Err(RuntimeError::Busy)
        );
        assert_eq!(runtime.connection_inflight.len(), 9);
        let stop_epoch = runtime.stop_requested.load(Ordering::Acquire);
        runtime.act(Action::Stop).unwrap();
        assert_eq!(
            runtime.stop_requested.load(Ordering::Acquire),
            stop_epoch + 1
        );
        until(&mut runtime, |view| view.connection_receipts.len() == 9);
        for request in 1..=9 {
            assert!(
                runtime
                    .view()
                    .connection_receipts
                    .iter()
                    .any(|receipt| receipt.request == request)
            );
        }
        assert!(runtime.connection_inflight.is_empty());
        assert_eq!(
            runtime
                .view()
                .connection_receipts
                .iter()
                .find(|receipt| receipt.request == 9)
                .unwrap()
                .result,
            Err(RuntimeError::Busy)
        );
    }

    #[test]
    fn cs02_failed_try_send_does_not_mark_an_unsent_draft_request_as_inflight() {
        // A real bounded channel with no worker is a deterministic saturation
        // fixture for Runtime::act itself, not a replacement implementation.
        let (commands, incoming) = mpsc::sync_channel(1);
        let mut runtime = Runtime {
            commands,
            connection_inflight: Default::default(),
            last_connection_request: 0,
            pending: Arc::new(Mutex::new(Pending::default())),
            view: View::default(),
            exit: Arc::new(AtomicBool::new(false)),
            stop_requested: Arc::new(AtomicU64::new(0)),
            worker: None,
        };
        runtime.act(Action::RefreshAccounts).unwrap();
        assert_eq!(
            runtime.act(Action::CreateConnectionProfile { request: 1 }),
            Err(RuntimeError::Busy)
        );
        assert!(runtime.connection_inflight.is_empty());
        assert_eq!(runtime.last_connection_request, 0);
        let (_, _, first) = incoming.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(first, Action::RefreshAccounts));
        runtime
            .act(Action::CreateConnectionProfile { request: 1 })
            .unwrap();
        assert_eq!(runtime.last_connection_request, 1);
        assert_eq!(
            runtime.connection_inflight.get(&1).map(String::as_str),
            Some("")
        );
        let (_, delivered, action) = incoming.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(delivered.is_empty());
        assert!(matches!(
            action,
            Action::CreateConnectionProfile { request: 1 }
        ));
    }

    #[test]
    fn cs02_owner_explicit_fresh_account_local_choices_never_overwrite_applied_routes() {
        let (root, mut owner, accounts) = catalogue_owner();
        let peer = CataloguePeer::new(catalogue_reply);
        owner.gateway.fixture_catalogue_upstream(peer.url());
        let applied = owner.config.clone();
        let bytes = std::fs::read(root.0.join("gateway.json")).unwrap();
        let mut login = LoginCoordinator::new(
            owner.store.clone(),
            LoginMetadata {
                version: "synthetic-court".into(),
                locale: "en".into(),
                timezone_offset_seconds: 0,
            },
        );
        for account in &accounts {
            owner
                .action(Action::RefreshAccountModels(account.clone()), &mut login)
                .unwrap();
            finish_catalogue(&mut owner);
        }
        let profile = "b".repeat(32);
        for (index, account) in accounts.iter().enumerate() {
            let revision = owner.drafts.revision(&profile).unwrap();
            owner
                .action(
                    Action::ChooseConnectionModel {
                        request: index as u64 + 1,
                        profile: profile.clone(),
                        expected_revision: revision,
                        slot: 0,
                        account: account.clone(),
                        catalogue_id: "opencode/remote/same".into(),
                    },
                    &mut login,
                )
                .unwrap();
            let current = owner
                .view
                .connection_profiles
                .iter()
                .find(|entry| entry.id == profile)
                .unwrap();
            assert_eq!(
                current.input.allocations[0].account.as_deref(),
                Some(account.as_str())
            );
            assert_eq!(
                current.allocations[0].upstream_model.as_deref(),
                Some(if index == 0 { "wire-a" } else { "wire-b" })
            );
            assert_eq!(
                current.allocations[0].evidence,
                ConnectionEvidence::Unverified
            );
            assert_eq!(current.allocations[0].checked_unix_ms, None);
            assert_eq!(owner.config, applied);
            assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), bytes);
            assert!(owner.operation.is_none());
        }
        let revision = owner.drafts.revision(&profile).unwrap();
        owner.catalogues.invalidate(&accounts[1]);
        owner
            .action(
                Action::ChooseConnectionModel {
                    request: 3,
                    profile: profile.clone(),
                    expected_revision: revision,
                    slot: 0,
                    account: accounts[1].clone(),
                    catalogue_id: "opencode/remote/same".into(),
                },
                &mut login,
            )
            .unwrap();
        assert_eq!(owner.drafts.revision(&profile), Some(revision));
        assert_eq!(
            owner
                .view
                .connection_receipts
                .iter()
                .find(|receipt| receipt.request == 3)
                .unwrap()
                .result,
            Err(RuntimeError::VerificationRequired)
        );
        assert_eq!(owner.config, applied);
    }

    #[test]
    fn cs02_runtime_accepts_only_bounded_draft_messages_and_reports_create_discard_identity() {
        let root = Root::new();
        let mut runtime = root.runtime();
        until(&mut runtime, |view| view.load_state == LoadState::Loaded);
        runtime
            .act(Action::CreateConnectionProfile { request: 1 })
            .unwrap();
        until(&mut runtime, |view| {
            view.connection_receipts
                .iter()
                .any(|receipt| receipt.request == 1)
        });
        let created = runtime
            .view()
            .connection_receipts
            .iter()
            .find(|receipt| receipt.request == 1)
            .unwrap()
            .clone();
        assert_eq!(created.result, Ok(()));
        assert!(crate::provider_server::opaque_id(&created.profile));
        assert_eq!(runtime.view().connection_profile, created.profile);
        let mut input = runtime
            .view()
            .connection_profiles
            .iter()
            .find(|view| view.id == created.profile)
            .unwrap()
            .input
            .clone();
        input.label = "x".repeat(129);
        assert_eq!(
            runtime.act(Action::EditConnectionProfile {
                request: 2,
                profile: created.profile.clone(),
                expected_revision: created.revision.unwrap(),
                input: Box::new(input)
            }),
            Err(RuntimeError::InvalidAction)
        );
        runtime
            .act(Action::DiscardUnappliedProfile {
                request: 3,
                profile: created.profile.clone(),
                expected_revision: created.revision.unwrap(),
            })
            .unwrap();
        until(&mut runtime, |view| {
            view.connection_receipts
                .iter()
                .any(|receipt| receipt.request == 3)
        });
        let discarded = runtime
            .view()
            .connection_receipts
            .iter()
            .find(|receipt| receipt.request == 3)
            .unwrap();
        assert_eq!(discarded.profile, created.profile);
        assert_eq!(discarded.revision, None);
        assert_eq!(discarded.result, Ok(()));
        assert_eq!(runtime.view().connection_profile, "default");
        assert!(!root.0.join("gateway.json").exists());
        assert!(!root.0.join("client-key").exists());
        assert_eq!(runtime.view().listener, ListenerState::Stopped);
    }

    struct Root(PathBuf);
    impl Root {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "minicon-runtime-{}",
                crate::provider_server::random_id().unwrap()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
        fn runtime(&self) -> Runtime {
            Runtime::new(
                self.0.clone(),
                LoginMetadata {
                    version: "synthetic-court".into(),
                    locale: "en".into(),
                    timezone_offset_seconds: 0,
                },
                Arc::new(|| {}),
            )
            .unwrap()
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn until(runtime: &mut Runtime, condition: impl Fn(&View) -> bool) -> Vec<Effect> {
        until_with_timeout(runtime, Duration::from_secs(5), condition)
    }
    fn until_with_timeout(
        runtime: &mut Runtime,
        timeout: Duration,
        condition: impl Fn(&View) -> bool,
    ) -> Vec<Effect> {
        let deadline = Instant::now() + timeout;
        let mut effects = Vec::new();
        loop {
            effects.extend(runtime.poll());
            if condition(runtime.view()) {
                return effects;
            }
            assert!(
                Instant::now() < deadline,
                "runtime did not publish expected metadata: {:?}",
                runtime.view()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    struct CataloguePeer {
        address: std::net::SocketAddr,
        stopping: Arc<AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }
    impl CataloguePeer {
        fn new(mut response: impl FnMut(usize, &str) -> Vec<u8> + Send + 'static) -> Self {
            use std::io::{Read, Write};
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            listener.set_nonblocking(true).unwrap();
            let stopping = Arc::new(AtomicBool::new(false));
            let stop = stopping.clone();
            let worker = std::thread::spawn(move || {
                let mut index = 0;
                while !stop.load(Ordering::Acquire) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(pair) => pair,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2));
                            continue;
                        }
                        Err(error) => panic!("synthetic metadata peer accept: {error}"),
                    };
                    // Windows accepted streams can inherit nonblocking mode.
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut head = Vec::new();
                    while !head.ends_with(b"\r\n\r\n") {
                        let mut byte = [0];
                        stream.read_exact(&mut byte).unwrap();
                        head.push(byte[0]);
                        assert!(head.len() <= 16_384);
                    }
                    let head = String::from_utf8(head).unwrap();
                    assert!(
                        head.starts_with("GET "),
                        "metadata must not infer or refresh a fresh token"
                    );
                    assert!(index < 64);
                    let body = response(index, &head);
                    index += 1;
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    // Stop/body caps may close the actual client before this
                    // finite fixture reply; do not mask a request assertion.
                    let _ = stream
                        .write_all(header.as_bytes())
                        .and_then(|()| stream.write_all(&body));
                }
            });
            Self {
                address,
                stopping,
                worker: Some(worker),
            }
        }
        fn url(&self) -> String {
            format!("http://{}/metadata", self.address)
        }
    }
    impl Drop for CataloguePeer {
        fn drop(&mut self) {
            self.stopping.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let result = worker.join();
                if !std::thread::panicking() {
                    assert!(result.is_ok(), "synthetic metadata peer failed");
                }
            }
        }
    }
    fn catalogue_reply(index: usize, head: &str) -> Vec<u8> {
        match index % 3 {
            0 => br#"{"id":"synthetic-user","email":"synthetic.invalid"}"#.to_vec(),
            1 => b"[]".to_vec(),
            _ => {
                let account = if head.contains("synthetic-access-b") {
                    "b"
                } else {
                    "a"
                };
                serde_json::to_vec(&serde_json::json!({"providers":{"remote":{
                    "package":"@opencode/ai/providers/openai-compatible",
                    "settings":{"baseURL":format!("https://opencode.ai/inference/account-{account}/v1")},
                    "models":{"same":{"modelID":format!("wire-{account}")}}
                }}})).unwrap()
            }
        }
    }
    fn catalogue_owner() -> (Root, Owner, [String; 2]) {
        use crate::provider_auth::{TokenGrant, TokenLifecycle};
        use crate::provider_server::{ClientAllocation, ClientProfile, WireProtocol};
        let root = Root::new();
        let mut owner = Owner::load(root.0.clone()).unwrap();
        let mut accounts = Vec::new();
        for name in ["a", "b"] {
            let account = owner
                .store
                .insert(
                    &format!("Synthetic {name}"),
                    TokenGrant {
                        provider: Provider::OpenCode,
                        access_token: Secret::new(&format!("synthetic-access-{name}")).unwrap(),
                        lifecycle: TokenLifecycle::ExpiringRefresh {
                            refresh_token: Secret::new("synthetic-refresh").unwrap(),
                            expires_at: Instant::now() + Duration::from_secs(3600),
                        },
                        organization: None,
                    },
                )
                .unwrap();
            owner.store.mark_ready(&account.id).unwrap();
            accounts.push(account.id);
        }
        let mut config = owner.config.clone();
        config.version = 2;
        config.selected_account = Some(accounts[0].clone());
        config.model = "opencode/remote/same".into();
        config.models = vec![ModelRoute {
            id: config.model.clone(),
            model_id: "legacy-wire".into(),
            provider: Provider::OpenCode,
            protocol: WireProtocol::Chat,
            url: "https://opencode.ai/inference/legacy/v1/chat/completions".into(),
            headers: vec![],
        }];
        config.profiles = vec![ClientProfile {
            id: "b".repeat(32),
            label: "Other client".into(),
            enabled: true,
            default_model: "work".into(),
            allocations: vec![ClientAllocation {
                alias: "work".into(),
                account: accounts[1].clone(),
                route: config.model.clone(),
            }],
        }];
        owner = load_legacy_fixture(owner, config);
        owner
            .gateway
            .set_key(Some(Secret::new("synthetic-default").unwrap()))
            .unwrap();
        owner
            .gateway
            .set_client_key(
                &"b".repeat(32),
                Some(Secret::new("synthetic-custom").unwrap()),
            )
            .unwrap();
        (root, owner, accounts.try_into().unwrap())
    }
    fn finish_catalogue(owner: &mut Owner) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while owner.operation.is_some() {
            assert!(
                Instant::now() < deadline,
                "bounded synthetic catalogue did not complete"
            );
            owner.poll_operation();
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn catalogue_binding(address: std::net::SocketAddr, key: &str) -> serde_json::Value {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        write!(stream, "GET /v1/minicon/binding HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {key}\r\nx-minicon-session: catalogue-stable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        let mut bytes = String::new();
        stream.read_to_string(&mut bytes).unwrap();
        assert!(
            bytes.starts_with("HTTP/1.1 200"),
            "local binding was changed by metadata: {bytes}"
        );
        serde_json::from_str(bytes.split_once("\r\n\r\n").unwrap().1).unwrap()
    }

    #[test]
    fn cs02_runtime_load_migrates_only_memory_keeps_private_bytes_keys_and_existing_route() {
        let root = Root::new();
        let (owner, account) = seed_ready(&root);
        let original = owner.config.clone();
        drop(owner);
        let path = root.0.join("gateway.json");
        // Prepare the exact old file immediately before the observed load;
        // fixture setup also uses the decoder and cannot stand in for it.
        let mut legacy = original.clone();
        legacy.version = 1;
        legacy.default_route = None;
        legacy.applied_evidence.clear();
        filesystem::write_private_atomic(&path, &legacy.encode_applied().unwrap()).unwrap();
        let before = std::fs::read(&path).unwrap();
        let key_path = root.0.join("client-key");
        let key = std::fs::read(&key_path).unwrap();
        let mut loaded = Owner::load(root.0.clone()).unwrap();
        assert_eq!(loaded.config.version, 3);
        assert_eq!(
            loaded.config.selected_account.as_deref(),
            Some(account.as_str())
        );
        assert_eq!(loaded.config.model, original.model);
        assert_eq!(loaded.config.models, original.models);
        assert!(matches!(
            loaded.config.applied_evidence[0].origin,
            crate::provider_server::AppliedOrigin::LegacyApplied { .. }
        ));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "read-time migration rewrote applied bytes"
        );
        assert_eq!(std::fs::read(&key_path).unwrap(), key);
        assert!(loaded.server.is_none());
        let mut login = LoginCoordinator::new(
            loaded.store.clone(),
            LoginMetadata {
                version: "synthetic".into(),
                locale: "en".into(),
                timezone_offset_seconds: 0,
            },
        );
        for action in [
            Action::SelectAccount(account),
            Action::SelectModel(original.model),
        ] {
            assert!(matches!(
                loaded.action(action, &mut login),
                Err(RuntimeError::VerificationRequired)
            ));
        }
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(std::fs::read(&key_path).unwrap(), key);
        // A deliberate unrelated save persists v3 provenance through the real
        // private publication path; reopening must preserve that origin.
        let config = loaded.config.clone();
        loaded.save(config).unwrap();
        let stored = std::fs::read(&path).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&stored).unwrap()["version"],
            3
        );
        let reopened = Owner::load(root.0.clone()).unwrap();
        assert_eq!(
            reopened.config.applied_evidence,
            loaded.config.applied_evidence
        );
        assert_eq!(std::fs::read(&key_path).unwrap(), key);
    }

    #[test]
    fn cs02_runtime_keeps_incomplete_legacy_model_on_load_but_refuses_broken_complete_reference() {
        for version in [1, 2] {
            let root = Root::new();
            let mut pending = Config::fresh().unwrap();
            pending.version = version;
            pending.model = "legacy-pending".into();
            assert!(pending.selected_account.is_none());
            assert!(pending.models.is_empty());
            let path = root.0.join("gateway.json");
            let before = serde_json::to_vec(&pending).unwrap();
            filesystem::write_private_atomic(&path, &before).unwrap();
            let loaded = Owner::load(root.0.clone());
            assert!(
                loaded.is_ok(),
                "valid incomplete legacy setup must load: {:?}",
                loaded.as_ref().err()
            );
            let owner = loaded.unwrap();
            assert_eq!(owner.config.version, 3);
            assert_eq!(owner.config.model, "legacy-pending");
            assert!(owner.config.models.is_empty());
            assert!(owner.config.selected_account.is_none());
            assert!(owner.config.applied_evidence.is_empty());
            assert_eq!(owner.view.model, "legacy-pending");
            assert!(owner.view.models.is_empty());
            assert!(owner.server.is_none());
            assert_eq!(owner.view.listener, ListenerState::Stopped);
            assert_eq!(std::fs::read(&path).unwrap(), before);
            assert!(!root.0.join("client-key").exists());
            // Completing the account reference without supplying its route
            // is structurally broken. Keep the same model string on disk and
            // return typed unavailable rather than deleting it during load.
            pending.selected_account = Some("e".repeat(32));
            let broken = serde_json::to_vec(&pending).unwrap();
            filesystem::write_private_atomic(&path, &broken).unwrap();
            assert!(matches!(
                Owner::load(root.0.clone()),
                Err(RuntimeError::Corrupt)
            ));
            assert_eq!(std::fs::read(&path).unwrap(), broken);
            assert!(!root.0.join("client-key").exists());
        }
    }

    #[test]
    fn cs01_real_account_catalogues_do_not_modify_applied_disk_or_named_bindings() {
        let (root, mut owner, accounts) = catalogue_owner();
        let peer = CataloguePeer::new(catalogue_reply);
        owner.gateway.fixture_catalogue_upstream(peer.url());
        let mut config = owner.config.clone();
        config.port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        owner.save(config).unwrap();
        let mut server = owner.gateway.start(owner.config.port).unwrap();
        let before = [
            catalogue_binding(server.address(), "synthetic-default"),
            catalogue_binding(server.address(), "synthetic-custom"),
        ];
        let disk = std::fs::read(root.0.join("gateway.json")).unwrap();
        let applied = owner.config.clone();
        let mut login = LoginCoordinator::new(
            owner.store.clone(),
            LoginMetadata {
                version: "synthetic".into(),
                locale: "en".into(),
                timezone_offset_seconds: 0,
            },
        );
        for account in &accounts {
            owner
                .action(Action::RefreshAccountModels(account.clone()), &mut login)
                .unwrap();
            assert_eq!(owner.catalogues.status(account), CatalogueStatus::Loading);
            finish_catalogue(&mut owner);
            assert_eq!(owner.catalogues.status(account), CatalogueStatus::Fresh);
            assert_eq!(owner.config, applied);
            assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), disk);
        }
        let a = owner
            .catalogues
            .select(
                &owner.store.snapshot(&accounts[0]).unwrap(),
                "opencode/remote/same",
            )
            .unwrap();
        let b = owner
            .catalogues
            .select(
                &owner.store.snapshot(&accounts[1]).unwrap(),
                "opencode/remote/same",
            )
            .unwrap();
        assert_eq!(a.route().model_id, "wire-a");
        assert_eq!(b.route().model_id, "wire-b");
        assert_ne!(a.route().url, b.route().url);
        assert_eq!(owner.view.selected_account, applied.selected_account);
        assert_eq!(owner.view.model, applied.model);
        assert_eq!(
            before,
            [
                catalogue_binding(server.address(), "synthetic-default"),
                catalogue_binding(server.address(), "synthetic-custom")
            ]
        );
        assert!(!owner.view.login_active);
        let reopened = Owner::load(root.0.clone()).unwrap();
        assert_eq!(reopened.config, applied);
        assert!(
            reopened
                .view
                .catalogues
                .iter()
                .all(|entry| entry.status == CatalogueStatus::NotLoaded && entry.models.is_empty())
        );
        server.stop();
        login.shutdown(Duration::from_secs(3)).unwrap();
    }

    #[test]
    fn cs01_real_late_catalogue_replies_cannot_cross_account_authority_changes() {
        use crate::provider_auth::{TokenGrant, TokenLifecycle};
        for change in [
            "credential",
            "service",
            "disabled",
            "removed",
            "organization",
        ] {
            let (root, mut owner, accounts) = catalogue_owner();
            let (arrived, waiting) = std::sync::mpsc::sync_channel(1);
            let (release, released) = std::sync::mpsc::sync_channel(1);
            let peer = CataloguePeer::new(move |index, head| {
                if index == 2 {
                    arrived.send(()).unwrap();
                    let _ = released.recv_timeout(Duration::from_secs(3));
                }
                catalogue_reply(index, head)
            });
            owner.gateway.fixture_catalogue_upstream(peer.url());
            let disk = std::fs::read(root.0.join("gateway.json")).unwrap();
            owner.begin_catalogue(accounts[0].clone()).unwrap();
            waiting.recv_timeout(Duration::from_secs(3)).unwrap();
            let snapshot = owner.store.snapshot(&accounts[0]).unwrap();
            match change {
                "credential" => {
                    owner
                        .store
                        .reconnect_if_current(
                            &accounts[0],
                            &snapshot.generation,
                            TokenGrant {
                                provider: Provider::OpenCode,
                                access_token: Secret::new("synthetic-reconnected").unwrap(),
                                lifecycle: TokenLifecycle::ExpiringRefresh {
                                    refresh_token: Secret::new("synthetic-new-refresh").unwrap(),
                                    expires_at: Instant::now() + Duration::from_secs(3600),
                                },
                                organization: None,
                            },
                        )
                        .unwrap();
                }
                "service" | "disabled" => {
                    owner
                        .gateway
                        .update_account(&AccountEdit {
                            account: accounts[0].clone(),
                            label: snapshot.summary.label.clone(),
                            login: snapshot.preferences.login.clone(),
                            service: if change == "service" {
                                NetworkPolicy::Proxy("http://127.0.0.1:9".into())
                            } else {
                                snapshot.preferences.service.clone()
                            },
                            enabled: change != "disabled",
                            expected: snapshot.preferences.clone(),
                            expected_label: snapshot.summary.label,
                        })
                        .unwrap();
                }
                "removed" => {
                    owner.store.remove(&accounts[0]).unwrap();
                }
                "organization" => {
                    owner
                        .store
                        .set_organization_if_current(
                            &accounts[0],
                            &snapshot.generation,
                            Some("new-org".into()),
                        )
                        .unwrap();
                }
                _ => unreachable!(),
            }
            release.send(()).unwrap();
            finish_catalogue(&mut owner);
            assert_ne!(
                owner.catalogues.status(&accounts[0]),
                CatalogueStatus::Fresh,
                "late {change} response became Fresh"
            );
            assert!(owner.catalogues.models(&accounts[0]).is_empty());
            if change == "removed" {
                assert!(!owner.catalogue_errors.contains_key(&accounts[0]));
            }
            assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), disk);
        }
    }

    #[test]
    fn cs01_real_catalogue_stop_reaps_native_transport_and_allows_a_new_metadata_read() {
        let (_root, mut owner, accounts) = catalogue_owner();
        let (arrived, waiting) = std::sync::mpsc::sync_channel(1);
        let (release, released) = std::sync::mpsc::sync_channel(1);
        let peer = CataloguePeer::new(move |index, head| {
            if index == 2 {
                arrived.send(()).unwrap();
                let _ = released.recv_timeout(Duration::from_secs(3));
            }
            catalogue_reply(index, head)
        });
        owner.gateway.fixture_catalogue_upstream(peer.url());
        owner.begin_catalogue(accounts[0].clone()).unwrap();
        waiting.recv_timeout(Duration::from_secs(3)).unwrap();
        let started = Instant::now();
        owner.stop();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(owner.operation.is_none());
        assert_eq!(owner.gateway.fixture_catalogue_leases(&accounts[0]), 0);
        assert_eq!(
            owner.catalogue_errors.get(&accounts[0]),
            Some(&RuntimeError::Cancelled)
        );
        let cancelled_view = owner
            .view
            .catalogues
            .iter()
            .find(|entry| entry.account == accounts[0])
            .unwrap();
        assert_eq!(cancelled_view.status, CatalogueStatus::Error);
        assert_eq!(cancelled_view.error, Some(RuntimeError::Cancelled));
        release.send(()).unwrap();
        owner.begin_catalogue(accounts[0].clone()).unwrap();
        finish_catalogue(&mut owner);
        assert_eq!(
            owner.catalogues.status(&accounts[0]),
            CatalogueStatus::Fresh
        );
        assert_eq!(owner.gateway.fixture_catalogue_leases(&accounts[0]), 0);
        assert_eq!(owner.view.listener, ListenerState::Stopped);
    }

    #[test]
    fn cs01_completed_http_result_is_rechecked_under_store_lock_before_cache_publication() {
        use crate::provider_auth::{TokenGrant, TokenLifecycle};
        let (root, mut owner, accounts) = catalogue_owner();
        let peer = CataloguePeer::new(catalogue_reply);
        owner.gateway.fixture_catalogue_upstream(peer.url());
        owner.begin_catalogue(accounts[0].clone()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        while !owner.operation.as_ref().unwrap().worker.is_finished() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        // The real HTTP result and its transport-side check are complete, but
        // the owner has deliberately not yet consumed that result.
        let snapshot = owner.store.snapshot(&accounts[0]).unwrap();
        let disk = std::fs::read(root.0.join("gateway.json")).unwrap();
        owner
            .store
            .reconnect_if_current(
                &accounts[0],
                &snapshot.generation,
                TokenGrant {
                    provider: Provider::OpenCode,
                    access_token: Secret::new("synthetic-new-generation").unwrap(),
                    lifecycle: TokenLifecycle::ExpiringRefresh {
                        refresh_token: Secret::new("synthetic-refresh").unwrap(),
                        expires_at: Instant::now() + Duration::from_secs(3600),
                    },
                    organization: None,
                },
            )
            .unwrap();
        owner.poll_operation();
        assert!(owner.operation.is_none());
        assert_ne!(
            owner.catalogues.status(&accounts[0]),
            CatalogueStatus::Fresh
        );
        assert!(
            owner.catalogues.models(&accounts[0]).is_empty(),
            "a new late body was retained as stale metadata"
        );
        assert_eq!(
            owner.catalogue_errors.get(&accounts[0]),
            Some(&RuntimeError::AccountChanged)
        );
        assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), disk);
    }

    #[test]
    fn cs01_real_catalogue_response_budget_is_cumulative_before_the_config_request() {
        let (_root, mut owner, accounts) = catalogue_owner();
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = count.clone();
        let peer = CataloguePeer::new(move |index, head| {
            observed.fetch_add(1, Ordering::AcqRel);
            let mut body = catalogue_reply(index, head);
            if index < 2 {
                body.extend(std::iter::repeat_n(b' ', 1_100_000));
            }
            body
        });
        owner.gateway.fixture_catalogue_upstream(peer.url());
        owner.begin_catalogue(accounts[0].clone()).unwrap();
        finish_catalogue(&mut owner);
        assert_eq!(
            count.load(Ordering::Acquire),
            2,
            "oversized metadata chain reached config"
        );
        assert_ne!(
            owner.catalogues.status(&accounts[0]),
            CatalogueStatus::Fresh
        );
        assert!(
            owner
                .view
                .catalogues
                .iter()
                .find(|entry| entry.account == accounts[0])
                .unwrap()
                .error
                .is_some()
        );
    }

    #[test]
    fn cs01_real_failed_refresh_retains_previous_rows_without_publishing_routes() {
        let (root, mut owner, accounts) = catalogue_owner();
        let peer = CataloguePeer::new(|index, head| {
            if index == 5 {
                b"invalid catalogue JSON".to_vec()
            } else {
                catalogue_reply(index, head)
            }
        });
        owner.gateway.fixture_catalogue_upstream(peer.url());
        let disk = std::fs::read(root.0.join("gateway.json")).unwrap();
        owner.begin_catalogue(accounts[0].clone()).unwrap();
        finish_catalogue(&mut owner);
        assert_eq!(
            owner.catalogues.status(&accounts[0]),
            CatalogueStatus::Fresh
        );
        let rows = owner.catalogues.models(&accounts[0]);
        assert_eq!(rows.len(), 1);
        owner.begin_catalogue(accounts[0].clone()).unwrap();
        finish_catalogue(&mut owner);
        assert_eq!(
            owner.catalogues.status(&accounts[0]),
            CatalogueStatus::Error
        );
        assert_eq!(owner.catalogues.models(&accounts[0]), rows);
        assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), disk);
    }

    #[test]
    fn cs01_failed_start_reprojects_retained_catalogue_without_a_fresh_claim() {
        for reason in ["missing-credential", "disabled"] {
            let (root, mut owner, accounts) = catalogue_owner();
            let peer = CataloguePeer::new(catalogue_reply);
            owner.gateway.fixture_catalogue_upstream(peer.url());
            owner.begin_catalogue(accounts[0].clone()).unwrap();
            finish_catalogue(&mut owner);
            let rows = owner.catalogues.models(&accounts[0]);
            assert_eq!(
                owner.catalogues.status(&accounts[0]),
                CatalogueStatus::Fresh
            );
            let disk = std::fs::read(root.0.join("gateway.json")).unwrap();
            match reason {
                "missing-credential" => {
                    std::fs::remove_file(
                        root.0
                            .join("accounts/credentials")
                            .join(format!("{}.json", accounts[0])),
                    )
                    .unwrap();
                }
                "disabled" => {
                    let snapshot = owner.store.snapshot(&accounts[0]).unwrap();
                    owner
                        .gateway
                        .update_account(&AccountEdit {
                            account: accounts[0].clone(),
                            label: snapshot.summary.label.clone(),
                            login: snapshot.preferences.login.clone(),
                            service: snapshot.preferences.service.clone(),
                            enabled: false,
                            expected: snapshot.preferences,
                            expected_label: snapshot.summary.label,
                        })
                        .unwrap();
                }
                _ => unreachable!(),
            }
            assert!(owner.begin_catalogue(accounts[0].clone()).is_err());
            assert!(owner.operation.is_none());
            assert!(!owner.view.busy);
            let reading = owner
                .view
                .catalogues
                .iter()
                .find(|entry| entry.account == accounts[0])
                .unwrap();
            assert_ne!(
                reading.status,
                CatalogueStatus::Fresh,
                "failed {reason} refresh retained a Fresh claim"
            );
            assert!(
                reading.error.is_some(),
                "failed {reason} refresh has no account error"
            );
            assert_eq!(
                reading.models, rows,
                "failed start discarded prior readings"
            );
            assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), disk);
        }
    }

    #[test]
    fn cs01_failed_metadata_read_publishes_terminal_operation_and_recovers() {
        let (root, mut owner, accounts) = catalogue_owner();
        let peer = CataloguePeer::new(catalogue_reply);
        owner.gateway.fixture_catalogue_upstream(peer.url());
        owner.begin_catalogue(accounts[0].clone()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        while !owner.operation.as_ref().unwrap().worker.is_finished() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        let registry_path = root.0.join("accounts/accounts.json");
        let registry = std::fs::read(&registry_path).unwrap();
        minicon_platform::filesystem::write_private_atomic(
            &registry_path,
            b"invalid registry JSON",
        )
        .unwrap();
        owner.poll_operation();
        assert!(owner.operation.is_none());
        assert!(
            !owner.view.busy,
            "completed operation remained busy after registry read failed"
        );
        assert!(matches!(owner.view.load_state, LoadState::Unavailable(_)));
        assert!(owner.view.status.is_some());
        assert!(owner.view.catalogues.iter().all(|entry| !matches!(
            entry.status,
            CatalogueStatus::Loading | CatalogueStatus::Fresh
        )));
        assert!(
            owner
                .view
                .catalogues
                .iter()
                .find(|entry| entry.account == accounts[0])
                .unwrap()
                .error
                .is_some()
        );
        minicon_platform::filesystem::write_private_atomic(&registry_path, &registry).unwrap();
        owner.refresh_view().unwrap();
        assert_eq!(owner.view.load_state, LoadState::Loaded);
        assert!(!owner.view.busy);
        assert_eq!(owner.gateway.fixture_catalogue_leases(&accounts[0]), 0);
    }

    #[test]
    fn h0_runtime_load_state_is_published_only_after_successful_private_metadata_read() {
        let good = Root::new();
        let mut runtime = good.runtime();
        assert_eq!(runtime.view().load_state, LoadState::Loading);
        until(&mut runtime, |view| view.load_state == LoadState::Loaded);
        assert!(runtime.view().accounts.is_empty());
        let bad = Root::new();
        std::fs::create_dir(bad.0.join("gateway.json")).unwrap();
        let mut failed = bad.runtime();
        assert_eq!(failed.view().load_state, LoadState::Loading);
        until(&mut failed, |view| {
            matches!(view.load_state, LoadState::Unavailable(_))
        });
        assert_eq!(
            failed.view().load_state,
            LoadState::Unavailable(RuntimeError::Storage)
        );
        assert!(!failed.view().busy);
        assert!(failed.view().accounts.is_empty());
    }

    #[test]
    fn login_completion_immediately_publishes_account_preferences_and_preserves_completion_effect()
    {
        use crate::provider_auth::{TokenGrant, TokenLifecycle};
        let root = Root::new();
        let mut owner = Owner::load(root.0.clone()).unwrap();
        let account = owner
            .store
            .insert(
                "New account",
                TokenGrant {
                    provider: Provider::DeepSeek,
                    access_token: Secret::new("synthetic-new-account").unwrap(),
                    lifecycle: TokenLifecycle::OpaqueUntilRejected,
                    organization: None,
                },
            )
            .unwrap();
        let effect = owner.login_event(LoginEvent::CredentialStored {
            attempt: crate::provider_login::AttemptId(1),
            account: account.clone(),
            completion_url: Some(
                Secret::new("https://platform.deepseek.com/synthetic-complete").unwrap(),
            ),
        });
        assert!(matches!(effect, Some(Effect::AuthFinished(AttemptId(1)))));
        assert_eq!(
            owner.view.auth_result,
            Some((AttemptId(1), AuthOutcome::CredentialStored))
        );
        assert_eq!(owner.view.accounts, vec![account.clone()]);
        assert_eq!(
            owner.view.account_preferences,
            vec![(account.id.clone(), AccountPreferences::legacy(&account.id))]
        );
        assert!(
            owner
                .view
                .status
                .as_deref()
                .unwrap()
                .contains("Credential stored")
        );
        let mut login = LoginCoordinator::new(
            owner.store.clone(),
            LoginMetadata {
                version: "fixture".into(),
                locale: "en-US".into(),
                timezone_offset_seconds: 0,
            },
        );
        assert!(matches!(
            owner.action(Action::SelectAccount(account.id.clone()), &mut login),
            Err(RuntimeError::VerificationRequired)
        ));
        assert!(owner.view.selected_account.is_none());
    }

    #[test]
    fn queued_account_saves_publish_each_receipt_and_keep_failed_and_stale_edits_out_of_storage() {
        use crate::provider_auth::{TokenGrant, TokenLifecycle};
        let root = Root::new();
        let store = AccountStore::new(root.0.join("accounts")).unwrap();
        let mut accounts = Vec::new();
        for label in ["First", "Second"] {
            accounts.push(
                store
                    .insert(
                        label,
                        TokenGrant {
                            provider: Provider::DeepSeek,
                            access_token: Secret::new("synthetic-account-save").unwrap(),
                            lifecycle: TokenLifecycle::OpaqueUntilRejected,
                            organization: None,
                        },
                    )
                    .unwrap(),
            );
        }
        let mut runtime = root.runtime();
        until(&mut runtime, |view| view.accounts.len() == 2 && !view.busy);
        let edit = |index: usize, label: &str| AccountEdit {
            account: accounts[index].id.clone(),
            label: label.into(),
            login: NetworkPolicy::Direct,
            service: NetworkPolicy::Direct,
            enabled: true,
            expected: AccountPreferences::legacy(&accounts[index].id),
            expected_label: accounts[index].label.clone(),
        };
        runtime
            .act(Action::SaveAccount {
                request: 1,
                edit: Box::new(edit(0, "Invalid\nlabel")),
            })
            .unwrap();
        runtime
            .act(Action::SaveAccount {
                request: 2,
                edit: Box::new(edit(1, "Renamed second")),
            })
            .unwrap();
        until(&mut runtime, |view| view.account_save_receipts.len() == 2);
        let receipts = &runtime.view().account_save_receipts;
        assert_eq!(
            receipts
                .iter()
                .find(|receipt| receipt.account == accounts[0].id)
                .unwrap()
                .result,
            Err(RuntimeError::InvalidAction)
        );
        assert_eq!(
            receipts
                .iter()
                .find(|receipt| receipt.account == accounts[1].id)
                .unwrap()
                .result,
            Ok(())
        );
        let stored = store.list().unwrap();
        assert_eq!(stored[0].label, "First");
        assert_eq!(stored[1].label, "Renamed second");
        // A second queued form still carrying the old label is a real CAS
        // failure, with an updated view for an explicit revert/retry.
        runtime
            .act(Action::SaveAccount {
                request: 3,
                edit: Box::new(edit(1, "Stale replacement")),
            })
            .unwrap();
        until(&mut runtime, |view| {
            view.account_save_receipts
                .iter()
                .any(|receipt| receipt.request == 3)
        });
        assert_eq!(
            runtime
                .view()
                .account_save_receipts
                .iter()
                .find(|receipt| receipt.request == 3)
                .unwrap()
                .result,
            Err(RuntimeError::AccountChanged)
        );
        assert_eq!(runtime.view().accounts[1].label, "Renamed second");
        assert_eq!(store.list().unwrap()[1].label, "Renamed second");
    }

    #[test]
    fn client_profiles_load_private_keys_and_keep_sessions_and_revocation_isolated() {
        use crate::provider_auth::{TokenGrant, TokenLifecycle};
        use crate::provider_server::{Config, ModelRoute, WireProtocol};
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};

        let root = Root::new();
        prepare_root(&root.0).unwrap();
        let store = AccountStore::new(root.0.join("accounts")).unwrap();
        let mut accounts = Vec::new();
        for (label, provider) in [
            ("DeepSeek A", Provider::DeepSeek),
            ("DeepSeek B", Provider::DeepSeek),
            ("OpenCode Go C", Provider::OpenCode),
        ] {
            let account = store
                .insert(
                    label,
                    TokenGrant {
                        provider,
                        access_token: Secret::new("synthetic-provider").unwrap(),
                        lifecycle: match provider {
                            Provider::DeepSeek => TokenLifecycle::OpaqueUntilRejected,
                            Provider::OpenCode => TokenLifecycle::ExpiringRefresh {
                                refresh_token: Secret::new("synthetic-refresh").unwrap(),
                                expires_at: Instant::now() + Duration::from_secs(3600),
                            },
                        },
                        organization: None,
                    },
                )
                .unwrap();
            store.mark_ready(&account.id).unwrap();
            accounts.push(account);
        }
        let mut config = Config::fresh().unwrap();
        config.port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        config.selected_account = Some(accounts[0].id.clone());
        config.model = "deepseek/model".into();
        config.models = vec![
            ModelRoute {
                id: "deepseek/model".into(),
                model_id: "ds-model".into(),
                provider: Provider::DeepSeek,
                protocol: WireProtocol::Messages,
                url: "https://api.deepseek.com/anthropic/v1/messages".into(),
                headers: vec![],
            },
            ModelRoute {
                id: "opencode/model".into(),
                model_id: "go-model".into(),
                provider: Provider::OpenCode,
                protocol: WireProtocol::Chat,
                url: "https://opencode.ai/inference/go/openai/v1/chat/completions".into(),
                headers: vec![],
            },
        ];
        let first_profile = "b".repeat(32);
        let second_profile = "c".repeat(32);
        let mut value = serde_json::to_value(&config).unwrap();
        value["version"] = 2.into();
        value["profiles"] = serde_json::json!([
            {"id":first_profile,"label":"Pi","enabled":true,"default_model":"work",
             "allocations":[{"alias":"work","account":accounts[1].id,"route":"deepseek/model"}]},
            {"id":second_profile,"label":"Go client","enabled":true,"default_model":"coding",
             "allocations":[{"alias":"coding","account":accounts[2].id,"route":"opencode/model"}]}
        ]);
        filesystem::write_private_atomic(
            &root.0.join("gateway.json"),
            &serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        for (profile, key) in [
            ("default".to_owned(), "synthetic-default"),
            (first_profile.clone(), "synthetic-pi"),
            (second_profile.clone(), "synthetic-go"),
        ] {
            let name = if profile == "default" {
                "client-key".to_owned()
            } else {
                format!("client-key-{profile}")
            };
            filesystem::write_private_atomic(&root.0.join(name), key.as_bytes()).unwrap();
        }
        // Real private-store/runtime/listener composition, using synthetic
        // ready accounts only. No provider inference or clipboard operation.
        let owner = Owner::load(root.0.clone()).unwrap();
        let mut server = owner.gateway.start(config.port).unwrap();
        let request = |path: &str, key: &str, body: Option<&str>| {
            let mut stream = TcpStream::connect(server.address()).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(4)))
                .unwrap();
            let body = body.unwrap_or("");
            write!(stream, "{} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {key}\r\nx-minicon-session: same-name\r\nContent-Length: {}\r\n\r\n{body}", if body.is_empty() { "GET" } else { "POST" }, server.address(), body.len()).unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            String::from_utf8(bytes).unwrap()
        };
        for (key, profile, account, model) in [
            ("synthetic-default", "default", &accounts[0].id, "ds-model"),
            (
                "synthetic-pi",
                first_profile.as_str(),
                &accounts[1].id,
                "ds-model",
            ),
            (
                "synthetic-go",
                second_profile.as_str(),
                &accounts[2].id,
                "go-model",
            ),
        ] {
            let response = request("/v1/minicon/binding", key, None);
            assert!(response.starts_with("HTTP/1.1 200"), "{response}");
            let binding: serde_json::Value =
                serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
            assert_eq!(binding["profile"], profile);
            assert_eq!(binding["account"], account.as_str());
            assert_eq!(binding["model"], model);
        }
        let models = request("/v1/models", "synthetic-pi", None);
        let models: serde_json::Value =
            serde_json::from_str(models.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(models["data"][0]["id"], "work");
        assert_eq!(models["data"].as_array().unwrap().len(), 1);
        let denied = request(
            "/v1/chat/completions",
            "synthetic-pi",
            Some(r#"{"model":"opencode/model","messages":[{"role":"user","content":"denied"}]}"#),
        );
        assert!(denied.starts_with("HTTP/1.1 400"), "{denied}");
        assert!(denied.contains("model_unavailable"), "{denied}");
        owner.gateway.set_key(None).unwrap();
        assert!(request("/v1/models", "synthetic-default", None).starts_with("HTTP/1.1 401"));
        assert!(request("/v1/minicon/binding", "synthetic-pi", None).starts_with("HTTP/1.1 200"));
        assert!(request("/v1/minicon/binding", "synthetic-go", None).starts_with("HTTP/1.1 200"));
        server.stop();
    }

    #[test]
    fn legacy_selectors_and_other_account_catalogue_keep_applied_allocations_unchanged() {
        use crate::provider_server::{ClientAllocation, ClientProfile};
        let root = Root::new();
        let (mut owner, account) = seed_ready(&root);
        let other = owner
            .store
            .insert(
                "Other synthetic account",
                crate::provider_auth::TokenGrant {
                    provider: Provider::OpenCode,
                    access_token: Secret::new("synthetic-other-access").unwrap(),
                    lifecycle: crate::provider_auth::TokenLifecycle::ExpiringRefresh {
                        refresh_token: Secret::new("synthetic-other-refresh").unwrap(),
                        expires_at: Instant::now() + Duration::from_secs(3600),
                    },
                    organization: None,
                },
            )
            .unwrap();
        owner.store.mark_ready(&other.id).unwrap();
        let mut config = owner.config.clone();
        config.version = 2;
        config.profiles.push(ClientProfile {
            id: "a".repeat(32),
            label: "Keep working".into(),
            enabled: true,
            default_model: "work".into(),
            allocations: vec![ClientAllocation {
                alias: "work".into(),
                account,
                route: config.model.clone(),
            }],
        });
        owner = load_legacy_fixture(owner, config);
        let applied = owner.config.clone();
        let disk = std::fs::read(root.0.join("gateway.json")).unwrap();
        let mut login = LoginCoordinator::new(
            owner.store.clone(),
            LoginMetadata {
                version: "fixture".into(),
                locale: "en".into(),
                timezone_offset_seconds: 0,
            },
        );
        assert!(matches!(
            owner.action(Action::SelectAccount(other.id.clone()), &mut login),
            Err(RuntimeError::VerificationRequired)
        ));
        assert_eq!(owner.config.models.len(), 1);
        assert_eq!(owner.view.models, vec![applied.model.clone()]);
        let route = ModelRoute {
            id: "opencode/go/new".into(),
            model_id: "new-wire".into(),
            provider: Provider::OpenCode,
            protocol: crate::provider_server::WireProtocol::Chat,
            url: "https://opencode.ai/inference/go/openai/v1/chat/completions".into(),
            headers: vec![],
        };
        let snapshot = owner.store.snapshot(&other.id).unwrap();
        let ticket = owner.catalogues.begin(&snapshot).unwrap();
        let authority = ticket.authority().clone();
        owner.operation = Some(Operation {
            account: other.id.clone(),
            kind: OperationKind::Catalogue(ticket),
            cancel: Arc::new(NetworkHttpCancel::new()),
            worker: std::thread::spawn(move || {
                Ok(OperationResult::Catalog(CatalogueResult {
                    authority,
                    response_bytes: 128,
                    routes: vec![route],
                }))
            }),
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while owner.operation.is_some() {
            assert!(Instant::now() < deadline);
            owner.poll_operation();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(owner.view.status.is_none(), "{:?}", owner.view.status);
        assert_eq!(
            owner.config.models.len(),
            1,
            "metadata cannot import applied routes"
        );
        assert_eq!(owner.view.models, vec![applied.model.clone()]);
        assert_eq!(owner.config, applied);
        assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), disk);
        let catalogue = owner
            .view
            .catalogues
            .iter()
            .find(|entry| entry.account == other.id)
            .unwrap();
        assert_eq!(catalogue.status, CatalogueStatus::Fresh);
        assert_eq!(catalogue.models[0].id, "opencode/go/new");
        let loaded = Owner::load(root.0.clone()).unwrap();
        assert_eq!(loaded.config.models, owner.config.models);
        assert_eq!(loaded.config.profiles, owner.config.profiles);
        login.shutdown(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn conflicting_other_account_catalog_keeps_applied_routes_and_sessions_unchanged() {
        use crate::provider_auth::{TokenGrant, TokenLifecycle};
        use crate::provider_server::{ClientAllocation, ClientProfile, WireProtocol};
        let root = Root::new();
        let (mut owner, _) = seed_ready(&root);
        let mut accounts = Vec::new();
        for label in ["Synthetic OpenCode A", "Synthetic OpenCode B"] {
            let account = owner
                .store
                .insert(
                    label,
                    TokenGrant {
                        provider: Provider::OpenCode,
                        access_token: Secret::new("synthetic-access").unwrap(),
                        lifecycle: TokenLifecycle::ExpiringRefresh {
                            refresh_token: Secret::new("synthetic-refresh").unwrap(),
                            expires_at: Instant::now() + Duration::from_secs(3600),
                        },
                        organization: None,
                    },
                )
                .unwrap();
            owner.store.mark_ready(&account.id).unwrap();
            accounts.push(account);
        }
        let mut config = owner.config.clone();
        config.version = 2;
        config.selected_account = Some(accounts[0].id.clone());
        config.model = "opencode/shared-id".into();
        let original = ModelRoute {
            id: config.model.clone(),
            model_id: "same-wire-model".into(),
            provider: Provider::OpenCode,
            protocol: WireProtocol::Chat,
            url: "https://opencode.ai/inference/openai/v1/chat/completions".into(),
            headers: vec![],
        };
        config.models = vec![original.clone()];
        let profile = "a".repeat(32);
        config.profiles.push(ClientProfile {
            id: profile.clone(),
            label: "Applied A".into(),
            enabled: true,
            default_model: "work".into(),
            allocations: vec![ClientAllocation {
                alias: "work".into(),
                account: accounts[0].id.clone(),
                route: original.id.clone(),
            }],
        });
        owner = load_legacy_fixture(owner, config);
        owner
            .gateway
            .set_client_key(&profile, Some(Secret::new("synthetic-profile").unwrap()))
            .unwrap();
        let mut server = owner.gateway.start(owner.config.port).unwrap();
        let transport = crate::harness_gateway::GatewayTransport {
            base: format!("http://{}/v1", server.address()),
            session: "stable-account-a".into(),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        let before = transport
            .binding("synthetic-profile", Duration::from_secs(4))
            .unwrap();
        let mut login = LoginCoordinator::new(
            owner.store.clone(),
            LoginMetadata {
                version: "fixture".into(),
                locale: "en".into(),
                timezone_offset_seconds: 0,
            },
        );
        assert!(matches!(
            owner.action(Action::SelectAccount(accounts[1].id.clone()), &mut login),
            Err(RuntimeError::VerificationRequired)
        ));
        let disk = std::fs::read(root.0.join("gateway.json")).unwrap();
        let mut conflicting = original.clone();
        conflicting.url = "https://opencode.ai/inference/go/openai/v1/chat/completions".into();
        let snapshot = owner.store.snapshot(&accounts[1].id).unwrap();
        let ticket = owner.catalogues.begin(&snapshot).unwrap();
        let authority = ticket.authority().clone();
        owner.operation = Some(Operation {
            account: accounts[1].id.clone(),
            kind: OperationKind::Catalogue(ticket),
            cancel: Arc::new(NetworkHttpCancel::new()),
            worker: std::thread::spawn(move || {
                Ok(OperationResult::Catalog(CatalogueResult {
                    authority,
                    response_bytes: 128,
                    routes: vec![conflicting],
                }))
            }),
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while owner.operation.is_some() {
            assert!(Instant::now() < deadline);
            owner.poll_operation();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(owner.view.status.is_none(), "{:?}", owner.view.status);
        let catalogue = owner
            .view
            .catalogues
            .iter()
            .find(|entry| entry.account == accounts[1].id)
            .unwrap();
        assert_eq!(catalogue.status, CatalogueStatus::Fresh);
        assert!(catalogue.error.is_none());
        assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), disk);
        assert_eq!(owner.config.models, vec![original]);
        assert_eq!(
            transport
                .binding("synthetic-profile", Duration::from_secs(4))
                .unwrap(),
            before
        );
        server.stop();
        login.shutdown(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn port_save_failure_keeps_previous_configuration_and_view() {
        let root = Root::new();
        let mut runtime = root.runtime();
        until(&mut runtime, |view| !view.busy);
        let previous = runtime.view().port;
        std::fs::create_dir(root.0.join("gateway.json")).unwrap();
        runtime.act(Action::SetPort(previous + 1)).unwrap();
        until(&mut runtime, |view| view.status.is_some());
        assert_eq!(runtime.view().port, previous);
        assert_eq!(runtime.view().listener, ListenerState::Stopped);
        assert!(runtime.view().status.as_ref().unwrap().contains("Storage"));
    }

    #[test]
    fn gateway_prepare_failure_cannot_publish_a_new_disk_configuration() {
        let root = Root::new();
        let (mut owner, _) = seed_ready(&root);
        let before = std::fs::read(root.0.join("gateway.json")).unwrap();
        let port = owner.config.port;
        let mut changed = owner.config.clone();
        changed.port = if port == u16::MAX { port - 1 } else { port + 1 };
        owner.gateway.fail_next_update();
        assert_eq!(owner.save(changed), Err(RuntimeError::Storage));
        assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), before);
        assert_eq!(owner.config.port, port);
        assert_eq!(owner.view.port, port);
        assert_eq!(Owner::load(root.0.clone()).unwrap().config.port, port);
    }

    #[test]
    fn published_configuration_warning_keeps_disk_gateway_owner_and_view_aligned() {
        let root = Root::new();
        let (mut owner, _) = seed_ready(&root);
        let mut server = owner.gateway.start(owner.config.port).unwrap();
        let transport = |session: &str| crate::harness_gateway::GatewayTransport {
            base: format!("http://{}/v1", server.address()),
            session: session.into(),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        let old = transport("before-publication")
            .binding("synthetic-client", Duration::from_secs(4))
            .unwrap();
        let mut changed = owner.config.clone();
        changed.models[0].model_id = "published-wire-model".into();
        fixture_verified_default(&owner, &mut changed);
        let registry = root.0.join("accounts/accounts.json");
        let account_bytes = filesystem::read_private_bounded(&registry, 1024 * 1024).unwrap();
        let result = owner.save_with(changed.clone(), |path, bytes| {
            minicon_platform::filesystem_publish::write_private_atomic(path, bytes).unwrap();
            // Declared consumer fault: the bytes really changed, then an
            // unconfirmed barrier outcome is supplied. The shared writer's
            // independent test supplies the actual replace/barrier failure.
            // Also make a post-publication metadata read genuinely fail.
            filesystem::write_private_atomic(&registry, b"unavailable metadata").unwrap();
            Ok(ConfigPublication::DurabilityUnconfirmed)
        });
        filesystem::write_private_atomic(&registry, &account_bytes).unwrap();
        assert_eq!(result, Ok(()));
        assert_eq!(owner.config, changed);
        assert!(owner.view.config_durability_unconfirmed);
        assert_eq!(owner.view.model, changed.model);
        assert_eq!(Owner::load(root.0.clone()).unwrap().config, changed);
        let after = transport("after-publication")
            .binding("synthetic-client", Duration::from_secs(4))
            .unwrap();
        assert_eq!(after.model, "published-wire-model");
        assert_ne!(after.generation, old.generation);
        assert!(
            transport("before-publication")
                .binding("synthetic-client", Duration::from_secs(4))
                .is_err()
        );

        let applied =
            filesystem::read_private_bounded(&root.0.join("gateway.json"), 1024 * 1024).unwrap();
        let mut rejected = changed.clone();
        rejected.models[0].model_id = "must-not-apply".into();
        fixture_verified_default(&owner, &mut rejected);
        assert_eq!(
            owner.save_with(rejected, |_, _| Err(ServerError::Storage)),
            Err(RuntimeError::Storage)
        );
        assert_eq!(owner.config, changed);
        assert!(owner.view.config_durability_unconfirmed);
        assert_eq!(std::fs::read(root.0.join("gateway.json")).unwrap(), applied);
        assert_eq!(
            transport("after-publication")
                .binding("synthetic-client", Duration::from_secs(4))
                .unwrap(),
            after
        );

        owner.save(changed).unwrap();
        assert!(!owner.view.config_durability_unconfirmed);
        assert_eq!(
            transport("after-publication")
                .binding("synthetic-client", Duration::from_secs(4))
                .unwrap(),
            after
        );
        server.stop();
    }

    #[test]
    fn private_client_key_creation_copy_and_explicit_revocation_persist() {
        let root = Root::new();
        let mut runtime = root.runtime();
        until(&mut runtime, |view| !view.busy);
        runtime.act(Action::CreateClientKey).unwrap();
        until(&mut runtime, |view| view.local_key_exists);
        let stored = filesystem::read_private_bounded(&root.0.join("client-key"), 256).unwrap();
        runtime.act(Action::CopyClientKey).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let effects = runtime.poll();
            if let Some(Effect::CopyClientKey(secret)) = effects.into_iter().next() {
                assert_eq!(secret.expose().as_bytes(), stored);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        runtime.act(Action::CreateClientKey).unwrap();
        until(&mut runtime, |view| view.status.is_some());
        assert_eq!(
            filesystem::read_private_bounded(&root.0.join("client-key"), 256).unwrap(),
            stored
        );
        runtime.act(Action::RevokeClientKey).unwrap();
        until(&mut runtime, |view| !view.local_key_exists);
        assert!(!root.0.join("client-key").exists());
        drop(runtime);
        let mut restarted = root.runtime();
        until(&mut restarted, |view| !view.busy);
        assert!(!restarted.view().local_key_exists);
        assert_eq!(restarted.view().listener, ListenerState::Stopped);
    }
    #[test]
    fn missing_binding_start_is_typed_error_and_can_be_stopped() {
        let root = Root::new();
        let mut runtime = root.runtime();
        until(&mut runtime, |view| !view.busy);
        runtime.act(Action::Start).unwrap();
        until(&mut runtime, |view| view.listener == ListenerState::Error);
        assert!(
            runtime
                .view()
                .status
                .as_ref()
                .unwrap()
                .contains("MissingAccount")
        );
        assert_eq!(runtime.view().actual_port, None);
        runtime.act(Action::Stop).unwrap();
        until(&mut runtime, |view| view.listener == ListenerState::Stopped);
    }
    #[test]
    fn configured_port_collision_does_not_migrate_or_change_readiness() {
        let root = Root::new();
        let mut owner = Owner::load(root.0.clone()).unwrap();
        let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = occupied.local_addr().unwrap().port();
        // Synthetic stored-ready data only exercises local listener setup; it
        // is never evidence of real provider authorization or inference.
        let account = owner
            .store
            .insert(
                "Synthetic ready",
                crate::provider_auth::TokenGrant {
                    provider: Provider::DeepSeek,
                    access_token: Secret::new("synthetic-access").unwrap(),
                    lifecycle: crate::provider_auth::TokenLifecycle::OpaqueUntilRejected,
                    organization: None,
                },
            )
            .unwrap();
        owner.store.mark_ready(&account.id).unwrap();
        let mut config = owner.config.clone();
        config.version = 1;
        config.port = port;
        config.selected_account = Some(account.id.clone());
        config.model = "deepseek/synthetic".into();
        config.models = vec![ModelRoute {
            id: config.model.clone(),
            model_id: "synthetic".into(),
            provider: Provider::DeepSeek,
            protocol: crate::provider_server::WireProtocol::Messages,
            url: "https://api.deepseek.com/anthropic/v1/messages".into(),
            headers: vec![],
        }];
        owner = load_legacy_fixture(owner, config);
        owner
            .gateway
            .set_key(Some(Secret::new("synthetic-client").unwrap()))
            .unwrap();
        let mut login = LoginCoordinator::new(
            owner.store.clone(),
            LoginMetadata {
                version: "fixture".into(),
                locale: "en".into(),
                timezone_offset_seconds: 0,
            },
        );
        assert_eq!(
            owner.action(Action::Start, &mut login).unwrap_err(),
            RuntimeError::Busy
        );
        assert_eq!(owner.view.port, port);
        assert_eq!(owner.view.listener, ListenerState::Error);
        assert_eq!(owner.view.actual_port, None);
        assert!(owner.server.is_none());
        let available = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        owner
            .action(Action::SetPort(available), &mut login)
            .unwrap();
        assert_eq!(owner.view.listener, ListenerState::Stopped);
        owner.action(Action::Start, &mut login).unwrap();
        assert_eq!(owner.view.actual_port, Some(available));
        owner.stop();
        assert!(TcpListener::bind(("127.0.0.1", available)).is_ok());
    }
    fn seed_ready(root: &Root) -> (Owner, String) {
        let mut owner = Owner::load(root.0.clone()).unwrap();
        let account = owner
            .store
            .insert(
                "Synthetic queue fixture",
                crate::provider_auth::TokenGrant {
                    provider: Provider::DeepSeek,
                    access_token: Secret::new("synthetic-access").unwrap(),
                    lifecycle: crate::provider_auth::TokenLifecycle::OpaqueUntilRejected,
                    organization: None,
                },
            )
            .unwrap();
        owner.store.mark_ready(&account.id).unwrap();
        let mut config = owner.config.clone();
        config.version = 1;
        config.port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        config.selected_account = Some(account.id.clone());
        config.model = "deepseek/synthetic".into();
        config.models = vec![ModelRoute {
            id: config.model.clone(),
            model_id: "synthetic".into(),
            provider: Provider::DeepSeek,
            protocol: crate::provider_server::WireProtocol::Messages,
            url: "https://api.deepseek.com/anthropic/v1/messages".into(),
            headers: vec![],
        }];
        // This fixture represents an already-applied v1 private file. The
        // production v3 publication API must reject a downgrade/new legacy
        // allocation; prepare old bytes before exercising the real loader.
        owner = load_legacy_fixture(owner, config);
        filesystem::write_private_atomic(&root.0.join("client-key"), b"synthetic-client").unwrap();
        owner
            .gateway
            .set_key(Some(Secret::new("synthetic-client").unwrap()))
            .unwrap();
        (owner, account.id)
    }

    fn load_legacy_fixture(owner: Owner, mut config: Config) -> Owner {
        assert!(matches!(config.version, 1 | 2));
        config.default_route = None;
        config.applied_evidence.clear();
        let root = owner.root.clone();
        filesystem::write_private_atomic(
            &root.join("gateway.json"),
            &config.encode_applied().unwrap(),
        )
        .unwrap();
        drop(owner);
        Owner::load(root).unwrap()
    }

    // Supplies an already-successful synthetic prerequisite solely for the
    // publication consumer fault test. It performs no provider inference and
    // is not evidence of the future production paid-probe/Apply consumer.
    fn fixture_verified_default(owner: &Owner, config: &mut Config) {
        use crate::provider_selection::{
            AllocationDraft, DEFAULT_PROFILE, DraftBook, DraftEdit, FrozenRoute,
        };
        use crate::provider_server::{AppliedEvidence, AppliedOrigin, MODEL_ALIAS};
        let allocation = config.resolve_allocation(DEFAULT_PROFILE, None).unwrap();
        let account = allocation.account.to_owned();
        let snapshot = owner.store.snapshot(&account).unwrap();
        let selected = FrozenRoute::from_applied_snapshot(&snapshot, allocation.route).unwrap();
        let mut drafts = DraftBook::default();
        drafts
            .replace(
                DEFAULT_PROFILE,
                DraftEdit {
                    label: "Synthetic publication prerequisite".into(),
                    enabled: true,
                    default_alias: MODEL_ALIAS.into(),
                    allocations: vec![AllocationDraft {
                        alias: MODEL_ALIAS.into(),
                        selected,
                    }],
                },
            )
            .unwrap();
        let ticket = drafts
            .begin_probe(DEFAULT_PROFILE, MODEL_ALIAS, &snapshot)
            .unwrap();
        drafts.accept_probe(ticket, &snapshot, 1).unwrap();
        let proof = drafts
            .verification(DEFAULT_PROFILE, MODEL_ALIAS, &snapshot)
            .unwrap()
            .unwrap()
            .persisted();
        config
            .applied_evidence
            .retain(|entry| entry.profile != DEFAULT_PROFILE || entry.alias != MODEL_ALIAS);
        config.applied_evidence.push(AppliedEvidence {
            profile: DEFAULT_PROFILE.into(),
            alias: MODEL_ALIAS.into(),
            account,
            origin: AppliedOrigin::Inference {
                proof: Box::new(proof),
            },
        });
    }
    #[test]
    fn reconnect_action_and_cancel_keep_the_applied_connection_and_existing_grant() {
        struct AwaitCancel(std::sync::mpsc::SyncSender<()>);
        impl crate::provider_login::AuthTransport for AwaitCancel {
            fn request(
                &self,
                request: &minicon_platform::network_http::NetworkHttpRequest,
                cancel: &NetworkHttpCancel,
            ) -> Result<
                minicon_platform::network_http::NetworkHttpResponse,
                crate::provider_auth::AuthError,
            > {
                assert!(request.url.ends_with("/auth_init"));
                self.0.send(()).unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                while !cancel.is_cancelled() {
                    assert!(
                        Instant::now() < deadline,
                        "reconnect cancellation did not reach auth transport"
                    );
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(crate::provider_auth::AuthError::Cancelled)
            }
        }
        let root = Root::new();
        let (mut owner, account) = seed_ready(&root);
        let before = owner.store.snapshot(&account).unwrap();
        let config = serde_json::to_value(&owner.config).unwrap();
        let (entered, incoming) = mpsc::sync_channel(1);
        let mut login = LoginCoordinator::with_transport(
            owner.store.clone(),
            LoginMetadata {
                version: "fixture".into(),
                locale: "en-US".into(),
                timezone_offset_seconds: 0,
            },
            Arc::new(AwaitCancel(entered)),
        );
        assert!(
            owner
                .action(Action::Reconnect(account.clone()), &mut login)
                .unwrap()
                .is_none()
        );
        incoming.recv_timeout(Duration::from_secs(5)).unwrap();
        let attempt = owner.view.auth_attempt.unwrap();
        assert!(owner.view.login_active);
        assert!(owner.view.busy);
        assert_eq!(serde_json::to_value(&owner.config).unwrap(), config);
        assert_eq!(owner.store.snapshot(&account).unwrap(), before);
        owner.action(Action::CancelLogin, &mut login).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let event = loop {
            if let Some(event) = login.poll() {
                break event;
            }
            assert!(
                Instant::now() < deadline,
                "no reconnect cancellation result"
            );
            std::thread::sleep(Duration::from_millis(2));
        };
        assert!(
            matches!(&event, LoginEvent::Cancelled { attempt: current } if *current == attempt)
        );
        assert!(
            matches!(owner.login_event(event), Some(Effect::AuthFinished(current)) if current == attempt)
        );
        assert_eq!(
            owner.view.auth_result,
            Some((attempt, AuthOutcome::Cancelled))
        );
        assert!(!owner.view.login_active);
        assert_eq!(owner.store.snapshot(&account).unwrap(), before);
        assert_eq!(serde_json::to_value(&owner.config).unwrap(), config);
        assert_eq!(
            owner
                .store
                .load_grant(&account)
                .unwrap()
                .access_token
                .expose(),
            "synthetic-access"
        );
        login.shutdown(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn rejected_logout_does_not_disable_the_account() {
        let root = Root::new();
        let (mut owner, account) = seed_ready(&root);
        let mut login = LoginCoordinator::new(
            owner.store.clone(),
            LoginMetadata {
                version: "fixture".into(),
                locale: "en".into(),
                timezone_offset_seconds: 0,
            },
        );
        login.refresh().unwrap();
        assert_eq!(
            owner
                .action(Action::RemoveAccount(account.clone()), &mut login)
                .unwrap_err(),
            RuntimeError::Busy
        );
        assert!(owner.store.load_grant(&account).is_ok());
        assert!(owner.gateway.start(owner.config.port).is_ok());
        login.shutdown(Duration::from_secs(5)).unwrap();
    }
    #[test]
    fn stop_invalidates_already_queued_start_without_discarding_later_actions() {
        let root = Root::new();
        let (owner, _) = seed_ready(&root);
        let previous = owner.config.port;
        drop(owner);
        let (ready, wait_ready) = mpsc::channel();
        let (release, wait_release) = mpsc::channel();
        let gate = Arc::new(Mutex::new(Some(wait_release)));
        let notify = Arc::new(move || {
            if let Some(wait) = gate.lock().unwrap().take() {
                ready.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        });
        let mut runtime = Runtime::new(
            root.0.clone(),
            LoginMetadata {
                version: "fixture".into(),
                locale: "en".into(),
                timezone_offset_seconds: 0,
            },
            notify,
        )
        .unwrap();
        wait_ready.recv_timeout(Duration::from_secs(5)).unwrap();
        runtime.act(Action::Start).unwrap();
        runtime.act(Action::Stop).unwrap();
        let next = if previous == 65535 {
            previous - 1
        } else {
            previous + 1
        };
        runtime.act(Action::SetPort(next)).unwrap();
        release.send(()).unwrap();
        until(&mut runtime, |view| {
            view.port == next || view.status.is_some()
        });
        assert_eq!(runtime.view().port, next);
        assert_eq!(runtime.view().listener, ListenerState::Stopped);
        assert!(runtime.view().status.is_none());
    }
    #[test]
    fn removal_cancel_and_failure_restore_only_the_still_stored_account() {
        use std::io::{Read, Write};
        let root = Root::new();
        let (mut owner, account) = seed_ready(&root);
        let server = owner.gateway.start(owner.config.port).unwrap();
        let get_binding = || {
            let mut stream = std::net::TcpStream::connect(server.address()).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(4)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(4)))
                .unwrap();
            write!(stream,"GET /v1/minicon/binding HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer synthetic-client\r\nx-minicon-session: removal-stable\r\nContent-Length: 0\r\n\r\n",server.address()).unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            String::from_utf8(bytes).unwrap()
        };
        let before = get_binding();
        assert!(before.starts_with("HTTP/1.1 200"));
        // Controlled coordinator-to-runtime terminal handoff: cancellation can
        // win before commit, and persistence failure can leave the account in
        // place. The actual gateway must retain the original conversation.
        for failed in [false, true] {
            let attempt = crate::provider_login::AttemptId(900 + u64::from(failed));
            owner.removing = Some((attempt, account.clone()));
            owner.gateway.disable_account(&account);
            assert!(get_binding().starts_with("HTTP/1.1 401"));
            let event = if failed {
                LoginEvent::Failed {
                    attempt,
                    error: crate::provider_login::LoginError::Store(
                        crate::provider_accounts::StoreError::Storage,
                    ),
                }
            } else {
                LoginEvent::Cancelled { attempt }
            };
            owner.login_event(event);
            assert_eq!(get_binding(), before);
            assert!(owner.store.load_grant(&account).is_ok());
            assert_eq!(
                owner.config.selected_account.as_deref(),
                Some(account.as_str())
            );
        }
        let attempt = crate::provider_login::AttemptId(902);
        owner.removing = Some((attempt, account.clone()));
        owner.gateway.disable_account(&account);
        owner.store.remove(&account).unwrap();
        owner.login_event(LoginEvent::Cancelled { attempt });
        assert!(!get_binding().starts_with("HTTP/1.1 200"));
        assert!(owner.store.list().unwrap().is_empty());
    }
    #[cfg(windows)]
    #[test]
    fn real_failed_removal_restores_runtime_start_without_recreating_credentials() {
        struct Permissions {
            path: PathBuf,
            original: std::fs::Permissions,
        }
        impl Drop for Permissions {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(&self.path, self.original.clone());
            }
        }
        let root = Root::new();
        let (owner, account) = seed_ready(&root);
        drop(owner);
        let mut runtime = root.runtime();
        until(&mut runtime, |view| !view.busy);
        let path = root.0.join("accounts/accounts.json");
        let permissions = Permissions {
            original: std::fs::metadata(&path).unwrap().permissions(),
            path,
        };
        let mut readonly = permissions.original.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(&permissions.path, readonly).unwrap();
        runtime.act(Action::RemoveAccount(account.clone())).unwrap();
        // Windows atomic publication deliberately retries a real readonly
        // destination 400 times. Parallel test scheduling can push that
        // ~4-second failure path beyond the ordinary 5-second view deadline.
        until_with_timeout(&mut runtime, Duration::from_secs(15), |view| {
            !view.login_active && view.status.is_some()
        });
        assert!(runtime.view().status.as_ref().unwrap().contains("Storage"));
        assert!(
            runtime
                .view()
                .accounts
                .iter()
                .any(|entry| entry.id == account)
        );
        runtime.act(Action::Start).unwrap();
        until(&mut runtime, |view| {
            matches!(
                view.listener,
                ListenerState::Listening | ListenerState::Error
            )
        });
        assert_eq!(runtime.view().listener, ListenerState::Listening);
        drop(runtime);
        drop(permissions);
        let store = AccountStore::new(root.0.join("accounts")).unwrap();
        assert!(store.load_grant(&account).is_ok());
    }
}
