//! User-facing browser state and recovery copy. Product owner: PRD 25.

use crate::{browser::PageLoad, ui::UiLanguage};

pub(super) struct BrowserFeedback {
    pub title: &'static str,
    pub detail: &'static str,
    pub failed: bool,
}

pub(super) fn for_load(load: &PageLoad, language: UiLanguage) -> BrowserFeedback {
    let (title, detail) = match (load, language) {
        (PageLoad::Idle, UiLanguage::English) => (
            "Browse beside your terminal",
            "Enter a URL above, or open a link from a terminal.",
        ),
        (PageLoad::Idle, UiLanguage::ChineseSimplified) => {
            ("在终端旁预览网页", "在上方输入网址，或从终端打开链接。")
        }
        (PageLoad::Idle, UiLanguage::ChineseTraditional) => {
            ("在終端旁預覽網頁", "在上方輸入網址，或從終端開啟連結。")
        }
        (PageLoad::Loading { .. }, UiLanguage::English) => (
            "Opening page…",
            "You can keep working in your terminal while this page loads.",
        ),
        (PageLoad::Loading { .. }, UiLanguage::ChineseSimplified) => {
            ("正在打开网页…", "加载期间，仍可切回终端继续工作。")
        }
        (PageLoad::Loading { .. }, UiLanguage::ChineseTraditional) => {
            ("正在開啟網頁…", "載入期間，仍可切回終端繼續工作。")
        }
        (PageLoad::Ready { .. }, UiLanguage::English) => (
            "Page ready",
            "Switch back to your terminal whenever you need it.",
        ),
        (PageLoad::Ready { .. }, UiLanguage::ChineseSimplified) => {
            ("网页已就绪", "随时可以切回终端继续工作。")
        }
        (PageLoad::Ready { .. }, UiLanguage::ChineseTraditional) => {
            ("網頁已就緒", "隨時可以切回終端繼續工作。")
        }
        (PageLoad::Unavailable { reason, .. }, language) => failure_copy(reason, language),
    };
    BrowserFeedback {
        title,
        detail,
        failed: matches!(load, PageLoad::Unavailable { .. }),
    }
}

fn failure_copy(reason: &str, language: UiLanguage) -> (&'static str, &'static str) {
    // The timeout proves a deadline, not whether DNS, a local service or the
    // network caused it. Only a known configuration/runtime token is specific.
    match (reason, language) {
        ("browser-navigation-timeout", UiLanguage::English) => (
            "This page is taking too long",
            "The address is saved. Check the service or connection, then retry.",
        ),
        ("browser-navigation-timeout", UiLanguage::ChineseSimplified) => (
            "网页加载超时",
            "网址已保留。请检查服务或网络连接，然后重试。",
        ),
        ("browser-navigation-timeout", UiLanguage::ChineseTraditional) => (
            "網頁載入逾時",
            "網址已保留。請檢查服務或網路連線，然後重試。",
        ),
        ("browser-proxy-unsupported", UiLanguage::English) => (
            "Proxy unavailable on this system",
            "This native browser cannot apply that proxy. Edit or clear the proxy field.",
        ),
        ("browser-proxy-unsupported", UiLanguage::ChineseSimplified) => (
            "当前系统不支持此代理",
            "网页组件无法应用此代理。请修改或清空代理输入框。",
        ),
        ("browser-proxy-unsupported", UiLanguage::ChineseTraditional) => (
            "目前系統不支援此代理",
            "網頁元件無法套用此代理。請修改或清空代理輸入框。",
        ),
        ("browser-proxy-invalid", UiLanguage::English) => (
            "Check browser proxy settings",
            "The configured proxy is invalid. Correct it before retrying.",
        ),
        ("browser-proxy-invalid", UiLanguage::ChineseSimplified) => (
            "请检查浏览器代理配置",
            "当前代理配置无效。修正配置后可重试。",
        ),
        ("browser-proxy-invalid", UiLanguage::ChineseTraditional) => (
            "請檢查瀏覽器代理設定",
            "目前的代理設定無效。修正設定後可重試。",
        ),
        (
            "browser-native-create-failed"
            | "browser-native-unavailable"
            | "browser-host-not-configured",
            UiLanguage::English,
        ) => (
            "Browser unavailable",
            "The native page could not start. Your terminal is still available.",
        ),
        (
            "browser-native-create-failed"
            | "browser-native-unavailable"
            | "browser-host-not-configured",
            UiLanguage::ChineseSimplified,
        ) => (
            "浏览器暂时不可用",
            "网页组件未能启动。仍可使用终端继续工作。",
        ),
        (
            "browser-native-create-failed"
            | "browser-native-unavailable"
            | "browser-host-not-configured",
            UiLanguage::ChineseTraditional,
        ) => (
            "瀏覽器暫時無法使用",
            "網頁元件未能啟動。仍可使用終端繼續工作。",
        ),
        (_, UiLanguage::English) => (
            "Unable to open this page",
            "The address is saved. Check the address or local service, then retry.",
        ),
        (_, UiLanguage::ChineseSimplified) => (
            "无法打开此网页",
            "网址已保留。请检查网址或本地服务，然后重试。",
        ),
        (_, UiLanguage::ChineseTraditional) => (
            "無法開啟此網頁",
            "網址已保留。請檢查網址或本機服務，然後重試。",
        ),
    }
}

