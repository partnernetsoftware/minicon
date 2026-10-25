//! Product-owned local model routing. This module never executes model tools.

use crate::{
    provider_accounts::{
        AccountEdit, AccountPreferences, AccountState, AccountStore, NetworkPolicy,
        OrganizationAuthority, StoreError,
    },
    provider_auth::{self, AuthClient, Provider, Secret, TokenGrant, TokenLifecycle},
    provider_protocol::{self as protocol, Protocol, SseBridge},
    provider_selection::ResolvedAuthority,
};
use minicon_platform::{
    network_http::{NetworkHttpMethod, NetworkHttpRequest, NetworkHttpResponse},
    network_http_stream::{
        self as http, NetworkHttpCancel, NetworkHttpStreamControl as Control,
        NetworkHttpStreamRequest,
    },
    network_loopback::{
        self as local, Handler, Reply, Request, RequestContext, RequestHead, Response,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

pub const MODEL_ALIAS: &str = "minicon-selected";
pub(crate) const CONFIG_BYTES: usize = 1024 * 1024;
const NON_STREAM_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const CATALOG_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_MODELS: usize = 512;
const MAX_SESSIONS: usize = 256;
pub(crate) const MAX_CLIENT_PROFILES: usize = 8;
pub(crate) const MAX_CLIENT_ALLOCATIONS: usize = 32;

pub(crate) struct CatalogueResult {
    pub authority: ResolvedAuthority,
    pub routes: Vec<ModelRoute>,
    pub response_bytes: usize,
}

struct CatalogueBudget {
    deadline: Instant,
    response_bytes: usize,
}
impl CatalogueBudget {
    fn new() -> Self {
        Self {
            deadline: Instant::now() + Duration::from_secs(60),
            response_bytes: 0,
        }
    }
    fn remaining(&self) -> Result<Duration, ServerError> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            Err(ServerError::Upstream)
        } else {
            Ok(remaining)
        }
    }
    fn collect(
        &mut self,
        gateway: &Gateway,
        mut request: NetworkHttpRequest,
        cancel: &NetworkHttpCancel,
        limit: usize,
        network: &NetworkPolicy,
    ) -> Result<NetworkHttpResponse, ServerError> {
        request.timeout = request.timeout.min(self.remaining()?);
        let remaining = CATALOG_BYTES.saturating_sub(self.response_bytes);
        if remaining == 0 {
            return Err(ServerError::Upstream);
        }
        let response = gateway.collect(request, cancel, limit.min(remaining), network)?;
        self.response_bytes = self
            .response_bytes
            .checked_add(response.body.len())
            .ok_or(ServerError::Upstream)?;
        if self.response_bytes > CATALOG_BYTES {
            return Err(ServerError::Upstream);
        }
        Ok(response)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerError {
    InvalidConfig,
    Storage,
    MissingAccount,
    MissingModel,
    MissingKey,
    NeedsLogin,
    Busy,
    BindingChanged,
    Upstream,
    Http(u16),
    Protocol(protocol::ProtocolError),
    Cancelled,
}
impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "model service: {self:?}")
    }
}
impl std::error::Error for ServerError {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum WireProtocol {
    Chat,
    Messages,
}
impl WireProtocol {
    fn protocol(self) -> Protocol {
        match self {
            Self::Chat => Protocol::OpenAiChat,
            Self::Messages => Protocol::AnthropicMessages,
        }
    }
}

/// Catalog-derived, non-secret metadata. Authorization is injected only from
/// the selected grant immediately before the native request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRoute {
    pub id: String,
    pub model_id: String,
    pub provider: Provider,
    pub protocol: WireProtocol,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
}

/// Non-secret allocations. Keys remain separate private files owned by the
/// runtime, and an alias never grants access to the whole provider catalogue.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientAllocation {
    pub alias: String,
    pub account: String,
    pub route: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientProfile {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub default_model: String,
    pub allocations: Vec<ClientAllocation>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u8,
    pub port: u16,
    pub selected_account: Option<String>,
    pub model: String,
    pub identity: String,
    #[serde(default)]
    pub models: Vec<ModelRoute>,
    /// Version 1's selected account/model and client-key remain the default
    /// profile. Additional profiles are admitted by the version 2/3 schemas.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<ClientProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_route: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applied_evidence: Vec<AppliedEvidence>,
}

/// Both outcomes have installed the complete configuration. A durability
/// warning must not leave live routing behind already-published disk state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigPublication {
    Durable,
    DurabilityUnconfirmed,
}

/// Publication fact for a key mutation; NoChange confirms no new barrier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientKeyPublication {
    NoChange,
    Durable,
    DurabilityUnconfirmed,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ClientKeyUpdate {
    pub publication: ClientKeyPublication,
    pub changed: bool,
}

impl Config {
    pub fn fresh() -> Result<Self, ServerError> {
        Ok(Self {
            version: 1,
            port: 28789,
            selected_account: None,
            model: String::new(),
            identity: random_id()?,
            models: Vec::new(),
            profiles: Vec::new(),
            default_route: None,
            applied_evidence: Vec::new(),
        })
    }
    pub fn validate(&self) -> Result<(), ServerError> {
        if !matches!(self.version, 1..=3)
            || (self.version == 1 && !self.profiles.is_empty())
            || self.profiles.len() >= MAX_CLIENT_PROFILES
            || self.port < 1024
            || !opaque_id(&self.identity)
            || self
                .selected_account
                .as_ref()
                .is_some_and(|id| !opaque_id(id))
            || !safe_model(&self.model, true)
            || self.models.len() > MAX_MODELS
        {
            return Err(ServerError::InvalidConfig);
        }
        let mut ids = HashSet::new();
        for model in &self.models {
            validate_route(model)?;
            if !ids.insert(&model.id) {
                return Err(ServerError::InvalidConfig);
            }
        }
        if self.selected_account.is_some()
            && !self.model.is_empty()
            && self.default_route.is_none()
            && !ids.contains(&self.model)
        {
            return Err(ServerError::InvalidConfig);
        }
        let mut profiles = HashSet::new();
        for profile in &self.profiles {
            if !opaque_id(&profile.id)
                || !profiles.insert(&profile.id)
                || profile.label.trim().is_empty()
                || profile.label.len() > 128
                || profile.label.chars().any(char::is_control)
                || profile.allocations.len() > MAX_CLIENT_ALLOCATIONS
                || (profile.allocations.is_empty() != profile.default_model.is_empty())
            {
                return Err(ServerError::InvalidConfig);
            }
            let mut aliases = HashSet::new();
            for allocation in &profile.allocations {
                if !safe_model(&allocation.alias, false)
                    || allocation.alias.chars().any(char::is_whitespace)
                    || !aliases.insert(&allocation.alias)
                    || !opaque_id(&allocation.account)
                    || !ids.contains(&allocation.route)
                {
                    return Err(ServerError::InvalidConfig);
                }
            }
            if !profile.allocations.is_empty() && !aliases.contains(&profile.default_model) {
                return Err(ServerError::InvalidConfig);
            }
        }
        self.validate_applied()
    }
}

/// Explicit origin of an existing allocation. A legacy import is not an
/// inference and carries no manufactured credential authority or check time.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AppliedOrigin {
    LegacyApplied {
        snapshot_digest: String,
    },
    Inference {
        proof: Box<crate::provider_selection::InferenceProof>,
    },
}
impl std::fmt::Debug for AppliedOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::LegacyApplied { .. } => "LegacyApplied([REDACTED])",
            Self::Inference { .. } => "Inference([REDACTED])",
        })
    }
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedEvidence {
    pub profile: String,
    pub alias: String,
    pub account: String,
    pub origin: AppliedOrigin,
}

/// Borrowed resolution only; no grant, I/O or mutation. All product routing
/// paths share this interpretation of public aliases and private route keys.
pub(crate) struct ResolvedAllocation<'a> {
    pub account: &'a str,
    pub alias: &'a str,
    pub route: &'a ModelRoute,
    pub evidence: Option<&'a AppliedEvidence>,
}

impl Config {
    pub(crate) fn decode_applied(bytes: &[u8]) -> Result<Self, ServerError> {
        if bytes.len() > CONFIG_BYTES {
            return Err(ServerError::InvalidConfig);
        }
        let config: Self = serde_json::from_slice(bytes).map_err(|_| ServerError::InvalidConfig)?;
        config.upgrade_applied()
    }

    /// In-memory only. No catalogue/account/key reads and no disk rewrite.
    pub(crate) fn upgrade_applied(mut self) -> Result<Self, ServerError> {
        self.validate()?;
        if self.version == 3 {
            return Ok(self);
        }
        let mut evidence = Vec::new();
        if let Ok(allocation) = self.resolve_allocation("default", None) {
            evidence.push(AppliedEvidence {
                profile: "default".into(),
                alias: MODEL_ALIAS.into(),
                account: allocation.account.into(),
                origin: AppliedOrigin::LegacyApplied {
                    snapshot_digest: crate::provider_selection::legacy_snapshot_digest(
                        allocation.route,
                    )
                    .map_err(|_| ServerError::InvalidConfig)?,
                },
            });
        }
        for profile in &self.profiles {
            for allocation in &profile.allocations {
                let resolved = self.resolve_allocation(&profile.id, Some(&allocation.alias))?;
                evidence.push(AppliedEvidence {
                    profile: profile.id.clone(),
                    alias: allocation.alias.clone(),
                    account: allocation.account.clone(),
                    origin: AppliedOrigin::LegacyApplied {
                        snapshot_digest: crate::provider_selection::legacy_snapshot_digest(
                            resolved.route,
                        )
                        .map_err(|_| ServerError::InvalidConfig)?,
                    },
                });
            }
        }
        self.version = 3;
        self.applied_evidence = evidence;
        self.validate()?;
        Ok(self)
    }

