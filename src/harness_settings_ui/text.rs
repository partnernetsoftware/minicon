//! Product-private read-only explanation projection; continuation rows are inert.
use super::*;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Presentation {
    pub width: u32,
    pub size: u16,
    pub language: ui::UiLanguage,
    pub visible_rows: usize,
}

pub(super) fn label(
    settings: &HarnessSettings,
    row: Row,
    language: ui::UiLanguage,
) -> Option<(String, bool)> {
    let tr = |en, zh, tw| super::super::harness_workspace_ui::text(language, en, zh, tw);
    let mut label = String::new();
    let mut heading = false;
    match row {
        Row::AccountRemovalInfo(index) => {
            label = settings
                .removal_labels(language)
                .get(index)
                .cloned()
                .unwrap_or_default();
            heading = index == 0;
        }
        Row::Overview(index) => {
            label = settings.overview_label(index, language);
            heading = index == 0;
        }
        Row::Local => {
            label = "Local API".to_owned();
            heading = true;
        }
        Row::Accounts => {
            label = "Provider accounts".to_owned();
            heading = true;
        }
        Row::Connect => {
            label = tr(
                "Connect the selected applied profile",
                "连接所选已应用连接",
                "連接所選已套用連線",
            )
            .to_owned();
            heading = true;
        }
        Row::Listener => {
            label = format!(
                "{}{}",
                settings.overview_label(2, language),
                settings
                    .view
                    .actual_port
                    .filter(|_| settings.view.listener == ListenerState::Listening)
                    .map_or_else(String::new, |port| format!(" / 127.0.0.1:{port}"))
            )
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
        Row::AccountStatus(index) => {
            let account = &settings.view.accounts[index];
            label = format!(
                "{:?}: {}",
                account.provider,
                pool::state(&settings.view, account).caption(language)
            );
        }
        Row::AccountDetail(index) => {
            label = settings
                .account_details(language)
                .get(index)
                .cloned()
                .unwrap_or_default()
        }
        Row::AccountFieldCaption(field) => label = account_field_caption(field, language).into(),
        Row::AccountNetworkHint => {
            label = tr(
                "Empty proxy means Direct.",
                "代理留空表示直连。",
                "代理留空表示直連。",
            )
            .into();
        }
        Row::ConnectNetworkSeparation | Row::AccountNetworkSeparation => {
            label = tr(
                "Login and service networks are independent.",
                "登录与服务网络独立设置。",
                "登入與服務網路獨立設定。",
            )
            .into();
        }
        Row::CatalogueStatus => {
            let catalogue = settings
                .view
                .catalogues
                .iter()
                .find(|entry| Some(&entry.account) == settings.account_focus.as_ref());
            label = catalogue.map_or_else(
                || "Catalogue: Not loaded".into(),
                |entry| {
                    format!(
                        "Catalogue: {:?} · {} models · read only{}",
                        entry.status,
                        entry.models.len(),
                        if entry.error.is_some() {
                            " · refresh failed"
                        } else {
                            ""
                        }
                    )
                },
            );
        }
        Row::CatalogueModel(index) => {
            if let Some(model) = settings
                .view
                .catalogues
                .iter()
                .find(|entry| Some(&entry.account) == settings.account_focus.as_ref())
                .and_then(|entry| entry.models.get(index))
            {
                label = format!("{} · {:?} · read only", model.id, model.protocol);
            }
        }
        Row::AccountReconnectHint => {
            label = tr(
                "Reconnect uses saved login settings.",
                "重新连接使用已保存的登录设置。",
                "重新連線使用已儲存的登入設定。",
            )
            .into();
        }
        Row::AccountReconnectVerification => {
            label = tr(
                "Verify this account's models again afterwards.",
                "重新连接后需再次验证此账号的模型。",
                "重新連線後需再次驗證此帳號的模型。",
            )
            .into();
        }
        Row::ConnectNetworkHint => {
            label = tr(
                "Choose both networks before Continue.",
                "继续前请设置两种网络。",
                "繼續前請設定兩種網路。",
            )
            .into();
        }
        Row::NoAccounts => label = "No account. Add a provider to continue.".to_owned(),
        Row::ProfileState => {
            label = settings.forms.current().map_or_else(
                || {
                    match settings.view.load_state {
                        provider_runtime::LoadState::Loading => {
                            tr("Loading connections...", "正在加载连接…", "正在載入連線…")
                        }
                        provider_runtime::LoadState::Loaded => tr(
                            "Select or create a connection.",
                            "请选择或新建连接。",
                            "請選擇或建立連線。",
                        ),
                        provider_runtime::LoadState::Unavailable(_) => tr(
                            "Connections unavailable; check saved settings.",
                            "连接不可用，请检查保存的设置。",
                            "連線不可用，請檢查儲存的設定。",
                        ),
                    }
                    .into()
                },
                |form| {
                    if form.waiting() {
                        return tr(
                            "Saving connection changes...",
                            "正在保存连接更改…",
                            "正在儲存連線變更…",
                        )
                        .into();
                    }
                    if let Some(error) = &form.local_error {
                        return error.clone();
                    }
                    if form.unsent() {
                        return tr(
                            "Changes not saved; retry saving.",
                            "更改尚未保存，请重试保存。",
                            "變更尚未儲存，請重試儲存。",
                        )
                        .into();
                    }
                    if let Some(error) = form.view.error {
                        use crate::provider_selection::ConnectionInputError as E;
                        return match error {
                            E::InvalidLabel => tr(
                                "Enter a valid connection name.",
                                "请输入有效的连接名称。",
                                "請輸入有效的連線名稱。",
                            ),
                            E::InvalidAlias => tr(
                                "Use a valid public model alias.",
                                "请输入有效的公开模型别名。",
                                "請輸入有效的公開模型別名。",
                            ),
                            E::DuplicateAlias => tr(
                                "Use a different alias for each model.",
                                "每个模型需使用不同的别名。",
                                "每個模型需使用不同的別名。",
                            ),
                            E::InvalidDefaultAlias => tr(
                                "Choose a default alias from this connection.",
                                "请从此连接中选择默认模型别名。",
                                "請從此連線中選擇預設模型別名。",
                            ),
                            E::IncompleteAllocation => tr(
                                "Choose an account and model.",
                                "请选择账号和模型。",
                                "請選擇帳號和模型。",
                            ),
                            E::UnselectedModel => tr(
                                "Choose a model from this account's catalogue.",
                                "请从此账号的目录中选择模型。",
                                "請從此帳號的目錄中選擇模型。",
                            ),
                            E::DefaultCompatibility => tr(
                                "Use a new connection for custom settings.",
                                "自定义设置请使用新建连接。",
                                "自訂設定請使用新建連線。",
                            ),
                        }
                        .into();
                    }
                    if form.view.dirty {
                        tr(
                            "Saved draft; verify before applying.",
                            "草稿已保存，请先验证再应用。",
                            "草稿已儲存，請先驗證再套用。",
                        )
                    } else {
                        tr(
                            "Applied connection; no pending edits.",
                            "连接已应用，没有待处理的更改。",
                            "連線已套用，沒有待處理的變更。",
                        )
                    }
                    .into()
                },
            );
        }
        Row::ProfileApplied => {
            label = settings.forms.current().map_or_else(String::new, |form| {
                form.view.applied.as_ref().map_or_else(
                    || {
                        tr(
                            "Applied: none; save and verify this draft.",
                            "尚未应用，请保存并验证此草稿。",
                            "尚未套用，請儲存並驗證此草稿。",
                        )
                        .into()
                    },
                    |applied| {
                        format!(
                            "{}: {} / {} {}",
                            tr("Applied", "已应用", "已套用"),
                            applied.label,
                            applied.allocations.len(),
                            tr("models", "个模型", "個模型"),
                        )
                    },
                )
            });
        }
        Row::ProfileDraftHint => {
            label = tr(
                "Draft edits do not change the applied connection.",
                "编辑草稿不会改变已应用的连接。",
                "編輯草稿不會改變已套用的連線。",
            )
            .into()
        }
        Row::AppliedAlias(index) | Row::AppliedAccount(index) | Row::AppliedModel(index) => {
            if let Some(allocation) = settings.applied_allocations().get(index) {
                let account = allocation.account.as_deref().map_or_else(
                    || tr("No account", "未选择账号", "未選擇帳號").into(),
                    |id| {
                        settings
                            .view
                            .accounts
                            .iter()
                            .find(|account| account.id == id)
                            .map_or_else(
                                || {
                                    format!(
                                        "{} [{}]",
                                        tr("Unavailable account", "账号不可用", "帳號不可用"),
                                        id.get(..8).unwrap_or(id)
                                    )
                                },
                                |account| {
                                    format!("{} [{}]", account.label, id.get(..8).unwrap_or(id))
                                },
                            )
                    },
                );
                label = match row {
                    Row::AppliedAlias(_) => format!(
                        "{}: {}",
                        tr("Applied alias", "生效别名", "生效別名"),
                        allocation.alias
                    ),
                    Row::AppliedAccount(_) => format!(
                        "{}: {}",
                        tr("Applied account", "生效账号", "生效帳號"),
                        account
                    ),
                    _ => format!(
                        "{}: {}",
                        tr("Applied model", "生效模型", "生效模型"),
                        allocation.model
                    ),
                };
            }
        }
        Row::ProbeNotice => {
            label = settings
                .forms
                .current()
                .and_then(|form| form.local_error.clone())
                .unwrap_or_else(|| {
                    tr(
                        "Output ≤8 tokens; input may also be billed.",
                        "输出≤8 tokens，输入也可能收费。",
                        "輸出≤8 tokens，輸入也可能收費。",
                    )
                    .into()
                })
        }
        Row::ProbeBehavior => {
            label = tr(
                "No automatic retry or Apply.",
                "不会自动重试或应用。",
                "不會自動重試或套用。",
            )
            .into()
        }
        Row::KeyState => label = settings.key_state_label(),
        Row::KeyDurability => {
            label = if settings.selected_key_durability_unconfirmed() {
                "Crash durability: unconfirmed"
            } else {
                "No outstanding key durability warning"
            }
            .into()
        }
        Row::KeyFeedback => label = settings.key_feedback_label(),
        Row::KeyApplied => label = settings.key_applied_label(),
        Row::KeyShell => {
            label = if cfg!(windows) {
                "PowerShell: private prompt in setup"
            } else {
                "bash: private prompt in setup"
            }
            .into()
        }
        Row::OpenAi => label = settings.endpoint_label(true),
        Row::Anthropic => label = settings.endpoint_label(false),
        Row::Status => {
            label = settings
                .status()
                .unwrap_or_else(|| "Start and model tests are explicit actions.".to_owned())
        }
        _ => return None,
    }
    Some((label, heading))
}

/// Preserve every scalar, including spaces. Hard breaks are the only removed separators.
pub(super) fn lines(value: &str, width: u32, size: u16) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for ch in value.chars() {
        if ch == '\n' {
            lines.push(std::mem::take(&mut current));
            continue;
        }
        let mut candidate = current.clone();
        candidate.push(ch);
        if !current.is_empty() && host_ui::host_ui_text_width(&candidate, size) > width {
            let split = current
                .char_indices()
                .rev()
                .find_map(|(i, c)| c.is_whitespace().then_some(i + c.len_utf8()));
            if let Some(split) = split.filter(|&split| split < current.len()) {
                let tail = current.split_off(split);
                lines.push(std::mem::replace(&mut current, tail));
            } else {
                lines.push(std::mem::take(&mut current));
            }
            let mut candidate = current.clone();
            candidate.push(ch);
            if !current.is_empty() && host_ui::host_ui_text_width(&candidate, size) > width {
                lines.push(std::mem::take(&mut current));
            }
        }
        current.push(ch);
    }
    lines.push(current);
    lines
}

