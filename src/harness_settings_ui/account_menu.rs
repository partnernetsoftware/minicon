//! Account-scoped rare actions. Existing form/runtime owners perform mutations.
use super::*;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Item {
    Open(usize),
    Close,
    Toggle,
    Reconnect,
    Remove,
}
impl Item {
    pub(super) const ACTIONS: [Self; 4] =
        [Self::Close, Self::Toggle, Self::Reconnect, Self::Remove];
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Open(_) => "account_actions_open",
            Self::Close => "account_menu_close",
            Self::Toggle => "account_menu_toggle",
            Self::Reconnect => "account_menu_reconnect",
            Self::Remove => "account_menu_remove",
        }
    }
}

impl HarnessSettings {
    pub(super) fn cancel_account_menu(&mut self) {
        self.account_menu = None;
        if matches!(self.focused, Some(Field::AccountAction(_))) {
            self.focused = None;
        }
    }

    pub(super) fn account_menu_action(&mut self, item: Item) {
        if let Item::Open(index) = item {
            let Some(account) = self.view.accounts.get(index) else {
                return;
            };
            let id = account.id.clone();
            self.cancel_removal();
            self.focus_account(&id);
            self.blur();
            self.account_menu = Some(id);
            self.focused = Some(Field::AccountAction(Item::Close));
            self.scroll = self
                .rows()
                .iter()
                .position(|r| matches!(r, Row::AccountMenuInfo(0)))
                .unwrap_or(0);
            return;
        }
        let Some(id) = self.account_menu.clone() else {
            return;
        };
        let Some(index) = self.view.accounts.iter().position(|a| a.id == id) else {
            self.cancel_account_menu();
            return;
        };
        if item == Item::Close {
            self.cancel_account_menu();
            self.focused = Some(Field::AccountAction(Item::Open(index)));
            self.scroll = self
                .rows()
                .iter()
                .position(|r| matches!(r, Row::Account(i) if *i == index))
                .unwrap_or(0);
            return;
        }
        // Resolve the named account, never whichever draft happened to be focused.
        self.focus_account(&id);
        if self.view.busy || self.view.login_active || self.account_draft.submitted.is_some() {
            return;
        }
        match item {
            Item::Toggle => {
                self.account_draft.enabled = !self.account_draft.enabled;
                self.cancel_account_menu();
                self.scroll = self
                    .rows()
                    .iter()
                    .position(|r| matches!(r, Row::AccountActions))
                    .unwrap_or(0);
            }
            Item::Reconnect if self.account_draft.dirty() => {
                // The visible menu explains why this cannot authorize using the draft.
            }
            Item::Reconnect => {
                self.activate(Row::AccountReconnect, 0);
                if self.error.is_none() {
                    self.cancel_account_menu();
                }
            }
            Item::Remove => {
                self.cancel_account_menu();
                self.begin_removal(&id);
            }
            Item::Open(_) | Item::Close => {}
        }
    }

    pub(super) fn account_menu_caption(
        &self,
        item: Item,
        language: ui::UiLanguage,
    ) -> &'static str {
        let tr = |en, zh, tw| super::super::harness_workspace_ui::text(language, en, zh, tw);
        match item {
            Item::Open(_) => tr("Actions", "操作", "操作"),
            Item::Close => tr("Close actions", "关闭操作菜单", "關閉操作選單"),
            Item::Toggle if self.account_draft.enabled => {
                tr("Disable in draft", "在草稿中停用", "在草稿中停用")
            }
            Item::Toggle => tr("Enable in draft", "在草稿中启用", "在草稿中啟用"),
            Item::Reconnect => tr("Reconnect this account", "重新连接此账号", "重新連線此帳號"),
            Item::Remove => tr(
                "Review sign-out and removal",
                "复核注销与移除",
                "覆核登出與移除",
            ),
        }
    }

    pub(super) fn account_menu_labels(&self, language: ui::UiLanguage) -> Vec<String> {
        let tr = |en, zh, tw| super::super::harness_workspace_ui::text(language, en, zh, tw);
        let Some(account) = self
            .view
            .accounts
            .iter()
            .find(|a| self.account_menu.as_ref() == Some(&a.id))
        else {
            return vec![];
        };
        let mut labels = vec![format!(
            "{}: {} / {:?} [{}]",
            tr("Account actions", "账号操作", "帳號操作"),
            account.label,
            account.provider,
            account.id
        )];
        labels.push(format!(
            "{}: {}",
            tr(
                "Affected applied clients",
                "受影响的已应用客户端",
                "受影響的已套用用戶端"
            ),
            pool::affected_clients(&self.view, &account.id)
        ));
        labels.extend(self.view.connection_profiles.iter().filter_map(|p| {
            let a = p.applied.as_ref()?;
            a.allocations
                .iter()
                .any(|r| r.account.as_deref() == Some(&account.id))
                .then(|| format!("{} [{}]", a.label, p.id))
        }));
        labels.push(format!(
            "{}: {}",
            tr(
                "Active requests across the service (account count unavailable)",
                "全服务活动请求（暂无账号级计数）",
                "全服務活動請求（暫無帳號級計數）"
            ),
            self.view.usage.active_requests
        ));
        labels.push(tr("Enable/disable changes the draft; Save applies it. Reconnect uses saved login settings and requires model verification afterwards. Removal has a separate review.", "启用或停用先修改草稿，保存后生效。重新连接使用已保存的登录设置，之后需重新验证模型。移除需要另行复核。", "啟用或停用先修改草稿，儲存後生效。重新連線使用已儲存的登入設定，之後需重新驗證模型。移除需要另行覆核。").into());
        if self.account_draft.dirty() {
            labels.push(
                tr(
                    "Save or revert account changes before reconnecting.",
                    "重新连接前请保存或撤销账号修改。",
                    "重新連線前請儲存或還原帳號修改。",
                )
                .into(),
            );
        }
        if self.view.busy || self.view.login_active || self.account_draft.submitted.is_some() {
            labels.push(
                tr(
                    "Wait for the current operation or account save.",
                    "请等待当前操作或账号保存完成。",
                    "請等待目前操作或帳號儲存完成。",
                )
                .into(),
            );
        }
        labels
    }
}
