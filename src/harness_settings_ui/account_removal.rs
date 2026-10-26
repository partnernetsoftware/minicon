//! Transient product confirmation; the runtime retains logout/storage authority.
use super::*;

#[derive(Clone, PartialEq)]
pub(super) struct Review {
    account: AccountSummary,
    preferences: AccountPreferences,
    clients: Vec<(String, provider_runtime::ConnectionDraftInput)>,
}

impl Review {
    fn capture(view: &View, id: &str) -> Option<Self> {
        let account = view.accounts.iter().find(|a| a.id == id)?.clone();
        let preferences = view
            .account_preferences
            .iter()
            .find(|(a, _)| a == id)?
            .1
            .clone();
        let mut clients: Vec<_> = view
            .connection_profiles
            .iter()
            .filter_map(|profile| {
                let applied = profile.applied.as_ref()?;
                applied
                    .allocations
                    .iter()
                    .any(|a| a.account.as_deref() == Some(id))
                    .then(|| (profile.id.clone(), applied.clone()))
            })
            .collect();
        clients.sort_by(|a, b| a.0.cmp(&b.0));
        Some(Self {
            account,
            preferences,
            clients,
        })
    }
}

#[derive(Default)]
pub(super) enum Removal {
    #[default]
    Closed,
    Review(Review),
    Changed,
}

impl HarnessSettings {
    pub(super) fn cancel_removal(&mut self) {
        self.removal = Removal::Closed;
        if matches!(self.focused, Some(Field::AccountRemoval(_))) {
            self.focused = None;
        }
    }

    pub(super) fn begin_removal(&mut self, id: &str) {
        self.focus_account(id);
        self.blur();
        self.removal = Review::capture(&self.view, id).map_or(Removal::Changed, Removal::Review);
        self.focused = Some(Field::AccountRemoval(false));
        self.scroll = self
            .rows()
            .iter()
            .position(|r| matches!(r, Row::AccountRemovalInfo(0)))
            .unwrap_or(0);
    }

    pub(super) fn reconcile_removal(&mut self) {
        if let Removal::Review(review) = &self.removal
            && Review::capture(&self.view, &review.account.id).as_ref() != Some(review)
        {
            self.removal = Removal::Changed;
            self.focused = Some(Field::AccountRemoval(false));
        }
    }

    pub(super) fn removal_action(&self) -> Option<Action> {
        let Removal::Review(review) = &self.removal else {
            return None;
        };
        if self.view.busy
            || self.view.login_active
            || self.account_draft.submitted.is_some()
            || self
                .account_drafts
                .values()
                .any(|draft| draft.submitted.is_some())
            || Review::capture(&self.view, &review.account.id).as_ref() != Some(review)
        {
            return None;
        }
        Some(Action::RemoveAccount(review.account.id.clone()))
    }

    pub(super) fn confirm_removal(&mut self) {
        self.reconcile_removal();
        if let Some(action) = self.removal_action() {
            self.act(action);
            if self.error.is_none() {
                self.cancel_removal();
            }
        }
    }

    pub(super) fn removal_labels(&self, language: ui::UiLanguage) -> Vec<String> {
        let tr = |en, zh, tw| super::super::harness_workspace_ui::text(language, en, zh, tw);
        let Removal::Review(review) = &self.removal else {
            return if matches!(self.removal, Removal::Changed) {
                vec![
                    tr(
                        "Account or applied clients changed. Cancel and review removal again.",
                        "账号或已应用客户端已变化。请取消后重新复核移除。",
                        "帳號或已套用用戶端已變更。請取消後重新覆核移除。",
                    )
                    .into(),
                ]
            } else {
                Vec::new()
            };
        };
        let mut labels = vec![
            format!("{}: {} / {:?} [{}]", tr("Review removal", "复核移除", "覆核移除"), review.account.label, review.account.provider, review.account.id),
            format!("{}: {}", tr("Affected applied clients", "受影响的已应用客户端", "受影響的已套用用戶端"), review.clients.len()),
            tr("Active requests using this account will be cancelled. Its client allocations will need repair.", "使用此账号的进行中请求将被取消，相关客户端分配需要修复。", "使用此帳號的進行中請求將被取消，相關用戶端分配需要修復。").into(),
            tr("Local credentials will be removed. Remote sign-out is reported separately and may be unavailable.", "本地凭据将被移除。远端注销会单独报告，且可能不可用。", "本機憑據將被移除。遠端登出會單獨回報，且可能無法使用。").into(),
        ];
        labels.push(pool::activity_label(
            &self.view,
            &review.account.id,
            language,
        ));
        labels.extend(
            review
                .clients
                .iter()
                .map(|(id, applied)| format!("{} [{}]", applied.label, id)),
        );
        if self.removal_action().is_none() {
            labels.push(
                tr(
                    "Wait for the current operation or account save before confirming.",
                    "请等待当前操作或账号保存完成后再确认。",
                    "請等待目前操作或帳號儲存完成後再確認。",
                )
                .into(),
            );
        }
        labels
    }

    pub(super) fn removal_observation(&self) -> minicon_core::json::JsonValue {
        use minicon_core::json::{self, JsonValue};
        let (state, account, clients) = match &self.removal {
            Removal::Closed => ("closed", JsonValue::Null, 0),
            Removal::Changed => ("changed", JsonValue::Null, 0),
            Removal::Review(review) => (
                "review",
                review.account.id.as_str().into(),
                review.clients.len(),
            ),
        };
        json::object(vec![
            ("state", state.into()),
            ("account", account),
            ("affected_clients", clients.into()),
            ("can_confirm", self.removal_action().is_some().into()),
        ])
    }
}
