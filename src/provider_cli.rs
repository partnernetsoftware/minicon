//! Foreground composition of the same process-owned Harness service.
use crate::{
    provider_login::LoginMetadata,
    provider_runtime::{Action, ListenerState, Runtime},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub fn serve(args: &[String]) -> Result<(), String> {
    let port = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["provider", "serve"] => None,
        ["provider", "serve", "--port", value] => Some(
            value
                .parse::<u16>()
                .ok()
                .filter(|port| *port >= 1024)
                .ok_or("provider: port must be 1024..65535")?,
        ),
        _ => return Err("usage: minicon provider serve [--port PORT]".into()),
    };
    let _console = minicon_platform::parent_console::ParentConsoleSession::attach();
    let cancelled = Arc::new(AtomicBool::new(false));
    let _interrupt = crate::cli_interrupt::InterruptWatch::start(cancelled.clone())?;
    let root = crate::config_path()
        .and_then(|path| path.parent().map(|parent| parent.join("harness")))
        .ok_or("provider: configuration directory unavailable")?;
    let root = if root.is_absolute() {
        root
    } else {
        std::env::current_dir()
            .map_err(|_| "provider: current directory unavailable")?
            .join(root)
    };
    let mut runtime = Runtime::new(
        root,
        LoginMetadata {
            version: env!("CARGO_PKG_VERSION").into(),
            locale: "en-US".into(),
            timezone_offset_seconds: 0,
        },
        Arc::new(|| {}),
    )
    .map_err(|error| error.to_string())?;
    if !start_listener(
        &mut runtime,
        port,
        &cancelled,
        Instant::now() + Duration::from_secs(10),
    )? {
        return Ok(());
    }
    let port = runtime
        .view()
        .actual_port
        .ok_or("provider: listener address unavailable")?;
    let _ = minicon_platform::parent_console::write_stdout(&format!(
        "MiniCon model API listening at http://127.0.0.1:{port}/v1; Ctrl+C to stop"
    ));
    loop {
        runtime.poll();
        if runtime.view().listener == ListenerState::Error {
            return Err(runtime
                .view()
                .status
                .clone()
                .unwrap_or_else(|| "provider: listener failed".into()));
        }
        if cancelled.load(Ordering::Acquire) {
            runtime
                .act(Action::Stop)
                .map_err(|error| error.to_string())?;
            // Runtime Drop owns actual cancellation and joins before returning.
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Compose the public asynchronous runtime without treating an enqueued write
/// as a committed configuration. False means startup was cancelled.
fn start_listener(
    runtime: &mut Runtime,
    port: Option<u16>,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<bool, String> {
    loop {
        runtime.poll();
        if cancelled.load(Ordering::Acquire) {
            return Ok(false);
        }
        if !runtime.view().busy {
            break;
        }
        if Instant::now() >= deadline {
            return Err("provider: configuration load timed out".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if let Some(error) = &runtime.view().status {
        return Err(error.clone());
    }
    if let Some(port) = port.filter(|port| *port != runtime.view().port) {
        runtime
            .act(Action::SetPort(port))
            .map_err(|error| error.to_string())?;
        loop {
            runtime.poll();
            if cancelled.load(Ordering::Acquire) {
                return Ok(false);
            }
            if let Some(error) = &runtime.view().status {
                return Err(error.clone());
            }
            if runtime.view().port == port {
                break;
            }
            if Instant::now() >= deadline {
                return Err("provider: port save was not confirmed".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    runtime
        .act(Action::Start)
        .map_err(|error| error.to_string())?;
    loop {
        runtime.poll();
        if cancelled.load(Ordering::Acquire) {
            runtime
                .act(Action::Stop)
                .map_err(|error| error.to_string())?;
            return Ok(false);
        }
        if let Some(error) = &runtime.view().status {
            return Err(error.clone());
        }
        match runtime.view().listener {
            ListenerState::Listening => return Ok(true),
            ListenerState::Error => return Err("provider: listener failed".into()),
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err("provider: listener did not start".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        provider_accounts::AccountStore,
        provider_auth::{Provider, Secret, TokenGrant, TokenLifecycle},
        provider_server::{Config, ModelRoute, WireProtocol, random_id},
    };
    use minicon_platform::filesystem;
    use std::{net::TcpListener, path::PathBuf};
    struct Root(PathBuf);
    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn failed_port_save_never_starts_old_listener_and_same_port_needs_no_write() {
        let path =
            std::env::temp_dir().join(format!("minicon-provider-cli-{}", random_id().unwrap()));
        std::fs::create_dir_all(&path).unwrap();
        let root = Root(path.canonicalize().unwrap());
        filesystem::protect_private_directory(&root.0).unwrap();
        let store = AccountStore::new(root.0.join("accounts")).unwrap();
        // Stored-ready fixture only; this court makes no upstream request and
        // does not qualify provider authentication.
        let account = store
            .insert(
                "Synthetic CLI court",
                TokenGrant {
                    provider: Provider::DeepSeek,
                    access_token: Secret::new("synthetic-provider").unwrap(),
                    lifecycle: TokenLifecycle::OpaqueUntilRejected,
                    organization: None,
                },
            )
            .unwrap();
        store.mark_ready(&account.id).unwrap();
        let mut config = Config::fresh().unwrap();
        config.port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        config.selected_account = Some(account.id);
        config.model = "deepseek/synthetic".into();
        config.models = vec![ModelRoute {
            id: config.model.clone(),
            model_id: "synthetic".into(),
            provider: Provider::DeepSeek,
            protocol: WireProtocol::Messages,
            url: "https://api.deepseek.com/anthropic/v1/messages".into(),
            headers: vec![],
        }];
        filesystem::write_private_atomic(
            &root.0.join("gateway.json"),
            &serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        filesystem::write_private_atomic(&root.0.join("client-key"), b"synthetic-local").unwrap();
        let mut runtime = Runtime::new(
            root.0.clone(),
            LoginMetadata {
                version: "synthetic".into(),
                locale: "en".into(),
                timezone_offset_seconds: 0,
            },
            Arc::new(|| {}),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while runtime.view().busy {
            runtime.poll();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        std::fs::remove_file(root.0.join("gateway.json")).unwrap();
        std::fs::create_dir(root.0.join("gateway.json")).unwrap();
        let different = if config.port == 65535 {
            config.port - 1
        } else {
            config.port + 1
        };
        let error = start_listener(
            &mut runtime,
            Some(different),
            &AtomicBool::new(false),
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(error.contains("Storage"));
        // A public FIFO observation also flushes any incorrectly queued Start;
        // a transient stopped snapshot alone would not prove its absence.
        runtime.act(Action::RefreshAccounts).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while runtime.view().status.is_some() {
            runtime.poll();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(runtime.view().port, config.port);
        assert_eq!(runtime.view().listener, ListenerState::Stopped);
        assert_eq!(runtime.view().actual_port, None);
        assert!(TcpListener::bind(("127.0.0.1", config.port)).is_ok());
        assert!(
            start_listener(
                &mut runtime,
                Some(config.port),
                &AtomicBool::new(false),
                Instant::now() + Duration::from_secs(5)
            )
            .unwrap()
        );
        assert_eq!(runtime.view().actual_port, Some(config.port));
        drop(runtime);
        assert!(TcpListener::bind(("127.0.0.1", config.port)).is_ok());
    }
}
