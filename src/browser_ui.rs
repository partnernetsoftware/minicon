//! Browser workspace chrome and address editing. Product behavior: PRD 25.
use super::*;
use minicon_core::field_editor::{self, Command, Intent, Limits, Outcome};

/// One native/helper batch. Shortcuts can replace the presenting host before
/// its remaining fields are consumed, so both adapters share this boundary.
#[derive(Default)]
pub(super) struct BrowserObservation {
    pub title: Option<String>,
    pub url: Option<String>,
    pub error: Option<String>,
    pub loading: Option<bool>,
    pub can_back: Option<bool>,
    pub can_forward: Option<bool>,
    pub shortcuts: Vec<char>,
}

/// Authorization addresses remain usable in the real address bar, while
/// automation and diagnostics expose only their origin/path.
pub(super) fn is_authorization_page(url: &str) -> bool {
    let base = url.split(['?', '#']).next().unwrap_or(url);
    let Some((scheme, rest)) = base.split_once("://") else {
        return false;
    };
    let Some((authority, path)) = rest.split_once('/') else {
        return false;
    };
    if scheme.eq_ignore_ascii_case("https") {
        let authority = authority.to_ascii_lowercase();
        let authority = authority.strip_suffix(":443").unwrap_or(&authority);
        return (authority == "platform.deepseek.com"
            && matches!(path, "dsh/authorize" | "dsh/authorized" | "oauth/authorize"))
            || (authority == "opencode.ai"
                && matches!(
                    path,
                    "console/device"
                        | "console/auth/device"
                        | "console/auth/social/github/callback"
                ))
            || (authority == "github.com" && (path == "login" || path.starts_with("login/")));
    }
    if !scheme.eq_ignore_ascii_case("http") || !matches!(path, "oauth/callback" | "minicon/oauth") {
        return false;
    }
    let authority = authority.to_ascii_lowercase();
    let port = authority
        .strip_prefix("127.0.0.1:")
        .or_else(|| authority.strip_prefix("localhost:"))
        .or_else(|| authority.strip_prefix("[::1]:"));
    matches!(authority.as_str(), "127.0.0.1" | "localhost" | "[::1]")
        || port.is_some_and(|value| !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
}

pub(super) fn public_browser_url(url: &str) -> &str {
    if is_authorization_page(url) {
        url.split(['?', '#']).next().unwrap_or(url)
    } else {
        url
    }
}

/// Preserve only exact transport-owned diagnostics on private authorization
/// pages. Every result is fixed text; unknown codes and provider text stay
/// fully redacted. Codes follow COREWEBVIEW2_WEB_ERROR_STATUS.
pub(super) fn public_authorization_failure(reason: &str) -> &'static str {
    match reason {
        "browser-navigation-error-0" => "browser-navigation-unknown",
        "browser-navigation-error-1" => "browser-navigation-certificate-name-mismatch",
        "browser-navigation-error-2" => "browser-navigation-certificate-expired",
        "browser-navigation-error-3" => "browser-navigation-client-certificate-invalid",
        "browser-navigation-error-4" => "browser-navigation-certificate-revoked",
        "browser-navigation-error-5" => "browser-navigation-certificate-invalid",
        "browser-navigation-error-6" => "browser-navigation-server-unreachable",
        "browser-navigation-error-7" | "browser-navigation-timeout" => "browser-navigation-timeout",
        "browser-navigation-error-8" => "browser-navigation-invalid-server-response",
        "browser-navigation-error-9" => "browser-navigation-connection-aborted",
        "browser-navigation-error-10" => "browser-navigation-connection-reset",
        "browser-navigation-error-11" => "browser-navigation-disconnected",
        "browser-navigation-error-12" => "browser-navigation-cannot-connect",
        "browser-navigation-error-13" => "browser-navigation-host-name-not-resolved",
        "browser-navigation-error-14" => "browser-navigation-operation-canceled",
        "browser-navigation-error-15" => "browser-navigation-redirect-failed",
        "browser-navigation-error-16" => "browser-navigation-unexpected-error",
        "browser-navigation-error-17" => "browser-navigation-authentication-required",
        "browser-navigation-error-18" => "browser-navigation-proxy-authentication-required",
        _ => "authorization-page-unavailable",
    }
}

