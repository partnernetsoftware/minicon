//! Read-only Harness destination; product owner: PRD 02.36.
use super::*;
pub(super) mod usage_report;
use crate::harness_account_ui as pool;
use provider_runtime::{ListenerState, LoadState, View};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Section {
    #[default]
    Overview,
    Accounts,
    Clients,
    Usage,
    Activity,
}
impl Section {
    const ALL: [Self; 5] = [
        Self::Overview,
        Self::Accounts,
        Self::Clients,
        Self::Usage,
        Self::Activity,
    ];
    fn title(self, language: ui::UiLanguage) -> &'static str {
        match self {
            Self::Overview => text(language, "Overview", "概览", "概覽"),
            Self::Accounts => text(language, "Accounts", "账号", "帳號"),
            Self::Clients => text(language, "Clients", "客户端", "用戶端"),
            Self::Usage => text(language, "Usage", "用量", "用量"),
            Self::Activity => text(language, "Activity", "活动", "活動"),
        }
    }
}
pub(super) fn text(
    language: ui::UiLanguage,
    en: &'static str,
    hans: &'static str,
    hant: &'static str,
) -> &'static str {
    match language {
        ui::UiLanguage::English => en,
        ui::UiLanguage::ChineseSimplified => hans,
        ui::UiLanguage::ChineseTraditional => hant,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum SettingsTarget {
    Service,
    Accounts(Option<String>),
    Connection,
    Client(String),
    Monitoring,
}

struct Card {
    id: String,
    title: String,
    lines: Vec<String>,
    target: SettingsTarget,
}

fn monitoring_lines(
    summary: &provider_server::usage::UsageSummary,
    language: ui::UiLanguage,
) -> Vec<String> {
    vec![
        usage_report::compact(summary, language),
        format!(
            "{}: {}",
            text(
                language,
                "Active model requests",
                "活动模型请求",
                "活動模型請求"
            ),
            summary.active_requests
        ),
        text(
            language,
            "External processes are not observed",
            "未观测外部进程",
            "未觀測外部程序",
        )
        .into(),
    ]
}

fn account_card(
    view: &View,
    account: &provider_runtime::AccountSummary,
    language: ui::UiLanguage,
) -> Card {
    let tr = |en, zh, tw| text(language, en, zh, tw);
    let network = view
        .account_preferences
        .iter()
        .find(|(id, _)| id == &account.id)
        .map_or_else(
            || {
                tr(
                    "Network policies unavailable",
                    "网络策略不可用",
                    "網路原則無法使用",
                )
                .into()
            },
            |(_, p)| {
                let mode = |p: &provider_runtime::NetworkPolicy| {
                    if p.proxy().is_some() {
                        tr("Proxy", "代理", "代理")
                    } else {
                        tr("Direct", "直连", "直連")
                    }
                };
                format!(
                    "{}: {} / {}: {}",
                    tr("Login", "登录", "登入"),
                    mode(&p.login),
                    tr("Service", "服务", "服務"),
                    mode(&p.service)
                )
            },
        );
    let mut lines = vec![
        format!(
            "{:?} · {}",
            account.provider,
            pool::state(view, account).caption(language)
        ),
        network,
        pool::resource_preview(view, account, language),
    ];
    lines.extend(pool::details(view, account, language));
    Card {
        id: account.id.clone(),
        title: account.label.clone(),
        lines,
        target: SettingsTarget::Accounts(Some(account.id.clone())),
    }
}

fn allocation_label(
    view: &View,
    row: &provider_runtime::ConnectionAllocationInput,
    language: ui::UiLanguage,
) -> String {
    let account = view
        .accounts
        .iter()
        .find(|r| Some(&r.id) == row.account.as_ref())
        .map_or_else(
            || {
                text(
                    language,
                    "Account unavailable",
                    "账号不可用",
                    "帳號無法使用",
                )
                .into()
            },
            |a| format!("{} ({})", a.label, pool::state(view, a).caption(language)),
        );
    format!("{} → {} / {}", row.alias, account, row.model)
}

fn client_card(
    view: &View,
    profile: &provider_runtime::ConnectionProfileView,
    language: ui::UiLanguage,
) -> Card {
    let tr = |en, zh, tw| text(language, en, zh, tw);
    let mut lines = Vec::new();
    let title = if let Some(applied) = &profile.applied {
        lines.push(format!(
            "{} / {}: {}",
            if applied.enabled {
                tr("Profile enabled", "配置已启用", "設定已啟用")
            } else {
                tr("Profile disabled", "配置已停用", "設定已停用")
            },
            tr("Applied mappings", "已应用映射", "已套用對應"),
            applied.allocations.len()
        ));
        let default = applied
            .allocations
            .iter()
            .find(|a| a.alias == applied.default_alias);
        lines.push(default.map_or_else(
            || {
                tr(
                    "No applied default model",
                    "尚无已应用的默认模型",
                    "尚無已套用的預設模型",
                )
                .into()
            },
            |a| allocation_label(view, a, language),
        ));
        applied.label.clone()
    } else {
        lines.push(
            tr(
                "Draft only — no applied connection",
                "仅有草稿，尚未应用连接",
                "僅有草稿，尚未套用連線",
            )
            .into(),
        );
        lines.push(
            tr(
                "Choose and verify model allocations",
                "请选择并验证模型分配",
                "請選擇並驗證模型分配",
            )
            .into(),
        );
        profile.input.label.clone()
    };
    lines.push(
        if profile.dirty && profile.applied.is_none() {
            tr("Unapplied draft", "尚未应用的草稿", "尚未套用的草稿")
        } else if profile.dirty {
            tr(
                "Unapplied changes; saved connection retained",
                "有未应用更改；保留已保存连接",
                "有未套用變更；保留已儲存連線",
            )
        } else {
            tr(
                "External process is not observed",
                "未观测外部进程",
                "未觀測外部程序",
            )
        }
        .into(),
    );
    if let Some(applied) = &profile.applied {
        for a in &applied.allocations {
            lines.push(allocation_label(view, a, language));
        }
    }
    Card {
        id: format!("client:{}", profile.id),
        title,
        lines,
        target: SettingsTarget::Client(profile.id.clone()),
    }
}
fn cards(view: Option<&View>, section: Section, language: ui::UiLanguage) -> Vec<Card> {
    let unknown = match view.map(|view| view.load_state) {
        Some(LoadState::Loading) => text(
            language,
            "Loading runtime metadata",
            "正在加载运行时数据",
            "正在載入執行階段資料",
        ),
        Some(LoadState::Unavailable(_)) => text(
            language,
            "Runtime unavailable — open Settings",
            "运行时暂不可用 — 打开设置",
            "執行階段暫不可用 — 開啟設定",
        ),
        _ => text(
            language,
            "Not loaded / unavailable",
            "未加载／暂不可用",
            "未載入／暫不可用",
        ),
    };
    let view = view.filter(|view| view.load_state == LoadState::Loaded);
    let card = |id: &str, title: &str, lines: Vec<String>, target| Card {
        id: id.into(),
        title: title.into(),
        lines,
        target,
    };
    let service = card(
        "service",
        text(language, "Local service", "本地服务", "本機服務"),
        vec![
            match view {
                None => unknown.into(),
                Some(view) => match view.listener {
                    ListenerState::Listening => view.actual_port.map_or_else(
                        || unknown.into(),
                        |port| {
                            format!(
                                "{} · {port}",
                                text(language, "Listening", "监听中", "監聽中")
                            )
                        },
                    ),
                    ListenerState::Stopped => text(language, "Stopped", "已停止", "已停止").into(),
                    ListenerState::Starting => {
                        text(language, "Starting", "启动中", "啟動中").into()
                    }
                    ListenerState::Draining => {
                        text(language, "Stopping", "停止中", "停止中").into()
                    }
                    ListenerState::Error => text(
                        language,
                        "Error — open Settings",
                        "错误 — 打开设置",
                        "錯誤 — 開啟設定",
                    )
                    .into(),
                },
            },
            text(
                language,
                "Opening this view does not start a service.",
                "打开此页不会启动服务。",
                "開啟此頁不會啟動服務。",
            )
            .into(),
        ],
        SettingsTarget::Service,
    );
    let supply = card(
        "accounts",
        text(language, "Account supply", "账号供给", "帳號供給"),
        view.and_then(pool::summary).map_or_else(
            || vec![unknown.into()],
            |s| {
                vec![
                    format!(
                        "{}: {}",
                        text(language, "Stored accounts", "已保存账号", "已儲存帳號"),
                        s.stored + s.tested + s.disabled + s.unknown
                    ),
                    format!(
                        "{}: {} / {}: {}",
                        text(language, "Previously tested", "曾通过测试", "曾通過測試"),
                        s.tested,
                        text(language, "Disabled", "已停用", "已停用"),
                        s.disabled
                    ),
                    format!(
                        "{}: {} / {}: {}",
                        text(language, "Reading attention", "读数需关注", "讀數需注意"),
                        s.attention,
                        text(language, "Unknown readings", "读数未知", "讀數未知"),
                        s.readings_unknown
                    ),
                ]
            },
        ),
        SettingsTarget::Accounts(None),
    );
    let connection = card(
        "connection",
        text(language, "Saved connection", "已保存连接", "已儲存連線"),
        view.map_or_else(
            || vec![unknown.into()],
            |v| {
                vec![
                    format!(
                        "{}: {}",
                        text(language, "Applied clients", "已应用客户端", "已套用用戶端"),
                        pool::configured_client_count(v)
                    ),
                    format!(
                        "{}: {}",
                        text(language, "Unapplied drafts", "未应用草稿", "未套用草稿"),
                        v.connection_profiles
                            .iter()
                            .filter(|p| p.dirty || p.applied.is_none())
                            .count()
                    ),
                    text(
                        language,
                        "External processes are not observed",
                        "未观测外部进程",
                        "未觀測外部程序",
                    )
                    .into(),
                ]
            },
        ),
        SettingsTarget::Connection,
    );
    let monitoring = card(
        "monitoring",
        text(language, "Monitoring", "监控", "監控"),
        view.map_or_else(
            || vec![unknown.into()],
            |v| monitoring_lines(&v.usage, language),
        ),
        SettingsTarget::Monitoring,
    );
    match section {
        Section::Overview => vec![service, supply, connection, monitoring],
        Section::Accounts => match view {
            Some(view) if !view.accounts.is_empty() => view
                .accounts
                .iter()
                .map(|account| account_card(view, account, language))
                .collect(),
            _ => vec![supply],
        },
        Section::Clients => match view {
            Some(view) if !view.connection_profiles.is_empty() => view
                .connection_profiles
                .iter()
                .map(|p| client_card(view, p, language))
                .collect(),
            _ => vec![connection],
        },
        Section::Usage => vec![monitoring],
        Section::Activity => vec![card(
            "runtime-status",
            text(
                language,
                "Current runtime status",
                "当前运行时状态",
                "目前執行階段狀態",
            ),
            vec![
                view.and_then(|v| v.status.clone())
                    .unwrap_or_else(|| unknown.into()),
                text(
                    language,
                    "Activity history is not loaded.",
                    "活动历史未加载。",
                    "活動歷史未載入。",
                )
                .into(),
            ],
            SettingsTarget::Service,
        )],
    }
}

struct ReturnContext {
    tab: Option<workspace::TabId>,
    home: bool,
    focus: browser_menu::MenuFocus,
    surface: InputSurface,
    tree_scroll: usize,
    notes_scroll: usize,
    history_scroll: usize,
}
#[derive(Default)]
pub(super) struct HarnessWorkspace {
    previous: Option<ReturnContext>,
    auth: Option<provider_runtime::AttemptId>,
    section: Section,
    scroll: usize,
    selected: Option<String>,
    focus: usize,
    header_focus: Option<usize>,
    header_return_focus: Option<browser_menu::MenuFocus>,
    settings_return_focus: Option<(
        Option<workspace::TabId>,
        browser_menu::MenuFocus,
        InputSurface,
    )>,
    entry_error: bool,
    report: usage_report::Report,
}
impl HarnessWorkspace {
    pub(super) fn active(&self) -> bool {
        self.previous.is_some() && self.auth.is_none()
    }
    pub(super) fn begin_auth(&mut self, attempt: provider_runtime::AttemptId) {
        if self.previous.is_some() {
            self.auth = Some(attempt);
        }
    }
    pub(super) fn finish_auth(&mut self, attempt: provider_runtime::AttemptId) {
        if self.auth == Some(attempt) {
            self.auth = None;
        }
    }
}

#[derive(Clone, Copy)]
struct Geometry {
    back: ui::Rect,
    nav: [ui::Rect; 5],
    body: ui::Rect,
    inspector: ui::Rect,
    configure: ui::Rect,
    row_height: u32,
}
fn geometry(layout: ui::Layout, width: u32, height: u32, scale: f64) -> Geometry {
    let d = |n: f64| (n * scale.max(1.0)) as u32;
    let pad = d(12.0);
    let available = width.saturating_sub(pad * 2);
    let top =
        (layout.harness.y + layout.harness.height).max(layout.settings.y + layout.settings.height);
    let back = ui::Rect {
        x: pad,
        y: top + pad,
        width: d(180.0).min(available.saturating_sub(pad) / 2),
        height: d(36.0),
    };
    let configure = ui::Rect {
        x: back.x + back.width + pad,
        y: back.y,
        width: available.saturating_sub(back.width + pad).min(d(220.0)),
        height: back.height,
    };
    let compact = width < d(760.0) || height < d(480.0);
    let nav = std::array::from_fn(|i| {
        if compact {
            ui::Rect {
                x: pad + (i as u32) * (available / 5),
                y: back.y + back.height + pad,
                width: (available / 5).saturating_sub(d(4.0)),
                height: d(32.0),
            }
        } else {
            ui::Rect {
                x: pad,
                y: back.y + back.height + pad + (i as u32) * d(36.0),
                width: d(132.0),
                height: d(32.0),
            }
        }
    });
    let body_x = if compact {
        pad
    } else {
        nav[0].x + nav[0].width + pad
    };
    let body_y = if compact {
        nav[0].y + nav[0].height + pad
    } else {
        nav[0].y
    };
    let inspector_width = if !compact && width >= d(1080.0) {
        d(250.0)
    } else {
        0
    };
    let body = ui::Rect {
        x: body_x,
        y: body_y,
        width: width.saturating_sub(body_x + pad + inspector_width),
        height: height.saturating_sub(body_y + pad),
    };
    let inspector = if inspector_width > 0 {
        ui::Rect {
            x: body.x + body.width + pad,
            y: body.y,
            width: inspector_width.saturating_sub(pad),
            height: body.height,
        }
    } else {
        ui::Rect::default()
    };
    Geometry {
        back,
        nav,
        body,
        inspector,
        configure,
        row_height: d(142.0).max(1),
    }
}

impl Geometry {
    fn start(self, count: usize, requested: usize) -> usize {
        requested.min(count.saturating_sub(self.visible()))
    }

    fn visible(self) -> usize {
        if self.body.height == 0 {
            0
        } else {
            ((self.body.height / self.row_height) as usize).max(1)
        }
    }
    fn row(self, index: usize, start: usize) -> ui::Rect {
        if index < start || index >= start + self.visible() {
            return ui::Rect::default();
        }
        ui::Rect {
            x: self.body.x,
            y: self.body.y + ((index - start) as u32) * self.row_height,
            width: self.body.width,
            height: self.row_height.min(self.body.height).saturating_sub(8),
        }
    }
}

impl ConApp {
    fn harness_cached_view(&self) -> Option<&View> {
        self.harness_settings
            .as_ref()
            .map(|settings| settings.cached_workspace_view())
    }
    pub(super) fn toggle_harness_workspace(
        &mut self,
        window: &PixelWindow,
    ) -> Result<(), PixelWindowError> {
        if self.browser_auth.is_some() {
            self.harness_workspace.entry_error = true;
            self.mark_host_ui_full_and_repaint(window);
            return Ok(());
        }
        if let Some(saved) = self.harness_workspace.previous.take() {
            self.settings_open = false;
            if let Some(id) = saved.tab.filter(|id| self.workspace.node(*id).is_some()) {
                self.activate_session(window, id);
            }
            let same = self.workspace.active() == saved.tab;
            self.tree_scroll_offset = saved.tree_scroll;
            if !same {
                self.reveal_active_tree_row(window)?;
            }
            self.notes_scroll = saved.notes_scroll;
            self.history_panel_scroll = saved.history_scroll;
            self.input_surface = saved.surface;
            self.home_active = saved.home;
            self.composer.focused = same && saved.focus == browser_menu::MenuFocus::Composer;
            self.notes_focused = same && saved.focus == browser_menu::MenuFocus::Notes;
            self.browser_address_focused =
                same && self.active_is_browser() && saved.focus == browser_menu::MenuFocus::Address;
            self.browser_proxy_focused =
                same && self.active_is_browser() && saved.focus == browser_menu::MenuFocus::Proxy;
            self.browser_chrome_trigger = if same {
                match saved.focus {
                    browser_menu::MenuFocus::NewTrigger => Some(browser_menu::ChromeTrigger::New),
                    browser_menu::MenuFocus::ProxyTrigger => {
                        Some(browser_menu::ChromeTrigger::Proxy)
                    }
                    _ => None,
                }
            } else {
                None
            };
            self.sync_embedded_page(window);
            if same && self.active_is_browser() && saved.focus == browser_menu::MenuFocus::Page {
                self.focus_browser_page(window);
            }
            if self.composer.focused {
                self.update_composer_ime_anchor(window)?;
            }
            if self.browser_address_focused {
                self.update_browser_ime_anchor(window);
            }
            if self.browser_proxy_focused {
                self.update_proxy_ime_anchor(window);
            }
        } else {
            if !self.park_page_for_harness(window) {
                self.harness_workspace.entry_error = true;
                self.mark_host_ui_full_and_repaint(window);
                return Ok(());
            }
            let focus = self
                .harness_workspace
                .header_return_focus
                .take()
                .or(self.menu_return_focus)
                .unwrap_or_else(|| self.current_menu_focus());
            let focus = match focus {
                browser_menu::MenuFocus::Settings | browser_menu::MenuFocus::Candidate => {
                    if self.active_is_browser() {
                        browser_menu::MenuFocus::Page
                    } else {
                        browser_menu::MenuFocus::Terminal
                    }
                }
                other => other,
            };
            self.harness_workspace.previous = Some(ReturnContext {
                tab: self.workspace.active(),
                home: self.home_active,
                focus,
                surface: self.input_surface,
                tree_scroll: self.tree_scroll_offset,
                notes_scroll: self.notes_scroll,
                history_scroll: self.history_panel_scroll,
            });
            self.close_browser_menu(window, false, false);
            self.blur_browser_composition();
            self.composer.focused = false;
            self.notes_focused = false;
            self.browser_address_focused = false;
            self.browser_proxy_focused = false;
            self.settings_proxy_focused = false;
            self.settings_open = false;
            self.home_active = false;
            self.harness_workspace.entry_error = false;
            self.harness_workspace.focus = 0;
            self.cancel_pointer_gestures_for_activation(window);
            self.sync_embedded_page(window);
            #[cfg(any(target_os = "macos", windows))]
            if let Some(page) = &self.native_browser_page {
                page.focus_parent();
            }
        }
        self.harness_workspace.header_focus = None;
        self.harness_workspace.header_return_focus = None;
        self.mark_host_ui_full_and_repaint(window);
        Ok(())
    }
    fn park_page_for_harness(&self, window: &PixelWindow) -> bool {
        if !self.active_is_browser() {
            return true;
        }
        #[cfg(any(target_os = "macos", windows))]
        {
            let _ = window;
            self.native_browser_page
                .as_ref()
                .is_none_or(|page| page.set_visible(false).is_ok())
        }
        #[cfg(target_os = "linux")]
        {
            self.browser_page.as_ref().is_none_or(|page| {
                self.content_panel_slot(window).is_some_and(|slot| {
                    browser::set_embedded_page_visible_for_process(slot.parent, page.id(), false)
                })
            })
        }
    }
    pub(super) fn open_harness_usage_report(
        &mut self,
        window: &PixelWindow,
    ) -> Result<(), PixelWindowError> {
        // Actual close intent invalidates Copy even for close/reopen between polls.
        self.settings_open = false;
        self.blur_harness_fields();
        self.invalidate_inactive_harness_key_copy();
        self.restore_harness_settings_focus(window);
        if !self.harness_workspace.active() {
            self.toggle_harness_workspace(window)?;
        }
        if self.harness_workspace.active() {
            self.harness_workspace.section = Section::Usage;
            self.harness_workspace.report.restore_focus();
            let latest = self
                .harness_settings
                .as_ref()
                .map(|s| s.cached_workspace_view().usage.clone());
            self.harness_workspace
                .report
                .reveal_latest(latest, Instant::now());
        }
        self.sync_embedded_page(window);
        self.mark_host_ui_full_and_repaint(window);
        Ok(())
    }
    pub(super) fn observe_harness_usage(
        &mut self,
        usage: std::sync::Arc<provider_server::usage::UsageSummary>,
        visible: bool,
        now: Instant,
    ) -> bool {
        self.harness_workspace.report.observe(usage, visible, now)
    }
    pub(super) fn harness_usage_presentation_deadline(&self) -> Option<Instant> {
        self.harness_workspace.report.deadline()
    }
    pub(super) fn service_harness_usage_presentation(
        &mut self,
        window: &PixelWindow,
        now: Instant,
    ) {
        let visible = self.harness_workspace.active()
            || (self.settings_open && self.settings_section == ui::SettingsSection::Harness);
        if self.harness_workspace.report.service(now, visible) {
            self.mark_host_ui_full_and_repaint(window);
        }
    }
    pub(super) fn presented_harness_usage(&self) -> Option<&provider_server::usage::UsageSummary> {
        self.harness_workspace
            .report
            .presented(self.harness_cached_view())
    }
    fn report_body(g: Geometry) -> ui::Rect {
        ui::Rect {
            width: if g.inspector.width > 0 {
                g.inspector
                    .x
                    .saturating_add(g.inspector.width)
                    .saturating_sub(g.body.x)
            } else {
                g.body.width
            },
            ..g.body
        }
    }
    fn open_workspace_settings(&mut self, window: &PixelWindow, target: SettingsTarget) {
        if !self.harness_workspace.active()
            && let Some(owner) = self.harness_workspace.header_return_focus.take()
        {
            self.harness_workspace.settings_return_focus =
                Some((self.workspace.active(), owner, self.input_surface));
        }
        self.harness_workspace.header_focus = None;
        self.harness_workspace.header_return_focus = None;
        self.settings_open = true;
        self.settings_section = ui::SettingsSection::Harness;
        self.poll_harness_settings(window);
        if let Some(settings) = &mut self.harness_settings {
            settings.show_workspace_target(target);
        }
        self.sync_embedded_page(window);
        self.mark_host_ui_full_and_repaint(window);
    }
    pub(super) fn restore_harness_settings_focus(&mut self, window: &PixelWindow) {
        if self.settings_open || self.harness_workspace.active() || self.browser_auth.is_some() {
            return;
        }
        let Some((tab, owner, surface)) = self.harness_workspace.settings_return_focus.take()
        else {
            return;
        };
        // An explicit tab change owns its own focus; never assign an old drawer
        // owner to another live page/session.
        if self.workspace.active() != tab {
            return;
        }
        self.harness_workspace.header_focus = None;
        self.harness_workspace.header_return_focus = None;
        self.input_surface = surface;
        self.composer.focused = owner == browser_menu::MenuFocus::Composer;
        self.notes_focused = owner == browser_menu::MenuFocus::Notes;
        self.settings_proxy_focused = false;
        self.browser_address_focused =
            self.active_is_browser() && owner == browser_menu::MenuFocus::Address;
        self.browser_proxy_focused =
            self.active_is_browser() && owner == browser_menu::MenuFocus::Proxy;
        self.browser_chrome_trigger = if self.active_is_browser() {
            match owner {
                browser_menu::MenuFocus::NewTrigger => Some(browser_menu::ChromeTrigger::New),
                browser_menu::MenuFocus::ProxyTrigger => Some(browser_menu::ChromeTrigger::Proxy),
                _ => None,
            }
        } else {
            None
        };
        if self.active_is_browser() && owner == browser_menu::MenuFocus::Page {
            self.focus_browser_page(window);
        }
        if self.composer.focused {
            let _ = self.update_composer_ime_anchor(window);
        }
        if self.browser_address_focused {
            self.update_browser_ime_anchor(window);
        }
        if self.browser_proxy_focused {
            self.update_proxy_ime_anchor(window);
        }
    }
    pub(super) fn harness_has_input_owner(&self) -> bool {
        self.harness_workspace.active() || self.harness_workspace.header_focus.is_some()
    }
    pub(super) fn harness_workspace_input_owner(&self) -> Option<&'static str> {
        if self.harness_workspace.header_focus.is_some() {
            Some("harness_header")
        } else if self.harness_workspace.active() {
            if self.settings_open && self.settings_proxy_focused {
                None
            } else if self.settings_open {
                Some("harness_settings_shell")
            } else {
                Some("harness_workspace")
            }
        } else {
            None
        }
    }
    pub(super) fn consume_harness_settings_input(
        &mut self,
        window: &PixelWindow,
        event: &PixelWindowEvent,
    ) -> bool {
        if !self.harness_workspace.active() || !self.settings_open {
            return false;
        }
        match event {
            PixelWindowEvent::Keyboard(key) => {
                if key.state == KeyPressState::Pressed
                    && key.modifiers.control
                    && key.modifiers.shift
                    && matches!(&key.logical,LogicalKey::Character(value)if value==",")
                {
                    self.settings_open = false;
                    self.mark_host_ui_full_and_repaint(window);
                    return true;
                }
                if key.state == KeyPressState::Pressed
                    && key.logical == LogicalKey::Named(NamedKey::Escape)
                    && !key.modifiers.control
                    && !key.modifiers.alt
                    && !key.modifiers.meta
                {
                    self.settings_open = false;
                    self.mark_host_ui_full_and_repaint(window);
                }
                true
            }
            PixelWindowEvent::Ime(_) => true,
            _ => false,
        }
    }
    fn leave_harness_header_focus(&mut self) {
        self.harness_workspace.header_focus = None;
        let owner = self.harness_workspace.header_return_focus.take();
        self.composer.focused = owner == Some(browser_menu::MenuFocus::Composer);
        self.notes_focused = owner == Some(browser_menu::MenuFocus::Notes);
        self.browser_chrome_trigger = if self.active_is_browser() {
            Some(if owner == Some(browser_menu::MenuFocus::NewTrigger) {
                browser_menu::ChromeTrigger::New
            } else {
                browser_menu::ChromeTrigger::Proxy
            })
        } else {
            None
        };
    }
    pub(super) fn route_harness_workspace(
        &mut self,
        window: &PixelWindow,
        event: &PixelWindowEvent,
    ) -> Result<bool, PixelWindowError> {
        let metrics = window.metrics()?;
        let scale = metrics.scale_factor.max(1.0);
        let layout = self.layout(metrics.physical_width, metrics.physical_height, scale);
        if let PixelWindowEvent::PointerButton {
            state: PointerButtonState::Pressed,
            button: PointerButton::Left,
            position: Some(position),
            ..
        } = event
        {
            let x = (position.x * scale).max(0.0) as u32;
            let y = (position.y * scale).max(0.0) as u32;
            if layout.harness.contains(x, y) {
                self.toggle_harness_workspace(window)?;
                return Ok(true);
            }
        }
        if self.settings_open {
            return Ok(false);
        }
        let active = self.harness_workspace.active();
        if let PixelWindowEvent::Keyboard(key) = event
            && key.state == KeyPressState::Pressed
            && !key.modifiers.control
            && !key.modifiers.alt
            && !key.modifiers.meta
        {
            let tab = key.logical == LogicalKey::Named(NamedKey::Tab);
            if !active
                && tab
                && !self.browser_proxy_menu_open
                && !self.new_tab_menu_open
                && self.harness_workspace.header_focus.is_none()
                && ((!key.modifiers.shift
                    && (self.workspace.active().is_none()
                        || self.composer.focused
                        || self.browser_chrome_trigger
                            == Some(browser_menu::ChromeTrigger::Proxy)))
                    || (key.modifiers.shift
                        && self.browser_chrome_trigger == Some(browser_menu::ChromeTrigger::New)))
            {
                self.harness_workspace.header_return_focus = Some(self.current_menu_focus());
                self.harness_workspace.header_focus = Some(usize::from(key.modifiers.shift));
                self.composer.focused = false;
                self.browser_chrome_trigger = None;
                self.mark_host_ui_full_and_repaint(window);
                return Ok(true);
            }
            if !active && let Some(focus) = self.harness_workspace.header_focus {
                if tab {
                    if (focus == 0 && key.modifiers.shift) || (focus == 1 && !key.modifiers.shift) {
                        self.harness_workspace.header_focus = None;
                        let owner = self.harness_workspace.header_return_focus.take();
                        if self.active_is_browser() {
                            self.browser_chrome_trigger = Some(if key.modifiers.shift {
                                browser_menu::ChromeTrigger::Proxy
                            } else {
                                browser_menu::ChromeTrigger::New
                            });
                        } else {
                            self.composer.focused =
                                owner == Some(browser_menu::MenuFocus::Composer);
                        }
                    } else {
                        self.harness_workspace.header_focus = Some(1 - focus);
                    }
                    self.mark_host_ui_full_and_repaint(window);
                    return Ok(true);
                }
                if matches!(
                    key.logical,
                    LogicalKey::Named(NamedKey::Enter | NamedKey::Space)
                ) || matches!(&key.logical,LogicalKey::Character(value)if value==" ")
                {
                    self.menu_key_latch = Some(
                        if matches!(&key.logical, LogicalKey::Named(NamedKey::Enter)) {
                            NamedKey::Enter
                        } else {
                            NamedKey::Space
                        },
                    );
                    if focus == 0 {
                        self.toggle_harness_workspace(window)?;
                    } else {
                        self.open_workspace_settings(window, SettingsTarget::Service);
                    }
                    return Ok(true);
                }
            }
        }
        if !active && self.harness_workspace.header_focus.is_some() {
            match event {
                PixelWindowEvent::Keyboard(key) => {
                    if key.state == KeyPressState::Pressed
                        && key.logical == LogicalKey::Named(NamedKey::Escape)
                        && !key.modifiers.control
                        && !key.modifiers.alt
                        && !key.modifiers.meta
                    {
                        self.leave_harness_header_focus();
                        self.mark_host_ui_full_and_repaint(window);
                    }
                    return Ok(true);
                }
                PixelWindowEvent::Ime(_) => return Ok(true),
                PixelWindowEvent::PointerButton {
                    state: PointerButtonState::Pressed,
                    ..
                }
                | PixelWindowEvent::FocusChanged(false) => self.leave_harness_header_focus(),
                _ => {}
            }
        }
        if !active {
            return Ok(false);
        }
        let g = geometry(
            layout,
            metrics.physical_width,
            metrics.physical_height,
            scale,
        );
        if self.harness_workspace.section == Section::Usage {
            let size = scaled_host_ui_font(14, self.host_font_logical(), scale);
            let view = self
                .harness_settings
                .as_ref()
                .map(|s| s.cached_workspace_view());
            let projection = self.harness_workspace.report.project(
                view,
                self.ui_language,
                Self::report_body(g),
                size,
            );
            let content_pointer = matches!(event, PixelWindowEvent::PointerButton { position: Some(p), .. }
                if Self::report_body(g).contains((p.x*scale).max(0.0) as u32,(p.y*scale).max(0.0) as u32));
            let content_key = self.harness_workspace.report.focused()
                || matches!(event, PixelWindowEvent::Keyboard(key) if key.logical == LogicalKey::Named(NamedKey::Tab)
                    && key.state == KeyPressState::Pressed && self.harness_workspace.focus == 6 && !key.modifiers.shift);
            if (content_pointer
                || content_key
                || matches!(event, PixelWindowEvent::MouseWheel { .. }))
                && let Some(intent) =
                    self.harness_workspace
                        .report
                        .route(event, view, &projection, scale)
            {
                match intent {
                    usage_report::Intent::Monitoring(account) => {
                        self.open_workspace_settings(window, SettingsTarget::Monitoring);
                        if let Some(settings) = &mut self.harness_settings {
                            settings.select_monitoring_account(account.as_deref());
                        }
                    }
                    usage_report::Intent::ClearHistory => {
                        let result = self.harness_settings.as_mut().map_or(
                            Err(provider_server::usage::UsageClearError::Unavailable),
                            |settings| settings.request_usage_clear(),
                        );
                        self.harness_workspace.report.clear_submitted(result);
                    }
                    usage_report::Intent::Leave(backwards) => {
                        self.harness_workspace.focus = if backwards { 6 } else { 0 }
                    }
                    usage_report::Intent::None => {}
                }
                self.mark_host_ui_full_and_repaint(window);
                return Ok(true);
            }
        }
        let rows = cards(
            self.harness_cached_view(),
            self.harness_workspace.section,
            self.ui_language,
        );
        let count = rows.len();
        self.harness_workspace.scroll = g.start(count, self.harness_workspace.scroll);
        match event {
            PixelWindowEvent::PointerButton {
                state: PointerButtonState::Pressed,
                button: PointerButton::Left,
                position: Some(position),
                ..
            } => {
                let x = (position.x * scale).max(0.0) as u32;
                let y = (position.y * scale).max(0.0) as u32;
                if layout.settings.contains(x, y) {
                    self.open_workspace_settings(window, SettingsTarget::Service);
                    return Ok(true);
                }
                if g.back.contains(x, y) {
                    self.toggle_harness_workspace(window)?;
                    return Ok(true);
                }
                if let Some(index) = g.nav.iter().position(|rect| rect.contains(x, y)) {
                    self.harness_workspace.report.leave_focus();
                    self.harness_workspace.section = Section::ALL[index];
                    self.harness_workspace.scroll = 0;
                    self.harness_workspace.selected = None;
                } else if g.configure.contains(x, y) {
                    let target = rows
                        .iter()
                        .find(|card| Some(&card.id) == self.harness_workspace.selected.as_ref())
                        .or_else(|| rows.first())
                        .map_or(SettingsTarget::Service, |card| card.target.clone());
                    self.open_workspace_settings(window, target);
                } else if let Some(card) = rows
                    .iter()
                    .enumerate()
                    .find(|(index, _)| g.row(*index, self.harness_workspace.scroll).contains(x, y))
                    .map(|(_, card)| card)
                {
                    self.harness_workspace.selected = Some(card.id.clone());
                }
            }
            PixelWindowEvent::Keyboard(key) if key.state == KeyPressState::Pressed => {
                if key.modifiers.control
                    && key.modifiers.shift
                    && matches!(&key.logical,LogicalKey::Character(value)if value==",")
                {
                    self.open_workspace_settings(window, SettingsTarget::Service);
                    return Ok(true);
                }
                if !key.modifiers.control && !key.modifiers.alt && !key.modifiers.meta {
                    let activation = matches!(
                        &key.logical,
                        LogicalKey::Named(NamedKey::Enter | NamedKey::Space)
                    ) || matches!(&key.logical,LogicalKey::Character(value)if value==" ");
                    if activation {
                        self.menu_key_latch = Some(
                            if matches!(&key.logical, LogicalKey::Named(NamedKey::Enter)) {
                                NamedKey::Enter
                            } else {
                                NamedKey::Space
                            },
                        );
                        let focus = self.harness_workspace.focus;
                        if focus == 0 {
                            self.toggle_harness_workspace(window)?;
                        } else if focus <= 5 {
                            self.harness_workspace.report.leave_focus();
                            self.harness_workspace.section = Section::ALL[focus - 1];
                            self.harness_workspace.scroll = 0;
                        } else {
                            self.open_workspace_settings(
                                window,
                                rows.iter()
                                    .find(|card| {
                                        Some(&card.id) == self.harness_workspace.selected.as_ref()
                                    })
                                    .or_else(|| rows.first())
                                    .map_or(SettingsTarget::Service, |card| card.target.clone()),
                            );
                        }
                        self.mark_host_ui_full_and_repaint(window);
                        return Ok(true);
                    }
                    match &key.logical {
                        LogicalKey::Named(NamedKey::Tab) => {
                            self.harness_workspace.focus = (self.harness_workspace.focus
                                + if key.modifiers.shift { 6 } else { 1 })
                                % 7
                        }

                        LogicalKey::Named(NamedKey::ArrowDown | NamedKey::PageDown) => {
                            self.harness_workspace.scroll = self
                                .harness_workspace
                                .scroll
                                .saturating_add(1)
                                .min(count.saturating_sub(g.visible()))
                        }
                        LogicalKey::Named(NamedKey::ArrowUp | NamedKey::PageUp) => {
                            self.harness_workspace.scroll =
                                self.harness_workspace.scroll.saturating_sub(1)
                        }
                        _ => {}
                    }
                }
            }
            PixelWindowEvent::MouseWheel { delta, .. } => {
                let direction = match delta {
                    WheelDelta::Lines { y, .. } => f64::from(*y),
                    WheelDelta::LogicalPixels { y, .. } => *y,
                    _ => 0.0,
                };
                if direction < 0.0 {
                    self.harness_workspace.scroll = self
                        .harness_workspace
                        .scroll
                        .saturating_add(1)
                        .min(count.saturating_sub(g.visible()));
                } else if direction > 0.0 {
                    self.harness_workspace.scroll = self.harness_workspace.scroll.saturating_sub(1);
                }
            }
            PixelWindowEvent::Keyboard(_)
            | PixelWindowEvent::PointerButton { .. }
            | PixelWindowEvent::Ime(_) => {}
            PixelWindowEvent::PointerMoved { .. } => {}
            _ => return Ok(false),
        }
        self.mark_host_ui_full_and_repaint(window);
        Ok(true)
    }
    pub(super) fn paint_harness_entry(
        &self,
        surface: &mut Surface<'_>,
        layout: ui::Layout,
        scale: f64,
        font_size: u16,
    ) {
        let t = theme::Theme::for_choice(self.ui_theme);
        paint_header_icon_button(
            surface,
            layout.harness,
            HeaderIcon::Harness,
            t.accent,
            self.harness_workspace.active(),
            t.surface,
            font_size,
            scale,
        );
        if self.harness_workspace.header_focus == Some(0)
            || self.hovered_header == Some(HeaderIcon::Harness)
        {
            host_ui::stroke_rect(surface, layout.harness, (scale as u32).max(1), t.accent);
        }
        if self.harness_workspace.header_focus == Some(1) {
            host_ui::stroke_rect(surface, layout.settings, (scale as u32).max(1), t.accent);
        }
        if (self.hovered_header == Some(HeaderIcon::Harness) && !self.harness_workspace.active())
            || self.harness_workspace.entry_error
        {
            let label = if self.harness_workspace.entry_error {
                text(
                    self.ui_language,
                    "Cannot switch browser view yet",
                    "暂时无法切换浏览器视图",
                    "暫時無法切換瀏覽器檢視",
                )
            } else {
                text(
                    self.ui_language,
                    "Harness — operation and monitoring",
                    "Harness — 运行与监控",
                    "Harness — 執行與監控",
                )
            };
            let y = layout.tree_header_height;
            surface.fill_rect(
                layout.sidebar.width,
                y,
                (360.0 * scale) as u32,
                (30.0 * scale) as u32,
                t.surface.to_xrgb(),
            );
            host_ui::paint_host_ui_text(
                surface,
                layout.sidebar.width + 4,
                y + 4,
                label,
                t.text,
                font_size,
                (350.0 * scale) as u32,
            );
        }
    }
    pub(super) fn paint_harness_workspace(
        &self,
        pixels: &mut [u32],
        width: u32,
        height: u32,
        scale: f64,
    ) {
        let mut surface = Surface::with_clip(
            pixels,
            width,
            height,
            PixelRect::from_xywh(0, 0, width, height),
        );
        let layout = self.layout(width, height, scale);
        let g = geometry(layout, width, height, scale);
        let t = theme::Theme::for_choice(self.ui_theme);
        let size = scaled_host_ui_font(14, self.host_font_logical(), scale);
        surface.fill_rect(0, 0, width, height, t.canvas_bg.to_xrgb());
        host_ui::paint_host_ui_text(
            &mut surface,
            layout.sidebar.width + 12,
            4,
            "Harness",
            t.accent,
            size,
            width.saturating_sub(layout.sidebar.width + 24),
        );
        self.paint_harness_entry(&mut surface, layout, scale, size);
        paint_header_icon_button(
            &mut surface,
            layout.settings,
            HeaderIcon::Settings,
            t.accent,
            self.settings_open,
            t.surface,
            size,
            scale,
        );
        let button = |surface: &mut Surface<'_>, rect: ui::Rect, label: &str, selected: bool| {
            surface.fill_rect(
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                if selected { t.surface } else { t.sidebar_bg }.to_xrgb(),
            );
            paint_button_label(surface, rect, label, t.text, size);
        };
        button(
            &mut surface,
            g.back,
            text(
                self.ui_language,
                "Back to workspace",
                "返回工作区",
                "返回工作區",
            ),
            self.harness_workspace.focus == 0,
        );
        button(
            &mut surface,
            g.configure,
            text(
                self.ui_language,
                "Open Harness settings",
                "打开 Harness 设置",
                "開啟 Harness 設定",
            ),
            self.harness_workspace.focus == 6,
        );
        for (index, section) in Section::ALL.iter().enumerate() {
            button(
                &mut surface,
                g.nav[index],
                section.title(self.ui_language),
                self.harness_workspace.section == *section
                    || self.harness_workspace.focus == index + 1,
            );
        }
        if self.harness_workspace.section == Section::Usage {
            let p = self.harness_workspace.report.project(
                self.harness_cached_view(),
                self.ui_language,
                Self::report_body(g),
                size,
            );
            self.harness_workspace
                .report
                .paint(&mut surface, &p, size, t);
        } else {
            let mut rows = cards(
                self.harness_cached_view(),
                self.harness_workspace.section,
                self.ui_language,
            );
            if let Some(summary) = self.presented_harness_usage()
                && let Some(card) = rows.iter_mut().find(|c| c.id == "monitoring")
            {
                card.lines = monitoring_lines(summary, self.ui_language);
            }
            let start = g.start(rows.len(), self.harness_workspace.scroll);
            for (index, card) in rows.iter().enumerate() {
                let rect = g.row(index, start);
                if rect.height == 0 {
                    continue;
                }
                surface.fill_rect(rect.x, rect.y, rect.width, rect.height, t.surface.to_xrgb());
                host_ui::paint_host_ui_text(
                    &mut surface,
                    rect.x + 8,
                    rect.y + 8,
                    &card.title,
                    t.accent,
                    size,
                    rect.width.saturating_sub(16),
                );
                for (line, value) in card.lines.iter().take(3).enumerate() {
                    host_ui::paint_host_ui_text(
                        &mut surface,
                        rect.x + 8,
                        rect.y + ((36 + line * 28) as f64 * scale) as u32,
                        value,
                        t.text,
                        size,
                        rect.width.saturating_sub(16),
                    );
                }
            }
            if g.inspector.width > 0 {
                if let Some(card) = rows
                    .iter()
                    .find(|card| Some(&card.id) == self.harness_workspace.selected.as_ref())
                {
                    let label = std::iter::once(card.title.as_str())
                        .chain(card.lines.iter().map(String::as_str))
                        .collect::<Vec<_>>()
                        .join("\n");
                    host_ui::paint_host_ui_paragraph(
                        &mut surface,
                        g.inspector,
                        &label,
                        t.text,
                        size,
                    );
                } else {
                    host_ui::paint_host_ui_paragraph(
                        &mut surface,
                        g.inspector,
                        text(
                            self.ui_language,
                            "Select a card to inspect",
                            "选择卡片查看详情",
                            "選擇卡片查看詳情",
                        ),
                        t.muted,
                        size,
                    );
                }
            }
        }
        if self.settings_open {
            let cost = self.ui_language.restart_cost(self.workspace.nodes().len());
            paint_settings_panel(
                &mut surface,
                layout,
                scale,
                t,
                self.ui_language,
                self.ui_theme,
                self.ui_language.help_lines(),
                size,
                self.font_zoom_percent(),
                &self.update,
                &cost,
                self.settings_section,
                &self.settings_proxy_input,
                self.settings_proxy_focused,
                self.settings_proxy_error.as_deref(),
                &self.config.browser_proxy_options,
                self.settings_proxy_list_start,
            );
            self.paint_harness_settings(&mut surface, layout, scale, size);
        }
    }
    pub(super) fn harness_workspace_snapshot(
        &self,
        layout: ui::Layout,
        width: u32,
        height: u32,
        scale: f64,
    ) -> json::JsonValue {
        let g = geometry(layout, width, height, scale);
        let rect = |r: ui::Rect| {
            json::object(vec![
                ("x", r.x.into()),
                ("y", r.y.into()),
                ("width", r.width.into()),
                ("height", r.height.into()),
            ])
        };
        let mut rows = cards(
            self.harness_cached_view(),
            self.harness_workspace.section,
            self.ui_language,
        );
        if self.harness_workspace.section == Section::Usage {
            rows.clear();
        }
        if let Some(summary) = self.presented_harness_usage()
            && let Some(card) = rows.iter_mut().find(|c| c.id == "monitoring")
        {
            card.lines = monitoring_lines(summary, self.ui_language);
        }
        let start = g.start(rows.len(), self.harness_workspace.scroll);
        json::object(vec![
            ("active", self.harness_workspace.active().into()),
            (
                "usage",
                if self.harness_workspace.section == Section::Usage {
                    let size = scaled_host_ui_font(14, self.host_font_logical(), scale);
                    let p = self.harness_workspace.report.project(
                        self.harness_cached_view(),
                        self.ui_language,
                        Self::report_body(g),
                        size,
                    );
                    self.harness_workspace.report.snapshot(
                        &p,
                        self.harness_cached_view(),
                        self.ui_language,
                    )
                } else {
                    json::JsonValue::Null
                },
            ),
            (
                "metadata",
                match self.harness_cached_view().map(|view| view.load_state) {
                    None => "not_loaded",
                    Some(LoadState::Loading) => "loading",
                    Some(LoadState::Loaded) => "loaded",
                    Some(LoadState::Unavailable(_)) => "unavailable",
                }
                .into(),
            ),
            ("entry", rect(layout.harness)),
            ("entry_error", self.harness_workspace.entry_error.into()),
            (
                "header_focus",
                match self.harness_workspace.header_focus {
                    Some(0) => "robot".into(),
                    Some(1) => "settings".into(),
                    _ => json::JsonValue::Null,
                },
            ),
            (
                "selected",
                self.harness_workspace
                    .selected
                    .as_ref()
                    .map_or(json::JsonValue::Null, |id| id.clone().into()),
            ),
            ("inspector", rect(g.inspector)),
            (
                "name",
                text(
                    self.ui_language,
                    "Harness — operation and monitoring",
                    "Harness — 运行与监控",
                    "Harness — 執行與監控",
                )
                .into(),
            ),
            ("back", rect(g.back)),
            ("settings", rect(g.configure)),
            (
                "section",
                self.harness_workspace
                    .section
                    .title(ui::UiLanguage::English)
                    .into(),
            ),
            ("scroll", start.into()),
            ("body", rect(g.body)),
            ("card_pitch", g.row_height.into()),
            ("visible_capacity", g.visible().into()),
            (
                "navigation",
                json::JsonValue::Array(g.nav.into_iter().map(rect).collect()),
            ),
            (
                "cards",
                json::JsonValue::Array(
                    rows.iter()
                        .enumerate()
                        .map(|(i, card)| {
                            json::object(vec![
                                ("id", card.id.clone().into()),
                                ("title", card.title.clone().into()),
                                (
                                    "lines",
                                    json::JsonValue::Array(
                                        card.lines.iter().cloned().map(Into::into).collect(),
                                    ),
                                ),
                                ("rect", rect(g.row(i, start))),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn h0_unknown_and_cached_states_do_not_invent_observation() {
        let rows = cards(None, Section::Overview, ui::UiLanguage::English);
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|r| {
            !r.lines
                .iter()
                .any(|line| line == "0" || line.starts_with("Online"))
        }));
        let view = View {
            load_state: LoadState::Loaded,
            busy: false,
            listener: ListenerState::Listening,
            actual_port: Some(23456),
            ..View::default()
        };
        let loading = View::default();
        let pending = cards(Some(&loading), Section::Overview, ui::UiLanguage::English);
        assert!(pending[0].lines[0].starts_with("Loading"));
        assert!(!pending[1].lines[0].contains("Stored accounts: 0"));
        let failed = View {
            load_state: LoadState::Unavailable(provider_runtime::RuntimeError::Storage),
            busy: false,
            ..View::default()
        };
        let unavailable = cards(Some(&failed), Section::Overview, ui::UiLanguage::English);
        assert!(unavailable[0].lines[0].starts_with("Runtime unavailable"));
        assert!(!unavailable[1].lines[0].contains("Stored accounts: 0"));
        let rows = cards(Some(&view), Section::Overview, ui::UiLanguage::English);
        assert!(rows[0].lines[0].contains("23456"));
        assert!(rows[1].lines[0].ends_with('0'));
        assert!(
            rows[3]
                .lines
                .iter()
                .any(|line| line == "Active model requests: 0")
        );
    }
    #[test]
    fn h1_cached_cards_keep_stale_wallets_and_applied_client_identity() {
        use crate::provider_monitoring as m;
        let mut view = View {
            load_state: LoadState::Loaded,
            ..Default::default()
        };
        view.accounts.push(provider_runtime::AccountSummary {
            id: "a".into(),
            provider: provider_runtime::Provider::DeepSeek,
            label: "Account A".into(),
            state: provider_accounts::AccountState::AccountReady,
        });
        view.account_preferences.push((
            "a".into(),
            provider_runtime::AccountPreferences::legacy("a"),
        ));
        std::sync::Arc::make_mut(&mut view.account_monitoring)
            .accounts
            .push(m::AccountMonitorView {
                account: "a".into(),
                provider: provider_runtime::Provider::DeepSeek,
                policy_revision: 0,
                policy: Default::default(),
                paused: Some(m::PauseReason::Off),
                active_request: None,
                next_due_unix_ms: None,
                last_success: Some(m::Reading {
                    source: m::MetadataSource::DeepSeekSummaryV0,
                    observed_unix_ms: 1,
                    provider_as_of: None,
                    resources: m::Resources::DeepSeekWallets(vec![m::Wallet {
                        kind: m::WalletKind::Normal,
                        currency: m::Currency::Cny,
                        balance: m::Decimal("1.2500".into()),
                    }]),
                }),
                freshness: m::Freshness::Stale,
                latest_attempt: Some(m::Attempt {
                    request: 1,
                    finished_unix_ms: Some(2),
                    result: Err(m::MonitorError::Forbidden),
                }),
                attention: true,
                wallet_capability: m::Capability::OfficialContract,
                allowance_capability: m::Capability::Unknown,
            });
        let applied = provider_runtime::ConnectionDraftInput {
            label: "Applied client".into(),
            enabled: true,
            default_alias: "work".into(),
            allocations: vec![provider_runtime::ConnectionAllocationInput {
                slot: 0,
                alias: "work".into(),
                account: Some("a".into()),
                model: "applied-model".into(),
            }],
        };
        let mut draft = applied.clone();
        draft.label = "Unapplied label".into();
        draft.allocations[0].model = "unapplied-model".into();
        view.connection_profiles
            .push(provider_runtime::ConnectionProfileView {
                id: "dddddddddddddddddddddddddddddddd".into(),
                revision: 2,
                input: draft,
                applied: Some(applied),
                dirty: true,
                error: None,
                allocations: vec![],
            });
        let mut new_profile = view.connection_profiles[0].clone();
        new_profile.id = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".into();
        new_profile.applied = None;
        new_profile.input.label = "Draft only client".into();
        view.connection_profiles.push(new_profile);
        let before = view.clone();
        let accounts = cards(Some(&view), Section::Accounts, ui::UiLanguage::English);
        assert!(
            accounts[0].lines[2].contains("Stale") && accounts[0].lines[2].contains("CNY: 1.2500")
        );
        assert!(
            accounts[0]
                .lines
                .iter()
                .any(|line| line.contains("Provider denied access"))
        );
        let clients = cards(Some(&view), Section::Clients, ui::UiLanguage::English);
        assert_eq!(clients[0].title, "Applied client");
        assert_eq!(clients[1].title, "Draft only client");
        assert!(clients[1].lines[0].starts_with("Draft only"));
        assert!(
            !clients[1]
                .lines
                .iter()
                .any(|line| line.contains("saved connection retained"))
        );
        assert!(
            clients[0]
                .lines
                .iter()
                .any(|line| line.contains("applied-model"))
        );
        assert!(
            !clients[0]
                .lines
                .iter()
                .any(|line| line.contains("unapplied-model"))
        );
        assert_eq!(
            clients[0].target,
            SettingsTarget::Client("dddddddddddddddddddddddddddddddd".into())
        );
        assert_eq!(view, before, "projecting cards cannot mutate the runtime");
        view.account_preferences[0].1.enabled = false;
        let accounts = cards(Some(&view), Section::Accounts, ui::UiLanguage::English);
        assert!(accounts[0].lines[0].contains("Disabled"));
        view.load_state = LoadState::Loading;
        let overview = cards(Some(&view), Section::Overview, ui::UiLanguage::English);
        assert!(overview[1].lines[0].contains("Loading"));
    }

    #[test]
    fn h0_auth_suspension_is_attempt_scoped_and_retains_the_return_context() {
        let mut state = HarnessWorkspace {
            previous: Some(ReturnContext {
                tab: None,
                home: false,
                focus: browser_menu::MenuFocus::Terminal,
                surface: InputSurface::Terminal,
                tree_scroll: 7,
                notes_scroll: 3,
                history_scroll: 2,
            }),
            ..HarnessWorkspace::default()
        };
        let attempt = provider_runtime::AttemptId(19);
        assert!(state.active());
        state.begin_auth(attempt);
        assert!(!state.active());
        state.finish_auth(provider_runtime::AttemptId(18));
        assert!(!state.active());
        state.finish_auth(attempt);
        assert!(state.active());
        assert_eq!(state.previous.as_ref().unwrap().tree_scroll, 7);
    }
    #[test]
    fn h0_resize_clamps_one_card_position_for_paint_hits_and_observation() {
        let small = geometry(ui::Layout::collapsed(540, 300, 1.0), 540, 300, 1.0);
        let large = geometry(ui::Layout::new(1400, 900, 1.0), 1400, 900, 1.0);
        assert_eq!(small.start(4, 3), 3);
        let start = large.start(4, 3);
        assert_eq!(start, 0);
        let first = large.row(0, start);
        assert!(first.contains(first.x + first.width / 2, first.y + first.height / 2));
        assert_eq!(
            (0..4).find(|index| large
                .row(*index, start)
                .contains(first.x + first.width / 2, first.y + first.height / 2)),
            Some(0)
        );
        assert_eq!(large.start(1, 3), 0);
    }
    #[test]
    fn h0_narrow_and_scaled_layout_keeps_back_and_cards_reachable() {
        for scale in [1.0, 1.5, 2.0] {
            for width in [420, 540, 1000, 1400] {
                for height in [300, 360, 700] {
                    for collapsed in [false, true] {
                        let w = (f64::from(width) * scale) as u32;
                        let h = (f64::from(height) * scale) as u32;
                        let layout =
                            ui::Layout::with_sidebar(w, h, scale, ui::SIDEBAR_WIDTH_DIP, collapsed);
                        let g = geometry(layout, w, h, scale);
                        assert!(
                            g.back.width > 0
                                && g.configure.width > 0
                                && g.body.width > 0
                                && g.visible() > 0
                        );
                        assert!(
                            g.back.x + g.back.width <= g.configure.x
                                || g.back.y + g.back.height <= g.configure.y
                        );
                        for rect in std::iter::once(g.back)
                            .chain(std::iter::once(g.configure))
                            .chain(g.nav)
                        {
                            assert!(
                                rect.x + rect.width <= w && rect.y + rect.height <= h,
                                "H0 control escapes {width}x{height}@{scale}: {rect:?}"
                            );
                        }
                        assert!(g.row(0, 0).height > 0);
                        assert_eq!(g.row(0, 1).height, 0);
                    }
                }
            }
        }
    }
}
