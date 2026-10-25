//! Browser content-panel API (`minicon-web`).
//!
//! Product face stays a **tab** and its **content panel** (1:1 now, 1:N later).
//! This crate does not own the tab tree or the chrome sent-history / settings
//! panels, and it has no system tray.
//!
//! # Boundary
//!
//! [`PageHost`] is the swappable surface: [`ProcessPageHost`] speaks
//! `--load <url>` to a helper process. On macOS/Windows, feature `native-page-host`
//! owns a same-process child WKWebView/WebView2 for the GUI binary while core/headless
//! consumers stay WebView-free. Feature `wry-host` retains the Linux helper.
//!
//! Stable names: [`BrowserTab`], [`BrowserPanel`], [`PageLoad`], [`PageHost`],
//! [`http_url_at`], [`resolve_proxy`], [`page_host_request`], [`spawn_page_host`].
//!
//! Proxy mechanisms: `prd/PRD_02_28_shared_core.md`.
//! Product editing and saved choices: `prd/PRD_02_25_01_browser_preferences.md`.
//! The PRD series owns staged callers and product non-goals.

/// Hard cap so a hand-edited config cannot hand the future network stack an
/// unbounded header.
const PROXY_VALUE_MAX_BYTES: usize = 512;

mod profile;
mod session;
pub use profile::{PageContext, PageContextError, PageProfile};
pub use session::{
    NodeReference, PageCommand, PageError, PageSession, PageState, PageView, SemanticNode,
};

/// A native lifetime observation, independent of storage persistence or deletion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativePageReleaseStatus {
    Pending,
    Released,
    Failed(&'static str),
}

#[cfg(target_os = "linux")]
mod embedded_program;
#[cfg(target_os = "linux")]
pub use embedded_program::EmbeddedPageProgram;

#[cfg(all(feature = "native-page-host", any(target_os = "macos", windows)))]
mod native_page;
#[cfg(all(feature = "native-page-host", any(target_os = "macos", windows)))]
pub use native_page::NativePageHost;
#[cfg(all(feature = "native-page-host", windows))]
pub use native_page::NativePageRelease;
#[cfg(all(
    windows,
    feature = "native-page-host",
    feature = "native-profile-diagnostics"
))]
pub use native_page::{
    NativeCookieMetadataCounts, NativeCookieMetadataProbe, NativeCookieMetadataStatus,
    NativeCookieMetadataSummary,
};

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
    /// The wry WebView lives behind this crate's native/process host boundary,
    /// never in core/headless callers. `Idle` means no navigation was requested.
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
    Ready {
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
        match &load {
            PageLoad::Loading { url }
            | PageLoad::Ready { url }
            | PageLoad::Unavailable { url, .. }
                if !url.is_empty() =>
            {
                self.panel.url = Some(url.clone());
            }
            _ => {}
        }
        self.panel.load = load;
    }

    pub fn note_title(&mut self, title: &str) {
        self.panel.title = title
            .chars()
            .filter(|c| !c.is_control())
            .take(256)
            .collect();
    }

    /// One line for the content panel. The chrome sent-history panel does not use this.
    pub fn load_summary(&self) -> String {
        match &self.panel.load {
            PageLoad::Idle => "page: idle".to_owned(),
            PageLoad::Loading { url } => format!("page: loading {url}"),
            PageLoad::Ready { url } => format!("page: ready {url}"),
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

/// A temporary browser-field override. Empty input retains the resolved
/// config/environment hook; an explicit HTTP proxy is used for both URL schemes
/// without inheriting a bypass list that could silently route pages directly.
/// Authentication, TLS-to-proxy, paths and browser switches are not supported.
pub fn proxy_override(base: &BrowserProxy, input: &str) -> Result<BrowserProxy, String> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(base.clone());
    }
    let (host, port) =
        parse_http_proxy_endpoint(input).ok_or_else(|| "browser-proxy-invalid".to_owned())?;
    let endpoint = format!("http://{host}:{port}");
    Ok(BrowserProxy {
        http: Some(endpoint.clone()),
        https: Some(endpoint),
        no_proxy: None,
    })
}

fn parse_http_proxy_endpoint(raw: &str) -> Option<(String, String)> {
    let raw = raw.trim();
    if raw.len() > 2048 {
        return None;
    }
    let authority = raw.strip_prefix("http://")?;
    let authority = authority.strip_suffix('/').unwrap_or(authority);
    let (host, port) = authority.rsplit_once(':')?;
    if host.is_empty()
        || !host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-".contains(&byte))
        || port.is_empty()
        || !port.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let port = port.parse::<u16>().ok().filter(|port| *port != 0)?;
    Some((host.to_owned(), port.to_string()))
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
    let ipc = slot.is_some();
    command
        .args(args)
        .stdin(if ipc {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(if ipc {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stderr(std::process::Stdio::null());
    if ipc {
        command.env("MINICON_BROWSER_HOST_IPC", "1");
    }
    if let Some(proxy) = proxy {
        apply_proxy_env(&mut command, proxy);
    }
    command.spawn().map_err(|error| error.to_string())
}

/// Start an explicitly isolated Linux page helper. Startup context travels on
/// bounded stdin, followed by the existing command stream; it never changes
/// process-global environment or uses legacy proxy/profile resolution.
pub fn spawn_page_host_in_context(
    program: &std::path::Path,
    url: &str,
    context: &PageContext,
    slot: Option<PanelSlot>,
) -> Result<std::process::Child, PageContextError> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (program, url, context, slot);
        Err(PageContextError::Unsupported(
            "browser-context-helper-unsupported",
        ))
    }
    #[cfg(target_os = "linux")]
    {
        use std::io::Write;
        let frame = context.frame()?;
        let (program, mut args) =
            page_host_request(program, url).map_err(|_| PageContextError::InvalidProfile)?;
        args[0] = "--load-context".to_owned();
        if let Some(slot) = slot {
            args.extend(
                page_host_embed_args(url, Some(slot))
                    .map_err(|_| PageContextError::InvalidProfile)?
                    .into_iter()
                    .skip(2),
            );
        }
        let mut child = std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(if slot.is_some() {
                std::process::Stdio::piped()
            } else {
                std::process::Stdio::null()
            })
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|_| PageContextError::NativeCreateFailed)?;
        // A fresh Linux pipe has at least PIPE_BUF (4096) bytes available;
        // this complete frame fits before the helper starts reading it.
        if child
            .stdin
            .as_mut()
            .is_none_or(|input| input.write_all(&frame).and_then(|_| input.flush()).is_err())
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(PageContextError::NativeCreateFailed);
        }
        Ok(child)
    }
}

/// True when an X window class is the page host we reparented.
pub fn embedded_page_class(name: &str) -> bool {
    name.eq_ignore_ascii_case("minicon-web-host")
}

/// Map or unmap the reparented page inside `parent`.
///
/// The host window stays a child of the main window. Hiding it is what lets
/// a terminal tab receive clicks again. No-op without X11. Does not open a
/// browser.
pub fn set_embedded_page_visible(parent: u64, visible: bool) {
    #[cfg(target_os = "linux")]
    unsafe {
        x_update_embedded_page(parent, None, Some(visible), None);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (parent, visible);
    }
}

/// Apply the current content-column bounds to the already-reparented page.
///
/// The child is found by its page-host X class, not by process-global state,
/// so tab activation and every layout owner can call the same seam. No-op on
/// a host without X11 or when the page has not reached the parent yet.
pub fn resize_embedded_page(slot: PanelSlot) {
    #[cfg(target_os = "linux")]
    unsafe {
        x_update_embedded_page(slot.parent, None, None, Some(slot));
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = slot;
    }
}

