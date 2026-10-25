//! Pure account-auth messages and attempt lifetime for PRD 02.31.01.
//!
//! No socket, browser or credential store is owned here. Anonymous initialization
//! established both flows with the MiniCon identity; it did not establish any
//! account grant or inference readiness. Check attempts before dispatch/commit;
//! never
//! log `NetworkHttpRequest` (its platform Debug includes headers and body).

use minicon_core::json::{JsonValue, object, to_vec};
use minicon_platform::network_http::{
    self, NetworkHttpMethod, NetworkHttpRequest, NetworkHttpResponse,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::time::{Duration, Instant};

pub const AUTH_MAX_RESPONSE_BYTES: usize = 64 * 1024;
// Interactive login includes browser sign-in and confirmation. Keep HTTP
// exchanges short, but allow the official client's ten-minute login window.
// A shorter provider expiry still wins in Attempt::limit_by_provider.
pub const AUTH_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(600);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_FIELD_BYTES: usize = 4096;
const DEEPSEEK_ORIGIN: &str = "https://platform.deepseek.com";
const OPENCODE_ORIGIN: &str = "https://opencode.ai";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    DeepSeek,
    OpenCode,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ClientQualification {
    #[default]
    ResearchOnly,
    FlowAvailable,
}

/// Availability describes a compatible protocol, never account readiness.
#[derive(Debug)]
pub struct AuthClient {
    provider: Provider,
    qualification: ClientQualification,
    client_id: Option<String>,
}

impl AuthClient {
    #[cfg(test)]
    pub fn research_only(provider: Provider) -> Self {
        Self {
            provider,
            qualification: ClientQualification::ResearchOnly,
            client_id: None,
        }
    }

    pub fn available(provider: Provider) -> Self {
        Self {
            provider,
            qualification: ClientQualification::FlowAvailable,
            client_id: Some("minicon".to_owned()),
        }
    }

    /// Public client IDs are identifiers, not secrets or registration evidence.
    /// Production selects `available`, which identifies MiniCon as itself.
    #[cfg(test)]
    pub fn compatible_client(provider: Provider, client_id: &str) -> Result<Self, AuthError> {
        if !identifier(client_id, 128) {
            return Err(AuthError::InvalidInput);
        }
        Ok(Self {
            provider,
            qualification: ClientQualification::FlowAvailable,
            client_id: Some(client_id.to_owned()),
        })
    }

    #[cfg(test)]
    pub fn qualification(&self) -> ClientQualification {
        self.qualification
    }

    fn admit(&self, provider: Provider) -> Result<&str, AuthError> {
        if self.provider != provider || self.qualification != ClientQualification::FlowAvailable {
            return Err(AuthError::UnverifiedClient);
        }
        self.client_id.as_deref().ok_or(AuthError::UnverifiedClient)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthError {
    UnverifiedClient,
    InvalidInput,
    InvalidResponse,
    ResponseTooLarge,
    HttpStatus(u16),
    AuthorizationRejected,
    NeedsLogin,
    Expired,
    Cancelled,
    StaleAttempt,
    InvalidState,
    EntropyUnavailable,
    TransportUnavailable,
    TransportFailure(network_http::NetworkHttpErrorKind),
}

impl fmt::Display for AuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No provider body, URL, authorization code or token is interpolated.
        match self {
            Self::HttpStatus(status) => write!(formatter, "provider auth: HTTP {status}"),
            other => write!(formatter, "provider auth: {other:?}"),
        }
    }
}

impl std::error::Error for AuthError {}

/// Explicit access is necessary for transport/storage; Debug always redacts.
#[derive(Eq, PartialEq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: &str) -> Result<Self, AuthError> {
        if !opaque(value) {
            return Err(AuthError::InvalidInput);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

#[derive(Debug)]
pub enum TokenLifecycle {
    OpaqueUntilRejected,
    ExpiringRefresh {
        refresh_token: Secret,
        expires_at: Instant,
    },
}

#[derive(Debug)]
pub struct TokenGrant {
    pub provider: Provider,
    pub access_token: Secret,
    pub lifecycle: TokenLifecycle,
    pub organization: Option<String>,
}

#[derive(Debug)]
pub struct Pkce {
    pub state: Secret,
    verifier: Secret,
}

impl Pkce {
    pub fn generate() -> Result<Self, AuthError> {
        let entropy = minicon_platform::entropy::secure_random_array::<64>()
            .map_err(|_| AuthError::EntropyUnavailable)?;
        Self::from_parts(&base64url(&entropy[..32]), &base64url(&entropy[32..]))
    }

    /// A deterministic seam for synthetic courts. Production uses `generate`.
    fn from_parts(verifier: &str, state: &str) -> Result<Self, AuthError> {
        if !(43..=128).contains(&verifier.len())
            || !verifier.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
            })
            || !identifier(state, 128)
            || state.len() < 32
        {
            return Err(AuthError::InvalidInput);
        }
        Ok(Self {
            state: Secret::new(state)?,
            verifier: Secret::new(verifier)?,
        })
    }

    pub fn challenge(&self) -> String {
        base64url(&Sha256::digest(self.verifier.expose().as_bytes()))
    }
}

fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::new();
    let mut bits = 0_u32;
    let mut remaining = 0_u32;
    for byte in bytes {
        bits = (bits << 8) | u32::from(*byte);
        remaining += 8;
        while remaining >= 6 {
            remaining -= 6;
            output.push(char::from(ALPHABET[((bits >> remaining) & 63) as usize]));
        }
    }
    if remaining != 0 {
        output.push(char::from(
            ALPHABET[((bits << (6 - remaining)) & 63) as usize],
        ));
    }
    output
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AttemptPhase {
    Waiting,
    Exchanging,
    Committed,
    #[cfg(test)]
    Cancelled,
}

/// The account owner increments its generation on removal/replacement. Every
/// outbound action checks this object; publication calls `commit` immediately
/// before its synchronous state change. A store spanning await points must also
/// compare the generation atomically at publication.
#[derive(Debug)]
pub struct Attempt {
    generation: u64,
    deadline: Instant,
    phase: AttemptPhase,
}

impl Attempt {
    pub fn new(generation: u64, now: Instant) -> Result<Self, AuthError> {
        Ok(Self {
            generation,
            deadline: now
                .checked_add(AUTH_ATTEMPT_TIMEOUT)
                .ok_or(AuthError::InvalidInput)?,
            phase: AttemptPhase::Waiting,
        })
    }

    pub fn check(&self, generation: u64, now: Instant) -> Result<Duration, AuthError> {
        if self.generation != generation {
            return Err(AuthError::StaleAttempt);
        }
        match self.phase {
            #[cfg(test)]
            AttemptPhase::Cancelled => return Err(AuthError::Cancelled),
            AttemptPhase::Committed => return Err(AuthError::InvalidState),
            AttemptPhase::Waiting | AttemptPhase::Exchanging => {}
        }
        let remaining = self.deadline.saturating_duration_since(now);
        if remaining.is_zero() {
            return Err(AuthError::Expired);
        }
        Ok(remaining)
    }

    pub fn limit_by_provider(
        &mut self,
        generation: u64,
        now: Instant,
        expires_in: Duration,
    ) -> Result<(), AuthError> {
        self.check(generation, now)?;
        if expires_in.is_zero() {
            return Err(AuthError::Expired);
        }
        let deadline = now
            .checked_add(expires_in)
            .ok_or(AuthError::InvalidResponse)?;
        self.deadline = self.deadline.min(deadline);
        Ok(())
    }

    /// Call only after the callback decoder has rejected duplicate state/code
    /// query parameters. The returned code can be exchanged only once locally.
    pub fn accept_callback(
        &mut self,
        generation: u64,
        now: Instant,
        pkce: &Pkce,
        received_state: &str,
        code: &str,
    ) -> Result<Secret, AuthError> {
        self.check(generation, now)?;
        if self.phase != AttemptPhase::Waiting || !same_secret(pkce.state.expose(), received_state)
        {
            return Err(AuthError::InvalidState);
        }
        let code = Secret::new(code)?;
        self.phase = AttemptPhase::Exchanging;
        Ok(code)
    }

    pub fn commit(&mut self, generation: u64, now: Instant) -> Result<(), AuthError> {
        self.check(generation, now)?;
        self.phase = AttemptPhase::Committed;
        Ok(())
    }

    #[cfg(test)]
    pub fn cancel(&mut self) {
        if self.phase != AttemptPhase::Committed {
            self.phase = AttemptPhase::Cancelled;
        }
    }
}

fn same_secret(expected: &str, actual: &str) -> bool {
    // State length is public and fixed by our generator; compare equal-length
    // values without stopping at the first differing byte.
    expected.len() == actual.len()
        && expected
            .bytes()
            .zip(actual.bytes())
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0
}

pub struct DeepSeekMetadata<'a> {
    pub version: &'a str,
    pub platform: &'a str,
    pub locale: &'a str,
    pub timezone_offset_seconds: i32,
}

pub fn deepseek_initialize(
    client: &AuthClient,
    pkce: &Pkce,
    callback_port: u16,
    metadata: &DeepSeekMetadata<'_>,
) -> Result<NetworkHttpRequest, AuthError> {
    let body = object(vec![
        ("code_challenge", pkce.challenge().into()),
        ("code_challenge_method", "S256".into()),
        ("state", pkce.state.expose().into()),
        ("redirect_uri", redirect_uri(callback_port)?.into()),
        ("locale", metadata.locale.into()),
        ("login_source", "desktop".into()),
    ]);
    deepseek_request(client, "/auth-api/v0/dsh/auth_init", body, metadata)
}

pub struct DeepSeekExchange<'a> {
    pub code: &'a Secret,
    pub pkce: &'a Pkce,
    pub callback_port: u16,
    pub device_id: &'a str,
    pub device_model: &'a str,
    pub os_version: &'a str,
}

pub fn deepseek_exchange(
    client: &AuthClient,
    input: &DeepSeekExchange<'_>,
    metadata: &DeepSeekMetadata<'_>,
) -> Result<NetworkHttpRequest, AuthError> {
    for value in [input.device_id, input.device_model, input.os_version] {
        if !text(value, 256) {
            return Err(AuthError::InvalidInput);
        }
    }
    deepseek_request(
        client,
        "/auth-api/v0/dsh/auth_exchange",
        object(vec![
            ("code", input.code.expose().into()),
            ("code_verifier", input.pkce.verifier.expose().into()),
            ("redirect_uri", redirect_uri(input.callback_port)?.into()),
            ("device_id", input.device_id.into()),
            ("device_model", input.device_model.into()),
            ("os_version", input.os_version.into()),
        ]),
        metadata,
    )
}

