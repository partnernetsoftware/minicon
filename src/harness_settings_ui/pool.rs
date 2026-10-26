//! Cached account presentation; Runtime retains provider and measurement authority.
use super::*;
use crate::harness_workspace_ui::usage_report::{attempt_label, freshness_label, utc_moment};
use crate::provider_monitoring as monitor;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum State {
    Stored,
    Tested,
    Disabled,
    Unknown,
}
impl State {
    pub(super) fn caption(self, l: ui::UiLanguage) -> &'static str {
        let (en, zh, tw) = match self {
            Self::Stored => ("Credentials stored", "凭据已保存", "憑據已儲存"),
            Self::Tested => ("Previously tested", "曾通过测试", "曾通過測試"),
            Self::Disabled => ("Disabled", "已停用", "已停用"),
            Self::Unknown => ("Policy unavailable", "账号策略不可用", "帳號原則無法使用"),
        };
        super::super::harness_workspace_ui::text(l, en, zh, tw)
    }
}
pub(super) fn state(view: &View, account: &AccountSummary) -> State {
    match view
        .account_preferences
        .iter()
        .find(|(id, _)| id == &account.id)
    {
        None => State::Unknown,
        Some((_, p)) if !p.enabled => State::Disabled,
        Some(_) if account.state == provider_accounts::AccountState::AccountReady => State::Tested,
        Some(_) => State::Stored,
    }
}
pub(super) fn measurement<'a>(
    view: &'a View,
    account: &AccountSummary,
) -> Option<&'a monitor::AccountMonitorView> {
    view.account_monitoring
        .accounts
        .iter()
        .find(|m| m.account == account.id && m.provider == account.provider)
}
#[derive(Default)]
pub(super) struct Summary {
    pub stored: usize,
    pub tested: usize,
    pub disabled: usize,
    pub unknown: usize,
    pub attention: usize,
    pub readings_unknown: usize,
}
pub(super) fn summary(view: &View) -> Option<Summary> {
    if view.load_state != provider_runtime::LoadState::Loaded {
        return None;
    }
    let mut result = Summary::default();
    for account in &view.accounts {
        match state(view, account) {
            State::Stored => result.stored += 1,
            State::Tested => result.tested += 1,
            State::Disabled => result.disabled += 1,
            State::Unknown => result.unknown += 1,
        }
        match measurement(view, account) {
            Some(m) => {
                result.attention += usize::from(m.attention);
                result.readings_unknown += usize::from(m.last_success.is_none());
            }
            None => result.readings_unknown += 1,
        }
    }
    Some(result)
}
pub(super) fn affected_clients(view: &View, account: &str) -> usize {
    view.connection_profiles
        .iter()
        .filter(|p| {
            p.applied.as_ref().is_some_and(|a| {
                a.allocations
                    .iter()
                    .any(|r| r.account.as_deref() == Some(account))
            })
        })
        .count()
}
pub(super) fn resource_label(
    resources: &monitor::Resources,
    index: usize,
    l: ui::UiLanguage,
) -> Option<String> {
    let tr = |en, zh, tw| super::super::harness_workspace_ui::text(l, en, zh, tw);
    match resources {
        monitor::Resources::DeepSeekWallets(rows) => rows.get(index).map(|w| {
            format!(
                "{} {}: {}",
                match w.kind {
                    monitor::WalletKind::Normal => tr("Wallet", "普通钱包", "一般錢包"),
                    monitor::WalletKind::Bonus => tr("Bonus", "赠送钱包", "贈送錢包"),
                },
                match w.currency {
                    monitor::Currency::Cny => "CNY",
                    monitor::Currency::Usd => "USD",
                },
                w.balance.0
            )
        }),
        monitor::Resources::OpenCodeGo(rows) => rows.get(index).map(|w| {
            format!(
                "Go {}: {}% / {}: {}",
                match w.window {
                    monitor::Window::Rolling => tr("Rolling used", "滚动用量", "滾動用量"),
                    monitor::Window::Weekly => tr("Weekly used", "周用量", "週用量"),
                    monitor::Window::Monthly => tr("Monthly used", "月用量", "月用量"),
                },
                w.used_percent.0,
                tr("Limited", "已限流", "已限流"),
                if w.limited {
                    tr("yes", "是", "是")
                } else {
                    tr("no", "否", "否")
                }
            )
        }),
    }
}
pub(super) fn details(view: &View, account: &AccountSummary, l: ui::UiLanguage) -> Vec<String> {
    let tr = |en, zh, tw| super::super::harness_workspace_ui::text(l, en, zh, tw);
    let mut rows = vec![state(view, account).caption(l).into()];
    if let Some((_, p)) = view
        .account_preferences
        .iter()
        .find(|(id, _)| id == &account.id)
    {
        let mode = |p: &NetworkPolicy| {
            if p.proxy().is_some() {
                tr("Proxy", "代理", "代理")
            } else {
                tr("Direct", "直连", "直連")
            }
        };
        rows.push(format!(
            "{}: {}",
            tr("Applied login network", "已应用登录网络", "已套用登入網路"),
            mode(&p.login)
        ));
        rows.push(format!(
            "{}: {}",
            tr(
                "Applied service network",
                "已应用服务网络",
                "已套用服務網路"
            ),
            mode(&p.service)
        ));
    }
    rows.push(format!(
        "{}: {}",
        tr(
            "Applied clients using this account",
            "使用此账号的已应用客户端",
            "使用此帳號的已套用用戶端"
        ),
        affected_clients(view, &account.id)
    ));
    let Some(m) = measurement(view, account) else {
        rows.push(
            tr(
                "Resource readings unavailable",
                "资源读数不可用",
                "資源讀數無法使用",
            )
            .into(),
        );
        return rows;
    };
    rows.push(format!(
        "{}: {}",
        tr("Resource reading", "资源读数", "資源讀數"),
        freshness_label(m.freshness, l)
    ));
    rows.push(format!(
        "{}: {}",
        tr("Latest metadata read", "最近元数据读取", "最近中繼資料讀取"),
        attempt_label(m.latest_attempt.as_ref().map(|a| a.result), l)
    ));
    if m.attention {
        rows.push(
            tr(
                "Resource reading needs attention",
                "资源读数需要关注",
                "資源讀數需要注意",
            )
            .into(),
        );
    }
    if let Some(reading) = &m.last_success {
        rows.push(format!(
            "{}: {}",
            tr("Source", "来源", "來源"),
            match reading.source {
                monitor::MetadataSource::DeepSeekSummaryV0 => tr(
                    "DeepSeek account wallets",
                    "DeepSeek 账号钱包",
                    "DeepSeek 帳號錢包"
                ),
                monitor::MetadataSource::OpenCodeGoV1 => tr(
                    "OpenCode Go allowance",
                    "OpenCode Go 额度",
                    "OpenCode Go 額度"
                ),
            }
        ));
        rows.push(format!(
            "{}: {}",
            tr("Observed", "观测时间", "觀測時間"),
            utc_moment(Some(reading.observed_unix_ms), l)
        ));
        let count = match &reading.resources {
            monitor::Resources::DeepSeekWallets(r) => r.len(),
            monitor::Resources::OpenCodeGo(r) => r.len(),
        };
        if count == 0 {
            rows.push(
                tr(
                    "Provider did not report resource values",
                    "提供方未报告资源数值",
                    "提供方未回報資源數值",
                )
                .into(),
            );
        }
        for i in 0..count {
            if let Some(label) = reading_resource_label(m, i, l) {
                rows.push(label);
            }
            if let monitor::Resources::OpenCodeGo(r) = &reading.resources {
                rows.push(format!(
                    "{}: {}",
                    tr("Provider reset", "提供方重置", "提供方重設"),
                    r[i].resets_at
                ));
            }
        }
    } else {
        rows.push(
            tr(
                "No successful reading; unknown is not zero",
                "尚无成功读数，未知不等于零",
                "尚無成功讀數，未知不等於零",
            )
            .into(),
        );
    }
    rows
}

