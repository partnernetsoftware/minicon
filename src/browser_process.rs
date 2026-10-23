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
}

impl BrowserProcess {
    pub(super) fn attach(child: &mut Child, wake: impl Fn() + Send + 'static) -> Option<Self> {
        let mut input = child.stdin.take()?;
        let output = child.stdout.take()?;
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
                        // One pending snapshot/shortcut batch needs one wake.
                        // Coalescing prevents a noisy page from filling the
                        // GUI's event queue while it is busy painting.
                        let empty = state.latest.is_none() && state.shortcuts.is_empty();
                        if let Some(shortcut) =
                            value["shortcut"].as_str().and_then(|s| s.chars().next())
                        {
                            if state.shortcuts.len() < 16 {
                                state.shortcuts.push(shortcut);
                            }
                        } else {
                            state.latest = Some(value);
                        }
                        empty
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
