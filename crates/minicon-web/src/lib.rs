//! Browser content-panel API (`minicon-web`).
//!
//! Product face stays a **tab** and its **content panel** (1:1 now, 1:N later).
//! This crate does not own the tab tree or the chrome sent-history / settings
//! panels, and it has no system tray.
//!
//! # Boundary
//!
//! [`PageHost`] is the swappable surface: [`ProcessPageHost`] speaks
//! `--load <url>` to a helper process today. A later wry/WebView backend
//! implements the same trait inside the host binary (`minicon-web-host`).
//! Core `minicon` depends on this crate and does **not** link wry or tao.
//!
//! Stable names: [`BrowserTab`], [`BrowserPanel`], [`PageLoad`], [`PageHost`],
//! [`http_url_at`], [`resolve_proxy`], [`page_host_request`], [`spawn_page_host`].
//!
//! Proxy is a settings hook only. Environment wins over `minicon.json`.
//! The hook is not applied to a WebView session yet. OAuth, dashboard,
//! preview and wasm are later [`PageHost`] callers, not extra traits.

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
    /// What the content panel last asked the page host to do.
    ///
    /// The wry WebView lives in that host process, not in this one. `Idle`
    /// means no navigation has been requested.
    pub load: PageLoad,
}

/// A navigation the content panel asked for. Not a chrome-panel state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PageLoad {
    Idle,
    /// `program --load url` was spawned. The page itself is in that process.
    Loading {
        url: String,
    },
    /// No host, or the host refused. The OS browser is not a fallback.
    Unavailable {
        url: String,
        reason: String,
    },
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
                load: PageLoad::Idle,
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

    pub fn note_load(&mut self, load: PageLoad) {
        self.panel.load = load;
    }

    /// One line for the content panel. The chrome sent-history panel does not use this.
    pub fn load_summary(&self) -> String {
        match &self.panel.load {
            PageLoad::Idle => "page: idle".to_owned(),
            PageLoad::Loading { url } => format!("page: loading {url}"),
            PageLoad::Unavailable { url, reason } => {
                format!("page: unavailable ({reason}) {url}")
            }
        }
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

/// Argv for the wry page host. The program must be the helper, never the
/// system opener (`xdg-open`, `open`, `gio`, `gtk-launch`).
pub fn page_host_request(
    program: &std::path::Path,
    url: &str,
) -> Result<(std::path::PathBuf, Vec<String>), &'static str> {
    let canonical = http_url_at(url, 0).ok_or("not-http")?;
    if canonical != url {
        return Err("not-http");
    }
    let name = program
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("host-name")?;
    if matches!(
        name,
        "xdg-open" | "open" | "gio" | "gtk-launch" | "explorer" | "cmd" | "start"
    ) {
        return Err("system-browser");
    }
    Ok((program.to_path_buf(), vec!["--load".to_owned(), canonical]))
}

/// Spawn the page host. Does not wait, and does not call `host_open`.
/// The child inherits the parent environment, including any proxy hook.
pub fn spawn_page_host(
    program: &std::path::Path,
    url: &str,
) -> Result<std::process::Child, String> {
    spawn_page_host_with_proxy(program, url, None)
}

/// Spawn the page host and install `proxy` as `MINICON_BROWSER_*` on that
/// process only. `None` leaves the inherited environment alone. A set field
/// is written; an absent field is removed so a parent value cannot leak in.
/// Still does not call `host_open`, and does not configure a WebView.
pub fn spawn_page_host_with_proxy(
    program: &std::path::Path,
    url: &str,
    proxy: Option<&BrowserProxy>,
) -> Result<std::process::Child, String> {
    let (program, args) = page_host_request(program, url).map_err(str::to_owned)?;
    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Some(proxy) = proxy {
        apply_proxy_env(&mut command, proxy);
    }
    command.spawn().map_err(|error| error.to_string())
}

fn apply_proxy_env(command: &mut std::process::Command, proxy: &BrowserProxy) {
    apply_proxy_var(command, "MINICON_BROWSER_HTTP_PROXY", &proxy.http);
    apply_proxy_var(command, "MINICON_BROWSER_HTTPS_PROXY", &proxy.https);
    apply_proxy_var(command, "MINICON_BROWSER_NO_PROXY", &proxy.no_proxy);
}

fn apply_proxy_var(command: &mut std::process::Command, key: &str, value: &Option<String>) {
    match value {
        Some(value) => {
            command.env(key, value);
        }
        None => {
            command.env_remove(key);
        }
    }
}

