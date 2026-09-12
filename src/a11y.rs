//! Accessibility snapshot for the standalone console host UI.
//!
//! The host is a custom-raster (winit/softbuffer) toolkit, so GTK atk-bridge
//! never sees it. This module names the painted host UI as a small widget tree
//! and lets the Linux AT-SPI publisher register those children.

use std::collections::VecDeque;
use std::sync::Mutex;

use agenterm_platform::accessibility_publish::{
    AccessibilityBounds, NODE_APPLICATION, NODE_COMMAND, NODE_FRAME, NODE_OFFSCREEN_FIELD,
    NODE_SEND, NODE_SESSION, NODE_TABS, PublishedAction, PublishedNode, PublishedRole,
    PublishedTree,
};

use crate::ui::{Layout, Rect};

pub const COMMAND_NAME: &str = "Command";
pub const SEND_NAME: &str = "SEND";
pub const NEWLINE_NAME: &str = "NEWLINE";

/// Published id for the newline button.
///
/// Defined here rather than in `agenterm-platform` because that crate is
/// pinned by revision and its ids stop at 6; 7 is the next free value and the
/// adapter only needs the number to be stable, which a constant in one place
/// makes it. Move it upstream when the pin next moves.
pub const NODE_NEWLINE: u32 = 7;
pub const TABS_NAME: &str = "Tabs";
pub const SESSION_NAME: &str = "Session";
pub const OFFSCREEN_FIELD_NAME: &str = "OffscreenField";
const OFFSCREEN_FIELD_GAP: u32 = 2000;
const OFFSCREEN_FIELD_HEIGHT: u32 = 24;
const ACTION_QUEUE_CAPACITY: usize = 64;
const ACTION_QUEUE_MAX_BYTES: usize = 256 * 1024;
pub const ACTION_DRAIN_BUDGET: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    pub node: u32,
    pub action: PublishedAction,
}

