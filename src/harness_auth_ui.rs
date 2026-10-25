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
