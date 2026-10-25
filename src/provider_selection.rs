//! In-memory account catalogues and exact connection candidates. PRD 31.01.02.01.
//! No network, grants, persistence, admission, or automatic inference lives here.
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
pub(crate) const MAX_DRAFT_BYTES: usize = 4 * 1024 * 1024;
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
    CatalogueUnavailable,
    MissingRoute,
    InvalidDraft,
    VerificationRequired,
    ProfileBound,
    MissingProfile,
    MissingAlias,
    RevisionExhausted,
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

impl Verification {
    /// A successful verification's exact identity is serialized directly,
    /// never reconstructed from a catalogue row or account-wide health.
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
#[derive(Clone)]
pub(crate) struct FrozenRoute {
    authority: ResolvedAuthority,
    route: ModelRoute,
    wire: WireIdentity,
}
impl fmt::Debug for FrozenRoute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FrozenRoute([REDACTED])")
    }
}
impl FrozenRoute {
    /// Reconstruct a draft from an already applied private route snapshot after
    /// restart/Revert. This creates no catalogue entry or inference proof.
    #[cfg(test)]
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
    #[cfg(test)]
    pub(crate) fn authority(&self) -> &ResolvedAuthority {
        &self.authority
    }
    fn bytes(&self) -> usize {
        self.authority.bytes() + route_bytes(&self.route) + size_of::<WireIdentity>()
    }
}

#[derive(Clone)]
pub(crate) struct AllocationDraft {
    pub alias: String,
    pub selected: FrozenRoute,
}
impl fmt::Debug for AllocationDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AllocationDraft([REDACTED])")
    }
}

#[derive(Clone)]
pub(crate) struct DraftEdit {
    pub label: String,
    pub enabled: bool,
    pub default_alias: String,
    pub allocations: Vec<AllocationDraft>,
}
impl fmt::Debug for DraftEdit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DraftEdit")
            .field("allocations", &self.allocations.len())
            .finish_non_exhaustive()
    }
}
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

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct CandidateIdentity {
    profile: String,
    alias: String,
    authority: ResolvedAuthority,
    wire: WireIdentity,
}
impl fmt::Debug for CandidateIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CandidateIdentity([REDACTED])")
    }
}

/// Successful explicit inference only. No legacy-applied provenance is minted
/// here; migration/publication need a separate later product transaction.
#[derive(Clone, Debug)]
pub(crate) struct Verification {
    identity: CandidateIdentity,
    checked_unix_ms: u64,
}
impl Verification {
    pub(crate) fn checked_unix_ms(&self) -> u64 {
        self.checked_unix_ms
    }
}

#[derive(Clone)]
pub(crate) struct ProbeTicket {
    identity: CandidateIdentity,
    revision: u64,
}
#[derive(Clone)]
pub(crate) struct ConnectionDraft {
    revision: u64,
    edit: DraftEdit,
    proofs: BTreeMap<String, Verification>,
    bytes: usize,
    form: Option<ProfileForm>,
}
impl ConnectionDraft {
    #[cfg(test)]
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
    #[cfg(test)]
    pub(crate) fn edit(&self) -> &DraftEdit {
        &self.edit
    }
}

