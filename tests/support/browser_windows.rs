//! Public-CLI evidence for native browser adapters (PRD 25 / 26).
use super::*;

struct BrowserGui {
    gui: OwnedGui,
    endpoint: String,
    exe: PathBuf,
    _trace: HostTrace,
    config_root: PathBuf,
}

impl BrowserGui {
    fn spawn(missing_runtime: bool, http_proxy: Option<&str>, https_proxy: Option<&str>) -> Self {
        Self::spawn_with_config(missing_runtime, http_proxy, https_proxy, None)
    }

    fn spawn_with_config(
        missing_runtime: bool,
        http_proxy: Option<&str>,
        https_proxy: Option<&str>,
        config_root: Option<PathBuf>,
    ) -> Self {
        let exe = minicon_binary();
        let suffix = unique_suffix();
        let endpoint = control_endpoint(&suffix);
        let root = exe.parent().unwrap().join("browser-court").join(suffix);
        fs::create_dir_all(&root).unwrap();
        let trace = HostTrace(root.join("host.log"));
        let config_root = config_root.unwrap_or_else(|| root.join("config"));
        let mut command = Command::new(&exe);
        command.args(["--no-activate", "--control", &endpoint, "-e"]);
        #[cfg(windows)]
        command.args(["cmd.exe", "/d", "/k"]);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.arg("/bin/sh").process_group(0);
        }
        command
            .env("MINICON_CONFIG_DIR", &config_root)
            .env("MINICON_BROWSER_PROFILE_ROOT", root.join("profile"))
            .env_remove("WEBVIEW2_USER_DATA_FOLDER")
            .env_remove("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER")
            .env_remove("MINICON_BROWSER_HTTP_PROXY")
            .env_remove("MINICON_BROWSER_HTTPS_PROXY")
            .env_remove("MINICON_BROWSER_NO_PROXY")
            .stdout(std::process::Stdio::null())
            .stderr(fs::File::create(&trace.0).unwrap());
        if missing_runtime {
            // Process-local override: never uninstall or modify the installed runtime.
            command.env(
                "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
                root.join("absent-runtime"),
            );
        }
        if let Some(proxy) = http_proxy {
            command.env("MINICON_BROWSER_HTTP_PROXY", proxy);
        }
        if let Some(proxy) = https_proxy {
            command.env("MINICON_BROWSER_HTTPS_PROXY", proxy);
        }
        let child = command.spawn().expect("start isolated browser GUI");
        let mut fixture = Self {
            gui: OwnedGui {
                child,
                screenshot: root.join("browser.png"),
            },
            endpoint,
            exe,
            _trace: trace,
            config_root,
        };
        wait_until_ready_for(
            &fixture.exe,
            &fixture.endpoint,
            Duration::from_secs(15),
            Some(&mut fixture.gui.child),
        );
        fixture
    }

    fn cli(&self, args: &[&str]) -> Value {
        cli_json(&self.exe, &self.endpoint, args)
    }

    fn browser(&self) -> Value {
        let tabs = self.cli(&["list-tabs"]);
        tabs["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tab| tab["kind"] == "browser")
            .expect("browser tab must exist")
            .clone()
    }

    fn click_geometry(&self, name: &str) {
        let snapshot = self.cli(&["ui-snapshot"]);
        let geometry = &snapshot["geometry"];
        self.click_rect(&geometry[name], name, geometry);
    }

    fn click_geometry_row(&self, name: &str, index: usize) {
        let snapshot = self.cli(&["ui-snapshot"]);
        let geometry = &snapshot["geometry"];
        self.click_rect(&geometry[name][index], name, geometry);
    }

    fn click_rect(&self, rect: &Value, name: &str, geometry: &Value) {
        let width = rect["width"].as_u64().expect("published hit-target width");
        let height = rect["height"]
            .as_u64()
            .expect("published hit-target height");
        assert!(
            width > 0 && height > 0,
            "{name} must remain a visible target"
        );
        let scale = geometry["scale_permille"].as_u64().unwrap() as f64 / 1000.0;
        let x = ((rect["x"].as_u64().unwrap() as f64 + width as f64 / 2.0) / scale)
            .round()
            .to_string();
        let y = ((rect["y"].as_u64().unwrap() as f64 + height as f64 / 2.0) / scale)
            .round()
            .to_string();
        self.cli(&[
            "send-ui-mouse",
            "--action",
            "click",
            "--button",
            "left",
            "--column",
            &x,
            "--row",
            &y,
        ]);
    }

    fn open_browser_from_menu(&self) -> Value {
        self.click_geometry("new_root");
        assert_eq!(
            self.cli(&["ui-snapshot"])["geometry"]["new_tab_menu_open"],
            true,
            "the visible New entry must open its menu"
        );
        self.click_geometry("new_browser_action");
        let browser = self.browser();
        assert_eq!(
            browser["active"], true,
            "the Browser action must focus its tab"
        );
        assert_eq!(
            self.cli(&["list-tabs"])["tabs"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|tab| tab["kind"] == "browser")
                .count(),
            1,
            "the visible action must reuse one Browser root"
        );
        assert_eq!(
            self.cli(&["ui-snapshot"])["geometry"]["new_tab_menu_open"],
            false
        );
        browser
    }

    fn wait_browser(&self, url: &str, load: &str, title: Option<&str>) -> Value {
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            let row = self.browser();
            if row["browser"]["url"] == url
                && row["browser"]["load"] == load
                && title.is_none_or(|title| row["browser"]["title"] == title)
            {
                return row;
            }
            assert!(
                Instant::now() < deadline,
                "expected {load} {url}, got {row}"
            );
            thread::sleep(Duration::from_millis(40));
        }
    }

    fn open_web_settings(&self) -> Value {
        self.click_geometry("settings");
        assert_eq!(self.cli(&["ui-snapshot"])["settings_open"], true);
        self.click_geometry("settings_console_route");
        assert_eq!(
            self.cli(&["ui-snapshot"])["geometry"]["settings_section"],
            "console"
        );
        self.click_geometry("settings_web_route");
        let snapshot = self.cli(&["ui-snapshot"]);
        assert_eq!(snapshot["geometry"]["settings_section"], "web");
        snapshot
    }

    fn close_and_wait(&mut self) {
        self.cli(&["close-window"]);
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.gui.child.try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "GUI did not close before restart"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn apply_proxy(&self, value: &str) {
        self.click_geometry("browser_proxy");
        self.cli(&["send-ui-keys", "Ctrl+a"]);
        self.cli(&["send-ui-keys", "Backspace"]);
        if !value.is_empty() {
            self.cli(&["send-ui-ime", "commit", value]);
        }
        self.cli(&["send-ui-keys", "Enter"]);
    }
}

