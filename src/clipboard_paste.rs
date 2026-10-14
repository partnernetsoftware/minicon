//! One owner for the clipboard-paste payload, shared by the composer and the
//! terminal content area.
//!
//! Both surfaces answer the same question -- what does a paste carry? -- with
//! the same shape: the clipboard's text unchanged when there is text,
//! otherwise a clipboard image saved to a temp file whose absolute path is
//! pasted as if typed, otherwise nothing. The composer asks synchronously; the
//! terminal asks asynchronously, so a native clipboard read never blocks the
//! event loop (`prd/PRD_02_25_con_workspace.md`, "Screenshot/image paste").
//!
//! `src/clipboard_image.rs` owns turning clipboard image bytes into a file;
//! this module owns which payload a paste carries. Keeping that decision here,
//! not at each call site, is what stops the composer (which had the image
//! fallback) and the terminal (which did not) from drifting apart.

use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};

use minicon_platform::contract::clipboard::ClipboardError;

/// What a paste carries, once resolved.
pub enum Payload {
    /// The clipboard's text, unchanged.
    Text(String),
    /// A clipboard image's temp-file absolute path, pasted as if typed.
    ImagePath(String),
}

impl Payload {
    fn into_string(self) -> String {
        match self {
            Payload::Text(text) => text,
            Payload::ImagePath(path) => path,
        }
    }
}

/// The paste payload, for a surface that reports failures (the terminal).
///
/// The clipboard's text when it holds non-empty text (so a copy that carries
/// both text and an image keeps its existing behavior), else a clipboard
/// image's temp-file absolute path, else an `Err` with a human-readable
/// reason ready for host UI. A text read that genuinely failed (a busy
/// clipboard, text over the caller's limit) is reported as itself when no
/// image rescued the paste, rather than folded into "nothing to paste".
pub fn payload(limit_bytes: usize) -> Result<Payload, String> {
    let text = minicon_platform::clipboard::get_text(limit_bytes);
    // An empty read is "no text", not "paste an empty string": it must not
    // shadow the image fallback below, and neither must a failed read.
    if let Ok(text) = &text
        && !text.is_empty()
    {
        return Ok(Payload::Text(text.clone()));
    }
    if let Some(path) = crate::clipboard_image::image_paste_as_temp_file_path() {
        return Ok(Payload::ImagePath(path));
    }
    match text {
        Err(error @ ClipboardError::Failed { .. }) => Err(error.message()),
        _ => Err("the clipboard holds no text or image".to_owned()),
    }
}

/// The paste payload as a plain string for a surface that surfaces no failure
/// (the composer): the payload's text or image path, else `None`.
pub fn text_or_image_path(limit_bytes: usize) -> Option<String> {
    payload(limit_bytes).ok().map(Payload::into_string)
}

/// Poll result for [`PayloadRead`].
pub enum PayloadPoll {
    Pending,
    /// The paste payload, or a human-readable reason the clipboard carried
    /// nothing pasteable. The caller prefixes it for host UI.
    Ready(Result<String, String>),
}

/// A paste payload being read off the event loop.
pub struct PayloadRead {
    receiver: Receiver<Result<String, String>>,
}

impl PayloadRead {
    pub fn try_poll(&self) -> PayloadPoll {
        match self.receiver.try_recv() {
            Ok(result) => PayloadPoll::Ready(result),
            Err(TryRecvError::Empty) => PayloadPoll::Pending,
            // The sender vanished without a result: the worker panicked before
            // the guard could report through it. Surface a terminal state so a
            // caller polling on Wake never waits forever.
            Err(TryRecvError::Disconnected) => PayloadPoll::Ready(Err(
                "clipboard read worker stopped without a result".to_owned(),
            )),
        }
    }
}

/// Reports exactly one terminal outcome for a paste read and wakes the caller
/// once. `complete` is the normal path; `Drop` is the fallback that a panicking
/// worker unwinds through, so the sender cannot be dropped silently and the
/// caller cannot be left un-woken.
struct PayloadCompletion {
    sender: Option<SyncSender<Result<String, String>>>,
    wake: Option<Box<dyn FnOnce() + Send + 'static>>,
}

impl PayloadCompletion {
    fn complete(mut self, result: Result<String, String>) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(result);
        }
        if let Some(wake) = self.wake.take() {
            wake();
        }
        // `self` drops here with both fields already taken, so a normal
        // completion wakes once -- `Drop` cannot re-send or re-wake.
    }
}

impl Drop for PayloadCompletion {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(Err("clipboard read worker panicked".to_owned()));
        }
        if let Some(wake) = self.wake.take() {
            wake();
        }
    }
}

