//! Harness settings presentation. Runtime authority stays with provider_runtime.
use super::*;
mod connection_forms;
mod monitoring;
mod profile_keys;
use connection_forms::{ConnectionForms, FormFocus, ProfileField};
use minicon_core::field_editor::{self, Command, Intent, Limits};
use profile_keys::{KeyUi, RecipeKind};
use provider_runtime::ConnectionChange;
use provider_runtime::{AccountEdit, AccountPreferences, AccountSummary, NetworkPolicy};
use provider_runtime::{Action, Effect, ListenerState, Provider, View};

const PORT_LIMITS: Limits = Limits {
    max_bytes: 5,
    preedit_chars: 5,
};
const MODEL_LIMITS: Limits = Limits {
    max_bytes: 256,
    preedit_chars: 256,
};

#[derive(Clone, Copy, Eq, PartialEq)]
enum Field {
    Port,
    Model,
    Profile(ProfileField),
    ProfileChooser,
    AccountLabel,
    LoginProxy,
    ServiceProxy,
    ConnectLabel,
    ConnectLoginProxy,
    ConnectServiceProxy,
}

#[derive(Clone, Copy)]
enum Row {
    Local,
    Listener,
    Port,
    ListenerActions,
    Accounts,
    ConnectInput(Field),
    ConnectNetworkHint,
    Login,
    LoginActions,
    Challenge,
    ChallengeActions,
    Account(usize),
    AccountInput(Field),
    AccountActions,
    AccountConnection,
    AccountReconnect,
    AccountReconnectHint,
    AccountNetworkHint,
    CatalogueStatus,
    CatalogueRefresh,
    CatalogueModel(usize),
    NoAccounts,
    Model,
    ModelInput,
    ModelActions,
    VerificationActions,
    VerificationCancel,
    VerificationRetry,
    ProbeNotice,
    ModelChoice(usize),
    Profile(usize),
    ProfileCreate,
    ProfileState,
    ProfileInput(ProfileField),
    ProfileToggle,
    ProfileApplied,
    AppliedAlias(usize),
    AppliedAccount(usize),
    AppliedModel(usize),
    Allocation(u8),
    AllocationActions,
    AllocationAccount,
    AllocationAccountChoice(usize),
    ProfileCommands,
    ProfileVerification,
    Connect,
    KeyState,
    KeyDurability,
    KeyFeedback,
    KeyApplied,
    KeyShell,
    KeyActions,
    OpenAi,
    Anthropic,
    Recipe,
    RecipePi,
    Status,
    Monitoring(monitoring::Item),
}

struct DeviceChallenge {
    attempt: provider_runtime::AttemptId,
    context: browser::PageContext,
    verification_url: provider_auth::Secret,
    user_code: provider_auth::Secret,
    deadline: Instant,
    remaining_seconds: u64,
}

struct ConnectionDraft {
    label: composer::ComposerState,
    login: composer::ComposerState,
    service: composer::ComposerState,
    provider: Provider,
}
impl Default for ConnectionDraft {
    fn default() -> Self {
        Self {
            label: Default::default(),
            login: Default::default(),
            service: Default::default(),
            provider: Provider::DeepSeek,
        }
    }
}

#[derive(Default)]
struct AccountDraft {
    label: composer::ComposerState,
    login: composer::ComposerState,
    service: composer::ComposerState,
    enabled: bool,
    applied_label: String,
    applied: Option<AccountPreferences>,
    submitted: Option<u64>,
    save_error: Option<provider_runtime::RuntimeError>,
}
impl AccountDraft {
    fn from_account(account: &AccountSummary, preferences: &AccountPreferences) -> Self {
        let editor = |text: &str| {
            let mut state = composer::ComposerState::default();
            state.text = text.to_owned();
            state.caret = text.len();
            state
        };
        Self {
            label: editor(&account.label),
            login: editor(preferences.login.proxy().unwrap_or("")),
            service: editor(preferences.service.proxy().unwrap_or("")),
            enabled: preferences.enabled,
            applied_label: account.label.clone(),
            applied: Some(preferences.clone()),
            submitted: None,
            save_error: None,
        }
    }
    fn matches(&self, account: &AccountSummary, preferences: &AccountPreferences) -> bool {
        self.label.text == account.label
            && self.enabled == preferences.enabled
            && NetworkPolicy::from_input(&self.login.text).as_ref() == Ok(&preferences.login)
            && NetworkPolicy::from_input(&self.service.text).as_ref() == Ok(&preferences.service)
    }
    fn dirty(&self) -> bool {
        self.applied.as_ref().is_some_and(|applied| {
            self.label.text != self.applied_label
                || self.login.text != applied.login.proxy().unwrap_or("")
                || self.service.text != applied.service.proxy().unwrap_or("")
                || self.enabled != applied.enabled
        })
    }
    fn acknowledge(
        &mut self,
        account: &AccountSummary,
        preferences: &AccountPreferences,
        receipt: Option<&provider_runtime::AccountSaveReceipt>,
    ) {
        let Some(receipt) = receipt.filter(|receipt| {
            receipt.account == account.id && self.submitted == Some(receipt.request)
        }) else {
            return;
        };
        if receipt.result.is_ok() {
            // Acknowledge this form's save without replacing later edits,
            // selection, preedit or caret with the older submitted contents.
            self.applied_label.clone_from(&account.label);
            self.applied = Some(preferences.clone());
        }
        self.save_error = receipt.result.err();
        self.submitted = None;
    }
}

fn row_field(row: Row) -> Option<Field> {
    match row {
        Row::Port => Some(Field::Port),
        Row::ModelInput => Some(Field::Model),
        Row::Model => Some(Field::ProfileChooser),
        Row::AccountInput(field) | Row::ConnectInput(field) => Some(field),
        Row::ProfileInput(field) => Some(Field::Profile(field)),
        _ => None,
    }
}
fn field_limits(field: Field) -> Limits {
    match field {
        Field::Port => PORT_LIMITS,
        Field::Model => MODEL_LIMITS,
        Field::ProfileChooser => Limits {
            max_bytes: 0,
            preedit_chars: 0,
        },
        Field::Profile(ProfileField::Label) => Limits {
            max_bytes: 128,
            preedit_chars: 128,
        },
        Field::Profile(_) => Limits {
            max_bytes: 200,
            preedit_chars: 200,
        },
        Field::AccountLabel | Field::ConnectLabel => Limits {
            max_bytes: 128,
            preedit_chars: 128,
        },
        Field::LoginProxy
        | Field::ServiceProxy
        | Field::ConnectLoginProxy
        | Field::ConnectServiceProxy => Limits {
            max_bytes: 2048,
            preedit_chars: 256,
        },
    }
}

#[derive(Default)]
pub(super) struct HarnessSettings {
    runtime: Option<provider_runtime::Runtime>,
    view: View,
    port: composer::ComposerState,
    model: composer::ComposerState,
    forms: ConnectionForms,
    keys: KeyUi,
    monitoring: monitoring::MonitoringUi,
    open_usage: bool,
    pending_workspace_target: Option<super::harness_workspace_ui::SettingsTarget>,
    profile_navigation: Option<usize>,
    focused: Option<Field>,
    scroll: usize,
    drag: Option<usize>,
    clicks: minicon_core::click::ClickCounter<usize>,
    error: Option<String>,
    auth_error: Option<(provider_runtime::AttemptId, String)>,
    cancelled_attempt: Option<provider_runtime::AttemptId>,
    challenge: Option<DeviceChallenge>,
    challenge_expired: bool,
    account_focus: Option<String>,
    account_draft: AccountDraft,
    account_drafts: std::collections::HashMap<String, AccountDraft>,
    account_save_sequence: u64,
    connection_draft: ConnectionDraft,
}

impl HarnessSettings {
    pub(super) fn cached_workspace_view(&self) -> &View {
        &self.view
    }
    pub(super) fn show_workspace_target(
        &mut self,
        target: super::harness_workspace_ui::SettingsTarget,
    ) {
        use super::harness_workspace_ui::SettingsTarget;
        // Loading can insert profile rows and restore a default profile scroll.
        // Resolve the requested destination again after that initial transition.
        self.pending_workspace_target =
            (self.view.load_state == provider_runtime::LoadState::Loading).then(|| target.clone());
        self.blur();
        let marker = match target {
            SettingsTarget::Service => Row::Local,
            SettingsTarget::Accounts(account) => {
                if let Some(id) = account {
                    self.focus_account(&id);
                    Row::AccountInput(Field::AccountLabel)
                } else {
                    Row::Accounts
                }
            }
            SettingsTarget::Connection => Row::Model,
            SettingsTarget::Monitoring => {
                self.monitoring.sync(&self.view);
                self.monitoring.enter();
                Row::Monitoring(monitoring::Item::Heading)
            }
        };
        self.scroll = self
            .rows()
            .iter()
            .position(|row| match (row, marker) {
                (Row::Monitoring(a), Row::Monitoring(b)) => *a == b,
                _ => std::mem::discriminant(row) == std::mem::discriminant(&marker),
            })
            .unwrap_or(0);
    }

    fn finish_workspace_target(&mut self) {
        if self.view.load_state != provider_runtime::LoadState::Loading
            && let Some(target) = self.pending_workspace_target.take()
        {
            self.show_workspace_target(target);
        }
    }

    fn supersede_workspace_target(&mut self, event: &PixelWindowEvent) {
        if matches!(event, PixelWindowEvent::Keyboard(key) if key.state == KeyPressState::Pressed)
            || matches!(
                event,
                PixelWindowEvent::Ime(_)
                    | PixelWindowEvent::MouseWheel { .. }
                    | PixelWindowEvent::PointerButton {
                        state: PointerButtonState::Pressed,
                        ..
                    }
            )
        {
            self.pending_workspace_target = None;
        }
    }

    #[cfg(feature = "native-auth-court")]
    pub(super) fn for_native_auth_court(attempt: provider_runtime::AttemptId) -> Self {
        let view = View {
            busy: false,
            login_active: true,
            auth_attempt: Some(attempt),
            ..View::default()
        };
        let mut port = composer::ComposerState::default();
        port.text = view.port.to_string();
        port.caret = port.text.len();
        Self {
            view,
            port,
            ..Self::default()
        }
    }

    fn receive_device_challenge(
        &mut self,
        challenge: DeviceChallenge,
        dismissed: Option<provider_runtime::AttemptId>,
    ) -> bool {
        if !self.view.login_active
            || self.auth_attempt() != Some(challenge.attempt)
            || dismissed == Some(challenge.attempt)
            || challenge.deadline <= Instant::now()
        {
            return false;
        }
        self.challenge = Some(challenge);
        self.challenge_expired = false;
        true
    }

    pub(super) fn auth_attempt(&self) -> Option<provider_runtime::AttemptId> {
        self.view
            .auth_attempt
            .filter(|attempt| Some(*attempt) != self.cancelled_attempt)
    }
    pub(super) fn cancel_login(&mut self) {
        self.cancelled_attempt = self.view.auth_attempt;
        self.challenge = None;
        self.challenge_expired = false;
        let prior_error = self.error.take();
        self.act(Action::CancelLogin);
        if self.error.is_none() {
            self.error = prior_error;
        }
    }
    pub(super) fn auth_failed(&mut self, message: &str) {
        let attempt = self.view.auth_attempt;
        self.cancel_login();
        if let Some(attempt) = attempt {
            self.auth_error = Some((attempt, message.to_owned()));
        }
    }