/// Submit visibility only to matching direct children of this parent/PID.
/// Missing or malformed native identity never falls back to class-only lookup.
/// `true` means at least one matching child's checked operation succeeded, not
/// that its page has rendered. Ownership and native evidence: PRD 28.
pub fn set_embedded_page_visible_for_process(parent: u64, pid: u32, visible: bool) -> bool {
    if parent == 0 || pid == 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    unsafe {
        x_update_embedded_page(parent, Some(pid), Some(visible), None)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = visible;
        false
    }
}

/// Submit bounds only to matching direct children of the owned process.
/// This does not authenticate arbitrary clients that forge X11 properties.
pub fn resize_embedded_page_for_process(slot: PanelSlot, pid: u32) -> bool {
    if slot.parent == 0 || pid == 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    unsafe {
        x_update_embedded_page(slot.parent, Some(pid), None, Some(slot))
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(any(target_os = "linux", test))]
fn embedded_process_property_matches(
    pid: u32,
    actual_type: u64,
    format: i32,
    items: u64,
    bytes_after: u64,
    value: Option<u64>,
) -> bool {
    // XA_CARDINAL is predefined atom 6. The transport normalizes its format-32
    // value before this platform-independent identity check.
    pid != 0
        && actual_type == 6
        && format == 32
        && items == 1
        && bytes_after == 0
        && value == Some(u64::from(pid))
}

#[cfg(target_os = "linux")]
unsafe fn x_update_embedded_page(
    parent: u64,
    pid: Option<u32>,
    visible: Option<bool>,
    slot: Option<PanelSlot>,
) -> bool {
    embedded_xcb::update(parent, pid, visible, slot)
}

#[cfg(any(target_os = "linux", test))]
fn embedded_class_property_matches(kind: u32, format: u8, bytes_after: u32, value: &[u8]) -> bool {
    if kind != 31 || format != 8 || bytes_after != 0 || value.len() > 512 {
        return false;
    }
    let fields: Vec<_> = value.split(|byte| *byte == 0).collect();
    fields.len() == 3
        && fields[2].is_empty()
        && fields[..2].iter().any(|field| {
            std::str::from_utf8(field)
                .ok()
                .is_some_and(embedded_page_class)
        })
}

#[cfg(target_os = "linux")]
mod embedded_xcb {
    use super::{PanelSlot, embedded_class_property_matches, embedded_process_property_matches};
    use std::{
        ffi::{CStr, c_char, c_int, c_void},
        ptr,
    };

    // ABI: public libxcb xcb.h/xproto.h; cookie/core calls also checked against
    // pinned x11rb 0.13.2 raw_ffi. Mechanism ownership/evidence: PRD 28.
    #[derive(Clone, Copy)]
    #[repr(C)]
    struct Cookie {
        _sequence: u32,
    }
    #[repr(C)]
    struct AtomReply {
        _response: u8,
        _pad: u8,
        _sequence: u16,
        _length: u32,
        atom: u32,
    }
    #[repr(C)]
    struct PropertyReply {
        _response: u8,
        format: u8,
        _sequence: u16,
        length: u32,
        kind: u32,
        bytes_after: u32,
        value_len: u32,
        _pad: [u8; 12],
    }
    type Connect = unsafe extern "C" fn(*const c_char, *mut c_int) -> *mut c_void;
    type Disconnect = unsafe extern "C" fn(*mut c_void);
    type HasError = unsafe extern "C" fn(*mut c_void) -> c_int;
    type Check = unsafe extern "C" fn(*mut c_void, Cookie) -> *mut c_void;
    type Query = unsafe extern "C" fn(*mut c_void, u32) -> Cookie;
    type QueryReply = unsafe extern "C" fn(*mut c_void, Cookie, *mut *mut c_void) -> *mut c_void;
    type Children = unsafe extern "C" fn(*const c_void) -> *mut u32;
    type ChildrenLength = unsafe extern "C" fn(*const c_void) -> c_int;
    type Intern = unsafe extern "C" fn(*mut c_void, u8, u16, *const c_char) -> Cookie;
    type InternReply =
        unsafe extern "C" fn(*mut c_void, Cookie, *mut *mut c_void) -> *mut AtomReply;
    type Property = unsafe extern "C" fn(*mut c_void, u8, u32, u32, u32, u32, u32) -> Cookie;
    type GetReply =
        unsafe extern "C" fn(*mut c_void, Cookie, *mut *mut c_void) -> *mut PropertyReply;
    type Value = unsafe extern "C" fn(*const PropertyReply) -> *mut c_void;
    type Configure = unsafe extern "C" fn(*mut c_void, u32, u16, *const c_void) -> Cookie;
    type Map = unsafe extern "C" fn(*mut c_void, u32) -> Cookie;
    unsafe extern "C" {
        fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
        fn dlclose(handle: *mut c_void) -> c_int;
        fn free(pointer: *mut c_void);
    }
    struct Library(*mut c_void);
    impl Drop for Library {
        fn drop(&mut self) {
            unsafe {
                dlclose(self.0);
            }
        }
    }
    impl Library {
        unsafe fn symbol<T>(&self, name: &CStr) -> Option<T> {
            let pointer = unsafe { dlsym(self.0, name.as_ptr()) };
            if pointer.is_null() {
                None
            } else {
                Some(unsafe { std::mem::transmute_copy(&pointer) })
            }
        }
    }
    struct Allocation<T>(*mut T);
    impl<T> Drop for Allocation<T> {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    free(self.0.cast());
                }
            }
        }
    }
    fn reply<T>(pointer: *mut T, error: *mut c_void) -> Option<Allocation<T>> {
        let allocation = Allocation(pointer);
        let error = Allocation(error);
        if allocation.0.is_null() || !error.0.is_null() {
            None
        } else {
            Some(allocation)
        }
    }
    struct Api {
        _library: Library,
        connect: Connect,
        disconnect: Disconnect,
        has_error: HasError,
        check: Check,
        query: Query,
        query_reply: QueryReply,
        children: Children,
        children_length: ChildrenLength,
        intern: Intern,
        intern_reply: InternReply,
        property: Property,
        property_reply: GetReply,
        value: Value,
        configure: Configure,
        map: Map,
        unmap: Map,
    }
    impl Api {
        fn load() -> Option<Self> {
            let pointer = unsafe { dlopen(c"libxcb.so.1".as_ptr(), 1) };
            if pointer.is_null() {
                return None;
            }
            let library = Library(pointer);
            unsafe {
                Some(Self {
                    connect: library.symbol(c"xcb_connect")?,
                    disconnect: library.symbol(c"xcb_disconnect")?,
                    has_error: library.symbol(c"xcb_connection_has_error")?,
                    check: library.symbol(c"xcb_request_check")?,
                    query: library.symbol(c"xcb_query_tree")?,
                    query_reply: library.symbol(c"xcb_query_tree_reply")?,
                    children: library.symbol(c"xcb_query_tree_children")?,
                    children_length: library.symbol(c"xcb_query_tree_children_length")?,
                    intern: library.symbol(c"xcb_intern_atom")?,
                    intern_reply: library.symbol(c"xcb_intern_atom_reply")?,
                    property: library.symbol(c"xcb_get_property")?,
                    property_reply: library.symbol(c"xcb_get_property_reply")?,
                    value: library.symbol(c"xcb_get_property_value")?,
                    configure: library.symbol(c"xcb_configure_window_checked")?,
                    map: library.symbol(c"xcb_map_window_checked")?,
                    unmap: library.symbol(c"xcb_unmap_window_checked")?,
                    _library: library,
                })
            }
        }
    }
    struct Connection<'a> {
        pointer: *mut c_void,
        api: &'a Api,
    }
    impl Drop for Connection<'_> {
        fn drop(&mut self) {
            unsafe {
                (self.api.disconnect)(self.pointer);
            }
        }
    }
    impl Connection<'_> {
        fn good(&self) -> bool {
            unsafe { (self.api.has_error)(self.pointer) == 0 }
        }
        fn checked(&self, cookie: Cookie) -> bool {
            let error = Allocation(unsafe { (self.api.check)(self.pointer, cookie) });
            error.0.is_null() && self.good()
        }
        fn atom(&self, name: &CStr) -> Option<u32> {
            let len = u16::try_from(name.to_bytes().len()).ok()?;
            let cookie = unsafe { (self.api.intern)(self.pointer, 1, len, name.as_ptr()) };
            let mut error = ptr::null_mut();
            let pointer = unsafe { (self.api.intern_reply)(self.pointer, cookie, &mut error) };
            let answer = reply(pointer, error)?;
            let atom = unsafe { (*answer.0).atom };
            (atom != 0).then_some(atom)
        }
        fn property(
            &self,
            window: u32,
            atom: u32,
            words: u32,
        ) -> Option<Allocation<PropertyReply>> {
            let cookie = unsafe { (self.api.property)(self.pointer, 0, window, atom, 0, 0, words) };
            let mut error = ptr::null_mut();
            let pointer = unsafe { (self.api.property_reply)(self.pointer, cookie, &mut error) };
            reply(pointer, error)
        }
        fn pid_matches(&self, window: u32, atom: u32, pid: u32) -> bool {
            let Some(answer) = self.property(window, atom, 1) else {
                return false;
            };
            let header = unsafe { &*answer.0 };
            let pointer = unsafe { (self.api.value)(answer.0) };
            let value = if header.format == 32
                && header.value_len == 1
                && header.length == 1
                && !pointer.is_null()
            {
                Some(u64::from(unsafe {
                    ptr::read_unaligned(pointer.cast::<u32>())
                }))
            } else {
                None
            };
            embedded_process_property_matches(
                pid,
                u64::from(header.kind),
                i32::from(header.format),
                u64::from(header.value_len),
                u64::from(header.bytes_after),
                value,
            )
        }
        fn class_matches(&self, window: u32) -> bool {
            // WM_CLASS=67, STRING=31. Refuse truncation/malformed pairs.
            let Some(answer) = self.property(window, 67, 128) else {
                return false;
            };
            let header = unsafe { &*answer.0 };
            if header.format != 8
                || header.value_len > 512
                || header.value_len == 0
                || u64::from(header.length) * 4 < u64::from(header.value_len)
            {
                return false;
            }
            let pointer = unsafe { (self.api.value)(answer.0) };
            if pointer.is_null() {
                return false;
            }
            let value = unsafe {
                std::slice::from_raw_parts(pointer.cast::<u8>(), header.value_len as usize)
            };
            embedded_class_property_matches(header.kind, header.format, header.bytes_after, value)
        }
    }
    pub(super) fn update(
        parent: u64,
        pid: Option<u32>,
        visible: Option<bool>,
        slot: Option<PanelSlot>,
    ) -> bool {
        let Some(parent) = u32::try_from(parent).ok().filter(|value| *value != 0) else {
            return false;
        };
        if pid == Some(0)
            || slot.is_some_and(|slot| {
                slot.width == 0 || slot.height == 0 || slot.width > 65535 || slot.height > 65535
            })
        {
            return false;
        }
        let Some(api) = Api::load() else {
            return false;
        };
        let pointer = unsafe { (api.connect)(ptr::null(), ptr::null_mut()) };
        if pointer.is_null() {
            return false;
        }
        let connection = Connection { pointer, api: &api };
        if !connection.good() {
            return false;
        }
        let pid_atom = if pid.is_some() {
            let Some(atom) = connection.atom(c"_NET_WM_PID") else {
                return false;
            };
            atom
        } else {
            0
        };
        let cookie = unsafe { (api.query)(pointer, parent) };
        let mut error = ptr::null_mut();
        let pointer_reply = unsafe { (api.query_reply)(pointer, cookie, &mut error) };
        let Some(answer) = reply(pointer_reply, error) else {
            return false;
        };
        let count = unsafe { (api.children_length)(answer.0) };
        if !(0..=4096).contains(&count) {
            return false;
        }
        let children = unsafe { (api.children)(answer.0) };
        if children.is_null() {
            return false;
        }
        let children = unsafe { std::slice::from_raw_parts(children, count as usize) };
        let mut operated = false;
        for &child in children {
            if pid.is_some_and(|pid| !connection.pid_matches(child, pid_atom, pid))
                || !connection.class_matches(child)
            {
                continue;
            }
            if let Some(slot) = slot {
                let values = [slot.x as u32, slot.y as u32, slot.width, slot.height];
                let cookie = unsafe { (api.configure)(pointer, child, 15, values.as_ptr().cast()) };
                operated |= connection.checked(cookie);
            }
            if let Some(visible) = visible {
                let action = if visible { api.map } else { api.unmap };
                operated |= connection.checked(unsafe { action(pointer, child) });
            }
        }
        operated
    }
    #[cfg(test)]
    pub(super) fn checked_mapping_for_test(window: u32) -> bool {
        let Some(api) = Api::load() else {
            return false;
        };
        let pointer = unsafe { (api.connect)(ptr::null(), ptr::null_mut()) };
        if pointer.is_null() {
            return false;
        }
        let connection = Connection { pointer, api: &api };
        connection.good() && connection.checked(unsafe { (api.map)(pointer, window) })
    }
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

