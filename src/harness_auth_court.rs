//! Dedicated native context/live-page court. Owner: PRD 28 / PRD 31.01.02.
//! Build only with the non-default `native-auth-court` feature, never in a
//! production artifact.
//!
//! Required seam: HarnessSettings::for_native_auth_court(AttemptId), with no
//! Runtime, account, credential, listener, login transport, or grant creation.
//! Default product builds exclude this module and its command entry point.

#![cfg(feature = "native-auth-court")]

use super::*;
use std::io::{Read as CourtRead, Write as CourtWrite};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as CourtOrdering};
use std::sync::mpsc as court_channel;
use std::thread::JoinHandle;
use std::time::Duration;

const HTTP_LIMIT: usize = 65_536;
const MANIFEST_LIMIT: u64 = 32_768;

struct Recipe {
    address: SocketAddr,
    token: String,
    run: String,
    pages: std::collections::HashMap<String, String>,
    a: browser::PageContext,
    b: browser::PageContext,
    output: PathBuf,
    timeout: Duration,
}

fn hex16(value: &str) -> Result<[u8; 16], &'static str> {
    if value.len() != 32 || !value.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("native-court-identity-invalid");
    }
    let mut bytes = [0; 16];
    for (index, item) in bytes.iter_mut().enumerate() {
        *item = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| "native-court-identity-invalid")?;
    }
    Ok(bytes)
}

impl Recipe {
    fn read(path: &Path) -> Result<Self, &'static str> {
        let mut raw = Vec::new();
        std::fs::File::open(path)
            .map_err(|_| "native-court-manifest-unreadable")?
            .take(MANIFEST_LIMIT + 1)
            .read_to_end(&mut raw)
            .map_err(|_| "native-court-manifest-unreadable")?;
        if raw.len() as u64 > MANIFEST_LIMIT {
            return Err("native-court-manifest-bound");
        }
        let json: serde_json::Value =
            serde_json::from_slice(&raw).map_err(|_| "native-court-manifest-invalid")?;
        if json["schema"] != 1 || json["test_only"] != true {
            return Err("native-court-manifest-invalid");
        }
        let text = |name: &str| {
            json[name]
                .as_str()
                .map(str::to_owned)
                .ok_or("native-court-manifest-invalid")
        };
        let origin = text("origin")?;
        let address: SocketAddr = origin
            .strip_prefix("http://")
            .ok_or("native-court-origin-invalid")?
            .parse()
            .map_err(|_| "native-court-origin-invalid")?;
        if address.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
            || address.port() == 0
            || origin != format!("http://{address}")
        {
            return Err("native-court-origin-invalid");
        }
        let run = text("run")?;
        let token = text("driver_token")?;
        hex16(&run)?;
        hex16(&token)?;
        let output = PathBuf::from(text("output")?)
            .canonicalize()
            .map_err(|_| "native-court-output-invalid")?;
        // The caller supplies an isolated process environment before launching
        // this test artifact. Never mutate process-global environment here.
        for name in ["MINICON_CONFIG_DIR", "MINICON_BROWSER_PROFILE_ROOT"] {
            let expected = json["launch_environment"][name]
                .as_str()
                .ok_or("native-court-environment-invalid")?;
            let expected_path = Path::new(expected);
            let leaf = if name == "MINICON_CONFIG_DIR" {
                "config"
            } else {
                "ordinary-profile"
            };
            if expected_path.file_name() != Some(std::ffi::OsStr::new(leaf))
                || expected_path
                    .parent()
                    .and_then(|parent| parent.canonicalize().ok())
                    .as_ref()
                    != Some(&output)
            {
                return Err("native-court-environment-outside-output");
            }
            if std::env::var_os(name).as_deref() != Some(std::ffi::OsStr::new(expected)) {
                return Err("native-court-environment-not-isolated");
            }
        }
        for name in [
            "WEBVIEW2_USER_DATA_FOLDER",
            "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
            "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
            "MINICON_BROWSER_HTTP_PROXY",
            "MINICON_BROWSER_HTTPS_PROXY",
            "MINICON_BROWSER_NO_PROXY",
        ] {
            if std::env::var_os(name).is_some_and(|value| !value.is_empty()) {
                return Err("native-court-environment-override");
            }
        }
        let helper = std::env::var_os("MINICON_BROWSER_BIN");
        match (
            json.get("helper").and_then(serde_json::Value::as_str),
            helper.as_deref(),
        ) {
            (None, None) if !cfg!(target_os = "linux") => {}
            (Some(expected), Some(actual)) if cfg!(target_os = "linux") => {
                let expected = Path::new(expected);
                let actual = Path::new(actual);
                if !expected.is_absolute()
                    || !actual.is_absolute()
                    || !expected.is_file()
                    || !actual.is_file()
                    || expected.canonicalize().ok() != actual.canonicalize().ok()
                {
                    return Err("native-court-helper-identity-mismatch");
                }
            }
            _ => return Err("native-court-helper-not-configured"),
        }
        let profile = |name: &str| -> Result<browser::PageContext, &'static str> {
            let value = &json["profiles"][name];
            let directory = PathBuf::from(
                value["directory"]
                    .as_str()
                    .ok_or("native-court-profile-invalid")?,
            );
            let directory = directory
                .canonicalize()
                .map_err(|_| "native-court-profile-invalid")?;
            if !directory.starts_with(&output) || directory == output {
                return Err("native-court-profile-outside-output");
            }
            minicon_platform::filesystem::protect_private_directory(&directory)
                .map_err(|_| "native-court-profile-not-private")?;
            let context = browser::PageContext {
                profile: minicon_web::PageProfile {
                    data_directory: directory,
                    store_identifier: hex16(
                        value["store_identifier"]
                            .as_str()
                            .ok_or("native-court-profile-invalid")?,
                    )?,
                },
                proxy: browser::BrowserProxy::default(),
            };
            context
                .validate()
                .map_err(|_| "native-court-profile-invalid")?;
            Ok(context)
        };
        let a = profile("a")?;
        let b = profile("b")?;
        if a.profile.data_directory == b.profile.data_directory
            || a.profile.store_identifier == b.profile.store_identifier
        {
            return Err("native-court-profile-identities-shared");
        }
        let mut pages = std::collections::HashMap::new();
        for phase in ["ordinary", "a", "b", "a-reopen"] {
            let url = json["pages"][phase]
                .as_str()
                .ok_or("native-court-page-invalid")?;
            let prefix = format!("{origin}/page/{run}/{phase}/");
            let ticket = url
                .strip_prefix(&prefix)
                .ok_or("native-court-page-invalid")?;
            hex16(ticket)?;
            pages.insert(phase.to_owned(), url.to_owned());
        }
        let seconds = json["timeout_seconds"]
            .as_f64()
            .filter(|value| value.is_finite() && *value > 0.0 && *value <= 180.0)
            .ok_or("native-court-deadline-invalid")?;
        Ok(Self {
            address,
            token,
            run,
            pages,
            a,
            b,
            output,
            timeout: Duration::from_secs_f64(seconds),
        })
    }
}

