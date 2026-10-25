//! One bounded product parent gate and sequential reuse of the existing check.
use super::*;
use crate::provider_selection::{ProbePermit, ProfileVerificationPlan};
use std::collections::BTreeMap;

#[derive(Clone, Eq, PartialEq)]
pub(super) struct BatchIdentity {
    pub request: u64,
    pub profile: String,
    pub revision: u64,
    pub plan: u64,
    pub epoch: u64,
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum GatePhase {
    Pending,
    Active,
    Revoked(RuntimeError),
    CommitAdmitted,
    Finished,
}
struct GateEntry {
    identity: BatchIdentity,
    phase: GatePhase,
    cancel: Arc<NetworkHttpCancel>,
    child: Option<ProbePermit>,
    deadline: Option<Instant>,
}
#[derive(Clone, Default)]
pub(super) struct BatchGate(Arc<Mutex<Option<GateEntry>>>);
impl BatchGate {
    pub fn register(&self, identity: BatchIdentity) -> Result<(), RuntimeError> {
        let mut guard = self.0.lock().map_err(|_| RuntimeError::Storage)?;
        if guard.is_some() {
            return Err(RuntimeError::Busy);
        }
        *guard = Some(GateEntry {
            identity,
            phase: GatePhase::Pending,
            cancel: Arc::new(NetworkHttpCancel::new()),
            child: None,
            deadline: None,
        });
        Ok(())
    }
    pub fn withdraw(&self, identity: &BatchIdentity) {
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if guard.as_ref().is_some_and(|entry| {
            entry.identity == *identity
                && matches!(entry.phase, GatePhase::Pending | GatePhase::Revoked(_))
        }) {
            *guard = None;
        }
    }
    pub fn reserved(&self) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }
    fn revoke(entry: &mut GateEntry, error: RuntimeError) -> bool {
        if !matches!(entry.phase, GatePhase::Pending | GatePhase::Active) {
            return false;
        }
        entry.phase = GatePhase::Revoked(error);
        if let Some(permit) = entry.child.take() {
            permit.invalidate();
        }
        entry.cancel.cancel();
        true
    }
    pub fn invalidate_profile(&self, profile: &str) -> bool {
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .as_mut()
            .filter(|entry| entry.identity.profile == profile)
            .is_some_and(|entry| Self::revoke(entry, RuntimeError::DraftChanged))
    }
    pub fn cancel_exact(&self, request: u64) -> Result<(), RuntimeError> {
        let mut guard = self.0.lock().map_err(|_| RuntimeError::Storage)?;
        let entry = guard
            .as_mut()
            .filter(|entry| entry.identity.request == request)
            .ok_or(RuntimeError::InvalidAction)?;
        if Self::revoke(entry, RuntimeError::Cancelled) {
            Ok(())
        } else {
            Err(RuntimeError::InvalidAction)
        }
    }
    pub fn stop(&self, epoch: &AtomicU64) {
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        epoch.fetch_add(1, Ordering::AcqRel);
        if let Some(entry) = guard.as_mut() {
            Self::revoke(entry, RuntimeError::Cancelled);
        }
    }
    pub fn invalidate_all(&self) {
        if let Some(entry) = self.0.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            Self::revoke(entry, RuntimeError::Cancelled);
        }
    }
    pub fn activate(
        &self,
        id: &BatchIdentity,
        deadline: Instant,
        current_epoch: u64,
    ) -> Result<Arc<NetworkHttpCancel>, RuntimeError> {
        let mut guard = self.0.lock().map_err(|_| RuntimeError::Storage)?;
        let entry = guard
            .as_mut()
            .filter(|entry| entry.identity == *id)
            .ok_or(RuntimeError::Cancelled)?;
        if let GatePhase::Revoked(error) = entry.phase {
            return Err(error);
        }
        if entry.phase != GatePhase::Pending || id.epoch != current_epoch {
            return Err(RuntimeError::Cancelled);
        }
        entry.phase = GatePhase::Active;
        entry.deadline = Some(deadline);
        Ok(entry.cancel.clone())
    }
    fn check(
        entry: &GateEntry,
        id: &BatchIdentity,
        current_epoch: u64,
    ) -> Result<(), RuntimeError> {
        if entry.identity != *id {
            return Err(RuntimeError::Cancelled);
        }
        if let GatePhase::Revoked(error) = entry.phase {
            return Err(error);
        }
        if entry.phase != GatePhase::Active
            || id.epoch != current_epoch
            || entry.cancel.is_cancelled()
        {
            return Err(RuntimeError::Cancelled);
        }
        if entry
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(RuntimeError::DeadlineExceeded);
        }
        Ok(())
    }
    pub fn with_active<T>(
        &self,
        id: &BatchIdentity,
        epoch: u64,
        accept: impl FnOnce() -> Result<T, RuntimeError>,
    ) -> Result<T, RuntimeError> {
        let guard = self.0.lock().map_err(|_| RuntimeError::Storage)?;
        let entry = guard.as_ref().ok_or(RuntimeError::Cancelled)?;
        Self::check(entry, id, epoch)?;
        accept()
    }
    pub fn child(
        &self,
        id: &BatchIdentity,
        epoch: u64,
        permit: ProbePermit,
    ) -> Result<(), RuntimeError> {
        let mut guard = self.0.lock().map_err(|_| RuntimeError::Storage)?;
        let entry = guard.as_mut().ok_or(RuntimeError::Cancelled)?;
        Self::check(entry, id, epoch)?;
        entry.child = Some(permit);
        Ok(())
    }
    fn commit(&self, id: &BatchIdentity, epoch: u64) -> Result<(), RuntimeError> {
        let mut guard = self.0.lock().map_err(|_| RuntimeError::Storage)?;
        let entry = guard.as_mut().ok_or(RuntimeError::Cancelled)?;
        Self::check(entry, id, epoch)?;
        entry.phase = GatePhase::CommitAdmitted;
        entry.child = None;
        Ok(())
    }
    fn finish(&self, id: &BatchIdentity) {
        if let Some(entry) = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .filter(|entry| entry.identity == *id)
        {
            entry.phase = GatePhase::Finished;
            entry.child = None;
        }
    }
    pub fn finish_request(&self, request: u64) {
        if let Some(entry) = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .filter(|entry| entry.identity.request == request)
        {
            entry.phase = GatePhase::Finished;
            entry.child = None;
        }
    }
    pub fn acknowledge(&self, request: u64) {
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if guard.as_ref().is_some_and(|entry| {
            entry.identity.request == request && entry.phase == GatePhase::Finished
        }) {
            *guard = None;
        }
    }
    pub fn protects_terminal(&self, request: u64) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|entry| {
                entry.identity.request == request && entry.phase == GatePhase::Finished
            })
    }
}

