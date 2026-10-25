//! In-memory account catalogues and exact connection candidates. PRD 31.01.02.01.
//! No network, grants, persistence, admission, or automatic inference lives here.
#[cfg(test)]
use crate::provider_server::{MAX_CLIENT_ALLOCATIONS, MAX_CLIENT_PROFILES, safe_model};
use crate::{
    provider_accounts::{AccountSnapshot, NetworkPolicy, OrganizationAuthority},
    provider_auth::Provider,
    provider_server::{
        CATALOG_BYTES, MAX_MODELS, ModelRoute, WireProtocol, opaque_id, validate_route,
    },
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt, mem::size_of};

pub(crate) const MAX_CATALOGUE_ACCOUNTS: usize = 16;
pub(crate) const MAX_CACHE_BYTES: usize = 8 * 1024 * 1024;
#[cfg(test)]
pub(crate) const MAX_DRAFT_BYTES: usize = 4 * 1024 * 1024;
#[cfg(test)]
pub(crate) const DEFAULT_PROFILE: &str = "default";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SelectionError {
    InvalidAuthority,
    Disabled,
    AuthorityChanged,
    LateResult,
    AccountBound,
    RouteBound,
    ResponseBound,
    AggregateBound,
    InvalidRoute,
    DuplicateCatalogueId,
    DuplicateHeader,
    OrganizationConflict,
    #[cfg(test)]
    CatalogueUnavailable,
    #[cfg(test)]
    MissingRoute,
    #[cfg(test)]
    InvalidDraft,
    #[cfg(test)]
    ProfileBound,
    #[cfg(test)]
    MissingProfile,
    #[cfg(test)]
    MissingAlias,
    RevisionExhausted,
    #[cfg(test)]
    DraftChanged,
}

/// Private service authority only; labels/login settings and grants are absent.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct ResolvedAuthority {
    account: String,
    provider: Provider,
    credential: String,
    service_generation: String,
    service: NetworkPolicy,
    organization: OrganizationAuthority,
}

impl fmt::Debug for ResolvedAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ResolvedAuthority([REDACTED])")
    }
}