/// True only when this build carries the process helper's `wry-host` body.
/// The macOS GUI's separate `native-page-host` feature does not change this
/// helper probe.
pub fn webview_backend_linked() -> bool {
    cfg!(feature = "wry-host")
}

/// Default DeepSeek authorize endpoint. The login flow loads this in the
/// built-in content panel. It is not an OS-browser URL.
pub const DEEPSEEK_AUTHORIZE_URL: &str = "https://platform.deepseek.com/oauth/authorize";

/// Loopback redirect for the DeepSeek authorize page. The host must stay on
/// the allowlist. This does not store a token.
pub const DEEPSEEK_OAUTH_REDIRECT: &str = "http://127.0.0.1/minicon/oauth";

/// DeepSeek authorize URL for the one content panel.
///
/// The caller loads the result with the page host. That host already
/// installs `MINICON_BROWSER_HTTP_PROXY`. No system browser, no token.
pub fn deepseek_oauth_authorize() -> Result<String, &'static str> {
    deepseek_oauth_authorize_redirect(DEEPSEEK_OAUTH_REDIRECT)
}

/// Same authorize URL with a caller-chosen loopback redirect.
///
/// The redirect host must be `127.0.0.1`. Anything else is rejected. The
/// result is what the built-in page host loads. No system browser.
pub fn deepseek_oauth_authorize_redirect(redirect_uri: &str) -> Result<String, &'static str> {
    oauth_authorize_load(DEEPSEEK_AUTHORIZE_URL, redirect_uri, &["127.0.0.1"])
}

const OAUTH_CODE_MAX_BYTES: usize = 256;

