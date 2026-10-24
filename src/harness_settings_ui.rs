//! Harness settings presentation. Runtime authority stays with provider_runtime.
use super::*;
use minicon_core::field_editor::{self, Command, Intent, Limits};
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
}

#[derive(Clone, Copy)]
enum Row {
    Local,
    Listener,
    Port,
    ListenerActions,
    Accounts,
    Login,
    LoginActions,
    Challenge,
    ChallengeActions,
    Account(usize),
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
    verification_url: provider_auth::Secret,
    user_code: provider_auth::Secret,
    deadline: Instant,
    remaining_seconds: u64,
}

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
    challenge: Option<DeviceChallenge>,
    challenge_expired: bool,
}

impl HarnessSettings {
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
            challenge: None,
            challenge_expired: false,
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
        }
    }

    fn editor_mut(&mut self, field: Field) -> &mut composer::ComposerState {
        match field {
            Field::Port => &mut self.port,
            Field::Model => &mut self.model,
        }
    }

    fn blur(&mut self) {
        field_editor::apply(&mut self.port, Command::Blur, PORT_LIMITS);
        field_editor::apply(&mut self.model, Command::Blur, MODEL_LIMITS);
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
            Row::Login => self.act(Action::Login(if column == 0 {
                Provider::DeepSeek
            } else {
                Provider::OpenCode
            })),
            Row::LoginActions => self.act(if column == 0 {
                Action::CancelLogin
            } else {
                Action::RefreshAccounts
            }),
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
                self.act(if column == 0 {
                    Action::SelectAccount(id)
                } else {
                    Action::RemoveAccount(id)
                });
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
        Row::Login => "login",
        Row::LoginActions => "login_actions",
        Row::Challenge => "device_challenge",
        Row::ChallengeActions => "device_actions",
        Row::NoAccounts => "no_accounts",
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
                    button(surface, cell(0, 2), "Add DeepSeek", false);
                    button(surface, cell(1, 2), "Add OpenCode", false);
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
                        settings.view.selected_account.as_ref() == Some(&account.id),
                    );
                    button(surface, cell(1, 2), "Remove account", false);
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
                        .error
                        .as_ref()
                        .or(settings.view.status.as_ref())
                        .cloned()
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
                    .error
                    .as_deref()
                    .or(settings.view.status.as_deref())
                    .map_or(json::JsonValue::Null, Into::into),
            ),
            ("port_editor", editor(Field::Port)),
            ("model_editor", editor(Field::Model)),
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
            settings.act(Action::Login(provider));
        }
        self.sync_embedded_page(window);
        self.mark_host_ui_full_and_repaint(window);
    }

    pub(super) fn cancel_harness_pointer(&mut self) {
        if let Some(settings) = &mut self.harness_settings {
            settings.drag = None;
            field_editor::apply(&mut settings.port, Command::Blur, PORT_LIMITS);
            field_editor::apply(&mut settings.model, Command::Blur, MODEL_LIMITS);
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
            changed = true;
        }
        for effect in effects {
            match effect {
                Effect::CopyClientKey(secret) => {
                    let _ = clipboard_status::set_text(secret.expose());
                }
                Effect::DeviceChallenge {
                    verification_url,
                    user_code,
                    expires_in,
                } => {
                    if let Some(settings) = &mut self.harness_settings {
                        let now = Instant::now();
                        settings.challenge = Some(DeviceChallenge {
                            verification_url,
                            user_code,
                            deadline: now.checked_add(expires_in).unwrap_or(now),
                            remaining_seconds: expires_in.as_secs(),
                        });
                        settings.challenge_expired = false;
                    }
                    changed = true;
                }
                Effect::OpenBrowser(url) => self.open_harness_login(window, url.expose()),
            }
        }
        if changed {
            self.mark_host_ui_full_and_repaint(window);
        }
    }

    fn open_harness_login(&mut self, window: &PixelWindow, url: &str) {
        self.blur_harness_fields();
        self.settings_open = false;
        self.close_browser_menu(window, false, false);
        self.present_http_link(window, url);
        self.sync_embedded_page(window);
        self.mark_host_ui_full_and_repaint(window);
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
                        .position(|row| {
                            matches!(
                                (field, row),
                                (Field::Port, Row::Port) | (Field::Model, Row::ModelInput)
                            )
                        })
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
                    let next = if settings.focused == Some(Field::Port) || !settings.port_editable()
                    {
                        Field::Model
                    } else {
                        Field::Port
                    };
                    settings.blur();
                    settings.focused = Some(next);
                    let index = rows
                        .iter()
                        .position(|row| {
                            matches!(
                                (next, row),
                                (Field::Port, Row::Port) | (Field::Model, Row::ModelInput)
                            )
                        })
                        .unwrap_or(0);
                    settings.scroll = geometry.rows.reveal(index);
                }
            }
            PixelWindowEvent::Keyboard(_) | PixelWindowEvent::Ime(_) => {
                if let Some(field) = settings.focused {
                    let limits = if field == Field::Port {
                        PORT_LIMITS
                    } else {
                        MODEL_LIMITS
                    };
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
                .position(|row| {
                    matches!(
                        (field, row),
                        (Field::Port, Row::Port) | (Field::Model, Row::ModelInput)
                    )
                })
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