    pub(crate) fn encode_applied(&self) -> Result<Vec<u8>, ServerError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| ServerError::InvalidConfig)?;
        if bytes.len() > CONFIG_BYTES {
            return Err(ServerError::InvalidConfig);
        }
        Ok(bytes)
    }

    pub(crate) fn resolve_allocation(
        &self,
        profile: &str,
        requested: Option<&str>,
    ) -> Result<ResolvedAllocation<'_>, ServerError> {
        let (account, alias, key) = if profile == "default" {
            (
                self.selected_account
                    .as_deref()
                    .ok_or(ServerError::MissingAccount)?,
                MODEL_ALIAS,
                self.default_route.as_deref().unwrap_or(&self.model),
            )
        } else {
            let client = self
                .profiles
                .iter()
                .find(|entry| entry.id == profile)
                .ok_or(ServerError::MissingKey)?;
            let alias = requested.unwrap_or(&client.default_model);
            let allocation = client
                .allocations
                .iter()
                .find(|entry| entry.alias == alias)
                .ok_or(ServerError::MissingModel)?;
            (
                allocation.account.as_str(),
                allocation.alias.as_str(),
                allocation.route.as_str(),
            )
        };
        let route = self
            .models
            .iter()
            .find(|entry| entry.id == key)
            .ok_or(ServerError::MissingModel)?;
        if profile == "default"
            && requested.is_some_and(|name| {
                name != MODEL_ALIAS && name != self.model && name != route.model_id
            })
        {
            return Err(ServerError::MissingModel);
        }
        let evidence = self
            .applied_evidence
            .iter()
            .find(|entry| entry.profile == profile && entry.alias == alias);
        if self.version == 3 && evidence.is_none() {
            return Err(ServerError::InvalidConfig);
        }
        Ok(ResolvedAllocation {
            account,
            alias,
            route,
            evidence,
        })
    }

    fn validate_applied(&self) -> Result<(), ServerError> {
        if self.version != 3 {
            return if self.default_route.is_none() && self.applied_evidence.is_empty() {
                Ok(())
            } else {
                Err(ServerError::InvalidConfig)
            };
        }
        if self.default_route.as_ref().is_some_and(|key| {
            self.model.is_empty() || !self.models.iter().any(|route| &route.id == key)
        }) {
            return Err(ServerError::InvalidConfig);
        }
        let mut expected = 0;
        let mut identities = HashSet::new();
        for evidence in &self.applied_evidence {
            if !identities.insert((&evidence.profile, &evidence.alias))
                || !opaque_id(&evidence.account)
            {
                return Err(ServerError::InvalidConfig);
            }
            let allocation = self.resolve_allocation(&evidence.profile, Some(&evidence.alias))?;
            if allocation.account != evidence.account || allocation.alias != evidence.alias {
                return Err(ServerError::InvalidConfig);
            }
            match &evidence.origin {
                AppliedOrigin::LegacyApplied { snapshot_digest } => {
                    if crate::provider_selection::legacy_snapshot_digest(allocation.route)
                        .map_err(|_| ServerError::InvalidConfig)?
                        != *snapshot_digest
                    {
                        return Err(ServerError::InvalidConfig);
                    }
                }
                AppliedOrigin::Inference { proof } => {
                    if !proof.matches_route(
                        &evidence.profile,
                        &evidence.alias,
                        &evidence.account,
                        allocation.route,
                    ) {
                        return Err(ServerError::InvalidConfig);
                    }
                }
            }
        }
        if self.selected_account.is_some() && !self.model.is_empty() {
            self.resolve_allocation("default", None)?;
            expected += 1;
        }
        for profile in &self.profiles {
            for allocation in &profile.allocations {
                self.resolve_allocation(&profile.id, Some(&allocation.alias))?;
                expected += 1;
            }
        }
        if expected != self.applied_evidence.len() {
            return Err(ServerError::InvalidConfig);
        }
        Ok(())
    }

    /// A self-consistent JSON origin cannot establish history. Preserve legacy
    /// only against the previous actual allocation, never on a new selection.
    fn validate_legacy_transition(&self, previous: &Self) -> Result<(), ServerError> {
        if previous.version == 3 && self.version != 3 {
            return Err(ServerError::InvalidConfig);
        }
        if self.version != 3 {
            return Ok(());
        }
        for evidence in &self.applied_evidence {
            let AppliedOrigin::LegacyApplied { snapshot_digest } = &evidence.origin else {
                continue;
            };
            let prior = previous.resolve_allocation(&evidence.profile, Some(&evidence.alias))?;
            if prior.account != evidence.account
                || prior.alias != evidence.alias
                || crate::provider_selection::legacy_snapshot_digest(prior.route)
                    .map_err(|_| ServerError::InvalidConfig)?
                    != *snapshot_digest
                || (evidence.profile == "default" && previous.model != self.model)
                || (previous.version == 3
                    && !matches!(
                        prior.evidence.map(|entry| &entry.origin),
                        Some(AppliedOrigin::LegacyApplied { .. })
                    ))
            {
                return Err(ServerError::InvalidConfig);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Binding {
    pub profile: String,
    pub alias: String,
    pub provider: Provider,
    pub account: String,
    pub model: String,
    pub generation: String,
}

struct Session {
    name: String,
    binding: Binding,
    upstream: String,
}
struct Active {
    account: String,
    profile: Option<String>,
    cancel: Weak<NetworkHttpCancel>,
}
struct ClientAccess {
    key: Secret,
    generation: String,
}
struct State {
    config: Config,
    key: Option<Secret>,
    clients: HashMap<String, ClientAccess>,
    route_generations: HashMap<(String, String), String>,
    sessions: VecDeque<Session>,
    disabled: HashSet<String>,
    leases: HashMap<String, usize>,
    active: Vec<Active>,
    draining: bool,
    generation: String,
}

pub struct Gateway {
    store: Arc<AccountStore>,
    executable: PathBuf,
    state: Mutex<State>,
    refresh: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    #[cfg(test)]
    fixture_upstream: Mutex<Option<String>>,
    #[cfg(test)]
    fail_update: std::sync::atomic::AtomicBool,
}

impl Gateway {
    pub fn new(
        store: Arc<AccountStore>,
        config: Config,
        key: Option<Secret>,
    ) -> Result<Arc<Self>, ServerError> {
        Self::with_client_keys(store, config, key, Vec::new())
    }

    pub fn with_client_keys(
        store: Arc<AccountStore>,
        config: Config,
        key: Option<Secret>,
        client_keys: Vec<(String, Secret)>,
    ) -> Result<Arc<Self>, ServerError> {
        config.validate()?;
        let mut route_generations = HashMap::new();
        for profile in &config.profiles {
            for allocation in &profile.allocations {
                route_generations
                    .insert((profile.id.clone(), allocation.alias.clone()), random_id()?);
            }
        }
        let executable = std::env::current_exe().map_err(|_| ServerError::Storage)?;
        let gateway = Arc::new(Self {
            store,
            executable,
            state: Mutex::new(State {
                config,
                key,
                clients: HashMap::new(),
                route_generations,
                sessions: VecDeque::new(),
                disabled: HashSet::new(),
                leases: HashMap::new(),
                active: Vec::new(),
                draining: false,
                generation: random_id()?,
            }),
            refresh: Mutex::new(HashMap::new()),
            #[cfg(test)]
            fixture_upstream: Mutex::new(None),
            #[cfg(test)]
            fail_update: std::sync::atomic::AtomicBool::new(false),
        });
        if client_keys.len() >= MAX_CLIENT_PROFILES {
            return Err(ServerError::InvalidConfig);
        }
        let mut seen = HashSet::new();
        for (profile, key) in client_keys {
            if !seen.insert(profile.clone()) {
                return Err(ServerError::InvalidConfig);
            }
            gateway.set_client_key(&profile, Some(key))?;
        }
        Ok(gateway)
    }

    #[cfg(test)]
    pub fn update(&self, config: Config) -> Result<(), ServerError> {
        self.update_and_publish(config, || Ok(ConfigPublication::Durable))
            .map(|_| ())
    }

    pub fn update_and_publish(
        &self,
        config: Config,
        publish: impl FnOnce() -> Result<ConfigPublication, ServerError>,
    ) -> Result<ConfigPublication, ServerError> {
        config.encode_applied()?;
        #[cfg(test)]
        if self
            .fail_update
            .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            return Err(ServerError::Storage);
        }
        let mut state = self.state.lock().map_err(|_| ServerError::Storage)?;
        config.validate_legacy_transition(&state.config)?;
        let selected = |config: &Config| {
            config
                .resolve_allocation("default", None)
                .ok()
                .map(|allocation| {
                    (
                        allocation.account.to_owned(),
                        model_transport(allocation.route),
                    )
                })
        };
        let generation = if selected(&state.config) != selected(&config) {
            random_id()?
        } else {
            state.generation.clone()
        };
        let route_identity = |config: &Config, profile: &str, alias: &str| {
            let allocation = config.resolve_allocation(profile, Some(alias)).ok()?;
            Some((
                allocation.account.to_owned(),
                model_transport(allocation.route),
            ))
        };
        let mut generations = HashMap::new();
        for profile in &config.profiles {
            for allocation in &profile.allocations {
                let identity = (profile.id.clone(), allocation.alias.clone());
                let generation = if route_identity(&state.config, &profile.id, &allocation.alias)
                    == route_identity(&config, &profile.id, &allocation.alias)
                {
                    state.route_generations.get(&identity).cloned()
                } else {
                    None
                };
                generations.insert(identity, generation.map_or_else(random_id, Ok)?);
            }
        }
        // Validation, locking and entropy can fail; finish them before the
        // runtime publishes bytes. No fallible preparation follows publication.
        let publication = publish()?;
        state.generation = generation;
        state.route_generations = generations;
        state.sessions.retain(|session| {
            session.binding.profile == "default"
                || config
                    .profiles
                    .iter()
                    .any(|profile| profile.id == session.binding.profile)
        });
        state
            .clients
            .retain(|id, _| config.profiles.iter().any(|profile| profile.id == *id));
        for active in &state.active {
            if let Some(profile) = &active.profile
                && profile != "default"
                && !config
                    .profiles
                    .iter()
                    .any(|entry| entry.id == *profile && entry.enabled)
                && let Some(cancel) = active.cancel.upgrade()
            {
                cancel.cancel();
            }
        }
        state.config = config;
        // Keep existing mappings: their old generation must produce 409 rather
        // than silently starting a replacement upstream conversation.
        Ok(publication)
    }

    /// All Config preparation and state locking precedes the one Store
    /// authority lock. No callback can observe a partially prepared Gateway.
    pub(crate) fn update_with_authorities(
        &self,
        config: Config,
        expected: &[crate::provider_accounts::AccountSnapshot],
        publish: impl FnOnce() -> Result<ConfigPublication, ServerError>,
    ) -> Result<ConfigPublication, ServerError> {
        self.update_and_publish(config, || {
            self.store
                .with_current_snapshots(expected, publish)
                .map_err(credential_error)?
        })
    }

    #[cfg(test)]
    pub fn fail_next_update(&self) {
        self.fail_update
            .store(true, std::sync::atomic::Ordering::Release);
    }

    #[cfg(test)]
    pub fn set_key(&self, key: Option<Secret>) -> Result<(), ServerError> {
        self.set_client_key("default", key)
    }

    /// Existing in-memory compatibility setter (startup and existing callers).
    /// Persistent consumers use publish_client_key to join disk and live state.
    pub fn set_client_key(&self, profile: &str, key: Option<Secret>) -> Result<(), ServerError> {
        self.publish_client_key(profile, key, || Ok(ClientKeyPublication::NoChange))
            .map(|_| ())
    }
    pub(crate) fn publish_client_key(
        &self,
        profile: &str,
        key: Option<Secret>,
        publish: impl FnOnce() -> Result<ClientKeyPublication, ServerError>,
    ) -> Result<ClientKeyUpdate, ServerError> {
        self.publish_client_key_with_generation(profile, key, publish, random_id)
    }
    fn publish_client_key_with_generation(
        &self,
        profile: &str,
        key: Option<Secret>,
        publish: impl FnOnce() -> Result<ClientKeyPublication, ServerError>,
        generation: impl FnOnce() -> Result<String, ServerError>,
    ) -> Result<ClientKeyUpdate, ServerError> {
        let mut state = self.state.lock().map_err(|_| ServerError::Storage)?;
        if profile != "default"
            && !state
                .config
                .profiles
                .iter()
                .any(|entry| entry.id == profile)
        {
            return Err(ServerError::InvalidConfig);
        }
        let unchanged = if profile == "default" {
            state.key == key
        } else {
            state.clients.get(profile).map(|access| &access.key) == key.as_ref()
        };
        if let Some(key) = &key
            && ((profile != "default"
                && state
                    .key
                    .as_ref()
                    .is_some_and(|other| same_key(other.expose(), key.expose())))
                || state
                    .clients
                    .iter()
                    .any(|(id, other)| id != profile && same_key(other.key.expose(), key.expose())))
        {
            return Err(ServerError::InvalidConfig);
        }
        // Prepare all fallible entropy/capacity and owned fields before I/O.
        let generation = if unchanged { None } else { Some(generation()?) };
        let owned_profile = profile.to_owned();
        if !unchanged && profile != "default" && key.is_some() {
            state
                .clients
                .try_reserve(1)
                .map_err(|_| ServerError::Storage)?;
        }
        // Even an unchanged Revoke must observe real intended absence. NoChange
        // is not a new durability confirmation and must not clear old warnings.
        let publication = publish()?;
        if !unchanged {
            let generation = generation.expect("prepared changed-key generation");
            if profile == "default" {
                state.key = key;
                state.generation = generation;
            } else if let Some(key) = key {
                state
                    .clients
                    .insert(owned_profile, ClientAccess { key, generation });
            } else {
                state.clients.remove(profile);
            }
            state
                .sessions
                .retain(|session| session.binding.profile != profile);
            for active in &state.active {
                if active.profile.as_deref() == Some(profile)
                    && let Some(cancel) = active.cancel.upgrade()
                {
                    cancel.cancel();
                }
            }
        }
        Ok(ClientKeyUpdate {
            publication,
            changed: !unchanged,
        })
    }

    /// First close admission, then cancel every currently owned exchange. The
    /// caller joins its operation and listener only after this fan-out.
    pub fn begin_stop(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.draining = true;
            for active in &state.active {
                if let Some(cancel) = active.cancel.upgrade() {
                    cancel.cancel();
                }
            }
        }
    }
    pub fn forget_account(&self, account: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.disabled.remove(account);
        }
        if let Ok(mut refresh) = self.refresh.lock() {
            refresh.remove(account);
        }
    }
    pub fn disable_account(&self, account: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.disabled.insert(account.to_owned());
            for active in &state.active {
                if active.account == account
                    && let Some(cancel) = active.cancel.upgrade()
                {
                    cancel.cancel();
                }
            }
        }
    }

    pub fn update_account(&self, edit: &AccountEdit) -> Result<(), ServerError> {
        edit.login
            .validate()
            .map_err(|_| ServerError::InvalidConfig)?;
        edit.service
            .validate()
            .map_err(|_| ServerError::InvalidConfig)?;
        let state = self.state.lock().map_err(|_| ServerError::Storage)?;
        let account = edit.account.as_str();
        let mut preferences = self
            .store
            .preferences(account)
            .map_err(|_| ServerError::MissingAccount)?;
        let summary = self
            .store
            .list()
            .map_err(|_| ServerError::Storage)?
            .into_iter()
            .find(|account| account.id == edit.account)
            .ok_or(ServerError::MissingAccount)?;
        if preferences != edit.expected || summary.label != edit.expected_label {
            return Err(ServerError::BindingChanged);
        }
        if preferences.service != edit.service || preferences.enabled != edit.enabled {
            preferences.service_generation = random_id()?;
        }
        preferences.login = edit.login.clone();
        preferences.service = edit.service.clone();
        preferences.enabled = edit.enabled;
        self.store
            .update_preferences(account, &edit.label, preferences)
            .map_err(|error| match error {
                crate::provider_accounts::StoreError::InvalidAccount => ServerError::InvalidConfig,
                crate::provider_accounts::StoreError::Missing => ServerError::MissingAccount,
                _ => ServerError::Storage,
            })?;
        if !edit.enabled {
            for active in &state.active {
                if active.account == account
                    && let Some(cancel) = active.cancel.upgrade()
                {
                    cancel.cancel();
                }
            }
        }
        Ok(())
    }

    pub fn start(self: &Arc<Self>, port: u16) -> Result<local::Server, ServerError> {
        let state = self.state.lock().map_err(|_| ServerError::Storage)?;
        let mut candidates = Vec::new();
        if state.key.is_some() {
            candidates.push(("default", None));
        }
        for profile in &state.config.profiles {
            if profile.enabled && state.clients.contains_key(&profile.id) {
                for allocation in &profile.allocations {
                    candidates.push((profile.id.as_str(), Some(allocation.alias.as_str())));
                }
            }
        }
        // An unfinished default profile cannot prevent another complete
        // profile from starting the shared listener.
        let mut ready = false;
        let mut error = self
            .binding_for(&state, "default", None)
            .err()
            .unwrap_or(ServerError::MissingKey);
        for (profile, alias) in candidates {
            match self
                .binding_for(&state, profile, alias)
                .and_then(|(binding, _)| {
                    self.store
                        .load_grant(&binding.account)
                        .map_err(|_| ServerError::NeedsLogin)
                }) {
                Ok(_) => {
                    ready = true;
                    break;
                }
                Err(failure) => error = failure,
            }
        }
        if !ready {
            return Err(error);
        }
        drop(state);
        let server = local::Server::start(port, self.clone()).map_err(|_| ServerError::Busy)?;
        self.state
            .lock()
            .map_err(|_| ServerError::Storage)?
            .draining = false;
        Ok(server)
    }

    fn binding_for(
        &self,
        state: &State,
        profile: &str,
        requested: Option<&str>,
    ) -> Result<(Binding, ModelRoute), ServerError> {
        let allocation = state.config.resolve_allocation(profile, requested)?;
        let account = allocation.account;
        let alias = allocation.alias;
        let generation = if profile == "default" {
            state.generation.clone()
        } else {
            if !state
                .config
                .profiles
                .iter()
                .any(|entry| entry.id == profile && entry.enabled)
            {
                return Err(ServerError::MissingKey);
            }
            let access = state.clients.get(profile).ok_or(ServerError::MissingKey)?;
            let generation = state
                .route_generations
                .get(&(profile.to_owned(), alias.to_owned()))
                .ok_or(ServerError::InvalidConfig)?;
            format!("{}-{generation}", access.generation)
        };
        if state.disabled.contains(account) {
            return Err(ServerError::NeedsLogin);
        }
        let snapshot = self.store.snapshot(account).map_err(credential_error)?;
        let summary = &snapshot.summary;
        if summary.state != AccountState::AccountReady {
            return Err(ServerError::NeedsLogin);
        }
        let preferences = &snapshot.preferences;
        if !preferences.enabled {
            return Err(ServerError::NeedsLogin);
        }
        let route = allocation.route.clone();
        if route.provider != summary.provider {
            return Err(ServerError::MissingModel);
        }
        if let Some(AppliedEvidence {
            origin: AppliedOrigin::Inference { proof },
            ..
        }) = allocation.evidence
            && !proof.matches_current(&snapshot)
        {
            return Err(ServerError::BindingChanged);
        }
        Ok((
            Binding {
                profile: profile.to_owned(),
                alias: alias.to_owned(),
                provider: summary.provider,
                account: account.to_owned(),
                model: route.model_id.clone(),
                generation: format!(
                    "{generation}:{}:{}:{}:{}",
                    preferences.service_generation,
                    snapshot.generation,
                    snapshot.organization.epoch,
                    snapshot.organization.digest
                ),
            },
            route,
        ))
    }

    fn sticky(
        &self,
        head: &RequestHead,
        requested: Option<&str>,
    ) -> Result<(Binding, ModelRoute, String), ServerError> {
        let name = head
            .header("x-minicon-session")
            .map_err(|_| ServerError::InvalidConfig)?
            .unwrap_or("no-session");
        if name.is_empty()
            || name.len() > 128
            || !name.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            })
        {
            return Err(ServerError::InvalidConfig);
        }
        let mut state = self.state.lock().map_err(|_| ServerError::Storage)?;
        let profile = authorized_profile(&state, head).ok_or(ServerError::MissingKey)?;
        let (binding, route) = self.binding_for(&state, profile, requested)?;
        if let Some(session) = state
            .sessions
            .iter_mut()
            .find(|session| session.name == name && session.binding.profile == binding.profile)
        {
            if session.binding != binding {
                return Err(ServerError::BindingChanged);
            }
            return Ok((binding, route, session.upstream.clone()));
        }
        if state.sessions.len() >= MAX_SESSIONS {
            return Err(ServerError::Busy);
        }
        let upstream = random_id()?;
        state.sessions.push_back(Session {
            name: name.to_owned(),
            binding: binding.clone(),
            upstream: upstream.clone(),
        });
        Ok((binding, route, upstream))
    }

    fn authorized(&self, head: &RequestHead) -> bool {
        let Ok(state) = self.state.lock() else {
            return false;
        };
        authorized_profile(&state, head).is_some()
    }

    fn grant(
        &self,
        account: &str,
        cancel: &NetworkHttpCancel,
        network: &NetworkPolicy,
        generation: &str,
    ) -> Result<TokenGrant, ServerError> {
        self.grant_budgeted(account, cancel, network, generation, None)
    }

    fn grant_budgeted(
        &self,
        account: &str,
        cancel: &NetworkHttpCancel,
        network: &NetworkPolicy,
        generation: &str,
        budget: Option<&mut CatalogueBudget>,
    ) -> Result<TokenGrant, ServerError> {
        if cancel.is_cancelled() {
            return Err(ServerError::Cancelled);
        }
        let lock = self
            .refresh
            .lock()
            .map_err(|_| ServerError::Storage)?
            .entry(account.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = loop {
            if let Some(budget) = budget.as_deref() {
                let _ = budget.remaining()?;
            }
            if cancel.is_cancelled() {
                return Err(ServerError::Cancelled);
            }
            match lock.try_lock() {
                Ok(guard) => break guard,
                Err(std::sync::TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(_) => return Err(ServerError::Storage),
            }
        };
        let mut grant = self
            .store
            .load_grant_for_generation(account, generation)
            .map_err(credential_error)?;
        if let TokenLifecycle::ExpiringRefresh { expires_at, .. } = &grant.lifecycle
            && *expires_at <= Instant::now() + Duration::from_secs(30)
        {
            let request =
                provider_auth::opencode_refresh(&AuthClient::available(Provider::OpenCode), &grant)
                    .map_err(|_| ServerError::NeedsLogin)?;
            let response = if let Some(budget) = budget {
                budget.collect(
                    self,
                    request,
                    cancel,
                    provider_auth::AUTH_MAX_RESPONSE_BYTES,
                    network,
                )?
            } else {
                self.collect(
                    request,
                    cancel,
                    provider_auth::AUTH_MAX_RESPONSE_BYTES,
                    network,
                )?
            };
            let fresh = provider_auth::parse_opencode_refresh(&response, Instant::now())
                .map_err(|_| ServerError::NeedsLogin)?;
            self.store
                .replace_grant_if_current(account, generation, fresh)
                .map_err(credential_error)?;
            grant = self
                .store
                .load_grant_for_generation(account, generation)
                .map_err(credential_error)?;
        }
        Ok(grant)
    }

    fn stream_request(&self, request: NetworkHttpRequest) -> NetworkHttpStreamRequest {
        #[cfg(test)]
        let request = {
            let mut request = request;
            if let Some(url) = self.fixture_upstream.lock().unwrap().as_ref() {
                request.url = url.clone();
            }
            request
        };
        NetworkHttpStreamRequest::new(request, &self.executable)
    }
    fn collect(
        &self,
        mut request: NetworkHttpRequest,
        cancel: &NetworkHttpCancel,
        limit: usize,
        network: &NetworkPolicy,
    ) -> Result<NetworkHttpResponse, ServerError> {
        request.proxy = network.proxy().map(str::to_owned);
        request.max_redirects = 0;
        request.max_response_bytes = limit;
        let mut status = 0;
        let mut headers = Vec::new();
        let mut body = Vec::new();
        http::request_stream(
            &self.stream_request(request),
            cancel,
            |head| {
                status = head.status;
                headers = head.headers.clone();
                Control::Continue
            },
            |bytes| {
                body.extend_from_slice(bytes);
                Control::Continue
            },
        )
        .map_err(|error| match error {
            http::NetworkHttpStreamError::Cancelled => ServerError::Cancelled,
            _ => ServerError::Upstream,
        })?;
        Ok(NetworkHttpResponse {
            status,
            headers,
            body,
            truncated: false,
        })
    }

    /// Account-local metadata only. The returned authority is captured before
    /// the final request and checked again afterward; routes are never applied.
    pub(crate) fn catalogue(
        &self,
        account: &str,
        requested: &ResolvedAuthority,
        cancel: &Arc<NetworkHttpCancel>,
    ) -> Result<CatalogueResult, ServerError> {
        self.catalogue_budgeted(account, requested, cancel, CatalogueBudget::new())
    }

    fn catalogue_budgeted(
        &self,
        account: &str,
        requested: &ResolvedAuthority,
        cancel: &Arc<NetworkHttpCancel>,
        mut budget: CatalogueBudget,
    ) -> Result<CatalogueResult, ServerError> {
        let lease = BorrowedLease::acquire(self, account, cancel, None)?;
        let initial = self.store.snapshot(account).map_err(credential_error)?;
        if ResolvedAuthority::from_snapshot(&initial).map_err(|_| ServerError::BindingChanged)?
            != *requested
            || initial.generation != lease.generation
            || initial.preferences.service_generation != lease.preferences.service_generation
            || initial.preferences.service != lease.preferences.service
        {
            return Err(ServerError::BindingChanged);
        }
        let network = &lease.preferences.service;
        self.grant_budgeted(
            account,
            cancel,
            network,
            &lease.generation,
            Some(&mut budget),
        )?;
        let acquire = || {
            let snapshot = self.store.snapshot(account).map_err(credential_error)?;
            let authority = ResolvedAuthority::from_snapshot(&snapshot)
                .map_err(|_| ServerError::BindingChanged)?;
            if !requested.same_transport_generation(&authority) {
                return Err(ServerError::BindingChanged);
            }
            let grant = self
                .store
                .load_grant_for_authority(account, &snapshot.generation, &snapshot.organization)
                .map_err(credential_error)?;
            Ok((snapshot, authority, grant))
        };
        let (mut snapshot, mut authority, mut grant) = acquire()?;
        let routes = match grant.provider {
            Provider::DeepSeek => {
                let response = budget.collect(
                    self,
                    account_request(
                        NetworkHttpMethod::Get,
                        "https://api.deepseek.com/models",
                        &grant,
                        "",
                        "",
                    ),
                    cancel,
                    CATALOG_BYTES,
                    network,
                )?;
                check_status(response.status)?;
                parse_deepseek_catalog(&response.body)?
            }
            Provider::OpenCode => {
                let without_org = |url: &str, grant: &TokenGrant| {
                    let mut request = account_request(NetworkHttpMethod::Get, url, grant, "", "");
                    request
                        .headers
                        .retain(|(name, _)| !name.eq_ignore_ascii_case("x-org-id"));
                    request
                };
                let user = budget.collect(
                    self,
                    without_org("https://opencode.ai/console/api/user", &grant),
                    cancel,
                    CATALOG_BYTES,
                    network,
                )?;
                check_status(user.status)?;
                let user: Value =
                    serde_json::from_slice(&user.body).map_err(|_| ServerError::Upstream)?;
                if user.get("id").and_then(Value::as_str).is_none()
                    || user.get("email").and_then(Value::as_str).is_none()
                {
                    return Err(ServerError::Upstream);
                }
                // Reacquire before each metadata exchange. The requested
                // service generation stays fixed even through token refresh.
                (snapshot, _, grant) = acquire()?;
                let orgs = budget.collect(
                    self,
                    without_org("https://opencode.ai/console/api/orgs", &grant),
                    cancel,
                    CATALOG_BYTES,
                    network,
                )?;
                check_status(orgs.status)?;
                let selected = select_organization(&orgs.body, grant.organization.as_deref())?;
                self.store
                    .resolve_catalogue_organization(&snapshot, selected)
                    .map_err(credential_error)?;
                (snapshot, authority, grant) = acquire()?;
                let response = budget.collect(
                    self,
                    account_request(
                        NetworkHttpMethod::Get,
                        "https://opencode.ai/console/api/v2/config",
                        &grant,
                        "",
                        "",
                    ),
                    cancel,
                    CATALOG_BYTES,
                    network,
                )?;
                check_status(response.status)?;
                parse_opencode_catalog(&response.body, grant.organization.as_deref())?
            }
        };
        if cancel.is_cancelled() {
            return Err(ServerError::Cancelled);
        }
        let current = self.store.snapshot(account).map_err(credential_error)?;
        if ResolvedAuthority::from_snapshot(&current).map_err(|_| ServerError::BindingChanged)?
            != authority
            || current.generation != snapshot.generation
        {
            return Err(ServerError::BindingChanged);
        }
        Ok(CatalogueResult {
            authority,
            routes,
            response_bytes: budget.response_bytes,
        })
    }

    #[cfg(test)]
    pub fn catalog(
        &self,
        account: &str,
        cancel: &Arc<NetworkHttpCancel>,
    ) -> Result<Vec<ModelRoute>, ServerError> {
        let snapshot = self.store.snapshot(account).map_err(credential_error)?;
        let authority =
            ResolvedAuthority::from_snapshot(&snapshot).map_err(|_| ServerError::BindingChanged)?;
        self.catalogue(account, &authority, cancel)
            .map(|result| result.routes)
    }

    #[cfg(test)]
    pub(crate) fn fixture_catalogue_upstream(&self, url: String) {
        *self.fixture_upstream.lock().unwrap() = Some(url);
    }

    #[cfg(test)]
    pub(crate) fn fixture_catalogue_leases(&self, account: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .leases
            .get(account)
            .copied()
            .unwrap_or(0)
    }

    #[cfg(test)]
    pub fn probe(
        &self,
        account: &str,
        route: &ModelRoute,
        identity: &str,
        cancel: &Arc<NetworkHttpCancel>,
    ) -> Result<(), ServerError> {
        validate_route(route)?;
        let lease = BorrowedLease::acquire(self, account, cancel, None)?;
        let network = &lease.preferences.service;
        let grant = self.grant(account, cancel, network, &lease.generation)?;
        if grant.provider != route.provider {
            return Err(ServerError::InvalidConfig);
        }
        let body = match route.protocol {
            WireProtocol::Chat => {
                json!({"model":route.model_id,"messages":[{"role":"user","content":"Reply OK."}],"max_tokens":8,"stream":false})
            }
            WireProtocol::Messages => {
                json!({"model":route.model_id,"messages":[{"role":"user","content":"Reply OK."}],"max_tokens":8,"stream":false,"thinking":{"type":"disabled"}})
            }
        };
        let mut request = route_request(route, &grant, identity, &random_id()?);
        request.body = serde_json::to_vec(&body).map_err(|_| ServerError::InvalidConfig)?;
        let response = self.collect(request, cancel, NON_STREAM_BYTES, network)?;
        check_status(response.status)?;
        protocol::convert_response(
            route.protocol.protocol(),
            route.protocol.protocol(),
            &response.body,
            &route.model_id,
        )
        .map_err(ServerError::Protocol)?;
        self.store
            .mark_ready_if_current(account, &lease.generation)
            .map_err(credential_error)?;
        Ok(())
    }
}

impl Handler for Gateway {
    fn admit(&self, head: &RequestHead) -> Result<(), Response> {
        let state = self
            .state
            .lock()
            .map_err(|_| error_response(None, 503, "internal_error"))?;
        if state.draining {
            return Err(error_response(protocol_for(head), 503, "listener_stopping"));
        }
        let port = state.config.port;
        drop(state);
        if head.header("host").ok().flatten() != Some(format!("127.0.0.1:{port}").as_str())
            || !matches!(head.header("origin"), Ok(None))
        {
            return Err(error_response(protocol_for(head), 403, "invalid_origin"));
        }
        if !self.authorized(head) {
            return Err(error_response(
                protocol_for(head),
                401,
                "authentication_error",
            ));
        }
        match (head.method.as_str(), head.path.as_str()) {
            ("GET", "/v1/models" | "/v1/minicon/binding") if head.content_length == 0 => Ok(()),
            ("POST", "/v1/chat/completions" | "/v1/messages") => Ok(()),
            _ => Err(error_response(protocol_for(head), 404, "not_found")),
        }
    }

    fn handle(
        &self,
        request: Request,
        context: &RequestContext,
        reply: &mut Reply,
    ) -> Result<(), local::Failure> {
        // Authority is checked again after body collection, so a revoked key
        // cannot execute work merely because its header was admitted earlier.
        if let Err(response) = self.admit(&request.head) {
            return reply.send(response);
        }
        let downstream = protocol_for(&request.head);
        let result = self.handle_model(request, context, reply);
        if let Err(error) = result {
            if reply.started() {
                let event = stream_error(downstream, error);
                reply.write(event.as_bytes())?;
            } else {
                reply.send(server_error_response(downstream, error))?;
            }
        }
        Ok(())
    }

    fn failure(&self, failure: local::Failure) -> Response {
        let status = match failure {
            local::Failure::QueueFull | local::Failure::QueueTimeout => 503,
            local::Failure::BodyTooLarge => 413,
            _ => 400,
        };
        error_response(None, status, "request_error")
    }
}

impl Gateway {
    fn handle_model(
        &self,
        request: Request,
        context: &RequestContext,
        reply: &mut Reply,
    ) -> Result<(), ServerError> {
        if request.head.path == "/v1/models" {
            let state = self.state.lock().map_err(|_| ServerError::Storage)?;
            let profile =
                authorized_profile(&state, &request.head).ok_or(ServerError::MissingKey)?;
            let aliases = if profile == "default" {
                vec![MODEL_ALIAS]
            } else {
                state
                    .config
                    .profiles
                    .iter()
                    .find(|entry| entry.id == profile)
                    .ok_or(ServerError::MissingKey)?
                    .allocations
                    .iter()
                    .map(|allocation| allocation.alias.as_str())
                    .collect()
            };
            let data: Vec<_> = aliases
                .into_iter()
                .filter(|alias| self.binding_for(&state, profile, Some(alias)).is_ok())
                .map(|alias| json!({"id":alias,"object":"model","owned_by":"minicon"}))
                .collect();
            let binding = self
                .binding_for(&state, profile, None)
                .ok()
                .map(|(binding, _)| binding);
            let body = serde_json::to_vec(&json!({"object":"list","data":data,"binding":binding}))
                .map_err(|_| ServerError::InvalidConfig)?;
            drop(state);
            return reply
                .send(Response {
                    status: 200,
                    content_type: "application/json",
                    body,
                })
                .map_err(|_| ServerError::Cancelled);
        }
        if request.head.path == "/v1/minicon/binding" {
            let (binding, _, _) = self.sticky(&request.head, None)?;
            return reply
                .send(Response {
                    status: 200,
                    content_type: "application/json",
                    body: serde_json::to_vec(&binding).map_err(|_| ServerError::InvalidConfig)?,
                })
                .map_err(|_| ServerError::Cancelled);
        }
        let downstream = protocol_for(&request.head).ok_or(ServerError::InvalidConfig)?;
        let value: Value = serde_json::from_slice(&request.body)
            .map_err(|_| ServerError::Protocol(protocol::ProtocolError::InvalidJson))?;
        let model = match value.get("model") {
            None => None,
            Some(Value::String(model)) => Some(model.as_str()),
            _ => return Err(ServerError::Protocol(protocol::ProtocolError::InvalidJson)),
        };
        let (binding, route, upstream_session) = self.sticky(&request.head, model)?;
        let prepared = protocol::prepare_request(
            downstream,
            route.protocol.protocol(),
            &request.body,
            &route.model_id,
        )
        .map_err(ServerError::Protocol)?;
        let cancel = Arc::new(NetworkHttpCancel::new());
        let hook_cancel = cancel.clone();
        context.on_cancel(Arc::new(move || hook_cancel.cancel()));
        if context.is_cancelled() {
            return Err(ServerError::Cancelled);
        }
        // The Handler receives &self; retain a lease through a narrow borrowed
        // guard rather than transferring tool or route authority elsewhere.
        let lease = BorrowedLease::acquire(
            self,
            &binding.account,
            &cancel,
            Some((&request.head, &binding)),
        )?;
        let network = &lease.preferences.service;
        // Refresh may legitimately resolve a new organization at the same
        // credential generation. It cannot retarget this bound conversation.
        // Reacquire the exact bound authority before any model request; an
        // already acquired grant remains valid for its admitted I/O lifetime.
        let _ = self.grant(&binding.account, &cancel, network, &lease.generation)?;
        let grant = self
            .store
            .load_grant_for_authority(&binding.account, &lease.generation, &lease.organization)
            .map_err(credential_error)?;
        let identity = self
            .state
            .lock()
            .map_err(|_| ServerError::Storage)?
            .config
            .identity
            .clone();
        let mut upstream = route_request(&route, &grant, &identity, &upstream_session);
        upstream.proxy = network.proxy().map(str::to_owned);
        upstream.body = prepared.body;
        if prepared.requires_thinking_disabled {
            let mut body: Value =
                serde_json::from_slice(&upstream.body).map_err(|_| ServerError::InvalidConfig)?;
            body["thinking"] = json!({"type":"disabled"});
            upstream.body = serde_json::to_vec(&body).map_err(|_| ServerError::InvalidConfig)?;
        }
        if !prepared.streaming {
            let response = self.collect(upstream, &cancel, NON_STREAM_BYTES, network)?;
            check_status(response.status)?;
            let body = protocol::convert_response(
                downstream,
                route.protocol.protocol(),
                &response.body,
                &binding.alias,
            )
            .map_err(ServerError::Protocol)?;
            return reply
                .send(Response {
                    status: 200,
                    content_type: "application/json",
                    body,
                })
                .map_err(|_| ServerError::Cancelled);
        }
        upstream.max_response_bytes = protocol::MAX_STREAM_BYTES;
        let mut bridge = SseBridge::new(downstream, route.protocol.protocol(), &binding.alias)
            .map_err(ServerError::Protocol)?
            .with_include_usage(prepared.include_usage);
        let mut error = None;
        let mut status = 0;
        // Headers are published only after the first successfully decoded event,
        // keeping early semantic failures in the protocol's ordinary error shape.
        let result = http::request_stream(
            &self.stream_request(upstream),
            &cancel,
            |head| {
                status = head.status;
                if !(200..300).contains(&head.status) {
                    Control::Stop
                } else {
                    Control::Continue
                }
            },
            |bytes| {
                match bridge.push(bytes) {
                    Ok(events) => {
                        for event in events {
                            if !reply.started() && reply.start(200, "text/event-stream").is_err() {
                                error = Some(ServerError::Cancelled);
                                return Control::Stop;
                            }
                            if reply.write(&event).is_err() {
                                error = Some(ServerError::Cancelled);
                                return Control::Stop;
                            }
                        }
                    }
                    Err(failure) => {
                        error = Some(ServerError::Protocol(failure));
                        return Control::Stop;
                    }
                }
                Control::Continue
            },
        );
        if let Some(error) = error {
            return Err(error);
        }
        result.map_err(|error| match error {
            http::NetworkHttpStreamError::Cancelled => ServerError::Cancelled,
            _ => ServerError::Upstream,
        })?;
        check_status(status)?;
        for event in bridge.finish().map_err(ServerError::Protocol)? {
            if !reply.started() {
                reply
                    .start(200, "text/event-stream")
                    .map_err(|_| ServerError::Cancelled)?;
            }
            reply.write(&event).map_err(|_| ServerError::Cancelled)?;
        }
        Ok(())
    }
}

fn credential_error(error: StoreError) -> ServerError {
    match error {
        StoreError::Changed => ServerError::BindingChanged,
        StoreError::Missing => ServerError::MissingAccount,
        StoreError::Storage | StoreError::InvalidRoot => ServerError::Storage,
        _ => ServerError::NeedsLogin,
    }
}

struct BorrowedLease<'a> {
    gateway: &'a Gateway,
    account: String,
    preferences: AccountPreferences,
    generation: String,
    organization: OrganizationAuthority,
}
impl<'a> BorrowedLease<'a> {
    fn acquire(
        gateway: &'a Gateway,
        account: &str,
        cancel: &Arc<NetworkHttpCancel>,
        client: Option<(&RequestHead, &Binding)>,
    ) -> Result<Self, ServerError> {
        let mut state = gateway.state.lock().map_err(|_| ServerError::Storage)?;
        if state.disabled.contains(account) {
            return Err(ServerError::NeedsLogin);
        }
        if client.is_some() && state.draining {
            return Err(ServerError::Busy);
        }
        // Read policy and credential generation together before the binding
        // revalidation. Reconnect publishes outside the gateway state lock.
        let snapshot = gateway.store.snapshot(account).map_err(credential_error)?;
        if !snapshot.preferences.enabled {
            return Err(ServerError::NeedsLogin);
        }
        let profile = client
            .map(|(head, expected)| {
                let profile = authorized_profile(&state, head).ok_or(ServerError::MissingKey)?;
                if profile != expected.profile {
                    return Err(ServerError::BindingChanged);
                }
                let (current, _) = gateway.binding_for(&state, profile, Some(&expected.alias))?;
                // Body preparation is outside this lock. Key/profile/route
                // changes since resolution must not admit the stale snapshot.
                if &current != expected || current.account != account {
                    return Err(ServerError::BindingChanged);
                }
                Ok(profile.to_owned())
            })
            .transpose()?;
        if state.leases.values().sum::<usize>() >= 8 {
            return Err(ServerError::Busy);
        }
        let count = state.leases.entry(account.to_owned()).or_default();
        if *count >= 2 {
            return Err(ServerError::Busy);
        }
        *count += 1;
        state
            .active
            .retain(|active| active.cancel.strong_count() != 0);
        state.active.push(Active {
            account: account.to_owned(),
            profile,
            cancel: Arc::downgrade(cancel),
        });
        Ok(Self {
            gateway,
            account: account.to_owned(),
            preferences: snapshot.preferences,
            generation: snapshot.generation,
            organization: snapshot.organization,
        })
    }
}
impl Drop for BorrowedLease<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.gateway.state.lock()
            && let Some(count) = state.leases.get_mut(&self.account)
        {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.leases.remove(&self.account);
            }
        }
    }
}

