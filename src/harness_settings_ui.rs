//! Harness settings presentation. Runtime authority stays with provider_runtime.
use super::*;
use minicon_core::field_editor::{self, Command, Intent, Limits};
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
    AccountNetworkHint,
    NoAccounts,
    Model,
    ModelInput,
    ModelActions,
    ProbeNotice,
    ModelChoice(usize),
    Connect,
    KeyState,
    KeyActions,
    OpenAi,
    Anthropic,
    Recipe,
    Status,
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
        Row::AccountInput(field) | Row::ConnectInput(field) => Some(field),
        _ => None,
    }
}
fn field_limits(field: Field) -> Limits {
    match field {
        Field::Port => PORT_LIMITS,
        Field::Model => MODEL_LIMITS,
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
        let (runtime, error) = match result {
            Ok(runtime) => (Some(runtime), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let view = runtime
            .as_ref()
            .map(|runtime| runtime.view().clone())
            .unwrap_or_else(|| View {
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
            ]);
        }
        rows.extend([
            Row::Model,
            Row::ModelInput,
            Row::ModelActions,
            Row::ProbeNotice,
        ]);
        rows.extend((0..self.view.models.len()).map(Row::ModelChoice));
        rows.extend([
            Row::Connect,
            Row::KeyState,
            Row::KeyActions,
            Row::OpenAi,
            Row::Anthropic,
            Row::Recipe,
        ]);
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
            Field::Model => &self.model,
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
            Field::Model => &mut self.model,
            Field::AccountLabel => &mut self.account_draft.label,
            Field::LoginProxy => &mut self.account_draft.login,
            Field::ServiceProxy => &mut self.account_draft.service,
            Field::ConnectLabel => &mut self.connection_draft.label,
            Field::ConnectLoginProxy => &mut self.connection_draft.login,
            Field::ConnectServiceProxy => &mut self.connection_draft.service,
        }
    }

    fn blur(&mut self) {
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

    fn act(&mut self, action: Action) {
        self.error = self.runtime.as_mut().map_or_else(
            || Some(provider_runtime::RuntimeError::WorkerUnavailable.to_string()),
            |runtime| runtime.act(action).err().map(|error| error.to_string()),
        );
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
            .or_else(|| self.auth_error.as_ref().map(|(_, message)| message.clone()))
            .or_else(|| self.account_draft.save_error.map(|error| error.to_string()))
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
            Field::Model => {
                let model = self.model.text.trim();
                if model.is_empty() {
                    self.error = Some("Enter or select a model first.".to_owned());
                } else {
                    self.act(Action::SelectModel(model.to_owned()));
                }
            }
            Field::AccountLabel | Field::LoginProxy | Field::ServiceProxy => self.save_account(),
            Field::ConnectLabel | Field::ConnectLoginProxy | Field::ConnectServiceProxy => {
                self.connect_account(self.connection_draft.provider)
            }
        }
    }

    fn activate(&mut self, row: Row, column: usize) -> Option<String> {
        match row {
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
                if let Some(id) = self.account_focus.clone() {
                    self.act(Action::SelectAccount(id));
                }
            }
            Row::AccountConnection => {
                self.account_draft.enabled = !self.account_draft.enabled;
            }
            Row::ModelInput if column == 1 => self.submit(Field::Model),
            Row::ModelActions => self.act(if column == 0 {
                Action::RefreshModels
            } else {
                Action::TestModel
            }),
            Row::ModelChoice(index) => {
                let model = self.view.models[index].clone();
                self.model.text.clone_from(&model);
                self.model.caret = model.len();
                self.model.anchor = None;
                self.model.select_all = false;
                self.act(Action::SelectModel(model));
            }
            Row::KeyActions => self.act(match column {
                0 => Action::CreateClientKey,
                1 => Action::CopyClientKey,
                _ => Action::RevokeClientKey,
            }),
            Row::Recipe => {
                let port = self.view.actual_port.unwrap_or(self.view.port);
                let command = format!(
                    "minicon harness --root <path> --task <task> --gateway http://127.0.0.1:{port}/v1 --client-key-env MINICON_LOCAL_API_KEY --allow-cmd <command>"
                );
                if !clipboard_status::set_text(&command) {
                    self.error = Some("Could not copy the connection template.".to_owned());
                }
            }
            _ => {}
        }
        None
    }
}

fn row_columns(row: Row) -> usize {
    match row {
        Row::KeyActions => 3,
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
        | Row::ModelActions => 2,
        _ => 1,
    }
}

