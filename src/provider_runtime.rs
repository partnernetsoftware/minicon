//! Process-owned Harness runtime. GUI methods only exchange bounded messages;
//! filesystem, login and listener work belongs to the background owner.

pub use crate::provider_accounts::{
    AccountEdit, AccountPreferences, AccountSummary, NetworkPolicy,
};
pub use crate::provider_auth::Provider;
pub use crate::provider_login::AttemptId;
use crate::provider_selection::{CatalogueCache, CatalogueRequest, SelectionError};
pub use crate::provider_selection::{CatalogueModel, CatalogueStatus};
use crate::provider_server::CatalogueResult;
use crate::{
    provider_accounts::AccountStore,
    provider_auth::Secret,
    provider_login::{LoginChallenge, LoginCoordinator, LoginEvent, LoginMetadata},
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
    pub listener: ListenerState,
    pub busy: bool,
    pub login_active: bool,
    pub auth_attempt: Option<AttemptId>,
    pub auth_result: Option<(AttemptId, AuthOutcome)>,
    pub local_key_exists: bool,
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
            listener: ListenerState::Stopped,
            busy: true,
            login_active: false,
            auth_attempt: None,
            auth_result: None,
            local_key_exists: false,
            config_durability_unconfirmed: false,
            status: None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Action {
    Connect {
        provider: Provider,
        label: String,
        login: NetworkPolicy,
        service: NetworkPolicy,
    },
    Reconnect(String),
    CancelLogin,
    RefreshAccounts,
    RefreshModels,
    RefreshAccountModels(String),
    SelectAccount(String),
    SaveAccount {
        request: u64,
        edit: Box<AccountEdit>,
    },
    SelectModel(String),
    SetPort(u16),
    TestModel,
    Start,
    Stop,
    RemoveAccount(String),
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
    CopyClientKey(Secret),
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
    view: Option<View>,
    effects: VecDeque<Effect>,
    last_view: Option<View>,
}

pub struct Runtime {
    commands: SyncSender<(u64, Action)>,
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
            pending,
            view: View::default(),
            exit,
            stop_requested,
            worker: Some(worker),
        })
    }

    pub fn act(&mut self, action: Action) -> Result<(), RuntimeError> {
        if matches!(action, Action::Stop) {
            self.stop_requested.fetch_add(1, Ordering::AcqRel);
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
        self.commands
            .try_send((self.stop_requested.load(Ordering::Acquire), action))
            .map_err(|error| match error {
                TrySendError::Full(_) => RuntimeError::Busy,
                TrySendError::Disconnected(_) => RuntimeError::WorkerUnavailable,
            })
    }

    /// Applies already-produced metadata/effects. No store or transport calls.
    pub fn poll(&mut self) -> Vec<Effect> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(view) = pending.view.take() {
            self.view = view;
        }
        pending.effects.drain(..).collect()
    }

    pub fn view(&self) -> &View {
        &self.view
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
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
    Tested,
}