pub(super) fn retry_label(language: UiLanguage) -> &'static str {
    match language {
        UiLanguage::English => "Retry",
        UiLanguage::ChineseSimplified => "重试",
        UiLanguage::ChineseTraditional => "重試",
    }
}

pub(super) fn edit_label(language: UiLanguage) -> &'static str {
    match language {
        UiLanguage::English => "Edit address",
        UiLanguage::ChineseSimplified => "编辑地址",
        UiLanguage::ChineseTraditional => "編輯網址",
    }
}

pub(super) fn is_proxy_failure(load: &PageLoad) -> bool {
    matches!(load, PageLoad::Unavailable { reason, .. }
        if reason == "browser-proxy-invalid" || reason == "browser-proxy-unsupported")
}

pub(super) fn edit_proxy_label(language: UiLanguage) -> &'static str {
    match language {
        UiLanguage::English => "Edit proxy",
        UiLanguage::ChineseSimplified => "修改代理",
        UiLanguage::ChineseTraditional => "修改代理",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(reason: &str) -> PageLoad {
        PageLoad::Unavailable {
            url: "http://localhost/preview".into(),
            reason: reason.into(),
        }
    }

    #[test]
    fn unknown_failure_copy_does_not_invent_a_network_diagnosis_or_expose_the_reason() {
        let unknown = for_load(&failure("raw-private-error-token"), UiLanguage::English);
        assert!(unknown.failed);
        assert_eq!(unknown.title, "Unable to open this page");
        assert_eq!(
            unknown.detail,
            "The address is saved. Check the address or local service, then retry."
        );
        let timeout = for_load(&failure("browser-navigation-timeout"), UiLanguage::English);
        assert_eq!(timeout.title, "This page is taking too long");
        assert_ne!(unknown.title, timeout.title);
    }

    #[test]
    fn loading_and_ready_are_distinct_non_failure_states() {
        let idle = for_load(&PageLoad::Idle, UiLanguage::English);
        let loading = for_load(
            &PageLoad::Loading {
                url: "http://localhost/preview".into(),
            },
            UiLanguage::English,
        );
        let ready = for_load(
            &PageLoad::Ready {
                url: "http://localhost/preview".into(),
            },
            UiLanguage::English,
        );
        assert!(!idle.failed && !loading.failed && !ready.failed);
        assert_eq!(loading.title, "Opening page…");
        assert_eq!(ready.title, "Page ready");
        assert_ne!(idle.title, loading.title);
    }

    #[test]
    fn recovery_actions_and_failure_copy_follow_the_interface_language() {
        for (language, title, retry, edit) in [
            (
                UiLanguage::English,
                "Unable to open this page",
                "Retry",
                "Edit address",
            ),
            (
                UiLanguage::ChineseSimplified,
                "无法打开此网页",
                "重试",
                "编辑地址",
            ),
            (
                UiLanguage::ChineseTraditional,
                "無法開啟此網頁",
                "重試",
                "編輯網址",
            ),
        ] {
            let feedback = for_load(&failure("unknown"), language);
            assert_eq!(feedback.title, title);
            assert!(feedback.failed);
            assert_eq!(retry_label(language), retry);
            assert_eq!(edit_label(language), edit);
        }
    }
}