fn authorized_profile<'a>(state: &'a State, head: &RequestHead) -> Option<&'a str> {
    let supplied = supplied_key(head)?;
    if state
        .key
        .as_ref()
        .is_some_and(|key| same_key(key.expose(), supplied))
    {
        return Some("default");
    }
    state
        .config
        .profiles
        .iter()
        .find(|profile| {
            profile.enabled
                && state
                    .clients
                    .get(&profile.id)
                    .is_some_and(|access| same_key(access.key.expose(), supplied))
        })
        .map(|profile| profile.id.as_str())
}

fn account_request(
    method: NetworkHttpMethod,
    url: &str,
    grant: &TokenGrant,
    identity: &str,
    session: &str,
) -> NetworkHttpRequest {
    let mut request = NetworkHttpRequest::new(method, url)
        .timeout(Duration::from_secs(600))
        .max_redirects(0)
        .header("User-Agent", concat!("MiniCon/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/json");
    match grant.provider {
        Provider::DeepSeek => {
            request = request.header("x-dsh-auth-token", grant.access_token.expose());
            if !identity.is_empty() {
                request = request.header("x-deepseek-harness-user-id", identity);
            }
        }
        Provider::OpenCode => {
            request = request.header(
                "Authorization",
                format!("Bearer {}", grant.access_token.expose()),
            );
            if let Some(org) = &grant.organization {
                request = request.header("x-org-id", org);
            }
            if !session.is_empty() {
                request = request.header("x-opencode-session", session);
            }
        }
    }
    request
}

#[derive(Eq, PartialEq)]
struct ModelTransport {
    provider: Provider,
    protocol: WireProtocol,
    model_id: String,
    url: String,
    headers: Vec<(String, String)>,
}

fn emitted_route_headers(route: &ModelRoute) -> impl Iterator<Item = (&str, &str)> {
    route.headers.iter().filter_map(|(name, value)| {
        (!name.eq_ignore_ascii_case("x-org-id") && !name.eq_ignore_ascii_case("anthropic-version"))
            .then_some((name.as_str(), value.as_str()))
    })
}

fn model_transport(route: &ModelRoute) -> ModelTransport {
    let mut headers: Vec<_> = emitted_route_headers(route)
        .map(|(name, value)| (name.to_ascii_lowercase(), value.to_owned()))
        .collect();
    headers.sort();
    ModelTransport {
        provider: route.provider,
        protocol: route.protocol,
        model_id: route.model_id.clone(),
        url: route.url.clone(),
        headers,
    }
}

fn route_request(
    route: &ModelRoute,
    grant: &TokenGrant,
    identity: &str,
    session: &str,
) -> NetworkHttpRequest {
    let mut request = account_request(
        NetworkHttpMethod::Post,
        &route.url,
        grant,
        identity,
        session,
    )
    .header("Content-Type", "application/json");
    if route.protocol == WireProtocol::Messages {
        request = request.header("anthropic-version", "2023-06-01");
    }
    for (name, value) in emitted_route_headers(route) {
        request = request.header(name, value);
    }
    request
}

fn parse_deepseek_catalog(body: &[u8]) -> Result<Vec<ModelRoute>, ServerError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| ServerError::Upstream)?;
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or(ServerError::Upstream)?;
    if data.len() > MAX_MODELS {
        return Err(ServerError::Upstream);
    }
    let mut result = Vec::new();
    for model in data {
        let Some(id) = model.get("id").and_then(Value::as_str) else {
            return Err(ServerError::Upstream);
        };
        if !model["api_capabilities"]["anthropic_messages"].is_object() {
            continue;
        }
        let route = ModelRoute {
            id: format!("deepseek/{id}"),
            model_id: id.to_owned(),
            provider: Provider::DeepSeek,
            protocol: WireProtocol::Messages,
            url: "https://api.deepseek.com/anthropic/v1/messages".into(),
            headers: Vec::new(),
        };
        validate_route(&route)?;
        result.push(route);
    }
    if result.is_empty() {
        return Err(ServerError::MissingModel);
    }
    Ok(result)
}

fn parse_opencode_catalog(
    body: &[u8],
    organization: Option<&str>,
) -> Result<Vec<ModelRoute>, ServerError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| ServerError::Upstream)?;
    let providers = value
        .get("providers")
        .and_then(Value::as_object)
        .ok_or(ServerError::Upstream)?;
    let mut result = Vec::new();
    for (provider_id, provider) in providers {
        let models = provider
            .get("models")
            .and_then(Value::as_object)
            .ok_or(ServerError::Upstream)?;
        for (id, model) in models {
            if model.get("disabled").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let package = model
                .get("package")
                .or_else(|| provider.get("package"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let protocol = match package {
                "@opencode/ai/providers/anthropic-compatible" => WireProtocol::Messages,
                "@opencode/ai/providers/openai-compatible" => WireProtocol::Chat,
                _ => continue,
            };
            let base = model
                .get("settings")
                .and_then(|settings| settings.get("baseURL"))
                .or_else(|| {
                    provider
                        .get("settings")
                        .and_then(|settings| settings.get("baseURL"))
                })
                .and_then(Value::as_str)
                .ok_or(ServerError::Upstream)?;
            let mut headers = Vec::new();
            for source in [provider.get("headers"), model.get("headers")]
                .into_iter()
                .flatten()
            {
                let source = source.as_object().ok_or(ServerError::Upstream)?;
                for (name, value) in source {
                    let value = value.as_str().ok_or(ServerError::Upstream)?;
                    if name.eq_ignore_ascii_case("x-org-id") {
                        if Some(value) != organization {
                            return Err(ServerError::InvalidConfig);
                        }
                    } else {
                        return Err(ServerError::InvalidConfig);
                    }
                    headers.retain(|(key, _): &(String, String)| !key.eq_ignore_ascii_case(name));
                    headers.push((name.clone(), value.to_owned()));
                }
            }
            let route = ModelRoute {
                id: format!("opencode/{provider_id}/{id}"),
                model_id: model
                    .get("modelID")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_owned(),
                provider: Provider::OpenCode,
                protocol,
                url: format!(
                    "{}/{}",
                    base.trim_end_matches('/'),
                    if protocol == WireProtocol::Chat {
                        "chat/completions"
                    } else {
                        "messages"
                    }
                ),
                headers,
            };
            validate_route(&route)?;
            result.push(route);
            if result.len() > MAX_MODELS {
                return Err(ServerError::InvalidConfig);
            }
        }
    }
    if result.is_empty() {
        return Err(ServerError::MissingModel);
    }
    Ok(result)
}

fn select_organization(body: &[u8], expected: Option<&str>) -> Result<Option<String>, ServerError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| ServerError::Upstream)?;
    let mut organizations: Vec<_> = value
        .as_array()
        .ok_or(ServerError::Upstream)?
        .iter()
        .map(|entry| {
            Ok((
                entry
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or(ServerError::Upstream)?
                    .to_owned(),
                entry
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or(ServerError::Upstream)?
                    .to_owned(),
            ))
        })
        .collect::<Result<_, ServerError>>()?;
    if organizations.len() > 256
        || organizations.iter().any(|(name, id)| {
            name.len() > 256 || id.is_empty() || id.len() > 128 || id.chars().any(char::is_control)
        })
    {
        return Err(ServerError::Upstream);
    }
    if let Some(expected) = expected {
        return organizations
            .iter()
            .find(|(_, id)| id == expected)
            .map(|(_, id)| Some(id.clone()))
            .ok_or(ServerError::NeedsLogin);
    }
    organizations.sort();
    Ok(organizations.first().map(|(_, id)| id.clone()))
}

pub(crate) fn validate_route(route: &ModelRoute) -> Result<(), ServerError> {
    if !safe_model(&route.id, false)
        || !safe_model(&route.model_id, false)
        || route.headers.len() > 8
    {
        return Err(ServerError::InvalidConfig);
    }
    let uri: minicon_platform::network_http::NetworkHttpRequest =
        NetworkHttpRequest::new(NetworkHttpMethod::Get, &route.url);
    minicon_platform::network_http::validate(&uri).map_err(|_| ServerError::InvalidConfig)?;
    let allowed = match route.provider {
        Provider::DeepSeek => {
            route.url == "https://api.deepseek.com/anthropic/v1/messages"
                && route.protocol == WireProtocol::Messages
        }
        Provider::OpenCode => {
            route.url.starts_with("https://opencode.ai/inference/")
                && !route.url.contains(['?', '#', '@', '%'])
                && !route.url.contains("..")
                && route
                    .url
                    .ends_with(if route.protocol == WireProtocol::Chat {
                        "/chat/completions"
                    } else {
                        "/messages"
                    })
        }
    };
    if !allowed
        || route.headers.iter().any(|(key, value)| {
            !key.eq_ignore_ascii_case("x-org-id")
                || value.len() > 128
                || value.chars().any(char::is_control)
        })
    {
        return Err(ServerError::InvalidConfig);
    }
    Ok(())
}

fn protocol_for(head: &RequestHead) -> Option<Protocol> {
    match head.path.as_str() {
        "/v1/messages" => Some(Protocol::AnthropicMessages),
        "/v1/chat/completions" => Some(Protocol::OpenAiChat),
        _ => None,
    }
}
fn check_status(status: u16) -> Result<(), ServerError> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(ServerError::NeedsLogin),
        _ => Err(ServerError::Http(status)),
    }
}
fn same_key(expected: &str, supplied: &str) -> bool {
    let mut difference = expected.len() ^ supplied.len();
    for (a, b) in expected.bytes().zip(supplied.bytes()) {
        difference |= usize::from(a ^ b);
    }
    difference == 0
}
pub(crate) fn opaque_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
pub(crate) fn safe_model(value: &str, empty: bool) -> bool {
    (empty || !value.is_empty()) && value.len() <= 200 && !value.chars().any(char::is_control)
}
pub fn random_id() -> Result<String, ServerError> {
    minicon_platform::entropy::secure_random_array::<16>()
        .map(|bytes| bytes.iter().map(|byte| format!("{byte:02x}")).collect())
        .map_err(|_| ServerError::Storage)
}
fn error_response(protocol: Option<Protocol>, status: u16, kind: &str) -> Response {
    Response {
        status,
        content_type: "application/json",
        body: serde_json::to_vec(&match protocol {
            Some(Protocol::AnthropicMessages) => {
                json!({"type":"error","error":{"type":kind,"message":kind}})
            }
            _ => json!({"error":{"type":kind,"message":kind,"code":kind}}),
        })
        .expect("bounded fixed error schema"),
    }
}
fn server_error_response(protocol: Option<Protocol>, error: ServerError) -> Response {
    let (status, kind) = match error {
        ServerError::BindingChanged => (409, "binding_changed"),
        ServerError::NeedsLogin | ServerError::MissingKey => (401, "needs_login"),
        ServerError::MissingModel => (400, "model_unavailable"),
        ServerError::Busy => (429, "account_busy"),
        ServerError::Protocol(_) => (400, "unsupported_model_request"),
        ServerError::Cancelled => (499, "cancelled"),
        _ => (502, "upstream_error"),
    };
    error_response(protocol, status, kind)
}
fn stream_error(protocol: Option<Protocol>, _error: ServerError) -> String {
    match protocol { Some(Protocol::AnthropicMessages)=>"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"model_stream_failed\"}}\n\n".into(), _=>"data: {\"error\":{\"type\":\"api_error\",\"message\":\"model_stream_failed\"}}\n\n".into() }
}

fn supplied_key(head: &RequestHead) -> Option<&str> {
    if head.path == "/v1/messages" {
        head.header("x-api-key").ok().flatten()
    } else {
        head.header("authorization")
            .ok()
            .flatten()
            .and_then(|value| value.strip_prefix("Bearer "))
    }
}

/// Only the successful real candidate transport path can construct this token.
pub(crate) struct ProbeObservation {
    identity: crate::provider_selection::CandidateIdentity,
    checked_unix_ms: u64,
}
impl std::fmt::Debug for ProbeObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProbeObservation([REDACTED])")
    }
}
impl ProbeObservation {
    pub(crate) fn into_parts(self) -> (crate::provider_selection::CandidateIdentity, u64) {
        (self.identity, self.checked_unix_ms)
    }
}

