//! Account authorization presentation; product ownership: PRD 31.01.01/02.
use super::*;

pub(super) struct OrdinaryBrowser {
    model: Option<browser::BrowserTab>,
    address: Option<composer::ComposerState>,
    proxy_input: composer::ComposerState,
    proxy_applied: String,
    can_back: bool,
    can_forward: bool,
    deadline: Option<Instant>,
    pub(super) page: Option<std::process::Child>,
    #[cfg(target_os = "linux")]
    process: Option<browser_process::BrowserProcess>,
    #[cfg(target_os = "linux")]
    program: Option<browser::EmbeddedPageProgram>,
    #[cfg(any(target_os = "macos", windows))]
    native: Option<browser::NativePageHost>,
}

impl Drop for OrdinaryBrowser {
    fn drop(&mut self) {
        if let Some(mut child) = self.page.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub(super) struct AuthPresentation {
    pub(super) attempt: provider_runtime::AttemptId,
    pub(super) context: browser::PageContext,
    pub(super) ordinary: OrdinaryBrowser,
}

#[cfg(windows)]
struct AuthReleaseRecord {
    profile: minicon_web::PageProfile,
    observation: minicon_web::NativePageRelease,
}

#[cfg(windows)]
struct PendingAuthNavigation {
    attempt: provider_runtime::AttemptId,
    context: browser::PageContext,
    url: String,
    deadline: Instant,
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingAuthDecision {
    Wait,
    Open,
    Cancel,
    Failed(&'static str),
}

#[cfg(windows)]
impl PendingAuthNavigation {
    fn decision(
        &self,
        now: Instant,
        current: Option<provider_runtime::AttemptId>,
        presentation: Option<(provider_runtime::AttemptId, &browser::PageContext)>,
        dismissed: Option<provider_runtime::AttemptId>,
        release: minicon_web::NativePageReleaseStatus,
    ) -> PendingAuthDecision {
        if current != Some(self.attempt)
            || presentation != Some((self.attempt, &self.context))
            || dismissed == Some(self.attempt)
        {
            return PendingAuthDecision::Cancel;
        }
        if now >= self.deadline {
            return PendingAuthDecision::Failed("browser-context-release-timeout");
        }
        match release {
            minicon_web::NativePageReleaseStatus::Pending => PendingAuthDecision::Wait,
            minicon_web::NativePageReleaseStatus::Released => PendingAuthDecision::Open,
            minicon_web::NativePageReleaseStatus::Failed(reason) => {
                PendingAuthDecision::Failed(reason)
            }
        }
    }
}

#[cfg(windows)]
#[derive(Default)]
pub(super) struct AuthContextLifecycle {
    releases: Vec<AuthReleaseRecord>,
    pending: Option<PendingAuthNavigation>,
    overflowed: bool,
}

#[cfg(windows)]
impl AuthContextLifecycle {
    fn defer(
        &mut self,
        attempt: provider_runtime::AttemptId,
        context: &browser::PageContext,
        url: &str,
        now: Instant,
    ) {
        let deadline = self
            .pending
            .as_ref()
            .filter(|pending| pending.attempt == attempt && &pending.context == context)
            .map_or_else(|| now + Duration::from_secs(10), |pending| pending.deadline);
        self.pending = Some(PendingAuthNavigation {
            attempt,
            context: context.clone(),
            url: url.to_owned(),
            deadline,
        });
    }

    fn retire_released(&mut self) {
        self.releases.retain(|record| {
            record.observation.status() != minicon_web::NativePageReleaseStatus::Released
        });
    }

    fn status(&self, profile: &minicon_web::PageProfile) -> minicon_web::NativePageReleaseStatus {
        if self.overflowed {
            return minicon_web::NativePageReleaseStatus::Failed(
                "browser-context-release-capacity",
            );
        }
        self.releases
            .iter()
            .find(|record| &record.profile == profile)
            .map_or(minicon_web::NativePageReleaseStatus::Released, |record| {
                record.observation.status()
            })
    }
}

impl ConApp {
    pub(super) fn open_harness_auth(
        &mut self,
        window: &PixelWindow,
        attempt: provider_runtime::AttemptId,
        context: browser::PageContext,
        url: &str,
    ) {
        let current = self
            .harness_settings
            .as_ref()
            .and_then(|settings| settings.auth_attempt());
        if current != Some(attempt) || self.dismissed_auth_attempt == Some(attempt) {
            return;
        }
        if self
            .browser_auth
            .as_ref()
            .is_some_and(|auth| auth.attempt != attempt || auth.context != context)
        {
            let previous = self.browser_auth.as_ref().unwrap().attempt;
            self.finish_harness_auth(window, previous);
        }
        #[cfg(windows)]
        {
            self.auth_context_lifecycle.retire_released();
            if let minicon_web::NativePageReleaseStatus::Failed(_) =
                self.auth_context_lifecycle.status(&context.profile)
            {
                self.fail_harness_auth("The previous authorization view could not be safely closed. Close MiniCon and retry.");
                return;
            }
        }
        if self.browser_auth.is_none() {
            if self.open_browser_tab(window).is_err() {
                self.fail_harness_auth("Could not open the authorization view.");
                return;
            }
            let Some(id) = self.workspace.browser_tab() else {
                return;
            };
            self.sync_embedded_page(window);
            #[cfg(any(target_os = "macos", windows))]
            if let Some(page) = &self.native_browser_page
                && page.set_visible(false).is_err()
            {
                self.fail_harness_auth("Could not hide the ordinary browser page.");
                return;
            }
            #[cfg(target_os = "linux")]
            if let Some(child) = &self.browser_page {
                let hidden = self.content_panel_slot(window).is_some_and(|slot| {
                    self.browser_process
                        .as_ref()
                        .is_some_and(|process| process.embedded_ready(slot.parent, child.id()))
                        && browser::set_embedded_page_visible_for_process(
                            slot.parent,
                            child.id(),
                            false,
                        )
                });
                if !hidden {
                    self.fail_harness_auth("The ordinary page is not ready to park safely. Wait for it to finish opening, then retry.");
                    return;
                }
            }
            let model = self
                .browsers
                .get_mut(&id)
                .map(|model| std::mem::replace(model, browser::BrowserTab::shell()));
            let ordinary = OrdinaryBrowser {
                model,
                address: self.browser_address.take(),
                proxy_input: std::mem::take(&mut self.browser_proxy_input),
                proxy_applied: std::mem::take(&mut self.browser_proxy_applied),
                can_back: self.browser_can_back,
                can_forward: self.browser_can_forward,
                deadline: self.browser_load_deadline.take(),
                page: self.browser_page.take(),
                #[cfg(target_os = "linux")]
                process: self.browser_process.take(),
                #[cfg(target_os = "linux")]
                program: self.browser_embedded_program.take(),
                #[cfg(any(target_os = "macos", windows))]
                native: self.native_browser_page.take(),
            };
            self.browser_auth = Some(AuthPresentation {
                attempt,
                context,
                ordinary,
            });
            self.browser_can_back = false;
            self.browser_can_forward = false;
            self.browser_visible = None;
        }
        self.blur_harness_fields();
        self.settings_open = false;
        self.close_browser_menu(window, false, false);
        self.browser_address_focused = false;
        self.browser_proxy_focused = false;
        self.composer.focused = false;
        self.present_harness_auth_url(window, attempt, url);
    }

    /// All navigation entry points use this before creating an auth host,
    /// including address submission and reload while an old view is closing.
    #[cfg(windows)]
    pub(super) fn defer_harness_auth_navigation(&mut self, url: &str) -> bool {
        let Some(auth) = &self.browser_auth else {
            return false;
        };
        let release = self.auth_context_lifecycle.status(&auth.context.profile);
        match release {
            minicon_web::NativePageReleaseStatus::Released => {
                self.auth_context_lifecycle.pending = None;
                false
            }
            minicon_web::NativePageReleaseStatus::Failed(reason) => {
                self.note_page_load(browser::PageLoad::Unavailable {
                    url: String::new(),
                    reason: reason.to_owned(),
                });
                true
            }
            minicon_web::NativePageReleaseStatus::Pending => {
                if url.len() > 8192 || browser::http_url_at(url, 0).as_deref() != Some(url) {
                    self.note_page_load(browser::PageLoad::Unavailable {
                        url: String::new(),
                        reason: "browser-auth-address-invalid".to_owned(),
                    });
                    return true;
                }
                self.auth_context_lifecycle
                    .defer(auth.attempt, &auth.context, url, Instant::now());
                self.note_page_load(browser::PageLoad::Loading {
                    url: url.to_owned(),
                });
                true
            }
        }
    }

    #[cfg(windows)]
    pub(super) fn harness_auth_context_released(&self) -> bool {
        self.browser_auth.as_ref().is_none_or(|auth| {
            self.auth_context_lifecycle.status(&auth.context.profile)
                == minicon_web::NativePageReleaseStatus::Released
        })
    }

    fn present_harness_auth_url(
        &mut self,
        window: &PixelWindow,
        attempt: provider_runtime::AttemptId,
        url: &str,
    ) {
        self.present_http_link(window, url);
        let unavailable = self
            .workspace
            .browser_tab()
            .and_then(|id| self.browsers.get(&id))
            .is_some_and(|tab| matches!(tab.panel().load, browser::PageLoad::Unavailable { .. }));
        if unavailable {
            self.fail_harness_auth("The requested isolated authorization browser is unavailable.");
            self.finish_harness_auth(window, attempt);
        }
        self.sync_embedded_page(window);
        self.mark_host_ui_full_and_repaint(window);
    }

    #[cfg(windows)]
    pub(super) fn harness_auth_release_deadline(&self) -> Option<Instant> {
        self.auth_context_lifecycle
            .pending
            .as_ref()
            .map(|pending| pending.deadline)
    }

    #[cfg(windows)]
    pub(super) fn poll_harness_auth_release(&mut self, window: &PixelWindow, now: Instant) {
        self.auth_context_lifecycle.retire_released();
        let Some(pending) = self.auth_context_lifecycle.pending.take() else {
            return;
        };
        let decision = pending.decision(
            now,
            self.harness_settings
                .as_ref()
                .and_then(|settings| settings.auth_attempt()),
            self.browser_auth
                .as_ref()
                .map(|auth| (auth.attempt, &auth.context)),
            self.dismissed_auth_attempt,
            self.auth_context_lifecycle.status(&pending.context.profile),
        );
        match decision {
            PendingAuthDecision::Wait => self.auth_context_lifecycle.pending = Some(pending),
            PendingAuthDecision::Open => {
                self.present_harness_auth_url(window, pending.attempt, &pending.url)
            }
            PendingAuthDecision::Cancel => self.finish_harness_auth(window, pending.attempt),
            PendingAuthDecision::Failed(reason) => {
                self.fail_harness_auth(if reason == "browser-context-release-timeout" {
                    "The previous authorization view is still closing. Wait, then retry."
                } else {
                    "The previous authorization view could not be safely closed. Close MiniCon and retry."
                });
                self.finish_harness_auth(window, pending.attempt);
            }
        }
    }

    fn fail_harness_auth(&mut self, message: &str) {
        if let Some(settings) = &mut self.harness_settings {
            settings.auth_failed(message);
        }
    }

    pub(super) fn finish_harness_auth(
        &mut self,
        window: &PixelWindow,
        attempt: provider_runtime::AttemptId,
    ) {
        if !self.restore_harness_auth(attempt) {
            return;
        }
        self.sync_embedded_page(window);
        let _ = self.refresh_title(window);
        self.mark_host_ui_full_and_repaint(window);
    }

    /// Restore the parked live host before any tab teardown. Shared by native
    /// close gestures, control CloseTab and asynchronous authorization results.
    pub(super) fn restore_harness_auth(&mut self, attempt: provider_runtime::AttemptId) -> bool {
        if !self
            .browser_auth
            .as_ref()
            .is_some_and(|auth| auth.attempt == attempt)
        {
            return false;
        }
        let mut auth = self.browser_auth.take().unwrap();
        #[cfg(windows)]
        {
            if self
                .auth_context_lifecycle
                .pending
                .as_ref()
                .is_some_and(|pending| pending.attempt == attempt)
            {
                self.auth_context_lifecycle.pending = None;
            }
            if let Some(page) = self.native_browser_page.take() {
                page.focus_parent();
                let waker = self.waker_slot.lock().ok().and_then(|slot| slot.clone());
                let observation = page.begin_release(move || {
                    if let Some(waker) = &waker {
                        let _ = waker.wake();
                    }
                });
                self.auth_context_lifecycle.retire_released();
                if self.auth_context_lifecycle.releases.len() < 18 {
                    self.auth_context_lifecycle
                        .releases
                        .push(AuthReleaseRecord {
                            profile: auth.context.profile.clone(),
                            observation,
                        });
                } else {
                    // We cannot forget an unknown release and safely reuse its
                    // profile. Bound memory and refuse subsequent auth opens.
                    self.auth_context_lifecycle.overflowed = true;
                }
            }
        }
        let return_to_settings = self.active_is_browser();
        self.stop_page_host();
        if let Some(id) = self.workspace.browser_tab() {
            if let (Some(target), Some(model)) =
                (self.browsers.get_mut(&id), auth.ordinary.model.take())
            {
                *target = model;
            }
            self.browser_address = auth.ordinary.address.take();
            self.browser_proxy_input = std::mem::take(&mut auth.ordinary.proxy_input);
            self.browser_proxy_applied = std::mem::take(&mut auth.ordinary.proxy_applied);
            self.browser_can_back = auth.ordinary.can_back;
            self.browser_can_forward = auth.ordinary.can_forward;
            self.browser_load_deadline = auth.ordinary.deadline;
            self.browser_page = auth.ordinary.page.take();
            #[cfg(target_os = "linux")]
            {
                self.browser_process = auth.ordinary.process.take();
                self.browser_embedded_program = auth.ordinary.program.take();
            }
            #[cfg(any(target_os = "macos", windows))]
            {
                self.native_browser_page = auth.ordinary.native.take();
            }
            if return_to_settings {
                self.settings_open = true;
                self.settings_section = ui::SettingsSection::Harness;
            }
        }
        self.browser_visible = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn pending_auth_release_requires_the_current_attempt_context_and_unexpired_owned_release() {
        use minicon_web::NativePageReleaseStatus::{Pending, Released};
        let (app, _, _, attempt) = parked_browser();
        let context = app.browser_auth.as_ref().unwrap().context.clone();
        let now = Instant::now();
        let pending = PendingAuthNavigation {
            attempt,
            context: context.clone(),
            url: "https://provider.invalid/authorize".into(),
            deadline: now + Duration::from_secs(10),
        };
        let active = Some((attempt, &context));
        assert_eq!(
            pending.decision(now, Some(attempt), active, None, Pending),
            PendingAuthDecision::Wait
        );
        assert_eq!(
            pending.decision(now, Some(attempt), active, None, Released),
            PendingAuthDecision::Open
        );
        assert_eq!(
            pending.decision(now, None, active, None, Released),
            PendingAuthDecision::Cancel
        );
        assert_eq!(
            pending.decision(now, Some(attempt), active, Some(attempt), Released),
            PendingAuthDecision::Cancel
        );
        assert_eq!(
            pending.decision(now, Some(attempt), None, None, Released),
            PendingAuthDecision::Cancel
        );
        let other_attempt = provider_runtime::AttemptId(attempt.0 + 1);
        assert_eq!(
            pending.decision(now, Some(other_attempt), active, None, Released),
            PendingAuthDecision::Cancel
        );
        let mut other_context = context.clone();
        other_context.profile.store_identifier = [9; 16];
        assert_eq!(
            pending.decision(
                now,
                Some(attempt),
                Some((attempt, &other_context)),
                None,
                Released
            ),
            PendingAuthDecision::Cancel
        );
        assert_eq!(
            pending.decision(pending.deadline, Some(attempt), active, None, Released),
            PendingAuthDecision::Failed("browser-context-release-timeout")
        );
        assert_eq!(
            pending.decision(
                now,
                Some(attempt),
                active,
                None,
                minicon_web::NativePageReleaseStatus::Failed("synthetic-release-failed")
            ),
            PendingAuthDecision::Failed("synthetic-release-failed")
        );
    }

    #[cfg(windows)]
    #[test]
    fn pending_auth_release_retains_its_first_deadline_and_only_the_latest_address() {
        let (mut app, _, _, attempt) = parked_browser();
        let context = app.browser_auth.as_ref().unwrap().context.clone();
        let now = Instant::now();
        app.auth_context_lifecycle
            .defer(attempt, &context, "https://provider.invalid/first", now);
        let deadline = app.harness_auth_release_deadline().unwrap();
        app.auth_context_lifecycle.defer(
            attempt,
            &context,
            "https://provider.invalid/latest",
            now + Duration::from_secs(8),
        );
        let pending = app.auth_context_lifecycle.pending.as_ref().unwrap();
        assert_eq!(pending.url, "https://provider.invalid/latest");
        assert_eq!(pending.deadline, deadline);
        app.auth_context_lifecycle.defer(
            provider_runtime::AttemptId(attempt.0 + 1),
            &context,
            "https://provider.invalid/new-attempt",
            now + Duration::from_secs(8),
        );
        assert_eq!(
            app.harness_auth_release_deadline().unwrap(),
            now + Duration::from_secs(18)
        );
    }

    #[cfg(windows)]
    #[test]
    fn pending_auth_release_navigation_is_discarded_on_real_auth_close_and_ordinary_page_returns() {
        let (mut app, _, browser, attempt) = parked_browser();
        let context = app.browser_auth.as_ref().unwrap().context.clone();
        app.auth_context_lifecycle.defer(
            attempt,
            &context,
            "https://provider.invalid/stale",
            Instant::now(),
        );
        assert!(!app.detach_session(browser));
        assert!(app.auth_context_lifecycle.pending.is_none());
        assert!(app.browser_auth.is_none());
        assert_eq!(
            app.browsers.get(&browser).unwrap().panel().url.as_deref(),
            Some("https://ordinary.invalid/live")
        );
        assert_eq!(
            app.browser_address.as_ref().unwrap().text,
            "unfinished address draft"
        );
    }

    fn parked_browser() -> (
        ConApp,
        workspace::TabId,
        workspace::TabId,
        provider_runtime::AttemptId,
    ) {
        let mut app = ConApp::new(None, None);
        let terminal = app.workspace.active().unwrap();
        let browser = app.workspace.add_browser_root().unwrap();
        let attempt = provider_runtime::AttemptId(42);
        let mut model = browser::BrowserTab::shell();
        assert!(model.show_http("https://ordinary.invalid/live"));
        assert!(
            app.browsers
                .insert(browser, browser::BrowserTab::shell())
                .is_ok()
        );
        app.workspace.set_active(browser);
        let mut address = composer::ComposerState::default();
        address.text = "unfinished address draft".into();
        address.caret = 5;
        address.anchor = Some(2);
        app.browser_auth = Some(AuthPresentation {
            attempt,
            context: browser::PageContext {
                profile: minicon_web::PageProfile {
                    data_directory: std::env::temp_dir().join("unused-auth-fixture"),
                    store_identifier: [1; 16],
                },
                proxy: browser::BrowserProxy::default(),
            },
            ordinary: OrdinaryBrowser {
                model: Some(model),
                address: Some(address),
                proxy_input: Default::default(),
                proxy_applied: "http://ordinary-proxy.invalid:8080".into(),
                can_back: true,
                can_forward: true,
                deadline: None,
                page: None,
                #[cfg(target_os = "linux")]
                process: None,
                #[cfg(target_os = "linux")]
                program: None,
                #[cfg(any(target_os = "macos", windows))]
                native: None,
            },
        });
        (app, terminal, browser, attempt)
    }

    #[test]
    fn closing_authorization_restores_ordinary_page_and_consumes_only_the_first_close() {
        let (mut app, terminal, browser, attempt) = parked_browser();
        assert!(!app.detach_session(browser));
        assert_eq!(app.workspace.browser_tab(), Some(browser));
        assert_eq!(app.workspace.active(), Some(browser));
        assert_eq!(
            app.browsers.get(&browser).unwrap().panel().url.as_deref(),
            Some("https://ordinary.invalid/live")
        );
        let address = app.browser_address.as_ref().unwrap();
        assert_eq!(address.text, "unfinished address draft");
        assert_eq!(address.caret, 5);
        assert_eq!(address.anchor, Some(2));
        assert_eq!(
            app.browser_proxy_applied,
            "http://ordinary-proxy.invalid:8080"
        );
        assert!(app.browser_can_back && app.browser_can_forward);
        assert!(app.browser_auth.is_none());
        assert_eq!(app.dismissed_auth_attempt, Some(attempt));
        assert!(app.sessions.contains_key(&terminal));
        assert!(!app.restore_harness_auth(attempt));
        assert!(!app.detach_session(browser));
        assert!(app.workspace.browser_tab().is_none());
        assert!(app.sessions.contains_key(&terminal));
    }

    #[test]
    fn authorization_close_shortcut_discards_the_old_hosts_remaining_observations_and_keys() {
        let (mut app, _, browser, _) = parked_browser();
        let mut keys = Vec::new();
        let applied = app.consume_browser_observation(
            browser_ui::BrowserObservation {
                title: Some("synthetic SSO private title".into()),
                url: Some("https://accounts.google.com/synthetic?code=private-value".into()),
                error: Some("synthetic private failure".into()),
                loading: Some(true),
                can_back: Some(false),
                can_forward: Some(false),
                shortcuts: vec!['W', 'W'],
            },
            |app, key| {
                keys.push(key);
                assert!(!app.detach_session(browser));
            },
        );
        assert!(!applied);
        assert_eq!(keys, vec!['W']);
        let panel = app.browsers.get(&browser).unwrap().panel();
        assert_eq!(panel.url.as_deref(), Some("https://ordinary.invalid/live"));
        assert_eq!(panel.load, browser::PageLoad::Idle);
        assert_ne!(panel.title, "synthetic SSO private title");
        assert!(app.browser_can_back && app.browser_can_forward);
        assert!(app.browser_load_deadline.is_none());
    }
}
