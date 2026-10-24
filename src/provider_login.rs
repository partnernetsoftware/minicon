//! Background account login for PRD 02.31.01; no UI or CLI dependency.
//! `start`, `poll`, `cancel` and `logout` never wait on disk/network.

use crate::provider_accounts::{AccountStore, AccountSummary, StoreError};
use crate::provider_auth::{
    self as auth, Attempt, AuthClient, AuthError, DevicePoll, Pkce, Provider, Secret, TokenGrant,
};
use minicon_platform::network_http::{self, NetworkHttpRequest, NetworkHttpResponse};
use minicon_platform::network_http_stream::{
    self, NetworkHttpCancel, NetworkHttpStreamControl, NetworkHttpStreamError,
    NetworkHttpStreamRequest,
};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU64, Ordering},
    mpsc::{self, Receiver, SyncSender, TryRecvError},
};
use std::time::{Duration, Instant};

const ACTIVE: u8 = 0;
const CANCELLED: u8 = 1;
const COMMITTING: u8 = 2;
const DONE: u8 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttemptId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoginError {
    Auth(AuthError),
    Store(StoreError),
    Busy,
    WorkerUnavailable,
    CallbackUnavailable,
    CallbackExpired {
        received_requests: u8,
        rejected_requests: u8,
    },
}

impl From<AuthError> for LoginError {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}
impl From<StoreError> for LoginError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}
impl std::fmt::Display for LoginError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CallbackExpired {
                received_requests: 0,
                ..
            } => {
                formatter.write_str("Login expired before the browser returned. Start login again.")
            }
            Self::CallbackExpired {
                rejected_requests: 0,
                ..
            } => formatter.write_str(
                "Login expired while receiving the browser callback. Start login again.",
            ),
            Self::CallbackExpired { .. } => formatter
                .write_str("Login expired after invalid browser callbacks. Start login again."),
            Self::Auth(AuthError::Expired) => {
                formatter.write_str("Login expired. Start login again.")
            }
            _ => write!(formatter, "provider login: {self:?}"),
        }
    }
}
impl std::error::Error for LoginError {}

#[derive(Debug)]
pub enum LoginChallenge {
    BrowserUrl {
        url: Secret,
    },
    Device {
        verification_url: Secret,
        user_code: Secret,
        expires_in: Duration,
    },
}

#[derive(Debug)]
pub enum LoginEvent {
    Challenge {
        attempt: AttemptId,
        challenge: LoginChallenge,
    },
    /// A real token was persisted, but inference has not yet established readiness.
    CredentialStored {
        attempt: AttemptId,
        account: AccountSummary,
        /// Provider-validated completion page, released only after the account
        /// commit succeeds. Never copy this URL into a status or snapshot.
        completion_url: Option<Secret>,
    },
    #[cfg(test)]
    Accounts {
        attempt: AttemptId,
        accounts: Vec<AccountSummary>,
    },
    LoggedOut {
        attempt: AttemptId,
        account_id: String,
        remote_revoked: bool,
    },
    Cancelled {
        attempt: AttemptId,
    },
    Failed {
        attempt: AttemptId,
        error: LoginError,
    },
}