/// Authorization code from a loopback redirect query.
///
/// Accepts `code` or `access_token`. Empty, oversized, or non-ascii values
/// are rejected. This does not open a browser.
pub fn oauth_code_from_query(query: &str) -> Result<String, &'static str> {
    let mut found = None;
    for pair in query.split('&') {
        let Some((key, raw)) = pair.split_once('=') else {
            continue;
        };
        if key != "code" && key != "access_token" {
            continue;
        }
        let value = percent_decode(raw).ok_or("oauth-rejected")?;
        if value.is_empty()
            || value.len() > OAUTH_CODE_MAX_BYTES
            || !value.is_ascii()
            || value
                .bytes()
                .any(|byte| byte.is_ascii_control() || byte == b' ')
        {
            return Err("oauth-rejected");
        }
        found = Some(value);
    }
    found.ok_or("oauth-rejected")
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let high = hex_nibble(bytes[index + 1])?;
            let low = hex_nibble(bytes[index + 2])?;
            out.push((high << 4) | low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// How many loopback connections a bridge will answer before giving up.
/// A request with the wrong `state` does not consume the login.
const OAUTH_ACCEPT_LIMIT: usize = 8;

/// Loopback listener for the OAuth redirect. Binds `127.0.0.1` only.
///
/// The accept thread answers each browser request. A missing or wrong `state`
/// is refused and the listener keeps waiting, up to [`OAUTH_ACCEPT_LIMIT`].
/// [`Self::poll`] is what the GUI reads. This does not call the OS browser.
pub struct OAuthBridge {
    redirect_uri: String,
    state: String,
    verifier: String,
    challenge: String,
    incoming: std::sync::mpsc::Receiver<Result<String, &'static str>>,
    taken: std::sync::atomic::AtomicBool,
}

impl OAuthBridge {
    pub fn bind() -> Result<Self, String> {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").map_err(|error| error.to_string())?;
        let port = listener
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        let state = oauth_token()?;
        let (verifier, challenge) = oauth_pkce()?;
        let expect = state.clone();
        let (tx, incoming) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for _ in 0..OAUTH_ACCEPT_LIMIT {
                let Ok((mut stream, _)) = listener.accept() else {
                    let _ = tx.send(Err("oauth-rejected"));
                    return;
                };
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
                if let Ok(code) = read_oauth_request(&mut stream, port, &expect) {
                    let _ = tx.send(Ok(code));
                    return;
                }
            }
            let _ = tx.send(Err("oauth-rejected"));
        });
        Ok(Self {
            redirect_uri: format!("http://127.0.0.1:{port}/minicon/oauth"),
            state,
            verifier,
            challenge,
            incoming,
            taken: std::sync::atomic::AtomicBool::new(false),
        })
    }

    pub fn redirect_uri(&self) -> String {
        self.redirect_uri.clone()
    }

    /// Unpredictable `state` this bridge will accept on the redirect.
    pub fn state(&self) -> &str {
        &self.state
    }

    /// PKCE verifier. It stays in this process and is sent only to the token endpoint.
    pub fn code_verifier(&self) -> &str {
        &self.verifier
    }

    /// DeepSeek authorize URL for this bridge: redirect, `state`, and S256 challenge.
    pub fn authorize_url(&self) -> Result<String, &'static str> {
        oauth_authorize_with_proof(
            DEEPSEEK_AUTHORIZE_URL,
            &self.redirect_uri,
            &["127.0.0.1"],
            &self.state,
            &self.challenge,
        )
    }

    /// One pending redirect. `None` means nothing arrived yet.
    pub fn poll(&self) -> Option<Result<String, &'static str>> {
        match self.incoming.try_recv() {
            Ok(result) => {
                self.taken.store(true, std::sync::atomic::Ordering::Relaxed);
                Some(result)
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                if self.taken.load(std::sync::atomic::Ordering::Relaxed) {
                    None
                } else {
                    Some(Err("oauth-rejected"))
                }
            }
        }
    }
}

fn read_oauth_request(
    stream: &mut std::net::TcpStream,
    port: u16,
    expected_state: &str,
) -> Result<String, &'static str> {
    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0u8; 512];
    while buf.len() < 2048 && !buf.windows(4).any(|window| window == b"\r\n\r\n") {
        match std::io::Read::read(stream, &mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(_) => return Err("oauth-rejected"),
        }
    }
    if buf.is_empty() {
        return Err("oauth-rejected");
    }
    let text = String::from_utf8_lossy(&buf);
    let Some(request) = text.lines().next() else {
        let _ = write_oauth_reply(stream, false);
        return Err("oauth-rejected");
    };
    let mut parts = request.split(' ');
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    if method != "GET" {
        let _ = write_oauth_reply(stream, false);
        return Err("oauth-rejected");
    }
    let host_ok = text.lines().any(|line| {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        name.eq_ignore_ascii_case("host")
            && value
                .trim()
                .eq_ignore_ascii_case(&format!("127.0.0.1:{port}"))
    });
    if !host_ok {
        let _ = write_oauth_reply(stream, false);
        return Err("oauth-rejected");
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != "/minicon/oauth" {
        let _ = write_oauth_reply(stream, false);
        return Err("oauth-rejected");
    }
    let states = query_values(query, "state");
    let decoded = states
        .first()
        .and_then(|value| percent_decode(value))
        .unwrap_or_default();
    if states.len() != 1 || !tokens_match(&decoded, expected_state) {
        let _ = write_oauth_reply(stream, false);
        return Err("oauth-state");
    }
    let codes = query_values(query, "code").len() + query_values(query, "access_token").len();
    if codes != 1 {
        let _ = write_oauth_reply(stream, false);
        return Err("oauth-rejected");
    }
    match oauth_code_from_query(query) {
        Ok(code) => {
            let _ = write_oauth_reply(stream, true);
            Ok(code)
        }
        Err(reason) => {
            let _ = write_oauth_reply(stream, false);
            Err(reason)
        }
    }
}

fn write_oauth_reply(stream: &mut std::net::TcpStream, ok: bool) -> std::io::Result<()> {
    use std::io::Write;
    let body = if ok {
        "<html><body>MiniCon received the sign-in.</body></html>"
    } else {
        "<html><body>MiniCon refused the sign-in.</body></html>"
    };
    let status = if ok { "200 OK" } else { "400 Bad Request" };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()?;
    let _ = stream.shutdown(std::net::Shutdown::Both);
    Ok(())
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

/// Authorize URL plus this attempt's `state` and S256 `code_challenge`.
///
/// `state` and `code_challenge` must be short URL-safe tokens. The verifier
/// that produced the challenge is not placed on this URL.
pub fn oauth_authorize_with_proof(
    authorize_url: &str,
    redirect_uri: &str,
    allowed_hosts: &[&str],
    state: &str,
    code_challenge: &str,
) -> Result<String, &'static str> {
    if !oauth_token_ok(state) || !oauth_token_ok(code_challenge) {
        return Err("oauth-rejected");
    }
    let base = oauth_authorize_load(authorize_url, redirect_uri, allowed_hosts)?;
    Ok(format!(
        "{base}&state={}&code_challenge={}&code_challenge_method=S256",
        percent_encode(state),
        percent_encode(code_challenge)
    ))
}

fn oauth_token_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn query_values<'a>(query: &'a str, key: &str) -> Vec<&'a str> {
    query
        .split('&')
        .filter_map(|pair| {
            let (name, value) = pair.split_once('=')?;
            (name == key).then_some(value)
        })
        .collect()
}

