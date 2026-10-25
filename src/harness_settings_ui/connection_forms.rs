//! Process-local Settings buffers. Owner: PRD_02_31_01_02_01_connection_selection.
//! Uses the existing editor; never stores a route, grant, proof or key.
use crate::{composer, provider_runtime as runtime};
use minicon_core::field_editor::{self, Command, Limits};
use runtime::{
    Action, ConnectionAllocationInput, ConnectionChange, ConnectionDraftInput,
    ConnectionEditReceipt, ConnectionProfileView, RuntimeError, View,
};
use std::collections::BTreeMap;

const LOCAL_MODEL_BYTES: usize = 256;
const OWNER_MODEL_BYTES: usize = 200;
const LABEL_LIMITS: Limits = Limits {
    max_bytes: 128,
    preedit_chars: 128,
};
const SELECTOR_LIMITS: Limits = Limits {
    max_bytes: 256,
    preedit_chars: 256,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ProfileField {
    Label,
    DefaultAlias,
    Alias(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum EditField {
    Label,
    Enabled,
    DefaultAlias,
    Alias(u8),
    Account(u8),
    Model(u8),
    Slot(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FormFocus {
    Field(ProfileField),
    Model(Option<u8>),
}

fn editor(text: &str) -> composer::ComposerState {
    let mut state = composer::ComposerState::default();
    state.text = text.into();
    state.caret = text.len();
    state
}
fn sync_editor(state: &mut composer::ComposerState, text: &str) {
    if state.text != text {
        *state = editor(text);
    }
}

pub(super) struct AllocationEditors {
    pub slot: u8,
    pub alias: composer::ComposerState,
    pub account: Option<String>,
    pub model: composer::ComposerState,
}
impl AllocationEditors {
    fn new(input: &ConnectionAllocationInput) -> Self {
        Self {
            slot: input.slot,
            alias: editor(&input.alias),
            account: input.account.clone(),
            model: editor(&input.model),
        }
    }
    fn input(&self) -> ConnectionAllocationInput {
        ConnectionAllocationInput {
            slot: self.slot,
            alias: self.alias.text.clone(),
            account: self.account.clone(),
            model: self.model.text.clone(),
        }
    }
}
struct Pending {
    request: u64,
    change: ConnectionChange,
    input: ConnectionDraftInput,
    epoch: u64,
    fields: BTreeMap<EditField, u64>,
}
pub(super) struct ProfileEditors {
    pub label: composer::ComposerState,
    pub enabled: bool,
    pub default_alias: composer::ComposerState,
    pub allocations: BTreeMap<u8, AllocationEditors>,
    pub empty_model: composer::ComposerState,
    pub selected_slot: Option<u8>,
    pub account_picker: bool,
    pub scroll: usize,
    pub visited: bool,
    pub last_focus: Option<FormFocus>,
    pub revision: u64,
    pub local_error: Option<String>,
    pub view: ConnectionProfileView,
    epoch: u64,
    unsent: bool,
    blocked_epoch: Option<u64>,
    pending: Option<Pending>,
    fields: BTreeMap<EditField, u64>,
    field_sequence: u64,
}
impl ProfileEditors {
    fn new(view: &ConnectionProfileView) -> Self {
        Self {
            label: editor(&view.input.label),
            enabled: view.input.enabled,
            default_alias: editor(&view.input.default_alias),
            allocations: view
                .input
                .allocations
                .iter()
                .map(|a| (a.slot, AllocationEditors::new(a)))
                .collect(),
            empty_model: Default::default(),
            selected_slot: view.input.allocations.first().map(|a| a.slot),
            account_picker: false,
            scroll: 0,
            visited: false,
            last_focus: None,
            revision: view.revision,
            local_error: None,
            view: view.clone(),
            epoch: 0,
            unsent: false,
            blocked_epoch: None,
            pending: None,
            fields: BTreeMap::new(),
            field_sequence: 0,
        }
    }
    pub fn input(&self) -> ConnectionDraftInput {
        ConnectionDraftInput {
            label: self.label.text.clone(),
            enabled: self.enabled,
            default_alias: self.default_alias.text.clone(),
            allocations: self
                .allocations
                .values()
                .map(AllocationEditors::input)
                .collect(),
        }
    }
    pub fn selected_account(&self) -> Option<&str> {
        self.selected_slot
            .and_then(|s| self.allocations.get(&s))
            .and_then(|a| a.account.as_deref())
    }
    pub fn waiting(&self) -> bool {
        self.pending.is_some()
    }
    pub fn discarding(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.change == ConnectionChange::Discard)
    }
    pub fn unsent(&self) -> bool {
        self.unsent
    }
    pub fn model(&self) -> &composer::ComposerState {
        self.selected_slot
            .and_then(|s| self.allocations.get(&s))
            .map_or(&self.empty_model, |a| &a.model)
    }
    pub fn model_mut(&mut self) -> &mut composer::ComposerState {
        if let Some(allocation) = self
            .selected_slot
            .and_then(|s| self.allocations.get_mut(&s))
        {
            &mut allocation.model
        } else {
            &mut self.empty_model
        }
    }
    pub fn field(&self, field: ProfileField) -> &composer::ComposerState {
        match field {
            ProfileField::Label => &self.label,
            ProfileField::DefaultAlias => &self.default_alias,
            ProfileField::Alias(slot) => {
                &self
                    .allocations
                    .get(&slot)
                    .expect("published alias field")
                    .alias
            }
        }
    }
    pub fn field_mut(&mut self, field: ProfileField) -> &mut composer::ComposerState {
        match field {
            ProfileField::Label => &mut self.label,
            ProfileField::DefaultAlias => &mut self.default_alias,
            ProfileField::Alias(slot) => {
                &mut self
                    .allocations
                    .get_mut(&slot)
                    .expect("published alias field")
                    .alias
            }
        }
    }
    pub fn has_field(&self, field: ProfileField) -> bool {
        match field {
            ProfileField::Alias(s) => self.allocations.contains_key(&s),
            _ => true,
        }
    }
    fn mark_fields(&mut self, fields: &[EditField]) -> bool {
        let Some(epoch) = self.epoch.checked_add(1) else {
            self.local_error = Some("Draft edit sequence exhausted. Restart MiniCon.".into());
            return false;
        };
        if !self.touch_fields(fields) {
            return false;
        }
        self.epoch = epoch;
        self.unsent = true;
        self.blocked_epoch = None;
        self.local_error = None;
        true
    }
    fn touch_fields(&mut self, fields: &[EditField]) -> bool {
        let Some(sequence) = self.field_sequence.checked_add(1) else {
            self.local_error = Some("Draft field sequence exhausted. Restart MiniCon.".into());
            return false;
        };
        self.field_sequence = sequence;
        for field in fields {
            self.fields.insert(*field, sequence);
        }
        true
    }
    pub fn note_field_activity(&mut self, field: ProfileField) -> bool {
        self.touch_fields(&[match field {
            ProfileField::Label => EditField::Label,
            ProfileField::DefaultAlias => EditField::DefaultAlias,
            ProfileField::Alias(slot) => EditField::Alias(slot),
        }])
    }
    pub fn note_model_activity(&mut self) -> bool {
        self.selected_slot
            .is_some_and(|slot| self.touch_fields(&[EditField::Model(slot)]))
    }
    pub fn edit_field(&mut self, field: ProfileField) -> bool {
        self.mark_fields(&[match field {
            ProfileField::Label => EditField::Label,
            ProfileField::DefaultAlias => EditField::DefaultAlias,
            ProfileField::Alias(slot) => EditField::Alias(slot),
        }])
    }
    pub fn edit_model(&mut self) -> bool {
        self.selected_slot
            .is_some_and(|slot| self.mark_fields(&[EditField::Model(slot)]))
    }
    pub fn commit_model_edit(&mut self, id: &str) -> bool {
        if self.allocations.is_empty() {
            self.add_slot(id)
        } else {
            self.edit_model()
        }
    }
    pub fn toggle_enabled(&mut self) {
        self.enabled = !self.enabled;
        self.mark_fields(&[EditField::Enabled]);
    }
    pub fn remember_focus(&mut self, focus: FormFocus) {
        self.last_focus = Some(focus);
    }
    pub fn restore_focus(&mut self) -> Option<FormFocus> {
        let focus = self.last_focus?;
        let slot = match focus {
            FormFocus::Field(ProfileField::Alias(slot)) | FormFocus::Model(Some(slot)) => {
                Some(slot)
            }
            _ => None,
        };
        if slot.is_some_and(|slot| !self.allocations.contains_key(&slot))
            || matches!(focus, FormFocus::Model(None)) && !self.allocations.is_empty()
        {
            self.last_focus = None;
            return None;
        }
        if let Some(slot) = slot {
            self.selected_slot = Some(slot);
        }
        Some(focus)
    }
    pub fn blur(&mut self) {
        field_editor::apply(&mut self.label, Command::Blur, LABEL_LIMITS);
        field_editor::apply(&mut self.default_alias, Command::Blur, SELECTOR_LIMITS);
        field_editor::apply(&mut self.empty_model, Command::Blur, SELECTOR_LIMITS);
        for allocation in self.allocations.values_mut() {
            field_editor::apply(&mut allocation.alias, Command::Blur, SELECTOR_LIMITS);
            field_editor::apply(&mut allocation.model, Command::Blur, SELECTOR_LIMITS);
        }
    }
    pub fn add_slot(&mut self, id: &str) -> bool {
        let slot = if id == "default" {
            if !self.allocations.is_empty() {
                return false;
            }
            0
        } else {
            let Some(slot) = (0..32u8).find(|slot| !self.allocations.contains_key(slot)) else {
                self.local_error = Some("A connection has at most 32 model slots.".into());
                return false;
            };
            slot
        };
        let alias = if id == "default" {
            crate::provider_server::MODEL_ALIAS.into()
        } else {
            format!("model-{}", u16::from(slot) + 1)
        };
        self.allocations.insert(
            slot,
            AllocationEditors {
                slot,
                alias: editor(&alias),
                account: None,
                model: std::mem::take(&mut self.empty_model),
            },
        );
        let mut fields = vec![
            EditField::Slot(slot),
            EditField::Alias(slot),
            EditField::Account(slot),
            EditField::Model(slot),
        ];
        if self.default_alias.text.is_empty() {
            self.default_alias = editor(&alias);
            fields.push(EditField::DefaultAlias);
        }
        self.selected_slot = Some(slot);
        self.mark_fields(&fields)
    }
    pub fn assign_account(&mut self, account: &str) {
        let Some(allocation) = self
            .selected_slot
            .and_then(|s| self.allocations.get_mut(&s))
        else {
            return;
        };
        if allocation.account.as_deref() != Some(account) {
            let slot = allocation.slot;
            allocation.account = Some(account.into());
            allocation.model = Default::default();
            self.mark_fields(&[EditField::Account(slot), EditField::Model(slot)]);
        }
        self.account_picker = false;
    }
    fn replace_input(&mut self, input: &ConnectionDraftInput) {
        sync_editor(&mut self.label, &input.label);
        self.enabled = input.enabled;
        sync_editor(&mut self.default_alias, &input.default_alias);
        self.allocations
            .retain(|slot, _| input.allocations.iter().any(|a| a.slot == *slot));
        for allocation in &input.allocations {
            let local = self
                .allocations
                .entry(allocation.slot)
                .or_insert_with(|| AllocationEditors::new(allocation));
            sync_editor(&mut local.alias, &allocation.alias);
            sync_editor(&mut local.model, &allocation.model);
            local.account.clone_from(&allocation.account);
        }
        if !self
            .selected_slot
            .is_some_and(|s| self.allocations.contains_key(&s))
        {
            self.selected_slot = input.allocations.first().map(|a| a.slot);
        }
    }
    fn observe(&mut self, view: &ConnectionProfileView, receipt: Option<&ConnectionEditReceipt>) {
        if let Some(receipt) = receipt.filter(|r| {
            self.pending
                .as_ref()
                .is_some_and(|p| p.request == r.request && p.change == r.change)
        }) {
            let pending = self.pending.take().expect("matched pending draft action");
            if let Some(revision) = receipt.revision {
                self.revision = revision;
            }
            if receipt.result.is_ok() {
                if pending.change != ConnectionChange::Edit {
                    let rebased =
                        merge_later_fields(&pending.input, &self.input(), &view.input, &|field| {
                            self.fields.get(&field) != pending.fields.get(&field)
                        });
                    self.replace_input(&rebased);
                }
                self.unsent = self.input() != view.input;
                self.local_error = None;
                self.blocked_epoch = None;
            } else {
                self.local_error = receipt.result.err().map(|error| error.to_string());
                self.unsent = self.input() != view.input;
                // A rejected generation must not be resent on every poll. A
                // genuinely later edit is a distinct bounded attempt.
                self.blocked_epoch = (self.epoch <= pending.epoch).then_some(self.epoch);
            }
        } else if self.pending.is_none() && !self.unsent {
            self.revision = view.revision;
            self.replace_input(&view.input);
        }
        self.view = view.clone();
    }
    fn may_send(&mut self) -> bool {
        if self.pending.is_some() || !self.unsent || self.blocked_epoch == Some(self.epoch) {
            return false;
        }
        if self
            .allocations
            .values()
            .any(|a| a.model.text.len() > OWNER_MODEL_BYTES)
        {
            self.local_error = Some(format!(
                "Model selector exceeds the {OWNER_MODEL_BYTES}-byte draft limit; this editor retains up to {LOCAL_MODEL_BYTES} bytes. Shorten it to continue."
            ));
            self.blocked_epoch = Some(self.epoch);
            return false;
        }
        true
    }
}

/// Revert/Choose acknowledgements reset only fields unchanged since dispatch.
/// Later text remains local, including its selection/caret in replace_input.
fn merge_later_fields(
    before: &ConnectionDraftInput,
    local: &ConnectionDraftInput,
    authoritative: &ConnectionDraftInput,
    touched: &impl Fn(EditField) -> bool,
) -> ConnectionDraftInput {
    let mut result = authoritative.clone();
    if touched(EditField::Label) {
        result.label.clone_from(&local.label);
    }
    if touched(EditField::Enabled) {
        result.enabled = local.enabled;
    }
    if touched(EditField::DefaultAlias) {
        result.default_alias.clone_from(&local.default_alias);
    }
    for old in &before.allocations {
        let current = local.allocations.iter().find(|a| a.slot == old.slot);
        if current.is_none() {
            if touched(EditField::Slot(old.slot)) {
                result.allocations.retain(|a| a.slot != old.slot);
            }
            continue;
        }
        let current = current.unwrap();
        let changed = [
            EditField::Slot(old.slot),
            EditField::Alias(old.slot),
            EditField::Account(old.slot),
            EditField::Model(old.slot),
        ]
        .into_iter()
        .any(touched);
        if let Some(next) = result.allocations.iter_mut().find(|a| a.slot == old.slot) {
            if touched(EditField::Alias(old.slot)) {
                next.alias.clone_from(&current.alias);
            }
            if touched(EditField::Account(old.slot)) {
                next.account.clone_from(&current.account);
            }
            if touched(EditField::Model(old.slot)) {
                next.model.clone_from(&current.model);
            }
        } else if changed {
            result.allocations.push(current.clone());
        }
    }
    for added in &local.allocations {
        if !before.allocations.iter().any(|a| a.slot == added.slot)
            && touched(EditField::Slot(added.slot))
        {
            result.allocations.retain(|a| a.slot != added.slot);
            result.allocations.push(added.clone());
        }
    }
    result.allocations.sort_by_key(|a| a.slot);
    result
}

struct PublicationBaseline {
    request: u64,
    profile: String,
    input: ConnectionDraftInput,
    fields: BTreeMap<EditField, u64>,
}
#[derive(Default)]
pub(super) struct ConnectionForms {
    pub forms: BTreeMap<String, ProfileEditors>,
    pub selected: Option<String>,
    pub create_request: Option<u64>,
    pub error: Option<String>,
    sequence: u64,
    publication: Option<PublicationBaseline>,
}
impl ConnectionForms {
    pub fn publication_request(&self) -> Option<u64> {
        self.publication.as_ref().map(|baseline| baseline.request)
    }
    pub fn current(&self) -> Option<&ProfileEditors> {
        self.selected.as_ref().and_then(|id| self.forms.get(id))
    }
    pub fn current_mut(&mut self) -> Option<&mut ProfileEditors> {
        self.selected.as_ref().and_then(|id| self.forms.get_mut(id))
    }
    pub fn blur(&mut self) {
        for form in self.forms.values_mut() {
            form.blur();
        }
    }
    pub fn focus(&mut self, id: &str, scroll: usize) -> Option<usize> {
        if !self.forms.contains_key(id) {
            return None;
        }
        if let Some(form) = self.current_mut() {
            form.scroll = scroll;
            form.visited = true;
        }
        self.blur();
        self.selected = Some(id.into());
        self.current().map(|form| form.scroll)
    }
    fn request(&mut self) -> Option<u64> {
        self.sequence = self.sequence.checked_add(1).or_else(|| {
            self.error = Some("Connection request sequence exhausted. Restart MiniCon.".into());
            None
        })?;
        Some(self.sequence)
    }
    pub fn sync(&mut self, view: &View) {
        self.consume_publication(view);
        if let Some(receipt) = view.connection_receipts.iter().find(|r| {
            self.create_request == Some(r.request) && r.change == ConnectionChange::Create
        }) {
            self.create_request = None;
            if receipt.result.is_ok() {
                self.blur();
                self.selected = Some(receipt.profile.clone());
                self.error = None;
            } else {
                self.error = receipt.result.err().map(|e| e.to_string());
            }
        }
        // A successful Discard has no remaining profile row to observe. Its
        // exact owned receipt is authoritative even when account metadata is
        // unavailable; failure must preserve the form and every other buffer.
        let mut discarded = Vec::new();
        for receipt in &view.connection_receipts {
            let Some(form) = self.forms.get_mut(&receipt.profile) else {
                continue;
            };
            let matches = form.pending.as_ref().is_some_and(|pending| {
                pending.request == receipt.request && pending.change == receipt.change
            });
            if !matches {
                continue;
            }
            if receipt.change == ConnectionChange::Discard && receipt.result.is_ok() {
                discarded.push(receipt.profile.clone());
            } else if receipt.result.is_err()
                && !view
                    .connection_profiles
                    .iter()
                    .any(|profile| profile.id == receipt.profile)
            {
                let pending = form.pending.take().unwrap();
                if let Some(revision) = receipt.revision {
                    form.revision = revision;
                }
                form.local_error = receipt.result.err().map(|error| error.to_string());
                form.blocked_epoch = (form.epoch <= pending.epoch).then_some(form.epoch);
            }
        }
        if self
            .selected
            .as_ref()
            .is_some_and(|id| discarded.contains(id))
        {
            self.blur();
            self.selected = None;
        }
        for id in &discarded {
            self.forms.remove(id);
        }
        for profile in &view.connection_profiles {
            if discarded.contains(&profile.id) {
                continue;
            }
            if !self.forms.contains_key(&profile.id) && self.forms.len() >= 8 {
                self.error=Some("Connection forms changed while unavailable. Reopen MiniCon before creating more profiles.".into());
                continue;
            }
            self.forms
                .entry(profile.id.clone())
                .or_insert_with(|| ProfileEditors::new(profile))
                .observe(
                    profile,
                    view.connection_receipts
                        .iter()
                        .find(|r| r.profile == profile.id),
                );
        }
        // The book and account metadata have separate availability. An actual
        // projected default book is usable with unavailable account metadata;
        // a genuinely empty Loading/Unavailable view cannot delete buffers.
        if view.load_state == runtime::LoadState::Loaded {
            self.forms.retain(|id, form| {
                form.waiting() || view.connection_profiles.iter().any(|p| &p.id == id)
            });
        }
        if !self
            .selected
            .as_ref()
            .is_some_and(|id| self.forms.contains_key(id))
        {
            self.blur();
            self.selected = Some(view.connection_profile.clone())
                .filter(|id| self.forms.contains_key(id))
                .or_else(|| self.forms.keys().next().cloned());
        }
    }
    pub fn can_apply(&self) -> bool {
        self.current().is_some_and(|form| {
            !form.waiting()
                && !form.unsent()
                && form.input() == form.view.input
                && form.view.error.is_none()
                && form
                    .allocations
                    .values()
                    .all(|row| row.model.text.len() <= OWNER_MODEL_BYTES)
                && form.view.allocations.len() == form.view.input.allocations.len()
                && form.view.input.allocations.iter().all(|input| {
                    form.view
                        .allocations
                        .iter()
                        .any(|row| row.slot == input.slot)
                })
                && form.view.allocations.iter().all(|row| {
                    row.evidence == runtime::ConnectionEvidence::LegacyApplied
                        || (row.evidence == runtime::ConnectionEvidence::Verified
                            && row.availability == runtime::ConnectionAvailability::Available)
                })
        })
    }
    // Independent of edit receipts: this baseline must not block later edits.
    pub fn verify_profile(&mut self, plan: &runtime::ConnectionPlanView) -> Option<Action> {
        let id = self.selected.clone()?;
        if self.publication.is_some() {
            return None;
        }
        let form = self.current_mut()?;
        if form.waiting()
            || form.unsent()
            || form.input() != form.view.input
            || form.view.error.is_some()
            || form
                .allocations
                .values()
                .any(|row| row.model.text.len() > OWNER_MODEL_BYTES)
            || plan.error.is_some()
            || plan.id == 0
            || plan.profile != id
            || plan.revision != form.revision
            || plan.paid_slots.is_empty()
        {
            form.local_error = Some(
                "Acknowledge valid input and the current complete-profile quote before verifying."
                    .into(),
            );
            return None;
        }
        let baseline = PublicationBaseline {
            request: 0,
            profile: id.clone(),
            input: form.input(),
            fields: form.fields.clone(),
        };
        let request = self.request()?;
        self.publication = Some(PublicationBaseline {
            request,
            ..baseline
        });
        Some(Action::VerifyAndApplyConnectionProfile {
            request,
            profile: id,
            expected_revision: plan.revision,
            plan_id: plan.id,
            paid_slots: plan.paid_slots.clone(),
        })
    }
    fn consume_publication(&mut self, view: &View) {
        let Some(baseline) = self.publication.as_ref() else {
            return;
        };
        let Some(terminal) = view
            .connection_check_last_terminal
            .as_ref()
            .filter(|terminal| {
                terminal.scope == runtime::ConnectionCheckScope::Profile
                    && terminal.request == baseline.request
                    && terminal.profile == baseline.profile
            })
        else {
            return;
        };
        if terminal.phase == runtime::ConnectionCheckPhase::Applied {
            let Some(authoritative) = view.connection_profiles.iter().find(|profile| {
                profile.id == baseline.profile
                    && Some(profile.revision) == terminal.applied_revision
            }) else {
                return;
            };
            if let Some(form) = self.forms.get_mut(&baseline.profile) {
                let rebased = merge_later_fields(
                    &baseline.input,
                    &form.input(),
                    &authoritative.input,
                    &|field| form.fields.get(&field) != baseline.fields.get(&field),
                );
                form.replace_input(&rebased);
                form.revision = authoritative.revision;
                form.unsent = form.input() != authoritative.input;
                form.view = authoritative.clone();
                form.blocked_epoch = None;
                form.local_error = None;
            }
        } else if !matches!(
            terminal.phase,
            runtime::ConnectionCheckPhase::Failed | runtime::ConnectionCheckPhase::Cancelled
        ) {
            return;
        }
        self.publication = None;
    }

    pub fn verify(&mut self) -> Option<Action> {
        let id = self.selected.clone()?;
        let form = self.current_mut()?;
        if form.waiting()
            || form.unsent()
            || form.input() != form.view.input
            || form.view.error.is_some()
            || form.model().text.len() > OWNER_MODEL_BYTES
        {
            form.local_error =
                Some("Acknowledge valid staged edits before verifying this exact model.".into());
            return None;
        }
        let slot = form.selected_slot?;
        let expected_revision = form.revision;
        let request = self.request()?;
        Some(Action::VerifyConnectionAllocation {
            request,
            profile: id,
            expected_revision,
            slot,
        })
    }

    pub fn next_edit(&mut self) -> Option<Action> {
        let id = self
            .forms
            .iter_mut()
            .find_map(|(id, form)| form.may_send().then(|| id.clone()))?;
        let request = self.request()?;
        let form = self.forms.get_mut(&id).unwrap();
        let input = form.input();
        form.pending = Some(Pending {
            request,
            change: ConnectionChange::Edit,
            input: input.clone(),
            epoch: form.epoch,
            fields: form.fields.clone(),
        });
        Some(Action::EditConnectionProfile {
            request,
            profile: id,
            expected_revision: form.revision,
            input: Box::new(input),
        })
    }
    pub fn create(&mut self) -> Option<Action> {
        if self.create_request.is_some() || self.forms.len() >= 8 {
            self.error =
                Some("Wait for profile creation, or discard an unapplied custom profile.".into());
            return None;
        }
        let request = self.request()?;
        self.create_request = Some(request);
        Some(Action::CreateConnectionProfile { request })
    }
    pub fn command(
        &mut self,
        change: ConnectionChange,
        choice: Option<(String, String)>,
    ) -> Option<Action> {
        let id = self.selected.clone()?;
        if change == ConnectionChange::Apply && !self.can_apply() {
            self.current_mut()?.local_error = Some("Acknowledge valid edits and verify every changed model before Apply. Apply sends no paid requests.".into());
            return None;
        }
        if self.current().is_some_and(|f| {
            f.waiting()
                || (f.unsent()
                    && !matches!(change, ConnectionChange::Revert | ConnectionChange::Discard))
        }) {
            self.current_mut().unwrap().local_error =
                Some("Wait for draft acknowledgement, or retry the staged edits first.".into());
            return None;
        }
        let request = self.request()?;
        let form = self.forms.get_mut(&id)?;
        let slot = form.selected_slot.unwrap_or(0);
        let action = match change {
            ConnectionChange::Apply => Action::ApplyConnectionProfile {
                request,
                profile: id.clone(),
                expected_revision: form.revision,
            },
            ConnectionChange::Revert => Action::RevertConnectionProfile {
                request,
                profile: id.clone(),
                expected_revision: form.revision,
            },
            ConnectionChange::Discard => Action::DiscardUnappliedProfile {
                request,
                profile: id.clone(),
                expected_revision: form.revision,
            },
            ConnectionChange::Remove => Action::RemoveConnectionAllocation {
                request,
                profile: id.clone(),
                expected_revision: form.revision,
                slot,
            },
            ConnectionChange::Choose => {
                let (account, catalogue_id) = choice?;
                Action::ChooseConnectionModel {
                    request,
                    profile: id.clone(),
                    expected_revision: form.revision,
                    slot,
                    account,
                    catalogue_id,
                }
            }
            _ => return None,
        };
        form.pending = Some(Pending {
            request,
            change,
            input: form.input(),
            epoch: form.epoch,
            fields: form.fields.clone(),
        });
        Some(action)
    }
    pub fn retry(&mut self) {
        if let Some(form) = self.current_mut() {
            form.blocked_epoch = None;
            form.unsent = true;
            form.local_error = None;
        }
    }
    pub fn send_failed(&mut self, action: &Action, error: RuntimeError) {
        if let Action::VerifyAndApplyConnectionProfile { request, .. } = action {
            if self
                .publication
                .as_ref()
                .is_some_and(|baseline| baseline.request == *request)
            {
                let baseline = self.publication.take().unwrap();
                if let Some(form) = self.forms.get_mut(&baseline.profile) {
                    form.local_error = Some(error.to_string());
                }
            }
            return;
        }
        let request = match action {
            Action::CreateConnectionProfile { request }
            | Action::ApplyConnectionProfile { request, .. }
            | Action::EditConnectionProfile { request, .. }
            | Action::ChooseConnectionModel { request, .. }
            | Action::RemoveConnectionAllocation { request, .. }
            | Action::RevertConnectionProfile { request, .. }
            | Action::DiscardUnappliedProfile { request, .. } => *request,
            _ => return,
        };
        if self.create_request == Some(request) {
            self.create_request = None;
            self.error = Some(error.to_string());
        }
        for form in self.forms.values_mut() {
            if form.pending.as_ref().is_some_and(|p| p.request == request) {
                form.pending = None;
                form.local_error = Some(error.to_string());
                form.blocked_epoch = Some(form.epoch);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    fn cs05_plan() -> runtime::ConnectionPlanView {
        runtime::ConnectionPlanView {
            profile: "default".into(),
            revision: 1,
            id: 10,
            paid_slots: vec![0],
            error: None,
        }
    }
    fn cs05_terminal(view: &mut View, request: u64, phase: runtime::ConnectionCheckPhase) {
        view.connection_check_last_terminal = Some(runtime::ConnectionCheckView {
            scope: runtime::ConnectionCheckScope::Profile,
            total: 1,
            completed: u8::from(phase == runtime::ConnectionCheckPhase::Applied),
            applied_revision: (phase == runtime::ConnectionCheckPhase::Applied).then_some(3),
            request,
            profile: "default".into(),
            revision: 1,
            slot: 0,
            phase,
            error: None,
            checked_unix_ms: None,
            warning: None,
        });
    }
    #[test]
    fn cs05_profile_result_uses_nonblocking_baseline_and_keeps_later_text_aba_and_caret() {
        let mut view = loaded(vec![profile("default"), profile("other")]);
        let mut forms = ConnectionForms::default();
        forms.sync(&view);
        let action = forms.verify_profile(&cs05_plan()).unwrap();
        let request = match action {
            Action::VerifyAndApplyConnectionProfile { request, .. } => request,
            _ => panic!("profile action required"),
        };
        assert!(
            !forms.current().unwrap().waiting(),
            "batch must not occupy an edit receipt"
        );
        let form = forms.current_mut().unwrap();
        form.label = editor("changed");
        form.edit_field(ProfileField::Label);
        form.label = editor("default");
        form.edit_field(ProfileField::Label);
        form.label.caret = 3;
        form.label.anchor = Some(1);
        assert!(
            forms.next_edit().is_some(),
            "data edits must remain sendable during a batch"
        );
        view.connection_profiles[0].revision = 3;
        view.connection_profiles[0].input.label = "published normalization".into();
        cs05_terminal(
            &mut view,
            request + 1,
            runtime::ConnectionCheckPhase::Failed,
        );
        forms.sync(&view);
        assert!(forms.publication.is_some());
        cs05_terminal(&mut view, request, runtime::ConnectionCheckPhase::Applied);
        forms.sync(&view);
        let form = forms.current().unwrap();
        assert_eq!(form.label.text, "default");
        assert_eq!((form.label.caret, form.label.anchor), (3, Some(1)));
        assert_eq!(form.revision, 3);
        assert!(forms.publication.is_none());
        assert_eq!(forms.forms["other"].label.text, "other");
    }
    #[test]
    fn cs05_profile_ack_requires_exact_applied_revision_and_failure_retains_input_without_retry() {
        let mut view = loaded(vec![profile("default")]);
        let mut forms = ConnectionForms::default();
        forms.sync(&view);
        let action = forms.verify_profile(&cs05_plan()).unwrap();
        let request = match action {
            Action::VerifyAndApplyConnectionProfile { request, .. } => request,
            _ => unreachable!(),
        };
        cs05_terminal(&mut view, request, runtime::ConnectionCheckPhase::Applied);
        forms.sync(&view);
        assert!(
            forms.publication.is_some(),
            "stale projection cannot acknowledge publication"
        );
        cs05_terminal(&mut view, request, runtime::ConnectionCheckPhase::Failed);
        forms.sync(&view);
        assert!(forms.publication.is_none());
        assert!(forms.next_edit().is_none());
        let action = forms.verify_profile(&cs05_plan()).unwrap();
        forms.send_failed(&action, RuntimeError::Busy);
        assert!(forms.publication.is_none());
        assert!(!forms.current().unwrap().waiting());
        assert!(forms.next_edit().is_none());
        forms.current_mut().unwrap().model_mut().text = "x".repeat(201);
        forms.current_mut().unwrap().edit_model();
        let caret = forms.current().unwrap().model().caret;
        assert!(forms.verify_profile(&cs05_plan()).is_none());
        assert_eq!(forms.current().unwrap().model().text.len(), 201);
        assert_eq!(forms.current().unwrap().model().caret, caret);
    }

    fn cs04_apply_view() -> View {
        let mut view = loaded(vec![profile("default"), profile("other")]);
        view.busy = false;
        for profile in &mut view.connection_profiles {
            profile.allocations = profile
                .input
                .allocations
                .iter()
                .map(|row| crate::provider_selection::ConnectionAllocationView {
                    slot: row.slot,
                    upstream_model: Some("wire".into()),
                    protocol: Some(crate::provider_server::WireProtocol::Chat),
                    availability: runtime::ConnectionAvailability::Available,
                    evidence: runtime::ConnectionEvidence::Verified,
                    checked_unix_ms: Some(1234),
                })
                .collect();
        }
        view
    }

    #[test]
    fn cs04_apply_requires_exact_acknowledged_fields_and_never_auto_retries_a_failure() {
        let mut view = cs04_apply_view();
        let mut forms = ConnectionForms::default();
        forms.sync(&view);
        forms.current_mut().unwrap().model_mut().text = "x".repeat(201);
        forms.current_mut().unwrap().model_mut().caret = 17;
        forms.current_mut().unwrap().model_mut().anchor = Some(4);
        forms.current_mut().unwrap().edit_model();
        assert!(!forms.can_apply());
        assert!(forms.command(ConnectionChange::Apply, None).is_none());
        assert_eq!(
            (
                forms.current().unwrap().model().caret,
                forms.current().unwrap().model().anchor
            ),
            (17, Some(4))
        );
        assert!(forms.next_edit().is_none());
        forms.current_mut().unwrap().model_mut().text = "staged".into();
        forms.current_mut().unwrap().edit_model();
        let edit = forms.next_edit().unwrap();
        assert!(forms.command(ConnectionChange::Apply, None).is_none());
        view.connection_profiles[0].input.allocations[0].model = "staged".into();
        acknowledge(
            &mut view,
            "default",
            request(&edit),
            ConnectionChange::Edit,
            Ok(()),
        );
        forms.sync(&view);
        let apply = forms.command(ConnectionChange::Apply, None).unwrap();
        assert!(matches!(
            apply,
            Action::ApplyConnectionProfile {
                request: 2,
                expected_revision: 2,
                ..
            }
        ));
        assert!(!forms.can_apply());
        acknowledge(
            &mut view,
            "default",
            request(&apply),
            ConnectionChange::Apply,
            Err(RuntimeError::Storage),
        );
        forms.sync(&view);
        for _ in 0..3 {
            forms.sync(&view);
            assert!(forms.next_edit().is_none());
        }
        assert!(!forms.current().unwrap().waiting());
        assert!(
            forms
                .current()
                .unwrap()
                .local_error
                .as_ref()
                .unwrap()
                .contains("Storage")
        );
        assert!(forms.command(ConnectionChange::Apply, None).is_some()); // explicit retry only
    }

    #[test]
    fn cs04_apply_ack_rebases_only_unedited_fields_and_keeps_other_form_focus_and_caret() {
        let mut view = cs04_apply_view();
        let mut forms = ConnectionForms::default();
        forms.sync(&view);
        let apply = forms.command(ConnectionChange::Apply, None).unwrap();
        forms.current_mut().unwrap().model_mut().text = "typed-after-apply".into();
        forms.current_mut().unwrap().model_mut().caret = 3;
        forms.current_mut().unwrap().model_mut().anchor = Some(1);
        forms.current_mut().unwrap().edit_model();
        forms.focus("other", 9);
        forms.current_mut().unwrap().model_mut().caret = 4;
        forms.current_mut().unwrap().model_mut().anchor = Some(2);
        forms
            .current_mut()
            .unwrap()
            .remember_focus(FormFocus::Model(Some(0)));
        view.connection_profiles[0].input.allocations[0].model = "applied-wire".into();
        view.connection_profiles[0].applied = Some(view.connection_profiles[0].input.clone());
        acknowledge(
            &mut view,
            "default",
            request(&apply),
            ConnectionChange::Apply,
            Ok(()),
        );
        forms.sync(&view);
        let target = forms.forms.get("default").unwrap();
        assert_eq!(
            target.view.applied.as_ref().unwrap().allocations[0].model,
            "applied-wire"
        );
        assert_eq!(target.model().text, "typed-after-apply");
        assert_eq!((target.model().caret, target.model().anchor), (3, Some(1)));
        assert!(target.unsent());
        let other = forms.current_mut().unwrap();
        assert_eq!((other.model().caret, other.model().anchor), (4, Some(2)));
        assert_eq!(other.restore_focus(), Some(FormFocus::Model(Some(0))));
        assert_eq!(forms.selected.as_deref(), Some("other"));
    }
    #[test]
    fn cs03_verify_requires_acknowledged_exact_input_and_uses_global_request_sequence() {
        let mut view = loaded(vec![profile("default"), profile("custom")]);
        let mut forms = ConnectionForms::default();
        forms.sync(&view);
        let first = forms.verify().unwrap();
        assert!(
            matches!(first,Action::VerifyConnectionAllocation {request:1,ref profile,expected_revision:1,slot:0} if profile=="default")
        );
        let form = forms.current_mut().unwrap();
        form.model_mut().text = "x".repeat(201);
        form.model_mut().caret = 20;
        form.model_mut().anchor = Some(3);
        form.edit_model();
        assert!(forms.verify().is_none());
        assert_eq!(forms.current().unwrap().model().text.len(), 201);
        assert_eq!(
            (
                forms.current().unwrap().model().caret,
                forms.current().unwrap().model().anchor
            ),
            (20, Some(3))
        );
        forms.focus("custom", 7);
        let next = forms.verify().unwrap();
        assert!(
            matches!(next,Action::VerifyConnectionAllocation {request:2,ref profile,..} if profile=="custom")
        );
        forms.focus("default", 0);
        forms.current_mut().unwrap().model_mut().text = "valid".into();
        forms.current_mut().unwrap().edit_model();
        let sent = forms.next_edit().unwrap();
        assert!(forms.verify().is_none());
        let request = request(&sent);
        view.connection_profiles[0].input = forms.current().unwrap().input();
        acknowledge(
            &mut view,
            "default",
            request,
            ConnectionChange::Edit,
            Ok(()),
        );
        forms.sync(&view);
        assert!(matches!(
            forms.verify(),
            Some(Action::VerifyConnectionAllocation { request: 4, .. })
        ));
    }

    use super::*;

    fn profile(id: &str) -> ConnectionProfileView {
        ConnectionProfileView {
            id: id.into(),
            revision: 1,
            input: ConnectionDraftInput {
                label: id.into(),
                enabled: true,
                default_alias: "main".into(),
                allocations: vec![ConnectionAllocationInput {
                    slot: 0,
                    alias: "main".into(),
                    account: Some("account-a".into()),
                    model: "catalogue-a".into(),
                }],
            },
            applied: None,
            dirty: false,
            error: None,
            allocations: vec![],
        }
    }
    fn loaded(profiles: Vec<ConnectionProfileView>) -> View {
        View {
            load_state: runtime::LoadState::Loaded,
            connection_profile: "default".into(),
            connection_profiles: profiles,
            ..View::default()
        }
    }
    fn request(action: &Action) -> u64 {
        match action {
            Action::EditConnectionProfile { request, .. }
            | Action::ApplyConnectionProfile { request, .. }
            | Action::RevertConnectionProfile { request, .. }
            | Action::DiscardUnappliedProfile { request, .. }
            | Action::CreateConnectionProfile { request } => *request,
            _ => panic!("unexpected action"),
        }
    }
    fn acknowledge(
        view: &mut View,
        id: &str,
        request: u64,
        change: ConnectionChange,
        result: Result<(), RuntimeError>,
    ) {
        let profile = view
            .connection_profiles
            .iter_mut()
            .find(|p| p.id == id)
            .unwrap();
        profile.revision += 1;
        view.connection_receipts = vec![ConnectionEditReceipt {
            request,
            profile: id.into(),
            change,
            revision: Some(profile.revision),
            result,
        }];
    }

    #[test]
    fn one_inflight_coalesces_later_text_and_preserves_caret_on_ack() {
        let mut view = loaded(vec![profile("default")]);
        let mut forms = ConnectionForms::default();
        forms.sync(&view);
        let form = forms.current_mut().unwrap();
        form.label = editor("first edit");
        form.edit_field(ProfileField::Label);
        let sent = forms.next_edit().unwrap();
        assert!(forms.next_edit().is_none());
        let form = forms.current_mut().unwrap();
        form.label = editor("later edit");
        form.label.caret = 3;
        form.label.anchor = Some(1);
        form.edit_field(ProfileField::Label);
        view.connection_profiles[0].input.label = "first edit".into();
        acknowledge(
            &mut view,
            "default",
            request(&sent),
            ConnectionChange::Edit,
            Ok(()),
        );
        forms.sync(&view);
        let form = forms.current().unwrap();
        assert_eq!(form.label.text, "later edit");
        assert_eq!((form.label.caret, form.label.anchor), (3, Some(1)));
        match forms.next_edit().unwrap() {
            Action::EditConnectionProfile {
                input,
                expected_revision,
                ..
            } => {
                assert_eq!(input.label, "later edit");
                assert_eq!(expected_revision, 2);
            }
            _ => panic!("expected coalesced edit"),
        }
        assert!(forms.next_edit().is_none());
    }

    #[test]
    fn unrelated_ack_does_not_release_another_forms_pending_edit() {
        let mut view = loaded(vec![profile("default"), profile("custom")]);
        let mut forms = ConnectionForms::default();
        forms.sync(&view);
        forms.current_mut().unwrap().label = editor("edited");
        forms.current_mut().unwrap().edit_field(ProfileField::Label);
        let sent = forms.next_edit().unwrap();
        acknowledge(
            &mut view,
            "custom",
            request(&sent),
            ConnectionChange::Edit,
            Ok(()),
        );
        forms.sync(&view);
        assert!(forms.current().unwrap().waiting());
        assert!(forms.next_edit().is_none());
        acknowledge(
            &mut view,
            "default",
            request(&sent) + 1,
            ConnectionChange::Edit,
            Ok(()),
        );
        forms.sync(&view);
        assert!(forms.current().unwrap().waiting());
    }

    #[test]
    fn revert_ack_resets_untouched_fields_and_rebases_only_later_edits() {
        let mut view = loaded(vec![profile("default")]);
        let mut forms = ConnectionForms::default();
        forms.sync(&view);
        let form = forms.current_mut().unwrap();
        form.label = editor("old local");
        form.default_alias = editor("bad alias");
        form.edit_field(ProfileField::Label);
        form.edit_field(ProfileField::DefaultAlias);
        let sent = forms.command(ConnectionChange::Revert, None).unwrap();
        let form = forms.current_mut().unwrap();
        form.label = editor("typed after revert");
        form.label.caret = 5;
        form.edit_field(ProfileField::Label);
        acknowledge(
            &mut view,
            "default",
            request(&sent),
            ConnectionChange::Revert,
            Ok(()),
        );
        forms.sync(&view);
        let form = forms.current().unwrap();
        assert_eq!(form.label.text, "typed after revert");
        assert_eq!(form.label.caret, 5);
        assert_eq!(form.default_alias.text, "main");
        match forms.next_edit().unwrap() {
            Action::EditConnectionProfile { input, .. } => assert_eq!(input.default_alias, "main"),
            _ => panic!("expected rebased edit"),
        }
    }

    #[test]
    fn rejected_ack_stays_local_without_poll_retry_until_an_edit_or_explicit_retry() {
        let mut view = loaded(vec![profile("default")]);
        let mut forms = ConnectionForms::default();
        forms.sync(&view);
        forms.current_mut().unwrap().label = editor("rejected");
        forms.current_mut().unwrap().edit_field(ProfileField::Label);
        let sent = forms.next_edit().unwrap();
        acknowledge(
            &mut view,
            "default",
            request(&sent),
            ConnectionChange::Edit,
            Err(RuntimeError::InvalidAction),
        );
        forms.sync(&view);
        let form = forms.current_mut().unwrap();
        form.label.caret = 1;
        form.note_field_activity(ProfileField::Label);
        for _ in 0..4 {
            forms.sync(&view);
            assert!(forms.next_edit().is_none());
        }
        assert_eq!(forms.current().unwrap().label.text, "rejected");
        forms.retry();
        assert!(forms.next_edit().is_some());
    }

    #[test]
    fn model_256_byte_buffer_and_selection_survive_limit_rejection_and_profile_switch() {
        let view = loaded(vec![profile("default"), profile("custom")]);
        let mut forms = ConnectionForms::default();
        forms.sync(&view);
        let form = forms.current_mut().unwrap();
        field_editor::apply(
            form.model_mut(),
            Command::Action {
                action: crate::keymap::Action::SelectAll,
                text: None,
            },
            SELECTOR_LIMITS,
        );
        field_editor::apply(
            form.model_mut(),
            Command::Commit(&"x".repeat(256)),
            SELECTOR_LIMITS,
        );
        form.model_mut().caret = 42;
        form.model_mut().anchor = Some(4);
        form.edit_model();
        assert!(forms.next_edit().is_none());
        assert!(
            forms
                .current()
                .unwrap()
                .local_error
                .as_ref()
                .unwrap()
                .contains("200-byte")
        );
        forms.focus("custom", 18);
        forms.focus("default", 7);
        forms.sync(&view);
        let form = forms.current().unwrap();
        assert_eq!(form.model().text.len(), 256);
        assert_eq!((form.model().caret, form.model().anchor), (42, Some(4)));
        assert_eq!(form.scroll, 18);
        assert!(forms.next_edit().is_none());
        let form = forms.current_mut().unwrap();
        form.model_mut().text = "valid".into();
        form.model_mut().caret = 5;
        form.model_mut().anchor = None;
        form.edit_model();
        assert!(forms.next_edit().is_some());
    }

    #[test]
    fn all_eight_forms_keep_their_own_editor_and_scroll_during_switching_and_unavailable_view() {
        let view = loaded((0..8).map(|n| profile(&format!("p{n}"))).collect());
        let mut forms = ConnectionForms::default();
        forms.sync(&view);
        for n in 0..8 {
            assert!(
                forms
                    .focus(&format!("p{n}"), if n == 0 { 0 } else { 99 + n })
                    .is_some(),
                "an existing independent form must remain focusable"
            );
            let form = forms.current_mut().unwrap();
            form.label = editor(&format!("local{n}"));
            form.label.caret = n;
            form.edit_field(ProfileField::Label);
            form.scroll = 100 + n;
        }
        forms.sync(&View::default());
        assert_eq!(forms.forms.len(), 8);
        for n in 0..8 {
            let form = &forms.forms[&format!("p{n}")];
            assert_eq!(form.label.text, format!("local{n}"));
            assert_eq!(form.label.caret, n);
            assert_eq!(form.scroll, 100 + n);
        }
        assert!(forms.create().is_none());
    }

    #[test]
    fn create_selects_only_matching_receipt_and_discard_does_not_leave_stale_focus() {
        let mut view = loaded(vec![profile("default")]);
        let mut forms = ConnectionForms::default();
        forms.sync(&view);
        let sent = forms.create().unwrap();
        view.connection_profiles.push(profile("new"));
        view.connection_receipts = vec![ConnectionEditReceipt {
            request: request(&sent) + 1,
            profile: "new".into(),
            change: ConnectionChange::Create,
            revision: Some(1),
            result: Ok(()),
        }];
        forms.sync(&view);
        assert_eq!(forms.selected.as_deref(), Some("default"));
        view.connection_receipts[0].request = request(&sent);
        forms.sync(&view);
        assert_eq!(forms.selected.as_deref(), Some("new"));
        let action = forms.command(ConnectionChange::Discard, None).unwrap();
        assert!(matches!(action, Action::DiscardUnappliedProfile { .. }));
        view.connection_profiles.retain(|p| p.id != "new");
        view.connection_receipts = vec![ConnectionEditReceipt {
            request: request(&action),
            profile: "new".into(),
            change: ConnectionChange::Discard,
            revision: None,
            result: Ok(()),
        }];
        forms.sync(&view);
        assert_eq!(forms.selected.as_deref(), Some("default"));
        assert_eq!(forms.forms.len(), 1);
        let sent = forms.create().unwrap();
        let unavailable = View {
            connection_receipts: vec![ConnectionEditReceipt {
                request: request(&sent),
                profile: String::new(),
                change: ConnectionChange::Create,
                revision: None,
                result: Err(RuntimeError::WorkerUnavailable),
            }],
            ..View::default()
        };
        forms.sync(&unavailable);
        assert!(forms.create_request.is_none());
        assert!(forms.error.is_some());
        assert_eq!(forms.forms.len(), 1);
    }

    #[test]
    fn remove_readd_and_account_change_do_not_reuse_a_previous_model_selection() {
        let view = loaded(vec![profile("default")]);
        let mut forms = ConnectionForms::default();
        forms.sync(&view);
        let form = forms.current_mut().unwrap();
        form.assign_account("account-b");
        assert_eq!(form.model().text, "");
        assert_eq!(form.selected_account(), Some("account-b"));
        form.allocations.clear();
        form.selected_slot = None;
        assert!(form.add_slot("default"));
        assert_eq!(form.selected_slot, Some(0));
        assert_eq!(form.selected_account(), None);
        assert_eq!(form.model().text, "");
    }

    #[test]
    fn revert_ack_preserves_later_text_aba_and_its_selection() {
        let mut view = loaded(vec![profile("default")]);
        let mut forms = ConnectionForms::default();
        forms.sync(&view);
        let form = forms.current_mut().unwrap();
        form.label = editor("old dirty");
        form.edit_field(ProfileField::Label);
        let sent = forms.command(ConnectionChange::Revert, None).unwrap();
        let form = forms.current_mut().unwrap();
        field_editor::apply(
            &mut form.label,
            Command::Action {
                action: crate::keymap::Action::SelectAll,
                text: None,
            },
            LABEL_LIMITS,
        );
        field_editor::apply(&mut form.label, Command::Commit("different"), LABEL_LIMITS);
        form.edit_field(ProfileField::Label);
        field_editor::apply(
            &mut form.label,
            Command::Action {
                action: crate::keymap::Action::SelectAll,
                text: None,
            },
            LABEL_LIMITS,
        );
        field_editor::apply(&mut form.label, Command::Commit("old dirty"), LABEL_LIMITS);
        form.edit_field(ProfileField::Label);
        form.label.caret = 4;
        form.label.anchor = Some(1);
        view.connection_profiles[0].input.label = "saved".into();
        acknowledge(
            &mut view,
            "default",
            request(&sent),
            ConnectionChange::Revert,
            Ok(()),
        );
        forms.sync(&view);
        let form = forms.current().unwrap();
        assert_eq!(form.label.text, "old dirty");
        assert_eq!((form.label.caret, form.label.anchor), (4, Some(1)));
        assert!(forms.next_edit().is_some());
    }

    #[test]
    fn choose_ack_cannot_refill_a_model_cleared_by_a_later_account_change() {
        let mut view = loaded(vec![profile("default")]);
        view.connection_profiles[0].input.allocations[0]
            .model
            .clear();
        let mut forms = ConnectionForms::default();
        forms.sync(&view);
        let sent = forms
            .command(
                ConnectionChange::Choose,
                Some(("account-a".into(), "model-y".into())),
            )
            .unwrap();
        forms.current_mut().unwrap().assign_account("account-b");
        view.connection_profiles[0].input.allocations[0].model = "model-y".into();
        let request = match sent {
            Action::ChooseConnectionModel { request, .. } => request,
            _ => panic!("expected Choose"),
        };
        acknowledge(
            &mut view,
            "default",
            request,
            ConnectionChange::Choose,
            Ok(()),
        );
        forms.sync(&view);
        let form = forms.current().unwrap();
        assert_eq!(form.selected_account(), Some("account-b"));
        assert_eq!(form.model().text, "");
        match forms.next_edit().unwrap() {
            Action::EditConnectionProfile { input, .. } => {
                assert_eq!(input.allocations[0].account.as_deref(), Some("account-b"));
                assert_eq!(input.allocations[0].model, "");
            }
            _ => panic!("expected rebase"),
        }
    }

    #[test]
    fn unavailable_book_discards_only_the_exact_owned_success_and_can_create_again() {
        let mut view = loaded(vec![
            profile("default"),
            profile("custom"),
            profile("other"),
        ]);
        view.load_state = runtime::LoadState::Unavailable(RuntimeError::Storage);
        let mut forms = ConnectionForms::default();
        forms.sync(&view);
        assert_eq!(forms.selected.as_deref(), Some("default"));
        assert_eq!(forms.forms.len(), 3);
        forms.focus("other", 0);
        forms.current_mut().unwrap().label = editor("retained other draft");
        forms.current_mut().unwrap().edit_field(ProfileField::Label);
        forms.focus("custom", 0);
        let action = forms.command(ConnectionChange::Discard, None).unwrap();
        let request = match action {
            Action::DiscardUnappliedProfile { request, .. } => request,
            _ => panic!("expected Discard"),
        };
        view.connection_profiles.clear();
        view.connection_receipts = vec![ConnectionEditReceipt {
            request: request + 1,
            profile: "custom".into(),
            change: ConnectionChange::Discard,
            revision: None,
            result: Ok(()),
        }];
        forms.sync(&view);
        assert!(forms.current().unwrap().discarding());
        assert!(forms.forms.contains_key("custom"));
        view.connection_receipts[0].request = request;
        view.connection_receipts[0].result = Err(RuntimeError::Storage);
        forms.sync(&view);
        assert!(!forms.current().unwrap().waiting());
        assert!(forms.current().unwrap().local_error.is_some());
        assert_eq!(forms.forms.len(), 3);
        let action = forms.command(ConnectionChange::Discard, None).unwrap();
        let request = match action {
            Action::DiscardUnappliedProfile { request, .. } => request,
            _ => panic!("expected Discard"),
        };
        view.connection_receipts[0].request = request;
        view.connection_receipts[0].result = Ok(());
        forms.sync(&view);
        assert!(!forms.forms.contains_key("custom"));
        assert_eq!(forms.selected.as_deref(), Some("default"));
        assert_eq!(forms.forms["other"].label.text, "retained other draft");
        let create = forms.create().unwrap();
        view.connection_profiles = vec![profile("default"), profile("new")];
        view.connection_receipts = vec![ConnectionEditReceipt {
            request: self::request(&create),
            profile: "new".into(),
            change: ConnectionChange::Create,
            revision: Some(1),
            result: Ok(()),
        }];
        forms.sync(&view);
        assert_eq!(forms.selected.as_deref(), Some("new"));
        assert!(!forms.forms.contains_key("custom"));
        assert_eq!(forms.forms["other"].label.text, "retained other draft");
    }

    #[test]
    fn zero_slot_default_keeps_preedit_local_then_stages_committed_canonical_slot() {
        let mut view = loaded(vec![profile("default")]);
        view.connection_profiles[0].input.allocations.clear();
        let mut forms = ConnectionForms::default();
        forms.sync(&view);
        let form = forms.current_mut().unwrap();
        field_editor::apply(form.model_mut(), Command::Preedit("ni"), SELECTOR_LIMITS);
        assert_eq!(form.model().preedit, "ni");
        assert!(form.allocations.is_empty());
        assert!(forms.next_edit().is_none());
        let form = forms.current_mut().unwrap();
        field_editor::apply(
            form.model_mut(),
            Command::Commit("provider/中-draft"),
            SELECTOR_LIMITS,
        );
        assert!(form.commit_model_edit("default"));
        forms.blur();
        assert_eq!(forms.current().unwrap().model().text, "provider/中-draft");
        assert_eq!(forms.current().unwrap().model().preedit, "");
        match forms.next_edit().unwrap() {
            Action::EditConnectionProfile { input, .. } => {
                assert_eq!(input.allocations.len(), 1);
                assert_eq!(input.allocations[0].slot, 0);
                assert_eq!(
                    input.allocations[0].alias,
                    crate::provider_server::MODEL_ALIAS
                );
                assert_eq!(input.allocations[0].model, "provider/中-draft");
            }
            _ => panic!("expected staged canonical input"),
        }
    }
}
