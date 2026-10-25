//! Product request accounting. No model response, grant or proxy value is retained.
use super::{Binding, BorrowedLease, ModelRoute, Provider, ServerError};
use crate::provider_protocol::usage::{InputSemantics, UsageSnapshot};
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
mod history;
pub(crate) use history::{UsageClose, UsageHistory, UsageSummary};

pub(super) const RECEIPT_BYTES: usize = 4096;
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum Purpose {
    Client,
    Probe,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum Outcome {
    Completed,
    Cancelled,
    AuthorityChanged,
    Auth,
    Capacity,
    Protocol,
    Transport,
    Internal,
    Interrupted,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum Delivery {
    NotApplicable,
    LocalWritesAccepted,
    Failed,
    Unknown,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum Coverage {
    Missing,
    Partial,
    Final,
    Invalid,
}
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthorityStamp {
    binding: Option<String>,
    credential: String,
    service: String,
    organization_epoch: String,
    organization_digest: String,
    service_uses_proxy: bool,
}
impl std::fmt::Debug for AuthorityStamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthorityStamp([REDACTED])")
    }
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Attribution {
    pub profile: String,
    pub account: String,
    pub alias: String,
    pub model: String,
    pub provider: Provider,
    pub protocol: InputSemantics,
    authority: AuthorityStamp,
}
impl Attribution {
    pub fn client(binding: &Binding, lease: &BorrowedLease<'_>, route: &ModelRoute) -> Self {
        Self::new(
            &binding.profile,
            &binding.account,
            &binding.alias,
            route,
            lease,
            Some(binding.generation.clone()),
        )
    }
    pub fn probe(
        profile: &str,
        alias: &str,
        lease: &BorrowedLease<'_>,
        route: &ModelRoute,
    ) -> Self {
        Self::new(profile, &lease.account, alias, route, lease, None)
    }
    fn new(
        profile: &str,
        account: &str,
        alias: &str,
        route: &ModelRoute,
        lease: &BorrowedLease<'_>,
        binding: Option<String>,
    ) -> Self {
        Self {
            profile: profile.into(),
            account: account.into(),
            alias: alias.into(),
            model: route.model_id.clone(),
            provider: route.provider,
            protocol: match route.protocol {
                super::WireProtocol::Chat => InputSemantics::ChatInclusive,
                super::WireProtocol::Messages => InputSemantics::MessagesBasePlusCache,
            },
            authority: AuthorityStamp {
                binding,
                credential: lease.generation.clone(),
                service: lease.preferences.service_generation.clone(),
                organization_epoch: lease.organization.epoch.clone(),
                organization_digest: lease.organization.digest.clone(),
                service_uses_proxy: matches!(
                    &lease.preferences.service,
                    super::NetworkPolicy::Proxy(_)
                ),
            },
        }
    }
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt {
    pub run_id: String,
    pub sequence: u64,
    pub purpose: Purpose,
    pub attribution: Attribution,
    pub started_unix_ms: Option<u64>,
    pub finished_unix_ms: Option<u64>,
    pub lease_duration_ms: u64,
    pub first_response_ms: Option<u64>,
    pub first_text_ms: Option<u64>,
    pub queue_wait_ms: Option<u64>,
    pub transport_dispatched: bool,
    pub transport_complete: bool,
    pub semantic_complete: bool,
    pub outcome: Outcome,
    pub delivery: Delivery,
    pub coverage: Coverage,
    pub usage: UsageSnapshot,
}
pub(super) fn now_ms() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()
}
fn millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}
pub(super) fn count(counter: &AtomicU64, overflow: &AtomicBool) {
    if counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
        .is_err()
    {
        overflow.store(true, Ordering::Relaxed);
    }
}
#[derive(Default)]
pub(super) struct SharedCounters {
    pub admitted: AtomicU64,
    pub terminal: AtomicU64,
    pub dropped: AtomicU64,
    pub oversized: AtomicU64,
    pub sequence: AtomicU64,
    pub rejected: AtomicU64,
    pub overflow: AtomicBool,
}
pub(super) struct Capture {
    sink: Option<history::TerminalSink>,
    record: Option<Receipt>,
    started: Option<Instant>,
    activated: bool,
}
impl Capture {
    pub fn unbound() -> Self {
        Self {
            sink: None,
            record: None,
            started: None,
            activated: false,
        }
    }
    pub fn activate(
        &mut self,
        history: Option<&UsageHistory>,
        purpose: Purpose,
        attribution: Attribution,
    ) {
        // A capture is activated only once, immediately after the real lease.
        if self.activated {
            return;
        }
        self.activated = true;
        let Some(history) = history else {
            return;
        };
        let Some((sink, sequence, run_id)) = history.begin() else {
            return;
        };
        self.started = Some(Instant::now());
        self.sink = Some(sink);
        let semantics = attribution.protocol;
        self.record = Some(Receipt {
            run_id,
            sequence,
            purpose,
            attribution,
            started_unix_ms: now_ms(),
            finished_unix_ms: None,
            lease_duration_ms: 0,
            first_response_ms: None,
            first_text_ms: None,
            queue_wait_ms: None,
            transport_dispatched: false,
            transport_complete: false,
            semantic_complete: false,
            outcome: Outcome::Interrupted,
            delivery: Delivery::Unknown,
            coverage: Coverage::Missing,
            usage: UsageSnapshot {
                tokens: crate::provider_protocol::usage::TokenSnapshot {
                    semantics,
                    ..Default::default()
                },
                ..Default::default()
            },
        });
    }
    pub fn bound(&self) -> bool {
        self.activated
    }
    pub fn elapsed(&self) -> Duration {
        self.started
            .map_or(Duration::ZERO, |started| started.elapsed())
    }
    pub fn dispatched(&mut self) {
        if let Some(record) = &mut self.record {
            record.transport_dispatched = true;
        }
    }
    pub fn transport_complete(&mut self) {
        if let Some(record) = &mut self.record {
            record.transport_complete = true;
        }
    }
    pub fn semantic_complete(&mut self) {
        if let Some(record) = &mut self.record {
            record.semantic_complete = true;
        }
    }
    pub fn observe(&mut self, usage: UsageSnapshot) {
        if let Some(record) = &mut self.record {
            record.usage = usage;
        }
    }
    pub fn timings(&mut self, first_response: Option<Duration>, first_text: Option<Duration>) {
        if let Some(record) = &mut self.record {
            record.first_response_ms = first_response.map(millis);
            record.first_text_ms = first_text.map(millis);
        }
    }
    pub fn lease_released(&mut self) {
        if let Some(started) = self.started.take()
            && let Some(record) = &mut self.record
        {
            record.lease_duration_ms = millis(started.elapsed());
        }
    }
    pub fn delivery_failed(&mut self) {
        if let Some(record) = &mut self.record {
            record.delivery = Delivery::Failed;
        }
    }
    pub fn finish(&mut self, result: Result<(), ServerError>, delivery: Delivery) {
        if let Some(record) = &mut self.record {
            record.outcome = match result {
                Ok(()) => Outcome::Completed,
                Err(ServerError::Cancelled) => Outcome::Cancelled,
                Err(ServerError::BindingChanged) => Outcome::AuthorityChanged,
                Err(
                    ServerError::NeedsLogin | ServerError::MissingAccount | ServerError::MissingKey,
                ) => Outcome::Auth,
                Err(ServerError::Busy) => Outcome::Capacity,
                Err(
                    ServerError::Protocol(_)
                    | ServerError::InvalidConfig
                    | ServerError::MissingModel,
                ) => Outcome::Protocol,
                Err(ServerError::Upstream | ServerError::Http(_)) => Outcome::Transport,
                Err(ServerError::Storage) => Outcome::Internal,
            };
            if record.delivery != Delivery::Failed {
                record.delivery = delivery;
            }
        }
        self.submit();
    }
    fn submit(&mut self) {
        let Some(mut record) = self.record.take() else {
            return;
        };
        record.finished_unix_ms = now_ms();
        if let Some(started) = self.started.take() {
            record.lease_duration_ms = millis(started.elapsed());
        }
        let tokens = &record.usage.tokens;
        record.coverage = if tokens.invalid || tokens.non_monotonic {
            Coverage::Invalid
        } else if tokens.input.is_some()
            && tokens.output.is_some()
            && record.transport_complete
            && record.semantic_complete
        {
            Coverage::Final
        } else if tokens.input.is_some()
            || tokens.output.is_some()
            || record.usage.billing.is_some()
        {
            Coverage::Partial
        } else {
            Coverage::Missing
        };
        if let Some(sink) = self.sink.take() {
            sink.submit(record);
        }
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.submit();
    }
}