impl ResolvedAuthority {
    pub(crate) fn from_snapshot(snapshot: &AccountSnapshot) -> Result<Self, SelectionError> {
        snapshot
            .preferences
            .validate()
            .map_err(|_| SelectionError::InvalidAuthority)?;
        if !opaque_id(&snapshot.summary.id)
            || !opaque_id(&snapshot.generation)
            || !opaque_id(&snapshot.organization.epoch)
            || snapshot.organization.digest.len() != 64
            || !snapshot
                .organization
                .digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(SelectionError::InvalidAuthority);
        }
        if !snapshot.preferences.enabled {
            return Err(SelectionError::Disabled);
        }
        Ok(Self {
            account: snapshot.summary.id.clone(),
            provider: snapshot.summary.provider,
            credential: snapshot.generation.clone(),
            service_generation: snapshot.preferences.service_generation.clone(),
            service: snapshot.preferences.service.clone(),
            organization: snapshot.organization.clone(),
        })
    }

    pub(crate) fn account(&self) -> &str {
        &self.account
    }

    pub(crate) fn same_transport_generation(&self, other: &Self) -> bool {
        self.account == other.account
            && self.provider == other.provider
            && self.credential == other.credential
            && self.service_generation == other.service_generation
            && self.service == other.service
    }

    fn bytes(&self) -> usize {
        size_of::<Self>()
            + self.account.capacity()
            + self.credential.capacity()
            + self.service_generation.capacity()
            + self.organization.epoch.capacity()
            + self.organization.digest.capacity()
            + match &self.service {
                NetworkPolicy::Direct => 0,
                NetworkPolicy::Proxy(value) => value.capacity(),
            }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogueStatus {
    NotLoaded,
    Loading,
    Fresh,
    Stale,
    Error,
}

/// Opaque operation identity; it cannot be created from a catalogue/model ID.
#[derive(Clone)]
pub(crate) struct CatalogueRequest {
    serial: u64,
    requested: ResolvedAuthority,
}

impl CatalogueRequest {
    pub(crate) fn authority(&self) -> &ResolvedAuthority {
        &self.requested
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogueModel {
    pub id: String,
    pub upstream_model: String,
    pub protocol: WireProtocol,
}

struct CatalogueData {
    authority: ResolvedAuthority,
    routes: Box<[ModelRoute]>,
    bytes: usize,
}
struct CatalogueEntry {
    status: CatalogueStatus,
    pending: Option<u64>,
    data: Option<CatalogueData>,
}

#[derive(Default)]
pub(crate) struct CatalogueCache {
    entries: BTreeMap<String, CatalogueEntry>,
    serial: u64,
    bytes: usize,
}

impl fmt::Debug for CatalogueCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CatalogueCache")
            .field("accounts", &self.entries.len())
            .field("retained_bytes", &self.bytes)
            .finish()
    }
}

impl CatalogueCache {
    pub(crate) fn begin(
        &mut self,
        current: &AccountSnapshot,
    ) -> Result<CatalogueRequest, SelectionError> {
        let requested = ResolvedAuthority::from_snapshot(current)?;
        if !self.entries.contains_key(requested.account())
            && self.entries.len() >= MAX_CATALOGUE_ACCOUNTS
        {
            return Err(SelectionError::AccountBound);
        }
        let serial = self
            .serial
            .checked_add(1)
            .ok_or(SelectionError::RevisionExhausted)?;
        self.serial = serial;
        let entry = self
            .entries
            .entry(requested.account.clone())
            .or_insert(CatalogueEntry {
                status: CatalogueStatus::NotLoaded,
                pending: None,
                data: None,
            });
        entry.pending = Some(serial);
        entry.status = CatalogueStatus::Loading;
        Ok(CatalogueRequest { serial, requested })
    }

    pub(crate) fn status(&self, account: &str) -> CatalogueStatus {
        self.entries
            .get(account)
            .map_or(CatalogueStatus::NotLoaded, |entry| entry.status)
    }

    /// Mark readings stale on account/service/org changes. Keep their bounded
    /// rows for explicit stale UI observation; they cannot produce candidates.
    pub(crate) fn invalidate(&mut self, account: &str) {
        if let Some(entry) = self.entries.get_mut(account) {
            entry.pending = None;
            entry.status = if entry.data.is_some() {
                CatalogueStatus::Stale
            } else {
                CatalogueStatus::NotLoaded
            };
        }
    }

    /// Eviction affects metadata only, never an immutable draft/applied route.
    pub(crate) fn evict(&mut self, account: &str) {
        if let Some(entry) = self.entries.remove(account) {
            self.bytes -= entry.data.map_or(0, |data| data.bytes);
        }
    }

    pub(crate) fn fail(&mut self, request: &CatalogueRequest) -> Result<(), SelectionError> {
        let entry = self.pending(request)?;
        entry.pending = None;
        entry.status = CatalogueStatus::Error;
        Ok(())
    }

    fn pending(
        &mut self,
        request: &CatalogueRequest,
    ) -> Result<&mut CatalogueEntry, SelectionError> {
        self.entries
            .get_mut(request.requested.account())
            .filter(|entry| entry.pending == Some(request.serial))
            .ok_or(SelectionError::LateResult)
    }

    /// `resolved` is the authority actually acquired by the existing catalogue
    /// transport, not a fresh snapshot attached to an old HTTP body. `current`
    /// must be reacquired at acceptance under the owning store/gateway policy.
    /// Only initial no-organization resolution may refine the requested scope;
    /// any other organization change requires another explicit refresh.
    pub(crate) fn complete(
        &mut self,
        request: &CatalogueRequest,
        resolved: ResolvedAuthority,
        current: Option<&AccountSnapshot>,
        routes: Vec<ModelRoute>,
        response_bytes: usize,
    ) -> Result<(), SelectionError> {
        self.pending(request)?;
        let result = self.prepare(request, resolved, current, routes, response_bytes);
        match result {
            Ok(data) => {
                let old = self
                    .entries
                    .get(request.requested.account())
                    .and_then(|e| e.data.as_ref())
                    .map_or(0, |d| d.bytes);
                let next = self
                    .bytes
                    .checked_sub(old)
                    .and_then(|n| n.checked_add(data.bytes))
                    .ok_or(SelectionError::AggregateBound)?;
                if next > MAX_CACHE_BYTES {
                    self.fail(request)?;
                    return Err(SelectionError::AggregateBound);
                }
                let entry = self.pending(request)?;
                entry.data = Some(data);
                entry.pending = None;
                entry.status = CatalogueStatus::Fresh;
                self.bytes = next;
                Ok(())
            }
            Err(error) => {
                self.fail(request)?;
                Err(error)
            }
        }
    }

    fn prepare(
        &self,
        request: &CatalogueRequest,
        resolved: ResolvedAuthority,
        current: Option<&AccountSnapshot>,
        routes: Vec<ModelRoute>,
        response_bytes: usize,
    ) -> Result<CatalogueData, SelectionError> {
        let actual = current
            .and_then(|s| ResolvedAuthority::from_snapshot(s).ok())
            .ok_or(SelectionError::AuthorityChanged)?;
        let empty_org = organization_value_digest(None);
        let initial_org_resolution = request.requested.organization.digest == empty_org
            && resolved.organization.digest != empty_org;
        if actual != resolved
            || !request.requested.same_transport_generation(&resolved)
            || (request.requested.organization != resolved.organization && !initial_org_resolution)
        {
            return Err(SelectionError::AuthorityChanged);
        }
        if routes.len() > MAX_MODELS {
            return Err(SelectionError::RouteBound);
        }
        if response_bytes > CATALOG_BYTES {
            return Err(SelectionError::ResponseBound);
        }
        let mut ids = std::collections::HashSet::new();
        for route in &routes {
            catalogue_route_headers(&resolved, route)?;
            if !ids.insert(&route.id) {
                return Err(SelectionError::DuplicateCatalogueId);
            }
        }
        let encoded = serde_json::to_vec(&routes).map_err(|_| SelectionError::InvalidRoute)?;
        if encoded.len() > CATALOG_BYTES {
            return Err(SelectionError::ResponseBound);
        }
        // Clone into compact-owned records; never retain a caller's oversized
        // Vec/String capacities. Account actual retained capacities below.
        let routes: Box<[ModelRoute]> = routes.iter().map(compact_route).collect();
        let bytes = routes
            .iter()
            .try_fold(resolved.bytes(), |n, row| n.checked_add(route_bytes(row)))
            .ok_or(SelectionError::AggregateBound)?;
        Ok(CatalogueData {
            authority: resolved,
            routes,
            bytes,
        })
    }

    /// Refresh only observed metadata authority, never an applied route.
    pub(crate) fn observe(&mut self, account: &str, current: Option<&AccountSnapshot>) {
        let actual = current.and_then(|snapshot| ResolvedAuthority::from_snapshot(snapshot).ok());
        if self.entries.get(account).is_some_and(|entry| {
            entry
                .data
                .as_ref()
                .is_some_and(|data| Some(&data.authority) != actual.as_ref())
        }) || actual.is_none()
        {
            self.invalidate(account);
        }
    }

    pub(crate) fn accounts(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    /// Stale/error readings remain visible, but cannot create a candidate.
    pub(crate) fn models(&self, account: &str) -> Vec<CatalogueModel> {
        self.entries
            .get(account)
            .and_then(|entry| entry.data.as_ref())
            .map_or_else(Vec::new, |data| {
                data.routes
                    .iter()
                    .map(|route| CatalogueModel {
                        id: route.id.clone(),
                        upstream_model: route.model_id.clone(),
                        protocol: route.protocol,
                    })
                    .collect()
            })
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.bytes
    }

    pub(crate) fn stale_rows(&self, account: &str) -> usize {
        self.entries
            .get(account)
            .and_then(|entry| entry.data.as_ref())
            .map_or(0, |data| data.routes.len())
    }

    #[cfg(test)]
    pub(crate) fn select(
        &self,
        current: &AccountSnapshot,
        id: &str,
    ) -> Result<FrozenRoute, SelectionError> {
        let authority = ResolvedAuthority::from_snapshot(current)?;
        let entry = self
            .entries
            .get(authority.account())
            .ok_or(SelectionError::CatalogueUnavailable)?;
        let data = entry
            .data
            .as_ref()
            .filter(|data| entry.status == CatalogueStatus::Fresh && data.authority == authority)
            .ok_or(SelectionError::CatalogueUnavailable)?;
        let route = data
            .routes
            .iter()
            .find(|route| route.id == id)
            .ok_or(SelectionError::MissingRoute)?;
        FrozenRoute::new(authority, compact_route(route))
    }
}

fn compact_route(route: &ModelRoute) -> ModelRoute {
    ModelRoute {
        id: route.id.as_str().into(),
        model_id: route.model_id.as_str().into(),
        provider: route.provider,
        protocol: route.protocol,
        url: route.url.as_str().into(),
        headers: route
            .headers
            .iter()
            .map(|(k, v)| (k.as_str().into(), v.as_str().into()))
            .collect(),
    }
}
fn route_bytes(route: &ModelRoute) -> usize {
    size_of::<ModelRoute>()
        + route.id.capacity()
        + route.model_id.capacity()
        + route.url.capacity()
        + route.headers.capacity() * size_of::<(String, String)>()
        + route
            .headers
            .iter()
            .map(|(k, v)| k.capacity() + v.capacity())
            .sum::<usize>()
}

fn framed(hash: &mut Sha256, value: &[u8]) {
    hash.update((value.len() as u64).to_be_bytes());
    hash.update(value);
}

// Matches the store's existing tagged value digest; no raw org is retained in
// the new authority. This only normalizes allowed x-org-id route headers.
fn organization_value_digest(value: Option<&str>) -> String {
    let mut hash = Sha256::new();
    hash.update(b"minicon.organization.v1\0");
    match value {
        None => hash.update([0]),
        Some(value) => {
            hash.update([1]);
            framed(&mut hash, value.as_bytes());
        }
    }
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Clone, Eq, PartialEq)]
struct WireIdentity([u8; 32]);

fn catalogue_route_headers<'a>(
    authority: &ResolvedAuthority,
    route: &'a ModelRoute,
) -> Result<BTreeMap<String, &'a str>, SelectionError> {
    validate_route(route).map_err(|_| SelectionError::InvalidRoute)?;
    if route.provider != authority.provider {
        return Err(SelectionError::InvalidRoute);
    }
    let mut headers = BTreeMap::new();
    for (name, value) in &route.headers {
        if headers
            .insert(name.to_ascii_lowercase(), value.as_str())
            .is_some()
        {
            return Err(SelectionError::DuplicateHeader);
        }
    }
    // Existing route_request discards catalogue x-org-id and injects OpenCode's
    // actual organization from the acquired grant. Hash that effective value,
    // not a header hint that will never be sent by this transport.
    let organization_hint = headers.remove("x-org-id");
    if route.provider == Provider::OpenCode
        && let Some(value) = organization_hint
        && organization_value_digest(Some(value)) != authority.organization.digest
    {
        return Err(SelectionError::OrganizationConflict);
    }
    Ok(headers)
}

