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
    ///
    /// Only a canonical http(s) token is stored. Anything else leaves the
    /// panel unchanged and returns false. This is not an OS browser open.
    pub fn show_http(&mut self, url: &str) -> bool {
        let Some(canonical) = http_url_at(url, 0) else {
            return false;
        };
        if canonical != url {
            return false;
        }
        self.panel.url = Some(canonical);
        true
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

const SYSTEM_OPENERS: &[&str] = &[
    "xdg-open",
    "open",
    "gio",
    "gtk-launch",
    "explorer",
    "cmd",
    "start",
];

fn is_system_opener(program: &std::path::Path) -> bool {
    program
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| SYSTEM_OPENERS.contains(&name))
}

/// The wry host for the one content panel.
///
/// `explicit` is `MINICON_BROWSER_BIN`. When it is unset, the sibling
/// `minicon-web-host` next to `current_exe` is used if that file exists.
/// A system opener is never selected.
pub fn page_host_program(
    explicit: Option<&std::path::Path>,
    current_exe: &std::path::Path,
) -> Option<std::path::PathBuf> {
    if let Some(program) = explicit {
        if is_system_opener(program) {
            return None;
        }
        return Some(program.to_path_buf());
    }
    let sibling = current_exe.parent()?.join("minicon-web-host");
    if sibling.is_file() && !is_system_opener(&sibling) {
        Some(sibling)
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
    if is_system_opener(program) {
        return Err("system-browser");
    }
    Ok((program.to_path_buf(), vec!["--load".to_owned(), canonical]))
}

/// Where the one content panel sits inside the main window, in parent pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PanelSlot {
    pub parent: u64,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// `--load` argv, plus the main-window slot when the GUI has an embed parent.
pub fn page_host_embed_args(
    url: &str,
    slot: Option<PanelSlot>,
) -> Result<Vec<String>, &'static str> {
    let canonical = http_url_at(url, 0).ok_or("not-http")?;
    if canonical != url {
        return Err("not-http");
    }
    let mut args = vec!["--load".to_owned(), canonical];
    if let Some(slot) = slot {
        args.push("--parent".to_owned());
        args.push(slot.parent.to_string());
        args.push("--bounds".to_owned());
        args.push(format!(
            "{},{},{},{}",
            slot.x, slot.y, slot.width, slot.height
        ));
    }
    Ok(args)
}

fn parse_slot(parent: &str, bounds: &str) -> Result<PanelSlot, &'static str> {
    let parent = parent.parse::<u64>().map_err(|_| "usage")?;
    let mut parts = bounds.split(',');
    let x = parts
        .next()
        .ok_or("usage")?
        .parse::<i32>()
        .map_err(|_| "usage")?;
    let y = parts
        .next()
        .ok_or("usage")?
        .parse::<i32>()
        .map_err(|_| "usage")?;
    let width = parts
        .next()
        .ok_or("usage")?
        .parse::<u32>()
        .map_err(|_| "usage")?;
    let height = parts
        .next()
        .ok_or("usage")?
        .parse::<u32>()
        .map_err(|_| "usage")?;
    if parts.next().is_some() || width == 0 || height == 0 {
        return Err("usage");
    }
    Ok(PanelSlot {
        parent,
        x,
        y,
        width,
        height,
    })
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
    spawn_page_host_in_panel(program, url, proxy, None)
}

/// Spawn `--load` for the one content panel. `slot` asks the host to embed
/// in the main window. Still does not call `host_open`.
pub fn spawn_page_host_in_panel(
    program: &std::path::Path,
    url: &str,
    proxy: Option<&BrowserProxy>,
    slot: Option<PanelSlot>,
) -> Result<std::process::Child, String> {
    let (program, mut args) = page_host_request(program, url).map_err(str::to_owned)?;
    if let Some(slot) = slot {
        args.extend(
            page_host_embed_args(url, Some(slot))
                .map_err(str::to_owned)?
                .into_iter()
                .skip(2),
        );
    }
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
    for key in webkit_proxy_clear(proxy) {
        command.env_remove(key);
    }
    for (key, value) in webkit_proxy_env(proxy) {
        command.env(key, value);
    }
}

