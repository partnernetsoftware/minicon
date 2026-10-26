//! Account form feedback, bound to its existing draft and save receipt.
use super::*;

#[derive(Clone, Copy)]
pub(super) enum Validation {
    LoginProxy,
    ServiceProxy,
    Sequence,
}

impl HarnessSettings {
    pub(super) fn account_save_state(&self) -> &'static str {
        let draft = &self.account_draft;
        if draft.submitted.is_some() {
            return "saving";
        }
        if let Some(issue) = draft.validation {
            return match issue {
                Validation::LoginProxy => "invalid_login_proxy",
                Validation::ServiceProxy => "invalid_service_proxy",
                Validation::Sequence => "sequence_exhausted",
            };
        }
        if draft.save_error.is_some() {
            return "failed";
        }
        if draft.saved_request.is_some() {
            return if draft.dirty() {
                "saved_with_draft"
            } else {
                "saved"
            };
        }
        if draft.dirty() { "unsaved" } else { "idle" }
    }

    pub(super) fn account_save_label(&self, language: ui::UiLanguage) -> &'static str {
        let tr = |en, zh, tw| super::super::harness_workspace_ui::text(language, en, zh, tw);
        match self.account_save_state() {
            "saving" => tr(
                "Saving account. Later edits remain in the draft.",
                "正在保存账号，后续编辑将保留为草稿。",
                "正在儲存帳號，後續編輯將保留為草稿。",
            ),
            "invalid_login_proxy" => tr(
                "Login proxy is invalid. Use a plain HTTP proxy URL, or leave it empty for Direct.",
                "登录代理无效。请使用普通 HTTP 代理网址，或留空以直连。",
                "登入代理無效。請使用一般 HTTP 代理網址，或留空以直連。",
            ),
            "invalid_service_proxy" => tr(
                "Service proxy is invalid. Use a plain HTTP proxy URL, or leave it empty for Direct.",
                "服务代理无效。请使用普通 HTTP 代理网址，或留空以直连。",
                "服務代理無效。請使用一般 HTTP 代理網址，或留空以直連。",
            ),
            "sequence_exhausted" => tr(
                "Save request limit reached. Reopen MiniCon before saving again.",
                "保存请求已达上限，请重新打开 MiniCon 后再保存。",
                "儲存請求已達上限，請重新開啟 MiniCon 後再儲存。",
            ),
            "failed" => match self.account_draft.save_error {
                Some(provider_runtime::RuntimeError::AccountChanged) => tr(
                    "Account changed since editing. Revert to load saved settings, then edit again.",
                    "账号在编辑期间已变化。请撤销修改以载入已保存设置，再重新编辑。",
                    "帳號在編輯期間已變更。請還原修改以載入已儲存設定，再重新編輯。",
                ),
                Some(provider_runtime::RuntimeError::Busy) => tr(
                    "Account save was not accepted while busy. Keep your draft and retry when the operation finishes.",
                    "当前忙碌，未接受账号保存。草稿已保留，请在操作完成后重试。",
                    "目前忙碌，未接受帳號儲存。草稿已保留，請在操作完成後重試。",
                ),
                Some(provider_runtime::RuntimeError::WorkerUnavailable) => tr(
                    "Account service is unavailable. Keep your draft; retry when available or reopen MiniCon.",
                    "账号服务不可用。草稿已保留，请在恢复后重试，或重新打开 MiniCon。",
                    "帳號服務無法使用。草稿已保留，請在恢復後重試，或重新開啟 MiniCon。",
                ),
                _ => tr(
                    "Could not confirm account save. Draft retained. Retry, or Revert to inspect saved settings.",
                    "无法确认账号已保存，草稿已保留。请重试，或撤销修改以查看已保存设置。",
                    "無法確認帳號已儲存，草稿已保留。請重試，或還原修改以查看已儲存設定。",
                ),
            },
            "saved_with_draft" => tr(
                "Submitted changes saved. Newer edits are still unsaved.",
                "提交的修改已保存，后续编辑尚未保存。",
                "提交的修改已儲存，後續編輯尚未儲存。",
            ),
            "saved" => tr(
                "Account changes saved.",
                "账号修改已保存。",
                "帳號修改已儲存。",
            ),
            "unsaved" => tr(
                "Account changes are unsaved. Save to apply, or Revert changes.",
                "账号修改尚未保存。请保存以应用，或撤销修改。",
                "帳號修改尚未儲存。請儲存以套用，或還原修改。",
            ),
            _ => tr(
                "No unsaved account changes.",
                "没有未保存的账号修改。",
                "沒有未儲存的帳號修改。",
            ),
        }
    }
}