fn wire_identity(
    authority: &ResolvedAuthority,
    route: &ModelRoute,
) -> Result<WireIdentity, SelectionError> {
    let headers = catalogue_route_headers(authority, route)?;
    let effective_org = if route.provider == Provider::OpenCode {
        authority.organization.digest.clone()
    } else {
        organization_value_digest(None)
    };
    Ok(wire_fingerprint(route, headers, &effective_org))
}

fn wire_fingerprint(
    route: &ModelRoute,
    headers: BTreeMap<String, &str>,
    effective_org: &str,
) -> WireIdentity {
    let mut hash = Sha256::new();
    hash.update(b"minicon.selection.wire.v1\0");
    framed(
        &mut hash,
        match route.provider {
            Provider::DeepSeek => b"deepseek",
            Provider::OpenCode => b"opencode",
        },
    );
    framed(&mut hash, route.model_id.as_bytes());
    framed(
        &mut hash,
        match route.protocol {
            WireProtocol::Chat => b"chat",
            WireProtocol::Messages => b"messages",
        },
    );
    framed(&mut hash, route.url.as_bytes());
    framed(&mut hash, effective_org.as_bytes());
    framed(&mut hash, &(headers.len() as u64).to_be_bytes());
    for (name, value) in headers {
        framed(&mut hash, name.as_bytes());
        framed(&mut hash, value.as_bytes());
    }
    WireIdentity(hash.finalize().into())
}

/// Legacy provenance records only the unchanged effective transport. It is not
/// an exact inference proof and deliberately supplies no account/org authority.
pub(crate) fn legacy_snapshot_digest(route: &ModelRoute) -> Result<String, SelectionError> {
    validate_route(route).map_err(|_| SelectionError::InvalidRoute)?;
    let headers = route
        .headers
        .iter()
        .filter(|(name, _)| {
            !name.eq_ignore_ascii_case("x-org-id")
                && !name.eq_ignore_ascii_case("anthropic-version")
        })
        .map(|(name, value)| (name.to_ascii_lowercase(), value.as_str()))
        .collect();
    let wire = wire_fingerprint(route, headers, &organization_value_digest(None));
    Ok(wire.0.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Stable private serialization of an exact candidate. Only this module owns
/// effective wire hashing; loading this record never invents an inference.
#[derive(Clone, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CandidateRecord {
    version: u8,
    profile: String,
    alias: String,
    account: String,
    provider: Provider,
    credential: String,
    service_generation: String,
    service: NetworkPolicy,
    organization_epoch: String,
    organization_digest: String,
    wire_digest: String,
}
impl fmt::Debug for CandidateRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CandidateRecord([REDACTED])")
    }
}
impl CandidateRecord {
    fn authority(&self) -> Result<ResolvedAuthority, SelectionError> {
        let hex_digest = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        };
        if self.version != 1
            || (self.profile != "default" && !opaque_id(&self.profile))
            || !crate::provider_server::safe_model(&self.alias, false)
            || self.alias.chars().any(char::is_whitespace)
            || !opaque_id(&self.account)
            || !opaque_id(&self.credential)
            || !opaque_id(&self.service_generation)
            || !opaque_id(&self.organization_epoch)
            || !hex_digest(&self.organization_digest)
            || !hex_digest(&self.wire_digest)
        {
            return Err(SelectionError::InvalidAuthority);
        }
        self.service
            .validate()
            .map_err(|_| SelectionError::InvalidAuthority)?;
        Ok(ResolvedAuthority {
            account: self.account.clone(),
            provider: self.provider,
            credential: self.credential.clone(),
            service_generation: self.service_generation.clone(),
            service: self.service.clone(),
            organization: OrganizationAuthority {
                epoch: self.organization_epoch.clone(),
                digest: self.organization_digest.clone(),
            },
        })
    }
    pub(crate) fn matches_route(
        &self,
        profile: &str,
        alias: &str,
        account: &str,
        route: &ModelRoute,
    ) -> bool {
        let Ok(authority) = self.authority() else {
            return false;
        };
        if self.profile != profile || self.alias != alias || self.account != account {
            return false;
        }
        wire_identity(&authority, route).is_ok_and(|wire| {
            let digest: String = wire.0.iter().map(|byte| format!("{byte:02x}")).collect();
            digest == self.wire_digest
        })
    }
    pub(crate) fn matches_current(&self, current: &AccountSnapshot) -> bool {
        match (self.authority(), ResolvedAuthority::from_snapshot(current)) {
            (Ok(expected), Ok(actual)) => expected == actual,
            _ => false,
        }
    }
}

/// Persisted evidence from a successful exact-candidate inference. No legacy
/// conversion constructor exists. CS-03 will consume the checked ticket seam.
#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceProof {
    candidate: CandidateRecord,
    checked_unix_ms: u64,
}
impl InferenceProof {
    pub(crate) fn matches_route(
        &self,
        profile: &str,
        alias: &str,
        account: &str,
        route: &ModelRoute,
    ) -> bool {
        self.checked_unix_ms != 0 && self.candidate.matches_route(profile, alias, account, route)
    }
    pub(crate) fn matches_current(&self, current: &AccountSnapshot) -> bool {
        self.candidate.matches_current(current)
    }
}

#[cfg(test)]
impl Verification {
    /// Future production persistence bridge: a successful verification's exact
    /// identity is serialized directly, never reconstructed from a cache row.
    pub(crate) fn persisted(&self) -> InferenceProof {
        let identity = &self.identity;
        let authority = &identity.authority;
        InferenceProof {
            candidate: CandidateRecord {
                version: 1,
                profile: identity.profile.clone(),
                alias: identity.alias.clone(),
                account: authority.account.clone(),
                provider: authority.provider,
                credential: authority.credential.clone(),
                service_generation: authority.service_generation.clone(),
                service: authority.service.clone(),
                organization_epoch: authority.organization.epoch.clone(),
                organization_digest: authority.organization.digest.clone(),
                wire_digest: identity
                    .wire
                    .0
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
            },
            checked_unix_ms: self.checked_unix_ms,
        }
    }
}

/// Immutable selected wire snapshot, deliberately independent of cache IDs and
/// of the applied registry. Debug cannot print URLs, headers, or raw org values.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct FrozenRoute {
    authority: ResolvedAuthority,
    route: ModelRoute,
    wire: WireIdentity,
}
#[cfg(test)]
impl fmt::Debug for FrozenRoute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FrozenRoute([REDACTED])")
    }
}
#[cfg(test)]
impl FrozenRoute {
    /// Reconstruct a draft from an already applied private route snapshot after
    /// restart/Revert. This creates no catalogue entry or inference proof.
    pub(crate) fn from_applied_snapshot(
        current: &AccountSnapshot,
        route: &ModelRoute,
    ) -> Result<Self, SelectionError> {
        Self::new(
            ResolvedAuthority::from_snapshot(current)?,
            compact_route(route),
        )
    }