fn row_id(row: Row) -> String {
    match row {
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
        Row::AccountNetworkHint => "account_network_hint",
        Row::Model => "model",
        Row::ModelInput => "model_input",
        Row::ModelActions => "model_actions",
        Row::ProbeNotice => "probe_notice",
        Row::Connect => "connect",
        Row::KeyState => "key_state",
        Row::KeyActions => "key_actions",
        Row::OpenAi => "openai_base_url",
        Row::Anthropic => "anthropic_base_url",
        Row::Recipe => "recipe",
        Row::Status => "status",
        Row::Account(index) => return format!("account_{index}"),
        Row::ModelChoice(index) => return format!("model_{index}"),
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
                Row::Local => {
                    label = "Local API".to_owned();
                    heading = true;
                }
                Row::Accounts => {
                    label = "Provider accounts".to_owned();
                    heading = true;
                }
                Row::Model => {
                    label = "Model / manual account binding".to_owned();
                    heading = true;
                }
                Row::Connect => {
                    label = "Connect a client".to_owned();
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
                        "Use for default client",
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
                Row::ConnectNetworkHint => {
                    label =
                        "New account: choose separate login and service networks, then Continue."
                            .to_owned()
                }
                Row::NoAccounts => label = "No account. Add a provider to continue.".to_owned(),
                Row::ModelInput => {
                    host_ui::paint_single_line_host_editor(
                        surface,
                        cell(0, 2),
                        &settings.model,
                        settings.focused == Some(Field::Model),
                        "Provider model ID",
                        theme,
                        size,
                        scale,
                    );
                    button(surface, cell(1, 2), "Apply model", false);
                }
                Row::ModelActions => {
                    button(surface, cell(0, 2), "Refresh models", false);
                    button(surface, cell(1, 2), "Test model", false);
                }
                Row::ProbeNotice => {
                    label = "Test model sends a small paid inference request.".to_owned()
                }
                Row::ModelChoice(model_index) => button(
                    surface,
                    rect,
                    &settings.view.models[model_index],
                    settings.view.model == settings.view.models[model_index],
                ),
                Row::KeyState => {
                    label = if settings.view.local_key_exists {
                        "Local client key: stored privately"
                    } else {
                        "Local client key: not created"
                    }
                    .to_owned()
                }
                Row::KeyActions => {
                    button(surface, cell(0, 3), "Create key", false);
                    button(surface, cell(1, 3), "Copy key", false);
                    button(surface, cell(2, 3), "Revoke key", false);
                }
                Row::OpenAi => {
                    label = format!(
                        "OpenAI: http://127.0.0.1:{}/v1",
                        settings.view.actual_port.unwrap_or(settings.view.port)
                    )
                }
                Row::Anthropic => {
                    label = format!(
                        "Anthropic: http://127.0.0.1:{}",
                        settings.view.actual_port.unwrap_or(settings.view.port)
                    )
                }
                Row::Recipe => {
                    button(
                        surface,
                        rect,
                        "Copy worker command template (key from environment)",
                        false,
                    );
                }
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
            // Menus suspend a settings field; real route changes explicitly blur.
            field_editor::apply(&mut settings.port, Command::Blur, PORT_LIMITS);
            field_editor::apply(&mut settings.model, Command::Blur, MODEL_LIMITS);
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
            if settings.port.text == settings.view.port.to_string() {
                settings.port.text = next.port.to_string();
                settings.port.caret = settings.port.text.len();
                settings.port.anchor = None;
                settings.port.select_all = false;
            }
            if settings.model.text == settings.view.model {
                settings.model.text.clone_from(&next.model);
                settings.model.caret = settings.model.text.len();
                settings.model.anchor = None;
                settings.model.select_all = false;
            }
            if !next.login_active {
                settings.challenge = None;
                settings.challenge_expired = false;
            }
            settings.view = next;
            settings.reconcile_auth_result();
            settings.sync_account_drafts();
            changed = true;
        }
        for effect in effects {
            match effect {
                Effect::CopyClientKey(secret) => {
                    let _ = clipboard_status::set_text(secret.expose());
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
        let rows = settings.rows();
        let geometry = layout.harness_settings(scale, rows.len(), settings.scroll);
        settings.scroll = geometry.rows.start;
        let mut open_url = None;
        let mut consumed = true;
        match event {
            PixelWindowEvent::FocusChanged(false) | PixelWindowEvent::PointerCaptureLost => {
                settings.blur();
                let _ = window.set_pointer_capture(false);
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
                    field_editor::drag(settings.editor_mut(field), anchor, offset);
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
                        let field = match (row, column) {
                            (Row::Port, 0) if settings.port_editable() => Some(Field::Port),
                            (Row::ModelInput, 0) => Some(Field::Model),
                            (Row::AccountInput(field) | Row::ConnectInput(field), 0) => Some(field),
                            _ => None,
                        };
                        if let Some(field) = field {
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
                            settings.drag = field_editor::press(
                                settings.editor_mut(field),
                                offset,
                                modifiers.shift,
                                clicks,
                            );
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
                            open_url = settings.activate(row, column);
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
            PixelWindowEvent::Keyboard(_) | PixelWindowEvent::Ime(_) => {
                if let Some(field) = settings.focused {
                    let limits = field_limits(field);
                    if field == Field::Port && !settings.port_editable() {
                        settings.blur();
                    } else if let Some(outcome) = super::browser_ui::edit_host_field(
                        settings.editor_mut(field),
                        event,
                        limits,
                    ) {
                        match outcome.intent {
                            Intent::Submit => settings.submit(field),
                            Intent::Cancel => settings.blur(),
                            Intent::None => {}
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
        if let Some(field) = settings.focused {
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
        self.harness_settings = Some(settings);
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