fn tokens_match(left: &str, right: &str) -> bool {
    if left.len() != right.len() || left.is_empty() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.bytes().zip(right.bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn oauth_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    fill_random(&mut bytes)?;
    Ok(base64url(&bytes))
}

fn oauth_pkce() -> Result<(String, String), String> {
    let verifier = oauth_token()?;
    use sha2::Digest;
    let digest = sha2::Sha256::digest(verifier.as_bytes());
    Ok((verifier, base64url(&digest)))
}

fn base64url(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    let mut index = 0;
    while index + 3 <= bytes.len() {
        let n = (u32::from(bytes[index]) << 16)
            | (u32::from(bytes[index + 1]) << 8)
            | u32::from(bytes[index + 2]);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push(TABLE[(n & 63) as usize] as char);
        index += 3;
    }
    let rest = bytes.len() - index;
    if rest == 1 {
        let n = u32::from(bytes[index]) << 16;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
    } else if rest == 2 {
        let n = (u32::from(bytes[index]) << 16) | (u32::from(bytes[index + 1]) << 8);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
    }
    out
}

fn fill_random(out: &mut [u8]) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::io::Read;
        let mut file = std::fs::File::open("/dev/urandom").map_err(|error| error.to_string())?;
        file.read_exact(out).map_err(|error| error.to_string())?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        #[link(name = "advapi32")]
        unsafe extern "system" {
            fn SystemFunction036(random_buffer: *mut u8, random_buffer_length: u32) -> u8;
        }
        let ok = unsafe { SystemFunction036(out.as_mut_ptr(), out.len() as u32) };
        if ok == 0 {
            Err("oauth-random-unavailable".to_owned())
        } else {
            Ok(())
        }
    }
}

fn redirect_host(redirect_uri: &str) -> Option<String> {
    let lower = redirect_uri.to_ascii_lowercase();
    let https = lower.strip_prefix("https://");
    let http = lower.strip_prefix("http://");
    let (rest, loopback_only) = if let Some(rest) = https {
        (rest, false)
    } else {
        (http?, true)
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.split('@').next_back().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty() || (loopback_only && host != "127.0.0.1" && host != "localhost") {
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
        [flag, url] if flag == "--load-context" => load_context_host(url, None),
        [flag, url, parent_flag, parent, bounds_flag, bounds]
            if flag == "--load-context"
                && parent_flag == "--parent"
                && bounds_flag == "--bounds" =>
        {
            load_context_host(url, Some(parse_slot(parent, bounds)?))
        }
        [flag] if flag == "--probe" => probe_webview(),
        [flag, url] if flag == "--load" || flag == "--smoke" => {
            let _ = http_url_at(url, 0)
                .filter(|canonical| canonical == url)
                .ok_or("not-http")?;
            load_webview(url, flag == "--smoke", None, None)
        }
        [flag, url, redirect] if flag == "--oauth-smoke" => {
            let _ = http_url_at(url, 0)
                .filter(|canonical| canonical == url)
                .ok_or("not-http")?;
            let _ = loopback_http_target(redirect).ok_or("not-loopback")?;
            load_webview(url, true, None, Some(redirect))
        }
        [flag, url, parent_flag, parent, bounds_flag, bounds]
            if flag == "--load" && parent_flag == "--parent" && bounds_flag == "--bounds" =>
        {
            let _ = http_url_at(url, 0)
                .filter(|canonical| canonical == url)
                .ok_or("not-http")?;
            let slot = parse_slot(parent, bounds)?;
            load_webview(url, false, Some(slot), None)
        }
        _ => Err("usage"),
    }
}

fn load_context_host(url: &str, slot: Option<PanelSlot>) -> Result<(), &'static str> {
    let result = (|| {
        if http_url_at(url, 0).as_deref() != Some(url) {
            return Err(PageContextError::InvalidProfile);
        }
        let context = PageContext::read_frame(&mut std::io::stdin().lock())?;
        #[cfg(all(feature = "wry-host", target_os = "linux"))]
        {
            wry_host::load_with_context(url, slot, &context)
        }
        #[cfg(not(all(feature = "wry-host", target_os = "linux")))]
        {
            let _ = (context, slot);
            Err(PageContextError::Unsupported(
                "browser-context-helper-unsupported",
            ))
        }
    })();
    result.map_err(|error| {
        // Codes contain no paths, endpoint strings, or underlying errors.
        if slot.is_some() {
            println!("{{\"error\":\"{}\"}}", error.code());
        }
        error.code()
    })
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

fn load_webview(
    url: &str,
    smoke: bool,
    slot: Option<PanelSlot>,
    redirect_smoke: Option<&str>,
) -> Result<(), &'static str> {
    #[cfg(feature = "wry-host")]
    {
        wry_host::load(url, smoke, slot, redirect_smoke)
    }
    #[cfg(not(feature = "wry-host"))]
    {
        let _ = (url, smoke, slot, redirect_smoke);
        Err("webkit-host-not-linked")
    }
}

/// `http://host:port` from the resolved hook, for wry `ProxyConfig::Http`.
/// Socks and userinfo are refused. `None` means a direct WebView load.
pub fn wry_proxy_endpoint(proxy: &BrowserProxy) -> Option<(String, String)> {
    let raw = proxy.http.as_deref().or(proxy.https.as_deref())?;
    parse_http_proxy_endpoint(raw)
}

/// True when `url` is an http loopback the WebView proxy must not carry.
///
/// wry 0.56 `ProxyConfig::Http` builds WebKit `NetworkProxySettings` with an
/// empty ignore list, so `no_proxy` does not exempt `127.0.0.1`. The page host
/// cancels that navigation and [`deliver_loopback_get`] fetches it directly.
pub fn webview_must_bypass_proxy(url: &str) -> bool {
    loopback_http_target(url).is_some()
}

/// GET `url` on `127.0.0.1` with `TcpStream`. Does not read `http_proxy`.
pub fn deliver_loopback_get(url: &str) -> Result<(), &'static str> {
    let target = loopback_http_target(url).ok_or("not-loopback")?;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], target.port));
    let mut stream = std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(2))
        .map_err(|_| "loopback-refused")?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .map_err(|_| "loopback-refused")?;
    stream
        .set_write_timeout(Some(std::time::Duration::from_secs(2)))
        .map_err(|_| "loopback-refused")?;
    use std::io::{Read, Write};
    write!(
        stream,
        "GET {} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n\r\n",
        target.path_and_query, target.host, target.port
    )
    .map_err(|_| "loopback-refused")?;
    stream.flush().map_err(|_| "loopback-refused")?;
    let mut buf = [0u8; 2048];
    let _ = stream.read(&mut buf);
    Ok(())
}

struct LoopbackHttp {
    host: String,
    port: u16,
    path_and_query: String,
}

