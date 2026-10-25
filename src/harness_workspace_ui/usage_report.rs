//! Read-only report projection; source semantics remain with the accounting owner.
use super::*;
use crate::provider_monitoring as monitoring;
use crate::provider_server::usage::{
    BillingTotals, DecimalTotal, GroupKey, GroupSummary, Metrics, UsageState, UsageSummary,
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Period {
    #[default]
    Today,
    Days7,
    Days30,
}
impl Period {
    fn index(self) -> usize {
        match self {
            Self::Today => 0,
            Self::Days7 => 1,
            Self::Days30 => 2,
        }
    }
    fn next(self) -> Self {
        match self {
            Self::Today => Self::Days7,
            Self::Days7 => Self::Days30,
            Self::Days30 => Self::Today,
        }
    }
    fn label(self, l: ui::UiLanguage) -> &'static str {
        match self {
            Self::Today => text(l, "Today (UTC)", "今天（UTC）", "今天（UTC）"),
            Self::Days7 => text(l, "Last 7 UTC days", "最近7个UTC日", "最近7個UTC日"),
            Self::Days30 => text(l, "Last 30 UTC days", "最近30个UTC日", "最近30個UTC日"),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Traffic {
    #[default]
    Clients,
    Probes,
}
impl Traffic {
    fn label(self, l: ui::UiLanguage) -> &'static str {
        match self {
            Self::Clients => text(l, "Client requests", "客户端请求", "用戶端請求"),
            Self::Probes => text(l, "Explicit model checks", "显式模型检查", "明確模型檢查"),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Dimension {
    #[default]
    Account,
    Profile,
    Model,
}
impl Dimension {
    fn index(self) -> usize {
        match self {
            Self::Account => 0,
            Self::Profile => 1,
            Self::Model => 2,
        }
    }
    fn next(self) -> Self {
        match self {
            Self::Account => Self::Profile,
            Self::Profile => Self::Model,
            Self::Model => Self::Account,
        }
    }
    fn label(self, l: ui::UiLanguage) -> &'static str {
        match self {
            Self::Account => text(l, "Account", "账号", "帳號"),
            Self::Profile => text(l, "Client profile", "客户端连接", "用戶端連線"),
            Self::Model => text(
                l,
                "Model / provider / protocol",
                "模型／提供方／协议",
                "模型／提供方／協定",
            ),
        }
    }
    fn groups(self, summary: &UsageSummary) -> &[GroupSummary] {
        match self {
            Self::Account => &summary.accounts,
            Self::Profile => &summary.profiles,
            Self::Model => &summary.models,
        }
    }
}
#[derive(Clone, Default, Eq, PartialEq)]
enum Choice {
    #[default]
    All,
    Named(GroupKey),
    Other,
}
#[derive(Clone, Eq, PartialEq)]
enum Action {
    Period,
    Traffic,
    Dimension,
    Groups,
    Choose(Choice),
    Monitoring,
    MetadataAccount,
}
#[derive(Default)]
pub(super) struct Report {
    period: Period,
    traffic: Traffic,
    dimension: Dimension,
    choices: [Choice; 3],
    picker: bool,
    focus: Option<Action>,
    scroll: usize,
    metadata_account: Option<String>,
    displayed: Option<Arc<UsageSummary>>,
    pending: Option<Arc<UsageSummary>>,
    next_metric: Option<Instant>,
    due: Option<Instant>,
}
pub(super) enum Intent {
    None,
    Monitoring(Option<String>),
    Leave(bool),
}
struct Row {
    id: String,
    label: String,
    action: Option<Action>,
}
struct Entry {
    id: String,
    label: String,
    action: Option<Action>,
    start: usize,
    lines: Vec<String>,
}
pub(super) struct Projection {
    entries: Vec<Entry>,
    body: ui::Rect,
    line_height: u32,
    font_size: u16,
    total: usize,
    visible: usize,
    start: usize,
}
impl Projection {
    fn rect(&self, entry: &Entry) -> ui::Rect {
        let end = entry.start.saturating_add(entry.lines.len());
        let top = entry.start.max(self.start);
        let bottom = end.min(self.start.saturating_add(self.visible));
        if bottom <= top {
            return ui::Rect::default();
        }
        ui::Rect {
            x: self.body.x,
            y: self
                .body
                .y
                .saturating_add(((top - self.start) as u32).saturating_mul(self.line_height)),
            width: self.body.width,
            height: ((bottom - top) as u32)
                .saturating_mul(self.line_height)
                .min(self.body.height),
        }
    }
    fn complete(&self, e: &Entry) -> bool {
        !e.lines.is_empty()
            && self.visible > 0
            && e.lines.iter().all(|line| {
                host_ui::host_ui_text_width(line, self.font_size)
                    <= self.body.width.saturating_sub(12)
            })
            && e.start >= self.start
            && e.start.saturating_add(e.lines.len()) <= self.start.saturating_add(self.visible)
    }
    fn max_start(&self) -> usize {
        self.total.saturating_sub(self.visible)
    }
}

impl Report {
    pub(super) fn presented<'a>(&'a self, view: Option<&'a View>) -> Option<&'a UsageSummary> {
        self.displayed
            .as_deref()
            .or_else(|| view.map(|v| v.usage.as_ref()))
    }
    pub(super) fn observe(&mut self, next: Arc<UsageSummary>, visible: bool, now: Instant) -> bool {
        if self.displayed.as_ref().is_some_and(|old| **old == *next) && self.pending.is_none() {
            return false;
        }
        let urgent = self.displayed.as_ref().is_none_or(|old| {
            old.state != next.state
                || old.gaps != next.gaps
                || old.today_utc_day != next.today_utc_day
        });
        if !visible {
            self.pending = Some(next);
            self.due = None;
            return false;
        }
        if urgent || self.next_metric.is_none_or(|due| now >= due) {
            self.displayed = Some(next);
            self.pending = None;
            self.due = None;
            self.next_metric = Some(now + Duration::from_secs(1));
            true
        } else {
            self.pending = Some(next);
            self.due = self.next_metric;
            false
        }
    }
    pub(super) fn reveal_latest(&mut self, next: Option<Arc<UsageSummary>>, now: Instant) {
        self.displayed = next.or_else(|| self.pending.take());
        self.due = None;
        self.next_metric = Some(now + Duration::from_secs(1));
    }
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.due
    }
    pub(super) fn service(&mut self, now: Instant, visible: bool) -> bool {
        if !visible {
            self.due = None;
            return false;
        }
        if self.due.is_some_and(|due| now >= due)
            && let Some(next) = self.pending.take()
        {
            self.displayed = Some(next);
            self.due = None;
            self.next_metric = Some(now + Duration::from_secs(1));
            return true;
        }
        false
    }
    fn choice(&self) -> &Choice {
        &self.choices[self.dimension.index()]
    }
    fn metrics<'a>(&self, s: &'a UsageSummary) -> Option<(&'a Metrics, bool)> {
        let period = self.period.index();
        let (totals, incomplete) = match self.choice() {
            Choice::All => (&s.windows[period], false),
            choice => {
                let key = match choice {
                    Choice::Named(key) => Some(key),
                    _ => None,
                };
                let group = self
                    .dimension
                    .groups(s)
                    .iter()
                    .find(|g| g.key.as_ref() == key)?;
                (&group.windows[period], group.incomplete)
            }
        };
        Some((
            match self.traffic {
                Traffic::Clients => &totals.client,
                Traffic::Probes => &totals.probe,
            },
            incomplete,
        ))
    }
    fn account<'a>(&'a self, view: &'a View) -> Option<&'a monitoring::AccountMonitorView> {
        let id = self
            .metadata_account
            .as_ref()
            .or_else(|| match self.choice() {
                Choice::Named(GroupKey::Account(id)) if self.dimension == Dimension::Account => {
                    Some(id)
                }
                _ => None,
            });
        if let Some(id) = id {
            return view
                .account_monitoring
                .accounts
                .iter()
                .find(|a| &a.account == id);
        }
        view.account_monitoring.accounts.first()
    }
    fn rows(&self, view: Option<&View>, l: ui::UiLanguage) -> Vec<Row> {
        let mut rows = vec![
            button(
                "period",
                format!(
                    "{}: {}",
                    text(l, "Period", "时段", "時段"),
                    self.period.label(l)
                ),
                Action::Period,
            ),
            button(
                "purpose",
                format!(
                    "{}: {}",
                    text(l, "Traffic", "请求用途", "請求用途"),
                    self.traffic.label(l)
                ),
                Action::Traffic,
            ),
            button(
                "dimension",
                format!(
                    "{}: {}",
                    text(
                        l,
                        "Group by (one dimension)",
                        "分组（单一维度）",
                        "分組（單一維度）"
                    ),
                    self.dimension.label(l)
                ),
                Action::Dimension,
            ),
            button(
                "group",
                text(l, "Choose group", "选择分组", "選擇分組").into(),
                Action::Groups,
            ),
            line(
                "current_group",
                format!(
                    "{}: {}",
                    text(l, "Current group", "当前分组", "目前分組"),
                    choice_label(self.choice(), view, l)
                ),
            ),
            button(
                "monitoring",
                text(l, "Monitoring settings", "监测设置", "監測設定").into(),
                Action::Monitoring,
            ),
        ];
        let Some(view) = view else {
            rows.push(line(
                "source",
                text(
                    l,
                    "Not loaded. Open Monitoring settings to load local state.",
                    "尚未加载；打开监测设置以加载本地状态。",
                    "尚未載入；開啟監測設定以載入本機狀態。",
                )
                .into(),
            ));
            return rows;
        };
        let s = self.presented(Some(view)).unwrap_or(view.usage.as_ref());
        if self.picker {
            rows.push(button(
                "all",
                choice_label(&Choice::All, Some(view), l),
                Action::Choose(Choice::All),
            ));
            for (i, g) in self.dimension.groups(s).iter().enumerate() {
                let choice = g.key.clone().map_or(Choice::Other, Choice::Named);
                let label = choice_label(&choice, Some(view), l);
                // Keep the reversible choice control usable even when a long ID
                // needs more lines than the viewport; its full text follows.
                let short: String = label.chars().take(16).collect();
                rows.push(button(
                    &format!("group_{i}"),
                    format!("{} {}: {short}", text(l, "Select", "选择", "選擇"), i + 1),
                    Action::Choose(choice),
                ));
                rows.push(line(&format!("group_{i}_identity"), label));
            }
            return rows;
        }
        rows.push(line(
            "grouping_help",
            text(
                l,
                "Groups are independent. No combined account + client + model filter.",
                "各分组独立，不是账号＋客户端＋模型联合筛选。",
                "各分組獨立，並非帳號＋用戶端＋模型聯合篩選。",
            )
            .into(),
        ));
        rows.push(line(
            "source",
            format!(
                "{}: {}",
                text(l, "Local history", "本地历史", "本機歷史"),
                source_label(s.state, l)
            ),
        ));
        rows.push(line(
            "computed",
            format!(
                "{}: {}",
                text(
                    l,
                    "Local totals updated",
                    "本地汇总更新于",
                    "本機彙總更新於"
                ),
                utc_moment(s.computed_at_unix_ms, l)
            ),
        ));
        if s.computed_at_unix_ms.is_none() || s.today_utc_day.is_none() {
            rows.push(line("clock_unknown",text(l,"UTC window placement unavailable; these are retained observations, not current Today totals.","UTC时段不可确定；这是保留观测，不能当作今天的精确总量。","UTC時段無法確定；這是保留觀測，不能當作今天的精確總量。").into()));
        } else if matches!(s.state, UsageState::Opening) {
            rows.push(line(
                "loading",
                text(
                    l,
                    "Loading retained measurements; no zero totals inferred.",
                    "正在加载保留观测，不推定为零。",
                    "正在載入保留觀測，不推定為零。",
                )
                .into(),
            ));
        } else if let Some((m, incomplete)) = self.metrics(s) {
            if matches!(s.state, UsageState::Unavailable) {
                rows.push(line(
                    "unsaved",
                    text(
                        l,
                        "Known observations only; retained history/save is unavailable.",
                        "仅显示已知观测；保留历史或保存不可用。",
                        "僅顯示已知觀測；保留歷史或儲存不可用。",
                    )
                    .into(),
                ));
            }
            if incomplete {
                rows.push(line(
                    "group_incomplete",
                    text(
                        l,
                        "Some groups are included in Other.",
                        "部分分组归入“其他”。",
                        "部分分組歸入「其他」。",
                    )
                    .into(),
                ));
            }
            if matches!(s.state, UsageState::Unavailable) && m.requests == 0 {
                rows.push(line(
                    "history_unknown",
                    text(
                        l,
                        "Retained totals unknown; no known observations to display.",
                        "保留总量未知；没有可显示的已知观测。",
                        "保留總量未知；沒有可顯示的已知觀測。",
                    )
                    .into(),
                ));
            } else {
                metrics_rows(&mut rows, m, l);
            }
        } else {
            rows.push(line(
                "group_missing",
                text(
                    l,
                    "Selected retained group is unavailable. Choose another group explicitly.",
                    "所选历史分组不可用，请显式选择其他分组。",
                    "所選歷史分組不可用，請明確選擇其他分組。",
                )
                .into(),
            ));
        }
        rows.push(line(
            "detail_span",
            format!(
                "{}: {} .. {}",
                text(
                    l,
                    "Recent detail span (not aggregate span)",
                    "近期明细范围（不是汇总范围）",
                    "近期明細範圍（不是彙總範圍）"
                ),
                optional(s.retained_from_ms, l),
                optional(s.retained_to_ms, l)
            ),
        ));
        rows.push(line(
            "global_live",
            format!(
                "{}: {} / {} / {} / {}",
                text(
                    l,
                    "This process, unfiltered: active / admitted / terminal / rejected",
                    "本进程未筛选：活跃／准入／结束／拒绝",
                    "本程序未篩選：作用中／准入／結束／拒絕"
                ),
                s.active_requests,
                s.run_admitted,
                s.run_terminals,
                s.run_rejected
            ),
        ));
        let gaps = &s.gaps;
        for (id, label, value) in [
            (
                "queue_drop",
                text(l, "Queue drops", "队列丢失", "佇列遺失"),
                gaps.queue_dropped,
            ),
            (
                "oversized",
                text(l, "Oversized receipts", "过大记录", "過大記錄"),
                gaps.oversized,
            ),
            (
                "write_failure",
                text(l, "History write failures", "历史写入失败", "歷史寫入失敗"),
                gaps.write_failures,
            ),
            (
                "time_missing",
                text(
                    l,
                    "Unavailable request timestamps",
                    "请求时间戳不可用",
                    "請求時間戳無法使用",
                ),
                gaps.time_unavailable,
            ),
            (
                "detail_capped_records",
                text(
                    l,
                    "Evicted recent detail records",
                    "已淘汰近期明细记录",
                    "已淘汰近期明細記錄",
                ),
                gaps.detail_capped,
            ),
            (
                "interrupted",
                text(l, "Interrupted runs", "中断运行", "中斷執行"),
                gaps.interrupted_runs,
            ),
        ] {
            rows.push(line(id, format!("{label}: {value}")));
        }
        for (id, label, value) in [
            (
                "invalid_history",
                text(
                    l,
                    "Invalid retained history",
                    "保留历史损坏",
                    "保留歷史損毀",
                ),
                gaps.invalid_history,
            ),
            (
                "detail_cap",
                text(
                    l,
                    "Recent detail capped; aggregates retained independently",
                    "近期明细截断，汇总独立保留",
                    "近期明細截斷，彙總獨立保留",
                ),
                gaps.detail_capped > 0,
            ),
            (
                "group_cap",
                text(
                    l,
                    "Grouped detail capped / Other",
                    "分组明细截断／其他",
                    "分組明細截斷／其他",
                ),
                gaps.grouped_detail_capped,
            ),
            (
                "counter_overflow",
                text(l, "Counter overflow", "计数溢出", "計數溢位"),
                gaps.counter_overflow,
            ),
            (
                "shutdown",
                text(
                    l,
                    "Previous orderly close unconfirmed",
                    "上次有序关闭未确认",
                    "上次有序關閉未確認",
                ),
                gaps.shutdown_unconfirmed,
            ),
        ] {
            if value {
                rows.push(line(id, label.into()));
            }
        }
        rows.push(button(
            "metadata_account",
            format!(
                "{}: {}",
                text(
                    l,
                    "Official resource source account",
                    "官方资源来源账号",
                    "官方資源來源帳號"
                ),
                self.account(view).map_or_else(
                    || text(l, "Unavailable", "不可用", "無法使用").into(),
                    |a| a.account.clone()
                )
            ),
            Action::MetadataAccount,
        ));
        rows.extend(metadata_rows(
            self.account(view),
            view.account_monitoring.persistence,
            l,
        ));
        rows
    }
    pub(super) fn project(
        &self,
        view: Option<&View>,
        l: ui::UiLanguage,
        body: ui::Rect,
        size: u16,
    ) -> Projection {
        let line_height = font::cell_metrics(size).height.max(1).saturating_add(3);
        let mut total = 0usize;
        let entries = self
            .rows(view, l)
            .into_iter()
            .map(|r| {
                let lines =
                    host_ui::host_ui_paragraph_lines(&r.label, body.width.saturating_sub(12), size);
                let entry = Entry {
                    id: r.id,
                    label: r.label,
                    action: r.action,
                    start: total,
                    lines,
                };
                total = total.saturating_add(entry.lines.len());
                entry
            })
            .collect();
        let visible = (body.height / line_height) as usize;
        Projection {
            entries,
            body,
            line_height,
            font_size: size,
            total,
            visible,
            start: self.scroll.min(total.saturating_sub(visible)),
        }
    }
    fn activate(&mut self, action: Action, view: Option<&View>) -> Intent {
        match action {
            Action::Period => self.period = self.period.next(),
            Action::Traffic => {
                self.traffic = match self.traffic {
                    Traffic::Clients => Traffic::Probes,
                    Traffic::Probes => Traffic::Clients,
                }
            }
            Action::Dimension => {
                self.dimension = self.dimension.next();
                self.picker = false;
            }
            Action::Groups => self.picker = !self.picker,
            Action::Choose(choice) => {
                self.choices[self.dimension.index()] = choice;
                self.picker = false;
                self.focus = Some(Action::Groups);
            }
            Action::Monitoring => {
                return Intent::Monitoring(
                    view.and_then(|v| self.account(v))
                        .map(|a| a.account.clone()),
                );
            }
            Action::MetadataAccount => {
                if let Some(view) = view {
                    let entries = &view.account_monitoring.accounts;
                    if !entries.is_empty() {
                        let next = self
                            .account(view)
                            .and_then(|a| entries.iter().position(|r| r.account == a.account))
                            .map_or(0, |i| (i + 1) % entries.len());
                        self.metadata_account = Some(entries[next].account.clone());
                    }
                }
            }
        }
        self.scroll = 0;
        Intent::None
    }
    pub(super) fn route(
        &mut self,
        event: &PixelWindowEvent,
        view: Option<&View>,
        p: &Projection,
        scale: f64,
    ) -> Option<Intent> {
        let actions: Vec<_> = p.entries.iter().filter_map(|e| e.action.clone()).collect();
        let position = self
            .focus
            .as_ref()
            .and_then(|f| actions.iter().position(|a| a == f));
        match event {
            PixelWindowEvent::PointerButton {
                state: PointerButtonState::Pressed,
                button: PointerButton::Left,
                position: Some(pos),
                ..
            } => {
                let (x, y) = (
                    (pos.x * scale).max(0.0) as u32,
                    (pos.y * scale).max(0.0) as u32,
                );
                let e = p.entries.iter().find(|e| p.rect(e).contains(x, y))?;
                if let Some(action) = &e.action {
                    self.focus = Some(action.clone());
                    if p.complete(e) {
                        return Some(self.activate(action.clone(), view));
                    }
                }
                Some(Intent::None)
            }
            PixelWindowEvent::Keyboard(key)
                if key.state == KeyPressState::Pressed
                    && !key.modifiers.control
                    && !key.modifiers.alt
                    && !key.modifiers.meta =>
            {
                match &key.logical {
                    LogicalKey::Named(NamedKey::Tab) => {
                        let next = match position {
                            None => {
                                if key.modifiers.shift {
                                    actions.len().checked_sub(1)
                                } else {
                                    Some(0)
                                }
                            }
                            Some(i) => {
                                if key.modifiers.shift {
                                    i.checked_sub(1)
                                } else {
                                    Some(i + 1).filter(|n| *n < actions.len())
                                }
                            }
                        };
                        if let Some(i) = next {
                            self.focus = actions.get(i).cloned();
                            self.reveal(p);
                        } else {
                            self.focus = None;
                            return Some(Intent::Leave(key.modifiers.shift));
                        }
                    }
                    LogicalKey::Named(NamedKey::ArrowDown | NamedKey::ArrowUp)
                        if self.focus.is_some() =>
                    {
                        let i = position.unwrap_or(0);
                        let n = if key.logical == LogicalKey::Named(NamedKey::ArrowUp) {
                            i.saturating_sub(1)
                        } else {
                            (i + 1).min(actions.len().saturating_sub(1))
                        };
                        self.focus = actions.get(n).cloned();
                        self.reveal(p);
                    }
                    LogicalKey::Named(NamedKey::PageDown) => {
                        self.scroll = p.start.saturating_add(p.visible.max(1)).min(p.max_start())
                    }
                    LogicalKey::Named(NamedKey::PageUp) => {
                        self.scroll = p.start.saturating_sub(p.visible.max(1))
                    }
                    LogicalKey::Named(NamedKey::Home) => self.scroll = 0,
                    LogicalKey::Named(NamedKey::End) => self.scroll = p.max_start(),
                    LogicalKey::Named(NamedKey::Enter | NamedKey::Space)
                        if self.focus.is_some() =>
                    {
                        let action = self.focus.clone()?;
                        if p.entries
                            .iter()
                            .any(|e| e.action.as_ref() == Some(&action) && p.complete(e))
                        {
                            return Some(self.activate(action, view));
                        }
                        self.reveal(p);
                    }
                    LogicalKey::Character(value) if value == " " && self.focus.is_some() => {
                        let action = self.focus.clone()?;
                        if p.entries
                            .iter()
                            .any(|e| e.action.as_ref() == Some(&action) && p.complete(e))
                        {
                            return Some(self.activate(action, view));
                        }
                        self.reveal(p);
                    }
                    LogicalKey::Named(NamedKey::Escape) if self.picker => {
                        self.picker = false;
                        self.focus = Some(Action::Groups);
                        self.scroll = 0;
                    }
                    _ => return None,
                }
                Some(Intent::None)
            }
            PixelWindowEvent::MouseWheel { delta, .. } => {
                let direction = match delta {
                    WheelDelta::Lines { y, .. } => f64::from(*y),
                    WheelDelta::LogicalPixels { y, .. } => *y,
                    _ => 0.0,
                };
                if direction < 0.0 {
                    self.scroll = p.start.saturating_add(3).min(p.max_start());
                } else if direction > 0.0 {
                    self.scroll = p.start.saturating_sub(3);
                }
                Some(Intent::None)
            }
            _ => None,
        }
    }
    fn reveal(&mut self, p: &Projection) {
        if let Some(e) = p
            .entries
            .iter()
            .find(|e| e.action.as_ref() == self.focus.as_ref())
        {
            if e.start < p.start {
                self.scroll = e.start;
            } else if e.start.saturating_add(e.lines.len()) > p.start.saturating_add(p.visible) {
                self.scroll = e
                    .start
                    .saturating_add(e.lines.len())
                    .saturating_sub(p.visible);
            }
        }
    }
    pub(super) fn restore_focus(&mut self) {
        // A Settings round trip retains the report position and prior focus.
        // First entry can focus Period; a scrolled view may retain header focus.
        if self.focus.is_none() && self.scroll == 0 {
            self.focus = Some(Action::Period);
        }
    }
    pub(super) fn focused(&self) -> bool {
        self.focus.is_some()
    }
    pub(super) fn leave_focus(&mut self) {
        self.focus = None;
    }
    pub(super) fn paint(
        &self,
        surface: &mut Surface<'_>,
        p: &Projection,
        size: u16,
        t: theme::Theme,
    ) {
        for e in &p.entries {
            let rect = p.rect(e);
            if rect.height == 0 {
                continue;
            }
            if e.action.is_some() {
                surface.fill_rect(
                    rect.x,
                    rect.y,
                    rect.width,
                    rect.height,
                    if e.action == self.focus {
                        t.surface
                    } else {
                        t.sidebar_bg
                    }
                    .to_xrgb(),
                );
            }
            for (i, line) in e.lines.iter().enumerate() {
                let line_index = e.start + i;
                if line_index < p.start || line_index >= p.start.saturating_add(p.visible) {
                    continue;
                }
                host_ui::paint_host_ui_text(
                    surface,
                    rect.x.saturating_add(6),
                    p.body.y.saturating_add(
                        ((line_index - p.start) as u32).saturating_mul(p.line_height),
                    ),
                    line,
                    if e.action.is_some() { t.accent } else { t.text },
                    size,
                    rect.width.saturating_sub(12),
                );
            }
        }
    }
    pub(super) fn snapshot(
        &self,
        p: &Projection,
        view: Option<&View>,
        l: ui::UiLanguage,
    ) -> json::JsonValue {
        let s = self.presented(view);
        json::object(vec![
            (
                "period",
                ["today", "days7", "days30"][self.period.index()].into(),
            ),
            ("timezone_basis", "utc".into()),
            (
                "traffic",
                if self.traffic == Traffic::Clients {
                    "client"
                } else {
                    "probe"
                }
                .into(),
            ),
            (
                "group_by",
                ["account", "client_profile", "model"][self.dimension.index()].into(),
            ),
            ("selected", choice_json(self.choice())),
            ("picker", self.picker.into()),
            ("scroll", p.start.into()),
            ("total_lines", p.total.into()),
            ("visible_lines", p.visible.into()),
            (
                "revision",
                s.map_or(json::JsonValue::Null, |s| s.revision.into()),
            ),
            (
                "source",
                s.map_or("not_loaded", |s| source_tag(s.state)).into(),
            ),
            (
                "computed_at_unix_ms",
                s.and_then(|s| s.computed_at_unix_ms)
                    .map_or(json::JsonValue::Null, Into::into),
            ),
            (
                "today_utc_day",
                s.and_then(|s| s.today_utc_day)
                    .map_or(json::JsonValue::Null, Into::into),
            ),
            (
                "group_incomplete",
                s.and_then(|s| self.metrics(s))
                    .is_some_and(|(_, i)| i)
                    .into(),
            ),
            (
                "selected_label",
                choice_label(self.choice(), view, l).into(),
            ),
            (
                "rows",
                json::JsonValue::Array(
                    p.entries
                        .iter()
                        .filter(|e| p.rect(e).height > 0)
                        .map(|e| {
                            json::object(vec![
                                ("id", e.id.clone().into()),
                                ("text", e.label.clone().into()),
                                ("action", e.action.is_some().into()),
                                ("complete", p.complete(e).into()),
                                ("rect", rect_json(p.rect(e))),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }
}
fn button(id: &str, label: String, action: Action) -> Row {
    Row {
        id: id.into(),
        label,
        action: Some(action),
    }
}
fn line(id: &str, label: String) -> Row {
    Row {
        id: id.into(),
        label,
        action: None,
    }
}
fn optional(n: Option<u64>, l: ui::UiLanguage) -> String {
    n.map_or_else(
        || text(l, "Unavailable", "不可用", "無法使用").into(),
        |n| n.to_string(),
    )
}
fn utc_moment(unix_ms: Option<u64>, l: ui::UiLanguage) -> String {
    let civil = unix_ms
        .and_then(|value| i64::try_from(value / 1000).ok())
        .map(minicon_platform::contract::local_clock::civil_from_unix_seconds);
    match civil {
        Some(c) if (1970..=9999).contains(&c.year) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
            c.year, c.month, c.day, c.hour, c.minute, c.second
        ),
        _ => text(l, "Unavailable", "不可用", "無法使用").into(),
    }
}
fn source_tag(state: UsageState) -> &'static str {
    match state {
        UsageState::Opening => "opening",
        UsageState::Ready => "ready",
        UsageState::DurabilityUnconfirmed => "durability_unconfirmed",
        UsageState::Unavailable => "unavailable",
        UsageState::Closing => "closing",
        UsageState::Closed => "closed",
    }
}
fn source_label(state: UsageState, l: ui::UiLanguage) -> &'static str {
    match state {
        UsageState::Opening => text(l, "Loading", "加载中", "載入中"),
        UsageState::Ready => text(l, "Ready", "已就绪", "已就緒"),
        UsageState::DurabilityUnconfirmed => text(
            l,
            "Observed; crash durability unconfirmed",
            "已观测，断电持久性未确认",
            "已觀測，斷電持久性未確認",
        ),
        UsageState::Unavailable => text(
            l,
            "Retained history / save unavailable",
            "保留历史／保存不可用",
            "保留歷史／儲存無法使用",
        ),
        UsageState::Closing => text(
            l,
            "Closing, not yet confirmed",
            "关闭中，尚未确认",
            "關閉中，尚未確認",
        ),
        UsageState::Closed => text(l, "Orderly closed", "已完成有序关闭", "已完成有序關閉"),
    }
}
fn choice_label(choice: &Choice, view: Option<&View>, l: ui::UiLanguage) -> String {
    match choice {
        Choice::All => text(l, "All", "全部", "全部").into(),
        Choice::Other => text(
            l,
            "Other (combined groups)",
            "其他（合并分组）",
            "其他（合併分組）",
        )
        .into(),
        Choice::Named(GroupKey::Account(id)) => view
            .and_then(|v| v.accounts.iter().find(|a| &a.id == id))
            .map_or_else(
                || {
                    format!(
                        "{}: {id}",
                        text(l, "Historical account", "历史账号", "歷史帳號")
                    )
                },
                |a| format!("{} ({id})", a.label),
            ),
        Choice::Named(GroupKey::Profile(id)) => view
            .and_then(|v| v.connection_profiles.iter().find(|p| &p.id == id))
            .map_or_else(
                || {
                    format!(
                        "{}: {id}",
                        text(
                            l,
                            "Client profile / historical",
                            "客户端连接／历史",
                            "用戶端連線／歷史"
                        )
                    )
                },
                |p| format!("{} ({id})", p.input.label),
            ),
        Choice::Named(GroupKey::Model {
            provider,
            protocol,
            model,
        }) => format!("{provider:?} / {protocol:?} / {model}"),
    }
}
fn choice_json(choice: &Choice) -> json::JsonValue {
    match choice {
        Choice::All => json::object(vec![("kind", "all".into())]),
        Choice::Other => json::object(vec![("kind", "other".into())]),
        Choice::Named(GroupKey::Account(id)) => {
            json::object(vec![("kind", "account".into()), ("id", id.clone().into())])
        }
        Choice::Named(GroupKey::Profile(id)) => json::object(vec![
            ("kind", "client_profile".into()),
            ("id", id.clone().into()),
        ]),
        Choice::Named(GroupKey::Model {
            provider,
            protocol,
            model,
        }) => json::object(vec![
            ("kind", "model".into()),
            ("provider", format!("{provider:?}").into()),
            ("protocol", format!("{protocol:?}").into()),
            ("id", model.clone().into()),
        ]),
    }
}
fn rect_json(r: ui::Rect) -> json::JsonValue {
    json::object(vec![
        ("x", r.x.into()),
        ("y", r.y.into()),
        ("width", r.width.into()),
        ("height", r.height.into()),
    ])
}

fn metrics_rows(rows: &mut Vec<Row>, m: &Metrics, l: ui::UiLanguage) {
    if m.requests == 0 {
        rows.push(line(
            "empty",
            text(
                l,
                "No recorded requests in this retained UTC window.",
                "此保留UTC时段没有已记录请求。",
                "此保留UTC時段沒有已記錄請求。",
            )
            .into(),
        ));
    }
    for (id, label, n) in [
        ("requests", text(l, "Requests", "请求", "請求"), m.requests),
        (
            "attempts",
            text(
                l,
                "Provider request attempts",
                "提供方请求尝试",
                "提供方請求嘗試",
            ),
            m.transport_attempts,
        ),
        (
            "completed",
            text(l, "Completed", "完成", "完成"),
            m.completed,
        ),
        (
            "cancelled",
            text(l, "Cancelled", "取消", "取消"),
            m.cancelled,
        ),
        ("failed", text(l, "Failed", "失败", "失敗"), m.failed),
        (
            "missing",
            text(l, "Missing usage", "用量缺失", "用量缺失"),
            m.coverage[0],
        ),
        (
            "partial",
            text(l, "Partial usage", "部分用量", "部分用量"),
            m.coverage[1],
        ),
        (
            "final",
            text(l, "Final usage", "最终用量", "最終用量"),
            m.coverage[2],
        ),
        (
            "invalid",
            text(l, "Invalid usage", "无效用量", "無效用量"),
            m.coverage[3],
        ),
    ] {
        rows.push(line(id, format!("{label}: {n}")));
    }
    for (kind, (purpose, totals)) in [
        (text(l, "Final", "最终", "最終"), &m.tokens_final),
        (
            text(l, "Partial / inconsistent", "部分／不一致", "部分／不一致"),
            &m.tokens_partial,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        for (convention_index, t) in totals.iter().enumerate() {
            let convention = if convention_index == 0 {
                text(
                    l,
                    "Chat inclusive input; subsets already included",
                    "Chat输入含子集；勿重复相加",
                    "Chat輸入含子集；勿重複相加",
                )
            } else {
                text(
                    l,
                    "Messages base input; cache fields separate",
                    "Messages基础输入；缓存单列",
                    "Messages基礎輸入；快取單列",
                )
            };
            rows.push(line(
                &format!("token_header_{kind}_{convention_index}"),
                format!("{purpose} / {convention}"),
            ));
            for (i, label) in [
                text(l, "Input", "输入", "輸入"),
                text(l, "Output", "输出", "輸出"),
                text(l, "Reported total", "上游总量", "上游總量"),
                text(l, "Cache read subset", "缓存读取子项", "快取讀取子項"),
                text(l, "Cache create", "缓存创建", "快取建立"),
                text(l, "Reasoning subset", "推理子项", "推理子項"),
            ]
            .iter()
            .enumerate()
            {
                let value = if t.known[i] == 0 {
                    text(l, "Unavailable", "不可用", "無法使用").into()
                } else {
                    t.sum[i].to_string()
                };
                rows.push(line(
                    &format!("tokens_{kind}_{convention_index}_{i}"),
                    format!(
                        "{label}: {value}; {} {} / {} {}{}",
                        text(l, "known", "已知", "已知"),
                        t.known[i],
                        text(l, "missing", "缺失", "缺失"),
                        t.missing[i],
                        if t.overflow {
                            text(l, "; overflow", "；溢出", "；溢位")
                        } else {
                            ""
                        }
                    ),
                ));
            }
        }
    }
    for (kind, b) in [("final", &m.billing_final), ("partial", &m.billing_partial)] {
        billing_rows(rows, b, kind, l);
    }
    if m.overflow {
        rows.push(line(
            "metrics_overflow",
            text(
                l,
                "Totals overflow; not exact complete totals.",
                "总量溢出，不是完整精确值。",
                "總量溢位，並非完整精確值。",
            )
            .into(),
        ));
    }
}
fn decimal_label(value: &DecimalTotal, l: ui::UiLanguage) -> String {
    let number = if value.samples == 0 {
        text(l, "Unavailable", "不可用", "無法使用")
    } else {
        value.exact.as_str()
    };
    format!(
        "{number}; {} {} / {} {} / {} {}{}",
        text(l, "samples", "样本", "樣本"),
        value.samples,
        text(l, "missing", "缺失", "缺失"),
        value.missing,
        text(l, "overflow samples", "溢出样本", "溢位樣本"),
        value.overflow_samples,
        if value.overflow {
            text(l, "; overflow", "；溢出", "；溢位")
        } else {
            ""
        }
    )
}
fn billing_rows(rows: &mut Vec<Row>, b: &BillingTotals, kind: &str, l: ui::UiLanguage) {
    let coverage = if kind == "final" {
        text(
            l,
            "Final response financial metadata",
            "最终响应金融元数据",
            "最終回應金融中繼資料",
        )
    } else {
        text(
            l,
            "Partial response financial metadata",
            "部分响应金融元数据",
            "部分回應金融中繼資料",
        )
    };
    rows.push(line(&format!("billing_{kind}"), coverage.into()));
    for (id, label, value) in [
        (
            "reported",
            text(
                l,
                "Reported charge USD",
                "上游报告费用 USD",
                "上游回報費用 USD",
            ),
            &b.reported_cost_usd,
        ),
        (
            "estimated",
            text(l, "Estimated charge USD", "估算费用 USD", "估算費用 USD"),
            &b.estimated_cost_usd,
        ),
        (
            "unclassified",
            text(
                l,
                "Charge USD; estimation status absent",
                "费用 USD；估算状态缺失",
                "費用 USD；估算狀態缺失",
            ),
            &b.unclassified_cost_usd,
        ),
        (
            "list",
            text(
                l,
                "List cost USD (not cash charge)",
                "标价 USD（非现金费用）",
                "牌價 USD（非現金費用）",
            ),
            &b.list_cost_usd,
        ),
        (
            "allowance",
            text(
                l,
                "Plan allowance consumed, plan-USD (not wallet)",
                "套餐额度消耗，plan-USD（非钱包）",
                "方案額度消耗，plan-USD（非錢包）",
            ),
            &b.allowance_plan_usd,
        ),
    ] {
        rows.push(line(
            &format!("billing_{kind}_{id}"),
            format!("{label}: {}", decimal_label(value, l)),
        ));
    }
    for (i, label) in [
        text(l, "Billed input", "计费输入", "計費輸入"),
        text(l, "Billed output", "计费输出", "計費輸出"),
        text(l, "Billed cache read", "计费缓存读取", "計費快取讀取"),
        text(
            l,
            "Billed cache write 5m",
            "计费缓存写入5分钟",
            "計費快取寫入5分鐘",
        ),
        text(
            l,
            "Billed cache write 1h",
            "计费缓存写入1小时",
            "計費快取寫入1小時",
        ),
        text(l, "Billed reasoning", "计费推理", "計費推理"),
    ]
    .iter()
    .enumerate()
    {
        rows.push(line(
            &format!("billing_{kind}_tokens_{i}"),
            format!(
                "{label}: {}; {} {}",
                if b.billed_known[i] == 0 {
                    text(l, "Unavailable", "不可用", "無法使用").into()
                } else {
                    b.billed_sum[i].to_string()
                },
                text(l, "known samples", "已知样本", "已知樣本"),
                b.billed_known[i]
            ),
        ));
    }
    for (i, source) in [
        "free", "balance", "go", "go-plus", "byok", "unknown", "absent",
    ]
    .iter()
    .enumerate()
    {
        rows.push(line(
            &format!("billing_{kind}_source_{i}"),
            format!(
                "{} {source}: {}",
                text(l, "Billing source samples", "计费来源样本", "計費來源樣本"),
                b.sources[i]
            ),
        ));
    }
    rows.push(line(
        &format!("billing_{kind}_coverage"),
        format!(
            "{}: {} / {}; {}: {}",
            text(
                l,
                "Missing / invalid billing",
                "计费缺失／无效",
                "計費缺失／無效"
            ),
            b.missing_records,
            b.invalid_records,
            text(l, "Overflow", "溢出", "溢位"),
            b.overflow
        ),
    ));
}

pub(crate) fn freshness_label(value: monitoring::Freshness, l: ui::UiLanguage) -> &'static str {
    match value {
        monitoring::Freshness::Never => text(l, "Never observed", "从未观测", "從未觀測"),
        monitoring::Freshness::Fresh => text(l, "Fresh", "新鲜", "最新"),
        monitoring::Freshness::Stale => text(l, "Stale", "已过时", "已過時"),
        monitoring::Freshness::PreviousAuthority => text(
            l,
            "Previous authority (not current funds)",
            "旧授权（非当前资金）",
            "舊授權（非目前資金）",
        ),
        monitoring::Freshness::ClockUncertain => {
            text(l, "Clock uncertain", "时钟不确定", "時鐘不確定")
        }
    }
}
pub(crate) fn persistence_label(value: monitoring::Persistence, l: ui::UiLanguage) -> &'static str {
    match value {
        monitoring::Persistence::Opening => text(l, "Loading", "加载中", "載入中"),
        monitoring::Persistence::Ready => text(l, "Saved", "已保存", "已儲存"),
        monitoring::Persistence::Pending => text(l, "Save pending", "等待保存", "等待儲存"),
        monitoring::Persistence::DurabilityUnconfirmed => text(
            l,
            "Crash durability unconfirmed",
            "断电持久性未确认",
            "斷電持久性未確認",
        ),
        monitoring::Persistence::Unavailable => {
            text(l, "Storage unavailable", "存储不可用", "儲存無法使用")
        }
    }
}

fn metadata_rows(
    account: Option<&monitoring::AccountMonitorView>,
    persistence: monitoring::Persistence,
    l: ui::UiLanguage,
) -> Vec<Row> {
    let mut rows = vec![line(
        "metadata_storage",
        format!(
            "{}: {}",
            text(
                l,
                "Official metadata cache storage",
                "官方元数据缓存存储",
                "官方中繼資料快取儲存"
            ),
            persistence_label(persistence, l)
        ),
    )];
    let Some(a) = account else {
        rows.push(line(
            "metadata_missing",
            text(
                l,
                "Official resource reading unavailable. Configure an account explicitly.",
                "官方资源读数不可用，请显式配置账号。",
                "官方資源讀數無法使用，請明確設定帳號。",
            )
            .into(),
        ));
        return rows;
    };
    rows.push(line(
        "metadata_freshness",
        format!(
            "{}: {:?}; {}: {:?}",
            text(
                l,
                "Provider reading freshness",
                "提供方读数新鲜度",
                "提供方讀數新鮮度"
            ),
            freshness_label(a.freshness, l),
            text(l, "Monitoring pause", "监测暂停", "監測暫停"),
            a.paused
        ),
    ));
    rows.push(line(
        "metadata_capability",
        format!(
            "{}: {:?}; {}: {:?}",
            text(
                l,
                "Wallet contract capability",
                "钱包合约能力",
                "錢包契約能力"
            ),
            a.wallet_capability,
            text(
                l,
                "Allowance contract capability",
                "额度合约能力",
                "額度契約能力"
            ),
            a.allowance_capability
        ),
    ));
    rows.push(line(
        "metadata_policy",
        format!(
            "{}: {}; {}: {}",
            text(
                l,
                "Explicit monitoring opt-in",
                "显式监测授权",
                "明確監測授權"
            ),
            a.policy.enabled,
            text(l, "Needs attention", "需要关注", "需要注意"),
            a.attention
        ),
    ));
    if a.active_request.is_some() {
        rows.push(line(
            "metadata_active",
            text(
                l,
                "Official metadata read in progress",
                "正在读取官方元数据",
                "正在讀取官方中繼資料",
            )
            .into(),
        ));
    }
    if let Some(attempt) = &a.latest_attempt {
        rows.push(line(
            "metadata_attempt",
            format!(
                "{}: {:?}; {}: {}",
                text(
                    l,
                    "Latest metadata attempt",
                    "最近元数据尝试",
                    "最近中繼資料嘗試"
                ),
                attempt.result,
                text(l, "Finished", "结束于", "結束於"),
                utc_moment(attempt.finished_unix_ms, l)
            ),
        ));
    }
    if let Some(reading) = &a.last_success {
        rows.push(line(
            "metadata_source",
            format!(
                "{}: {:?}",
                text(
                    l,
                    "Official source contract",
                    "官方来源合约",
                    "官方來源契約"
                ),
                reading.source
            ),
        ));
        rows.push(line(
            "metadata_observed",
            format!(
                "{}: {}; {}: {}",
                text(
                    l,
                    "Official reading observed",
                    "官方读数观测于",
                    "官方讀數觀測於"
                ),
                utc_moment(Some(reading.observed_unix_ms), l),
                text(l, "Provider as-of", "提供方截至时间", "提供方截至時間"),
                reading.provider_as_of.as_deref().unwrap_or(text(
                    l,
                    "Unavailable",
                    "不可用",
                    "無法使用"
                ))
            ),
        ));
        match &reading.resources {
            monitoring::Resources::DeepSeekWallets(wallets) => {
                for (i, w) in wallets.iter().enumerate() {
                    rows.push(line(
                        &format!("wallet_{i}"),
                        format!(
                            "{} {:?} {:?}: {}",
                            text(l, "Official wallet", "官方钱包", "官方錢包"),
                            w.kind,
                            w.currency,
                            w.balance.0
                        ),
                    ));
                }
            }
            monitoring::Resources::OpenCodeGo(windows) => {
                for (i, w) in windows.iter().enumerate() {
                    rows.push(line(
                        &format!("allowance_{i}"),
                        format!(
                            "{} {:?}: {}%; {}: {}; {}: {}",
                            text(
                                l,
                                "Official Go allowance (not cash)",
                                "官方Go额度（非现金）",
                                "官方Go額度（非現金）"
                            ),
                            w.window,
                            w.used_percent.0,
                            text(l, "Rate limit reached", "已触及限额", "已達限額"),
                            w.limited,
                            text(l, "Provider reset", "提供方重置", "提供方重設"),
                            w.resets_at
                        ),
                    ));
                }
            }
        }
    } else {
        rows.push(line(
            "metadata_never",
            text(
                l,
                "No successful official reading. Unknown is not zero or unsupported.",
                "尚无成功官方读数；未知不等于零或不支持。",
                "尚無成功官方讀數；未知不等於零或不支援。",
            )
            .into(),
        ));
    }
    rows.push(line("metadata_disclaimer",text(l,"Current resource snapshot, independent from local UTC report totals. Known contract is not real-account qualification.","当前资源快照与本地UTC报告独立；已知合约不代表实际账号验收。","目前資源快照與本機UTC報告獨立；已知契約不代表實際帳號驗收。").into()));
    rows
}

pub(crate) fn compact(s: &UsageSummary, l: ui::UiLanguage) -> String {
    if s.today_utc_day.is_none() {
        return text(
            l,
            "Local UTC time unavailable",
            "本地UTC时间不可用",
            "本機UTC時間無法使用",
        )
        .into();
    }
    if matches!(s.state, UsageState::Opening | UsageState::Unavailable) {
        return source_label(s.state, l).into();
    }
    format!(
        "{}: {}; {}: {}; {}",
        text(
            l,
            "Today UTC client requests",
            "今天UTC客户端请求",
            "今天UTC用戶端請求"
        ),
        s.windows[0].client.requests,
        text(l, "Explicit checks", "显式检查", "明確檢查"),
        s.windows[0].probe.requests,
        source_label(s.state, l)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn report_reentry_preserves_scroll_query_and_existing_focus() {
        let mut report = Report {
            period: Period::Days30,
            traffic: Traffic::Probes,
            dimension: Dimension::Model,
            scroll: 37,
            focus: Some(Action::Monitoring),
            ..Default::default()
        };
        report.choices[2] = Choice::Other;
        report.restore_focus();
        assert_eq!(report.scroll, 37);
        assert_eq!(report.period, Period::Days30);
        assert_eq!(report.traffic, Traffic::Probes);
        assert_eq!(report.dimension, Dimension::Model);
        assert!(matches!(report.choices[2], Choice::Other));
        assert!(matches!(report.focus, Some(Action::Monitoring)));
        report.leave_focus();
        report.restore_focus();
        assert_eq!(report.scroll, 37);
        assert!(!report.focused());
        let mut first = Report::default();
        first.restore_focus();
        assert!(matches!(first.focus, Some(Action::Period)));
        assert_eq!(first.scroll, 0);
    }
    #[test]
    fn report_independent_dimensions_other_and_missing_selection_are_not_joint_filters() {
        let mut s = UsageSummary::default();
        s.windows[0].client.requests = 70;
        let mut a = GroupSummary {
            key: Some(GroupKey::Account("a".repeat(32))),
            ..Default::default()
        };
        a.windows[0].client.requests = 11;
        let mut other = GroupSummary::default();
        other.windows[0].client.requests = 3;
        s.accounts = vec![a, other];
        let mut r = Report::default();
        r.choices[0] = Choice::Other;
        assert_eq!(r.metrics(&s).map(|(m, _)| m.requests), Some(3));
        r.dimension = Dimension::Profile;
        assert_eq!(r.metrics(&s).map(|(m, _)| m.requests), Some(70));
        r.choices[1] = Choice::Named(GroupKey::Profile("gone".into()));
        assert!(
            r.metrics(&s).is_none(),
            "missing selection cannot silently become All"
        );
    }
    #[test]
    fn report_client_probe_windows_and_exact_financial_channels_remain_distinct() {
        let mut s = UsageSummary::default();
        s.windows[2].probe.requests = 9;
        s.windows[0].client.requests = 1;
        let mut r = Report {
            period: Period::Days30,
            traffic: Traffic::Probes,
            ..Default::default()
        };
        assert_eq!(r.metrics(&s).map(|(m, _)| m.requests), Some(9));
        r.period = Period::Today;
        r.traffic = Traffic::Clients;
        assert_eq!(r.metrics(&s).map(|(m, _)| m.requests), Some(1));
        let mut b = BillingTotals::default();
        b.reported_cost_usd.exact = "0.000000000000000001".into();
        b.reported_cost_usd.samples = 1;
        b.list_cost_usd.exact = "99".into();
        b.list_cost_usd.samples = 1;
        let mut rows = Vec::new();
        billing_rows(&mut rows, &b, "final", ui::UiLanguage::English);
        assert!(
            rows.iter()
                .any(|r| r.id == "billing_final_reported"
                    && r.label.contains("0.000000000000000001"))
        );
        assert!(
            rows.iter()
                .any(|r| r.id == "billing_final_list" && r.label.contains("99"))
        );
        assert!(
            rows.iter()
                .any(|r| r.id == "billing_final_allowance" && r.label.contains("Unavailable"))
        );
    }
    #[test]
    fn report_metric_coalescer_releases_last_pending_frame_and_lifecycle_immediately() {
        let now = Instant::now();
        let mut r = Report::default();
        let s = Arc::new(UsageSummary::default());
        assert!(r.observe(s.clone(), true, now));
        let mut next = (*s).clone();
        next.run_admitted = 4;
        assert!(!r.observe(Arc::new(next), true, now + Duration::from_millis(5)));
        assert!(r.deadline().is_some());
        assert!(r.service(now + Duration::from_secs(1), true));
        assert_eq!(r.presented(None).map(|s| s.run_admitted), Some(4));
        let mut error = r.presented(None).unwrap().clone();
        error.state = UsageState::Unavailable;
        assert!(r.observe(Arc::new(error), true, now + Duration::from_millis(1001)));
    }
    #[test]
    fn report_narrow_multilingual_buttons_share_complete_hit_and_painted_lines() {
        for l in [
            ui::UiLanguage::English,
            ui::UiLanguage::ChineseSimplified,
            ui::UiLanguage::ChineseTraditional,
        ] {
            let mut r = Report::default();
            let p = r.project(
                None,
                l,
                ui::Rect {
                    x: 0,
                    y: 0,
                    width: 280,
                    height: 140,
                },
                14,
            );
            let mut pixels = vec![0; 280 * 140];
            {
                let mut surface =
                    Surface::with_clip(&mut pixels, 280, 140, PixelRect::from_xywh(0, 0, 280, 140));
                r.paint(
                    &mut surface,
                    &p,
                    14,
                    theme::Theme::for_choice(theme::ThemeChoice::default()),
                );
            }
            let theme = theme::Theme::for_choice(theme::ThemeChoice::default());
            assert!(
                pixels.iter().any(|pixel| *pixel != 0
                    && *pixel != theme.sidebar_bg.to_xrgb()
                    && *pixel != theme.surface.to_xrgb()),
                "actual report glyphs must be painted"
            );
            let e = p.entries.iter().find(|e| e.id == "period").unwrap();
            assert!(p.complete(e));
            assert_eq!(e.lines.concat().replace(' ', ""), e.label.replace(' ', ""));
            let _ = r.activate(Action::Period, None);
            assert_eq!(r.period, Period::Days7);
        }
    }
    #[test]
    fn report_unavailable_or_unknown_utc_never_paints_a_default_zero_window() {
        let mut view = View::default();
        let summary = Arc::make_mut(&mut view.usage);
        summary.state = UsageState::Ready;
        summary.today_utc_day = None;
        summary.computed_at_unix_ms = None;
        let r = Report::default();
        let rows = r.rows(Some(&view), ui::UiLanguage::English);
        assert!(rows.iter().any(|r| r.id == "clock_unknown"));
        assert!(!rows.iter().any(|r| r.id == "requests" || r.id == "empty"));
        let summary = Arc::make_mut(&mut view.usage);
        summary.today_utc_day = Some(20_000);
        summary.computed_at_unix_ms = Some(20_000 * 86_400_000);
        summary.state = UsageState::Unavailable;
        let rows = r.rows(Some(&view), ui::UiLanguage::English);
        assert!(rows.iter().any(|r| r.id == "history_unknown"));
        assert!(!rows.iter().any(|r| r.id == "requests" || r.id == "empty"));
    }
    #[test]
    fn report_probe_coverage_and_optional_subsets_are_rendered_without_inclusive_addition() {
        let mut m = Metrics {
            requests: 4,
            coverage: [1, 1, 1, 1],
            ..Default::default()
        };
        m.tokens_final[0].sum[0] = 13;
        m.tokens_final[0].known[0] = 1;
        m.tokens_final[0].sum[3] = 5;
        m.tokens_final[0].known[3] = 1;
        m.tokens_partial[0].sum[1] = 2;
        m.tokens_partial[0].known[1] = 1;
        let mut rows = Vec::new();
        metrics_rows(&mut rows, &m, ui::UiLanguage::English);
        assert!(
            rows.iter()
                .any(|r| r.id == "tokens_0_0_0" && r.label.starts_with("Input: 13;"))
        );
        assert!(
            rows.iter()
                .any(|r| r.id == "tokens_0_0_3" && r.label.starts_with("Cache read subset: 5;"))
        );
        assert!(
            rows.iter()
                .any(|r| r.id == "tokens_1_0_1" && r.label.starts_with("Output: 2;"))
        );
        for id in ["missing", "partial", "final", "invalid"] {
            assert!(
                rows.iter().any(|r| r.id == id && r.label.ends_with(": 1")),
                "missing coverage {id}"
            );
        }
    }
    #[test]
    fn report_model_identity_and_presentation_snapshot_keep_safe_dimensions_only() {
        let mut s = UsageSummary {
            today_utc_day: Some(20_000),
            computed_at_unix_ms: Some(1),
            state: UsageState::Ready,
            ..Default::default()
        };
        let model = |protocol| GroupKey::Model {
            provider: provider_auth::Provider::DeepSeek,
            protocol,
            model: "same-model".into(),
        };
        let first = model(crate::provider_protocol::usage::InputSemantics::ChatInclusive);
        let second = model(crate::provider_protocol::usage::InputSemantics::MessagesBasePlusCache);
        let mut a = GroupSummary {
            key: Some(first.clone()),
            ..Default::default()
        };
        a.windows[0].client.requests = 7;
        let mut b = GroupSummary {
            key: Some(second),
            ..Default::default()
        };
        b.windows[0].client.requests = 2;
        s.models = vec![a, b];
        let mut r = Report {
            dimension: Dimension::Model,
            ..Default::default()
        };
        r.choices[2] = Choice::Named(first);
        assert_eq!(r.metrics(&s).map(|(m, _)| m.requests), Some(7));
        let view = View {
            usage: Arc::new(s),
            ..Default::default()
        };
        r.displayed = Some(view.usage.clone());
        let p = r.project(
            Some(&view),
            ui::UiLanguage::English,
            ui::Rect {
                x: 0,
                y: 0,
                width: 320,
                height: 200,
            },
            14,
        );
        let encoded = String::from_utf8(json::to_vec(&r.snapshot(
            &p,
            Some(&view),
            ui::UiLanguage::English,
        )))
        .unwrap();
        assert!(encoded.contains("same-model") && encoded.contains("ChatInclusive"));
        assert!(
            !encoded.contains("credential")
                && !encoded.contains("organization_digest")
                && !encoded.contains("headers")
        );
    }
}
