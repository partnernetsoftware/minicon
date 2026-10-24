//! Process-owned Harness runtime. GUI methods only exchange bounded messages;
//! filesystem, login and listener work belongs to the background owner.

pub use crate::provider_accounts::AccountSummary;
pub use crate::provider_auth::Provider;
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
    time::Duration,
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
pub struct View {
    pub port: u16,
    pub actual_port: Option<u16>,
    pub accounts: Vec<AccountSummary>,
    pub selected_account: Option<String>,
    pub model: String,
    pub models: Vec<String>,
    pub listener: ListenerState,
    pub busy: bool,
    pub login_active: bool,
    pub local_key_exists: bool,
    pub status: Option<String>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            port: 28789,
            actual_port: None,
            accounts: Vec::new(),
            selected_account: None,
            model: String::new(),
            models: Vec::new(),
            listener: ListenerState::Stopped,
            busy: true,
            login_active: false,
            local_key_exists: false,
            status: None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Action {
    Login(Provider),
    CancelLogin,
    RefreshAccounts,
    RefreshModels,
    SelectAccount(String),
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
    OpenBrowser(Secret),
    CopyClientKey(Secret),
    DeviceChallenge {
        verification_url: Secret,
        user_code: Secret,
        expires_in: Duration,
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
    match event {
        LoginEvent::Challenge {
            challenge: LoginChallenge::BrowserUrl { url },
            ..
        } => return Some(Effect::OpenBrowser(url)),
        LoginEvent::Challenge {
            challenge:
                LoginChallenge::Device {
                    verification_url,
                    user_code,
                    expires_in,
                },
            ..
        } => {
            return Some(Effect::DeviceChallenge {
                verification_url,
                user_code,
                expires_in,
            });
        }
        LoginEvent::CredentialStored {
            account,
            completion_url,
            ..
        } => {
            view.accounts.retain(|entry| entry.id != account.id);
            view.accounts.push(account);
            view.status =
                Some("Credential stored; Test model is required before readiness.".into());
            view.login_active = false;
            view.busy = false;
            return completion_url.map(Effect::OpenBrowser);
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
        LoginEvent::Cancelled { .. } => view.status = Some("Login cancelled.".into()),
        LoginEvent::Failed { error, .. } => view.status = Some(error.to_string()),
    }
    view.login_active = false;
    view.busy = false;
    if let Ok(accounts) = store.list() {
        view.accounts = accounts;
    }
    None
}

use crate::provider_server::{Config, Gateway, ModelRoute, ServerError};
use minicon_platform::{
    filesystem, network_http_stream::NetworkHttpCancel, network_loopback::Server,
};
use std::path::Path;

enum OperationResult {
    Catalog(Vec<ModelRoute>),
    Tested,
}
struct Operation {
    cancel: Arc<NetworkHttpCancel>,
    worker: JoinHandle<Result<OperationResult, RuntimeError>>,
    account: String,
}
struct Owner {
    root: PathBuf,
    store: Arc<AccountStore>,
    config: Config,
    key: Option<Secret>,
    gateway: Arc<Gateway>,
    server: Option<Server>,
    operation: Option<Operation>,
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
        let mut owner = Self {
            root,
            store,
            config,
            key,
            gateway,
            server: None,
            operation: None,
            removing: None,
            view: View::default(),
        };
        owner.refresh_view()?;
        Ok(owner)
    }
    fn refresh_view(&mut self) -> Result<(), RuntimeError> {
        self.view.accounts = self.store.list().map_err(|_| RuntimeError::Storage)?;
        self.view.port = self.config.port;
        self.view.selected_account = self.config.selected_account.clone();
        self.view.model = self.config.model.clone();
        self.view.models = self
            .config
            .models
            .iter()
            .map(|route| route.id.clone())
            .collect();
        self.view.local_key_exists = self.key.is_some();
        self.view.busy = self.operation.is_some() || self.view.login_active;
        Ok(())
    }
    fn save(&mut self, config: Config) -> Result<(), RuntimeError> {
        config.validate().map_err(map_server)?;
        let bytes = serde_json::to_vec(&config).map_err(|_| RuntimeError::Corrupt)?;
        filesystem::write_private_atomic(&self.root.join("gateway.json"), &bytes)
            .map_err(|_| RuntimeError::Storage)?;
        self.gateway.update(config.clone()).map_err(map_server)?;
        self.config = config;
        self.refresh_view()
    }
    fn cancel_operation(&mut self) {
        if let Some(operation) = self.operation.take() {
            operation.cancel.cancel();
            let _ = operation.worker.join();
        }
        self.view.busy = self.view.login_active;
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
        let result = match result {
            Ok(OperationResult::Catalog(models))
                if self.config.selected_account.as_deref() == Some(&operation.account) =>
            {
                let mut config = self.config.clone();
                config.models = models;
                if !config.models.iter().any(|route| route.id == config.model) {
                    config.model.clear();
                }
                self.save(config)
            }
            Ok(OperationResult::Catalog(_)) => Err(RuntimeError::InvalidAction),
            Ok(OperationResult::Tested) => {
                self.view.status = Some("Model authorization verified.".into());
                Ok(())
            }
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            self.view.status = Some(error.to_string());
        }
        if let Err(error) = self.refresh_view() {
            self.view.status = Some(error.to_string());
        }
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
        let route = if probe {
            Some(
                self.config
                    .models
                    .iter()
                    .find(|route| route.id == self.config.model)
                    .cloned()
                    .ok_or(RuntimeError::MissingModel)?,
            )
        } else {
            None
        };
        let gateway = self.gateway.clone();
        let identity = self.config.identity.clone();
        let cancel = Arc::new(NetworkHttpCancel::new());
        let network = cancel.clone();
        let selected = account.clone();
        let worker = std::thread::Builder::new()
            .name("minicon-provider-operation".into())
            .spawn(move || {
                if let Some(route) = route {
                    gateway
                        .probe(&selected, &route, &identity, &network)
                        .map_err(map_server)?;
                    Ok(OperationResult::Tested)
                } else {
                    gateway
                        .catalog(&selected, &network)
                        .map(OperationResult::Catalog)
                        .map_err(map_server)
                }
            })
            .map_err(|_| RuntimeError::WorkerUnavailable)?;
        self.operation = Some(Operation {
            cancel,
            worker,
            account,
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
        login_event(event, &mut self.view, &self.store)
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
            Action::Login(provider) => {
                if self.operation.is_some() {
                    return Err(RuntimeError::Busy);
                }
                login
                    .start(
                        provider,
                        match provider {
                            Provider::DeepSeek => "DeepSeek",
                            Provider::OpenCode => "OpenCode",
                        }
                        .into(),
                    )
                    .map_err(|_| RuntimeError::Busy)?;
                self.view.login_active = true;
            }
            Action::RefreshAccounts => self.refresh_view()?,
            Action::RefreshModels => self.begin_operation(false)?,
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
                config.models.clear();
                config.model.clear();
                self.save(config)?;
            }
            Action::SelectModel(id) => {
                if self.operation.is_some() {
                    return Err(RuntimeError::Busy);
                }
                if !self.config.models.iter().any(|route| route.id == id) {
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
                        Action::Start | Action::RefreshModels | Action::TestModel
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
    match filesystem::read_private_bounded(&root.join("client-key"), 256) {
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
fn map_server(error: ServerError) -> RuntimeError {
    match error {
        ServerError::MissingAccount => RuntimeError::MissingAccount,
        ServerError::MissingModel => RuntimeError::MissingModel,
        ServerError::MissingKey => RuntimeError::MissingClientKey,
        ServerError::NeedsLogin => RuntimeError::NeedsLogin,
        ServerError::Busy => RuntimeError::Busy,
        ServerError::Storage => RuntimeError::Storage,
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
        let deadline = Instant::now() + Duration::from_secs(5);
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
        until(&mut runtime, |view| {
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