    fn new(authority: ResolvedAuthority, route: ModelRoute) -> Result<Self, SelectionError> {
        let wire = wire_identity(&authority, &route)?;
        Ok(Self {
            authority,
            route,
            wire,
        })
    }
    pub(crate) fn route(&self) -> &ModelRoute {
        &self.route
    }
    pub(crate) fn authority(&self) -> &ResolvedAuthority {
        &self.authority
    }
    fn bytes(&self) -> usize {
        self.authority.bytes() + route_bytes(&self.route) + size_of::<WireIdentity>()
    }
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct AllocationDraft {
    pub alias: String,
    pub selected: FrozenRoute,
}
#[cfg(test)]
impl fmt::Debug for AllocationDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AllocationDraft([REDACTED])")
    }
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct DraftEdit {
    pub label: String,
    pub enabled: bool,
    pub default_alias: String,
    pub allocations: Vec<AllocationDraft>,
}
#[cfg(test)]
impl fmt::Debug for DraftEdit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DraftEdit")
            .field("allocations", &self.allocations.len())
            .finish_non_exhaustive()
    }
}
#[cfg(test)]
impl DraftEdit {
    fn compact(&self) -> Self {
        Self {
            label: self.label.as_str().into(),
            enabled: self.enabled,
            default_alias: self.default_alias.as_str().into(),
            allocations: self
                .allocations
                .iter()
                .map(|allocation| AllocationDraft {
                    alias: allocation.alias.as_str().into(),
                    selected: FrozenRoute {
                        authority: allocation.selected.authority.clone(),
                        route: compact_route(&allocation.selected.route),
                        wire: allocation.selected.wire.clone(),
                    },
                })
                .collect(),
        }
    }
    fn validate(&self) -> Result<usize, SelectionError> {
        if self.label.trim().is_empty()
            || self.label.len() > 128
            || self.label.chars().any(char::is_control)
            || self.allocations.len() > MAX_CLIENT_ALLOCATIONS
            || self.allocations.is_empty() != self.default_alias.is_empty()
        {
            return Err(SelectionError::InvalidDraft);
        }
        let mut aliases = std::collections::HashSet::new();
        let mut bytes = size_of::<Self>()
            + self.label.capacity()
            + self.default_alias.capacity()
            + self.allocations.capacity() * size_of::<AllocationDraft>();
        for allocation in &self.allocations {
            if !safe_model(&allocation.alias, false)
                || allocation.alias.chars().any(char::is_whitespace)
                || !aliases.insert(&allocation.alias)
            {
                return Err(SelectionError::InvalidDraft);
            }
            // Revalidate frozen data before accepting a full replacement.
            if wire_identity(&allocation.selected.authority, &allocation.selected.route)?
                != allocation.selected.wire
            {
                return Err(SelectionError::InvalidRoute);
            }
            bytes = bytes
                .checked_add(allocation.selected.bytes() + allocation.alias.capacity())
                .ok_or(SelectionError::AggregateBound)?;
        }
        if !self.allocations.is_empty() && !aliases.contains(&self.default_alias) {
            return Err(SelectionError::InvalidDraft);
        }
        Ok(bytes)
    }
}

#[cfg(test)]
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct CandidateIdentity {
    profile: String,
    alias: String,
    authority: ResolvedAuthority,
    wire: WireIdentity,
}
#[cfg(test)]
impl fmt::Debug for CandidateIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CandidateIdentity([REDACTED])")
    }
}

/// Successful explicit inference only. No legacy-applied provenance is minted
/// here; migration/publication need a separate later product transaction.
#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct Verification {
    identity: CandidateIdentity,
    checked_unix_ms: u64,
}
#[cfg(test)]
impl Verification {
    pub(crate) fn checked_unix_ms(&self) -> u64 {
        self.checked_unix_ms
    }
}

#[cfg(test)]
pub(crate) struct ProbeTicket {
    identity: CandidateIdentity,
    revision: u64,
}
#[cfg(test)]
pub(crate) struct ConnectionDraft {
    revision: u64,
    edit: DraftEdit,
    proofs: BTreeMap<String, Verification>,
    bytes: usize,
}
#[cfg(test)]
impl ConnectionDraft {
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
    pub(crate) fn edit(&self) -> &DraftEdit {
        &self.edit
    }
}

#[cfg(test)]
pub(crate) struct DraftBook {
    drafts: BTreeMap<String, ConnectionDraft>,
    selected: String,
    serial: u64,
    bytes: usize,
}

#[cfg(test)]
impl Default for DraftBook {
    fn default() -> Self {
        let edit = DraftEdit {
            label: "Default".into(),
            enabled: true,
            default_alias: String::new(),
            allocations: Vec::new(),
        };
        let bytes = edit.validate().expect("fixed empty default draft");
        Self {
            drafts: BTreeMap::from([(
                DEFAULT_PROFILE.into(),
                ConnectionDraft {
                    revision: 0,
                    edit,
                    proofs: BTreeMap::new(),
                    bytes,
                },
            )]),
            selected: DEFAULT_PROFILE.into(),
            serial: 0,
            bytes,
        }
    }
}

#[cfg(test)]
impl DraftBook {
    pub(crate) fn selected(&self) -> &str {
        &self.selected
    }
    pub(crate) fn select(&mut self, profile: &str) -> Result<(), SelectionError> {
        if !self.drafts.contains_key(profile) {
            return Err(SelectionError::MissingProfile);
        }
        self.selected = profile.into();
        Ok(())
    }
    pub(crate) fn get(&self, profile: &str) -> Result<&ConnectionDraft, SelectionError> {
        self.drafts
            .get(profile)
            .ok_or(SelectionError::MissingProfile)
    }

    /// Used both for explicit edits and product-owned Revert from applied
    /// snapshots. No applied object/config is held or mutated by this book.
    pub(crate) fn replace(
        &mut self,
        profile: &str,
        edit: DraftEdit,
    ) -> Result<u64, SelectionError> {
        if profile != DEFAULT_PROFILE && !opaque_id(profile) {
            return Err(SelectionError::InvalidDraft);
        }
        // Validate bounds before copying, then measure compact retained storage.
        edit.validate()?;
        let edit = edit.compact();
        let bytes = edit.validate()?;
        if !self.drafts.contains_key(profile) && self.drafts.len() >= MAX_CLIENT_PROFILES {
            return Err(SelectionError::ProfileBound);
        }
        let old = self.drafts.get(profile).map_or(0, |draft| draft.bytes);
        let total = self
            .bytes
            .checked_sub(old)
            .and_then(|n| n.checked_add(bytes))
            .ok_or(SelectionError::AggregateBound)?;
        if total > MAX_DRAFT_BYTES {
            return Err(SelectionError::AggregateBound);
        }
        let revision = self
            .serial
            .checked_add(1)
            .ok_or(SelectionError::RevisionExhausted)?;
        // Keep successful proofs for still-present aliases. Their exact identity
        // is checked at use; draft revision only guards in-flight probe returns.
        let proofs = self
            .drafts
            .get(profile)
            .map(|draft| {
                draft
                    .proofs
                    .iter()
                    .filter(|(alias, _)| edit.allocations.iter().any(|a| &a.alias == *alias))
                    .map(|(alias, proof)| (alias.clone(), proof.clone()))
                    .collect()
            })
            .unwrap_or_default();
        self.drafts.insert(
            profile.into(),
            ConnectionDraft {
                revision,
                edit,
                proofs,
                bytes,
            },
        );
        self.serial = revision;
        self.bytes = total;
        Ok(revision)
    }

    fn identity(
        &self,
        profile: &str,
        alias: &str,
        current: &AccountSnapshot,
    ) -> Result<CandidateIdentity, SelectionError> {
        let draft = self.get(profile)?;
        let allocation = draft
            .edit
            .allocations
            .iter()
            .find(|a| a.alias == alias)
            .ok_or(SelectionError::MissingAlias)?;
        let authority = ResolvedAuthority::from_snapshot(current)?;
        if allocation.selected.authority != authority {
            return Err(SelectionError::AuthorityChanged);
        }
        Ok(CandidateIdentity {
            profile: profile.into(),
            alias: alias.into(),
            authority,
            wire: allocation.selected.wire.clone(),
        })
    }

