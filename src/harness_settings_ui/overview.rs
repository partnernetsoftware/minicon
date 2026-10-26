//! Read-only orientation and navigation; runtime actions remain explicit below.
use super::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum Section {
    #[default]
    Accounts,
    Clients,
    Monitoring,
    Service,
}

impl Section {
    pub(super) fn link(self) -> Link {
        match self {
            Self::Accounts => Link::Accounts,
            Self::Clients => Link::Allocations,
            Self::Monitoring => Link::Monitoring,
            Self::Service => Link::Service,
        }
    }
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Accounts => "accounts",
            Self::Clients => "clients",
            Self::Monitoring => "monitoring",
            Self::Service => "service",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Link {
    Next,
    Accounts,
    Allocations,
    Monitoring,
    Service,
}

impl Link {
    pub(super) const NAVIGATION: [Self; 4] = [
        Self::Accounts,
        Self::Allocations,
        Self::Monitoring,
        Self::Service,
    ];
    pub(super) fn section(self) -> Option<Section> {
        match self {
            Self::Next => None,
            Self::Accounts => Some(Section::Accounts),
            Self::Allocations => Some(Section::Clients),
            Self::Monitoring => Some(Section::Monitoring),
            Self::Service => Some(Section::Service),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Step {
    Loading,
    Unavailable,
    Authorization,
    Account,
    Allocation,
    AccountRepair,
    Draft,
    Key,
    Service,
    Handoff,
}

impl Step {
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Loading => "loading",
            Self::Unavailable => "unavailable",
            Self::Authorization => "authorization",
            Self::Account => "connect_account",
            Self::Allocation => "verify_allocation",
            Self::AccountRepair => "repair_account",
            Self::Draft => "review_draft",
            Self::Key => "client_key",
            Self::Service => "local_service",
            Self::Handoff => "client_handoff",
        }
    }

    pub(super) fn caption(self, language: ui::UiLanguage) -> &'static str {
        let (en, zh, tw) = match self {
            Self::Loading => ("Loading connections…", "正在加载连接…", "正在載入連線…"),
            Self::Unavailable => (
                "Review unavailable settings",
                "查看设置加载失败",
                "查看設定載入失敗",
            ),
            Self::Authorization => ("Review current sign-in", "查看当前登录", "查看目前登入"),
            Self::Account => ("1. Connect an account", "1. 连接账号", "1. 連接帳號"),
            Self::Allocation => (
                "2. Allocate and verify model",
                "2. 分配并验证模型",
                "2. 分配並驗證模型",
            ),
            Self::AccountRepair => (
                "Review unavailable account",
                "检查不可用账号",
                "檢查不可用帳號",
            ),
            Self::Draft => (
                "Review unapplied draft",
                "检查尚未应用的草稿",
                "檢查尚未套用的草稿",
            ),
            Self::Key => (
                "3. Set up the client key",
                "3. 设置客户端密钥",
                "3. 設定用戶端金鑰",
            ),
            Self::Service => (
                "3. Review local service",
                "3. 检查本地服务",
                "3. 檢查本機服務",
            ),
            Self::Handoff => (
                "3. View client connection steps",
                "3. 查看客户端连接步骤",
                "3. 查看用戶端連線步驟",
            ),
        };
        super::super::harness_workspace_ui::text(language, en, zh, tw)
    }
}

impl HarnessSettings {
    pub(super) fn toggle_section_picker(&mut self) {
        self.cancel_removal();
        self.cancel_account_menu();
        let open = !self.section_picker_open;
        self.blur();
        self.section_picker_open = open;
        self.focused = Some(if open {
            Field::Overview(self.section.link())
        } else {
            Field::SectionSelector
        });
    }

    pub(super) fn adapt_navigation(&mut self, rail: bool) {
        if rail {
            self.section_picker_open = false;
            if self.focused == Some(Field::SectionSelector) {
                self.focused = Some(Field::Overview(self.section.link()));
            }
        } else if !self.section_picker_open
            && matches!(self.focused, Some(Field::Overview(link)) if link != Link::Next)
        {
            self.focused = Some(Field::SectionSelector);
        }
    }

    pub(super) fn focus_targets(&self, rows: &[Row], rail: bool) -> Vec<FocusTarget> {
        if !matches!(self.removal, Removal::Closed) {
            let mut targets = vec![FocusTarget::Field(Field::AccountRemoval(false))];
            if self.removal_action().is_some() {
                targets.push(FocusTarget::Field(Field::AccountRemoval(true)));
            }
            return targets;
        }
        if self.account_menu.is_some() {
            return account_menu::Item::ACTIONS
                .into_iter()
                .map(|item| FocusTarget::Field(Field::AccountAction(item)))
                .collect();
        }
        let mut targets = Vec::new();
        if !rail && !self.section_picker_open {
            targets.push(FocusTarget::Field(Field::SectionSelector));
        }
        if rail || self.section_picker_open {
            targets.extend(
                Link::NAVIGATION
                    .into_iter()
                    .map(|link| FocusTarget::Field(Field::Overview(link))),
            );
        }
        if !rail && self.section_picker_open {
            return targets;
        }
        for &row in rows {
            if let Row::Monitoring(item) = row {
                if monitoring::MonitoringUi::interactive(item) {
                    targets.push(FocusTarget::Monitoring(item));
                }
            } else if let Some(field) = row_field(row)
                && (field != Field::Port || self.port_editable())
            {
                targets.push(FocusTarget::Field(field));
            }
        }
        targets
    }

