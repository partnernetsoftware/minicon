//! Account metadata monitoring. Private storage is owned by the existing usage writer.
//! PRD 31.01.03.02; metadata never confers inference readiness.
use crate::provider_auth::Provider;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const MAX_ACCOUNTS: usize = 16;
pub const SNAPSHOT_BYTES: usize = 512 * 1024;
pub const POLICY_BYTES: usize = 16 * 1024;
pub const RESPONSE_BYTES: usize = 64 * 1024;
pub const HTTP_SECONDS: u64 = 15;
pub const INTERVAL_SECONDS: u64 = 300;
pub const FRESH_SECONDS: u64 = 600;

/// String decimal from the provider; never an IEEE monetary amount.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Decimal(pub String);
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Currency {
    Cny,
    Usd,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum WalletKind {
    Normal,
    Bonus,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Wallet {
    pub kind: WalletKind,
    pub currency: Currency,
    pub balance: Decimal,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Window {
    Rolling,
    Weekly,
    Monthly,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Allowance {
    pub window: Window,
    pub limited: bool,
    pub used_percent: Decimal,
    /// Official reset string, not the local observation timestamp.
    pub resets_at: String,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Resources {
    DeepSeekWallets(Vec<Wallet>),
    OpenCodeGo(Vec<Allowance>),
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum MetadataSource {
    DeepSeekSummaryV0,
    OpenCodeGoV1,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Reading {
    pub source: MetadataSource,
    pub observed_unix_ms: u64,
    pub provider_as_of: Option<String>,
    pub resources: Resources,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Freshness {
    Never,
    Fresh,
    Stale,
    PreviousAuthority,
    ClockUncertain,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum MonitorError {
    Disabled,
    MissingAccount,
    NeedsAuth,
    Forbidden,
    RateLimited,
    NeedsOrganization,
    Network,
    Deadline,
    Malformed,
    Busy,
    Storage,
    Cancelled,
    StaleAuthority,
    WorkerUnavailable,
    InvalidAction,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PauseReason {
    Off,
    Stop,
    Disabled,
    NeedsAuth,
    AuthorityChanged,
    Backoff,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Persistence {
    #[default]
    Opening,
    Ready,
    Pending,
    DurabilityUnconfirmed,
    Unavailable,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Capability {
    OfficialContract,
    Unknown,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum MonitorOperation {
    Refresh,
    Policy,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Attempt {
    pub request: u64,
    pub finished_unix_ms: Option<u64>,
    pub result: Result<(), MonitorError>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MonitorReceipt {
    pub account: String,
    pub request: u64,
    pub operation: MonitorOperation,
    pub result: Result<(), MonitorError>,
    pub persistence: Persistence,
}
/// A threshold addresses one actual official resource. No cross-currency totals.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Threshold {
    WalletBelow {
        kind: WalletKind,
        currency: Currency,
        value: Decimal,
    },
    GoUsedAtLeast {
        value: Decimal,
    },
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PolicyInput {
    pub enabled: bool,
    pub thresholds: Vec<Threshold>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountMonitorView {
    pub account: String,
    pub provider: Provider,
    pub policy_revision: u64,
    pub policy: PolicyInput,
    pub paused: Option<PauseReason>,
    pub active_request: Option<u64>,
    pub next_due_unix_ms: Option<u64>,
    pub last_success: Option<Reading>,
    pub freshness: Freshness,
    pub latest_attempt: Option<Attempt>,
    pub attention: bool,
    pub wallet_capability: Capability,
    pub allowance_capability: Capability,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MonitorView {
    pub revision: u64,
    pub persistence: Persistence,
    pub accounts: Vec<AccountMonitorView>,
    /// Exact delivery acknowledgements; never evict an unacknowledged account.
    pub receipts: Vec<MonitorReceipt>,
}
pub type SharedMonitorView = Arc<MonitorView>;

use crate::provider_accounts::{AccountSnapshot, AccountStore, StoreError};
use crate::provider_login::LoginMetadata;
use crate::provider_server::Gateway;
use minicon_platform::network_http::NetworkHttpResponse;
use minicon_platform::network_http_stream::NetworkHttpCancel;
use minicon_platform::{filesystem, filesystem_publish};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::Mutex,
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_BACKOFF_SECONDS: u64 = INTERVAL_SECONDS * 16;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RetryAfter {
    Missing,
    Invalid,
    NotBefore(u64),
    Paused,
}
pub(crate) struct MetadataRead {
    pub result: Result<Reading, MonitorError>,
    pub retry_after: RetryAfter,
    pub grant_stamp: Option<GrantStamp>,
}
/// Private and deliberately neither Debug nor Serialize. Scope is the exact
/// authorization/refresh material, excluding reconstructed Instant expiry.
pub(crate) struct GrantStamp([u8; 32]);
impl GrantStamp {
    pub(crate) fn capture(grant: &crate::provider_auth::TokenGrant) -> Self {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update(b"minicon-account-metadata-grant-v1");
        hash.update([match grant.provider {
            Provider::DeepSeek => 0,
            Provider::OpenCode => 1,
        }]);
        let mut field = |value: &str| {
            hash.update((value.len() as u64).to_le_bytes());
            hash.update(value.as_bytes());
        };
        field(grant.access_token.expose());
        match &grant.lifecycle {
            crate::provider_auth::TokenLifecycle::OpaqueUntilRejected => field("opaque"),
            crate::provider_auth::TokenLifecycle::ExpiringRefresh { refresh_token, .. } => {
                field("refresh");
                field(refresh_token.expose());
            }
        }
        field(grant.organization.as_deref().unwrap_or(""));
        Self(hash.finalize().into())
    }
    fn matches(&self, grant: &crate::provider_auth::TokenGrant) -> bool {
        let current = Self::capture(grant);
        self.0
            .iter()
            .zip(current.0)
            .fold(0u8, |a, (b, c)| a | (b ^ c))
            == 0
    }
}
impl MetadataRead {
    pub(crate) fn plain(result: Result<Reading, MonitorError>) -> Self {
        Self {
            result,
            retry_after: RetryAfter::Missing,
            grant_stamp: None,
        }
    }
}
/// RFC9110 10.2.3 / 5.6.7. Never retain raw headers. A large or ambiguous
/// directive pauses automatic traffic rather than retrying earlier than asked.
pub(crate) fn parse_retry_after(headers: &[(String, String)], now: Option<u64>) -> RetryAfter {
    let mut values = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("retry-after"));
    let Some((_, raw)) = values.next() else {
        return RetryAfter::Missing;
    };
    if values.next().is_some() {
        return RetryAfter::Paused;
    }
    let raw = raw.trim_matches([' ', '\t']);
    if raw.is_empty() {
        return RetryAfter::Invalid;
    }
    if raw.len() > 128 {
        return RetryAfter::Paused;
    }
    let Some(now) = now else {
        return RetryAfter::Paused;
    };
    let until = if raw.bytes().all(|b| b.is_ascii_digit()) {
        let mut seconds = 0u64;
        for b in raw.bytes() {
            seconds = (seconds * 10 + u64::from(b - b'0')).min(MAX_BACKOFF_SECONDS + 1);
        }
        if seconds > MAX_BACKOFF_SECONDS {
            return RetryAfter::Paused;
        }
        now.checked_add(seconds * 1000)
    } else {
        match http_date_seconds(raw, now / 1000) {
            Some(date) => u64::try_from(date)
                .ok()
                .and_then(|s| s.checked_mul(1000))
                .or(Some(0)),
            None => return RetryAfter::Invalid,
        }
    };
    let Some(until) = until else {
        return RetryAfter::Paused;
    };
    if until.saturating_sub(now) > MAX_BACKOFF_SECONDS * 1000 {
        return RetryAfter::Paused;
    }
    RetryAfter::NotBefore(until.max(now))
}
fn http_date_seconds(raw: &str, now: u64) -> Option<i64> {
    if raw.len() > 64 || !raw.is_ascii() {
        return None;
    }
    let fields: Vec<_> = raw.split_ascii_whitespace().collect();
    let number = |v: &str| {
        (!v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
            .then(|| v.parse::<i64>().ok())
            .flatten()
    };
    let (weekday, day, month, year, time) = match fields.as_slice() {
        [weekday, day, month, year, time, "GMT"] if weekday.ends_with(',') && year.len() == 4 => (
            weekday.trim_end_matches(','),
            number(day)?,
            *month,
            number(year)?,
            *time,
        ),
        [weekday, date, time, "GMT"] if weekday.ends_with(',') => {
            let pieces: Vec<_> = date.split('-').collect();
            let [day, month, year] = pieces.as_slice() else {
                return None;
            };
            if year.len() != 2 {
                return None;
            }
            let current = http_year(now)?;
            let mut year = current / 100 * 100 + number(year)?;
            if year > current + 50 {
                year -= 100;
            }
            (
                weekday.trim_end_matches(','),
                number(day)?,
                *month,
                year,
                *time,
            )
        }
        [weekday, month, day, time, year] if year.len() == 4 => {
            (*weekday, number(day)?, *month, number(year)?, *time)
        }
        _ => return None,
    };
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|m| *m == month)?
        + 1;
    let times: Vec<_> = time.split(':').collect();
    let [hour, minute, second] = times.as_slice() else {
        return None;
    };
    if [hour, minute, second].iter().any(|v| v.len() != 2) {
        return None;
    }
    let (hour, minute, second) = (number(hour)?, number(minute)?, number(second)?);
    if !(1..=9999).contains(&year) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if day < 1 || day > months[month - 1] {
        return None;
    }
    let days = days_at_year(year) + months[..month - 1].iter().sum::<i64>() + day - 1;
    let expected = (days + 4).rem_euclid(7) as usize;
    let short = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let long = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    if weekday != short[expected] && weekday != long[expected] {
        return None;
    }
    Some(days * 86400 + hour * 3600 + minute * 60 + second)
}
fn leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}
fn days_at_year(year: i64) -> i64 {
    let before = year - 1;
    365 * (year - 1970) + (before / 4 - before / 100 + before / 400)
        - (1969 / 4 - 1969 / 100 + 1969 / 400)
}
fn http_year(seconds: u64) -> Option<i64> {
    let seconds = i64::try_from(seconds).ok()?;
    (1970..=9999).find(|year| seconds < days_at_year(year + 1) * 86400)
}
fn retry_wait(stored: &StoredAccount) -> Option<Duration> {
    if stored.retry_after_paused {
        return None;
    }
    let Some(until) = stored.retry_after_unix_ms else {
        return Some(Duration::ZERO);
    };
    let remaining = until.saturating_sub(now_ms()?);
    (remaining <= MAX_BACKOFF_SECONDS * 1000).then(|| Duration::from_millis(remaining))
}

/// Persisted authority contains no token, raw organization, proxy, or label.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Authority {
    account: String,
    provider: Provider,
    credential: String,
    service: String,
    organization_epoch: String,
    organization_digest: String,
}
impl Authority {
    pub(crate) fn capture(s: &AccountSnapshot) -> Self {
        Self {
            account: s.summary.id.clone(),
            provider: s.summary.provider,
            credential: s.generation.clone(),
            service: s.preferences.service_generation.clone(),
            organization_epoch: s.organization.epoch.clone(),
            organization_digest: s.organization.digest.clone(),
        }
    }
    pub(crate) fn matches(&self, s: &AccountSnapshot) -> bool {
        s.preferences.enabled && *self == Self::capture(s)
    }
    fn valid(&self) -> bool {
        [
            &self.account,
            &self.credential,
            &self.service,
            &self.organization_epoch,
        ]
        .iter()
        .all(|v| crate::provider_server::opaque_id(v))
            && self.organization_digest.len() == 64
            && self
                .organization_digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredAccount {
    authority: Authority,
    policy_revision: u64,
    policy: PolicyInput,
    auto_needs_auth: bool,
    #[serde(default)]
    retry_after_unix_ms: Option<u64>,
    #[serde(default)]
    retry_after_paused: bool,
    #[serde(default)]
    retry_authority: Option<Authority>,
    reading: Option<Reading>,
    reading_authority: Option<Authority>,
    attempt: Option<Attempt>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MonitorSnapshot {
    version: u8,
    revision: u64,
    accounts: Vec<StoredAccount>,
}
impl Default for MonitorSnapshot {
    fn default() -> Self {
        Self {
            version: 1,
            revision: 0,
            accounts: Vec::new(),
        }
    }
}
impl MonitorSnapshot {
    fn bounded(&self) -> bool {
        self.version == 1
            && self.revision < u64::MAX
            && self.accounts.len() <= MAX_ACCOUNTS
            && self.accounts.iter().enumerate().all(|(i, a)| {
                a.authority.valid()
                    && !self.accounts[..i]
                        .iter()
                        .any(|b| b.authority.account == a.authority.account)
                    && valid_policy(&a.policy)
                    && policy_matches_provider(&a.policy, a.authority.provider)
                    && (!a.policy.enabled || a.policy_revision > 0)
                    && (a.retry_after_unix_ms.is_none() && !a.retry_after_paused
                        || a.retry_authority.is_some())
                    && a.retry_authority.as_ref().is_none_or(|r| {
                        r.valid()
                            && r.account == a.authority.account
                            && r.provider == a.authority.provider
                    })
                    && a.reading_authority.as_ref().is_none_or(|r| {
                        r.valid()
                            && r.account == a.authority.account
                            && r.provider == a.authority.provider
                    })
                    && (a.reading.is_none() || a.reading_authority.is_some())
                    && a.reading.as_ref().is_none_or(valid_reading)
            })
    }
}
#[derive(Clone)]
pub(crate) struct DiskView {
    pub snapshot: Arc<MonitorSnapshot>,
    pub state: Persistence,
    pub completed_revision: u64,
}
/// Shared with the ONE usage writer. Submit is bounded/typed and performs no JSON or I/O.
pub(crate) struct DiskSlot {
    pending: Mutex<Option<Arc<MonitorSnapshot>>>,
    cache: Mutex<DiskView>,
}
impl Default for DiskSlot {
    fn default() -> Self {
        Self {
            pending: Mutex::new(None),
            cache: Mutex::new(DiskView {
                snapshot: Arc::new(MonitorSnapshot::default()),
                state: Persistence::Opening,
                completed_revision: 0,
            }),
        }
    }
}
impl DiskSlot {
    pub(crate) fn submit(&self, snapshot: Arc<MonitorSnapshot>) -> Result<(), MonitorError> {
        if !snapshot.bounded() {
            return Err(MonitorError::Malformed);
        }
        let cached_revision = self
            .cache
            .try_lock()
            .map_err(|_| MonitorError::Busy)?
            .snapshot
            .revision;
        if snapshot.revision < cached_revision {
            return Err(MonitorError::StaleAuthority);
        }
        let mut slot = self.pending.try_lock().map_err(|_| MonitorError::Busy)?;
        if slot
            .as_ref()
            .is_some_and(|old| old.revision > snapshot.revision)
        {
            return Err(MonitorError::StaleAuthority);
        }
        *slot = Some(snapshot);
        Ok(())
    }
    pub(crate) fn view(&self) -> Option<DiskView> {
        self.cache.try_lock().ok().map(|v| v.clone())
    }
    /// Writer thread only. Corruption is local to this file, never usage history.
    pub(crate) fn open(&self, root: &Path, prepared: bool) -> bool {
        let loaded = if prepared {
            match filesystem::read_private_bounded(
                &root.join("account-monitoring.json"),
                SNAPSHOT_BYTES,
            ) {
                Ok(b) => serde_json::from_slice::<MonitorSnapshot>(&b)
                    .ok()
                    .filter(MonitorSnapshot::bounded),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    Some(MonitorSnapshot::default())
                }
                Err(_) => None,
            }
        } else {
            None
        };
        let writable = loaded.is_some();
        let snapshot = Arc::new(loaded.unwrap_or_default());
        let completed_revision = snapshot.revision;
        if let Ok(mut cache) = self.cache.lock() {
            *cache = DiskView {
                snapshot,
                state: if writable {
                    Persistence::Ready
                } else {
                    Persistence::Unavailable
                },
                completed_revision,
            };
        }
        writable
    }
    /// At most one latest immutable snapshot per writer turn. Request receipts drain first.
    pub(crate) fn flush(&self, root: &Path, writable: bool) {
        let Some(snapshot) = self.pending.lock().ok().and_then(|mut s| s.take()) else {
            return;
        };
        let bytes = serde_json::to_vec(snapshot.as_ref())
            .ok()
            .filter(|b| b.len() <= SNAPSHOT_BYTES);
        let policies = serde_json::to_vec(
            &snapshot
                .accounts
                .iter()
                .map(|a| (&a.authority, &a.policy, a.policy_revision))
                .collect::<Vec<_>>(),
        )
        .ok()
        .is_some_and(|b| b.len() <= POLICY_BYTES);
        let publication = if writable && policies {
            bytes.map(|b| {
                filesystem_publish::write_private_atomic(&root.join("account-monitoring.json"), &b)
            })
        } else {
            None
        };
        let state = match publication {
            Some(Ok(())) => Persistence::Ready,
            Some(Err(e)) if e.published() => Persistence::DurabilityUnconfirmed,
            _ => Persistence::Unavailable,
        };
        if let Ok(mut cache) = self.cache.lock() {
            *cache = DiskView {
                completed_revision: snapshot.revision,
                snapshot,
                state,
            };
        }
    }
}

/// Preserve the official lexical decimal (including `.5`, `1.`, and `0E-16`).
/// Bounded base-ten comparison uses digits/exponent, never a binary money float.
#[derive(Eq, PartialEq)]
struct DecimalParts {
    negative: bool,
    digits: String,
    exponent: i32,
}
fn decimal_parts(raw: &str) -> Option<DecimalParts> {
    if raw.is_empty() || raw.len() > 128 {
        return None;
    }
    let (negative, rest) = raw.strip_prefix('-').map_or((false, raw), |v| (true, v));
    let split = rest.find(['e', 'E']);
    let (mantissa, exponent) = if let Some(i) = split {
        let e = &rest[i + 1..];
        if e.is_empty() || e.len() > 6 {
            return None;
        }
        (&rest[..i], e.parse::<i32>().ok()?)
    } else {
        (rest, 0)
    };
    if exponent.unsigned_abs() > 4096 {
        return None;
    }
    let mut point = false;
    let mut fractional = 0;
    let mut digits = String::new();
    for c in mantissa.chars() {
        match c {
            '.' if !point => point = true,
            '0'..='9' => {
                digits.push(c);
                if point {
                    fractional += 1
                }
            }
            _ => return None,
        }
    }
    if digits.is_empty() {
        return None;
    }
    let trimmed = digits.trim_start_matches('0');
    let mut digits = if trimmed.is_empty() {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    };
    let mut exponent = exponent - fractional;
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
        exponent += 1;
    }
    Some(DecimalParts {
        negative: negative && digits != "0",
        digits,
        exponent,
    })
}
fn decimal_cmp(a: &Decimal, b: &Decimal) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;
    let a = decimal_parts(&a.0)?;
    let b = decimal_parts(&b.0)?;
    if a.negative != b.negative {
        return Some(if a.negative {
            Ordering::Less
        } else {
            Ordering::Greater
        });
    }
    let magnitude = if a.digits == "0" && b.digits == "0" {
        Ordering::Equal
    } else if a.digits == "0" {
        Ordering::Less
    } else if b.digits == "0" {
        Ordering::Greater
    } else {
        let order = (a.digits.len() as i32 + a.exponent).cmp(&(b.digits.len() as i32 + b.exponent));
        if order != Ordering::Equal {
            order
        } else {
            let n = a.digits.len().max(b.digits.len());
            (0..n)
                .map(|i| a.digits.as_bytes().get(i).copied().unwrap_or(b'0'))
                .cmp((0..n).map(|i| b.digits.as_bytes().get(i).copied().unwrap_or(b'0')))
        }
    };
    Some(if a.negative {
        magnitude.reverse()
    } else {
        magnitude
    })
}
pub fn valid_policy(policy: &PolicyInput) -> bool {
    policy.thresholds.len() <= 5
        && policy.thresholds.iter().enumerate().all(|(i, t)| {
            let value = match t {
                Threshold::WalletBelow { value, .. } | Threshold::GoUsedAtLeast { value } => value,
            };
            decimal_parts(&value.0).is_some_and(|v| !v.negative)
                && !policy.thresholds[..i].iter().any(|p| match (p, t) {
                    (
                        Threshold::WalletBelow {
                            kind: a,
                            currency: b,
                            ..
                        },
                        Threshold::WalletBelow {
                            kind: c,
                            currency: d,
                            ..
                        },
                    ) => a == c && b == d,
                    (Threshold::GoUsedAtLeast { .. }, Threshold::GoUsedAtLeast { .. }) => true,
                    _ => false,
                })
        })
}
fn policy_matches_provider(policy: &PolicyInput, provider: Provider) -> bool {
    policy.thresholds.iter().all(|t| {
        matches!(
            (provider, t),
            (Provider::DeepSeek, Threshold::WalletBelow { .. })
                | (Provider::OpenCode, Threshold::GoUsedAtLeast { .. })
        )
    })
}

fn valid_reading(r: &Reading) -> bool {
    r.observed_unix_ms > 0
        && matches!(
            (&r.source, &r.resources),
            (
                MetadataSource::DeepSeekSummaryV0,
                Resources::DeepSeekWallets(_)
            ) | (MetadataSource::OpenCodeGoV1, Resources::OpenCodeGo(_))
        )
        && r.provider_as_of.as_ref().is_none_or(|v| valid_stamp(v))
        && match &r.resources {
            Resources::DeepSeekWallets(rows) => {
                rows.len() <= 16 && rows.iter().all(|r| decimal_parts(&r.balance.0).is_some())
            }
            Resources::OpenCodeGo(rows) => {
                rows.len() == 3
                    && rows.iter().enumerate().all(|(i, r)| {
                        decimal_parts(&r.used_percent.0).is_some()
                            && valid_stamp(&r.resets_at)
                            && !rows[..i].iter().any(|p| p.window == r.window)
                    })
            }
        }
}
fn valid_stamp(v: &str) -> bool {
    // A bounded RFC3339 subset of the official DateFromString contract. Invalid
    // or unfamiliar source date formats retain the old reading as Malformed.
    if v.len() < 20 || v.len() > 64 || !v.is_ascii() {
        return false;
    }
    let b = v.as_bytes();
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return false;
    }
    let number = |a: usize, z: usize| {
        v.get(a..z)
            .filter(|s| s.bytes().all(|c| c.is_ascii_digit()))
            .and_then(|s| s.parse::<u32>().ok())
    };
    let Some(year) = number(0, 4) else {
        return false;
    };
    let Some(month) = number(5, 7) else {
        return false;
    };
    let Some(day) = number(8, 10) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => return false,
    };
    if year == 0
        || day == 0
        || day > days
        || number(11, 13).is_none_or(|n| n > 23)
        || number(14, 16).is_none_or(|n| n > 59)
        || number(17, 19).is_none_or(|n| n > 59)
    {
        return false;
    }
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    if &v[i..] == "Z" {
        return true;
    }
    if b.len() != i + 6 || !matches!(b[i], b'+' | b'-') || b[i + 3] != b':' {
        return false;
    }
    number(i + 1, i + 3).is_some_and(|n| n <= 23) && number(i + 4, i + 6).is_some_and(|n| n <= 59)
}

pub(crate) fn now_ms() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .filter(|v| *v > 0)
}
#[derive(Deserialize)]
struct DeepSeekEnvelope {
    code: i64,
    data: Option<DeepSeekData>,
}
#[derive(Deserialize)]
struct DeepSeekData {
    biz_code: i64,
    biz_data: Option<DeepSeekWallets>,
}
#[derive(Deserialize)]
struct DeepSeekWallets {
    normal_wallets: Vec<WireWallet>,
    bonus_wallets: Vec<WireWallet>,
}
#[derive(Deserialize)]
struct WireWallet {
    currency: String,
    balance: String,
}
#[derive(Deserialize)]
struct GoEnvelope {
    usage: GoWindows,
}
#[derive(Deserialize)]
struct GoWindows {
    rolling: GoMeter,
    weekly: GoMeter,
    monthly: GoMeter,
}
#[derive(Deserialize)]
struct GoMeter {
    status: String,
    percent: Box<serde_json::value::RawValue>,
    #[serde(rename = "resetsAt")]
    resets_at: String,
}
pub(crate) fn parse_response(
    provider: Provider,
    response: &NetworkHttpResponse,
    observed: u64,
) -> Result<Reading, MonitorError> {
    if response.body.len() > RESPONSE_BYTES || response.truncated {
        return Err(MonitorError::Malformed);
    }
    match response.status {
        401 => return Err(MonitorError::NeedsAuth),
        403 => return Err(MonitorError::Forbidden),
        429 => return Err(MonitorError::RateLimited),
        200..=299 => {}
        _ => return Err(MonitorError::Network),
    }
    let resources = match provider {
        Provider::DeepSeek => {
            let envelope: DeepSeekEnvelope =
                serde_json::from_slice(&response.body).map_err(|_| MonitorError::Malformed)?;
            if envelope.code == 40003 {
                return Err(MonitorError::NeedsAuth);
            }
            if envelope.code != 0 {
                return Err(MonitorError::Malformed);
            }
            let data = envelope.data.ok_or(MonitorError::Malformed)?;
            if data.biz_code != 0 {
                return Err(MonitorError::Malformed);
            }
            let data = data.biz_data.ok_or(MonitorError::Malformed)?;
            if data.normal_wallets.len() + data.bonus_wallets.len() > 16 {
                return Err(MonitorError::Malformed);
            }
            let mut wallets = Vec::new();
            for (kind, rows) in [
                (WalletKind::Normal, data.normal_wallets),
                (WalletKind::Bonus, data.bonus_wallets),
            ] {
                for row in rows {
                    let currency = match row.currency.as_str() {
                        "CNY" => Currency::Cny,
                        "USD" => Currency::Usd,
                        _ => return Err(MonitorError::Malformed),
                    };
                    if decimal_parts(&row.balance).is_none()
                        || wallets
                            .iter()
                            .any(|w: &Wallet| w.kind == kind && w.currency == currency)
                    {
                        return Err(MonitorError::Malformed);
                    }
                    wallets.push(Wallet {
                        kind,
                        currency,
                        balance: Decimal(row.balance),
                    });
                }
            }
            Resources::DeepSeekWallets(wallets)
        }
        Provider::OpenCode => {
            let data: GoEnvelope =
                serde_json::from_slice(&response.body).map_err(|_| MonitorError::Malformed)?;
            let mut rows = Vec::new();
            for (window, meter) in [
                (Window::Rolling, data.usage.rolling),
                (Window::Weekly, data.usage.weekly),
                (Window::Monthly, data.usage.monthly),
            ] {
                let raw = meter.percent.get();
                if decimal_parts(raw).is_none()
                    || raw.parse::<f64>().ok().is_none_or(|v| !v.is_finite())
                    || !valid_stamp(&meter.resets_at)
                {
                    return Err(MonitorError::Malformed);
                }
                let limited = match meter.status.as_str() {
                    "ok" => false,
                    "rate-limited" => true,
                    _ => return Err(MonitorError::Malformed),
                };
                rows.push(Allowance {
                    window,
                    limited,
                    used_percent: Decimal(raw.to_owned()),
                    resets_at: meter.resets_at,
                });
            }
            Resources::OpenCodeGo(rows)
        }
    };
    let reading = Reading {
        source: match provider {
            Provider::DeepSeek => MetadataSource::DeepSeekSummaryV0,
            Provider::OpenCode => MetadataSource::OpenCodeGoV1,
        },
        observed_unix_ms: observed,
        provider_as_of: None,
        resources,
    };
    if !valid_reading(&reading) {
        return Err(MonitorError::Malformed);
    }
    Ok(reading)
}
fn attention(policy: &PolicyInput, reading: &Reading) -> bool {
    policy
        .thresholds
        .iter()
        .any(|threshold| match (threshold, &reading.resources) {
            (
                Threshold::WalletBelow {
                    kind,
                    currency,
                    value,
                },
                Resources::DeepSeekWallets(rows),
            ) => rows.iter().any(|r| {
                r.kind == *kind
                    && r.currency == *currency
                    && decimal_cmp(&r.balance, value) == Some(std::cmp::Ordering::Less)
            }),
            (Threshold::GoUsedAtLeast { value }, Resources::OpenCodeGo(rows)) => rows
                .iter()
                .any(|r| decimal_cmp(&r.used_percent, value).is_some_and(|v| !v.is_lt())),
            _ => false,
        })
}

#[derive(Clone)]
struct Reservation {
    request: u64,
    operation: MonitorOperation,
    off: bool,
    epoch: u64,
}
#[derive(Default)]
struct GateState {
    reservations: BTreeMap<String, Reservation>,
    off_reservations: BTreeMap<String, Reservation>,
    acknowledged: Vec<(String, u64)>,
    active: Option<(String, Arc<NetworkHttpCancel>)>,
    last_request: u64,
}
#[derive(Clone, Default)]
pub(crate) struct MonitorGate(Arc<Mutex<GateState>>);
impl MonitorGate {
    pub(crate) fn next_request(&self) -> Result<u64, MonitorError> {
        self.0
            .lock()
            .map_err(|_| MonitorError::Storage)?
            .last_request
            .checked_add(1)
            .ok_or(MonitorError::InvalidAction)
    }
    /// GUI side: reserve before try_send. An off intent cancels only metadata.
    pub(crate) fn reserve(
        &self,
        account: &str,
        request: u64,
        operation: MonitorOperation,
        off: bool,
        epoch: u64,
    ) -> Result<(), MonitorError> {
        if request == 0 || !crate::provider_server::opaque_id(account) {
            return Err(MonitorError::InvalidAction);
        }
        let mut state = self.0.lock().map_err(|_| MonitorError::Storage)?;
        if request <= state.last_request {
            return Err(MonitorError::InvalidAction);
        }
        let known = state.reservations.contains_key(account)
            || state.off_reservations.contains_key(account);
        if !known
            && state
                .reservations
                .keys()
                .chain(state.off_reservations.keys())
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                >= MAX_ACCOUNTS
        {
            return Err(MonitorError::Busy);
        }
        if off {
            if operation != MonitorOperation::Policy
                || state.off_reservations.contains_key(account)
                || state
                    .reservations
                    .get(account)
                    .is_some_and(|r| r.operation != MonitorOperation::Refresh)
            {
                return Err(MonitorError::Busy);
            }
        } else if state.reservations.contains_key(account)
            || state.off_reservations.contains_key(account)
        {
            return Err(MonitorError::Busy);
        }
        state.last_request = request;
        let reservation = Reservation {
            request,
            operation,
            off,
            epoch,
        };
        if off {
            state
                .off_reservations
                .insert(account.to_owned(), reservation);
        } else {
            state.reservations.insert(account.to_owned(), reservation);
        }
        if off
            && let Some((active, cancel)) = &state.active
            && active == account
        {
            cancel.cancel();
        }
        Ok(())
    }
    pub(crate) fn withdraw(&self, account: &str, request: u64) {
        if let Ok(mut state) = self.0.lock() {
            if state
                .reservations
                .get(account)
                .is_some_and(|r| r.request == request)
            {
                state.reservations.remove(account);
            }
            if state
                .off_reservations
                .get(account)
                .is_some_and(|r| r.request == request)
            {
                state.off_reservations.remove(account);
            }
        }
    }
    pub(crate) fn acknowledge(&self, receipts: &[MonitorReceipt]) {
        if let Ok(mut state) = self.0.lock() {
            for r in receipts {
                let normal = state
                    .reservations
                    .get(&r.account)
                    .is_some_and(|s| s.request == r.request);
                let off = state
                    .off_reservations
                    .get(&r.account)
                    .is_some_and(|s| s.request == r.request);
                if normal {
                    state.reservations.remove(&r.account);
                }
                if off {
                    state.off_reservations.remove(&r.account);
                }
                if normal || off {
                    state.acknowledged.push((r.account.clone(), r.request));
                }
            }
        }
    }
    pub(crate) fn stop(&self) {
        if let Ok(state) = self.0.lock()
            && let Some((_, cancel)) = &state.active
        {
            cancel.cancel();
        }
    }
    fn pending_off(&self, account: &str) -> bool {
        self.0.lock().map_or(true, |s| {
            s.off_reservations.get(account).is_some_and(|r| r.off)
        })
    }
    fn matches(
        &self,
        account: &str,
        request: u64,
        operation: MonitorOperation,
        epoch: u64,
    ) -> bool {
        self.0.lock().is_ok_and(|s| {
            s.reservations
                .get(account)
                .into_iter()
                .chain(s.off_reservations.get(account))
                .any(|r| r.request == request && r.operation == operation && r.epoch == epoch)
        })
    }
}
struct Entry {
    stored: StoredAccount,
    next: Instant,
    last_started: Option<Instant>,
    fresh_this_run: bool,
    observed_instant: Option<Instant>,
    paused: Option<PauseReason>,
    failures: u8,
}
struct Job {
    account: String,
    request: u64,
    epoch: u64,
    authority: Authority,
    cancel: Arc<NetworkHttpCancel>,
    worker: JoinHandle<MetadataRead>,
}
pub(crate) struct Controller {
    store: Arc<AccountStore>,
    gateway: Arc<Gateway>,
    metadata: LoginMetadata,
    gate: MonitorGate,
    stop_epoch: Arc<std::sync::atomic::AtomicU64>,
    entries: BTreeMap<String, Entry>,
    receipts: BTreeMap<(String, u64), MonitorReceipt>,
    policy_wait: BTreeMap<String, (u64, u64)>,
    job: Option<Job>,
    revision: u64,
    persistence: Persistence,
    completed_revision: u64,
    observed_stop_epoch: u64,
    last_pause_epoch: Option<u64>,
    loaded: bool,
    session_paused: bool,
    resumed: std::collections::BTreeSet<String>,
    last_sync: Instant,
    disk_pending: bool,
}
impl Controller {
    pub(crate) fn new(
        store: Arc<AccountStore>,
        gateway: Arc<Gateway>,
        metadata: LoginMetadata,
        gate: MonitorGate,
        stop_epoch: Arc<std::sync::atomic::AtomicU64>,
    ) -> Self {
        Self {
            store,
            gateway,
            metadata,
            gate,
            stop_epoch,
            entries: BTreeMap::new(),
            receipts: BTreeMap::new(),
            policy_wait: BTreeMap::new(),
            job: None,
            revision: 0,
            persistence: Persistence::Opening,
            completed_revision: 0,
            observed_stop_epoch: 0,
            last_pause_epoch: None,
            loaded: false,
            session_paused: false,
            resumed: Default::default(),
            last_sync: Instant::now() - Duration::from_secs(2),
            disk_pending: false,
        }
    }
    fn changed(&mut self) {
        self.revision = self.revision.saturating_add(1);
        self.disk_pending = true;
    }
    fn snapshot(&self) -> Arc<MonitorSnapshot> {
        Arc::new(MonitorSnapshot {
            version: 1,
            revision: self.revision,
            accounts: self.entries.values().map(|e| e.stored.clone()).collect(),
        })
    }
    fn receipt(
        &mut self,
        account: String,
        request: u64,
        operation: MonitorOperation,
        result: Result<(), MonitorError>,
        persistence: Persistence,
    ) {
        self.drain_acknowledged();
        // Normal Refresh and priority Off each own an exact terminal slot.
        self.receipts
            .entry((account.clone(), request))
            .or_insert(MonitorReceipt {
                account,
                request,
                operation,
                result,
                persistence,
            });
    }
    fn drain_acknowledged(&mut self) {
        if let Ok(mut gate) = self.gate.0.lock() {
            for exact in gate.acknowledged.drain(..) {
                self.receipts.remove(&exact);
            }
        }
    }
    fn sync_actual_stop(&mut self) -> u64 {
        let current = self.stop_epoch.load(std::sync::atomic::Ordering::Acquire);
        if current != self.observed_stop_epoch {
            self.observed_stop_epoch = current;
            self.pause_for_epoch(current);
        }
        current
    }
    fn pause_for_epoch(&mut self, epoch: u64) {
        if self.last_pause_epoch == Some(epoch) {
            return;
        }
        self.last_pause_epoch = Some(epoch);
        self.session_paused = true;
        self.resumed.clear();
        self.gate.stop();
        if let Some(job) = &self.job {
            job.cancel.cancel();
        }
    }
    fn sync_accounts(&mut self) {
        let Ok(accounts) = self.store.list() else {
            return;
        };
        let ids: std::collections::BTreeSet<_> = accounts.iter().map(|a| a.id.clone()).collect();
        let before = self.entries.len();
        self.entries.retain(|id, _| ids.contains(id));
        // Receipts remain deliverable even when their account was removed.
        if before != self.entries.len() {
            self.changed();
        }
        for account in accounts.into_iter().take(MAX_ACCOUNTS) {
            let Ok(snapshot) = self.store.snapshot(&account.id) else {
                continue;
            };
            let authority = Authority::capture(&snapshot);
            self.entries
                .entry(account.id.clone())
                .or_insert_with(|| Entry {
                    stored: StoredAccount {
                        authority: authority.clone(),
                        policy_revision: 0,
                        policy: PolicyInput::default(),
                        auto_needs_auth: false,
                        retry_after_unix_ms: None,
                        retry_after_paused: false,
                        retry_authority: None,
                        reading: None,
                        reading_authority: None,
                        attempt: None,
                    },
                    next: Instant::now(),
                    last_started: None,
                    fresh_this_run: false,
                    observed_instant: None,
                    paused: None,
                    failures: 0,
                });
            if let Some(e) = self.entries.get_mut(&account.id) {
                if !snapshot.preferences.enabled {
                    e.paused = Some(PauseReason::Disabled);
                } else if e.stored.authority != authority {
                    e.paused = Some(PauseReason::AuthorityChanged);
                    e.fresh_this_run = false;
                } else if matches!(e.paused, Some(PauseReason::Disabled)) {
                    e.paused = None;
                }
            }
        }
    }
    fn load_disk(&mut self) {
        let Some(view) = self.gateway.monitor_disk_view() else {
            return;
        };
        self.accept_disk_view(view);
    }
    fn accept_disk_view(&mut self, view: DiskView) {
        if view.completed_revision < self.completed_revision {
            return;
        }
        self.persistence = view.state;
        self.completed_revision = view.completed_revision;
        if !self.loaded && view.state != Persistence::Opening {
            for stored in &view.snapshot.accounts {
                self.entries.insert(
                    stored.authority.account.clone(),
                    Entry {
                        stored: stored.clone(),
                        next: Instant::now() + retry_wait(stored).unwrap_or_default(),
                        last_started: None,
                        fresh_this_run: false,
                        observed_instant: None,
                        paused: if stored.auto_needs_auth {
                            Some(PauseReason::NeedsAuth)
                        } else {
                            None
                        },
                        failures: 0,
                    },
                );
            }
            self.revision = view.snapshot.revision;
            self.loaded = true;
            self.sync_accounts();
        }
        let completed: Vec<_> = self
            .policy_wait
            .iter()
            .filter(|(_, (_, revision))| *revision <= view.completed_revision)
            .map(|(a, (r, _))| (a.clone(), *r))
            .collect();
        for (account, request) in completed {
            self.policy_wait.remove(&account);
            let result = if view.state == Persistence::Unavailable {
                Err(MonitorError::Storage)
            } else {
                Ok(())
            };
            self.receipt(
                account,
                request,
                MonitorOperation::Policy,
                result,
                view.state,
            );
        }
    }
    /// Owner side. Exact reservation/Stop epoch is checked again before work.
    pub(crate) fn refresh(
        &mut self,
        account: String,
        request: u64,
        epoch: u64,
    ) -> Result<(), MonitorError> {
        self.drain_acknowledged();
        let current = self.sync_actual_stop();
        if epoch != current {
            self.receipt(
                account,
                request,
                MonitorOperation::Refresh,
                Err(MonitorError::Cancelled),
                self.visible_persistence(),
            );
            return Err(MonitorError::Cancelled);
        }
        if !self
            .gate
            .matches(&account, request, MonitorOperation::Refresh, epoch)
        {
            return Err(MonitorError::Cancelled);
        }
        let result = self.start(&account, request, epoch);
        if let Err(error) = result {
            self.receipt(
                account,
                request,
                MonitorOperation::Refresh,
                Err(error),
                self.persistence,
            );
        }
        result
    }
    pub(crate) fn policy(
        &mut self,
        account: String,
        request: u64,
        expected: u64,
        policy: PolicyInput,
        epoch: u64,
        resume: bool,
    ) -> Result<(), MonitorError> {
        self.drain_acknowledged();
        let current = self.sync_actual_stop();
        if epoch != current {
            self.receipt(
                account,
                request,
                MonitorOperation::Policy,
                Err(MonitorError::Cancelled),
                self.visible_persistence(),
            );
            return Err(MonitorError::Cancelled);
        }
        if !self
            .gate
            .matches(&account, request, MonitorOperation::Policy, epoch)
        {
            return Err(MonitorError::Cancelled);
        }
        let result = (|| {
            if !valid_policy(&policy) || self.revision >= u64::MAX - 1 {
                return Err(MonitorError::InvalidAction);
            }
            if !self.loaded {
                return Err(MonitorError::Busy);
            }
            let snapshot = self.store.snapshot(&account).map_err(store_error)?;
            let entry = self
                .entries
                .get_mut(&account)
                .ok_or(MonitorError::MissingAccount)?;
            if !policy_matches_provider(&policy, snapshot.summary.provider) {
                return Err(MonitorError::InvalidAction);
            }
            if entry.stored.policy_revision != expected {
                return Err(MonitorError::StaleAuthority);
            }
            let revision = expected.checked_add(1).ok_or(MonitorError::InvalidAction)?;
            entry.stored.policy_revision = revision;
            entry.stored.policy = policy;
            if resume {
                entry.stored.auto_needs_auth = false;
                // Oversized/ambiguous server hints require explicit user intent;
                // a finite unexpired not-before remains binding even on Resume.
                entry.stored.retry_after_paused = false;
            }
            let authority = Authority::capture(&snapshot);
            if entry.stored.authority != authority {
                entry.fresh_this_run = false;
            }
            entry.stored.authority = authority;
            if entry
                .stored
                .retry_authority
                .as_ref()
                .is_some_and(|a| !a.matches(&snapshot))
            {
                entry.stored.retry_after_paused = false;
                entry.stored.retry_after_unix_ms = None;
                entry.stored.retry_authority = None;
            }
            entry.next = Instant::now() + retry_wait(&entry.stored).unwrap_or_default();
            entry.failures = 0;
            entry.paused = if snapshot.preferences.enabled {
                None
            } else {
                Some(PauseReason::Disabled)
            };
            if resume && entry.stored.policy.enabled {
                self.resumed.insert(account.clone());
            }
            if self.gate.pending_off(&account)
                && let Some(job) = &self.job
                && job.account == account
            {
                job.cancel.cancel();
            }
            self.changed();
            self.policy_wait
                .insert(account.clone(), (request, self.revision));
            Ok(())
        })();
        if let Err(error) = result {
            self.receipt(
                account,
                request,
                MonitorOperation::Policy,
                Err(error),
                self.persistence,
            );
        }
        result
    }
    pub(crate) fn reject(
        &mut self,
        account: String,
        request: u64,
        operation: MonitorOperation,
        error: MonitorError,
    ) {
        self.receipt(account, request, operation, Err(error), self.persistence);
    }
    fn start(&mut self, account: &str, request: u64, epoch: u64) -> Result<(), MonitorError> {
        if epoch != self.sync_actual_stop() {
            return Err(MonitorError::Cancelled);
        }
        if !self.loaded || self.job.is_some() {
            return Err(MonitorError::Busy);
        }
        let snapshot = self.store.snapshot(account).map_err(store_error)?;
        if !snapshot.preferences.enabled {
            return Err(MonitorError::Disabled);
        }
        let entry = self
            .entries
            .get_mut(account)
            .ok_or(MonitorError::MissingAccount)?;
        if entry
            .stored
            .retry_authority
            .as_ref()
            .is_some_and(|a| !a.matches(&snapshot))
        {
            entry.stored.retry_after_paused = false;
            entry.stored.retry_after_unix_ms = None;
            entry.stored.retry_authority = None;
        }
        if entry.stored.retry_after_paused
            || retry_wait(&entry.stored).is_none_or(|wait| !wait.is_zero())
        {
            return Err(MonitorError::RateLimited);
        }
        if entry
            .last_started
            .is_some_and(|t| t.elapsed() < Duration::from_secs(60))
        {
            return Err(MonitorError::Busy);
        }
        if request == 0
            && ((self.session_paused && !self.resumed.contains(account))
                || !entry.stored.policy.enabled
                || entry.paused.is_some()
                || self.gate.pending_off(account)
                || !entry.stored.authority.matches(&snapshot))
        {
            return Err(MonitorError::Cancelled);
        }
        let authority = Authority::capture(&snapshot);
        let cancel = Arc::new(NetworkHttpCancel::new());
        let gateway = self.gateway.clone();
        let metadata = self.metadata.clone();
        let account_owned = account.to_owned();
        let requested = authority.clone();
        let cancelling = cancel.clone();
        let stop_epoch = self.stop_epoch.clone();
        {
            let mut state = self.gate.0.lock().map_err(|_| MonitorError::Storage)?;
            if state.off_reservations.get(account).is_some_and(|r| r.off) {
                return Err(MonitorError::Cancelled);
            }
            state.active = Some((account.to_owned(), cancel.clone()));
        }

        let worker = std::thread::Builder::new()
            .name("minicon-account-metadata".into())
            .spawn(move || {
                gateway.read_account_metadata_retry(
                    &account_owned,
                    &requested,
                    &metadata,
                    &cancelling,
                    &stop_epoch,
                    epoch,
                )
            });
        let worker = match worker {
            Ok(worker) => worker,
            Err(_) => {
                if let Ok(mut state) = self.gate.0.lock() {
                    state.active = None;
                }
                return Err(MonitorError::WorkerUnavailable);
            }
        };
        entry.last_started = Some(Instant::now());
        self.job = Some(Job {
            account: account.to_owned(),
            request,
            epoch,
            authority,
            cancel,
            worker,
        });
        Ok(())
    }
    pub(crate) fn stop(&mut self) {
        let current = self.stop_epoch.load(std::sync::atomic::Ordering::Acquire);
        self.observed_stop_epoch = current;
        self.pause_for_epoch(current);
    }
    /// No worker is dropped/detached: cancellation uses the native stream handle,
    /// then the actual finite HTTP worker is joined on the background Owner only.
    pub(crate) fn shutdown(&mut self) {
        self.stop();
        if let Some(job) = self.job.take() {
            job.cancel.cancel();
            let _ = job.worker.join();
        }
        if self.loaded
            && self.disk_pending
            && self
                .gateway
                .submit_monitor_snapshot(self.snapshot())
                .is_ok()
        {
            self.disk_pending = false;
        }
    }
    pub(crate) fn tick(
        &mut self,
        epoch: u64,
        incompatible_account: Option<&str>,
        blocked: bool,
    ) -> SharedMonitorView {
        let current_epoch = self.sync_actual_stop();
        let allow_due = epoch == current_epoch;
        let epoch = current_epoch;
        self.load_disk();
        self.drain_acknowledged();
        if self.last_sync.elapsed() >= Duration::from_secs(1) {
            self.sync_accounts();
            self.last_sync = Instant::now();
        }
        if self
            .job
            .as_ref()
            .is_some_and(|job| job.worker.is_finished())
        {
            let job = self.job.take().expect("completed metadata job");
            let outcome = job
                .worker
                .join()
                .unwrap_or_else(|_| MetadataRead::plain(Err(MonitorError::WorkerUnavailable)));
            let grant_stamp = outcome.grant_stamp;
            let retry_after = if job.epoch == epoch && !job.cancel.is_cancelled() {
                outcome.retry_after
            } else {
                RetryAfter::Missing
            };
            let result = outcome.result;
            if let Ok(mut gate) = self.gate.0.lock() {
                gate.active = None;
            }
            let result = if job.epoch != epoch || job.cancel.is_cancelled() {
                Err(MonitorError::Cancelled)
            } else {
                result
            };
            let mut accepted = Err(MonitorError::StaleAuthority);
            let store = self.store.clone();
            let checked =
                store.with_snapshot_and_current_grant(&job.account, |snapshot, current_grant| {
                    if !job.authority.matches(&snapshot) {
                        return;
                    }
                    let Some(entry) = self.entries.get_mut(&job.account) else {
                        return;
                    };
                    let needs_stamp = !matches!(
                        &result,
                        Err(MonitorError::Cancelled | MonitorError::WorkerUnavailable)
                    );
                    if needs_stamp
                        && !grant_stamp
                            .as_ref()
                            .is_some_and(|stamp| stamp.matches(&current_grant))
                    {
                        // A dated rejection from an older access token is not an
                        // authorization state for the newly refreshed grant.
                        entry.fresh_this_run = false;
                        entry.stored.attempt = Some(Attempt {
                            request: job.request,
                            finished_unix_ms: now_ms(),
                            result: Err(MonitorError::StaleAuthority),
                        });
                        return;
                    }
                    accepted = result;
                    match &accepted {
                        Ok(reading) => {
                            entry.stored.reading_authority = Some(job.authority.clone());
                            entry.stored.reading = Some(reading.clone());
                            entry.fresh_this_run = true;
                            entry.observed_instant = Some(Instant::now());
                            entry.failures = 0;
                            // A successful manual read is not an automatic-monitor Resume.
                            if job.request == 0 {
                                entry.paused = None;
                            }
                        }
                        Err(error) => {
                            entry.fresh_this_run = false;
                            entry.failures = entry.failures.saturating_add(1).min(4);
                            if matches!(error, MonitorError::NeedsAuth | MonitorError::Forbidden) {
                                entry.paused = Some(PauseReason::NeedsAuth);
                                entry.stored.auto_needs_auth = true;
                            }
                        }
                    }
                    entry.stored.attempt = Some(Attempt {
                        request: job.request,
                        finished_unix_ms: now_ms(),
                        result: accepted.as_ref().map(|_| ()).map_err(|e| *e),
                    });
                    entry.next = Instant::now()
                        + Duration::from_secs(
                            INTERVAL_SECONDS.saturating_mul(1u64 << entry.failures),
                        );
                    entry.stored.retry_after_unix_ms = match retry_after {
                        RetryAfter::NotBefore(unix) => Some(unix),
                        _ => None,
                    };
                    entry.stored.retry_after_paused = matches!(retry_after, RetryAfter::Paused);
                    entry.stored.retry_authority =
                        if matches!(retry_after, RetryAfter::NotBefore(_) | RetryAfter::Paused) {
                            Some(job.authority.clone())
                        } else {
                            None
                        };
                    if let Some(wait) = retry_wait(&entry.stored) {
                        entry.next = entry.next.max(Instant::now() + wait);
                    } else {
                        entry.stored.retry_after_paused = true;
                    }
                });
            if checked.is_err() {
                accepted = Err(MonitorError::StaleAuthority);
            }
            self.changed();
            if job.request != 0 {
                self.receipt(
                    job.account,
                    job.request,
                    MonitorOperation::Refresh,
                    accepted.map(|_| ()),
                    self.visible_persistence(),
                );
            }
        }
        let no_pending = self
            .gate
            .0
            .lock()
            .is_ok_and(|s| s.reservations.is_empty() && s.off_reservations.is_empty());
        if self.loaded && self.job.is_none() && no_pending && !blocked && allow_due {
            let due = self
                .entries
                .iter()
                .find(|(id, e)| {
                    e.stored.policy.enabled
                        && (!self.session_paused || self.resumed.contains(*id))
                        && e.paused.is_none()
                        && !e.stored.retry_after_paused
                        && retry_wait(&e.stored).is_some_and(|wait| wait.is_zero())
                        && e.next <= Instant::now()
                        && Some(id.as_str()) != incompatible_account
                })
                .map(|(id, _)| id.clone());
            if let Some(account) = due
                && let Err(error) = self.start(&account, 0, epoch)
                && let Some(entry) = self.entries.get_mut(&account)
            {
                entry.next = Instant::now() + Duration::from_secs(INTERVAL_SECONDS);
                if matches!(error, MonitorError::NeedsAuth | MonitorError::Disabled) {
                    entry.paused = Some(PauseReason::NeedsAuth);
                }
            }
        }
        if self.disk_pending
            && self.loaded
            && self
                .gateway
                .submit_monitor_snapshot(self.snapshot())
                .is_ok()
        {
            self.disk_pending = false;
        }
        self.view()
    }
    fn visible_persistence(&self) -> Persistence {
        if self.disk_pending
            || !self.policy_wait.is_empty()
            || self.revision > self.completed_revision
        {
            Persistence::Pending
        } else {
            self.persistence
        }
    }
    fn view(&self) -> SharedMonitorView {
        let now = now_ms();
        let mut rows = Vec::new();
        for (account, e) in &self.entries {
            let current = self.store.snapshot(account).ok();
            let freshness = if e
                .stored
                .reading_authority
                .as_ref()
                .is_some_and(|a| current.as_ref().is_none_or(|s| !a.matches(s)))
            {
                Freshness::PreviousAuthority
            } else if let Some(reading) = &e.stored.reading {
                match now.and_then(|n| n.checked_sub(reading.observed_unix_ms)) {
                    None => Freshness::ClockUncertain,
                    Some(age)
                        if e.observed_instant.is_some_and(|t| {
                            age.abs_diff(u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX))
                                > 5000
                        }) =>
                    {
                        Freshness::ClockUncertain
                    }
                    Some(age) if e.fresh_this_run && age <= FRESH_SECONDS * 1000 => {
                        Freshness::Fresh
                    }
                    Some(_) => Freshness::Stale,
                }
            } else {
                Freshness::Never
            };
            let paused = if !e.stored.policy.enabled {
                Some(PauseReason::Off)
            } else if self.session_paused && !self.resumed.contains(account) {
                Some(PauseReason::Stop)
            } else if e.stored.retry_after_paused
                || retry_wait(&e.stored).is_none()
                || (e.paused.is_none() && e.failures > 0 && e.next > Instant::now())
            {
                Some(PauseReason::Backoff)
            } else {
                e.paused
            };
            rows.push(AccountMonitorView {
                account: account.clone(),
                provider: e.stored.authority.provider,
                policy_revision: e.stored.policy_revision,
                policy: e.stored.policy.clone(),
                paused,
                active_request: self
                    .job
                    .as_ref()
                    .filter(|j| &j.account == account)
                    .map(|j| j.request),
                next_due_unix_ms: if matches!(paused,None|Some(PauseReason::Backoff))&&!e.stored.retry_after_paused&&retry_wait(&e.stored).is_some() {
                    now.and_then(|n| {
                        n.checked_add(
                            u64::try_from(
                                e.next.saturating_duration_since(Instant::now()).max(retry_wait(&e.stored).unwrap_or_default()).as_millis(),
                            )
                            .ok()?,
                        )
                    })
                } else {
                    None
                },
                last_success: e.stored.reading.clone(),
                freshness,
                latest_attempt: e.stored.attempt.clone(),
                attention: matches!(freshness,Freshness::Stale|Freshness::PreviousAuthority|Freshness::ClockUncertain)
                    || e.stored.attempt.as_ref().is_some_and(|a|matches!(a.result,Err(MonitorError::NeedsAuth|MonitorError::Forbidden)))
                    || e.stored.reading.as_ref().is_some_and(|r|attention(&e.stored.policy,r)
                        || matches!(&r.resources,Resources::OpenCodeGo(rows) if rows.iter().any(|m|m.limited))),
                wallet_capability: if e.stored.authority.provider == Provider::DeepSeek {
                    Capability::OfficialContract
                } else {
                    Capability::Unknown
                },
                allowance_capability: if e.stored.authority.provider == Provider::OpenCode {
                    Capability::OfficialContract
                } else {
                    Capability::Unknown
                },
            });
        }
        Arc::new(MonitorView {
            revision: self.revision,
            persistence: self.visible_persistence(),
            accounts: rows,
            receipts: self.receipts.values().cloned().collect(),
        })
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.shutdown();
    }
}
pub(crate) fn store_error(error: StoreError) -> MonitorError {
    match error {
        StoreError::Missing => MonitorError::MissingAccount,
        StoreError::Changed => MonitorError::StaleAuthority,
        StoreError::Storage | StoreError::InvalidRoot => MonitorError::Storage,
        _ => MonitorError::NeedsAuth,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn provider_monitor_actual_refresh_rate_limit_backs_off_without_auth_pause() {
        use crate::provider_auth::{Secret, TokenGrant, TokenLifecycle};
        use std::io::{Read, Write};
        let mut f = Fixture::new(false, false);
        let id = f
            .controller
            .store
            .insert(
                "Synthetic refresh cooldown",
                TokenGrant {
                    provider: Provider::OpenCode,
                    access_token: Secret::new("synthetic-expiring-access").unwrap(),
                    lifecycle: TokenLifecycle::ExpiringRefresh {
                        refresh_token: Secret::new("synthetic-expiring-refresh").unwrap(),
                        expires_at: Instant::now() + Duration::from_secs(5),
                    },
                    organization: Some("org_selected".into()),
                },
            )
            .unwrap()
            .id;
        f.controller.store.mark_ready(&id).unwrap();
        f.controller.sync_accounts();
        let entry = f.controller.entries.get_mut(&id).unwrap();
        entry.stored.policy.enabled = true;
        entry.stored.policy_revision = 1;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        f.controller.gateway.fixture_catalogue_upstream(format!(
            "http://{}/refresh-owner",
            listener.local_addr().unwrap()
        ));
        f.controller
            .gate
            .reserve(&id, 1, MonitorOperation::Refresh, false, 0)
            .unwrap();
        f.controller.refresh(id.clone(), 1, 0).unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut connection = loop {
            match listener.accept() {
                Ok((p, _)) => break p,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "controlled refresh did not arrive"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("controlled accept {e}"),
            }
        };
        connection.set_nonblocking(false).unwrap();
        connection
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        let mut headers = vec![];
        let mut byte = [0];
        while !headers.ends_with(b"\r\n\r\n") {
            assert!(headers.len() < 8192);
            connection.read_exact(&mut byte).unwrap();
            headers.push(byte[0]);
        }
        assert!(headers.starts_with(b"POST /refresh-owner "));
        let header_text = std::str::from_utf8(&headers).unwrap();
        let length = header_text
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        assert!(length > 0 && length <= 4096);
        let mut body = vec![0; length];
        connection.read_exact(&mut body).unwrap();
        write!(connection,"HTTP/1.1 429 Fixture\r\nRetry-After: 1200\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").unwrap();
        drop(connection);
        let view = loop {
            let view = f.controller.tick(0, None, true);
            if f.controller.job.is_none() {
                break view;
            }
            assert!(
                Instant::now() < deadline,
                "controlled refresh did not finish"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        let receipt = view
            .receipts
            .iter()
            .find(|r| r.account == id && r.request == 1)
            .unwrap();
        assert_eq!(
            receipt.result,
            Err(MonitorError::RateLimited),
            "temporary refresh rejection became NeedsAuth"
        );
        let entry = &f.controller.entries[&id];
        assert!(
            !entry.stored.auto_needs_auth,
            "refresh429 persisted an authorization pause"
        );
        assert!(
            entry.next.saturating_duration_since(Instant::now()) >= Duration::from_secs(1190),
            "refresh Retry-After did not reach Owner due-time"
        );
        let row = view.accounts.iter().find(|r| r.account == id).unwrap();
        assert_eq!(row.paused, Some(PauseReason::Backoff));
        assert_eq!(
            f.controller.store.snapshot(&id).unwrap().summary.state,
            crate::provider_accounts::AccountState::AccountReady
        );
        assert!(
            matches!(listener.accept(),Err(e)if e.kind()==std::io::ErrorKind::WouldBlock),
            "failed refresh issued a Go metadata GET"
        );
    }

    #[test]
    fn provider_monitor_actual_stop_epoch_window_blocks_due_and_keeps_explicit_resume() {
        let mut f = Fixture::new(false, false);
        let a = f.ids[0].clone();
        let entry = f.controller.entries.get_mut(&a).unwrap();
        entry.stored.policy.enabled = true;
        entry.stored.policy_revision = 1;
        entry.next = Instant::now();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        f.controller.gateway.fixture_catalogue_upstream(format!(
            "http://{}/stop-window",
            listener.local_addr().unwrap()
        ));
        // Same causal window as Owner's subject-tail tick: caller supplies the
        // NEW epoch before its outer loop has called monitor.stop().
        f.controller
            .stop_epoch
            .store(1, std::sync::atomic::Ordering::Release);
        let view = f.controller.tick(1, None, false);
        assert!(
            f.controller.job.is_none(),
            "new Stop epoch admitted automatic metadata before pause"
        );
        assert_eq!(
            view.accounts
                .iter()
                .find(|r| r.account == a)
                .unwrap()
                .paused,
            Some(PauseReason::Stop)
        );
        assert!(
            matches!(listener.accept(),Err(e)if e.kind()==std::io::ErrorKind::WouldBlock),
            "Stop window sent a metadata request"
        );
        f.controller
            .gate
            .reserve(&a, 1, MonitorOperation::Policy, false, 1)
            .unwrap();
        f.controller
            .policy(
                a.clone(),
                1,
                1,
                PolicyInput {
                    enabled: true,
                    thresholds: vec![],
                },
                1,
                true,
            )
            .unwrap();
        f.controller.stop(); // outer-loop acknowledgement of this SAME Stop
        assert_ne!(
            f.controller
                .view()
                .accounts
                .iter()
                .find(|r| r.account == a)
                .unwrap()
                .paused,
            Some(PauseReason::Stop),
            "same Stop acknowledgement erased explicit new-epoch Resume"
        );
        f.controller
            .stop_epoch
            .store(2, std::sync::atomic::Ordering::Release);
        f.controller.tick(2, None, true);
        assert_eq!(
            f.controller
                .view()
                .accounts
                .iter()
                .find(|r| r.account == a)
                .unwrap()
                .paused,
            Some(PauseReason::Stop)
        );
    }
    #[test]
    fn provider_monitor_actual_off_cancels_reserved_refresh_and_retains_both_exact_results() {
        use std::io::Read;
        let mut f = Fixture::new(false, false);
        let a = f.ids[0].clone();
        let entry = f.controller.entries.get_mut(&a).unwrap();
        entry.stored.policy.enabled = true;
        entry.stored.policy_revision = 1;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        f.controller.gateway.fixture_catalogue_upstream(format!(
            "http://{}/off-window",
            listener.local_addr().unwrap()
        ));
        f.controller
            .gate
            .reserve(&a, 1, MonitorOperation::Refresh, false, 0)
            .unwrap();
        f.controller.refresh(a.clone(), 1, 0).unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut peer = loop {
            match listener.accept() {
                Ok((p, _)) => break p,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "controlled reserved Refresh missing"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("controlled accept {e}"),
            }
        };
        peer.set_nonblocking(false).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
        let mut headers = vec![];
        let mut b = [0];
        while !headers.ends_with(b"\r\n\r\n") {
            assert!(headers.len() < 8192);
            peer.read_exact(&mut b).unwrap();
            headers.push(b[0]);
        }
        // Unlike the old mirror fixture, the actual Refresh reservation is
        // still present when the real priority Off producer runs.
        assert_eq!(
            f.controller
                .gate
                .reserve(&a, 2, MonitorOperation::Policy, true, 0),
            Ok(()),
            "explicit Off was rejected behind its own in-flight Refresh"
        );
        assert!(
            f.controller.job.as_ref().unwrap().cancel.is_cancelled(),
            "Off did not cancel the reserved metadata worker"
        );
        f.controller
            .policy(a.clone(), 2, 1, PolicyInput::default(), 0, false)
            .unwrap();
        drop(peer);
        let view = loop {
            let view = f.controller.tick(0, None, true);
            if view.receipts.len() == 2 {
                break view;
            }
            assert!(Instant::now() < deadline, "Refresh/Off terminal missing");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(
            view.receipts
                .iter()
                .any(|r| r.request == 1 && r.result == Err(MonitorError::Cancelled)),
            "Off lost original Refresh terminal"
        );
        assert!(
            view.receipts
                .iter()
                .any(|r| r.request == 2 && r.result == Ok(())),
            "Off lost its own persisted terminal"
        );
        assert!(
            !view
                .accounts
                .iter()
                .find(|r| r.account == a)
                .unwrap()
                .policy
                .enabled
        );
        let off = view
            .receipts
            .iter()
            .find(|r| r.request == 2)
            .unwrap()
            .clone();
        f.controller.gate.acknowledge(&[off]);
        f.controller.tick(0, None, true);
        assert_eq!(f.controller.view().receipts.len(), 1);
        assert_eq!(
            f.controller.view().receipts[0].request,
            1,
            "Off acknowledgement deleted Refresh terminal"
        );
        assert_eq!(
            f.controller
                .gate
                .reserve(&a, 3, MonitorOperation::Refresh, false, 0),
            Err(MonitorError::Busy)
        );
    }
    #[test]
    fn provider_monitor_actual_old_token_401_cannot_pause_refreshed_same_generation_grant() {
        use crate::provider_auth::{Secret, TokenGrant, TokenLifecycle};
        use std::io::{Read, Write};
        let mut f = Fixture::new(false, false);
        let a = f.ids[0].clone();
        f.controller.store.mark_ready(&a).unwrap();
        f.controller
            .entries
            .get_mut(&a)
            .unwrap()
            .stored
            .policy
            .enabled = true;
        f.controller
            .entries
            .get_mut(&a)
            .unwrap()
            .stored
            .policy_revision = 1;
        let reading =
            parse_response(Provider::DeepSeek, &wallet("9.75"), now_ms().unwrap()).unwrap();
        f.complete(&a, 0, Ok(reading.clone()));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        f.controller.gateway.fixture_catalogue_upstream(format!(
            "http://{}/old-grant",
            listener.local_addr().unwrap()
        ));
        f.controller
            .gate
            .reserve(&a, 1, MonitorOperation::Refresh, false, 0)
            .unwrap();
        f.controller.refresh(a.clone(), 1, 0).unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut peer = loop {
            match listener.accept() {
                Ok((p, _)) => break p,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "controlled old-token request missing"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("controlled accept {e}"),
            }
        };
        peer.set_nonblocking(false).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
        let mut headers = vec![];
        let mut byte = [0];
        while !headers.ends_with(b"\r\n\r\n") {
            assert!(headers.len() < 8192);
            peer.read_exact(&mut byte).unwrap();
            headers.push(byte[0]);
        }
        assert!(
            String::from_utf8(headers)
                .unwrap()
                .to_ascii_lowercase()
                .contains("x-dsh-auth-token: synthetic-a")
        );
        let before = f.controller.store.snapshot(&a).unwrap();
        f.controller
            .store
            .replace_grant_if_current(
                &a,
                &before.generation,
                TokenGrant {
                    provider: Provider::DeepSeek,
                    access_token: Secret::new("synthetic-fresh-access").unwrap(),
                    lifecycle: TokenLifecycle::OpaqueUntilRejected,
                    organization: None,
                },
            )
            .unwrap();
        assert_eq!(
            f.controller.store.snapshot(&a).unwrap().generation,
            before.generation,
            "fixture must isolate ordinary same-generation refresh"
        );
        write!(
            peer,
            "HTTP/1.1 401 Fixture\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        )
        .unwrap();
        drop(peer);
        loop {
            f.controller.tick(0, None, true);
            if f.controller.job.is_none() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "controlled old-token completion missing"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let view = f.controller.view();
        let receipt = view
            .receipts
            .iter()
            .find(|r| r.account == a && r.request == 1)
            .unwrap();
        assert_eq!(
            receipt.result,
            Err(MonitorError::StaleAuthority),
            "old grant rejection became current authorization failure"
        );
        let row = view.accounts.iter().find(|r| r.account == a).unwrap();
        assert_eq!(
            row.latest_attempt.as_ref().unwrap().result,
            Err(MonitorError::StaleAuthority)
        );
        assert_eq!(row.last_success.as_ref(), Some(&reading));
        assert_ne!(
            row.paused,
            Some(PauseReason::NeedsAuth),
            "old token rejection paused new grant"
        );
        assert!(
            !f.controller.entries[&a].stored.auto_needs_auth,
            "old token rejection persisted new-grant pause"
        );
        assert_eq!(
            f.controller.store.snapshot(&a).unwrap().summary.state,
            crate::provider_accounts::AccountState::AccountReady
        );
        assert_eq!(
            f.controller
                .store
                .load_grant_for_generation(&a, &before.generation)
                .unwrap()
                .access_token
                .expose(),
            "synthetic-fresh-access"
        );
    }
    #[test]
    fn provider_monitor_retry_after_legal_dates_seconds_and_finite_rejections() {
        let until = 784111777000u64;
        let now = until - 120000;
        for raw in [
            "120",
            "Sun, 06 Nov 1994 08:49:37 GMT",
            "Sunday, 06-Nov-94 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994",
        ] {
            assert_eq!(
                parse_retry_after(&[("Retry-After".into(), raw.into())], Some(now)),
                RetryAfter::NotBefore(until),
                "legal Retry-After was not honored: {raw}"
            );
        }
        for raw in [
            "-1",
            "+1",
            "1.5",
            "NaN",
            "Mon, 06 Nov 1994 08:49:37 GMT",
            "Sun, 31 Feb 1994 08:49:37 GMT",
        ] {
            assert_eq!(
                parse_retry_after(&[("Retry-After".into(), raw.into())], Some(now)),
                RetryAfter::Invalid
            );
        }
        assert_eq!(
            parse_retry_after(
                &[("retry-after".into(), "9999999999999999999999999999".into())],
                Some(now)
            ),
            RetryAfter::Paused,
            "oversized server delay silently became an earlier retry"
        );
        assert_eq!(
            parse_retry_after(
                &[
                    ("retry-after".into(), "120".into()),
                    ("Retry-After".into(), "240".into())
                ],
                Some(now)
            ),
            RetryAfter::Paused
        );
        assert_eq!(
            parse_retry_after(&[("retry-after".into(), "120".into())], None),
            RetryAfter::Paused
        );
    }
    fn response(value: serde_json::Value) -> NetworkHttpResponse {
        NetworkHttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&value).unwrap(),
            truncated: false,
        }
    }
    fn wallet(balance: &str) -> NetworkHttpResponse {
        response(json!({"code":0,"data":{"biz_code":0,"biz_data":{
            "normal_wallets":[{"currency":"CNY","balance":balance}],"bonus_wallets":[]}}}))
    }
    #[test]
    fn provider_monitor_wallet_decimal_preserves_official_lexical_forms_without_rounding() {
        for raw in [
            ".5",
            "1.",
            "0E-16",
            "0001.500e+2",
            "9007199254740993",
            "-0.000000000001",
        ] {
            let reading = parse_response(Provider::DeepSeek, &wallet(raw), 123).unwrap();
            let Resources::DeepSeekWallets(rows) = reading.resources else {
                panic!("wrong resource")
            };
            assert_eq!(
                rows[0].balance.0, raw,
                "official decimal lexeme was changed"
            );
        }
        assert_eq!(
            decimal_cmp(
                &Decimal("9007199254740993".into()),
                &Decimal("9007199254740992".into())
            ),
            Some(std::cmp::Ordering::Greater)
        );
        for raw in ["", "NaN", "Infinity", "+1", " 1", "1e9999999", "1.2.3"] {
            assert_eq!(
                parse_response(Provider::DeepSeek, &wallet(raw), 123),
                Err(MonitorError::Malformed)
            );
        }
    }
    #[test]
    fn provider_monitor_go_allowance_is_not_cash_and_keeps_percent_above_100() {
        let meter =
            json!({"status":"rate-limited","percent":125.5,"resetsAt":"2026-10-26T01:00:00Z"});
        let reading = parse_response(
            Provider::OpenCode,
            &response(json!({"usage":{
            "rolling":meter,"weekly":meter,"monthly":meter}})),
            123,
        )
        .unwrap();
        let Resources::OpenCodeGo(rows) = reading.resources else {
            panic!("Go quota was represented as cash")
        };
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].used_percent.0, "125.5");
        assert!(rows.iter().all(|r| r.limited));
    }
    #[test]
    fn provider_monitor_auth_business_error_and_partial_payload_never_become_zero() {
        assert_eq!(
            parse_response(Provider::DeepSeek, &response(json!({"code":40003})), 123),
            Err(MonitorError::NeedsAuth)
        );
        let mut forbidden = wallet("5");
        forbidden.status = 403;
        assert_eq!(
            parse_response(Provider::DeepSeek, &forbidden, 123),
            Err(MonitorError::Forbidden)
        );
        assert_eq!(
            parse_response(
                Provider::DeepSeek,
                &response(json!({"code":0,"data":{"biz_code":0,"biz_data":{}}})),
                123
            ),
            Err(MonitorError::Malformed)
        );
        let mut duplicate = wallet("5");
        duplicate.body=serde_json::to_vec(&json!({"code":0,"data":{"biz_code":0,"biz_data":{
            "normal_wallets":[{"currency":"USD","balance":"5"},{"currency":"USD","balance":"6"}],"bonus_wallets":[]}}})).unwrap();
        assert_eq!(
            parse_response(Provider::DeepSeek, &duplicate, 123),
            Err(MonitorError::Malformed)
        );
    }
    #[test]
    fn provider_monitor_thresholds_use_exact_resource_and_bounded_unique_policy() {
        let reading = parse_response(Provider::DeepSeek, &wallet(".5"), 123).unwrap();
        let threshold = Threshold::WalletBelow {
            kind: WalletKind::Normal,
            currency: Currency::Cny,
            value: Decimal("1".into()),
        };
        let mut policy = PolicyInput {
            enabled: true,
            thresholds: vec![threshold.clone()],
        };
        assert!(valid_policy(&policy) && attention(&policy, &reading));
        policy.thresholds = vec![Threshold::WalletBelow {
            kind: WalletKind::Bonus,
            currency: Currency::Cny,
            value: Decimal("999".into()),
        }];
        assert!(
            !attention(&policy, &reading),
            "normal wallet was compared to bonus threshold"
        );
        policy.thresholds = vec![threshold.clone(), threshold];
        assert!(!valid_policy(&policy));
    }
    #[test]
    fn provider_monitor_pending_gate_receipts_ack_exact_account_and_request() {
        let gate = MonitorGate::default();
        let a = "a".repeat(32);
        let b = "b".repeat(32);
        gate.reserve(&a, 1, MonitorOperation::Refresh, false, 0)
            .unwrap();
        assert_eq!(
            gate.reserve(&a, 2, MonitorOperation::Refresh, false, 0),
            Err(MonitorError::Busy)
        );
        gate.reserve(&b, 3, MonitorOperation::Policy, true, 0)
            .unwrap();
        let receipt = MonitorReceipt {
            account: a.clone(),
            request: 1,
            operation: MonitorOperation::Refresh,
            result: Ok(()),
            persistence: Persistence::Ready,
        };
        gate.acknowledge(&[MonitorReceipt {
            request: 99,
            ..receipt.clone()
        }]);
        assert!(gate.matches(&a, 1, MonitorOperation::Refresh, 0));
        gate.acknowledge(&[receipt]);
        assert!(!gate.matches(&a, 1, MonitorOperation::Refresh, 0));
        assert!(gate.matches(&b, 3, MonitorOperation::Policy, 0));
        gate.withdraw(&b, 99);
        assert!(gate.pending_off(&b));
    }
    #[test]
    fn provider_monitor_off_fence_cancels_only_matching_metadata() {
        let gate = MonitorGate::default();
        let a = "a".repeat(32);
        let b = "b".repeat(32);
        let cancel = Arc::new(NetworkHttpCancel::new());
        gate.0.lock().unwrap().active = Some((a.clone(), cancel.clone()));
        gate.reserve(&b, 1, MonitorOperation::Policy, true, 0)
            .unwrap();
        assert!(!cancel.is_cancelled());
        gate.reserve(&a, 2, MonitorOperation::Policy, true, 0)
            .unwrap();
        assert!(cancel.is_cancelled());
    }

    struct Fixture {
        root: std::path::PathBuf,
        controller: Controller,
        ids: Vec<String>,
    }
    #[test]
    fn provider_monitor_manual_success_stays_pending_until_matching_writer_completion() {
        let mut f = Fixture::new(false, false);
        let a = f.ids[0].clone();
        f.controller
            .gate
            .reserve(&a, 1, MonitorOperation::Refresh, false, 0)
            .unwrap();
        let reading =
            parse_response(Provider::DeepSeek, &wallet("17.25"), now_ms().unwrap()).unwrap();
        f.complete(&a, 1, Ok(reading));
        let view = f.controller.view();
        assert_eq!(
            view.persistence,
            Persistence::Pending,
            "manual success claimed old writer Ready before new revision ack"
        );
        assert_eq!(
            view.receipts[0].persistence,
            Persistence::Pending,
            "manual receipt claimed unacknowledged bytes persisted"
        );
        let snapshot = f.controller.snapshot();
        let revision = snapshot.revision;
        // Actual production completion consumer; a stale Ready is not an ack.
        f.controller.accept_disk_view(DiskView {
            snapshot: Arc::new(MonitorSnapshot::default()),
            state: Persistence::Ready,
            completed_revision: 0,
        });
        assert_eq!(f.controller.view().persistence, Persistence::Pending);
        f.controller.accept_disk_view(DiskView {
            snapshot,
            state: Persistence::Ready,
            completed_revision: revision,
        });
        assert_eq!(f.controller.view().persistence, Persistence::Ready);
    }
    #[test]
    fn provider_monitor_restart_disk_ack_is_the_loaded_snapshot_revision() {
        let mut f = Fixture::new(false, false);
        f.controller.revision = 17;
        let root = f.root.join("restart-ack");
        std::fs::create_dir_all(&root).unwrap();
        filesystem::protect_private_directory(&root).unwrap();
        let slot = DiskSlot::default();
        assert!(slot.open(&root, true));
        slot.submit(f.controller.snapshot()).unwrap();
        slot.flush(&root, true);
        let restarted = DiskSlot::default();
        assert!(restarted.open(&root, true));
        let loaded = restarted.view().unwrap();
        assert_eq!(
            loaded.completed_revision, 17,
            "restart invented an unacknowledged revision for durable snapshot"
        );
        let mut controller = Controller::new(
            f.controller.store.clone(),
            f.controller.gateway.clone(),
            f.controller.metadata.clone(),
            MonitorGate::default(),
            Arc::new(std::sync::atomic::AtomicU64::new(0)),
        );
        controller.accept_disk_view(loaded);
        assert_eq!(
            controller.view().persistence,
            Persistence::Ready,
            "loaded current disk snapshot stayed Pending forever"
        );
        drop(controller);
    }
    #[test]
    fn provider_monitor_actual_retry_after_defers_automatic_and_manual_metadata() {
        use std::io::{Read, Write};
        let mut f = Fixture::new(false, false);
        let a = f.ids[0].clone();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        f.controller.gateway.fixture_catalogue_upstream(format!(
            "http://{}/rate-limit",
            listener.local_addr().unwrap()
        ));
        f.controller
            .gate
            .reserve(&a, 1, MonitorOperation::Refresh, false, 0)
            .unwrap();
        f.controller.refresh(a.clone(), 1, 0).unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut peer = loop {
            match listener.accept() {
                Ok((p, _)) => break p,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "controlled metadata request missing"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("controlled accept {e}"),
            }
        };
        peer.set_nonblocking(false).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
        let mut headers = vec![];
        let mut byte = [0];
        while !headers.ends_with(b"\r\n\r\n") {
            assert!(headers.len() < 8192);
            peer.read_exact(&mut byte).unwrap();
            headers.push(byte[0]);
        }
        write!(peer,"HTTP/1.1 503 Fixture\r\nRetry-After: 1200\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").unwrap();
        drop(peer);
        loop {
            f.controller.tick(0, None, true);
            if f.controller.job.is_none() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "controlled metadata completion missing"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let entry = &f.controller.entries[&a];
        assert!(
            entry.next.saturating_duration_since(Instant::now()) >= Duration::from_secs(1190),
            "actual Retry-After header did not reach due-time consumer"
        );
        assert!(
            entry.stored.retry_after_unix_ms.is_some(),
            "retry boundary cannot survive restart"
        );
        // Isolate the server boundary from the independent minimum60s gate,
        // without sleeping or altering the actual response/header producer.
        f.controller.entries.get_mut(&a).unwrap().last_started =
            Some(Instant::now() - Duration::from_secs(61));
        assert_eq!(
            f.controller.start(&a, 2, 0),
            Err(MonitorError::RateLimited),
            "manual refresh ignored active server not-before"
        );
    }
    impl Fixture {
        fn new(corrupt_usage: bool, corrupt_monitor: bool) -> Self {
            use crate::provider_auth::{Secret, TokenGrant, TokenLifecycle};
            let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "minicon-monitor-owner-{}",
                minicon_platform::entropy::secure_random_array::<8>()
                    .unwrap()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            ));
            std::fs::create_dir_all(root.join("usage")).unwrap();
            if corrupt_usage {
                filesystem_publish::write_private_atomic(
                    &root.join("usage/history.json"),
                    b"corrupt",
                )
                .unwrap();
            }
            if corrupt_monitor {
                filesystem_publish::write_private_atomic(
                    &root.join("usage/account-monitoring.json"),
                    b"corrupt",
                )
                .unwrap();
            }
            let store = Arc::new(AccountStore::new(root.join("accounts")).unwrap());
            let mut ids = vec![];
            for token in ["synthetic-a", "synthetic-b"] {
                ids.push(
                    store
                        .insert(
                            "Synthetic owner fixture",
                            TokenGrant {
                                provider: Provider::DeepSeek,
                                access_token: Secret::new(token).unwrap(),
                                lifecycle: TokenLifecycle::OpaqueUntilRejected,
                                organization: None,
                            },
                        )
                        .unwrap()
                        .id,
                );
            }
            let gateway = Gateway::new(
                store.clone(),
                crate::provider_server::Config::fresh().unwrap(),
                None,
            )
            .unwrap();
            gateway.install_usage(root.join("usage"));
            let mut controller = Controller::new(
                store,
                gateway,
                LoginMetadata {
                    version: env!("CARGO_PKG_VERSION").into(),
                    locale: "en-US".into(),
                    timezone_offset_seconds: 0,
                },
                MonitorGate::default(),
                Arc::new(std::sync::atomic::AtomicU64::new(0)),
            );
            let deadline = Instant::now() + Duration::from_secs(4);
            while !controller.loaded {
                controller.tick(0, None, true);
                assert!(Instant::now() < deadline, "monitor cache never loaded");
                std::thread::sleep(Duration::from_millis(5));
            }
            Self {
                root,
                controller,
                ids,
            }
        }
        fn complete(&mut self, account: &str, request: u64, result: Result<Reading, MonitorError>) {
            let current = self
                .controller
                .store
                .load_grant_for_generation(
                    account,
                    &self.controller.store.snapshot(account).unwrap().generation,
                )
                .unwrap();
            let stamp = GrantStamp::capture(&current);
            let worker = std::thread::spawn(move || MetadataRead {
                result,
                retry_after: RetryAfter::Missing,
                grant_stamp: Some(stamp),
            });
            let deadline = Instant::now() + Duration::from_secs(1);
            while !worker.is_finished() {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            let authority = Authority::capture(&self.controller.store.snapshot(account).unwrap());
            self.controller.job = Some(Job {
                account: account.to_owned(),
                request,
                epoch: 0,
                authority,
                cancel: Arc::new(NetworkHttpCancel::new()),
                worker,
            });
            self.controller.tick(0, None, true);
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.controller.shutdown();
            let close = self.controller.gateway.usage_close_handle();
            if let Ok(placeholder) = Gateway::new(
                self.controller.store.clone(),
                crate::provider_server::Config::fresh().unwrap(),
                None,
            ) {
                let old = std::mem::replace(&mut self.controller.gateway, placeholder);
                drop(old);
                if let Some(close) = close {
                    let _ = close.wait_owner();
                }
                let _ = std::fs::remove_dir_all(&self.root);
            }
        }
    }
    #[test]
    fn provider_monitor_owner_preserves_reading_errors_and_both_unacknowledged_receipts() {
        let mut f = Fixture::new(false, false);
        let a = f.ids[0].clone();
        let b = f.ids[1].clone();
        let reading =
            parse_response(Provider::DeepSeek, &wallet("12.75"), now_ms().unwrap()).unwrap();
        f.complete(&a, 0, Ok(reading.clone()));
        f.controller
            .gate
            .reserve(&a, 1, MonitorOperation::Refresh, false, 0)
            .unwrap();
        f.complete(&a, 1, Err(MonitorError::Forbidden));
        f.controller
            .gate
            .reserve(&b, 2, MonitorOperation::Refresh, false, 0)
            .unwrap();
        f.complete(&b, 2, Ok(reading.clone()));
        let view = f.controller.view();
        assert_eq!(
            view.receipts.len(),
            2,
            "Pending coalescing lost an account terminal"
        );
        let row = view.accounts.iter().find(|r| r.account == a).unwrap();
        assert_eq!(
            row.last_success.as_ref(),
            Some(&reading),
            "metadata error replaced retained wallet with zero/missing"
        );
        assert_eq!(
            row.latest_attempt.as_ref().unwrap().result,
            Err(MonitorError::Forbidden)
        );
        assert_eq!(
            row.freshness,
            Freshness::Stale,
            "failed read kept the old wallet Fresh"
        );
        assert_eq!(
            f.controller.store.snapshot(&a).unwrap().summary.state,
            crate::provider_accounts::AccountState::CredentialStored
        );
        f.controller.gate.acknowledge(&view.receipts[..1]);
        f.controller.tick(0, None, true);
        assert_eq!(
            f.controller.view().receipts.len(),
            1,
            "ack dropped another account receipt"
        );
    }
    #[test]
    fn provider_monitor_manual_success_does_not_resume_stop_or_replace_policy_authority() {
        let mut f = Fixture::new(false, false);
        let a = f.ids[0].clone();
        let before = f.controller.entries[&a].stored.authority.clone();
        f.controller
            .entries
            .get_mut(&a)
            .unwrap()
            .stored
            .policy
            .enabled = true;
        f.controller.stop();
        let reading = parse_response(Provider::DeepSeek, &wallet("1"), now_ms().unwrap()).unwrap();
        f.complete(&a, 1, Ok(reading));
        assert!(
            f.controller.session_paused,
            "manual read resumed monitoring after Stop"
        );
        assert!(f.controller.entries[&a].stored.authority == before);
        assert_eq!(
            f.controller
                .view()
                .accounts
                .iter()
                .find(|r| r.account == a)
                .unwrap()
                .paused,
            Some(PauseReason::Stop)
        );
    }
    #[test]
    fn provider_monitor_existing_writer_keeps_usage_and_monitor_file_failures_independent() {
        let mut f = Fixture::new(true, false);
        f.controller.changed();
        f.controller.tick(0, None, true);
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            let view = f.controller.gateway.monitor_disk_view().unwrap();
            if view.completed_revision >= f.controller.revision {
                assert_eq!(
                    view.state,
                    Persistence::Ready,
                    "corrupt usage disabled independent monitor publication"
                );
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(f.root.join("usage/account-monitoring.json").is_file());
        let other = Fixture::new(false, true);
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            let summary = other.controller.gateway.usage_summary().unwrap();
            if filesystem::read_private_bounded(&other.root.join("usage/history.json"), 32 * 1024)
                .ok()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .is_some_and(|v| v["version"] == 1 && v["gaps"]["invalid_history"] == false)
            {
                break;
            }
            assert!(!summary.gaps.invalid_history);
            assert!(
                Instant::now() < deadline,
                "corrupt monitor disabled request history writer"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            other.controller.gateway.monitor_disk_view().unwrap().state,
            Persistence::Unavailable
        );
    }
    #[test]
    fn provider_monitor_policy_waits_for_exact_writer_revision_and_keeps_two_results() {
        let mut f = Fixture::new(false, false);
        for (i, a) in f.ids.clone().into_iter().enumerate() {
            let request = i as u64 + 1;
            f.controller
                .gate
                .reserve(&a, request, MonitorOperation::Policy, false, 0)
                .unwrap();
            f.controller
                .policy(
                    a,
                    request,
                    0,
                    PolicyInput {
                        enabled: true,
                        thresholds: vec![],
                    },
                    0,
                    true,
                )
                .unwrap();
        }
        // Controlled old writer completion enters the same production consumer;
        // it cannot acknowledge a later policy even if the disk thread races.
        f.controller.accept_disk_view(DiskView {
            snapshot: Arc::new(MonitorSnapshot::default()),
            state: Persistence::Ready,
            completed_revision: 0,
        });
        assert!(
            f.controller.receipts.is_empty(),
            "policy claimed persistence before writer completion"
        );
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            f.controller.tick(0, None, true);
            if f.controller.receipts.len() == 2 {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            f.controller
                .receipts
                .values()
                .all(|r| r.result == Ok(()) && r.persistence == Persistence::Ready)
        );
    }

    #[test]
    fn provider_monitor_stop_resume_is_account_local_and_restart_optin_is_authority_bound() {
        let mut f = Fixture::new(false, false);
        let a = f.ids[0].clone();
        let b = f.ids[1].clone();
        for id in [&a, &b] {
            let e = f.controller.entries.get_mut(id).unwrap();
            e.stored.policy.enabled = true;
            e.stored.policy_revision = 1;
        }
        f.controller.stop();
        f.controller
            .gate
            .reserve(&a, 1, MonitorOperation::Policy, false, 0)
            .unwrap();
        f.controller
            .policy(
                a.clone(),
                1,
                1,
                PolicyInput {
                    enabled: true,
                    thresholds: vec![],
                },
                0,
                true,
            )
            .unwrap();
        let view = f.controller.view();
        assert_ne!(
            view.accounts
                .iter()
                .find(|r| r.account == a)
                .unwrap()
                .paused,
            Some(PauseReason::Stop)
        );
        assert_eq!(
            view.accounts
                .iter()
                .find(|r| r.account == b)
                .unwrap()
                .paused,
            Some(PauseReason::Stop),
            "resuming A resumed B after Stop"
        );
        // Test the production decoder and actual private file, not a JSON-number
        // upgrade which could fabricate persisted opt-in or account authority.
        let snapshot = f.controller.snapshot();
        let slot = DiskSlot::default();
        assert!(!slot.open(&f.root.join("restart-cache"), false));
        std::fs::create_dir_all(f.root.join("restart-cache")).unwrap();
        assert!(slot.open(&f.root.join("restart-cache"), true));
        slot.submit(snapshot).unwrap();
        slot.flush(&f.root.join("restart-cache"), true);
        let reopened = DiskSlot::default();
        assert!(reopened.open(&f.root.join("restart-cache"), true));
        let restored = reopened.view().unwrap();
        assert!(restored.snapshot.accounts.iter().all(|a| a.policy.enabled));
        let before = f.controller.store.snapshot(&b).unwrap();
        f.controller
            .store
            .reconnect_if_current(
                &b,
                &before.generation,
                crate::provider_auth::TokenGrant {
                    provider: Provider::DeepSeek,
                    access_token: crate::provider_auth::Secret::new("synthetic-reconnected")
                        .unwrap(),
                    lifecycle: crate::provider_auth::TokenLifecycle::OpaqueUntilRejected,
                    organization: None,
                },
            )
            .unwrap();
        assert!(
            !restored
                .snapshot
                .accounts
                .iter()
                .find(|a| a.authority.account == b)
                .unwrap()
                .authority
                .matches(&f.controller.store.snapshot(&b).unwrap()),
            "reconnect reused an old persisted opt-in authority"
        );
    }

    #[test]
    fn provider_monitor_restored_optin_starts_finite_metadata_without_settings_or_resume() {
        use std::io::{Read, Write};
        let mut f = Fixture::new(false, false);
        let account = f.ids[0].clone();
        let entry = f.controller.entries.get_mut(&account).unwrap();
        entry.stored.policy.enabled = true;
        entry.stored.policy_revision = 1;
        f.controller.changed();
        let revision = f.controller.revision;
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            f.controller.tick(0, None, true);
            if f.controller
                .gateway
                .monitor_disk_view()
                .unwrap()
                .completed_revision
                >= revision
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            f.controller.gateway.monitor_disk_view().unwrap().state,
            Persistence::Ready
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        f.controller.gateway.fixture_catalogue_upstream(format!(
            "http://{}/monitor-only",
            listener.local_addr().unwrap()
        ));
        let mut restored = Controller::new(
            f.controller.store.clone(),
            f.controller.gateway.clone(),
            f.controller.metadata.clone(),
            MonitorGate::default(),
            Arc::new(std::sync::atomic::AtomicU64::new(0)),
        );
        restored.tick(0, None, false);
        assert!(
            restored.job.is_some(),
            "persisted opt-in did not reserve actual metadata work"
        );
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut peer = loop {
            match listener.accept() {
                Ok((p, _)) => break p,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "persisted explicit opt-in did not schedule actual metadata"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("controlled accept {e}"),
            }
        };
        peer.set_nonblocking(false).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
        let mut bytes = vec![];
        let mut b = [0];
        while !bytes.ends_with(b"\r\n\r\n") {
            assert!(bytes.len() < 8192);
            peer.read_exact(&mut b).unwrap();
            bytes.push(b[0]);
        }
        assert!(
            bytes.starts_with(b"GET /monitor-only "),
            "monitor scheduled a model request"
        );
        let body = wallet("7.25").body;
        write!(
            peer,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        peer.write_all(&body).unwrap();
        drop(peer);
        loop {
            restored.tick(0, None, true);
            if restored.job.is_none() {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let view = restored.view();
        let row = view.accounts.iter().find(|r| r.account == account).unwrap();
        assert_eq!(row.freshness, Freshness::Fresh);
        assert!(row.last_success.is_some());
        assert_eq!(
            row.latest_attempt.as_ref().unwrap().request,
            0,
            "automatic work forged a manual UI request"
        );
        restored.shutdown();
        drop(restored);
    }
}
