//! Control-endpoint dispatch: how an incoming `minicon cli` request is served.
//!
//! Holds the request pump (`drain_control`), the windowed and windowless
//! dispatch paths, and the detached turn that keeps a headless process
//! answering control while no surface exists.
//!
//! Moved out of `main.rs` verbatim; no behavior change.

use super::*;

impl ConApp {
    fn validate_self_context(
        &self,
        context: Option<control::SelfContext>,
        command: &control::CliCommand,
    ) -> Result<(), String> {
        let Some(context) = context else {
            return Ok(());
        };
        if self.instance_id != Some(context.instance) {
            return Err("self target belongs to another MiniCon instance".to_owned());
        }
        if !self.workspace.node(context.tab).is_some_and(|node| {
            node.kind == workspace::TabKind::Terminal && self.session_for(context.tab).is_some()
        }) {
            return Err(format!(
                "self terminal @{} no longer exists",
                context.tab.get()
            ));
        }
        if !command.targets_self(context.tab) {
            return Err("self target does not match the control command".to_owned());
        }
        Ok(())
    }

    fn reap_finished_control_screenshot(&mut self) {
        self.pending_control.reap_finished_screenshot();
    }

    fn validate_control_cell(session: &ConTerminal, row: u16, column: u16) -> Result<(), String> {
        if row >= session.rows || column >= session.cols {
            return Err(format!(
                "mouse cell {row},{column} is outside {}x{}",
                session.rows, session.cols
            ));
        }
        Ok(())
    }

    /// Commands that need no window, so one implementation serves both the
    /// attached path and a detached process.
    ///
    /// `None` means "this one needs a window"; the caller decides whether that
    /// is a normal dispatch or a typed refusal.
    fn windowless_command(
        &mut self,
        command: &control::CliCommand,
    ) -> Option<Result<json::JsonValue, String>> {
        use control::CliCommand;
        self.apply_session_titles();
        match command {
            CliCommand::ListTabs => {
                Some({
                    let active = self.workspace.active();
                    let tabs: Vec<_> = self
                    .workspace
                    .nodes()
                    .iter()
                    .map(|node| {
                        let session = self.session_for(node.id);
                        let auth_role_page = self
                            .browser_auth
                            .as_ref()
                            .is_some_and(|auth| auth.tab_id == node.id);
                        let authorization_page = self.browsers.get(&node.id).is_some_and(|tab| {
                            auth_role_page || tab.panel()
                                .url
                                .as_deref()
                                .is_some_and(browser_ui::is_authorization_page)
                        });
                        let (can_back, can_forward) = self.browser_navigation_flags(node.id);
                        json::object(vec![
                            ("id", tab_id_json(Some(node.id))),
                            ("parent", tab_id_json(node.parent)),
                            (
                                "title",
                                if authorization_page {
                                    "Authorization page"
                                } else {
                                    session.map_or(node.title.as_str(), |session| {
                                        session.current_title.as_str()
                                    })
                                }
                                .into(),
                            ),
                            ("active", (active == Some(node.id)).into()),
                            (
                                "kind",
                                match node.kind {
                                    workspace::TabKind::Terminal => "terminal",
                                    workspace::TabKind::Browser => "browser",
                                }
                                .into(),
                            ),
                            (
                                "browser",
                                self.browsers
                                    .get(&node.id)
                                    .map_or(json::JsonValue::Null, |tab| {
                                        let panel = tab.panel();
                                        let (load, reason) = match &panel.load {
                                            browser::PageLoad::Idle => ("idle", None),
                                            browser::PageLoad::Loading { .. } => ("loading", None),
                                            browser::PageLoad::Ready { .. } => ("ready", None),
                                            browser::PageLoad::Unavailable { reason, .. } => {
                                                ("unavailable", Some(reason.as_str()))
                                            }
                                        };
                                        json::object(vec![
                                            (
                                                "url",
                                                panel
                                                    .url
                                                    .as_deref()
                                                    .map(|url| if auth_role_page { "authorization://private" } else { browser_ui::public_browser_url(url) })
                                                    .map_or(json::JsonValue::Null, Into::into),
                                            ),
                                            ("url_redacted", authorization_page.into()),
                                            ("load", load.into()),
                                            (
                                                "title",
                                                if authorization_page {
                                                    "Authorization page"
                                                } else {
                                                    panel.title.as_str()
                                                }
                                                .into(),
                                            ),
                                            ("can_back", can_back.into()),
                                            ("can_forward", can_forward.into()),
                                            (
                                                "reason",
                                                reason.map_or(json::JsonValue::Null, |reason| {
                                                    if authorization_page {
                                                        browser_ui::public_authorization_failure(
                                                            reason,
                                                        )
                                                        .into()
                                                    } else {
                                                        browser_ui::public_browser_text(reason)
                                                            .as_ref()
                                                            .into()
                                                    }
                                                }),
                                            ),
                                        ])
                                    }),
                            ),
                            (
                                "child_alive",
                                session.is_some_and(|session| !session.child_gone).into(),
                            ),
                            (
                                "child_exit_code",
                                exit_code_json(session.and_then(|session| session.child_exit_code)),
                            ),
                            // Each tab owns its own `ConTerminal`, which already tracks its
                            // live `cols`/`rows` -- serving them here costs nothing extra and
                            // answers `mux list-panes`'s `pane_width`/`pane_height` from the
                            // same response `list-tabs` already returns, with no per-tab
                            // round trip (see prd/PRD_02_31_v0_2_horizon.md's "mux hardening
                            // against moltbaby-shaped real usage" for why this was BLOCKED).
                            ("cols", session.map_or(0, |session| session.cols).into()),
                            ("rows", session.map_or(0, |session| session.rows).into()),
                        ])
                    })
                    .collect();
                    Ok(single_field_json("tabs", json::JsonValue::Array(tabs)))
                })
            }
            CliCommand::CapturePane {
                target, max_bytes, ..
            } => Some(self.control_session_mut(*target).map(|session| {
                session.drain_pty();
                let max_bytes = *max_bytes;
                let mut text = session.build_snapshot().rows_text.join("\n");
                if text.len() > max_bytes {
                    let mut end = max_bytes;
                    while end > 0 && !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    text.truncate(end);
                }
                json::JsonValue::String(text)
            })),
            CliCommand::SendText { target, text } => {
                let text = text.clone();
                Some(
                    self.send_to_control_terminal(*target, "sent_bytes", |session| {
                        session.scroll_to_bottom();
                        session.write_pty(text.as_bytes()).map(|()| text.len())
                    }),
                )
            }
            CliCommand::SetWindowAttached { attached } => {
                let attached = *attached;
                if !attached {
                    self.pending_paste_review = None;
                }
                Some(match self.attachment.as_ref() {
                    Some(attachment) => attachment
                        .set(attached)
                        .map_err(|error| error.to_string())
                        .map(|()| single_field_json("attached", attached.into())),
                    None => Err("this platform has no detachable window".to_owned()),
                })
            }
            _ => None,
        }
    }