    pub(super) fn move_focus(
        &mut self,
        backwards: bool,
        rows: &[Row],
        geometry: ui::HarnessSettingsLayout,
    ) {
        let targets = self.focus_targets(rows, geometry.navigation_rail);
        let current = self
            .monitoring
            .current()
            .map(FocusTarget::Monitoring)
            .or_else(|| self.focused.map(FocusTarget::Field));
        let index =
            current.and_then(|current| targets.iter().position(|target| *target == current));
        let next = match (index, backwards) {
            (Some(i), true) => (i + targets.len() - 1) % targets.len(),
            (Some(i), false) => (i + 1) % targets.len(),
            (None, true) => targets.len() - 1,
            (None, false) => 0,
        };
        let target = targets[next];
        self.blur();
        match target {
            FocusTarget::Field(field) => self.focused = Some(field),
            FocusTarget::Monitoring(item) => self.monitoring.focus_item(item),
        }
        if let Some(index) = rows.iter().position(|row| match target {
            FocusTarget::Field(field) => row_field(*row) == Some(field),
            FocusTarget::Monitoring(item) => matches!(row, Row::Monitoring(i) if *i == item),
        }) {
            self.scroll = geometry.rows.reveal(index);
        }
    }

    pub(super) fn select_section(&mut self, section: Section) {
        self.section_picker_open = false;
        if self.section != section {
            self.cancel_removal();
            self.cancel_account_menu();
            self.blur();
            self.section = section;
            self.scroll = 0;
        }
    }

    pub(super) fn configured_client_count(&self) -> usize {
        pool::configured_client_count(&self.view)
    }

    pub(super) fn orientation_step(&self) -> Step {
        match self.view.load_state {
            provider_runtime::LoadState::Loading => return Step::Loading,
            provider_runtime::LoadState::Unavailable(_) => return Step::Unavailable,
            provider_runtime::LoadState::Loaded => {}
        }
        if self.view.login_active {
            return Step::Authorization;
        }
        if self.view.accounts.is_empty() {
            return Step::Account;
        }
        let Some(form) = self.forms.current() else {
            return Step::Allocation;
        };
        let Some(applied) = &form.view.applied else {
            return Step::Allocation;
        };
        if !applied.enabled || applied.allocations.is_empty() {
            return Step::Allocation;
        }
        // Inspect applied account identities, never the draft allocation list.
        if applied.allocations.iter().any(|row| {
            !self
                .view
                .accounts
                .iter()
                .any(|a| Some(&a.id) == row.account.as_ref())
                || !self
                    .view
                    .account_preferences
                    .iter()
                    .any(|(id, p)| Some(id) == row.account.as_ref() && p.enabled)
        }) {
            return Step::AccountRepair;
        }
        if form.view.dirty || form.input() != *applied {
            return Step::Draft;
        }
        if form.view.allocations.len() != applied.allocations.len()
            || form.view.allocations.iter().any(|row| {
                row.availability != provider_runtime::ConnectionAvailability::Available
                    || row.evidence != provider_runtime::ConnectionEvidence::Verified
            })
        {
            return Step::Allocation;
        }
        if !self
            .view
            .profile_keys
            .iter()
            .any(|key| Some(&key.profile) == self.forms.selected.as_ref() && key.exists)
        {
            return Step::Key;
        }
        if self.view.listener != ListenerState::Listening || self.view.actual_port.is_none() {
            return Step::Service;
        }
        Step::Handoff
    }