#[derive(Clone)]
pub(crate) struct DraftBook {
    drafts: BTreeMap<String, ConnectionDraft>,
    selected: String,
    applied_registry: BTreeMap<String, std::sync::Arc<ModelRoute>>,
    serial: u64,
    bytes: usize,
}

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
                    form: None,
                },
            )]),
            selected: DEFAULT_PROFILE.into(),
            applied_registry: BTreeMap::new(),
            serial: 0,
            bytes,
        }
    }
}

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
        let proofs: BTreeMap<String, Verification> = self
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
        let bytes = bytes
            .checked_add(proofs.values().map(verification_bytes).sum::<usize>())
            .ok_or(SelectionError::AggregateBound)?;
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
        self.drafts.insert(
            profile.into(),
            ConnectionDraft {
                revision,
                edit,
                proofs,
                bytes,
                form: None,
            },
        );
        self.serial = revision;
        self.bytes = total;
        Ok(revision)
    }

    #[cfg(test)]
    fn identity(
        &self,
        profile: &str,
        alias: &str,
        current: &AccountSnapshot,
    ) -> Result<CandidateIdentity, SelectionError> {
        let draft = self.get(profile)?;
        if draft.form.as_ref().is_some_and(|form| {
            form.error.is_some()
                || form.input.allocations.iter().any(|row| {
                    !matches!(
                        form.selections
                            .get(&row.slot)
                            .map(|selection| &selection.source),
                        Some(FormSource::Selected(_))
                    )
                })
        }) {
            return Err(SelectionError::DraftChanged);
        }
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
    #[cfg(test)]
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
    #[cfg(test)]
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
    #[cfg(test)]
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

    #[cfg(test)]
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

/// Safe editable fields; no endpoint, headers, grant or evidence can be supplied.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectionAllocationInput {
    pub slot: u8,
    pub alias: String,
    pub account: Option<String>,
    pub model: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectionDraftInput {
    pub label: String,
    pub enabled: bool,
    pub default_alias: String,
    pub allocations: Vec<ConnectionAllocationInput>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionInputError {
    InvalidLabel,
    InvalidAlias,
    DuplicateAlias,
    InvalidDefaultAlias,
    IncompleteAllocation,
    UnselectedModel,
    DefaultCompatibility,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionAvailability {
    Available,
    MissingAccount,
    DisabledAccount,
    CredentialUnavailable,
    AuthorityChanged,
    NotSelected,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionEvidence {
    Unverified,
    LegacyApplied,
    Verified,
    StaleAuthority,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectionAllocationView {
    pub slot: u8,
    pub upstream_model: Option<String>,
    pub protocol: Option<WireProtocol>,
    pub availability: ConnectionAvailability,
    pub evidence: ConnectionEvidence,
    pub checked_unix_ms: Option<u64>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectionProfileView {
    pub id: String,
    pub revision: u64,
    pub input: ConnectionDraftInput,
    pub applied: Option<ConnectionDraftInput>,
    pub dirty: bool,
    pub error: Option<ConnectionInputError>,
    pub allocations: Vec<ConnectionAllocationView>,
}

/// An observation is not a new authority. Missing/corrupt accounts are retained
/// as finite observations, independently of the immutable applied route.
pub(crate) type ConnectionAccounts =
    BTreeMap<String, Result<AccountSnapshot, ConnectionAvailability>>;

#[derive(Clone)]
struct AppliedReference {
    profile: String,
    alias: String,
    account: String,
    route: std::sync::Arc<ModelRoute>,
    origin: crate::provider_server::AppliedOrigin,
}
#[derive(Clone)]
enum FormSource {
    Applied(AppliedReference),
    Selected(FrozenRoute),
}
impl FormSource {
    fn account(&self) -> &str {
        match self {
            Self::Applied(reference) => &reference.account,
            Self::Selected(selected) => selected.authority.account(),
        }
    }
    fn route(&self) -> &ModelRoute {
        match self {
            Self::Applied(reference) => &reference.route,
            Self::Selected(selected) => selected.route(),
        }
    }
    fn bytes(&self) -> usize {
        match self {
            Self::Applied(reference) => {
                size_of::<AppliedReference>()
                    + reference.profile.len()
                    + reference.alias.len()
                    + reference.account.len()
                    // Immutable applied routes are charged once in DraftBook's
                    // registry; account/alias/evidence remain per allocation.
                    // Applied evidence originated from bounded Config bytes.
                    + match &reference.origin {
                        crate::provider_server::AppliedOrigin::LegacyApplied { snapshot_digest } => snapshot_digest.len(),
                        crate::provider_server::AppliedOrigin::Inference { proof } => proof.retained_bytes(),
                    }
            }
            Self::Selected(selected) => selected.bytes(),
        }
    }
}
#[derive(Clone)]
struct FormSelection {
    text: String,
    source: FormSource,
}
#[derive(Clone)]
struct ProfileForm {
    input: ConnectionDraftInput,
    applied: Option<ConnectionDraftInput>,
    selections: BTreeMap<u8, FormSelection>,
    error: Option<ConnectionInputError>,
}

impl ConnectionDraftInput {
    fn compact(&self) -> Self {
        Self {
            label: self.label.as_str().into(),
            enabled: self.enabled,
            default_alias: self.default_alias.as_str().into(),
            allocations: self
                .allocations
                .iter()
                .map(|row| ConnectionAllocationInput {
                    slot: row.slot,
                    alias: row.alias.as_str().into(),
                    account: row.account.as_deref().map(Into::into),
                    model: row.model.as_str().into(),
                })
                .collect(),
        }
    }
    pub(crate) fn bounded(&self) -> Result<(), SelectionError> {
        let text =
            |value: &str, limit| value.len() <= limit && !value.chars().any(char::is_control);
        if !text(&self.label, 128)
            || !text(&self.default_alias, 200)
            || self.allocations.len() > MAX_CLIENT_ALLOCATIONS
        {
            return Err(SelectionError::InvalidDraft);
        }
        let mut slots = std::collections::BTreeSet::new();
        for row in &self.allocations {
            if usize::from(row.slot) >= MAX_CLIENT_ALLOCATIONS
                || !slots.insert(row.slot)
                || !text(&row.alias, 200)
                || !text(&row.model, 200)
                || row.account.as_ref().is_some_and(|id| !opaque_id(id))
            {
                return Err(SelectionError::InvalidDraft);
            }
        }
        Ok(())
    }
    fn bytes(&self) -> usize {
        size_of::<Self>()
            + self.label.len()
            + self.default_alias.len()
            + self.allocations.len() * size_of::<ConnectionAllocationInput>()
            + self
                .allocations
                .iter()
                .map(|row| {
                    row.alias.len() + row.model.len() + row.account.as_ref().map_or(0, String::len)
                })
                .sum::<usize>()
    }
}
impl ProfileForm {
    fn validate(&self, profile: &str) -> Option<ConnectionInputError> {
        use ConnectionInputError as E;
        if self.input.label.trim().is_empty() {
            return Some(E::InvalidLabel);
        }
        if profile == DEFAULT_PROFILE
            && (!self.input.enabled
                || self.input.label != "Default"
                || self.input.allocations.len() > 1
                || self
                    .input
                    .allocations
                    .iter()
                    .any(|row| row.slot != 0 || row.alias != crate::provider_server::MODEL_ALIAS)
                || (!self.input.default_alias.is_empty()
                    && self.input.default_alias != crate::provider_server::MODEL_ALIAS))
        {
            return Some(E::DefaultCompatibility);
        }
        let mut aliases = std::collections::BTreeSet::new();
        for row in &self.input.allocations {
            if !safe_model(&row.alias, false) || row.alias.chars().any(char::is_whitespace) {
                return Some(E::InvalidAlias);
            }
            if !aliases.insert(&row.alias) {
                return Some(E::DuplicateAlias);
            }
        }
        for row in &self.input.allocations {
            if row.account.is_none() || row.model.trim().is_empty() {
                return Some(E::IncompleteAllocation);
            }
            if !self.selections.contains_key(&row.slot) {
                return Some(E::UnselectedModel);
            }
        }
        if self.input.allocations.is_empty() != self.input.default_alias.is_empty()
            || (!self.input.allocations.is_empty() && !aliases.contains(&self.input.default_alias))
        {
            return Some(E::InvalidDefaultAlias);
        }
        None
    }
    fn bytes(&self) -> Result<usize, SelectionError> {
        self.selections.values().try_fold(
            self.input.bytes() + self.applied.as_ref().map_or(0, ConnectionDraftInput::bytes),
            |total, selected| {
                total
                    .checked_add(
                        selected.text.len()
                            + selected.source.bytes()
                            + size_of::<(u8, FormSelection)>(),
                    )
                    .ok_or(SelectionError::AggregateBound)
            },
        )
    }
}

impl InferenceProof {
    fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.candidate.profile.len()
            + self.candidate.alias.len()
            + self.candidate.wire_digest.len()
            + self
                .candidate
                .authority()
                .map_or(0, |authority| authority.bytes())
    }
    /// Imports only already persisted exact evidence. No clock is read and no
    /// successful inference is simulated by calling accept_probe.
    fn restore(
        &self,
        profile: &str,
        alias: &str,
        account: &str,
        route: &ModelRoute,
    ) -> Result<Verification, SelectionError> {
        if !self.matches_route(profile, alias, account, route) {
            return Err(SelectionError::InvalidDraft);
        }
        let authority = self.candidate.authority()?;
        let wire = wire_identity(&authority, route)?;
        Ok(Verification {
            identity: CandidateIdentity {
                profile: profile.into(),
                alias: alias.into(),
                authority,
                wire,
            },
            checked_unix_ms: self.checked_unix_ms,
        })
    }
}

impl DraftBook {
    pub(crate) fn from_applied(
        config: &crate::provider_server::Config,
    ) -> Result<Self, SelectionError> {
        config
            .validate()
            .map_err(|_| SelectionError::InvalidDraft)?;
        let mut book = Self::default();
        for route in &config.models {
            let route = compact_route(route);
            let bytes = route_bytes(&route)
                + route.id.len()
                + size_of::<(String, std::sync::Arc<ModelRoute>)>();
            book.bytes = book
                .bytes
                .checked_add(bytes)
                .ok_or(SelectionError::AggregateBound)?;
            if book.bytes > MAX_DRAFT_BYTES {
                return Err(SelectionError::AggregateBound);
            }
            book.applied_registry
                .insert(route.id.clone(), std::sync::Arc::new(route));
        }
        book.install_applied(config, DEFAULT_PROFILE)?;
        for profile in &config.profiles {
            book.install_applied(config, &profile.id)?;
        }
        Ok(book)
    }

    pub(crate) fn referenced_accounts(&self) -> Vec<String> {
        self.drafts
            .values()
            .filter_map(|draft| draft.form.as_ref())
            .flat_map(|form| {
                form.input
                    .allocations
                    .iter()
                    .filter_map(|row| row.account.clone())
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn applied_form(
        config: &crate::provider_server::Config,
        profile: &str,
        registry: &BTreeMap<String, std::sync::Arc<ModelRoute>>,
    ) -> Result<(ProfileForm, BTreeMap<String, Verification>), SelectionError> {
        use crate::provider_server::{AppliedOrigin, MODEL_ALIAS};
        let mut selections = BTreeMap::new();
        let mut proofs = BTreeMap::new();
        let mut allocations = Vec::new();
        let shared_route = |route: &ModelRoute| {
            registry
                .get(&route.id)
                .filter(|cached| cached.as_ref() == route)
                .cloned()
                .ok_or(SelectionError::LateResult)
        };
        let (label, enabled, default_alias) = if profile == DEFAULT_PROFILE {
            if let Ok(resolved) = config.resolve_allocation(profile, None) {
                let origin = resolved
                    .evidence
                    .ok_or(SelectionError::InvalidDraft)?
                    .origin
                    .clone();
                let model = resolved.route.model_id.clone();
                allocations.push(ConnectionAllocationInput {
                    slot: 0,
                    alias: MODEL_ALIAS.into(),
                    account: Some(resolved.account.into()),
                    model: model.clone(),
                });
                if let AppliedOrigin::Inference { proof } = &origin {
                    proofs.insert(
                        MODEL_ALIAS.into(),
                        proof.restore(profile, MODEL_ALIAS, resolved.account, resolved.route)?,
                    );
                }
                selections.insert(
                    0,
                    FormSelection {
                        text: model,
                        source: FormSource::Applied(AppliedReference {
                            profile: profile.into(),
                            alias: MODEL_ALIAS.into(),
                            account: resolved.account.into(),
                            route: shared_route(resolved.route)?,
                            origin,
                        }),
                    },
                );
            } else if config.selected_account.is_some() || !config.model.is_empty() {
                allocations.push(ConnectionAllocationInput {
                    slot: 0,
                    alias: MODEL_ALIAS.into(),
                    account: config.selected_account.clone(),
                    model: config.model.clone(),
                });
            }
            (
                "Default".into(),
                true,
                if allocations.is_empty() {
                    String::new()
                } else {
                    MODEL_ALIAS.into()
                },
            )
        } else {
            let applied = config
                .profiles
                .iter()
                .find(|entry| entry.id == profile)
                .ok_or(SelectionError::MissingProfile)?;
            for (index, row) in applied.allocations.iter().enumerate() {
                let slot = u8::try_from(index).map_err(|_| SelectionError::RouteBound)?;
                let resolved = config
                    .resolve_allocation(profile, Some(&row.alias))
                    .map_err(|_| SelectionError::MissingRoute)?;
                let origin = resolved
                    .evidence
                    .ok_or(SelectionError::InvalidDraft)?
                    .origin
                    .clone();
                let model = resolved.route.model_id.clone();
                allocations.push(ConnectionAllocationInput {
                    slot,
                    alias: row.alias.clone(),
                    account: Some(row.account.clone()),
                    model: model.clone(),
                });
                if let AppliedOrigin::Inference { proof } = &origin {
                    proofs.insert(
                        row.alias.clone(),
                        proof.restore(profile, &row.alias, &row.account, resolved.route)?,
                    );
                }
                selections.insert(
                    slot,
                    FormSelection {
                        text: model,
                        source: FormSource::Applied(AppliedReference {
                            profile: profile.into(),
                            alias: row.alias.clone(),
                            account: row.account.clone(),
                            route: shared_route(resolved.route)?,
                            origin,
                        }),
                    },
                );
            }
            (
                applied.label.clone(),
                applied.enabled,
                applied.default_model.clone(),
            )
        };
        let input = ConnectionDraftInput {
            label,
            enabled,
            default_alias,
            allocations,
        };
        input.bounded()?;
        let mut form = ProfileForm {
            applied: Some(input.clone()),
            input,
            selections,
            error: None,
        };
        form.error = form.validate(profile);
        Ok((form, proofs))
    }

    fn install_applied(
        &mut self,
        config: &crate::provider_server::Config,
        profile: &str,
    ) -> Result<u64, SelectionError> {
        let (form, proofs) = Self::applied_form(config, profile, &self.applied_registry)?;
        self.commit_form(profile, form, Some(proofs))
    }

    fn check_revision(&self, profile: &str, expected: u64) -> Result<(), SelectionError> {
        if self.get(profile)?.revision != expected {
            return Err(SelectionError::DraftChanged);
        }
        Ok(())
    }

    pub(crate) fn revision(&self, profile: &str) -> Option<u64> {
        self.drafts.get(profile).map(|draft| draft.revision)
    }

    fn form(&self, profile: &str) -> Result<&ProfileForm, SelectionError> {
        self.get(profile)?
            .form
            .as_ref()
            .ok_or(SelectionError::InvalidDraft)
    }

    fn commit_form(
        &mut self,
        profile: &str,
        mut form: ProfileForm,
        imported: Option<BTreeMap<String, Verification>>,
    ) -> Result<u64, SelectionError> {
        form.input.bounded()?;
        form.input = form.input.compact();
        form.error = form.validate(profile);
        // The original tested semantic DraftEdit remains the checked candidate
        // layer. Incomplete raw forms cannot use its last valid candidate.
        let mut edit = DraftEdit {
            label: form.input.label.clone(),
            enabled: form.input.enabled,
            default_alias: String::new(),
            allocations: Vec::new(),
        };
        if edit.label.trim().is_empty() {
            edit.label = "Draft".into();
        }
        if form.error.is_none()
            && form
                .selections
                .values()
                .all(|selected| matches!(selected.source, FormSource::Selected(_)))
        {
            edit.default_alias = form.input.default_alias.clone();
            for row in &form.input.allocations {
                let Some(FormSelection {
                    source: FormSource::Selected(selected),
                    ..
                }) = form.selections.get(&row.slot)
                else {
                    return Err(SelectionError::InvalidDraft);
                };
                edit.allocations.push(AllocationDraft {
                    alias: row.alias.clone(),
                    selected: selected.clone(),
                });
            }
        } else if let Some(previous) = self.drafts.get(profile) {
            // Keep the last validated candidate while raw text is incomplete.
            // Form validation/ownership still blocks any use of this snapshot.
            edit = previous.edit.clone();
        }
        let compact = edit.compact();
        let edit_bytes = compact.validate()?;
        if profile != DEFAULT_PROFILE && !opaque_id(profile) {
            return Err(SelectionError::InvalidDraft);
        }
        if !self.drafts.contains_key(profile) && self.drafts.len() >= MAX_CLIENT_PROFILES {
            return Err(SelectionError::ProfileBound);
        }
        let proofs = imported.unwrap_or_else(|| {
            self.drafts
                .get(profile)
                .map(|draft| {
                    draft
                        .proofs
                        .iter()
                        .filter(|(alias, _)| {
                            form.input.allocations.iter().any(|row| {
                                &row.alias == *alias
                                    && draft.form.as_ref().is_none_or(|old| {
                                        old.input.allocations.iter().any(|previous| {
                                            previous.slot == row.slot && previous.alias == row.alias
                                        })
                                    })
                            })
                        })
                        .map(|(alias, proof)| (alias.clone(), proof.clone()))
                        .collect()
                })
                .unwrap_or_default()
        });
        let bytes = edit_bytes
            .checked_add(form.bytes()?)
            .and_then(|n| n.checked_add(proofs.values().map(verification_bytes).sum::<usize>()))
            .ok_or(SelectionError::AggregateBound)?;
        let old = self.drafts.get(profile).map_or(0, |draft| draft.bytes);
        let total = self
            .bytes
            .checked_sub(old)
            .and_then(|value| value.checked_add(bytes))
            .ok_or(SelectionError::AggregateBound)?;
        if total > MAX_DRAFT_BYTES {
            return Err(SelectionError::AggregateBound);
        }
        let revision = self.replace(profile, compact)?;
        let draft = self
            .drafts
            .get_mut(profile)
            .ok_or(SelectionError::MissingProfile)?;
        draft.proofs = proofs;
        draft.bytes = bytes;
        draft.form = Some(form);
        self.bytes = total;
        Ok(revision)
    }

    pub(crate) fn create_profile(&mut self, id: &str) -> Result<u64, SelectionError> {
        if id == DEFAULT_PROFILE || self.drafts.contains_key(id) {
            return Err(SelectionError::InvalidDraft);
        }
        let input = ConnectionDraftInput {
            label: "New client".into(),
            enabled: true,
            default_alias: String::new(),
            allocations: Vec::new(),
        };
        let revision = self.commit_form(
            id,
            ProfileForm {
                input,
                applied: None,
                selections: BTreeMap::new(),
                error: None,
            },
            None,
        )?;
        self.selected = id.into();
        Ok(revision)
    }

    pub(crate) fn edit_input(
        &mut self,
        profile: &str,
        expected: u64,
        input: ConnectionDraftInput,
    ) -> Result<u64, SelectionError> {
        self.check_revision(profile, expected)?;
        input.bounded()?;
        let mut form = self.form(profile)?.clone();
        form.selections.retain(|slot, selected| {
            input.allocations.iter().any(|row| {
                row.slot == *slot
                    && row.account.as_deref() == Some(selected.source.account())
                    && row.model == selected.text
            })
        });
        form.input = input;
        self.commit_form(profile, form, None)
    }

    pub(crate) fn choose(
        &mut self,
        profile: &str,
        expected: u64,
        slot: u8,
        selected: FrozenRoute,
        catalogue_id: &str,
    ) -> Result<u64, SelectionError> {
        self.check_revision(profile, expected)?;
        if !safe_model(catalogue_id, false) {
            return Err(SelectionError::InvalidDraft);
        }
        let mut form = self.form(profile)?.clone();
        let row = form
            .input
            .allocations
            .iter_mut()
            .find(|row| row.slot == slot)
            .ok_or(SelectionError::MissingAlias)?;
        row.account = Some(selected.authority.account().into());
        row.model = catalogue_id.into();
        form.selections.insert(
            slot,
            FormSelection {
                text: catalogue_id.into(),
                source: FormSource::Selected(selected),
            },
        );
        self.commit_form(profile, form, None)
    }

    pub(crate) fn remove_allocation(
        &mut self,
        profile: &str,
        expected: u64,
        slot: u8,
    ) -> Result<u64, SelectionError> {
        self.check_revision(profile, expected)?;
        let mut input = self.form(profile)?.input.clone();
        if !input.allocations.iter().any(|row| row.slot == slot) {
            return Err(SelectionError::MissingAlias);
        }
        input.allocations.retain(|row| row.slot != slot);
        if input.allocations.is_empty() {
            input.default_alias.clear();
        }
        self.edit_input(profile, expected, input)
    }

    pub(crate) fn revert(
        &mut self,
        profile: &str,
        expected: u64,
        config: &crate::provider_server::Config,
    ) -> Result<u64, SelectionError> {
        self.check_revision(profile, expected)?;
        if self.form(profile)?.applied.is_none() {
            return Err(SelectionError::MissingProfile);
        }
        self.install_applied(config, profile)
    }

    pub(crate) fn discard(&mut self, profile: &str, expected: u64) -> Result<(), SelectionError> {
        self.check_revision(profile, expected)?;
        if profile == DEFAULT_PROFILE || self.form(profile)?.applied.is_some() {
            return Err(SelectionError::InvalidDraft);
        }
        let serial = self
            .serial
            .checked_add(1)
            .ok_or(SelectionError::RevisionExhausted)?;
        let removed = self
            .drafts
            .remove(profile)
            .ok_or(SelectionError::MissingProfile)?;
        self.bytes -= removed.bytes;
        self.serial = serial;
        if self.selected == profile {
            self.selected = DEFAULT_PROFILE.into();
        }
        Ok(())
    }

    pub(crate) fn views(&self, accounts: &ConnectionAccounts) -> Vec<ConnectionProfileView> {
        self.drafts.iter().filter_map(|(id, draft)| {
            let form = draft.form.as_ref()?;
            let allocations = form.input.allocations.iter().map(|row| {
                let selected = form.selections.get(&row.slot);
                let observation = row.account.as_ref().and_then(|account| accounts.get(account));
                let current = observation.and_then(|value| value.as_ref().ok());
                let availability = match observation {
                    Some(Err(reason)) => *reason,
                    None => ConnectionAvailability::MissingAccount,
                    Some(Ok(snapshot)) if !snapshot.preferences.enabled => ConnectionAvailability::DisabledAccount,
                    Some(Ok(snapshot)) => match selected.map(|selected| &selected.source) {
                        Some(FormSource::Selected(selected)) => {
                            if ResolvedAuthority::from_snapshot(snapshot).is_ok_and(|authority| authority == selected.authority) {
                                ConnectionAvailability::Available
                            } else { ConnectionAvailability::AuthorityChanged }
                        }
                        Some(FormSource::Applied(reference)) => {
                            if snapshot.summary.provider != reference.route.provider {
                                ConnectionAvailability::AuthorityChanged
                            } else if draft.proofs.get(&row.alias).is_some_and(|proof| {
                                proof.identity.profile == *id && proof.identity.alias == row.alias
                                    && proof.identity.authority.account() == reference.account
                                    && ResolvedAuthority::from_snapshot(snapshot).is_ok_and(|authority| {
                                        authority == proof.identity.authority
                                            && wire_identity(&authority,&reference.route).is_ok_and(|wire|wire==proof.identity.wire)
                                    })
                            }) { ConnectionAvailability::Available } else { match &reference.origin {
                                crate::provider_server::AppliedOrigin::Inference { proof } if !proof.matches_current(snapshot) => ConnectionAvailability::AuthorityChanged,
                                _ => ConnectionAvailability::Available,
                            } }
                        }
                        None => ConnectionAvailability::NotSelected,
                    },
                };
                let proof = draft.proofs.get(&row.alias).filter(|proof| selected.is_some_and(|selected| {
                    proof.identity.profile == *id && proof.identity.alias == row.alias
                        && proof.identity.authority.account() == selected.source.account()
                        && wire_identity(&proof.identity.authority, selected.source.route()).is_ok_and(|wire| wire == proof.identity.wire)
                }));
                let legacy = selected.is_some_and(|selected| match &selected.source {
                    FormSource::Applied(reference) => reference.profile == *id && reference.alias == row.alias
                        && matches!(reference.origin, crate::provider_server::AppliedOrigin::LegacyApplied { .. }),
                    FormSource::Selected(_) => false,
                });
                let evidence = if let Some(proof) = proof {
                    if current.and_then(|snapshot| ResolvedAuthority::from_snapshot(snapshot).ok()).as_ref() == Some(&proof.identity.authority) {
                        ConnectionEvidence::Verified
                    } else { ConnectionEvidence::StaleAuthority }
                } else if legacy { ConnectionEvidence::LegacyApplied } else { ConnectionEvidence::Unverified };
                ConnectionAllocationView { slot: row.slot,
                    upstream_model: selected.map(|selected| selected.source.route().model_id.clone()),
                    protocol: selected.map(|selected| selected.source.route().protocol),
                    availability, evidence, checked_unix_ms: proof.map(|proof| proof.checked_unix_ms()) }
            }).collect();
            Some(ConnectionProfileView { id: id.clone(), revision: draft.revision,
                input: form.input.clone(), applied: form.applied.clone(),
                dirty: form.applied.as_ref() != Some(&form.input)
                    || form.selections.values().any(|selected| matches!(selected.source, FormSource::Selected(_))),
                error: form.error, allocations })
        }).collect()
    }
}

/// Immutable exact raw-form candidate. Never contains a grant or public route.
#[derive(Clone)]
pub(crate) struct FrozenCandidate {
    identity: CandidateIdentity,
    route: std::sync::Arc<ModelRoute>,
}
impl fmt::Debug for FrozenCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FrozenCandidate([REDACTED])")
    }
}
impl FrozenCandidate {
    pub(crate) fn profile(&self) -> &str {
        &self.identity.profile
    }
    pub(crate) fn alias(&self) -> &str {
        &self.identity.alias
    }
    pub(crate) fn account(&self) -> &str {
        self.identity.authority.account()
    }
    pub(crate) fn route(&self) -> &ModelRoute {
        &self.route
    }
    pub(crate) fn identity(&self) -> &CandidateIdentity {
        &self.identity
    }
    pub(crate) fn matches_current(&self, current: &AccountSnapshot) -> bool {
        ResolvedAuthority::from_snapshot(current).is_ok_and(|authority| {
            authority == self.identity.authority
                && wire_identity(&authority, &self.route)
                    .is_ok_and(|wire| wire == self.identity.wire)
        })
    }
}

#[derive(Clone)]
pub(crate) struct ProbePermit(std::sync::Arc<std::sync::atomic::AtomicU8>);
impl ProbePermit {
    pub(crate) fn new() -> Self {
        Self(std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0)))
    }
    pub(crate) fn invalidate(&self) {
        self.0.store(2, std::sync::atomic::Ordering::Release);
    }
    pub(crate) fn try_admit(&self) -> bool {
        self.0
            .compare_exchange(
                0,
                1,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
    }
    pub(crate) fn is_admitted(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire) == 1
    }
}

impl DraftBook {
    pub(crate) fn probe_account(
        &self,
        profile: &str,
        revision: u64,
        slot: u8,
    ) -> Result<&str, SelectionError> {
        self.check_revision(profile, revision)?;
        let form = self.form(profile)?;
        if form.error.is_some() {
            return Err(SelectionError::InvalidDraft);
        }
        form.input
            .allocations
            .iter()
            .find(|row| row.slot == slot)
            .and_then(|row| row.account.as_deref())
            .ok_or(SelectionError::InvalidDraft)
    }

    pub(crate) fn freeze_probe(
        &self,
        profile: &str,
        revision: u64,
        slot: u8,
        current: &AccountSnapshot,
    ) -> Result<(ProbeTicket, FrozenCandidate), SelectionError> {
        let account = self.probe_account(profile, revision, slot)?;
        let form = self.form(profile)?;
        let row = form
            .input
            .allocations
            .iter()
            .find(|row| row.slot == slot)
            .ok_or(SelectionError::MissingAlias)?;
        let selected = form
            .selections
            .get(&slot)
            .ok_or(SelectionError::MissingRoute)?;
        if selected.text != row.model || account != current.summary.id {
            return Err(SelectionError::DraftChanged);
        }
        let authority = ResolvedAuthority::from_snapshot(current)?;
        let route = match &selected.source {
            FormSource::Selected(frozen) => {
                if frozen.authority != authority {
                    return Err(SelectionError::AuthorityChanged);
                }
                std::sync::Arc::new(compact_route(&frozen.route))
            }
            FormSource::Applied(applied) => {
                if applied.profile != profile
                    || applied.account != account
                    || applied.route.model_id != row.model
                {
                    return Err(SelectionError::DraftChanged);
                }
                applied.route.clone()
            }
        };
        let identity = CandidateIdentity {
            profile: profile.into(),
            alias: row.alias.clone(),
            wire: wire_identity(&authority, &route)?,
            authority,
        };
        Ok((
            ProbeTicket {
                identity: identity.clone(),
                revision,
            },
            FrozenCandidate { identity, route },
        ))
    }

    pub(crate) fn accept_probe_observation(
        &mut self,
        ticket: ProbeTicket,
        observation: crate::provider_server::ProbeObservation,
        current: &AccountSnapshot,
        permit: &ProbePermit,
    ) -> Result<(), SelectionError> {
        if !permit.is_admitted() {
            return Err(SelectionError::LateResult);
        }
        let profile = &ticket.identity.profile;
        let form = self.form(profile)?;
        let slot = form
            .input
            .allocations
            .iter()
            .find(|row| row.alias == ticket.identity.alias)
            .map(|row| row.slot)
            .ok_or(SelectionError::MissingAlias)?;
        let (fresh, _) = self.freeze_probe(profile, ticket.revision, slot, current)?;
        let (observed, checked_unix_ms) = observation.into_parts();
        if fresh.identity != ticket.identity || observed != ticket.identity || checked_unix_ms == 0
        {
            return Err(SelectionError::DraftChanged);
        }
        // Bounded current proof state only; never accumulate operation history.
        let proof = Verification {
            identity: ticket.identity.clone(),
            checked_unix_ms,
        };
        let additional = verification_bytes(&proof);
        let old = self
            .get(profile)?
            .proofs
            .get(&ticket.identity.alias)
            .map_or(0, verification_bytes);
        let total = self
            .bytes
            .checked_sub(old)
            .and_then(|n| n.checked_add(additional))
            .ok_or(SelectionError::AggregateBound)?;
        if total > MAX_DRAFT_BYTES {
            return Err(SelectionError::AggregateBound);
        }
        let draft = self
            .drafts
            .get_mut(profile)
            .ok_or(SelectionError::MissingProfile)?;
        draft.bytes = draft
            .bytes
            .checked_sub(old)
            .and_then(|n| n.checked_add(additional))
            .ok_or(SelectionError::AggregateBound)?;
        draft.proofs.insert(ticket.identity.alias, proof);
        self.bytes = total;
        Ok(())
    }
}

/// All data needed after replacement is prepared before either publication
/// lock. Fields remain product-private; this is not a public proof constructor.
pub(crate) struct PreparedConnectionApply {
    pub config: crate::provider_server::Config,
    pub book: DraftBook,
    pub authorities: Vec<AccountSnapshot>,
}

impl DraftBook {
    /// Build from the actual editable form, including mixed retained-applied
    /// and selected rows. The last valid semantic edit is never an Apply input.
    pub(crate) fn prepare_apply(
        &self,
        profile: &str,
        revision: u64,
        previous: &crate::provider_server::Config,
        accounts: &ConnectionAccounts,
    ) -> Result<PreparedConnectionApply, SelectionError> {
        use crate::provider_server::{
            AppliedEvidence, AppliedOrigin, ClientAllocation, ClientProfile, MODEL_ALIAS,
        };
        self.check_revision(profile, revision)?;
        let form = self.form(profile)?;
        if form.error.is_some() {
            return Err(SelectionError::InvalidDraft);
        }
        let mut config = previous.clone();
        config.version = 3;
        let mut allocations = Vec::new();
        let mut evidence = Vec::new();
        let mut authorities = BTreeMap::new();
        for row in &form.input.allocations {
            let account = row.account.as_deref().ok_or(SelectionError::InvalidDraft)?;
            let selected = form
                .selections
                .get(&row.slot)
                .ok_or(SelectionError::MissingRoute)?;
            if selected.text != row.model || selected.source.account() != account {
                return Err(SelectionError::DraftChanged);
            }
            // Legacy is retained only from the actual previous applied object.
            // Neither account readiness nor a catalogue row can create it.
            let retained_legacy = Self::retained_legacy(profile, row, selected, previous);
            let current = accounts.get(account).and_then(|entry| entry.as_ref().ok());
            let checked = current.and_then(|current| {
                let (ticket, candidate) = self
                    .freeze_probe(profile, revision, row.slot, current)
                    .ok()?;
                let proof = self.get(profile).ok()?.proofs.get(&row.alias)?;
                (proof.identity == ticket.identity && proof.checked_unix_ms != 0)
                    .then_some((current, candidate, proof))
            });
            let (route, origin) = if let Some((current, candidate, proof)) = checked {
                let key = config
                    .models
                    .iter()
                    .find(|route| {
                        wire_identity(&candidate.identity.authority, route)
                            .is_ok_and(|wire| wire == candidate.identity.wire)
                    })
                    .map(|route| route.id.clone());
                let key = match key {
                    Some(key) => key,
                    None => {
                        let mut route = compact_route(candidate.route());
                        let id = crate::provider_server::random_id()
                            .map_err(|_| SelectionError::RevisionExhausted)?;
                        route.id = format!("snapshot-{id}");
                        if config.models.iter().any(|existing| existing.id == route.id) {
                            return Err(SelectionError::DuplicateCatalogueId);
                        }
                        let key = route.id.clone();
                        config.models.push(route);
                        key
                    }
                };
                authorities
                    .entry(account.to_owned())
                    .or_insert_with(|| current.clone());
                (
                    key,
                    AppliedOrigin::Inference {
                        proof: Box::new(proof.persisted()),
                    },
                )
            } else if let Some(retained) = retained_legacy {
                retained
            } else {
                // Invalid authority and absence of an exact successful check
                // remain distinct from the preserved legacy-only exception.
                let current = current.ok_or(SelectionError::AuthorityChanged)?;
                self.freeze_probe(profile, revision, row.slot, current)?;
                return Err(SelectionError::VerificationRequired);
            };
            allocations.push(ClientAllocation {
                alias: row.alias.clone(),
                account: account.into(),
                route,
            });
            evidence.push(AppliedEvidence {
                profile: profile.into(),
                alias: row.alias.clone(),
                account: account.into(),
                origin,
            });
        }
        config
            .applied_evidence
            .retain(|entry| entry.profile != profile);
        config.applied_evidence.extend(evidence);
        if profile == DEFAULT_PROFILE {
            if let Some(row) = allocations.first() {
                // Preserve a legacy accepted model ID whenever its effective
                // original allocation remains unchanged; private IDs are not wire IDs.
                let unchanged =
                    previous
                        .resolve_allocation(profile, None)
                        .ok()
                        .is_some_and(|prior| {
                            prior.account == row.account
                                && config
                                    .models
                                    .iter()
                                    .find(|route| route.id == row.route)
                                    .is_some_and(|route| {
                                        legacy_snapshot_digest(prior.route).ok()
                                            == legacy_snapshot_digest(route).ok()
                                    })
                        });
                config.selected_account = Some(row.account.clone());
                config.model = if unchanged {
                    previous.model.clone()
                } else {
                    MODEL_ALIAS.into()
                };
                config.default_route = Some(row.route.clone());
            } else {
                config.selected_account = None;
                config.model.clear();
                config.default_route = None;
            }
        } else {
            let prepared = ClientProfile {
                id: profile.into(),
                label: form.input.label.clone(),
                enabled: form.input.enabled,
                default_model: form.input.default_alias.clone(),
                allocations,
            };
            if let Some(existing) = config.profiles.iter_mut().find(|entry| entry.id == profile) {
                *existing = prepared;
            } else {
                config.profiles.push(prepared);
            }
        }
        let mut referenced: std::collections::BTreeSet<String> = config
            .profiles
            .iter()
            .flat_map(|entry| entry.allocations.iter().map(|row| row.route.clone()))
            .collect();
        if config.selected_account.is_some() && !config.model.is_empty() {
            referenced.insert(
                config
                    .default_route
                    .as_deref()
                    .unwrap_or(&config.model)
                    .into(),
            );
        }
        config.models.retain(|route| referenced.contains(&route.id));
        config
            .encode_applied()
            .map_err(|_| SelectionError::InvalidDraft)?;
        if authorities.len() > MAX_CATALOGUE_ACCOUNTS {
            return Err(SelectionError::AccountBound);
        }

        // Preparing a bounded copy is fallible. Do it before replacement; never
        // rebuild all eight forms from Config after already committing bytes.
        let mut book = self.clone();
        let old_registry_bytes = book
            .applied_registry
            .iter()
            .map(|(key, route)| {
                route_bytes(route) + key.len() + size_of::<(String, std::sync::Arc<ModelRoute>)>()
            })
            .sum::<usize>();
        let mut registry = BTreeMap::new();
        let mut new_registry_bytes = 0usize;
        for route in &config.models {
            let shared = book
                .applied_registry
                .get(&route.id)
                .filter(|existing| existing.as_ref() == route)
                .cloned()
                .unwrap_or_else(|| std::sync::Arc::new(compact_route(route)));
            new_registry_bytes = new_registry_bytes
                .checked_add(
                    route_bytes(&shared)
                        + route.id.len()
                        + size_of::<(String, std::sync::Arc<ModelRoute>)>(),
                )
                .ok_or(SelectionError::AggregateBound)?;
            registry.insert(route.id.clone(), shared);
        }
        book.bytes = book
            .bytes
            .checked_sub(old_registry_bytes)
            .and_then(|bytes| bytes.checked_add(new_registry_bytes))
            .ok_or(SelectionError::AggregateBound)?;
        if book.bytes > MAX_DRAFT_BYTES {
            return Err(SelectionError::AggregateBound);
        }
        book.applied_registry = registry;
        let (mut applied, mut proofs) =
            Self::applied_form(&config, profile, &book.applied_registry)?;
        // An Apply ack must not reinterpret sparse stable UI slot identities.
        let slots: BTreeMap<_, _> = form
            .input
            .allocations
            .iter()
            .map(|row| (row.alias.clone(), row.slot))
            .collect();
        let mut selections = BTreeMap::new();
        for row in &mut applied.input.allocations {
            let old = row.slot;
            row.slot = *slots.get(&row.alias).ok_or(SelectionError::MissingAlias)?;
            selections.insert(
                row.slot,
                applied
                    .selections
                    .remove(&old)
                    .ok_or(SelectionError::MissingRoute)?,
            );
        }
        applied.selections = selections;
        applied.applied = Some(applied.input.clone());
        for (alias, proof) in &self.get(profile)?.proofs {
            if slots.contains_key(alias) {
                proofs.entry(alias.clone()).or_insert_with(|| proof.clone());
            }
        }
        book.commit_form(profile, applied, Some(proofs))?;
        Ok(PreparedConnectionApply {
            config,
            book,
            authorities: authorities.into_values().collect(),
        })
    }
}

#[derive(Clone)]
pub(crate) struct ProfileVerificationStep {
    pub slot: u8,
    pub ticket: ProbeTicket,
    pub candidate: FrozenCandidate,
}
#[derive(Clone)]
pub(crate) struct ProfileVerificationPlan {
    pub fingerprint: [u8; 32],
    pub scope: [u8; 32],
    pub steps: Vec<ProfileVerificationStep>,
}
impl DraftBook {
    fn retained_legacy(
        profile: &str,
        row: &ConnectionAllocationInput,
        selected: &FormSelection,
        previous: &crate::provider_server::Config,
    ) -> Option<(String, crate::provider_server::AppliedOrigin)> {
        use crate::provider_server::AppliedOrigin;
        let account = row.account.as_deref()?;
        match &selected.source {
            FormSource::Applied(reference) => previous
                .resolve_allocation(profile, Some(&row.alias))
                .ok()
                .filter(|prior| {
                    reference.profile == profile
                        && reference.alias == row.alias
                        && reference.account == account
                        && reference.route.model_id == row.model
                        && prior.account == account
                        && prior.route == reference.route.as_ref()
                        && prior
                            .evidence
                            .is_some_and(|old| old.origin == reference.origin)
                        && matches!(&reference.origin, AppliedOrigin::LegacyApplied { .. })
                })
                .map(|prior| {
                    (
                        prior.route.id.clone(),
                        prior.evidence.unwrap().origin.clone(),
                    )
                }),
            FormSource::Selected(_) => None,
        }
    }

    pub(crate) fn verification_plan(
        &self,
        profile: &str,
        revision: u64,
        previous: &crate::provider_server::Config,
        accounts: &ConnectionAccounts,
    ) -> Result<ProfileVerificationPlan, SelectionError> {
        self.check_revision(profile, revision)?;
        let form = self.form(profile)?;
        if form.error.is_some() {
            return Err(SelectionError::InvalidDraft);
        }
        let mut scope = Sha256::new();
        scope.update(b"minicon.profile-verification.scope.v1\0");
        for part in [profile, &form.input.label, &form.input.default_alias] {
            framed(&mut scope, part.as_bytes());
        }
        scope.update(revision.to_be_bytes());
        scope.update([u8::from(form.input.enabled)]);
        let mut evidence = Sha256::new();
        let mut steps = Vec::new();
        for row in &form.input.allocations {
            scope.update([row.slot]);
            for part in [
                row.alias.as_str(),
                row.account.as_deref().ok_or(SelectionError::InvalidDraft)?,
                row.model.as_str(),
            ] {
                framed(&mut scope, part.as_bytes());
            }
            let selected = form
                .selections
                .get(&row.slot)
                .ok_or(SelectionError::MissingRoute)?;
            if selected.text != row.model
                || row.account.as_deref() != Some(selected.source.account())
            {
                return Err(SelectionError::DraftChanged);
            }
            let legacy = Self::retained_legacy(profile, row, selected, previous);
            let current = row
                .account
                .as_ref()
                .and_then(|account| accounts.get(account))
                .and_then(|entry| entry.as_ref().ok());
            let frozen = current
                .and_then(|current| self.freeze_probe(profile, revision, row.slot, current).ok());
            let proof = frozen.as_ref().and_then(|(ticket, _)| {
                self.get(profile)
                    .ok()?
                    .proofs
                    .get(&row.alias)
                    .filter(|proof| proof.identity == ticket.identity && proof.checked_unix_ms != 0)
            });
            if let Some((ticket, candidate)) = frozen
                .as_ref()
                .filter(|_| proof.is_some() || legacy.is_none())
            {
                let authority = &ticket.identity.authority;
                for part in [
                    &authority.account,
                    &authority.credential,
                    &authority.service_generation,
                    &authority.organization.epoch,
                    &authority.organization.digest,
                ] {
                    framed(&mut scope, part.as_bytes());
                }
                scope.update([match authority.provider {
                    Provider::DeepSeek => 0,
                    Provider::OpenCode => 1,
                }]);
                match &authority.service {
                    NetworkPolicy::Direct => scope.update([0]),
                    NetworkPolicy::Proxy(endpoint) => {
                        scope.update([1]);
                        framed(&mut scope, endpoint.as_bytes());
                    }
                }
                framed(&mut scope, &ticket.identity.wire.0);
                if let Some(proof) = proof {
                    evidence.update([1]);
                    evidence.update(proof.checked_unix_ms.to_be_bytes());
                } else {
                    evidence.update([0]);
                    steps.push(ProfileVerificationStep {
                        slot: row.slot,
                        ticket: ticket.clone(),
                        candidate: candidate.clone(),
                    });
                }
            } else if let Some((
                _,
                crate::provider_server::AppliedOrigin::LegacyApplied { snapshot_digest },
            )) = legacy
            {
                scope.update([2]);
                framed(&mut scope, snapshot_digest.as_bytes());
                evidence.update([2]);
            } else {
                let current = current.ok_or(SelectionError::AuthorityChanged)?;
                self.freeze_probe(profile, revision, row.slot, current)?;
                return Err(SelectionError::VerificationRequired);
            }
        }
        let scope: [u8; 32] = scope.finalize().into();
        evidence.update(scope);
        Ok(ProfileVerificationPlan {
            fingerprint: evidence.finalize().into(),
            scope,
            steps,
        })
    }
}

fn verification_bytes(proof: &Verification) -> usize {
    size_of::<(String, Verification)>()
        + proof.identity.profile.capacity()
        + 2 * proof.identity.alias.capacity()
        + proof.identity.authority.bytes()
}

#[cfg(test)]
mod tests {

    #[test]
    fn cs04_apply_plan_preserves_sparse_slot_identity_other_drafts_and_exact_proof_import() {
        let config = cs02_applied_fixture();
        let current = account('a', None);
        let mut book = DraftBook::from_applied(&config).unwrap();
        let profile = config.profiles[0].id.clone();
        let view = book
            .views(&ConnectionAccounts::new())
            .into_iter()
            .find(|p| p.id == profile)
            .unwrap();
        let mut input = view.input;
        input.allocations[0].slot = 7;
        let revision = book.edit_input(&profile, view.revision, input).unwrap();
        let selected = selection(&current, route("catalogue-private-id", "prepared"));
        let revision = book
            .choose(&profile, revision, 7, selected, "catalogue-private-id")
            .unwrap();
        // Pure selection precondition only; runtime tests obtain their proof
        // through the real CS03 HTTP/observation/Owner acceptance path.
        let ticket = book
            .begin_probe(
                &profile,
                &book.form(&profile).unwrap().input.allocations[0].alias,
                &current,
            )
            .unwrap();
        book.accept_probe(ticket, &current, 1234).unwrap();
        let accounts =
            ConnectionAccounts::from([(current.summary.id.clone(), Ok(current.clone()))]);
        let old = book.views(&accounts);
        let prepared = book
            .prepare_apply(&profile, revision, &config, &accounts)
            .unwrap();
        let views = prepared.book.views(&accounts);
        let applied = views.iter().find(|p| p.id == profile).unwrap();
        assert_eq!(applied.input.allocations[0].slot, 7);
        assert!(!applied.dirty);
        assert_eq!(
            applied.allocations[0].evidence,
            ConnectionEvidence::Verified
        );
        assert_eq!(applied.allocations[0].checked_unix_ms, Some(1234));
        assert_eq!(
            old.iter().filter(|p| p.id != profile).collect::<Vec<_>>(),
            views.iter().filter(|p| p.id != profile).collect::<Vec<_>>()
        );
        let bytes = prepared.config.encode_applied().unwrap();
        let restored = DraftBook::from_applied(
            &crate::provider_server::Config::decode_applied(&bytes).unwrap(),
        )
        .unwrap();
        let row = restored
            .views(&accounts)
            .into_iter()
            .find(|p| p.id == profile)
            .unwrap();
        assert_eq!(row.allocations[0].checked_unix_ms, Some(1234));
        let mut changed = current;
        changed.generation = "f".repeat(32);
        let changed = ConnectionAccounts::from([(changed.summary.id.clone(), Ok(changed))]);
        assert!(matches!(
            book.prepare_apply(&profile, revision, &config, &changed),
            Err(SelectionError::AuthorityChanged)
        ));
        assert_eq!(book.revision(&profile), Some(revision));
    }

    #[test]
    fn cs04_apply_plan_cannot_launder_legacy_into_a_new_alias_or_selected_wire() {
        let config = cs02_applied_fixture();
        let mut book = DraftBook::from_applied(&config).unwrap();
        let profile = config.profiles[0].id.clone();
        let view = book
            .views(&ConnectionAccounts::new())
            .into_iter()
            .find(|p| p.id == profile)
            .unwrap();
        let mut input = view.input;
        input.allocations[0].alias = "new-alias".into();
        input.default_alias = "new-alias".into();
        let revision = book.edit_input(&profile, view.revision, input).unwrap();
        let current = account('a', None);
        let accounts =
            ConnectionAccounts::from([(current.summary.id.clone(), Ok(current.clone()))]);
        assert!(matches!(
            book.prepare_apply(&profile, revision, &config, &accounts),
            Err(SelectionError::VerificationRequired)
        ));
        let route = config.models[0].clone();
        let revision = book
            .choose(
                &profile,
                revision,
                0,
                selection(&current, route.clone()),
                &route.id,
            )
            .unwrap();
        assert!(matches!(
            book.prepare_apply(&profile, revision, &config, &accounts),
            Err(SelectionError::VerificationRequired)
        ));
        assert_eq!(
            config.resolve_allocation(&profile, None).unwrap().alias,
            "model"
        );
    }
    #[test]
    fn cs03_mixed_raw_applied_and_selected_rows_freeze_their_actual_independent_wires() {
        let config = cs02_applied_fixture();
        let account = account('a', None);
        let mut book = DraftBook::from_applied(&config).unwrap();
        let profile = config.profiles[0].id.clone();
        let first = book
            .views(&ConnectionAccounts::new())
            .into_iter()
            .find(|row| row.id == profile)
            .unwrap();
        let mut input = first.input;
        input.allocations.push(ConnectionAllocationInput {
            slot: 1,
            alias: "new-model".into(),
            account: Some(account.summary.id.clone()),
            model: "new-catalogue".into(),
        });
        let revision = book.edit_input(&profile, first.revision, input).unwrap();
        let fresh_route = route("new-catalogue", "a-new-scope/v1");
        let revision = book
            .choose(
                &profile,
                revision,
                1,
                selection(&account, fresh_route.clone()),
                "new-catalogue",
            )
            .unwrap();
        let (_, applied) = book.freeze_probe(&profile, revision, 0, &account).unwrap();
        let (_, selected) = book.freeze_probe(&profile, revision, 1, &account).unwrap();
        assert_eq!(applied.route(), &config.models[0]);
        assert_eq!(selected.route(), &fresh_route);
        assert_ne!(applied.identity(), selected.identity());
        assert!(
            book.views(&ConnectionAccounts::from([(
                account.summary.id.clone(),
                Ok(account.clone())
            )]))
            .iter()
            .all(|profile| profile
                .allocations
                .iter()
                .all(|row| row.checked_unix_ms.is_none()))
        );
        let mut disabled = account.clone();
        disabled.preferences.enabled = false;
        assert!(matches!(
            book.freeze_probe(&profile, revision, 0, &disabled),
            Err(SelectionError::Disabled)
        ));
        assert!(matches!(
            book.freeze_probe(&profile, revision - 1, 1, &account),
            Err(SelectionError::DraftChanged)
        ));
        let mut invalid = book
            .views(&ConnectionAccounts::new())
            .into_iter()
            .find(|row| row.id == profile)
            .unwrap()
            .input;
        invalid.allocations[1].alias = invalid.allocations[0].alias.clone();
        let revision = book.edit_input(&profile, revision, invalid).unwrap();
        assert!(matches!(
            book.freeze_probe(&profile, revision, 0, &account),
            Err(SelectionError::InvalidDraft)
        ));
    }

    #[test]
    fn cs03_permit_cannot_be_readmitted_or_revived_after_a_real_candidate_is_cancelled() {
        let account = account('a', None);
        let config = cs02_applied_fixture();
        let book = DraftBook::from_applied(&config).unwrap();
        let revision = book.revision(DEFAULT_PROFILE).unwrap();
        let (_, candidate) = book
            .freeze_probe(DEFAULT_PROFILE, revision, 0, &account)
            .unwrap();
        assert!(candidate.matches_current(&account));
        let permit = ProbePermit::new();
        assert!(permit.try_admit());
        assert!(!permit.try_admit());
        permit.invalidate();
        assert!(!permit.is_admitted());
        assert!(!permit.try_admit());
        let mut changed = account.clone();
        changed.organization.epoch = "e".repeat(32);
        assert!(!candidate.matches_current(&changed));
    }

    use super::*;
    use crate::provider_accounts::{AccountPreferences, AccountState, AccountSummary};

    fn cs02_applied_fixture() -> crate::provider_server::Config {
        use crate::provider_server::{ClientAllocation, ClientProfile, Config};
        let current = account('a', None);
        let row = route("catalogue-id", "fixture/v1");
        let mut config = Config::fresh().unwrap();
        config.version = 2;
        config.selected_account = Some(current.summary.id.clone());
        config.model = row.id.clone();
        config.models = vec![row];
        for index in 1..8 {
            config.profiles.push(ClientProfile {
                id: format!("{index:032x}"),
                label: format!("Client {index}"),
                enabled: index != 7,
                default_model: "model".into(),
                allocations: vec![ClientAllocation {
                    alias: "model".into(),
                    account: current.summary.id.clone(),
                    route: config.model.clone(),
                }],
            });
        }
        config.upgrade_applied().unwrap()
    }

    #[test]
    fn cs02_production_import_keeps_225_large_applied_references_and_account_proofs_independent() {
        use crate::provider_server::{AppliedOrigin, ClientAllocation, MODEL_ALIAS};
        let a = account('a', None);
        let b = account('b', None);
        let mut config = cs02_applied_fixture();
        config.version = 2;
        config.applied_evidence.clear();
        let mut large = route("shared-registry", &"x".repeat(8000));
        large.model_id = "m".repeat(200);
        config.model = large.id.clone();
        config.models = vec![large.clone()];
        for profile in &mut config.profiles {
            profile.allocations = (0..32)
                .map(|index| ClientAllocation {
                    alias: format!("m{index:02}{}", "x".repeat(197)),
                    account: if index % 2 == 0 {
                        a.summary.id.clone()
                    } else {
                        b.summary.id.clone()
                    },
                    route: large.id.clone(),
                })
                .collect();
            profile.default_model = profile.allocations[0].alias.clone();
        }
        config = config.upgrade_applied().unwrap();
        let target = config.profiles[0].clone();
        let mut checked = DraftBook::default();
        let mut input = edit(selection(&a, large.clone()));
        input.default_alias = target.allocations[0].alias.clone();
        input.allocations = vec![
            AllocationDraft {
                alias: target.allocations[0].alias.clone(),
                selected: selection(&a, large.clone()),
            },
            AllocationDraft {
                alias: target.allocations[1].alias.clone(),
                selected: selection(&b, large.clone()),
            },
        ];
        checked.replace(&target.id, input).unwrap();
        for (index, current) in [&a, &b].into_iter().enumerate() {
            let alias = &target.allocations[index].alias;
            let ticket = checked.begin_probe(&target.id, alias, current).unwrap();
            checked
                .accept_probe(ticket, current, if index == 0 { 111 } else { 222 })
                .unwrap();
            let proof = checked
                .verification(&target.id, alias, current)
                .unwrap()
                .unwrap()
                .persisted();
            config
                .applied_evidence
                .iter_mut()
                .find(|entry| entry.profile == target.id && entry.alias == *alias)
                .unwrap()
                .origin = AppliedOrigin::Inference {
                proof: Box::new(proof),
            };
        }
        let encoded = config.encode_applied().unwrap();
        assert!(encoded.len() <= crate::provider_server::CONFIG_BYTES);
        let book = DraftBook::from_applied(&config)
            .expect("legal Config must retain every applied reference");
        let mut accounts = ConnectionAccounts::from([
            (a.summary.id.clone(), Ok(a.clone())),
            (b.summary.id.clone(), Ok(b.clone())),
        ]);
        let views = book.views(&accounts);
        assert_eq!(views.len(), 8);
        assert_eq!(
            views
                .iter()
                .map(|profile| profile.allocations.len())
                .sum::<usize>(),
            225
        );
        assert!(views.iter().all(|profile| profile.allocations.len()
            == if profile.id == DEFAULT_PROFILE { 1 } else { 32 }));
        assert!(
            views
                .iter()
                .all(|profile| profile
                    .allocations
                    .iter()
                    .all(
                        |row| row.upstream_model.as_deref() == Some(large.model_id.as_str())
                            && row.availability == ConnectionAvailability::Available
                    ))
        );
        let selected = views
            .iter()
            .find(|profile| profile.id == target.id)
            .unwrap();
        assert_eq!(
            selected.input.allocations[0].account.as_deref(),
            Some(a.summary.id.as_str())
        );
        assert_eq!(
            selected.input.allocations[1].account.as_deref(),
            Some(b.summary.id.as_str())
        );
        assert_eq!(selected.allocations[0].checked_unix_ms, Some(111));
        assert_eq!(selected.allocations[1].checked_unix_ms, Some(222));
        assert_eq!(
            selected.allocations[0].evidence,
            ConnectionEvidence::Verified
        );
        assert_eq!(
            selected.allocations[1].evidence,
            ConnectionEvidence::Verified
        );
        let mut changed_b = b.clone();
        changed_b.generation = "f".repeat(32);
        accounts.insert(b.summary.id.clone(), Ok(changed_b));
        let changed = book.views(&accounts);
        let selected = changed
            .iter()
            .find(|profile| profile.id == target.id)
            .unwrap();
        assert_eq!(
            selected.allocations[0].evidence,
            ConnectionEvidence::Verified
        );
        assert_eq!(
            selected.allocations[1].evidence,
            ConnectionEvidence::StaleAuthority
        );
        assert_eq!(
            changed
                .iter()
                .find(|profile| profile.id == DEFAULT_PROFILE)
                .unwrap()
                .input
                .default_alias,
            MODEL_ALIAS
        );
        assert_eq!(config.encode_applied().unwrap(), encoded);
        assert!(views.iter().all(|profile| !profile.dirty));
    }

    #[test]
    fn cs02_production_book_retains_all_eight_applied_profiles_without_account_authority() {
        let config = cs02_applied_fixture();
        let mut book = DraftBook::from_applied(&config).unwrap();
        let missing = ConnectionAccounts::new();
        let initial = book.views(&missing);
        assert_eq!(initial.len(), 8);
        assert!(
            initial
                .iter()
                .all(|view| view.allocations.len() == 1 && !view.dirty)
        );
        assert!(
            initial
                .iter()
                .all(|view| view.allocations[0].availability
                    == ConnectionAvailability::MissingAccount)
        );
        assert!(initial.iter().all(|view| view.allocations[0].evidence
            == ConnectionEvidence::LegacyApplied
            && view.allocations[0].checked_unix_ms.is_none()));
        assert_eq!(book.referenced_accounts(), vec!["a".repeat(32)]);
        let first = format!("{:032x}", 1);
        let second = format!("{:032x}", 2);
        let mut input = book.form(&first).unwrap().input.clone();
        input.label = "Retained edit".into();
        book.edit_input(&first, book.revision(&first).unwrap(), input)
            .unwrap();
        let mut input = book.form(&second).unwrap().input.clone();
        input.label.clear();
        book.edit_input(&second, book.revision(&second).unwrap(), input.clone())
            .unwrap();
        assert_eq!(
            book.form(&second).unwrap().error,
            Some(ConnectionInputError::InvalidLabel)
        );
        let before = book.revision(&first).unwrap();
        book.revert(&first, before, &config).unwrap();
        assert!(book.revision(&first).unwrap() > before);
        assert_eq!(book.form(&second).unwrap().input, input);
        assert_eq!(
            book.form(&first).unwrap().input,
            initial.iter().find(|view| view.id == first).unwrap().input
        );
        assert_eq!(
            book.create_profile(&"f".repeat(32)),
            Err(SelectionError::ProfileBound)
        );
    }

    #[test]
    fn cs02_production_book_restores_only_persisted_exact_evidence_without_a_new_check() {
        use crate::provider_server::{AppliedOrigin, MODEL_ALIAS};
        let current = account('a', None);
        let mut config = cs02_applied_fixture();
        let mut checked = DraftBook::default();
        let mut input = edit(selection(&current, config.models[0].clone()));
        input.default_alias = MODEL_ALIAS.into();
        input.allocations[0].alias = MODEL_ALIAS.into();
        checked.replace(DEFAULT_PROFILE, input).unwrap();
        let ticket = checked
            .begin_probe(DEFAULT_PROFILE, MODEL_ALIAS, &current)
            .unwrap();
        checked.accept_probe(ticket, &current, 1234).unwrap();
        let proof = checked
            .verification(DEFAULT_PROFILE, MODEL_ALIAS, &current)
            .unwrap()
            .unwrap()
            .persisted();
        let record = serde_json::to_vec(&proof).unwrap();
        config
            .applied_evidence
            .iter_mut()
            .find(|entry| entry.profile == DEFAULT_PROFILE)
            .unwrap()
            .origin = AppliedOrigin::Inference {
            proof: Box::new(proof),
        };
        let mut book = DraftBook::from_applied(&config).unwrap();
        let mut accounts =
            ConnectionAccounts::from([(current.summary.id.clone(), Ok(current.clone()))]);
        let default = |views: Vec<ConnectionProfileView>| {
            views
                .into_iter()
                .find(|view| view.id == DEFAULT_PROFILE)
                .unwrap()
        };
        let restored = default(book.views(&accounts));
        assert_eq!(
            restored.allocations[0].evidence,
            ConnectionEvidence::Verified
        );
        assert_eq!(restored.allocations[0].checked_unix_ms, Some(1234));
        let imported = book
            .get(DEFAULT_PROFILE)
            .unwrap()
            .proofs
            .get(MODEL_ALIAS)
            .unwrap()
            .persisted();
        assert_eq!(serde_json::to_vec(&imported).unwrap(), record);
        let mut changed = current.clone();
        changed.generation = "e".repeat(32);
        accounts.insert(changed.summary.id.clone(), Ok(changed));
        let stale = default(book.views(&accounts));
        assert_eq!(
            stale.allocations[0].evidence,
            ConnectionEvidence::StaleAuthority
        );
        assert_eq!(
            stale.allocations[0].availability,
            ConnectionAvailability::AuthorityChanged
        );
        assert_eq!(stale.allocations[0].checked_unix_ms, Some(1234));
        let revision = book.revision(DEFAULT_PROFILE).unwrap();
        book.revert(DEFAULT_PROFILE, revision, &config).unwrap();
        assert_eq!(
            default(book.views(&accounts)).allocations[0].evidence,
            ConnectionEvidence::StaleAuthority
        );
        let other = format!("{:032x}", 1);
        let mut forged = config.clone();
        let copied = forged
            .applied_evidence
            .iter()
            .find(|entry| entry.profile == DEFAULT_PROFILE)
            .unwrap()
            .origin
            .clone();
        forged
            .applied_evidence
            .iter_mut()
            .find(|entry| entry.profile == other)
            .unwrap()
            .origin = copied;
        assert!(matches!(
            DraftBook::from_applied(&forged),
            Err(SelectionError::InvalidDraft)
        ));
    }

    #[test]
    fn cs02_production_raw_edits_use_slot_identity_and_cas_without_resurrecting_late_proofs() {
        let current = account('a', None);
        let mut book = DraftBook::from_applied(&cs02_applied_fixture()).unwrap();
        let profile = format!("{:032x}", 1);
        let row = route("catalogue-choice", "different/v1");
        let mut cache = CatalogueCache::default();
        load(&mut cache, &current, vec![row]);
        let rev = book.revision(&profile).unwrap();
        book.choose(
            &profile,
            rev,
            0,
            cache.select(&current, "catalogue-choice").unwrap(),
            "catalogue-choice",
        )
        .unwrap();
        let ticket = book.begin_probe(&profile, "model", &current).unwrap();
        let mut invalid = book.form(&profile).unwrap().input.clone();
        invalid.label.clear();
        let revision = book.revision(&profile).unwrap();
        let accepted = book
            .edit_input(&profile, revision, invalid.clone())
            .unwrap();
        assert!(accepted > revision);
        assert_eq!(book.form(&profile).unwrap().input, invalid);
        assert_eq!(
            book.form(&profile).unwrap().error,
            Some(ConnectionInputError::InvalidLabel)
        );
        assert!(matches!(
            book.begin_probe(&profile, "model", &current),
            Err(SelectionError::DraftChanged)
        ));
        assert_eq!(
            book.accept_probe(ticket, &current, 5000),
            Err(SelectionError::DraftChanged)
        );
        invalid.label = "Later".into();
        assert_eq!(
            book.edit_input(&profile, revision, invalid.clone()),
            Err(SelectionError::DraftChanged)
        );
        assert!(book.form(&profile).unwrap().input.label.is_empty());
        invalid.allocations[0].model = "x".repeat(201);
        assert_eq!(
            book.edit_input(&profile, accepted, invalid),
            Err(SelectionError::InvalidDraft)
        );
        assert_eq!(book.revision(&profile), Some(accepted));
        let mut repaired = book.form(&profile).unwrap().input.clone();
        repaired.label = "Repaired".into();
        book.edit_input(&profile, accepted, repaired).unwrap();
        let ticket = book.begin_probe(&profile, "model", &current).unwrap();
        book.accept_probe(ticket, &current, 5001).unwrap();
        let mut moved = book.form(&profile).unwrap().input.clone();
        moved.allocations[0].slot = 3;
        book.edit_input(&profile, book.revision(&profile).unwrap(), moved)
            .unwrap();
        assert!(book.get(&profile).unwrap().proofs.is_empty());
        assert_eq!(
            book.form(&profile).unwrap().error,
            Some(ConnectionInputError::UnselectedModel)
        );
        let views = book.views(&ConnectionAccounts::from([(
            current.summary.id.clone(),
            Ok(current),
        )]));
        assert_eq!(
            views
                .iter()
                .find(|view| view.id == profile)
                .unwrap()
                .allocations[0]
                .evidence,
            ConnectionEvidence::Unverified
        );
    }

    #[test]
    fn cs02_production_revert_discards_candidate_only_proof_and_preserves_other_profiles() {
        let current = account('a', None);
        let config = cs02_applied_fixture();
        let mut book = DraftBook::from_applied(&config).unwrap();
        let profile = format!("{:032x}", 1);
        let mut cache = CatalogueCache::default();
        load(&mut cache, &current, vec![route("choice", "new/v1")]);
        book.choose(
            &profile,
            book.revision(&profile).unwrap(),
            0,
            cache.select(&current, "choice").unwrap(),
            "choice",
        )
        .unwrap();
        let ticket = book.begin_probe(&profile, "model", &current).unwrap();
        book.accept_probe(ticket, &current, 6000).unwrap();
        assert!(!book.get(&profile).unwrap().proofs.is_empty());
        book.revert(&profile, book.revision(&profile).unwrap(), &config)
            .unwrap();
        assert!(book.get(&profile).unwrap().proofs.is_empty());
        let views = book.views(&ConnectionAccounts::from([(
            current.summary.id.clone(),
            Ok(current),
        )]));
        assert!(views.iter().all(|view| !view.dirty));
        assert!(
            views
                .iter()
                .all(|view| view.allocations[0].evidence == ConnectionEvidence::LegacyApplied)
        );
        let mut default = book.form(DEFAULT_PROFILE).unwrap().input.clone();
        default.allocations[0].alias = "new-alias".into();
        book.edit_input(
            DEFAULT_PROFILE,
            book.revision(DEFAULT_PROFILE).unwrap(),
            default.clone(),
        )
        .unwrap();
        assert_eq!(
            book.form(DEFAULT_PROFILE).unwrap().error,
            Some(ConnectionInputError::DefaultCompatibility)
        );
        assert_eq!(book.form(DEFAULT_PROFILE).unwrap().input, default);
    }

    #[test]
    fn cs02_production_raw_duplicate_alias_and_budget_rejections_keep_last_state() {
        let mut config = cs02_applied_fixture();
        config.profiles.pop();
        config
            .applied_evidence
            .retain(|entry| entry.profile != format!("{:032x}", 7));
        let mut book = DraftBook::from_applied(&config).unwrap();
        let profile = "f".repeat(32);
        let first = book.create_profile(&profile).unwrap();
        let mut input = book.form(&profile).unwrap().input.clone();
        input.default_alias = "duplicate".into();
        input.allocations = (0..2)
            .map(|slot| ConnectionAllocationInput {
                slot,
                alias: "duplicate".into(),
                account: None,
                model: String::new(),
            })
            .collect();
        let revision = book.edit_input(&profile, first, input.clone()).unwrap();
        // Alias duplicate is detected before incomplete model state.
        assert_eq!(book.form(&profile).unwrap().input, input);
        assert_eq!(
            book.form(&profile).unwrap().error,
            Some(ConnectionInputError::DuplicateAlias)
        );
        input.allocations[1].slot = 0;
        assert_eq!(
            book.edit_input(&profile, revision, input),
            Err(SelectionError::InvalidDraft)
        );
        assert_eq!(book.revision(&profile), Some(revision));
        // Explicit pure accounting fault, not evidence of process RSS or a
        // naturally reached 4 MiB form. The actual rejection is exercised.
        book.bytes = MAX_DRAFT_BYTES;
        let mut larger = book.form(&profile).unwrap().input.clone();
        larger.label = "x".repeat(128);
        assert_eq!(
            book.edit_input(&profile, revision, larger),
            Err(SelectionError::AggregateBound)
        );
        assert_eq!(book.revision(&profile), Some(revision));
        book.discard(&profile, revision).unwrap();
        assert_eq!(book.selected(), DEFAULT_PROFILE);
        assert!(book.revision(&profile).is_none());
    }

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