pub(super) fn public_browser_text(text: &str) -> std::borrow::Cow<'_, str> {
    let mut output = String::new();
    let mut rest = text;
    let mut changed = false;
    loop {
        let lower = rest.to_ascii_lowercase();
        let Some(start) = lower
            .find("http://")
            .into_iter()
            .chain(lower.find("https://"))
            .min()
        else {
            break;
        };
        output.push_str(&rest[..start]);
        let token = &rest[start..];
        let end = token
            .find(|ch: char| ch.is_whitespace() || matches!(ch, '\'' | '"' | '<' | '>'))
            .unwrap_or(token.len());
        let token = &token[..end];
        let public = public_browser_url(token);
        changed |= public.len() != token.len();
        output.push_str(public);
        rest = &rest[start + end..];
    }
    if changed {
        output.push_str(rest);
        std::borrow::Cow::Owned(output)
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

/// Convert host input once; field ownership is decided by the caller even
/// when the reducer reports no change or refuses an oversized insertion.
pub(super) fn edit_host_field(
    editor: &mut composer::ComposerState,
    event: &PixelWindowEvent,
    limits: Limits,
) -> Option<Outcome> {
    let outcome = match event {
        PixelWindowEvent::Keyboard(key) => {
            if key.state != KeyPressState::Pressed {
                return Some(Outcome::default());
            }
            let Some(action) = composer_chord(key).and_then(|chord| {
                keymap::action(chord, CLIPBOARD_MODIFIER, keymap::Draft::single_line())
            }) else {
                return Some(Outcome::default());
            };
            let pasted = if action == keymap::Action::Paste {
                composer_paste_text_or_image_path()
            } else {
                None
            };
            let typed = if let LogicalKey::Character(text) = &key.logical {
                Some(text.as_str())
            } else {
                None
            };
            field_editor::apply(
                editor,
                Command::Action {
                    action,
                    text: if action == keymap::Action::Paste {
                        pasted.as_deref()
                    } else {
                        typed
                    },
                },
                limits,
            )
        }
        PixelWindowEvent::Ime(ime) => {
            use minicon_platform::ime::{ImeAction, classify_event};
            match classify_event(ime.clone(), true) {
                ImeAction::UpdatePreedit { text, .. } => {
                    field_editor::apply(editor, Command::Preedit(&text), limits)
                }
                ImeAction::ClearPreedit => {
                    field_editor::apply(editor, Command::ClearPreedit, limits)
                }
                ImeAction::CommitText(text) => {
                    field_editor::apply(editor, Command::Commit(&text), limits)
                }
                ImeAction::None => Outcome::default(),
                _ => Outcome::default(),
            }
        }
        _ => return None,
    };
    if let Some(text) = &outcome.clipboard {
        let _ = clipboard_status::set_text(text);
    }
    Some(outcome)
}

pub(super) fn host_field_offset(
    editor: &composer::ComposerState,
    rect: ui::Rect,
    position: &LogicalPoint,
    scale: f64,
    size: u16,
) -> usize {
    let cell = font::cell_metrics(size).width.max(1);
    let columns = (rect.width.saturating_sub((16.0 * scale) as u32) / cell).max(1) as usize;
    let visible = composer::visible_window(&editor.text, &editor.preedit, editor.caret, 1, columns);
    let column = ((position.x * scale).max(0.0) as u32)
        .saturating_sub(rect.x.saturating_add((8.0 * scale) as u32))
        / cell;
    composer::caret_at_cell(&editor.text, visible.text, column as usize)
}

impl ConApp {
    pub(super) fn focus_browser_address(&mut self, window: &PixelWindow) {
        self.browser_chrome_trigger = None;
        self.browser_proxy_focused = false;
        self.settings_proxy_focused = false;
        field_editor::apply(&mut self.browser_proxy_input, Command::Blur, Limits::PROXY);
        field_editor::apply(&mut self.settings_proxy_input, Command::Blur, Limits::PROXY);
        self.begin_browser_address();
        self.browser_address_focused = true;
        self.composer.focused = false;
        #[cfg(any(target_os = "macos", windows))]
        if let Some(page) = &self.native_browser_page {
            page.focus_parent();
        }
        #[cfg(target_os = "linux")]
        window.focus();
        self.update_browser_ime_anchor(window);
        self.mark_host_ui_full_and_repaint(window);
    }

    fn begin_browser_address(&mut self) {
        // Re-focusing an existing editor must preserve its unsubmitted draft.
        let editor = self.browser_address.get_or_insert_with(|| {
            let mut editor = composer::ComposerState::default();
            editor.text = self
                .workspace
                .browser_tab()
                .and_then(|id| self.browsers.get(&id))
                .and_then(|tab| tab.panel().url.clone())
                .unwrap_or_default();
            editor.caret = editor.text.len();
            editor
        });
        composer::select_all(editor);
        self.browser_address_drag = None;
        self.browser_address_clicks = Default::default();
    }

    fn press_browser_address(&mut self, offset: usize, shift: bool) {
        let Some(editor) = self.browser_address.as_mut() else {
            return;
        };
        let clicks = self.browser_address_clicks.register(offset, Instant::now());
        self.browser_address_drag = field_editor::press(editor, offset, shift, clicks);
    }

    fn browser_address_offset_at(
        &self,
        window: &PixelWindow,
        position: &LogicalPoint,
    ) -> Result<usize, PixelWindowError> {
        let Some(editor) = &self.browser_address else {
            return Ok(0);
        };
        let metrics = window.metrics()?;
        let scale = metrics.scale_factor.max(1.0);
        let rect = self
            .layout(metrics.physical_width, metrics.physical_height, scale)
            .browser_address(scale);
        let size = scaled_host_ui_font(16, self.session_seed.font_size_logical(), scale);
        let cell = font::cell_metrics(size).width.max(1);
        let width = rect.width.saturating_sub((16.0 * scale) as u32);
        let visible = composer::visible_window(
            &editor.text,
            &editor.preedit,
            editor.caret,
            1,
            (width / cell).max(1) as usize,
        );
        let origin = rect.x.saturating_add((8.0 * scale) as u32);
        let column = ((position.x * scale).max(0.0) as u32).saturating_sub(origin) / cell;
        Ok(composer::caret_at_cell(
            &editor.text,
            visible.text,
            column as usize,
        ))
    }

    pub(super) fn route_browser_input(
        &mut self,
        window: &PixelWindow,
        event: &PixelWindowEvent,
    ) -> Result<bool, PixelWindowError> {
        if self.settings_open {
            return Ok(false);
        }
        if self.route_browser_proxy_input(window, event)? {
            return Ok(true);
        }
        if !self.active_is_browser() {
            self.browser_address_focused = false;
            if let Some(editor) = &mut self.browser_address {
                field_editor::apply(editor, Command::Blur, Limits::ADDRESS);
            }
            self.browser_address_drag = None;
            return Ok(false);
        }
        if matches!(
            event,
            PixelWindowEvent::PointerCaptureLost | PixelWindowEvent::FocusChanged(false)
        ) {
            self.browser_address_drag = None;
        }
        if let PixelWindowEvent::PointerMoved { position, .. } = event
            && self.browser_address_focused
            && let Some(anchor) = self.browser_address_drag
        {
            let offset = self.browser_address_offset_at(window, position)?;
            if let Some(editor) = self.browser_address.as_mut() {
                field_editor::drag(editor, anchor, offset);
            }
            self.update_browser_ime_anchor(window);
            self.mark_host_ui_full_and_repaint(window);
            return Ok(true);
        }
        if let PixelWindowEvent::PointerButton {
            state: PointerButtonState::Released,
            button: PointerButton::Left,
            ..
        } = event
            && self.browser_address_focused
            && self.browser_address_drag.take().is_some()
        {
            let _ = window.set_pointer_capture(false);
            return Ok(true);
        }
        if let PixelWindowEvent::Keyboard(key) = event
            && key.state == KeyPressState::Pressed
            && (key.modifiers.control || key.modifiers.meta)
            && matches!(&key.logical, LogicalKey::Character(text) if text.eq_ignore_ascii_case("l"))
        {
            self.focus_browser_address(window);
            return Ok(true);
        }
        #[cfg(target_os = "macos")]
        if let PixelWindowEvent::Keyboard(key) = event
            && key.state == KeyPressState::Pressed
            && key.modifiers.meta
            && !key.modifiers.control
            && !key.modifiers.alt
            && !key.modifiers.shift
            && let LogicalKey::Character(text) = &key.logical
            && let Some(control) = match text.as_str() {
                "[" => Some(0),
                "]" => Some(1),
                "r" | "R" => Some(2),
                _ => None,
            }
        {
            self.browser_navigation(window, control);
            return Ok(true);
        }
        if let PixelWindowEvent::PointerButton {
            state: PointerButtonState::Pressed,
            button: PointerButton::Left,
            position: Some(position),
            modifiers,
        } = event
        {
            let metrics = window.metrics()?;
            let scale = metrics.scale_factor.max(1.0);
            let layout = self.layout(metrics.physical_width, metrics.physical_height, scale);
            let x = (position.x * scale).max(0.0) as u32;
            let y = (position.y * scale).max(0.0) as u32;
            if layout.browser_toolbar(scale).contains(x, y) {
                if layout.browser_address(scale).contains(x, y) {
                    if !self.browser_address_focused {
                        self.focus_browser_address(window);
                        let offset = self.browser_address_offset_at(window, position)?;
                        self.browser_address_clicks.register(offset, Instant::now());
                    } else {
                        let offset = self.browser_address_offset_at(window, position)?;
                        self.press_browser_address(offset, modifiers.shift);
                        let _ = window.set_pointer_capture(self.browser_address_drag.is_some());
                        self.update_browser_ime_anchor(window);
                        self.mark_host_ui_full_and_repaint(window);
                    }
                } else if y < layout
                    .browser_toolbar(scale)
                    .y
                    .saturating_add((44.0 * scale) as u32)
                {
                    let control =
                        ((x.saturating_sub(layout.composer.x)) as f64 / (34.0 * scale)) as u32;
                    self.browser_navigation(window, control);
                }
                return Ok(true);
            }
            self.browser_address_focused = false;
            if let Some(editor) = &mut self.browser_address {
                field_editor::apply(editor, Command::Blur, Limits::ADDRESS);
            }
            self.browser_address_drag = None;
        }
        if !self.browser_address_focused {
            return Ok(false);
        }
        let Some(editor) = self.browser_address.as_mut() else {
            return Ok(false);
        };
        let Some(outcome) = edit_host_field(editor, event, Limits::ADDRESS) else {
            return Ok(false);
        };
        match outcome.intent {
            Intent::Submit => {
                let text = editor.text.trim().to_owned();
                let url = if text.contains("://") {
                    text
                } else {
                    format!("https://{text}")
                };
                if browser::http_url_at(&url, 0).as_deref() == Some(url.as_str()) {
                    self.browser_address = None;
                    self.browser_address_focused = false;
                    self.present_http_link(window, &url);
                    self.sync_embedded_page(window);
                    #[cfg(any(target_os = "macos", windows))]
                    if let Some(page) = &self.native_browser_page {
                        page.focus();
                    }
                    #[cfg(target_os = "linux")]
                    if let Some(process) = &self.browser_process {
                        let _ = process.send("focus", None);
                    }
                }
            }
            Intent::Cancel => {
                self.browser_address = None;
                self.browser_address_focused = false;
                #[cfg(any(target_os = "macos", windows))]
                if let Some(page) = &self.native_browser_page {
                    page.focus();
                }
                #[cfg(target_os = "linux")]
                if let Some(process) = &self.browser_process {
                    let _ = process.send("focus", None);
                }
            }
            Intent::None => {}
        }
        self.update_browser_ime_anchor(window);
        self.mark_host_ui_full_and_repaint(window);
        Ok(true)
    }

    pub(super) fn update_browser_ime_anchor(&self, window: &PixelWindow) {
        if !self.browser_address_focused {
            return;
        }
        let Some(editor) = &self.browser_address else {
            return;
        };
        let Ok(metrics) = window.metrics() else {
            return;
        };
        let scale = metrics.scale_factor.max(1.0);
        let rect = self
            .layout(metrics.physical_width, metrics.physical_height, scale)
            .browser_address(scale);
        let size = scaled_host_ui_font(16, self.session_seed.font_size_logical(), scale);
        let cell = font::cell_metrics(size).width.max(1);
        let cells = (rect.width.saturating_sub((16.0 * scale) as u32) / cell).max(1) as usize;
        let visible =
            composer::visible_window(&editor.text, &editor.preedit, editor.caret, 1, cells);
        let caret = editor.caret.clamp(visible.text, editor.text.len());
        let offset = composer::cells(&editor.text[visible.text..caret])
            + composer::cells(&editor.preedit[visible.preedit..]);
        let _ = window.set_ime_cursor_area(minicon_platform::window_host::LogicalRect::new(
            f64::from(rect.x + cell.saturating_mul(offset as u32)) / scale + 8.0,
            f64::from(rect.y + rect.height.saturating_sub(font::cell_metrics(size).height) / 2)
                / scale,
            2.0,
            20.0,
        ));
    }

    fn browser_navigation(&mut self, window: &PixelWindow, control: u32) {
        if control > 2
            || (control == 0 && !self.browser_can_back)
            || (control == 1 && !self.browser_can_forward)
        {
            return;
        }
        if control == 2
            && self
                .workspace
                .browser_tab()
                .and_then(|id| self.browsers.get(&id))
                .is_some_and(|tab| {
                    matches!(tab.panel().load, browser::PageLoad::Unavailable { .. })
                })
        {
            self.retry_browser_page(window);
            return;
        }
        self.browser_address = None;
        self.browser_address_focused = false;
        self.browser_load_deadline = Some(Instant::now() + Duration::from_secs(30));
        #[cfg(target_os = "linux")]
        if let Some(process) = &self.browser_process {
            let action = match control {
                0 if self.browser_can_back => "back",
                1 if self.browser_can_forward => "forward",
                2 => "reload",
                _ => return,
            };
            if let Err(reason) = process.send(action, None) {
                self.note_page_load(browser::PageLoad::Unavailable {
                    url: String::new(),
                    reason,
                });
            }
        }
        if control == 2 && self.browser_page.is_none() {
            #[cfg(any(target_os = "macos", windows))]
            let missing = self.native_browser_page.is_none();
            #[cfg(not(any(target_os = "macos", windows)))]
            let missing = true;
            if missing {
                let url = self
                    .workspace
                    .browser_tab()
                    .and_then(|id| self.browsers.get(&id))
                    .and_then(|tab| tab.panel().url.clone());
                if let Some(url) = url {
                    self.launch_page_host(window, &url);
                }
            }
        }
        #[cfg(any(target_os = "macos", windows))]
        if let Some(page) = &self.native_browser_page {
            let result = match control {
                0 if self.browser_can_back => page.back(),
                1 if self.browser_can_forward => page.forward(),
                2 => page.reload(),
                _ => return,
            };
            if let Err(reason) = result {
                let url = self
                    .workspace
                    .browser_tab()
                    .and_then(|id| self.browsers.get(&id))
                    .and_then(|tab| tab.panel().url.clone())
                    .unwrap_or_default();
                self.note_page_load(browser::PageLoad::Unavailable { url, reason });
            }
        }
        self.mark_host_ui_full_and_repaint(window);
    }

    fn prepare_browser_retry(&mut self) -> Option<String> {
        let tab = self
            .workspace
            .browser_tab()
            .and_then(|id| self.browsers.get_mut(&id))?;
        let url = tab.panel().url.clone().filter(|url| !url.is_empty())?;
        tab.note_title("");
        tab.note_load(browser::PageLoad::Loading { url: url.clone() });
        Some(url)
    }

    /// Retry the attempted URL, even if the WebView is still on an earlier
    /// successful page. Reloading that page would silently retry the wrong URL.
    pub(super) fn retry_browser_page(&mut self, window: &PixelWindow) {
        if !self.active_is_browser() {
            return;
        }
        self.browser_address = None;
        self.browser_address_focused = false;
        self.browser_address_drag = None;
        if let Some(url) = self.prepare_browser_retry() {
            self.launch_page_host(window, &url);
        } else {
            #[cfg(any(target_os = "macos", windows))]
            {
                self.ensure_native_page(window);
                if self.native_browser_page.is_some() {
                    self.note_page_load(browser::PageLoad::Idle);
                }
            }
            #[cfg(not(any(target_os = "macos", windows)))]
            self.focus_browser_address(window);
        }
        self.sync_embedded_page(window);
        let _ = self.refresh_title(window);
        self.mark_host_ui_full_and_repaint(window);
    }

    fn browser_host_shortcut(&mut self, window: &PixelWindow, shortcut: char) {
        if shortcut == 'L' {
            self.focus_browser_address(window);
        } else if let Some(control) = match shortcut {
            '<' => Some(0),
            '>' => Some(1),
            'R' => Some(2),
            _ => None,
        } {
            self.browser_navigation(window, control);
        } else {
            let key = injected_key_event(
                InjectedKey::Char(shortcut.to_ascii_lowercase()),
                true,
                false,
                true,
            );
            let _ = self.handle_workspace_shortcut(window, &key);
        }
    }

    pub(super) fn consume_browser_observation(
        &mut self,
        state: BrowserObservation,
        mut shortcut: impl FnMut(&mut Self, char),
    ) -> bool {
        let owner = (
            self.workspace.browser_tab(),
            self.browser_auth.as_ref().map(|auth| auth.attempt),
        );
        for key in state.shortcuts {
            shortcut(self, key);
            if owner
                != (
                    self.workspace.browser_tab(),
                    self.browser_auth.as_ref().map(|auth| auth.attempt),
                )
            {
                return false;
            }
        }
        if let Some(back) = state.can_back {
            self.browser_can_back = back;
        }
        if let Some(forward) = state.can_forward {
            self.browser_can_forward = forward;
        }
        if let Some(loading) = state.loading {
            self.browser_load_deadline = if loading {
                self.browser_load_deadline
                    .or(Some(Instant::now() + Duration::from_secs(30)))
            } else {
                None
            };
        }
        if let Some(id) = self.workspace.browser_tab()
            && let Some(tab) = self.browsers.get_mut(&id)
        {
            if let Some(title) = state.title {
                tab.note_title(&title);
            }
            let url = state.url.unwrap_or_default();
            if let Some(reason) = state.error {
                let url = tab.panel().url.clone().unwrap_or(url);
                tab.note_load(browser::PageLoad::Unavailable { url, reason });
            } else if browser::http_url_at(&url, 0).as_deref() == Some(url.as_str()) {
                tab.note_load(if state.loading.unwrap_or(false) {
                    browser::PageLoad::Loading { url }
                } else {
                    browser::PageLoad::Ready { url }
                });
            }
        }
        true
    }

    #[cfg(any(target_os = "macos", windows))]
    pub(super) fn drain_browser_state(&mut self, window: &PixelWindow) {
        let Some(state) = self
            .native_browser_page
            .as_ref()
            .and_then(|page| page.take_state())
        else {
            return;
        };
        let observation = BrowserObservation {
            title: Some(state.title),
            url: Some(state.url),
            error: state.error,
            loading: Some(state.loading),
            can_back: Some(state.can_back),
            can_forward: Some(state.can_forward),
            shortcuts: state.shortcuts,
        };
        if !self.consume_browser_observation(observation, |app, key| {
            app.browser_host_shortcut(window, key)
        }) {
            return;
        }
        if let Some(page) = &self.native_browser_page {
            let _ = page.set_visible(self.browser_page_visible());
        }
        if self.active_is_browser() {
            let _ = self.refresh_title(window);
        }
        self.mark_host_ui_full_and_repaint(window);
    }

    #[cfg(target_os = "linux")]
    pub(super) fn drain_browser_process(&mut self, window: &PixelWindow) {
        let state = self
            .browser_process
            .as_ref()
            .and_then(|process| process.take_state());
        if let Some(state) = state {
            let observation = BrowserObservation {
                title: state["title"].as_str().map(str::to_owned),
                url: state["url"].as_str().map(str::to_owned),
                error: state["error"].as_str().map(str::to_owned),
                loading: state["loading"].as_bool(),
                can_back: state["can_back"].as_bool(),
                can_forward: state["can_forward"].as_bool(),
                shortcuts: state["shortcuts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|key| key.as_str().and_then(|s| s.chars().next()))
                    .collect(),
            };
            if !self.consume_browser_observation(observation, |app, key| {
                app.browser_host_shortcut(window, key)
            }) {
                return;
            }
            if self.active_is_browser() {
                let _ = self.refresh_title(window);
            }
            // The helper is born asynchronously. A tab/settings/geometry
            // change may predate its X11 child, so reapply the current bounds
            // and visibility once its first native state arrives.
            self.sync_embedded_page(window);
            self.mark_host_ui_full_and_repaint(window);
        }
        if let Some(child) = &mut self.browser_page
            && let Ok(Some(status)) = child.try_wait()
        {
            let url = self
                .workspace
                .browser_tab()
                .and_then(|id| self.browsers.get(&id))
                .and_then(|tab| tab.panel().url.clone())
                .unwrap_or_default();
            self.stop_page_host();
            self.note_page_load(browser::page_load_from_exit(&url, status.code()));
            self.mark_host_ui_full_and_repaint(window);
        }
    }

    pub(super) fn expire_browser_load(&mut self, window: &PixelWindow, now: Instant) {
        if self
            .browser_load_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            self.browser_load_deadline = None;
            let url = self
                .workspace
                .browser_tab()
                .and_then(|id| self.browsers.get(&id))
                .and_then(|tab| tab.panel().url.clone())
                .unwrap_or_default();
            if let Some(id) = self.workspace.browser_tab()
                && let Some(tab) = self.browsers.get_mut(&id)
            {
                tab.note_load(browser::PageLoad::Unavailable {
                    url,
                    reason: "browser-navigation-timeout".to_owned(),
                });
            }
            self.sync_embedded_page(window);
            self.mark_host_ui_full_and_repaint(window);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_keeps_the_failed_url_and_clears_the_previous_page_title_and_error() {
        let mut app = ConApp::new(None, None);
        let attempted = "http://localhost/preview";
        assert!(ConApp::handoff_http_link(
            &mut app.workspace,
            &mut app.browsers,
            attempted
        ));
        let id = app.workspace.browser_tab().unwrap();
        let tab = app.browsers.get_mut(&id).unwrap();
        tab.note_title("Earlier successful page");
        tab.note_load(browser::PageLoad::Unavailable {
            url: attempted.into(),
            reason: "browser-navigation-timeout".into(),
        });
        assert_eq!(app.prepare_browser_retry().as_deref(), Some(attempted));
        let panel = app.browsers.get(&id).unwrap().panel();
        assert_eq!(panel.url.as_deref(), Some(attempted));
        assert!(panel.title.is_empty());
        assert_eq!(
            panel.load,
            browser::PageLoad::Loading {
                url: attempted.into()
            }
        );
    }

    #[test]
    fn retry_without_an_attempted_url_does_not_create_a_navigation() {
        let mut app = ConApp::new(None, None);
        assert_eq!(app.prepare_browser_retry(), None);
        assert!(app.workspace.browser_tab().is_none());
    }

    #[test]
    fn refocusing_browser_address_preserves_the_unsubmitted_draft() {
        let mut app = ConApp::new(None, None);
        app.begin_browser_address();
        composer::insert(
            app.browser_address.as_mut().unwrap(),
            "https://example.test/draft",
        );
        app.begin_browser_address();
        let editor = app.browser_address.as_ref().unwrap();
        assert_eq!(editor.text, "https://example.test/draft");
        assert_eq!(composer::selection_text(editor), Some(editor.text.as_str()));
    }

    #[test]
    fn browser_address_click_inserts_at_the_caret_without_replacing_the_draft() {
        let mut app = ConApp::new(None, None);
        app.begin_browser_address();
        composer::insert(
            app.browser_address.as_mut().unwrap(),
            "https://example.test/path",
        );
        app.press_browser_address(21, false);
        let editor = app.browser_address.as_mut().unwrap();
        assert_eq!(composer::selection_text(editor), None);
        composer::insert(editor, "new/");
        assert_eq!(editor.text, "https://example.test/new/path");
    }

    #[test]
    fn browser_address_shift_click_extends_from_the_existing_caret() {
        let mut app = ConApp::new(None, None);
        app.begin_browser_address();
        composer::insert(
            app.browser_address.as_mut().unwrap(),
            "https://example.test/path",
        );
        app.press_browser_address(21, false);
        app.press_browser_address(25, true);
        assert_eq!(
            composer::selection_text(app.browser_address.as_ref().unwrap()),
            Some("path")
        );
    }

    #[test]
    fn browser_address_double_click_selects_a_token_and_ends_dragging() {
        let mut app = ConApp::new(None, None);
        app.begin_browser_address();
        composer::insert(
            app.browser_address.as_mut().unwrap(),
            "https://example.test/path",
        );
        app.press_browser_address(10, false);
        app.press_browser_address(10, false);
        assert_eq!(
            composer::selection_text(app.browser_address.as_ref().unwrap()),
            Some("https://example.test/path")
        );
        assert!(app.browser_address_drag.is_none());
    }
}
