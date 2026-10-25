//! Direct pinned-wry adapter for the unchanged native fixture protocol.
//! Test-only adapter; lifetime behavior and evidence scope belong to PRD 28.01.
use minicon_web::{PageContext, PageProfile};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    path::Path,
    rc::Rc,
    time::{Duration, Instant},
};
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    platform::run_return::EventLoopExtRunReturn,
    window::{Window, WindowBuilder},
};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2_13, ICoreWebView2Environment, ICoreWebView2Environment7,
};
use windows_core::{IUnknown, Interface};
use wry::{WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows, WebViewExtWindows};

// Physical source reuse, not a second HTTP client, Cookie reader or exit observer.
#[path = "../../src/native_page/windows_cookie_diagnostics.rs"]
mod cookies;
#[path = "../../tests/support/native_auth_fixture.rs"]
mod fixture_protocol;
#[path = "../../src/native_page/windows_release.rs"]
mod release;
use fixture_protocol::{Action, Notice, Recipe, WindowsProfileAclMode, Worker};
use release::{NativePageRelease, NativePageReleaseStatus as Exit};

const OPTIONS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --autoplay-policy=no-user-gesture-required --no-proxy-server";
const CHECKS: [&str; 8] = [
    "ordinary-initial-storage",
    "a-initial-storage",
    "b-initial-storage",
    "a-reopen-initial-storage",
    "ordinary-live-dom-cookie-storage-return",
    "ordinary-one-document-load",
    "ordinary-native-history-back",
    "ordinary-native-history-forward",
];
type Result<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    KeepBrowser,
    NormalExit,
}
impl Mode {
    fn read(s: &str) -> Result<Self> {
        match s {
            "keep-browser" => Ok(Self::KeepBrowser),
            "normal-exit" => Ok(Self::NormalExit),
            _ => Err("lifetime-mode-invalid"),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::KeepBrowser => "keep-browser",
            Self::NormalExit => "normal-exit",
        }
    }
}

// Used by the actual reopen consumer before any controller is constructed.
fn authorize_reopen(
    mode: Mode,
    keeper: bool,
    environment: bool,
    exit: Option<Exit>,
    matched: bool,
) -> bool {
    match mode {
        Mode::KeepBrowser => keeper && environment && exit == Some(Exit::Pending),
        Mode::NormalExit => !keeper && !environment && exit.is_none() && matched,
    }
}

/// Original action authority, separate from current page/probe state.
fn authorized(recipe: &Recipe, action: &Action) -> bool {
    let (phase, kind) = match action.seq {
        0 => ("ordinary", "ordinary"),
        1 => ("a", "present-auth"),
        2 => ("b", "present-auth"),
        3 => ("a-reopen", "present-auth"),
        4 => ("ordinary-return", "close-auth"),
        _ => return false,
    };
    action.phase == phase
        && action.kind == kind
        && if action.seq == 4 {
            action.url.is_none()
        } else {
            action.url.as_ref() == recipe.pages.get(phase)
        }
}

// wry NavigationCompleted reads CurrentSource after fixture JS establishes its
// ordinary History entry. Only that exact original history query is admitted.
fn finished_target(actual: Option<&str>, expected: &str, ordinary: bool) -> bool {
    actual.is_some_and(|actual| {
        actual == expected || (ordinary && actual == format!("{expected}?step=two"))
    })
}

