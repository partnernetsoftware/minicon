//! Bounded Linux page-host transport; adapter contract owner: PRD 28.
use minicon_web::NativePageReleaseStatus as ReleaseStatus;
use std::{
    cell::{Cell, RefCell},
    io::{BufRead, Read, Write},
    process::{Child, ExitStatus},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

pub(super) struct BrowserProcess {
    pid: u32,
    parent: u64,
    commands: mpsc::SyncSender<String>,
    state: Arc<Mutex<ProcessState>>,
}

#[derive(Default)]
struct ProcessState {
    latest: Option<serde_json::Value>,
    shortcuts: Vec<char>,
    embedding: Option<(u32, u64)>,
    close_command_received: bool,
    native_view_destroyed: bool,
    closed: bool,
    reader_finished: bool,
}

impl ProcessState {
    fn receive(&mut self, value: serde_json::Value, pid: u32) -> bool {
        // Ready frames are not page snapshots; retain the last URL/title batch.
        let empty = self.latest.is_none() && self.shortcuts.is_empty();
        if let Some(progress) = value.get("release_progress") {
            if progress["pid"].as_u64() == Some(u64::from(pid)) {
                match progress["stage"].as_str() {
                    Some("command-received") => self.close_command_received = true,
                    Some("native-view-destroyed") => self.native_view_destroyed = true,
                    _ => {}
                }
            }
        } else if let Some(closed) = value.get("closed") {
            if closed["pid"].as_u64() == Some(u64::from(pid)) {
                self.closed = true;
            }
        } else if let Some(ready) = value.get("embedded_ready") {
            if ready["pid"].as_u64() == Some(u64::from(pid))
                && let Some(parent) = ready["parent"].as_u64().filter(|p| *p != 0)
            {
                self.embedding = Some((pid, parent));
                return true;
            }
        } else if let Some(shortcut) = value["shortcut"].as_str().and_then(|s| s.chars().next()) {
            if self.shortcuts.len() < 16 {
                self.shortcuts.push(shortcut);
            }
        } else {
            self.latest = Some(value);
        }
        empty
    }
}

impl BrowserProcess {
    pub(super) fn attach(
        child: &mut Child,
        parent: u64,
        wake: impl Fn() + Send + 'static,
    ) -> Option<Self> {
        let mut input = child.stdin.take()?;
        let output = child.stdout.take()?;
        let pid = child.id();
        let (commands, receive) = mpsc::sync_channel::<String>(8);
        std::thread::spawn(move || {
            for command in receive {
                if writeln!(input, "{command}")
                    .and_then(|_| input.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
        let state = Arc::new(Mutex::new(ProcessState::default()));
        let observed = state.clone();
        std::thread::spawn(move || {
            let mut output = std::io::BufReader::new(output);
            loop {
                let mut bytes = Vec::new();
                match output.by_ref().take(16386).read_until(b'\n', &mut bytes) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if bytes.len() > 16384 => break,
                    _ => {}
                }
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    if !value.is_object() {
                        continue;
                    }
                    let notify = if let Ok(mut state) = observed.lock() {
                        state.receive(value, pid)
                    } else {
                        false
                    };
                    if notify {
                        wake();
                    }
                }
            }
            if let Ok(mut state) = observed.lock() {
                state.reader_finished = true;
            }
            wake();
        });
        Some(Self {
            pid,
            parent,
            commands,
            state,
        })
    }

    pub(super) fn send(&self, action: &str, url: Option<&str>) -> Result<(), String> {
        let message = serde_json::json!({ "action":action, "url":url }).to_string();
        if message.len() > 8191 {
            return Err("browser-url-too-long".to_owned());
        }
        self.commands
            .try_send(message)
            .map_err(|_| "browser-host-busy-or-closed".to_owned())
    }

    /// A frame emitted after the helper's last initial map and checked native
    /// ownership publication. A starting helper must never be parked early.
    pub(super) fn embedded_ready(&self, parent: u64, pid: u32) -> bool {
        self.state
            .lock()
            .is_ok_and(|state| state.embedding == Some((pid, parent)))
    }

    pub(super) fn hide_embedded(&self, pid: u32) -> bool {
        self.parent != 0
            && self.pid == pid
            && self.embedded_ready(self.parent, pid)
            && crate::browser::set_embedded_page_visible_for_process(self.parent, pid, false)
    }

    pub(super) fn hide_for_release(&self, pid: u32) -> bool {
        self.hide_embedded(pid)
    }

    pub(super) fn take_state(&self) -> Option<serde_json::Value> {
        let mut state = self.state.lock().ok()?;
        if state.latest.is_none() && state.shortcuts.is_empty() {
            return None;
        }
        let mut value = state.latest.take().unwrap_or(serde_json::json!({}));
        value["shortcuts"] = serde_json::json!(std::mem::take(&mut state.shortcuts));
        Some(value)
    }
}

/// GUI-thread polling owns the exact child, its pipe and extracted executable
/// until normal close is observed. It never waits/sleeps on the active GUI.
pub(super) struct BrowserRelease {
    child: RefCell<Option<Child>>,
    transport: RefCell<Option<BrowserProcess>>,
    program: RefCell<Option<crate::browser::EmbeddedPageProgram>>,
    exit: Cell<Option<ExitStatus>>,
    state: Cell<ReleaseStatus>,
    deadline: Instant,
    kill_requested: Cell<bool>,
}

impl BrowserRelease {
    pub(super) fn begin(
        child: Child,
        transport: Option<BrowserProcess>,
        program: Option<crate::browser::EmbeddedPageProgram>,
        initial_error: Option<&'static str>,
    ) -> Self {
        let state = match initial_error {
            Some(error) => ReleaseStatus::Failed(error),
            None => match &transport {
                Some(process) if process.pid == child.id() => {
                    if process.send("close", None).is_ok() {
                        ReleaseStatus::Pending
                    } else {
                        ReleaseStatus::Failed("browser-native-release-command-failed")
                    }
                }
                _ => ReleaseStatus::Failed("browser-native-release-transport-unavailable"),
            },
        };
        Self {
            child: RefCell::new(Some(child)),
            transport: RefCell::new(transport),
            program: RefCell::new(program),
            exit: Cell::new(None),
            state: Cell::new(state),
            deadline: Instant::now() + Duration::from_secs(10),
            kill_requested: Cell::new(false),
        }
    }

    pub(super) fn status(&self) -> ReleaseStatus {
        if Instant::now() >= self.deadline && self.state.get() == ReleaseStatus::Pending {
            self.state
                .set(ReleaseStatus::Failed("browser-context-release-timeout"));
        }
        let mut child = self.child.borrow_mut();
        if let Some(process) = child.as_mut() {
            if matches!(self.state.get(), ReleaseStatus::Failed(_))
                && !self.kill_requested.replace(true)
            {
                let _ = process.kill();
            }
            match process.try_wait() {
                Ok(Some(exit)) => {
                    self.exit.set(Some(exit));
                    *child = None;
                }
                Ok(None) => {}
                Err(_) => self.state.set(ReleaseStatus::Failed(
                    "browser-native-release-process-unreadable",
                )),
            }
        }
        if let Some(exit) = self.exit.get() {
            if self.state.get() == ReleaseStatus::Pending {
                if !exit.success() {
                    self.state.set(ReleaseStatus::Failed(
                        "browser-native-release-abnormal-exit",
                    ));
                } else if let Some(transport) = self.transport.borrow().as_ref() {
                    match transport.state.lock() {
                        Ok(state) if state.closed => self.state.set(ReleaseStatus::Released),
                        Ok(state) if state.reader_finished => self
                            .state
                            .set(ReleaseStatus::Failed("browser-native-release-missing-ack")),
                        Ok(_) => {}
                        Err(_) => self.state.set(ReleaseStatus::Failed(
                            "browser-native-release-transport-unavailable",
                        )),
                    }
                }
            }
            if self.state.get() != ReleaseStatus::Pending {
                self.transport.borrow_mut().take();
                self.program.borrow_mut().take();
            }
        }
        self.state.get()
    }

    pub(super) fn poll_deadline(&self) -> Option<Instant> {
        (self.child.borrow().is_some() || self.state.get() == ReleaseStatus::Pending)
            .then(|| Instant::now() + Duration::from_millis(20))
    }
}

#[cfg(feature = "native-auth-court")]
impl BrowserRelease {
    pub(super) fn diagnostic(&self) -> serde_json::Value {
        let transport = self.transport.borrow();
        let pipe = transport
            .as_ref()
            .and_then(|process| process.state.lock().ok());
        serde_json::json!({
            "status":format!("{:?}", self.state.get()),
            "child_retained":self.child.borrow().is_some(),
            "exit_success":self.exit.get().map(|exit| exit.success()),
            "kill_requested":self.kill_requested.get(),
            "close_command_received":pipe.as_ref().map(|state| state.close_command_received),
            "native_view_destroyed":pipe.as_ref().map(|state| state.native_view_destroyed),
            "closed_ack":pipe.as_ref().map(|state| state.closed),
            "pipe_finished":pipe.as_ref().map(|state| state.reader_finished),
        })
    }
}

impl Drop for BrowserRelease {
    fn drop(&mut self) {
        // Normally already reaped by nonblocking status(). This fallback is
        // final host teardown, never a successful release observation.
        if let Some(mut child) = self.child.get_mut().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_progress_requires_the_exact_child_and_never_acknowledges_release() {
        let mut state = ProcessState::default();
        let page = serde_json::json!({"title":"Owned"});
        state.receive(page.clone(), 17);
        for stage in ["command-received", "native-view-destroyed"] {
            state.receive(
                serde_json::json!({"release_progress":{"pid":29,"stage":stage}}),
                17,
            );
            assert!(
                !state.close_command_received && !state.native_view_destroyed,
                "a different helper must not report release progress"
            );
        }
        state.receive(
            serde_json::json!({"release_progress":{"pid":17,"stage":"unrecognized"}}),
            17,
        );
        assert!(!state.close_command_received && !state.native_view_destroyed);
        for stage in ["command-received", "native-view-destroyed"] {
            state.receive(
                serde_json::json!({"release_progress":{"pid":17,"stage":stage}}),
                17,
            );
        }
        assert!(state.close_command_received && state.native_view_destroyed);
        assert!(
            !state.closed,
            "release progress is not a native close acknowledgement"
        );
        assert_eq!(state.latest, Some(page));
    }

    #[test]
    fn closed_frame_requires_the_exact_child_and_preserves_the_page_snapshot() {
        let mut state = ProcessState::default();
        let page = serde_json::json!({"url":"https://synthetic.invalid/page","title":"Owned"});
        state.receive(page.clone(), 17);
        for pid in [0, 29] {
            state.receive(serde_json::json!({"closed":{"pid":pid}}), 17);
            assert!(
                !state.closed,
                "a different helper must not acknowledge native close"
            );
        }
        state.receive(serde_json::json!({"closed":{"pid":17}}), 17);
        assert!(state.closed);
        assert_eq!(state.latest, Some(page));
    }

    fn release_fixture(script: &str) -> BrowserRelease {
        let script = format!(
            "read -r line; [ \"$line\" = '{{\"action\":\"close\",\"url\":null}}' ] || exit 23; {script}"
        );
        let mut child = std::process::Command::new("sh")
            .args(["-c", &script])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let transport = BrowserProcess::attach(&mut child, 0, || {}).unwrap();
        BrowserRelease::begin(child, Some(transport), None, None)
    }

    fn terminal(release: &BrowserRelease) -> ReleaseStatus {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let status = release.status();
            if status != ReleaseStatus::Pending && release.child.borrow().is_none() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "owned release did not finish and reap within the test deadline"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn native_release_requires_the_close_ack_and_actual_normal_child_exit() {
        let release = release_fixture("printf '{\"closed\":{\"pid\":%s}}\\n' \"$$\"; exit 0");
        assert_eq!(terminal(&release), ReleaseStatus::Released);
        assert!(release.exit.get().unwrap().success());
        assert!(release.transport.borrow().is_none());
    }

    #[test]
    fn native_release_refuses_normal_exit_without_a_native_close_ack() {
        let release = release_fixture("exit 0");
        assert_eq!(
            terminal(&release),
            ReleaseStatus::Failed("browser-native-release-missing-ack"),
            "normal exit alone is not a native close acknowledgement"
        );
    }

    #[test]
    fn native_release_refuses_an_abnormal_child_even_with_a_matching_ack() {
        let release = release_fixture("printf '{\"closed\":{\"pid\":%s}}\\n' \"$$\"; exit 7");
        assert_eq!(
            terminal(&release),
            ReleaseStatus::Failed("browser-native-release-abnormal-exit"),
            "a matching acknowledgement cannot turn an abnormal exit into release"
        );
    }

    #[test]
    fn native_release_deadline_terminates_and_reaps_only_its_owned_child() {
        let mut release = release_fixture("while :; do :; done");
        release.deadline = Instant::now() + Duration::from_millis(20);
        assert_eq!(
            terminal(&release),
            ReleaseStatus::Failed("browser-context-release-timeout")
        );
        assert!(release.kill_requested.get());
        assert!(!release.exit.get().unwrap().success());
    }

    #[test]
    fn ready_frame_requires_the_spawned_pid_and_never_replaces_the_page_snapshot() {
        let mut state = ProcessState::default();
        let page = serde_json::json!({"url":"https://ordinary.invalid/live", "title":"Ordinary"});
        assert!(state.receive(page.clone(), 17));
        assert!(!state.receive(
            serde_json::json!({"embedded_ready":{"pid":29,"parent":42}}),
            17,
        ));
        assert_eq!(state.embedding, None);
        assert!(!state.receive(
            serde_json::json!({"embedded_ready":{"pid":17,"parent":0}}),
            17,
        ));
        assert_eq!(state.embedding, None);
        assert!(state.receive(
            serde_json::json!({"embedded_ready":{"pid":17,"parent":42}}),
            17,
        ));
        assert_eq!(state.embedding, Some((17, 42)));
        assert_eq!(state.latest, Some(page));
        let (commands, _receiver) = mpsc::sync_channel(1);
        let process = BrowserProcess {
            pid: 17,
            parent: 42,
            commands,
            state: Arc::new(Mutex::new(state)),
        };
        assert!(process.embedded_ready(42, 17));
        assert!(!process.embedded_ready(43, 17));
        assert!(!process.embedded_ready(42, 29));
    }
}