    /// The existing transport must acquire/validate this authority again before
    /// sending paid work. That I/O/lease contract is not implemented by a ticket.
    pub(crate) fn begin_probe(
        &self,
        profile: &str,
        alias: &str,
        current: &AccountSnapshot,
    ) -> Result<ProbeTicket, SelectionError> {
        Ok(ProbeTicket {
            identity: self.identity(profile, alias, current)?,
            revision: self.get(profile)?.revision,
        })
    }

    /// Revalidate after actual credential acquisition and immediately before
    /// preparing the paid request. The returned wire snapshot is immutable.
    pub(crate) fn probe_candidate(
        &self,
        ticket: &ProbeTicket,
        current: &AccountSnapshot,
    ) -> Result<&FrozenRoute, SelectionError> {
        let profile = &ticket.identity.profile;
        if self.get(profile)?.revision != ticket.revision {
            return Err(SelectionError::DraftChanged);
        }
        if self.identity(profile, &ticket.identity.alias, current)? != ticket.identity {
            return Err(SelectionError::DraftChanged);
        }
        self.get(profile)?
            .edit
            .allocations
            .iter()
            .find(|a| a.alias == ticket.identity.alias)
            .map(|a| &a.selected)
            .ok_or(SelectionError::MissingAlias)
    }

    /// Call only after an actual successful explicit inference, with authority
    /// reacquired on response. Failed I/O does not call this method or apply.
    pub(crate) fn accept_probe(
        &mut self,
        ticket: ProbeTicket,
        current: &AccountSnapshot,
        checked_unix_ms: u64,
    ) -> Result<(), SelectionError> {
        let profile = &ticket.identity.profile;
        self.probe_candidate(&ticket, current)?;
        self.drafts
            .get_mut(profile)
            .ok_or(SelectionError::MissingProfile)?
            .proofs
            .insert(
                ticket.identity.alias.clone(),
                Verification {
                    identity: ticket.identity,
                    checked_unix_ms,
                },
            );
        Ok(())
    }