#[derive(Clone)]
struct Action {
    seq: u64,
    kind: String,
    phase: String,
    url: Option<String>,
}

enum Notice {
    Action(Action),
    Complete(serde_json::Value),
    Failed(&'static str),
}

// All HTTP runs on this owned worker. GUI callbacks never do socket I/O.
fn request(
    address: SocketAddr,
    token: &str,
    path: &str,
    body: Option<&[u8]>,
) -> Result<serde_json::Value, &'static str> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut socket = TcpStream::connect_timeout(&address, Duration::from_millis(500))
        .map_err(|_| "native-court-fixture-connect")?;
    socket
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|_| "native-court-fixture-io")?;
    let payload = body.unwrap_or_default();
    let method = if body.is_some() { "POST" } else { "GET" };
    let header = format!(
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nX-Court-Token: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    socket
        .write_all(header.as_bytes())
        .and_then(|_| socket.write_all(payload))
        .map_err(|_| "native-court-fixture-io")?;
    let mut bytes = Vec::new();
    let mut chunk = [0; 2048];
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("native-court-fixture-timeout")?;
        socket
            .set_read_timeout(Some(remaining))
            .map_err(|_| "native-court-fixture-io")?;
        let count = socket
            .read(&mut chunk)
            .map_err(|_| "native-court-fixture-io")?;
        if count == 0 {
            break;
        }
        if bytes.len() + count > HTTP_LIMIT {
            return Err("native-court-fixture-response-bound");
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    let split = bytes
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .ok_or("native-court-fixture-response-invalid")?;
    if split > 8192 {
        return Err("native-court-fixture-header-bound");
    }
    let headers = std::str::from_utf8(&bytes[..split])
        .map_err(|_| "native-court-fixture-response-invalid")?;
    if !headers
        .lines()
        .next()
        .is_some_and(|line| line.starts_with("HTTP/1.1 200 "))
    {
        return Err("native-court-fixture-rejected");
    }
    let lengths: Vec<_> = headers
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .collect();
    if lengths.len() != 1
        || lengths[0].1.trim().parse::<usize>().ok() != Some(bytes.len() - split - 4)
        || headers
            .lines()
            .any(|line| line.to_ascii_lowercase().starts_with("transfer-encoding:"))
    {
        return Err("native-court-fixture-framing-invalid");
    }
    serde_json::from_slice(&bytes[split + 4..]).map_err(|_| "native-court-fixture-json-invalid")
}

struct Worker {
    notices: court_channel::Receiver<Notice>,
    acknowledgements: court_channel::SyncSender<u64>,
    stop: Arc<AtomicBool>,
    overflow: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    fn start(recipe: &Recipe, waker: WindowWaker) -> Self {
        let (send, notices) = court_channel::sync_channel(8);
        let (acknowledgements, receive) = court_channel::sync_channel::<u64>(8);
        let stop = Arc::new(AtomicBool::new(false));
        let overflow = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker_overflow = overflow.clone();
        let address = recipe.address;
        let token = recipe.token.clone();
        let run = recipe.run.clone();
        let deadline = Instant::now() + recipe.timeout;
        let handle = std::thread::spawn(move || {
            let emit = |notice| {
                if send.try_send(notice).is_err() {
                    worker_overflow.store(true, CourtOrdering::Release);
                }
                let _ = waker.wake();
            };
            let result = (|| -> Result<(), &'static str> {
                let mut last = None;
                while !worker_stop.load(CourtOrdering::Acquire) && Instant::now() < deadline {
                    while let Ok(seq) = receive.try_recv() {
                        request(
                            address,
                            &token,
                            "/driver/ack",
                            Some(serde_json::json!({"seq":seq}).to_string().as_bytes()),
                        )?;
                    }
                    let view = request(address, &token, "/driver/status", None)?;
                    if view["run"].as_str() != Some(run.as_str()) {
                        return Err("native-court-run-identity-mismatch");
                    }
                    match view["state"].as_str() {
                        Some("failed") => return Err("native-court-page-observation-failed"),
                        Some("observations-complete") => {
                            let receipt = request(address, &token, "/driver/receipt", None)?;
                            emit(Notice::Complete(receipt));
                            return Ok(());
                        }
                        Some("observing") => {}
                        _ => return Err("native-court-fixture-state-invalid"),
                    }
                    if let Some(seq) = view["action"]["seq"]
                        .as_u64()
                        .filter(|seq| last != Some(*seq))
                    {
                        let action = &view["action"];
                        emit(Notice::Action(Action {
                            seq,
                            kind: action["kind"]
                                .as_str()
                                .ok_or("native-court-action-invalid")?
                                .to_owned(),
                            phase: action["phase"]
                                .as_str()
                                .ok_or("native-court-action-invalid")?
                                .to_owned(),
                            url: action["url"].as_str().map(str::to_owned),
                        }));
                        last = Some(seq);
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                if worker_stop.load(CourtOrdering::Acquire) {
                    Ok(())
                } else {
                    Err("native-court-deadline-expired")
                }
            })();
            if let Err(code) = result {
                emit(Notice::Failed(code));
            }
        });
        Self {
            notices,
            acknowledgements,
            stop,
            overflow,
            handle: Some(handle),
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, CourtOrdering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct CourtApplication {
    app: ConApp,
    recipe: Recipe,
    worker: Option<Worker>,
    pending: Option<Action>,
    browser_id: Option<workspace::TabId>,
    deadline: Instant,
    outcome: Arc<Mutex<Option<Result<(), &'static str>>>>,
    fixture_receipt: Arc<Mutex<Option<serde_json::Value>>>,
    #[cfg(windows)]
    diagnostics: Arc<Mutex<Vec<serde_json::Value>>>,
    #[cfg(windows)]
    private_locators: Arc<Mutex<std::collections::BTreeMap<String, serde_json::Value>>>,
}

impl CourtApplication {
    #[cfg(windows)]
    fn record_context_observation(&self) {
        let Some(auth) = &self.app.browser_auth else {
            return;
        };
        let phase = match auth.attempt.0 {
            101 => "a",
            102 => "b",
            103 => "a-reopen",
            _ => "unrecognized-attempt",
        };
        if matches!(phase, "a" | "b" | "a-reopen")
            && let Some(page) = self.app.native_browser_page.as_ref()
            && let Ok(relative) =
                page.context_profile_relative_directory_for_diagnostics(&auth.context.profile)
            && let Some(relative) = relative.to_str()
            && let Ok(mut locators) = self.private_locators.lock()
        {
            locators.insert(
                phase.to_owned(),
                serde_json::json!({
                    "store_identifier": auth.context.profile.store_identifier,
                    "relative_directory": relative,
                }),
            );
        }
        let observation = serde_json::json!({
            "phase": phase,
            "native_host_present": self.app.native_browser_page.is_some(),
            "persistent_profile_verified": self.app.native_browser_page.as_ref()
                .map(|page| page.context_profile_matches(&auth.context.profile)),
            "persistent_profile_layout": self.app.native_browser_page.as_ref()
                .map(|page| page.context_profile_layout(&auth.context.profile))
                .unwrap_or("unavailable"),
            "owned_profile_release": self.app.auth_context_lifecycle.release_observation(&auth.context.profile),
        });
        if let Ok(mut observations) = self.diagnostics.lock()
            && observations.last() != Some(&observation)
        {
            if observations.len() == 16 {
                observations.remove(0);
            }
            observations.push(observation);
        }
    }

    fn fail(&self, code: &'static str) -> PixelWindowError {
        if let Ok(mut result) = self.outcome.lock() {
            *result = Some(Err(code));
        }
        PixelWindowError::failed("native_auth_court_failed", code)
    }

    fn action(&mut self, window: &PixelWindow, action: Action) -> Result<(), PixelWindowError> {
        if self.pending.is_some() {
            return Err(self.fail("native-court-overlapping-actions"));
        }
        match action.kind.as_str() {
            "ordinary" if action.seq == 0 && action.phase == "ordinary" => {
                let url = self.recipe.pages.get("ordinary").unwrap();
                if action.url.as_deref() != Some(url.as_str())
                    || !self.app.present_http_link(window, url)
                {
                    return Err(self.fail("native-court-ordinary-navigation-failed"));
                }
                self.browser_id = self.app.workspace.browser_tab();
            }
            "present-auth" if (1..=3).contains(&action.seq) => {
                let expected_phase = ["a", "b", "a-reopen"][(action.seq - 1) as usize];
                let url = self.recipe.pages.get(expected_phase).unwrap();
                if action.phase != expected_phase || action.url.as_deref() != Some(url.as_str()) {
                    return Err(self.fail("native-court-auth-action-invalid"));
                }
                let context = if action.phase == "b" {
                    self.recipe.b.clone()
                } else {
                    self.recipe.a.clone()
                };
                let attempt = provider_runtime::AttemptId(100 + action.seq);
                self.app.harness_settings = Some(
                    harness_settings_ui::HarnessSettings::for_native_auth_court(attempt),
                );
                self.app.open_harness_auth(window, attempt, context, url);
                if !self
                    .app
                    .browser_auth
                    .as_ref()
                    .is_some_and(|auth| auth.attempt == attempt)
                {
                    return Err(self.fail("native-court-auth-presentation-refused"));
                }
            }
            "close-auth" if action.seq == 4 && action.phase == "ordinary-return" => {
                let id = self
                    .browser_id
                    .ok_or_else(|| self.fail("native-court-browser-missing"))?;
                if self.app.browser_auth.is_none()
                    || self.app.detach_session(id)
                    || self.app.workspace.browser_tab() != Some(id)
                    || self.app.browser_auth.is_some()
                {
                    return Err(self.fail("native-court-first-close-did-not-restore"));
                }
                // Closing auth deliberately returns to Harness. Dismiss that
                // panel before observing the returned ordinary native page.
                self.app.settings_open = false;
                self.app.sync_embedded_page(window);
                self.app.mark_host_ui_full_and_repaint(window);
            }
            _ => return Err(self.fail("native-court-action-invalid")),
        }
        self.pending = Some(action);
        Ok(())
    }

    fn turn(&mut self, window: &PixelWindow) -> Result<(), PixelWindowError> {
        if Instant::now() >= self.deadline {
            return Err(self.fail("native-court-deadline-expired"));
        }
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.overflow.load(CourtOrdering::Acquire))
        {
            return Err(self.fail("native-court-notice-queue-overflow"));
        }
        loop {
            let notice = self
                .worker
                .as_ref()
                .and_then(|worker| worker.notices.try_recv().ok());
            match notice {
                Some(Notice::Action(action)) => self.action(window, action)?,
                Some(Notice::Failed(code)) => return Err(self.fail(code)),
                Some(Notice::Complete(receipt)) => {
                    if self.pending.is_some()
                        || self.app.browser_auth.is_some()
                        || self.app.workspace.browser_tab() != self.browser_id
                    {
                        return Err(self.fail("native-court-final-presentation-mismatch"));
                    }
                    let checks = [
                        "ordinary-initial-storage",
                        "a-initial-storage",
                        "b-initial-storage",
                        "a-reopen-initial-storage",
                        "ordinary-live-dom-cookie-storage-return",
                        "ordinary-one-document-load",
                        "ordinary-native-history-back",
                        "ordinary-native-history-forward",
                    ];
                    if receipt["observation_status"] != "complete"
                        || !checks.iter().all(|name| receipt["checks"][*name] == true)
                    {
                        return Err(self.fail("native-court-fixture-receipt-incomplete"));
                    }
                    if let Ok(mut value) = self.fixture_receipt.lock() {
                        *value = Some(receipt);
                    }
                    if let Ok(mut result) = self.outcome.lock() {
                        *result = Some(Ok(()));
                    }
                    self.app.exit = true;
                }
                None => break,
            }
        }
        if self.pending.is_some() {
            let ready = self
                .app
                .workspace
                .browser_tab()
                .and_then(|id| self.app.browsers.get(&id))
                .is_some_and(|tab| matches!(tab.panel().load, browser::PageLoad::Ready { .. }));
            #[cfg(target_os = "linux")]
            let ready = ready
                && self.app.content_panel_slot(window).is_some_and(|slot| {
                    self.app.browser_page.as_ref().is_some_and(|child| {
                        self.app
                            .browser_process
                            .as_ref()
                            .is_some_and(|process| process.embedded_ready(slot.parent, child.id()))
                    })
                });
            if ready {
                let seq = self.pending.take().unwrap().seq;
                if self
                    .worker
                    .as_ref()
                    .unwrap()
                    .acknowledgements
                    .try_send(seq)
                    .is_err()
                {
                    return Err(self.fail("native-court-ack-queue-full"));
                }
            }
        }
        Ok(())
    }
}

impl PixelWindowApplication for CourtApplication {
    fn opened(&mut self, window: &PixelWindow) -> Result<PixelWindowDirective, PixelWindowError> {
        self.app.opened(window)?;
        self.worker = Some(Worker::start(&self.recipe, window.waker()));
        Ok(PixelWindowDirective::Continue)
    }

    fn event(
        &mut self,
        window: &PixelWindow,
        event: PixelWindowEvent,
    ) -> Result<PixelWindowDirective, PixelWindowError> {
        if matches!(event, PixelWindowEvent::CloseRequested)
            && self
                .outcome
                .lock()
                .ok()
                .is_some_and(|value| value.is_none())
        {
            return Err(self.fail("native-court-closed-before-observations"));
        }
        self.app.event(window, event)
    }

    fn render(
        &mut self,
        window: &PixelWindow,
        frame: &mut XrgbPixelFrame<'_>,
    ) -> Result<PixelWindowDirective, PixelWindowError> {
        self.app.render(window, frame)
    }

    fn about_to_wait(
        &mut self,
        window: &PixelWindow,
        now: Instant,
    ) -> Result<PixelWindowDirective, PixelWindowError> {
        let directive = self.app.about_to_wait(window, now)?;
        #[cfg(windows)]
        self.record_context_observation();
        self.turn(window)?;
        if self.app.exit {
            return Ok(PixelWindowDirective::Exit);
        }
        let next = (now + Duration::from_millis(100)).min(self.deadline);
        Ok(match directive {
            PixelWindowDirective::Exit => PixelWindowDirective::Exit,
            PixelWindowDirective::Continue => PixelWindowDirective::Continue,
            PixelWindowDirective::WaitUntil(current) => {
                PixelWindowDirective::WaitUntil(current.min(next))
            }
            PixelWindowDirective::Wait => PixelWindowDirective::WaitUntil(next),
            _ => return Err(self.fail("native-court-window-directive-unsupported")),
        })
    }
}

impl Drop for CourtApplication {
    fn drop(&mut self) {
        self.worker = None;
        // At most one auth-close restoration, then one ordinary teardown.
        // Invoke the real teardown path rather than dropping live Child handles.
        for _ in 0..2 {
            if let Some(id) = self.app.workspace.browser_tab() {
                self.app.detach_session(id);
            }
        }
    }
}

/// Invoke from the executable's main thread, before ordinary product arg
/// handling. The default build must not compile or accept this entry point.
pub(super) fn run_application(manifest: &Path) -> Result<(), &'static str> {
    let recipe = Recipe::read(manifest)?;
    let output = recipe.output.clone();
    #[cfg(windows)]
    let locator_run = recipe.run.clone();
    #[cfg(windows)]
    let private_locators = Arc::new(Mutex::new(std::collections::BTreeMap::new()));
    let deadline = Instant::now() + recipe.timeout;
    let outcome = Arc::new(Mutex::new(None));
    let receipt = Arc::new(Mutex::new(None));
    #[cfg(windows)]
    let diagnostics = Arc::new(Mutex::new(Vec::new()));
    let mut app = ConApp::new(None, None);
    app.no_activate = true;
    let native = CourtApplication {
        app,
        recipe,
        worker: None,
        pending: None,
        browser_id: None,
        deadline,
        outcome: outcome.clone(),
        fixture_receipt: receipt.clone(),
        #[cfg(windows)]
        diagnostics: diagnostics.clone(),
        #[cfg(windows)]
        private_locators: private_locators.clone(),
    };
    let result = run_pixel_window(
        PixelWindowOptions::new("MiniCon native auth court", LogicalSize::new(960.0, 600.0))
            .with_no_activate(true)
            .with_ime_allowed(true),
        Box::new(native),
    );
    // The window has returned. No filesystem publication is added to GUI
    // callbacks, and a failed optional diagnostic never changes the oracle.
    #[cfg(windows)]
    let private_locator_status = (|| {
        let locators = private_locators.lock().ok()?;
        if locators.len() > 3 {
            return None;
        }
        let bytes = serde_json::to_vec(&serde_json::json!({
            "schema": 1, "test_only": true, "run": locator_run, "phases": &*locators,
        }))
        .ok()?;
        if bytes.len() > 16_384 {
            return None;
        }
        minicon_platform::filesystem::write_private_atomic(
            &output.join("native-profile-locator.private.json"),
            &bytes,
        )
        .ok()?;
        Some(())
    })()
    .map_or("unavailable", |_| "written");
    let observed = outcome
        .lock()
        .map_err(|_| "native-court-outcome-lock")?
        .unwrap_or(Err("native-court-observations-not-complete"));
    let unsupported = matches!(&result, Err(PixelWindowError::Unsupported { .. }));
    let failure = if unsupported {
        Some("native-court-native-host-unsupported")
    } else {
        observed.err().or(if result.is_err() {
            Some("native-court-window-failed")
        } else {
            None
        })
    };
    let status = if unsupported {
        "BLOCKED"
    } else if result.is_ok() && observed.is_ok() {
        "native-observations-complete"
    } else {
        "failed"
    };
    let features: Vec<_> = [
        cfg!(feature = "default").then_some("default"),
        cfg!(feature = "experimental-rust-console").then_some("experimental-rust-console"),
        Some("native-auth-court"),
    ]
    .into_iter()
    .flatten()
    .collect();
    let data = serde_json::json!({
        "kind":"native-auth-court-test-build", "status":status,
        "test_only":true, "production_test_build":true,
        "features":features,
        "os":std::env::consts::OS, "isa":std::env::consts::ARCH,
        "failure_code":failure,
        "production_artifact_qualification":false, "provider_authorization_claimed":false,
        "fixture":receipt.lock().map_err(|_| "native-court-outcome-lock")?.take(),
        "unclaimed":["proxy/no-direct", "delayed embedded_ready", "native map/focus/pixels", "physical input/IME/clipboard", "toolbar-driven history"],
    });
    #[cfg(windows)]
    let data = {
        let mut data = data;
        data["private_profile_locator"] = serde_json::json!(private_locator_status);
        data["windows_context_observations"] = serde_json::json!(
            diagnostics
                .lock()
                .ok()
                .map(|observations| observations.clone())
        );
        data
    };
    std::fs::write(
        output.join("native-driver-receipt.json"),
        serde_json::to_vec_pretty(&data).map_err(|_| "native-court-receipt-serialize")?,
    )
    .map_err(|_| "native-court-receipt-write")?;
    if let Some(failure) = failure {
        return Err(failure);
    }
    observed
}