struct LocalPreviewServer {
    address: std::net::SocketAddr,
    requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    blocked_requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl LocalPreviewServer {
    fn start(address: std::net::SocketAddr) -> Self {
        Self::start_mode(address, false)
    }

    fn start_proxy() -> Self {
        Self::start_mode("127.0.0.1:0".parse().unwrap(), true)
    }

    fn read_headers(stream: &mut std::net::TcpStream) -> Vec<u8> {
        use std::io::Read;
        let mut bytes = Vec::new();
        let mut chunk = [0; 1024];
        while bytes.len() < 8192 && !bytes.ends_with(b"\r\n\r\n") {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(count) => bytes.extend_from_slice(&chunk[..count]),
            }
        }
        bytes
    }

    fn start_mode(address: std::net::SocketAddr, proxy: bool) -> Self {
        use std::io::Write;
        use std::sync::{Arc, atomic::AtomicBool, atomic::AtomicUsize};
        let server =
            std::net::TcpListener::bind(address).expect("restart the failed local service");
        let address = server.local_addr().unwrap();
        server.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicUsize::new(0));
        let blocked_requests = Arc::new(AtomicUsize::new(0));
        let stopped = stop.clone();
        let observed = requests.clone();
        let blocked = blocked_requests.clone();
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = server.accept() else {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = Self::read_headers(&mut stream);
                let tunnel = proxy
                    && (bytes.starts_with(b"CONNECT preview.minicon.invalid:80 ")
                        || bytes.starts_with(b"CONNECT preview.minicon.invalid "));
                if tunnel {
                    if stream
                        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                        .is_err()
                    {
                        continue;
                    }
                    bytes = Self::read_headers(&mut stream);
                }
                if !bytes.ends_with(b"\r\n\r\n") {
                    continue;
                }
                if proxy
                    && !(bytes.starts_with(b"GET http://preview.minicon.invalid/")
                        || (tunnel && bytes.starts_with(b"GET /")))
                {
                    let _ = stream.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    continue;
                }
                observed.fetch_add(1, Ordering::Relaxed);
                if bytes.starts_with(b"GET /blocked ") {
                    blocked.fetch_add(1, Ordering::Relaxed);
                }
                let body = if proxy {
                    "<title>Proxy preview</title><h1>Proxy preview</h1>"
                } else if bytes.starts_with(b"GET /next ") {
                    "<title>Next preview</title><h1>Next preview</h1>"
                } else {
                    "<title>Local preview ready</title><h1>Local preview ready</h1>"
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self {
            address,
            requests,
            blocked_requests,
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for LocalPreviewServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[test]
#[ignore = "requires a native desktop/WebView runtime; run the explicit local-preview court"]
fn native_browser_visible_entry_retry_and_terminal_draft() {
    let host = BrowserGui::spawn(false, None, None);
    let terminal_id = host.cli(&["list-tabs"])["tabs"][0]["id"].clone();
    let draft = "keep this terminal preview draft";
    host.cli(&["send-ui-keys", "Ctrl+Shift+I"]);
    host.cli(&["send-ui-ime", "commit", draft]);
    assert_eq!(host.cli(&["ui-snapshot"])["composer_text"], draft);
    let browser_id = host.open_browser_from_menu()["id"].clone();
    assert_eq!(host.open_browser_from_menu()["id"], browser_id);

    let refused = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = refused.local_addr().unwrap();
    drop(refused);
    let failed_url = format!("http://{address}/unavailable");
    host.cli(&["send-ui-keys", "Ctrl+l"]);
    host.cli(&["send-ui-ime", "commit", &failed_url]);
    host.cli(&["send-ui-keys", "Enter"]);
    let failed = host.wait_browser(&failed_url, "unavailable", None);
    assert!(
        failed["browser"]["reason"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty())
    );
    host.click_geometry("browser_edit_address");
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["address_focused"],
        true
    );
    assert_eq!(host.browser()["browser"]["url"], failed_url);
    host.cli(&["send-ui-keys", "Escape"]);

    let service = LocalPreviewServer::start(address);
    let before_retry = service.requests.load(Ordering::Relaxed);
    host.click_geometry("browser_retry");
    let recovered = host.wait_browser(&failed_url, "ready", Some("Local preview ready"));
    assert!(
        service.requests.load(Ordering::Relaxed) > before_retry,
        "Retry must request the restarted service"
    );
    assert_eq!(
        recovered["id"], browser_id,
        "Retry must keep the same Browser root"
    );
    assert!(
        recovered["browser"]["reason"].is_null(),
        "ready must clear stale failure state"
    );

    let next_url = format!("http://{address}/next");
    host.cli(&["send-ui-keys", "Ctrl+l"]);
    host.cli(&["send-ui-ime", "commit", &next_url]);
    host.cli(&["send-ui-keys", "Enter"]);
    host.wait_browser(&next_url, "ready", Some("Next preview"));
    host.cli(&["select-tab", "--target", tab_id(&terminal_id)]);
    let terminal = host.cli(&["ui-snapshot"]);
    assert_eq!(terminal["active"], terminal_id);
    assert_eq!(
        terminal["composer_text"], draft,
        "preview and recovery must preserve the terminal draft"
    );
    assert_eq!(host.open_browser_from_menu()["id"], browser_id);
    assert_eq!(host.browser()["browser"]["url"], next_url);
}

#[test]
#[ignore = "requires a native desktop/WebView runtime; run the explicit visible proxy court"]
fn native_browser_visible_proxy_apply_clear_and_invalid_block() {
    let host = BrowserGui::spawn(false, None, None);
    let browser_id = host.open_browser_from_menu()["id"].clone();
    let origin = LocalPreviewServer::start("127.0.0.1:0".parse().unwrap());
    let direct_url = format!("http://{}/direct", origin.address);
    host.cli(&["send-ui-keys", "Ctrl+l"]);
    host.cli(&["send-ui-ime", "commit", &direct_url]);
    host.cli(&["send-ui-keys", "Enter"]);
    host.wait_browser(&direct_url, "ready", Some("Local preview ready"));

    let proxy = LocalPreviewServer::start_proxy();
    host.apply_proxy(&format!("http://{}", proxy.address));
    let proxy_url = "http://preview.minicon.invalid/page";
    host.cli(&["send-ui-keys", "Ctrl+l"]);
    host.cli(&["send-ui-ime", "commit", proxy_url]);
    host.cli(&["send-ui-keys", "Enter"]);
    host.wait_browser(proxy_url, "ready", Some("Proxy preview"));
    assert!(
        proxy.requests.load(Ordering::Relaxed) > 0,
        "visible proxy input must affect actual native requests"
    );
    assert_eq!(host.browser()["id"], browser_id);

    host.cli(&["send-ui-keys", "Ctrl+l"]);
    host.cli(&["send-ui-ime", "commit", &direct_url]);
    host.cli(&["send-ui-keys", "Enter"]);
    let before_clear = origin.requests.load(Ordering::Relaxed);
    host.apply_proxy("");
    host.wait_browser(&direct_url, "ready", Some("Local preview ready"));
    assert!(
        origin.requests.load(Ordering::Relaxed) > before_clear,
        "clearing proxy must reload the local origin directly"
    );

    host.apply_proxy("http://127.0.0.1:not-a-port");
    let rejected = host.wait_browser(&direct_url, "unavailable", None);
    assert_eq!(rejected["browser"]["reason"], "browser-proxy-invalid");
    let before_invalid = origin.blocked_requests.load(Ordering::Relaxed);
    let blocked_url = format!("http://{}/blocked", origin.address);
    host.cli(&["send-ui-keys", "Ctrl+l"]);
    host.cli(&["send-ui-ime", "commit", &blocked_url]);
    host.cli(&["send-ui-keys", "Enter"]);
    let rejected = host.wait_browser(&blocked_url, "unavailable", None);
    assert_eq!(rejected["browser"]["reason"], "browser-proxy-invalid");
    host.click_geometry("browser_retry");
    let observation = Instant::now() + Duration::from_millis(500);
    while Instant::now() < observation {
        assert_eq!(
            origin.blocked_requests.load(Ordering::Relaxed),
            before_invalid,
            "invalid proxy leaked direct traffic"
        );
        thread::sleep(Duration::from_millis(10));
    }
    host.click_geometry("browser_edit_address");
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["proxy_focused"],
        true,
        "a proxy error must offer editing the proxy rather than the address"
    );
    host.cli(&["send-ui-keys", "Escape"]);
    host.apply_proxy("");
    host.wait_browser(&blocked_url, "ready", Some("Local preview ready"));
    assert_eq!(host.browser()["id"], browser_id);
}

#[test]
#[ignore = "requires a native desktop/WebView runtime; run the explicit settings proxy court"]
fn native_browser_settings_proxy_candidates_persist_select_and_delete() {
    let proxy = LocalPreviewServer::start_proxy();
    let endpoint = format!("http://{}", proxy.address);
    let mut host = BrowserGui::spawn(false, None, None);
    assert_eq!(
        host.open_web_settings()["geometry"]["web_proxy_options_count"],
        0
    );
    host.click_geometry("web_proxy_input");
    host.cli(&["send-ui-ime", "commit", &endpoint]);
    // A directory at the public configuration file path forces the atomic
    // replacement to fail, without permissions or platform-specific hooks.
    let blocked_config = host.config_root.join("minicon.json");
    fs::create_dir_all(&blocked_config).unwrap();
    host.click_geometry("web_proxy_add");
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["web_proxy_options_count"],
        0,
        "a failed settings save must retain the previous list"
    );
    assert!(blocked_config.is_dir());
    fs::remove_dir(&blocked_config).unwrap();
    host.click_geometry("web_proxy_add");
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["web_proxy_options_count"],
        1
    );
    host.cli(&["send-ui-keys", "Escape"]);
    assert_eq!(host.cli(&["ui-snapshot"])["settings_open"], false);
    host.open_browser_from_menu();
    host.click_geometry("browser_proxy_dropdown");
    host.click_geometry_row("browser_proxy_option_rows", 1);
    let url = "http://preview.minicon.invalid/page";
    host.cli(&["send-ui-keys", "Ctrl+l"]);
    host.cli(&["send-ui-ime", "commit", url]);
    host.cli(&["send-ui-keys", "Enter"]);
    host.wait_browser(url, "ready", Some("Proxy preview"));
    let before_restart = proxy.requests.load(Ordering::Relaxed);
    assert!(
        before_restart > 0,
        "newly added candidate must be immediately usable"
    );
    let config_root = host.config_root.clone();
    host.close_and_wait();
    drop(host);