pub(super) fn reading_resource_label(
    account: &monitor::AccountMonitorView,
    index: usize,
    l: ui::UiLanguage,
) -> Option<String> {
    let reading = account.last_success.as_ref()?;
    resource_label(&reading.resources, index, l)
        .map(|label| format!("{} · {label}", freshness_label(account.freshness, l)))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn observation(account: &str, provider: Provider) -> monitor::AccountMonitorView {
        monitor::AccountMonitorView {
            account: account.into(),
            provider,
            policy_revision: 0,
            policy: Default::default(),
            paused: Some(monitor::PauseReason::Off),
            active_request: None,
            next_due_unix_ms: None,
            last_success: None,
            freshness: monitor::Freshness::Never,
            latest_attempt: None,
            attention: false,
            wallet_capability: monitor::Capability::Unknown,
            allowance_capability: monitor::Capability::Unknown,
        }
    }
    #[test]
    fn cached_pool_keeps_disabled_and_old_readings_separate_from_inference() {
        let mut view = View {
            load_state: provider_runtime::LoadState::Loaded,
            ..Default::default()
        };
        for (id, provider, state) in [
            (
                "a",
                Provider::DeepSeek,
                provider_accounts::AccountState::AccountReady,
            ),
            (
                "b",
                Provider::DeepSeek,
                provider_accounts::AccountState::AccountReady,
            ),
            (
                "c",
                Provider::OpenCode,
                provider_accounts::AccountState::CredentialStored,
            ),
        ] {
            view.accounts.push(AccountSummary {
                id: id.into(),
                provider,
                label: id.into(),
                state,
            });
        }
        view.account_preferences
            .push(("a".into(), AccountPreferences::legacy("a")));
        let mut disabled = AccountPreferences::legacy("b");
        disabled.enabled = false;
        view.account_preferences.push(("b".into(), disabled));
        let mut stale = observation("a", Provider::DeepSeek);
        stale.freshness = monitor::Freshness::Stale;
        stale.attention = true;
        stale.latest_attempt = Some(monitor::Attempt {
            request: 1,
            finished_unix_ms: Some(1),
            result: Err(monitor::MonitorError::Forbidden),
        });
        stale.last_success = Some(monitor::Reading {
            source: monitor::MetadataSource::DeepSeekSummaryV0,
            observed_unix_ms: 1,
            provider_as_of: None,
            resources: monitor::Resources::DeepSeekWallets(vec![
                monitor::Wallet {
                    kind: monitor::WalletKind::Normal,
                    currency: monitor::Currency::Cny,
                    balance: monitor::Decimal("0.000000000000000001".into()),
                },
                monitor::Wallet {
                    kind: monitor::WalletKind::Bonus,
                    currency: monitor::Currency::Usd,
                    balance: monitor::Decimal("0.2500".into()),
                },
            ]),
        });
        std::sync::Arc::make_mut(&mut view.account_monitoring).accounts =
            vec![stale, observation("b", Provider::OpenCode)];
        let s = summary(&view).unwrap();
        assert_eq!((s.tested, s.disabled, s.unknown), (1, 1, 1));
        assert_eq!((s.attention, s.readings_unknown), (1, 2));
        assert!(
            measurement(&view, &view.accounts[1]).is_none(),
            "foreign-provider observation cannot attach by ID alone"
        );
        let before = view.clone();
        let lines = details(&view, &view.accounts[0], ui::UiLanguage::English);
        assert!(lines.iter().any(|s| s.contains("Provider denied access")));
        assert!(
            lines
                .iter()
                .any(|s| s.contains("Stale") && s.contains("CNY: 0.000000000000000001"))
        );
        assert!(
            lines
                .iter()
                .any(|s| s.contains("Stale") && s.contains("USD: 0.2500"))
        );
        assert_eq!(view, before);
        assert_eq!(
            state(&view, &view.accounts[0]),
            State::Tested,
            "metadata failure cannot revoke inference evidence"
        );
        view.account_preferences
            .push(("c".into(), AccountPreferences::legacy("c")));
        let mut go = observation("c", Provider::OpenCode);
        go.freshness = monitor::Freshness::Fresh;
        go.last_success = Some(monitor::Reading {
            source: monitor::MetadataSource::OpenCodeGoV1,
            observed_unix_ms: 1,
            provider_as_of: None,
            resources: monitor::Resources::OpenCodeGo(vec![monitor::Allowance {
                window: monitor::Window::Rolling,
                limited: true,
                used_percent: monitor::Decimal("125.50".into()),
                resets_at: "provider-reset".into(),
            }]),
        });
        std::sync::Arc::make_mut(&mut view.account_monitoring)
            .accounts
            .push(go);
        let lines = details(&view, &view.accounts[2], ui::UiLanguage::English);
        assert!(
            lines
                .iter()
                .any(|s| s.contains("125.50%") && s.contains("Limited: yes"))
        );
        assert!(
            lines
                .iter()
                .any(|s| s.contains("Provider reset: provider-reset"))
        );
        assert_eq!(
            state(&view, &view.accounts[2]),
            State::Stored,
            "fresh metadata is not a model check"
        );
        assert_eq!(summary(&view).unwrap().readings_unknown, 1);
        view.load_state = provider_runtime::LoadState::Loading;
        assert!(
            summary(&view).is_none(),
            "a loading pool must not publish zero counts"
        );
    }
}