    fn dispatch_control(&mut self, window: &PixelWindow, request: control::IncomingRequest) {
        use control::CliCommand;
        if let Err(error) = self.validate_self_context(request.self_context, &request.command) {
            let _ = request.reply.send(Err(error));
            return;
        }
        self.perf_stats.sync_present_stats(window.present_stats());
        self.poll_harness_settings(window);
        if matches!(&request.command, CliCommand::UiSnapshot) && self.refresh_ime_status() {
            self.request_dirty_redraw(window);
        }
        let mut reply = Some(request.reply);
        if let Some(result) = self.windowless_command(&request.command) {
            if let Some(reply) = reply.take() {
                let _ = reply.send(result);
            }
            return;
        }
        let result = match request.command {
            // Served above by `windowless_command`; unreachable here, and kept
            // only so this match stays exhaustive over the public verbs.
            CliCommand::SetWindowAttached { .. }
            | CliCommand::ListTabs
            | CliCommand::CapturePane { .. }
            | CliCommand::SendText { .. } => {
                Err("this command is handled before dispatch".to_owned())
            }
            CliCommand::UiSnapshot => self.ui_snapshot_json(window),
            CliCommand::PerfStats => Ok(self.perf_stats.json()),
            CliCommand::ResetPerfStats => {
                self.perf_stats.reset(window.present_stats());
                Ok(single_field_json("reset", true.into()))
            }
            CliCommand::CancelPointer => {
                let cancelled = self.control_pointer_owner;
                self.cancel_pointer_gestures_for_activation(window);
                Ok(single_field_json("cancelled_owner", tab_id_json(cancelled)))
            }
            CliCommand::CloseWindow => {
                self.cancel_pointer_gestures_for_activation(window);
                self.cancel_all_control_requests(
                    "terminal window closed while control request was pending",
                );
                self.exit = true;
                Ok(single_field_json("closing", true.into()))
            }
            CliCommand::ResizeWindow { width, height } => window
                .request_logical_inner_size(LogicalSize::new(f64::from(width), f64::from(height)))
                .map_err(|error| error.to_string())
                .map(|()| json::object(vec![("width", width.into()), ("height", height.into())])),
            CliCommand::NewTab { parent } => (|| {
                if let Some(parent) = parent {
                    self.control_target(Some(parent))?;
                    self.activate_session(window, parent);
                }
                self.open_session(window, parent.is_some())
                    .map_err(|error| error.to_string())?;
                let id = self
                    .workspace
                    .active()
                    .ok_or_else(|| "new terminal was not activated".to_owned())?;
                Ok(json::object(vec![
                    ("id", tab_id_json(Some(id))),
                    ("parent", tab_id_json(parent)),
                ]))
            })(),
            CliCommand::SelectTab { target } => {
                if self.workspace.node(target).is_none() {
                    Err(format!("terminal @{} does not exist", target.get()))
                } else {
                    self.mark_host_ui_full();
                    self.activate_session(window, target);
                    window.request_redraw();
                    Ok(single_field_json("active", tab_id_json(Some(target))))
                }
            }
            CliCommand::CloseTab { target } => (|| {
                // Lifecycle belongs to every tab; PTY-only targeting would
                // reject a browser row that list-tabs just returned.
                if self.workspace.node(target).is_none() {
                    return Err(format!("terminal @{} does not exist", target.get()));
                }
                let home_before = self.home_active;
                let previous_active = self.workspace.active();
                self.activate_session(window, target);
                self.close_active_session(window)
                    .map_err(|error| error.to_string())?;
                if home_before && self.browser_auth.is_none() {
                    if let Some(previous) =
                        previous_active.filter(|id| self.workspace.node(*id).is_some())
                    {
                        self.select_tab(previous);
                    }
                    self.show_home(window);
                }
                if self.workspace.node(target).is_some() {
                    Ok(single_field_json(
                        "authorization_dismissed",
                        tab_id_json(Some(target)),
                    ))
                } else {
                    Ok(single_field_json("closed", tab_id_json(Some(target))))
                }
            })(),

            CliCommand::SendPaste { target, text } => {
                self.send_to_control_terminal(target, "sent_bytes", |session| {
                    session.paste_text(&text).map(|()| text.len())
                })
            }
            CliCommand::SendKeys { target, keys } => (|| {
                let id = self.control_target(target)?;
                // Validate every key before injecting any of them, so a
                // malformed key late in the sequence cannot leave an earlier
                // one already delivered.
                let parsed = parse_control_keys(&keys)?;
                self.forward_keys_to_terminal(window, id, |session| {
                    for (key, ctrl, alt, shift) in parsed {
                        session
                            .inject_key(key, ctrl, alt, shift)
                            .map_err(|error| format!("terminal input failed: {error}"))?;
                    }
                    Ok(())
                })?;
                Ok(single_field_json("sent_keys", keys.len().into()))
            })(),
            CliCommand::SendUiKeys { keys } => (|| {
                // Same rule as `SendKeys`, and it matters more here: a UI key
                // can move focus or open a menu, so a prefix applied before a
                // later key is rejected leaves the interface in a state the
                // caller never asked for and cannot predict from the error.
                let parsed = parse_ui_control_keys(&keys)?;
                // Injected keys take the exact routing a physical keystroke
                // takes — workspace shortcut, live rename, composer, then the
                // terminal — by going through `event` itself. A private copy
                // of that routing here once drifted: rename shipped with typed
                // text invisible because only the real path knew the damage
                // contract. One routing authority; the injection site adds no
                // routing of its own. The provenance flag rides along so the
                // paste-chord drain inside `event` can keep the documented
                // injected-bypasses-review contract (PRD 02.26) without the
                // two paths diverging anywhere else.
                self.injected_ui_keys_live = true;
                let delivered: Result<(), String> = (|| {
                    for mut event in parsed {
                        self.event(window, PixelWindowEvent::Keyboard(event.clone()))
                            .map_err(|error| error.to_string())?;
                        // A logical chord owns press through release, just as
                        // native input does. Keep sent_keys counting chords.
                        event.state = KeyPressState::Released;
                        event.text = None;
                        self.event(window, PixelWindowEvent::Keyboard(event))
                            .map_err(|error| error.to_string())?;
                    }
                    Ok(())
                })();
                self.injected_ui_keys_live = false;
                delivered?;
                self.request_dirty_redraw(window);
                Ok(single_field_json("sent_keys", keys.len().into()))
            })(),
            CliCommand::SendUiIme { event } => (|| {
                let action = match &event {
                    minicon_platform::ime::ImeEvent::Enabled => "enabled",
                    minicon_platform::ime::ImeEvent::Preedit { .. } => "preedit",
                    minicon_platform::ime::ImeEvent::Commit(_) => "commit",
                    minicon_platform::ime::ImeEvent::Disabled => "disabled",
                    _ => "unknown",
                };
                // Preedit, enable and disable are local-only state and stay
                // available on a retained exited tab; a commit writes the
                // PTY and carries the same explicit exited-tab contract as
                // send-keys (PRD 02.26) — checked before anything moves, so
                // a refused commit also leaves the preedit untouched.
                if matches!(event, minicon_platform::ime::ImeEvent::Commit(_))
                    && !self.harness_has_input_owner()
                    && self.rename.is_none()
                    && !self.composer.focused
                    && !self.browser_address_focused
                    && !self.browser_proxy_focused
                    && !self.settings_proxy_focused
                    && !(self.settings_open
                        && self.settings_section == ui::SettingsSection::Harness)
                {
                    let session = self.active_session().map_err(|error| error.to_string())?;
                    session.ensure_pty_input_open()?;
                }
                // Same single-routing-authority rule as `SendUiKeys` above:
                // the IME event rides the real `event` pipeline, so a live
                // rename or focused composer claims it exactly as it would a
                // physical IME delivery.
                let delivered = PixelWindowEvent::Ime(event);
                self.event(window, delivered)
                    .map_err(|error| error.to_string())?;
                self.request_dirty_redraw(window);
                // The reply's `route` names the surface that owns IME input
                // after delivery — the contract tests pin `terminal` and
                // `composer` against it. Delivery decides the owner, so the
                // reply reads the post-delivery state instead of predicting
                // it.
                let route = if self.settings_open
                    && self.settings_section == ui::SettingsSection::Harness
                {
                    self.harness_input_owner()
                } else if let Some(owner) = self.harness_workspace_input_owner() {
                    owner
                } else if self.rename.is_some() {
                    "rename"
                } else if self.settings_proxy_focused {
                    "web_proxy_candidate"
                } else if self.browser_proxy_focused {
                    "browser_proxy"
                } else if self.browser_address.is_some() {
                    "browser_address"
                } else if self.composer.focused {
                    "composer"
                } else {
                    "terminal"
                };
                Ok(json::object(vec![
                    ("action", action.into()),
                    ("route", route.into()),
                ]))
            })(),
            CliCommand::SendUiMouse {
                action,
                button,
                column,
                row,
            } => (|| {
                // The cell-addressed `send-mouse` reaches one terminal's grid;
                // host UI (the sidebar tree, the composer chrome) has no cell
                // address at all. This command injects at window logical
                // pixels and rides the same `event` pipeline a physical mouse
                // drives, so hover, hit-testing and focus resolve identically
                // — which is what makes the pencil-click rename journey a
                // blackbox test instead of a whitebox reproduction.
                let position = LogicalPoint::new(f64::from(column), f64::from(row));
                let modifiers = ModifierState::default();
                let moved = PixelWindowEvent::PointerMoved {
                    position,
                    modifiers,
                };
                let events: Vec<PixelWindowEvent> = match action {
                    control::MouseAction::Move => vec![moved],
                    control::MouseAction::Press
                    | control::MouseAction::Release
                    | control::MouseAction::Click => {
                        let platform_button = match button {
                            control::MouseButton::Left => PointerButton::Left,
                            control::MouseButton::Middle => PointerButton::Middle,
                            control::MouseButton::Right => PointerButton::Right,
                            control::MouseButton::None => {
                                return Err(
                                    "send-ui-mouse button actions require a button".to_owned()
                                );
                            }
                        };
                        let pointer = |state| PixelWindowEvent::PointerButton {
                            button: platform_button,
                            state,
                            position: Some(position),
                            modifiers,
                        };
                        match action {
                            control::MouseAction::Press => {
                                vec![moved, pointer(PointerButtonState::Pressed)]
                            }
                            control::MouseAction::Release => {
                                vec![pointer(PointerButtonState::Released)]
                            }
                            _ => vec![
                                moved,
                                pointer(PointerButtonState::Pressed),
                                pointer(PointerButtonState::Released),
                            ],
                        }
                    }
                };
                let count = events.len();
                for event in events {
                    self.event(window, event)
                        .map_err(|error| error.to_string())?;
                }
                self.request_dirty_redraw(window);
                Ok(single_field_json("injected_events", count.into()))
            })(),
            CliCommand::SendMouse {
                target,
                action,
                button,
                column,
                row,
            } => (|| {
                let id = self.control_target(target)?;
                match (action, self.control_pointer_owner) {
                    (control::MouseAction::Press, Some(owner)) => {
                        return Err(format!(
                            "control pointer gesture is already owned by @{}",
                            owner.get()
                        ));
                    }
                    (control::MouseAction::Release, owner) if owner != Some(id) => {
                        return Err(format!(
                            "no matching control pointer press for @{}",
                            id.get()
                        ));
                    }
                    (control::MouseAction::Click, Some(owner)) => {
                        return Err(format!(
                            "control pointer gesture is already owned by @{}",
                            owner.get()
                        ));
                    }
                    (control::MouseAction::Move, Some(owner)) if owner != id => {
                        return Err(format!(
                            "control pointer gesture is owned by @{}",
                            owner.get()
                        ));
                    }
                    _ => {}
                }
                let outcome = {
                    let session = self
                        .sessions
                        .get_mut(&id)
                        .ok_or_else(|| format!("terminal @{} is unavailable", id.get()))?;
                    Self::validate_control_cell(session, row, column)?;
                    match action {
                        control::MouseAction::Move => {
                            session.inject_mouse_move(window, row, column)
                        }
                        control::MouseAction::Click => {
                            let button = control_mouse_button(button)?;
                            session.inject_click(window, row, column, button)
                        }
                        control::MouseAction::Press | control::MouseAction::Release => {
                            let button = control_mouse_button(button)?;
                            let state = if action == control::MouseAction::Press {
                                PointerButtonState::Pressed
                            } else {
                                PointerButtonState::Released
                            };
                            session.inject_pointer_button(window, row, column, button, state)
                        }
                    }
                };
                if action == control::MouseAction::Release {
                    self.control_pointer_owner = None;
                }
                let outcome = outcome.map_err(|error| format!("terminal input failed: {error}"))?;
                if action == control::MouseAction::Press {
                    self.control_pointer_owner = Some(id);
                }
                let requested = self
                    .sessions
                    .get_mut(&id)
                    .is_some_and(ConTerminal::take_clipboard_paste_request);
                if requested {
                    self.request_terminal_clipboard_paste(window, id, false)?;
                }
                Ok(mouse_outcome_json(outcome))
            })(),
            CliCommand::SendWheel {
                target,
                column,
                row,
                notches,
                ctrl,
            } => self.control_session_mut(target).and_then(|session| {
                Self::validate_control_cell(session, row, column)?;
                session
                    .inject_wheel(window, row, column, f32::from(notches), ctrl)
                    .map(wheel_outcome_json)
                    .map_err(|error| format!("terminal input failed: {error}"))
            }),
            CliCommand::ScreenshotPane { target, output } => {
                let capture_target = if target.is_none() && self.harness_workspace.active() {
                    Ok(None)
                } else {
                    self.control_target(target).map(Some)
                };
                capture_target.and_then(|id| {
                    agent_interface::initialize_png_worker()
                        .map_err(|error| format!("initialize PNG worker: {error}"))?;
                    self.pending_control.enqueue_screenshot(
                        id,
                        PathBuf::from(output),
                        &mut reply,
                    )?;
                    window.request_redraw();
                    Ok(json::JsonValue::Null)
                })
            }
            CliCommand::WaitText {
                target,
                text,
                timeout_ms,
            } => self.control_target(target).and_then(|id| {
                if self
                    .sessions
                    .get(&id)
                    .is_some_and(|session| session.screen_contains(&text))
                {
                    return Ok(single_field_json("matched", true.into()));
                }
                self.pending_control.enqueue_wait(
                    id,
                    WaitKind::Text(text),
                    timeout_ms,
                    &mut reply,
                    "too many pending wait-text requests",
                )?;
                Ok(json::JsonValue::Null)
            }),
            CliCommand::WaitTabExit { target, timeout_ms } => {
                self.control_target(Some(target)).and_then(|id| {
                    if let Some(session) = self.sessions.get(&id)
                        && session.child_gone
                    {
                        return Ok(tab_exit_json(id, session.child_exit_code));
                    }
                    self.pending_control.enqueue_wait(
                        id,
                        WaitKind::TabExit,
                        timeout_ms,
                        &mut reply,
                        "too many pending control wait requests",
                    )?;
                    Ok(json::JsonValue::Null)
                })
            }
        };
        if let Some(reply) = reply {
            let _ = reply.send(result);
        }
    }

