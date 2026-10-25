//! WebView2 release observation. Owner: PRD 28.01; no profile deletion authority.
use std::{
    cell::Cell,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
};

use crate::NativePageReleaseStatus;
use webview2_com::{
    BrowserProcessExitedEventHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_NORMAL, ICoreWebView2BrowserProcessExitedEventArgs,
        ICoreWebView2Environment5,
    },
};
use windows_core::Interface;
use wry::{WebView, WebViewExtWindows};

/// Retains the actual WebView2 environment and its event subscription after
/// the view closes. Create, poll and drop only on the owning GUI/COM thread.
/// The callback stores one finite status; it never serializes SDK diagnostics.
pub struct NativePageRelease {
    status: Rc<Cell<NativePageReleaseStatus>>,
    subscription: Option<Subscription>,
}

struct Subscription {
    environment: ICoreWebView2Environment5,
    token: i64,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        // No callback captures this environment/subscription, avoiding a COM
        // reference cycle. Dropping this observer cannot reopen or kill a view.
        let _ = unsafe { self.environment.remove_BrowserProcessExited(self.token) };
    }
}

impl NativePageRelease {
    pub fn status(&self) -> NativePageReleaseStatus {
        self.status.get()
    }

    pub(super) fn observe(webview: &WebView, wake: impl Fn() + 'static) -> Self {
        let status = Rc::new(Cell::new(NativePageReleaseStatus::Pending));
        let mut observation = Self {
            status: status.clone(),
            subscription: None,
        };
        let mut owned_pid = 0;
        if unsafe { webview.webview().BrowserProcessId(&mut owned_pid) }.is_err() || owned_pid == 0
        {
            status.set(NativePageReleaseStatus::Failed(
                "browser-native-release-process-unreadable",
            ));
            return observation;
        }
        let Ok(environment) = webview.environment().cast::<ICoreWebView2Environment5>() else {
            status.set(NativePageReleaseStatus::Failed(
                "browser-native-release-unsupported",
            ));
            return observation;
        };
        let mut token = 0;
        let event_status = status.clone();
        let handler = BrowserProcessExitedEventHandler::create(Box::new(move |_, args| {
            // SDK callbacks must never unwind across COM. Only constant-size
            // native reads and a finite status reduction occur here.
            let reduced = catch_unwind(AssertUnwindSafe(|| {
                let before = event_status.get();
                let after = reduce(before, owned_pid, read_event(args.as_ref()));
                event_status.set(after);
                after != before
            }));
            let changed = match reduced {
                Ok(changed) => changed,
                Err(_) => {
                    let before = event_status.get();
                    let after = terminal_failure(before, "browser-native-release-callback-failed");
                    event_status.set(after);
                    after != before
                }
            };
            if changed {
                // A wake failure cannot unwind across FFI or rewrite an
                // already observed terminal native fact. The caller may poll.
                let _ = catch_unwind(AssertUnwindSafe(&wake));
            }
            Ok(())
        }));
        if unsafe { environment.add_BrowserProcessExited(&handler, &mut token) }.is_err() {
            status.set(NativePageReleaseStatus::Failed(
                "browser-native-release-subscribe-failed",
            ));
            return observation;
        }
        observation.subscription = Some(Subscription { environment, token });
        observation
    }
}

#[derive(Clone, Copy)]
struct ExitEvent {
    pid: u32,
    normal: bool,
}

fn read_event(args: Option<&ICoreWebView2BrowserProcessExitedEventArgs>) -> Result<ExitEvent, ()> {
    let args = args.ok_or(())?;
    let mut pid = 0;
    let mut kind = Default::default();
    unsafe {
        args.BrowserProcessId(&mut pid).map_err(|_| ())?;
        args.BrowserProcessExitKind(&mut kind).map_err(|_| ())?;
    }
    if pid == 0 {
        return Err(());
    }
    Ok(ExitEvent {
        pid,
        normal: kind == COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_NORMAL,
    })
}

fn terminal_failure(
    current: NativePageReleaseStatus,
    code: &'static str,
) -> NativePageReleaseStatus {
    if current == NativePageReleaseStatus::Pending {
        NativePageReleaseStatus::Failed(code)
    } else {
        current
    }
}

fn reduce(
    current: NativePageReleaseStatus,
    owned_pid: u32,
    event: Result<ExitEvent, ()>,
) -> NativePageReleaseStatus {
    if current != NativePageReleaseStatus::Pending {
        return current;
    }
    let Ok(event) = event else {
        return NativePageReleaseStatus::Failed("browser-native-release-event-unreadable");
    };
    if event.pid != owned_pid {
        return NativePageReleaseStatus::Pending;
    }
    if !event.normal {
        return NativePageReleaseStatus::Failed("browser-native-release-abnormal-exit");
    }
    NativePageReleaseStatus::Released
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_owned_normal_exit_releases_the_observation() {
        let pending = NativePageReleaseStatus::Pending;
        assert_eq!(
            reduce(
                pending,
                42,
                Ok(ExitEvent {
                    pid: 42,
                    normal: true
                })
            ),
            NativePageReleaseStatus::Released
        );
        assert_eq!(
            reduce(
                pending,
                42,
                Ok(ExitEvent {
                    pid: 43,
                    normal: true
                })
            ),
            NativePageReleaseStatus::Pending
        );
        let unrelated = reduce(
            pending,
            42,
            Ok(ExitEvent {
                pid: 43,
                normal: false,
            }),
        );
        assert_eq!(unrelated, NativePageReleaseStatus::Pending);
        assert_eq!(
            reduce(
                unrelated,
                42,
                Ok(ExitEvent {
                    pid: 42,
                    normal: true
                })
            ),
            NativePageReleaseStatus::Released
        );
        assert_eq!(
            reduce(
                pending,
                42,
                Ok(ExitEvent {
                    pid: 42,
                    normal: false
                })
            ),
            NativePageReleaseStatus::Failed("browser-native-release-abnormal-exit")
        );
    }

    #[test]
    fn unreadable_events_fail_without_native_diagnostic_payloads() {
        assert_eq!(
            reduce(NativePageReleaseStatus::Pending, 42, Err(())),
            NativePageReleaseStatus::Failed("browser-native-release-event-unreadable")
        );
    }

    #[test]
    fn observation_terminal_states_do_not_change_on_late_events() {
        for terminal in [
            NativePageReleaseStatus::Released,
            NativePageReleaseStatus::Failed("browser-native-release-abnormal-exit"),
        ] {
            for event in [
                Ok(ExitEvent {
                    pid: 42,
                    normal: true,
                }),
                Ok(ExitEvent {
                    pid: 43,
                    normal: false,
                }),
                Err(()),
            ] {
                assert_eq!(reduce(terminal, 42, event), terminal);
            }
            assert_eq!(terminal_failure(terminal, "callback-panic"), terminal);
        }
    }
}
