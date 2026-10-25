//! One finite private snapshot writer; no model request is ever replayed here.
use super::*;
use crate::provider_protocol::usage::{BillingSource, ExactDecimal, TokenSnapshot};
use minicon_platform::{filesystem, filesystem_publish};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        Condvar, Mutex,
        mpsc::{self, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
};
const QUEUE: usize = 256;
const DETAILS: usize = 10_000;
const DETAIL_BYTES: usize = 16 * 1024 * 1024;
const AGGREGATE_BYTES: usize = 8 * 1024 * 1024;
const SNAPSHOT_BYTES: usize = DETAIL_BYTES + AGGREGATE_BYTES + 65536;
const DAY_MS: u64 = 86_400_000;

/// Explicit local retained-history operation; no provider or model side effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UsageClearError {
    Busy,
    Unavailable,
    InvalidRequest,
    Storage,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UsageClearPublication {
    Durable,
    PublishedDurabilityUnconfirmed,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UsageClearReceipt {
    pub request: u64,
    pub result: Result<UsageClearPublication, UsageClearError>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct UsageClearView {
    pub pending_request: Option<u64>,
    pub receipt: Option<UsageClearReceipt>,
}
#[derive(Default)]
struct ClearSlot {
    last_request: u64,
    reserved: Option<u64>,
    queued: bool,
    receipt: Option<UsageClearReceipt>,
}
/// Non-producing capability: never retains a Sender, UsageHistory or Gateway.
#[derive(Clone, Default)]
pub(crate) struct UsageClearHandle(Arc<Mutex<ClearSlot>>);
fn clear_lock_error<T>(error: std::sync::TryLockError<T>) -> UsageClearError {
    match error {
        std::sync::TryLockError::WouldBlock => UsageClearError::Busy,
        std::sync::TryLockError::Poisoned(_) => UsageClearError::Unavailable,
    }
}
impl UsageClearHandle {
    pub fn next_request(&self) -> Result<u64, UsageClearError> {
        self.0
            .try_lock()
            .map_err(clear_lock_error)?
            .last_request
            .checked_add(1)
            .ok_or(UsageClearError::InvalidRequest)
    }
    pub fn reserve(&self, request: u64) -> Result<(), UsageClearError> {
        let mut slot = self.0.try_lock().map_err(clear_lock_error)?;
        if request == 0 || request <= slot.last_request {
            return Err(UsageClearError::InvalidRequest);
        }
        if slot.reserved.is_some() {
            return Err(UsageClearError::Busy);
        }
        slot.last_request = request;
        slot.reserved = Some(request);
        slot.queued = false;
        Ok(())
    }
    /// Only a failed original Actor enqueue may withdraw this unsubmitted intent.
    pub fn withdraw(&self, request: u64) {
        if let Ok(mut slot) = self.0.lock()
            && slot.reserved == Some(request)
            && !slot.queued
            && slot.receipt.is_none()
        {
            slot.reserved = None;
        }
    }
    pub fn acknowledge(&self, request: u64) {
        if let Ok(mut slot) = self.0.lock()
            && slot.receipt.as_ref().is_some_and(|r| r.request == request)
        {
            slot.reserved = None;
            slot.queued = false;
            slot.receipt = None;
        }
    }
    pub fn view(&self) -> Option<UsageClearView> {
        let slot = self.0.try_lock().ok()?;
        Some(UsageClearView {
            pending_request: if slot.receipt.is_none() {
                slot.reserved
            } else {
                None
            },
            receipt: slot.receipt.clone(),
        })
    }
    /// Only unqueued admission/errors/ACK state may overlay a cached history.
    /// A queued publication terminal must come from the writer's matching Arc.
    fn unpublished_view(&self) -> Option<UsageClearView> {
        let slot = self.0.try_lock().ok()?;
        if slot.queued {
            return None;
        }
        Some(UsageClearView {
            pending_request: if slot.receipt.is_none() {
                slot.reserved
            } else {
                None
            },
            receipt: slot.receipt.clone(),
        })
    }
    fn finish(&self, request: u64, result: Result<UsageClearPublication, UsageClearError>) {
        if let Ok(mut slot) = self.0.lock()
            && slot.reserved == Some(request)
            && slot.receipt.is_none()
        {
            slot.receipt = Some(UsageClearReceipt { request, result });
        }
    }
}
// Keep receipts inline in their existing finite channel; Clear must not add a
// separate allocation to every model terminal producer.
#[allow(clippy::large_enum_variant)]
enum HistoryEvent {
    Record(Receipt),
    Clear(u64),
}
fn clear_publication(state: UsageState) -> Result<UsageClearPublication, UsageClearError> {
    match state {
        UsageState::Ready | UsageState::Closed => Ok(UsageClearPublication::Durable),
        UsageState::DurabilityUnconfirmed => {
            Ok(UsageClearPublication::PublishedDurabilityUnconfirmed)
        }
        _ => Err(UsageClearError::Storage),
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum UsageState {
    #[default]
    Opening,
    Ready,
    DurabilityUnconfirmed,
    Unavailable,
    Closing,
    Closed,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum TimezoneBasis {
    #[default]
    Utc,
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Gaps {
    pub queue_dropped: u64,
    pub oversized: u64,
    pub write_failures: u64,
    pub interrupted_runs: u64,
    pub invalid_history: bool,
    pub time_unavailable: u64,
    pub detail_capped: u64,
    pub grouped_detail_capped: bool,
    pub counter_overflow: bool,
    pub shutdown_unconfirmed: bool,
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TokenTotals {
    pub sum: [u64; 6],
    pub known: [u64; 6],
    pub missing: [u64; 6],
    pub overflow: bool,
}
fn checked_add(target: &mut u64, value: u64, overflow: &mut bool) {
    match target.checked_add(value) {
        Some(sum) => *target = sum,
        None => {
            *target = u64::MAX;
            *overflow = true;
        }
    }
}
impl TokenTotals {
    fn add(&mut self, tokens: &TokenSnapshot) {
        for (index, value) in [
            tokens.input,
            tokens.output,
            tokens.reported_total,
            tokens.cache_read,
            tokens.cache_create,
            tokens.reasoning,
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(value) = value {
                checked_add(&mut self.sum[index], value, &mut self.overflow);
                checked_add(&mut self.known[index], 1, &mut self.overflow);
            } else {
                checked_add(&mut self.missing[index], 1, &mut self.overflow);
            }
        }
    }
    fn merge(&mut self, other: &Self) {
        self.overflow |= other.overflow;
        for index in 0..6 {
            checked_add(&mut self.sum[index], other.sum[index], &mut self.overflow);
            checked_add(
                &mut self.known[index],
                other.known[index],
                &mut self.overflow,
            );
            checked_add(
                &mut self.missing[index],
                other.missing[index],
                &mut self.overflow,
            );
        }
    }
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DecimalTotal {
    pub exact: String,
    pub samples: u64,
    pub missing: u64,
    pub overflow_samples: u64,
    pub overflow: bool,
}
impl Default for DecimalTotal {
    fn default() -> Self {
        Self {
            exact: "0".into(),
            samples: 0,
            missing: 0,
            overflow_samples: 0,
            overflow: false,
        }
    }
}
impl DecimalTotal {
    fn add(&mut self, amount: Option<&ExactDecimal>) {
        let Some(amount) = amount else {
            checked_add(&mut self.missing, 1, &mut self.overflow);
            return;
        };
        match decimal_add(&self.exact, amount.as_str()) {
            Some(sum) => {
                self.exact = sum;
                checked_add(&mut self.samples, 1, &mut self.overflow);
            }
            None => checked_add(&mut self.overflow_samples, 1, &mut self.overflow),
        }
    }
    fn merge(&mut self, other: &Self) {
        self.overflow |= other.overflow;
        checked_add(&mut self.missing, other.missing, &mut self.overflow);
        checked_add(
            &mut self.overflow_samples,
            other.overflow_samples,
            &mut self.overflow,
        );
        if other.samples != 0 {
            match decimal_add(&self.exact, &other.exact) {
                Some(sum) => {
                    self.exact = sum;
                    checked_add(&mut self.samples, other.samples, &mut self.overflow);
                }
                None => checked_add(
                    &mut self.overflow_samples,
                    other.samples,
                    &mut self.overflow,
                ),
            }
        }
    }
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BillingTotals {
    pub reported_cost_usd: DecimalTotal,
    pub estimated_cost_usd: DecimalTotal,
    pub unclassified_cost_usd: DecimalTotal,
    pub list_cost_usd: DecimalTotal,
    pub allowance_plan_usd: DecimalTotal,
    pub sources: [u64; 7],
    pub billed_sum: [u64; 6],
    pub billed_known: [u64; 6],
    pub invalid_records: u64,
    pub missing_records: u64,
    pub overflow: bool,
}
impl BillingTotals {
    fn add(&mut self, usage: &UsageSnapshot) {
        let Some(billing) = &usage.billing else {
            self.missing_records = self.missing_records.saturating_add(1);
            return;
        };
        if billing.invalid {
            checked_add(&mut self.invalid_records, 1, &mut self.overflow);
            return;
        }
        let index = match billing.source {
            Some(BillingSource::Free) => 0,
            Some(BillingSource::Balance) => 1,
            Some(BillingSource::Go) => 2,
            Some(BillingSource::GoPlus) => 3,
            Some(BillingSource::Byok) => 4,
            Some(BillingSource::Unknown) => 5,
            None => 6,
        };
        checked_add(&mut self.sources[index], 1, &mut self.overflow);
        let cost = if billing.estimated == Some(true) {
            &mut self.estimated_cost_usd
        } else if billing.estimated == Some(false)
            || matches!(
                billing.source,
                Some(
                    BillingSource::Free
                        | BillingSource::Balance
                        | BillingSource::Go
                        | BillingSource::GoPlus
                )
            )
        {
            &mut self.reported_cost_usd
        } else {
            &mut self.unclassified_cost_usd
        };
        cost.add(billing.cost_usd.as_ref());
        self.list_cost_usd.add(billing.list_cost_usd.as_ref());
        self.allowance_plan_usd
            .add(billing.allowance_plan_usd.as_ref());
        for (index, value) in [
            billing.billed_input,
            billing.billed_output,
            billing.reasoning,
            billing.cache_read,
            billing.cache_write_5m,
            billing.cache_write_1h,
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(value) = value {
                checked_add(&mut self.billed_sum[index], value, &mut self.overflow);
                checked_add(&mut self.billed_known[index], 1, &mut self.overflow);
            }
        }
    }
    fn merge(&mut self, other: &Self) {
        self.reported_cost_usd.merge(&other.reported_cost_usd);
        self.estimated_cost_usd.merge(&other.estimated_cost_usd);
        self.unclassified_cost_usd
            .merge(&other.unclassified_cost_usd);
        self.list_cost_usd.merge(&other.list_cost_usd);
        self.allowance_plan_usd.merge(&other.allowance_plan_usd);
        self.overflow |= other.overflow;
        for index in 0..7 {
            checked_add(
                &mut self.sources[index],
                other.sources[index],
                &mut self.overflow,
            );
        }
        for index in 0..6 {
            checked_add(
                &mut self.billed_sum[index],
                other.billed_sum[index],
                &mut self.overflow,
            );
            checked_add(
                &mut self.billed_known[index],
                other.billed_known[index],
                &mut self.overflow,
            );
        }
        self.invalid_records = self.invalid_records.saturating_add(other.invalid_records);
        self.missing_records = self.missing_records.saturating_add(other.missing_records);
    }
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Metrics {
    pub requests: u64,
    pub transport_attempts: u64,
    pub completed: u64,
    pub cancelled: u64,
    pub failed: u64,
    pub coverage: [u64; 4],
    pub tokens_final: [TokenTotals; 2],
    pub tokens_partial: [TokenTotals; 2],
    pub billing_final: BillingTotals,
    pub billing_partial: BillingTotals,
    pub overflow: bool,
}
impl Metrics {
    fn add(&mut self, record: &Receipt) {
        checked_add(&mut self.requests, 1, &mut self.overflow);
        if record.transport_dispatched {
            checked_add(&mut self.transport_attempts, 1, &mut self.overflow);
        }
        match record.outcome {
            Outcome::Completed => checked_add(&mut self.completed, 1, &mut self.overflow),
            Outcome::Cancelled => checked_add(&mut self.cancelled, 1, &mut self.overflow),
            _ => checked_add(&mut self.failed, 1, &mut self.overflow),
        }
        let index = match record.coverage {
            Coverage::Missing => 0,
            Coverage::Partial => 1,
            Coverage::Final => 2,
            Coverage::Invalid => 3,
        };
        checked_add(&mut self.coverage[index], 1, &mut self.overflow);
        let convention = match record.usage.tokens.semantics {
            InputSemantics::ChatInclusive => 0,
            InputSemantics::MessagesBasePlusCache => 1,
        };
        if record.coverage == Coverage::Final {
            self.tokens_final[convention].add(&record.usage.tokens);
            self.billing_final.add(&record.usage);
        } else {
            self.tokens_partial[convention].add(&record.usage.tokens);
            self.billing_partial.add(&record.usage);
        }
    }
    fn merge(&mut self, other: &Self) {
        self.overflow |= other.overflow;
        for (target, value) in [
            (&mut self.requests, other.requests),
            (&mut self.transport_attempts, other.transport_attempts),
            (&mut self.completed, other.completed),
            (&mut self.cancelled, other.cancelled),
            (&mut self.failed, other.failed),
        ] {
            checked_add(target, value, &mut self.overflow);
        }
        for index in 0..4 {
            checked_add(
                &mut self.coverage[index],
                other.coverage[index],
                &mut self.overflow,
            );
        }
        for index in 0..2 {
            self.tokens_final[index].merge(&other.tokens_final[index]);
            self.tokens_partial[index].merge(&other.tokens_partial[index]);
        }
        self.billing_final.merge(&other.billing_final);
        self.billing_partial.merge(&other.billing_partial);
    }
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PurposeTotals {
    pub client: Metrics,
    pub probe: Metrics,
}
impl PurposeTotals {
    fn add(&mut self, record: &Receipt) {
        match record.purpose {
            Purpose::Client => self.client.add(record),
            Purpose::Probe => self.probe.add(record),
        }
    }
    fn merge(&mut self, other: &Self) {
        self.client.merge(&other.client);
        self.probe.merge(&other.probe);
    }
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum GroupKey {
    Account(String),
    Profile(String),
    Model {
        provider: Provider,
        protocol: InputSemantics,
        model: String,
    },
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Groups {
    #[serde(deserialize_with = "bounded_rows")]
    rows: Vec<(GroupKey, PurposeTotals)>,
    other: PurposeTotals,
    capped: bool,
}
impl Groups {
    fn add(&mut self, key: GroupKey, record: &Receipt, cap: usize) {
        if let Some((_, totals)) = self.rows.iter_mut().find(|(existing, _)| existing == &key) {
            totals.add(record);
        } else if self.rows.len() < cap {
            let mut totals = PurposeTotals::default();
            totals.add(record);
            self.rows.push((key, totals));
        } else {
            self.other.add(record);
            self.capped = true;
        }
    }
    fn fold_all(&mut self) -> bool {
        if self.rows.is_empty() {
            return false;
        }
        for (_, totals) in self.rows.drain(..) {
            self.other.merge(&totals);
        }
        self.capped = true;
        true
    }
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Day {
    day: u64,
    global: PurposeTotals,
    accounts: Groups,
    profiles: Groups,
    models: Groups,
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GroupSummary {
    pub key: Option<GroupKey>,
    pub windows: [PurposeTotals; 3],
    pub incomplete: bool,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct UsageSummary {
    pub clear: UsageClearView,
    pub revision: u64,
    pub state: UsageState,
    pub timezone_basis: TimezoneBasis,
    pub today_utc_day: Option<u64>,
    pub computed_at_unix_ms: Option<u64>,
    pub retained_from_ms: Option<u64>,
    pub retained_to_ms: Option<u64>,
    pub windows: [PurposeTotals; 3],
    pub accounts: Vec<GroupSummary>,
    pub profiles: Vec<GroupSummary>,
    pub models: Vec<GroupSummary>,
    pub gaps: Gaps,
    pub active_requests: u64,
    pub run_admitted: u64,
    pub run_terminals: u64,
    pub run_rejected: u64,
    pub run_queue_dropped: u64,
    pub run_oversized: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u8,
    open_run: bool,
    #[serde(deserialize_with = "bounded_details")]
    details: VecDeque<Receipt>,
    #[serde(deserialize_with = "bounded_days")]
    days: Vec<Day>,
    gaps: Gaps,
    #[serde(skip)]
    detail_bytes: usize,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            version: 1,
            open_run: true,
            details: VecDeque::new(),
            days: vec![],
            gaps: Gaps::default(),
            detail_bytes: 0,
        }
    }
}
impl Snapshot {
    fn add(&mut self, record: Receipt) {
        if let Some(time) = record.finished_unix_ms {
            let day = time / DAY_MS;
            self.days.retain(|row| row.day.saturating_add(30) > day);
            let index = self
                .days
                .iter()
                .position(|row| row.day == day)
                .unwrap_or_else(|| {
                    self.days.push(Day {
                        day,
                        global: Default::default(),
                        accounts: Default::default(),
                        profiles: Default::default(),
                        models: Default::default(),
                    });
                    self.days.len() - 1
                });
            let row = &mut self.days[index];
            row.global.add(&record);
            row.accounts.add(
                GroupKey::Account(record.attribution.account.clone()),
                &record,
                16,
            );
            row.profiles.add(
                GroupKey::Profile(record.attribution.profile.clone()),
                &record,
                8,
            );
            row.models.add(
                GroupKey::Model {
                    provider: record.attribution.provider,
                    protocol: record.attribution.protocol,
                    model: record.attribution.model.clone(),
                },
                &record,
                512,
            );
        } else {
            self.gaps.time_unavailable = self.gaps.time_unavailable.saturating_add(1);
        }
        self.detail_bytes += serde_json::to_vec(&record).map_or(0, |bytes| bytes.len() + 1);
        self.details.push_back(record);
        while self.details.len() > DETAILS || self.detail_bytes > DETAIL_BYTES {
            if let Some(record) = self.details.pop_front() {
                self.detail_bytes = self
                    .detail_bytes
                    .saturating_sub(serde_json::to_vec(&record).map_or(0, |bytes| bytes.len() + 1));
                self.gaps.detail_capped = self.gaps.detail_capped.saturating_add(1);
            } else {
                break;
            }
        }
        self.days.sort_by_key(|row| row.day);
        while self.days.len() > 30 {
            self.days.remove(0);
            self.gaps.grouped_detail_capped = true;
        }
    }
    fn cap_aggregates(&mut self) {
        while serde_json::to_vec(&self.days).map_or(true, |data| data.len() > AGGREGATE_BYTES) {
            let folded = self
                .days
                .iter_mut()
                .find_map(|day| {
                    if day.models.fold_all() || day.profiles.fold_all() || day.accounts.fold_all() {
                        Some(())
                    } else {
                        None
                    }
                })
                .is_some();
            self.gaps.grouped_detail_capped |= folded;
            if !folded {
                break;
            }
        }
    }
    fn summary(
        &self,
        revision: u64,
        state: UsageState,
        computed_at_unix_ms: Option<u64>,
    ) -> UsageSummary {
        let today = computed_at_unix_ms.map(|time| time / DAY_MS);
        let mut summary = UsageSummary {
            revision,
            state,
            today_utc_day: today,
            computed_at_unix_ms,
            gaps: self.gaps.clone(),
            retained_from_ms: self
                .details
                .front()
                .and_then(|record| record.finished_unix_ms),
            retained_to_ms: self
                .details
                .back()
                .and_then(|record| record.finished_unix_ms),
            ..Default::default()
        };
        let Some(today) = today else {
            return summary;
        };
        for day in &self.days {
            for (index, length) in [1, 7, 30].into_iter().enumerate() {
                if day.day <= today && day.day.saturating_add(length) > today {
                    summary.windows[index].merge(&day.global);
                }
            }
            summarize_groups(&mut summary.accounts, &day.accounts, day.day, today, 16);
            summarize_groups(&mut summary.profiles, &day.profiles, day.day, today, 8);
            summarize_groups(&mut summary.models, &day.models, day.day, today, 512);
        }
        summary
    }
}
fn summarize_groups(
    out: &mut Vec<GroupSummary>,
    groups: &Groups,
    day: u64,
    today: u64,
    cap: usize,
) {
    for (key, totals) in groups
        .rows
        .iter()
        .map(|(key, totals)| (Some(key), totals))
        .chain(std::iter::once((None, &groups.other)))
    {
        let mut target = out.iter().position(|row| row.key.as_ref() == key);
        let has_room = out.iter().filter(|row| row.key.is_some()).count() < cap;
        if target.is_none() && key.is_some() && !has_room {
            target = out.iter().position(|row| row.key.is_none());
        }
        let index = target.unwrap_or_else(|| {
            out.push(GroupSummary {
                key: if key.is_none() || !has_room {
                    None
                } else {
                    key.cloned()
                },
                incomplete: groups.capped || (key.is_some() && !has_room),
                ..Default::default()
            });
            out.len() - 1
        });
        out[index].incomplete |=
            groups.capped || (!has_room && key.is_some() && out[index].key.is_none());
        for (window, length) in [1, 7, 30].into_iter().enumerate() {
            if day <= today && day.saturating_add(length) > today {
                out[index].windows[window].merge(totals);
            }
        }
    }
}

struct Shared {
    counters: SharedCounters,
    cache: Mutex<Arc<UsageSummary>>,
    monitor: crate::provider_monitoring::DiskSlot,
    clear: UsageClearHandle,
    cleared_dropped: AtomicU64,
    cleared_oversized: AtomicU64,
}
pub(super) struct TerminalSink {
    sender: SyncSender<HistoryEvent>,
    shared: Arc<Shared>,
}
impl TerminalSink {
    pub fn submit(&self, record: Receipt) {
        count(
            &self.shared.counters.terminal,
            &self.shared.counters.overflow,
        );
        if serde_json::to_vec(&record).map_or(true, |bytes| bytes.len() > RECEIPT_BYTES) {
            count(
                &self.shared.counters.oversized,
                &self.shared.counters.overflow,
            );
            return;
        }
        if let Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) =
            self.sender.try_send(HistoryEvent::Record(record))
        {
            count(
                &self.shared.counters.dropped,
                &self.shared.counters.overflow,
            );
        }
    }
}
pub(crate) struct UsageHistory {
    sender: SyncSender<HistoryEvent>,
    shared: Arc<Shared>,
    run_id: String,
    worker: Arc<Mutex<Option<JoinHandle<()>>>>,
    done: Arc<(Mutex<bool>, Condvar)>,
}
impl UsageHistory {
    pub fn open(root: PathBuf) -> Self {
        let (sender, incoming) = mpsc::sync_channel(QUEUE);
        let shared = Arc::new(Shared {
            counters: Default::default(),
            cache: Mutex::new(Arc::new(UsageSummary::default())),
            monitor: Default::default(),
            clear: Default::default(),
            cleared_dropped: AtomicU64::new(0),
            cleared_oversized: AtomicU64::new(0),
        });
        let run_id = super::super::random_id().unwrap_or_default();
        let done = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_done = done.clone();
        let worker_shared = shared.clone();
        let worker = if run_id.is_empty() {
            None
        } else {
            thread::Builder::new()
                .name("minicon-usage".into())
                .spawn(move || {
                    let _completion = WriterCompletion {
                        done: worker_done,
                        shared: worker_shared.clone(),
                    };
                    writer(root, incoming, worker_shared);
                })
                .ok()
        };
        if worker.is_none() {
            *done.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
            Arc::make_mut(&mut shared.cache.lock().unwrap_or_else(|e| e.into_inner())).state =
                UsageState::Unavailable;
        }
        Self {
            sender,
            shared,
            run_id,
            worker: Arc::new(Mutex::new(worker)),
            done,
        }
    }

    pub(crate) fn clear_handle(&self) -> UsageClearHandle {
        self.shared.clear.clone()
    }
    /// Called by the existing background Owner after GUI reservation/Actor enqueue.
    pub(crate) fn enqueue_reserved_clear(&self, request: u64) -> Result<(), UsageClearError> {
        // Existing background Owner only. A transient contention must not leave
        // an already admitted Actor command without a terminal receipt.
        let mut slot = self
            .shared
            .clear
            .0
            .lock()
            .map_err(|_| UsageClearError::Unavailable)?;
        if slot.reserved != Some(request) || slot.queued || slot.receipt.is_some() {
            return Err(UsageClearError::InvalidRequest);
        }
        let result = match self.sender.try_send(HistoryEvent::Clear(request)) {
            Ok(()) => {
                slot.queued = true;
                return Ok(());
            }
            Err(TrySendError::Full(_)) => UsageClearError::Busy,
            Err(TrySendError::Disconnected(_)) => UsageClearError::Unavailable,
        };
        // This is not a dropped model receipt. Retain its exact operation result.
        slot.receipt = Some(UsageClearReceipt {
            request,
            result: Err(result),
        });
        Err(result)
    }

    pub(crate) fn monitor_view(&self) -> Option<crate::provider_monitoring::DiskView> {
        self.shared.monitor.view()
    }
    pub(crate) fn submit_monitor(
        &self,
        snapshot: Arc<crate::provider_monitoring::MonitorSnapshot>,
    ) -> Result<(), crate::provider_monitoring::MonitorError> {
        self.shared.monitor.submit(snapshot)
    }
    pub(super) fn begin(&self) -> Option<(TerminalSink, u64, String)> {
        if self.run_id.is_empty() {
            count(
                &self.shared.counters.dropped,
                &self.shared.counters.overflow,
            );
            return None;
        }
        let previous = self
            .shared
            .counters
            .sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| {
                self.shared.counters.overflow.store(true, Ordering::Relaxed);
                count(
                    &self.shared.counters.dropped,
                    &self.shared.counters.overflow,
                );
            })
            .ok()?;
        count(
            &self.shared.counters.admitted,
            &self.shared.counters.overflow,
        );
        Some((
            TerminalSink {
                sender: self.sender.clone(),
                shared: self.shared.clone(),
            },
            previous + 1,
            self.run_id.clone(),
        ))
    }
    pub fn reject(&self) {
        count(
            &self.shared.counters.rejected,
            &self.shared.counters.overflow,
        );
    }
    pub fn summary(&self) -> Option<Arc<UsageSummary>> {
        let mut cache = self.shared.cache.try_lock().ok()?;
        let c = &self.shared.counters;
        if cache.run_admitted != c.admitted.load(Ordering::Relaxed)
            || cache.run_terminals != c.terminal.load(Ordering::Relaxed)
            || cache.run_rejected != c.rejected.load(Ordering::Relaxed)
            || cache.run_queue_dropped != c.dropped.load(Ordering::Relaxed)
            || cache.run_oversized != c.oversized.load(Ordering::Relaxed)
            || (!cache.gaps.counter_overflow && c.overflow.load(Ordering::Relaxed))
        {
            overlay_run(Arc::make_mut(&mut cache), &self.shared);
        }
        // A Full/Disconnected writer queue did not consume a cutoff and may
        // have no future writer wake. Publish its exact error with unchanged
        // history through this same bounded, nonblocking cache read seam.
        if let Some(clear) = self.shared.clear.unpublished_view()
            && clear != cache.clear
        {
            Arc::make_mut(&mut cache).clear = clear;
        }
        Some(cache.clone())
    }
}
impl UsageHistory {
    pub fn close_handle(&self) -> UsageClose {
        UsageClose {
            shared: self.shared.clone(),
            worker: self.worker.clone(),
            done: self.done.clone(),
        }
    }
}
/// Does not retain a Sender or a Gateway. Waiting cannot keep the run open.
pub(crate) struct UsageClose {
    shared: Arc<Shared>,
    worker: Arc<Mutex<Option<JoinHandle<()>>>>,
    done: Arc<(Mutex<bool>, Condvar)>,
}
impl UsageClose {
    /// Explicit final background Owner boundary only; never model/Stop/render.
    pub fn wait_owner(self) -> bool {
        let (done, signal) = &*self.done;
        let completed = done
            .lock()
            .ok()
            .and_then(|done| {
                signal
                    .wait_timeout_while(done, Duration::from_millis(500), |done| !*done)
                    .ok()
            })
            .is_some_and(|(done, _)| *done);
        if completed {
            if let Ok(mut slot) = self.worker.lock()
                && let Some(worker) = slot.take()
                && worker.join().is_err()
            {
                if let Ok(mut cache) = self.shared.cache.try_lock() {
                    let cache = Arc::make_mut(&mut cache);
                    cache.state = UsageState::Unavailable;
                    cache.gaps.shutdown_unconfirmed = true;
                }
                return false;
            }
            true
        } else {
            if let Ok(mut cache) = self.shared.cache.try_lock() {
                let cache = Arc::make_mut(&mut cache);
                cache.state = UsageState::Closing;
                cache.gaps.shutdown_unconfirmed = true;
            }
            false
        }
    }
}
fn overlay_run(summary: &mut UsageSummary, shared: &Shared) {
    let counters = &shared.counters;
    let dropped_baseline = shared.cleared_dropped.load(Ordering::Acquire);
    let oversized_baseline = shared.cleared_oversized.load(Ordering::Acquire);
    summary.run_admitted = counters.admitted.load(Ordering::Relaxed);
    summary.run_terminals = counters.terminal.load(Ordering::Relaxed);
    summary.active_requests = summary.run_admitted.saturating_sub(summary.run_terminals);
    summary.run_rejected = counters.rejected.load(Ordering::Relaxed);
    let dropped = counters.dropped.load(Ordering::Relaxed);
    summary.gaps.queue_dropped = summary
        .gaps
        .queue_dropped
        .saturating_sub(summary.run_queue_dropped.saturating_sub(dropped_baseline))
        .saturating_add(dropped.saturating_sub(dropped_baseline));
    summary.run_queue_dropped = dropped;
    let oversized = counters.oversized.load(Ordering::Relaxed);
    summary.gaps.oversized = summary
        .gaps
        .oversized
        .saturating_sub(summary.run_oversized.saturating_sub(oversized_baseline))
        .saturating_add(oversized.saturating_sub(oversized_baseline));
    summary.run_oversized = oversized;
    summary.gaps.counter_overflow |= counters.overflow.load(Ordering::Relaxed);
}
/// Called by the existing writer wake, including idle receive timeouts. This
/// updates only the shared read cache; it neither marks disk dirty nor flushes.
fn publish_summary(
    snapshot: &Snapshot,
    shared: &Shared,
    revision: u64,
    state: UsageState,
    changed: bool,
    computed_at_unix_ms: Option<u64>,
) {
    let Ok(mut cache) = shared.cache.try_lock() else {
        return;
    };
    let counters = &shared.counters;
    let counters_changed = cache.run_admitted != counters.admitted.load(Ordering::Relaxed)
        || cache.run_terminals != counters.terminal.load(Ordering::Relaxed)
        || cache.run_rejected != counters.rejected.load(Ordering::Relaxed)
        || cache.run_queue_dropped != counters.dropped.load(Ordering::Relaxed)
        || cache.run_oversized != counters.oversized.load(Ordering::Relaxed)
        || (!cache.gaps.counter_overflow && counters.overflow.load(Ordering::Relaxed));
    let day_changed = cache.today_utc_day != computed_at_unix_ms.map(|time| time / DAY_MS);
    let clear = shared.clear.view().unwrap_or_else(|| cache.clear.clone());
    let clear_changed = cache.clear != clear;
    if changed || counters_changed || day_changed || clear_changed || cache.state != state {
        let mut summary = snapshot.summary(revision, state, computed_at_unix_ms);
        summary.clear = clear;
        overlay_run(&mut summary, shared);
        *cache = Arc::new(summary);
    }
}
struct WriterCompletion {
    done: Arc<(Mutex<bool>, Condvar)>,
    shared: Arc<Shared>,
}
impl Drop for WriterCompletion {
    fn drop(&mut self) {
        if thread::panicking()
            && let Ok(mut cache) = self.shared.cache.try_lock()
        {
            let cache = Arc::make_mut(&mut cache);
            cache.state = UsageState::Unavailable;
            cache.gaps.shutdown_unconfirmed = true;
        }
        let (done, signal) = &*self.done;
        if let Ok(mut done) = done.lock() {
            *done = true;
            signal.notify_all();
        }
    }
}
fn writer(root: PathBuf, incoming: mpsc::Receiver<HistoryEvent>, shared: Arc<Shared>) {
    let path = root.join("history.json");
    let prepared = prepare_directory(&root);
    let loaded = if prepared {
        load_snapshot(&path)
    } else {
        Err(())
    };
    let mut writable = loaded.is_ok();
    let monitor_writable = shared.monitor.open(&root, prepared);
    let mut snapshot = loaded.unwrap_or_else(|_| Snapshot {
        gaps: Gaps {
            invalid_history: true,
            ..Default::default()
        },
        ..Default::default()
    });
    if snapshot.open_run && (!snapshot.details.is_empty() || !snapshot.days.is_empty()) {
        snapshot.gaps.interrupted_runs = snapshot.gaps.interrupted_runs.saturating_add(1);
    }
    snapshot.open_run = true;
    let mut revision = 0u64;
    let mut state = UsageState::Opening;
    let mut dirty = true;
    let mut next_flush = Instant::now();
    let mut clear_waiting = None;
    loop {
        let received = incoming.recv_timeout(Duration::from_millis(100));
        let disconnected = matches!(&received, Err(mpsc::RecvTimeoutError::Disconnected));
        let mut changed = received.is_ok();
        if let Ok(event) = received {
            apply_history_event(
                event,
                &mut snapshot,
                &mut writable,
                prepared,
                &path,
                &shared,
                &mut clear_waiting,
            );
            dirty = true;
        }
        for _ in 0..QUEUE - 1 {
            if let Ok(event) = incoming.try_recv() {
                apply_history_event(
                    event,
                    &mut snapshot,
                    &mut writable,
                    prepared,
                    &path,
                    &shared,
                    &mut clear_waiting,
                );
                dirty = true;
                changed = true;
            } else {
                break;
            }
        }
        // A disconnected receiver has consumed every queued item, and no
        // possible producer can submit another terminal. Stop alone cannot close.
        let closing = disconnected;
        if closing {
            for _ in 0..QUEUE {
                match incoming.try_recv() {
                    Ok(event) => apply_history_event(
                        event,
                        &mut snapshot,
                        &mut writable,
                        prepared,
                        &path,
                        &shared,
                        &mut clear_waiting,
                    ),
                    Err(_) => break,
                }
            }
            snapshot.open_run = shared.counters.admitted.load(Ordering::Acquire)
                != shared.counters.terminal.load(Ordering::Acquire);
            snapshot.gaps.shutdown_unconfirmed = snapshot.open_run;
            dirty = true;
        }
        if dirty && (Instant::now() >= next_flush || closing || clear_waiting.is_some()) {
            changed = true;
            snapshot.cap_aggregates();
            let mut stored = Snapshot {
                version: snapshot.version,
                open_run: snapshot.open_run,
                details: snapshot.details.clone(),
                days: snapshot.days.clone(),
                gaps: snapshot.gaps.clone(),
                detail_bytes: snapshot.detail_bytes,
            };
            stored.gaps.queue_dropped = stored.gaps.queue_dropped.saturating_add(
                shared
                    .counters
                    .dropped
                    .load(Ordering::Relaxed)
                    .saturating_sub(shared.cleared_dropped.load(Ordering::Acquire)),
            );
            stored.gaps.counter_overflow |= shared.counters.overflow.load(Ordering::Relaxed);
            stored.gaps.oversized = stored.gaps.oversized.saturating_add(
                shared
                    .counters
                    .oversized
                    .load(Ordering::Relaxed)
                    .saturating_sub(shared.cleared_oversized.load(Ordering::Acquire)),
            );
            let publication = if writable {
                serde_json::to_vec(&stored)
                    .ok()
                    .filter(|bytes| bytes.len() <= SNAPSHOT_BYTES)
                    .map(|bytes| filesystem_publish::write_private_atomic(&path, &bytes))
            } else {
                None
            };
            state = match publication {
                Some(Ok(())) => {
                    dirty = false;
                    if closing && !snapshot.open_run {
                        UsageState::Closed
                    } else {
                        UsageState::Ready
                    }
                }
                Some(Err(error)) if error.published() => {
                    dirty = false;
                    UsageState::DurabilityUnconfirmed
                }
                _ => {
                    snapshot.gaps.write_failures = snapshot.gaps.write_failures.saturating_add(1);
                    UsageState::Unavailable
                }
            };
            if let Some(request) = clear_waiting.take() {
                shared.clear.finish(request, clear_publication(state));
            }
            revision = revision.saturating_add(1);
            next_flush = Instant::now() + Duration::from_secs(1);
        }
        publish_summary(
            &snapshot,
            &shared,
            revision,
            state,
            changed || closing,
            now_ms(),
        );
        shared.monitor.flush(&root, monitor_writable);
        if closing {
            break;
        }
    }
}

/// Existing private-read trust boundary only: unsafe/oversize/unknown reads fail
/// closed, not a new SID/UID ownership assertion. Recheck at explicit cutoff.
fn clear_target_is_private(path: &Path) -> bool {
    match filesystem::read_private_bounded(path, SNAPSHOT_BYTES) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    }
}
fn apply_history_event(
    event: HistoryEvent,
    snapshot: &mut Snapshot,
    writable: &mut bool,
    prepared: bool,
    path: &Path,
    shared: &Shared,
    clear_waiting: &mut Option<u64>,
) {
    match event {
        HistoryEvent::Record(record) => snapshot.add(record),
        HistoryEvent::Clear(request) => {
            let accepted = shared.clear.0.lock().is_ok_and(|slot| {
                slot.reserved == Some(request) && slot.queued && slot.receipt.is_none()
            });
            if !accepted {
                return;
            }
            *clear_waiting = Some(request);
            *writable = prepared && clear_target_is_private(path);
            if !*writable {
                return;
            }
            // This is the queue cutoff. Earlier queued records cannot return;
            // later completions retain their existing live capture attribution.
            *snapshot = Snapshot::default();
            shared.cleared_dropped.store(
                shared.counters.dropped.load(Ordering::Acquire),
                Ordering::Release,
            );
            shared.cleared_oversized.store(
                shared.counters.oversized.load(Ordering::Acquire),
                Ordering::Release,
            );
        }
    }
}

fn prepare_directory(root: &Path) -> bool {
    if !root.is_absolute() {
        return false;
    }
    for ancestor in root.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) if filesystem::metadata_is_link_like(&metadata) || !metadata.is_dir() => {
                return false;
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return false,
        }
    }
    std::fs::create_dir_all(root).is_ok() && filesystem::protect_private_directory(root).is_ok()
}
// Read bounded sequences directly; reject element N+1 before allocating it.
fn bounded_sequence<'de, D, T, const CAP: usize>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Finite<T, const CAP: usize>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const CAP: usize> serde::de::Visitor<'de> for Finite<T, CAP> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "at most {CAP} usage entries")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            for _ in 0..CAP {
                match sequence.next_element()? {
                    Some(row) => out.push(row),
                    None => return Ok(out),
                }
            }
            if sequence.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::custom("usage-entry-limit"));
            }
            Ok(out)
        }
    }
    deserializer.deserialize_seq(Finite::<T, CAP>(std::marker::PhantomData))
}
fn bounded_rows<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<(GroupKey, PurposeTotals)>, D::Error> {
    bounded_sequence::<D, _, 512>(d)
}
fn bounded_details<'de, D: serde::Deserializer<'de>>(d: D) -> Result<VecDeque<Receipt>, D::Error> {
    bounded_sequence::<D, _, DETAILS>(d).map(VecDeque::from)
}
fn bounded_days<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Day>, D::Error> {
    bounded_sequence::<D, _, 30>(d)
}
fn load_snapshot(path: &Path) -> Result<Snapshot, ()> {
    let bytes = match filesystem::read_private_bounded(path, SNAPSHOT_BYTES) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Snapshot::default());
        }
        Err(_) => return Err(()),
    };
    let mut snapshot: Snapshot = serde_json::from_slice(&bytes).map_err(|_| ())?;
    if snapshot.version != 1
        || snapshot.details.len() > DETAILS
        || snapshot.days.len() > 30
        || !valid_snapshot(&snapshot)
    {
        return Err(());
    }
    if serde_json::to_vec(&snapshot.days).map_or(true, |bytes| bytes.len() > AGGREGATE_BYTES) {
        return Err(());
    }
    let mut detail_bytes = 0usize;
    for record in &snapshot.details {
        let bytes = serde_json::to_vec(record).map_err(|_| ())?;
        if bytes.len() > RECEIPT_BYTES {
            return Err(());
        }
        detail_bytes = detail_bytes.checked_add(bytes.len() + 1).ok_or(())?;
    }
    if detail_bytes > DETAIL_BYTES {
        return Err(());
    }
    snapshot.detail_bytes = detail_bytes;
    for day in &snapshot.days {
        if day.accounts.rows.len() > 16
            || day.profiles.rows.len() > 8
            || day.models.rows.len() > 512
        {
            return Err(());
        }
    }
    Ok(snapshot)
}

