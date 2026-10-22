//! Built-in browser tab shell.
//!
//! One browser tab is one panel (1:1). A later leaf may put several panels
//! inside one tab; this module does not. The wry surface is not linked here:
//! the tab is a peer of a terminal tab in the same strip, and page loads stay
//! a follow-up. There is no system tray.
//!
//! Proxy is a settings hook only. Environment wins over `minicon.json`. The
//! settings panel does not edit these keys yet.

/// Hard cap so a hand-edited config cannot hand the future network stack an
/// unbounded header.
const PROXY_VALUE_MAX_BYTES: usize = 512;

/// The single content panel a browser tab hosts today.
///
/// This is the tab's own page surface. It is not the chrome sent-history
/// panel on the right of the window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrowserPanel {
    pub title: String,
    /// http(s) handed off from the terminal. Empty until a link is opened.
    pub url: Option<String>,
}

/// A browser tab: exactly one panel, same lifecycle as a terminal tab.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrowserTab {
    panel: BrowserPanel,
}

impl BrowserTab {
    pub fn shell() -> Self {
        Self {
            panel: BrowserPanel {
                title: "browser".to_owned(),
                url: None,
            },
        }
    }

    pub fn panel(&self) -> &BrowserPanel {
        &self.panel
    }

    /// Replace the one content panel's URL. Does not add a second panel.
    pub fn show_http(&mut self, url: &str) {
        self.panel.url = Some(url.to_owned());
    }

    /// The current model is 1 tab ↔ 1 panel. This stays 1 until the layout
    /// engine exists.
    pub fn panel_count(&self) -> usize {
        1
    }
}

/// Proxy values read from `minicon.json` before environment overrides.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BrowserProxyConfig {
    pub http: Option<String>,
    pub https: Option<String>,
    pub no_proxy: Option<String>,
}

/// The value a future wry network session must use. `None` means direct.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BrowserProxy {
    pub http: Option<String>,
    pub https: Option<String>,
    pub no_proxy: Option<String>,
}

impl BrowserProxy {
    /// One line for the tab shell. Empty when no hook is set.
    pub fn summary(&self) -> String {
        match (&self.http, &self.https, &self.no_proxy) {
            (None, None, None) => "proxy: direct".to_owned(),
            (http, https, no_proxy) => format!(
                "proxy: http={} https={} no_proxy={}",
                http.as_deref().unwrap_or("-"),
                https.as_deref().unwrap_or("-"),
                no_proxy.as_deref().unwrap_or("-"),
            ),
        }
    }
}

/// `MINICON_BROWSER_*` overrides the file. An empty or illegal value is absent.
pub fn resolve_proxy(
    config: &BrowserProxyConfig,
    env_lookup: impl Fn(&str) -> Option<String>,
) -> BrowserProxy {
    BrowserProxy {
        http: env_lookup("MINICON_BROWSER_HTTP_PROXY")
            .and_then(usable_proxy_value)
            .or_else(|| config.http.clone().and_then(usable_proxy_value)),
        https: env_lookup("MINICON_BROWSER_HTTPS_PROXY")
            .and_then(usable_proxy_value)
            .or_else(|| config.https.clone().and_then(usable_proxy_value)),
        no_proxy: env_lookup("MINICON_BROWSER_NO_PROXY")
            .and_then(usable_proxy_value)
            .or_else(|| config.no_proxy.clone().and_then(usable_proxy_value)),
    }
}

pub fn resolve_proxy_from_process(config: &BrowserProxyConfig) -> BrowserProxy {
    resolve_proxy(config, |key| std::env::var(key).ok())
}

/// An http(s) token under `char_index` on one terminal row.
///
/// `Some` means the product opens that URL in a browser tab. `None` means
/// this click is not a terminal http link: the caller must not call
/// `host_open` / `xdg-open` either. Non-http schemes stay out of this leaf.
pub fn http_url_at(line: &str, char_index: usize) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let index = char_index.min(chars.len() - 1);
    if chars[index].is_whitespace() {
        return None;
    }
    let mut start = index;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }
    let mut end = index + 1;
    while end < chars.len() && !chars[end].is_whitespace() {
        end += 1;
    }
    while end > start && matches!(chars[end - 1], '.' | ',' | ';' | ')' | ']' | '"' | '\'') {
        end -= 1;
    }
    let token: String = chars[start..end].iter().collect();
    if token.len() > 2048 {
        return None;
    }
    let lower = token.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        Some(token)
    } else {
        None
    }
}

fn usable_proxy_value(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > PROXY_VALUE_MAX_BYTES {
        return None;
    }
    if trimmed.chars().any(|character| character.is_control()) {
        return None;
    }
    Some(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_browser_tab_hosts_exactly_one_panel() {
        let mut tab = BrowserTab::shell();
        tab.show_http("https://example.test/a");
        assert_eq!(tab.panel_count(), 1);
        assert_eq!(tab.panel().url.as_deref(), Some("https://example.test/a"));
    }

    #[test]
    fn http_links_are_browser_urls_and_other_schemes_are_not_opened() {
        let line = "see https://example.test/a), mailto:a@b.test and ./file";
        let at = line.find("https").unwrap();
        assert_eq!(
            http_url_at(line, at + 4).as_deref(),
            Some("https://example.test/a")
        );
        let mailto = line.find("mailto").unwrap();
        assert_eq!(http_url_at(line, mailto), None);
        assert_eq!(http_url_at(line, line.find("./file").unwrap()), None);
        assert_eq!(
            http_url_at("http://ok.test", 0).as_deref(),
            Some("http://ok.test")
        );
    }

    #[test]
    fn environment_overrides_config_and_blanks_are_absent() {
        let config = BrowserProxyConfig {
            http: Some("http://from-file:8080".into()),
            https: Some("  ".into()),
            no_proxy: Some("localhost".into()),
        };
        let proxy = resolve_proxy(&config, |key| match key {
            "MINICON_BROWSER_HTTP_PROXY" => Some("http://from-env:9".into()),
            "MINICON_BROWSER_HTTPS_PROXY" => Some("".into()),
            _ => None,
        });
        assert_eq!(proxy.http.as_deref(), Some("http://from-env:9"));
        assert_eq!(proxy.https, None);
        assert_eq!(proxy.no_proxy.as_deref(), Some("localhost"));
    }

    #[test]
    fn control_characters_and_overlong_values_are_dropped() {
        let config = BrowserProxyConfig {
            http: Some("http://ok\nproxy".into()),
            https: Some("h".repeat(PROXY_VALUE_MAX_BYTES + 1)),
            no_proxy: None,
        };
        let proxy = resolve_proxy(&config, |_| None);
        assert_eq!(proxy, BrowserProxy::default());
        assert_eq!(proxy.summary(), "proxy: direct");
    }
}