    let mut host = BrowserGui::spawn_with_config(false, None, None, Some(config_root.clone()));
    assert_eq!(
        host.open_web_settings()["geometry"]["web_proxy_options_count"],
        1,
        "saved candidates must survive a new process using the same config"
    );
    host.cli(&["send-ui-keys", "Escape"]);
    host.open_browser_from_menu();
    host.click_geometry("browser_proxy_dropdown");
    let geometry = host.cli(&["ui-snapshot"])["geometry"].clone();
    assert_eq!(geometry["browser_proxy_menu_open"], true);
    assert_eq!(
        geometry["browser_proxy_option_rows"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    host.click_geometry_row("browser_proxy_option_rows", 1);
    host.cli(&["send-ui-keys", "Ctrl+l"]);
    host.cli(&["send-ui-ime", "commit", url]);
    host.cli(&["send-ui-keys", "Enter"]);
    host.wait_browser(url, "ready", Some("Proxy preview"));
    assert!(
        proxy.requests.load(Ordering::Relaxed) > before_restart,
        "selecting the persisted candidate must configure actual native requests"
    );
    assert_eq!(
        host.open_web_settings()["geometry"]["web_proxy_options_count"],
        1
    );
    host.click_geometry_row("web_proxy_remove", 0);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["web_proxy_options_count"],
        0
    );
    host.cli(&["send-ui-keys", "Escape"]);
    host.close_and_wait();
    drop(host);

    let mut host = BrowserGui::spawn_with_config(false, None, None, Some(config_root));
    assert_eq!(
        host.open_web_settings()["geometry"]["web_proxy_options_count"],
        0,
        "deleting a candidate must persist across restart"
    );
    host.cli(&["send-ui-keys", "Escape"]);
    host.open_browser_from_menu();
    host.click_geometry("browser_proxy_dropdown");
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["browser_proxy_option_rows"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "deleted candidate must not remain selectable"
    );
    host.cli(&["send-ui-keys", "Escape"]);
    host.close_and_wait();
}

#[test]
#[cfg(windows)]
fn missing_webview2_runtime_is_unavailable_and_keeps_terminal_usable() {
    let mut host = BrowserGui::spawn(true, None, None);
    host.cli(&["send-ui-keys", "Ctrl+Shift+O"]);
    let browser = host.browser();
    assert_eq!(browser["browser"]["load"], "unavailable", "{browser}");
    assert_eq!(browser["browser"]["reason"], "browser-native-create-failed");
    assert_eq!(browser["active"], true);
    assert_eq!(host.cli(&["list-tabs"])["tabs"][0]["browser"], Value::Null);
    let browser_id = tab_id(&browser["id"]);
    host.cli(&["send-ui-keys", "Ctrl+Shift+O"]);
    assert_eq!(host.browser()["id"], browser_id);
    host.cli(&["select-tab", "--target", "@1"]);
    host.cli(&[
        "send-text",
        "--target",
        "@1",
        "echo BROWSER_RUNTIME_^RECOVERED",
    ]);
    host.cli(&["send-keys", "--target", "@1", "Enter"]);
    host.cli(&[
        "wait-text",
        "--target",
        "@1",
        "--timeout-ms",
        "10000",
        "BROWSER_RUNTIME_RECOVERED",
    ]);
    assert!(host.gui.child.try_wait().unwrap().is_none());
    host.cli(&["close-tab", "--target", browser_id]);
    assert_eq!(
        host.cli(&["list-tabs"])["tabs"].as_array().unwrap().len(),
        1
    );
}

#[test]
#[cfg(windows)]
#[ignore = "requires WebView2 runtime; run the explicit native Windows browser court"]
fn native_webview2_navigation_uses_the_browser_proxy() {
    for https_override in [false, true] {
        proxy_navigation(https_override);
    }
}

#[cfg(windows)]
fn proxy_navigation(https_override: bool) {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    let proxy = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    proxy.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", proxy.local_addr().unwrap());
    let (send, receive) = mpsc::channel();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Ok((mut stream, _)) = proxy.accept() {
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = [0; 8192];
                let count = stream.read(&mut bytes).unwrap_or(0);
                let request = String::from_utf8_lossy(&bytes[..count]);
                // The proxy refuses every tunnel: the court never contacts the provider.
                let _ = stream.write_all(
                    b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
                if request.starts_with("CONNECT platform.deepseek.com:443 ") {
                    send.send(()).unwrap();
                    return;
                }
            } else {
                thread::sleep(Duration::from_millis(10));
            }
        }
    });
    let host = if https_override {
        BrowserGui::spawn(false, Some("http://127.0.0.1:1"), Some(&endpoint))
    } else {
        BrowserGui::spawn(false, Some(&endpoint), None)
    };
    host.cli(&["send-ui-keys", "Ctrl+Shift+D"]);
    let browser = host.browser();
    assert_eq!(browser["browser"]["load"], "loading", "{browser}");
    let observed = receive.recv_timeout(Duration::from_secs(22));
    server.join().unwrap();
    assert!(
        observed.is_ok(),
        "WebView2 navigation did not reach the configured local proxy"
    );
}