impl LoginEvent {
    fn attempt(&self) -> AttemptId {
        match self {
            Self::Challenge { attempt, .. }
            | Self::CredentialStored { attempt, .. }
            | Self::LoggedOut { attempt, .. }
            | Self::Cancelled { attempt }
            | Self::Failed { attempt, .. } => *attempt,
            #[cfg(test)]
            Self::Accounts { attempt, .. } => *attempt,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancelResult {
    Requested,
    /// Publication has already won its atomic race with cancellation. Poll the
    /// result; do not tell the user cancellation succeeded and later save an account.
    Completing,
    Idle,
}

#[derive(Clone)]
pub struct LoginMetadata {
    pub version: String,
    pub locale: String,
    pub timezone_offset_seconds: i32,
}

/// One finite HTTP exchange; fixtures inject responses without login/network.
pub trait AuthTransport: Send + Sync + 'static {
    /// Must release actual I/O when cancellation is requested. Fixture
    /// transports obey the same rule; a detached blocking request is invalid.
    fn request(
        &self,
        request: &NetworkHttpRequest,
        cancel: &NetworkHttpCancel,
    ) -> Result<NetworkHttpResponse, AuthError>;
}

pub struct PlatformTransport;
impl AuthTransport for PlatformTransport {
    fn request(
        &self,
        request: &NetworkHttpRequest,
        cancel: &NetworkHttpCancel,
    ) -> Result<NetworkHttpResponse, AuthError> {
        let executable = std::env::current_exe().map_err(|_| AuthError::TransportUnavailable)?;
        let streaming = NetworkHttpStreamRequest::new(request.clone(), executable)
            .idle_timeout(request.timeout);
        let mut response = NetworkHttpResponse {
            status: 0,
            headers: Vec::new(),
            body: Vec::new(),
            truncated: false,
        };
        let mut body = Vec::new();
        let outcome = network_http_stream::request_stream(
            &streaming,
            cancel,
            |head| {
                response.status = head.status;
                response.headers = head.headers.clone();
                NetworkHttpStreamControl::Continue
            },
            |chunk| {
                if body.len().saturating_add(chunk.len()) > request.max_response_bytes {
                    return NetworkHttpStreamControl::Stop;
                }
                body.extend_from_slice(chunk);
                NetworkHttpStreamControl::Continue
            },
        )
        .map_err(|error| match error {
            NetworkHttpStreamError::Cancelled => AuthError::Cancelled,
            NetworkHttpStreamError::Http(error) => AuthError::TransportFailure(error.kind()),
        })?;
        response.body = body;
        response.truncated = outcome.stopped;
        Ok(response)
    }
}

struct ActiveAttempt {
    id: AttemptId,
    phase: Arc<AtomicU8>,
    incoming: Receiver<LoginEvent>,
    cancel: NetworkHttpCancel,
}

pub struct LoginCoordinator {
    store: Arc<AccountStore>,
    transport: Arc<dyn AuthTransport>,
    metadata: LoginMetadata,
    generation: Arc<AtomicU64>,
    active: Option<ActiveAttempt>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl LoginCoordinator {
    pub fn new(store: Arc<AccountStore>, metadata: LoginMetadata) -> Self {
        Self::with_transport(store, metadata, Arc::new(PlatformTransport))
    }

    pub fn with_transport(
        store: Arc<AccountStore>,
        metadata: LoginMetadata,
        transport: Arc<dyn AuthTransport>,
    ) -> Self {
        Self {
            store,
            transport,
            metadata,
            generation: Arc::new(AtomicU64::new(0)),
            active: None,
            workers: Vec::new(),
        }
    }

    pub fn start(&mut self, provider: Provider, label: String) -> Result<AttemptId, LoginError> {
        if label.trim().is_empty() || label.len() > 128 || label.chars().any(char::is_control) {
            return Err(AuthError::InvalidInput.into());
        }
        self.spawn(Operation::Login { provider, label })
    }

    #[cfg(test)]
    pub fn refresh(&mut self) -> Result<AttemptId, LoginError> {
        self.spawn(Operation::List)
    }

    pub fn logout(&mut self, account_id: String) -> Result<AttemptId, LoginError> {
        if account_id.len() != 32 || !account_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(StoreError::InvalidAccount.into());
        }
        self.spawn(Operation::Logout { account_id })
    }

    pub fn cancel(&mut self) -> CancelResult {
        let Some(active) = &self.active else {
            return CancelResult::Idle;
        };
        match active
            .phase
            .compare_exchange(ACTIVE, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => {
                self.generation.fetch_add(1, Ordering::AcqRel);
                active.cancel.cancel();
                CancelResult::Requested
            }
            Err(COMMITTING) => CancelResult::Completing,
            Err(CANCELLED) => CancelResult::Requested,
            Err(_) => CancelResult::Idle,
        }
    }

    pub fn poll(&mut self) -> Option<LoginEvent> {
        self.reap_workers();
        let active = self.active.as_ref()?;
        let event = loop {
            let event = match active.incoming.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => LoginEvent::Failed {
                    attempt: active.id,
                    error: LoginError::WorkerUnavailable,
                },
            };
            if event.attempt() != active.id {
                // A stale worker result cannot change the current login view.
                continue;
            }
            if self.generation.load(Ordering::Acquire) != active.id.0 {
                if matches!(event, LoginEvent::Challenge { .. }) {
                    continue;
                }
                break LoginEvent::Cancelled { attempt: active.id };
            }
            if matches!(event, LoginEvent::Challenge { .. })
                && active.phase.load(Ordering::Acquire) == DONE
            {
                // A queued authorization URL is not allowed to reopen login
                // after cancel, expiry or completion while the UI was away.
                continue;
            }
            break event;
        };
        if !matches!(event, LoginEvent::Challenge { .. }) {
            self.active = None;
        }
        Some(event)
    }

    fn spawn(&mut self, operation: Operation) -> Result<AttemptId, LoginError> {
        self.reap_workers();
        if self.active.is_some() {
            return Err(LoginError::Busy);
        }
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let id = AttemptId(generation);
        let phase = Arc::new(AtomicU8::new(ACTIVE));
        let cancel = NetworkHttpCancel::new();
        let (outgoing, incoming) = mpsc::sync_channel(4);
        let job = Job {
            id,
            generation: self.generation.clone(),
            phase: phase.clone(),
            store: self.store.clone(),
            transport: self.transport.clone(),
            metadata: self.metadata.clone(),
            outgoing,
            cancel: cancel.clone(),
        };
        let worker = std::thread::Builder::new()
            .name("minicon-provider-login".into())
            .spawn(move || {
                let result = job.run(operation);
                let event = match result {
                    Ok(event) => event,
                    Err(_) if job.phase.load(Ordering::Acquire) == CANCELLED => {
                        LoginEvent::Cancelled { attempt: id }
                    }
                    Err(LoginError::Auth(AuthError::Cancelled | AuthError::StaleAttempt)) => {
                        LoginEvent::Cancelled { attempt: id }
                    }
                    Err(error) => LoginEvent::Failed { attempt: id, error },
                };
                job.phase.store(DONE, Ordering::Release);
                let _ = job.outgoing.send(event);
            })
            .map_err(|_| LoginError::WorkerUnavailable)?;
        self.workers.push(worker);
        self.active = Some(ActiveAttempt {
            id,
            phase,
            incoming,
            cancel,
        });
        Ok(id)
    }

    fn reap_workers(&mut self) {
        let mut index = 0;
        while index < self.workers.len() {
            if self.workers[index].is_finished() {
                let _ = self.workers.swap_remove(index).join();
            } else {
                index += 1;
            }
        }
    }

    /// Cancel actual HTTP/loopback work and join completed workers within the
    /// caller's shutdown budget. On timeout ownership remains here, allowing a
    /// retry; success never means a detached worker is still running.
    pub fn shutdown(&mut self, timeout: Duration) -> Result<(), LoginError> {
        let _ = self.cancel();
        if let Some(active) = &self.active {
            // Local commit may already have won; remote logout is nevertheless
            // interruptible when the application is closing.
            active.cancel.cancel();
        }
        let started = Instant::now();
        loop {
            self.reap_workers();
            if self.workers.is_empty() {
                self.active = None;
                return Ok(());
            }
            if started.elapsed() >= timeout {
                return Err(LoginError::WorkerUnavailable);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Drop for LoginCoordinator {
    fn drop(&mut self) {
        let _ = self.cancel();
        if let Some(active) = &self.active {
            active.cancel.cancel();
        }
        // Every actual worker is owned until joined. Platform I/O observes the
        // cancellation handle; no request thread is abandoned on GUI exit.
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

enum Operation {
    Login {
        provider: Provider,
        label: String,
    },
    #[cfg(test)]
    List,
    Logout {
        account_id: String,
    },
}

struct Job {
    id: AttemptId,
    generation: Arc<AtomicU64>,
    phase: Arc<AtomicU8>,
    store: Arc<AccountStore>,
    transport: Arc<dyn AuthTransport>,
    metadata: LoginMetadata,
    outgoing: SyncSender<LoginEvent>,
    cancel: NetworkHttpCancel,
}

impl Job {
    fn run(&self, operation: Operation) -> Result<LoginEvent, LoginError> {
        let mut attempt = Attempt::new(self.id.0, Instant::now())?;
        match operation {
            #[cfg(test)]
            Operation::List => {
                self.check(&attempt)?;
                let accounts = self.store.list()?;
                self.check(&attempt)?;
                Ok(LoginEvent::Accounts {
                    attempt: self.id,
                    accounts,
                })
            }
            Operation::Login { provider, label } => {
                let (grant, completion_url) = match provider {
                    Provider::DeepSeek => {
                        let result = self.deepseek(&mut attempt)?;
                        (result.grant, Some(result.completion_url))
                    }
                    Provider::OpenCode => (self.opencode(&mut attempt)?, None),
                };
                self.commit_login(&mut attempt, &label, grant, completion_url)
            }
            Operation::Logout { account_id } => {
                self.begin_commit(&mut attempt)?;
                let grant = self.store.remove(&account_id)?;
                let remote_revoked = if let Some(grant) =
                    grant.filter(|grant| grant.provider == Provider::DeepSeek)
                {
                    let request = auth::deepseek_logout(
                        &AuthClient::available(Provider::DeepSeek),
                        &grant,
                        &self.deepseek_metadata(),
                    )?;
                    self.transport
                        .request(&request, &self.cancel)
                        .is_ok_and(|response| auth::parse_deepseek_logout(&response).is_ok())
                } else {
                    // No inferred OpenCode revocation endpoint. Local credential
                    // removal succeeds independently; remote revocation is unclaimed.
                    false
                };
                Ok(LoginEvent::LoggedOut {
                    attempt: self.id,
                    account_id,
                    remote_revoked,
                })
            }
        }
    }

    fn check(&self, attempt: &Attempt) -> Result<Duration, LoginError> {
        if self.phase.load(Ordering::Acquire) == CANCELLED || self.cancel.is_cancelled() {
            return Err(AuthError::Cancelled.into());
        }
        Ok(attempt.check(self.generation.load(Ordering::Acquire), Instant::now())?)
    }

    fn begin_commit(&self, attempt: &mut Attempt) -> Result<(), LoginError> {
        self.check(attempt)?;
        self.phase
            .compare_exchange(ACTIVE, COMMITTING, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| AuthError::Cancelled)?;
        attempt.commit(self.generation.load(Ordering::Acquire), Instant::now())?;
        Ok(())
    }

    fn commit_login(
        &self,
        attempt: &mut Attempt,
        label: &str,
        grant: TokenGrant,
        completion_url: Option<Secret>,
    ) -> Result<LoginEvent, LoginError> {
        self.begin_commit(attempt)?;
        let account = self.store.insert(label, grant)?;
        Ok(LoginEvent::CredentialStored {
            attempt: self.id,
            account,
            completion_url,
        })
    }

    fn request(
        &self,
        attempt: &Attempt,
        mut request: NetworkHttpRequest,
    ) -> Result<NetworkHttpResponse, LoginError> {
        request.timeout = request.timeout.min(self.check(attempt)?);
        request.headers.push((
            "User-Agent".into(),
            format!("MiniCon/{}", self.metadata.version),
        ));
        network_http::validate(&request).map_err(|_| AuthError::InvalidInput)?;
        let response = self.transport.request(&request, &self.cancel)?;
        self.check(attempt)?;
        Ok(response)
    }

    fn challenge(&self, challenge: LoginChallenge) -> Result<(), LoginError> {
        self.outgoing
            .send(LoginEvent::Challenge {
                attempt: self.id,
                challenge,
            })
            .map_err(|_| AuthError::Cancelled.into())
    }

    fn deepseek_metadata(&self) -> auth::DeepSeekMetadata<'_> {
        // Host settings use BCP 47. DeepSeek currently accepts only these two
        // provider locale values; keep that wire convention at this boundary.
        let language = self
            .metadata
            .locale
            .split(['-', '_'])
            .next()
            .unwrap_or_default();
        auth::DeepSeekMetadata {
            version: &self.metadata.version,
            platform: if cfg!(windows) {
                "desktop-win"
            } else if cfg!(target_os = "macos") {
                "desktop-mac"
            } else {
                "desktop-linux"
            },
            locale: if language.eq_ignore_ascii_case("zh") {
                "zh_CN"
            } else {
                "en_US"
            },
            timezone_offset_seconds: self.metadata.timezone_offset_seconds,
        }
    }

    fn deepseek(&self, attempt: &mut Attempt) -> Result<auth::DeepSeekGrant, LoginError> {
        self.check(attempt)?;
        let listener =
            TcpListener::bind(("127.0.0.1", 0)).map_err(|_| LoginError::CallbackUnavailable)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| LoginError::CallbackUnavailable)?;
        let port = listener
            .local_addr()
            .map_err(|_| LoginError::CallbackUnavailable)?
            .port();
        let proof = Pkce::generate()?;
        let client = AuthClient::available(Provider::DeepSeek);
        let response = self.request(
            attempt,
            auth::deepseek_initialize(&client, &proof, port, &self.deepseek_metadata())?,
        )?;
        let challenge = auth::parse_deepseek_initialize(&response)?;
        attempt.limit_by_provider(self.id.0, Instant::now(), challenge.expires_in)?;
        let result = (|| {
            self.challenge(LoginChallenge::BrowserUrl {
                url: challenge.authorize_url,
            })?;
            let code = self.callback(&listener, port, attempt, &proof)?;
            drop(listener);
            let device = device_id()?;
            let input = auth::DeepSeekExchange {
                code: &code,
                pkce: &proof,
                callback_port: port,
                device_id: &device,
                device_model: std::env::consts::ARCH,
                os_version: std::env::consts::OS,
            };
            let response = self.request(
                attempt,
                auth::deepseek_exchange(&client, &input, &self.deepseek_metadata())?,
            )?;
            Ok(auth::parse_deepseek_exchange(&response)?)
        })();
        if result.is_err()
            && let Ok(mut request) = auth::deepseek_cancel(
                &client,
                &challenge.authorize_id,
                &proof,
                &self.deepseek_metadata(),
            )
        {
            request.timeout = Duration::from_secs(1);
            let _ = self.transport.request(&request, &self.cancel);
        }
        result
    }

    fn callback(
        &self,
        listener: &TcpListener,
        port: u16,
        attempt: &mut Attempt,
        proof: &Pkce,
    ) -> Result<Secret, LoginError> {
        let mut accepted: u8 = 0;
        let mut rejected: u8 = 0;
        loop {
            self.check(attempt).map_err(|error| match error {
                LoginError::Auth(AuthError::Expired) => LoginError::CallbackExpired {
                    received_requests: accepted,
                    rejected_requests: rejected,
                },
                other => other,
            })?;
            match listener.accept() {
                Ok((mut stream, peer)) => {
                    if !peer.ip().is_loopback() {
                        continue;
                    }
                    accepted += 1;
                    let parsed =
                        read_callback(&mut stream, port, || self.check(attempt).map(|_| ()))
                            .and_then(|(state, code)| {
                                Ok(attempt.accept_callback(
                                    self.id.0,
                                    Instant::now(),
                                    proof,
                                    &state,
                                    &code,
                                )?)
                            });
                    let code = match parsed {
                        Ok(code) => Some(code),
                        Err(LoginError::Auth(AuthError::Expired)) => {
                            return Err(LoginError::CallbackExpired {
                                received_requests: accepted,
                                rejected_requests: rejected,
                            });
                        }
                        Err(
                            error
                            @ LoginError::Auth(AuthError::Cancelled | AuthError::StaleAttempt),
                        ) => {
                            return Err(error);
                        }
                        Err(_) => {
                            rejected += 1;
                            None
                        }
                    };
                    let status = if code.is_some() {
                        "200 OK"
                    } else {
                        "400 Bad Request"
                    };
                    let reply = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n"
                    );
                    let _ = stream.write_all(reply.as_bytes());
                    if let Some(code) = code {
                        return Ok(code);
                    }
                    if rejected >= 8 {
                        return Err(AuthError::AuthorizationRejected.into());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => return Err(LoginError::CallbackUnavailable),
            }
        }
    }

    fn opencode(&self, attempt: &mut Attempt) -> Result<TokenGrant, LoginError> {
        let client = AuthClient::available(Provider::OpenCode);
        let response = self.request(attempt, auth::opencode_initialize(&client)?)?;
        let challenge = auth::parse_opencode_initialize(&response)?;
        attempt.limit_by_provider(self.id.0, Instant::now(), challenge.expires_in)?;
        self.challenge(LoginChallenge::Device {
            verification_url: challenge.verification_url,
            user_code: challenge.user_code,
            expires_in: self.check(attempt)?,
        })?;
        let mut interval = challenge.poll_interval;
        loop {
            let wait = Instant::now()
                .checked_add(interval)
                .ok_or(AuthError::InvalidResponse)?;
            while Instant::now() < wait {
                self.check(attempt)?;
                std::thread::sleep(Duration::from_millis(10));
            }
            let response = self.request(
                attempt,
                auth::opencode_poll(&client, &challenge.device_code)?,
            )?;
            match auth::parse_opencode_token(&response, Instant::now())? {
                DevicePoll::Granted(grant) => return Ok(grant),
                DevicePoll::Pending => {}
                DevicePoll::SlowDown => interval = interval.saturating_add(Duration::from_secs(5)),
            }
        }
    }
}

fn read_callback(
    stream: &mut TcpStream,
    port: u16,
    check: impl Fn() -> Result<(), LoginError>,
) -> Result<(String, String), LoginError> {
    stream
        .set_nonblocking(true)
        .map_err(|_| LoginError::CallbackUnavailable)?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut bytes = Vec::new();
    while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
        check()?;
        if Instant::now() >= deadline || bytes.len() >= 8192 {
            return Err(AuthError::InvalidResponse.into());
        }
        let mut buffer = [0_u8; 512];
        match stream.read(&mut buffer) {
            Ok(0) => return Err(AuthError::InvalidResponse.into()),
            Ok(count) => bytes.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(_) => return Err(LoginError::CallbackUnavailable),
        }
    }
    parse_callback(&bytes, port)
}

fn parse_callback(bytes: &[u8], port: u16) -> Result<(String, String), LoginError> {
    if bytes.len() > 8192 {
        return Err(AuthError::InvalidResponse.into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| AuthError::InvalidResponse)?;
    let mut lines = text.split("\r\n");
    let first: Vec<&str> = lines.next().unwrap_or_default().split(' ').collect();
    if first.len() != 3 || first[0] != "GET" || !matches!(first[2], "HTTP/1.1" | "HTTP/1.0") {
        return Err(AuthError::InvalidResponse.into());
    }
    let mut hosts = 0;
    for line in lines.take_while(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or(AuthError::InvalidResponse)?;
        if name.eq_ignore_ascii_case("host") {
            hosts += 1;
            if value.trim() != format!("127.0.0.1:{port}") {
                return Err(AuthError::InvalidResponse.into());
            }
        }
    }
    if hosts != 1 {
        return Err(AuthError::InvalidResponse.into());
    }
    let (path, query) = first[1].split_once('?').ok_or(AuthError::InvalidResponse)?;
    if path != "/oauth/callback" || query.contains('#') {
        return Err(AuthError::InvalidResponse.into());
    }
    let mut state = None;
    let mut code = None;
    for field in query.split('&') {
        let (key, value) = field.split_once('=').ok_or(AuthError::InvalidResponse)?;
        let key = decode_query(key)?;
        let target = match key.as_str() {
            "state" => &mut state,
            "code" => &mut code,
            _ => continue,
        };
        if target.is_some() {
            return Err(AuthError::InvalidResponse.into());
        }
        *target = Some(decode_query(value)?);
    }
    Ok((
        state.ok_or(AuthError::InvalidResponse)?,
        code.ok_or(AuthError::InvalidResponse)?,
    ))
}

fn decode_query(value: &str) -> Result<String, LoginError> {
    if value.len() > 4096 {
        return Err(AuthError::InvalidResponse.into());
    }
    let mut decoded = Vec::new();
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        decoded.push(match byte {
            b'%' => {
                let high = bytes
                    .next()
                    .and_then(|byte| char::from(byte).to_digit(16))
                    .ok_or(AuthError::InvalidResponse)?;
                let low = bytes
                    .next()
                    .and_then(|byte| char::from(byte).to_digit(16))
                    .ok_or(AuthError::InvalidResponse)?;
                ((high << 4) | low) as u8
            }
            b'+' => b' ',
            other => other,
        });
    }
    let decoded = String::from_utf8(decoded).map_err(|_| AuthError::InvalidResponse)?;
    if decoded.is_empty() || decoded.chars().any(char::is_control) {
        return Err(AuthError::InvalidResponse.into());
    }
    Ok(decoded)
}

fn device_id() -> Result<String, AuthError> {
    let mut bytes = minicon_platform::entropy::secure_random_array::<16>()
        .map_err(|_| AuthError::EntropyUnavailable)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn temporary_parent() -> std::path::PathBuf {
        // Resolve the existing host temp ancestry before adding a still-missing
        // account root. The account store must keep its no-follow protection.
        std::fs::canonicalize(std::env::temp_dir()).expect("resolve temporary parent")
    }

    #[test]
    fn callback_requires_loopback_host_exact_path_and_unique_decoded_fields() {
        let valid =
            b"GET /oauth/callback?state=state&code=a%2Bb HTTP/1.1\r\nHost: 127.0.0.1:12345\r\n\r\n";
        assert_eq!(
            parse_callback(valid, 12345).unwrap(),
            ("state".into(), "a+b".into())
        );
        for invalid in [
            "GET /oauth/callback?state=s&state=t&code=c HTTP/1.1\r\nHost: 127.0.0.1:12345\r\n\r\n",
            "GET /oauth/callback?state=s&%73tate=t&code=c HTTP/1.1\r\nHost: 127.0.0.1:12345\r\n\r\n",
            "GET /oauth/callback?state=s&code=c HTTP/1.1\r\nHost: evil.test\r\n\r\n",
            "GET /other?state=s&code=c HTTP/1.1\r\nHost: 127.0.0.1:12345\r\n\r\n",
            "POST /oauth/callback?state=s&code=c HTTP/1.1\r\nHost: 127.0.0.1:12345\r\n\r\n",
        ] {
            assert!(parse_callback(invalid.as_bytes(), 12345).is_err());
        }
    }

    struct HeldTransport {
        entered: SyncSender<()>,
        exited: Arc<std::sync::atomic::AtomicBool>,
    }

    struct DeepSeekInitProbe {
        expected_locale: &'static str,
        reached: Arc<std::sync::atomic::AtomicBool>,
    }

    impl AuthTransport for DeepSeekInitProbe {
        fn request(
            &self,
            request: &NetworkHttpRequest,
            _: &NetworkHttpCancel,
        ) -> Result<NetworkHttpResponse, AuthError> {
            assert_eq!(
                request.url,
                "https://platform.deepseek.com/auth-api/v0/dsh/auth_init"
            );
            assert_eq!(request.max_redirects, 0);
            assert!(network_http::validate(request).is_ok());
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["locale"], self.expected_locale);
            assert_eq!(body["code_challenge_method"], "S256");
            assert_eq!(
                request
                    .headers
                    .iter()
                    .find(|(name, _)| name == "x-client-locale")
                    .map(|(_, value)| value.as_str()),
                Some(self.expected_locale)
            );
            self.reached.store(true, Ordering::Release);
            // Stop at the real dispatch boundary without opening the browser,
            // contacting a provider or accepting/exchanging an account grant.
            Err(AuthError::AuthorizationRejected)
        }
    }

    #[test]
    fn host_bcp47_locales_reach_a_valid_deepseek_initialize_request() {
        for (locale, expected_locale) in [
            ("en-US", "en_US"),
            ("zh-CN", "zh_CN"),
            ("zh-TW", "zh_CN"),
            ("zh-Hant-TW", "zh_CN"),
            ("zh_CN", "zh_CN"),
            ("fr-FR", "en_US"),
            ("", "en_US"),
        ] {
            let root =
                temporary_parent().join(format!("minicon-login-locale-{}", device_id().unwrap()));
            let reached = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let mut login = LoginCoordinator::with_transport(
                Arc::new(AccountStore::new(root.clone()).unwrap()),
                LoginMetadata {
                    version: "0.2.23".into(),
                    locale: locale.into(),
                    timezone_offset_seconds: 28_800,
                },
                Arc::new(DeepSeekInitProbe {
                    expected_locale,
                    reached: reached.clone(),
                }),
            );
            let id = login
                .start(Provider::DeepSeek, "Synthetic locale probe".into())
                .unwrap();
            assert!(
                matches!(next_event(&mut login), LoginEvent::Failed {attempt,error:LoginError::Auth(AuthError::AuthorizationRejected)} if attempt == id)
            );
            assert!(reached.load(Ordering::Acquire));
            login.shutdown(Duration::from_secs(1)).unwrap();
            assert!(!root.exists());
        }
    }

    #[test]
    fn platform_transport_preserves_safe_failure_kind_without_request_secrets() {
        let request = NetworkHttpRequest::new(
            network_http::NetworkHttpMethod::Get,
            "synthetic-private-code-is-not-an-absolute-url",
        );
        let error = PlatformTransport
            .request(&request, &NetworkHttpCancel::new())
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::TransportFailure(network_http::NetworkHttpErrorKind::InvalidUrl)
        );
        assert!(!format!("{error:?} {error}").contains("synthetic-private-code"));
    }

    #[test]
    fn platform_transport_pre_cancel_does_not_open_a_socket() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let cancel = NetworkHttpCancel::new();
        cancel.cancel();
        let request = NetworkHttpRequest::new(
            minicon_platform::network_http::NetworkHttpMethod::Get,
            format!("http://{}/", listener.local_addr().unwrap()),
        )
        .max_redirects(0);
        assert_eq!(
            PlatformTransport.request(&request, &cancel),
            Err(AuthError::Cancelled)
        );
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    }

    impl AuthTransport for HeldTransport {
        fn request(
            &self,
            _: &NetworkHttpRequest,
            cancel: &NetworkHttpCancel,
        ) -> Result<NetworkHttpResponse, AuthError> {
            self.entered.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            while !cancel.is_cancelled() {
                assert!(
                    Instant::now() < deadline,
                    "transport must receive real cancellation"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            self.exited.store(true, Ordering::Release);
            Err(AuthError::Cancelled)
        }
    }

    #[test]
    fn cancel_during_network_interrupts_transport_and_joins_actual_worker() {
        let root = temporary_parent().join(format!("minicon-login-{}", device_id().unwrap()));
        let store = Arc::new(AccountStore::new(root.clone()).unwrap());
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let exited = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let transport = Arc::new(HeldTransport {
            entered: entered_tx,
            exited: exited.clone(),
        });
        let mut login = LoginCoordinator::with_transport(
            store.clone(),
            LoginMetadata {
                version: "0.2.23".into(),
                locale: "en_US".into(),
                timezone_offset_seconds: 0,
            },
            transport,
        );
        let id = login.start(Provider::OpenCode, "Synthetic".into()).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(login.cancel(), CancelResult::Requested);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(event) = login.poll() {
                assert!(matches!(event, LoginEvent::Cancelled { attempt } if attempt == id));
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(store.list().unwrap().is_empty());
        assert!(!root.exists());
        login.shutdown(Duration::from_secs(1)).unwrap();
        assert!(login.workers.is_empty());
        assert!(exited.load(Ordering::Acquire));
    }

    #[test]
    fn drop_interrupts_transport_and_waits_for_the_owned_worker_to_exit() {
        let root = temporary_parent().join(format!("minicon-login-drop-{}", device_id().unwrap()));
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let exited = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut login = LoginCoordinator::with_transport(
            Arc::new(AccountStore::new(root).unwrap()),
            LoginMetadata {
                version: "0.2.23".into(),
                locale: "en_US".into(),
                timezone_offset_seconds: 0,
            },
            Arc::new(HeldTransport {
                entered: entered_tx,
                exited: exited.clone(),
            }),
        );
        login.start(Provider::OpenCode, "Synthetic".into()).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let started = Instant::now();
        drop(login);
        assert!(exited.load(Ordering::Acquire));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn shutdown_timeout_keeps_worker_ownership_until_a_later_successful_join() {
        let root = temporary_parent().join(format!("minicon-login-join-{}", device_id().unwrap()));
        let mut login = LoginCoordinator::new(
            Arc::new(AccountStore::new(root).unwrap()),
            LoginMetadata {
                version: "0.2.23".into(),
                locale: "en_US".into(),
                timezone_offset_seconds: 0,
            },
        );
        let (release, incoming) = mpsc::sync_channel(1);
        login.workers.push(std::thread::spawn(move || {
            incoming.recv_timeout(Duration::from_secs(2)).unwrap();
        }));
        assert_eq!(
            login.shutdown(Duration::ZERO),
            Err(LoginError::WorkerUnavailable)
        );
        assert_eq!(login.workers.len(), 1);
        release.send(()).unwrap();
        login.shutdown(Duration::from_secs(1)).unwrap();
        assert!(login.workers.is_empty());
    }

    struct SequenceTransport(Mutex<std::collections::VecDeque<NetworkHttpResponse>>);
    impl AuthTransport for SequenceTransport {
        fn request(
            &self,
            request: &NetworkHttpRequest,
            cancel: &NetworkHttpCancel,
        ) -> Result<NetworkHttpResponse, AuthError> {
            if cancel.is_cancelled() {
                return Err(AuthError::Cancelled);
            }
            assert_eq!(request.max_redirects, 0);
            assert!(
                request
                    .headers
                    .iter()
                    .any(|(name, value)| name == "User-Agent" && value == "MiniCon/0.2.23")
            );
            self.0
                .lock()
                .unwrap()
                .pop_front()
                .ok_or(AuthError::InvalidState)
        }
    }

    fn next_event(login: &mut LoginCoordinator) -> LoginEvent {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            if let Some(event) = login.poll() {
                return event;
            }
            assert!(Instant::now() < deadline, "bounded synthetic login event");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn device_challenge_uses_the_actual_attempt_deadline_and_can_cancel_immediately() {
        let root = temporary_parent().join(format!(
            "minicon-login-device-expiry-{}",
            device_id().unwrap()
        ));
        let store = Arc::new(AccountStore::new(root.clone()).unwrap());
        let response = NetworkHttpResponse {
            status: 200,
            headers: vec![],
            body: br#"{"device_code":"synthetic-device","user_code":"SYNTHETIC","verification_uri_complete":"/console/device","expires_in":1200,"interval":5}"#.to_vec(),
            truncated: false,
        };
        let mut login = LoginCoordinator::with_transport(
            store.clone(),
            LoginMetadata {
                version: "0.2.23".into(),
                locale: "en-US".into(),
                timezone_offset_seconds: 0,
            },
            Arc::new(SequenceTransport(Mutex::new(
                [response].into_iter().collect(),
            ))),
        );
        let id = login
            .start(Provider::OpenCode, "Synthetic expiry probe".into())
            .unwrap();
        let LoginEvent::Challenge {
            attempt,
            challenge: LoginChallenge::Device { expires_in, .. },
        } = next_event(&mut login)
        else {
            panic!("device challenge expected before polling the provider");
        };
        assert_eq!(attempt, id);
        assert!(expires_in > Duration::ZERO);
        assert!(expires_in <= auth::AUTH_ATTEMPT_TIMEOUT);
        assert!(expires_in < Duration::from_secs(1200));

        let started = Instant::now();
        assert_eq!(login.cancel(), CancelResult::Requested);
        assert!(
            matches!(next_event(&mut login), LoginEvent::Cancelled { attempt } if attempt == id)
        );
        login.shutdown(Duration::from_secs(2)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(login.workers.is_empty());
        assert!(store.list().unwrap().is_empty());
        assert!(!root.exists());
    }

    #[test]
    fn synthetic_device_grant_persists_unready_then_logout_removes_local_credentials() {
        let root =
            temporary_parent().join(format!("minicon-login-success-{}", device_id().unwrap()));
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let store = Arc::new(AccountStore::new(root.clone()).unwrap());
        let responses = [
            br#"{"device_code":"synthetic-device","user_code":"SYNTHETIC","verification_uri_complete":"/console/device","expires_in":60,"interval":1}"#.as_slice(),
            br#"{"access_token":"synthetic-access","refresh_token":"synthetic-refresh","expires_in":600,"org_id":"org_synthetic"}"#.as_slice(),
        ].into_iter().map(|body| NetworkHttpResponse { status: 200, headers: vec![], body: body.to_vec(), truncated: false }).collect();
        let metadata = LoginMetadata {
            version: "0.2.23".into(),
            locale: "en_US".into(),
            timezone_offset_seconds: 0,
        };
        let _offline = LoginCoordinator::new(store.clone(), metadata.clone());
        let mut login = LoginCoordinator::with_transport(
            store.clone(),
            metadata,
            Arc::new(SequenceTransport(Mutex::new(responses))),
        );
        let id = login.start(Provider::OpenCode, "Synthetic".into()).unwrap();
        match next_event(&mut login) {
            LoginEvent::Challenge {
                attempt,
                challenge:
                    LoginChallenge::Device {
                        verification_url,
                        user_code,
                        expires_in,
                    },
            } => {
                assert_eq!(attempt, id);
                assert_eq!(
                    verification_url.expose(),
                    "https://opencode.ai/console/device"
                );
                assert_eq!(user_code.expose(), "SYNTHETIC");
                assert!(expires_in > Duration::ZERO);
                assert!(expires_in <= Duration::from_secs(60));
            }
            other => panic!("expected synthetic challenge: {other:?}"),
        }
        let account = match next_event(&mut login) {
            LoginEvent::CredentialStored {
                attempt,
                account,
                completion_url,
            } => {
                assert_eq!(attempt, id);
                assert!(completion_url.is_none());
                account
            }
            other => panic!("expected synthetic credential: {other:?}"),
        };
        assert_eq!(
            account.state,
            crate::provider_accounts::AccountState::CredentialStored
        );
        assert_eq!(
            store.load_grant(&account.id).unwrap().access_token.expose(),
            "synthetic-access"
        );
        let logout_id = login.logout(account.id.clone()).unwrap();
        assert!(
            matches!(next_event(&mut login), LoginEvent::LoggedOut { attempt, account_id, remote_revoked: false } if attempt == logout_id && account_id == account.id)
        );
        let refresh_id = login.refresh().unwrap();
        assert!(
            matches!(next_event(&mut login), LoginEvent::Accounts { attempt, accounts } if attempt == refresh_id && accounts.is_empty())
        );
        assert_eq!(login.cancel(), CancelResult::Idle);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn incomplete_callback_read_observes_cancellation_and_commit_is_not_reported_cancelled() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        let error =
            read_callback(&mut stream, 12345, || Err(AuthError::Cancelled.into())).unwrap_err();
        assert_eq!(error, LoginError::Auth(AuthError::Cancelled));
        drop(client);
        let root =
            temporary_parent().join(format!("minicon-login-commit-{}", device_id().unwrap()));
        let mut login = LoginCoordinator::new(
            Arc::new(AccountStore::new(root).unwrap()),
            LoginMetadata {
                version: "0.2.23".into(),
                locale: "en_US".into(),
                timezone_offset_seconds: 0,
            },
        );
        let (_sender, incoming) = mpsc::sync_channel(1);
        login.active = Some(ActiveAttempt {
            id: AttemptId(1),
            phase: Arc::new(AtomicU8::new(COMMITTING)),
            incoming,
            cancel: NetworkHttpCancel::new(),
        });
        assert_eq!(login.cancel(), CancelResult::Completing);
        assert_eq!(login.generation.load(Ordering::Acquire), 0);
    }

    #[test]
    fn callback_expiry_distinguishes_absent_incomplete_and_rejected_returns() {
        for (kind, received_requests, rejected_requests, detail) in [
            ("absent", 0, 0, "before the browser returned"),
            ("incomplete", 1, 0, "while receiving the browser callback"),
            ("rejected", 1, 1, "after invalid browser callbacks"),
        ] {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap();
            let root =
                temporary_parent().join(format!("minicon-login-expiry-{}", device_id().unwrap()));
            let (outgoing, _incoming) = mpsc::sync_channel(4);
            let job = Job {
                id: AttemptId(1),
                generation: Arc::new(AtomicU64::new(1)),
                phase: Arc::new(AtomicU8::new(ACTIVE)),
                store: Arc::new(AccountStore::new(root.clone()).unwrap()),
                transport: Arc::new(SequenceTransport(Mutex::new(Default::default()))),
                metadata: LoginMetadata {
                    version: "0.2.23".into(),
                    locale: "en_US".into(),
                    timezone_offset_seconds: 0,
                },
                outgoing,
                cancel: NetworkHttpCancel::new(),
            };
            let _client = if kind != "absent" {
                let mut client = TcpStream::connect(address).unwrap();
                if kind == "incomplete" {
                    client.write_all(b"GET /oauth/callback?state=").unwrap();
                } else {
                    write!(client, "GET /oauth/callback?state=synthetic-wrong-state&code=synthetic-private-code HTTP/1.1\r\nHost: {address}\r\n\r\n").unwrap();
                }
                Some(client)
            } else {
                None
            };
            let proof = Pkce::generate().unwrap();
            let now = Instant::now();
            let mut attempt = Attempt::new(1, now).unwrap();
            attempt
                .limit_by_provider(1, now, Duration::from_secs(1))
                .unwrap();
            let error = job
                .callback(&listener, address.port(), &mut attempt, &proof)
                .unwrap_err();
            assert_eq!(
                error,
                LoginError::CallbackExpired {
                    received_requests,
                    rejected_requests
                }
            );
            let message = error.to_string();
            assert!(message.contains(detail));
            assert!(message.contains("Start login again"));
            assert!(!message.contains("synthetic-private-code"));
            assert!(
                !root.exists(),
                "an expired callback cannot store an account"
            );
        }
    }

    #[test]
    fn queued_challenge_is_not_presented_after_cancellation() {
        let root =
            temporary_parent().join(format!("minicon-login-queued-{}", device_id().unwrap()));
        let mut login = LoginCoordinator::new(
            Arc::new(AccountStore::new(root).unwrap()),
            LoginMetadata {
                version: "0.2.23".into(),
                locale: "en_US".into(),
                timezone_offset_seconds: 0,
            },
        );
        let (sender, incoming) = mpsc::sync_channel(4);
        let id = AttemptId(1);
        login.generation.store(2, Ordering::Release);
        login.active = Some(ActiveAttempt {
            id,
            phase: Arc::new(AtomicU8::new(DONE)),
            incoming,
            cancel: NetworkHttpCancel::new(),
        });
        sender
            .send(LoginEvent::Challenge {
                attempt: id,
                challenge: LoginChallenge::BrowserUrl {
                    url: Secret::new("https://platform.deepseek.com/dsh/authorize?synthetic")
                        .unwrap(),
                },
            })
            .unwrap();
        sender.send(LoginEvent::Cancelled { attempt: id }).unwrap();
        assert!(matches!(login.poll(), Some(LoginEvent::Cancelled { attempt }) if attempt == id));
    }

    #[test]
    fn completion_page_is_released_only_after_successful_account_commit() {
        let root =
            temporary_parent().join(format!("minicon-login-completion-{}", device_id().unwrap()));
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let store = Arc::new(AccountStore::new(root.clone()).unwrap());
        for label in ["", "Synthetic"] {
            let (outgoing, _incoming) = mpsc::sync_channel(4);
            let job = Job {
                id: AttemptId(1),
                generation: Arc::new(AtomicU64::new(1)),
                phase: Arc::new(AtomicU8::new(ACTIVE)),
                store: store.clone(),
                transport: Arc::new(SequenceTransport(Mutex::new(Default::default()))),
                metadata: LoginMetadata {
                    version: "0.2.23".into(),
                    locale: "en_US".into(),
                    timezone_offset_seconds: 0,
                },
                outgoing,
                cancel: NetworkHttpCancel::new(),
            };
            let parsed = auth::parse_deepseek_exchange(&NetworkHttpResponse {
                status:200, headers:vec![], truncated:false,
                body:br#"{"code":0,"data":{"biz_code":0,"biz_data":{"token":"synthetic-access","authorized_url":"https://platform.deepseek.com/dsh/authorized?synthetic=1"}}}"#.to_vec(),
            }).unwrap();
            let result = job.commit_login(
                &mut Attempt::new(1, Instant::now()).unwrap(),
                label,
                parsed.grant,
                Some(parsed.completion_url),
            );
            if label.is_empty() {
                assert!(matches!(
                    result,
                    Err(LoginError::Store(StoreError::InvalidAccount))
                ));
                assert!(store.list().unwrap().is_empty());
            } else {
                let LoginEvent::CredentialStored {
                    account,
                    completion_url: Some(url),
                    ..
                } = result.unwrap()
                else {
                    panic!("committed completion event expected")
                };
                assert_eq!(
                    url.expose(),
                    "https://platform.deepseek.com/dsh/authorized?synthetic=1"
                );
                assert_eq!(store.list().unwrap(), vec![account.clone()]);
                assert_eq!(
                    account.state,
                    crate::provider_accounts::AccountState::CredentialStored
                );
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_events_must_match_the_active_attempt_and_generation() {
        let root = temporary_parent().join(format!("minicon-login-stale-{}", device_id().unwrap()));
        let mut login = LoginCoordinator::new(
            Arc::new(AccountStore::new(root).unwrap()),
            LoginMetadata {
                version: "0.2.23".into(),
                locale: "en_US".into(),
                timezone_offset_seconds: 0,
            },
        );
        let (sender, incoming) = mpsc::sync_channel(4);
        login.generation.store(2, Ordering::Release);
        login.active = Some(ActiveAttempt {
            id: AttemptId(2),
            phase: Arc::new(AtomicU8::new(DONE)),
            incoming,
            cancel: NetworkHttpCancel::new(),
        });
        sender
            .send(LoginEvent::Failed {
                attempt: AttemptId(1),
                error: LoginError::CallbackUnavailable,
            })
            .unwrap();
        sender
            .send(LoginEvent::Accounts {
                attempt: AttemptId(2),
                accounts: vec![],
            })
            .unwrap();
        assert!(matches!(
            login.poll(),
            Some(LoginEvent::Accounts {
                attempt: AttemptId(2),
                ..
            })
        ));

        let (sender, incoming) = mpsc::sync_channel(4);
        login.generation.store(3, Ordering::Release);
        login.active = Some(ActiveAttempt {
            id: AttemptId(2),
            phase: Arc::new(AtomicU8::new(DONE)),
            incoming,
            cancel: NetworkHttpCancel::new(),
        });
        sender
            .send(LoginEvent::Failed {
                attempt: AttemptId(2),
                error: LoginError::CallbackUnavailable,
            })
            .unwrap();
        assert!(matches!(
            login.poll(),
            Some(LoginEvent::Cancelled {
                attempt: AttemptId(2)
            })
        ));
    }
}