/// Reads a paste payload on a named detached thread, invoking `wake` once when
/// the read reaches a terminal state (result, failure or panic).
pub fn read_async(
    limit_bytes: usize,
    wake: impl FnOnce() + Send + 'static,
) -> Result<PayloadRead, String> {
    let (sender, receiver) = mpsc::sync_channel(1);
    minicon_platform::threading::spawn_named_detached(
        "minicon-clipboard-paste",
        Box::new(move || {
            // Built before any fallible work, so a panic anywhere in the read
            // still unwinds through this guard and reports a panicked result.
            let completion = PayloadCompletion {
                sender: Some(sender),
                wake: Some(Box::new(wake)),
            };
            let result = payload(limit_bytes).map(Payload::into_string);
            completion.complete(result);
        }),
    )
    .map_err(|error| format!("could not start clipboard paste worker: {error}"))?;
    Ok(PayloadRead { receiver })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    /// `try_poll` must distinguish the three states a caller branches on:
    /// nothing yet, a result, and a vanished sender.
    #[test]
    fn poll_distinguishes_pending_ready_and_disconnect() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let read = PayloadRead { receiver };
        assert!(matches!(read.try_poll(), PayloadPoll::Pending));

        sender.send(Ok("payload".to_owned())).unwrap();
        assert!(matches!(
            read.try_poll(),
            PayloadPoll::Ready(Ok(text)) if text == "payload"
        ));

        let (sender, receiver) = mpsc::sync_channel(1);
        let read = PayloadRead { receiver };
        sender.send(Err("no paste".to_owned())).unwrap();
        assert!(matches!(
            read.try_poll(),
            PayloadPoll::Ready(Err(reason)) if reason == "no paste"
        ));
    }

    #[test]
    fn poll_reports_a_result_when_the_sender_is_dropped() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let read = PayloadRead { receiver };
        drop(sender);
        assert!(matches!(
            read.try_poll(),
            PayloadPoll::Ready(Err(reason))
                if reason == "clipboard read worker stopped without a result"
        ));
    }

    /// A normal completion must deliver the payload and wake exactly once; the
    /// guard's `Drop` must not fire again afterwards.
    #[test]
    fn completion_delivers_once_and_wakes_once() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&wakes);
        let completion = PayloadCompletion {
            sender: Some(sender),
            wake: Some(Box::new(move || {
                counter.fetch_add(1, Ordering::Relaxed);
            })),
        };
        completion.complete(Ok("payload".to_owned()));

        assert_eq!(
            wakes.load(Ordering::Relaxed),
            1,
            "a normal completion must wake exactly once, not again on drop"
        );

        let read = PayloadRead { receiver };
        assert!(matches!(
            read.try_poll(),
            PayloadPoll::Ready(Ok(text)) if text == "payload"
        ));
        // Exactly one result: the next poll sees the sender already gone rather
        // than a duplicate payload the guard sent a second time.
        assert!(matches!(
            read.try_poll(),
            PayloadPoll::Ready(Err(reason))
                if reason == "clipboard read worker stopped without a result"
        ));
    }

    /// A worker that panics (unwinds past `complete`) must still surface a
    /// terminal, non-panicking result and wake the caller once.
    #[test]
    fn completion_guard_reports_panic_shape_and_wakes_once() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&wakes);
        drop(PayloadCompletion {
            sender: Some(sender),
            wake: Some(Box::new(move || {
                counter.fetch_add(1, Ordering::Relaxed);
            })),
        });

        let read = PayloadRead { receiver };
        assert!(matches!(
            read.try_poll(),
            PayloadPoll::Ready(Err(reason)) if reason == "clipboard read worker panicked"
        ));
        assert_eq!(wakes.load(Ordering::Relaxed), 1);
    }

    /// End to end through the real worker: whatever the environment's clipboard
    /// holds, the read must reach a terminal state and wake exactly once,
    /// without the worker's panic reaching this thread.
    #[test]
    fn read_async_reaches_a_terminal_state_and_wakes_once() {
        let (woke_sender, woke_receiver) = mpsc::sync_channel(1);
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&wakes);
        let read = read_async(4096, move || {
            counter.fetch_add(1, Ordering::Relaxed);
            let _ = woke_sender.send(());
        })
        .expect("worker start");

        woke_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("the worker woke the caller");

        let poll = read.try_poll();
        assert!(
            matches!(poll, PayloadPoll::Ready(_)),
            "the worker must have published a terminal result"
        );
        assert_eq!(
            wakes.load(Ordering::Relaxed),
            1,
            "wake must fire exactly once"
        );
    }

    /// The decision must not panic when this process has no clipboard (a
    /// headless/service session) -- both entry points answer something.
    #[test]
    fn payload_does_not_panic_without_a_clipboard() {
        let _ = text_or_image_path(4096);
        let _ = payload(4096);
    }
}