pub const CONFIG_DURABILITY_WARNING: &str = "Settings saved and applied; disk durability could not be confirmed. Save again when storage is available.";
struct Operation {
    cancel: Arc<NetworkHttpCancel>,
    worker: JoinHandle<Result<OperationResult, RuntimeError>>,
    account: String,
    catalogue: Option<CatalogueRequest>,
}
struct Owner {
    root: PathBuf,
    store: Arc<AccountStore>,
    config: Config,
    key: Option<Secret>,
    gateway: Arc<Gateway>,
    server: Option<Server>,
    operation: Option<Operation>,
    catalogues: CatalogueCache,
    catalogue_errors: std::collections::BTreeMap<String, RuntimeError>,
    removing: Option<(crate::provider_login::AttemptId, String)>,
    view: View,
}
impl Owner {
    fn load(root: PathBuf) -> Result<Self, RuntimeError> {
        prepare_root(&root)?;
        let config = match filesystem::read_private_bounded(&root.join("gateway.json"), 1024 * 1024)
        {
            Ok(bytes) => {
                serde_json::from_slice::<Config>(&bytes).map_err(|_| RuntimeError::Corrupt)?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Config::fresh().map_err(map_server)?
            }
            Err(_) => return Err(RuntimeError::Storage),
        };
        config.validate().map_err(map_server)?;
        let key = read_key(&root)?;
        let store = Arc::new(
            AccountStore::new(root.join("accounts")).map_err(|_| RuntimeError::InvalidRoot)?,
        );
        let gateway = Gateway::new(store.clone(), config.clone(), clone_secret(key.as_ref())?)
            .map_err(map_server)?;
        for profile in &config.profiles {
            if let Some(key) = read_key_path(&root.join(format!("client-key-{}", profile.id)))? {
                gateway
                    .set_client_key(&profile.id, Some(key))
                    .map_err(map_server)?;
            }
        }
        let mut owner = Self {
            root,
            store,
            config,
            key,
            gateway,
            server: None,
            operation: None,
            catalogues: CatalogueCache::default(),
            catalogue_errors: Default::default(),
            removing: None,
            view: View::default(),
        };
        owner.refresh_view()?;
        owner.view.load_state = LoadState::Loaded;
        Ok(owner)
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
        self.view.local_key_exists = self.key.is_some();
        self.view.busy = self.operation.is_some() || self.view.login_active;
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
        config.validate().map_err(map_server)?;
        let bytes = serde_json::to_vec(&config).map_err(|_| RuntimeError::Corrupt)?;
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
    fn cancel_operation(&mut self) {
        if let Some(operation) = self.operation.take() {
            operation.cancel.cancel();
            let _ = operation.worker.join();
            if let Some(ticket) = &operation.catalogue {
                let _ = self.catalogues.fail(ticket);
                self.catalogue_errors
                    .insert(operation.account, RuntimeError::Cancelled);
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
        if let Some(ticket) = &operation.catalogue {
            let result = if operation.cancel.is_cancelled() {
                Err(RuntimeError::Cancelled)
            } else {
                match result {
                    Ok(OperationResult::Catalog(result)) => {
                        let store = self.store.clone();
                        store
                            .with_snapshot(&operation.account, |current| {
                                self.catalogues.complete(
                                    ticket,
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
                    Ok(OperationResult::Tested) => Err(RuntimeError::InvalidAction),
                    Err(error) => Err(error),
                }
            };
            match result {
                Ok(()) => {
                    self.catalogue_errors.remove(&operation.account);
                }
                Err(error) => {
                    let _ = self.catalogues.fail(ticket);
                    self.catalogue_errors
                        .insert(operation.account.clone(), error);
                }
            }
        } else {
            match result {
                Ok(OperationResult::Tested) => {
                    self.view.status = Some("Model authorization verified.".into())
                }
                Ok(OperationResult::Catalog(_)) => {
                    self.view.status = Some(RuntimeError::InvalidAction.to_string())
                }
                Err(error) => self.view.status = Some(error.to_string()),
            }
        }
        if let Err(error) = self.refresh_view() {
            self.view.status = Some(error.to_string());
        }
    }
    fn begin_catalogue(&mut self, account: String) -> Result<(), RuntimeError> {
        if self.operation.is_some() || self.view.login_active {
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
            catalogue: Some(ticket),
        });
        self.refresh_connection_view();
        Ok(())
    }

    fn begin_operation(&mut self, probe: bool) -> Result<(), RuntimeError> {
        if self.operation.is_some() || self.view.login_active {
            return Err(RuntimeError::Busy);
        }
        let account = self
            .config
            .selected_account
            .clone()
            .ok_or(RuntimeError::MissingAccount)?;
        if !probe {
            return self.begin_catalogue(account);
        }
        let route = self
            .config
            .models
            .iter()
            .find(|route| route.id == self.config.model)
            .cloned()
            .ok_or(RuntimeError::MissingModel)?;
        let gateway = self.gateway.clone();
        let identity = self.config.identity.clone();
        let cancel = Arc::new(NetworkHttpCancel::new());
        let network = cancel.clone();
        let selected = account.clone();
        let worker = std::thread::Builder::new()
            .name("minicon-provider-operation".into())
            .spawn(move || {
                gateway
                    .probe(&selected, &route, &identity, &network)
                    .map_err(map_server)?;
                Ok(OperationResult::Tested)
            })
            .map_err(|_| RuntimeError::WorkerUnavailable)?;
        self.operation = Some(Operation {
            cancel,
            worker,
            account,
            catalogue: None,
        });
        self.view.busy = true;
        self.view.status = None;
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
        match action {
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
                if self.operation.is_some() {
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
                if self.operation.is_some() || self.view.login_active {
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
            Action::RefreshModels => self.begin_operation(false)?,
            Action::RefreshAccountModels(account) => self.begin_catalogue(account)?,
            Action::TestModel => self.begin_operation(true)?,
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
            Action::SelectAccount(id) => {
                if self.operation.is_some() {
                    return Err(RuntimeError::Busy);
                }
                if !self
                    .store
                    .list()
                    .map_err(|_| RuntimeError::Storage)?
                    .iter()
                    .any(|account| account.id == id)
                {
                    return Err(RuntimeError::MissingAccount);
                }
                let mut config = self.config.clone();
                config.selected_account = Some(id);
                config.model.clear();
                self.save(config)?;
            }
            Action::SelectModel(id) => {
                if self.operation.is_some() {
                    return Err(RuntimeError::Busy);
                }
                if !self.view.models.contains(&id) {
                    return Err(RuntimeError::MissingModel);
                }
                let mut config = self.config.clone();
                config.model = id;
                self.save(config)?;
            }
            Action::Start => {
                if self.server.is_some() || self.operation.is_some() {
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
            Action::CreateClientKey => {
                if self.key.is_some() {
                    return Err(RuntimeError::InvalidAction);
                }
                let key = Secret::new(&format!(
                    "mc_{}{}",
                    crate::provider_server::random_id().map_err(map_server)?,
                    crate::provider_server::random_id().map_err(map_server)?
                ))
                .map_err(|_| RuntimeError::Corrupt)?;
                filesystem::write_private_atomic(
                    &self.root.join("client-key"),
                    key.expose().as_bytes(),
                )
                .map_err(|_| RuntimeError::Storage)?;
                self.gateway
                    .set_key(clone_secret(Some(&key))?)
                    .map_err(map_server)?;
                self.key = Some(key);
            }
            Action::CopyClientKey => {
                return Ok(Some(Effect::CopyClientKey(
                    clone_secret(self.key.as_ref())?.ok_or(RuntimeError::MissingClientKey)?,
                )));
            }
            Action::RevokeClientKey => {
                match std::fs::remove_file(self.root.join("client-key")) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err(RuntimeError::Storage),
                }
                self.gateway.set_key(None).map_err(map_server)?;
                self.key = None;
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
    incoming: mpsc::Receiver<(u64, Action)>,
) {
    let mut owner = match Owner::load(root) {
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
        while let Some(event) = login.poll() {
            let effect = owner.login_event(event);
            publish(&pending, &notify, owner.view.clone(), effect);
        }
        match incoming.recv_timeout(Duration::from_millis(20)) {
            Ok((epoch, action)) => {
                if epoch != stop_requested.load(Ordering::Acquire)
                    && matches!(
                        action,
                        Action::Start
                            | Action::RefreshModels
                            | Action::RefreshAccountModels(_)
                            | Action::TestModel
                    )
                {
                    continue;
                }
                owner.view.status = None;
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
        SelectionError::AccountBound => RuntimeError::Busy,
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
    use super::*;
    use std::{net::TcpListener, time::Instant};
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
        owner.save(config).unwrap();
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
        owner
            .action(Action::SelectAccount(account.id.clone()), &mut login)
            .unwrap();
        assert_eq!(owner.view.selected_account.as_ref(), Some(&account.id));
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
    fn switching_default_account_and_refreshing_catalog_preserves_allocated_routes() {
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
        owner.save(config).unwrap();
        let mut login = LoginCoordinator::new(
            owner.store.clone(),
            LoginMetadata {
                version: "fixture".into(),
                locale: "en".into(),
                timezone_offset_seconds: 0,
            },
        );
        owner
            .action(Action::SelectAccount(other.id.clone()), &mut login)
            .unwrap();
        assert_eq!(owner.config.models.len(), 1);
        assert!(
            owner.view.models.is_empty(),
            "Other provider's cached routes must not be selectable"
        );
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
            catalogue: Some(ticket),
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
        assert!(owner.view.models.is_empty());
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
        owner.save(config).unwrap();
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
        owner
            .action(Action::SelectAccount(accounts[1].id.clone()), &mut login)
            .unwrap();
        let disk = std::fs::read(root.0.join("gateway.json")).unwrap();
        let mut conflicting = original.clone();
        conflicting.url = "https://opencode.ai/inference/go/openai/v1/chat/completions".into();
        let snapshot = owner.store.snapshot(&accounts[1].id).unwrap();
        let ticket = owner.catalogues.begin(&snapshot).unwrap();
        let authority = ticket.authority().clone();
        owner.operation = Some(Operation {
            account: accounts[1].id.clone(),
            catalogue: Some(ticket),
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
        owner.save(config).unwrap();
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
        owner.save(config).unwrap();
        filesystem::write_private_atomic(&root.0.join("client-key"), b"synthetic-client").unwrap();
        owner
            .gateway
            .set_key(Some(Secret::new("synthetic-client").unwrap()))
            .unwrap();
        (owner, account.id)
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