    pub(super) fn overview_link(&mut self, link: Link) {
        self.section_picker_open = false;
        use super::super::harness_workspace_ui::SettingsTarget;
        let target = match link {
            Link::Accounts => Some(SettingsTarget::Accounts(None)),
            Link::Allocations => Some(SettingsTarget::Connection),
            Link::Monitoring => Some(SettingsTarget::Monitoring),
            Link::Service => Some(SettingsTarget::Service),
            Link::Next => None,
        };
        if let Some(target) = target {
            self.show_workspace_target(target);
            return;
        }
        let marker = match self.orientation_step() {
            Step::Loading => None,
            Step::Unavailable => Some(Row::Status),
            Step::Authorization | Step::Account | Step::AccountRepair => Some(Row::Accounts),
            Step::Allocation | Step::Draft => Some(Row::Model),
            Step::Key | Step::Handoff => Some(Row::Connect),
            Step::Service => Some(Row::Local),
        };
        if let Some(marker) = marker {
            self.select_section(match marker {
                Row::Accounts => Section::Accounts,
                Row::Model | Row::Connect => Section::Clients,
                Row::Local | Row::Status => Section::Service,
                _ => unreachable!("overview links have typed form destinations"),
            });
            self.blur();
            self.scroll = self
                .rows()
                .iter()
                .position(|row| std::mem::discriminant(row) == std::mem::discriminant(&marker))
                .unwrap_or(0);
        }
    }

    pub(super) fn overview_label(&self, index: u8, language: ui::UiLanguage) -> String {
        let tr = |en, zh, tw| super::super::harness_workspace_ui::text(language, en, zh, tw);
        if matches!(index, 1 | 3 | 5..=7)
            && self.view.load_state != provider_runtime::LoadState::Loaded
        {
            return match self.view.load_state {
                provider_runtime::LoadState::Loading => tr(
                    "Connections: loading…",
                    "连接：正在加载…",
                    "連線：正在載入…",
                ),
                _ => tr(
                    "Connections: unavailable",
                    "连接：无法加载",
                    "連線：無法載入",
                ),
            }
            .into();
        }
        match index {
            0 => tr("Model connections", "模型连接", "模型連線").into(),
            4 => self.status().unwrap_or_else(|| {
                tr(
                    "Sign-in, model checks and Start are explicit actions.",
                    "登录、模型验证和启动服务均需明确操作。",
                    "登入、模型驗證和啟動服務均需明確操作。",
                )
                .into()
            }),
            1 => format!(
                "{}: {}  /  {}: {}",
                tr("Accounts", "账号", "帳號"),
                self.view.accounts.len(),
                tr("Applied clients", "已应用客户端", "已套用用戶端"),
                self.configured_client_count()
            ),
            5..=7 => {
                let s = pool::summary(&self.view).unwrap_or_default();
                match index {
                    5 => format!(
                        "{}: {} / {}: {}",
                        tr("Previously tested", "曾通过测试", "曾通過測試"),
                        s.tested,
                        tr("Stored", "凭据已保存", "憑據已儲存"),
                        s.stored
                    ),
                    6 => format!(
                        "{}: {} / {}: {}",
                        tr("Disabled", "已停用", "已停用"),
                        s.disabled,
                        tr("Unknown policy", "策略未知", "原則未知"),
                        s.unknown
                    ),
                    _ => format!(
                        "{}: {} / {}: {}",
                        tr("Reading attention", "读数需关注", "讀數需注意"),
                        s.attention,
                        tr("Unknown readings", "读数未知", "讀數未知"),
                        s.readings_unknown
                    ),
                }
            }
            2 => format!(
                "{}: {}",
                tr("Local API", "本地 API", "本機 API"),
                match self.view.listener {
                    ListenerState::Stopped => tr("Stopped", "已停止", "已停止"),
                    ListenerState::Starting => tr("Starting", "正在启动", "正在啟動"),
                    ListenerState::Listening => tr("Listening", "监听中", "接聽中"),
                    ListenerState::Draining => tr("Stopping", "正在停止", "正在停止"),
                    ListenerState::Error => tr("Needs attention", "需要处理", "需要處理"),
                }
            ),
            _ => {
                let applied = self
                    .forms
                    .current()
                    .and_then(|form| form.view.applied.as_ref())
                    .filter(|a| !a.allocations.is_empty() && !a.default_alias.is_empty());
                match applied {
                    Some(applied) => format!(
                        "{}: {} / {}{}",
                        tr("Applied", "当前已应用", "目前已套用"),
                        applied.label,
                        applied.default_alias,
                        if applied.enabled {
                            ""
                        } else {
                            tr(" (disabled)", "（已停用）", "（已停用）")
                        }
                    ),
                    None => tr("Applied: none", "当前已应用：无", "目前已套用：無").into(),
                }
            }
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum FocusTarget {
    Field(Field),
    Monitoring(monitoring::Item),
}

pub(super) fn link_caption(link: Link, step: Step, language: ui::UiLanguage) -> &'static str {
    let (en, zh, tw) = match link {
        Link::Next => return step.caption(language),
        Link::Accounts => ("Account pool", "账号池", "帳號池"),
        Link::Allocations => ("Client allocations", "客户端分配", "用戶端分配"),
        Link::Monitoring => ("Usage and allowance", "用量与额度", "用量與額度"),
        Link::Service => ("Advanced / Local API", "高级 / 本地 API", "進階 / 本機 API"),
    };
    super::super::harness_workspace_ui::text(language, en, zh, tw)
}