#[test]
#[ignore = "requires a native desktop/WebView runtime; run the explicit browser court"]
fn native_browser_address_redirect_history_and_failure() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let server = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    server.set_nonblocking(true).unwrap();
    let origin = format!("http://{}", server.local_addr().unwrap());
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let worker = thread::spawn(move || {
        while !stopped.load(Ordering::Relaxed) {
            if let Ok((mut stream, _)) = server.accept() {
                // Winsock accepts inherit the listener's nonblocking mode.
                // Wait for a complete request before replying or closing:
                // otherwise Chromium observes CONNECTION_ABORTED under load.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut chunk = [0; 1024];
                while bytes.len() < 8192 && !bytes.ends_with(b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => bytes.extend_from_slice(&chunk[..count]),
                    }
                }
                if !bytes.ends_with(b"\r\n\r\n") {
                    continue;
                }
                let request = String::from_utf8_lossy(&bytes);
                if request.starts_with("GET /redirect ") {
                    let _ = stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /second\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                } else {
                    let body = if request.starts_with("GET /second ") {
                        "<title>Second page</title><h1>Second page</h1>"
                    } else {
                        "<title>First page</title><a href='/redirect'>Next page</a>"
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
            } else {
                thread::sleep(Duration::from_millis(10));
            }
        }
    });
    let host = BrowserGui::spawn(false, None, None);
    host.cli(&["send-ui-keys", "Ctrl+Shift+O"]);
    let navigate = |url: &str| {
        host.cli(&["send-ui-keys", "Ctrl+l"]);
        host.cli(&["send-ui-ime", "commit", url]);
        host.cli(&["send-ui-keys", "Enter"]);
    };
    let wait = |url: &str, load: &str, title: Option<&str>| {
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            let row = host.browser();
            if row["browser"]["url"] == url
                && row["browser"]["load"] == load
                && title.is_none_or(|title| row["browser"]["title"] == title)
            {
                return row;
            }
            assert!(
                Instant::now() < deadline,
                "expected {load} {url}, got {row}"
            );
            thread::sleep(Duration::from_millis(40));
        }
    };
    navigate(&format!("{origin}/first"));
    let first = wait(&format!("{origin}/first"), "ready", Some("First page"));
    assert_eq!(first["browser"]["title"], "First page");
    // Clicking an already-edited address must place the caret in the draft,
    // rather than resetting it to the loaded page. Submit the resulting URL
    // so this public black box observes actual navigation, not editor internals.
    host.cli(&["send-ui-keys", "Ctrl+l"]);
    host.cli(&["send-ui-ime", "commit", &format!("{origin}/draft")]);
    let geometry = host.cli(&["ui-snapshot"]);
    let address = &geometry["geometry"]["browser_address"];
    let scale = geometry["geometry"]["scale_permille"].as_u64().unwrap() as f64 / 1000.0;
    let x = ((address["x"].as_u64().unwrap() + address["width"].as_u64().unwrap() - 4) as f64
        / scale)
        .round()
        .to_string();
    let y = ((address["y"].as_u64().unwrap() + address["height"].as_u64().unwrap() / 2) as f64
        / scale)
        .round()
        .to_string();
    host.cli(&[
        "send-ui-mouse",
        "--action",
        "click",
        "--button",
        "left",
        "--column",
        &x,
        "--row",
        &y,
    ]);
    host.cli(&["send-ui-ime", "commit", "?edited=1"]);
    host.cli(&["send-ui-keys", "Enter"]);
    wait(
        &format!("{origin}/draft?edited=1"),
        "ready",
        Some("First page"),
    );
    navigate(&format!("{origin}/first"));
    wait(&format!("{origin}/first"), "ready", Some("First page"));
    navigate(&format!("{origin}/redirect"));
    let second = wait(&format!("{origin}/second"), "ready", Some("Second page"));
    assert_eq!(second["browser"]["title"], "Second page");
    assert_eq!(second["browser"]["can_back"], true);
    let snapshot = host.cli(&["ui-snapshot"]);
    let toolbar = &snapshot["geometry"]["browser_toolbar"];
    let scale = snapshot["geometry"]["scale_permille"].as_u64().unwrap() as f64 / 1000.0;
    let x = (((toolbar["x"].as_u64().unwrap() as f64 / scale) + 16.0).round() as u32).to_string();
    host.cli(&[
        "send-ui-mouse",
        "--action",
        "click",
        "--button",
        "left",
        "--column",
        &x,
        "--row",
        "20",
    ]);
    wait(&format!("{origin}/first"), "ready", Some("First page"));
    assert_eq!(host.browser()["browser"]["can_forward"], true);
    host.cli(&["send-ui-keys", "Ctrl+Shift+["]);
    host.cli(&["send-ui-keys", "Ctrl+Shift+O"]);
    assert_eq!(host.browser()["browser"]["url"], format!("{origin}/first"));
    let port = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let failed = format!("http://{}/unavailable", port.local_addr().unwrap());
    drop(port);
    navigate(&failed);
    let failure = wait(&failed, "unavailable", None);
    assert!(
        failure["browser"]["reason"]
            .as_str()
            .unwrap()
            .starts_with("browser-navigation-")
    );
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap();
}