impl Gateway {
    pub(crate) fn probe_candidate(
        &self,
        candidate: &crate::provider_selection::FrozenCandidate,
        permit: &crate::provider_selection::ProbePermit,
        identity: &str,
        cancel: &Arc<NetworkHttpCancel>,
        deadline: Instant,
    ) -> Result<ProbeObservation, ServerError> {
        let account = candidate.account();
        let route = candidate.route();
        validate_route(route)?;
        let lease = BorrowedLease::acquire(self, account, cancel, None)?;
        let initial = self.store.snapshot(account).map_err(credential_error)?;
        if !candidate.matches_current(&initial)
            || initial.generation != lease.generation
            || initial.preferences.service_generation != lease.preferences.service_generation
            || initial.preferences.service != lease.preferences.service
        {
            return Err(ServerError::BindingChanged);
        }
        let mut budget = CatalogueBudget {
            deadline: deadline.min(Instant::now() + Duration::from_secs(60)),
            response_bytes: 0,
        };
        let network = &lease.preferences.service;
        self.grant_budgeted(
            account,
            cancel,
            network,
            &lease.generation,
            Some(&mut budget),
        )?;
        // A same-generation refresh may change organization. Never attach its
        // new scope to the old exact candidate or send a paid request for it.
        let refreshed = self.store.snapshot(account).map_err(credential_error)?;
        if !candidate.matches_current(&refreshed) {
            return Err(ServerError::BindingChanged);
        }
        let grant = self
            .store
            .load_grant_for_authority(account, &refreshed.generation, &refreshed.organization)
            .map_err(credential_error)?;
        if grant.provider != route.provider {
            return Err(ServerError::BindingChanged);
        }
        let body = match route.protocol {
            WireProtocol::Chat => {
                json!({"model":route.model_id,"messages":[{"role":"user","content":"Reply OK."}],"max_tokens":8,"stream":false})
            }
            WireProtocol::Messages => {
                json!({"model":route.model_id,"messages":[{"role":"user","content":"Reply OK."}],"max_tokens":8,"stream":false,"thinking":{"type":"disabled"}})
            }
        };
        let mut request = route_request(route, &grant, identity, &random_id()?);
        request.body = serde_json::to_vec(&body).map_err(|_| ServerError::InvalidConfig)?;
        {
            // Admission is a linearization point, not a promise that a later
            // UI edit can retract a byte already handed to the native engine.
            let state = self.state.lock().map_err(|_| ServerError::Storage)?;
            self.store
                .with_snapshot(account, |current| {
                    if state.disabled.contains(account) || !candidate.matches_current(&current) {
                        return Err(ServerError::BindingChanged);
                    }
                    if cancel.is_cancelled() || !permit.try_admit() {
                        return Err(ServerError::Cancelled);
                    }
                    Ok(())
                })
                .map_err(credential_error)??;
        }
        let response = budget.collect(self, request, cancel, CATALOG_BYTES, network)?;
        let _ = budget.remaining()?;
        check_status(response.status)?;
        protocol::convert_response(
            route.protocol.protocol(),
            route.protocol.protocol(),
            &response.body,
            &route.model_id,
        )
        .map_err(ServerError::Protocol)?;
        if cancel.is_cancelled() || !permit.is_admitted() {
            return Err(ServerError::Cancelled);
        }
        let current = self.store.snapshot(account).map_err(credential_error)?;
        if !candidate.matches_current(&current) {
            return Err(ServerError::BindingChanged);
        }
        let checked_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| ServerError::Storage)?
            .as_millis()
            .try_into()
            .map_err(|_| ServerError::Storage)?;
        if checked_unix_ms == 0 {
            return Err(ServerError::Storage);
        }
        Ok(ProbeObservation {
            identity: candidate.identity().clone(),
            checked_unix_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn k1_gateway_generation_failure_precedes_actual_disk_callback_and_live_swap() {
        let fixture = Fixture::new();
        let server = fixture.server();
        let before =
            read_binding(server.get("/v1/minicon/binding", "x-minicon-session: k1-entropy\r\n"));
        let path = fixture.root.join("k1-entropy-key");
        std::fs::write(&path, b"synthetic-old").unwrap();
        let called = std::sync::atomic::AtomicBool::new(false);
        let result = fixture.gateway.publish_client_key_with_generation(
            "default",
            Some(Secret::new("synthetic-next").unwrap()),
            || {
                called.store(true, std::sync::atomic::Ordering::Release);
                std::fs::write(&path, b"synthetic-next").unwrap();
                Ok(ClientKeyPublication::Durable)
            },
            || Err(ServerError::Storage),
        );
        assert_eq!(result, Err(ServerError::Storage));
        assert!(
            !called.load(std::sync::atomic::Ordering::Acquire),
            "fallible key generation ran after the actual publication callback"
        );
        assert!(
            std::fs::read(&path).unwrap() == b"synthetic-old",
            "failed prepublication preparation changed actual disk bytes"
        );
        assert_eq!(
            read_binding(server.get("/v1/minicon/binding", "x-minicon-session: k1-entropy\r\n")),
            before
        );
    }
    #[test]
    fn k1_gateway_duplicate_and_unpublished_failure_never_reach_live_key_swap() {
        let fixture = Fixture::new();
        let profile = add_profile(&fixture, &fixture.account);
        let server = fixture.server();
        let old = read_binding(server.get_key(
            "/v1/minicon/binding",
            "synthetic-other",
            "x-minicon-session: k1-existing\r\n",
        ));
        let called = std::sync::atomic::AtomicBool::new(false);
        assert_eq!(
            fixture.gateway.publish_client_key(
                &profile,
                Some(Secret::new("synthetic-local").unwrap()),
                || {
                    called.store(true, std::sync::atomic::Ordering::Release);
                    Ok(ClientKeyPublication::Durable)
                }
            ),
            Err(ServerError::InvalidConfig)
        );
        assert!(
            !called.load(std::sync::atomic::Ordering::Acquire),
            "duplicate key reached publication"
        );
        assert_eq!(
            fixture.gateway.publish_client_key(
                &profile,
                Some(Secret::new("synthetic-new").unwrap()),
                || {
                    called.store(true, std::sync::atomic::Ordering::Release);
                    Err(ServerError::Storage)
                }
            ),
            Err(ServerError::Storage)
        );
        assert!(called.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(
            read_binding(server.get_key(
                "/v1/minicon/binding",
                "synthetic-other",
                "x-minicon-session: k1-existing\r\n"
            )),
            old
        );
        assert!(
            server
                .get_key("/v1/models", "synthetic-new", "")
                .starts_with("HTTP/1.1 401")
        );
    }
    #[test]
    fn k1_gateway_unchanged_mutation_still_calls_publisher_without_rotating_binding() {
        let fixture = Fixture::new();
        let server = fixture.server();
        let before =
            read_binding(server.get("/v1/minicon/binding", "x-minicon-session: k1-same\r\n"));
        let called = std::sync::atomic::AtomicBool::new(false);
        let result = fixture
            .gateway
            .publish_client_key(
                "default",
                Some(Secret::new("synthetic-local").unwrap()),
                || {
                    called.store(true, std::sync::atomic::Ordering::Release);
                    Ok(ClientKeyPublication::NoChange)
                },
            )
            .unwrap();
        assert!(
            called.load(std::sync::atomic::Ordering::Acquire),
            "unchanged key must not bypass the caller's publication boundary"
        );
        assert!(!result.changed);
        assert_eq!(result.publication, ClientKeyPublication::NoChange);
        assert_eq!(
            read_binding(server.get("/v1/minicon/binding", "x-minicon-session: k1-same\r\n")),
            before
        );
    }
    #[test]
    fn k1_published_revoke_warning_cancels_only_target_and_recreate_refuses_old_key() {
        let fixture = Fixture::new();
        let profile = add_profile(&fixture, &fixture.account);
        let server = fixture.server();
        let unaffected =
            read_binding(server.get("/v1/minicon/binding", "x-minicon-session: k1-default\r\n"));
        let (binding, _, _) = fixture
            .gateway
            .sticky(&head("synthetic-other"), None)
            .unwrap();
        let (ordinary, _, _) = fixture
            .gateway
            .sticky(&head("synthetic-local"), None)
            .unwrap();
        let one = Arc::new(NetworkHttpCancel::new());
        let two = Arc::new(NetworkHttpCancel::new());
        let first = BorrowedLease::acquire(
            &fixture.gateway,
            &fixture.account,
            &one,
            Some((&head("synthetic-other"), &binding)),
        )
        .unwrap();
        let second = BorrowedLease::acquire(
            &fixture.gateway,
            &fixture.account,
            &two,
            Some((&head("synthetic-local"), &ordinary)),
        )
        .unwrap();
        let path = fixture.root.join("k1-custom-key");
        std::fs::write(&path, b"synthetic-other").unwrap();
        let result = fixture
            .gateway
            .publish_client_key(&profile, None, || {
                std::fs::remove_file(&path).unwrap();
                Ok(ClientKeyPublication::DurabilityUnconfirmed)
            })
            .unwrap();
        assert!(result.changed);
        assert_eq!(
            result.publication,
            ClientKeyPublication::DurabilityUnconfirmed
        );
        assert!(!path.exists());
        assert!(
            one.is_cancelled(),
            "target active lease did not receive revocation"
        );
        assert!(!two.is_cancelled(), "other profile work was cancelled");
        assert!(
            server
                .get_key("/v1/models", "synthetic-other", "")
                .starts_with("HTTP/1.1 401")
        );
        assert_eq!(
            read_binding(server.get("/v1/minicon/binding", "x-minicon-session: k1-default\r\n")),
            unaffected
        );
        fixture
            .gateway
            .publish_client_key(
                &profile,
                Some(Secret::new("synthetic-recreated").unwrap()),
                || {
                    std::fs::write(&path, b"synthetic-recreated").unwrap();
                    Ok(ClientKeyPublication::Durable)
                },
            )
            .unwrap();
        assert!(
            server
                .get_key("/v1/models", "synthetic-other", "")
                .starts_with("HTTP/1.1 401")
        );
        assert!(
            server
                .get_key("/v1/models", "synthetic-recreated", "")
                .starts_with("HTTP/1.1 200")
        );
        assert!(!two.is_cancelled());
        drop(first);
        drop(second);
    }
    fn cs03_candidate(
        fixture: &Fixture,
    ) -> (
        crate::provider_selection::DraftBook,
        crate::provider_selection::ProbeTicket,
        crate::provider_selection::FrozenCandidate,
    ) {
        let config = fixture
            .gateway
            .state
            .lock()
            .unwrap()
            .config
            .clone()
            .upgrade_applied()
            .unwrap();
        let book = crate::provider_selection::DraftBook::from_applied(&config).unwrap();
        let snapshot = fixture.gateway.store.snapshot(&fixture.account).unwrap();
        let (ticket, candidate) = book
            .freeze_probe("default", book.revision("default").unwrap(), 0, &snapshot)
            .unwrap();
        (book, ticket, candidate)
    }
    fn cs03_completion() -> Value {
        json!({"id":"synthetic-probe","model":"fixture-wire","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}]})
    }

    #[test]
    fn cs03_candidate_transport_uses_one_small_real_request_and_exact_observation() {
        let fixture = Fixture::new();
        let (mut book, ticket, candidate) = cs03_candidate(&fixture);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        fixture.gateway.fixture_catalogue_upstream(format!(
            "http://{}/candidate",
            listener.local_addr().unwrap()
        ));
        let gateway = fixture.gateway.clone();
        let permit = crate::provider_selection::ProbePermit::new();
        let worker_permit = permit.clone();
        let worker = thread::spawn(move || {
            gateway.probe_candidate(
                &candidate,
                &worker_permit,
                "synthetic-check",
                &Arc::new(NetworkHttpCancel::new()),
                Instant::now() + Duration::from_secs(5),
            )
        });
        let (stream, head, body) = controlled_peer_request(&listener);
        assert!(head.starts_with("POST /candidate "));
        assert_eq!(body["model"], "fixture-wire");
        assert_eq!(body["max_tokens"], 8);
        assert_eq!(body["stream"], false);
        reply_controlled_peer(stream, cs03_completion());
        let observation = worker.join().unwrap().unwrap();
        let snapshot = fixture.gateway.store.snapshot(&fixture.account).unwrap();
        book.accept_probe_observation(ticket, observation, &snapshot, &permit)
            .unwrap();
        let views = book.views(&crate::provider_selection::ConnectionAccounts::from([(
            fixture.account.clone(),
            Ok(snapshot),
        )]));
        assert_eq!(
            views[0].allocations[0].evidence,
            crate::provider_selection::ConnectionEvidence::Verified
        );
        assert!(
            views[0].allocations[0]
                .checked_unix_ms
                .is_some_and(|time| time > 0)
        );
        listener.set_nonblocking(true).unwrap();
        assert!(
            matches!(listener.accept(),Err(error) if error.kind()==std::io::ErrorKind::WouldBlock)
        );
    }

    #[test]
    fn cs03_refreshed_organization_change_refuses_before_any_paid_inference() {
        let fixture = Fixture::new();
        let (_, _, candidate) = cs03_candidate(&fixture);
        let before = fixture.gateway.store.snapshot(&fixture.account).unwrap();
        fixture
            .gateway
            .store
            .replace_grant_if_current(
                &fixture.account,
                &before.generation,
                TokenGrant {
                    provider: Provider::OpenCode,
                    access_token: Secret::new("synthetic-expired").unwrap(),
                    lifecycle: TokenLifecycle::ExpiringRefresh {
                        refresh_token: Secret::new("synthetic-refresh").unwrap(),
                        expires_at: Instant::now(),
                    },
                    organization: None,
                },
            )
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        fixture.gateway.fixture_catalogue_upstream(format!(
            "http://{}/refresh",
            listener.local_addr().unwrap()
        ));
        let gateway = fixture.gateway.clone();
        let worker = thread::spawn(move || {
            gateway.probe_candidate(
                &candidate,
                &crate::provider_selection::ProbePermit::new(),
                "synthetic-check",
                &Arc::new(NetworkHttpCancel::new()),
                Instant::now() + Duration::from_secs(5),
            )
        });
        let (stream, _, body) = controlled_peer_request(&listener);
        assert_eq!(body["grant_type"], "refresh_token");
        fixture
            .gateway
            .store
            .replace_grant_if_current(
                &fixture.account,
                &before.generation,
                TokenGrant {
                    provider: Provider::OpenCode,
                    access_token: Secret::new("synthetic-latest").unwrap(),
                    lifecycle: TokenLifecycle::ExpiringRefresh {
                        refresh_token: Secret::new("synthetic-refresh").unwrap(),
                        expires_at: Instant::now() + Duration::from_secs(3600),
                    },
                    organization: Some("changed-org".into()),
                },
            )
            .unwrap();
        reply_controlled_peer(
            stream,
            json!({"access_token":"synthetic-refreshed","refresh_token":"synthetic-new","expires_in":3600}),
        );
        listener.set_nonblocking(true).unwrap();
        let mut paid_requests = 0;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !worker.is_finished() {
            match listener.accept() {
                Ok((mut inference, _)) => {
                    inference.set_nonblocking(false).unwrap();
                    inference
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    inference
                        .set_write_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let headers = read_headers(&mut inference);
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            line.split_once(':')
                                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                .map(|(_, value)| value.trim().parse().unwrap())
                        })
                        .unwrap();
                    assert!(length <= 16384);
                    let mut body = vec![0; length];
                    inference.read_exact(&mut body).unwrap();
                    let body: Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(body["max_tokens"], 8);
                    paid_requests += 1;
                    assert!(paid_requests <= 1, "candidate check retried inference");
                    reply_controlled_peer(inference, cs03_completion());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "controlled candidate did not reach a terminal result"
                    );
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("candidate inference observation: {error}"),
            }
        }
        assert!(matches!(
            worker.join().unwrap(),
            Err(ServerError::BindingChanged)
        ));
        assert_eq!(paid_requests, 0, "authority change emitted paid inference");
    }

    #[test]
    fn cs03_candidate_response_rechecks_authority_after_real_success_body() {
        let fixture = Fixture::new();
        let (_, _, candidate) = cs03_candidate(&fixture);
        let before = fixture.gateway.store.snapshot(&fixture.account).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        fixture.gateway.fixture_catalogue_upstream(format!(
            "http://{}/candidate",
            listener.local_addr().unwrap()
        ));
        let gateway = fixture.gateway.clone();
        let worker = thread::spawn(move || {
            gateway.probe_candidate(
                &candidate,
                &crate::provider_selection::ProbePermit::new(),
                "synthetic-check",
                &Arc::new(NetworkHttpCancel::new()),
                Instant::now() + Duration::from_secs(5),
            )
        });
        let (stream, _, body) = controlled_peer_request(&listener);
        assert_eq!(body["max_tokens"], 8);
        let mut preferences = before.preferences.clone();
        preferences.service_generation = "e".repeat(32);
        fixture
            .gateway
            .store
            .update_preferences(&fixture.account, "Changed service", preferences)
            .unwrap();
        reply_controlled_peer(stream, cs03_completion());
        assert!(matches!(
            worker.join().unwrap(),
            Err(ServerError::BindingChanged)
        ));
    }

    #[test]
    fn cs03_refresh_and_inference_share_the_actual_two_mib_response_budget() {
        let fixture = Fixture::new();
        let (_, _, candidate) = cs03_candidate(&fixture);
        let before = fixture.gateway.store.snapshot(&fixture.account).unwrap();
        fixture
            .gateway
            .store
            .replace_grant_if_current(
                &fixture.account,
                &before.generation,
                TokenGrant {
                    provider: Provider::OpenCode,
                    access_token: Secret::new("synthetic-expired").unwrap(),
                    lifecycle: TokenLifecycle::ExpiringRefresh {
                        refresh_token: Secret::new("synthetic-refresh").unwrap(),
                        expires_at: Instant::now(),
                    },
                    organization: None,
                },
            )
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        fixture.gateway.fixture_catalogue_upstream(format!(
            "http://{}/budget",
            listener.local_addr().unwrap()
        ));
        let gateway = fixture.gateway.clone();
        let worker = thread::spawn(move || {
            gateway.probe_candidate(
                &candidate,
                &crate::provider_selection::ProbePermit::new(),
                "synthetic-check",
                &Arc::new(NetworkHttpCancel::new()),
                Instant::now() + Duration::from_secs(5),
            )
        });
        let (refresh, _, body) = controlled_peer_request(&listener);
        assert_eq!(body["grant_type"], "refresh_token");
        reply_controlled_peer(
            refresh,
            json!({"access_token":"synthetic-fresh","refresh_token":"synthetic-new","expires_in":3600,"padding":"x".repeat(16*1024)}),
        );
        let (mut inference, _, body) = controlled_peer_request(&listener);
        assert_eq!(body["max_tokens"], 8);
        let mut response = cs03_completion();
        response["padding"] = json!("x".repeat(CATALOG_BYTES - 8192));
        let response = response.to_string();
        assert!(response.len() < CATALOG_BYTES);
        // The only expected early close is the real cumulative client cap.
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response.len()
        );
        let _ = inference
            .write_all(header.as_bytes())
            .and_then(|()| inference.write_all(response.as_bytes()));
        assert!(
            matches!(worker.join().unwrap(), Err(ServerError::Upstream)),
            "individually bounded exchanges bypassed their cumulative response cap"
        );
    }

    use super::*;
    use crate::provider_auth::{Secret, TokenGrant, TokenLifecycle};
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::mpsc,
        thread,
    };

    struct Fixture {
        root: PathBuf,
        gateway: Arc<Gateway>,
        account: String,
    }
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("minicon-gateway-{}", random_id().unwrap()));
            std::fs::create_dir_all(&root).unwrap();
            let root = root.canonicalize().unwrap();
            let store = Arc::new(AccountStore::new(root.join("accounts")).unwrap());
            let account = store
                .insert(
                    "Synthetic fixture",
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
            store.mark_ready(&account.id).unwrap();
            let mut config = Config::fresh().unwrap();
            config.selected_account = Some(account.id.clone());
            config.model = "opencode/remote/fixture".into();
            config.models = vec![ModelRoute {
                id: config.model.clone(),
                model_id: "fixture-wire".into(),
                provider: Provider::OpenCode,
                protocol: WireProtocol::Chat,
                url: "https://opencode.ai/inference/openai/v1/chat/completions".into(),
                headers: vec![],
            }];
            let gateway =
                Gateway::new(store, config, Some(Secret::new("synthetic-local").unwrap())).unwrap();
            Self {
                root,
                gateway,
                account: account.id,
            }
        }
        fn server(&self) -> ServerFixture {
            let server = local::Server::start(0, self.gateway.clone()).unwrap();
            self.gateway.state.lock().unwrap().config.port = server.address().port();
            ServerFixture { server }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    struct ServerFixture {
        server: local::Server,
    }
    impl ServerFixture {
        fn request(&self, path: &str, extra: &str, body: Option<&str>) -> TcpStream {
            self.request_key(path, "synthetic-local", extra, body)
        }
        fn request_key(&self, path: &str, key: &str, extra: &str, body: Option<&str>) -> TcpStream {
            let mut stream = TcpStream::connect(self.server.address()).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(4)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(4)))
                .unwrap();
            let body = body.unwrap_or("");
            write!(stream,"{} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {key}\r\n{extra}Content-Length: {}\r\n\r\n{body}",if body.is_empty(){"GET"}else{"POST"},self.server.address(),body.len()).unwrap();
            stream
        }
        fn get_key(&self, path: &str, key: &str, extra: &str) -> String {
            let mut stream = self.request_key(path, key, extra, None);
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            String::from_utf8(bytes).unwrap()
        }
        fn get(&self, path: &str, extra: &str) -> String {
            let mut stream = self.request(path, extra, None);
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            String::from_utf8(bytes).unwrap()
        }
    }
    fn head(key: &str) -> RequestHead {
        RequestHead {
            method: "POST".into(),
            path: "/v1/chat/completions".into(),
            headers: vec![("authorization".into(), format!("Bearer {key}"))],
            content_length: 0,
        }
    }
    fn read_headers(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut byte = [0];
        while !bytes.ends_with(b"\r\n\r\n") {
            assert!(bytes.len() < 65536);
            stream.read_exact(&mut byte).unwrap();
            bytes.push(byte[0]);
        }
        String::from_utf8(bytes).unwrap()
    }
    fn read_binding(response: String) -> Value {
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
    }
    fn controlled_peer_request(listener: &TcpListener) -> (TcpStream, String, Value) {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(pair) => break pair,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "controlled provider request did not arrive"
                    );
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("controlled provider accept: {error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let headers = read_headers(&mut stream);
        let length: usize = headers
            .lines()
            .find_map(|line| {
                line.split_once(':')
                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse().unwrap())
            })
            .unwrap_or(0);
        assert!(length <= 16384);
        let mut body = vec![0; length];
        stream.read_exact(&mut body).unwrap();
        let value = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap()
        };
        (stream, headers, value)
    }
    fn reply_controlled_peer(mut stream: TcpStream, value: Value) {
        let body = value.to_string();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        stream.flush().unwrap();
    }
    fn upstream_request(stream: &mut TcpStream) -> String {
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        let headers = read_headers(stream);
        let length = headers
            .lines()
            .find_map(|line| {
                line.split_once(':')
                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        let mut body = vec![0; length];
        stream.read_exact(&mut body).unwrap();
        assert!(
            headers
                .to_lowercase()
                .contains("authorization: bearer synthetic-access")
        );
        assert!(headers.to_lowercase().contains("x-opencode-session:"));
        assert!(String::from_utf8(body).unwrap().contains("fixture-wire"));
        headers
    }
    const FIRST: &str = "data: {\"id\":\"synthetic\",\"model\":\"fixture-wire\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}\n\n";
    const END: &str = "data: {\"id\":\"synthetic\",\"model\":\"fixture-wire\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    const BODY: &str = r#"{"model":"minicon-selected","stream":true,"messages":[{"role":"user","content":"test"}]}"#;

    #[test]
    fn cs02_v1_v2_migration_and_private_interning_preserve_real_http_names_and_named_bindings() {
        for version in [1, 2] {
            let fixture = Fixture::new();
            let profile = (version == 2).then(|| add_profile(&fixture, &fixture.account));
            let server = fixture.server();
            let session = "x-minicon-session: cs02-migration\r\n";
            let before = server.get("/v1/minicon/binding", session);
            assert!(before.starts_with("HTTP/1.1 200"));
            let custom_before = profile
                .as_ref()
                .map(|_| server.get_key("/v1/minicon/binding", "synthetic-other", session));
            let old = fixture.gateway.state.lock().unwrap().config.clone();
            assert_eq!(old.version, version);
            let bytes = old.encode_applied().unwrap();
            let mut migrated = Config::decode_applied(&bytes).unwrap();
            assert_eq!(migrated.version, 3);
            assert_eq!(migrated.identity, old.identity);
            assert_eq!(migrated.model, old.model);
            assert_eq!(migrated.profiles, old.profiles);
            assert_eq!(migrated.models, old.models);
            assert!(
                migrated
                    .applied_evidence
                    .iter()
                    .all(|entry| matches!(entry.origin, AppliedOrigin::LegacyApplied { .. }))
            );
            // Intern only the private key, retaining old accepted names and
            // immutable provenance. This must not rotate default/custom pins.
            let private = "private/cs02-snapshot";
            migrated.models[0].id = private.into();
            migrated.default_route = Some(private.into());
            for client in &mut migrated.profiles {
                client.allocations[0].route = private.into();
            }
            fixture.gateway.update(migrated).unwrap();
            assert_eq!(server.get("/v1/minicon/binding", session), before);
            if let Some(custom_before) = custom_before {
                assert_eq!(
                    server.get_key("/v1/minicon/binding", "synthetic-other", session),
                    custom_before
                );
            }
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            *fixture.gateway.fixture_upstream.lock().unwrap() = Some(format!(
                "http://{}/inference",
                listener.local_addr().unwrap()
            ));
            let request_count = 3 + usize::from(profile.is_some());
            thread::scope(|scope| {
                let peer = scope.spawn(|| {
                    for _ in 0..request_count {
                        let (stream, headers, body) = controlled_peer_request(&listener);
                        assert!(headers.starts_with("POST "));
                        assert!(headers.to_ascii_lowercase().contains("x-opencode-session:"));
                        assert_eq!(body["model"], "fixture-wire");
                        reply_controlled_peer(stream, json!({"id":"cs02","object":"chat.completion","model":"fixture-wire","choices":[{"index":0,"message":{"role":"assistant","content":"CS02_OK"},"finish_reason":"stop"}]}));
                    }
                });
                for model in [MODEL_ALIAS, old.model.as_str(), "fixture-wire"] {
                    let body = json!({"model":model,"messages":[{"role":"user","content":"synthetic"}],"max_tokens":1,"stream":false}).to_string();
                    let mut stream = server.request("/v1/chat/completions", session, Some(&body));
                    let mut response = String::new();
                    stream.read_to_string(&mut response).unwrap();
                    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
                    let json: Value =
                        serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
                    assert_eq!(json["choices"][0]["message"]["content"], "CS02_OK");
                }
                if profile.is_some() {
                    let body = json!({"model":"work","messages":[{"role":"user","content":"synthetic"}],"max_tokens":1,"stream":false}).to_string();
                    let mut stream = server.request_key(
                        "/v1/chat/completions",
                        "synthetic-other",
                        session,
                        Some(&body),
                    );
                    let mut response = String::new();
                    stream.read_to_string(&mut response).unwrap();
                    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
                }
                peer.join().unwrap();
            });
            // The new private key is not a newly published public alias.
            let body = json!({"model":private,"messages":[{"role":"user","content":"synthetic"}],"max_tokens":1}).to_string();
            let mut stream = server.request("/v1/chat/completions", "", Some(&body));
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            assert!(response.starts_with("HTTP/1.1 400"), "{response}");
        }
    }

    #[test]
    fn cs02_v3_rejects_forged_legacy_origins_bad_references_and_wrong_evidence_identity() {
        let fixture = Fixture::new();
        add_profile(&fixture, &fixture.account);
        let old = fixture.gateway.state.lock().unwrap().config.clone();
        let migrated = old.clone().upgrade_applied().unwrap();
        for change in [
            "wire",
            "account",
            "alias",
            "duplicate",
            "missing",
            "unknown-field",
            "version",
            "bad-digest",
        ] {
            let mut value = serde_json::to_value(&migrated).unwrap();
            match change {
                "wire" => {
                    value["models"][0]["url"] =
                        "https://opencode.ai/inference/go/openai/v1/chat/completions".into()
                }
                "account" => value["applied_evidence"][0]["account"] = "e".repeat(32).into(),
                "alias" => value["applied_evidence"][0]["alias"] = "fixture-wire".into(),
                "duplicate" => {
                    let entry = value["applied_evidence"][0].clone();
                    value["applied_evidence"]
                        .as_array_mut()
                        .unwrap()
                        .push(entry);
                }
                "missing" => {
                    value["applied_evidence"].as_array_mut().unwrap().remove(0);
                }
                "unknown-field" => {
                    value["applied_evidence"][0]["origin"]["checked_unix_ms"] = 1.into()
                }
                "version" => value["version"] = 2.into(),
                "bad-digest" => {
                    value["applied_evidence"][0]["origin"]["snapshot_digest"] = "bad".into()
                }
                _ => unreachable!(),
            }
            assert!(
                Config::decode_applied(&serde_json::to_vec(&value).unwrap()).is_err(),
                "accepted {change}"
            );
        }
        let mut invalid_legacy = serde_json::to_value(&old).unwrap();
        let duplicate = invalid_legacy["models"][0].clone();
        invalid_legacy["models"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert!(Config::decode_applied(&serde_json::to_vec(&invalid_legacy).unwrap()).is_err());
        assert_eq!(
            Config::fresh()
                .unwrap()
                .upgrade_applied()
                .unwrap()
                .applied_evidence,
            Vec::new()
        );
    }

    #[test]
    fn cs02_all_seven_profiles_and_unresolved_accounts_survive_memory_migration() {
        let fixture = Fixture::new();
        let mut config = fixture.gateway.state.lock().unwrap().config.clone();
        config.version = 2;
        config.selected_account = Some("f".repeat(32)); // deliberately not stored
        for index in 1..=7 {
            config.profiles.push(ClientProfile {
                id: format!("{index:032x}"),
                label: format!("Client {index}"),
                enabled: index != 7,
                default_model: "default-work".into(),
                allocations: (0..32)
                    .map(|alias| ClientAllocation {
                        alias: if alias == 0 {
                            "default-work".into()
                        } else {
                            format!("work-{alias}")
                        },
                        account: "e".repeat(32),
                        route: config.model.clone(),
                    })
                    .collect(),
            });
        }
        let migrated = Config::decode_applied(&config.encode_applied().unwrap()).unwrap();
        assert_eq!(migrated.profiles, config.profiles);
        assert_eq!(migrated.selected_account, config.selected_account);
        assert_eq!(migrated.applied_evidence.len(), 1 + 7 * 32);
        assert_eq!(
            migrated
                .resolve_allocation("default", None)
                .unwrap()
                .account,
            "f".repeat(32)
        );
        for profile in &config.profiles {
            assert_eq!(
                migrated
                    .resolve_allocation(&profile.id, None)
                    .unwrap()
                    .alias,
                profile.default_model
            );
        }
        let mut over = migrated.clone();
        over.profiles.push(over.profiles[0].clone());
        over.profiles.last_mut().unwrap().id = "8".repeat(32);
        assert_eq!(over.validate(), Err(ServerError::InvalidConfig));
        let mut over = migrated.clone();
        over.profiles[0].allocations.push(ClientAllocation {
            alias: "thirty-third".into(),
            account: "e".repeat(32),
            route: config.model,
        });
        assert_eq!(over.validate(), Err(ServerError::InvalidConfig));
    }

    #[test]
    fn cs02_config_encode_bound_blocks_publication_and_keeps_live_binding() {
        let fixture = Fixture::new();
        let server = fixture.server();
        let before = server.get("/v1/minicon/binding", "x-minicon-session: cs02-size\r\n");
        let old = fixture.gateway.state.lock().unwrap().config.clone();
        let mut config = old.clone().upgrade_applied().unwrap();
        let route = config.models[0].clone();
        for index in 1..MAX_MODELS {
            let mut extra = route.clone();
            extra.id = format!("extra-{index}");
            extra.url = format!(
                "https://opencode.ai/inference/{}/chat/completions",
                "a".repeat(2400)
            );
            config.models.push(extra);
        }
        config.validate().unwrap();
        assert!(serde_json::to_vec(&config).unwrap().len() > CONFIG_BYTES);
        let called = std::sync::atomic::AtomicBool::new(false);
        assert_eq!(
            fixture.gateway.update_and_publish(config, || {
                called.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(ConfigPublication::Durable)
            }),
            Err(ServerError::InvalidConfig)
        );
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(fixture.gateway.state.lock().unwrap().config, old);
        assert_eq!(
            server.get("/v1/minicon/binding", "x-minicon-session: cs02-size\r\n"),
            before
        );
    }

    #[test]
    fn cs02_maximum_legacy_file_still_loads_when_provenance_would_exceed_the_write_bound() {
        let fixture = Fixture::new();
        let mut config = fixture.gateway.state.lock().unwrap().config.clone();
        config.version = 2;
        for profile in 1..=7 {
            config.profiles.push(ClientProfile {
                id: format!("{profile:032x}"),
                label: format!("Client {profile}"),
                enabled: true,
                default_model: "work-0".into(),
                allocations: (0..32)
                    .map(|index| ClientAllocation {
                        alias: format!("work-{index}"),
                        account: fixture.account.clone(),
                        route: config.model.clone(),
                    })
                    .collect(),
            });
        }
        let base = config.models[0].clone();
        for index in 1..MAX_MODELS {
            let mut row = base.clone();
            row.id = format!("spare-{index}");
            config.models.push(row);
        }
        let mut low: usize = 0;
        let mut high = 7000;
        while low < high {
            let middle = (low + high).div_ceil(2);
            let url = format!(
                "https://opencode.ai/inference/{}/chat/completions",
                "p".repeat(middle)
            );
            for row in &mut config.models[1..] {
                row.url.clone_from(&url);
            }
            if serde_json::to_vec(&config).unwrap().len() <= CONFIG_BYTES {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        let url = format!(
            "https://opencode.ai/inference/{}/chat/completions",
            "p".repeat(low)
        );
        for row in &mut config.models[1..] {
            row.url.clone_from(&url);
        }
        let bytes = config.encode_applied().unwrap();
        assert!(bytes.len() > CONFIG_BYTES - 1024);
        let migrated = Config::decode_applied(&bytes);
        assert!(
            migrated.is_ok(),
            "valid legacy file must migrate before write-size checks: {:?}",
            migrated.as_ref().err()
        );
        let migrated = migrated.unwrap();
        assert!(serde_json::to_vec(&migrated).unwrap().len() > CONFIG_BYTES);
        assert_eq!(migrated.encode_applied(), Err(ServerError::InvalidConfig));
        let gateway = Gateway::new(
            fixture.gateway.store.clone(),
            migrated,
            Some(Secret::new("synthetic-local").unwrap()),
        )
        .unwrap();
        let server = local::Server::start(0, gateway.clone()).unwrap();
        gateway.state.lock().unwrap().config.port = server.address().port();
        let response = ServerFixture { server }.get("/v1/minicon/binding", "");
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    }

    #[test]
    fn cs02_gateway_will_not_copy_legacy_provenance_into_a_new_account_or_profile() {
        let fixture = Fixture::new();
        let server = fixture.server();
        let session = "x-minicon-session: cs02-legacy-copy\r\n";
        let before = server.get("/v1/minicon/binding", session);
        let original = fixture.gateway.state.lock().unwrap().config.clone();
        let migrated = original.clone().upgrade_applied().unwrap();
        fixture.gateway.update(migrated.clone()).unwrap();
        for change in ["account", "profile"] {
            let mut config = migrated.clone();
            if change == "account" {
                config.selected_account = Some("e".repeat(32));
                config.applied_evidence[0].account = "e".repeat(32);
            } else {
                let id = "c".repeat(32);
                config.profiles.push(ClientProfile {
                    id: id.clone(),
                    label: "Unverified new client".into(),
                    enabled: true,
                    default_model: "copied".into(),
                    allocations: vec![ClientAllocation {
                        alias: "copied".into(),
                        account: fixture.account.clone(),
                        route: config.model.clone(),
                    }],
                });
                let mut copied = config.applied_evidence[0].clone();
                copied.profile = id;
                copied.alias = "copied".into();
                config.applied_evidence.push(copied);
            }
            // The document is locally consistent, but history is false. The
            // live transition guard must reject before calling its writer.
            config.validate().unwrap();
            let called = std::sync::atomic::AtomicBool::new(false);
            assert!(
                fixture
                    .gateway
                    .update_and_publish(config, || {
                        called.store(true, std::sync::atomic::Ordering::SeqCst);
                        Ok(ConfigPublication::Durable)
                    })
                    .is_err()
            );
            assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
            assert_eq!(fixture.gateway.state.lock().unwrap().config, migrated);
            assert_eq!(server.get("/v1/minicon/binding", session), before);
        }
    }

    #[test]
    fn cs02_live_v3_cannot_downgrade_then_mint_legacy_for_a_changed_binding() {
        let fixture = Fixture::new();
        let profile = add_profile(&fixture, &fixture.account);
        let server = fixture.server();
        let session = "x-minicon-session: cs02-downgrade\r\n";
        let before = server.get("/v1/minicon/binding", session);
        let custom_before = server.get_key("/v1/minicon/binding", "synthetic-other", session);
        assert!(before.starts_with("HTTP/1.1 200"));
        assert!(custom_before.starts_with("HTTP/1.1 200"));
        let old = fixture.gateway.state.lock().unwrap().config.clone();
        let applied = old.upgrade_applied().unwrap();
        fixture.gateway.update(applied.clone()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        *fixture.gateway.fixture_upstream.lock().unwrap() = Some(format!(
            "http://{}/inference",
            listener.local_addr().unwrap()
        ));
        thread::scope(|scope| {
            let peer = scope.spawn(|| {
                for _ in 0..2 {
                    let (stream, headers, body) = controlled_peer_request(&listener);
                    assert!(headers.starts_with("POST "));
                    assert_eq!(body["model"], "fixture-wire");
                    reply_controlled_peer(stream, json!({"id":"unchanged","model":"fixture-wire","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"STILL_ORIGINAL"},"finish_reason":"stop"}]}));
                }
            });
            for version in [1, 2] {
                let mut downgraded = applied.clone();
                downgraded.version = version;
                downgraded.default_route = None;
                downgraded.applied_evidence.clear();
                downgraded.selected_account = Some("e".repeat(32));
                downgraded.models[0].model_id = "unverified-new-model".into();
                if version == 1 {
                    downgraded.profiles.clear();
                }
                downgraded.validate().unwrap();
                let writer_called = std::sync::atomic::AtomicBool::new(false);
                let result = fixture.gateway.update_and_publish(downgraded.clone(), || {
                    writer_called.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(ConfigPublication::Durable)
                });
                assert_eq!(result, Err(ServerError::InvalidConfig));
                assert!(!writer_called.load(std::sync::atomic::Ordering::SeqCst));
                assert_eq!(fixture.gateway.state.lock().unwrap().config, applied);
                // A second-stage re-import must not launder the new connection
                // as historical even when called as a separate operation.
                let laundered = downgraded.upgrade_applied().unwrap();
                let writer_called = std::sync::atomic::AtomicBool::new(false);
                assert!(
                    fixture
                        .gateway
                        .update_and_publish(laundered, || {
                            writer_called.store(true, std::sync::atomic::Ordering::SeqCst);
                            Ok(ConfigPublication::Durable)
                        })
                        .is_err()
                );
                assert!(!writer_called.load(std::sync::atomic::Ordering::SeqCst));
                assert_eq!(server.get("/v1/minicon/binding", session), before);
                assert_eq!(
                    server.get_key("/v1/minicon/binding", "synthetic-other", session),
                    custom_before
                );
                assert!(
                    fixture
                        .gateway
                        .state
                        .lock()
                        .unwrap()
                        .config
                        .profiles
                        .iter()
                        .any(|entry| entry.id == profile)
                );
                let body = json!({"model":applied.model,"messages":[{"role":"user","content":"synthetic"}],"max_tokens":1,"stream":false}).to_string();
                let mut stream = server.request("/v1/chat/completions", session, Some(&body));
                let mut response = String::new();
                stream.read_to_string(&mut response).unwrap();
                assert!(response.starts_with("HTTP/1.1 200"), "{response}");
                assert!(response.contains("STILL_ORIGINAL"));
            }
            peer.join().unwrap();
        });
    }

    #[test]
    fn cs01_real_catalogue_deadline_spans_all_metadata_exchanges() {
        let fixture = Fixture::new();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        *fixture.gateway.fixture_upstream.lock().unwrap() = Some(format!(
            "http://{}/metadata",
            listener.local_addr().unwrap()
        ));
        let stopping = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = stopping.clone();
        struct OwnedPeer {
            stopping: Arc<std::sync::atomic::AtomicBool>,
            worker: Option<thread::JoinHandle<usize>>,
        }
        impl OwnedPeer {
            fn finish(mut self) -> usize {
                self.stopping
                    .store(true, std::sync::atomic::Ordering::Release);
                self.worker.take().unwrap().join().unwrap()
            }
        }
        impl Drop for OwnedPeer {
            fn drop(&mut self) {
                self.stopping
                    .store(true, std::sync::atomic::Ordering::Release);
                if let Some(worker) = self.worker.take() {
                    let result = worker.join();
                    if !thread::panicking() {
                        assert!(result.is_ok());
                    }
                }
            }
        }
        let worker = thread::spawn(move || {
            let mut count = 0;
            while !stop.load(std::sync::atomic::Ordering::Acquire) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("bounded metadata accept: {error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(4)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(4)))
                    .unwrap();
                let request = read_headers(&mut stream);
                assert!(request.starts_with("GET /metadata "));
                assert!(
                    request
                        .to_lowercase()
                        .contains("authorization: bearer synthetic-access")
                );
                let body = match count {
                    0 => br#"{"id":"synthetic","email":"synthetic.invalid"}"#.to_vec(),
                    1 => b"[]".to_vec(),
                    _ => br#"{"providers":{"remote":{"package":"@opencode/ai/providers/openai-compatible","settings":{"baseURL":"https://opencode.ai/inference/fixture/v1"},"models":{"same":{"modelID":"wire"}}}}}"#.to_vec(),
                };
                count += 1;
                thread::sleep(Duration::from_millis(350));
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream
                    .write_all(header.as_bytes())
                    .and_then(|()| stream.write_all(&body));
            }
            count
        });
        let peer = OwnedPeer {
            stopping,
            worker: Some(worker),
        };
        let snapshot = fixture.gateway.store.snapshot(&fixture.account).unwrap();
        let requested = ResolvedAuthority::from_snapshot(&snapshot).unwrap();
        let started = Instant::now();
        let result = fixture.gateway.catalogue_budgeted(
            &fixture.account,
            &requested,
            &Arc::new(NetworkHttpCancel::new()),
            CatalogueBudget {
                deadline: Instant::now() + Duration::from_millis(600),
                response_bytes: 0,
            },
        );
        let requests = peer.finish();
        assert!(
            result.is_err(),
            "three individually short exchanges outlived the shared deadline"
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(
            requests < 3,
            "metadata chain reached config after the global deadline"
        );
    }

    #[test]
    fn loopback_admission_and_binding_stay_authenticated_and_sticky() {
        let fixture = Fixture::new();
        let server = fixture.server();
        assert!(server.get("/v1/models", "").contains(MODEL_ALIAS));
        assert!(
            server
                .get("/v1/minicon/binding", "x-minicon-session: stable\r\n")
                .starts_with("HTTP/1.1 200")
        );
        assert!(
            server
                .get("/v1/models", "Origin: https://untrusted.invalid\r\n")
                .starts_with("HTTP/1.1 403")
        );
        assert!(
            server
                .get("/v1/models", "Authorization: Bearer another\r\n")
                .starts_with("HTTP/1.1 401")
        );
        let mut config = fixture.gateway.state.lock().unwrap().config.clone();
        config.models[0].model_id = "changed-wire".into();
        fixture.gateway.update(config).unwrap();
        assert!(
            server
                .get("/v1/minicon/binding", "x-minicon-session: stable\r\n")
                .starts_with("HTTP/1.1 409")
        );
        assert!(
            server
                .get("/v1/minicon/binding", "x-minicon-session: fresh\r\n")
                .contains("changed-wire")
        );
        let binding_json = |response: String| -> Value {
            serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
        };
        let before =
            binding_json(server.get("/v1/minicon/binding", "x-minicon-session: fresh\r\n"));
        fixture
            .gateway
            .set_key(Some(Secret::new("synthetic-local").unwrap()))
            .unwrap();
        let unchanged =
            binding_json(server.get("/v1/minicon/binding", "x-minicon-session: fresh\r\n"));
        assert_eq!(before["generation"], unchanged["generation"]);
        fixture.gateway.set_key(None).unwrap();
        fixture
            .gateway
            .set_key(Some(Secret::new("synthetic-local").unwrap()))
            .unwrap();
        let after = binding_json(server.get("/v1/minicon/binding", "x-minicon-session: fresh\r\n"));
        assert_ne!(
            before["generation"], after["generation"],
            "lost upstream session mapping must invalidate worker resume even when account/model are unchanged"
        );
        for field in ["profile", "provider", "account", "model"] {
            assert_eq!(before[field], after[field]);
        }
    }

    #[test]
    fn default_account_aba_cannot_revive_an_old_conversation() {
        let fixture = Fixture::new();
        add_profile(&fixture, &fixture.account);
        let other = fixture
            .gateway
            .store
            .insert(
                "Second synthetic account",
                TokenGrant {
                    provider: Provider::OpenCode,
                    access_token: Secret::new("synthetic-other-access").unwrap(),
                    lifecycle: TokenLifecycle::ExpiringRefresh {
                        refresh_token: Secret::new("synthetic-other-refresh").unwrap(),
                        expires_at: Instant::now() + Duration::from_secs(3600),
                    },
                    organization: None,
                },
            )
            .unwrap();
        fixture.gateway.store.mark_ready(&other.id).unwrap();
        let server = fixture.server();
        let session = "x-minicon-session: account-aba\r\n";
        assert!(
            server
                .get("/v1/minicon/binding", session)
                .starts_with("HTTP/1.1 200")
        );
        let custom = server.get_key("/v1/minicon/binding", "synthetic-other", session);
        assert!(custom.starts_with("HTTP/1.1 200"));
        let mut config = fixture.gateway.state.lock().unwrap().config.clone();
        for account in [&other.id, &fixture.account] {
            config.selected_account = Some(account.clone());
            fixture.gateway.update(config.clone()).unwrap();
            assert_eq!(
                server
                    .get("/v1/minicon/binding", session)
                    .split_whitespace()
                    .nth(1),
                Some("409"),
                "returning to an earlier account must not revive its old conversation"
            );
            assert_eq!(
                server.get_key("/v1/minicon/binding", "synthetic-other", session),
                custom
            );
        }
        assert!(
            server
                .get(
                    "/v1/minicon/binding",
                    "x-minicon-session: account-aba-fresh\r\n"
                )
                .starts_with("HTTP/1.1 200")
        );
    }

    #[test]
    fn registry_metadata_migration_preserves_default_and_custom_conversations() {
        let fixture = Fixture::new();
        add_profile(&fixture, &fixture.account);
        let server = fixture.server();
        let session = "x-minicon-session: registry-migration\r\n";
        let default_before = server.get("/v1/minicon/binding", session);
        let custom_before = server.get_key("/v1/minicon/binding", "synthetic-other", session);
        assert!(default_before.starts_with("HTTP/1.1 200"));
        assert!(custom_before.starts_with("HTTP/1.1 200"));
        let mut config = fixture.gateway.state.lock().unwrap().config.clone();
        // Private registry identity and an ignored header hint are metadata.
        // route_request obtains the actual organization from the account grant.
        config.models[0].id = "snapshot/private-registry-key".into();
        config.models[0].headers = vec![("X-Org-ID".into(), "ignored-synthetic-hint".into())];
        config.model = config.models[0].id.clone();
        config.profiles[0].allocations[0].route = config.models[0].id.clone();
        config.profiles[0].label = "Renamed client".into();
        fixture.gateway.update(config.clone()).unwrap();
        assert_eq!(
            server.get("/v1/minicon/binding", session),
            default_before,
            "registry-only migration must preserve the default conversation"
        );
        assert_eq!(
            server.get_key("/v1/minicon/binding", "synthetic-other", session),
            custom_before,
            "registry-only migration must preserve the custom conversation"
        );
        let mut changed = config.models[0].clone();
        changed.id = "snapshot/changed-wire".into();
        changed.url = "https://opencode.ai/inference/go/openai/v1/chat/completions".into();
        config.profiles[0].allocations[0].route = changed.id.clone();
        config.models.push(changed);
        fixture.gateway.update(config).unwrap();
        assert!(
            server
                .get_key("/v1/minicon/binding", "synthetic-other", session)
                .starts_with("HTTP/1.1 409"),
            "changed effective custom wire must reject the old conversation"
        );
        assert_eq!(server.get("/v1/minicon/binding", session), default_before);
        assert!(
            server
                .get_key(
                    "/v1/minicon/binding",
                    "synthetic-other",
                    "x-minicon-session: after-migration\r\n"
                )
                .starts_with("HTTP/1.1 200")
        );
    }

    #[test]
    fn selected_full_route_identity_invalidates_binding_but_unselected_changes_do_not() {
        let fixture = Fixture::new();
        let server = fixture.server();
        let read_binding = |response: String| -> Value {
            assert!(response.starts_with("HTTP/1.1 200"));
            serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
        };
        let before =
            read_binding(server.get("/v1/minicon/binding", "x-minicon-session: route-stable\r\n"));
        let mut config = fixture.gateway.state.lock().unwrap().config.clone();
        let mut alternative = config.models[0].clone();
        alternative.id = "opencode/other/fixture".into();
        alternative.protocol = WireProtocol::Messages;
        alternative.url = "https://opencode.ai/inference/anthropic/v1/messages".into();
        config.models.push(alternative.clone());
        fixture.gateway.update(config.clone()).unwrap();
        let unchanged =
            read_binding(server.get("/v1/minicon/binding", "x-minicon-session: route-stable\r\n"));
        assert_eq!(before, unchanged);
        config.models[1].url = "https://opencode.ai/inference/go/anthropic/v1/messages".into();
        fixture.gateway.update(config.clone()).unwrap();
        let unchanged =
            read_binding(server.get("/v1/minicon/binding", "x-minicon-session: route-stable\r\n"));
        assert_eq!(before, unchanged);
        config.model = alternative.id;
        fixture.gateway.update(config).unwrap();
        assert!(
            server
                .get("/v1/minicon/binding", "x-minicon-session: route-stable\r\n")
                .starts_with("HTTP/1.1 409")
        );
        let after =
            read_binding(server.get("/v1/minicon/binding", "x-minicon-session: route-fresh\r\n"));
        assert_ne!(before["generation"], after["generation"]);
        for field in ["profile", "provider", "account", "model"] {
            assert_eq!(before[field], after[field]);
        }
    }
    #[test]
    fn account_network_edits_invalidate_only_service_bindings_and_preserve_admitted_policy() {
        let fixture = Fixture::new();
        let server = fixture.server();
        let before = read_binding(server.get(
            "/v1/minicon/binding",
            "x-minicon-session: policy-stable\r\n",
        ));
        let expected = fixture.gateway.store.preferences(&fixture.account).unwrap();
        let mut edit = AccountEdit {
            account: fixture.account.clone(),
            label: "Renamed account".into(),
            login: NetworkPolicy::from_input("http://login.invalid:8080").unwrap(),
            service: NetworkPolicy::Direct,
            enabled: true,
            expected: expected.clone(),
            expected_label: "Synthetic fixture".into(),
        };
        fixture.gateway.update_account(&edit).unwrap();
        assert_eq!(
            before,
            read_binding(server.get(
                "/v1/minicon/binding",
                "x-minicon-session: policy-stable\r\n"
            ))
        );
        // A stale settings form cannot undo another save or change the network.
        assert_eq!(
            fixture.gateway.update_account(&edit),
            Err(ServerError::BindingChanged)
        );
        let after_login = fixture.gateway.store.preferences(&fixture.account).unwrap();
        assert_eq!(after_login.service_generation, expected.service_generation);
        assert_eq!(after_login.context_id, expected.context_id);
        let cancel = Arc::new(NetworkHttpCancel::new());
        let lease =
            BorrowedLease::acquire(&fixture.gateway, &fixture.account, &cancel, None).unwrap();
        edit.expected = after_login;
        edit.expected_label = edit.label.clone();
        edit.service = NetworkPolicy::from_input("http://service.invalid:8081").unwrap();
        fixture.gateway.update_account(&edit).unwrap();
        assert!(!cancel.is_cancelled());
        assert_eq!(lease.preferences.service, NetworkPolicy::Direct);
        assert!(
            server
                .get(
                    "/v1/minicon/binding",
                    "x-minicon-session: policy-stable\r\n"
                )
                .starts_with("HTTP/1.1 409")
        );
        let fresh =
            read_binding(server.get("/v1/minicon/binding", "x-minicon-session: policy-fresh\r\n"));
        assert_ne!(before["generation"], fresh["generation"]);
        let fresh_cancel = Arc::new(NetworkHttpCancel::new());
        let fresh_lease =
            BorrowedLease::acquire(&fixture.gateway, &fixture.account, &fresh_cancel, None)
                .unwrap();
        assert_eq!(fresh_lease.preferences.service, edit.service);
        edit.expected = fixture.gateway.store.preferences(&fixture.account).unwrap();
        edit.enabled = false;
        fixture.gateway.update_account(&edit).unwrap();
        assert!(cancel.is_cancelled());
        assert!(fresh_cancel.is_cancelled());
        let disabled_models = read_binding(server.get("/v1/models", ""));
        assert_eq!(disabled_models["data"], json!([]));
        assert!(disabled_models["binding"].is_null());
        assert!(
            server
                .get("/v1/minicon/binding", "x-minicon-session: disabled\r\n")
                .starts_with("HTTP/1.1 401")
        );
        assert!(matches!(
            BorrowedLease::acquire(
                &fixture.gateway,
                &fixture.account,
                &Arc::new(NetworkHttpCancel::new()),
                None
            ),
            Err(ServerError::NeedsLogin)
        ));
        drop(fresh_lease);
        drop(lease);
        // Re-enabling creates a new generation; it cannot resurrect old sessions.
        edit.expected = fixture.gateway.store.preferences(&fixture.account).unwrap();
        edit.enabled = true;
        fixture.gateway.update_account(&edit).unwrap();
        assert!(
            server
                .get("/v1/minicon/binding", "x-minicon-session: policy-fresh\r\n")
                .starts_with("HTTP/1.1 409")
        );
        assert!(server.get("/v1/models", "").starts_with("HTTP/1.1 200"));
    }

    #[test]
    fn service_proxy_is_account_scoped_and_failed_proxy_never_falls_back_direct() {
        let fixture = Fixture::new();
        let direct = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
        let target = direct.local_addr().unwrap();
        *fixture.gateway.fixture_upstream.lock().unwrap() = Some(format!("http://{target}/"));
        let second = fixture
            .gateway
            .store
            .insert(
                "Direct account",
                TokenGrant {
                    provider: Provider::OpenCode,
                    access_token: Secret::new("synthetic-direct-grant").unwrap(),
                    lifecycle: TokenLifecycle::ExpiringRefresh {
                        refresh_token: Secret::new("synthetic-direct-refresh").unwrap(),
                        expires_at: Instant::now() + Duration::from_secs(3600),
                    },
                    organization: None,
                },
            )
            .unwrap();
        fixture.gateway.store.mark_ready(&second.id).unwrap();
        add_profile(&fixture, &second.id);
        let mut edit = AccountEdit {
            account: fixture.account.clone(),
            label: "Synthetic fixture".into(),
            login: NetworkPolicy::from_input("http://login-only.invalid:8080").unwrap(),
            service: NetworkPolicy::from_input(&format!("http://{}", proxy.local_addr().unwrap()))
                .unwrap(),
            enabled: true,
            expected: fixture.gateway.store.preferences(&fixture.account).unwrap(),
            expected_label: "Synthetic fixture".into(),
        };
        fixture.gateway.update_account(&edit).unwrap();
        let (release, wait) = mpsc::channel();
        thread::scope(|scope| {
            let proxy_peer = scope.spawn(move || {
                let mut stream = accept_upstream(&proxy);
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
                stream.set_write_timeout(Some(Duration::from_secs(4))).unwrap();
                let connect = read_headers(&mut stream);
                assert!(connect.starts_with(&format!("CONNECT {target} HTTP/1.1\r\n")), "{connect}");
                assert!(!connect.to_ascii_lowercase().contains("authorization:"));
                stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").unwrap();
                upstream_request(&mut stream);
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{FIRST}", FIRST.len() + END.len()).unwrap();
                stream.flush().unwrap();
                wait.recv_timeout(Duration::from_secs(4)).unwrap();
                stream.write_all(END.as_bytes()).unwrap();
            });
            let direct_peer = scope.spawn(|| {
                for token in ["synthetic-direct-grant", "synthetic-access"] {
                    let mut stream = accept_upstream(&direct);
                    stream.set_nonblocking(false).unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
                    stream.set_write_timeout(Some(Duration::from_secs(4))).unwrap();
                    let headers = read_headers(&mut stream);
                    assert!(headers.starts_with("POST / HTTP/1.1\r\n"));
                    assert!(headers.to_ascii_lowercase().contains(&format!("authorization: bearer {token}\r\n")));
                    let length: usize = headers.lines().find_map(|line| line.split_once(':')
                        .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                        .map(|(_, value)| value.trim().parse().unwrap())).unwrap();
                    let mut body = vec![0; length];
                    stream.read_exact(&mut body).unwrap();
                    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["model"], "fixture-wire");
                    let response = r#"{"id":"synthetic","model":"fixture-wire","choices":[{"index":0,"message":{"role":"assistant","content":"direct"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
                }
            });
            let server = fixture.server();
            let mut ongoing = server.request(
                "/v1/chat/completions",
                "x-minicon-session: old-proxy\r\n",
                Some(BODY),
            );
            assert!(read_headers(&mut ongoing).starts_with("HTTP/1.1 200"));
            assert_eq!(
                sse_event(&mut ongoing)["choices"][0]["delta"]["content"],
                "hello"
            );
            edit.expected = fixture.gateway.store.preferences(&fixture.account).unwrap();
            edit.service = NetworkPolicy::Direct;
            fixture.gateway.update_account(&edit).unwrap();
            // Existing I/O finishes through its original proxy, while new A
            // requests and unrelated B requests use their own Direct policy.
            for (key, model, session) in [
                ("synthetic-other", "work", "direct-b"),
                ("synthetic-local", MODEL_ALIAS, "new-direct-a"),
            ] {
                let body = format!(
                    r#"{{"model":"{model}","stream":false,"messages":[{{"role":"user","content":"test"}}]}}"#
                );
                let mut client = server.request_key(
                    "/v1/chat/completions",
                    key,
                    &format!("x-minicon-session: {session}\r\n"),
                    Some(&body),
                );
                let mut response = String::new();
                client.read_to_string(&mut response).unwrap();
                assert!(response.starts_with("HTTP/1.1 200"), "{response}");
                assert!(response.contains("direct"));
            }
            release.send(()).unwrap();
            assert_eq!(
                sse_event(&mut ongoing)["choices"][0]["finish_reason"],
                "stop"
            );
            let mut tail = Vec::new();
            ongoing.read_to_end(&mut tail).unwrap();
            assert_eq!(tail, b"data: [DONE]\n\n");
            proxy_peer.join().unwrap();
            direct_peer.join().unwrap();
            let unreachable = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}", unreachable.local_addr().unwrap());
            drop(unreachable);
            edit.expected = fixture.gateway.store.preferences(&fixture.account).unwrap();
            edit.service = NetworkPolicy::from_input(&endpoint).unwrap();
            fixture.gateway.update_account(&edit).unwrap();
            let body = BODY.replace("true", "false");
            let mut client = server.request(
                "/v1/chat/completions",
                "x-minicon-session: failed-proxy\r\n",
                Some(&body),
            );
            let mut response = String::new();
            client.read_to_string(&mut response).unwrap();
            assert!(response.starts_with("HTTP/1.1 502"), "{response}");
            direct.set_nonblocking(true).unwrap();
            assert!(
                matches!(direct.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
            );
        });
    }

    #[test]
    fn account_lease_is_bounded_and_revoke_cancels_registered_io() {
        let fixture = Fixture::new();
        let (binding, _, _) = fixture
            .gateway
            .sticky(&head("synthetic-local"), None)
            .unwrap();
        let one = Arc::new(NetworkHttpCancel::new());
        let two = Arc::new(NetworkHttpCancel::new());
        let first = BorrowedLease::acquire(
            &fixture.gateway,
            &fixture.account,
            &one,
            Some((&head("synthetic-local"), &binding)),
        )
        .unwrap();
        let second = BorrowedLease::acquire(
            &fixture.gateway,
            &fixture.account,
            &two,
            Some((&head("synthetic-local"), &binding)),
        )
        .unwrap();
        assert!(matches!(
            BorrowedLease::acquire(
                &fixture.gateway,
                &fixture.account,
                &Arc::new(NetworkHttpCancel::new()),
                None
            ),
            Err(ServerError::Busy)
        ));
        fixture.gateway.set_key(None).unwrap();
        assert!(one.is_cancelled());
        assert!(two.is_cancelled());
        drop(first);
        drop(second);
        assert!(matches!(
            BorrowedLease::acquire(
                &fixture.gateway,
                &fixture.account,
                &Arc::new(NetworkHttpCancel::new()),
                Some((&head("synthetic-local"), &binding))
            ),
            Err(ServerError::MissingKey)
        ));
    }

    fn add_profile(fixture: &Fixture, account: &str) -> String {
        let id = "a".repeat(32);
        let mut config = fixture.gateway.state.lock().unwrap().config.clone();
        config.version = 2;
        config.profiles.push(ClientProfile {
            id: id.clone(),
            label: "Synthetic client".into(),
            enabled: true,
            default_model: "work".into(),
            allocations: vec![ClientAllocation {
                alias: "work".into(),
                account: account.to_owned(),
                route: config.model.clone(),
            }],
        });
        fixture.gateway.update(config).unwrap();
        fixture
            .gateway
            .set_client_key(&id, Some(Secret::new("synthetic-other").unwrap()))
            .unwrap();
        id
    }

    #[test]
    fn organization_authority_aba_refuses_old_sessions_without_cancelling_admitted_sse_or_other_accounts()
     {
        let fixture = Fixture::new();
        let snapshot = fixture.gateway.store.snapshot(&fixture.account).unwrap();
        fixture
            .gateway
            .store
            .set_organization_if_current(
                &fixture.account,
                &snapshot.generation,
                Some("org_a".into()),
            )
            .unwrap();
        let other = fixture
            .gateway
            .store
            .insert(
                "Unaffected",
                TokenGrant {
                    provider: Provider::OpenCode,
                    access_token: Secret::new("synthetic-other-access").unwrap(),
                    lifecycle: TokenLifecycle::ExpiringRefresh {
                        refresh_token: Secret::new("synthetic-other-refresh").unwrap(),
                        expires_at: Instant::now() + Duration::from_secs(3600),
                    },
                    organization: None,
                },
            )
            .unwrap();
        fixture.gateway.store.mark_ready(&other.id).unwrap();
        add_profile(&fixture, &other.id);
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        *fixture.gateway.fixture_upstream.lock().unwrap() =
            Some(format!("http://{}/", upstream.local_addr().unwrap()));
        let server = fixture.server();
        let original =
            read_binding(server.get("/v1/minicon/binding", "x-minicon-session: org-a\r\n"));
        let independent = read_binding(server.get_key(
            "/v1/minicon/binding",
            "synthetic-other",
            "x-minicon-session: independent\r\n",
        ));
        let mut ongoing = server.request(
            "/v1/chat/completions",
            "x-minicon-session: org-a\r\n",
            Some(BODY),
        );
        let (mut peer, headers, body) = controlled_peer_request(&upstream);
        assert!(headers.to_ascii_lowercase().contains("x-org-id: org_a\r\n"));
        assert_eq!(body["model"], "fixture-wire");
        write!(peer, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{FIRST}", FIRST.len() + END.len()).unwrap();
        peer.flush().unwrap();
        assert!(read_headers(&mut ongoing).starts_with("HTTP/1.1 200"));
        assert_eq!(
            sse_event(&mut ongoing)["choices"][0]["delta"]["content"],
            "hello"
        );

        fixture
            .gateway
            .store
            .set_organization_if_current(
                &fixture.account,
                &snapshot.generation,
                Some("org_b".into()),
            )
            .unwrap();
        assert!(
            server
                .get("/v1/minicon/binding", "x-minicon-session: org-a\r\n")
                .starts_with("HTTP/1.1 409")
        );
        fixture
            .gateway
            .store
            .set_organization_if_current(
                &fixture.account,
                &snapshot.generation,
                Some("org_a".into()),
            )
            .unwrap();
        assert!(
            server
                .get("/v1/minicon/binding", "x-minicon-session: org-a\r\n")
                .starts_with("HTTP/1.1 409")
        );
        let fresh =
            read_binding(server.get("/v1/minicon/binding", "x-minicon-session: org-a-new\r\n"));
        assert_ne!(fresh["generation"], original["generation"]);
        for field in ["profile", "alias", "provider", "account", "model"] {
            assert_eq!(fresh[field], original[field]);
        }
        assert_eq!(
            read_binding(server.get_key(
                "/v1/minicon/binding",
                "synthetic-other",
                "x-minicon-session: independent\r\n"
            )),
            independent
        );
        assert_eq!(
            fixture
                .gateway
                .store
                .snapshot(&fixture.account)
                .unwrap()
                .generation,
            snapshot.generation
        );
        peer.write_all(END.as_bytes()).unwrap();
        peer.flush().unwrap();
        drop(peer);
        assert_eq!(
            sse_event(&mut ongoing)["choices"][0]["finish_reason"],
            "stop"
        );
        let mut tail = Vec::new();
        ongoing.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, b"data: [DONE]\n\n");
    }

    #[test]
    fn organization_authority_changed_by_refresh_is_refused_before_a_bound_model_request() {
        let fixture = Fixture::new();
        let snapshot = fixture.gateway.store.snapshot(&fixture.account).unwrap();
        fixture
            .gateway
            .store
            .replace_grant_if_current(
                &fixture.account,
                &snapshot.generation,
                TokenGrant {
                    provider: Provider::OpenCode,
                    access_token: Secret::new("synthetic-expired-access").unwrap(),
                    lifecycle: TokenLifecycle::ExpiringRefresh {
                        refresh_token: Secret::new("synthetic-refresh").unwrap(),
                        expires_at: Instant::now(),
                    },
                    organization: Some("org_a".into()),
                },
            )
            .unwrap();
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        *fixture.gateway.fixture_upstream.lock().unwrap() =
            Some(format!("http://{}/", upstream.local_addr().unwrap()));
        let server = fixture.server();
        let original =
            read_binding(server.get("/v1/minicon/binding", "x-minicon-session: refresh-org\r\n"));
        let request = BODY.replace("true", "false");
        let mut client = server.request(
            "/v1/chat/completions",
            "x-minicon-session: refresh-org\r\n",
            Some(&request),
        );
        let (refresh, _, body) = controlled_peer_request(&upstream);
        assert_eq!(body["grant_type"], "refresh_token");
        reply_controlled_peer(
            refresh,
            json!({"access_token":"synthetic-new-access","refresh_token":"synthetic-new-refresh","expires_in":3600,"org_id":"org_b"}),
        );

        // Also answer a forbidden inference if the authority guard is removed:
        // the counterexample must fail its 409/no-inference assertions, rather
        // than accidentally failing because a fake provider never replied.
        client.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 4096];
        let mut forbidden_model_requests = 0;
        loop {
            assert!(
                Instant::now() < deadline,
                "bound authority result did not arrive"
            );
            match upstream.accept() {
                Ok((mut peer, _)) => {
                    peer.set_nonblocking(false).unwrap();
                    peer.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
                    peer.set_write_timeout(Some(Duration::from_secs(4)))
                        .unwrap();
                    let headers = read_headers(&mut peer);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.split_once(':')
                                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    assert!(length <= 16384);
                    let mut body = vec![0; length];
                    peer.read_exact(&mut body).unwrap();
                    let model: Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(model["model"], "fixture-wire");
                    forbidden_model_requests += 1;
                    reply_controlled_peer(
                        peer,
                        json!({"id":"synthetic-unexpected","model":"fixture-wire","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}]}),
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("controlled provider accept: {error}"),
            }
            match client.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    assert!(bytes.len() + count <= 65536);
                    bytes.extend_from_slice(&chunk[..count]);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("bound client response: {error}"),
            }
            thread::sleep(Duration::from_millis(5));
        }
        let response = String::from_utf8(bytes).unwrap();
        assert!(response.starts_with("HTTP/1.1 409"), "{response}");
        assert_eq!(
            forbidden_model_requests, 0,
            "changed authority reached inference"
        );
        assert!(
            server
                .get("/v1/minicon/binding", "x-minicon-session: refresh-org\r\n")
                .starts_with("HTTP/1.1 409")
        );
        let fresh =
            read_binding(server.get("/v1/minicon/binding", "x-minicon-session: resolved-org\r\n"));
        assert_ne!(fresh["generation"], original["generation"]);
        let mut client = server.request(
            "/v1/chat/completions",
            "x-minicon-session: resolved-org\r\n",
            Some(&request),
        );
        let (inference, headers, body) = controlled_peer_request(&upstream);
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("authorization: bearer synthetic-new-access\r\n")
        );
        assert!(headers.to_ascii_lowercase().contains("x-org-id: org_b\r\n"));
        assert_eq!(body["model"], "fixture-wire");
        reply_controlled_peer(
            inference,
            json!({"id":"synthetic-new","model":"fixture-wire","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}]}),
        );
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    }

    #[test]
    fn organization_selection_preserves_a_concurrent_refresh_through_actual_catalog_and_probe() {
        let fixture = Fixture::new();
        let before = fixture.gateway.store.snapshot(&fixture.account).unwrap();
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        *fixture.gateway.fixture_upstream.lock().unwrap() =
            Some(format!("http://{}/", upstream.local_addr().unwrap()));
        let gateway = fixture.gateway.clone();
        let account = fixture.account.clone();
        let catalog =
            thread::spawn(move || gateway.catalog(&account, &Arc::new(NetworkHttpCancel::new())));
        let (user, _, _) = controlled_peer_request(&upstream);
        reply_controlled_peer(
            user,
            json!({"id":"synthetic-user","email":"synthetic@fixture.invalid"}),
        );
        // Hold the actual org response after the catalogue captured its grant.
        let (orgs, _, _) = controlled_peer_request(&upstream);
        fixture
            .gateway
            .store
            .replace_grant_if_current(
                &fixture.account,
                &before.generation,
                TokenGrant {
                    provider: Provider::OpenCode,
                    access_token: Secret::new("synthetic-access").unwrap(),
                    lifecycle: TokenLifecycle::ExpiringRefresh {
                        refresh_token: Secret::new("synthetic-refresh").unwrap(),
                        expires_at: Instant::now(),
                    },
                    organization: None,
                },
            )
            .unwrap();
        let gateway = fixture.gateway.clone();
        let route = gateway.state.lock().unwrap().config.models[0].clone();
        let account = fixture.account.clone();
        let probe = thread::spawn(move || {
            gateway.probe(
                &account,
                &route,
                "synthetic-org-probe",
                &Arc::new(NetworkHttpCancel::new()),
            )
        });
        let (refresh, _, body) = controlled_peer_request(&upstream);
        assert_eq!(body["grant_type"], "refresh_token");
        reply_controlled_peer(
            refresh,
            json!({"access_token":"synthetic-refreshed","refresh_token":"synthetic-refreshed-key","expires_in":3600}),
        );
        let (inference, inference_headers, body) = controlled_peer_request(&upstream);
        assert_eq!(body["model"], "fixture-wire");
        assert!(
            inference_headers
                .to_ascii_lowercase()
                .contains("authorization: bearer synthetic-refreshed")
        );
        reply_controlled_peer(
            inference,
            json!({"id":"synthetic-probe","model":"fixture-wire","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}]}),
        );
        probe.join().unwrap().unwrap();
        reply_controlled_peer(
            orgs,
            json!([{"id":"org_selected","name":"Synthetic workspace"}]),
        );
        let (config, config_headers, _) = controlled_peer_request(&upstream);
        reply_controlled_peer(
            config,
            json!({"providers":{"remote":{
                "package":"@opencode/ai/providers/openai-compatible",
                "settings":{"baseURL":"https://opencode.ai/inference/openai/v1"},
                "headers":{"x-org-id":"org_selected"},
                "models":{"fixture":{"modelID":"fixture-wire"}}
            }}}),
        );
        assert!(!catalog.join().unwrap().unwrap().is_empty());
        assert!(
            config_headers
                .to_ascii_lowercase()
                .contains("authorization: bearer synthetic-refreshed"),
            "catalogue restored an old token after refresh"
        );
        let current = fixture.gateway.store.snapshot(&fixture.account).unwrap();
        assert_eq!(current.generation, before.generation);
        assert_eq!(current.summary.state, AccountState::AccountReady);
        let grant = fixture.gateway.store.load_grant(&fixture.account).unwrap();
        assert_eq!(grant.access_token.expose(), "synthetic-refreshed");
        assert_eq!(grant.organization.as_deref(), Some("org_selected"));
        let TokenLifecycle::ExpiringRefresh {
            refresh_token,
            expires_at,
        } = grant.lifecycle
        else {
            panic!("refreshable grant expected")
        };
        assert_eq!(refresh_token.expose(), "synthetic-refreshed-key");
        assert!(expires_at > Instant::now() + Duration::from_secs(3000));
        assert!(fixture.gateway.state.lock().unwrap().leases.is_empty());
    }

    #[test]
    fn refresh_preserves_a_new_organization_selected_while_its_http_response_is_pending() {
        let fixture = Fixture::new();
        let before = fixture.gateway.store.snapshot(&fixture.account).unwrap();
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        *fixture.gateway.fixture_upstream.lock().unwrap() =
            Some(format!("http://{}/", upstream.local_addr().unwrap()));
        let gateway = fixture.gateway.clone();
        let account = fixture.account.clone();
        let catalog =
            thread::spawn(move || gateway.catalog(&account, &Arc::new(NetworkHttpCancel::new())));
        let (user, _, _) = controlled_peer_request(&upstream);
        reply_controlled_peer(
            user,
            json!({"id":"synthetic-user","email":"synthetic@fixture.invalid"}),
        );
        let (orgs, _, _) = controlled_peer_request(&upstream);
        fixture
            .gateway
            .store
            .replace_grant_if_current(
                &fixture.account,
                &before.generation,
                TokenGrant {
                    provider: Provider::OpenCode,
                    access_token: Secret::new("synthetic-access").unwrap(),
                    lifecycle: TokenLifecycle::ExpiringRefresh {
                        refresh_token: Secret::new("synthetic-refresh").unwrap(),
                        expires_at: Instant::now(),
                    },
                    organization: None,
                },
            )
            .unwrap();
        let gateway = fixture.gateway.clone();
        let route = gateway.state.lock().unwrap().config.models[0].clone();
        let account = fixture.account.clone();
        let probe = thread::spawn(move || {
            gateway.probe(
                &account,
                &route,
                "synthetic-refresh-probe",
                &Arc::new(NetworkHttpCancel::new()),
            )
        });
        // Refresh captured an organization-free grant; keep that HTTP response pending.
        let (refresh, _, body) = controlled_peer_request(&upstream);
        assert_eq!(body["grant_type"], "refresh_token");
        reply_controlled_peer(
            orgs,
            json!([{"id":"org_selected","name":"Synthetic workspace"}]),
        );
        let (config, config_headers, _) = controlled_peer_request(&upstream);
        reply_controlled_peer(
            config,
            json!({"providers":{"remote":{
                "package":"@opencode/ai/providers/openai-compatible",
                "settings":{"baseURL":"https://opencode.ai/inference/openai/v1"},
                "headers":{"x-org-id":"org_selected"},
                "models":{"fixture":{"modelID":"fixture-wire"}}
            }}}),
        );
        assert!(!catalog.join().unwrap().unwrap().is_empty());
        assert!(
            config_headers
                .to_ascii_lowercase()
                .contains("x-org-id: org_selected")
        );
        assert_eq!(
            fixture
                .gateway
                .store
                .load_grant(&fixture.account)
                .unwrap()
                .organization
                .as_deref(),
            Some("org_selected")
        );
        reply_controlled_peer(
            refresh,
            json!({"access_token":"synthetic-refreshed","refresh_token":"synthetic-refreshed-key","expires_in":3600}),
        );
        let (inference, headers, _) = controlled_peer_request(&upstream);
        reply_controlled_peer(
            inference,
            json!({"id":"synthetic-probe","model":"fixture-wire","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}]}),
        );
        probe.join().unwrap().unwrap();
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("authorization: bearer synthetic-refreshed")
        );
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("x-org-id: org_selected"),
            "refresh discarded organization selected while its response was pending"
        );
        let grant = fixture.gateway.store.load_grant(&fixture.account).unwrap();
        assert_eq!(grant.access_token.expose(), "synthetic-refreshed");
        assert_eq!(grant.organization.as_deref(), Some("org_selected"));
        let after = fixture.gateway.store.snapshot(&fixture.account).unwrap();
        assert_eq!(after.generation, before.generation);
        assert_eq!(after.summary.state, AccountState::AccountReady);
        assert!(fixture.gateway.state.lock().unwrap().leases.is_empty());
    }

    #[test]
    fn reconnect_changes_only_its_bindings_and_old_leases_cannot_borrow_the_new_grant() {
        let fixture = Fixture::new();
        let grant = |token: &str| TokenGrant {
            provider: Provider::OpenCode,
            access_token: Secret::new(token).unwrap(),
            lifecycle: TokenLifecycle::ExpiringRefresh {
                refresh_token: Secret::new("synthetic-refresh").unwrap(),
                expires_at: Instant::now() + Duration::from_secs(3600),
            },
            organization: None,
        };
        let other = fixture
            .gateway
            .store
            .insert("Unaffected", grant("synthetic-other-account"))
            .unwrap();
        fixture.gateway.store.mark_ready(&other.id).unwrap();
        add_profile(&fixture, &other.id);
        let server = fixture.server();
        let before = read_binding(server.get(
            "/v1/minicon/binding",
            "x-minicon-session: reconnect-old\r\n",
        ));
        let independent = read_binding(server.get_key(
            "/v1/minicon/binding",
            "synthetic-other",
            "x-minicon-session: other-stable\r\n",
        ));
        let cancel = Arc::new(NetworkHttpCancel::new());
        let lease =
            BorrowedLease::acquire(&fixture.gateway, &fixture.account, &cancel, None).unwrap();
        let acquired = fixture
            .gateway
            .grant(
                &fixture.account,
                &cancel,
                &lease.preferences.service,
                &lease.generation,
            )
            .unwrap();
        fixture
            .gateway
            .store
            .reconnect_if_current(
                &fixture.account,
                &lease.generation,
                grant("synthetic-reconnected"),
            )
            .unwrap();
        assert_eq!(acquired.access_token.expose(), "synthetic-access");
        assert!(!cancel.is_cancelled());
        assert!(matches!(
            fixture.gateway.grant(
                &fixture.account,
                &cancel,
                &lease.preferences.service,
                &lease.generation
            ),
            Err(ServerError::BindingChanged)
        ));
        assert!(
            server
                .get("/v1/minicon/binding", "")
                .starts_with("HTTP/1.1 401")
        );
        let current = fixture.gateway.store.snapshot(&fixture.account).unwrap();
        fixture
            .gateway
            .store
            .mark_ready_if_current(&fixture.account, &current.generation)
            .unwrap();
        assert!(
            server
                .get(
                    "/v1/minicon/binding",
                    "x-minicon-session: reconnect-old\r\n"
                )
                .starts_with("HTTP/1.1 409")
        );
        let after = read_binding(server.get(
            "/v1/minicon/binding",
            "x-minicon-session: reconnect-new\r\n",
        ));
        assert_ne!(before["generation"], after["generation"]);
        for field in ["profile", "provider", "account", "model"] {
            assert_eq!(before[field], after[field]);
        }
        assert_eq!(
            independent,
            read_binding(server.get_key(
                "/v1/minicon/binding",
                "synthetic-other",
                "x-minicon-session: other-stable\r\n"
            ))
        );
    }

    #[test]
    fn reconnect_rejects_late_refresh_and_probe_results_after_real_http_completes() {
        for refresh in [true, false] {
            let fixture = Fixture::new();
            let before = fixture.gateway.store.snapshot(&fixture.account).unwrap();
            let grant = |token: &str, expires_at: Instant| TokenGrant {
                provider: Provider::OpenCode,
                access_token: Secret::new(token).unwrap(),
                lifecycle: TokenLifecycle::ExpiringRefresh {
                    refresh_token: Secret::new("synthetic-refresh").unwrap(),
                    expires_at,
                },
                organization: None,
            };
            if refresh {
                fixture
                    .gateway
                    .store
                    .replace_grant_if_current(
                        &fixture.account,
                        &before.generation,
                        grant("synthetic-access", Instant::now()),
                    )
                    .unwrap();
            }
            let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
            upstream.set_nonblocking(true).unwrap();
            *fixture.gateway.fixture_upstream.lock().unwrap() =
                Some(format!("http://{}/", upstream.local_addr().unwrap()));
            let gateway = fixture.gateway.clone();
            let account = fixture.account.clone();
            let route = gateway.state.lock().unwrap().config.models[0].clone();
            let worker = thread::spawn(move || {
                gateway.probe(
                    &account,
                    &route,
                    "synthetic-probe",
                    &Arc::new(NetworkHttpCancel::new()),
                )
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            let (mut stream, _) = loop {
                match upstream.accept() {
                    Ok(pair) => break pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "probe did not reach the controlled HTTP peer"
                        );
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("controlled peer accept: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let headers = read_headers(&mut stream);
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .map(|(_, length)| length.trim().parse().unwrap())
                })
                .unwrap();
            assert!(length <= 16384);
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            if refresh {
                assert_eq!(body["grant_type"], "refresh_token");
            } else {
                assert_eq!(body["model"], "fixture-wire");
                assert!(
                    headers
                        .to_ascii_lowercase()
                        .contains("authorization: bearer synthetic-access")
                );
            }
            fixture
                .gateway
                .store
                .reconnect_if_current(
                    &fixture.account,
                    &before.generation,
                    grant(
                        "synthetic-reconnect-winner",
                        Instant::now() + Duration::from_secs(3600),
                    ),
                )
                .unwrap();
            let response = if refresh {
                json!({"access_token":"synthetic-stale-refresh","refresh_token":"synthetic-stale-refresh-key","expires_in":3600})
            } else {
                json!({"id":"synthetic-probe","model":"fixture-wire","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}})
            }.to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
            drop(stream);
            assert_eq!(worker.join().unwrap(), Err(ServerError::BindingChanged));
            let current = fixture.gateway.store.snapshot(&fixture.account).unwrap();
            assert_ne!(current.generation, before.generation);
            assert_eq!(current.summary.state, AccountState::CredentialStored);
            assert_eq!(
                fixture
                    .gateway
                    .store
                    .load_grant(&fixture.account)
                    .unwrap()
                    .access_token
                    .expose(),
                "synthetic-reconnect-winner"
            );
            assert!(fixture.gateway.state.lock().unwrap().leases.is_empty());
        }
    }

    #[test]
    fn lease_revalidates_the_resolved_binding_after_key_or_route_changes() {
        for change in ["same-key-generation", "other-profile", "route-change"] {
            let fixture = Fixture::new();
            let header = head("synthetic-local");
            let (binding, _, _) = fixture.gateway.sticky(&header, None).unwrap();
            match change {
                "same-key-generation" => {
                    fixture.gateway.set_key(None).unwrap();
                    fixture
                        .gateway
                        .set_key(Some(Secret::new("synthetic-local").unwrap()))
                        .unwrap();
                }
                "other-profile" => {
                    let profile = add_profile(&fixture, &fixture.account);
                    fixture.gateway.set_key(None).unwrap();
                    fixture
                        .gateway
                        .set_client_key(&profile, Some(Secret::new("synthetic-local").unwrap()))
                        .unwrap();
                }
                _ => {
                    let mut config = fixture.gateway.state.lock().unwrap().config.clone();
                    config.models[0].url =
                        "https://opencode.ai/inference/go/openai/v1/chat/completions".into();
                    fixture.gateway.update(config).unwrap();
                }
            }
            let cancel = Arc::new(NetworkHttpCancel::new());
            assert!(
                matches!(
                    BorrowedLease::acquire(
                        &fixture.gateway,
                        &binding.account,
                        &cancel,
                        Some((&header, &binding))
                    ),
                    Err(ServerError::BindingChanged)
                ),
                "stale {change} request acquired upstream authority"
            );
            assert!(fixture.gateway.state.lock().unwrap().leases.is_empty());
        }
    }

    #[test]
    fn profile_leases_share_account_capacity_and_revoke_only_their_own_work() {
        let fixture = Fixture::new();
        let profile = add_profile(&fixture, &fixture.account);
        let (binding, _, _) = fixture
            .gateway
            .sticky(&head("synthetic-local"), None)
            .unwrap();
        let (other, _, _) = fixture
            .gateway
            .sticky(&head("synthetic-other"), None)
            .unwrap();
        let one = Arc::new(NetworkHttpCancel::new());
        let two = Arc::new(NetworkHttpCancel::new());
        let first = BorrowedLease::acquire(
            &fixture.gateway,
            &fixture.account,
            &one,
            Some((&head("synthetic-local"), &binding)),
        )
        .unwrap();
        let second = BorrowedLease::acquire(
            &fixture.gateway,
            &fixture.account,
            &two,
            Some((&head("synthetic-other"), &other)),
        )
        .unwrap();
        assert!(matches!(
            BorrowedLease::acquire(
                &fixture.gateway,
                &fixture.account,
                &Arc::new(NetworkHttpCancel::new()),
                Some((&head("synthetic-other"), &other))
            ),
            Err(ServerError::Busy)
        ));
        fixture.gateway.set_key(None).unwrap();
        assert!(one.is_cancelled());
        assert!(!two.is_cancelled());
        drop(first);
        let probe = Arc::new(NetworkHttpCancel::new());
        let _probe =
            BorrowedLease::acquire(&fixture.gateway, &fixture.account, &probe, None).unwrap();
        fixture.gateway.set_client_key(&profile, None).unwrap();
        assert!(two.is_cancelled());
        assert!(!probe.is_cancelled());
        drop(second);
    }

    #[test]
    fn profile_route_changes_are_scoped_and_catalog_reads_do_not_bind_sessions() {
        let fixture = Fixture::new();
        let profile = add_profile(&fixture, &fixture.account);
        let server = fixture.server();
        let before = server.get("/v1/minicon/binding", "x-minicon-session: stable\r\n");
        assert!(before.starts_with("HTTP/1.1 200"));
        for _ in 0..3 {
            assert!(
                server
                    .get_key("/v1/models", "synthetic-other", "")
                    .contains("\"id\":\"work\"")
            );
        }
        assert_eq!(fixture.gateway.state.lock().unwrap().sessions.len(), 1);
        let bound = server.get_key(
            "/v1/minicon/binding",
            "synthetic-other",
            "x-minicon-session: stable\r\n",
        );
        assert!(bound.starts_with("HTTP/1.1 200"), "{bound}");
        let mut config = fixture.gateway.state.lock().unwrap().config.clone();
        let mut alternative = config.models[0].clone();
        alternative.id = "opencode/alternate/fixture".into();
        alternative.url = "https://opencode.ai/inference/go/openai/v1/chat/completions".into();
        config.models.push(alternative.clone());
        config.profiles[0].allocations[0].route = alternative.id;
        fixture.gateway.update(config.clone()).unwrap();
        assert!(
            server
                .get_key(
                    "/v1/minicon/binding",
                    "synthetic-other",
                    "x-minicon-session: stable\r\n"
                )
                .starts_with("HTTP/1.1 409")
        );
        assert_eq!(
            server.get("/v1/minicon/binding", "x-minicon-session: stable\r\n"),
            before
        );
        let fresh = server.get_key(
            "/v1/minicon/binding",
            "synthetic-other",
            "x-minicon-session: fresh\r\n",
        );
        assert!(fresh.starts_with("HTTP/1.1 200"));
        config.profiles[0].label = "Renamed client".into();
        fixture.gateway.update(config.clone()).unwrap();
        assert_eq!(
            server.get_key(
                "/v1/minicon/binding",
                "synthetic-other",
                "x-minicon-session: fresh\r\n"
            ),
            fresh
        );
        assert!(matches!(
            fixture
                .gateway
                .set_client_key(&profile, Some(Secret::new("synthetic-local").unwrap())),
            Err(ServerError::InvalidConfig)
        ));
        config.profiles[0].enabled = false;
        fixture.gateway.update(config).unwrap();
        assert!(
            server
                .get_key("/v1/models", "synthetic-other", "")
                .starts_with("HTTP/1.1 401")
        );
        assert!(server.get("/v1/models", "").starts_with("HTTP/1.1 200"));
    }

    #[test]
    fn profile_only_listener_routes_private_grant_and_survives_default_revoke_during_sse() {
        let fixture = Fixture::new();
        let account = fixture
            .gateway
            .store
            .insert(
                "Other synthetic account",
                TokenGrant {
                    provider: Provider::OpenCode,
                    access_token: Secret::new("synthetic-second-provider").unwrap(),
                    lifecycle: TokenLifecycle::ExpiringRefresh {
                        refresh_token: Secret::new("synthetic-second-refresh").unwrap(),
                        expires_at: Instant::now() + Duration::from_secs(3600),
                    },
                    organization: None,
                },
            )
            .unwrap();
        fixture.gateway.store.mark_ready(&account.id).unwrap();
        add_profile(&fixture, &account.id);
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        upstream.set_nonblocking(true).unwrap();
        *fixture.gateway.fixture_upstream.lock().unwrap() =
            Some(format!("http://{}/", upstream.local_addr().unwrap()));
        let (release, wait) = mpsc::channel();
        thread::scope(|scope| {
            let peer = scope.spawn(move || {
                let mut stream = accept_upstream(&upstream);
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
                stream.set_write_timeout(Some(Duration::from_secs(4))).unwrap();
                let headers = read_headers(&mut stream);
                assert!(headers.to_lowercase().contains("authorization: bearer synthetic-second-provider\r\n"));
                assert!(!headers.contains("synthetic-other"));
                assert!(!headers.contains("synthetic-local"));
                let length: usize = headers.lines().find_map(|line| line.split_once(':')
                    .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse().unwrap())).unwrap();
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                let value: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(value["model"], "fixture-wire");
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{FIRST}", FIRST.len() + END.len()).unwrap();
                stream.flush().unwrap();
                wait.recv_timeout(Duration::from_secs(4)).unwrap();
                stream.write_all(END.as_bytes()).unwrap();
            });
            let server = fixture.server();
            let mut client = server.request_key("/v1/chat/completions", "synthetic-other", "x-minicon-session: independent\r\n",
                Some(r#"{"model":"work","stream":true,"messages":[{"role":"user","content":"test"}]}"#));
            assert!(read_headers(&mut client).starts_with("HTTP/1.1 200"));
            assert_eq!(
                sse_event(&mut client)["choices"][0]["delta"]["content"],
                "hello"
            );
            fixture.gateway.set_key(None).unwrap();
            assert!(server.get("/v1/models", "").starts_with("HTTP/1.1 401"));
            assert!(
                server
                    .get_key("/v1/models", "synthetic-other", "")
                    .contains("\"id\":\"work\"")
            );
            release.send(()).unwrap();
            assert_eq!(
                sse_event(&mut client)["choices"][0]["finish_reason"],
                "stop"
            );
            let mut tail = Vec::new();
            client.read_to_end(&mut tail).unwrap();
            assert_eq!(tail, b"data: [DONE]\n\n");
            peer.join().unwrap();
        });
        fixture.gateway.set_key(None).unwrap();
        let mut config = fixture.gateway.state.lock().unwrap().config.clone();
        config.selected_account = None;
        config.model.clear();
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        config.port = port;
        fixture.gateway.update(config).unwrap();
        let mut server = fixture.gateway.start(port).unwrap();
        server.stop();
    }

    #[test]
    fn catalog_authority_and_capability_are_not_inferred_from_login() {
        let models=parse_deepseek_catalog(br#"{"data":[{"id":"supported","api_capabilities":{"anthropic_messages":{}}},{"id":"unknown"}]}"#).unwrap();
        assert_eq!(models.len(), 1);
        assert!(parse_deepseek_catalog(br#"{"data":[{"id":"unknown"}]}"#).is_err());
        let mut route = models[0].clone();
        route.url = "https://api.deepseek.com.attacker.invalid/anthropic/v1/messages".into();
        assert!(validate_route(&route).is_err());
        let fixture = Fixture::new();
        let mut route = fixture.gateway.state.lock().unwrap().config.models[0].clone();
        route.url = "https://opencode.ai/inference/%2e%2e/console/chat/completions".into();
        assert!(validate_route(&route).is_err());
        assert_eq!(
            select_organization(br#"[{"id":"z","name":"Z"},{"id":"a","name":"A"}]"#, None).unwrap(),
            Some("a".into())
        );
        assert_eq!(
            select_organization(br#"[{"id":"a","name":"A"}]"#, Some("missing")),
            Err(ServerError::NeedsLogin)
        );
        let config=br#"{"providers":{"remote":{"package":"@opencode/ai/providers/openai-compatible","settings":{"baseURL":"https://opencode.ai/inference/openai/v1"},"headers":{"x-org-id":"org"},"models":{"catalog":{"modelID":"wire"}}}}}"#;
        assert_eq!(
            parse_opencode_catalog(config, Some("org")).unwrap()[0].model_id,
            "wire"
        );
        assert!(parse_opencode_catalog(config, Some("other")).is_err());
    }

    fn sse_event(stream: &mut TcpStream) -> Value {
        let mut bytes = Vec::new();
        let mut byte = [0];
        while !bytes.ends_with(b"\n\n") {
            assert!(bytes.len() < protocol::MAX_EVENT_BYTES);
            stream.read_exact(&mut byte).unwrap();
            bytes.push(byte[0]);
        }
        let text = std::str::from_utf8(&bytes).unwrap();
        let data = text.strip_prefix("data: ").unwrap().trim_end();
        serde_json::from_str(data).unwrap()
    }
    fn accept_upstream(listener: &TcpListener) -> TcpStream {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            match listener.accept() {
                Ok((stream, _)) => return stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "upstream request did not arrive");
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("upstream accept failed: {error}"),
            }
        }
    }
    #[test]
    fn native_sse_delivers_before_upstream_finishes_without_buffering() {
        let fixture = Fixture::new();
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        *fixture.gateway.fixture_upstream.lock().unwrap() =
            Some(format!("http://{}/", upstream.local_addr().unwrap()));
        let (release, wait) = mpsc::channel();
        thread::scope(|scope| {
            let worker=scope.spawn(move || {
                let mut stream=accept_upstream(&upstream);upstream_request(&mut stream);
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{FIRST}",FIRST.len()+END.len()).unwrap();stream.flush().unwrap();
                // The terminal event cannot be sent until the client observes
                // and validates the first event through the real gateway.
                wait.recv_timeout(Duration::from_secs(4)).unwrap();stream.write_all(END.as_bytes()).unwrap();
            });
            let server = fixture.server();
            let mut client = server.request(
                "/v1/chat/completions",
                "x-minicon-session: stream\r\n",
                Some(BODY),
            );
            assert!(read_headers(&mut client).starts_with("HTTP/1.1 200"));
            let first = sse_event(&mut client);
            assert_eq!(first["id"], "synthetic");
            assert_eq!(first["model"], "fixture-wire");
            assert_eq!(first["choices"][0]["delta"]["content"], "hello");
            assert!(first["choices"][0]["finish_reason"].is_null());
            release.send(()).unwrap();
            let finish = sse_event(&mut client);
            assert_eq!(finish["choices"][0]["finish_reason"], "stop");
            let mut tail = Vec::new();
            client.read_to_end(&mut tail).unwrap();
            assert_eq!(tail, b"data: [DONE]\n\n");
            worker.join().unwrap();
        });
    }

    #[test]
    fn downstream_disconnect_shuts_real_upstream_and_releases_lease() {
        let fixture = Fixture::new();
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        *fixture.gateway.fixture_upstream.lock().unwrap() =
            Some(format!("http://{}/", upstream.local_addr().unwrap()));
        let (closed, observed) = mpsc::channel();
        thread::scope(|scope| {
            let worker = scope.spawn(move || {
            let mut stream = accept_upstream(&upstream);
            upstream_request(&mut stream);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n{FIRST}"
            )
            .unwrap();
            stream.flush().unwrap();
            let mut byte = [0];
            let result = stream.read(&mut byte);
            closed.send(matches!(result,Ok(0)) || matches!(result,Err(ref error) if matches!(error.kind(),std::io::ErrorKind::ConnectionReset|std::io::ErrorKind::ConnectionAborted))).unwrap();
        });
            let server = fixture.server();
            let mut client = server.request("/v1/chat/completions", "", Some(BODY));
            read_headers(&mut client);
            let mut first = vec![0; FIRST.len()];
            client.read_exact(&mut first).unwrap();
            let start = Instant::now();
            drop(client);
            assert!(observed.recv_timeout(Duration::from_secs(5)).unwrap());
            worker.join().unwrap();
            while fixture
                .gateway
                .state
                .lock()
                .unwrap()
                .leases
                .values()
                .sum::<usize>()
                != 0
            {
                assert!(start.elapsed() < Duration::from_secs(5));
                thread::sleep(Duration::from_millis(10));
            }
        });
    }
    #[test]
    fn draining_cancels_owned_io_and_refuses_incoming_but_allows_explicit_probe() {
        let fixture = Fixture::new();
        let (binding, _, _) = fixture
            .gateway
            .sticky(&head("synthetic-local"), None)
            .unwrap();
        let server = fixture.server();
        let network = Arc::new(NetworkHttpCancel::new());
        let lease =
            BorrowedLease::acquire(&fixture.gateway, &fixture.account, &network, None).unwrap();
        fixture.gateway.begin_stop();
        assert!(network.is_cancelled());
        assert!(server.get("/v1/models", "").starts_with("HTTP/1.1 503"));
        assert!(matches!(
            BorrowedLease::acquire(
                &fixture.gateway,
                &fixture.account,
                &Arc::new(NetworkHttpCancel::new()),
                Some((&head("synthetic-local"), &binding))
            ),
            Err(ServerError::Busy)
        ));
        drop(lease);
        let probe = Arc::new(NetworkHttpCancel::new());
        let _probe_lease =
            BorrowedLease::acquire(&fixture.gateway, &fixture.account, &probe, None).unwrap();
        assert!(!probe.is_cancelled());
    }
    /// Real external client; synthetic upstream only. This does not qualify a
    /// provider account, OAuth flow or paid inference. See the pinned Pi court.
    #[test]
    #[ignore = "requires pinned Pi 1.4.0 via absolute MINICON_PI_CLI; missing dependency is BLOCKED"]
    fn external_pi_1_4_0_uses_real_gateway_sse_and_usage() {
        use minicon_platform::contained_process::{ContainedChild, ContainedHeadlessCommand};
        use std::sync::atomic::{AtomicBool, Ordering};

        struct OwnedPi {
            child: ContainedChild,
            cleaned: bool,
        }
        impl Drop for OwnedPi {
            fn drop(&mut self) {
                if !self.cleaned {
                    let _ = self.child.terminate_and_wait(Duration::from_secs(5));
                }
            }
        }
        struct StopPeer(Arc<AtomicBool>);
        impl Drop for StopPeer {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }

        let cli=std::env::var_os("MINICON_PI_CLI").map(PathBuf::from)
            .filter(|path|path.is_absolute() && path.is_file())
            .expect("BLOCKED: MINICON_PI_CLI must name the installed pinned Pi 1.4.0 CLI by absolute path");
        let package = cli
            .parent()
            .and_then(|path| path.parent())
            .and_then(|path| path.parent())
            .expect("BLOCKED: Pi CLI must use the installed package dist/bundle/cli.js layout")
            .join("package.json");
        let mut package_bytes = Vec::new();
        std::fs::File::open(package)
            .expect("BLOCKED: Pi package metadata unavailable")
            .take(64 * 1024 + 1)
            .read_to_end(&mut package_bytes)
            .unwrap();
        assert!(
            package_bytes.len() <= 64 * 1024,
            "BLOCKED: oversized Pi package metadata"
        );
        let package: Value =
            serde_json::from_slice(&package_bytes).expect("BLOCKED: invalid Pi package metadata");
        assert_eq!(
            package["name"], "@earendil-works/pi-coding-agent",
            "BLOCKED: unexpected Pi package"
        );
        assert_eq!(
            package["version"], "1.4.0",
            "BLOCKED: this court pins Pi 1.4.0"
        );
        let node = std::env::var_os("MINICON_PI_NODE")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("PATH").and_then(|paths| {
                    std::env::split_paths(&paths)
                        .map(|path| path.join(if cfg!(windows) { "node.exe" } else { "node" }))
                        .find(|path| path.is_absolute() && path.is_file())
                })
            })
            .filter(|path| path.is_absolute() && path.is_file())
            .expect("BLOCKED: an absolute Node.js executable is required");

        let fixture = Fixture::new();
        let server = fixture.server();
        let agent = fixture.root.join("pi-agent");
        let workspace = fixture.root.join("pi-workspace");
        let home = fixture.root.join("pi-home");
        for directory in [&agent, &workspace, &home] {
            std::fs::create_dir_all(directory).unwrap();
        }
        // Compatibility choices are pinned to the checked Pi 1.4.0 contract:
        // keep real streaming usage, exclude unsupported request semantics.
        let models = json!({"providers":{"minicon":{
            "baseUrl":format!("http://{}/v1",server.server.address()),
            "api":"openai-completions","apiKey":"$MINICON_LOCAL_API_KEY","authHeader":true,
            "headers":{"User-Agent":"pi/1.4.0 (minicon-synthetic-court)","x-minicon-session":"$MINICON_PI_SESSION"},
            "compat":{"supportsStore":false,"supportsDeveloperRole":false,"supportsReasoningEffort":false,"supportsStrictMode":false,"supportsUsageInStreaming":true,"maxTokensField":"max_tokens","sendSessionAffinityHeaders":true,"sessionAffinityFormat":"openai"},
            "models":[{"id":MODEL_ALIAS,"name":"MiniCon synthetic selected model","reasoning":false,"input":["text"],"contextWindow":16384,"maxTokens":256}]
        }}});
        std::fs::write(
            agent.join("models.json"),
            serde_json::to_vec(&models).unwrap(),
        )
        .unwrap();
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        upstream.set_nonblocking(true).unwrap();
        *fixture.gateway.fixture_upstream.lock().unwrap() =
            Some(format!("http://{}/", upstream.local_addr().unwrap()));
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        thread::scope(|scope| {
            let _stop = StopPeer(stop);
            let peer=scope.spawn(move || {
                let deadline=Instant::now()+Duration::from_secs(30);
                let mut stream=loop {
                    if stopping.load(Ordering::Acquire) {return None;}
                    match upstream.accept() {
                        Ok((stream,_))=>break stream,
                        Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=>{
                            assert!(Instant::now()<deadline,"Pi never reached the real gateway upstream");thread::sleep(Duration::from_millis(10));
                        }
                        Err(error)=>panic!("Pi upstream accept failed: {error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();stream.set_read_timeout(Some(Duration::from_secs(4))).unwrap();stream.set_write_timeout(Some(Duration::from_secs(4))).unwrap();
                let headers=read_headers(&mut stream);assert!(headers.starts_with("POST / HTTP/1.1"));
                assert!(headers.to_ascii_lowercase().contains("authorization: bearer synthetic-access"));
                assert!(headers.to_ascii_lowercase().contains("x-opencode-session:"));
                let length=headers.lines().find_map(|line|line.split_once(':').filter(|(name,_)|name.eq_ignore_ascii_case("content-length")).map(|(_,value)|value.trim().parse::<usize>().unwrap())).unwrap();
                assert!(length<=64*1024,"Pi request unexpectedly exceeds the court's bounded prompt");
                let mut body=vec![0;length];stream.read_exact(&mut body).unwrap();let body:Value=serde_json::from_slice(&body).unwrap();
                assert_eq!(body["model"],"fixture-wire");assert_eq!(body["stream"],true);assert_eq!(body["stream_options"]["include_usage"],true);
                assert!(body.get("store").is_none());
                let first=json!({"id":"pi-synthetic","object":"chat.completion.chunk","model":"fixture-wire","choices":[{"index":0,"delta":{"role":"assistant","content":"MINICON_PI_OK"},"finish_reason":null}]});
                let finish=json!({"id":"pi-synthetic","object":"chat.completion.chunk","model":"fixture-wire","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
                let usage=json!({"id":"pi-synthetic","object":"chat.completion.chunk","model":"fixture-wire","choices":[],"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}});
                let first=format!("data: {first}\n\n");let tail=format!("data: {finish}\n\ndata: {usage}\n\ndata: [DONE]\n\n");
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{first}",first.len()+tail.len()).unwrap();stream.flush().unwrap();
                thread::sleep(Duration::from_millis(100));stream.write_all(tail.as_bytes()).unwrap();
                Some(body)
            });
            let mut command = ContainedHeadlessCommand::new(node);
            command.env_clear().current_dir(&workspace).capture_output();
            for name in [
                "PATH",
                "SystemRoot",
                "WINDIR",
                "TEMP",
                "TMP",
                "LANG",
                "LC_ALL",
            ] {
                if let Some(value) = std::env::var_os(name) {
                    command.env(name, value);
                }
            }
            command
                .env("HOME", home.as_os_str())
                .env("USERPROFILE", home.as_os_str())
                .env("XDG_CONFIG_HOME", home.join(".config"))
                .env("PI_CODING_AGENT_DIR", agent.as_os_str())
                .env("PI_OFFLINE", "1")
                .env("PI_TELEMETRY", "0")
                .env("DO_NOT_TRACK", "1")
                .env("CI", "1")
                .env("NO_COLOR", "1")
                .env("MINICON_LOCAL_API_KEY", "synthetic-local")
                .env("MINICON_PI_SESSION", "minicon-pi-1.4.0-synthetic-court");
            command.arg(cli).args([
                "--print",
                "--mode",
                "json",
                "--provider",
                "minicon",
                "--model",
                MODEL_ALIAS,
                "--thinking",
                "off",
                "--no-session",
                "--no-tools",
                "--no-extensions",
                "--no-mcp",
                "--no-skills",
                "--no-prompt-templates",
                "--no-themes",
                "--no-context-files",
                "--no-approve",
                "--offline",
                "--system-prompt",
                "Reply concisely to compatibility probes. No tool use.",
                "Reply exactly MINICON_PI_OK.",
            ]);
            let mut process = OwnedPi {
                child: command.spawn().expect("BLOCKED: Pi/Node launch failed"),
                cleaned: false,
            };
            let mut stdout = process.child.take_stdout().unwrap();
            let mut stderr = process.child.take_stderr().unwrap();
            let mut out = Vec::new();
            let mut err = Vec::new();
            let mut eof = [false; 2];
            let mut captured = 0usize;
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                for (index, pipe, target) in
                    [(0, &mut stdout, &mut out), (1, &mut stderr, &mut err)]
                {
                    if eof[index] {
                        continue;
                    }
                    let mut bytes = [0; 8192];
                    match pipe.read_timeout(&mut bytes, Duration::from_millis(5)) {
                        Ok(0) => eof[index] = true,
                        Ok(count) => {
                            captured += count;
                            assert!(
                                captured <= 1024 * 1024,
                                "Pi output exceeds cumulative 1 MiB bound"
                            );
                            target.extend_from_slice(&bytes[..count]);
                        }
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::TimedOut
                                    | std::io::ErrorKind::WouldBlock
                                    | std::io::ErrorKind::Interrupted
                            ) => {}
                        Err(error) => panic!("Pi captured output failed: {error}"),
                    }
                }
                assert!(
                    out.len() + err.len() <= 1024 * 1024,
                    "Pi output exceeds cumulative 1 MiB bound"
                );
                if process.child.exit_observed().unwrap() && eof.iter().all(|eof| *eof) {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "Pi did not exit within 30 seconds; stderr={}",
                    String::from_utf8_lossy(&err)
                );
            }
            process
                .child
                .terminate_and_wait(Duration::from_secs(5))
                .unwrap();
            process.cleaned = true;
            let status = process.child.try_wait().unwrap().unwrap();
            assert_eq!(
                status,
                minicon_platform::contained_process::ProcessExit::Code(0),
                "Pi failed: stderr={}; stdout={}",
                String::from_utf8_lossy(&err),
                String::from_utf8_lossy(&out)
            );
            let text = std::str::from_utf8(&out).unwrap();
            let events: Vec<Value> = text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| {
                    serde_json::from_str(line).expect("Pi JSON mode emitted a non-JSON event")
                })
                .collect();
            let message = events
                .iter()
                .rev()
                .find(|event| {
                    event["type"] == "message_end" && event["message"]["role"] == "assistant"
                })
                .expect("Pi emitted no final assistant message")["message"]
                .clone();
            let answer: String = message["content"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|block| block["type"] == "text")
                .map(|block| block["text"].as_str().unwrap())
                .collect();
            assert_eq!(answer, "MINICON_PI_OK");
            assert_eq!(message["stopReason"], "stop");
            assert_eq!(message["usage"]["input"], 7);
            assert_eq!(message["usage"]["output"], 3);
            assert_eq!(message["usage"]["totalTokens"], 10);
            let observed = peer
                .join()
                .unwrap()
                .expect("Pi never performed real upstream inference");
            assert_eq!(observed["model"], "fixture-wire");
            assert_eq!(
                fixture
                    .gateway
                    .state
                    .lock()
                    .unwrap()
                    .sessions
                    .front()
                    .unwrap()
                    .name,
                "minicon-pi-1.4.0-synthetic-court"
            );
            println!(
                "{}",
                json!({"client":"Pi","version":"1.4.0","upstream":"synthetic-loopback-not-provider-authorization","model":observed["model"],"stream":observed["stream"],"text":answer,"stop_reason":message["stopReason"],"usage":message["usage"],"exit":0})
            );
        });
    }
}