/// Environment pairs a WebKit/wry backend should see for this hook.
///
/// `MINICON_BROWSER_*` stays the product key. This maps a resolved hook onto
/// the names the system WebView already reads. It does not open a browser.
pub fn webkit_proxy_env(proxy: &BrowserProxy) -> Vec<(&'static str, String)> {
    let mut pairs = Vec::new();
    if let Some(value) = &proxy.http {
        pairs.push(("http_proxy", value.clone()));
    }
    if let Some(value) = &proxy.https {
        pairs.push(("https_proxy", value.clone()));
    }
    if let Some(value) = &proxy.no_proxy {
        pairs.push(("no_proxy", value.clone()));
    }
    pairs
}

/// WebKit proxy names the resolved hook does not set. A wry host must remove
/// these so a parent `http_proxy` cannot leak into the content panel.
pub fn webkit_proxy_clear(proxy: &BrowserProxy) -> Vec<&'static str> {
    // Upper-case and all_proxy are not the product hook. Always drop them so
    // a parent shell cannot override the content panel.
    let mut clear = vec![
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
    ];
    if proxy.http.is_none() {
        clear.push("http_proxy");
    }
    if proxy.https.is_none() {
        clear.push("https_proxy");
    }
    if proxy.no_proxy.is_none() {
        clear.push("no_proxy");
    }
    clear
}

/// A finished page-host status. Exit 69 is the Unavailable contract.
/// `None` means the host is still running.
pub fn page_load_from_exit(url: &str, code: Option<i32>) -> PageLoad {
    match code {
        None => PageLoad::Loading {
            url: url.to_owned(),
        },
        Some(69) => PageLoad::Unavailable {
            url: url.to_owned(),
            reason: "webkit-host-not-linked".to_owned(),
        },
        Some(code) => PageLoad::Unavailable {
            url: url.to_owned(),
            reason: format!("host-exit-{code}"),
        },
    }
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

/// True only when this build was compiled with `--features wry-host`.
/// The default build, including core `minicon`, is false.
pub fn webview_backend_linked() -> bool {
    cfg!(feature = "wry-host")
}

/// HTTPS authorize URL for the built-in content panel.
///
/// `redirect_uri` must be loopback or https, and its host must be in
/// `allowed_hosts`. The result is what `--load` should open. This does not
/// store a token and does not call the OS browser. DeepSeek OAuth uses the
/// same path as any other allowlisted provider.
pub fn oauth_authorize_load(
    authorize_url: &str,
    redirect_uri: &str,
    allowed_hosts: &[&str],
) -> Result<String, &'static str> {
    let authorize = http_url_at(authorize_url, 0).ok_or("not-http")?;
    if authorize != authorize_url || !authorize.to_ascii_lowercase().starts_with("https://") {
        return Err("not-https");
    }
    let redirect_host = redirect_host(redirect_uri).ok_or("redirect-rejected")?;
    if !allowed_hosts
        .iter()
        .any(|host| host.eq_ignore_ascii_case(&redirect_host))
    {
        return Err("redirect-rejected");
    }
    let encoded = percent_encode(redirect_uri);
    if authorize.contains("redirect_uri=") {
        return Ok(authorize);
    }
    let join = if authorize.contains('?') { '&' } else { '?' };
    Ok(format!("{authorize}{join}redirect_uri={encoded}"))
}

fn redirect_host(redirect_uri: &str) -> Option<String> {
    let lower = redirect_uri.to_ascii_lowercase();
    let rest = if let Some(rest) = lower.strip_prefix("https://") {
        rest
    } else if let Some(rest) = lower.strip_prefix("http://") {
        let host = rest.split(['/', '?', '#']).next().unwrap_or("");
        let host = host.split('@').next_back().unwrap_or(host);
        let host = host.split(':').next().unwrap_or(host);
        if host == "127.0.0.1" || host == "localhost" {
            return Some(host.to_owned());
        }
        return None;
    } else {
        return None;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.split('@').next_back().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty() {
        None
    } else {
        Some(host.to_owned())
    }
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Reply from `minicon-web-host`.
///
/// `--probe` and `--load` stay Unavailable (exit 69) unless `wry-host` is
/// on and the system WebView accepts the call. Neither path opens the OS browser.
pub fn host_reply(args: &[String]) -> Result<(), &'static str> {
    match args {
        [flag] if flag == "--probe" => probe_webview(),
        [flag, url] if flag == "--load" || flag == "--smoke" => {
            let _ = http_url_at(url, 0)
                .filter(|canonical| canonical == url)
                .ok_or("not-http")?;
            load_webview(url, flag == "--smoke", None)
        }
        [flag, url, parent_flag, parent, bounds_flag, bounds]
            if flag == "--load" && parent_flag == "--parent" && bounds_flag == "--bounds" =>
        {
            let _ = http_url_at(url, 0)
                .filter(|canonical| canonical == url)
                .ok_or("not-http")?;
            let slot = parse_slot(parent, bounds)?;
            load_webview(url, false, Some(slot))
        }
        _ => Err("usage"),
    }
}

fn probe_webview() -> Result<(), &'static str> {
    #[cfg(feature = "wry-host")]
    {
        wry_host::probe()
    }
    #[cfg(not(feature = "wry-host"))]
    {
        Err("webkit-host-not-linked")
    }
}