pub fn deepseek_cancel(
    client: &AuthClient,
    authorize_id: &Secret,
    pkce: &Pkce,
    metadata: &DeepSeekMetadata<'_>,
) -> Result<NetworkHttpRequest, AuthError> {
    deepseek_request(
        client,
        "/auth-api/v0/dsh/auth_cancel",
        object(vec![
            ("authorize_id", authorize_id.expose().into()),
            ("code_verifier", pkce.verifier.expose().into()),
        ]),
        metadata,
    )
}

pub fn deepseek_logout(
    client: &AuthClient,
    grant: &TokenGrant,
    metadata: &DeepSeekMetadata<'_>,
) -> Result<NetworkHttpRequest, AuthError> {
    if grant.provider != Provider::DeepSeek
        || !matches!(grant.lifecycle, TokenLifecycle::OpaqueUntilRejected)
    {
        return Err(AuthError::InvalidInput);
    }
    Ok(deepseek_request(
        client,
        "/auth-api/v0/users/logout",
        object(vec![]),
        metadata,
    )?
    .header("x-dsh-auth-token", grant.access_token.expose()))
}

fn deepseek_request(
    client: &AuthClient,
    path: &'static str,
    body: JsonValue,
    metadata: &DeepSeekMetadata<'_>,
) -> Result<NetworkHttpRequest, AuthError> {
    deepseek_client_headers(
        client,
        json_request(&format!("{DEEPSEEK_ORIGIN}{path}"), body)?,
        metadata,
    )
}

/// Official Platform GET with the same grant/client metadata as desktop auth.
pub(crate) fn deepseek_summary_request(
    client: &AuthClient,
    grant: &TokenGrant,
    metadata: &DeepSeekMetadata<'_>,
) -> Result<NetworkHttpRequest, AuthError> {
    if grant.provider != Provider::DeepSeek
        || !matches!(grant.lifecycle, TokenLifecycle::OpaqueUntilRejected)
    {
        return Err(AuthError::InvalidInput);
    }
    deepseek_client_headers(
        client,
        NetworkHttpRequest::new(
            NetworkHttpMethod::Get,
            "https://platform.deepseek.com/api/v0/users/get_user_summary",
        )
        .header("Accept", "application/json")
        .header("x-dsh-auth-token", grant.access_token.expose()),
        metadata,
    )
}
fn deepseek_client_headers(
    client: &AuthClient,
    request: NetworkHttpRequest,
    metadata: &DeepSeekMetadata<'_>,
) -> Result<NetworkHttpRequest, AuthError> {
    let client_id = client.admit(Provider::DeepSeek)?;
    if !identifier(metadata.version, 128)
        || !matches!(
            metadata.platform,
            "desktop-win" | "desktop-mac" | "desktop-linux"
        )
        || !matches!(metadata.locale, "zh_CN" | "en_US")
        || metadata.timezone_offset_seconds.unsigned_abs() > 86_400
    {
        return Err(AuthError::InvalidInput);
    }
    let request = request
        .header("x-client-bundle-id", client_id)
        .header("x-client-platform", metadata.platform)
        .header("x-client-version", metadata.version)
        .header("x-client-locale", metadata.locale)
        .header(
            "x-client-timezone-offset",
            metadata.timezone_offset_seconds.to_string(),
        );
    network_http::validate(&request).map_err(|_| AuthError::InvalidInput)?;
    Ok(request)
}

fn redirect_uri(port: u16) -> Result<String, AuthError> {
    if port == 0 {
        return Err(AuthError::InvalidInput);
    }
    Ok(format!("http://127.0.0.1:{port}/oauth/callback"))
}

pub fn opencode_initialize(client: &AuthClient) -> Result<NetworkHttpRequest, AuthError> {
    let client_id = client.admit(Provider::OpenCode)?;
    json_request(
        "https://opencode.ai/console/auth/device/code",
        object(vec![
            ("client_id", client_id.into()),
            ("supports_org_scope", true.into()),
        ]),
    )
}

pub fn opencode_poll(
    client: &AuthClient,
    device_code: &Secret,
) -> Result<NetworkHttpRequest, AuthError> {
    let client_id = client.admit(Provider::OpenCode)?;
    json_request(
        "https://opencode.ai/console/auth/device/token",
        object(vec![
            (
                "grant_type",
                "urn:ietf:params:oauth:grant-type:device_code".into(),
            ),
            ("device_code", device_code.expose().into()),
            ("client_id", client_id.into()),
        ]),
    )
}

pub fn opencode_refresh(
    client: &AuthClient,
    grant: &TokenGrant,
) -> Result<NetworkHttpRequest, AuthError> {
    let client_id = client.admit(Provider::OpenCode)?;
    let TokenLifecycle::ExpiringRefresh { refresh_token, .. } = &grant.lifecycle else {
        return Err(AuthError::InvalidInput);
    };
    if grant.provider != Provider::OpenCode {
        return Err(AuthError::InvalidInput);
    }
    json_request(
        "https://opencode.ai/console/auth/device/token",
        object(vec![
            ("grant_type", "refresh_token".into()),
            ("refresh_token", refresh_token.expose().into()),
            ("client_id", client_id.into()),
        ]),
    )
}