impl Request {
    fn payload_bytes(&self) -> usize {
        match &self.action {
            PublishedAction::SetText(text) => text.len(),
            PublishedAction::Key(key) => key.event_string.len(),
            PublishedAction::Click | PublishedAction::Focus => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ActionInboxStats {
    pub pending: usize,
    pub pending_bytes: usize,
    pub dropped: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ActionPush {
    pub accepted: bool,
    pub should_wake: bool,
}

#[derive(Default)]
struct ActionQueue {
    requests: VecDeque<Request>,
    pending_bytes: usize,
    dropped: u64,
}

#[derive(Default)]
pub struct ActionInbox {
    queue: Mutex<ActionQueue>,
}

impl ActionInbox {
    pub fn push(&self, request: Request) -> ActionPush {
        let payload_bytes = request.payload_bytes();
        let mut queue = self.queue.lock().unwrap_or_else(|error| error.into_inner());
        let next_bytes = queue.pending_bytes.checked_add(payload_bytes);
        if queue.requests.len() >= ACTION_QUEUE_CAPACITY
            || payload_bytes > crate::composer::PASTE_LIMIT_BYTES
            || next_bytes.is_none_or(|bytes| bytes > ACTION_QUEUE_MAX_BYTES)
        {
            queue.dropped = queue.dropped.saturating_add(1);
            return ActionPush::default();
        }
        let should_wake = queue.requests.is_empty();
        queue.pending_bytes = next_bytes.unwrap_or(queue.pending_bytes);
        queue.requests.push_back(request);
        ActionPush {
            accepted: true,
            should_wake,
        }
    }

    pub fn pop_batch(&self, limit: usize) -> (Vec<Request>, bool) {
        let mut queue = self.queue.lock().unwrap_or_else(|error| error.into_inner());
        let count = limit.min(queue.requests.len());
        let batch: Vec<_> = queue.requests.drain(..count).collect();
        let drained_bytes = batch
            .iter()
            .map(Request::payload_bytes)
            .fold(0usize, usize::saturating_add);
        queue.pending_bytes = queue.pending_bytes.saturating_sub(drained_bytes);
        (batch, !queue.requests.is_empty())
    }

    pub fn stats(&self) -> ActionInboxStats {
        let queue = self.queue.lock().unwrap_or_else(|error| error.into_inner());
        ActionInboxStats {
            pending: queue.requests.len(),
            pending_bytes: queue.pending_bytes,
            dropped: queue.dropped,
        }
    }
}

pub fn tree(
    app_name: &str,
    frame_title: &str,
    layout: Layout,
    frame_width: u32,
    frame_height: u32,
    composer_focused: bool,
    command_text: &str,
) -> PublishedTree {
    let frame = Rect {
        x: 0,
        y: 0,
        width: frame_width,
        height: frame_height,
    };
    let session = session_rect(layout, frame_width, frame_height);
    PublishedTree {
        app_name: app_name.to_owned(),
        nodes: vec![
            published(
                NODE_APPLICATION,
                None,
                PublishedRole::Application,
                app_name,
                "",
                frame,
                Flags::CONTAINER,
            ),
            published(
                NODE_FRAME,
                Some(NODE_APPLICATION),
                PublishedRole::Frame,
                frame_title,
                "",
                frame,
                Flags::FOCUSABLE.with_click(),
            ),
            published(
                NODE_TABS,
                Some(NODE_FRAME),
                PublishedRole::Panel,
                TABS_NAME,
                "",
                layout.sidebar,
                Flags::FOCUSABLE,
            ),
            published(
                NODE_SESSION,
                Some(NODE_FRAME),
                PublishedRole::Terminal,
                SESSION_NAME,
                "",
                session,
                Flags {
                    focusable: true,
                    focused: !composer_focused,
                    editable: false,
                    clickable: false,
                },
            ),
            published(
                NODE_OFFSCREEN_FIELD,
                Some(NODE_SESSION),
                PublishedRole::Label,
                OFFSCREEN_FIELD_NAME,
                "",
                offscreen_field_rect(session),
                Flags::CONTAINER,
            ),
            published(
                NODE_COMMAND,
                Some(NODE_FRAME),
                PublishedRole::Text,
                COMMAND_NAME,
                command_text,
                layout.composer_input,
                Flags {
                    focusable: true,
                    focused: composer_focused,
                    editable: true,
                    clickable: true,
                },
            ),
            published(
                NODE_SEND,
                Some(NODE_FRAME),
                PublishedRole::Button,
                SEND_NAME,
                "",
                layout.composer_send,
                Flags::FOCUSABLE.with_click(),
            ),
            published(
                NODE_NEWLINE,
                Some(NODE_FRAME),
                PublishedRole::Button,
                NEWLINE_NAME,
                "",
                layout.composer_newline,
                Flags::FOCUSABLE.with_click(),
            ),
        ],
    }
}

#[derive(Clone, Copy)]
struct Flags {
    focusable: bool,
    focused: bool,
    editable: bool,
    clickable: bool,
}

impl Flags {
    const CONTAINER: Self = Self {
        focusable: false,
        focused: false,
        editable: false,
        clickable: false,
    };
    const FOCUSABLE: Self = Self {
        focusable: true,
        focused: false,
        editable: false,
        clickable: false,
    };

    const fn with_click(self) -> Self {
        Self {
            clickable: true,
            ..self
        }
    }
}

fn session_rect(layout: Layout, frame_width: u32, frame_height: u32) -> Rect {
    let left = layout.sidebar.width;
    let bottom = layout.composer.y;
    Rect {
        x: left,
        y: 0,
        width: frame_width.saturating_sub(left),
        height: bottom.min(frame_height),
    }
}

fn offscreen_field_rect(session: Rect) -> Rect {
    Rect {
        x: session.x,
        y: session
            .y
            .saturating_add(session.height)
            .saturating_add(OFFSCREEN_FIELD_GAP),
        width: session.width.max(1),
        height: OFFSCREEN_FIELD_HEIGHT,
    }
}

fn published(
    id: u32,
    parent: Option<u32>,
    role: PublishedRole,
    name: &str,
    text: &str,
    rect: Rect,
    flags: Flags,
) -> PublishedNode {
    PublishedNode {
        id,
        parent,
        role,
        name: name.to_owned(),
        text: text.to_owned(),
        bounds: AccessibilityBounds {
            x: saturating_i32(rect.x),
            y: saturating_i32(rect.y),
            width: saturating_i32(rect.width),
            height: saturating_i32(rect.height),
        },
        focusable: flags.focusable,
        focused: flags.focused,
        editable: flags.editable,
        clickable: flags.clickable,
    }
}

fn saturating_i32(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_has_inner_named_controls() {
        let layout = Layout::new(960, 600, 1.0);
        let tree = tree(
            "minicon",
            "Inspect [@1]",
            layout,
            960,
            600,
            true,
            "probe-text",
        );
        assert!(tree.nodes.len() >= 5);
        let command = tree.node(NODE_COMMAND).expect("command input is published");
        assert_eq!(command.name, COMMAND_NAME);
        assert_eq!(command.role, PublishedRole::Text);
        assert_eq!(command.text, "probe-text");
        assert!(command.focused);
        assert!(command.editable);
        let send = tree.node(NODE_SEND).expect("send button is published");
        assert_eq!(send.name, SEND_NAME);
        let newline = tree
            .node(NODE_NEWLINE)
            .expect("newline button is published");
        assert_eq!(newline.name, NEWLINE_NAME);
        assert_eq!(newline.role, PublishedRole::Button);
        // Distinct boxes, or a screen reader and a click-by-name automation
        // would reach whichever one the hit test happened to find first.
        assert_ne!(send.bounds, newline.bounds);
        assert_eq!(send.role, PublishedRole::Button);
        assert_ne!(send.role.as_str(), "frame");
        assert_ne!(send.role.as_str(), "application");
        // Five since the send control became a pair: tabs, session, command,
        // send, newline. Counted rather than spot-checked so a node that stops
        // being published fails here instead of going quiet in the tree.
        assert_eq!(tree.children_of(NODE_FRAME).len(), 5);
        let field = tree
            .node(NODE_OFFSCREEN_FIELD)
            .expect("offscreen field is published");
        assert_eq!(field.name, OFFSCREEN_FIELD_NAME);
        assert_eq!(field.parent, Some(NODE_SESSION));
        assert!(field.bounds.width > 0);
        assert!(field.bounds.height > 0);
        let session = tree.node(NODE_SESSION).expect("session is published");
        assert!(field.bounds.y >= session.bounds.y.saturating_add(session.bounds.height));
        assert_eq!(tree.children_of(NODE_SESSION), vec![NODE_OFFSCREEN_FIELD]);
    }

    /// `saturating_i32` is the only bridge from the frame's `u32` geometry to
    /// the accessibility tree's `i32` bounds, so it must clamp rather than wrap:
    /// a wrapped negative would place a node off the screen's top-left, where a
    /// screen reader would read it as being behind the window.
    #[test]
    fn saturating_i32_clamps_instead_of_wrapping() {
        assert_eq!(saturating_i32(0), 0);
        assert_eq!(saturating_i32(1), 1);
        assert_eq!(saturating_i32(i32::MAX as u32), i32::MAX, "the boundary");
        assert_eq!(
            saturating_i32(i32::MAX as u32 + 1),
            i32::MAX,
            "one past the boundary clamps rather than going negative"
        );
        assert_eq!(saturating_i32(u32::MAX), i32::MAX);
        assert!(
            saturating_i32(u32::MAX) > 0,
            "a wrapped value would be negative, which is the bug this prevents"
        );
    }

    /// The session box is the frame inset by the sidebar and capped at the
    /// composer's top edge, so the published bounds never claim the tab column
    /// or the input strip. A frame narrower than the sidebar yields a zero-width
    /// box rather than an underflow.
    #[test]
    fn session_rect_is_inset_by_the_sidebar_and_capped_at_the_composer() {
        let layout = Layout::new(960, 600, 1.0);
        let session = session_rect(layout, 960, 600);
        assert_eq!(
            session.x, layout.sidebar.width,
            "the tab column is excluded"
        );
        assert_eq!(session.y, 0);
        assert_eq!(session.width, 960 - layout.sidebar.width);
        assert_eq!(
            session.height, layout.composer.y,
            "the box stops where the composer starts"
        );

        // A frame shorter than the composer's top edge is capped by the frame.
        let short = session_rect(layout, 960, 10);
        assert_eq!(short.height, 10, "the frame height caps the box");

        // A frame narrower than the sidebar has no width left, not a negative.
        let narrow = session_rect(layout, 10, 600);
        assert_eq!(narrow.width, 0, "width saturates rather than underflowing");
    }

    /// The offscreen field sits just below the session box — AT-SPI needs a
    /// text field that exists but is out of the visual way — and it keeps at
    /// least one pixel of width so the tool cannot reject a zero-sized node.
    #[test]
    fn the_offscreen_field_sits_below_the_session_and_keeps_a_pixel_of_width() {
        let layout = Layout::new(960, 600, 1.0);
        let session = session_rect(layout, 960, 600);
        let field = offscreen_field_rect(session);
        assert_eq!(field.x, session.x, "it aligns with the session's left edge");
        assert_eq!(
            field.y,
            session.y + session.height + OFFSCREEN_FIELD_GAP,
            "it starts a gap below the session"
        );
        assert_eq!(field.width, session.width);
        assert_eq!(field.height, OFFSCREEN_FIELD_HEIGHT);

        // A session with no width still yields a usable node.
        let empty = Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        let from_empty = offscreen_field_rect(empty);
        assert_eq!(
            from_empty.width, 1,
            "a zero-width session still gets a node"
        );
        assert_eq!(from_empty.y, OFFSCREEN_FIELD_GAP);

        // A session tall enough that the offset would overflow saturates, so the
        // field stays on the positive side of the origin.
        let huge = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: u32::MAX,
        };
        let from_huge = offscreen_field_rect(huge);
        assert_eq!(from_huge.y, u32::MAX, "the offset saturates");
        assert!(from_huge.y > 0, "a wrapped offset would be near zero");
    }

    #[test]
    fn extreme_frame_bounds_saturate_instead_of_collapsing() {
        let tree = tree(
            "minicon",
            "terminal",
            Layout::new(u32::MAX, u32::MAX, 1.0),
            u32::MAX,
            u32::MAX,
            false,
            "",
        );
        let application = tree.node(NODE_APPLICATION).unwrap();
        assert_eq!(application.bounds.width, i32::MAX);
        assert_eq!(application.bounds.height, i32::MAX);
        let command = tree.node(NODE_COMMAND).unwrap();
        assert!(command.bounds.x > 0);
        assert!(command.bounds.y > 0);
        assert!(command.bounds.width >= 0);
        assert!(command.bounds.height >= 0);
    }

    #[test]
    fn action_inbox_is_fifo_bounded_and_budgeted() {
        let inbox = ActionInbox::default();
        for node in 0..ACTION_QUEUE_CAPACITY {
            let outcome = inbox.push(Request {
                node: node as u32,
                action: PublishedAction::Focus,
            });
            assert!(outcome.accepted);
            assert_eq!(outcome.should_wake, node == 0);
        }
        assert_eq!(
            inbox.push(Request {
                node: u32::MAX,
                action: PublishedAction::Click,
            }),
            ActionPush::default()
        );
        assert_eq!(
            inbox.stats(),
            ActionInboxStats {
                pending: ACTION_QUEUE_CAPACITY,
                pending_bytes: 0,
                dropped: 1,
            }
        );

        let (first, backlog) = inbox.pop_batch(ACTION_DRAIN_BUDGET);
        assert!(backlog);
        assert_eq!(first.len(), ACTION_DRAIN_BUDGET);
        assert_eq!(first.first().map(|request| request.node), Some(0));
        assert_eq!(
            first.last().map(|request| request.node),
            Some(ACTION_DRAIN_BUDGET as u32 - 1)
        );
        let (second, backlog) = inbox.pop_batch(usize::MAX);
        assert!(!backlog);
        assert_eq!(second.len(), ACTION_QUEUE_CAPACITY - ACTION_DRAIN_BUDGET);
        assert_eq!(inbox.stats().pending, 0);

        // The inbox is empty again: the next push is the one that must wake the
        // GUI, and only that one. A wake on a non-empty queue would coalesce
        // nothing and, post-drain, a missing wake would strand the request.
        let first_after_drain = inbox.push(Request {
            node: 7,
            action: PublishedAction::Focus,
        });
        assert_eq!(
            first_after_drain,
            ActionPush {
                accepted: true,
                should_wake: true,
            }
        );
        assert!(
            !inbox
                .push(Request {
                    node: 8,
                    action: PublishedAction::Focus,
                })
                .should_wake
        );

        // A zero-limit drain is a no-op, not a wake with nothing behind it.
        let (nothing, backlog) = inbox.pop_batch(0);
        assert!(nothing.is_empty());
        assert!(backlog, "the queue is still non-empty");
        assert_eq!(inbox.stats().pending, 2);
    }

    #[test]
    fn action_inbox_bounds_payload_bytes_and_returns_drained_capacity() {
        let inbox = ActionInbox::default();
        let chunk = "x".repeat(crate::composer::PASTE_LIMIT_BYTES);
        for _ in 0..ACTION_QUEUE_MAX_BYTES / chunk.len() {
            assert!(
                inbox
                    .push(Request {
                        node: NODE_COMMAND,
                        action: PublishedAction::SetText(chunk.clone()),
                    })
                    .accepted
            );
        }
        assert!(
            !inbox
                .push(Request {
                    node: NODE_COMMAND,
                    action: PublishedAction::SetText("overflow".into()),
                })
                .accepted
        );
        assert_eq!(inbox.stats().pending_bytes, ACTION_QUEUE_MAX_BYTES);

        let (drained, _) = inbox.pop_batch(1);
        assert_eq!(drained.len(), 1);
        assert_eq!(
            inbox.stats().pending_bytes,
            ACTION_QUEUE_MAX_BYTES - crate::composer::PASTE_LIMIT_BYTES
        );
        assert!(
            !inbox
                .push(Request {
                    node: NODE_COMMAND,
                    action: PublishedAction::SetText(
                        "y".repeat(crate::composer::PASTE_LIMIT_BYTES + 1),
                    ),
                })
                .accepted
        );
    }

    /// A drained batch subtracts exactly the bytes it carried, including the
    /// zero-byte Click/Focus actions mixed among text and keys, and draining the
    /// last request returns the byte budget to zero. A leak here would leave the
    /// queue looking full and silently drop every later push.
    #[test]
    fn draining_a_mixed_batch_returns_the_byte_budget_to_zero() {
        let inbox = ActionInbox::default();
        let set_text = |bytes: usize| PublishedAction::SetText("t".repeat(bytes));
        let key = |event: &str| {
            PublishedAction::Key(agenterm_platform::accessibility_publish::PublishedKey {
                keysym: 0,
                event_string: event.to_owned(),
                is_text: true,
                modifiers: 0,
                pressed: true,
            })
        };
        let pushes = [
            set_text(10),
            PublishedAction::Click,
            key("Ctrl+C"),
            PublishedAction::Focus,
            set_text(5),
        ];
        for action in pushes.clone() {
            assert!(
                inbox
                    .push(Request {
                        node: NODE_COMMAND,
                        action
                    })
                    .accepted
            );
        }
        let expected_bytes = (10 + "Ctrl+C".len()) + 5;
        assert_eq!(inbox.stats().pending_bytes, expected_bytes);

        let (batch, backlog) = inbox.pop_batch(usize::MAX);
        assert!(!backlog);
        assert_eq!(batch.len(), pushes.len());
        assert_eq!(
            inbox.stats().pending_bytes,
            0,
            "a fully drained queue must return its byte budget to zero"
        );
        assert_eq!(inbox.stats().pending, 0);

        // The budget is usable again: a full-size paste is accepted after the
        // drain, which it would not be if `pending_bytes` had leaked.
        assert!(
            inbox
                .push(Request {
                    node: NODE_COMMAND,
                    action: set_text(crate::composer::PASTE_LIMIT_BYTES),
                })
                .accepted,
            "the byte budget must be available again after a full drain"
        );
    }

    /// `payload_bytes` is what the queue's byte budget is charged against, so
    /// its per-variant rule must be exact: text and key event strings count
    /// their bytes, and the pointer-free actions cost nothing. A change here
    /// silently moves every budget it feeds.
    #[test]
    fn payload_bytes_counts_text_and_keys_but_not_clicks() {
        let at = |action: PublishedAction| {
            Request {
                node: NODE_COMMAND,
                action,
            }
            .payload_bytes()
        };
        assert_eq!(at(PublishedAction::SetText(String::new())), 0);
        assert_eq!(at(PublishedAction::SetText("abc".to_owned())), 3);
        // Bytes, not characters: three CJK characters are nine bytes.
        assert_eq!(
            at(PublishedAction::SetText(
                "\u{4e2d}\u{6587}\u{5b57}".to_owned()
            )),
            9
        );
        assert_eq!(
            at(PublishedAction::Key(
                agenterm_platform::accessibility_publish::PublishedKey {
                    keysym: 0,
                    event_string: "Ctrl+Shift+P".to_owned(),
                    is_text: false,
                    modifiers: 0,
                    pressed: true,
                }
            )),
            "Ctrl+Shift+P".len(),
            "a key is charged for its event string"
        );
        assert_eq!(at(PublishedAction::Click), 0, "a click carries no payload");
        assert_eq!(at(PublishedAction::Focus), 0, "a focus carries no payload");
    }

    /// The single-action bound is inclusive: exactly `PASTE_LIMIT_BYTES` is
    /// accepted and one byte more is refused. Which side of that line the
    /// comparison falls on is the whole difference between a large paste
    /// working and being silently dropped.
    #[test]
    fn the_single_action_bound_accepts_exactly_the_paste_limit() {
        let inbox = ActionInbox::default();
        let set = |bytes: usize| Request {
            node: NODE_COMMAND,
            action: PublishedAction::SetText("x".repeat(bytes)),
        };
        // Exactly the limit is accepted.
        assert!(
            inbox.push(set(crate::composer::PASTE_LIMIT_BYTES)).accepted,
            "exactly the limit must be accepted"
        );
        // One byte more is refused, and the refusal is counted rather than
        // silent.
        assert!(
            !inbox
                .push(set(crate::composer::PASTE_LIMIT_BYTES + 1))
                .accepted,
            "one byte over the limit must be refused"
        );
        assert_eq!(inbox.stats().dropped, 1, "a refusal is counted");
        assert_eq!(
            inbox.stats().pending_bytes,
            crate::composer::PASTE_LIMIT_BYTES,
            "the refused push must not be charged"
        );
    }

    /// A wide character is charged by its UTF-8 bytes, so a text of
    /// `PASTE_LIMIT_BYTES / 3` three-byte characters is exactly at the limit
    /// while one more character crosses it. A character-counted budget would
    /// accept three times the intended payload.
    #[test]
    fn the_byte_budget_counts_utf8_bytes_not_characters() {
        let inbox = ActionInbox::default();
        // The limit is not divisible by three, so `n` characters hold
        // `3n` bytes and the remainder is what stops the next one from fitting.
        let limit = crate::composer::PASTE_LIMIT_BYTES;
        let cjk_chars = limit / 3;
        let fits = cjk_chars * 3;
        assert!(fits <= limit, "{fits} three-byte characters fit in {limit}");
        assert!(
            fits + 3 > limit,
            "one more character must not fit: {} > {limit}",
            fits + 3
        );
        assert!(
            inbox
                .push(Request {
                    node: NODE_COMMAND,
                    action: PublishedAction::SetText("\u{4e2d}".repeat(cjk_chars)),
                })
                .accepted,
            "as many three-byte characters as fit must be accepted"
        );
        assert_eq!(inbox.stats().pending_bytes, fits);
        assert!(
            !inbox
                .push(Request {
                    node: NODE_COMMAND,
                    action: PublishedAction::SetText("\u{4e2d}".repeat(cjk_chars + 1)),
                })
                .accepted,
            "one more three-byte character must cross the limit"
        );
    }

    /// Zero-cost actions still consume queue slots, so the capacity bound — not
    /// the byte bound — is what stops them from growing without limit.
    #[test]
    fn zero_byte_actions_are_bounded_by_capacity_not_bytes() {
        let inbox = ActionInbox::default();
        for _ in 0..ACTION_QUEUE_CAPACITY {
            assert!(
                inbox
                    .push(Request {
                        node: NODE_COMMAND,
                        action: PublishedAction::Focus,
                    })
                    .accepted
            );
        }
        assert_eq!(inbox.stats().pending_bytes, 0, "focus actions are free");
        assert_eq!(inbox.stats().pending, ACTION_QUEUE_CAPACITY);
        assert!(
            !inbox
                .push(Request {
                    node: NODE_COMMAND,
                    action: PublishedAction::Focus,
                })
                .accepted,
            "the slot bound stops free actions, not the byte budget"
        );
    }

    /// Four producers push while a consumer drains the same queue. Both take the
    /// same lock, so the byte ledger and the queue contents cannot interleave,
    /// and the ledger must balance: every push is either drained or counted as
    /// dropped, and draining to empty returns `pending_bytes` to zero. A leak
    /// here would make the queue look full and silently refuse later pushes.
    #[test]
    fn the_byte_ledger_balances_under_concurrent_push_and_drain() {
        const PRODUCERS: usize = 4;
        const PER_PRODUCER: usize = 500;
        let inbox = std::sync::Arc::new(ActionInbox::default());

        let producers: Vec<_> = (0..PRODUCERS)
            .map(|producer| {
                let inbox = std::sync::Arc::clone(&inbox);
                std::thread::spawn(move || {
                    let mut accepted = 0usize;
                    for index in 0..PER_PRODUCER {
                        let node = (index + producer * 1000) as u32;
                        let action = PublishedAction::SetText("x".repeat(1 + (index % 7)));
                        if inbox.push(Request { node, action }).accepted {
                            accepted += 1;
                        }
                    }
                    accepted
                })
            })
            .collect();

        let consumer = {
            let inbox = std::sync::Arc::clone(&inbox);
            std::thread::spawn(move || {
                let mut drained = 0usize;
                for _ in 0..(PRODUCERS * PER_PRODUCER) {
                    let (batch, _) = inbox.pop_batch(3);
                    drained += batch.len();
                    if drained >= PRODUCERS * PER_PRODUCER {
                        break;
                    }
                    std::thread::yield_now();
                }
                drained
            })
        };

        let accepted: usize = producers
            .into_iter()
            .map(|producer| producer.join().expect("a producer thread finishes"))
            .sum();
        let mut drained = consumer.join().expect("the consumer thread finishes");
        // Drain whatever the consumer's loop bound left behind, so the ledger is
        // read at emptiness rather than mid-flight.
        loop {
            let (batch, more) = inbox.pop_batch(usize::MAX);
            drained += batch.len();
            if !more {
                break;
            }
        }

        let stats = inbox.stats();
        assert_eq!(stats.pending, 0, "the queue must be empty");
        assert_eq!(
            stats.pending_bytes, 0,
            "a drained queue must return its byte budget to zero"
        );
        assert_eq!(
            accepted as u64 + stats.dropped,
            (PRODUCERS * PER_PRODUCER) as u64,
            "every push is either accepted or counted as dropped"
        );
        assert_eq!(
            drained, accepted,
            "every accepted push must come out exactly once"
        );
    }
}