pub(super) fn expand(
    settings: &HarnessSettings,
    rows: &[Row],
    presentation: Presentation,
) -> Vec<Row> {
    let mut expanded = Vec::new();
    for (source, &row) in rows.iter().enumerate() {
        expanded.push(row);
        if let Some((value, _)) = label(settings, row, presentation.language) {
            expanded.extend(
                (1..lines(&value, presentation.width, presentation.size).len())
                    .map(|line| Row::TextContinuation { source, line }),
            );
        }
    }
    expanded
}

pub(super) fn observation(settings: &HarnessSettings, row: Row) -> json::JsonValue {
    let Some(presentation) = settings.text_presentation else {
        return json::JsonValue::Null;
    };
    let (row, line) = match row {
        Row::TextContinuation { source, line } => (settings.base_rows()[source], line),
        row => (row, 0),
    };
    let Some((value, _)) = label(settings, row, presentation.language) else {
        return json::JsonValue::Null;
    };
    let lines = lines(&value, presentation.width, presentation.size);
    let measured = host_ui::host_ui_text_width(&lines[line], presentation.size);
    json::object(vec![
        ("source", row_id(row).into()),
        ("line", line.into()),
        ("line_count", lines.len().into()),
        ("fits", (measured <= presentation.width).into()),
    ])
}