fn json_request(url: &str, body: JsonValue) -> Result<NetworkHttpRequest, AuthError> {
    let request = NetworkHttpRequest::new(NetworkHttpMethod::Post, url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .body(to_vec(&body))
        .timeout(HTTP_TIMEOUT)
        .max_redirects(0)
        .max_response_bytes(AUTH_MAX_RESPONSE_BYTES);
    network_http::validate(&request).map_err(|_| AuthError::InvalidInput)?;
    Ok(request)
}

#[derive(Deserialize)]
struct DeepSeekEnvelope<T> {
    data: DeepSeekData<T>,
}

#[derive(Deserialize)]
struct DeepSeekData<T> {
    biz_data: T,
}

#[derive(Deserialize)]
struct DeepSeekStatus {
    code: i64,
    data: Option<DeepSeekBusinessStatus>,
}

#[derive(Deserialize)]
struct DeepSeekBusinessStatus {
    biz_code: i64,
}

#[derive(Deserialize)]
struct DeepSeekInitWire {
    authorize_url: String,
    authorize_id: String,
    expires_in: u64,
}

#[derive(Debug)]
pub struct DeepSeekChallenge {
    pub authorize_url: Secret,
    pub authorize_id: Secret,
    pub expires_in: Duration,
}

pub fn parse_deepseek_initialize(
    response: &NetworkHttpResponse,
) -> Result<DeepSeekChallenge, AuthError> {
    let wire: DeepSeekInitWire = deepseek_payload(response)?;
    Ok(DeepSeekChallenge {
        authorize_url: browser_url(&wire.authorize_url, DEEPSEEK_ORIGIN, Some("/dsh/authorize"))?,
        authorize_id: response_secret(&wire.authorize_id)?,
        expires_in: seconds(wire.expires_in)?,
    })
}

#[derive(Deserialize)]
struct DeepSeekGrantWire {
    token: String,
    authorized_url: String,
}

#[derive(Debug)]
pub struct DeepSeekGrant {
    pub grant: TokenGrant,
    pub completion_url: Secret,
}

pub fn parse_deepseek_exchange(response: &NetworkHttpResponse) -> Result<DeepSeekGrant, AuthError> {
    let wire: DeepSeekGrantWire = deepseek_payload(response)?;
    Ok(DeepSeekGrant {
        grant: TokenGrant {
            provider: Provider::DeepSeek,
            access_token: response_secret(&wire.token)?,
            lifecycle: TokenLifecycle::OpaqueUntilRejected,
            organization: None,
        },
        completion_url: browser_url(
            &wire.authorized_url,
            DEEPSEEK_ORIGIN,
            Some("/dsh/authorized"),
        )?,
    })
}

pub fn parse_deepseek_logout(response: &NetworkHttpResponse) -> Result<(), AuthError> {
    deepseek_payload::<serde::de::IgnoredAny>(response).map(|_| ())
}

fn deepseek_payload<T: serde::de::DeserializeOwned>(
    response: &NetworkHttpResponse,
) -> Result<T, AuthError> {
    // Error envelopes need not carry a success-shaped biz_data. Inspect only
    // status first, then deserialize the bounded success bytes into typed fields
    // (rather than a Value map that would overwrite duplicate token fields).
    let status: DeepSeekStatus = parse_json(response, false)?;
    if status.code == 40003 {
        return Err(AuthError::NeedsLogin);
    }
    if status.code != 0 {
        return Err(AuthError::AuthorizationRejected);
    }
    if status.data.ok_or(AuthError::InvalidResponse)?.biz_code != 0 {
        return Err(AuthError::AuthorizationRejected);
    }
    let wire: DeepSeekEnvelope<T> = parse_json(response, false)?;
    Ok(wire.data.biz_data)
}

#[derive(Deserialize)]
struct DeviceChallengeWire {
    device_code: String,
    user_code: String,
    verification_uri_complete: Option<String>,
    verification_uri: Option<String>,
    expires_in: u64,
    interval: u64,
}

#[derive(Debug)]
pub struct DeviceChallenge {
    pub device_code: Secret,
    pub user_code: Secret,
    pub verification_url: Secret,
    pub expires_in: Duration,
    pub poll_interval: Duration,
}

pub fn parse_opencode_initialize(
    response: &NetworkHttpResponse,
) -> Result<DeviceChallenge, AuthError> {
    let wire: DeviceChallengeWire = parse_json(response, false)?;
    let expires_in = seconds(wire.expires_in)?;
    let poll_interval = seconds(wire.interval)?;
    if poll_interval > expires_in {
        return Err(AuthError::InvalidResponse);
    }
    let verification = wire
        .verification_uri_complete
        .or(wire.verification_uri)
        .ok_or(AuthError::InvalidResponse)?;
    let verification = if verification.starts_with('/') && !verification.starts_with("//") {
        format!("{OPENCODE_ORIGIN}{verification}")
    } else {
        verification
    };
    Ok(DeviceChallenge {
        device_code: response_secret(&wire.device_code)?,
        user_code: response_secret(&wire.user_code)?,
        verification_url: browser_url(&verification, OPENCODE_ORIGIN, None)?,
        expires_in,
        poll_interval,
    })
}

#[derive(Deserialize)]
struct DeviceTokenWire {
    error: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    org_id: Option<String>,
}

#[derive(Debug)]
pub enum DevicePoll {
    Pending,
    /// Add five seconds to the previous interval, retaining the attempt deadline.
    SlowDown,
    Granted(TokenGrant),
}

pub fn parse_opencode_token(
    response: &NetworkHttpResponse,
    now: Instant,
) -> Result<DevicePoll, AuthError> {
    let wire: DeviceTokenWire = parse_json(response, true)?;
    if let Some(error) = wire.error {
        if wire.access_token.is_some()
            || wire.refresh_token.is_some()
            || wire.expires_in.is_some()
            || wire.org_id.is_some()
        {
            return Err(AuthError::InvalidResponse);
        }
        return match error.as_str() {
            "authorization_pending" => Ok(DevicePoll::Pending),
            "slow_down" => Ok(DevicePoll::SlowDown),
            "invalid_grant" => Err(AuthError::NeedsLogin),
            "expired_token" => Err(AuthError::Expired),
            _ => Err(AuthError::AuthorizationRejected),
        };
    }
    if !response.is_success() {
        return Err(AuthError::HttpStatus(response.status));
    }
    let expires = seconds(wire.expires_in.ok_or(AuthError::InvalidResponse)?)?;
    if wire
        .org_id
        .as_deref()
        .is_some_and(|id| !valid_organization_id(id))
    {
        return Err(AuthError::InvalidResponse);
    }
    Ok(DevicePoll::Granted(TokenGrant {
        provider: Provider::OpenCode,
        access_token: response_secret(
            wire.access_token
                .as_deref()
                .ok_or(AuthError::InvalidResponse)?,
        )?,
        lifecycle: TokenLifecycle::ExpiringRefresh {
            refresh_token: response_secret(
                wire.refresh_token
                    .as_deref()
                    .ok_or(AuthError::InvalidResponse)?,
            )?,
            expires_at: now.checked_add(expires).ok_or(AuthError::InvalidResponse)?,
        },
        organization: wire.org_id,
    }))
}

/// Refresh is not device polling: pending/slow_down never prolong a refresh.
pub fn parse_opencode_refresh(
    response: &NetworkHttpResponse,
    now: Instant,
) -> Result<TokenGrant, AuthError> {
    match parse_opencode_token(response, now)? {
        DevicePoll::Granted(grant) => Ok(grant),
        DevicePoll::Pending | DevicePoll::SlowDown => Err(AuthError::InvalidResponse),
    }
}

fn parse_json<T: serde::de::DeserializeOwned>(
    response: &NetworkHttpResponse,
    allow_device_error: bool,
) -> Result<T, AuthError> {
    if response.truncated || response.body.len() > AUTH_MAX_RESPONSE_BYTES {
        return Err(AuthError::ResponseTooLarge);
    }
    if response.status == 401 {
        return Err(AuthError::NeedsLogin);
    }
    if !response.is_success() && !(allow_device_error && response.status == 400) {
        return Err(AuthError::HttpStatus(response.status));
    }
    // Derived wire structs reject duplicate known fields. The serde default
    // nesting limit and the byte cap apply before any provider value is exposed.
    serde_json::from_slice(&response.body).map_err(|_| AuthError::InvalidResponse)
}

fn browser_url(value: &str, origin: &str, path: Option<&str>) -> Result<Secret, AuthError> {
    if !opaque(value) || value.contains(['\\', '#']) {
        return Err(AuthError::InvalidResponse);
    }
    let suffix = value
        .strip_prefix(origin)
        .ok_or(AuthError::InvalidResponse)?;
    if !suffix.starts_with('/') || suffix.starts_with("//") {
        return Err(AuthError::InvalidResponse);
    }
    if let Some(path) = path {
        let query = suffix
            .strip_prefix(path)
            .ok_or(AuthError::InvalidResponse)?;
        if !query.is_empty() && !query.starts_with('?') {
            return Err(AuthError::InvalidResponse);
        }
    }
    response_secret(value)
}

fn response_secret(value: &str) -> Result<Secret, AuthError> {
    Secret::new(value).map_err(|_| AuthError::InvalidResponse)
}

fn seconds(value: u64) -> Result<Duration, AuthError> {
    if value == 0 || value > 366 * 24 * 60 * 60 {
        return Err(AuthError::InvalidResponse);
    }
    Ok(Duration::from_secs(value))
}

fn opaque(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_FIELD_BYTES
        && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}

pub(crate) fn valid_organization_id(value: &str) -> bool {
    identifier(value, 256)
}

fn identifier(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn text(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(provider: Provider) -> AuthClient {
        AuthClient::compatible_client(provider, "minicon-synthetic-client").unwrap()
    }

    fn response(status: u16, body: &str) -> NetworkHttpResponse {
        NetworkHttpResponse {
            status,
            headers: vec![],
            body: body.as_bytes().to_vec(),
            truncated: false,
        }
    }

    fn pkce() -> Pkce {
        Pkce::from_parts(
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
            "synthetic-state-0123456789abcdefghijk",
        )
        .unwrap()
    }

    fn metadata() -> DeepSeekMetadata<'static> {
        DeepSeekMetadata {
            version: "0.2.23",
            platform: "desktop-win",
            locale: "en_US",
            timezone_offset_seconds: 28_800,
        }
    }

    #[test]
    fn pkce_generation_uses_fresh_native_entropy_and_redacts_its_state() {
        let first = Pkce::generate().unwrap();
        let second = Pkce::generate().unwrap();
        assert_eq!(first.state.expose().len(), 43);
        assert_eq!(first.challenge().len(), 43);
        assert_ne!(first.state, second.state);
        assert_ne!(first.challenge(), second.challenge());
        assert!(!format!("{first:?}").contains(first.state.expose()));
        assert!(!format!("{first:?}").contains(first.verifier.expose()));
    }

    #[test]
    fn research_is_unavailable_but_public_client_ids_are_not_registration_secrets() {
        let unverified = AuthClient::research_only(Provider::OpenCode);
        assert_eq!(
            unverified.qualification(),
            ClientQualification::ResearchOnly
        );
        assert_eq!(
            opencode_initialize(&unverified),
            Err(AuthError::UnverifiedClient)
        );
        assert!(AuthClient::compatible_client(Provider::OpenCode, "opencode-cli").is_ok());
        assert!(AuthClient::compatible_client(Provider::DeepSeek, "deepseek-harness").is_ok());
        assert!(AuthClient::compatible_client(Provider::OpenCode, "").is_err());
        let request = opencode_initialize(&AuthClient::available(Provider::OpenCode)).unwrap();
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["client_id"], "minicon");
        assert_eq!(
            deepseek_initialize(
                &AuthClient::research_only(Provider::DeepSeek),
                &pkce(),
                12345,
                &metadata()
            ),
            Err(AuthError::UnverifiedClient)
        );
        assert_eq!(
            opencode_initialize(&client(Provider::DeepSeek)),
            Err(AuthError::UnverifiedClient)
        );
    }

    #[test]
    fn deepseek_pkce_messages_use_bound_redirect_and_own_identity() {
        let proof = pkce();
        assert_eq!(
            proof.challenge(),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let client = client(Provider::DeepSeek);
        let request = deepseek_initialize(&client, &proof, 12345, &metadata()).unwrap();
        assert_eq!(
            request.url,
            "https://platform.deepseek.com/auth-api/v0/dsh/auth_init"
        );
        assert_eq!(request.max_redirects, 0);
        assert_eq!(request.max_response_bytes, AUTH_MAX_RESPONSE_BYTES);
        assert!(request.headers.contains(&(
            "x-client-bundle-id".into(),
            "minicon-synthetic-client".into()
        )));
        let init: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(
            init["redirect_uri"],
            "http://127.0.0.1:12345/oauth/callback"
        );
        assert_eq!(init["code_challenge_method"], "S256");
        assert_eq!(init["code_challenge"], proof.challenge());
        let code = Secret::new("synthetic-code").unwrap();
        let exchange = deepseek_exchange(
            &client,
            &DeepSeekExchange {
                code: &code,
                pkce: &proof,
                callback_port: 12345,
                device_id: "synthetic-device",
                device_model: "win32-x64",
                os_version: "Windows synthetic",
            },
            &metadata(),
        )
        .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&exchange.body).unwrap();
        assert_eq!(body["redirect_uri"], init["redirect_uri"]);
        assert_eq!(body["code_verifier"], proof.verifier.expose());
        assert_eq!(body["code"], code.expose());
        let cancel = deepseek_cancel(
            &client,
            &Secret::new("synthetic-attempt").unwrap(),
            &proof,
            &metadata(),
        )
        .unwrap();
        assert!(cancel.url.ends_with("/dsh/auth_cancel"));
        assert_eq!(
            deepseek_initialize(&client, &proof, 0, &metadata()),
            Err(AuthError::InvalidInput)
        );
    }

    #[test]
    fn attempt_rejects_wrong_state_replay_deadline_and_account_replacement() {
        let now = Instant::now();
        let proof = pkce();
        let mut attempt = Attempt::new(7, now).unwrap();
        assert_eq!(
            attempt.accept_callback(7, now, &proof, "wrong-state", "code"),
            Err(AuthError::InvalidState)
        );
        attempt
            .accept_callback(7, now, &proof, proof.state.expose(), "code")
            .unwrap();
        assert_eq!(
            attempt.accept_callback(7, now, &proof, proof.state.expose(), "code"),
            Err(AuthError::InvalidState)
        );
        assert_eq!(attempt.commit(8, now), Err(AuthError::StaleAttempt));
        assert_eq!(
            attempt.commit(7, now + AUTH_ATTEMPT_TIMEOUT),
            Err(AuthError::Expired)
        );
        attempt.cancel();
        assert_eq!(attempt.commit(7, now), Err(AuthError::Cancelled));
        let mut success = Attempt::new(7, now).unwrap();
        success.commit(7, now).unwrap();
        assert_eq!(success.commit(7, now), Err(AuthError::InvalidState));
    }

    #[test]
    fn provider_ttl_can_shorten_but_never_restart_the_absolute_deadline() {
        let now = Instant::now();
        let mut attempt = Attempt::new(1, now).unwrap();
        attempt
            .limit_by_provider(1, now + Duration::from_secs(100), AUTH_ATTEMPT_TIMEOUT * 2)
            .unwrap();
        assert_eq!(
            attempt.check(1, now + Duration::from_secs(100)).unwrap(),
            AUTH_ATTEMPT_TIMEOUT - Duration::from_secs(100)
        );
        attempt
            .limit_by_provider(1, now + Duration::from_secs(100), Duration::from_secs(2))
            .unwrap();
        assert_eq!(
            attempt.check(1, now + Duration::from_secs(102)),
            Err(AuthError::Expired)
        );
    }

    #[test]
    fn provider_browser_destinations_reject_origin_and_path_confusion() {
        for url in [
            "https://platform.deepseek.com.evil.test/dsh/authorize",
            "https://platform.deepseek.com@evil.test/dsh/authorize",
            "https://platform.deepseek.com/dsh/authorize/extra",
            "https://platform.deepseek.com/dsh/authorize#fragment",
            "https://platform.deepseek.com/dsh/authorize\\evil",
            "http://platform.deepseek.com/dsh/authorize",
        ] {
            let body = serde_json::json!({"code":0,"data":{"biz_code":0,"biz_data":{
                "authorize_url":url,"authorize_id":"synthetic-id","expires_in":60
            }}})
            .to_string();
            assert_eq!(
                parse_deepseek_initialize(&response(200, &body)).unwrap_err(),
                AuthError::InvalidResponse
            );
        }
        let body = r#"{"code":0,"data":{"biz_code":0,"biz_data":{"authorize_url":"https://platform.deepseek.com/dsh/authorize?attempt=synthetic","authorize_id":"id","expires_in":60}}}"#;
        let parsed = parse_deepseek_initialize(&response(200, body)).unwrap();
        assert_eq!(parsed.expires_in, Duration::from_secs(60));
        assert_eq!(parsed.authorize_id.expose(), "id");
        assert_eq!(
            parsed.authorize_url.expose(),
            "https://platform.deepseek.com/dsh/authorize?attempt=synthetic"
        );
    }

    #[test]
    fn deepseek_grant_stays_opaque_and_logout_uses_its_own_header() {
        let body = r#"{"code":0,"data":{"biz_code":0,"biz_data":{"token":"synthetic-deepseek-token","authorized_url":"https://platform.deepseek.com/dsh/authorized"}}}"#;
        let parsed = parse_deepseek_exchange(&response(200, body)).unwrap();
        assert!(matches!(
            parsed.grant.lifecycle,
            TokenLifecycle::OpaqueUntilRejected
        ));
        let logout =
            deepseek_logout(&client(Provider::DeepSeek), &parsed.grant, &metadata()).unwrap();
        assert_eq!(
            logout.url,
            "https://platform.deepseek.com/auth-api/v0/users/logout"
        );
        assert!(
            logout
                .headers
                .contains(&("x-dsh-auth-token".into(), "synthetic-deepseek-token".into()))
        );
        assert!(
            !logout
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        );
        assert_eq!(
            opencode_refresh(&client(Provider::OpenCode), &parsed.grant),
            Err(AuthError::InvalidInput)
        );
        assert!(!format!("{:?}", parsed.grant).contains("synthetic-deepseek-token"));
        assert_eq!(
            parse_deepseek_exchange(&response(200, r#"{"code":40003}"#)).unwrap_err(),
            AuthError::NeedsLogin
        );
        assert_eq!(
            parse_deepseek_exchange(&response(200, r#"{"code":0,"data":{"biz_code":17}}"#))
                .unwrap_err(),
            AuthError::AuthorizationRejected
        );
        assert!(
            parse_deepseek_logout(&response(
                200,
                r#"{"code":0,"data":{"biz_code":0,"biz_data":{}}}"#
            ))
            .is_ok()
        );
        assert_eq!(
            parse_deepseek_logout(&response(200, r#"{"code":0,"data":{"biz_code":17}}"#)),
            Err(AuthError::AuthorizationRejected)
        );
    }

    #[test]
    fn device_uri_fallback_retains_the_separate_redacted_user_code() {
        let challenge = parse_opencode_initialize(&response(200, r#"{"device_code":"synthetic-device","user_code":"SYNTHETIC-USER-CODE","verification_uri":"/console/device","expires_in":60,"interval":5}"#)).unwrap();
        assert_eq!(
            challenge.verification_url.expose(),
            "https://opencode.ai/console/device"
        );
        assert_eq!(challenge.user_code.expose(), "SYNTHETIC-USER-CODE");
        assert!(!format!("{challenge:?}").contains("SYNTHETIC-USER-CODE"));
        assert!(parse_opencode_initialize(&response(200, r#"{"device_code":"d","user_code":"u","verification_uri":"https://opencode.ai.evil.test/console/device","expires_in":60,"interval":5}"#)).is_err());
        assert!(
            parse_opencode_initialize(&response(
                200,
                r#"{"device_code":"d","user_code":"u","expires_in":60,"interval":5}"#
            ))
            .is_err()
        );
    }

    #[test]
    fn opencode_device_and_refresh_messages_preserve_explicit_client_identity() {
        let client = client(Provider::OpenCode);
        let init = opencode_initialize(&client).unwrap();
        assert_eq!(init.url, "https://opencode.ai/console/auth/device/code");
        assert_eq!(init.max_redirects, 0);
        let challenge = parse_opencode_initialize(&response(200, r#"{"device_code":"synthetic-device","user_code":"ABCD-EFGH","verification_uri_complete":"/console/auth/device?user_code=ABCD-EFGH","expires_in":600,"interval":5}"#)).unwrap();
        assert_eq!(challenge.poll_interval, Duration::from_secs(5));
        assert_eq!(challenge.expires_in, Duration::from_secs(600));
        assert_eq!(challenge.user_code.expose(), "ABCD-EFGH");
        assert!(
            challenge
                .verification_url
                .expose()
                .starts_with("https://opencode.ai/")
        );
        let request = opencode_poll(&client, &challenge.device_code).unwrap();
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["client_id"], "minicon-synthetic-client");
        assert_eq!(
            body["grant_type"],
            "urn:ietf:params:oauth:grant-type:device_code"
        );
        let now = Instant::now();
        let DevicePoll::Granted(grant) = parse_opencode_token(&response(200, r#"{"access_token":"synthetic-access","refresh_token":"synthetic-refresh","expires_in":3600,"org_id":"org_synthetic"}"#), now).unwrap() else { panic!("grant expected") };
        assert!(
            matches!(grant.lifecycle, TokenLifecycle::ExpiringRefresh { expires_at, .. } if expires_at == now + Duration::from_secs(3600))
        );
        assert_eq!(grant.organization.as_deref(), Some("org_synthetic"));
        let request = opencode_refresh(&client, &grant).unwrap();
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["grant_type"], "refresh_token");
        assert_eq!(body["refresh_token"], "synthetic-refresh");
        assert_eq!(body["client_id"], "minicon-synthetic-client");
        let debug = format!("{grant:?}");
        assert!(!debug.contains("synthetic-access"));
        assert!(!debug.contains("synthetic-refresh"));
    }

    #[test]
    fn device_pending_slow_down_and_failure_never_become_grants() {
        let now = Instant::now();
        assert!(matches!(
            parse_opencode_token(&response(400, r#"{"error":"authorization_pending"}"#), now)
                .unwrap(),
            DevicePoll::Pending
        ));
        assert!(matches!(
            parse_opencode_token(&response(400, r#"{"error":"slow_down"}"#), now).unwrap(),
            DevicePoll::SlowDown
        ));
        assert_eq!(
            parse_opencode_token(&response(400, r#"{"error":"invalid_grant"}"#), now).unwrap_err(),
            AuthError::NeedsLogin
        );
        assert_eq!(
            parse_opencode_token(
                &response(
                    400,
                    r#"{"error":"access_denied","description":"synthetic-secret-code"}"#
                ),
                now
            )
            .unwrap_err(),
            AuthError::AuthorizationRejected
        );
        assert_eq!(
            parse_opencode_token(
                &response(
                    400,
                    r#"{"error":"authorization_pending","access_token":"unexpected"}"#
                ),
                now
            )
            .unwrap_err(),
            AuthError::InvalidResponse
        );
        assert_eq!(
            parse_opencode_token(&response(503, r#"{"error":"authorization_pending"}"#), now)
                .unwrap_err(),
            AuthError::HttpStatus(503)
        );
        assert_eq!(
            parse_opencode_refresh(&response(400, r#"{"error":"authorization_pending"}"#), now)
                .unwrap_err(),
            AuthError::InvalidResponse
        );
    }

    #[test]
    fn bounded_parsers_reject_truncation_duplicate_token_and_malformed_secret() {
        let now = Instant::now();
        let mut truncated = response(200, "{}");
        truncated.truncated = true;
        assert_eq!(
            parse_opencode_token(&truncated, now).unwrap_err(),
            AuthError::ResponseTooLarge
        );
        let oversized = response(200, &"x".repeat(AUTH_MAX_RESPONSE_BYTES + 1));
        assert_eq!(
            parse_opencode_token(&oversized, now).unwrap_err(),
            AuthError::ResponseTooLarge
        );
        for body in [
            r#"{"access_token":"one","access_token":"two","refresh_token":"r","expires_in":60}"#,
            r#"{"access_token":"bad\nsecret","refresh_token":"r","expires_in":60}"#,
            r#"{"access_token":"a","refresh_token":"r","expires_in":0}"#,
            r#"{"access_token":"a","expires_in":60}"#,
            r#"{"access_token":"synthetic-secret"} trailing"#,
        ] {
            let error = parse_opencode_token(&response(200, body), now).unwrap_err();
            assert_eq!(error, AuthError::InvalidResponse);
            assert!(!error.to_string().contains("synthetic-secret"));
        }
        let wrong_origin = response(
            200,
            r#"{"device_code":"d","user_code":"u","verification_uri_complete":"https://opencode.ai.evil.test/verify","expires_in":60,"interval":5}"#,
        );
        assert_eq!(
            parse_opencode_initialize(&wrong_origin).unwrap_err(),
            AuthError::InvalidResponse
        );
    }
}