    /// One loop turn with no window attached.
    ///
    /// The process still owns its sessions and its endpoint, so both keep
    /// working: PTY output is drained so nothing is lost, and control requests
    /// are answered — served where the verb needs no window, refused by name
    /// where it does. A silent hang would be the worst of the three.
    pub(crate) fn detached_turn(&mut self) -> Result<PixelWindowDirective, PixelWindowError> {
        if self.exit {
            return Ok(PixelWindowDirective::Exit);
        }
        for session in self
            .workspace
            .nodes()
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>()
        {
            if let Some(session) = self.sessions.get_mut(&session) {
                session.drain_pty();
            }
        }
        let (requests, _backlog) = self
            .control_server
            .as_ref()
            .map(|server| server.recv_batch(CONTROL_DRAIN_BUDGET_REQUESTS))
            .unwrap_or_else(|| (Vec::new(), false));
        for request in requests {
            self.perf_stats
                .record_control_dispatch_wait(request.reply.age_before_dispatch());
            let result = self
                .validate_self_context(request.self_context, &request.command)
                .and_then(|()| {
                    self.windowless_command(&request.command)
                        .unwrap_or_else(|| {
                            Err("the window is detached; attach-gui first".to_owned())
                        })
                });
            let _ = request.reply.send(result);
        }
        if self.exit {
            return Ok(PixelWindowDirective::Exit);
        }
        Ok(PixelWindowDirective::Wait)
    }