fn profile_facts(
    view: &WebView,
    expected: &PageProfile,
) -> Result<(std::path::PathBuf, &'static str)> {
    let env = view
        .environment()
        .cast::<ICoreWebView2Environment7>()
        .map_err(|_| "lifetime-profile-api-unavailable")?;
    let mut root = Default::default();
    let mut path = Default::default();
    let mut private = windows_core::BOOL::default();
    unsafe { env.UserDataFolder(&mut root) }.map_err(|_| "lifetime-profile-unavailable")?;
    let root = std::path::PathBuf::from(webview2_com::take_pwstr(root))
        .canonicalize()
        .map_err(|_| "lifetime-profile-unavailable")?;
    let expected = expected
        .data_directory
        .canonicalize()
        .map_err(|_| "lifetime-profile-unavailable")?;
    let native = unsafe {
        view.webview()
            .cast::<ICoreWebView2_13>()
            .and_then(|v| v.Profile())
    }
    .map_err(|_| "lifetime-profile-api-unavailable")?;
    unsafe { native.IsInPrivateModeEnabled(&mut private) }
        .map_err(|_| "lifetime-profile-unavailable")?;
    unsafe { native.ProfilePath(&mut path) }.map_err(|_| "lifetime-profile-unavailable")?;
    let profile = std::path::PathBuf::from(webview2_com::take_pwstr(path))
        .canonicalize()
        .map_err(|_| "lifetime-profile-unavailable")?;
    if private.as_bool() || root != expected || profile == root || !profile.starts_with(&root) {
        return Err("lifetime-profile-identity-mismatch");
    }
    // Reuse the common canonical/link-like context validation, not discovery.
    PageContext {
        profile: PageProfile {
            data_directory: profile.clone(),
            store_identifier: [1; 16],
        },
        proxy: Default::default(),
    }
    .validate()
    .map_err(|_| "lifetime-profile-link-like")?;
    let relative = profile
        .strip_prefix(root)
        .map_err(|_| "lifetime-profile-identity-mismatch")?
        .to_path_buf();
    let layout = if relative == Path::new("EBWebView/Default") {
        "EBWebView/Default"
    } else if relative == Path::new("Default") {
        "Default"
    } else {
        "other-descendant"
    };
    Ok((relative, layout))
}
fn same_environment(a: &ICoreWebView2Environment, b: &ICoreWebView2Environment) -> Result<bool> {
    let a = a
        .cast::<IUnknown>()
        .map_err(|_| "lifetime-environment-identity-unavailable")?;
    let b = b
        .cast::<IUnknown>()
        .map_err(|_| "lifetime-environment-identity-unavailable")?;
    Ok(a.as_raw() == b.as_raw())
}
fn browser_pid(view: &WebView) -> Result<u32> {
    let mut pid = 0;
    unsafe { view.webview().BrowserProcessId(&mut pid) }
        .map_err(|_| "lifetime-browser-identity-unavailable")?;
    if pid == 0 {
        Err("lifetime-browser-identity-unavailable")
    } else {
        Ok(pid)
    }
}