/// Swappable page surface for one content panel.
///
/// A backend may be a child process or, later, an in-process WebView.
/// Both must refuse the OS browser and must not create a tray icon.
pub trait PageHost {
    fn open_url(&mut self, url: &str) -> Result<PageLoad, &'static str>;
}

/// Today's backend: `program --load url`. The child is kept so a new load
/// replaces the previous one.
pub struct ProcessPageHost {
    program: std::path::PathBuf,
    child: Option<std::process::Child>,
}

impl ProcessPageHost {
    pub fn new(program: impl Into<std::path::PathBuf>) -> Self {
        Self {
            program: program.into(),
            child: None,
        }
    }
}

impl PageHost for ProcessPageHost {
    fn open_url(&mut self, url: &str) -> Result<PageLoad, &'static str> {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        match spawn_page_host(&self.program, url) {
            Ok(child) => {
                self.child = Some(child);
                Ok(PageLoad::Loading {
                    url: url.to_owned(),
                })
            }
            Err(_) => Err("spawn-failed"),
        }
    }
}

impl Drop for ProcessPageHost {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Reply from `minicon-web-host`. No WebView is linked in the default host,
/// so `--probe` and `--load` are `Unavailable` (exit 69) instead of opening
/// a system browser.
pub fn host_reply(args: &[String]) -> Result<(), &'static str> {
    match args {
        [flag] if flag == "--probe" => Err("webkit-host-not-linked"),
        [flag, url] if flag == "--load" => {
            let _ = http_url_at(url, 0)
                .filter(|canonical| canonical == url)
                .ok_or("not-http")?;
            Err("webkit-host-not-linked")
        }
        _ => Err("usage"),
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
    fn page_host_request_is_the_helper_not_the_system_browser() {
        let (program, args) = page_host_request(
            std::path::Path::new("/opt/minicon-browser"),
            "https://example.test/a",
        )
        .unwrap();
        assert_eq!(program, std::path::Path::new("/opt/minicon-browser"));
        assert_eq!(args, ["--load", "https://example.test/a"]);
        assert_eq!(
            page_host_request(
                std::path::Path::new("/usr/bin/xdg-open"),
                "https://example.test/a"
            ),
            Err("system-browser")
        );
        assert_eq!(
            page_host_request(
                std::path::Path::new("/opt/minicon-browser"),
                "mailto:a@b.test"
            ),
            Err("not-http")
        );
    }

    #[cfg(unix)]
    #[test]
    fn spawn_page_host_passes_the_url_to_the_helper() {
        let dir = std::env::temp_dir().join(format!("minicon-page-host-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("log");
        let program = dir.join("minicon-page-host");
        let script = format!("#!/bin/sh\nprintf '%s\\n' \"$2\" > '{}'\n", log.display());
        std::fs::write(&program, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut child = spawn_page_host(&program, "https://example.test/a").unwrap();
        let status = child.wait().unwrap();
        assert!(status.success());
        assert_eq!(
            std::fs::read_to_string(&log).unwrap().trim(),
            "https://example.test/a"
        );
        assert!(spawn_page_host(&program, "mailto:a@b.test").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn spawn_page_host_installs_the_proxy_hook_on_the_child() {
        let dir = std::env::temp_dir().join(format!("minicon-proxy-host-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("log");
        let program = dir.join("minicon-page-host");
        let script = format!(
            "#!/bin/sh\nprintf '%s|%s|%s\\n' \"$MINICON_BROWSER_HTTP_PROXY\" \"$MINICON_BROWSER_HTTPS_PROXY\" \"$MINICON_BROWSER_NO_PROXY\" > '{}'\n",
            log.display()
        );
        std::fs::write(&program, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let proxy = BrowserProxy {
            http: Some("http://127.0.0.1:9".into()),
            https: None,
            no_proxy: Some("localhost".into()),
        };
        let mut child =
            spawn_page_host_with_proxy(&program, "https://example.test/a", Some(&proxy)).unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(
            std::fs::read_to_string(&log).unwrap().trim(),
            "http://127.0.0.1:9||localhost"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_host_reports_unavailable_without_a_webview() {
        assert_eq!(
            host_reply(&["--probe".into()]),
            Err("webkit-host-not-linked")
        );
        assert_eq!(
            host_reply(&["--load".into(), "https://example.test/a".into()]),
            Err("webkit-host-not-linked")
        );
        assert_eq!(
            host_reply(&["--load".into(), "mailto:a@b.test".into()]),
            Err("not-http")
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