    pub(crate) fn drain_control(&mut self, window: &PixelWindow, now: Instant) -> Option<Instant> {
        self.reap_finished_control_screenshot();
        let (requests, backlog) = self
            .control_server
            .as_ref()
            .map(|server| server.recv_batch(CONTROL_DRAIN_BUDGET_REQUESTS))
            .unwrap_or_else(|| (Vec::new(), false));
        let mut requests: std::collections::VecDeque<_> = requests.into();
        while let Some(request) = requests.pop_front() {
            self.perf_stats
                .record_control_dispatch_wait(request.reply.age_before_dispatch());
            if matches!(&request.command, control::CliCommand::ResizeWindow { .. }) {
                self.pending_resize_requests.push(request);
                self.pending_resize_deadline
                    .get_or_insert(now + std::time::Duration::from_millis(4));
            } else {
                // A non-resize command is an ordering barrier. Screenshots and
                // snapshots must observe every resize accepted before them,
                // while resize-only bursts may share one bounded native call.
                self.flush_pending_resize(window);
                self.perf_stats.control_requests =
                    self.perf_stats.control_requests.saturating_add(1);
                self.dispatch_control(window, request);
            }
        }
        if self
            .pending_resize_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            self.flush_pending_resize(window);
        }
        if backlog {
            self.perf_stats.control_budget_yields =
                self.perf_stats.control_budget_yields.saturating_add(1);
            let _ = window.waker().wake();
        }
        if !self.host_ui_dirty.is_empty() {
            self.request_dirty_redraw(window);
        }
        let sessions = &self.sessions;
        let wait_deadline = self.pending_control.poll_waits(now, |target, kind| {
            let Some(session) = sessions.get(&target) else {
                return WaitProbe::Missing(format!(
                    "terminal @{} disappeared while control request was pending",
                    target.get()
                ));
            };
            match kind {
                WaitKind::Text(text) if session.screen_contains(text) => {
                    WaitProbe::Completed(single_field_json("matched", true.into()))
                }
                WaitKind::TabExit if session.child_gone => {
                    WaitProbe::Completed(tab_exit_json(target, session.child_exit_code))
                }
                _ => WaitProbe::Pending,
            }
        });
        match (wait_deadline, self.pending_resize_deadline) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        }
    }
}

