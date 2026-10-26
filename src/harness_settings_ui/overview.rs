//! Read-only orientation and navigation; runtime actions remain explicit below.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Link {
    Next,
    Accounts,
    Allocations,
    Monitoring,
    Service,
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
    pub(super) fn configured_client_count(&self) -> usize {
        self.view
            .connection_profiles
            .iter()
            .filter(|p| {
                p.applied
                    .as_ref()
                    .is_some_and(|a| !a.allocations.is_empty() && !a.default_alias.is_empty())
            })
            .count()
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
        use super::super::harness_workspace_ui::SettingsTarget;
        let marker = match link {
            Link::Accounts => Some(Row::Accounts),
            Link::Allocations => Some(Row::Model),
            Link::Monitoring => {
                self.show_workspace_target(SettingsTarget::Monitoring);
                return;
            }
            Link::Service => Some(Row::Local),
            Link::Next => match self.orientation_step() {
                Step::Loading => None,
                Step::Unavailable => Some(Row::Status),
                Step::Authorization | Step::Account | Step::AccountRepair => Some(Row::Accounts),
                Step::Allocation | Step::Draft => Some(Row::Model),
                Step::Key | Step::Handoff => Some(Row::Connect),
                Step::Service => Some(Row::Local),
            },
        };
        if let Some(marker) = marker {
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
        if matches!(index, 1 | 3) && self.view.load_state != provider_runtime::LoadState::Loaded {
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