    fn reconcile_auth_result(&mut self) {
        if let Some((attempt, _)) = &self.auth_error
            && (self
                .view
                .auth_attempt
                .is_some_and(|active| active != *attempt)
                || self.view.auth_result
                    == Some((*attempt, provider_runtime::AuthOutcome::CredentialStored)))
        {
            self.auth_error = None;
        }
    }
    fn new(window: &PixelWindow, language: ui::UiLanguage) -> Self {
        let root = config_path()
            .and_then(|path| path.parent().map(|p| p.join("harness")))
            .and_then(|path| {
                if path.is_absolute() {
                    Some(path)
                } else {
                    std::env::current_dir().ok().map(|cwd| cwd.join(path))
                }
            });
        let waker = window.waker();
        let result = root
            .ok_or(provider_runtime::RuntimeError::InvalidRoot)
            .and_then(|root| {
                provider_runtime::Runtime::new(
                    root,
                    provider_login::LoginMetadata {
                        version: env!("CARGO_PKG_VERSION").to_owned(),
                        locale: match language {
                            ui::UiLanguage::English => "en-US",
                            ui::UiLanguage::ChineseSimplified => "zh-CN",
                            ui::UiLanguage::ChineseTraditional => "zh-TW",
                        }
                        .to_owned(),
                        timezone_offset_seconds: 0,
                    },
                    std::sync::Arc::new(move || {
                        let _ = waker.wake();
                    }),
                )
            });
        let load_failure = result.as_ref().err().copied();
        let (runtime, error) = match result {
            Ok(runtime) => (Some(runtime), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let view = runtime
            .as_ref()
            .map(|runtime| runtime.view().clone())
            .unwrap_or_else(|| View {
                load_state: provider_runtime::LoadState::Unavailable(
                    load_failure.unwrap_or(provider_runtime::RuntimeError::WorkerUnavailable),
                ),
                busy: false,
                ..View::default()
            });
        let mut port = composer::ComposerState::default();
        port.text = view.port.to_string();
        port.caret = port.text.len();
        Self {
            runtime,
            view,
            port,
            model: composer::ComposerState::default(),
            forms: ConnectionForms::default(),
            keys: KeyUi::default(),
            monitoring: Default::default(),
            open_usage: false,
            pending_workspace_target: None,
            profile_navigation: None,
            focused: None,
            scroll: 0,
            drag: None,
            clicks: Default::default(),
            error,
            auth_error: None,
            cancelled_attempt: None,
            challenge: None,
            challenge_expired: false,
            account_focus: None,
            account_draft: AccountDraft::default(),
            account_drafts: Default::default(),
            account_save_sequence: 0,
            connection_draft: Default::default(),
        }
    }

    pub(super) fn select_monitoring_account(&mut self, account: Option<&str>) {
        self.monitoring.select(account);
        self.monitoring.enter();
    }
    fn monitoring_action(&mut self, item: monitoring::Item) {
        let request = self
            .runtime
            .as_ref()
            .and_then(|r| r.next_account_monitoring_request().ok());
        match self.monitoring.activate(item, &self.view, request) {
            monitoring::UiAction::None => {}
            monitoring::UiAction::OpenReport => self.open_usage = true,
            monitoring::UiAction::Send(action) => self.act(action),
        }
    }
    fn rows(&self) -> Vec<Row> {
        let mut rows = vec![
            Row::Local,
            Row::Listener,
            Row::Status,
            Row::Port,
            Row::ListenerActions,
            Row::Accounts,
            Row::ConnectInput(Field::ConnectLabel),
            Row::ConnectInput(Field::ConnectLoginProxy),
            Row::ConnectInput(Field::ConnectServiceProxy),
            Row::ConnectNetworkHint,
            Row::Login,
            Row::LoginActions,
        ];
        if self.challenge.is_some() || self.challenge_expired {
            rows.extend([Row::Challenge, Row::ChallengeActions]);
        }
        if self.view.accounts.is_empty() {
            rows.push(Row::NoAccounts);
        }
        rows.extend((0..self.view.accounts.len()).map(Row::Account));
        if self.account_focus.is_some() {
            rows.extend([
                Row::AccountInput(Field::AccountLabel),
                Row::AccountInput(Field::LoginProxy),
                Row::AccountInput(Field::ServiceProxy),
                Row::AccountNetworkHint,
                Row::AccountActions,
                Row::AccountConnection,
                Row::AccountReconnect,
                Row::AccountReconnectHint,
                Row::CatalogueStatus,
                Row::CatalogueRefresh,
            ]);
            if let Some(catalogue) = self
                .view
                .catalogues
                .iter()
                .find(|entry| Some(&entry.account) == self.account_focus.as_ref())
            {
                rows.extend((0..catalogue.models.len()).map(Row::CatalogueModel));
            }
        }
        rows.push(Row::Model);
        rows.extend((0..self.view.connection_profiles.len()).map(Row::Profile));
        rows.push(Row::ProfileCreate);
        if let Some(form) = self.forms.current() {
            rows.push(Row::ProfileState);
            if self.forms.selected.as_deref() != Some("default") {
                rows.extend([
                    Row::ProfileInput(ProfileField::Label),
                    Row::ProfileToggle,
                    Row::ProfileInput(ProfileField::DefaultAlias),
                ]);
            }
            rows.push(Row::ProfileApplied);
            for index in 0..self.applied_allocations().len() {
                rows.extend([
                    Row::AppliedAlias(index),
                    Row::AppliedAccount(index),
                    Row::AppliedModel(index),
                ]);
            }
            rows.extend(form.allocations.keys().copied().map(Row::Allocation));
            rows.push(Row::AllocationActions);
            if let Some(slot) = form.selected_slot {
                if self.forms.selected.as_deref() != Some("default") {
                    rows.push(Row::ProfileInput(ProfileField::Alias(slot)));
                }
                rows.push(Row::AllocationAccount);
                if form.account_picker {
                    rows.extend((0..self.view.accounts.len()).map(Row::AllocationAccountChoice));
                }
            }
            rows.extend([
                Row::ModelInput,
                Row::ModelActions,
                Row::VerificationActions,
                Row::VerificationCancel,
                Row::VerificationRetry,
                Row::ProbeNotice,
            ]);
            if let Some(catalogue) = self.current_catalogue()
                && catalogue.status == provider_runtime::CatalogueStatus::Fresh
            {
                rows.extend((0..catalogue.models.len()).map(Row::ModelChoice));
            }
            rows.push(Row::ProfileCommands);
            rows.push(Row::ProfileVerification);
        } else {
            // Loading/unavailable does not invent an authoritative default form.
            rows.extend([Row::ProfileState, Row::ModelInput]);
        }
        rows.extend([
            Row::Connect,
            Row::KeyState,
            Row::KeyDurability,
            Row::KeyFeedback,
            Row::KeyApplied,
            Row::KeyShell,
            Row::KeyActions,
            Row::OpenAi,
            Row::Anthropic,
            Row::Recipe,
            Row::RecipePi,
        ]);
        rows.extend(
            self.monitoring
                .rows(&self.view)
                .into_iter()
                .map(Row::Monitoring),
        );
        rows
    }

    fn port_editable(&self) -> bool {
        !self.view.busy
            && matches!(
                self.view.listener,
                ListenerState::Stopped | ListenerState::Error
            )
    }

    fn editor(&self, field: Field) -> &composer::ComposerState {
        match field {
            Field::Port => &self.port,
            Field::Model => self
                .forms
                .current()
                .map_or(&self.model, |form| form.model()),
            Field::ProfileChooser => unreachable!("profile chooser is not a text editor"),
            Field::Profile(field) => self
                .forms
                .current()
                .expect("published profile field")
                .field(field),
            Field::AccountLabel => &self.account_draft.label,
            Field::LoginProxy => &self.account_draft.login,
            Field::ServiceProxy => &self.account_draft.service,
            Field::ConnectLabel => &self.connection_draft.label,
            Field::ConnectLoginProxy => &self.connection_draft.login,
            Field::ConnectServiceProxy => &self.connection_draft.service,
        }
    }

    fn editor_mut(&mut self, field: Field) -> &mut composer::ComposerState {
        match field {
            Field::Port => &mut self.port,
            Field::Model => self
                .forms
                .current_mut()
                .map_or(&mut self.model, |form| form.model_mut()),
            Field::ProfileChooser => unreachable!("profile chooser is not a text editor"),
            Field::Profile(field) => self
                .forms
                .current_mut()
                .expect("published profile field")
                .field_mut(field),
            Field::AccountLabel => &mut self.account_draft.label,
            Field::LoginProxy => &mut self.account_draft.login,
            Field::ServiceProxy => &mut self.account_draft.service,
            Field::ConnectLabel => &mut self.connection_draft.label,
            Field::ConnectLoginProxy => &mut self.connection_draft.login,
            Field::ConnectServiceProxy => &mut self.connection_draft.service,
        }
    }

    fn blur(&mut self) {
        self.monitoring.blur();
        self.keys.invalidate_copy();
        self.remember_connection_focus();
        self.forms.blur();
        field_editor::apply(&mut self.port, Command::Blur, PORT_LIMITS);
        field_editor::apply(&mut self.model, Command::Blur, MODEL_LIMITS);
        for field in [
            Field::AccountLabel,
            Field::LoginProxy,
            Field::ServiceProxy,
            Field::ConnectLabel,
            Field::ConnectLoginProxy,
            Field::ConnectServiceProxy,
        ] {
            field_editor::apply(self.editor_mut(field), Command::Blur, field_limits(field));
        }
        self.focused = None;
        self.drag = None;
        self.clicks = Default::default();
    }

    pub(super) fn request_usage_clear(
        &mut self,
    ) -> Result<u64, crate::provider_server::usage::UsageClearError> {
        use crate::provider_server::usage::UsageClearError;
        let map_error = |error| match error {
            provider_runtime::RuntimeError::Busy => UsageClearError::Busy,
            provider_runtime::RuntimeError::Storage => UsageClearError::Storage,
            provider_runtime::RuntimeError::InvalidAction => UsageClearError::InvalidRequest,
            _ => UsageClearError::Unavailable,
        };
        let runtime = self.runtime.as_mut().ok_or(UsageClearError::Unavailable)?;
        let request = runtime.next_usage_clear_request().map_err(map_error)?;
        runtime
            .act(Action::ClearUsageHistory { request })
            .map_err(map_error)?;
        Ok(request)
    }

    fn act(&mut self, action: Action) {
        if matches!(action, Action::Stop) {
            self.keys.invalidate_copy();
        }
        let result = self.runtime.as_mut().map_or(
            Err(provider_runtime::RuntimeError::WorkerUnavailable),
            |runtime| runtime.act(action.clone()),
        );
        if let Err(error) = result {
            self.forms.send_failed(&action, error);
            self.monitoring.send_failed(&action, error);
        }
        self.error = result.err().map(|error| error.to_string());
    }

    fn current_catalogue(&self) -> Option<&provider_runtime::AccountCatalogueView> {
        let account = self.forms.current()?.selected_account()?;
        self.view
            .catalogues
            .iter()
            .find(|entry| entry.account == account)
    }

    fn applied_allocations(&self) -> &[provider_runtime::ConnectionAllocationInput] {
        self.forms
            .current()
            .and_then(|form| form.view.applied.as_ref())
            .map_or(&[], |applied| applied.allocations.as_slice())
    }

    fn remember_connection_focus(&mut self) {
        if let Some(form) = self.forms.current_mut() {
            let focus = match self.focused {
                Some(Field::Model) => Some(FormFocus::Model(form.selected_slot)),
                Some(Field::Profile(field)) => Some(FormFocus::Field(field)),
                _ => None,
            };
            if let Some(focus) = focus {
                form.remember_focus(focus);
            }
        }
    }

    fn note_connection_activity(&mut self, field: Field) {
        if let Some(form) = self.forms.current_mut() {
            match field {
                Field::Model => {
                    form.note_model_activity();
                }
                Field::Profile(field) => {
                    form.note_field_activity(field);
                }
                _ => {}
            }
        }
    }

    fn restore_connection_focus(&mut self) {
        self.focused = self
            .forms
            .current_mut()
            .and_then(|form| form.restore_focus())
            .map(|focus| match focus {
                FormFocus::Model(_) => Field::Model,
                FormFocus::Field(field) => Field::Profile(field),
            });
    }

    fn navigate_connection_profiles(&mut self, delta: isize) -> Option<usize> {
        if self.view.connection_profiles.is_empty() {
            return None;
        }
        let current = self
            .profile_navigation
            .or_else(|| {
                self.view
                    .connection_profiles
                    .iter()
                    .position(|profile| Some(&profile.id) == self.forms.selected.as_ref())
            })
            .unwrap_or(0);
        let next = current
            .saturating_add_signed(delta)
            .min(self.view.connection_profiles.len() - 1);
        self.profile_navigation = Some(next);
        Some(next)
    }

    fn confirm_connection_profile(&mut self) {
        let Some(index) = self.profile_navigation else {
            return;
        };
        if let Some(profile) = self.view.connection_profiles.get(index) {
            let id = profile.id.clone();
            self.focus_connection(&id);
        }
    }

    fn flush_connection_edits(&mut self) {
        // At most one request per form. Rejected generations remain blocked
        // until a new local edit or an explicit Retry.
        for _ in 0..8 {
            let Some(action) = self.forms.next_edit() else {
                break;
            };
            self.act(action);
        }
    }

    fn sync_connection_forms(&mut self) {
        let previous = self.forms.selected.clone();
        self.forms.sync(&self.view);
        self.sync_key_ui();
        if previous != self.forms.selected {
            self.focused = None;
            self.drag = None;
            self.clicks = Default::default();
            let initial = self
                .rows()
                .iter()
                .position(|row| matches!(row, Row::Model))
                .unwrap_or(0);
            self.scroll = self.forms.current().map_or(self.scroll, |form| {
                if previous.is_some() && !form.visited {
                    initial
                } else {
                    form.scroll
                }
            });
            self.restore_connection_focus();
        }
        if let Some(Field::Profile(field)) = self.focused
            && !self
                .forms
                .current()
                .is_some_and(|form| form.has_field(field))
        {
            self.blur();
        }
        if matches!(self.focused, Some(Field::Model | Field::Profile(_)))
            && self
                .forms
                .current_mut()
                .and_then(|form| form.restore_focus())
                .is_none()
        {
            self.focused = None;
        }
        self.flush_connection_edits();
    }

    fn focus_connection(&mut self, id: &str) {
        self.blur();
        let initial = self
            .rows()
            .iter()
            .position(|row| matches!(row, Row::Model))
            .unwrap_or(0);
        if let Some(form) = self.forms.forms.get_mut(id)
            && !form.visited
        {
            form.scroll = initial;
            form.visited = true;
        }
        if let Some(scroll) = self.forms.focus(id, self.scroll) {
            self.scroll = scroll;
            self.sync_key_ui();
            self.restore_connection_focus();
            self.act(Action::FocusConnectionProfile(id.into()));
        }
    }

    fn connection_command(&mut self, change: ConnectionChange, choice: Option<(String, String)>) {
        if let Some(action) = self.forms.command(change, choice) {
            self.act(action);
        }
    }

    fn focus_account(&mut self, id: &str) {
        let current = self
            .view
            .accounts
            .iter()
            .find(|account| account.id == id)
            .cloned();
        let preferences = self
            .view
            .account_preferences
            .iter()
            .find(|(account, _)| account == id)
            .map(|(_, preferences)| preferences.clone());
        let (Some(account), Some(preferences)) = (current, preferences) else {
            return;
        };
        if self.account_focus.as_deref() != Some(id) {
            self.blur();
            if let Some(previous) = self.account_focus.replace(id.to_owned()) {
                self.account_drafts
                    .insert(previous, std::mem::take(&mut self.account_draft));
            }
            self.account_draft = self
                .account_drafts
                .remove(id)
                .unwrap_or_else(|| AccountDraft::from_account(&account, &preferences));
        }
        if (self.account_draft.applied.as_ref() != Some(&preferences)
            || self.account_draft.applied_label != account.label)
            && self.account_draft.submitted.is_none()
            && (!self.account_draft.dirty() || self.account_draft.matches(&account, &preferences))
        {
            let save_error = self.account_draft.save_error;
            self.account_draft = AccountDraft::from_account(&account, &preferences);
            self.account_draft.save_error = save_error;
        }
    }

    fn status(&self) -> Option<String> {
        self.error
            .clone()
            .or_else(|| {
                self.forms
                    .current()
                    .and_then(|form| form.local_error.clone())
            })
            .or_else(|| self.forms.error.clone())
            .or_else(|| self.auth_error.as_ref().map(|(_, message)| message.clone()))
            .or_else(|| self.account_draft.save_error.map(|error| error.to_string()))
            .or_else(|| {
                self.view
                    .config_durability_unconfirmed
                    .then(|| provider_runtime::CONFIG_DURABILITY_WARNING.to_owned())
            })
            .or_else(|| self.view.status.clone())
    }

    fn sync_account_drafts(&mut self) {
        for account in &self.view.accounts {
            if let Some((_, preferences)) = self
                .view
                .account_preferences
                .iter()
                .find(|(id, _)| id == &account.id)
            {
                let draft = if self.account_focus.as_ref() == Some(&account.id) {
                    Some(&mut self.account_draft)
                } else {
                    self.account_drafts.get_mut(&account.id)
                };
                if let Some(draft) = draft {
                    draft.acknowledge(
                        account,
                        preferences,
                        self.view
                            .account_save_receipts
                            .iter()
                            .find(|receipt| receipt.account == account.id),
                    );
                }
            }
        }
        self.account_drafts
            .retain(|id, _| self.view.accounts.iter().any(|account| &account.id == id));
        if !self
            .account_focus
            .as_ref()
            .is_some_and(|id| self.view.accounts.iter().any(|account| &account.id == id))
        {
            if self.account_focus.is_some() {
                self.blur();
            }
            self.account_focus = None;
            self.account_draft = AccountDraft::default();
        }
        let id = self
            .account_focus
            .clone()
            .or_else(|| self.view.selected_account.clone())
            .or_else(|| self.view.accounts.first().map(|account| account.id.clone()));
        if let Some(id) = id {
            self.focus_account(&id);
        }
    }

    fn save_account(&mut self) {
        if self.account_draft.submitted.is_some() {
            return;
        }
        let (Some(account), Some(expected)) = (
            self.account_focus.clone(),
            self.account_draft.applied.clone(),
        ) else {
            return;
        };
        let login = NetworkPolicy::from_input(&self.account_draft.login.text);
        let service = NetworkPolicy::from_input(&self.account_draft.service.text);
        let (Ok(login), Ok(service)) = (login, service) else {
            self.error =
                Some("Use a plain HTTP proxy URL, or leave the field empty for Direct.".into());
            return;
        };
        let Some(request) = self.account_save_sequence.checked_add(1) else {
            self.error = Some("Account save sequence exhausted. Reopen MiniCon.".into());
            return;
        };
        self.account_save_sequence = request;
        self.act(Action::SaveAccount {
            request,
            edit: Box::new(AccountEdit {
                account,
                label: self.account_draft.label.text.clone(),
                login,
                service,
                enabled: self.account_draft.enabled,
                expected,
                expected_label: self.account_draft.applied_label.clone(),
            }),
        });
        if self.error.is_none() {
            self.account_draft.submitted = Some(request);
            self.account_draft.save_error = None;
        }
    }

    fn connect_account(&mut self, provider: Provider) {
        let login = NetworkPolicy::from_input(&self.connection_draft.login.text);
        let service = NetworkPolicy::from_input(&self.connection_draft.service.text);
        let (Ok(login), Ok(service)) = (login, service) else {
            self.error = Some(
                "Choose a supported HTTP proxy, or leave each network field empty for Direct."
                    .into(),
            );
            return;
        };
        self.connection_draft.provider = provider;
        self.act(Action::Connect {
            provider,
            label: self.connection_draft.label.text.clone(),
            login,
            service,
        });
    }

    fn connection_text_changed(&mut self, field: Field) {
        self.invalidate_connection_data();
        if let Some(id) = self.forms.selected.clone()
            && let Some(form) = self.forms.current_mut()
        {
            match field {
                Field::Model => {
                    form.commit_model_edit(&id);
                }
                Field::Profile(field) => {
                    form.edit_field(field);
                }
                _ => {}
            }
        }
        self.flush_connection_edits();
    }
    fn invalidate_connection_data(&self) {
        if let Some(profile) = self.forms.selected.as_deref()
            && let Some(runtime) = &self.runtime
        {
            runtime.invalidate_connection_profile(profile);
        }
    }
    fn connection_input(&self) -> Option<provider_runtime::ConnectionDraftInput> {
        self.forms.current().map(|form| form.input())
    }
    fn invalidate_changed_input(&self, before: Option<provider_runtime::ConnectionDraftInput>) {
        if before != self.connection_input() {
            self.invalidate_connection_data();
        }
    }
    fn profile_plan(&self) -> Option<&provider_runtime::ConnectionPlanView> {
        self.view
            .connection_plans
            .iter()
            .find(|plan| Some(&plan.profile) == self.forms.selected.as_ref())
    }
    fn submit(&mut self, field: Field) {
        match field {
            Field::Port if self.port_editable() => {
                if let Ok(port) = self.port.text.parse::<u16>()
                    && port >= 1024
                    && self.port.text.bytes().all(|ch| ch.is_ascii_digit())
                {
                    self.act(Action::SetPort(port));
                } else {
                    self.error = Some("Enter a port from 1024 to 65535.".to_owned());
                }
            }
            Field::Port => {
                self.error = Some("Stop the listener before changing its port.".to_owned())
            }
            Field::Model | Field::Profile(_) => {
                self.forms.retry();
                self.flush_connection_edits();
            }
            Field::ProfileChooser => self.confirm_connection_profile(),
            Field::AccountLabel | Field::LoginProxy | Field::ServiceProxy => self.save_account(),
            Field::ConnectLabel | Field::ConnectLoginProxy | Field::ConnectServiceProxy => {
                self.connect_account(self.connection_draft.provider)
            }
        }
    }

    fn activate(&mut self, row: Row, column: usize) -> Option<String> {
        if self.forms.current().is_some_and(|form| form.discarding())
            && matches!(
                row,
                Row::ProfileToggle
                    | Row::AllocationActions
                    | Row::AllocationAccountChoice(_)
                    | Row::ModelChoice(_)
                    | Row::ModelInput
                    | Row::ModelActions
                    | Row::ProfileCommands
            )
        {
            self.error =
                Some("This unapplied connection is being removed. Wait before editing it.".into());
            return None;
        }
        match row {
            Row::Monitoring(monitoring::Item::Threshold(i)) if column == 1 => {
                self.monitoring_action(monitoring::Item::RemoveThreshold(i))
            }
            Row::Monitoring(item) => self.monitoring_action(item),
            Row::Port if column == 1 => self.submit(Field::Port),
            Row::ListenerActions => self.act(if column == 0 {
                Action::Start
            } else {
                Action::Stop
            }),
            Row::Login => self.connect_account(if column == 0 {
                Provider::DeepSeek
            } else {
                Provider::OpenCode
            }),
            Row::LoginActions => {
                if column == 0 {
                    self.cancel_login();
                } else {
                    self.act(Action::RefreshAccounts);
                }
            }
            Row::ChallengeActions => {
                if let Some(challenge) = &self.challenge
                    && Instant::now() < challenge.deadline
                {
                    if column == 0 {
                        let _ = clipboard_status::set_text(challenge.user_code.expose());
                    } else {
                        return Some(challenge.verification_url.expose().to_owned());
                    }
                } else if self.challenge_expired {
                    self.error = Some("Device code expired. Add the provider again.".to_owned());
                }
            }
            Row::Account(index) => {
                let id = self.view.accounts[index].id.clone();
                if column == 0 {
                    self.focus_account(&id);
                } else {
                    self.act(Action::RemoveAccount(id));
                }
            }
            Row::AccountActions if column == 0 => self.save_account(),
            Row::AccountActions => {
                if self.account_draft.submitted.is_some() {
                    return None;
                }
                if let Some(id) = self.account_focus.clone() {
                    self.account_draft = AccountDraft::default();
                    self.focus_account(&id);
                }
            }
            Row::AccountConnection if column == 0 => {
                self.scroll = self
                    .rows()
                    .iter()
                    .position(|row| matches!(row, Row::Model))
                    .unwrap_or(0);
            }
            Row::AccountConnection => {
                self.account_draft.enabled = !self.account_draft.enabled;
            }
            Row::AccountReconnect => {
                if self.account_draft.dirty() || self.account_draft.submitted.is_some() {
                    self.error = Some("Save or revert account changes before reconnecting.".into());
                } else if let Some(account) = self.account_focus.clone() {
                    self.act(Action::Reconnect(account));
                }
            }
            Row::CatalogueRefresh => {
                if let Some(account) = self.account_focus.clone() {
                    self.act(Action::RefreshAccountModels(account));
                }
            }
            Row::CatalogueStatus | Row::CatalogueModel(_) => {}
            Row::Profile(index) => {
                self.profile_navigation = Some(index);
                self.confirm_connection_profile();
            }
            Row::ProfileCreate => {
                if let Some(action) = self.forms.create() {
                    self.act(action);
                }
            }
            Row::ProfileToggle => {
                let before = self.connection_input();
                if let Some(form) = self.forms.current_mut() {
                    form.toggle_enabled();
                }
                self.invalidate_changed_input(before);
                self.flush_connection_edits();
            }
            Row::Allocation(slot) => {
                if let Some(form) = self.forms.current_mut() {
                    form.blur();
                    form.selected_slot = Some(slot);
                    form.account_picker = false;
                }
            }
            Row::AllocationActions if column == 0 => {
                let before = self.connection_input();
                if let Some(id) = self.forms.selected.clone()
                    && let Some(form) = self.forms.current_mut()
                {
                    form.add_slot(&id);
                }
                self.invalidate_changed_input(before);
                self.flush_connection_edits();
            }
            Row::AllocationActions => self.connection_command(ConnectionChange::Remove, None),
            Row::AllocationAccount => {
                if let Some(form) = self.forms.current_mut() {
                    form.account_picker = !form.account_picker;
                }
            }
            Row::AllocationAccountChoice(index) => {
                let before = self.connection_input();
                let account = self.view.accounts[index].id.clone();
                if let Some(form) = self.forms.current_mut() {
                    form.assign_account(&account);
                }
                self.invalidate_changed_input(before);
                self.flush_connection_edits();
            }
            Row::ModelInput if column == 1 => self.submit(Field::Model),
            Row::ModelActions if column == 0 => {
                if let Some(account) = self
                    .forms
                    .current()
                    .and_then(|form| form.selected_account())
                    .map(str::to_owned)
                {
                    self.act(Action::RefreshAccountModels(account));
                } else {
                    self.error = Some(
                        "Choose an account for this model slot before refreshing its catalogue."
                            .into(),
                    );
                }
            }
            Row::ModelActions => {
                self.forms.retry();
                self.flush_connection_edits();
            }
            Row::ProfileVerification => {
                self.flush_connection_edits();
                if self.view.busy || self.view.login_active {
                    self.error =
                        Some("Wait for the active operation before verifying a profile.".into());
                } else if let Some(plan) = self.profile_plan().cloned() {
                    if plan.error.is_some() {
                        self.error = plan.error.map(|error| error.to_string());
                    } else if plan.paid_slots.is_empty() {
                        self.connection_command(ConnectionChange::Apply, None);
                    } else if let Some(action) = self.forms.verify_profile(&plan) {
                        self.act(action);
                    }
                }
            }
            Row::VerificationActions => {
                if let Some(action) = self.forms.verify() {
                    self.act(action);
                }
            }
            Row::VerificationCancel => {
                let request = if let Some(active) = &self.view.connection_check_active {
                    (active.phase != provider_runtime::ConnectionCheckPhase::Applying)
                        .then_some(active.request)
                } else {
                    self.forms.publication_request()
                };
                if let Some(request) = request {
                    self.act(Action::CancelConnectionVerification { request });
                }
            }
            Row::VerificationRetry => {
                if let Some(terminal) = &self.view.connection_check_last_terminal
                    && terminal.warning
                        == Some(provider_runtime::ConnectionCheckWarning::ReadinessStorage)
                {
                    let request = terminal.request;
                    self.act(Action::RetryConnectionReadiness { request });
                }
            }
            Row::ModelChoice(index) => {
                if let Some(catalogue) = self.current_catalogue()
                    && catalogue.status == provider_runtime::CatalogueStatus::Fresh
                    && let Some(model) = catalogue.models.get(index)
                {
                    let choice = (catalogue.account.clone(), model.id.clone());
                    self.connection_command(ConnectionChange::Choose, Some(choice));
                }
            }
            Row::ProfileCommands if column == 0 => {
                if self
                    .forms
                    .current()
                    .is_some_and(|form| form.view.applied.is_some())
                {
                    self.connection_command(ConnectionChange::Revert, None);
                } else if self.forms.selected.as_deref() != Some("default") {
                    self.connection_command(ConnectionChange::Discard, None);
                } else {
                    self.error=Some("There is no applied default connection to revert. Remove its model entry to clear this draft.".into());
                }
            }
            Row::ProfileCommands => {
                self.flush_connection_edits();
                if self.view.busy || self.view.login_active {
                    self.error = Some(
                        "Wait for the active operation before Apply. No paid request was sent."
                            .into(),
                    );
                } else {
                    self.connection_command(ConnectionChange::Apply, None);
                }
            }
            Row::KeyActions => self.key_action(match column {
                0 => provider_runtime::ProfileKeyOperation::Create,
                1 => provider_runtime::ProfileKeyOperation::Copy,
                _ => provider_runtime::ProfileKeyOperation::Revoke,
            }),
            Row::Recipe => {
                self.copy_connection_template(RecipeKind::Worker, clipboard_status::set_text)
            }
            Row::RecipePi => {
                self.copy_connection_template(RecipeKind::Pi, clipboard_status::set_text)
            }
            _ => {}
        }
        None
    }
}

fn invalidate_key_for_settings_route(settings: Option<&mut HarnessSettings>, visible: bool) {
    if !visible && let Some(settings) = settings {
        settings.keys.invalidate_copy();
    }
}

fn key_button_caption(language: ui::UiLanguage, column: usize) -> &'static str {
    match (language, column) {
        (ui::UiLanguage::ChineseSimplified, 0) => "创建",
        (ui::UiLanguage::ChineseSimplified, 1) => "复制",
        (ui::UiLanguage::ChineseSimplified, _) => "撤销",
        (ui::UiLanguage::ChineseTraditional, 0) => "建立",
        (ui::UiLanguage::ChineseTraditional, 1) => "複製",
        (ui::UiLanguage::ChineseTraditional, _) => "撤銷",
        (_, 0) => "Create",
        (_, 1) => "Copy",
        (_, _) => "Revoke",
    }
}
fn key_button_fits(rect: ui::Rect, caption: &str, size: u16, scale: f64) -> bool {
    rect.height >= font::cell_metrics(size).height
        && rect.width.saturating_sub((16.0 * scale) as u32)
            >= host_ui::host_ui_text_width(caption, size)
}

fn row_columns(row: Row) -> usize {
    match row {
        Row::KeyActions => 3,
        Row::Monitoring(monitoring::Item::Threshold(_)) => 2,
        Row::Port
        | Row::ListenerActions
        | Row::Login
        | Row::LoginActions
        | Row::ChallengeActions
        | Row::Account(_)
        | Row::AccountInput(_)
        | Row::ConnectInput(_)
        | Row::AccountActions
        | Row::AccountConnection
        | Row::ModelInput
        | Row::ModelActions
        | Row::ProfileInput(_)
        | Row::AllocationActions
        | Row::ProfileCommands => 2,
        _ => 1,
    }
}

fn row_id(row: Row) -> String {
    match row {
        Row::Monitoring(item) => return monitoring::MonitoringUi::id(item),
        Row::Local => "local_api",
        Row::Listener => "listener",
        Row::Port => "port",
        Row::ListenerActions => "listener_actions",
        Row::Accounts => "accounts",
        Row::ConnectInput(Field::ConnectLabel) => "connect_label",
        Row::ConnectInput(Field::ConnectLoginProxy) => "connect_login_proxy",
        Row::ConnectInput(Field::ConnectServiceProxy) => "connect_service_proxy",
        Row::ConnectInput(_) => "connect_input",
        Row::ConnectNetworkHint => "connect_network_hint",
        Row::Login => "login",
        Row::LoginActions => "login_actions",
        Row::Challenge => "device_challenge",
        Row::ChallengeActions => "device_actions",
        Row::NoAccounts => "no_accounts",
        Row::AccountInput(Field::AccountLabel) => "account_label",
        Row::AccountInput(Field::LoginProxy) => "account_login_proxy",
        Row::AccountInput(Field::ServiceProxy) => "account_service_proxy",
        Row::AccountInput(_) => "account_input",
        Row::AccountActions => "account_edit_actions",
        Row::AccountConnection => "account_connection",
        Row::AccountReconnect => "account_reconnect",
        Row::AccountReconnectHint => "account_reconnect_hint",
        Row::AccountNetworkHint => "account_network_hint",
        Row::CatalogueStatus => "account_catalogue_status",
        Row::CatalogueRefresh => "account_catalogue_refresh",
        Row::CatalogueModel(index) => return format!("account_catalogue_model_{index}"),
        Row::Model => "model",
        Row::ModelInput => "model_input",
        Row::ModelActions => "model_actions",
        Row::VerificationActions => "model_verification",
        Row::VerificationCancel => "model_verification_cancel",
        Row::VerificationRetry => "model_readiness_retry",
        Row::ProbeNotice => "probe_notice",
        Row::Connect => "connect",
        Row::KeyState => "key_state",
        Row::KeyDurability => "key_durability",
        Row::KeyFeedback => "key_feedback",
        Row::KeyApplied => "key_applied_binding",
        Row::KeyShell => "key_shell_setup",
        Row::KeyActions => "key_actions",
        Row::OpenAi => "openai_base_url",
        Row::Anthropic => "anthropic_base_url",
        Row::Recipe => "recipe",
        Row::RecipePi => "recipe_pi",
        Row::Status => "status",
        Row::Account(index) => return format!("account_{index}"),
        Row::ModelChoice(index) => return format!("model_{index}"),
        Row::Profile(index) => return format!("connection_profile_{index}"),
        Row::ProfileCreate => "connection_profile_create",
        Row::ProfileState => "connection_profile_state",
        Row::ProfileInput(ProfileField::Label) => "connection_profile_label",
        Row::ProfileInput(ProfileField::DefaultAlias) => "connection_default_alias",
        Row::ProfileInput(ProfileField::Alias(slot)) => return format!("connection_alias_{slot}"),
        Row::ProfileToggle => "connection_profile_enabled",
        Row::ProfileApplied => "connection_applied",
        Row::AppliedAlias(index) => return format!("connection_applied_{index}_alias"),
        Row::AppliedAccount(index) => return format!("connection_applied_{index}_account"),
        Row::AppliedModel(index) => return format!("connection_applied_{index}_model"),
        Row::Allocation(slot) => return format!("connection_allocation_{slot}"),
        Row::AllocationActions => "connection_allocation_actions",
        Row::AllocationAccount => "connection_allocation_account",
        Row::AllocationAccountChoice(index) => return format!("connection_account_{index}"),
        Row::ProfileCommands => "connection_profile_commands",
        Row::ProfileVerification => "profile_verification",
    }
    .to_owned()
}

impl ConApp {
    pub(super) fn paint_harness_settings(
        &self,
        surface: &mut Surface<'_>,
        layout: ui::Layout,
        scale: f64,
        size: u16,
    ) {
        if !self.settings_open || self.settings_section != ui::SettingsSection::Harness {
            return;
        }
        let theme = theme::Theme::for_choice(self.ui_theme);
        let Some(settings) = &self.harness_settings else {
            return;
        };
        let rows = settings.rows();
        let geometry = layout.harness_settings(scale, rows.len(), settings.scroll);
        surface.fill_rect(
            geometry.panel.x,
            geometry.panel.y,
            geometry.panel.width,
            geometry.panel.height,
            theme.panel_bg.to_xrgb(),
        );
        stroke_rect(surface, geometry.panel, scale.max(1.0) as u32, theme.border);
        host_ui::paint_settings_routes(
            surface,
            geometry.routes,
            ui::SettingsSection::Harness,
            theme,
            size,
            scale,
        );
        let tr =
            |english: &'static str, chinese: &'static str, traditional: &'static str| match self
                .ui_language
            {
                ui::UiLanguage::English => english,
                ui::UiLanguage::ChineseSimplified => chinese,
                ui::UiLanguage::ChineseTraditional => traditional,
            };
        let button = |surface: &mut Surface<'_>, rect: ui::Rect, label: &str, active: bool| {
            surface.fill_rect(
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                if active { theme.accent } else { theme.surface }.to_xrgb(),
            );
            paint_host_ui_text(
                surface,
                rect.x + (8.0 * scale) as u32,
                rect.y + rect.height.saturating_sub(font::cell_metrics(size).height) / 2,
                label,
                if active { theme.panel_bg } else { theme.text },
                size,
                rect.width.saturating_sub((16.0 * scale) as u32),
            );
        };
        for (index, &row) in rows
            .iter()
            .enumerate()
            .skip(geometry.rows.start)
            .take(geometry.rows.visible_rows)
        {
            let rect = geometry.cell(index, 0, 1, scale);
            let cell = |column, count| geometry.cell(index, column, count, scale);
            let mut label = String::new();
            let mut heading = false;
            match row {
                Row::Monitoring(item) => {
                    let summary = self
                        .presented_harness_usage()
                        .unwrap_or(settings.view.usage.as_ref());
                    let label =
                        settings
                            .monitoring
                            .label(item, &settings.view, summary, self.ui_language);
                    if let Some(editor) = settings.monitoring.editor(item) {
                        host_ui::paint_single_line_host_editor(
                            surface,
                            cell(0, 2),
                            editor,
                            settings.monitoring.editor_focused(item),
                            &label,
                            theme,
                            size,
                            scale,
                        );
                        button(surface, cell(1, 2), tr("Remove", "移除", "移除"), false);
                    } else {
                        button(
                            surface,
                            rect,
                            &label,
                            settings.monitoring.current() == Some(item),
                        );
                    }
                }
                Row::Local => {
                    label = "Local API".to_owned();
                    heading = true;
                }
                Row::Accounts => {
                    label = "Provider accounts".to_owned();
                    heading = true;
                }
                Row::Model => {
                    label = tr(
                        "Connection drafts / applied connection unchanged",
                        "连接草稿 / 当前连接保持不变",
                        "連線草稿 / 目前連線保持不變",
                    )
                    .to_owned();
                    if settings.focused == Some(Field::ProfileChooser) {
                        stroke_rect(surface, rect, scale.max(1.0) as u32, theme.accent);
                        label.push_str(tr(
                            " / ↑↓ choose, Enter open",
                            " / ↑↓选择，回车打开",
                            " / ↑↓選擇，Enter開啟",
                        ));
                    }
                    heading = true;
                }
                Row::Connect => {
                    label = tr(
                        "Connect the selected applied profile",
                        "连接所选已应用连接",
                        "連接所選已套用連線",
                    )
                    .to_owned();
                    heading = true;
                }
                Row::Listener => {
                    label = format!(
                        "Listener: {:?}{}",
                        settings.view.listener,
                        if settings.view.busy { " / working" } else { "" }
                    )
                }
                Row::Port => {
                    host_ui::paint_single_line_host_editor(
                        surface,
                        cell(0, 2),
                        &settings.port,
                        settings.focused == Some(Field::Port),
                        "Port (1024-65535)",
                        theme,
                        size,
                        scale,
                    );
                    button(
                        surface,
                        cell(1, 2),
                        if settings.port_editable() {
                            "Save port"
                        } else {
                            "Port locked"
                        },
                        false,
                    );
                }
                Row::ListenerActions => {
                    button(
                        surface,
                        cell(0, 2),
                        "Start",
                        settings.view.listener == ListenerState::Listening,
                    );
                    button(surface, cell(1, 2), "Stop", false);
                }
                Row::Login => {
                    button(
                        surface,
                        cell(0, 2),
                        "Continue with DeepSeek",
                        settings.connection_draft.provider == Provider::DeepSeek,
                    );
                    button(
                        surface,
                        cell(1, 2),
                        "Continue with OpenCode",
                        settings.connection_draft.provider == Provider::OpenCode,
                    );
                }
                Row::LoginActions => {
                    button(
                        surface,
                        cell(0, 2),
                        "Cancel login",
                        settings.view.login_active,
                    );
                    button(surface, cell(1, 2), "Refresh accounts", false);
                }
                Row::Challenge => {
                    // The code exists only in this transient interactive layer.
                    if let Some(challenge) = &settings.challenge {
                        label = format!(
                            "Device code ({}s): {}",
                            challenge.remaining_seconds,
                            challenge.user_code.expose()
                        );
                    } else if settings.challenge_expired {
                        label = "Device code expired. Start login again.".to_owned();
                    }
                }
                Row::ChallengeActions => {
                    if settings.challenge_expired {
                        button(surface, cell(0, 2), "Code expired", false);
                        button(surface, cell(1, 2), "Verification expired", false);
                    } else {
                        button(surface, cell(0, 2), "Copy device code", false);
                        button(surface, cell(1, 2), "Open verification", false);
                    }
                }
                Row::Account(account_index) => {
                    let account = &settings.view.accounts[account_index];
                    let state = match account.state {
                        provider_accounts::AccountState::CredentialStored => {
                            "stored; test required"
                        }
                        provider_accounts::AccountState::AccountReady => "ready",
                    };
                    button(
                        surface,
                        cell(0, 2),
                        &format!(
                            "{} [{}] / {state}",
                            account.label,
                            account.id.get(..8).unwrap_or(&account.id)
                        ),
                        settings.account_focus.as_ref() == Some(&account.id),
                    );
                    button(surface, cell(1, 2), "Remove account", false);
                }
                Row::AccountInput(field) | Row::ConnectInput(field) => {
                    let name = match field {
                        Field::AccountLabel => "Account name",
                        Field::LoginProxy => "Login proxy",
                        Field::ServiceProxy => "Service proxy",
                        Field::ConnectLabel => "New account label (optional)",
                        Field::ConnectLoginProxy => "Login proxy (blank = Direct)",
                        Field::ConnectServiceProxy => "Service proxy (blank = Direct)",
                        _ => "Account setting",
                    };
                    host_ui::paint_single_line_host_editor(
                        surface,
                        cell(0, 2),
                        settings.editor(field),
                        settings.focused == Some(field),
                        name,
                        theme,
                        size,
                        scale,
                    );
                    paint_host_ui_text(
                        surface,
                        cell(1, 2).x + (8.0 * scale) as u32,
                        rect.y + rect.height.saturating_sub(font::cell_metrics(size).height) / 2,
                        name,
                        theme.text,
                        size,
                        cell(1, 2).width.saturating_sub((16.0 * scale) as u32),
                    );
                }
                Row::AccountActions => {
                    button(
                        surface,
                        cell(0, 2),
                        if settings.account_draft.submitted.is_some() {
                            "Saving account..."
                        } else {
                            "Save account"
                        },
                        settings.account_draft.dirty(),
                    );
                    button(surface, cell(1, 2), "Revert changes", false);
                }
                Row::AccountConnection => {
                    button(
                        surface,
                        cell(0, 2),
                        tr("Open connection drafts", "打开连接草稿", "開啟連線草稿"),
                        settings.account_focus == settings.view.selected_account,
                    );
                    button(
                        surface,
                        cell(1, 2),
                        if settings.account_draft.enabled {
                            "Disable (save to apply)"
                        } else {
                            "Enable (save to apply)"
                        },
                        false,
                    );
                }
                Row::AccountNetworkHint => {
                    label = "Empty proxy = Direct. Login and service networks are independent."
                        .to_owned();
                }
                Row::AccountReconnect => {
                    button(surface, cell(0, 1), "Reconnect this account", false);
                }
                Row::CatalogueRefresh => {
                    button(
                        surface,
                        cell(0, 1),
                        "Refresh this account's catalogue",
                        false,
                    );
                }
                Row::CatalogueStatus => {
                    let catalogue = settings
                        .view
                        .catalogues
                        .iter()
                        .find(|entry| Some(&entry.account) == settings.account_focus.as_ref());
                    label = catalogue.map_or_else(
                        || "Catalogue: Not loaded".into(),
                        |entry| {
                            format!(
                                "Catalogue: {:?} · {} models · read only{}",
                                entry.status,
                                entry.models.len(),
                                if entry.error.is_some() {
                                    " · refresh failed"
                                } else {
                                    ""
                                }
                            )
                        },
                    );
                }
                Row::CatalogueModel(index) => {
                    if let Some(model) = settings
                        .view
                        .catalogues
                        .iter()
                        .find(|entry| Some(&entry.account) == settings.account_focus.as_ref())
                        .and_then(|entry| entry.models.get(index))
                    {
                        label = format!("{} · {:?} · read only", model.id, model.protocol);
                    }
                }
                Row::AccountReconnectHint => {
                    label = "Uses saved login settings. Connections to this account require model verification again after reconnecting.".into();
                }
                Row::ConnectNetworkHint => {
                    label =
                        "New account: choose separate login and service networks, then Continue."
                            .to_owned()
                }
                Row::NoAccounts => label = "No account. Add a provider to continue.".to_owned(),
                Row::Profile(index) => {
                    let profile = &settings.view.connection_profiles[index];
                    let local = settings.forms.forms.get(&profile.id);
                    let name = local.map_or(profile.input.label.as_str(), |form| {
                        form.label.text.as_str()
                    });
                    button(
                        surface,
                        rect,
                        &format!(
                            "{}{} [{}]{}",
                            if settings.focused == Some(Field::ProfileChooser)
                                && settings.profile_navigation == Some(index)
                            {
                                "› "
                            } else {
                                ""
                            },
                            name,
                            if profile.id == "default" {
                                "default"
                            } else {
                                profile.id.get(..8).unwrap_or(&profile.id)
                            },
                            if profile.dirty { " / staged" } else { "" }
                        ),
                        settings.forms.selected.as_deref() == Some(profile.id.as_str()),
                    );
                }
                Row::ProfileCreate => button(
                    surface,
                    rect,
                    if settings.forms.create_request.is_some() {
                        tr("Creating profile...", "正在创建连接…", "正在建立連線…")
                    } else {
                        tr(
                            "New connection profile (up to 8 total)",
                            "新建连接（最多 8 个）",
                            "建立連線（最多 8 個）",
                        )
                    },
                    false,
                ),
                Row::ProfileState => {
                    label = settings.forms.current().map_or_else(
                        || format!("Connection drafts: {:?}", settings.view.load_state),
                        |form| {
                            format!(
                                "Draft / {} / {}",
                                if form.waiting() {
                                    "saving changes"
                                } else if form.unsent() {
                                    "local edits"
                                } else {
                                    "staged"
                                },
                                form.local_error.clone().unwrap_or_else(|| form
                                    .view
                                    .error
                                    .map_or_else(
                                        || "not applied".into(),
                                        |error| format!("{error:?}")
                                    ))
                            )
                        },
                    );
                }
                Row::ProfileInput(field) => {
                    let name = match field {
                        ProfileField::Label => tr("Connection label", "连接名称", "連線名稱"),
                        ProfileField::DefaultAlias => {
                            tr("Default public alias", "默认公开别名", "預設公開別名")
                        }
                        ProfileField::Alias(_) => {
                            tr("Public model alias", "公开模型别名", "公開模型別名")
                        }
                    };
                    host_ui::paint_single_line_host_editor(
                        surface,
                        cell(0, 2),
                        settings.editor(Field::Profile(field)),
                        settings.focused == Some(Field::Profile(field)),
                        name,
                        theme,
                        size,
                        scale,
                    );
                    paint_host_ui_text(
                        surface,
                        cell(1, 2).x + (8.0 * scale) as u32,
                        rect.y + rect.height.saturating_sub(font::cell_metrics(size).height) / 2,
                        name,
                        theme.text,
                        size,
                        cell(1, 2).width.saturating_sub((16.0 * scale) as u32),
                    );
                }
                Row::ProfileToggle => button(
                    surface,
                    rect,
                    if settings.forms.current().is_some_and(|form| form.enabled) {
                        tr(
                            "Enabled in draft (click to disable)",
                            "草稿已启用（点击停用）",
                            "草稿已啟用（點擊停用）",
                        )
                    } else {
                        tr(
                            "Disabled in draft (click to enable)",
                            "草稿已停用（点击启用）",
                            "草稿已停用（點擊啟用）",
                        )
                    },
                    false,
                ),
                Row::ProfileApplied => {
                    label = settings.forms.current().map_or_else(String::new, |form| {
                        form.view.applied.as_ref().map_or_else(
                            || "Applied: none / this is an unsaved profile".into(),
                            |applied| {
                                format!(
                                    "Applied: {} / {} model allocation(s); edits are separate",
                                    applied.label,
                                    applied.allocations.len()
                                )
                            },
                        )
                    });
                }
                Row::AppliedAlias(index)
                | Row::AppliedAccount(index)
                | Row::AppliedModel(index) => {
                    if let Some(allocation) = settings.applied_allocations().get(index) {
                        let account = allocation.account.as_deref().map_or_else(
                            || tr("No account", "未选择账号", "未選擇帳號").into(),
                            |id| {
                                settings
                                    .view
                                    .accounts
                                    .iter()
                                    .find(|account| account.id == id)
                                    .map_or_else(
                                        || {
                                            format!(
                                                "{} [{}]",
                                                tr(
                                                    "Unavailable account",
                                                    "账号不可用",
                                                    "帳號不可用"
                                                ),
                                                id.get(..8).unwrap_or(id)
                                            )
                                        },
                                        |account| {
                                            format!(
                                                "{} [{}]",
                                                account.label,
                                                id.get(..8).unwrap_or(id)
                                            )
                                        },
                                    )
                            },
                        );
                        label = match row {
                            Row::AppliedAlias(_) => format!(
                                "{}: {}",
                                tr("Applied alias", "生效别名", "生效別名"),
                                allocation.alias
                            ),
                            Row::AppliedAccount(_) => format!(
                                "{}: {}",
                                tr("Applied account", "生效账号", "生效帳號"),
                                account
                            ),
                            _ => format!(
                                "{}: {}",
                                tr("Applied model", "生效模型", "生效模型"),
                                allocation.model
                            ),
                        };
                    }
                }
                Row::Allocation(slot) => {
                    if let Some(form) = settings.forms.current()
                        && let Some(allocation) = form.allocations.get(&slot)
                    {
                        let same_input = form.view.input.allocations.iter().any(|old| {
                            old.slot == slot
                                && old.alias == allocation.alias.text
                                && old.account == allocation.account
                                && old.model == allocation.model.text
                        });
                        let state = form
                            .view
                            .allocations
                            .iter()
                            .find(|entry| same_input && entry.slot == slot)
                            .map_or_else(
                                || tr("Choose a model", "请选择模型", "請選擇模型").into(),
                                |entry| {
                                    use provider_runtime::{
                                        ConnectionAvailability as A, ConnectionEvidence as E,
                                    };
                                    let availability = match entry.availability {
                                        A::Available => {
                                            tr("Account available", "账号可用", "帳號可用")
                                        }
                                        A::MissingAccount => {
                                            tr("Account removed", "账号已移除", "帳號已移除")
                                        }
                                        A::DisabledAccount => {
                                            tr("Account disabled", "账号已停用", "帳號已停用")
                                        }
                                        A::CredentialUnavailable => {
                                            tr("Sign in required", "需要登录", "需要登入")
                                        }
                                        A::AuthorityChanged => tr(
                                            "Account changed; choose again",
                                            "账号已变更，请重新选择",
                                            "帳號已變更，請重新選擇",
                                        ),
                                        A::NotSelected => {
                                            tr("Choose a model", "请选择模型", "請選擇模型")
                                        }
                                    };
                                    let evidence = match entry.evidence {
                                        E::Unverified => tr("Not verified", "尚未验证", "尚未驗證"),
                                        E::LegacyApplied => {
                                            tr("Previously applied", "已应用旧连接", "已套用舊連線")
                                        }
                                        E::Verified => tr("Verified", "已验证", "已驗證"),
                                        E::StaleAuthority => tr(
                                            "Verification out of date",
                                            "验证已过期",
                                            "驗證已過期",
                                        ),
                                    };
                                    format!("{availability} / {evidence}")
                                },
                            );
                        button(
                            surface,
                            rect,
                            &format!(
                                "{} / {} / {}",
                                allocation.alias.text, allocation.model.text, state
                            ),
                            form.selected_slot == Some(slot),
                        );
                    }
                }
                Row::AllocationActions => {
                    button(
                        surface,
                        cell(0, 2),
                        tr("Add model entry", "添加模型", "新增模型"),
                        false,
                    );
                    button(
                        surface,
                        cell(1, 2),
                        tr("Remove selected entry", "移除选中模型", "移除所選模型"),
                        false,
                    );
                }
                Row::AllocationAccount => {
                    let account = settings
                        .forms
                        .current()
                        .and_then(|form| form.selected_account());
                    let name = account.map_or_else(
                        || "Not selected".into(),
                        |id| {
                            settings
                                .view
                                .accounts
                                .iter()
                                .find(|entry| entry.id == id)
                                .map_or_else(
                                    || format!("Unavailable [{}]", id.get(..8).unwrap_or(id)),
                                    |entry| {
                                        format!("{} [{}]", entry.label, id.get(..8).unwrap_or(id))
                                    },
                                )
                        },
                    );
                    button(
                        surface,
                        rect,
                        &format!("Model account: {name} / choose"),
                        false,
                    );
                }
                Row::AllocationAccountChoice(index) => {
                    let account = &settings.view.accounts[index];
                    button(
                        surface,
                        rect,
                        &format!(
                            "Choose {} [{}]",
                            account.label,
                            account.id.get(..8).unwrap_or(&account.id)
                        ),
                        settings
                            .forms
                            .current()
                            .and_then(|form| form.selected_account())
                            == Some(account.id.as_str()),
                    );
                }
                Row::ModelInput => {
                    host_ui::paint_single_line_host_editor(
                        surface,
                        cell(0, 2),
                        settings.editor(Field::Model),
                        settings.focused == Some(Field::Model),
                        tr(
                            "Model selector (choose a catalogue row)",
                            "模型（从目录中选择）",
                            "模型（從目錄中選擇）",
                        ),
                        theme,
                        size,
                        scale,
                    );
                    button(
                        surface,
                        cell(1, 2),
                        tr("Stage / retry edits", "暂存 / 重试编辑", "暫存 / 重試編輯"),
                        false,
                    );
                }
                Row::ModelActions => {
                    button(
                        surface,
                        cell(0, 2),
                        tr(
                            "Refresh this account catalogue",
                            "刷新此账号模型目录",
                            "重新整理此帳號模型目錄",
                        ),
                        false,
                    );
                    button(
                        surface,
                        cell(1, 2),
                        tr("Retry staged edits", "重试暂存编辑", "重試暫存編輯"),
                        false,
                    );
                }
                Row::ProfileVerification => {
                    let caption =
                        profile_verification_label(self.ui_language, settings.profile_plan());
                    paint_cost_button(surface, cell(0, 1), &caption, &theme, size, scale);
                }
                Row::VerificationActions => {
                    paint_verification_button(
                        surface,
                        cell(0, 1),
                        self.ui_language,
                        &theme,
                        size,
                        scale,
                    );
                }
                Row::VerificationCancel => {
                    button(
                        surface,
                        cell(0, 1),
                        &settings.view.connection_check_active.as_ref().map_or_else(
                            || "No active model check".into(),
                            |active| {
                                if active.phase == provider_runtime::ConnectionCheckPhase::Applying
                                {
                                    "Applying; commit admitted (cannot cancel)".into()
                                } else {
                                    format!("Cancel check #{}", active.request)
                                }
                            },
                        ),
                        false,
                    );
                }
                Row::VerificationRetry => {
                    if settings
                        .view
                        .connection_check_last_terminal
                        .as_ref()
                        .is_some_and(|check| {
                            check.warning
                                == Some(provider_runtime::ConnectionCheckWarning::ReadinessStorage)
                        })
                    {
                        button(
                            surface,
                            rect,
                            "Retry readiness storage (no upstream request)",
                            false,
                        );
                    } else {
                        label = settings
                            .view
                            .connection_check_active
                            .as_ref()
                            .or(settings.view.connection_check_last_terminal.as_ref())
                            .map_or_else(
                                || "No model check requested.".into(),
                                |check| {
                                    format!(
                                        "Check #{} / {} / slot {}: {:?} {}/{} {:?}{}",
                                        check.request,
                                        check.profile,
                                        check.slot,
                                        check.scope,
                                        check.completed,
                                        check.total,
                                        check.phase,
                                        check.error.map_or_else(String::new, |error| format!(
                                            " / {error}"
                                        ))
                                    )
                                },
                            );
                    }
                }
                Row::ProbeNotice => label = settings
                    .forms
                    .current()
                    .and_then(|form| form.local_error.clone())
                    .unwrap_or_else(|| {
                        "Output ≤8 tokens; input may also be billed. No automatic retry or Apply."
                            .into()
                    }),
                Row::ModelChoice(index) => {
                    if let Some(model) = settings
                        .current_catalogue()
                        .and_then(|entry| entry.models.get(index))
                    {
                        button(
                            surface,
                            rect,
                            &format!("Choose {} / {:?}", model.id, model.protocol),
                            false,
                        );
                    }
                }
                Row::ProfileCommands => {
                    button(
                        surface,
                        cell(0, 2),
                        if settings
                            .forms
                            .current()
                            .is_some_and(|form| form.view.applied.is_some())
                        {
                            tr("Revert this profile", "还原此连接", "還原此連線")
                        } else if settings.forms.selected.as_deref() == Some("default") {
                            tr(
                                "No applied connection to revert",
                                "尚无已应用连接可还原",
                                "尚無已套用連線可還原",
                            )
                        } else {
                            tr(
                                "Discard unapplied profile",
                                "放弃未应用连接",
                                "捨棄未套用連線",
                            )
                        },
                        false,
                    );
                    button(
                        surface,
                        cell(1, 2),
                        if settings.view.busy
                            || settings.view.login_active
                            || !settings.forms.can_apply()
                        {
                            tr("Apply (not ready)", "应用（未就绪）", "套用（未就緒）")
                        } else {
                            tr("Apply", "应用", "套用")
                        },
                        settings.forms.can_apply()
                            && !settings.view.busy
                            && !settings.view.login_active,
                    );
                }
                Row::KeyState => label = settings.key_state_label(),
                Row::KeyDurability => {
                    label = if settings.selected_key_durability_unconfirmed() {
                        "Crash durability: unconfirmed"
                    } else {
                        "No outstanding key durability warning"
                    }
                    .into()
                }
                Row::KeyFeedback => label = settings.key_feedback_label(),
                Row::KeyApplied => label = settings.key_applied_label(),
                Row::KeyShell => {
                    label = if cfg!(windows) {
                        "PowerShell: private prompt in setup"
                    } else {
                        "bash: private prompt in setup"
                    }
                    .into()
                }
                Row::KeyActions => {
                    for column in 0..3 {
                        let caption = key_button_caption(self.ui_language, column);
                        button(
                            surface,
                            cell(column, 3),
                            if key_button_fits(cell(column, 3), caption, size, scale) {
                                caption
                            } else {
                                "..."
                            },
                            false,
                        );
                    }
                }
                Row::OpenAi => label = settings.endpoint_label(true),
                Row::Anthropic => label = settings.endpoint_label(false),
                Row::Recipe => button(
                    surface,
                    rect,
                    tr("Copy worker setup", "复制 Worker 设置", "複製 Worker 設定"),
                    false,
                ),
                Row::RecipePi => button(
                    surface,
                    rect,
                    tr("Copy Pi 1.4 setup", "复制 Pi 1.4 设置", "複製 Pi 1.4 設定"),
                    false,
                ),
                Row::Status => {
                    label = settings
                        .status()
                        .unwrap_or_else(|| "Start and model tests are explicit actions.".to_owned())
                }
            }
            if !label.is_empty() {
                paint_host_ui_text(
                    surface,
                    rect.x + (4.0 * scale) as u32,
                    rect.y + rect.height.saturating_sub(font::cell_metrics(size).height) / 2,
                    &label,
                    if heading { theme.accent } else { theme.text },
                    size,
                    rect.width.saturating_sub((8.0 * scale) as u32),
                );
            }
        }
        button(surface, geometry.previous, "Previous", false);
        button(surface, geometry.next, "Next", false);
    }