#[cfg(test)]
mod title_tests {
    use super::*;

    #[test]
    fn list_tabs_keeps_parked_web_flags_and_other_web_public_during_auth() {
        let mut app = ConApp::new(None, None);
        let auth_id = app.workspace.add_browser_root().unwrap();
        let ordinary_id = app.workspace.add_browser_root().unwrap();
        let auth_url = "https://platform.deepseek.com/dsh/authorized?state=private-token";
        let ordinary_url = "https://ordinary.test/page?public=yes";
        let mut auth_tab = browser::BrowserTab::shell();
        assert!(auth_tab.show_http(auth_url));
        let mut ordinary_tab = browser::BrowserTab::shell();
        assert!(ordinary_tab.show_http(ordinary_url));
        ordinary_tab.note_title("Public page");
        assert!(app.browsers.insert(auth_id, auth_tab).is_ok());
        assert!(app.browsers.insert(ordinary_id, ordinary_tab).is_ok());

        app.loaded_browser_id = Some(auth_id);
        app.browser_can_forward = true;
        let parked_auth = harness_auth_ui::OrdinaryBrowser::take_current(&mut app);
        assert!(app.parked_browsers.insert(auth_id, parked_auth).is_ok());
        let ordinary = harness_auth_ui::OrdinaryBrowser::take_current(&mut app);
        app.loaded_browser_id = Some(ordinary_id);
        app.browser_can_back = true;
        app.browser_auth = Some(harness_auth_ui::AuthPresentation {
            tab_id: auth_id,
            attempt: provider_runtime::AttemptId(41),
            context: browser::PageContext {
                profile: minicon_web::PageProfile {
                    data_directory: std::env::temp_dir().join("unused-list-tabs-auth"),
                    store_identifier: [4; 16],
                },
                proxy: browser::BrowserProxy::default(),
            },
            ordinary,
        });

        let reply = app
            .windowless_command(&control::CliCommand::ListTabs)
            .unwrap()
            .unwrap();
        let observed: serde_json::Value = serde_json::from_slice(&json::to_vec(&reply)).unwrap();
        let rows = observed["tabs"].as_array().unwrap();
        let auth = rows
            .iter()
            .find(|row| row["id"] == format!("@{}", auth_id.get()))
            .unwrap();
        let other = rows
            .iter()
            .find(|row| row["id"] == format!("@{}", ordinary_id.get()))
            .unwrap();
        assert_eq!(auth["browser"]["url"], "authorization://private");
        assert_eq!(auth["browser"]["url_redacted"], true);
        assert_eq!(auth["browser"]["can_back"], false);
        assert_eq!(auth["browser"]["can_forward"], true);
        assert_eq!(other["browser"]["url"], ordinary_url);
        assert_eq!(other["browser"]["url_redacted"], false);
        assert_eq!(other["browser"]["title"], "Public page");
        assert_eq!(other["browser"]["can_back"], true);
        assert_eq!(other["browser"]["can_forward"], false);
        assert!(!observed.to_string().contains("private-token"));
    }