/// `http://127.0.0.1` or `http://localhost` only. `localhost` is connected as
/// `127.0.0.1`, never resolved. Userinfo and any other host are refused.
fn loopback_http_target(url: &str) -> Option<LoopbackHttp> {
    let rest = url
        .get(..7)
        .filter(|prefix| prefix.eq_ignore_ascii_case("http://"))
        .map(|_| &url[7..])?;
    if rest.contains('@') || rest.is_empty() {
        return None;
    }
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty() {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port))
            if !host.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            (host, port.parse::<u16>().ok().filter(|port| *port != 0)?)
        }
        Some(_) => return None,
        None => (authority, 80),
    };
    if !host.eq_ignore_ascii_case("127.0.0.1") && !host.eq_ignore_ascii_case("localhost") {
        return None;
    }
    let path = path.split('#').next().unwrap_or("");
    let path_and_query = if path.is_empty() {
        "/".to_owned()
    } else {
        format!("/{path}")
    };
    Some(LoopbackHttp {
        host: host.to_owned(),
        port,
        path_and_query,
    })
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
    fn embedded_process_selector_requires_exact_cardinal_identity_without_fallback() {
        assert!(embedded_process_property_matches(17, 6, 32, 1, 0, Some(17)));
        for (pid, kind, format, items, after, value) in [
            (0, 6, 32, 1, 0, Some(0)),
            (17, 0, 32, 1, 0, Some(17)),
            (17, 31, 32, 1, 0, Some(17)),
            (17, 6, 8, 1, 0, Some(17)),
            (17, 6, 16, 1, 0, Some(17)),
            (17, 6, 32, 0, 0, Some(17)),
            (17, 6, 32, 2, 0, Some(17)),
            (17, 6, 32, 1, 4, Some(17)),
            (17, 6, 32, 1, 0, None),
            (17, 6, 32, 1, 0, Some(18)),
            (17, 6, 32, 1, 0, Some((1u64 << 32) | 17)),
        ] {
            assert!(!embedded_process_property_matches(
                pid, kind, format, items, after, value
            ));
        }
        assert!(!set_embedded_page_visible_for_process(0, 17, true));
        assert!(!set_embedded_page_visible_for_process(1, 0, true));
        assert!(!resize_embedded_page_for_process(
            PanelSlot {
                parent: 1,
                x: 0,
                y: 0,
                width: 1,
                height: 1
            },
            0
        ));
    }

    #[test]
    fn embedded_process_class_parser_requires_a_complete_bounded_string_pair() {
        let valid = b"minicon-web-host\0MiniCon-web-host\0";
        assert!(embedded_class_property_matches(31, 8, 0, valid));
        for (kind, format, after, value) in [
            (6, 8, 0, valid.as_slice()),
            (31, 32, 0, valid.as_slice()),
            (31, 8, 1, valid.as_slice()),
            (31, 8, 0, b"other\0other\0".as_slice()),
            (31, 8, 0, b"minicon-web-host\0".as_slice()),
            (31, 8, 0, b"minicon-web-host\0other\0extra\0".as_slice()),
        ] {
            assert!(!embedded_class_property_matches(kind, format, after, value));
        }
        assert!(!embedded_class_property_matches(31, 8, 0, &vec![b'a'; 513]));
    }

    #[test]
    fn proxy_override_blank_retains_the_complete_config_environment_default() {
        let base = BrowserProxy {
            http: Some("http://config.test:8080".into()),
            https: Some("http://secure.test:8081".into()),
            no_proxy: Some("localhost,example.test".into()),
        };
        assert_eq!(proxy_override(&base, " \t ").unwrap(), base);
        assert_eq!(
            proxy_override(&BrowserProxy::default(), "").unwrap(),
            BrowserProxy::default()
        );
    }

    #[test]
    fn proxy_override_explicit_http_endpoint_covers_both_schemes_and_clears_bypass() {
        let base = BrowserProxy {
            no_proxy: Some("*".into()),
            ..BrowserProxy::default()
        };
        let proxy = proxy_override(&base, "  http://127.0.0.1:08890/  ").unwrap();
        assert_eq!(proxy.http.as_deref(), Some("http://127.0.0.1:8890"));
        assert_eq!(proxy.https, proxy.http);
        assert_eq!(proxy.no_proxy, None);
        assert_eq!(base.no_proxy.as_deref(), Some("*"));
    }

    #[test]
    fn proxy_override_invalid_input_is_an_error_instead_of_a_direct_connection() {
        for input in [
            "127.0.0.1:8890",
            "https://proxy.test:8890",
            "socks5://127.0.0.1:8890",
            "http://user:password@127.0.0.1:8890",
            "http://host --switch:80",
            "http://host:80/path",
            "http://host:80?query",
            "http://host:80#fragment",
            "http://host:0",
            "http://host:65536",
            "http://host:-1",
            "http://host:+80",
            "http://:80",
            "http://host:",
            "http://host:80\0",
        ] {
            assert_eq!(
                proxy_override(&BrowserProxy::default(), input),
                Err("browser-proxy-invalid".into()),
                "{input:?}"
            );
        }
    }

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
    fn the_reparented_host_class_is_the_only_window_we_hide() {
        assert!(embedded_page_class("minicon-web-host"));
        assert!(embedded_page_class("Minicon-web-host"));
        assert!(!embedded_page_class("minicon"));
        assert!(!embedded_page_class("xdg-open"));
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

    #[cfg(unix)]
    #[test]
    fn deepseek_oauth_authorize_uses_the_browser_proxy_hook() {
        let url = deepseek_oauth_authorize().unwrap();
        assert!(url.starts_with("https://platform.deepseek.com/oauth/authorize?"));
        assert!(url.contains("redirect_uri="));
        let proxy = BrowserProxy {
            http: Some("http://127.0.0.1:9".into()),
            https: None,
            no_proxy: None,
        };
        assert_eq!(
            wry_proxy_endpoint(&proxy),
            Some(("127.0.0.1".into(), "9".into()))
        );
        let dir = std::env::temp_dir().join(format!("minicon-oauth-proxy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("log");
        let program = dir.join("minicon-page-host");
        let script = format!(
            "#!/bin/sh\nprintf '%s|%s\\n' \"$2\" \"$MINICON_BROWSER_HTTP_PROXY\" > '{}'\n",
            log.display()
        );
        std::fs::write(&program, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut child = spawn_page_host_with_proxy(&program, &url, Some(&proxy)).unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(
            std::fs::read_to_string(&log).unwrap().trim(),
            format!("{url}|http://127.0.0.1:9")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oauth_code_comes_only_from_the_loopback_query() {
        assert_eq!(oauth_code_from_query("code=abc").as_deref(), Ok("abc"));
        assert_eq!(
            oauth_code_from_query("access_token=tok"),
            Ok("tok".to_owned())
        );
        assert_eq!(oauth_code_from_query("code="), Err("oauth-rejected"));
        assert_eq!(
            oauth_code_from_query(&format!("code={}", "x".repeat(257))),
            Err("oauth-rejected")
        );
        assert_eq!(oauth_code_from_query("code=a%20b"), Err("oauth-rejected"));
        let bridge = OAuthBridge::bind().unwrap();
        assert!(bridge.redirect_uri().starts_with("http://127.0.0.1:"));
        assert!(bridge.redirect_uri().ends_with("/minicon/oauth"));
        let authorize = bridge.authorize_url().unwrap();
        assert!(authorize.contains(&format!("state={}", bridge.state())));
        assert!(authorize.contains("code_challenge_method=S256"));
        assert!(!authorize.contains(bridge.code_verifier()));
        let uri = bridge.redirect_uri();
        let port = uri
            .trim_start_matches("http://127.0.0.1:")
            .trim_end_matches("/minicon/oauth");
        use std::io::Write;
        std::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .unwrap()
            .write_all(
                format!(
                    "GET /minicon/oauth?code=stolen HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
                )
                .as_bytes(),
            )
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert!(bridge.poll().is_none());
        std::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .unwrap()
            .write_all(
                format!(
                    "GET /minicon/oauth?code=from-loop&state={} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n",
                    bridge.state()
                )
                .as_bytes(),
            )
            .unwrap();
        let code = (0..50)
            .find_map(|_| {
                let got = bridge.poll();
                if got.is_none() {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                got
            })
            .unwrap()
            .unwrap();
        assert_eq!(code, "from-loop");
        assert!(bridge.poll().is_none());
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
        assert_eq!(
            host_reply(&[
                "--oauth-smoke".into(),
                "http://minicon.invalid/start".into(),
                "http://127.0.0.1:54321/minicon/oauth?code=live".into(),
            ]),
            Err("webkit-host-not-linked")
        );
        assert_eq!(
            host_reply(&[
                "--oauth-smoke".into(),
                "http://minicon.invalid/start".into(),
                "https://example.test/not-loopback".into(),
            ]),
            Err("not-loopback")
        );
    }

    #[test]
    fn wry_host_source_loads_with_wry_and_not_the_os_browser() {
        let source = include_str!("wry_host.rs");
        assert!(source.contains(".with_url(url)"));
        assert!(source.contains("default_vbox"));
        let library = include_str!("lib.rs")
            .split("\n#[cfg(test)]\nmod tests {")
            .next()
            .unwrap_or_default();
        assert!(
            source.contains("crate::resolve_proxy_from_process(&crate::BrowserProxyConfig::default())")
                && library.contains("apply_proxy_env(&mut command, proxy);")
                && library.contains("for key in webkit_proxy_clear(proxy) {\n        command.env_remove(key);\n    }")
                && library.contains("for (key, value) in webkit_proxy_env(proxy) {\n        command.env(key, value);\n    }")
        );
        assert!(
            source.contains("let parts = context.proxy_parts().map_err(|error| error.code())?;")
                && source.contains("manager.set_network_proxy_settings(NetworkProxyMode::NoProxy, None);")
                && source.contains("let bypass: Vec<_> = parts.bypass.iter().map(String::as_str).collect();")
                && source.contains("settings.add_proxy_for_scheme(\"http\", &format!(\"http://{host}:{port}\"));")
                && source.contains("settings.add_proxy_for_scheme(\"https\", &format!(\"http://{host}:{port}\"));")
                && source.contains("manager.set_network_proxy_settings(NetworkProxyMode::Custom, Some(&mut settings));")
        );
        assert!(source.contains("with_proxy_config"));
        assert!(source.contains("with_navigation_handler"));
        assert!(source.contains("webview_must_bypass_proxy"));
        assert!(source.contains("deliver_loopback_get"));
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
    fn loopback_oauth_redirect_is_delivered_without_the_webview_proxy() {
        assert!(!webview_must_bypass_proxy(
            "https://platform.deepseek.com/oauth/authorize"
        ));
        assert!(!webview_must_bypass_proxy(
            "http://example.test/minicon/oauth?code=abc"
        ));
        assert!(!webview_must_bypass_proxy(
            "http://user:pw@127.0.0.1/minicon/oauth?code=abc"
        ));
        assert!(!webview_must_bypass_proxy(
            "http://127.0.0.1.evil.test/minicon/oauth?code=abc"
        ));
        let bridge = OAuthBridge::bind().expect("bridge");
        let url = format!(
            "{}?code=from-loopback&state={}",
            bridge.redirect_uri(),
            bridge.state()
        );
        assert!(webview_must_bypass_proxy(&url));
        deliver_loopback_get(&url).expect("direct get");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let code = loop {
            if let Some(result) = bridge.poll() {
                break result.expect("code");
            }
            assert!(
                deadline > std::time::Instant::now(),
                "oauth code did not arrive"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(code, "from-loopback");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn embedded_page_resize_moves_and_sizes_a_classed_x_child() {
        embedded_x11_fixture(false);
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires an explicitly provisioned DISPLAY (for example Xvfb); missing display is Unavailable"]
    fn embedded_page_process_selector_maps_and_resizes_only_its_direct_owned_child() {
        embedded_x11_fixture(true);
    }

    #[cfg(target_os = "linux")]
    fn embedded_x11_fixture(owned_selector: bool) {
        use std::ffi::{CStr, CString, c_char, c_int, c_long, c_uint, c_ulong, c_void};

        if std::env::var_os("DISPLAY").is_none() {
            assert!(
                !owned_selector,
                "Unavailable: owned-window selector court requires DISPLAY"
            );
            assert!(
                std::env::var_os("MINICON_REQUIRE_DISPLAY").is_none(),
                "native X resize court requires DISPLAY"
            );
            return;
        }
        unsafe extern "C" {
            fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
            fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
            fn dlclose(handle: *mut c_void) -> c_int;
        }
        unsafe fn symbol<T>(handle: *mut c_void, name: &CStr) -> T {
            let pointer = unsafe { dlsym(handle, name.as_ptr()) };
            assert!(!pointer.is_null(), "missing X11 symbol {name:?}");
            unsafe { std::mem::transmute_copy(&pointer) }
        }
        type Open = unsafe extern "C" fn(*const c_char) -> *mut c_void;
        type Root = unsafe extern "C" fn(*mut c_void) -> c_ulong;
        type Create = unsafe extern "C" fn(
            *mut c_void,
            c_ulong,
            c_int,
            c_int,
            c_uint,
            c_uint,
            c_uint,
            c_ulong,
            c_ulong,
        ) -> c_ulong;
        type Class = unsafe extern "C" fn(*mut c_void, c_ulong, *mut *mut c_char) -> c_int;
        type Map = unsafe extern "C" fn(*mut c_void, c_ulong) -> c_int;
        type Sync = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
        type Geometry = unsafe extern "C" fn(
            *mut c_void,
            c_ulong,
            *mut c_ulong,
            *mut c_int,
            *mut c_int,
            *mut c_uint,
            *mut c_uint,
            *mut c_uint,
            *mut c_uint,
        ) -> c_int;
        type Destroy = unsafe extern "C" fn(*mut c_void, c_ulong) -> c_int;
        type Close = unsafe extern "C" fn(*mut c_void) -> c_int;
        type Intern = unsafe extern "C" fn(*mut c_void, *const c_char, c_int) -> c_ulong;
        type Change = unsafe extern "C" fn(
            *mut c_void,
            c_ulong,
            c_ulong,
            c_ulong,
            c_int,
            c_int,
            *const u8,
            c_int,
        ) -> c_int;
        type Delete = unsafe extern "C" fn(*mut c_void, c_ulong, c_ulong) -> c_int;
        // XWindowAttributes ABI: pinned x11-dl 2.21.0 src/xlib.rs.
        #[repr(C)]
        struct Attributes {
            x: c_int,
            y: c_int,
            width: c_int,
            height: c_int,
            _border: c_int,
            _depth: c_int,
            _visual: *mut c_void,
            _root: c_ulong,
            _class: c_int,
            _bit_gravity: c_int,
            _win_gravity: c_int,
            _backing_store: c_int,
            _backing_planes: c_ulong,
            _backing_pixel: c_ulong,
            _save_under: c_int,
            _colormap: c_ulong,
            _map_installed: c_int,
            map_state: c_int,
            _all_masks: c_long,
            _your_mask: c_long,
            _propagate_mask: c_long,
            _override_redirect: c_int,
            _screen: *mut c_void,
        }
        type GetAttributes = unsafe extern "C" fn(*mut c_void, c_ulong, *mut Attributes) -> c_int;

        let handle = unsafe { dlopen(c"libX11.so.6".as_ptr(), 1) };
        assert!(!handle.is_null(), "open libX11");
        let open: Open = unsafe { symbol(handle, c"XOpenDisplay") };
        let root: Root = unsafe { symbol(handle, c"XDefaultRootWindow") };
        let create: Create = unsafe { symbol(handle, c"XCreateSimpleWindow") };
        let set_class: Class = unsafe { symbol(handle, c"XSetClassHint") };
        let map: Map = unsafe { symbol(handle, c"XMapWindow") };
        let sync: Sync = unsafe { symbol(handle, c"XSync") };
        let geometry: Geometry = unsafe { symbol(handle, c"XGetGeometry") };
        let destroy: Destroy = unsafe { symbol(handle, c"XDestroyWindow") };
        let close: Close = unsafe { symbol(handle, c"XCloseDisplay") };
        let display = unsafe { open(std::ptr::null()) };
        assert!(!display.is_null(), "Unavailable: cannot open DISPLAY");
        struct OwnedDisplay {
            display: *mut c_void,
            parent: c_ulong,
            destroy: Destroy,
            close: Close,
            library: *mut c_void,
        }
        impl Drop for OwnedDisplay {
            fn drop(&mut self) {
                unsafe {
                    if self.parent != 0 {
                        (self.destroy)(self.display, self.parent);
                    }
                    (self.close)(self.display);
                    dlclose(self.library);
                }
            }
        }
        let mut owned = OwnedDisplay {
            display,
            parent: 0,
            destroy,
            close,
            library: handle,
        };
        let root = unsafe { root(display) };
        let parent = unsafe { create(display, root, 0, 0, 640, 480, 0, 0, 0) };
        owned.parent = parent;
        let child = unsafe { create(display, parent, 1, 2, 3, 4, 0, 0, 0) };
        assert_ne!(parent, 0);
        assert_ne!(child, 0);
        let name = CString::new("minicon-web-host").unwrap();
        let class = CString::new("minicon-web-host").unwrap();
        let mut hint = [name.as_ptr().cast_mut(), class.as_ptr().cast_mut()];
        assert_ne!(unsafe { set_class(display, child, hint.as_mut_ptr()) }, 0);
        unsafe {
            map(display, parent);
            map(display, child);
            sync(display, 0);
        }

        let slot = PanelSlot {
            parent,
            x: 17,
            y: 29,
            width: 311,
            height: 197,
        };
        resize_embedded_page(slot);
        unsafe { sync(display, 0) };
        let mut geometry_root = 0;
        let mut x = 0;
        let mut y = 0;
        let mut width = 0;
        let mut height = 0;
        let mut border = 0;
        let mut depth = 0;
        assert_ne!(
            unsafe {
                geometry(
                    display,
                    child,
                    &mut geometry_root,
                    &mut x,
                    &mut y,
                    &mut width,
                    &mut height,
                    &mut border,
                    &mut depth,
                )
            },
            0
        );
        assert_eq!((x, y, width, height), (17, 29, 311, 197));
        if owned_selector {
            let intern: Intern = unsafe { symbol(handle, c"XInternAtom") };
            let change: Change = unsafe { symbol(handle, c"XChangeProperty") };
            let delete: Delete = unsafe { symbol(handle, c"XDeleteProperty") };
            let get: GetAttributes = unsafe { symbol(handle, c"XGetWindowAttributes") };
            let pid_atom = unsafe { intern(display, c"_NET_WM_PID".as_ptr(), 0) };
            assert_ne!(pid_atom, 0);
            let second = unsafe { create(display, parent, 4, 5, 90, 80, 0, 0, 0) };
            let nested = unsafe { create(display, child, 6, 7, 30, 40, 0, 0, 0) };
            let wrong_class = unsafe { create(display, parent, 8, 9, 50, 60, 0, 0, 0) };
            let other = CString::new("other-client").unwrap();
            let mut wrong_hint = [other.as_ptr().cast_mut(), other.as_ptr().cast_mut()];
            let set_pid = |window, kind, format, count| {
                let values: [c_ulong; 2] = [17, 29];
                unsafe {
                    change(
                        display,
                        window,
                        pid_atom,
                        kind,
                        format,
                        0,
                        values.as_ptr().cast(),
                        count,
                    );
                }
            };
            unsafe {
                set_class(display, second, hint.as_mut_ptr());
                set_class(display, nested, hint.as_mut_ptr());
                set_class(display, wrong_class, wrong_hint.as_mut_ptr());
                map(display, second);
                map(display, nested);
                map(display, wrong_class);
            }
            set_pid(child, 6, 32, 1);
            let second_pid: c_ulong = 29;
            let nested_pid: c_ulong = 31;
            let wrong_pid: c_ulong = 37;
            unsafe {
                change(
                    display,
                    second,
                    pid_atom,
                    6,
                    32,
                    0,
                    (&second_pid as *const c_ulong).cast(),
                    1,
                );
                change(
                    display,
                    nested,
                    pid_atom,
                    6,
                    32,
                    0,
                    (&nested_pid as *const c_ulong).cast(),
                    1,
                );
                change(
                    display,
                    wrong_class,
                    pid_atom,
                    6,
                    32,
                    0,
                    (&wrong_pid as *const c_ulong).cast(),
                    1,
                );
                sync(display, 0);
            }
            let attributes = |window| {
                unsafe {
                    sync(display, 0);
                }
                let mut value: Attributes = unsafe { std::mem::zeroed() };
                assert_ne!(unsafe { get(display, window, &mut value) }, 0);
                (value.x, value.y, value.width, value.height, value.map_state)
            };
            let desired = PanelSlot {
                parent,
                x: 21,
                y: 34,
                width: 312,
                height: 198,
            };
            assert!(resize_embedded_page_for_process(desired, 17));
            assert_eq!(attributes(child), (21, 34, 312, 198, 2));
            assert_eq!(attributes(second), (4, 5, 90, 80, 2));
            assert_eq!(attributes(nested), (6, 7, 30, 40, 2));
            assert_eq!(attributes(wrong_class), (8, 9, 50, 60, 2));
            assert!(set_embedded_page_visible_for_process(parent, 17, false));
            assert_eq!(attributes(child).4, 0);
            assert_eq!(attributes(second).4, 2);
            assert!(set_embedded_page_visible_for_process(parent, 17, true));
            assert_eq!(attributes(child).4, 2);
            assert!(resize_embedded_page_for_process(
                PanelSlot {
                    x: 23,
                    y: 35,
                    ..desired
                },
                29
            ));
            assert_eq!(attributes(second), (23, 35, 312, 198, 2));
            assert_eq!(attributes(child), (21, 34, 312, 198, 2));
            for pid in [0, 31, 37, 999] {
                assert!(!set_embedded_page_visible_for_process(parent, pid, false));
                assert!(!resize_embedded_page_for_process(desired, pid));
            }
            for (kind, format, count) in
                [(31, 32, 1), (6, 8, 1), (6, 16, 1), (6, 32, 0), (6, 32, 2)]
            {
                set_pid(child, kind, format, count);
                unsafe {
                    sync(display, 0);
                }
                assert!(!set_embedded_page_visible_for_process(parent, 17, false));
                assert!(!resize_embedded_page_for_process(desired, 17));
                assert_eq!(attributes(child), (21, 34, 312, 198, 2));
                assert_eq!(attributes(second), (23, 35, 312, 198, 2));
            }
            unsafe {
                delete(display, child, pid_atom);
                sync(display, 0);
            }
            assert!(!set_embedded_page_visible_for_process(parent, 17, false));
            set_pid(child, 6, 32, 1);
            unsafe {
                sync(display, 0);
            }
            assert!(set_embedded_page_visible_for_process(parent, 17, false));
            assert!(set_embedded_page_visible_for_process(parent, 17, true));
            // A dead window produces a connection-local checked protocol error;
            // subsequent host operations still work, without a global handler.
            unsafe {
                destroy(display, wrong_class);
                sync(display, 0);
            }
            assert!(!embedded_xcb::checked_mapping_for_test(
                u32::try_from(wrong_class).unwrap()
            ));
            assert!(resize_embedded_page_for_process(desired, 17));
        }
        unsafe {
            destroy(display, parent);
            sync(display, 0);
        }
        owned.parent = 0;
        if owned_selector {
            assert!(!set_embedded_page_visible_for_process(parent, 17, true));
            assert!(!resize_embedded_page_for_process(slot, 17));
            // The legacy entry shares the same checked implementation.
            resize_embedded_page(slot);
        }
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
