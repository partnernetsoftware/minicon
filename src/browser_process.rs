//! Bounded Linux page-host transport; adapter contract owner: PRD 28.
use std::{
    io::{BufRead, Read, Write},
    process::Child,
    sync::{Arc, Mutex, mpsc},
};

pub(super) struct BrowserProcess {
    commands: mpsc::SyncSender<String>,
    state: Arc<Mutex<ProcessState>>,
}

#[derive(Default)]
struct ProcessState {
    latest: Option<serde_json::Value>,
    shortcuts: Vec<char>,
    embedding: Option<(u32, u64)>,
}

impl ProcessState {
    fn receive(&mut self, value: serde_json::Value, pid: u32) -> bool {
        // Ready frames are not page snapshots; retain the last URL/title batch.
        let empty = self.latest.is_none() && self.shortcuts.is_empty();
        if let Some(ready) = value.get("embedded_ready") {
            if ready["pid"].as_u64() == Some(u64::from(pid))
                && let Some(parent) = ready["parent"].as_u64().filter(|p| *p != 0)
            {
                self.embedding = Some((pid, parent));
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
    pub(super) fn attach(child: &mut Child, wake: impl Fn() + Send + 'static) -> Option<Self> {
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
        });
        Some(Self { commands, state })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_frame_requires_the_spawned_pid_and_never_replaces_the_page_snapshot() {
        let mut state = ProcessState::default();
        let page = serde_json::json!({"url":"https://ordinary.invalid/live", "title":"Ordinary"});
        assert!(state.receive(page.clone(), 17));
        state.receive(
            serde_json::json!({"embedded_ready":{"pid":29,"parent":42}}),
            17,
        );
        assert_eq!(state.embedding, None);
        state.receive(
            serde_json::json!({"embedded_ready":{"pid":17,"parent":0}}),
            17,
        );
        assert_eq!(state.embedding, None);
        state.receive(
            serde_json::json!({"embedded_ready":{"pid":17,"parent":42}}),
            17,
        );
        assert_eq!(state.embedding, Some((17, 42)));
        assert_eq!(state.latest, Some(page));
        let (commands, _receiver) = mpsc::sync_channel(1);
        let process = BrowserProcess {
            commands,
            state: Arc::new(Mutex::new(state)),
        };
        assert!(process.embedded_ready(42, 17));
        assert!(!process.embedded_ready(43, 17));
        assert!(!process.embedded_ready(42, 29));
    }
}
