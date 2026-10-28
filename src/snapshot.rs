//! The `ui-snapshot` reply: one JSON document describing the live window.
//!
//! Everything a black-box journey asserts about host UI state lives here -
//! geometry, focus, rename and composer buffers, the paste ledger, the a11y
//! inbox - so tests never re-derive layout math. The fixed top-level key set
//! is pinned by the snapshot key-set guard in minicon_blackbox.
//!
//! Moved out of `control_dispatch.rs` verbatim; no behavior change.

use super::*;

impl ConApp {
    fn home_system_snapshot_json(&self) -> json::JsonValue {
        let (status, observed_at, cpu_time_ms, resident_bytes, reason) = match self
            .home_system_sample
        {
            Some(HomeSystemSample {
                observed_at,
                process: Ok(metrics),
            }) => (
                "measured",
                observed_unix_millis(Some(observed_at)),
                u64::try_from(metrics.cpu_time.as_millis())
                    .unwrap_or(u64::MAX)
                    .into(),
                metrics.resident_bytes.into(),
                json::JsonValue::Null,
            ),
            Some(HomeSystemSample {
                observed_at,
                process: Err(kind),
            }) => (
                "unknown",
                observed_unix_millis(Some(observed_at)),
                json::JsonValue::Null,
                json::JsonValue::Null,
                (if kind == minicon_platform::process_metrics::ProcessMetricsErrorKind::Unsupported
                {
                    "unsupported"
                } else {
                    "read-failed"
                })
                .into(),
            ),
            None => (
                "unknown",
                json::JsonValue::Null,
                json::JsonValue::Null,
                json::JsonValue::Null,
                "not-sampled".into(),
            ),
        };
        json::object(vec![
            ("scope", "minicon-host-process".into()),
            ("source", "os-process-metrics".into()),
            ("status", status.into()),
            ("observed_at_unix_ms", observed_at),
            ("cpu_time_ms", cpu_time_ms),
            ("resident_bytes", resident_bytes),
            ("reason", reason),
        ])
    }