// Private history is input too: retain only finite typed identities and numbers.
fn safe_text(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}
fn opaque(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.'))
}
fn valid_profile(value: &str) -> bool {
    value == "default" || super::super::opaque_id(value)
}
fn valid_billing(value: &crate::provider_protocol::usage::OpenCodeBilling) -> bool {
    [
        &value.cost_usd,
        &value.list_cost_usd,
        &value.allowance_plan_usd,
    ]
    .into_iter()
    .all(|value| {
        value
            .as_ref()
            .is_none_or(|value| ExactDecimal::parse(value.as_str()).is_some())
    })
}
fn valid_receipt(record: &Receipt) -> bool {
    let a = &record.attribution;
    super::super::opaque_id(&record.run_id)
        && record.sequence != 0
        && valid_profile(&a.profile)
        && super::super::opaque_id(&a.account)
        && safe_text(&a.alias, 128)
        && safe_text(&a.model, 200)
        && a.authority
            .binding
            .as_ref()
            .is_none_or(|value| opaque(value))
        && opaque(&a.authority.credential)
        && opaque(&a.authority.service)
        && opaque(&a.authority.organization_epoch)
        && opaque(&a.authority.organization_digest)
        && record.usage.tokens.semantics == a.protocol
        && (record.coverage != Coverage::Final
            || (record.transport_complete
                && record.semantic_complete
                && record.usage.tokens.input.is_some()
                && record.usage.tokens.output.is_some()
                && !record.usage.tokens.invalid
                && !record.usage.tokens.non_monotonic))
        && record.usage.billing.as_ref().is_none_or(valid_billing)
        && record.queue_wait_ms.is_none()
}
fn valid_decimal_total(value: &DecimalTotal) -> bool {
    decimal_parts(&value.exact).is_some()
}
fn valid_metrics(value: &Metrics) -> bool {
    [&value.billing_final, &value.billing_partial]
        .into_iter()
        .all(|billing| {
            [
                &billing.reported_cost_usd,
                &billing.estimated_cost_usd,
                &billing.unclassified_cost_usd,
                &billing.list_cost_usd,
                &billing.allowance_plan_usd,
            ]
            .into_iter()
            .all(valid_decimal_total)
        })
}
fn valid_totals(value: &PurposeTotals) -> bool {
    valid_metrics(&value.client) && valid_metrics(&value.probe)
}
fn valid_groups(value: &Groups, kind: u8, cap: usize) -> bool {
    value.rows.len() <= cap
        && valid_totals(&value.other)
        && value.rows.iter().enumerate().all(|(index, (key, total))| {
            valid_totals(total)
                && !value.rows[..index].iter().any(|(old, _)| old == key)
                && match (kind, key) {
                    (0, GroupKey::Account(account)) => super::super::opaque_id(account),
                    (1, GroupKey::Profile(profile)) => valid_profile(profile),
                    (2, GroupKey::Model { model, .. }) => safe_text(model, 200),
                    _ => false,
                }
        })
}
fn valid_snapshot(value: &Snapshot) -> bool {
    value.details.iter().all(valid_receipt)
        && value.days.iter().enumerate().all(|(index, day)| {
            !value.days[..index].iter().any(|old| old.day == day.day)
                && valid_totals(&day.global)
                && valid_groups(&day.accounts, 0, 16)
                && valid_groups(&day.profiles, 1, 8)
                && valid_groups(&day.models, 2, 512)
        })
}