impl HarnessSettings {
    pub(super) fn configure_text(&mut self, presentation: Option<Presentation>) -> bool {
        if self.text_presentation == presentation {
            return false;
        }
        let old = self.rows().get(self.scroll).copied();
        let old_source = old.map(|row| match row {
            Row::TextContinuation { source, .. } => row_id(self.base_rows()[source]),
            row => row_id(row),
        });
        self.text_presentation = presentation;
        let rows = self.rows();
        self.scroll = old
            .and_then(|row| rows.iter().position(|r| row_id(*r) == row_id(row)))
            .or_else(|| old_source.and_then(|id| rows.iter().position(|r| row_id(*r) == id)))
            .unwrap_or(0);
        if let Some(presentation) = presentation && let Some(index) = rows.iter().position(|row| {
            row_field(*row).is_some_and(|field| Some(field) == self.focused)
                || matches!(row, Row::Monitoring(item) if Some(*item) == self.monitoring.current())
        }) {
            if index < self.scroll {
                self.scroll = index;
            } else if index >= self.scroll.saturating_add(presentation.visible_rows) {
                self.scroll = index.saturating_sub(presentation.visible_rows.saturating_sub(1));
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explanation_lines_preserve_unicode_and_fit_the_actual_font_width() {
        for width in [46, 85, 200] {
            for value in [
                "Login network and service network are independent.  Keep spaces.",
                "登录代理与服务代理互相独立，不会自动开始登录或保存配置。",
                "登入代理與服務代理互相獨立，不會自動開始登入或儲存設定。",
                "https://example.invalid/long-model-without-word-breaks",
                "  mixed中e\u{301}文 test 🧪 with trailing spaces  ",
                "first line\n第二行\n",
            ] {
                let result = lines(value, width, 14);
                assert_eq!(result.concat(), value.replace('\n', ""));
                assert!(
                    result
                        .iter()
                        .all(|line| host_ui::host_ui_text_width(line, 14) <= width),
                    "text exceeds {width}px: {result:?}"
                );
            }
        }
    }
}