    pub(super) fn ui_snapshot_json(&self, window: &PixelWindow) -> Result<json::JsonValue, String> {
        let a11y = self.a11y_inbox.stats();
        window
            .metrics()
            .map_err(|error| error.to_string())
            .map(|metrics| {
                let layout = self.layout(
                    metrics.physical_width,
                    metrics.physical_height,
                    metrics.scale_factor,
                );
                let rect_obj = |r: ui::Rect| {
                    json::object(vec![
                        ("x", r.x.into()),
                        ("y", r.y.into()),
                        ("width", r.width.into()),
                        ("height", r.height.into()),
                    ])
                };
                // Terminal viewport (the pixels the grid owns) and grid
                // shape come from the active session, which holds the
                // content insets the paint path uses. Exposing them makes
                // layout correctness — e.g. "the viewport bottom meets the
                // composer top with no overlap" — verifiable from the
                // control plane on any live window, including a headless
                // session with no GPU where a screenshot never rasterizes.
                let geometry = self
                    .active_session_opt()
                    .map(|session| {
                        let scrollbar = ui::terminal_scrollbar_width(metrics.scale_factor);
                        // Frame extent comes from the live window metrics, not
                        // the session's paint-time `frame_*` fields: those stay
                        // zero until the first present, so a headless or
                        // not-yet-painted window would otherwise report a
                        // zero-size viewport and defeat the very check this
                        // exists for. Content insets are set at layout time
                        // (not paint time), so pairing them with live metrics
                        // yields the true viewport in every state.
                        let frame_w = metrics.physical_width;
                        let frame_h = metrics.physical_height;
                        let viewport = ui::Rect {
                            x: session.content_left_px,
                            y: session.content_top_px,
                            width: frame_w
                                .saturating_sub(session.content_left_px)
                                .saturating_sub(scrollbar)
                                .saturating_sub(session.content_right_px),
                            height: frame_h
                                .saturating_sub(session.content_top_px)
                                .saturating_sub(session.content_bottom_px),
                        };
                        json::object(vec![
                            (
                                "pty_backend",
                                session
                                    .pty_backend()
                                    .map_or(json::JsonValue::Null, Into::into),
                            ),
                            // Integer permille (1500 == 1.5x) to keep float
                            // formatting out of the release binary, matching
                            // the delivery module's f64-removal discipline.
                            (
                                "scale_permille",
                                (minicon_core::numeric::round_f64(session.scale * 1000.0) as u64)
                                    .into(),
                            ),
                            (
                                "frame",
                                json::object(vec![
                                    ("width", frame_w.into()),
                                    ("height", frame_h.into()),
                                ]),
                            ),
                            ("terminal_viewport", rect_obj(viewport)),
                            ("composer_band", rect_obj(layout.composer)),
                            ("composer_scrollbar", rect_obj(layout.composer_scrollbar)),
                            ("composer_scrollbar_thumb", {
                                let rows = self.composer_rows(layout, metrics.scale_factor);
                                let lines = self.composer_visible_lines(rows);
                                let thumb = ui::composer_scrollbar_geometry(
                                    layout,
                                    rows,
                                    lines.first_line,
                                    composer::line_count(&self.composer.text),
                                )
                                .thumb;
                                json::object(vec![
                                    ("x", (thumb.left.max(0) as u32).into()),
                                    ("y", (thumb.top.max(0) as u32).into()),
                                    ("width", (thumb.width().max(0) as u32).into()),
                                    ("height", (thumb.height().max(0) as u32).into()),
                                ])
                            }),
                            (
                                "composer_visible_first_line",
                                self.composer_visible_lines(
                                    self.composer_rows(layout, metrics.scale_factor),
                                )
                                .first_line
                                .into(),
                            ),
                            ("status_band", rect_obj(layout.status)),
                            ("history_panel_band", rect_obj(layout.panel)),
                            // Where the notes scratchpad sits inside the
                            // open panel, physical pixels — the notes
                            // journey clicks this instead of re-deriving
                            // the panel's section math.
                            (
                                "notes_area",
                                if layout.panel_open {
                                    let p = layout.history_panel(metrics.scale_factor);
                                    rect_obj(p.notes.area)
                                } else {
                                    json::JsonValue::Null
                                },
                            ),
                            ("panel_toggle", rect_obj(layout.panel_toggle)),
                            ("sidebar", rect_obj(layout.sidebar)),
                            ("tree_header_height", layout.tree_header_height.into()),
                            ("tree_row_height", layout.tree_row_height.into()),
                            (
                                "tree_rename_editor",
                                self.rename_editor_geometry(window)
                                    .ok()
                                    .flatten()
                                    .map_or(json::JsonValue::Null, |(geometry, _)| {
                                        rect_obj(geometry.plate)
                                    }),
                            ),
                            (
                                "tree_edit_button",
                                // Where the active row's rename (pencil)
                                // button sits, in window physical pixels —
                                // or null with no active row or when that
                                // row is scrolled out of the tree. The
                                // rename journey clicks this instead of
                                // re-deriving the tree's button math, the
                                // same way terminal_viewport spares tests
                                // the inset arithmetic.
                                self.workspace
                                    .active()
                                    .map_or(json::JsonValue::Null, |active| {
                                        self.workspace
                                            .nodes()
                                            .iter()
                                            .position(|node| node.id == active)
                                            .and_then(|index| {
                                                index.checked_sub(self.tree_scroll_offset)
                                            })
                                            .filter(|visible| *visible < layout.tree_capacity())
                                            .map_or(json::JsonValue::Null, |visible| {
                                                rect_obj(
                                                    layout.tree_edit_rect(
                                                        visible,
                                                        metrics.scale_factor,
                                                    ),
                                                )
                                            })
                                    }),
                            ),
                            (
                                "grid",
                                json::object(vec![
                                    ("cols", session.cols.into()),
                                    ("rows", session.rows.into()),
                                    ("cell_w", session.cell_w.into()),
                                    ("cell_h", session.cell_h.into()),
                                ]),
                            ),
                        ])
                    })
                    .or_else(|| {
                        if !self.active_is_browser() {
                            return None;
                        }
                        let page = layout.browser_page(metrics.scale_factor)?;
                        Some(json::object(vec![
                            ("browser_page", rect_obj(page)),
                            (
                                "browser_toolbar",
                                rect_obj(layout.browser_toolbar(metrics.scale_factor)),
                            ),
                            (
                                "browser_address",
                                rect_obj(layout.browser_address(metrics.scale_factor)),
                            ),
                            ("address_focused", self.browser_address_focused.into()),
                            (
                                "browser_proxy",
                                rect_obj(layout.browser_proxy_text(metrics.scale_factor)),
                            ),
                            ("proxy_focused", self.browser_proxy_focused.into()),
                            ("sidebar", rect_obj(layout.sidebar)),
                            ("history_panel_band", rect_obj(layout.panel)),
                            (
                                "scale_permille",
                                (minicon_core::numeric::round_f64(metrics.scale_factor * 1000.0)
                                    as u64)
                                    .into(),
                            ),
                        ]))
                    })
                    .or_else(|| Some(json::object(vec![])))
                    .map(|mut geometry| {
                        if let json::JsonValue::Object(fields) = &mut geometry {
                            if !fields.iter().any(|(name, _)| *name == "frame") {
                                fields.push((
                                    "frame",
                                    json::object(vec![
                                        ("width", metrics.physical_width.into()),
                                        ("height", metrics.physical_height.into()),
                                    ]),
                                ));
                            }
                            if !fields.iter().any(|(name, _)| *name == "scale_permille") {
                                fields.push((
                                    "scale_permille",
                                    ((metrics.scale_factor * 1000.0) as u64).into(),
                                ));
                            }
                            let new_choices =
                                layout.new_tab_choices(metrics.scale_factor, self.new_menu_scroll);
                            let [home_new_terminal, home_new_web] = layout.home_actions(
                                metrics.physical_width,
                                metrics.physical_height,
                                metrics.scale_factor,
                            );
                            let home_capacity = layout.home_fleet_capacity(
                                metrics.physical_width,
                                metrics.physical_height,
                                metrics.scale_factor,
                            );
                            let home_nodes = self.home_visible_nodes();
                            let home_start = ui::clamp_tree_scroll(
                                self.home_list_start,
                                home_nodes.len(),
                                home_capacity,
                            );
                            let home_selected_hidden = self.home_selected_hidden_by_filter();
                            let home_selected_live = self
                                .home_selected_id
                                .is_some_and(|id| self.workspace.node(id).is_some());
                            fields.extend([
                                ("home", rect_obj(layout.home)),
                                ("home_active", self.home_active.into()),
                                ("home_mode", self.home_mode.key().into()),
                                (
                                    "home_mode_action",
                                    rect_obj(layout.home_mode_action(
                                        metrics.physical_width,
                                        metrics.physical_height,
                                        metrics.scale_factor,
                                    )),
                                ),
                                ("home_new_terminal", rect_obj(home_new_terminal)),
                                ("home_new_web", rect_obj(home_new_web)),
                                (
                                    "home_fleet",
                                    rect_obj(layout.home_fleet(
                                        metrics.physical_width,
                                        metrics.physical_height,
                                        metrics.scale_factor,
                                    )),
                                ),
                                ("home_list_start", home_start.into()),
                                ("home_tab_count", self.workspace.nodes().len().into()),
                                ("home_system", self.home_system_snapshot_json()),
                                ("home_gateway", {
                                    let gateway = self.home_gateway_observation();
                                    json::object(vec![
                                        ("source", "harness-runtime-view".into()),
                                        ("status", gateway.status.into()),
                                        (
                                            "observed_at_unix_ms",
                                            observed_unix_millis(gateway.observed_at),
                                        ),
                                        (
                                            "port",
                                            gateway.port.map_or(json::JsonValue::Null, |port| {
                                                u64::from(port).into()
                                            }),
                                        ),
                                        (
                                            "reason",
                                            gateway
                                                .reason
                                                .map_or(json::JsonValue::Null, Into::into),
                                        ),
                                        ("action", "open-harness".into()),
                                    ])
                                }),
                                (
                                    "home_gateway_action",
                                    rect_obj(layout.home_gateway_action(
                                        metrics.physical_width,
                                        metrics.physical_height,
                                        metrics.scale_factor,
                                    )),
                                ),
                                (
                                    "home_system_refresh",
                                    rect_obj(layout.home_system_card(
                                        metrics.physical_width,
                                        metrics.physical_height,
                                        metrics.scale_factor,
                                    )),
                                ),
                                ("home_visible_count", home_nodes.len().into()),
                                ("home_kind_filter", self.home_kind_filter.key().into()),
                                ("home_sort", self.home_sort.key().into()),
                                (
                                    "home_search",
                                    rect_obj(layout.home_search(
                                        metrics.physical_width,
                                        metrics.physical_height,
                                        metrics.scale_factor,
                                    )),
                                ),
                                ("home_search_focused", self.home_search.focused.into()),
                                (
                                    "home_search_query_bytes",
                                    self.home_search.text.len().into(),
                                ),
                                (
                                    "home_filter_action",
                                    rect_obj(layout.home_filter_action(
                                        metrics.physical_width,
                                        metrics.physical_height,
                                        metrics.scale_factor,
                                    )),
                                ),
                                (
                                    "home_sort_action",
                                    rect_obj(layout.home_sort_action(
                                        metrics.physical_width,
                                        metrics.physical_height,
                                        metrics.scale_factor,
                                    )),
                                ),
                                (
                                    "home_list_scrollbar",
                                    rect_obj(layout.home_list_scrollbar(
                                        metrics.physical_width,
                                        metrics.physical_height,
                                        metrics.scale_factor,
                                    )),
                                ),
                                (
                                    "home_list_scrollbar_visible",
                                    (home_nodes.len() > home_capacity && home_capacity > 0).into(),
                                ),
                                ("home_list_scrollbar_thumb", {
                                    let thumb = ui::home_list_scrollbar_geometry(
                                        layout,
                                        metrics.physical_width,
                                        metrics.physical_height,
                                        metrics.scale_factor,
                                        home_start,
                                        home_nodes.len(),
                                    )
                                    .thumb;
                                    json::object(vec![
                                        ("x", (thumb.left.max(0) as u32).into()),
                                        ("y", (thumb.top.max(0) as u32).into()),
                                        ("width", (thumb.width().max(0) as u32).into()),
                                        ("height", (thumb.height().max(0) as u32).into()),
                                    ])
                                }),
                                ("home_selected_id", tab_id_json(self.home_selected_id)),
                                (
                                    "home_selected_hidden_by_filter",
                                    home_selected_hidden.into(),
                                ),
                                (
                                    "home_attention_action",
                                    rect_obj(layout.home_attention_action(
                                        metrics.physical_width,
                                        metrics.physical_height,
                                        metrics.scale_factor,
                                    )),
                                ),
                                (
                                    "home_attention_coverage",
                                    "terminal-child-exit+local-api-listener-error".into(),
                                ),
                                (
                                    "home_attention_status",
                                    (if self.home_attention().is_empty() {
                                        "none-observed"
                                    } else {
                                        "observed"
                                    })
                                    .into(),
                                ),
                                (
                                    "home_attention_items",
                                    json::JsonValue::Array(
                                        self.home_attention()
                                            .into_iter()
                                            .map(|item| match item {
                                                HomeAttention::ListenerError { observed_at } => {
                                                    json::object(vec![
                                                        ("key", "local-api-listener".into()),
                                                        ("source", "harness-runtime-view".into()),
                                                        ("severity", "error".into()),
                                                        ("tab_id", json::JsonValue::Null),
                                                        ("component", "local-api".into()),
                                                        (
                                                            "observed_at_unix_ms",
                                                            observed_unix_millis(Some(observed_at)),
                                                        ),
                                                        ("action", "open-harness".into()),
                                                    ])
                                                }
                                                HomeAttention::ChildExit(item) => {
                                                    json::object(vec![
                                                        (
                                                            "key",
                                                            format!(
                                                                "terminal-child-exit:@{}",
                                                                item.tab.get()
                                                            )
                                                            .into(),
                                                        ),
                                                        ("source", "terminal-child".into()),
                                                        ("severity", "info".into()),
                                                        ("tab_id", tab_id_json(Some(item.tab))),
                                                        (
                                                            "child_exit_code",
                                                            exit_code_json(item.exit_code),
                                                        ),
                                                        (
                                                            "observed_at_unix_ms",
                                                            observed_unix_millis(item.observed_at),
                                                        ),
                                                        ("action", "inspect-tab".into()),
                                                    ])
                                                }
                                            })
                                            .collect(),
                                    ),
                                ),
                                (
                                    "home_selected_stale",
                                    self.home_selected_id
                                        .is_some_and(|id| self.workspace.node(id).is_none())
                                        .into(),
                                ),
                                (
                                    "home_inspector",
                                    rect_obj(layout.home_inspector(
                                        metrics.physical_width,
                                        metrics.physical_height,
                                        metrics.scale_factor,
                                    )),
                                ),
                                (
                                    "home_focus_tab",
                                    rect_obj(layout.home_focus_action(
                                        metrics.physical_width,
                                        metrics.physical_height,
                                        metrics.scale_factor,
                                    )),
                                ),
                                (
                                    "home_focus_enabled",
                                    (home_selected_live && !home_selected_hidden).into(),
                                ),
                                (
                                    "home_reveal_tab",
                                    if home_selected_hidden {
                                        rect_obj(layout.home_focus_action(
                                            metrics.physical_width,
                                            metrics.physical_height,
                                            metrics.scale_factor,
                                        ))
                                    } else {
                                        json::JsonValue::Null
                                    },
                                ),
                                (
                                    "home_inspector_action",
                                    if home_selected_hidden {
                                        "reveal"
                                    } else if home_selected_live {
                                        "focus"
                                    } else {
                                        "disabled"
                                    }
                                    .into(),
                                ),
                                (
                                    "home_list_row_ids",
                                    json::JsonValue::Array(
                                        home_nodes
                                            .iter()
                                            .skip(home_start)
                                            .take(home_capacity)
                                            .map(|node| format!("@{}", node.id.get()).into())
                                            .collect(),
                                    ),
                                ),
                                (
                                    "home_list_row_rects",
                                    json::JsonValue::Array(
                                        (0..home_capacity
                                            .min(home_nodes.len().saturating_sub(home_start)))
                                            .map(|visible| {
                                                rect_obj(if self.home_mode == HomeMode::Visual {
                                                    layout.home_visual_station(
                                                        metrics.physical_width,
                                                        metrics.physical_height,
                                                        metrics.scale_factor,
                                                        visible,
                                                    )
                                                } else {
                                                    layout.home_fleet_row(
                                                        metrics.physical_width,
                                                        metrics.physical_height,
                                                        metrics.scale_factor,
                                                        visible,
                                                    )
                                                })
                                            })
                                            .collect(),
                                    ),
                                ),
                                ("new_root", rect_obj(layout.new_root)),
                                ("new_web", rect_obj(layout.new_web)),
                                ("sidebar_toggle", rect_obj(layout.sidebar_toggle)),
                                ("harness_entry", rect_obj(layout.harness)),
                                (
                                    "host_scale_permille",
                                    ((metrics.scale_factor * 1000.0) as u64).into(),
                                ),
                                (
                                    "harness_workspace",
                                    self.harness_workspace_snapshot(
                                        layout,
                                        metrics.physical_width,
                                        metrics.physical_height,
                                        metrics.scale_factor,
                                    ),
                                ),
                                ("new_menu_highlight", self.new_menu_highlight.into()),
                                ("new_menu_scroll", new_choices.start.into()),
                                (
                                    "new_menu_option_indices",
                                    json::JsonValue::Array(
                                        (new_choices.start
                                            ..new_choices.start + new_choices.visible_rows)
                                            .map(Into::into)
                                            .collect(),
                                    ),
                                ),
                                (
                                    "new_menu_option_rows",
                                    json::JsonValue::Array(
                                        (new_choices.start
                                            ..new_choices.start + new_choices.visible_rows)
                                            .map(|index| rect_obj(new_choices.row(index)))
                                            .collect(),
                                    ),
                                ),
                                (
                                    "menu_return_focus",
                                    match self.menu_return_focus {
                                        Some(super::browser_menu::MenuFocus::Address) => "address",
                                        Some(super::browser_menu::MenuFocus::Proxy) => "proxy",
                                        Some(super::browser_menu::MenuFocus::Candidate) => {
                                            "candidate"
                                        }
                                        Some(super::browser_menu::MenuFocus::Settings) => {
                                            "settings"
                                        }
                                        Some(super::browser_menu::MenuFocus::NewTrigger) => {
                                            "new_trigger"
                                        }
                                        Some(super::browser_menu::MenuFocus::ProxyTrigger) => {
                                            "proxy_trigger"
                                        }
                                        Some(super::browser_menu::MenuFocus::Page) => "page",
                                        Some(super::browser_menu::MenuFocus::Composer) => {
                                            "composer"
                                        }
                                        Some(super::browser_menu::MenuFocus::Notes) => "notes",
                                        Some(super::browser_menu::MenuFocus::Terminal) => {
                                            "terminal"
                                        }
                                        None => "none",
                                    }
                                    .into(),
                                ),
                                (
                                    "browser_input_owner",
                                    if self.new_tab_menu_open {
                                        "new_menu"
                                    } else if self.browser_proxy_menu_open {
                                        "proxy_menu"
                                    } else if self.settings_open
                                        && self.settings_section == ui::SettingsSection::Harness
                                    {
                                        self.harness_input_owner()
                                    } else if self.settings_open && self.settings_proxy_focused {
                                        "candidate"
                                    } else if self.settings_open {
                                        "settings"
                                    } else if !self.active_is_browser() {
                                        if self.notes_focused {
                                            "notes"
                                        } else if self.composer.focused {
                                            "composer"
                                        } else {
                                            "terminal"
                                        }
                                    } else if self.browser_address_focused {
                                        "address"
                                    } else if self.browser_proxy_focused {
                                        "proxy"
                                    } else if self.browser_chrome_trigger
                                        == Some(super::browser_menu::ChromeTrigger::New)
                                    {
                                        "new_trigger"
                                    } else if self.browser_chrome_trigger
                                        == Some(super::browser_menu::ChromeTrigger::Proxy)
                                    {
                                        "proxy_trigger"
                                    } else {
                                        "page"
                                    }
                                    .into(),
                                ),
                                (
                                    "browser_chrome_trigger",
                                    match self.browser_chrome_trigger {
                                        Some(super::browser_menu::ChromeTrigger::New) => "new",
                                        Some(super::browser_menu::ChromeTrigger::Proxy) => "proxy",
                                        None => "none",
                                    }
                                    .into(),
                                ),
                                ("new_tab_menu_open", self.new_tab_menu_open.into()),
                                ("new_tab_menu", rect_obj(new_choices.rect)),
                                ("new_terminal_action", rect_obj(new_choices.row(0))),
                                ("new_browser_action", rect_obj(new_choices.row(1))),
                            ]);
                            let scale = metrics.scale_factor;
                            let editor_state =
                                |editor: Option<&composer::ComposerState>, focused: bool| {
                                    let empty = composer::ComposerState::default();
                                    let editor = editor.unwrap_or(&empty);
                                    let redacted = self.loaded_browser_auth().is_some()
                                        || super::browser_ui::is_authorization_page(&editor.text);
                                    let text = if self.loaded_browser_auth().is_some() {
                                        "authorization://private"
                                    } else {
                                        super::browser_ui::public_browser_url(&editor.text)
                                    };
                                    let selection = composer::selection_bounds(editor)
                                        .filter(|_| !redacted)
                                        .map_or(json::JsonValue::Null, |(start, end)| {
                                            json::JsonValue::Array(vec![start.into(), end.into()])
                                        });
                                    json::object(vec![
                                        ("text", text.into()),
                                        ("caret", editor.caret.min(text.len()).into()),
                                        ("selection", selection),
                                        (
                                            "preedit",
                                            if redacted {
                                                ""
                                            } else {
                                                editor.preedit.as_str()
                                            }
                                            .into(),
                                        ),
                                        ("focused", focused.into()),
                                        ("redacted", redacted.into()),
                                    ])
                                };
                            let menu_owns_focus =
                                self.browser_proxy_menu_open || self.new_tab_menu_open;
                            fields.extend([
                                (
                                    "browser_address_editor",
                                    editor_state(
                                        self.browser_address.as_ref(),
                                        self.browser_address_focused
                                            && !menu_owns_focus
                                            && !self.settings_open,
                                    ),
                                ),
                                (
                                    "browser_proxy_editor",
                                    editor_state(
                                        Some(&self.browser_proxy_input),
                                        self.browser_proxy_focused
                                            && !menu_owns_focus
                                            && !self.settings_open,
                                    ),
                                ),
                                (
                                    "web_proxy_candidate_editor",
                                    editor_state(
                                        Some(&self.settings_proxy_input),
                                        self.settings_proxy_focused && self.settings_open,
                                    ),
                                ),
                            ]);
                            let web = layout.web_settings(scale, self.settings_proxy_list_start);
                            let routes = match self.settings_section {
                                ui::SettingsSection::Web => web.routes,
                                ui::SettingsSection::Harness => {
                                    layout.harness_settings(scale, 0, 0).routes
                                }
                                ui::SettingsSection::Console => [
                                    layout.settings_route(ui::SettingsSection::Console, scale),
                                    layout.settings_route(ui::SettingsSection::Web, scale),
                                    layout.settings_route(ui::SettingsSection::Harness, scale),
                                ],
                            };
                            let rows = web.visible_rows.min(
                                self.config
                                    .browser_proxy_options
                                    .len()
                                    .saturating_sub(web.list_start),
                            );
                            fields.extend([
                                ("settings", rect_obj(layout.settings)),
                                ("settings_open", self.settings_open.into()),
                                (
                                    "settings_section",
                                    match self.settings_section {
                                        ui::SettingsSection::Console => "console",
                                        ui::SettingsSection::Web => "web",
                                        ui::SettingsSection::Harness => "harness",
                                    }
                                    .into(),
                                ),
                                ("settings_console_route", rect_obj(routes[0])),
                                ("settings_web_route", rect_obj(routes[1])),
                                ("settings_harness_route", rect_obj(routes[2])),
                                (
                                    "harness_settings",
                                    self.harness_settings_snapshot(layout, scale),
                                ),
                                ("web_proxy_input", rect_obj(web.proxy_input)),
                                ("web_proxy_add", rect_obj(web.proxy_add)),
                                ("web_proxy_previous", rect_obj(web.previous)),
                                ("web_proxy_next", rect_obj(web.next)),
                                (
                                    "web_proxy_rows",
                                    json::JsonValue::Array(
                                        web.proxy_rows
                                            .iter()
                                            .take(rows)
                                            .copied()
                                            .map(rect_obj)
                                            .collect(),
                                    ),
                                ),
                                (
                                    "web_proxy_remove",
                                    json::JsonValue::Array(
                                        web.proxy_remove
                                            .iter()
                                            .take(rows)
                                            .copied()
                                            .map(rect_obj)
                                            .collect(),
                                    ),
                                ),
                                (
                                    "web_proxy_options_count",
                                    self.config.browser_proxy_options.len().into(),
                                ),
                                ("web_proxy_list_start", web.list_start.into()),
                                (
                                    "web_proxy_candidate_focused",
                                    self.settings_proxy_focused.into(),
                                ),
                            ]);
                            if self.active_is_browser() {
                                let proxy_choices = layout.browser_proxy_menu(
                                    scale,
                                    self.config.browser_proxy_options.len() + 1,
                                    self.proxy_menu_scroll,
                                );
                                fields.extend([
                                    ("proxy_menu_highlight", self.proxy_menu_highlight.into()),
                                    (
                                        "proxy_menu_applied_index",
                                        if self.browser_proxy_applied.is_empty() {
                                            0usize.into()
                                        } else {
                                            self.config
                                                .browser_proxy_options
                                                .iter()
                                                .position(|value| {
                                                    value == &self.browser_proxy_applied
                                                })
                                                .map_or(json::JsonValue::Null, |index| {
                                                    (index + 1).into()
                                                })
                                        },
                                    ),
                                    ("proxy_menu_scroll", proxy_choices.start.into()),
                                    ("browser_proxy_menu", rect_obj(proxy_choices.rect)),
                                    (
                                        "browser_proxy_option_indices",
                                        json::JsonValue::Array(
                                            (proxy_choices.start
                                                ..proxy_choices.start + proxy_choices.visible_rows)
                                                .map(Into::into)
                                                .collect(),
                                        ),
                                    ),
                                    (
                                        "browser_proxy_dropdown",
                                        rect_obj(layout.browser_proxy_dropdown(scale)),
                                    ),
                                    (
                                        "browser_proxy_menu_open",
                                        self.browser_proxy_menu_open.into(),
                                    ),
                                    (
                                        "browser_proxy_option_rows",
                                        json::JsonValue::Array(
                                            (proxy_choices.start
                                                ..proxy_choices.start + proxy_choices.visible_rows)
                                                .map(|index| rect_obj(proxy_choices.row(index)))
                                                .collect(),
                                        ),
                                    ),
                                    (
                                        "browser_retry",
                                        rect_obj(layout.browser_retry(metrics.scale_factor)),
                                    ),
                                    (
                                        "browser_edit_address",
                                        rect_obj(layout.browser_edit_address(metrics.scale_factor)),
                                    ),
                                ]);
                            }
                        }
                        geometry
                    });
                json::object(vec![
                    ("active", tab_id_json(self.workspace.active())),
                    ("workspace_empty", self.workspace.active().is_none().into()),
                    ("settings_open", self.settings_open.into()),
                    (
                        "history_panel",
                        json::object(vec![
                            ("open", self.history_panel_open.into()),
                            ("entries", self.composer.history().len().into()),
                            // Rows back from the newest entry, so the
                            // one gesture a list owes the reader —
                            // reaching past its own height — is
                            // observable from the control plane.
                            (
                                "scroll",
                                self.history_visible_offset(
                                    layout.history_panel(metrics.scale_factor).row_capacity,
                                )
                                .into(),
                            ),
                        ]),
                    ),
                    ("update", {
                        let published = crate::update::snapshot(&self.update);
                        json::object(vec![
                            ("phase", published.phase.into()),
                            ("current", self.update.current.to_string().into()),
                            (
                                "remote",
                                published.remote.map_or(json::JsonValue::Null, Into::into),
                            ),
                            // The untranslated cause, for a bug report; and the
                            // stable kind, so automation never has to match a
                            // translated sentence.
                            (
                                "error",
                                published.cause.map_or(json::JsonValue::Null, Into::into),
                            ),
                            (
                                "failure",
                                published.failure.map_or(json::JsonValue::Null, Into::into),
                            ),
                            // The line the panel actually paints, in the
                            // language the panel is set to. Publishing the
                            // rendered string is what makes "is the failure
                            // localised" observable at all.
                            (
                                "status",
                                self.ui_language.update_status(&self.update).into(),
                            ),
                            ("handoff", published.handoff.into()),
                            // What the Restart press costs, rendered in
                            // the panel's language. Published even when
                            // no update is staged so the sentence can be
                            // asserted on its own, from any number of
                            // open tabs, without staging a release.
                            (
                                "restart_cost",
                                self.ui_language
                                    .restart_cost(self.workspace.nodes().len())
                                    .into(),
                            ),
                        ])
                    }),
                    ("geometry", geometry.unwrap_or(json::JsonValue::Null)),
                    (
                        "host_notice",
                        self.host_notice
                            .as_deref()
                            .map_or(json::JsonValue::Null, |text| {
                                super::browser_ui::public_browser_text(text)
                                    .into_owned()
                                    .into()
                            }),
                    ),
                    (
                        "control_pointer_owner",
                        tab_id_json(self.control_pointer_owner),
                    ),
                    (
                        "terminal_clipboard_paste",
                        json::object(vec![
                            (
                                // An open review is the same public
                                // state as a clipboard read still in
                                // flight: a paste is owned and not yet
                                // delivered. Splitting it would add a
                                // value to a contract the alignment
                                // gate pins, for no observer benefit.
                                "state",
                                if self.pending_clipboard_paste.is_some()
                                    || self.pending_paste_review.is_some()
                                {
                                    "pending"
                                } else {
                                    "idle"
                                }
                                .into(),
                            ),
                            (
                                "target",
                                tab_id_json(
                                    self.pending_clipboard_paste
                                        .as_ref()
                                        .map(|pending| pending.target)
                                        .or_else(|| {
                                            self.pending_paste_review
                                                .as_ref()
                                                .map(|pending| pending.target)
                                        }),
                                ),
                            ),
                            (
                                "error",
                                self.terminal_clipboard_error
                                    .as_deref()
                                    .map_or(json::JsonValue::Null, Into::into),
                            ),
                        ]),
                    ),
                    ("ui_language", self.ui_language.tag().into()),
                    ("ui_theme", self.ui_theme.tag().into()),
                    (
                        "rename",
                        self.rename
                            .as_ref()
                            .map_or(json::JsonValue::Null, |rename| {
                                json::object(vec![
                                    ("tab", tab_id_json(Some(rename.target))),
                                    ("text", rename.editor.text.as_str().into()),
                                    ("preedit", rename.editor.preedit.as_str().into()),
                                    ("caret", (rename.editor.caret as u64).into()),
                                    (
                                        "selection",
                                        composer::selection_bounds(&rename.editor).map_or(
                                            json::JsonValue::Null,
                                            |(start, end)| {
                                                json::object(vec![
                                                    ("start", (start as u64).into()),
                                                    ("end", (end as u64).into()),
                                                ])
                                            },
                                        ),
                                    ),
                                    ("selecting", self.rename_selecting.into()),
                                ])
                            }),
                    ),
                    (
                        "tree_drag",
                        self.tree_drag.map_or(json::JsonValue::Null, |drag| {
                            let (target, position) = match drag.proposal {
                                Some(workspace::MovePlacement::Before(target)) => {
                                    (Some(target), "before")
                                }
                                Some(workspace::MovePlacement::Into(target)) => {
                                    (Some(target), "into")
                                }
                                Some(workspace::MovePlacement::After(target)) => {
                                    (Some(target), "after")
                                }
                                Some(workspace::MovePlacement::RootTail) => (None, "root_tail"),
                                None => (None, "none"),
                            };
                            let row = target
                                .and_then(|id| {
                                    self.workspace.nodes().iter().position(|node| node.id == id)
                                })
                                .and_then(|index| index.checked_sub(self.tree_scroll_offset))
                                .filter(|visible| *visible < layout.tree_capacity())
                                .map(|visible| ui::Rect {
                                    x: layout.sidebar.x,
                                    y: layout.tree_header_height.saturating_add(
                                        layout.tree_row_height.saturating_mul(visible as u32),
                                    ),
                                    width: layout.sidebar.width,
                                    height: layout.tree_row_height,
                                });
                            json::object(vec![
                                ("source", tab_id_json(Some(drag.source))),
                                ("revision", drag.revision.into()),
                                (
                                    "phase",
                                    if drag.dragging { "dragging" } else { "armed" }.into(),
                                ),
                                ("target", tab_id_json(target)),
                                ("position", position.into()),
                                (
                                    "valid",
                                    (drag.proposal.is_some() && drag.refusal.is_none()).into(),
                                ),
                                (
                                    "reason",
                                    drag.refusal.map_or(json::JsonValue::Null, |error| {
                                        format!("{error:?}").into()
                                    }),
                                ),
                                ("target_row", row.map_or(json::JsonValue::Null, rect_obj)),
                            ])
                        }),
                    ),
                    ("composer_focused", self.composer.focused.into()),
                    ("composer_text", self.composer.text.as_str().into()),
                    ("composer_preedit", self.composer.preedit.as_str().into()),
                    ("notes_focused", self.notes_focused.into()),
                    ("notes_text", self.notes.text.as_str().into()),
                    (
                        "terminal_ime_preedit",
                        self.active_session_opt()
                            .map_or("", |session| session.ime_preedit.as_str())
                            .into(),
                    ),
                    ("ime_status", ime_status_json(self.ime_status.as_ref())),
                    (
                        "composer_submit_error",
                        self.composer
                            .submit_error
                            .as_deref()
                            .map_or(json::JsonValue::Null, Into::into),
                    ),
                    (
                        "pending_control_waits",
                        self.pending_control.wait_count().into(),
                    ),
                    (
                        "pending_control_screenshots",
                        self.pending_control.screenshot_count().into(),
                    ),
                    ("a11y_pending_actions", a11y.pending.into()),
                    ("a11y_pending_bytes", a11y.pending_bytes.into()),
                    ("a11y_dropped_actions", a11y.dropped.into()),
                    (
                        "composer_input",
                        json::object(vec![
                            ("x", layout.composer_input.x.into()),
                            ("y", layout.composer_input.y.into()),
                            ("width", layout.composer_input.width.into()),
                            ("height", layout.composer_input.height.into()),
                        ]),
                    ),
                ])
            })
    }
}

#[cfg(test)]
mod home_system_snapshot_tests {
    use super::*;

    #[test]
    fn failed_process_metric_read_is_unknown_with_no_synthetic_zero() {
        let mut app = ConApp::new(None, None);
        app.home_system_sample = Some(HomeSystemSample {
            observed_at: UNIX_EPOCH + Duration::from_secs(1),
            process: Err(minicon_platform::process_metrics::ProcessMetricsErrorKind::Read),
        });
        let json::JsonValue::Object(fields) = app.home_system_snapshot_json() else {
            panic!("Home system projection must be an object");
        };
        let field = |name| {
            fields
                .iter()
                .find(|(key, _)| *key == name)
                .unwrap()
                .1
                .clone()
        };
        assert_eq!(field("status"), "unknown".into());
        assert_eq!(field("reason"), "read-failed".into());
        assert_eq!(field("observed_at_unix_ms"), 1000_u64.into());
        assert_eq!(field("cpu_time_ms"), json::JsonValue::Null);
        assert_eq!(field("resident_bytes"), json::JsonValue::Null);
    }
}
