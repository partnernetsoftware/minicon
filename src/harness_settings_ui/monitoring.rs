//! Monitoring form state. Only explicit actions reach the metadata owner.
use super::*;
use crate::harness_workspace_ui::usage_report::{
    attempt_label, freshness_label, pause_label, persistence_label,
};
use crate::provider_monitoring as monitor;
use std::collections::BTreeMap;

const LIMITS: Limits = Limits {
    max_bytes: 128,
    preedit_chars: 128,
};
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Item {
    Heading,
    Summary,
    OpenReport,
    Account(usize),
    State,
    Resources(usize),
    Enabled,
    Threshold(usize),
    ThresholdLabel(usize),
    RemoveThreshold(usize),
    AddThreshold,
    Save,
    Off,
    PreviousRead,
    Refresh,
    Resume,
    Feedback,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn account(id: &str) -> monitor::AccountMonitorView {
        monitor::AccountMonitorView {
            account: id.into(),
            provider: Provider::DeepSeek,
            policy_revision: 1,
            policy: monitor::PolicyInput {
                enabled: false,
                thresholds: vec![monitor::Threshold::WalletBelow {
                    kind: monitor::WalletKind::Normal,
                    currency: monitor::Currency::Cny,
                    value: monitor::Decimal("1".into()),
                }],
            },
            paused: Some(monitor::PauseReason::Off),
            active_request: None,
            next_due_unix_ms: None,
            last_success: None,
            freshness: monitor::Freshness::Never,
            latest_attempt: None,
            attention: false,
            wallet_capability: monitor::Capability::OfficialContract,
            allowance_capability: monitor::Capability::Unknown,
        }
    }
    fn view(accounts: Vec<monitor::AccountMonitorView>) -> View {
        View {
            account_monitoring: std::sync::Arc::new(monitor::MonitorView {
                accounts,
                persistence: monitor::Persistence::Ready,
                ..Default::default()
            }),
            ..View::default()
        }
    }
    #[test]
    fn monitoring_state_and_action_feedback_are_readable_without_changing_policy() {
        let mut a = account("a");
        a.freshness = monitor::Freshness::Stale;
        a.latest_attempt = Some(monitor::Attempt {
            request: 1,
            finished_unix_ms: Some(1),
            result: Err(monitor::MonitorError::NeedsAuth),
        });
        let v = view(vec![a]);
        let mut ui = MonitoringUi::default();
        ui.sync(&v);
        ui.forms.get_mut("a").unwrap().feedback = Some(Err(monitor::MonitorError::NeedsAuth));
        for (language, off, auth, saved) in [
            (
                ui::UiLanguage::English,
                "Monitoring off",
                "Sign in again",
                "Saved",
            ),
            (
                ui::UiLanguage::ChineseSimplified,
                "监测已关闭",
                "请重新登录",
                "已保存",
            ),
            (
                ui::UiLanguage::ChineseTraditional,
                "監測已關閉",
                "請重新登入",
                "已儲存",
            ),
        ] {
            let state = ui.label(Item::State, &v, &v.usage, language);
            assert!(
                state.contains(off) && state.contains(auth) && state.contains(saved),
                "visible state must distinguish off policy, failed read and saved cache: {state}"
            );
            let feedback = ui.label(Item::Feedback, &v, &v.usage, language);
            assert!(
                feedback.contains(auth),
                "the action must explain how to recover: {feedback}"
            );
            for label in [state, feedback] {
                assert!(
                    !label.contains('"') && !label.contains("Some(") && !label.contains("Err("),
                    "internal debug syntax is not user feedback: {label}"
                );
            }
        }
        assert!(!v.account_monitoring.accounts[0].policy.enabled);
        assert!(ui.forms["a"].pending.is_none());
    }
    #[test]
    fn monitoring_policy_receipt_preserves_later_text_caret_and_account_isolation() {
        let mut v = view(vec![account("a"), account("b")]);
        let mut ui = MonitoringUi::default();
        ui.sync(&v);
        assert!(matches!(
            ui.activate(Item::Save, &v, Some(7)),
            UiAction::Send(Action::SetAccountMonitoring { request: 7, .. })
        ));
        let f = ui.forms.get_mut("a").unwrap();
        f.thresholds[0].value.text = "22".into();
        f.thresholds[0].value.caret = 1;
        f.thresholds[0].value.anchor = Some(0);
        ui.select(Some("b"));
        let metadata = std::sync::Arc::make_mut(&mut v.account_monitoring);
        metadata.accounts[0].policy_revision = 2;
        metadata.receipts.push(monitor::MonitorReceipt {
            account: "a".into(),
            request: 7,
            operation: monitor::MonitorOperation::Policy,
            result: Ok(()),
            persistence: monitor::Persistence::Ready,
        });
        ui.sync(&v);
        let f = &ui.forms["a"];
        assert_eq!(
            (
                &f.thresholds[0].value.text,
                f.thresholds[0].value.caret,
                f.thresholds[0].value.anchor
            ),
            (&"22".to_owned(), 1, Some(0))
        );
        assert!(f.pending.is_none());
        assert_eq!(ui.selected.as_deref(), Some("b"));
        assert_eq!(ui.forms["b"].thresholds[0].value.text, "1");
    }
    #[test]
    fn monitoring_invalid_threshold_refuses_send_without_erasing_selection_or_retrying() {
        let v = view(vec![account("a")]);
        let mut ui = MonitoringUi::default();
        ui.sync(&v);
        let editor = &mut ui.forms.get_mut("a").unwrap().thresholds[0].value;
        editor.text = "-1".into();
        editor.caret = 1;
        editor.anchor = Some(0);
        assert!(matches!(
            ui.activate(Item::Save, &v, Some(1)),
            UiAction::None
        ));
        ui.sync(&v);
        let form = &ui.forms["a"];
        assert!(form.pending.is_none());
        assert_eq!(
            (
                &form.thresholds[0].value.text,
                form.thresholds[0].value.caret,
                form.thresholds[0].value.anchor
            ),
            (&"-1".to_owned(), 1, Some(0))
        );
    }
    #[test]
    fn monitoring_manual_read_and_stale_receipts_do_not_enable_policy_or_consume_another_request() {
        let mut v = view(vec![account("a")]);
        let mut ui = MonitoringUi::default();
        ui.sync(&v);
        assert!(matches!(
            ui.activate(Item::Refresh, &v, Some(2)),
            UiAction::Send(Action::RefreshAccountMetadata { request: 2, .. })
        ));
        std::sync::Arc::make_mut(&mut v.account_monitoring)
            .receipts
            .push(monitor::MonitorReceipt {
                account: "a".into(),
                request: 1,
                operation: monitor::MonitorOperation::Refresh,
                result: Ok(()),
                persistence: monitor::Persistence::Ready,
            });
        ui.sync(&v);
        assert_eq!(ui.forms["a"].pending.as_ref().map(|p| p.request), Some(2));
        assert!(!ui.forms["a"].enabled);
        ui.send_failed(
            &Action::RefreshAccountMetadata {
                request: 2,
                account: "a".into(),
            },
            provider_runtime::RuntimeError::Busy,
        );
        assert!(ui.forms["a"].pending.is_none());
        assert_eq!(
            ui.forms["a"].feedback,
            Some(Err(monitor::MonitorError::Busy))
        );
    }
    #[test]
    fn monitoring_go_percent_above_one_hundred_is_preserved_as_explicit_policy_not_clamped() {
        let mut a = account("a");
        a.provider = Provider::OpenCode;
        a.policy.thresholds = vec![monitor::Threshold::GoUsedAtLeast {
            value: monitor::Decimal("150.5".into()),
        }];
        let v = view(vec![a]);
        let mut ui = MonitoringUi::default();
        ui.sync(&v);
        let action = ui.activate(Item::Save, &v, Some(3));
        assert!(
            matches!(action, UiAction::Send(Action::SetAccountMonitoring { policy, .. }) if matches!(&policy.thresholds[0], monitor::Threshold::GoUsedAtLeast { value } if value.0 == "150.5"))
        );
    }
    fn active_read_fixture() -> (MonitoringUi, View) {
        let mut a = account("a");
        a.policy.enabled = true;
        let v = view(vec![a]);
        let mut ui = MonitoringUi::default();
        ui.sync(&v);
        assert!(matches!(
            ui.activate(Item::Refresh, &v, Some(10)),
            UiAction::Send(Action::RefreshAccountMetadata { request: 10, .. })
        ));
        let editor = &mut ui.forms.get_mut("a").unwrap().thresholds[0].value;
        editor.text = "invalid-threshold".into();
        editor.caret = 3;
        editor.anchor = Some(0);
        (ui, v)
    }
    fn receipt(
        request: u64,
        operation: monitor::MonitorOperation,
        result: Result<(), monitor::MonitorError>,
    ) -> monitor::MonitorReceipt {
        monitor::MonitorReceipt {
            account: "a".into(),
            request,
            operation,
            result,
            persistence: monitor::Persistence::Ready,
        }
    }
    #[test]
    fn monitoring_off_preempts_read_and_consumes_both_exact_receipts_without_erasing_invalid_draft()
    {
        for old_first in [false, true] {
            let (mut ui, mut v) = active_read_fixture();
            // Existing Save(false) must carry the explicit Off intention too.
            ui.activate(Item::Enabled, &v, None);
            let sent = ui.activate(Item::Save, &v, Some(11));
            assert!(
                matches!(&sent,UiAction::Send(Action::SetAccountMonitoring{request:11,policy,..}) if !policy.enabled&&policy.thresholds==v.account_monitoring.accounts[0].policy.thresholds)
            );
            assert_eq!(
                ui.forms["a"].retired_refresh.as_ref().map(|p| p.request),
                Some(10)
            );
            assert_eq!(ui.forms["a"].pending.as_ref().map(|p| p.request), Some(11));
            let old = receipt(
                10,
                monitor::MonitorOperation::Refresh,
                Err(monitor::MonitorError::Cancelled),
            );
            let off = receipt(11, monitor::MonitorOperation::Policy, Ok(()));
            let next = std::sync::Arc::make_mut(&mut v.account_monitoring);
            next.accounts[0].policy.enabled = false;
            next.accounts[0].policy_revision = 2;
            next.receipts = if old_first {
                vec![old, off]
            } else {
                vec![off, old]
            };
            ui.sync(&v);
            let form = &ui.forms["a"];
            assert!(form.pending.is_none() && form.retired_refresh.is_none());
            assert_eq!(form.feedback, Some(Ok(())));
            assert_eq!(
                form.refresh_terminal.as_ref().map(|r| r.result),
                Some(Err(monitor::MonitorError::Cancelled))
            );
            assert_eq!(
                (
                    form.thresholds[0].value.text.as_str(),
                    form.thresholds[0].value.caret,
                    form.thresholds[0].value.anchor
                ),
                ("invalid-threshold", 3, Some(0))
            );
            assert_eq!(form.off_thresholds_unsaved, Some(true));
            assert!(
                ui.label(
                    Item::Feedback,
                    &v,
                    v.usage.as_ref(),
                    ui::UiLanguage::English
                )
                .contains("Monitoring is off; threshold edits are not saved.")
            );
        }
    }
    #[test]
    fn monitoring_off_queue_rejection_restores_old_read_without_turning_its_late_success_into_saved()
     {
        let (mut ui, mut v) = active_read_fixture();
        let sent = ui.activate(Item::Off, &v, Some(11));
        assert!(
            matches!(&sent,UiAction::Send(Action::SetAccountMonitoring{policy,..})if !policy.enabled)
        );
        let UiAction::Send(action) = sent else {
            unreachable!("checked Off action")
        };
        ui.send_failed(&action, provider_runtime::RuntimeError::Busy);
        assert_eq!(ui.forms["a"].pending.as_ref().map(|p| p.request), Some(10));
        assert!(ui.forms["a"].retired_refresh.is_none());
        assert_eq!(
            ui.forms["a"].feedback,
            Some(Err(monitor::MonitorError::Busy))
        );
        std::sync::Arc::make_mut(&mut v.account_monitoring).receipts =
            vec![receipt(10, monitor::MonitorOperation::Refresh, Ok(()))];
        ui.sync(&v);
        assert_eq!(
            ui.forms["a"].feedback,
            Some(Err(monitor::MonitorError::Busy))
        );
        assert_eq!(
            ui.forms["a"].refresh_terminal.as_ref().map(|r| r.result),
            Some(Ok(()))
        );
        assert!(ui.forms["a"].pending.is_none());
        assert_eq!(ui.forms["a"].off_thresholds_unsaved, None);
        assert!(
            v.account_monitoring.accounts[0].policy.enabled,
            "failed Off must not pretend the applied policy changed"
        );
        assert!(
            ui.label(
                Item::Feedback,
                &v,
                v.usage.as_ref(),
                ui::UiLanguage::English
            )
            .contains("Busy")
        );
    }
    #[test]
    fn monitoring_off_refuses_policy_overlap_and_a_third_inflight_slot() {
        let mut a = account("a");
        a.policy.enabled = true;
        let v = view(vec![a]);
        let mut ui = MonitoringUi::default();
        ui.sync(&v);
        assert!(matches!(
            ui.activate(Item::Save, &v, Some(1)),
            UiAction::Send(Action::SetAccountMonitoring { .. })
        ));
        assert!(matches!(
            ui.activate(Item::Off, &v, Some(2)),
            UiAction::None
        ));
        assert_eq!(ui.forms["a"].pending.as_ref().map(|p| p.request), Some(1));
        assert!(ui.forms["a"].retired_refresh.is_none());
        let (mut ui, v) = active_read_fixture();
        assert!(matches!(
            ui.activate(Item::Off, &v, Some(11)),
            UiAction::Send(Action::SetAccountMonitoring { .. })
        ));
        assert!(matches!(
            ui.activate(Item::Off, &v, Some(12)),
            UiAction::None
        ));
        assert_eq!(
            ui.forms["a"].retired_refresh.as_ref().map(|p| p.request),
            Some(10)
        );
        assert_eq!(ui.forms["a"].pending.as_ref().map(|p| p.request), Some(11));
    }
    #[test]
    fn monitoring_actual_off_control_disables_with_applied_thresholds_independently_of_invalid_editor()
     {
        let mut a = account("a");
        a.policy.enabled = true;
        let v = view(vec![a]);
        let mut ui = MonitoringUi::default();
        ui.sync(&v);
        let editor = &mut ui.forms.get_mut("a").unwrap().thresholds[0].value;
        editor.text = "-not-a-number".into();
        editor.caret = 2;
        editor.anchor = Some(0);
        assert!(ui.rows(&v).contains(&Item::Off));
        assert!(MonitoringUi::interactive(Item::Off));
        let sent = ui.activate(Item::Off, &v, Some(1));
        assert!(
            matches!(sent,UiAction::Send(Action::SetAccountMonitoring{policy,..})if !policy.enabled&&policy.thresholds==v.account_monitoring.accounts[0].policy.thresholds)
        );
        assert_eq!(
            (
                ui.forms["a"].thresholds[0].value.text.as_str(),
                ui.forms["a"].thresholds[0].value.caret,
                ui.forms["a"].thresholds[0].value.anchor
            ),
            ("-not-a-number", 2, Some(0))
        );
        assert!(!ui.forms["a"].enabled);
        for language in [
            ui::UiLanguage::English,
            ui::UiLanguage::ChineseSimplified,
            ui::UiLanguage::ChineseTraditional,
        ] {
            let caption = ui.label(Item::Off, &v, v.usage.as_ref(), language);
            assert!(
                host_ui::host_ui_text_width(&caption, 14) < 280,
                "Off caption must fit supported narrow cell: {caption}"
            );
        }
    }
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum Resource {
    Wallet(monitor::WalletKind, monitor::Currency),
    Go,
}
impl Resource {
    fn threshold(self, value: String) -> monitor::Threshold {
        match self {
            Self::Wallet(kind, currency) => monitor::Threshold::WalletBelow {
                kind,
                currency,
                value: monitor::Decimal(value),
            },
            Self::Go => monitor::Threshold::GoUsedAtLeast {
                value: monitor::Decimal(value),
            },
        }
    }
}
struct ThresholdDraft {
    resource: Resource,
    value: composer::ComposerState,
}
struct Pending {
    request: u64,
    operation: monitor::MonitorOperation,
    submitted: monitor::PolicyInput,
    off_only: bool,
}
struct Form {
    revision: u64,
    baseline: monitor::PolicyInput,
    enabled: bool,
    thresholds: Vec<ThresholdDraft>,
    pending: Option<Pending>,
    // One earlier manual read can coexist only with explicit Off. No queue.
    retired_refresh: Option<Pending>,
    refresh_terminal: Option<monitor::MonitorReceipt>,
    latest_intent: u64,
    off_thresholds_unsaved: Option<bool>,
    feedback: Option<Result<(), monitor::MonitorError>>,
}
impl Form {
    fn new(a: &monitor::AccountMonitorView) -> Self {
        let mut f = Self {
            revision: a.policy_revision,
            baseline: a.policy.clone(),
            enabled: a.policy.enabled,
            thresholds: Vec::new(),
            pending: None,
            retired_refresh: None,
            refresh_terminal: None,
            latest_intent: 0,
            off_thresholds_unsaved: None,
            feedback: None,
        };
        f.set_thresholds(&a.policy);
        f
    }
    fn set_thresholds(&mut self, policy: &monitor::PolicyInput) {
        self.thresholds = policy
            .thresholds
            .iter()
            .map(|t| {
                let (resource, value) = match t {
                    monitor::Threshold::WalletBelow {
                        kind,
                        currency,
                        value,
                    } => (Resource::Wallet(*kind, *currency), value),
                    monitor::Threshold::GoUsedAtLeast { value } => (Resource::Go, value),
                };
                let mut editor = composer::ComposerState::default();
                editor.text = value.0.clone();
                editor.caret = editor.text.len();
                ThresholdDraft {
                    resource,
                    value: editor,
                }
            })
            .collect();
    }
    fn input(&self) -> monitor::PolicyInput {
        monitor::PolicyInput {
            enabled: self.enabled,
            thresholds: self
                .thresholds
                .iter()
                .map(|t| t.resource.threshold(t.value.text.clone()))
                .collect(),
        }
    }
    fn receive(
        &mut self,
        receipt: &monitor::MonitorReceipt,
        current: &monitor::AccountMonitorView,
    ) {
        if self.retired_refresh.as_ref().is_some_and(|pending| {
            pending.request == receipt.request && pending.operation == receipt.operation
        }) {
            self.retired_refresh = None;
            self.refresh_terminal = Some(receipt.clone());
            return;
        }
        let Some(pending) = self
            .pending
            .as_ref()
            .filter(|p| p.request == receipt.request && p.operation == receipt.operation)
        else {
            return;
        };
        let current_intent = pending.request == self.latest_intent;
        let off_only = pending.off_only;
        if pending.operation == monitor::MonitorOperation::Refresh {
            self.refresh_terminal = Some(receipt.clone());
        }
        if !current_intent {
            self.pending = None;
            return;
        }
        if receipt.result.is_ok() && pending.operation == monitor::MonitorOperation::Policy {
            // Preserve later text/caret. Merge only unchanged submitted fields.
            if self.enabled == pending.submitted.enabled {
                self.enabled = current.policy.enabled;
            }
            let resources_unchanged = self.thresholds.len() == pending.submitted.thresholds.len()
                && self
                    .thresholds
                    .iter()
                    .zip(&pending.submitted.thresholds)
                    .all(|(t, s)| match s {
                        monitor::Threshold::WalletBelow { kind, currency, .. } => {
                            t.resource == Resource::Wallet(*kind, *currency)
                        }
                        monitor::Threshold::GoUsedAtLeast { .. } => t.resource == Resource::Go,
                    });
            if resources_unchanged {
                for (t, s) in self
                    .thresholds
                    .iter_mut()
                    .zip(&pending.submitted.thresholds)
                {
                    let submitted = match s {
                        monitor::Threshold::WalletBelow { value, .. }
                        | monitor::Threshold::GoUsedAtLeast { value } => &value.0,
                    };
                    if t.value.text == *submitted {
                        let current_value =
                            current
                                .policy
                                .thresholds
                                .iter()
                                .find_map(|v| match (v, t.resource) {
                                    (
                                        monitor::Threshold::WalletBelow {
                                            kind,
                                            currency,
                                            value,
                                        },
                                        Resource::Wallet(k, c),
                                    ) if *kind == k && *currency == c => Some(&value.0),
                                    (monitor::Threshold::GoUsedAtLeast { value }, Resource::Go) => {
                                        Some(&value.0)
                                    }
                                    _ => None,
                                });
                        if let Some(value) = current_value
                            && t.value.text != *value
                        {
                            t.value.text = value.clone();
                            t.value.caret = t.value.caret.min(value.len());
                            t.value.anchor = None;
                        }
                    }
                }
            }
        }
        // A conflict advances observed revision but retains the user's dirty draft.
        self.revision = current.policy_revision;
        self.baseline = current.policy.clone();
        self.feedback = Some(receipt.result);
        self.off_thresholds_unsaved =
            if off_only && receipt.result.is_ok() && !current.policy.enabled {
                Some(self.input().thresholds != current.policy.thresholds)
            } else {
                None
            };
        self.pending = None;
    }
}
#[derive(Clone, Copy)]
enum LocalError {
    TooLong,
    Busy,
    WorkerUnavailable,
    InvalidDecimal,
}
impl LocalError {
    fn label(self, l: ui::UiLanguage) -> &'static str {
        let t = super::super::harness_workspace_ui::text;
        match self {
            Self::TooLong => t(
                l,
                "Threshold limit: 128 bytes; draft retained.",
                "阈值上限128字节，草稿已保留。",
                "門檻上限128位元組，草稿已保留。",
            ),
            Self::Busy => t(
                l,
                "This account is busy; no repeat request.",
                "此账号忙碌，未重复请求。",
                "此帳號忙碌，未重複請求。",
            ),
            Self::WorkerUnavailable => t(
                l,
                "Metadata worker unavailable; nothing sent.",
                "元数据工作器不可用，未发送。",
                "中繼資料工作器無法使用，未傳送。",
            ),
            Self::InvalidDecimal => t(
                l,
                "Use a nonnegative decimal; draft retained.",
                "请输入非负十进制数，草稿已保留。",
                "請輸入非負十進位數，草稿已保留。",
            ),
        }
    }
}
#[derive(Default)]
pub(super) struct MonitoringUi {
    selected: Option<String>,
    forms: BTreeMap<String, Form>,
    focused: Option<(String, usize)>,
    navigation: Option<Item>,
    drag: Option<usize>,
    clicks: minicon_core::click::ClickCounter<usize>,
    local_error: Option<LocalError>,
}
pub(super) enum UiAction {
    None,
    OpenReport,
    Send(Action),
}
impl MonitoringUi {
    pub(super) fn select(&mut self, account: Option<&str>) {
        self.blur();
        if let Some(account) = account {
            self.selected = Some(account.into());
        }
    }
    pub(super) fn sync(&mut self, view: &View) {
        for a in &view.account_monitoring.accounts {
            let form = self
                .forms
                .entry(a.account.clone())
                .or_insert_with(|| Form::new(a));
            // A View can contain both account-matching terminals. Consume all
            // exact requests before the next Runtime.act acknowledges this View.
            for receipt in view
                .account_monitoring
                .receipts
                .iter()
                .filter(|r| r.account == a.account)
            {
                form.receive(receipt, a);
            }
            if form.pending.is_none()
                && form.retired_refresh.is_none()
                && form.input() == form.baseline
                && form.revision != a.policy_revision
            {
                form.enabled = a.policy.enabled;
                form.revision = a.policy_revision;
                form.baseline = a.policy.clone();
                form.set_thresholds(&a.policy);
            }
        }
        // A selected removed account stays a visible unavailable identity. A new
        // account cannot inherit its draft by list position.
        self.forms.retain(|id, form| {
            view.account_monitoring
                .accounts
                .iter()
                .any(|a| &a.account == id)
                || form.pending.is_some()
                || form.retired_refresh.is_some()
                || self.selected.as_ref() == Some(id)
        });
        if self.selected.is_none() {
            self.selected = view
                .account_monitoring
                .accounts
                .first()
                .map(|a| a.account.clone());
        }
    }
    fn account<'a>(&self, view: &'a View) -> Option<&'a monitor::AccountMonitorView> {
        let id = self.selected.as_ref()?;
        view.account_monitoring
            .accounts
            .iter()
            .find(|a| &a.account == id)
    }
    pub(super) fn rows(&self, view: &View) -> Vec<Item> {
        let mut rows = vec![Item::Heading, Item::Summary, Item::OpenReport];
        rows.extend((0..view.account_monitoring.accounts.len()).map(Item::Account));
        rows.push(Item::State);
        if let Some(account) = self.account(view) {
            let resource_count = account
                .last_success
                .as_ref()
                .map_or(0, |r| match &r.resources {
                    monitor::Resources::DeepSeekWallets(v) => v.len(),
                    monitor::Resources::OpenCodeGo(v) => v.len(),
                });
            rows.extend((0..resource_count).map(Item::Resources));
            rows.push(Item::Enabled);
            if let Some(form) = self.forms.get(&account.account) {
                for i in 0..form.thresholds.len() {
                    rows.extend([Item::ThresholdLabel(i), Item::Threshold(i)]);
                }
            }
            rows.extend([
                Item::AddThreshold,
                Item::Save,
                Item::Off,
                Item::Refresh,
                Item::Resume,
            ]);
        }
        rows.push(Item::Feedback);
        if self
            .selected
            .as_ref()
            .and_then(|id| self.forms.get(id))
            .is_some_and(|f| f.refresh_terminal.is_some())
        {
            rows.push(Item::PreviousRead);
        }
        rows
    }
    pub(super) fn id(item: Item) -> String {
        match item {
            Item::Heading => "monitoring".into(),
            Item::Summary => "monitoring_usage_summary".into(),
            Item::OpenReport => "monitoring_open_usage".into(),
            Item::Account(i) => format!("monitoring_account_{i}"),
            Item::State => "monitoring_state".into(),
            Item::Resources(i) => format!("monitoring_resource_{i}"),
            Item::Enabled => "monitoring_enabled".into(),
            Item::Threshold(i) => format!("monitoring_threshold_{i}"),
            Item::ThresholdLabel(i) => format!("monitoring_threshold_label_{i}"),
            Item::RemoveThreshold(i) => format!("monitoring_threshold_remove_{i}"),
            Item::AddThreshold => "monitoring_threshold_add".into(),
            Item::Save => "monitoring_save".into(),
            Item::Off => "monitoring_off".into(),
            Item::PreviousRead => "monitoring_previous_read".into(),
            Item::Refresh => "monitoring_refresh".into(),
            Item::Resume => "monitoring_resume".into(),
            Item::Feedback => "monitoring_feedback".into(),
        }
    }
    pub(super) fn label(
        &self,
        item: Item,
        view: &View,
        summary: &crate::provider_server::usage::UsageSummary,
        l: ui::UiLanguage,
    ) -> String {
        let tr = |en, zh, tw| super::super::harness_workspace_ui::text(l, en, zh, tw);
        match item{
            Item::Heading=>tr("Monitoring (configuration)","监测（配置）","監測（設定）").into(),
            Item::Summary=>super::super::harness_workspace_ui::usage_report::compact(summary,l),
            Item::OpenReport=>tr("Open usage report","打开用量报告","開啟用量報告").into(),
            Item::Account(i)=>view.account_monitoring.accounts.get(i).map_or_else(String::new,|a|format!("{}{}: {}",if self.selected.as_ref()==Some(&a.account){"> "}else{""},tr("Account","账号","帳號"),view.accounts.iter().find(|r|r.id==a.account).map_or(a.account.as_str(),|r|r.label.as_str()))),
            Item::State=>self.account(view).map_or_else(||tr("Metadata not loaded / selected account unavailable","元数据未加载／所选账号不可用","中繼資料未載入／所選帳號無法使用").into(),|a|format!("{}: {}; {}; {}; {}",tr("Reading / pause / attempt / storage","读数／暂停／尝试／存储","讀數／暫停／嘗試／儲存"),freshness_label(a.freshness,l),pause_label(a.paused,l),attempt_label(a.latest_attempt.as_ref().map(|r|r.result),l),persistence_label(view.account_monitoring.persistence,l))),
            Item::Resources(i)=>self.account(view).and_then(|a|a.last_success.as_ref()).map_or_else(String::new,|r|match &r.resources{
                monitor::Resources::DeepSeekWallets(wallets)=>wallets.get(i).map_or_else(String::new,|w|format!("{:?} {:?}: {}",w.kind,w.currency,w.balance.0)),
                monitor::Resources::OpenCodeGo(windows)=>windows.get(i).map_or_else(String::new,|w|format!("Go {:?}: {}%; {}: {}",w.window,w.used_percent.0,tr("Limited","有限额","有限額"),w.limited)),
            }),
            Item::Enabled=>format!("{}: {}",tr("Draft auto monitoring", "自动监测草稿", "自動監測草稿"),self.selected.as_ref().and_then(|id|self.forms.get(id)).is_some_and(|f|f.enabled)),
            Item::Threshold(i)|Item::ThresholdLabel(i)=>self.selected.as_ref().and_then(|id|self.forms.get(id)).and_then(|f|f.thresholds.get(i)).map_or_else(String::new,|t|format!("{} {}: {}",match t.resource{Resource::Wallet(k,c)=>format!("{k:?} {c:?}"),Resource::Go=>tr("Go used percent","Go用量百分比","Go用量百分比").into()},tr("threshold","阈值","門檻"),t.value.text)),
            Item::RemoveThreshold(_)=>tr("Remove", "移除", "移除").into(),
            Item::AddThreshold=>tr("Add a resource threshold","添加资源阈值","新增資源門檻").into(),
            Item::Save=>tr("Save monitoring policy","保存监测策略","儲存監測原則").into(),
            Item::Off=>tr("Turn monitoring off","关闭监测","關閉監測").into(),
            Item::PreviousRead=>self.selected.as_ref().and_then(|id|self.forms.get(id)).and_then(|f|f.refresh_terminal.as_ref()).map_or_else(String::new,|r|format!("{} #{}: {}",tr("Earlier one-off read","先前单次读取","先前單次讀取"),r.request,attempt_label(Some(r.result),l))),
            Item::Refresh=>tr("Read metadata once", "读取一次元数据", "讀取一次中繼資料").into(),
            Item::Resume=>tr("Resume opted-in monitoring","恢复已启用的监测","恢復已啟用的監測").into(),
            Item::Feedback => self.local_error.map(|error| error.label(l).to_owned()).unwrap_or_else(|| {
                let form = self.selected.as_ref().and_then(|id| self.forms.get(id));
                if form.is_some_and(|form| form.pending.as_ref().is_some_and(|pending|pending.request==form.latest_intent)) {
                    return tr("Waiting for this explicit action","正在等待此显式操作","正在等待此明確操作").into();
                }
                if let Some(unsaved) = form.and_then(|form| form.off_thresholds_unsaved) {
                    return if unsaved {
                        tr("Monitoring is off; threshold edits are not saved.","监测已关闭；阈值修改尚未保存。","監測已關閉；門檻修改尚未儲存。")
                    } else { tr("Monitoring is off.","监测已关闭。","監測已關閉。") }.into();
                }
                form.and_then(|form| form.feedback).map_or_else(||tr("Views use cached readings; manual read never resumes automatic monitoring.","界面只读缓存；手动读取不会恢复自动监测。","介面只讀快取；手動讀取不會恢復自動監測。").into(), |result|format!("{}: {}",tr("Latest explicit action","最近显式操作","最近明確操作"),attempt_label(Some(result),l)))
            }),
        }
    }
    pub(super) fn blur(&mut self) {
        for f in self.forms.values_mut() {
            for t in &mut f.thresholds {
                field_editor::apply(&mut t.value, Command::Blur, LIMITS);
            }
        }
        self.focused = None;
        self.drag = None;
        self.clicks = Default::default();
        self.navigation = None;
    }
    pub(super) fn focused(&self) -> bool {
        self.navigation.is_some()
    }
    pub(super) fn editing_item(&self) -> Option<Item> {
        self.focused.as_ref().map(|(_, i)| Item::Threshold(*i))
    }
    pub(super) fn drag_item(&self) -> Option<Item> {
        self.drag.and_then(|_| self.editing_item())
    }
    pub(super) fn end_drag(&mut self) {
        self.drag = None;
    }
    pub(super) fn press(
        &mut self,
        index: usize,
        position: &LogicalPoint,
        rect: ui::Rect,
        scale: f64,
        size: u16,
        shift: bool,
    ) -> bool {
        let Some(id) = self.selected.clone() else {
            return false;
        };
        if self.focused.as_ref() != Some(&(id.clone(), index)) {
            self.blur();
        }
        let Some(editor) = self
            .forms
            .get_mut(&id)
            .and_then(|f| f.thresholds.get_mut(index))
            .map(|t| &mut t.value)
        else {
            return false;
        };
        let offset =
            super::super::browser_ui::host_field_offset(editor, rect, position, scale, size);
        let clicks = self.clicks.register(offset, Instant::now());
        self.drag = field_editor::press(editor, offset, shift, clicks);
        self.focused = Some((id, index));
        self.navigation = Some(Item::Threshold(index));
        self.drag.is_some()
    }
    pub(super) fn drag_to(
        &mut self,
        position: &LogicalPoint,
        rect: ui::Rect,
        scale: f64,
        size: u16,
    ) {
        let Some((id, index)) = self.focused.clone() else {
            return;
        };
        let Some(anchor) = self.drag else {
            return;
        };
        if let Some(editor) = self
            .forms
            .get_mut(&id)
            .and_then(|f| f.thresholds.get_mut(index))
            .map(|t| &mut t.value)
        {
            let offset =
                super::super::browser_ui::host_field_offset(editor, rect, position, scale, size);
            field_editor::drag(editor, anchor, offset);
        }
    }
    pub(super) fn editing(&self) -> bool {
        self.focused.is_some()
    }
    pub(super) fn enter(&mut self) {
        self.navigation = Some(Item::OpenReport);
    }
    pub(super) fn current(&self) -> Option<Item> {
        self.navigation
    }
    fn interactive(item: Item) -> bool {
        matches!(
            item,
            Item::OpenReport
                | Item::Account(_)
                | Item::Enabled
                | Item::Threshold(_)
                | Item::AddThreshold
                | Item::Save
                | Item::Off
                | Item::Refresh
                | Item::Resume
        )
    }
    pub(super) fn navigate(&mut self, backwards: bool, view: &View) -> Option<Item> {
        let items: Vec<_> = self
            .rows(view)
            .into_iter()
            .filter(|item| Self::interactive(*item))
            .collect();
        let index = self
            .navigation
            .and_then(|a| items.iter().position(|b| *b == a));
        let next = match (index, backwards) {
            (None, false) => Some(0),
            (None, true) => items.len().checked_sub(1),
            (Some(i), true) => i.checked_sub(1),
            (Some(i), false) => Some(i + 1).filter(|i| *i < items.len()),
        };
        self.blur();
        self.navigation = next.and_then(|i| items.get(i).copied());
        self.navigation
    }
    pub(super) fn edit(&mut self, event: &PixelWindowEvent) -> bool {
        let Some((id, index)) = self.focused.clone() else {
            return false;
        };
        let Some(t) = self
            .forms
            .get_mut(&id)
            .and_then(|f| f.thresholds.get_mut(index))
        else {
            self.focused = None;
            return false;
        };
        if let Some(outcome) =
            super::super::browser_ui::edit_host_field(&mut t.value, event, LIMITS)
        {
            if outcome.rejected {
                self.local_error = Some(LocalError::TooLong);
            }
            if matches!(outcome.intent, Intent::Submit | Intent::Cancel) {
                self.focused = None;
            }
        }
        true
    }
    pub(super) fn activate(&mut self, item: Item, view: &View, request: Option<u64>) -> UiAction {
        self.local_error = None;
        self.navigation = Some(item);
        if item == Item::OpenReport {
            return UiAction::OpenReport;
        }
        if let Item::Account(index) = item {
            if let Some(a) = view.account_monitoring.accounts.get(index) {
                self.select(Some(&a.account));
            }
            return UiAction::None;
        }
        let Some(a) = self.account(view) else {
            return UiAction::None;
        };
        let id = a.account.clone();
        let provider = a.provider;
        let Some(form) = self.forms.get_mut(&id) else {
            return UiAction::None;
        };
        let revision = form.revision;
        match item {
            Item::Enabled => form.enabled = !form.enabled,
            Item::RemoveThreshold(index) => {
                if index < form.thresholds.len() {
                    form.thresholds.remove(index);
                    self.focused = None;
                }
            }
            Item::Threshold(index) => {
                self.focused = Some((id, index));
            }
            Item::AddThreshold => {
                let available = if provider == Provider::DeepSeek {
                    vec![
                        Resource::Wallet(monitor::WalletKind::Normal, monitor::Currency::Cny),
                        Resource::Wallet(monitor::WalletKind::Normal, monitor::Currency::Usd),
                        Resource::Wallet(monitor::WalletKind::Bonus, monitor::Currency::Cny),
                        Resource::Wallet(monitor::WalletKind::Bonus, monitor::Currency::Usd),
                    ]
                } else {
                    vec![Resource::Go]
                };
                if let Some(resource) = available
                    .into_iter()
                    .find(|r| !form.thresholds.iter().any(|t| t.resource == *r))
                {
                    let mut value = composer::ComposerState::default();
                    value.text = "0".into();
                    value.caret = 1;
                    form.thresholds.push(ThresholdDraft { resource, value });
                }
            }
            Item::Save | Item::Off | Item::Refresh | Item::Resume => {
                let refresh_pending = form
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.operation == monitor::MonitorOperation::Refresh);
                // Save(false) while a read is active, or while disabling the
                // applied enabled policy, is the same explicit Off intention.
                let off_only = item == Item::Off
                    || (item == Item::Save
                        && !form.enabled
                        && (refresh_pending || a.policy.enabled));
                let priority_off = off_only && refresh_pending && form.retired_refresh.is_none();
                if (form.pending.is_some() && !priority_off) || form.retired_refresh.is_some() {
                    self.local_error = Some(LocalError::Busy);
                    return UiAction::None;
                }
                let Some(request) = request else {
                    self.local_error = Some(LocalError::WorkerUnavailable);
                    return UiAction::None;
                };
                let (policy, expected_revision) = if off_only {
                    // Disable only. Dirty/invalid local threshold fields are
                    // neither submitted nor cleared. CAS still checks authority.
                    let mut applied = a.policy.clone();
                    applied.enabled = false;
                    (applied, a.policy_revision)
                } else {
                    (form.input(), revision)
                };
                if matches!(item, Item::Save | Item::Off) && !monitor::valid_policy(&policy) {
                    self.local_error = Some(LocalError::InvalidDecimal);
                    return UiAction::None;
                }
                let operation = if item == Item::Refresh {
                    monitor::MonitorOperation::Refresh
                } else {
                    monitor::MonitorOperation::Policy
                };
                if priority_off {
                    form.retired_refresh = form.pending.take();
                }
                if off_only {
                    form.enabled = false;
                }
                form.latest_intent = request;
                form.off_thresholds_unsaved = None;
                form.pending = Some(Pending {
                    request,
                    operation,
                    submitted: policy.clone(),
                    off_only,
                });
                let action = match item {
                    Item::Save | Item::Off => Action::SetAccountMonitoring {
                        request,
                        account: id,
                        expected_policy_revision: expected_revision,
                        policy,
                    },
                    Item::Resume => Action::ResumeAccountMonitoring {
                        request,
                        account: id,
                        expected_policy_revision: expected_revision,
                    },
                    _ => Action::RefreshAccountMetadata {
                        request,
                        account: id,
                    },
                };
                return UiAction::Send(action);
            }
            _ => {}
        }
        UiAction::None
    }
    pub(super) fn send_failed(&mut self, action: &Action, error: provider_runtime::RuntimeError) {
        let target = match action {
            Action::RefreshAccountMetadata { request, account }
            | Action::SetAccountMonitoring {
                request, account, ..
            }
            | Action::ResumeAccountMonitoring {
                request, account, ..
            } => Some((account, *request)),
            _ => None,
        };
        if let Some((account, request)) = target
            && let Some(form) = self.forms.get_mut(account)
            && form.pending.as_ref().is_some_and(|p| p.request == request)
        {
            let failed = form.pending.take().expect("matching pending request");
            if failed.off_only {
                form.pending = form.retired_refresh.take();
            }
            form.off_thresholds_unsaved = None;
            form.feedback = Some(Err(match error {
                provider_runtime::RuntimeError::Busy => monitor::MonitorError::Busy,
                provider_runtime::RuntimeError::WorkerUnavailable => {
                    monitor::MonitorError::WorkerUnavailable
                }
                provider_runtime::RuntimeError::Storage => monitor::MonitorError::Storage,
                provider_runtime::RuntimeError::Cancelled => monitor::MonitorError::Cancelled,
                _ => monitor::MonitorError::InvalidAction,
            }));
        }
    }
    pub(super) fn editor(&self, item: Item) -> Option<&composer::ComposerState> {
        let Item::Threshold(i) = item else {
            return None;
        };
        let id = self.selected.as_ref()?;
        Some(&self.forms.get(id)?.thresholds.get(i)?.value)
    }
    pub(super) fn editor_focused(&self, item: Item) -> bool {
        match item {
            Item::Threshold(i) | Item::ThresholdLabel(i) => self
                .focused
                .as_ref()
                .is_some_and(|(id, index)| self.selected.as_ref() == Some(id) && *index == i),
            _ => false,
        }
    }
    pub(super) fn snapshot(&self, view: &View) -> json::JsonValue {
        json::object(vec![
            (
                "selected_account",
                self.selected
                    .clone()
                    .map_or(json::JsonValue::Null, Into::into),
            ),
            (
                "storage",
                format!("{:?}", view.account_monitoring.persistence).into(),
            ),
            (
                "pending",
                self.selected
                    .as_ref()
                    .and_then(|id| self.forms.get(id))
                    .and_then(|f| f.pending.as_ref())
                    .map_or(json::JsonValue::Null, |p| p.request.into()),
            ),
        ])
    }
}