    pub(crate) fn verification(
        &self,
        profile: &str,
        alias: &str,
        current: &AccountSnapshot,
    ) -> Result<Option<&Verification>, SelectionError> {
        let identity = self.identity(profile, alias, current)?;
        Ok(self
            .get(profile)?
            .proofs
            .get(alias)
            .filter(|proof| proof.identity == identity))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_accounts::{AccountPreferences, AccountState, AccountSummary};

    fn account(id: char, organization: Option<&str>) -> AccountSnapshot {
        let id = id.to_string().repeat(32);
        AccountSnapshot {
            summary: AccountSummary {
                id: id.clone(),
                provider: Provider::OpenCode,
                label: "Synthetic".into(),
                state: AccountState::CredentialStored,
            },
            preferences: AccountPreferences::legacy(&id),
            generation: "c".repeat(32),
            organization: OrganizationAuthority {
                epoch: "d".repeat(32),
                digest: organization_value_digest(organization),
            },
        }
    }
    fn route(id: &str, scope: &str) -> ModelRoute {
        ModelRoute {
            id: id.into(),
            model_id: "same-upstream-model".into(),
            provider: Provider::OpenCode,
            protocol: WireProtocol::Chat,
            url: format!("https://opencode.ai/inference/{scope}/chat/completions"),
            headers: Vec::new(),
        }
    }
    fn load(cache: &mut CatalogueCache, account: &AccountSnapshot, rows: Vec<ModelRoute>) {
        let request = cache.begin(account).unwrap();
        let bytes = serde_json::to_vec(&rows).unwrap().len();
        cache
            .complete(
                &request,
                ResolvedAuthority::from_snapshot(account).unwrap(),
                Some(account),
                rows,
                bytes,
            )
            .unwrap();
    }
    fn selection(account: &AccountSnapshot, row: ModelRoute) -> FrozenRoute {
        FrozenRoute::new(ResolvedAuthority::from_snapshot(account).unwrap(), row).unwrap()
    }
    fn edit(selected: FrozenRoute) -> DraftEdit {
        DraftEdit {
            label: "Work".into(),
            enabled: true,
            default_alias: "model".into(),
            allocations: vec![AllocationDraft {
                alias: "model".into(),
                selected,
            }],
        }
    }

    #[test]
    fn cs02_exact_proof_json_uses_the_existing_candidate_identity_and_refuses_scope_or_wire_edits()
    {
        let current = account('a', None);
        let row = route("catalogue-id", "openai/v1");
        let mut drafts = DraftBook::default();
        drafts
            .replace(DEFAULT_PROFILE, edit(selection(&current, row.clone())))
            .unwrap();
        let ticket = drafts
            .begin_probe(DEFAULT_PROFILE, "model", &current)
            .unwrap();
        drafts.accept_probe(ticket, &current, 1234).unwrap();
        let proof = drafts
            .verification(DEFAULT_PROFILE, "model", &current)
            .unwrap()
            .unwrap()
            .persisted();
        let bytes = serde_json::to_vec(&proof).unwrap();
        let restored: InferenceProof = serde_json::from_slice(&bytes).unwrap();
        assert!(restored.matches_route(DEFAULT_PROFILE, "model", &current.summary.id, &row));
        assert!(restored.matches_current(&current));
        let mut renamed = row.clone();
        renamed.id = "private/other-registry-key".into();
        assert!(restored.matches_route(DEFAULT_PROFILE, "model", &current.summary.id, &renamed));
        renamed.model_id = "different-paid-model".into();
        assert!(!restored.matches_route(DEFAULT_PROFILE, "model", &current.summary.id, &renamed));
        let mut changed = current.clone();
        changed.organization.epoch = "e".repeat(32);
        assert!(!restored.matches_current(&changed));
        assert!(!restored.matches_route(DEFAULT_PROFILE, "new-alias", &current.summary.id, &row));
        let value = serde_json::to_value(&proof).unwrap();
        assert!(value["candidate"].get("organization").is_none());
        for field in ["version", "wire_digest", "organization_epoch"] {
            let mut value = value.clone();
            value["candidate"][field] = if field == "version" {
                99.into()
            } else {
                "invalid".into()
            };
            let invalid: InferenceProof = serde_json::from_value(value).unwrap();
            assert!(!invalid.matches_route(DEFAULT_PROFILE, "model", &current.summary.id, &row));
        }
        let mut unknown = value;
        unknown["candidate"]["token"] = "must-not-be-in-proof".into();
        assert!(serde_json::from_value::<InferenceProof>(unknown).is_err());
    }

    #[test]
    fn equal_catalogue_ids_are_account_local_and_refresh_cannot_overwrite_frozen_routes() {
        let a = account('a', None);
        let b = account('b', None);
        let mut cache = CatalogueCache::default();
        load(&mut cache, &a, vec![route("shared-id", "account-a")]);
        let applied_snapshot = cache.select(&a, "shared-id").unwrap();
        load(&mut cache, &b, vec![route("shared-id", "account-b")]);
        assert!(
            cache
                .select(&a, "shared-id")
                .unwrap()
                .route()
                .url
                .contains("account-a")
        );
        assert!(
            cache
                .select(&b, "shared-id")
                .unwrap()
                .route()
                .url
                .contains("account-b")
        );
        assert!(applied_snapshot.wire != cache.select(&b, "shared-id").unwrap().wire);
        load(&mut cache, &a, vec![route("shared-id", "later-a")]);
        assert!(applied_snapshot.route().url.contains("account-a"));
        cache.evict(a.summary.id.as_str());
        assert_eq!(cache.status(&a.summary.id), CatalogueStatus::NotLoaded);
        assert!(applied_snapshot.route().url.contains("account-a"));
        assert_eq!(applied_snapshot.authority().account(), a.summary.id);
    }

    #[test]
    fn superseded_and_evicted_catalogue_tickets_never_publish_late_rows() {
        let a = account('a', None);
        let mut cache = CatalogueCache::default();
        let old = cache.begin(&a).unwrap();
        let newest = cache.begin(&a).unwrap();
        assert_eq!(
            cache.complete(
                &old,
                ResolvedAuthority::from_snapshot(&a).unwrap(),
                Some(&a),
                vec![route("old", "old")],
                100
            ),
            Err(SelectionError::LateResult)
        );
        assert_eq!(cache.status(&a.summary.id), CatalogueStatus::Loading);
        cache
            .complete(
                &newest,
                ResolvedAuthority::from_snapshot(&a).unwrap(),
                Some(&a),
                vec![route("new", "new")],
                100,
            )
            .unwrap();
        assert_eq!(
            cache.select(&a, "old").err(),
            Some(SelectionError::MissingRoute)
        );
        let evicted = cache.begin(&a).unwrap();
        cache.evict(&a.summary.id);
        load(&mut cache, &a, vec![route("current", "current")]);
        assert_eq!(
            cache.complete(
                &evicted,
                ResolvedAuthority::from_snapshot(&a).unwrap(),
                Some(&a),
                vec![route("old", "old")],
                100
            ),
            Err(SelectionError::LateResult)
        );
        assert!(cache.select(&a, "current").is_ok());
    }

    #[test]
    fn failed_refresh_retains_stale_rows_but_cannot_select_them_as_fresh() {
        let a = account('a', None);
        let mut cache = CatalogueCache::default();
        load(&mut cache, &a, vec![route("same", "before")]);
        cache.invalidate(&a.summary.id);
        assert_eq!(cache.status(&a.summary.id), CatalogueStatus::Stale);
        assert_eq!(cache.stale_rows(&a.summary.id), 1);
        assert_eq!(
            cache.select(&a, "same").err(),
            Some(SelectionError::CatalogueUnavailable)
        );
        let request = cache.begin(&a).unwrap();
        cache.fail(&request).unwrap();
        assert_eq!(cache.status(&a.summary.id), CatalogueStatus::Error);
        assert_eq!(cache.stale_rows(&a.summary.id), 1);
        assert_eq!(
            cache.select(&a, "same").err(),
            Some(SelectionError::CatalogueUnavailable)
        );
    }

    #[test]
    fn catalogue_response_compares_actual_authority_and_allows_only_initial_org_resolution() {
        let a = account('a', None);
        let mut resolved = a.clone();
        resolved.organization = OrganizationAuthority {
            epoch: "e".repeat(32),
            digest: organization_value_digest(Some("synthetic-org")),
        };
        let mut cache = CatalogueCache::default();
        let request = cache.begin(&a).unwrap();
        cache
            .complete(
                &request,
                ResolvedAuthority::from_snapshot(&resolved).unwrap(),
                Some(&resolved),
                vec![route("same", "resolved")],
                100,
            )
            .unwrap();
        assert_eq!(
            cache.select(&a, "same").err(),
            Some(SelectionError::CatalogueUnavailable)
        );
        assert!(cache.select(&resolved, "same").is_ok());
        let request = cache.begin(&resolved).unwrap();
        let mut changed = resolved.clone();
        changed.organization.epoch = "f".repeat(32);
        assert_eq!(
            cache.complete(
                &request,
                ResolvedAuthority::from_snapshot(&resolved).unwrap(),
                Some(&changed),
                vec![route("same", "late")],
                100
            ),
            Err(SelectionError::AuthorityChanged)
        );
        let request = cache.begin(&a).unwrap();
        let mut absent_aba = a.clone();
        absent_aba.organization.epoch = "e".repeat(32);
        assert_eq!(
            cache.complete(
                &request,
                ResolvedAuthority::from_snapshot(&absent_aba).unwrap(),
                Some(&absent_aba),
                vec![],
                0
            ),
            Err(SelectionError::AuthorityChanged)
        );
        let request = cache.begin(&resolved).unwrap();
        assert_eq!(
            cache.complete(
                &request,
                ResolvedAuthority::from_snapshot(&resolved).unwrap(),
                None,
                vec![],
                0
            ),
            Err(SelectionError::AuthorityChanged)
        );
    }

    #[test]
    fn resolved_catalogue_body_cannot_cross_reconnect_service_change_or_disable() {
        let a = account('a', None);
        let mut changes = Vec::new();
        let mut reconnect = a.clone();
        reconnect.generation = "e".repeat(32);
        changes.push(reconnect);
        let mut service = a.clone();
        service.preferences.service_generation = "e".repeat(32);
        changes.push(service);
        let mut policy = a.clone();
        policy.preferences.service = NetworkPolicy::Proxy("http://127.0.0.1:8011".into());
        changes.push(policy);
        for changed in changes {
            let mut cache = CatalogueCache::default();
            let request = cache.begin(&a).unwrap();
            assert_eq!(
                cache.complete(
                    &request,
                    ResolvedAuthority::from_snapshot(&changed).unwrap(),
                    Some(&changed),
                    vec![route("same", "late")],
                    100
                ),
                Err(SelectionError::AuthorityChanged)
            );
            assert_eq!(
                cache.select(&changed, "same").err(),
                Some(SelectionError::CatalogueUnavailable)
            );
        }
        let mut cache = CatalogueCache::default();
        let request = cache.begin(&a).unwrap();
        let mut disabled = a.clone();
        disabled.preferences.enabled = false;
        assert_eq!(
            cache.complete(
                &request,
                ResolvedAuthority::from_snapshot(&a).unwrap(),
                Some(&disabled),
                vec![],
                0
            ),
            Err(SelectionError::AuthorityChanged)
        );
    }

    #[test]
    fn catalogue_caps_routes_response_and_duplicate_ids_before_replacing_data() {
        let a = account('a', None);
        let mut cache = CatalogueCache::default();
        load(&mut cache, &a, vec![route("retained", "before")]);
        let request = cache.begin(&a).unwrap();
        assert_eq!(
            cache.complete(
                &request,
                ResolvedAuthority::from_snapshot(&a).unwrap(),
                Some(&a),
                vec![route("x", "x"); MAX_MODELS + 1],
                1
            ),
            Err(SelectionError::RouteBound)
        );
        assert_eq!(cache.stale_rows(&a.summary.id), 1);
        let request = cache.begin(&a).unwrap();
        assert_eq!(
            cache.complete(
                &request,
                ResolvedAuthority::from_snapshot(&a).unwrap(),
                Some(&a),
                vec![],
                CATALOG_BYTES + 1
            ),
            Err(SelectionError::ResponseBound)
        );
        let request = cache.begin(&a).unwrap();
        assert_eq!(
            cache.complete(
                &request,
                ResolvedAuthority::from_snapshot(&a).unwrap(),
                Some(&a),
                vec![route("duplicate", "one"), route("duplicate", "two")],
                100
            ),
            Err(SelectionError::DuplicateCatalogueId)
        );
    }

    #[test]
    fn catalogue_account_and_aggregate_limits_are_independent() {
        let mut bounded = CatalogueCache::default();
        for i in 1..=MAX_CATALOGUE_ACCOUNTS {
            let mut a = account('a', None);
            a.summary.id = format!("{i:032x}");
            bounded.begin(&a).unwrap();
        }
        assert_eq!(
            bounded.begin(&account('b', None)).err(),
            Some(SelectionError::AccountBound)
        );
        let mut cache = CatalogueCache::default();
        let mut aggregate_rejected = false;
        for i in 1..=MAX_CATALOGUE_ACCOUNTS {
            let mut a = account('a', None);
            a.summary.id = format!("{i:032x}");
            let request = cache.begin(&a).unwrap();
            let rows = (0..MAX_MODELS)
                .map(|n| route(&format!("route-{n}"), &"x".repeat(2800)))
                .collect::<Vec<_>>();
            let before = cache.retained_bytes();
            match cache.complete(
                &request,
                ResolvedAuthority::from_snapshot(&a).unwrap(),
                Some(&a),
                rows,
                CATALOG_BYTES,
            ) {
                Ok(()) => assert!(cache.retained_bytes() <= MAX_CACHE_BYTES),
                Err(SelectionError::AggregateBound) => {
                    assert_eq!(cache.retained_bytes(), before);
                    assert_eq!(cache.status(&a.summary.id), CatalogueStatus::Error);
                    aggregate_rejected = true;
                    break;
                }
                Err(other) => panic!("unexpected bounded-fixture error: {other:?}"),
            }
        }
        assert!(
            aggregate_rejected,
            "individual valid catalogues must still encounter the aggregate memory cap"
        );
    }

    #[test]
    fn wire_identity_ignores_registry_key_and_header_case_but_not_effective_wire_or_org() {
        let a = account('a', Some("synthetic-private-org"));
        let authority = ResolvedAuthority::from_snapshot(&a).unwrap();
        let original = route("catalogue-key", "one");
        let mut renamed = original.clone();
        renamed.id = "private-registry-key".into();
        assert!(
            wire_identity(&authority, &original).unwrap()
                == wire_identity(&authority, &renamed).unwrap()
        );
        renamed.headers = vec![("X-Org-ID".into(), "synthetic-private-org".into())];
        assert!(
            wire_identity(&authority, &original).unwrap()
                == wire_identity(&authority, &renamed).unwrap()
        );
        renamed.headers[0].0 = "x-org-id".into();
        assert!(
            wire_identity(&authority, &original).unwrap()
                == wire_identity(&authority, &renamed).unwrap()
        );
        let mut changes = Vec::new();
        let mut model = original.clone();
        model.model_id = "another-wire-model".into();
        changes.push(model);
        let mut url = original.clone();
        url.url = route("same", "two").url;
        changes.push(url);
        let mut protocol = original.clone();
        protocol.protocol = WireProtocol::Messages;
        protocol.url = "https://opencode.ai/inference/one/messages".into();
        changes.push(protocol);
        for changed in changes {
            assert!(
                wire_identity(&authority, &original).unwrap()
                    != wire_identity(&authority, &changed).unwrap()
            );
        }
        let mut other_org = a.clone();
        other_org.organization.digest = organization_value_digest(Some("other-org"));
        assert!(
            wire_identity(&authority, &original).unwrap()
                != wire_identity(
                    &ResolvedAuthority::from_snapshot(&other_org).unwrap(),
                    &original
                )
                .unwrap()
        );
        renamed
            .headers
            .push(("X-ORG-ID".into(), "synthetic-private-org".into()));
        assert_eq!(
            wire_identity(&authority, &renamed).err(),
            Some(SelectionError::DuplicateHeader)
        );
        renamed.headers = vec![("x-org-id".into(), "wrong-org".into())];
        assert_eq!(
            wire_identity(&authority, &renamed).err(),
            Some(SelectionError::OrganizationConflict)
        );
    }

    #[test]
    fn default_and_seven_custom_drafts_retain_independent_edits_and_reject_an_eighth_custom() {
        let a = account('a', None);
        let mut book = DraftBook::default();
        book.replace(
            DEFAULT_PROFILE,
            edit(selection(&a, route("default", "default"))),
        )
        .unwrap();
        for i in 1..MAX_CLIENT_PROFILES {
            let profile = format!("{i:032x}");
            book.replace(
                &profile,
                edit(selection(&a, route("custom", &format!("custom-{i}")))),
            )
            .unwrap();
            book.select(&profile).unwrap();
            assert_eq!(book.selected(), profile);
        }
        assert_eq!(
            book.replace(
                &"f".repeat(32),
                edit(selection(&a, route("overflow", "overflow")))
            ),
            Err(SelectionError::ProfileBound)
        );
        book.select(DEFAULT_PROFILE).unwrap();
        assert!(
            book.get(DEFAULT_PROFILE).unwrap().edit().allocations[0]
                .selected
                .route()
                .url
                .contains("/default/")
        );
        book.select(&format!("{:032x}", 1)).unwrap();
        assert!(
            book.get(book.selected()).unwrap().edit().allocations[0]
                .selected
                .route()
                .url
                .contains("custom-1")
        );
        let mut invalid = book.get(DEFAULT_PROFILE).unwrap().edit().clone();
        invalid.default_alias = "missing".into();
        let revision = book.get(DEFAULT_PROFILE).unwrap().revision();
        assert_eq!(
            book.replace(DEFAULT_PROFILE, invalid),
            Err(SelectionError::InvalidDraft)
        );
        assert_eq!(book.get(DEFAULT_PROFILE).unwrap().revision(), revision);
    }

    #[test]
    fn editing_then_reverting_cannot_accept_an_old_probe_but_switching_forms_can() {
        let a = account('a', None);
        let original = edit(selection(&a, route("same", "original")));
        let mut book = DraftBook::default();
        book.replace(DEFAULT_PROFILE, original.clone()).unwrap();
        let old = book.begin_probe(DEFAULT_PROFILE, "model", &a).unwrap();
        book.replace(
            DEFAULT_PROFILE,
            edit(selection(&a, route("same", "changed"))),
        )
        .unwrap();
        book.replace(DEFAULT_PROFILE, original.clone()).unwrap();
        assert_eq!(
            book.accept_probe(old, &a, 10),
            Err(SelectionError::DraftChanged)
        );
        assert!(
            book.verification(DEFAULT_PROFILE, "model", &a)
                .unwrap()
                .is_none()
        );
        let current = book.begin_probe(DEFAULT_PROFILE, "model", &a).unwrap();
        let custom = "e".repeat(32);
        book.replace(&custom, original).unwrap();
        book.select(&custom).unwrap();
        book.select(DEFAULT_PROFILE).unwrap();
        book.accept_probe(current, &a, 11).unwrap();
        assert_eq!(
            book.verification(DEFAULT_PROFILE, "model", &a)
                .unwrap()
                .unwrap()
                .checked_unix_ms(),
            11
        );
    }

    #[test]
    fn allocation_bound_is_enforced_and_caller_overcapacity_is_not_retained() {
        let a = account('a', None);
        let mut book = DraftBook::default();
        let mut complete = edit(selection(&a, route("same", "same")));
        complete.default_alias = "model-0".into();
        complete.allocations = (0..MAX_CLIENT_ALLOCATIONS)
            .map(|i| AllocationDraft {
                alias: format!("model-{i}"),
                selected: selection(&a, route("same", "same")),
            })
            .collect();
        book.replace(DEFAULT_PROFILE, complete.clone()).unwrap();
        let revision = book.get(DEFAULT_PROFILE).unwrap().revision();
        complete.allocations.push(AllocationDraft {
            alias: "overflow".into(),
            selected: selection(&a, route("same", "same")),
        });
        assert_eq!(
            book.replace(DEFAULT_PROFILE, complete),
            Err(SelectionError::InvalidDraft)
        );
        assert_eq!(book.get(DEFAULT_PROFILE).unwrap().revision(), revision);
        let mut overcapacity = edit(selection(&a, route("one", "one")));
        overcapacity.allocations.reserve(1024);
        overcapacity.label.reserve(1024);
        book.replace(DEFAULT_PROFILE, overcapacity).unwrap();
        assert!(
            book.get(DEFAULT_PROFILE)
                .unwrap()
                .edit()
                .allocations
                .capacity()
                < 100
        );
        assert!(book.get(DEFAULT_PROFILE).unwrap().edit().label.capacity() < 100);
    }

    #[test]
    fn successful_proof_survives_label_and_registry_metadata_edits_but_not_changed_wire() {
        let a = account('a', None);
        let mut book = DraftBook::default();
        book.replace(
            DEFAULT_PROFILE,
            edit(selection(&a, route("one-key", "one"))),
        )
        .unwrap();
        let ticket = book.begin_probe(DEFAULT_PROFILE, "model", &a).unwrap();
        book.accept_probe(ticket, &a, 12).unwrap();
        let mut renamed = edit(selection(&a, route("another-registry-key", "one")));
        renamed.label = "Changed label".into();
        book.replace(DEFAULT_PROFILE, renamed).unwrap();
        let mut relabeled = a.clone();
        relabeled.summary.label = "New account label".into();
        assert!(
            book.verification(DEFAULT_PROFILE, "model", &relabeled)
                .unwrap()
                .is_some()
        );
        book.replace(
            DEFAULT_PROFILE,
            edit(selection(&a, route("another-registry-key", "two"))),
        )
        .unwrap();
        assert!(
            book.verification(DEFAULT_PROFILE, "model", &a)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn every_authority_change_refuses_late_probe_or_cached_proof() {
        let a = account('a', Some("synthetic-org"));
        let mut changes = Vec::new();
        let mut credential = a.clone();
        credential.generation = "e".repeat(32);
        changes.push(credential);
        let mut service = a.clone();
        service.preferences.service_generation = "e".repeat(32);
        changes.push(service);
        let mut policy = a.clone();
        policy.preferences.service = NetworkPolicy::Proxy("http://127.0.0.1:8011".into());
        changes.push(policy);
        let mut epoch = a.clone();
        epoch.organization.epoch = "e".repeat(32);
        changes.push(epoch);
        let mut organization = a.clone();
        organization.organization.digest = organization_value_digest(Some("other-org"));
        changes.push(organization);
        let mut disabled = a.clone();
        disabled.preferences.enabled = false;
        changes.push(disabled);
        for changed in changes {
            let mut book = DraftBook::default();
            book.replace(DEFAULT_PROFILE, edit(selection(&a, route("same", "same"))))
                .unwrap();
            let old = book.begin_probe(DEFAULT_PROFILE, "model", &a).unwrap();
            assert!(
                book.begin_probe(DEFAULT_PROFILE, "model", &changed)
                    .is_err()
            );
            assert!(book.probe_candidate(&old, &a).is_ok());
            assert!(book.probe_candidate(&old, &changed).is_err());
            assert!(book.accept_probe(old, &changed, 12).is_err());
            let current = book.begin_probe(DEFAULT_PROFILE, "model", &a).unwrap();
            book.accept_probe(current, &a, 13).unwrap();
            assert!(
                book.verification(DEFAULT_PROFILE, "model", &changed)
                    .is_err()
            );
        }
    }

    #[test]
    fn header_organization_and_private_routes_never_escape_new_type_debug() {
        let a = account('a', Some("synthetic-private-org"));
        let mut row = route("catalogue-key", "private-route-path");
        row.headers
            .push(("x-org-id".into(), "synthetic-private-org".into()));
        let selected = selection(&a, row);
        let values = format!(
            "{:?} {:?} {:?}",
            selected.authority(),
            selected,
            edit(selected.clone())
        );
        for private in [
            "synthetic-private-org",
            "private-route-path",
            "x-org-id",
            "catalogue-key",
        ] {
            assert!(!values.contains(private));
        }
    }

    #[test]
    fn reconstructing_an_applied_snapshot_does_not_invent_catalogue_or_verification_evidence() {
        let a = account('a', None);
        let cache = CatalogueCache::default();
        let frozen =
            FrozenRoute::from_applied_snapshot(&a, &route("private-applied-key", "applied"))
                .unwrap();
        let mut book = DraftBook::default();
        book.replace(DEFAULT_PROFILE, edit(frozen)).unwrap();
        assert_eq!(cache.status(&a.summary.id), CatalogueStatus::NotLoaded);
        assert_eq!(
            cache.select(&a, "private-applied-key").err(),
            Some(SelectionError::CatalogueUnavailable)
        );
        assert!(
            book.verification(DEFAULT_PROFILE, "model", &a)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn organization_header_normalization_matches_the_actual_persistent_store_digest() {
        use crate::{
            provider_accounts::AccountStore,
            provider_auth::{Secret, TokenGrant, TokenLifecycle},
        };
        struct OwnedRoot(std::path::PathBuf);
        impl Drop for OwnedRoot {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let root = std::env::temp_dir().join(format!(
            "minicon-selection-{}",
            crate::provider_server::random_id().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        let owned = OwnedRoot(root.canonicalize().unwrap());
        let store = AccountStore::new(owned.0.clone()).unwrap();
        for organization in [None, Some("synthetic-private-org")] {
            let inserted = store
                .insert(
                    "Synthetic",
                    TokenGrant {
                        provider: Provider::OpenCode,
                        access_token: Secret::new("synthetic-not-authorized").unwrap(),
                        lifecycle: TokenLifecycle::ExpiringRefresh {
                            refresh_token: Secret::new("synthetic-no-refresh").unwrap(),
                            expires_at: std::time::Instant::now()
                                + std::time::Duration::from_secs(3600),
                        },
                        organization: organization.map(str::to_owned),
                    },
                )
                .unwrap();
            let reopened = AccountStore::new(owned.0.clone()).unwrap();
            let snapshot = reopened.snapshot(&inserted.id).unwrap();
            assert_eq!(
                snapshot.organization.digest,
                organization_value_digest(organization)
            );
            let mut explicit = route("same", "same");
            if let Some(value) = organization {
                explicit.headers.push(("x-org-id".into(), value.into()));
            }
            let authority = ResolvedAuthority::from_snapshot(&snapshot).unwrap();
            assert!(
                wire_identity(&authority, &explicit).unwrap()
                    == wire_identity(&authority, &route("different-key", "same")).unwrap()
            );
        }
    }

    #[test]
    fn deepseek_unsent_organization_hints_are_not_transport_identity() {
        let mut a = account('a', None);
        a.summary.provider = Provider::DeepSeek;
        let authority = ResolvedAuthority::from_snapshot(&a).unwrap();
        let original = ModelRoute {
            id: "catalogue-id".into(),
            model_id: "fixture-model".into(),
            provider: Provider::DeepSeek,
            protocol: WireProtocol::Messages,
            url: "https://api.deepseek.com/anthropic/v1/messages".into(),
            headers: Vec::new(),
        };
        let mut hinted = original.clone();
        hinted
            .headers
            .push(("X-Org-ID".into(), "synthetic-unsent-hint".into()));
        assert!(
            wire_identity(&authority, &original).unwrap()
                == wire_identity(&authority, &hinted).unwrap()
        );
        a.organization.digest = organization_value_digest(Some("also-not-sent-by-deepseek"));
        let changed = ResolvedAuthority::from_snapshot(&a).unwrap();
        assert!(
            wire_identity(&authority, &original).unwrap()
                == wire_identity(&changed, &original).unwrap()
        );
        assert!(
            authority != changed,
            "organization authority remains distinct from transport header emission"
        );
    }
}