pub(super) struct Batch {
    identity: BatchIdentity,
    plan: ProfileVerificationPlan,
    cursor: usize,
    deadline: Instant,
    cancel: Arc<NetworkHttpCancel>,
}
pub(super) struct Quote {
    fingerprint: [u8; 32],
    id: u64,
}
#[derive(Clone)]
pub(super) struct ProgressSink {
    pub pending: Arc<Mutex<Pending>>,
    pub notify: Arc<dyn Fn() + Send + Sync>,
}
pub(super) struct PendingReadiness {
    pub request: u64,
    pub snapshots: BTreeMap<String, crate::provider_accounts::AccountSnapshot>,
}

impl Owner {
    pub(super) fn finish_rejected_profile(
        &mut self,
        view: ConnectionCheckView,
        error: RuntimeError,
    ) {
        self.batch_gate.finish_request(view.request);
        self.reject_check(view, error);
    }
    pub(super) fn refresh_profile_plans(&mut self) {
        let mut projection = Vec::new();
        for view in &self.view.connection_profiles {
            match self.drafts.verification_plan(
                &view.id,
                view.revision,
                &self.config,
                &self.connection_accounts,
            ) {
                Ok(plan) => {
                    let id = if let Some(quote) = self
                        .quotes
                        .get(&view.id)
                        .filter(|quote| quote.fingerprint == plan.fingerprint)
                    {
                        quote.id
                    } else {
                        let Some(next) = self.quote_sequence.checked_add(1) else {
                            projection.push(ConnectionPlanView {
                                profile: view.id.clone(),
                                revision: view.revision,
                                id: 0,
                                paid_slots: Vec::new(),
                                error: Some(RuntimeError::InvalidAction),
                            });
                            continue;
                        };
                        self.quote_sequence = next;
                        self.quotes.insert(
                            view.id.clone(),
                            Quote {
                                fingerprint: plan.fingerprint,
                                id: next,
                            },
                        );
                        next
                    };
                    projection.push(ConnectionPlanView {
                        profile: view.id.clone(),
                        revision: view.revision,
                        id,
                        paid_slots: plan.steps.iter().map(|step| step.slot).collect(),
                        error: None,
                    });
                }
                Err(error) => {
                    self.quotes.remove(&view.id);
                    projection.push(ConnectionPlanView {
                        profile: view.id.clone(),
                        revision: view.revision,
                        id: 0,
                        paid_slots: Vec::new(),
                        error: Some(map_selection(error)),
                    });
                }
            }
        }
        self.quotes.retain(|profile, _| {
            self.view
                .connection_profiles
                .iter()
                .any(|view| &view.id == profile)
        });
        self.view.connection_plans = projection;
    }
    pub(super) fn begin_profile_check(
        &mut self,
        request: u64,
        profile: String,
        revision: u64,
        plan_id: u64,
        slots: Vec<u8>,
        epoch: u64,
    ) {
        let id = BatchIdentity {
            request,
            profile: profile.clone(),
            revision,
            plan: plan_id,
            epoch,
        };
        let mut view = Self::check_view(
            request,
            profile.clone(),
            revision,
            slots.first().copied().unwrap_or(0),
        );
        view.scope = ConnectionCheckScope::Profile;
        view.total = slots.len() as u8;
        let prepared = (|| {
            if self.batch.is_some() || self.operation.is_some() || self.view.login_active {
                return Err(RuntimeError::Busy);
            }
            self.refresh_view()?;
            let quoted = self
                .view
                .connection_plans
                .iter()
                .find(|quote| {
                    quote.profile == profile
                        && quote.revision == revision
                        && quote.id == plan_id
                        && quote.error.is_none()
                        && quote.paid_slots == slots
                })
                .ok_or(RuntimeError::PlanChanged)?;
            if quoted.paid_slots.is_empty() {
                return Err(RuntimeError::InvalidAction);
            }
            let plan = self
                .drafts
                .verification_plan(&profile, revision, &self.config, &self.connection_accounts)
                .map_err(map_selection)?;
            let deadline = Instant::now() + Duration::from_secs(300);
            let cancel =
                self.batch_gate
                    .activate(&id, deadline, self.stop_epoch.load(Ordering::Acquire))?;
            Ok(Batch {
                identity: id.clone(),
                plan,
                cursor: 0,
                deadline,
                cancel,
            })
        })();
        match prepared {
            Ok(batch) => {
                self.batch = Some(batch);
                self.readiness_pending = None;
                self.view.connection_check_active = Some(view);
                self.refresh_connection_view();
            }
            Err(error) => {
                self.batch_gate.finish(&id);
                self.reject_check(view, error);
            }
        }
    }
    pub(super) fn drive_batch(&mut self) {
        if self.batch.is_none() || self.operation.is_some() {
            return;
        }
        let result = (|| {
            let batch = self.batch.as_ref().unwrap();
            self.batch_gate.with_active(
                &batch.identity,
                self.stop_epoch.load(Ordering::Acquire),
                || Ok(()),
            )?;
            self.refresh_view()?;
            let batch = self.batch.as_ref().unwrap();
            let current = self
                .drafts
                .verification_plan(
                    &batch.identity.profile,
                    batch.identity.revision,
                    &self.config,
                    &self.connection_accounts,
                )
                .map_err(map_selection)?;
            if current.scope != batch.plan.scope {
                return Err(RuntimeError::PlanChanged);
            }
            if batch.cursor == batch.plan.steps.len() {
                return Ok(None);
            }
            let step = batch.plan.steps[batch.cursor].clone();
            let deadline = batch.deadline.min(Instant::now() + Duration::from_secs(60));
            let operation = self.spawn_probe_operation(
                step.ticket,
                step.candidate,
                batch.cancel.clone(),
                deadline,
                Some(&batch.identity),
            )?;
            if let Some(view) = &mut self.view.connection_check_active {
                view.slot = step.slot;
            }
            Ok(Some(operation))
        })();
        match result {
            Ok(Some(operation)) => self.operation = Some(operation),
            Ok(None) => self.commit_profile_check(),
            Err(error) => self.abort_batch(error),
        }
    }
    pub(super) fn finish_batch_check(
        &mut self,
        ticket: crate::provider_selection::ProbeTicket,
        permit: ProbePermit,
        account: &str,
        result: Result<OperationResult, RuntimeError>,
        cancelled: bool,
    ) {
        let Some(batch) = self.batch.as_ref() else {
            return;
        };
        let id = batch.identity.clone();
        let gate = self.batch_gate.clone();
        let epoch = self.stop_epoch.load(Ordering::Acquire);
        let accepted = self.accept_check_result(
            ticket,
            permit,
            account,
            result,
            cancelled,
            Some((&gate, &id, epoch)),
        );
        match accepted {
            Ok(snapshot) => {
                self.connection_accounts
                    .insert(account.into(), Ok(snapshot.clone()));
                let health = self.record_readiness(id.request, snapshot);
                if let Err(error) = health {
                    self.abort_batch(error);
                    return;
                }
                self.batch.as_mut().unwrap().cursor += 1;
                if let Some(view) = &mut self.view.connection_check_active {
                    view.completed += 1;
                    view.warning = self
                        .readiness_pending
                        .as_ref()
                        .map(|_| ConnectionCheckWarning::ReadinessStorage);
                }
            }
            Err(error) => self.abort_batch(error),
        }
    }
    pub(super) fn abort_batch(&mut self, error: RuntimeError) {
        let Some(batch) = self.batch.take() else {
            return;
        };
        // Exact cancellation may already have revoked this gate. Do not touch
        // an unrelated parent or a CommitAdmitted write.
        batch.cancel.cancel();
        if let Some(operation) = self.operation.take() {
            if let OperationKind::Probe { permit, .. } = &operation.kind {
                permit.invalidate();
            }
            operation.cancel.cancel();
            let _ = operation.worker.join();
        }
        if let Some(view) = self.view.connection_check_active.take() {
            self.reject_check(view, error);
        }
        self.batch_gate.finish(&batch.identity);
        self.refresh_connection_view();
    }
    fn commit_profile_check(&mut self) {
        self.commit_profile_check_with(|path, bytes| {
            match minicon_platform::filesystem_publish::write_private_atomic(path, bytes) {
                Ok(()) => Ok(ConfigPublication::Durable),
                Err(error) if error.published() => Ok(ConfigPublication::DurabilityUnconfirmed),
                Err(_) => Err(ServerError::Storage),
            }
        });
    }
    pub(super) fn commit_profile_check_with(
        &mut self,
        writer: impl FnOnce(&Path, &[u8]) -> Result<ConfigPublication, ServerError>,
    ) {
        let Some(batch) = self.batch.take() else {
            return;
        };
        let gate = self.batch_gate.clone();
        let id = batch.identity.clone();
        let epoch = self.stop_epoch.clone();
        let mut applying = self.view.clone();
        if let Some(view) = &mut applying.connection_check_active {
            view.phase = ConnectionCheckPhase::Applying;
        }
        let progress = self.progress.clone();
        let mut admission_error = None;
        let result = self.apply_connection_profile_with(&id.profile, id.revision, |path, bytes| {
            if let Err(error) = gate.commit(&id, epoch.load(Ordering::Acquire)) {
                admission_error = Some(error);
                return Err(ServerError::Cancelled);
            }
            if let Some(progress) = &progress {
                publish(&progress.pending, &progress.notify, applying, None);
            }
            writer(path, bytes)
        });
        let Some(mut view) = self.view.connection_check_active.take() else {
            return;
        };
        match result {
            Ok(()) => {
                view.phase = ConnectionCheckPhase::Applied;
                view.applied_revision = self.drafts.revision(&id.profile);
                view.warning = self
                    .readiness_pending
                    .as_ref()
                    .map(|_| ConnectionCheckWarning::ReadinessStorage);
                self.view.connection_check_last_terminal = Some(view);
            }
            Err(error) => self.reject_check(view, admission_error.unwrap_or(error)),
        }
        gate.finish(&id);
        self.refresh_connection_view();
    }
    pub(super) fn record_readiness(
        &mut self,
        request: u64,
        snapshot: crate::provider_accounts::AccountSnapshot,
    ) -> Result<(), RuntimeError> {
        match self.store.mark_ready_for_snapshot(&snapshot) {
            Ok(_) => {
                if let Some(pending) = &mut self.readiness_pending {
                    pending.snapshots.remove(&snapshot.summary.id);
                    if pending.snapshots.is_empty() {
                        self.readiness_pending = None;
                    }
                }
                Ok(())
            }
            Err(crate::provider_accounts::ReadinessError::Persist) => {
                if self
                    .readiness_pending
                    .as_ref()
                    .is_none_or(|pending| pending.request != request)
                {
                    self.readiness_pending = Some(PendingReadiness {
                        request,
                        snapshots: BTreeMap::new(),
                    });
                }
                let pending = self.readiness_pending.as_mut().unwrap();
                if pending.snapshots.len() >= 16
                    && !pending.snapshots.contains_key(&snapshot.summary.id)
                {
                    return Err(RuntimeError::Busy);
                }
                pending
                    .snapshots
                    .insert(snapshot.summary.id.clone(), snapshot);
                Ok(())
            }
            Err(crate::provider_accounts::ReadinessError::Authority(error)) => {
                Err(map_login(crate::provider_login::LoginError::Store(error)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn k1_runtime_key_operations_refuse_each_reserved_batch_phase_without_replay() {
        for phase in [
            GatePhase::Pending,
            GatePhase::Active,
            GatePhase::CommitAdmitted,
            GatePhase::Finished,
        ] {
            let (commands, incoming) = mpsc::sync_channel(16);
            let pending = Arc::new(Mutex::new(Pending::default()));
            let gate = pending.lock().unwrap().batch_gate.clone();
            let id = identity();
            gate.register(id.clone()).unwrap();
            if phase != GatePhase::Pending {
                gate.activate(&id, Instant::now() + Duration::from_secs(60), 0)
                    .unwrap();
            }
            if phase == GatePhase::CommitAdmitted {
                gate.commit(&id, 0).unwrap();
            }
            if phase == GatePhase::Finished {
                gate.finish(&id);
            }
            let view = View {
                profile_keys: vec![ProfileKeyMetadata {
                    profile: "default".into(),
                    state: crate::provider_runtime::profile_keys::ProfileKeyState::NotCreated,
                    exists: false,
                    epoch: 1,
                    durability_unconfirmed: false,
                }],
                ..View::default()
            };
            let mut runtime = Runtime {
                commands,
                connection_inflight: Default::default(),
                last_connection_request: 0,
                pending,
                view,
                exit: Arc::new(AtomicBool::new(false)),
                stop_requested: Arc::new(AtomicU64::new(0)),
                worker: None,
            };
            for operation in [
                ProfileKeyOperation::Create,
                ProfileKeyOperation::Copy,
                ProfileKeyOperation::Revoke,
            ] {
                assert_eq!(
                    runtime.act(Action::ProfileKey {
                        request: 1,
                        profile: "default".into(),
                        operation
                    }),
                    Err(RuntimeError::Busy)
                );
                assert!(
                    matches!(incoming.try_recv(), Err(mpsc::TryRecvError::Empty)),
                    "Busy key intent leaked into the actual command queue"
                );
            }
            assert!(!runtime.keys_reserved());
            gate.finish(&id);
            gate.acknowledge(id.request);
            assert_eq!(
                runtime.act(Action::ProfileKey {
                    request: 1,
                    profile: "default".into(),
                    operation: ProfileKeyOperation::Create
                }),
                Ok(()),
                "explicit later click must not be poisoned by a rejected request"
            );
        }
    }
    use super::*;
    fn identity() -> BatchIdentity {
        BatchIdentity {
            request: 1,
            profile: "default".into(),
            revision: 3,
            plan: 4,
            epoch: 0,
        }
    }
    #[test]
    fn cs05_gate_exact_cancel_and_data_intent_close_pending_and_registered_child() {
        let gate = BatchGate::default();
        let id = identity();
        gate.register(id.clone()).unwrap();
        assert_eq!(gate.cancel_exact(9), Err(RuntimeError::InvalidAction));
        let cancel = gate
            .activate(&id, Instant::now() + Duration::from_secs(60), 0)
            .unwrap();
        let permit = ProbePermit::new();
        gate.child(&id, 0, permit.clone()).unwrap();
        assert!(!gate.invalidate_profile("other"));
        assert!(!cancel.is_cancelled());
        assert!(gate.invalidate_profile("default"));
        assert!(
            !permit.try_admit(),
            "data intent must invalidate the actual paid admission permit"
        );
        assert!(cancel.is_cancelled());
        assert_eq!(
            gate.child(&id, 0, ProbePermit::new()),
            Err(RuntimeError::DraftChanged)
        );
        assert_eq!(gate.commit(&id, 0), Err(RuntimeError::DraftChanged));
    }
    #[test]
    fn cs05_gate_deadline_and_commit_admission_are_exact_distinct_cutoffs() {
        let gate = BatchGate::default();
        let id = identity();
        gate.register(id.clone()).unwrap();
        gate.activate(&id, Instant::now() - Duration::from_secs(1), 0)
            .unwrap();
        assert_eq!(
            gate.child(&id, 0, ProbePermit::new()),
            Err(RuntimeError::DeadlineExceeded)
        );
        assert_eq!(gate.commit(&id, 0), Err(RuntimeError::DeadlineExceeded));
        gate.finish(&id);
        gate.acknowledge(1);
        gate.register(id.clone()).unwrap();
        let cancel = gate
            .activate(&id, Instant::now() + Duration::from_secs(60), 0)
            .unwrap();
        gate.commit(&id, 0).unwrap();
        assert_eq!(gate.cancel_exact(1), Err(RuntimeError::InvalidAction));
        assert!(!gate.invalidate_profile("default"));
        gate.stop(&AtomicU64::new(0));
        assert!(
            !cancel.is_cancelled(),
            "post-admission intent cannot fake publication rollback"
        );
        gate.finish(&id);
        gate.acknowledge(9);
        assert!(gate.reserved());
        gate.acknowledge(1);
        assert!(!gate.reserved());
    }
}