struct Page {
    view: WebView,
    _context: WebContext,
    loaded: Rc<RefCell<Option<String>>>,
    runtime: String,
    relative: std::path::PathBuf,
    layout: &'static str,
}
impl Page {
    fn create(
        window: &Window,
        profile: &PageProfile,
        environment: Option<&ICoreWebView2Environment>,
        recipe: &Recipe,
        wake: impl Fn() + 'static,
    ) -> Result<Self> {
        let mut context = WebContext::new(Some(profile.data_directory.clone()));
        let loaded = Rc::new(RefCell::new(None));
        let completion = loaded.clone();
        let pages: Vec<_> = recipe.pages.values().cloned().collect();
        let mut builder = WebViewBuilder::new_with_web_context(&mut context)
            .with_incognito(false)
            .with_focused(false)
            .with_additional_browser_args(OPTIONS)
            .with_navigation_handler(move |url| {
                url == "about:blank"
                    || pages
                        .iter()
                        .any(|p| url == *p || url.starts_with(&(p.clone() + "?")))
            })
            .with_on_page_load_handler(move |event, url| {
                if matches!(event, wry::PageLoadEvent::Finished) {
                    *completion.borrow_mut() = Some(url);
                    wake();
                }
            })
            .with_html("<!doctype html><title>Fixture blank controller</title>");
        if let Some(env) = environment {
            builder = builder.with_environment(env.clone());
        }
        let view = builder
            .build_as_child(window)
            .map_err(|_| "lifetime-native-create-failed")?;
        let (relative, layout) = profile_facts(&view, profile)?;
        let mut version = Default::default();
        unsafe { view.environment().BrowserVersionString(&mut version) }
            .map_err(|_| "lifetime-runtime-unavailable")?;
        let runtime = webview2_com::take_pwstr(version);
        if runtime.is_empty()
            || runtime.len() > 64
            || !runtime.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        {
            return Err("lifetime-runtime-invalid");
        }
        Ok(Self {
            view,
            _context: context,
            loaded,
            runtime,
            relative,
            layout,
        })
    }
    fn ready(&self) -> bool {
        self.loaded.borrow().is_some()
    }
    fn loaded(&self, url: &str, ordinary: bool) -> bool {
        finished_target(self.loaded.borrow().as_deref(), url, ordinary)
    }
    fn navigate(&self, url: &str) -> Result<()> {
        *self.loaded.borrow_mut() = None;
        self.view
            .load_url(url)
            .map_err(|_| "lifetime-navigation-failed")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Home,
    BeforeRelease,
    AwaitExit,
    BeforeNavigation,
    Navigating,
    OrdinaryReturn,
    Cleanup,
}
struct PendingProbe {
    probe: cookies::NativeCookieMetadataProbe,
    seq: u64,
    stage: Stage,
}
struct Host {
    recipe: Recipe,
    mode: Mode,
    deadline: Instant,
    worker: Option<Worker>,
    ordinary: Option<Page>,
    active: Option<Page>,
    keeper: Option<Page>,
    a_environment: Option<ICoreWebView2Environment>,
    a_monitor: Option<NativePageRelease>,
    closing: Vec<NativePageRelease>,
    a_pid: u32,
    matched_a_exit: bool,
    reopened_same_browser: bool,
    environment_generation: u8,
    runtime: Option<String>,
    pending: Option<Action>,
    stage: Stage,
    probe: Option<PendingProbe>,
    exit_deadline: Option<Instant>,
    last_ack: Option<u64>,
    fixture: Option<Value>,
    original_complete: bool,
    failure: Option<&'static str>,
    boundary_failure: Option<&'static str>,
    done: bool,
    trace: Vec<Value>,
    metadata: Vec<Value>,
    locators: BTreeMap<String, Value>,
    contexts: Vec<Value>,
}
impl Host {
    fn record(&mut self, phase: &str, status: &str) -> Result<()> {
        if self.trace.len() >= 30 {
            return Err("lifetime-trace-bound");
        }
        self.trace.push(json!({"phase":phase,"status":status}));
        Ok(())
    }
    fn runtime(&mut self, page: &Page) -> Result<()> {
        if self.runtime.as_ref().is_some_and(|v| v != &page.runtime) {
            return Err("lifetime-runtime-changed");
        }
        self.runtime = Some(page.runtime.clone());
        Ok(())
    }
    fn context(&mut self, phase: &str, page: &Page, profile: &PageProfile) -> Result<()> {
        self.runtime(page)?;
        self.locators.insert(
            phase.into(),
            json!({"relative_directory":page.relative,"store_identifier":profile.store_identifier}),
        );
        self.contexts.push(json!({"phase":phase,"persistent_profile_verified":true,"persistent_profile_layout":page.layout,"native_host_present":true}));
        Ok(())
    }
    fn check_pending(&self) -> Result<&Action> {
        let a = self.pending.as_ref().ok_or("lifetime-pending-missing")?;
        if !authorized(&self.recipe, a) {
            return Err("lifetime-pending-authority-changed");
        }
        Ok(a)
    }
    fn begin_action(&mut self, window: &Window, a: Action) -> Result<()> {
        if self.pending.is_some() || !authorized(&self.recipe, &a) {
            return Err("lifetime-action-invalid");
        }
        if a.seq != self.last_ack.map_or(0, |seq| seq + 1) {
            return Err("lifetime-action-sequence-invalid");
        }
        let seq = a.seq;
        self.pending = Some(a);
        match seq {
            0 => {
                let directory = self.recipe.output.join("ordinary-profile");
                std::fs::create_dir_all(&directory)
                    .map_err(|_| "lifetime-ordinary-profile-unavailable")?;
                minicon_platform::filesystem::protect_private_directory(&directory)
                    .map_err(|_| "lifetime-ordinary-profile-unavailable")?;
                let p = PageProfile {
                    data_directory: directory
                        .canonicalize()
                        .map_err(|_| "lifetime-ordinary-profile-unavailable")?,
                    store_identifier: [1; 16],
                };
                let page = Page::create(window, &p, None, &self.recipe, || {})?;
                self.runtime(&page)?;
                self.ordinary = Some(page);
                self.stage = Stage::Home;
            }
            1 => {
                self.ordinary
                    .as_ref()
                    .ok_or("lifetime-ordinary-missing")?
                    .view
                    .set_visible(false)
                    .map_err(|_| "lifetime-visibility-failed")?;
                let profile = self.recipe.a.profile.clone();
                let page = Page::create(window, &profile, None, &self.recipe, || {})?;
                self.a_environment = Some(page.view.environment());
                self.a_pid = browser_pid(&page.view)?;
                self.context("a", &page, &profile)?;
                self.active = Some(page);
                self.stage = Stage::Home;
            }
            2 => {
                let a = self.active.as_ref().ok_or("lifetime-a-missing")?;
                let env = self
                    .a_environment
                    .as_ref()
                    .ok_or("lifetime-environment-missing")?;
                let keeper = Page::create(
                    window,
                    &self.recipe.a.profile,
                    Some(env),
                    &self.recipe,
                    || {},
                )?;
                if !same_environment(&keeper.view.environment(), env)?
                    || browser_pid(&keeper.view)? != self.a_pid
                {
                    return Err("lifetime-keeper-identity-mismatch");
                }
                self.runtime = Some(a.runtime.clone());
                keeper
                    .view
                    .set_visible(false)
                    .map_err(|_| "lifetime-visibility-failed")?;
                self.keeper = Some(keeper);
                self.stage = Stage::BeforeRelease;
            }
            3 => {
                if !authorize_reopen(
                    self.mode,
                    self.keeper.is_some(),
                    self.a_environment.is_some(),
                    self.a_monitor.as_ref().map(|m| m.status()),
                    self.matched_a_exit,
                ) {
                    return Err("lifetime-reopen-boundary-invalid");
                }
                self.close_active();
                let profile = self.recipe.a.profile.clone();
                let env = match self.mode {
                    Mode::KeepBrowser => {
                        let keeper = self.keeper.as_ref().ok_or("lifetime-keeper-missing")?;
                        if self.a_monitor.as_ref().map(|m| m.status()) != Some(Exit::Pending)
                            || browser_pid(&keeper.view)? != self.a_pid
                        {
                            return Err("lifetime-keeper-not-live");
                        }
                        Some(
                            self.a_environment
                                .as_ref()
                                .ok_or("lifetime-environment-missing")?,
                        )
                    }
                    Mode::NormalExit => {
                        if !self.matched_a_exit
                            || self.keeper.is_some()
                            || self.a_environment.is_some()
                        {
                            return Err("lifetime-normal-exit-not-proven");
                        }
                        None
                    }
                };
                let page = Page::create(window, &profile, env, &self.recipe, || {})?;
                if let Some(env) = env {
                    if !same_environment(&page.view.environment(), env)?
                        || browser_pid(&page.view)? != self.a_pid
                    {
                        return Err("lifetime-reopen-live-identity-mismatch");
                    }
                    self.reopened_same_browser = true;
                } else {
                    self.environment_generation = 2;
                }
                self.context("a-reopen", &page, &profile)?;
                self.active = Some(page);
                self.stage = Stage::Home;
            }
            4 => {
                self.close_active();
                self.ordinary
                    .as_ref()
                    .ok_or("lifetime-ordinary-missing")?
                    .view
                    .set_visible(true)
                    .map_err(|_| "lifetime-visibility-failed")?;
                self.stage = Stage::OrdinaryReturn;
            }
            _ => return Err("lifetime-action-invalid"),
        }
        Ok(())
    }
    fn close_active(&mut self) {
        if let Some(page) = self.active.take() {
            self.closing
                .push(NativePageRelease::observe(&page.view, || {}));
            drop(page);
        }
    }
    fn begin_cleanup(&mut self) {
        self.probe = None;
        self.pending = None;
        self.close_active();
        if let Some(page) = self.ordinary.take() {
            self.closing
                .push(NativePageRelease::observe(&page.view, || {}));
            drop(page);
        }
        self.keeper = None;
        self.a_environment = None;
        if let Some(m) = self.a_monitor.take() {
            self.closing.push(m);
        }
        self.stage = Stage::Cleanup;
        self.exit_deadline = Some(self.deadline.min(Instant::now() + Duration::from_secs(8)));
    }
    fn metadata(
        &mut self,
        summary: cookies::NativeCookieMetadataSummary,
        phase: &str,
        stage: &str,
    ) -> Result<()> {
        if self.metadata.len() >= 4 {
            return Err("lifetime-cookie-metadata-bound");
        }
        if summary.status != cookies::NativeCookieMetadataStatus::Observed {
            self.boundary_failure
                .get_or_insert("lifetime-cookie-metadata-unavailable");
        }
        self.metadata.push(json!({"phase":phase,"stage":stage,"status":summary.status.as_str(),"counts":summary.counts.map(|c|json!({"total":c.total,"session":c.session,"persistent":c.persistent,"expiry_future":c.expiry_future,"expiry_elapsed":c.expiry_elapsed,"expiry_unknown":c.expiry_unknown}))}));
        Ok(())
    }
    fn probe(&mut self, phase: &str) -> Result<Option<cookies::NativeCookieMetadataSummary>> {
        let action = self.check_pending()?;
        let seq = action.seq;
        if self.probe.is_none() {
            let page = self.active.as_ref().ok_or("lifetime-probe-host-missing")?;
            let uri = self
                .recipe
                .pages
                .get(phase)
                .ok_or("lifetime-probe-recipe-invalid")?;
            let deadline = self.deadline.min(Instant::now() + Duration::from_secs(2));
            let profile = if phase == "b" {
                &self.recipe.b.profile
            } else {
                &self.recipe.a.profile
            };
            let probe = if profile_facts(&page.view, profile).is_ok() {
                cookies::NativeCookieMetadataProbe::start(&page.view, uri, seq + 1, deadline, || {})
            } else {
                cookies::NativeCookieMetadataProbe::unavailable(
                    cookies::NativeCookieMetadataStatus::ContextUnavailable,
                    deadline,
                )
            };
            self.probe = Some(PendingProbe {
                probe,
                seq,
                stage: self.stage,
            });
        }
        let probe = self.probe.as_mut().unwrap();
        if probe.seq != seq || probe.stage != self.stage {
            return Err("lifetime-probe-authority-changed");
        }
        let result = probe.probe.poll(Instant::now());
        if result.is_some() {
            self.probe = None;
        }
        Ok(result)
    }
    fn ack(&mut self) -> Result<()> {
        let action = self.check_pending()?;
        let seq = action.seq;
        self.worker
            .as_ref()
            .ok_or("lifetime-worker-missing")?
            .acknowledgements
            .try_send(seq)
            .map_err(|_| "lifetime-ack-bound")?;
        self.last_ack = Some(seq);
        self.pending = None;
        Ok(())
    }
    fn create_b(&mut self, window: &Window) -> Result<()> {
        let p = self.recipe.b.profile.clone();
        let page = Page::create(window, &p, None, &self.recipe, || {})?;
        self.context("b", &page, &p)?;
        self.active = Some(page);
        self.stage = Stage::Home;
        Ok(())
    }
    fn step(&mut self, window: &Window) -> Result<()> {
        let now = Instant::now();
        if self.stage == Stage::Cleanup {
            if self.closing.iter().all(|m| m.status() == Exit::Released) {
                self.record("cleanup", "all-matching-normal-exits")?;
                self.done = true;
                return Ok(());
            }
            if self
                .closing
                .iter()
                .any(|m| matches!(m.status(), Exit::Failed(_)))
                || self.exit_deadline.is_some_and(|d| now >= d)
            {
                self.boundary_failure
                    .get_or_insert("lifetime-cleanup-normal-exit-unavailable");
                self.done = true;
            }
            return Ok(());
        }
        if now >= self.deadline {
            return Err("lifetime-deadline-expired");
        }
        if self.mode == Mode::KeepBrowser
            && self
                .a_monitor
                .as_ref()
                .is_some_and(|m| m.status() != Exit::Pending)
        {
            return Err("lifetime-kept-browser-exited");
        }
        if let Some(a) = &self.pending
            && !authorized(&self.recipe, a)
        {
            return Err("lifetime-pending-authority-changed");
        }
        match self.stage {
            Stage::Home => {
                let seq = self.check_pending()?.seq;
                let page = if seq == 0 {
                    self.ordinary.as_ref()
                } else {
                    self.active.as_ref()
                }
                .ok_or("lifetime-target-host-missing")?;
                if page.ready() {
                    self.stage = Stage::BeforeNavigation;
                }
            }
            Stage::BeforeNavigation => {
                let a = self.check_pending()?.clone();
                if a.seq > 0 {
                    let Some(summary) = self.probe(&a.phase)? else {
                        return Ok(());
                    };
                    self.metadata(summary, &a.phase, "before-target-navigation")?;
                }
                self.check_pending()?;
                let page = if a.seq == 0 {
                    self.ordinary.as_ref()
                } else {
                    self.active.as_ref()
                }
                .ok_or("lifetime-target-host-missing")?;
                page.navigate(a.url.as_deref().ok_or("lifetime-target-url-missing")?)?;
                self.stage = Stage::Navigating;
            }
            Stage::Navigating => {
                let a = self.check_pending()?;
                let page = if a.seq == 0 {
                    self.ordinary.as_ref()
                } else {
                    self.active.as_ref()
                }
                .ok_or("lifetime-target-host-missing")?;
                if page.loaded(
                    a.url.as_deref().ok_or("lifetime-target-url-missing")?,
                    a.seq == 0,
                ) {
                    self.ack()?;
                }
            }
            Stage::BeforeRelease => {
                if self.check_pending()?.seq != 2 || self.last_ack != Some(1) {
                    return Err("lifetime-release-action-invalid");
                }
                let Some(summary) = self.probe("a")? else {
                    return Ok(());
                };
                self.metadata(summary, "a", "before-context-release")?;
                self.check_pending()?;
                let page = self.active.take().ok_or("lifetime-a-missing")?;
                self.a_monitor = Some(NativePageRelease::observe(&page.view, || {}));
                drop(page);
                self.record("a-close", "target-closed-after-subscription")?;
                match self.mode {
                    Mode::KeepBrowser => {
                        self.record("a-close", "keeper-and-environment-retained")?;
                        self.create_b(window)?;
                    }
                    Mode::NormalExit => {
                        self.keeper = None;
                        self.stage = Stage::AwaitExit;
                        self.exit_deadline = Some(self.deadline.min(now + Duration::from_secs(8)));
                    }
                }
            }
            Stage::AwaitExit => match self
                .a_monitor
                .as_ref()
                .ok_or("lifetime-exit-observer-missing")?
                .status()
            {
                Exit::Pending if self.exit_deadline.is_some_and(|d| now < d) => {}
                Exit::Released => {
                    self.matched_a_exit = true;
                    self.a_monitor = None;
                    self.a_environment = None;
                    self.record("a-close", "matched-normal-exit-old-environment-released")?;
                    self.create_b(window)?;
                }
                _ => return Err("lifetime-normal-exit-unavailable"),
            },
            Stage::OrdinaryReturn => {
                if self.ordinary.as_ref().is_some_and(Page::ready) {
                    self.ack()?;
                }
            }
            Stage::Cleanup => {}
        }
        Ok(())
    }
    fn turn(&mut self, window: &Window) {
        if self.done {
            return;
        }
        let result = (|| -> Result<()> {
            if self.stage != Stage::Cleanup {
                if Instant::now() >= self.deadline {
                    return Err("lifetime-deadline-expired");
                }
                if self.mode == Mode::KeepBrowser
                    && self
                        .a_monitor
                        .as_ref()
                        .is_some_and(|m| m.status() != Exit::Pending)
                {
                    return Err("lifetime-kept-browser-exited");
                }
                loop {
                    let notice = self.worker.as_ref().and_then(|w| w.notices.try_recv().ok());
                    match notice {
                        Some(Notice::Action(a)) => self.begin_action(window, a)?,
                        Some(Notice::Complete(f)) => {
                            if self.pending.is_some()
                                || self.last_ack != Some(4)
                                || !CHECKS.iter().all(|c| f["checks"][*c] == true)
                                || f["observation_status"] != "complete"
                            {
                                return Err("lifetime-original-eight-incomplete");
                            }
                            if self.mode == Mode::KeepBrowser
                                && self.a_monitor.as_ref().map(|m| m.status())
                                    != Some(Exit::Pending)
                            {
                                return Err("lifetime-keeper-not-live-at-completion");
                            }
                            self.fixture = Some(f);
                            self.original_complete = true;
                            self.begin_cleanup();
                            break;
                        }
                        Some(Notice::Failed(code)) => {
                            self.failure = Some(code);
                            self.begin_cleanup();
                            break;
                        }
                        None => break,
                    }
                }
                if self
                    .worker
                    .as_ref()
                    .is_some_and(|w| w.overflow.load(std::sync::atomic::Ordering::Acquire))
                {
                    return Err("lifetime-worker-notice-bound");
                }
            }
            if self.pending.is_some() || self.stage == Stage::Cleanup {
                self.step(window)?;
            }
            Ok(())
        })();
        if let Err(code) = result {
            self.boundary_failure.get_or_insert(code);
            if self.stage == Stage::Cleanup {
                self.done = true;
            } else {
                self.begin_cleanup();
            }
        }
    }
}

pub(super) fn main_entry() -> i32 {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 3 {
        return 64;
    }
    let Some(mode) = args[2].to_str().and_then(|s| Mode::read(s).ok()) else {
        return 64;
    };
    run(Path::new(&args[1]), mode).unwrap_or(69)
}
fn run(manifest: &Path, mode: Mode) -> Result<i32> {
    let recipe = Recipe::read(manifest)?;
    if recipe.profile_acl_mode != WindowsProfileAclMode::OwnerOnly {
        return Err("lifetime-alternate-permission-policy-refused");
    }
    let output = recipe.output.clone();
    let run = recipe.run.clone();
    let deadline = Instant::now() + recipe.timeout;
    let mut event_loop = EventLoopBuilder::<()>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_visible(false)
        .with_title("Owned lifecycle fixture")
        .build(&event_loop)
        .map_err(|_| "lifetime-window-unavailable")?;
    let proxy = event_loop.create_proxy();
    let worker = Worker::start(&recipe, move || {
        let _ = proxy.send_event(());
    });
    let mut host = Host {
        recipe,
        mode,
        deadline,
        worker: Some(worker),
        ordinary: None,
        active: None,
        keeper: None,
        a_environment: None,
        a_monitor: None,
        closing: Vec::new(),
        a_pid: 0,
        matched_a_exit: false,
        reopened_same_browser: false,
        environment_generation: 1,
        runtime: None,
        pending: None,
        stage: Stage::Home,
        probe: None,
        exit_deadline: None,
        last_ack: None,
        fixture: None,
        original_complete: false,
        failure: None,
        boundary_failure: None,
        done: false,
        trace: Vec::new(),
        metadata: Vec::new(),
        locators: BTreeMap::new(),
        contexts: Vec::new(),
    };
    event_loop.run_return(|event, _, flow| {
        *flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(20));
        if matches!(
            event,
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            }
        ) {
            host.boundary_failure = Some("lifetime-window-closed");
            host.begin_cleanup();
        }
        host.turn(&window);
        if host.done {
            *flow = ControlFlow::Exit;
        }
    });
    host.worker = None;
    let boundary = host.boundary_failure.is_none()
        && host.metadata.len() == 4
        && match mode {
            Mode::KeepBrowser => host.reopened_same_browser,
            Mode::NormalExit => host.matched_a_exit && host.environment_generation == 2,
        };
    let receipt = json!({"kind":"windows-cookie-lifetime-direct-wry-diagnostic","test_only":true,"native_qualification":false,"production_repair":false,"mode":mode.name(),"wry_package":"0.56.1","diagnostic_features":[cookies::NativeCookieMetadataProbe::feature_name()],"profile_acl_mode":host.recipe.profile_acl_mode.as_str(),"runtime_version":host.runtime,"same_options":true,"original_eight_complete":host.original_complete,"boundary_valid":boundary,"failure_code":host.failure,"boundary_failure_code":host.boundary_failure,"fixture":host.fixture,"environment_generation":host.environment_generation,"keep_reopened_same_live_browser":host.reopened_same_browser,"exit_matched_normal_before_new_environment":host.matched_a_exit,"windows_context_observations":host.contexts,"windows_cookie_metadata_observations":host.metadata,"finite_phase_records":host.trace});
    let bytes = serde_json::to_vec_pretty(&receipt).map_err(|_| "lifetime-receipt-encode")?;
    minicon_platform::filesystem::write_private_atomic(
        &output.join("native-driver-receipt.json"),
        &bytes,
    )
    .map_err(|_| "lifetime-receipt-storage")?;
    let private = json!({"schema":1,"test_only":true,"run":run,"phases":host.locators});
    minicon_platform::filesystem::write_private_atomic(
        &output.join("native-profile-locator.private.json"),
        &serde_json::to_vec(&private).map_err(|_| "lifetime-locator-encode")?,
    )
    .map_err(|_| "lifetime-locator-storage")?;
    Ok(if !boundary {
        69
    } else if host.original_complete {
        0
    } else {
        1
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_load_ack_accepts_only_the_original_ordinary_history_query() {
        let expected = "http://127.0.0.1:1/page/run/ordinary/ticket";
        assert!(finished_target(Some(expected), expected, false));
        assert!(finished_target(
            Some(&format!("{expected}?step=two")),
            expected,
            true
        ));
        assert!(!finished_target(
            Some(&format!("{expected}?step=two")),
            expected,
            false
        ));
        assert!(!finished_target(
            Some(&format!("{expected}?other=two")),
            expected,
            true
        ));
        assert!(!finished_target(Some("about:blank"), expected, true));
        assert!(!finished_target(None, expected, true));
    }
    #[test]
    fn keeper_and_pending_owned_exit_are_both_required_at_the_real_reopen_gate() {
        assert!(authorize_reopen(
            Mode::KeepBrowser,
            true,
            true,
            Some(Exit::Pending),
            false
        ));
        assert!(!authorize_reopen(
            Mode::KeepBrowser,
            false,
            true,
            Some(Exit::Pending),
            false
        ));
        assert!(!authorize_reopen(
            Mode::KeepBrowser,
            true,
            true,
            Some(Exit::Released),
            false
        ));
        assert!(!authorize_reopen(
            Mode::KeepBrowser,
            true,
            true,
            Some(Exit::Failed("synthetic")),
            false
        ));
    }
    #[test]
    fn matched_exit_is_insufficient_until_both_old_keeper_and_environment_are_released() {
        assert!(authorize_reopen(Mode::NormalExit, false, false, None, true));
        assert!(!authorize_reopen(
            Mode::NormalExit,
            false,
            false,
            None,
            false
        ));
        assert!(!authorize_reopen(Mode::NormalExit, true, false, None, true));
        assert!(!authorize_reopen(Mode::NormalExit, false, true, None, true));
        assert!(!authorize_reopen(
            Mode::NormalExit,
            false,
            false,
            Some(Exit::Released),
            true
        ));
    }
    #[test]
    fn only_the_two_finite_lifetime_modes_are_executable() {
        assert_eq!(Mode::read("keep-browser"), Ok(Mode::KeepBrowser));
        assert_eq!(Mode::read("normal-exit"), Ok(Mode::NormalExit));
        for bad in [
            "keep",
            "exit",
            "http://127.0.0.1:1/",
            "--eval",
            "",
            "normal-exit ",
        ] {
            assert!(Mode::read(bad).is_err());
        }
    }
    #[test]
    fn actual_dispatch_authority_refuses_changed_url_phase_kind_and_sequence() {
        let context = PageContext {
            profile: PageProfile {
                data_directory: "unused-by-action-validation".into(),
                store_identifier: [1; 16],
            },
            proxy: Default::default(),
        };
        let pages = [("ordinary", "o"), ("a", "a"), ("b", "b"), ("a-reopen", "r")]
            .into_iter()
            .map(|(p, t)| (p.into(), format!("http://127.0.0.1:1/page/run/{p}/{t}")))
            .collect();
        let recipe = Recipe {
            address: "127.0.0.1:1".parse().unwrap(),
            token: "synthetic".into(),
            run: "synthetic".into(),
            pages,
            a: context.clone(),
            b: context,
            output: Default::default(),
            timeout: Duration::from_secs(1),
            profile_acl_mode: WindowsProfileAclMode::OwnerOnly,
        };
        let action = Action {
            seq: 3,
            kind: "present-auth".into(),
            phase: "a-reopen".into(),
            url: recipe.pages.get("a-reopen").cloned(),
        };
        assert!(authorized(&recipe, &action));
        let mut changed = action.clone();
        changed.url = recipe.pages.get("a").cloned();
        assert!(!authorized(&recipe, &changed));
        let mut changed = action.clone();
        changed.phase = "a".into();
        assert!(!authorized(&recipe, &changed));
        let mut changed = action.clone();
        changed.kind = "ordinary".into();
        assert!(!authorized(&recipe, &changed));
        let mut changed = action;
        changed.seq = 1;
        assert!(!authorized(&recipe, &changed));
    }
}