fn load_webview(url: &str, smoke: bool, slot: Option<PanelSlot>) -> Result<(), &'static str> {
    #[cfg(feature = "wry-host")]
    {
        wry_host::load(url, smoke, slot)
    }
    #[cfg(not(feature = "wry-host"))]
    {
        let _ = (url, smoke, slot);
        Err("webkit-host-not-linked")
    }
}

/// `http://host:port` from the resolved hook, for wry `ProxyConfig::Http`.
/// Socks and userinfo are refused. `None` means a direct WebView load.
pub fn wry_proxy_endpoint(proxy: &BrowserProxy) -> Option<(String, String)> {
    let raw = proxy.http.as_deref().or(proxy.https.as_deref())?;
    let rest = raw
        .trim()
        .strip_prefix("http://")
        .or_else(|| raw.trim().strip_prefix("https://"))?;
    if rest.contains('@') {
        return None;
    }
    let (host, port) = rest.split_once(':')?;
    let port = port.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() || port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((host.to_owned(), port.to_owned()))
}

#[cfg(feature = "wry-host")]
mod wry_host;

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
        assert!(tab.show_http("https://example.test/a"));
        assert!(!tab.show_http("mailto:a@b.test"));
        assert!(!tab.show_http("https://example.test/a)"));
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
    fn page_host_program_is_the_panel_helper_not_the_system_browser() {
        assert_eq!(
            page_host_program(
                Some(std::path::Path::new("/usr/bin/xdg-open")),
                std::path::Path::new("/opt/minicon")
            ),
            None
        );
        assert_eq!(
            page_host_program(
                Some(std::path::Path::new("/opt/minicon-web-host")),
                std::path::Path::new("/opt/minicon")
            )
            .as_deref(),
            Some(std::path::Path::new("/opt/minicon-web-host"))
        );
        let dir = std::env::temp_dir().join(format!("minicon-host-bin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("minicon");
        let host = dir.join("minicon-web-host");
        std::fs::write(&exe, b"exe").unwrap();
        std::fs::write(&host, b"host").unwrap();
        assert_eq!(
            page_host_program(None, &exe).as_deref(),
            Some(host.as_path())
        );
        let _ = std::fs::remove_dir_all(&dir);
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
            "#!/bin/sh\nprintf '%s|%s|%s|%s|%s\\n' \"$MINICON_BROWSER_HTTP_PROXY\" \"$MINICON_BROWSER_HTTPS_PROXY\" \"$MINICON_BROWSER_NO_PROXY\" \"$HTTP_PROXY\" \"$http_proxy\" > '{}'\n",
            log.display()
        );
        std::fs::write(&program, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        struct ClearParentHttps;
        impl Drop for ClearParentHttps {
            fn drop(&mut self) {
                unsafe { std::env::remove_var("MINICON_BROWSER_HTTPS_PROXY") };
                unsafe { std::env::remove_var("HTTP_PROXY") };
            }
        }
        unsafe { std::env::set_var("MINICON_BROWSER_HTTPS_PROXY", "http://leak.test:9") };
        unsafe { std::env::set_var("HTTP_PROXY", "http://leak.test:9") };
        let _clear = ClearParentHttps;
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
            "http://127.0.0.1:9||localhost||http://127.0.0.1:9"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exit_69_is_unavailable_and_proxy_maps_onto_webkit_names() {
        assert!(matches!(
            page_load_from_exit("https://example.test/a", Some(69)),
            PageLoad::Unavailable { reason, .. } if reason == "webkit-host-not-linked"
        ));
        assert!(matches!(
            page_load_from_exit("https://example.test/a", None),
            PageLoad::Loading { .. }
        ));
        let pairs = webkit_proxy_env(&BrowserProxy {
            http: Some("http://127.0.0.1:9".into()),
            https: None,
            no_proxy: Some("localhost".into()),
        });
        assert_eq!(
            pairs,
            vec![
                ("http_proxy", "http://127.0.0.1:9".into()),
                ("no_proxy", "localhost".into()),
            ]
        );
        assert_eq!(
            webkit_proxy_clear(&BrowserProxy {
                http: Some("http://127.0.0.1:9".into()),
                https: None,
                no_proxy: Some("localhost".into()),
            }),
            vec![
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "all_proxy",
                "NO_PROXY",
                "https_proxy",
            ]
        );
        assert_eq!(
            wry_proxy_endpoint(&BrowserProxy {
                http: Some("http://127.0.0.1:9".into()),
                https: None,
                no_proxy: None,
            }),
            Some(("127.0.0.1".into(), "9".into()))
        );
        assert_eq!(
            wry_proxy_endpoint(&BrowserProxy {
                http: Some("socks5://127.0.0.1:9".into()),
                https: None,
                no_proxy: None,
            }),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_host_that_exits_69_is_unavailable_not_a_system_browser() {
        let dir = std::env::temp_dir().join(format!("minicon-exit69-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join("minicon-page-host");
        std::fs::write(&program, "#!/bin/sh\nexit 69\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut child = spawn_page_host(&program, "https://example.test/a").unwrap();
        let status = child.wait().unwrap();
        assert!(matches!(
            page_load_from_exit("https://example.test/a", status.code()),
            PageLoad::Unavailable { reason, .. } if reason == "webkit-host-not-linked"
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deepseek_oauth_authorize_loads_in_the_content_panel_only() {
        let url = oauth_authorize_load(
            "https://platform.deepseek.com/oauth/authorize",
            "http://127.0.0.1/callback",
            &["127.0.0.1"],
        )
        .unwrap();
        assert!(url.starts_with("https://platform.deepseek.com/oauth/authorize?"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%2Fcallback"));
        let (_program, args) =
            page_host_request(std::path::Path::new("/opt/minicon-web-host"), &url).unwrap();
        assert_eq!(args[0], "--load");
        assert_eq!(args[1], url);
        let embedded = page_host_embed_args(
            &url,
            Some(PanelSlot {
                parent: 7,
                x: 40,
                y: 0,
                width: 200,
                height: 100,
            }),
        )
        .unwrap();
        assert_eq!(
            embedded,
            vec![
                "--load".to_owned(),
                url.clone(),
                "--parent".to_owned(),
                "7".to_owned(),
                "--bounds".to_owned(),
                "40,0,200,100".to_owned(),
            ]
        );
        assert_eq!(host_reply(&embedded), Err("webkit-host-not-linked"));
        assert_eq!(
            oauth_authorize_load(
                "https://platform.deepseek.com/oauth/authorize",
                "https://evil.test/callback",
                &["127.0.0.1"]
            ),
            Err("redirect-rejected")
        );
        assert_eq!(
            oauth_authorize_load(
                "mailto:a@b.test",
                "http://127.0.0.1/callback",
                &["127.0.0.1"]
            ),
            Err("not-http")
        );
        assert_eq!(
            host_reply(&["--load".into(), url.clone()]),
            Err("webkit-host-not-linked")
        );
        assert_eq!(
            host_reply(&["--smoke".into(), url]),
            Err("webkit-host-not-linked")
        );
    }

    #[test]
    fn wry_host_source_loads_with_wry_and_not_the_os_browser() {
        let source = include_str!("wry_host.rs");
        assert!(source.contains(".with_url(url)"));
        assert!(source.contains("webkit_proxy_env"));
        assert!(source.contains("webkit_proxy_clear"));
        assert!(source.contains("with_proxy_config"));
        assert!(!source.contains("xdg-open"));
        assert!(!source.contains("host_open"));
    }

    #[test]
    fn default_host_reports_unavailable_without_a_webview() {
        assert!(!webview_backend_linked());
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