    fn authorization_failure_observation(
        app: &mut ConApp,
        url: &str,
        reason: &str,
    ) -> serde_json::Value {
        assert!(ConApp::handoff_http_link(
            &mut app.workspace,
            &mut app.browsers,
            url,
        ));
        let id = app.workspace.browser_tab().unwrap();
        let tab = app.browsers.get_mut(&id).unwrap();
        tab.note_title("SYNTHETIC-TITLE-PRIVATE");
        tab.note_load(browser::PageLoad::Unavailable {
            url: url.into(),
            reason: reason.into(),
        });
        let reply = app
            .windowless_command(&control::CliCommand::ListTabs)
            .unwrap()
            .unwrap();
        let observed: serde_json::Value = serde_json::from_slice(&json::to_vec(&reply)).unwrap();
        let encoded = observed.to_string();
        for secret in [
            "SYNTHETIC-STATE-PRIVATE",
            "SYNTHETIC-CODE-PRIVATE",
            "SYNTHETIC-FRAGMENT-PRIVATE",
            "SYNTHETIC-TITLE-PRIVATE",
            "SYNTHETIC-ERROR-PRIVATE",
            "SYNTHETIC-USER-CODE-PRIVATE",
        ] {
            assert!(!encoded.contains(secret), "private browser metadata leaked");
        }
        let browser = observed["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tab| tab["kind"] == "browser")
            .unwrap()["browser"]
            .clone();
        assert_eq!(browser["url"], url.split(['?', '#']).next().unwrap());
        assert_eq!(browser["url_redacted"], true);
        assert_eq!(browser["title"], "Authorization page");
        assert_eq!(browser["load"], "unavailable");
        browser
    }

    #[test]
    fn authorization_failure_observation_preserves_only_fixed_navigation_categories() {
        let mut app = ConApp::new(None, None);
        let url = "https://platform.deepseek.com/dsh/authorized?state=SYNTHETIC-STATE-PRIVATE&code=SYNTHETIC-CODE-PRIVATE#SYNTHETIC-FRAGMENT-PRIVATE";
        for (reason, category) in [
            ("browser-navigation-error-0", "browser-navigation-unknown"),
            (
                "browser-navigation-error-2",
                "browser-navigation-certificate-expired",
            ),
            ("browser-navigation-error-7", "browser-navigation-timeout"),
            (
                "browser-navigation-error-8",
                "browser-navigation-invalid-server-response",
            ),
            (
                "browser-navigation-error-12",
                "browser-navigation-cannot-connect",
            ),
            (
                "browser-navigation-error-14",
                "browser-navigation-operation-canceled",
            ),
            (
                "browser-navigation-error-17",
                "browser-navigation-authentication-required",
            ),
            (
                "browser-navigation-error-18",
                "browser-navigation-proxy-authentication-required",
            ),
            ("browser-navigation-timeout", "browser-navigation-timeout"),
        ] {
            assert_eq!(
                authorization_failure_observation(&mut app, url, reason)["reason"],
                category,
            );
        }
    }

    #[test]
    fn authorization_failure_observation_redacts_unknown_codes_and_free_text() {
        let mut app = ConApp::new(None, None);
        let url = "https://platform.deepseek.com/dsh/authorized?state=SYNTHETIC-STATE-PRIVATE&code=SYNTHETIC-CODE-PRIVATE#SYNTHETIC-FRAGMENT-PRIVATE";
        for reason in [
            "browser-navigation-error-19",
            "browser-navigation-error-999999999",
            "browser-navigation-error--1",
            "browser-navigation-error-+12",
            "browser-navigation-error-012",
            "browser-navigation-error-12\n",
            "browser-navigation-error-12?code=SYNTHETIC-ERROR-PRIVATE",
            "browser-navigation-error-12 SYNTHETIC-ERROR-PRIVATE",
            "browser-navigation-error-12 https://platform.deepseek.com/dsh/authorized?code=SYNTHETIC-ERROR-PRIVATE",
            "browser-navigation-timeout SYNTHETIC-ERROR-PRIVATE",
            "SYNTHETIC-ERROR-PRIVATE https://ordinary.test/error",
            "browser-navigation-cannot-connect",
        ] {
            assert_eq!(
                authorization_failure_observation(&mut app, url, reason)["reason"],
                "authorization-page-unavailable",
            );
        }

        let ordinary = "https://ordinary.test/page?q=visible";
        assert!(ConApp::handoff_http_link(
            &mut app.workspace,
            &mut app.browsers,
            ordinary,
        ));
        app.browsers
            .get_mut(&app.workspace.browser_tab().unwrap())
            .unwrap()
            .note_load(browser::PageLoad::Unavailable {
                url: ordinary.into(),
                reason: "browser-navigation-error-12".into(),
            });
        let reply = app
            .windowless_command(&control::CliCommand::ListTabs)
            .unwrap()
            .unwrap();
        let observed: serde_json::Value = serde_json::from_slice(&json::to_vec(&reply)).unwrap();
        let browser = observed["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tab| tab["kind"] == "browser")
            .unwrap()["browser"]
            .clone();
        assert_eq!(browser["url"], ordinary);
        assert_eq!(browser["url_redacted"], false);
        assert_eq!(browser["reason"], "browser-navigation-error-12");
    }

    #[test]
    fn authorization_failure_observation_redacts_both_official_device_paths() {
        let mut app = ConApp::new(None, None);
        for url in [
            "https://opencode.ai/console/device?user_code=SYNTHETIC-USER-CODE-PRIVATE#SYNTHETIC-FRAGMENT-PRIVATE",
            "https://opencode.ai/console/auth/device?user_code=SYNTHETIC-USER-CODE-PRIVATE#SYNTHETIC-FRAGMENT-PRIVATE",
        ] {
            assert_eq!(
                authorization_failure_observation(&mut app, url, "browser-navigation-error-12")["reason"],
                "browser-navigation-cannot-connect",
            );
        }
    }

    #[test]
    fn authorization_failure_observation_redacts_github_sso_and_nested_return_destinations() {
        let mut app = ConApp::new(None, None);
        for base in [
            "https://github.com/login",
            "https://github.com/login/oauth/authorize",
            "https://github.com/login/device",
            "https://github.com/login/two-factor/app",
            "https://opencode.ai/console/auth/social/github/callback",
        ] {
            let url = format!(
                "{base}?return_to=%2Flogin%2Foauth%2Fauthorize%3Fredirect_uri%3Dhttps%253A%252F%252Fopencode.ai%252Fconsole%252Fauth%252Fsocial%252Fgithub%252Fcallback%253Fcode%253DSYNTHETIC-CODE-PRIVATE%26state%3DSYNTHETIC-STATE-PRIVATE#SYNTHETIC-FRAGMENT-PRIVATE"
            );
            let reason = format!("SYNTHETIC-ERROR-PRIVATE while loading {url}");
            assert_eq!(
                authorization_failure_observation(&mut app, &url, &reason)["reason"],
                "authorization-page-unavailable",
            );
        }
    }

    #[test]
    fn ordinary_github_urls_keep_their_public_query_title_and_reason() {
        let mut app = ConApp::new(None, None);
        for url in [
            "https://github.com/example/repository?tab=readme#section",
            "https://github.com/example/login?return_to=ordinary",
            "https://github.com/login-research?tab=readme",
        ] {
            assert!(ConApp::handoff_http_link(
                &mut app.workspace,
                &mut app.browsers,
                url,
            ));
            let id = app.workspace.browser_tab().unwrap();
            let tab = app.browsers.get_mut(&id).unwrap();
            tab.note_title("Public repository");
            tab.note_load(browser::PageLoad::Unavailable {
                url: url.into(),
                reason: "browser-navigation-error-12".into(),
            });
            let reply = app
                .windowless_command(&control::CliCommand::ListTabs)
                .unwrap()
                .unwrap();
            let observed: serde_json::Value =
                serde_json::from_slice(&json::to_vec(&reply)).unwrap();
            let browser = observed["tabs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tab| tab["kind"] == "browser")
                .unwrap()["browser"]
                .clone();
            assert_eq!(browser["url"], url);
            assert_eq!(browser["url_redacted"], false);
            assert_eq!(browser["title"], "Public repository");
            assert_eq!(browser["reason"], "browser-navigation-error-12");
        }
    }

    #[test]
    fn gui_title_control_observation_normalizes_without_a_render() {
        let mut app = ConApp::new(None, None);
        app.host_ui_dirty = DirtyRegion::empty();
        app.active_session_mut()
            .unwrap()
            .parser
            .process(b"\x1b]2;  observed-3  \x07");
        let reply = app
            .windowless_command(&control::CliCommand::ListTabs)
            .unwrap()
            .unwrap();
        let observed: serde_json::Value = serde_json::from_slice(&json::to_vec(&reply)).unwrap();
        assert_eq!(observed["tabs"][0]["title"], "observed-3");
        assert_eq!(app.active_session().unwrap().current_title, "observed-3");
        assert!(app.host_ui_dirty.is_full());
    }
}