    pub(super) fn harness_settings_snapshot(
        &self,
        layout: ui::Layout,
        scale: f64,
    ) -> json::JsonValue {
        let Some(settings) = &self.harness_settings else {
            return json::JsonValue::Null;
        };
        let rows = settings.rows();
        let geometry = layout.harness_settings(scale, rows.len(), settings.scroll);
        let rect = |r: ui::Rect| {
            json::object(vec![
                ("x", r.x.into()),
                ("y", r.y.into()),
                ("width", r.width.into()),
                ("height", r.height.into()),
            ])
        };
        let active = self.settings_open
            && self.settings_section == ui::SettingsSection::Harness
            && !self.new_tab_menu_open
            && !self.browser_proxy_menu_open;
        let editor = |field| {
            let state = settings.editor(field);
            json::object(vec![
                ("text", state.text.as_str().into()),
                ("caret", state.caret.into()),
                (
                    "selection",
                    composer::selection_bounds(state)
                        .map_or(json::JsonValue::Null, |(start, end)| {
                            json::JsonValue::Array(vec![start.into(), end.into()])
                        }),
                ),
                ("preedit", state.preedit.as_str().into()),
                (
                    "focused",
                    (active && settings.focused == Some(field)).into(),
                ),
            ])
        };
        // Whitelist observations: no credentials, labels, device URL/code or recipe key.
        json::object(vec![
            ("panel", rect(geometry.panel)),
            ("monitoring", settings.monitoring.snapshot(&settings.view)),
            ("monitoring_usage_revision", self.presented_harness_usage().map_or(json::JsonValue::Null,|s|s.revision.into())),
            ("port", settings.view.port.into()),
            (
                "actual_port",
                settings
                    .view
                    .actual_port
                    .map_or(json::JsonValue::Null, Into::into),
            ),
            (
                "listener",
                format!("{:?}", settings.view.listener)
                    .to_ascii_lowercase()
                    .into(),
            ),
            ("busy", settings.view.busy.into()),
            ("login_active", settings.view.login_active.into()),
            ("local_key_exists", settings.view.local_key_exists.into()),
            ("client_key", settings.key_snapshot()),
            (
                "selected_account",
                settings
                    .view
                    .selected_account
                    .as_deref()
                    .map_or(json::JsonValue::Null, Into::into),
            ),
            ("model", settings.view.model.as_str().into()),
            (
                "catalogue_retained_bytes",
                settings.view.catalogue_bytes.into(),
            ),
            (
                "catalogues",
                json::JsonValue::Array(
                    settings
                        .view
                        .catalogues
                        .iter()
                        .map(|entry| {
                            json::object(vec![
                                ("account", entry.account.as_str().into()),
                                ("retained_rows", entry.retained_rows.into()),
                                (
                                    "state",
                                    format!("{:?}", entry.status).to_ascii_lowercase().into(),
                                ),
                                (
                                    "error",
                                    entry.error.map_or(json::JsonValue::Null, |error| {
                                        error.to_string().into()
                                    }),
                                ),
                                (
                                    "models",
                                    json::JsonValue::Array(
                                        entry
                                            .models
                                            .iter()
                                            .map(|model| {
                                                json::object(vec![
                                                    ("id", model.id.as_str().into()),
                                                    (
                                                        "upstream_model",
                                                        model.upstream_model.as_str().into(),
                                                    ),
                                                    (
                                                        "protocol",
                                                        format!("{:?}", model.protocol)
                                                            .to_ascii_lowercase()
                                                            .into(),
                                                    ),
                                                ])
                                            })
                                            .collect(),
                                    ),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "accounts",
                json::JsonValue::Array(
                    settings
                        .view
                        .accounts
                        .iter()
                        .map(|account| {
                            json::object(vec![
                                ("id", account.id.as_str().into()),
                                (
                                    "provider",
                                    match account.provider {
                                        Provider::DeepSeek => "deepseek",
                                        Provider::OpenCode => "opencode",
                                    }
                                    .into(),
                                ),
                                (
                                    "ready",
                                    (account.state
                                        == provider_accounts::AccountState::AccountReady)
                                        .into(),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "status",
                settings
                    .status()
                    .as_deref()
                    .map_or(json::JsonValue::Null, Into::into),
            ),
            ("port_editor", editor(Field::Port)),
            ("model_editor", editor(Field::Model)),
            ("connection_profile",settings.forms.selected.as_deref().map_or(json::JsonValue::Null,Into::into)),
            ("connection_profile_navigation",settings.profile_navigation.map_or(json::JsonValue::Null,Into::into)),
            ("connection_picker_focused",(active&&settings.focused==Some(Field::ProfileChooser)).into()),
            ("connection_plans",json::JsonValue::Array(settings.view.connection_plans.iter().map(|plan|json::object(vec![
                ("profile",plan.profile.as_str().into()),("revision",plan.revision.into()),("id",plan.id.into()),
                ("paid_slots",json::JsonValue::Array(plan.paid_slots.iter().map(|slot|u32::from(*slot).into()).collect())),
                ("error",plan.error.map_or(json::JsonValue::Null,|error|format!("{error:?}").into()))])).collect())),
            ("connection_check_active",check_observation(settings.view.connection_check_active.as_ref())),
            ("connection_check_last_terminal",check_observation(settings.view.connection_check_last_terminal.as_ref())),
            ("connection_profiles",json::JsonValue::Array(settings.view.connection_profiles.iter().map(|profile|{
                let form=settings.forms.forms.get(&profile.id);
                let input=form.map_or_else(||profile.input.clone(),|form|form.input());
                json::object(vec![
                    ("id",profile.id.as_str().into()),("revision",profile.revision.into()),
                    ("enabled",input.enabled.into()),("default_alias",input.default_alias.into()),
                    ("dirty",profile.dirty.into()),("has_applied",profile.applied.is_some().into()),
                    ("applied",profile.applied.as_ref().map_or(json::JsonValue::Null,|applied|json::object(vec![
                        ("enabled",applied.enabled.into()),("default_alias",applied.default_alias.as_str().into()),
                        ("allocations",json::JsonValue::Array(applied.allocations.iter().map(|row|json::object(vec![
                            ("slot",u32::from(row.slot).into()),("alias",row.alias.as_str().into()),
                            ("account",row.account.as_deref().map_or(json::JsonValue::Null,Into::into)),("model",row.model.as_str().into()),
                            ("account_present",row.account.as_ref().is_some_and(|id|settings.view.accounts.iter().any(|account|&account.id==id)).into()),
                        ])).collect())),
                    ]))),
                    ("pending",form.is_some_and(|form|form.waiting()).into()),
                    ("local_unsent",form.is_some_and(|form|form.unsent()).into()),
                    ("error",form.and_then(|form|form.local_error.as_deref()).map_or_else(||profile.error.map_or(json::JsonValue::Null,|error|format!("{error:?}").into()),Into::into)),
                    ("selected_slot",form.and_then(|form|form.selected_slot).map_or(json::JsonValue::Null,|slot|u32::from(slot).into())),
                    ("allocations",json::JsonValue::Array(input.allocations.iter().map(|allocation|{
                        let same_input=profile.input.allocations.iter().any(|old|old==allocation);
                        let observation=profile.allocations.iter().find(|entry|same_input&&entry.slot==allocation.slot);
                        json::object(vec![
                            ("slot",u32::from(allocation.slot).into()),("alias",allocation.alias.as_str().into()),
                            ("account",allocation.account.as_deref().map_or(json::JsonValue::Null,Into::into)),("model",allocation.model.as_str().into()),
                            ("availability",observation.map_or(json::JsonValue::Null,|entry|format!("{:?}",entry.availability).into())),
                            ("evidence",observation.map_or(json::JsonValue::Null,|entry|format!("{:?}",entry.evidence).into())),
                            ("upstream_model",observation.and_then(|entry|entry.upstream_model.as_deref()).map_or(json::JsonValue::Null,Into::into)),
                            ("protocol",observation.and_then(|entry|entry.protocol).map_or(json::JsonValue::Null,|protocol|format!("{protocol:?}").into())),
                        ])
                    }).collect())),
                ])
            }).collect())),
            ("connection_receipts",json::JsonValue::Array(settings.view.connection_receipts.iter().map(|receipt|json::object(vec![
                ("request",receipt.request.into()),("profile",receipt.profile.as_str().into()),
                ("change",format!("{:?}",receipt.change).into()),
                ("revision",receipt.revision.map_or(json::JsonValue::Null,Into::into)),
                ("ok",receipt.result.is_ok().into()),("error",receipt.result.err().map_or(json::JsonValue::Null,|error|error.to_string().into())),
            ])).collect())),
            ("connection_editors",settings.forms.current().map_or(json::JsonValue::Null,|form|{
                json::object(vec![
                    // Profile names receive the same snapshot redaction as account names.
                    ("label_bytes",form.label.text.len().into()),("label_caret",form.label.caret.into()),
                    ("label_focused",(active&&settings.focused==Some(Field::Profile(ProfileField::Label))).into()),
                    ("default_alias",if settings.forms.selected.as_deref()==Some("default"){json::JsonValue::Null}else{editor(Field::Profile(ProfileField::DefaultAlias))}),
                    ("alias",form.selected_slot.filter(|_|settings.forms.selected.as_deref()!=Some("default")).map_or(json::JsonValue::Null,|slot|editor(Field::Profile(ProfileField::Alias(slot))))),
                ])
            })),
            (
                "account_focus",
                settings
                    .account_focus
                    .as_deref()
                    .map_or(json::JsonValue::Null, Into::into),
            ),
            ("account_draft_dirty", settings.account_draft.dirty().into()),
            ("scroll", geometry.rows.start.into()),
            ("row_count", rows.len().into()),
            ("visible_rows", geometry.rows.visible_rows.into()),
            ("previous", rect(geometry.previous)),
            ("next", rect(geometry.next)),
            (
                "rows",
                json::JsonValue::Array(
                    (geometry.rows.start..geometry.rows.start + geometry.rows.visible_rows)
                        .map(|index| {
                            json::object(vec![
                                ("id", row_id(rows[index]).into()),
                                ("rect", rect(geometry.rows.row(index))),
                                (
                                    "cells",
                                    json::JsonValue::Array(
                                        (0..row_columns(rows[index]))
                                            .map(|column| {
                                                rect(geometry.cell(
                                                    index,
                                                    column,
                                                    row_columns(rows[index]),
                                                    scale,
                                                ))
                                            })
                                            .collect(),
                                    ),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }

    /// Synchronous close/route-transition hook: close->reopen between polls
    /// must not preserve a clipboard intent. The common repaint path calls it.
    pub(super) fn invalidate_inactive_harness_key_copy(&mut self) {
        invalidate_key_for_settings_route(
            self.harness_settings.as_mut(),
            self.settings_open && self.settings_section == ui::SettingsSection::Harness,
        );
    }

    pub(super) fn blur_harness_fields(&mut self) {
        if let Some(settings) = &mut self.harness_settings {
            settings.blur();
        }
    }

    pub(super) fn begin_harness_login(&mut self, window: &PixelWindow, provider: Provider) {
        self.close_browser_menu(window, false, false);
        self.blur_browser_composition();
        self.browser_address_focused = false;
        self.browser_proxy_focused = false;
        self.settings_proxy_focused = false;
        self.composer.focused = false;
        self.settings_open = true;
        self.settings_section = ui::SettingsSection::Harness;
        self.poll_harness_settings(window);
        if let Some(settings) = &mut self.harness_settings {
            settings.blur();
            settings.connection_draft.provider = provider;
            settings.focused = Some(Field::ConnectLabel);
        }
        self.sync_embedded_page(window);
        self.mark_host_ui_full_and_repaint(window);
    }

    pub(super) fn cancel_harness_pointer(&mut self) {
        if let Some(settings) = &mut self.harness_settings {
            settings.drag = None;
            field_editor::apply(&mut settings.port, Command::Blur, PORT_LIMITS);
            field_editor::apply(&mut settings.model, Command::Blur, MODEL_LIMITS);
            settings.forms.blur();
            for field in [
                Field::AccountLabel,
                Field::LoginProxy,
                Field::ServiceProxy,
                Field::ConnectLabel,
                Field::ConnectLoginProxy,
                Field::ConnectServiceProxy,
            ] {
                field_editor::apply(
                    settings.editor_mut(field),
                    Command::Blur,
                    field_limits(field),
                );
            }
        }
    }

    pub(super) fn harness_input_owner(&self) -> &'static str {
        match self
            .harness_settings
            .as_ref()
            .and_then(|settings| settings.focused)
        {
            Some(Field::Port) => "harness_port",
            Some(Field::Model) => "harness_model",
            Some(Field::Profile(_)) => "harness_connection",
            Some(Field::ProfileChooser) => "harness_connection_picker",
            Some(Field::AccountLabel) => "harness_account_label",
            Some(Field::LoginProxy) => "harness_login_proxy",
            Some(Field::ServiceProxy) => "harness_service_proxy",
            Some(Field::ConnectLabel) => "harness_connect_label",
            Some(Field::ConnectLoginProxy) => "harness_connect_login_proxy",
            Some(Field::ConnectServiceProxy) => "harness_connect_service_proxy",
            None => "harness_settings",
        }
    }

    pub(super) fn poll_harness_settings(&mut self, window: &PixelWindow) {
        if self.harness_settings.is_none()
            && self.settings_open
            && self.settings_section == ui::SettingsSection::Harness
        {
            self.harness_settings = Some(HarnessSettings::new(window, self.ui_language));
            self.mark_host_ui_full_and_repaint(window);
        }
        let Some(settings) = &mut self.harness_settings else {
            return;
        };
        if !self.settings_open || self.settings_section != ui::SettingsSection::Harness {
            settings.keys.invalidate_copy();
            settings.monitoring.blur();
            // Menus suspend a settings field; real route changes explicitly blur.
            field_editor::apply(&mut settings.port, Command::Blur, PORT_LIMITS);
            field_editor::apply(&mut settings.model, Command::Blur, MODEL_LIMITS);
            settings.forms.blur();
            for field in [
                Field::AccountLabel,
                Field::LoginProxy,
                Field::ServiceProxy,
                Field::ConnectLabel,
                Field::ConnectLoginProxy,
                Field::ConnectServiceProxy,
            ] {
                field_editor::apply(
                    settings.editor_mut(field),
                    Command::Blur,
                    field_limits(field),
                );
            }
            settings.drag = None;
        }
        let effects = settings
            .runtime
            .as_mut()
            .map_or_else(Vec::new, |runtime| runtime.poll());
        let mut changed = false;
        if let Some(challenge) = &mut settings.challenge {
            let now = Instant::now();
            if now >= challenge.deadline {
                settings.challenge = None;
                settings.challenge_expired = true;
                changed = true;
            } else {
                let seconds = challenge
                    .deadline
                    .saturating_duration_since(now)
                    .as_secs()
                    .max(1);
                if challenge.remaining_seconds != seconds {
                    challenge.remaining_seconds = seconds;
                    changed = true;
                }
            }
        }
        if let Some(runtime) = &settings.runtime
            && runtime.view() != &settings.view
        {
            let next = runtime.view().clone();
            // Keep the authoritative View/effects current; coalesce only usage raster.
            let usage_only = {
                let previous_usage =
                    std::mem::replace(&mut settings.view.usage, next.usage.clone());
                let unchanged = next == settings.view;
                settings.view.usage = previous_usage;
                unchanged
            };
            if settings.port.text == settings.view.port.to_string() {
                settings.port.text = next.port.to_string();
                settings.port.caret = settings.port.text.len();
                settings.port.anchor = None;
                settings.port.select_all = false;
            }
            if !next.login_active {
                settings.challenge = None;
                settings.challenge_expired = false;
            }
            settings.view = next;
            settings.reconcile_auth_result();
            settings.sync_account_drafts();
            settings.sync_connection_forms();
            settings.monitoring.sync(&settings.view);
            settings.finish_workspace_target();
            changed |= !usage_only;
        }
        let usage = settings.view.usage.clone();
        let visible = self.harness_workspace.active()
            || (self.settings_open && self.settings_section == ui::SettingsSection::Harness);
        changed |= self.observe_harness_usage(usage, visible, Instant::now());
        for effect in effects {
            match effect {
                Effect::CopyClientKey(copy) => {
                    if let Some(settings) = &mut self.harness_settings {
                        settings.consume_key_copy(copy, clipboard_status::set_text);
                        changed = true;
                    }
                }
                Effect::DeviceChallenge {
                    attempt,
                    context,
                    verification_url,
                    user_code,
                    deadline,
                } => {
                    if let Some(settings) = &mut self.harness_settings {
                        let now = Instant::now();
                        changed |= settings.receive_device_challenge(
                            DeviceChallenge {
                                attempt,
                                context,
                                verification_url,
                                user_code,
                                deadline,
                                remaining_seconds: deadline
                                    .saturating_duration_since(now)
                                    .as_secs()
                                    .max(1),
                            },
                            self.dismissed_auth_attempt,
                        );
                    }
                }
                Effect::OpenAuth {
                    attempt,
                    context,
                    url,
                } => self.open_harness_auth(window, attempt, context, url.expose()),
                Effect::AuthFinished(attempt) => {
                    if let Some(settings) = &mut self.harness_settings
                        && settings
                            .challenge
                            .as_ref()
                            .is_some_and(|c| c.attempt == attempt)
                    {
                        settings.challenge = None;
                        settings.challenge_expired = false;
                    }
                    self.finish_harness_auth(window, attempt);
                }
            }
        }
        if changed {
            self.mark_host_ui_full_and_repaint(window);
        }
    }

    fn open_harness_login(&mut self, window: &PixelWindow, url: &str) {
        let context = self
            .harness_settings
            .as_ref()
            .and_then(|settings| settings.challenge.as_ref())
            .map(|challenge| (challenge.attempt, challenge.context.clone()));
        if let Some((attempt, context)) = context {
            self.open_harness_auth(window, attempt, context, url);
        }
    }

    pub(super) fn route_harness_settings(
        &mut self,
        window: &PixelWindow,
        event: &PixelWindowEvent,
    ) -> Result<bool, PixelWindowError> {
        if !self.settings_open || self.settings_section != ui::SettingsSection::Harness {
            return Ok(false);
        }
        self.poll_harness_settings(window);
        let metrics = window.metrics()?;
        let scale = metrics.scale_factor.max(1.0);
        let layout = self.layout(metrics.physical_width, metrics.physical_height, scale);
        let size = scaled_host_ui_font(HOST_UI_STATUS_SIZE_PX, self.host_font_logical(), scale);
        let Some(mut settings) = self.harness_settings.take() else {
            return Ok(false);
        };
        settings.supersede_workspace_target(event);
        let rows = settings.rows();
        let geometry = layout.harness_settings(scale, rows.len(), settings.scroll);
        settings.scroll = geometry.rows.start;
        let mut open_url = None;
        let mut consumed = true;
        match event {
            PixelWindowEvent::Keyboard(key)
                if settings.monitoring.focused()
                    && key.state == KeyPressState::Pressed
                    && !key.modifiers.control
                    && !key.modifiers.alt
                    && !key.modifiers.meta
                    && matches!(
                        key.logical,
                        LogicalKey::Named(NamedKey::Tab | NamedKey::ArrowUp | NamedKey::ArrowDown)
                    ) =>
            {
                let backwards =
                    key.modifiers.shift || key.logical == LogicalKey::Named(NamedKey::ArrowUp);
                if let Some(item) = settings.monitoring.navigate(backwards, &settings.view) {
                    if let Some(index) = rows
                        .iter()
                        .position(|r| matches!(r, Row::Monitoring(i) if *i==item))
                    {
                        settings.scroll = geometry.rows.reveal(index);
                    }
                } else {
                    settings.focused = Some(Field::Port);
                    if let Some(index) = rows.iter().position(|r| matches!(r, Row::Port)) {
                        settings.scroll = geometry.rows.reveal(index);
                    }
                }
            }
            PixelWindowEvent::Keyboard(key)
                if settings.monitoring.focused()
                    && key.state == KeyPressState::Pressed
                    && !key.modifiers.control
                    && !key.modifiers.alt
                    && !key.modifiers.meta
                    && (matches!(
                        key.logical,
                        LogicalKey::Named(NamedKey::Enter | NamedKey::Space)
                    ) || matches!(&key.logical,LogicalKey::Character(t)if t==" "))
                    && !settings.monitoring.editing() =>
            {
                if let Some(item) = settings.monitoring.current() {
                    settings.monitoring_action(item);
                }
            }
            PixelWindowEvent::Keyboard(_) | PixelWindowEvent::Ime(_)
                if settings.monitoring.editing() =>
            {
                settings.monitoring.edit(event);
            }
            PixelWindowEvent::FocusChanged(false) | PixelWindowEvent::PointerCaptureLost => {
                settings.blur();
                let _ = window.set_pointer_capture(false);
            }
            PixelWindowEvent::PointerMoved { position, .. }
                if settings.monitoring.drag_item().is_some() =>
            {
                if let Some(item) = settings.monitoring.drag_item()
                    && let Some(index) = rows
                        .iter()
                        .position(|r| matches!(r,Row::Monitoring(i)if *i==item))
                {
                    settings.monitoring.drag_to(
                        position,
                        geometry.cell(index, 0, 2, scale),
                        scale,
                        size,
                    );
                }
            }
            PixelWindowEvent::PointerMoved { position, .. } => {
                if let (Some(field), Some(anchor)) = (settings.focused, settings.drag) {
                    let row = rows
                        .iter()
                        .position(|row| row_field(*row) == Some(field))
                        .unwrap_or(0);
                    let rect = geometry.cell(row, 0, 2, scale);
                    let offset = super::browser_ui::host_field_offset(
                        settings.editor(field),
                        rect,
                        position,
                        scale,
                        size,
                    );
                    let before = (
                        settings.editor(field).caret,
                        settings.editor(field).anchor,
                        settings.editor(field).select_all,
                    );
                    field_editor::drag(settings.editor_mut(field), anchor, offset);
                    let after = (
                        settings.editor(field).caret,
                        settings.editor(field).anchor,
                        settings.editor(field).select_all,
                    );
                    if before != after {
                        settings.note_connection_activity(field);
                    }
                } else {
                    consumed = false;
                }
            }
            PixelWindowEvent::PointerButton {
                button: PointerButton::Left,
                state: PointerButtonState::Released,
                ..
            } => {
                settings.drag = None;
                settings.monitoring.end_drag();
                let _ = window.set_pointer_capture(false);
            }
            PixelWindowEvent::PointerButton {
                button: PointerButton::Left,
                state: PointerButtonState::Pressed,
                position: Some(position),
                modifiers,
            } => {
                let x = (position.x * scale).max(0.0) as u32;
                let y = (position.y * scale).max(0.0) as u32;
                if geometry.previous.contains(x, y) {
                    settings.blur();
                    settings.scroll = settings
                        .scroll
                        .saturating_sub(geometry.rows.visible_rows.max(1));
                } else if geometry.next.contains(x, y) {
                    settings.blur();
                    settings.scroll = (settings.scroll + geometry.rows.visible_rows.max(1))
                        .min(rows.len().saturating_sub(geometry.rows.visible_rows));
                } else if let Some(index) = geometry.rows.hit(x, y) {
                    let row = rows[index];
                    let columns = row_columns(row);
                    let column = (0..columns).find(|&column| {
                        geometry.cell(index, column, columns, scale).contains(x, y)
                    });
                    if let Some(column) = column {
                        if let Row::Monitoring(monitoring::Item::Threshold(i)) = row
                            && column == 0
                        {
                            if !settings
                                .monitoring
                                .editor_focused(monitoring::Item::Threshold(i))
                            {
                                settings.blur();
                            }
                            let capture = settings.monitoring.press(
                                i,
                                position,
                                geometry.cell(index, 0, 2, scale),
                                scale,
                                size,
                                modifiers.shift,
                            );
                            let _ = window.set_pointer_capture(capture);
                        } else {
                            let field = match (row, column) {
                                (Row::Port, 0) if settings.port_editable() => Some(Field::Port),
                                (Row::ModelInput, 0) => Some(Field::Model),
                                (Row::Model, 0) => Some(Field::ProfileChooser),
                                (Row::AccountInput(field) | Row::ConnectInput(field), 0) => {
                                    Some(field)
                                }
                                (Row::ProfileInput(field), 0) => Some(Field::Profile(field)),
                                _ => None,
                            };
                            if field == Some(Field::ProfileChooser) {
                                settings.blur();
                                settings.focused = Some(Field::ProfileChooser);
                                settings.navigate_connection_profiles(0);
                            } else if let Some(field) = field {
                                let rect = geometry.cell(index, 0, 2, scale);
                                let offset = super::browser_ui::host_field_offset(
                                    settings.editor(field),
                                    rect,
                                    position,
                                    scale,
                                    size,
                                );
                                let was_focused = settings.focused == Some(field);
                                if !was_focused {
                                    settings.blur();
                                }
                                settings.focused = Some(field);
                                let clicks = settings.clicks.register(offset, Instant::now());
                                let selection_before = (
                                    settings.editor(field).caret,
                                    settings.editor(field).anchor,
                                    settings.editor(field).select_all,
                                );
                                settings.drag = field_editor::press(
                                    settings.editor_mut(field),
                                    offset,
                                    modifiers.shift,
                                    clicks,
                                );
                                let selection_after = (
                                    settings.editor(field).caret,
                                    settings.editor(field).anchor,
                                    settings.editor(field).select_all,
                                );
                                if selection_before != selection_after {
                                    settings.note_connection_activity(field);
                                }
                                let _ = window.set_pointer_capture(settings.drag.is_some());
                                self.composer.focused = false;
                                self.settings_proxy_focused = false;
                                self.browser_address_focused = false;
                                self.browser_proxy_focused = false;
                                #[cfg(any(target_os = "macos", windows))]
                                if let Some(page) = &self.native_browser_page {
                                    page.focus_parent();
                                }
                                #[cfg(target_os = "linux")]
                                window.focus();
                            } else {
                                settings.blur();
                                let profile_caption = profile_verification_label(
                                    self.ui_language,
                                    settings.profile_plan(),
                                );
                                let unreadable_profile = matches!(row, Row::ProfileVerification)
                                    && cost_button_font(
                                        geometry.cell(index, column, columns, scale),
                                        &profile_caption,
                                        size,
                                        scale,
                                    )
                                    .is_none();
                                let unreadable_key = matches!(row, Row::KeyActions)
                                    && !key_button_fits(
                                        geometry.cell(index, column, columns, scale),
                                        key_button_caption(self.ui_language, column),
                                        size,
                                        scale,
                                    );
                                if unreadable_key {
                                    settings.refuse_unreadable_key();
                                } else if unreadable_profile
                                    || (matches!(row, Row::VerificationActions)
                                        && verification_button_font(
                                            geometry.cell(index, column, columns, scale),
                                            self.ui_language,
                                            size,
                                            scale,
                                        )
                                        .is_none())
                                {
                                    settings.error=Some("Enlarge the window to read the complete model-check cost notice before verifying.".into());
                                } else {
                                    open_url = settings.activate(row, column);
                                }
                            }
                        }
                    }
                } else if !geometry.panel.contains(x, y) {
                    settings.blur();
                    // Let the existing settings dismiss/header handler own this press.
                    consumed = false;
                } else {
                    settings.blur();
                }
            }
            PixelWindowEvent::MouseWheel { delta, .. } => {
                let amount = match delta {
                    WheelDelta::Lines { y, .. } => minicon_core::numeric::round_f32(*y) as isize,
                    WheelDelta::LogicalPixels { y, .. } => {
                        minicon_core::numeric::round_f64(*y / 36.0) as isize
                    }
                    _ => 0,
                };
                settings.scroll = settings
                    .scroll
                    .saturating_add_signed(-amount)
                    .min(rows.len().saturating_sub(geometry.rows.visible_rows));
            }
            PixelWindowEvent::Keyboard(key) if key.logical == LogicalKey::Named(NamedKey::Tab) => {
                if key.state == KeyPressState::Pressed {
                    let fields: Vec<_> = rows
                        .iter()
                        .filter_map(|row| row_field(*row))
                        .filter(|field| *field != Field::Port || settings.port_editable())
                        .collect();
                    let current = fields
                        .iter()
                        .position(|field| Some(*field) == settings.focused);
                    let next_index = match (current, key.modifiers.shift) {
                        (Some(index), true) => (index + fields.len() - 1) % fields.len(),
                        (Some(index), false) => (index + 1) % fields.len(),
                        (None, true) => fields.len() - 1,
                        (None, false) => 0,
                    };
                    if current.is_some_and(|i| i + 1 == fields.len()) && !key.modifiers.shift {
                        settings.blur();
                        settings.monitoring.enter();
                        if let Some(index) = rows.iter().position(|r| {
                            matches!(r, Row::Monitoring(monitoring::Item::OpenReport))
                        }) {
                            settings.scroll = geometry.rows.reveal(index);
                        }
                    } else {
                        let next = fields[next_index];
                        settings.blur();
                        settings.focused = Some(next);
                        let index = rows
                            .iter()
                            .position(|row| row_field(*row) == Some(next))
                            .unwrap_or(0);
                        settings.scroll = geometry.rows.reveal(index);
                    }
                }
            }
            PixelWindowEvent::Keyboard(key) if settings.focused == Some(Field::ProfileChooser) => {
                if key.state == KeyPressState::Pressed {
                    match &key.logical {
                        LogicalKey::Named(NamedKey::ArrowUp) => {
                            settings.navigate_connection_profiles(-1);
                        }
                        LogicalKey::Named(NamedKey::ArrowDown) => {
                            settings.navigate_connection_profiles(1);
                        }
                        LogicalKey::Named(NamedKey::Enter | NamedKey::Space) => {
                            settings.confirm_connection_profile()
                        }
                        LogicalKey::Character(text) if text == " " => {
                            settings.confirm_connection_profile()
                        }
                        LogicalKey::Named(NamedKey::Escape) => settings.blur(),
                        _ => {}
                    }
                    if settings.focused == Some(Field::ProfileChooser)
                        && let Some(choice) = settings.profile_navigation
                        && let Some(index) = rows
                            .iter()
                            .position(|row| matches!(row,Row::Profile(index) if *index==choice))
                    {
                        settings.scroll = geometry.rows.reveal(index);
                    }
                }
            }
            PixelWindowEvent::Ime(_) if settings.focused == Some(Field::ProfileChooser) => {}
            PixelWindowEvent::Keyboard(_) | PixelWindowEvent::Ime(_) => {
                if let Some(field) = settings.focused {
                    let limits = field_limits(field);
                    if field == Field::Port && !settings.port_editable() {
                        settings.blur();
                    } else if matches!(field, Field::Model | Field::Profile(_))
                        && settings
                            .forms
                            .current()
                            .is_some_and(|form| form.discarding())
                    {
                        settings.error = Some(
                            "This unapplied connection is being removed. Wait before editing it."
                                .into(),
                        );
                    } else if matches!(field, Field::Model | Field::Profile(_))
                        && settings.forms.current().is_none()
                    {
                        settings.error =
                            Some("Wait for connection settings to load before editing.".into());
                    } else {
                        let before = settings.editor(field).text.clone();
                        if let Some(outcome) = super::browser_ui::edit_host_field(
                            settings.editor_mut(field),
                            event,
                            limits,
                        ) {
                            if before != settings.editor(field).text
                                && matches!(field, Field::Model | Field::Profile(_))
                            {
                                // This exact raw change revokes Pending as well as Active,
                                // before a bounded edit send can fail.
                                settings.connection_text_changed(field);
                            } else if outcome.changed && !outcome.rejected {
                                settings.note_connection_activity(field);
                            }
                            match outcome.intent {
                                Intent::Submit => settings.submit(field),
                                Intent::Cancel => settings.blur(),
                                Intent::None => {}
                            }
                        }
                    }
                } else if matches!(event, PixelWindowEvent::Keyboard(key)
                    if key.state == KeyPressState::Pressed && key.logical == LogicalKey::Named(NamedKey::Escape))
                {
                    self.settings_open = false;
                }
            }
            _ => consumed = false,
        }
        let rows = settings.rows();
        if let Some(form) = settings.forms.current_mut() {
            form.scroll = settings.scroll;
            form.visited = true;
        }
        settings.remember_connection_focus();
        if let Some(field) = settings
            .focused
            .filter(|field| *field != Field::ProfileChooser)
        {
            let index = rows
                .iter()
                .position(|row| row_field(*row) == Some(field))
                .unwrap_or(0);
            let rect = layout
                .harness_settings(scale, rows.len(), settings.scroll)
                .cell(index, 0, 2, scale);
            let editor = settings.editor(field);
            let cell = font::cell_metrics(size).width.max(1);
            let columns = (rect.width.saturating_sub((16.0 * scale) as u32) / cell).max(1) as usize;
            let visible =
                composer::visible_window(&editor.text, &editor.preedit, editor.caret, 1, columns);
            let offset = composer::cells(
                &editor.text[visible.text..editor.caret.clamp(visible.text, editor.text.len())],
            ) + composer::cells(&editor.preedit[visible.preedit..]);
            let _ = window.set_ime_cursor_area(minicon_platform::window_host::LogicalRect::new(
                f64::from(rect.x + cell.saturating_mul(offset as u32)) / scale + 8.0,
                f64::from(rect.y + rect.height) / scale,
                2.0,
                20.0,
            ));
        }
        if let Some(item) = settings.monitoring.editing_item()
            && let Some(index) = rows
                .iter()
                .position(|r| matches!(r,Row::Monitoring(i)if *i==item))
            && let Some(editor) = settings.monitoring.editor(item)
        {
            let rect = layout
                .harness_settings(scale, rows.len(), settings.scroll)
                .cell(index, 0, 2, scale);
            let cell = font::cell_metrics(size).width.max(1);
            let columns = (rect.width.saturating_sub((16.0 * scale) as u32) / cell).max(1) as usize;
            let visible =
                composer::visible_window(&editor.text, &editor.preedit, editor.caret, 1, columns);
            let offset = composer::cells(
                &editor.text[visible.text..editor.caret.clamp(visible.text, editor.text.len())],
            ) + composer::cells(&editor.preedit[visible.preedit..]);
            let _ = window.set_ime_cursor_area(minicon_platform::window_host::LogicalRect::new(
                f64::from(rect.x.saturating_add(cell.saturating_mul(offset as u32))) / scale + 8.0,
                f64::from(rect.y.saturating_add(rect.height)) / scale,
                2.0,
                20.0,
            ));
        }
        let open_usage = std::mem::take(&mut settings.open_usage);
        self.harness_settings = Some(settings);
        if open_usage {
            self.open_harness_usage_report(window)?;
        }
        if let Some(url) = open_url {
            self.open_harness_login(window, &url);
        }
        if consumed {
            self.sync_embedded_page(window);
            self.mark_host_ui_full_and_repaint(window);
        }
        Ok(consumed)
    }
}

fn check_observation(
    check: Option<&provider_runtime::ConnectionCheckView>,
) -> minicon_core::json::JsonValue {
    use minicon_core::json::{self, JsonValue};
    check.map_or(JsonValue::Null, |check| {
        json::object(vec![
            ("request", check.request.into()),
            ("profile", check.profile.as_str().into()),
            ("revision", check.revision.into()),
            ("scope", format!("{:?}", check.scope).into()),
            ("completed", u32::from(check.completed).into()),
            ("total", u32::from(check.total).into()),
            (
                "applied_revision",
                check.applied_revision.map_or(JsonValue::Null, Into::into),
            ),
            ("slot", u32::from(check.slot).into()),
            ("phase", format!("{:?}", check.phase).into()),
            (
                "error",
                check
                    .error
                    .map_or(JsonValue::Null, |error| format!("{error:?}").into()),
            ),
            (
                "checked_unix_ms",
                check.checked_unix_ms.map_or(JsonValue::Null, Into::into),
            ),
            (
                "warning",
                check
                    .warning
                    .map_or(JsonValue::Null, |warning| format!("{warning:?}").into()),
            ),
        ])
    })
}

fn verification_button_label(language: ui::UiLanguage) -> &'static str {
    match language {
        ui::UiLanguage::English => "Verify (may charge; output ≤8 tokens)",
        ui::UiLanguage::ChineseSimplified => "验证（可能收费；输出≤8 tokens）",
        ui::UiLanguage::ChineseTraditional => "驗證（可能收費；輸出≤8 tokens）",
    }
}

// One action's full cost caption must fit before it can accept a pointer.
// This is shared by its actual painter and the event route, not a snapshot hint.
fn verification_button_font(
    rect: ui::Rect,
    language: ui::UiLanguage,
    size: u16,
    scale: f64,
) -> Option<u16> {
    cost_button_font(rect, verification_button_label(language), size, scale)
}
fn cost_button_font(rect: ui::Rect, label: &str, size: u16, scale: f64) -> Option<u16> {
    let inset = (16.0 * scale) as u32;
    let minimum = (7.0 * scale).ceil().clamp(1.0, f64::from(u16::MAX)) as u16;
    (minimum..=size.max(minimum)).rev().find(|&font_size| {
        host_ui::host_ui_text_width(label, font_size) <= rect.width.saturating_sub(inset)
            && font::cell_metrics(font_size).height <= rect.height
    })
}

fn paint_verification_button(
    surface: &mut Surface<'_>,
    rect: ui::Rect,
    language: ui::UiLanguage,
    theme: &theme::Theme,
    size: u16,
    scale: f64,
) {
    paint_cost_button(
        surface,
        rect,
        verification_button_label(language),
        theme,
        size,
        scale,
    );
}
fn profile_verification_label(
    language: ui::UiLanguage,
    plan: Option<&provider_runtime::ConnectionPlanView>,
) -> String {
    let Some(plan) = plan.filter(|plan| plan.error.is_none() && plan.id != 0) else {
        return "Profile verification unavailable".into();
    };
    let n = plan.paid_slots.len();
    if n == 0 {
        return match language {
            ui::UiLanguage::English => "Apply: 0 paid checks".into(),
            ui::UiLanguage::ChineseSimplified => "应用：0次收费验证".into(),
            ui::UiLanguage::ChineseTraditional => "套用：0次收費驗證".into(),
        };
    }
    match language {
        ui::UiLanguage::English => {
            format!("Verify & Apply: {n} calls; may charge; output ≤8 tokens/call")
        }
        ui::UiLanguage::ChineseSimplified => {
            format!("验证并应用：{n}次；可能收费；每次输出≤8 tokens")
        }
        ui::UiLanguage::ChineseTraditional => {
            format!("驗證並套用：{n}次；可能收費；每次輸出≤8 tokens")
        }
    }
}
fn paint_cost_button(
    surface: &mut Surface<'_>,
    rect: ui::Rect,
    caption: &str,
    theme: &theme::Theme,
    size: u16,
    scale: f64,
) {
    surface.fill_rect(
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        theme.surface.to_xrgb(),
    );
    let (label, size) = cost_button_font(rect, caption, size, scale)
        .map_or(("Enlarge to verify", size), |size| (caption, size));
    paint_host_ui_text(
        surface,
        rect.x + (8.0 * scale) as u32,
        rect.y + rect.height.saturating_sub(font::cell_metrics(size).height) / 2,
        label,
        theme.text,
        size,
        rect.width.saturating_sub((16.0 * scale) as u32),
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn k2_recipe_handoff_refuses_actual_pending_gate_even_before_active_view() {
        cs05_pending_ui(|settings| {
            assert!(settings.view.connection_check_active.is_none());
            assert_eq!(
                settings
                    .runtime
                    .as_ref()
                    .unwrap()
                    .profile_key_handoff_ready("default"),
                Err(provider_runtime::RuntimeError::Busy)
            );
            settings.view.profile_keys = vec![provider_runtime::ProfileKeyMetadata {
                profile: "default".into(),
                state: provider_runtime::ProfileKeyState::Created,
                exists: true,
                epoch: 1,
                durability_unconfirmed: false,
            }];
            settings.copy_connection_template(RecipeKind::Worker, |_| {
                panic!("reserved Batch handoff wrote clipboard")
            });
            assert!(settings.key_feedback_label().contains("Busy"));
        });
    }

    #[test]
    fn k2_short_localized_key_captions_share_real_cell_fit_and_tiny_cells_refuse() {
        let size = 14;
        for (width, height) in [(760, 420), (420, 360), (760, 300)] {
            let layout = ui::Layout::new(width, height, 1.0);
            let mut settings = HarnessSettings::default();
            let rows = settings.rows();
            let index = rows
                .iter()
                .position(|row| matches!(row, Row::KeyActions))
                .unwrap();
            let initial = layout.harness_settings(1.0, rows.len(), 0);
            settings.scroll = initial.rows.reveal(index);
            let geometry = layout.harness_settings(1.0, rows.len(), settings.scroll);
            assert!(
                index >= geometry.rows.start
                    && index < geometry.rows.start + geometry.rows.visible_rows
            );
            for language in [
                ui::UiLanguage::English,
                ui::UiLanguage::ChineseSimplified,
                ui::UiLanguage::ChineseTraditional,
            ] {
                for column in 0..3 {
                    let caption = key_button_caption(language, column);
                    assert!(
                        key_button_fits(geometry.cell(index, column, 3, 1.0), caption, size, 1.0),
                        "actual supported viewport clipped a key action: {width}x{height}, {caption}"
                    );
                    assert!(!key_button_fits(
                        ui::Rect {
                            x: 0,
                            y: 0,
                            width: 1,
                            height: 1
                        },
                        caption,
                        size,
                        1.0
                    ));
                }
            }
        }
    }

    fn cs05_pending_ui(test: impl FnOnce(&mut HarnessSettings)) {
        use std::sync::{Arc, Mutex, mpsc};
        let root = std::env::temp_dir().join(format!(
            "minicon-cs05-ui-{}",
            crate::provider_server::random_id().unwrap()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let (ready, wait_ready) = mpsc::channel();
        let (release, wait_release) = mpsc::channel();
        let wait = Arc::new(Mutex::new(Some(wait_release)));
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if let Some(wait) = wait.lock().unwrap().take() {
                ready.send(()).unwrap();
                wait.recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
            }
        });
        let runtime = provider_runtime::Runtime::new(
            root.clone(),
            provider_login::LoginMetadata {
                version: "synthetic-ui".into(),
                locale: "en".into(),
                timezone_offset_seconds: 0,
            },
            notify,
        )
        .unwrap();
        let mut settings = HarnessSettings {
            runtime: Some(runtime),
            ..Default::default()
        };
        struct Unblock(Option<mpsc::Sender<()>>);
        impl Drop for Unblock {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }
        let unblock = Unblock(Some(release));
        wait_ready
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        settings.view.load_state = provider_runtime::LoadState::Loaded;
        settings.view.busy = false;
        settings.view.connection_profile = "default".into();
        settings.view.connection_profiles = vec![connection_profile("default", &"a".repeat(32))];
        settings.view.connection_plans = vec![provider_runtime::ConnectionPlanView {
            profile: "default".into(),
            revision: 1,
            id: 1,
            paid_slots: vec![0],
            error: None,
        }];
        settings.forms.sync(&settings.view);
        settings.activate(Row::ProfileVerification, 0);
        assert!(settings.forms.publication_request().is_some());
        test(&mut settings);
        drop(unblock);
        drop(settings);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn cs05_real_ui_data_aba_revokes_pending_batch_even_when_edit_queue_is_full() {
        cs05_pending_ui(|settings| {
            let request = settings.forms.publication_request().unwrap();
            for _ in 0..15 {
                settings
                    .runtime
                    .as_mut()
                    .unwrap()
                    .act(Action::RefreshAccounts)
                    .unwrap();
            }
            assert_eq!(
                settings
                    .runtime
                    .as_mut()
                    .unwrap()
                    .act(Action::RefreshAccounts),
                Err(provider_runtime::RuntimeError::Busy)
            );
            let original = settings.forms.current().unwrap().label.text.clone();
            settings.forms.current_mut().unwrap().label.text = "B".into();
            settings.connection_text_changed(Field::Profile(ProfileField::Label));
            settings.forms.current_mut().unwrap().label.text = original.clone();
            settings.connection_text_changed(Field::Profile(ProfileField::Label));
            assert_eq!(
                settings
                    .runtime
                    .as_mut()
                    .unwrap()
                    .act(Action::CancelConnectionVerification { request }),
                Err(provider_runtime::RuntimeError::InvalidAction),
                "actual first data intent must have already revoked Pending despite queue failure and value ABA"
            );
            assert_eq!(settings.forms.current().unwrap().label.text, original);
            assert!(!settings.forms.current().unwrap().waiting());
        });
    }
    #[test]
    fn cs05_real_ui_caret_activity_does_not_revoke_pending_but_oversized_committed_model_does() {
        cs05_pending_ui(|settings| {
            let request = settings.forms.publication_request().unwrap();
            settings.forms.current_mut().unwrap().label.caret = 1;
            settings.note_connection_activity(Field::Profile(ProfileField::Label));
            assert_eq!(
                settings
                    .runtime
                    .as_mut()
                    .unwrap()
                    .act(Action::CancelConnectionVerification { request }),
                Ok(()),
                "caret-only activity must leave exact cancellation available"
            );
        });
        cs05_pending_ui(|settings| {
            let request = settings.forms.publication_request().unwrap();
            settings.forms.current_mut().unwrap().model_mut().text = "x".repeat(201);
            settings.connection_text_changed(Field::Model);
            assert_eq!(
                settings
                    .runtime
                    .as_mut()
                    .unwrap()
                    .act(Action::CancelConnectionVerification { request }),
                Err(provider_runtime::RuntimeError::InvalidAction)
            );
            assert_eq!(settings.forms.current().unwrap().model().text.len(), 201);
            assert!(!settings.forms.current().unwrap().waiting());
        });
    }
    #[test]
    fn cs05_actual_profile_action_paints_exact_cost_in_supported_short_viewports_and_dispatches() {
        let mut settings = HarnessSettings::default();
        settings.view.load_state = provider_runtime::LoadState::Loaded;
        settings.view.busy = false;
        settings.view.connection_profile = "default".into();
        settings.view.connection_profiles = vec![connection_profile("default", "account-a")];
        settings.view.connection_plans = vec![provider_runtime::ConnectionPlanView {
            profile: "default".into(),
            revision: 1,
            id: 10,
            paid_slots: vec![0, 7],
            error: None,
        }];
        settings.forms.sync(&settings.view);
        settings.forms.current_mut().unwrap().local_error = Some("unrelated field error".into());
        let rows = settings.rows();
        let index = rows
            .iter()
            .position(|row| matches!(row, Row::ProfileVerification))
            .unwrap();
        for (width, height) in [(420u32, 300u32), (760, 420)] {
            let layout = ui::Layout::new(width, height, 1.0);
            let initial = layout.harness_settings(1.0, rows.len(), 0);
            let geometry = layout.harness_settings(1.0, rows.len(), initial.rows.reveal(index));
            let rect = geometry.cell(index, 0, row_columns(rows[index]), 1.0);
            assert_eq!(
                geometry
                    .rows
                    .hit(rect.x + rect.width / 2, rect.y + rect.height / 2),
                Some(index)
            );
            let theme = theme::Theme::for_choice(theme::ThemeChoice::Neutral);
            for (language, expected) in [
                (
                    ui::UiLanguage::English,
                    "Verify & Apply: 2 calls; may charge; output ≤8 tokens/call",
                ),
                (
                    ui::UiLanguage::ChineseSimplified,
                    "验证并应用：2次；可能收费；每次输出≤8 tokens",
                ),
                (
                    ui::UiLanguage::ChineseTraditional,
                    "驗證並套用：2次；可能收費；每次輸出≤8 tokens",
                ),
            ] {
                let caption = profile_verification_label(language, settings.profile_plan());
                assert_eq!(caption, expected);
                let size = cost_button_font(rect, &caption, 14, 1.0)
                    .expect("supported full-width profile control must show complete N and cost");
                let mut actual = vec![theme.canvas_bg.to_xrgb(); (width * height) as usize];
                let mut expected_image = actual.clone();
                let mut blank = actual.clone();
                Surface::new(&mut blank, width, height).fill_rect(
                    rect.x,
                    rect.y,
                    rect.width,
                    rect.height,
                    theme.surface.to_xrgb(),
                );
                expected_image.clone_from(&blank);
                paint_cost_button(
                    &mut Surface::new(&mut actual, width, height),
                    rect,
                    &caption,
                    &theme,
                    14,
                    1.0,
                );
                paint_host_ui_text(
                    &mut Surface::new(&mut expected_image, width, height),
                    rect.x + 8,
                    rect.y + rect.height.saturating_sub(font::cell_metrics(size).height) / 2,
                    expected,
                    theme.text,
                    size,
                    rect.width.saturating_sub(16),
                );
                assert_eq!(
                    actual, expected_image,
                    "actual paid action painter must use the exact displayed quote"
                );
                assert_ne!(actual, blank);
            }
        }
        settings.activate(Row::ProfileVerification, 0);
        assert_eq!(
            settings.forms.current().unwrap().local_error,
            Some(provider_runtime::RuntimeError::WorkerUnavailable.to_string())
        );
        assert!(!settings.forms.current().unwrap().waiting());
    }
    #[test]
    fn cs05_zero_call_profile_button_uses_existing_plain_apply_and_no_batch_baseline() {
        let mut settings = HarnessSettings::default();
        settings.view.load_state = provider_runtime::LoadState::Loaded;
        settings.view.busy = false;
        settings.view.connection_profile = "default".into();
        let mut profile = connection_profile("default", "account-a");
        profile.allocations = profile
            .input
            .allocations
            .iter()
            .map(|row| crate::provider_selection::ConnectionAllocationView {
                slot: row.slot,
                upstream_model: Some("wire".into()),
                protocol: Some(crate::provider_server::WireProtocol::Chat),
                availability: provider_runtime::ConnectionAvailability::MissingAccount,
                evidence: provider_runtime::ConnectionEvidence::LegacyApplied,
                checked_unix_ms: None,
            })
            .collect();
        settings.view.connection_profiles = vec![profile];
        settings.view.connection_plans = vec![provider_runtime::ConnectionPlanView {
            profile: "default".into(),
            revision: 1,
            id: 1,
            paid_slots: vec![],
            error: None,
        }];
        settings.forms.sync(&settings.view);
        settings.activate(Row::ProfileVerification, 0);
        assert_eq!(
            settings.forms.current().unwrap().local_error,
            Some(provider_runtime::RuntimeError::WorkerUnavailable.to_string())
        );
        assert!(settings.forms.publication_request().is_none());
        assert!(!settings.forms.current().unwrap().waiting());
    }

    #[test]
    fn cs04_actual_apply_column_is_reachable_and_dispatches_only_an_acknowledged_request() {
        let mut settings = HarnessSettings::default();
        settings.view.load_state = provider_runtime::LoadState::Loaded;
        settings.view.busy = false;
        settings.view.connection_profile = "default".into();
        let mut profile = connection_profile("default", "account-a");
        profile.allocations = profile
            .input
            .allocations
            .iter()
            .map(|row| crate::provider_selection::ConnectionAllocationView {
                slot: row.slot,
                upstream_model: Some("wire".into()),
                protocol: Some(crate::provider_server::WireProtocol::Chat),
                availability: provider_runtime::ConnectionAvailability::Available,
                evidence: provider_runtime::ConnectionEvidence::Verified,
                checked_unix_ms: Some(1234),
            })
            .collect();
        settings.view.connection_profiles = vec![profile];
        settings.forms.sync(&settings.view);
        let rows = settings.rows();
        let index = rows
            .iter()
            .position(|row| matches!(row, Row::ProfileCommands))
            .unwrap();
        for (width, height) in [(420, 300), (760, 420)] {
            let layout = ui::Layout::new(width, height, 1.0);
            let first = layout.harness_settings(1.0, rows.len(), 0);
            let geometry = layout.harness_settings(1.0, rows.len(), first.rows.reveal(index));
            let cell = geometry.cell(index, 1, row_columns(Row::ProfileCommands), 1.0);
            assert!(cell.width > 0 && cell.height > 0);
            assert_eq!(
                geometry
                    .rows
                    .hit(cell.x + cell.width / 2, cell.y + cell.height / 2),
                Some(index)
            );
        }
        settings.activate(Row::ProfileCommands, 1);
        // No Runtime exists in this pure UI fixture. The actual Apply producer
        // must reach act/send_failed and consume exactly its owned request.
        assert_eq!(
            settings.forms.current().unwrap().local_error,
            Some(provider_runtime::RuntimeError::WorkerUnavailable.to_string())
        );
        assert!(!settings.forms.current().unwrap().waiting());
        assert!(settings.forms.next_edit().is_none());
        settings.forms.current_mut().unwrap().model_mut().text = "unacknowledged".into();
        settings.forms.current_mut().unwrap().edit_model();
        settings.activate(Row::ProfileCommands, 1);
        assert!(
            settings
                .forms
                .current()
                .unwrap()
                .local_error
                .as_ref()
                .unwrap()
                .contains("Acknowledge")
        );
    }
    #[test]
    fn cs03_actual_verify_button_paints_visible_localized_cost_and_output_limit_with_a_local_error()
    {
        let mut settings = HarnessSettings::default();
        settings.view.load_state = provider_runtime::LoadState::Loaded;
        settings.view.connection_profile = "default".into();
        settings.view.connection_profiles = vec![connection_profile("default", "account-a")];
        settings.forms.sync(&settings.view);
        settings.forms.current_mut().unwrap().local_error =
            Some("Retained unrelated field error".into());
        let rows = settings.rows();
        let index = rows
            .iter()
            .position(|row| matches!(row, Row::VerificationActions))
            .unwrap();
        let notice = rows
            .iter()
            .position(|row| matches!(row, Row::ProbeNotice))
            .unwrap();
        for (width, height) in [(420u32, 300u32), (760, 420)] {
            let layout = ui::Layout::new(width, height, 1.0);
            let initial = layout.harness_settings(1.0, rows.len(), 0);
            let geometry = layout.harness_settings(1.0, rows.len(), initial.rows.reveal(index));
            assert!(
                notice >= geometry.rows.start + geometry.rows.visible_rows,
                "paid button test must cover an offscreen detailed notice"
            );
            let rect = geometry.cell(index, 0, row_columns(rows[index]), 1.0);
            assert_eq!(
                geometry
                    .rows
                    .hit(rect.x + rect.width / 2, rect.y + rect.height / 2),
                Some(index)
            );
            let theme = theme::Theme::for_choice(theme::ThemeChoice::Neutral);
            for (language, expected) in [
                (
                    ui::UiLanguage::English,
                    "Verify (may charge; output ≤8 tokens)",
                ),
                (
                    ui::UiLanguage::ChineseSimplified,
                    "验证（可能收费；输出≤8 tokens）",
                ),
                (
                    ui::UiLanguage::ChineseTraditional,
                    "驗證（可能收費；輸出≤8 tokens）",
                ),
            ] {
                let text_size = verification_button_font(rect, language, 14, 1.0)
                    .expect("supported narrow frame must display the full paid action caption");
                assert!(
                    host_ui::host_ui_text_width(expected, text_size)
                        <= rect.width.saturating_sub(16)
                );
                let mut actual = vec![theme.canvas_bg.to_xrgb(); (width * height) as usize];
                let mut reference = actual.clone();
                let mut blank = actual.clone();
                Surface::new(&mut blank, width, height).fill_rect(
                    rect.x,
                    rect.y,
                    rect.width,
                    rect.height,
                    theme.surface.to_xrgb(),
                );
                paint_verification_button(
                    &mut Surface::new(&mut actual, width, height),
                    rect,
                    language,
                    &theme,
                    14,
                    1.0,
                );
                let mut surface = Surface::new(&mut reference, width, height);
                surface.fill_rect(
                    rect.x,
                    rect.y,
                    rect.width,
                    rect.height,
                    theme.surface.to_xrgb(),
                );
                paint_host_ui_text(
                    &mut surface,
                    rect.x + 8,
                    rect.y
                        + rect
                            .height
                            .saturating_sub(font::cell_metrics(text_size).height)
                            / 2,
                    expected,
                    theme.text,
                    text_size,
                    rect.width.saturating_sub(16),
                );
                assert!(
                    actual == reference,
                    "actual Verify paint omitted the visible cost or output limit: {expected}"
                );
                assert!(
                    actual != blank,
                    "Verify label produced no visible text pixels"
                );
            }
        }
        assert!(
            verification_button_font(
                ui::Rect {
                    width: 1,
                    height: 1,
                    ..Default::default()
                },
                ui::UiLanguage::English,
                14,
                1.0
            )
            .is_none()
        );
    }

    #[test]
    fn cs03_verification_controls_reveal_and_hit_in_the_actual_short_settings_viewport() {
        let mut settings = HarnessSettings::default();
        settings.view.load_state = provider_runtime::LoadState::Loaded;
        settings.view.connection_profile = "default".into();
        settings.view.connection_profiles = vec![connection_profile("default", "account-a")];
        settings.forms.sync(&settings.view);
        let rows = settings.rows();
        for target in [Row::VerificationActions, Row::VerificationCancel] {
            let index = rows
                .iter()
                .position(|row| std::mem::discriminant(row) == std::mem::discriminant(&target));
            assert!(
                index.is_some(),
                "both explicit model-check actions must remain reachable in supported short viewports"
            );
            let index = index.unwrap();
            for (width, height) in [(420, 300), (760, 420)] {
                let layout = ui::Layout::new(width, height, 1.0);
                let initial = layout.harness_settings(1.0, rows.len(), 0);
                let geometry = layout.harness_settings(1.0, rows.len(), initial.rows.reveal(index));
                for column in 0..row_columns(rows[index]) {
                    let cell = geometry.cell(index, column, row_columns(rows[index]), 1.0);
                    assert!(
                        cell.width > 0 && cell.height > 0,
                        "{width}x{height} verification control {} column {column}",
                        row_id(target)
                    );
                    assert_eq!(
                        geometry
                            .rows
                            .hit(cell.x + cell.width / 2, cell.y + cell.height / 2),
                        Some(index)
                    );
                }
            }
        }
    }

    use super::*;

    fn connection_profile(id: &str, account: &str) -> provider_runtime::ConnectionProfileView {
        provider_runtime::ConnectionProfileView {
            id: id.into(),
            revision: 1,
            input: provider_runtime::ConnectionDraftInput {
                label: id.into(),
                enabled: true,
                default_alias: "minicon-selected".into(),
                allocations: vec![provider_runtime::ConnectionAllocationInput {
                    slot: 0,
                    alias: "minicon-selected".into(),
                    account: Some(account.into()),
                    model: "same-catalogue-id".into(),
                }],
            },
            applied: None,
            dirty: false,
            error: None,
            allocations: vec![],
        }
    }

    #[test]
    fn connection_form_catalogue_choices_follow_slot_account_and_preserve_applied_view() {
        let mut settings = HarnessSettings::default();
        settings.view.load_state = provider_runtime::LoadState::Loaded;
        settings.view.selected_account = Some("applied".into());
        settings.view.model = "applied-wire".into();
        settings.view.connection_profile = "default".into();
        settings.view.connection_profiles = vec![connection_profile("default", "account-b")];
        settings.view.models = vec!["wrong-global-model".into()];
        let catalogue =
            |account: &str, wire: &str, status| provider_runtime::AccountCatalogueView {
                account: account.into(),
                status,
                retained_rows: 1,
                error: None,
                models: vec![provider_runtime::CatalogueModel {
                    id: "same-catalogue-id".into(),
                    upstream_model: wire.into(),
                    protocol: provider_server::WireProtocol::Chat,
                }],
            };
        settings.view.catalogues = vec![
            catalogue(
                "account-a",
                "wire-a",
                provider_runtime::CatalogueStatus::Fresh,
            ),
            catalogue(
                "account-b",
                "wire-b",
                provider_runtime::CatalogueStatus::Fresh,
            ),
        ];
        settings.forms.sync(&settings.view);
        assert_eq!(
            settings
                .current_catalogue()
                .map(|entry| entry.account.as_str()),
            Some("account-b")
        );
        assert_eq!(
            settings.current_catalogue().unwrap().models[0].upstream_model,
            "wire-b"
        );
        assert_eq!(
            settings
                .rows()
                .iter()
                .filter(|row| matches!(row, Row::ModelChoice(_)))
                .count(),
            1
        );
        settings.view.catalogues[1].status = provider_runtime::CatalogueStatus::Error;
        assert!(
            !settings
                .rows()
                .iter()
                .any(|row| matches!(row, Row::ModelChoice(_)))
        );
        assert_eq!(settings.view.selected_account.as_deref(), Some("applied"));
        assert_eq!(settings.view.model, "applied-wire");
    }

    #[test]
    fn connection_profiles_remain_distinct_and_unavailable_allocations_remain_editable() {
        let mut settings = HarnessSettings::default();
        settings.view.load_state = provider_runtime::LoadState::Loaded;
        settings.view.connection_profile = "default".into();
        settings.view.connection_profiles = (0..8)
            .map(|n| {
                connection_profile(
                    if n == 0 {
                        "default"
                    } else {
                        ["one", "two", "three", "four", "five", "six", "seven"][n - 1]
                    },
                    "removed-account",
                )
            })
            .collect();
        settings.forms.sync(&settings.view);
        assert_eq!(
            settings
                .rows()
                .iter()
                .filter(|row| matches!(row, Row::Profile(_)))
                .count(),
            8
        );
        assert!(
            settings
                .rows()
                .iter()
                .any(|row| matches!(row, Row::Allocation(0)))
        );
        assert_eq!(
            settings.forms.current().unwrap().selected_account(),
            Some("removed-account")
        );
        assert!(settings.current_catalogue().is_none());
        let applied = settings.view.clone();
        settings.activate(Row::AccountConnection, 0);
        assert_eq!(settings.view, applied);
        assert!(settings.error.is_none());
    }

    #[test]
    fn connection_rows_use_existing_reveal_and_hit_geometry_in_narrow_short_windows() {
        let mut settings = HarnessSettings::default();
        settings.view.load_state = provider_runtime::LoadState::Loaded;
        settings.view.connection_profile = "custom".into();
        settings.view.connection_profiles = vec![connection_profile("custom", "missing")];
        settings.forms.sync(&settings.view);
        let rows = settings.rows();
        for (width, height) in [(760, 420), (420, 360), (760, 300)] {
            let layout = ui::Layout::new(width, height, 1.0);
            for target in [Row::Model, Row::ProfileCommands] {
                let position = rows
                    .iter()
                    .position(|row| std::mem::discriminant(row) == std::mem::discriminant(&target));
                assert!(
                    position.is_some(),
                    "missing connection control {}",
                    row_id(target)
                );
                let index = position.unwrap();
                let initial = layout.harness_settings(1.0, rows.len(), 0);
                let geometry = layout.harness_settings(1.0, rows.len(), initial.rows.reveal(index));
                let rect = geometry.cell(index, 0, row_columns(rows[index]), 1.0);
                assert!(
                    rect.width > 0 && rect.height > 0,
                    "{width}x{height}: {}",
                    row_id(rows[index])
                );
                assert_eq!(
                    geometry
                        .rows
                        .hit(rect.x + rect.width / 2, rect.y + rect.height / 2),
                    Some(index)
                );
                assert!(rect.x + rect.width <= width && rect.y + rect.height <= height);
            }
        }
    }

    #[test]
    fn connection_editor_submit_stages_only_and_does_not_mutate_applied_selection() {
        let mut settings = HarnessSettings::default();
        settings.view.load_state = provider_runtime::LoadState::Loaded;
        settings.view.connection_profile = "default".into();
        settings.view.connection_profiles = vec![connection_profile("default", "a")];
        settings.view.selected_account = Some("applied".into());
        settings.view.model = "applied-wire".into();
        settings.forms.sync(&settings.view);
        settings.editor_mut(Field::Model).text = "later-local-model".into();
        settings.editor_mut(Field::Model).caret = 5;
        settings.submit(Field::Model);
        assert_eq!(settings.editor(Field::Model).text, "later-local-model");
        assert_eq!(settings.editor(Field::Model).caret, 5);
        assert_eq!(settings.view.selected_account.as_deref(), Some("applied"));
        assert_eq!(settings.view.model, "applied-wire");
        assert!(settings.forms.current().unwrap().unsent());
        assert!(!settings.forms.current().unwrap().waiting());
        assert!(settings.forms.current().unwrap().local_error.is_some());
    }

    #[test]
    fn connection_profile_return_restores_its_field_selection_and_navigation_requires_confirmation()
    {
        let mut settings = HarnessSettings::default();
        settings.view.load_state = provider_runtime::LoadState::Loaded;
        settings.view.connection_profile = "default".into();
        settings.view.connection_profiles = vec![
            connection_profile("default", "a"),
            connection_profile("custom", "b"),
        ];
        settings.forms.sync(&settings.view);
        settings.focused = Some(Field::Model);
        let editor = settings.editor_mut(Field::Model);
        editor.text = "abcd".into();
        editor.caret = 3;
        editor.anchor = Some(1);
        settings.forms.current_mut().unwrap().edit_model();
        settings.remember_connection_focus();
        settings.blur();
        settings.focused = Some(Field::ProfileChooser);
        settings.navigate_connection_profiles(1);
        assert_eq!(settings.forms.selected.as_deref(), Some("default"));
        settings.confirm_connection_profile();
        assert_eq!(settings.forms.selected.as_deref(), Some("custom"));
        assert!(settings.focused.is_none());
        settings.focused = Some(Field::Model);
        settings.remember_connection_focus();
        let b = settings.editor(Field::Model).text.clone();
        settings.focus_connection("default");
        assert!(settings.focused == Some(Field::Model));
        assert_eq!(
            (
                settings.editor(Field::Model).caret,
                settings.editor(Field::Model).anchor
            ),
            (3, Some(1))
        );
        field_editor::apply(
            settings.editor_mut(Field::Model),
            Command::Commit("X"),
            MODEL_LIMITS,
        );
        settings.forms.current_mut().unwrap().edit_model();
        assert_eq!(settings.editor(Field::Model).text, "aXd");
        assert_eq!(settings.forms.forms["custom"].model().text, b);
        settings.focus_connection("custom");
        let form = settings.forms.forms.get_mut("default").unwrap();
        let mut remaining = form.allocations.remove(&0).unwrap();
        remaining.slot = 1;
        form.allocations.insert(1, remaining);
        form.selected_slot = Some(1);
        settings.focus_connection("default");
        assert!(
            settings.focused.is_none(),
            "deleted model field must not retarget another entry"
        );
    }

    #[test]
    fn connection_applied_rows_remain_visible_and_distinct_from_later_draft_choices() {
        let mut settings = HarnessSettings::default();
        settings.view.load_state = provider_runtime::LoadState::Loaded;
        settings.view.connection_profile = "default".into();
        let mut profile = connection_profile("default", "missing-applied-account");
        profile.input.allocations[0].model = "applied-wire".into();
        profile.applied = Some(profile.input.clone());
        settings.view.connection_profiles = vec![profile];
        settings.forms.sync(&settings.view);
        settings
            .forms
            .current_mut()
            .unwrap()
            .assign_account("new-account");
        settings.editor_mut(Field::Model).text = "new-catalogue-model".into();
        settings.forms.current_mut().unwrap().edit_model();
        let applied = settings.applied_allocations();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].model, "applied-wire");
        assert_eq!(
            applied[0].account.as_deref(),
            Some("missing-applied-account")
        );
        assert_eq!(settings.editor(Field::Model).text, "new-catalogue-model");
        let rows = settings.rows();
        for target in [
            Row::AppliedAlias(0),
            Row::AppliedAccount(0),
            Row::AppliedModel(0),
        ] {
            let position = rows
                .iter()
                .position(|row| std::mem::discriminant(row) == std::mem::discriminant(&target));
            assert!(position.is_some(), "missing applied row {}", row_id(target));
            let index = position.unwrap();
            for (width, height) in [(760, 420), (420, 360), (760, 300)] {
                let layout = ui::Layout::new(width, height, 1.0);
                let first = layout.harness_settings(1.0, rows.len(), 0);
                let geometry = layout.harness_settings(1.0, rows.len(), first.rows.reveal(index));
                let rect = geometry.cell(index, 0, 1, 1.0);
                assert!(rect.width > 0 && rect.height > 0);
                assert!(rect.x + rect.width <= width && rect.y + rect.height <= height);
                assert_eq!(
                    geometry
                        .rows
                        .hit(rect.x + rect.width / 2, rect.y + rect.height / 2),
                    Some(index)
                );
            }
        }
    }

    #[test]
    fn cs01_account_catalogue_rows_are_scoped_read_only_and_do_not_apply_models() {
        use provider_runtime::{AccountCatalogueView, CatalogueModel, CatalogueStatus};
        let a = "a".repeat(32);
        let b = "b".repeat(32);
        let model = |id: &str| CatalogueModel {
            id: id.into(),
            upstream_model: id.into(),
            protocol: provider_server::WireProtocol::Chat,
        };
        let mut settings = HarnessSettings::default();
        settings.view.selected_account = Some(a.clone());
        settings.view.model = "applied-legacy".into();
        settings.model.text = "unchanged draft".into();
        settings.view.catalogues = vec![
            AccountCatalogueView {
                account: a.clone(),
                status: CatalogueStatus::Fresh,
                models: vec![model("same")],
                retained_rows: 1,
                error: None,
            },
            AccountCatalogueView {
                account: b.clone(),
                status: CatalogueStatus::Error,
                models: vec![model("same"), model("stale-b")],
                retained_rows: 2,
                error: Some(provider_runtime::RuntimeError::ProviderUnavailable),
            },
        ];
        settings.account_focus = Some(a.clone());
        assert_eq!(
            settings
                .rows()
                .iter()
                .filter(|row| matches!(row, Row::CatalogueModel(_)))
                .count(),
            1
        );
        let applied = settings.view.clone();
        assert!(settings.activate(Row::CatalogueModel(0), 0).is_none());
        assert_eq!(settings.view, applied);
        assert_eq!(settings.model.text, "unchanged draft");
        assert!(
            settings.error.is_none(),
            "read-only row attempted a Runtime action"
        );
        settings.account_focus = Some(b);
        assert_eq!(
            settings
                .rows()
                .iter()
                .filter(|row| matches!(row, Row::CatalogueModel(_)))
                .count(),
            2
        );
        assert_eq!(settings.view.selected_account, Some(a));
        assert_eq!(settings.view.model, "applied-legacy");
    }

    #[test]
    fn reconnect_requires_saved_account_edits_and_never_applies_the_form_implicitly() {
        let preferences = AccountPreferences::legacy(&"a".repeat(32));
        let account = AccountSummary {
            id: "a".repeat(32),
            provider: Provider::DeepSeek,
            label: "Applied".into(),
            state: provider_accounts::AccountState::AccountReady,
        };
        let mut settings = HarnessSettings {
            account_focus: Some(account.id.clone()),
            account_draft: AccountDraft::from_account(&account, &preferences),
            ..Default::default()
        };
        settings.account_draft.login.text = "http://unsaved-login.invalid:8080".into();
        assert!(settings.activate(Row::AccountReconnect, 0).is_none());
        assert_eq!(
            settings.error.as_deref(),
            Some("Save or revert account changes before reconnecting.")
        );
        assert_eq!(settings.account_draft.applied, Some(preferences.clone()));
        assert_eq!(
            settings.account_draft.login.text,
            "http://unsaved-login.invalid:8080"
        );
        settings.account_draft = AccountDraft::from_account(&account, &preferences);
        settings.account_draft.submitted = Some(1);
        settings.error = None;
        assert!(settings.activate(Row::AccountReconnect, 0).is_none());
        assert_eq!(
            settings.error.as_deref(),
            Some("Save or revert account changes before reconnecting.")
        );
    }

    #[test]
    fn queued_device_challenge_cannot_return_after_cancel_or_replace_another_attempt() {
        let attempt = provider_runtime::AttemptId(42);
        let deadline = Instant::now() + Duration::from_secs(20);
        let challenge = || DeviceChallenge {
            attempt,
            context: browser::PageContext {
                profile: minicon_web::PageProfile {
                    data_directory: std::env::temp_dir().join("unused-device-fixture"),
                    store_identifier: [2; 16],
                },
                proxy: Default::default(),
            },
            verification_url: provider_auth::Secret::new("https://opencode.ai/console/device")
                .unwrap(),
            user_code: provider_auth::Secret::new("synthetic-code").unwrap(),
            deadline,
            remaining_seconds: 20,
        };
        let mut settings = HarnessSettings::default();
        assert!(!settings.receive_device_challenge(challenge(), None));
        assert!(settings.challenge.is_none());
        settings.view.login_active = true;
        settings.view.auth_attempt = Some(provider_runtime::AttemptId(43));
        assert!(!settings.receive_device_challenge(challenge(), None));
        assert!(settings.challenge.is_none());
        settings.view.auth_attempt = Some(attempt);
        assert!(!settings.receive_device_challenge(challenge(), Some(attempt)));
        assert!(settings.challenge.is_none());
        assert!(settings.receive_device_challenge(challenge(), None));
        assert_eq!(settings.challenge.as_ref().unwrap().deadline, deadline);
        settings.cancel_login();
        assert!(settings.challenge.is_none());
        assert!(!settings.receive_device_challenge(challenge(), None));
        settings.cancelled_attempt = None;
        settings.challenge = None;
        let mut expired = challenge();
        expired.deadline = Instant::now() - Duration::from_secs(1);
        assert!(!settings.receive_device_challenge(expired, None));
        assert!(settings.challenge.is_none());
    }

    #[test]
    fn committed_login_clears_only_its_own_browser_failure_after_cancel_loses_the_race() {
        let attempt = provider_runtime::AttemptId(42);
        let mut settings = HarnessSettings {
            auth_error: Some((attempt, "Isolated authorization page unavailable".into())),
            error: Some("Unrelated invalid port draft".into()),
            ..Default::default()
        };
        settings.view.auth_result = Some((
            provider_runtime::AttemptId(41),
            provider_runtime::AuthOutcome::CredentialStored,
        ));
        settings.reconcile_auth_result();
        assert!(settings.auth_error.is_some());
        settings.view.auth_result =
            Some((attempt, provider_runtime::AuthOutcome::CredentialStored));
        settings.reconcile_auth_result();
        assert!(settings.auth_error.is_none());
        assert_eq!(
            settings.status().as_deref(),
            Some("Unrelated invalid port draft")
        );
        settings.auth_error = Some((attempt, "Old failure".into()));
        settings.view.auth_attempt = Some(provider_runtime::AttemptId(43));
        settings.reconcile_auth_result();
        assert!(settings.auth_error.is_none());
    }

    #[test]
    fn published_config_warning_survives_metadata_status_until_a_confirmed_save() {
        let mut settings = HarnessSettings::default();
        settings.view.config_durability_unconfirmed = true;
        settings.view.status = Some("Accounts refreshed.".into());
        assert_eq!(
            settings.status().as_deref(),
            Some(provider_runtime::CONFIG_DURABILITY_WARNING)
        );
        settings.error = Some("Unrelated invalid draft".into());
        assert_eq!(
            settings.status().as_deref(),
            Some("Unrelated invalid draft")
        );
        settings.error = None;
        settings.view.config_durability_unconfirmed = false;
        assert_eq!(settings.status().as_deref(), Some("Accounts refreshed."));
    }

    #[test]
    fn save_acknowledgement_rebases_only_its_form_and_keeps_later_draft_and_caret() {
        let original = AccountSummary {
            id: "a".repeat(32),
            provider: Provider::DeepSeek,
            label: "Original".into(),
            state: provider_accounts::AccountState::CredentialStored,
        };
        let preferences = AccountPreferences::legacy(&original.id);
        let mut draft = AccountDraft::from_account(&original, &preferences);
        draft.label.text = "Later unsaved edit".into();
        draft.label.caret = 5;
        draft.label.anchor = Some(2);
        draft.submitted = Some(2);
        let saved = AccountSummary {
            label: "Saved label".into(),
            ..original.clone()
        };
        let mut receipt = provider_runtime::AccountSaveReceipt {
            account: original.id.clone(),
            request: 1,
            result: Ok(()),
        };
        draft.acknowledge(&saved, &preferences, Some(&receipt));
        assert_eq!(draft.applied_label, "Original");
        assert_eq!(draft.submitted, Some(2));
        receipt.request = 2;
        draft.acknowledge(&saved, &preferences, Some(&receipt));
        assert_eq!(draft.applied_label, "Saved label");
        assert_eq!(draft.submitted, None);
        assert_eq!(draft.label.text, "Later unsaved edit");
        assert_eq!(draft.save_error, None);
        assert_eq!(draft.label.caret, 5);
        assert_eq!(draft.label.anchor, Some(2));
        assert!(draft.dirty());
        draft.submitted = Some(3);
        receipt.request = 3;
        receipt.result = Err(provider_runtime::RuntimeError::AccountChanged);
        draft.acknowledge(&original, &preferences, Some(&receipt));
        assert_eq!(draft.applied_label, "Saved label");
        assert_eq!(draft.submitted, None);
        assert_eq!(draft.label.text, "Later unsaved edit");
        assert_eq!(
            draft.save_error,
            Some(provider_runtime::RuntimeError::AccountChanged)
        );
    }
    #[test]
    fn monitoring_deeplink_survives_initial_profile_load_and_yields_to_later_navigation() {
        let mut settings = HarnessSettings::default();
        settings.port.text = "23456".into();
        settings.port.caret = 2;
        settings
            .show_workspace_target(super::super::harness_workspace_ui::SettingsTarget::Monitoring);
        assert!(settings.pending_workspace_target.is_some());
        settings.view.load_state = provider_runtime::LoadState::Loaded;
        settings.view.connection_profile = "default".into();
        settings.view.connection_profiles = vec![connection_profile("default", "a")];
        settings.sync_connection_forms();
        settings.monitoring.sync(&settings.view);
        settings.finish_workspace_target();
        assert_eq!(row_id(settings.rows()[settings.scroll]), "monitoring");
        assert!(settings.pending_workspace_target.is_none());
        assert_eq!(
            (settings.port.text.as_str(), settings.port.caret),
            ("23456", 2)
        );
        settings.scroll = 2;
        settings.finish_workspace_target();
        assert_eq!(
            settings.scroll, 2,
            "resolved destination must not keep replaying"
        );

        settings.view.load_state = provider_runtime::LoadState::Loading;
        settings
            .show_workspace_target(super::super::harness_workspace_ui::SettingsTarget::Monitoring);
        settings.supersede_workspace_target(&PixelWindowEvent::Wake);
        assert!(settings.pending_workspace_target.is_some());
        settings.supersede_workspace_target(&PixelWindowEvent::PointerButton {
            button: PointerButton::Left,
            state: PointerButtonState::Pressed,
            position: None,
            modifiers: Default::default(),
        });
        settings.scroll = 3;
        settings.view.load_state = provider_runtime::LoadState::Loaded;
        settings.finish_workspace_target();
        assert_eq!(
            settings.scroll, 3,
            "later explicit navigation owns the page"
        );
    }

    #[test]
    fn monitoring_real_deeplink_and_report_action_preserve_existing_drafts() {
        let mut settings = HarnessSettings::default();
        settings.port.text = "23456".into();
        settings.port.caret = 2;
        settings
            .show_workspace_target(super::super::harness_workspace_ui::SettingsTarget::Monitoring);
        let rows = settings.rows();
        assert_eq!(row_id(rows[settings.scroll]), "monitoring");
        settings.activate(Row::Monitoring(monitoring::Item::OpenReport), 0);
        assert!(settings.open_usage);
        assert_eq!(
            (settings.port.text.as_str(), settings.port.caret),
            ("23456", 2)
        );
        assert!(
            settings.runtime.is_none(),
            "read-only route action cannot construct Runtime"
        );
    }
    #[test]
    fn monitoring_open_report_uses_actual_revealed_cell_and_full_localized_caption() {
        let settings = HarnessSettings::default();
        let rows = settings.rows();
        let index = rows
            .iter()
            .position(|r| matches!(r, Row::Monitoring(monitoring::Item::OpenReport)))
            .unwrap();
        for (width, height) in [(420, 300), (760, 360), (1400, 900)] {
            let layout = ui::Layout::new(width, height, 1.0);
            let initial = layout.harness_settings(1.0, rows.len(), 0);
            let g = layout.harness_settings(1.0, rows.len(), initial.rows.reveal(index));
            let rect = g.cell(index, 0, 1, 1.0);
            assert_eq!(
                g.rows
                    .hit(rect.x + rect.width / 2, rect.y + rect.height / 2),
                Some(index)
            );
            for language in [
                ui::UiLanguage::English,
                ui::UiLanguage::ChineseSimplified,
                ui::UiLanguage::ChineseTraditional,
            ] {
                let caption = settings.monitoring.label(
                    monitoring::Item::OpenReport,
                    &settings.view,
                    settings.view.usage.as_ref(),
                    language,
                );
                assert!(
                    host_ui::host_ui_text_width(&caption, 14) <= rect.width.saturating_sub(16),
                    "{width}x{height}: {caption}; width={}",
                    rect.width
                );
            }
        }
    }
}