// Checked decimal addition over bounded lexical digits, never binary floats.
fn decimal_parts(raw: &str) -> Option<(bool, Vec<u8>, usize)> {
    if raw.len() > 512
        || !raw
            .bytes()
            .all(|c| c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.' | b'e' | b'E'))
    {
        return None;
    }
    let _: &serde_json::value::RawValue = serde_json::from_str(raw).ok()?;
    let (negative, raw) = raw
        .strip_prefix('-')
        .map_or((false, raw), |raw| (true, raw));
    let (base, exponent) = if let Some((base, exp)) = raw.split_once(['e', 'E']) {
        (base, exp.parse::<i32>().ok()?)
    } else {
        (raw, 0)
    };
    if !(-256..=256).contains(&exponent) {
        return None;
    }
    let fractional = base
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len());
    let mut digits = base
        .bytes()
        .filter(|c| *c != b'.')
        .map(|c| c.checked_sub(b'0'))
        .collect::<Option<Vec<_>>>()?;
    if digits.is_empty() || digits.iter().any(|d| *d > 9) {
        return None;
    }
    let scale = i32::try_from(fractional).ok()?.checked_sub(exponent)?;
    let scale = if scale < 0 {
        digits.extend(std::iter::repeat_n(0, usize::try_from(-scale).ok()?));
        0
    } else {
        usize::try_from(scale).ok()?
    };
    if digits.len() > 512 || scale > 512 {
        return None;
    }
    while digits.len() > 1 && digits[0] == 0 {
        digits.remove(0);
    }
    Some((negative, digits, scale))
}
fn decimal_add(left: &str, right: &str) -> Option<String> {
    let (a_negative, mut a, a_scale) = decimal_parts(left)?;
    let (b_negative, mut b, b_scale) = decimal_parts(right)?;
    let scale = a_scale.max(b_scale);
    a.extend(std::iter::repeat_n(0, scale - a_scale));
    b.extend(std::iter::repeat_n(0, scale - b_scale));
    if a.len() > 512 || b.len() > 512 {
        return None;
    }
    let mut out = Vec::new();
    let negative;
    if a_negative == b_negative {
        negative = a_negative;
        let mut carry = 0u8;
        for index in 0..a.len().max(b.len()) {
            let sum = a.len().checked_sub(index + 1).map_or(0, |i| a[i])
                + b.len().checked_sub(index + 1).map_or(0, |i| b[i])
                + carry;
            out.push(sum % 10);
            carry = sum / 10;
        }
        if carry != 0 {
            out.push(carry);
        }
        out.reverse();
    } else {
        let ordering = a.len().cmp(&b.len()).then_with(|| a.cmp(&b));
        let (large, small) = if ordering.is_ge() {
            negative = a_negative;
            (&a, &b)
        } else {
            negative = b_negative;
            (&b, &a)
        };
        let mut borrow = 0i16;
        for index in 0..large.len() {
            let mut digit = i16::from(large[large.len() - index - 1])
                - borrow
                - i16::from(small.len().checked_sub(index + 1).map_or(0, |i| small[i]));
            borrow = if digit < 0 {
                digit += 10;
                1
            } else {
                0
            };
            out.push(digit as u8);
        }
        out.reverse();
    }
    while out.len() > 1 && out[0] == 0 {
        out.remove(0);
    }
    if out.len() > 512 {
        return None;
    }
    let mut text = out
        .into_iter()
        .map(|digit| char::from(b'0' + digit))
        .collect::<String>();
    if scale != 0 {
        if text.len() <= scale {
            text = format!("0.{}{text}", "0".repeat(scale - text.len()));
        } else {
            text.insert(text.len() - scale, '.');
        }
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    if text.len() > 512 {
        return None;
    }
    if negative && text != "0" {
        text.insert(0, '-');
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    // Present the actual queued receipt envelope to the unchanged old consumers.
    // No replacement data or expected value is generated by this projection.
    struct ReceiptEvents(mpsc::Receiver<HistoryEvent>);
    impl ReceiptEvents {
        fn try_iter(&self) -> impl Iterator<Item = Receipt> + '_ {
            self.0.try_iter().map(|event| match event {
                HistoryEvent::Record(record) => record,
                HistoryEvent::Clear(_) => panic!("unexpected Clear in original receipt fixture"),
            })
        }
    }
    fn await_clear_snapshot(history: &UsageHistory, request: u64) -> Arc<UsageSummary> {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            if let Some(summary) = history.summary()
                && summary
                    .clear
                    .receipt
                    .as_ref()
                    .is_some_and(|r| r.request == request)
            {
                return summary;
            }
            assert!(
                Instant::now() < deadline,
                "actual clear publication receipt did not reach its matching summary"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn um_clear_actual_disconnected_receiver_terminal_joins_existing_history_arc() {
        // Real receiver disconnection, not a fake response or product View.
        let (sender, incoming) = mpsc::sync_channel(1);
        drop(incoming);
        let mut retained = Snapshot::default();
        retained.add(record(1));
        let original = Arc::new(retained.summary(7, UsageState::Ready, now_ms()));
        let shared = Arc::new(Shared {
            counters: Default::default(),
            cache: Mutex::new(original.clone()),
            monitor: Default::default(),
            clear: Default::default(),
            cleared_dropped: AtomicU64::new(0),
            cleared_oversized: AtomicU64::new(0),
        });
        let history = UsageHistory {
            sender,
            shared,
            run_id: "a".repeat(32),
            worker: Arc::new(Mutex::new(None)),
            done: Arc::new((Mutex::new(true), Condvar::new())),
        };
        let clear = history.clear_handle();
        clear.reserve(1).unwrap();
        assert_eq!(
            history.enqueue_reserved_clear(1),
            Err(UsageClearError::Unavailable)
        );
        // No writer exists that could call publish_summary again. Read the
        // real cached-summary consumer, not the control handle as a fake View.
        let summary = history.summary().unwrap();
        assert_eq!(
            summary
                .clear
                .receipt
                .as_ref()
                .map(|r| (r.request, r.result)),
            Some((1, Err(UsageClearError::Unavailable))),
            "disconnected Clear terminal was permanently hidden from summary"
        );
        assert_eq!(
            summary.windows[2].client.requests, 1,
            "failed enqueue erased retained history"
        );
        assert_eq!(
            summary.revision, 7,
            "queue rejection manufactured a disk revision"
        );
        assert!(
            original.clear.receipt.is_none(),
            "new terminal mutated an older immutable Arc"
        );
        assert!(!Arc::ptr_eq(&original, &summary));
        assert_eq!(clear.reserve(2), Err(UsageClearError::Busy));
        clear.acknowledge(2);
        assert_eq!(clear.reserve(2), Err(UsageClearError::Busy));
        clear.acknowledge(1);
        assert!(history.summary().unwrap().clear.receipt.is_none());
        assert_eq!(
            clear.reserve(2),
            Ok(()),
            "delivered disconnected terminal could not release its exact slot"
        );
        clear.withdraw(2);
        let close = history.close_handle();
        drop(history);
        assert!(
            close.wait_owner(),
            "non-producing receipt handle extended the writer lifetime"
        );
    }

    #[test]
    fn um_clear_actual_queue_cutoff_keeps_late_capture_and_nonproducing_ack_handle() {
        let directory = Directory::new();
        assert!(prepare_directory(&directory.0));
        let monitor_path = directory.0.join("account-monitoring.json");
        let monitor_bytes =
            serde_json::to_vec(&crate::provider_monitoring::MonitorSnapshot::default()).unwrap();
        filesystem_publish::write_private_atomic(&monitor_path, &monitor_bytes).unwrap();
        let history = UsageHistory::open(directory.0.clone());
        let (before, before_sequence, before_run) = history.begin().unwrap();
        let mut old = record(before_sequence);
        old.run_id = before_run;
        before.submit(old);
        drop(before);
        let (late, late_sequence, late_run) = history.begin().unwrap();
        history.shared.counters.dropped.store(7, Ordering::Release);
        history
            .shared
            .counters
            .oversized
            .store(3, Ordering::Release);
        let clear = history.clear_handle();
        clear.reserve(1).unwrap();
        history.enqueue_reserved_clear(1).unwrap();
        let summary = await_clear_snapshot(&history, 1);
        assert!(matches!(
            summary.clear.receipt.as_ref().unwrap().result,
            Ok(UsageClearPublication::Durable
                | UsageClearPublication::PublishedDurabilityUnconfirmed)
        ));
        assert_eq!(
            summary.windows[2].client.requests, 0,
            "queued records before Clear survived the cutoff"
        );
        assert_eq!(
            summary.active_requests, 1,
            "Clear reset the live request diagnostic"
        );
        assert_eq!((summary.run_admitted, summary.run_terminals), (2, 1));
        assert_eq!(
            (summary.gaps.queue_dropped, summary.gaps.oversized),
            (0, 0),
            "pre-clear counter gaps reappeared in cleared history"
        );
        assert_eq!((summary.run_queue_dropped, summary.run_oversized), (7, 3));
        assert_eq!(
            filesystem::read_private_bounded(&monitor_path, monitor_bytes.len()).unwrap(),
            monitor_bytes,
            "Clear touched provider monitoring state"
        );
        // A capture admitted before the cutoff finishes AFTER it and is new use.
        history
            .shared
            .counters
            .dropped
            .fetch_add(1, Ordering::AcqRel);
        let mut new = record(late_sequence);
        new.run_id = late_run;
        late.submit(new);
        drop(late);
        let close = history.close_handle();
        drop(history);
        assert!(
            close.wait_owner(),
            "receipt handle retained a Sender and prevented writer closure"
        );
        let disk = load_snapshot(&directory.0.join("history.json")).unwrap();
        assert_eq!(
            disk.details.iter().map(|r| r.sequence).collect::<Vec<_>>(),
            vec![late_sequence],
            "old queued/disk records returned after Clear"
        );
        assert_eq!(disk.days[0].global.client.requests, 1);
        assert_eq!(
            disk.gaps.queue_dropped, 1,
            "new post-cutoff gap was silently erased"
        );
        clear.acknowledge(1);
        assert_eq!(clear.reserve(2), Ok(()));
        clear.withdraw(2);
    }
    #[test]
    fn um_clear_actual_corrupt_private_history_requires_explicit_request() {
        let directory = Directory::new();
        assert!(prepare_directory(&directory.0));
        let path = directory.0.join("history.json");
        filesystem_publish::write_private_atomic(&path, b"{bounded-malformed").unwrap();
        let history = UsageHistory::open(directory.0.clone());
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            if history
                .summary()
                .is_some_and(|s| s.state == UsageState::Unavailable)
            {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            filesystem::read_private_bounded(&path, 128).unwrap(),
            b"{bounded-malformed",
            "startup silently reset corrupt history"
        );
        let clear = history.clear_handle();
        clear.reserve(1).unwrap();
        history.enqueue_reserved_clear(1).unwrap();
        let summary = await_clear_snapshot(&history, 1);
        assert!(
            matches!(
                summary.clear.receipt.as_ref().unwrap().result,
                Ok(UsageClearPublication::Durable
                    | UsageClearPublication::PublishedDurabilityUnconfirmed)
            ),
            "explicit Clear did not recover privately readable corrupt history"
        );
        let disk = load_snapshot(&path).unwrap();
        assert!(disk.details.is_empty() && disk.days.is_empty() && !disk.gaps.invalid_history);
        let close = history.close_handle();
        drop(history);
        assert!(close.wait_owner());
    }
    #[test]
    fn um_clear_actual_full_queue_reports_busy_without_model_loss_and_acks_exact() {
        let (sender, incoming) = mpsc::sync_channel(1);
        let shared = Arc::new(Shared {
            counters: Default::default(),
            cache: Mutex::new(Arc::new(UsageSummary::default())),
            monitor: Default::default(),
            clear: Default::default(),
            cleared_dropped: AtomicU64::new(0),
            cleared_oversized: AtomicU64::new(0),
        });
        let history = UsageHistory {
            sender,
            shared,
            run_id: "a".repeat(32),
            worker: Arc::new(Mutex::new(None)),
            done: Arc::new((Mutex::new(true), Condvar::new())),
        };
        history
            .sender
            .try_send(HistoryEvent::Record(record(1)))
            .ok()
            .unwrap();
        let clear = history.clear_handle();
        clear.reserve(1).unwrap();
        assert_eq!(
            history.enqueue_reserved_clear(1),
            Err(UsageClearError::Busy),
            "full statistics queue claimed Clear was accepted"
        );
        assert_eq!(
            clear.view().unwrap().receipt.unwrap().result,
            Err(UsageClearError::Busy)
        );
        assert_eq!(
            history.shared.counters.dropped.load(Ordering::Acquire),
            0,
            "Clear rejection was counted as dropped model work"
        );
        assert!(matches!(incoming.try_recv(), Ok(HistoryEvent::Record(_))));
        assert!(
            matches!(incoming.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "full Clear was automatically replayed"
        );
        clear.acknowledge(2);
        assert_eq!(
            clear.reserve(2),
            Err(UsageClearError::Busy),
            "wrong Clear acknowledgement released another request"
        );
        clear.acknowledge(1);
        assert_eq!(clear.reserve(2), Ok(()));
        clear.withdraw(2);
    }
    #[test]
    fn um_clear_actual_unsafe_target_keeps_bytes_and_reports_unpublished_failure() {
        // Same production mapping used after real publisher result below. This
        // checks typed published-but-unconfirmed semantics, not a native fs fault.
        assert_eq!(
            clear_publication(UsageState::DurabilityUnconfirmed),
            Ok(UsageClearPublication::PublishedDurabilityUnconfirmed)
        );
        let directory = Directory::new();
        let history = UsageHistory::open(directory.0.clone());
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            if history
                .summary()
                .is_some_and(|s| s.state == UsageState::Ready)
            {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        let path = directory.0.join("history.json");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let canary = path.join("canary");
        std::fs::write(&canary, b"synthetic-preserve").unwrap();
        let clear = history.clear_handle();
        clear.reserve(1).unwrap();
        history.enqueue_reserved_clear(1).unwrap();
        let summary = await_clear_snapshot(&history, 1);
        assert_eq!(
            summary.clear.receipt.as_ref().unwrap().result,
            Err(UsageClearError::Storage),
            "unpublished Clear failure was reported as durable erasure"
        );
        assert_eq!(
            std::fs::read(&canary).unwrap(),
            b"synthetic-preserve",
            "Clear replaced an unsafe target"
        );
        let close = history.close_handle();
        drop(history);
        assert!(close.wait_owner());
    }

    fn record(sequence: u64) -> Receipt {
        Receipt {
            run_id: "a".repeat(32),
            sequence,
            purpose: Purpose::Client,
            attribution: Attribution {
                profile: "default".into(),
                account: "b".repeat(32),
                alias: "synthetic-alias".into(),
                model: "synthetic-model".into(),
                provider: Provider::OpenCode,
                protocol: InputSemantics::ChatInclusive,
                authority: AuthorityStamp {
                    binding: Some("c".repeat(32)),
                    credential: "d".repeat(32),
                    service: "e".repeat(32),
                    organization_epoch: "f".repeat(32),
                    organization_digest: "a".repeat(64),
                    service_uses_proxy: false,
                },
            },
            started_unix_ms: now_ms(),
            finished_unix_ms: now_ms(),
            lease_duration_ms: 12,
            first_response_ms: Some(3),
            first_text_ms: Some(4),
            queue_wait_ms: None,
            transport_dispatched: true,
            transport_complete: true,
            semantic_complete: true,
            outcome: Outcome::Completed,
            delivery: Delivery::LocalWritesAccepted,
            coverage: Coverage::Final,
            usage: UsageSnapshot {
                tokens: TokenSnapshot {
                    input: Some(7),
                    output: Some(3),
                    cache_read: Some(2),
                    reasoning: Some(1),
                    ..Default::default()
                },
                ..Default::default()
            },
        }
    }
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            Self(std::env::temp_dir().canonicalize().unwrap().join(format!(
                "minicon-usage-{}",
                super::super::super::random_id().unwrap()
            )))
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn await_summary(history: &UsageHistory, terminals: u64) -> Arc<UsageSummary> {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            if let Some(summary) = history.summary()
                && summary.run_terminals == terminals
                && summary.windows[2].client.requests + summary.windows[2].probe.requests
                    == terminals
            {
                return summary;
            }
            assert!(
                Instant::now() < deadline,
                "actual usage writer did not publish its finite terminal summary"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn um_idle_utc_rollover_republishes_actual_cache_without_new_receipts_or_disk_revision() {
        let mut snapshot = Snapshot::default();
        for (index, day) in [100, 94, 71].into_iter().enumerate() {
            let mut receipt = record(index as u64 + 1);
            receipt.finished_unix_ms = Some(day * DAY_MS);
            snapshot.add(receipt);
        }
        let stored = serde_json::to_vec(&snapshot).unwrap();
        let shared = Shared {
            counters: Default::default(),
            cache: Mutex::new(Arc::new(UsageSummary::default())),
            monitor: Default::default(),
            clear: Default::default(),
            cleared_dropped: AtomicU64::new(0),
            cleared_oversized: AtomicU64::new(0),
        };
        let before_midnight = 101 * DAY_MS - 1;
        publish_summary(
            &snapshot,
            &shared,
            7,
            UsageState::Ready,
            true,
            Some(before_midnight),
        );
        let original = shared.cache.lock().unwrap().clone();
        assert_eq!(
            original
                .windows
                .each_ref()
                .map(|window| window.client.requests),
            [1, 2, 3]
        );
        publish_summary(
            &snapshot,
            &shared,
            7,
            UsageState::Ready,
            false,
            Some(before_midnight),
        );
        assert!(Arc::ptr_eq(&original, &shared.cache.lock().unwrap()));

        let midnight = 101 * DAY_MS;
        publish_summary(
            &snapshot,
            &shared,
            7,
            UsageState::Ready,
            false,
            Some(midnight),
        );
        let updated = shared.cache.lock().unwrap().clone();
        assert_eq!(
            updated
                .windows
                .each_ref()
                .map(|window| window.client.requests),
            [0, 1, 2]
        );
        assert_eq!(updated.today_utc_day, Some(101));
        assert_eq!(updated.computed_at_unix_ms, Some(midnight));
        assert_eq!(updated.revision, 7);
        assert_eq!(original.windows[0].client.requests, 1);
        assert_eq!(serde_json::to_vec(&snapshot).unwrap(), stored);
    }
    #[test]
    fn um_cache_clock_loss_is_unavailable_not_epoch_zero_and_recovers_without_new_receipts() {
        let mut snapshot = Snapshot::default();
        let mut receipt = record(1);
        receipt.finished_unix_ms = Some(1);
        snapshot.add(receipt);
        let shared = Shared {
            counters: Default::default(),
            cache: Mutex::new(Arc::new(UsageSummary::default())),
            monitor: Default::default(),
            clear: Default::default(),
            cleared_dropped: AtomicU64::new(0),
            cleared_oversized: AtomicU64::new(0),
        };
        publish_summary(&snapshot, &shared, 3, UsageState::Ready, true, Some(2));
        assert_eq!(shared.cache.lock().unwrap().windows[0].client.requests, 1);
        publish_summary(&snapshot, &shared, 3, UsageState::Ready, false, None);
        {
            let cache = shared.cache.lock().unwrap();
            assert_eq!(cache.today_utc_day, None);
            assert_eq!(cache.computed_at_unix_ms, None);
            assert_eq!(cache.windows[0].client.requests, 0);
        }
        publish_summary(&snapshot, &shared, 3, UsageState::Ready, false, Some(4));
        let cache = shared.cache.lock().unwrap();
        assert_eq!(cache.today_utc_day, Some(0));
        assert_eq!(cache.computed_at_unix_ms, Some(4));
        assert_eq!(cache.windows[0].client.requests, 1);
        assert_eq!(cache.revision, 3);
    }
    #[test]
    fn um_last_sender_lifetime_keeps_run_open_until_late_terminal_is_drained() {
        let directory = Directory::new();
        let history = UsageHistory::open(directory.0.clone());
        let (sink, sequence, run_id) = history.begin().unwrap();
        let first = history.close_handle();
        let final_close = history.close_handle();
        drop(history);
        assert!(
            !first.wait_owner(),
            "a live capture Sender must prevent an early closed marker"
        );
        assert!(
            load_snapshot(&directory.0.join("history.json"))
                .unwrap()
                .open_run
        );
        let mut receipt = record(sequence);
        receipt.run_id = run_id;
        sink.submit(receipt);
        drop(sink);
        assert!(
            final_close.wait_owner(),
            "after the last Sender finishes, the actual writer must drain and close"
        );
        let persisted = load_snapshot(&directory.0.join("history.json")).unwrap();
        assert!(!persisted.open_run);
        assert_eq!(persisted.details.len(), 1);
        assert_eq!(persisted.days[0].global.client.requests, 1);
    }
    #[test]
    fn um_capture_finishes_once_and_full_queue_reports_gap_without_blocking() {
        let (sender, incoming) = mpsc::sync_channel(QUEUE);
        let shared = Arc::new(Shared {
            counters: Default::default(),
            cache: Mutex::new(Arc::new(UsageSummary::default())),
            monitor: Default::default(),
            clear: Default::default(),
            cleared_dropped: AtomicU64::new(0),
            cleared_oversized: AtomicU64::new(0),
        });
        let history = UsageHistory {
            sender,
            shared: shared.clone(),
            run_id: "a".repeat(32),
            worker: Arc::new(Mutex::new(None)),
            done: Arc::new((Mutex::new(true), Condvar::new())),
        };
        for _ in 0..=QUEUE {
            let mut capture = Capture::unbound();
            capture.activate(Some(&history), Purpose::Client, record(1).attribution);
            capture.activate(Some(&history), Purpose::Probe, record(1).attribution);
            capture.dispatched();
            capture.transport_complete();
            capture.semantic_complete();
            capture.observe(record(1).usage);
            capture.finish(Ok(()), Delivery::LocalWritesAccepted);
            capture.finish(Err(ServerError::Cancelled), Delivery::Failed);
        }
        let incoming = ReceiptEvents(incoming);
        let records = incoming.try_iter().collect::<Vec<_>>();
        assert_eq!(
            records.len(),
            QUEUE,
            "finish/drop must submit one terminal per actual capture"
        );
        assert!(
            records
                .iter()
                .all(|record| record.purpose == Purpose::Client
                    && record.coverage == Coverage::Final
                    && record.outcome == Outcome::Completed)
        );
        assert_eq!(
            shared.counters.admitted.load(Ordering::Relaxed),
            QUEUE as u64 + 1
        );
        assert_eq!(
            shared.counters.terminal.load(Ordering::Relaxed),
            QUEUE as u64 + 1
        );
        assert_eq!(
            shared.counters.dropped.load(Ordering::Relaxed),
            1,
            "bounded queue loss must remain observable"
        );
    }
    #[test]
    fn um_final_requires_both_transport_semantics_and_required_counts() {
        let (sender, incoming) = mpsc::sync_channel(8);
        let shared = Arc::new(Shared {
            counters: Default::default(),
            cache: Mutex::new(Arc::new(UsageSummary::default())),
            monitor: Default::default(),
            clear: Default::default(),
            cleared_dropped: AtomicU64::new(0),
            cleared_oversized: AtomicU64::new(0),
        });
        let history = UsageHistory {
            sender,
            shared,
            run_id: "a".repeat(32),
            worker: Arc::new(Mutex::new(None)),
            done: Arc::new((Mutex::new(true), Condvar::new())),
        };
        for scenario in 0..4 {
            let mut capture = Capture::unbound();
            capture.activate(Some(&history), Purpose::Client, record(1).attribution);
            let mut usage = record(1).usage;
            if scenario == 0 {
                usage.tokens.output = None;
            }
            capture.observe(usage);
            if scenario != 1 {
                capture.transport_complete();
            }
            if scenario != 2 {
                capture.semantic_complete();
            }
            capture.finish(Ok(()), Delivery::LocalWritesAccepted);
        }
        let incoming = ReceiptEvents(incoming);
        assert_eq!(
            incoming
                .try_iter()
                .map(|row| row.coverage)
                .collect::<Vec<_>>(),
            vec![
                Coverage::Partial,
                Coverage::Partial,
                Coverage::Partial,
                Coverage::Final
            ]
        );
    }
    #[test]
    fn um_daily_dimensions_survive_detail_eviction_with_explicit_other() {
        let mut snapshot = Snapshot::default();
        for sequence in 1..=DETAILS + 1 {
            let mut receipt = record(sequence as u64);
            receipt.attribution.model = format!("model-{}", sequence % 513);
            receipt.attribution.account = format!("{:032x}", sequence % 17 + 1);
            snapshot.add(receipt);
        }
        snapshot.cap_aggregates();
        let summary = snapshot.summary(1, UsageState::Ready, now_ms());
        assert_eq!(snapshot.details.len(), DETAILS);
        assert_eq!(
            summary.windows[2].client.requests,
            DETAILS as u64 + 1,
            "daily totals cannot be rebuilt from the capped detail tail"
        );
        assert_eq!(
            summary
                .accounts
                .iter()
                .map(|row| row.windows[2].client.requests)
                .sum::<u64>(),
            DETAILS as u64 + 1
        );
        assert_eq!(
            summary
                .models
                .iter()
                .map(|row| row.windows[2].client.requests)
                .sum::<u64>(),
            DETAILS as u64 + 1
        );
        assert!(
            summary.accounts.iter().any(|row| row.key.is_none()
                && row.windows[2].client.requests > 0
                && row.incomplete)
        );
        assert!(
            summary.models.iter().any(|row| row.key.is_none()
                && row.windows[2].client.requests > 0
                && row.incomplete)
        );
        assert_eq!(
            summary.windows[2].client.tokens_final[0].sum[0],
            7 * (DETAILS as u64 + 1)
        );
        assert_eq!(
            summary.windows[2].client.tokens_final[0].sum[4], 0,
            "absent cache-write subset is not added to inclusive input"
        );
    }
    #[test]
    fn um_exact_financial_totals_separate_estimate_cash_and_allowance() {
        let mut observer = crate::provider_protocol::usage::UsageObserver::new(
            crate::provider_protocol::Protocol::OpenAiChat,
        );
        observer.set_billing(crate::provider_protocol::usage::BillingDialect::OpenCode);
        observer.observe_raw(br#"{"usage":{"cost":0,"opencode":{"billing_source":"go","list_cost":0.12345678901234567890123456789,"allowance":0.25}}}"#);
        let mut totals = BillingTotals::default();
        totals.add(&observer.snapshot());
        totals.add(&observer.snapshot());
        assert_eq!(totals.reported_cost_usd.exact, "0");
        assert_eq!(totals.reported_cost_usd.samples, 2);
        assert_eq!(totals.allowance_plan_usd.exact, "0.5");
        assert_eq!(
            totals.list_cost_usd.exact,
            "0.24691357802469135780246913578"
        );
        observer.observe_raw(
            br#"{"usage":{"cost":1e-3,"opencode":{"billing_source":"byok","estimated":true}}}"#,
        );
        totals.add(&observer.snapshot());
        assert_eq!(totals.estimated_cost_usd.exact, "0.001");
        assert_eq!(totals.reported_cost_usd.exact, "0");
        let before = totals.list_cost_usd.exact.clone();
        totals
            .list_cost_usd
            .add(Some(&ExactDecimal::parse("1e999").unwrap()));
        assert_eq!(totals.list_cost_usd.exact, before);
        assert_eq!(totals.list_cost_usd.overflow_samples, 1);
        assert!(decimal_parts("1..2").is_none());
    }
    #[test]
    fn um_private_writer_restart_retains_totals_and_corrupt_bytes_are_not_replaced() {
        let directory = Directory::new();
        {
            let history = UsageHistory::open(directory.0.clone());
            let (sink, sequence, run_id) = history.begin().unwrap();
            let mut receipt = record(sequence);
            receipt.run_id = run_id;
            sink.submit(receipt);
            drop(sink);
            let summary = await_summary(&history, 1);
            assert_eq!(summary.windows[2].client.tokens_final[0].sum[0], 7);
            let close = history.close_handle();
            drop(history);
            assert!(
                close.wait_owner(),
                "finite explicit writer close did not complete before fixture cleanup"
            );
        }
        let persisted = load_snapshot(&directory.0.join("history.json")).unwrap();
        assert!(
            !persisted.open_run,
            "normal finite writer close should publish a closed marker"
        );
        assert_eq!(persisted.details.len(), 1);
        {
            let history = UsageHistory::open(directory.0.clone());
            let deadline = Instant::now() + Duration::from_secs(4);
            loop {
                if history
                    .summary()
                    .is_some_and(|summary| summary.windows[2].client.requests == 1)
                {
                    break;
                }
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(10));
            }
            let close = history.close_handle();
            drop(history);
            assert!(
                close.wait_owner(),
                "finite explicit writer close did not complete before fixture cleanup"
            );
        }
        let corrupt = b"{not-json-and-not-a-secret}";
        filesystem_publish::write_private_atomic(&directory.0.join("history.json"), corrupt)
            .unwrap();
        {
            let history = UsageHistory::open(directory.0.clone());
            let deadline = Instant::now() + Duration::from_secs(4);
            loop {
                if history
                    .summary()
                    .is_some_and(|summary| summary.gaps.invalid_history)
                {
                    break;
                }
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(
                history.summary().unwrap().state,
                UsageState::Unavailable,
                "corrupt retained bytes must not become an empty successful history"
            );
            let close = history.close_handle();
            drop(history);
            assert!(
                close.wait_owner(),
                "finite explicit writer close did not complete before fixture cleanup"
            );
        }
        assert_eq!(
            std::fs::read(directory.0.join("history.json")).unwrap(),
            corrupt,
            "unreadable private history must not be replaced by an empty success snapshot"
        );
    }
    #[test]
    fn um_history_rejects_bounded_sequence_overflow_and_untrusted_identity() {
        let directory = Directory::new();
        assert!(prepare_directory(&directory.0));
        let mut snapshot = Snapshot::default();
        snapshot.details.push_back(record(1));
        snapshot.details[0].attribution.account = "synthetic-path/credential.json".into();
        filesystem_publish::write_private_atomic(
            &directory.0.join("history.json"),
            &serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();
        assert!(load_snapshot(&directory.0.join("history.json")).is_err());
        let mut value = serde_json::to_value(Snapshot::default()).unwrap();
        value["days"]=serde_json::Value::Array((0..31).map(|day|serde_json::json!({"day":day,"global":PurposeTotals::default(),"accounts":Groups::default(),"profiles":Groups::default(),"models":Groups::default()})).collect());
        filesystem_publish::write_private_atomic(
            &directory.0.join("history.json"),
            &serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        assert!(
            load_snapshot(&directory.0.join("history.json")).is_err(),
            "the N+1 history day must be rejected at deserialization"
        );
    }
}
