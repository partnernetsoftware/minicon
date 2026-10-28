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
        Self::spawn_with_console_command(
            missing_runtime,
            http_proxy,
            https_proxy,
            config_root,
            false,
        )
    }

    #[cfg(windows)]
    fn spawn_with_exiting_console() -> Self {
        Self::spawn_with_console_command(false, None, None, None, true)
    }

    fn spawn_with_console_command(
        missing_runtime: bool,
        http_proxy: Option<&str>,
        https_proxy: Option<&str>,
        config_root: Option<PathBuf>,
        exiting_console: bool,
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
        if exiting_console {
            command.args(["cmd.exe", "/d", "/c", "exit", "0"]);
        } else {
            command.args(["cmd.exe", "/d", "/k"]);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.arg("/bin/sh").process_group(0);
            if exiting_console {
                command.args(["-c", "exit 0"]);
            }
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
            .find(|tab| tab["kind"] == "browser" && tab["active"] == true)
            .expect("active browser tab must exist")
            .clone()
    }

    fn browser_by_id(&self, id: &str) -> Value {
        self.cli(&["list-tabs"])["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tab| tab["id"] == id && tab["kind"] == "browser")
            .expect("browser ID must exist")
            .clone()
    }

    #[cfg(windows)]
    fn capture_home_client(&self, name: &str) -> PathBuf {
        use minicon_platform::{
            process_window,
            screenshot::{NativeCaptureArea, ScreenshotWindowHandle, capture_native_window_png},
        };
        let facts = process_window::facts(self.gui.child.id());
        assert!(facts.present && facts.window_id != 0, "owned Home window");
        let geometry = self.cli(&["ui-snapshot"])["geometry"].clone();
        assert_eq!(geometry["home_active"], true);
        let handle = unsafe { ScreenshotWindowHandle::from_raw(facts.window_id as isize) }
            .expect("owned native handle");
        let path = self.gui.screenshot.parent().unwrap().join(name);
        capture_native_window_png(
            handle,
            &path,
            NativeCaptureArea::Client {
                left: 0,
                top: 0,
                width: geometry["frame"]["width"]
                    .as_i64()
                    .unwrap_or_else(|| panic!("Home frame width missing: {geometry}"))
                    as i32,
                height: geometry["frame"]["height"]
                    .as_i64()
                    .unwrap_or_else(|| panic!("Home frame height missing: {geometry}"))
                    as i32,
            },
        )
        .expect("capture Home client");
        path
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

    fn click_browser_back(&self) {
        let snapshot = self.cli(&["ui-snapshot"]);
        let geometry = &snapshot["geometry"];
        // 0.2.22 publishes its toolbar rather than individual navigation
        // buttons. Its first 34-DIP column is Back; keep the same regression
        // runnable against that exact released baseline.
        let scale = geometry["scale_permille"].as_u64().unwrap() as f64 / 1000.0;
        let back = serde_json::json!({
            "x": geometry["browser_toolbar"]["x"],
            "y": geometry["browser_toolbar"]["y"],
            "width": (34.0 * scale) as u64,
            "height": (44.0 * scale) as u64,
        });
        self.click_rect(&back, "browser_back", geometry);
    }

    fn click_rect(&self, rect: &Value, name: &str, geometry: &Value) {
        let width = rect["width"]
            .as_u64()
            .unwrap_or_else(|| panic!("missing published hit-target width: {name}: {geometry}"));
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

    fn open_browser_directly(&self) -> Value {
        let before = self.cli(&["list-tabs"])["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|tab| tab["kind"] == "browser")
            .count();
        self.click_geometry("new_web");
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
            before + 1,
            "the visible Web action must create an independent Browser root"
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

#[test]
fn host_composer_scrollbar_pages_and_drags_without_changing_the_draft() {
    let mut host = BrowserGui::spawn(true, None, None);
    host.cli(&["send-ui-keys", "Ctrl+Shift+I"]);
    let draft = (0..24)
        .map(|line| format!("draft-{line:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    host.cli(&["send-ui-ime", "commit", &draft]);
    let before = host.cli(&["ui-snapshot"]);
    assert_eq!(before["composer_text"], draft);
    let first = before["geometry"]["composer_visible_first_line"]
        .as_u64()
        .expect("published composer scroll position");
    assert!(first > 0, "multiline draft must scroll: {before}");
    let scale = before["geometry"]["scale_permille"].as_u64().unwrap() as f64 / 1000.0;
    let track = &before["geometry"]["composer_scrollbar"];
    let x = ((track["x"].as_u64().unwrap() + track["width"].as_u64().unwrap() / 2) as f64 / scale)
        .round()
        .to_string();
    let top = ((track["y"].as_u64().unwrap() + 2) as f64 / scale)
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
        &top,
    ]);
    let paged = host.cli(&["ui-snapshot"]);
    assert!(
        paged["geometry"]["composer_visible_first_line"]
            .as_u64()
            .unwrap()
            < first,
        "track click must page toward the draft head: {paged}"
    );
    assert_eq!(paged["composer_text"], draft);
    let thumb = &paged["geometry"]["composer_scrollbar_thumb"];
    let thumb_y = ((thumb["y"].as_u64().unwrap() + thumb["height"].as_u64().unwrap() / 2) as f64
        / scale)
        .round()
        .to_string();
    let bottom = ((track["y"].as_u64().unwrap() + track["height"].as_u64().unwrap() - 2) as f64
        / scale)
        .round()
        .to_string();
    for (action, button, row) in [
        ("press", "left", thumb_y.as_str()),
        ("move", "none", bottom.as_str()),
        ("release", "left", bottom.as_str()),
    ] {
        host.cli(&[
            "send-ui-mouse",
            "--action",
            action,
            "--button",
            button,
            "--column",
            &x,
            "--row",
            row,
        ]);
    }
    let dragged = host.cli(&["ui-snapshot"]);
    assert_eq!(
        dragged["geometry"]["composer_visible_first_line"],
        before["geometry"]["composer_visible_first_line"],
        "drag to track bottom must restore the draft tail: {dragged}"
    );
    assert_eq!(dragged["composer_text"], draft);
    host.close_and_wait();
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
    let browser_id = host.open_browser_directly()["id"].clone();
    let second_browser_id = host.open_browser_directly()["id"].clone();
    assert_ne!(second_browser_id, browser_id);
    host.cli(&["select-tab", "--target", tab_id(&browser_id)]);

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
    host.cli(&["select-tab", "--target", tab_id(&browser_id)]);
    assert_eq!(host.browser()["id"], browser_id);
    assert_eq!(host.browser()["browser"]["url"], next_url);
}

#[test]
#[ignore = "requires a native desktop/WebView runtime; run the explicit two-Web court"]
fn native_two_web_tabs_keep_independent_pages_when_switched_and_closed() {
    let mut host = BrowserGui::spawn(false, None, None);
    let service = LocalPreviewServer::start("127.0.0.1:0".parse().unwrap());
    let first_url = format!("http://{}/first", service.address);
    let second_url = format!("http://{}/next", service.address);
    let navigate = |url: &str| {
        host.cli(&["send-ui-keys", "Ctrl+l"]);
        host.cli(&["send-ui-ime", "commit", url]);
        host.cli(&["send-ui-keys", "Enter"]);
    };

    let first_id = host.open_browser_directly()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    navigate(&first_url);
    host.wait_browser(&first_url, "ready", Some("Local preview ready"));
    navigate(&second_url);
    let first_history = host.wait_browser(&second_url, "ready", Some("Next preview"));
    assert_eq!(first_history["browser"]["can_back"], true);
    host.click_browser_back();
    host.wait_browser(&first_url, "ready", Some("Local preview ready"));
    let second_id = host.open_browser_directly()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(first_id, second_id);
    navigate(&second_url);
    host.wait_browser(&second_url, "ready", Some("Next preview"));
    assert_eq!(host.browser_by_id(&first_id)["browser"]["url"], first_url);
    assert_eq!(
        host.browser_by_id(&first_id)["browser"]["can_forward"],
        true
    );
    assert_eq!(
        host.browser_by_id(&second_id)["browser"]["can_forward"],
        false
    );
    assert_eq!(
        host.browser_by_id(&first_id)["browser"]["title"],
        "Local preview ready"
    );

    host.cli(&["select-tab", "--target", &first_id]);
    assert_eq!(host.browser()["browser"]["url"], first_url);
    assert_eq!(host.browser()["browser"]["title"], "Local preview ready");
    host.cli(&["select-tab", "--target", &second_id]);
    assert_eq!(host.browser()["browser"]["url"], second_url);
    assert_eq!(host.browser()["browser"]["title"], "Next preview");

    host.apply_proxy("http://127.0.0.1:not-a-port");
    let rejected = host.wait_browser(&second_url, "unavailable", None);
    assert_eq!(rejected["browser"]["reason"], "browser-proxy-invalid");
    host.cli(&["select-tab", "--target", &first_id]);
    assert_eq!(host.browser()["browser"]["url"], first_url);
    assert_eq!(host.browser()["browser"]["load"], "ready");
    let before_reload = service.requests.load(Ordering::Relaxed);
    navigate(&first_url);
    let deadline = Instant::now() + Duration::from_secs(10);
    while service.requests.load(Ordering::Relaxed) <= before_reload {
        assert!(Instant::now() < deadline, "first Web lost its direct route");
        thread::sleep(Duration::from_millis(20));
    }
    host.cli(&["select-tab", "--target", &second_id]);
    assert_eq!(host.browser()["browser"]["reason"], "browser-proxy-invalid");
    host.apply_proxy("");
    host.wait_browser(&second_url, "ready", Some("Next preview"));

    host.cli(&["close-tab", "--target", &first_id]);
    assert_eq!(host.browser()["id"], second_id);
    assert_eq!(host.browser()["browser"]["url"], second_url);
    let third_id = host.open_browser_directly()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(third_id, first_id);
    assert_ne!(third_id, second_id);
    navigate(&first_url);
    host.wait_browser(&first_url, "ready", Some("Local preview ready"));
    assert_eq!(host.browser_by_id(&second_id)["browser"]["url"], second_url);
    host.close_and_wait();
}

#[test]
#[ignore = "requires a native desktop/WebView runtime; run the explicit visible proxy court"]
fn native_browser_proxy_menu_outside_click_dismisses_without_navigation() {
    let mut host = BrowserGui::spawn(false, None, None);
    host.open_browser_directly();
    let origin = LocalPreviewServer::start("127.0.0.1:0".parse().unwrap());
    let first = format!("http://{}/first", origin.address);
    let second = format!("http://{}/second", origin.address);
    for url in [&first, &second] {
        host.cli(&["send-ui-keys", "Ctrl+l"]);
        host.cli(&["send-ui-ime", "commit", url]);
        host.cli(&["send-ui-keys", "Enter"]);
        host.wait_browser(url, "ready", None);
    }
    host.click_geometry("browser_proxy_dropdown");
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["browser_proxy_menu_open"],
        true
    );
    host.click_browser_back();
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["browser_proxy_menu_open"],
        false
    );
    // A fall-through navigation may finish after the input reply. Observe the
    // real engine for a bounded interval instead of trusting one immediate frame.
    let deadline = Instant::now() + Duration::from_millis(600);
    while Instant::now() < deadline {
        assert_eq!(
            host.browser()["browser"]["url"],
            second,
            "an outside Back click must dismiss only, not navigate behind the menu"
        );
        thread::sleep(Duration::from_millis(20));
    }
    host.click_browser_back();
    host.wait_browser(&first, "ready", None);
    host.close_and_wait();
}

#[test]
#[ignore = "requires a native desktop/WebView runtime; run the explicit visible proxy court"]
fn native_browser_visible_proxy_apply_clear_and_invalid_block() {
    let host = BrowserGui::spawn(false, None, None);
    let browser_id = host.open_browser_directly()["id"].clone();
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
#[ignore = "requires a native desktop; run the explicit @0 home court"]
fn native_home_is_a_fixed_destination_and_restores_real_tabs() {
    let mut host = BrowserGui::spawn(false, None, None);
    let before = host.cli(&["list-tabs"]);
    let terminal_id = before["tabs"][0]["id"].as_str().unwrap().to_owned();
    host.click_geometry("home");
    assert_eq!(host.cli(&["ui-snapshot"])["geometry"]["home_active"], true);
    assert_eq!(
        host.cli(&["list-tabs"])["tabs"].as_array().unwrap().len(),
        1
    );
    assert_eq!(host.cli(&["list-tabs"])["tabs"][0]["id"], terminal_id);
    host.cli(&["select-tab", "--target", &terminal_id]);
    assert_eq!(host.cli(&["ui-snapshot"])["geometry"]["home_active"], false);

    host.open_browser_directly();
    let browser = host.browser();
    let browser_id = browser["id"].as_str().unwrap().to_owned();
    host.click_geometry("home");
    assert_eq!(host.cli(&["ui-snapshot"])["geometry"]["home_active"], true);
    assert!(
        host.cli(&["list-tabs"])["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tab| tab["id"] == browser_id)
    );
    host.cli(&["select-tab", "--target", &browser_id]);
    assert_eq!(host.cli(&["ui-snapshot"])["geometry"]["home_active"], false);
    assert_eq!(host.browser()["id"], browser_id);
    host.cli(&["close-tab", "--target", &browser_id]);
    host.cli(&["close-tab", "--target", &terminal_id]);
    assert_eq!(host.cli(&["ui-snapshot"])["geometry"]["home_active"], true);
    assert!(
        host.cli(&["list-tabs"])["tabs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    host.click_geometry("home_new_terminal");
    let new_terminal = host.cli(&["list-tabs"]);
    let new_terminal_id = new_terminal["tabs"][0]["id"].as_str().unwrap().to_owned();
    assert_ne!(new_terminal_id, terminal_id);
    host.cli(&["close-tab", "--target", &new_terminal_id]);
    assert_eq!(host.cli(&["ui-snapshot"])["geometry"]["home_active"], true);
    host.click_geometry("home_new_web");
    let new_web = host.browser();
    assert_ne!(new_web["id"], browser_id);
    host.close_and_wait();
}

#[test]
#[ignore = "requires a native desktop; run the explicit @0 fleet court"]
fn native_home_fleet_rows_target_exact_tab_ids() {
    let mut host = BrowserGui::spawn(false, None, None);
    let first = host.cli(&["list-tabs"])["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let second = host.cli(&["new-tab"])["id"].as_str().unwrap().to_owned();
    let third = host.cli(&["new-tab"])["id"].as_str().unwrap().to_owned();
    host.click_geometry("home");
    let snapshot = host.cli(&["ui-snapshot"]);
    let geometry = &snapshot["geometry"];
    assert_eq!(geometry["home_active"], true);
    assert_eq!(geometry["home_tab_count"], 3);
    assert_eq!(
        geometry["home_list_row_ids"],
        serde_json::json!([&first, &second, &third])
    );
    host.click_geometry_row("home_list_row_rects", 0);
    let inspected = host.cli(&["ui-snapshot"]);
    assert_eq!(inspected["geometry"]["home_active"], true);
    assert_eq!(inspected["geometry"]["home_selected_id"], first);
    assert_eq!(inspected["geometry"]["home_focus_enabled"], true);
    assert_eq!(host.cli(&["list-tabs"])["tabs"][2]["active"], true);
    host.click_geometry("home_focus_tab");
    assert_eq!(host.cli(&["list-tabs"])["tabs"][0]["active"], true);
    assert_eq!(host.cli(&["ui-snapshot"])["geometry"]["home_active"], false);

    host.click_geometry("home");
    host.click_geometry_row("home_list_row_rects", 1);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["home_selected_id"],
        second
    );
    host.cli(&["close-tab", "--target", &second]);
    let stale = host.cli(&["ui-snapshot"]);
    assert_eq!(stale["geometry"]["home_active"], true);
    assert_eq!(stale["geometry"]["home_selected_id"], second);
    assert_eq!(stale["geometry"]["home_selected_stale"], true);
    assert_eq!(stale["geometry"]["home_focus_enabled"], false);
    host.click_geometry("home_focus_tab");
    assert_eq!(host.cli(&["ui-snapshot"])["geometry"]["home_active"], true);
    assert_eq!(host.cli(&["list-tabs"])["tabs"][1]["id"], third);
    host.click_geometry_row("home_list_row_rects", 1);
    host.cli(&["send-ui-keys", "Escape"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["home_selected_id"],
        Value::Null
    );
    host.click_geometry_row("home_list_row_rects", 1);
    host.cli(&["send-ui-keys", "Enter"]);
    assert_eq!(host.cli(&["list-tabs"])["tabs"][1]["id"], third);
    assert_eq!(host.cli(&["list-tabs"])["tabs"][1]["active"], true);
    host.close_and_wait();
}

#[cfg(windows)]
#[test]
#[ignore = "requires a native desktop; run the explicit Home Visual V0 court"]
fn native_home_visual_preview_keeps_exact_ids_and_list_return() {
    let mut host = BrowserGui::spawn(false, None, None);
    let first = host.cli(&["list-tabs"])["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let second = host.cli(&["new-tab"])["id"].as_str().unwrap().to_owned();
    let web = host.open_browser_directly()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let original = host.cli(&["list-tabs"])["tabs"].clone();
    host.click_geometry("home");
    let list = host.cli(&["ui-snapshot"]);
    assert_eq!(list["geometry"]["home_mode"], "list");
    assert_eq!(
        list["geometry"]["home_list_row_ids"],
        serde_json::json!([first, second, web])
    );
    host.click_geometry("home_mode_action");
    let visual = host.cli(&["ui-snapshot"]);
    assert_eq!(visual["geometry"]["home_mode"], "visual-preview");
    assert_eq!(
        visual["geometry"]["home_list_row_ids"],
        list["geometry"]["home_list_row_ids"]
    );
    let cards = visual["geometry"]["home_list_row_rects"]
        .as_array()
        .unwrap();
    assert_eq!(cards.len(), 3);
    assert_eq!(cards[0]["y"], cards[1]["y"]);
    assert!(cards[1]["x"].as_u64().unwrap() > cards[0]["x"].as_u64().unwrap());
    println!(
        "Home Visual V0 wide: {}",
        host.capture_home_client("home-visual-wide.png").display()
    );
    host.click_geometry("home_sort_action");
    let sorted = host.cli(&["ui-snapshot"]);
    assert_eq!(sorted["geometry"]["home_sort"], "label");
    assert_eq!(sorted["geometry"]["home_list_row_ids"][0], web);
    host.click_geometry("home_filter_action");
    let filtered = host.cli(&["ui-snapshot"]);
    assert_eq!(filtered["geometry"]["home_mode"], "visual-preview");
    assert_eq!(filtered["geometry"]["home_kind_filter"], "console");
    assert_eq!(
        filtered["geometry"]["home_list_row_ids"],
        serde_json::json!([first, second])
    );
    host.click_geometry_row("home_list_row_rects", 1);
    let selected = host.cli(&["ui-snapshot"]);
    assert_eq!(selected["geometry"]["home_selected_id"], second);
    assert_eq!(selected["geometry"]["home_active"], true);
    host.click_geometry("home_focus_tab");
    assert_eq!(host.cli(&["list-tabs"])["tabs"][1]["active"], true);
    host.click_geometry("home");
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["home_mode"],
        "visual-preview"
    );
    host.cli(&["resize-window", "--width", "500", "--height", "650"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    let narrow = loop {
        let value = host.cli(&["ui-snapshot"]);
        if value["geometry"]["frame"]["width"] == 500 {
            break value;
        }
        assert!(Instant::now() < deadline, "narrow Visual V0 did not settle");
        thread::sleep(Duration::from_millis(25));
    };
    let narrow_cards = narrow["geometry"]["home_list_row_rects"]
        .as_array()
        .unwrap();
    assert!(!narrow_cards.is_empty());
    if narrow_cards.len() > 1 {
        assert!(narrow_cards[1]["y"].as_u64().unwrap() > narrow_cards[0]["y"].as_u64().unwrap());
    }
    thread::sleep(Duration::from_millis(250));
    println!(
        "Home Visual V0 narrow: {}",
        host.capture_home_client("home-visual-narrow.png").display()
    );
    host.click_geometry("home_mode_action");
    let returned = host.cli(&["ui-snapshot"]);
    assert_eq!(returned["geometry"]["home_mode"], "list");
    let after = host.cli(&["list-tabs"]);
    for (index, tab) in after["tabs"].as_array().unwrap().iter().enumerate() {
        assert_eq!(tab["id"], original[index]["id"]);
        assert_eq!(tab["kind"], original[index]["kind"]);
        assert_eq!(tab["parent"], original[index]["parent"]);
    }
    assert_eq!(after["tabs"][2]["browser"], original[2]["browser"]);
    for id in [&first, &second, &web] {
        host.cli(&["close-tab", "--target", id]);
    }
    assert_eq!(host.cli(&["ui-snapshot"])["geometry"]["home_active"], true);
    host.click_geometry("home_mode_action");
    let empty = host.cli(&["ui-snapshot"]);
    assert_eq!(empty["geometry"]["home_mode"], "visual-preview");
    assert_eq!(empty["geometry"]["home_tab_count"], 0);
    assert!(
        empty["geometry"]["home_list_row_ids"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    host.click_geometry("home_new_terminal");
    assert_eq!(
        host.cli(&["list-tabs"])["tabs"].as_array().unwrap().len(),
        1
    );
    host.close_and_wait();
}

#[cfg(windows)]
#[test]
#[ignore = "requires a native desktop; run the explicit Home inspector visual court"]
fn native_home_inspector_captures_wide_and_narrow_layouts() {
    let mut host = BrowserGui::spawn(false, None, None);
    let selected = host.cli(&["new-tab"])["id"].as_str().unwrap().to_owned();
    host.cli(&["new-tab"]);
    host.click_geometry("home");
    host.click_geometry_row("home_list_row_rects", 1);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["home_selected_id"],
        selected
    );
    let wide = host.capture_home_client("home-inspector-wide.png");
    host.cli(&["resize-window", "--width", "800", "--height", "600"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    let four_three = loop {
        let snapshot = host.cli(&["ui-snapshot"]);
        if snapshot["geometry"]["frame"]["width"] == 800
            && snapshot["geometry"]["frame"]["height"] == 600
        {
            break snapshot;
        }
        assert!(Instant::now() < deadline, "4:3 Home resize did not settle");
        thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(four_three["geometry"]["home_selected_id"], selected);
    assert_eq!(four_three["geometry"]["home_inspector_action"], "focus");
    let row = &four_three["geometry"]["home_list_row_rects"][1];
    let inspector = &four_three["geometry"]["home_inspector"];
    assert!(
        row["x"].as_u64().unwrap() + row["width"].as_u64().unwrap()
            <= inspector["x"].as_u64().unwrap()
            || row["y"].as_u64().unwrap() + row["height"].as_u64().unwrap()
                <= inspector["y"].as_u64().unwrap(),
        "4:3 row and inspector must not overlap: row={row}, inspector={inspector}"
    );
    thread::sleep(Duration::from_millis(250));
    let four_three_capture = host.capture_home_client("home-inspector-4-3.png");
    host.click_geometry("home_focus_tab");
    assert_eq!(host.cli(&["list-tabs"])["tabs"][1]["active"], true);
    host.click_geometry("home");
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["home_selected_id"],
        selected
    );
    host.cli(&["resize-window", "--width", "500", "--height", "650"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.cli(&["ui-snapshot"])["geometry"]["frame"]["width"] != 500 {
        assert!(
            Instant::now() < deadline,
            "narrow Home resize did not settle"
        );
        thread::sleep(Duration::from_millis(25));
    }
    thread::sleep(Duration::from_millis(250));
    let narrow = host.capture_home_client("home-inspector-narrow.png");
    println!(
        "Home visual captures: {} | {} | {}",
        wide.display(),
        four_three_capture.display(),
        narrow.display()
    );
    host.close_and_wait();
}

#[cfg(windows)]
#[test]
#[ignore = "requires a native desktop; run the explicit Chinese 4:3 Home court"]
fn native_home_chinese_4_3_keeps_exact_inspect_and_focus_routes() {
    let config_root = minicon_binary()
        .parent()
        .unwrap()
        .join("browser-court")
        .join(unique_suffix())
        .join("zh-config");
    fs::create_dir_all(&config_root).unwrap();
    fs::write(
        config_root.join("minicon.json"),
        br#"{"language":"zh-hans"}"#,
    )
    .unwrap();
    let mut host = BrowserGui::spawn_with_config(false, None, None, Some(config_root));
    let selected = host.cli(&["new-tab"])["id"].as_str().unwrap().to_owned();
    host.cli(&["resize-window", "--width", "800", "--height", "600"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.cli(&["ui-snapshot"])["geometry"]["frame"]["width"] != 800 {
        assert!(Instant::now() < deadline, "Chinese 4:3 Home did not settle");
        thread::sleep(Duration::from_millis(25));
    }
    host.click_geometry("home");
    let home = host.cli(&["ui-snapshot"]);
    assert_eq!(home["ui_language"], "zh-hans");
    assert_eq!(home["geometry"]["home_tab_count"], 2);
    host.click_geometry_row("home_list_row_rects", 1);
    let selected_home = host.cli(&["ui-snapshot"]);
    assert_eq!(selected_home["geometry"]["home_selected_id"], selected);
    assert_eq!(selected_home["geometry"]["home_inspector_action"], "focus");
    thread::sleep(Duration::from_millis(250));
    println!(
        "Home Chinese 4:3 capture: {}",
        host.capture_home_client("home-inspector-4-3-zh-hans.png")
            .display()
    );
    host.click_geometry("home_focus_tab");
    assert_eq!(host.cli(&["list-tabs"])["tabs"][1]["active"], true);
    host.close_and_wait();
}

#[cfg(windows)]
#[test]
#[ignore = "requires a native desktop; run the explicit Home process metrics court"]
fn native_home_process_sample_names_scope_source_and_entry_time() {
    let mut host = BrowserGui::spawn(false, None, None);
    let before = host.cli(&["ui-snapshot"]);
    assert_eq!(before["geometry"]["home_system"]["status"], "unknown");
    assert_eq!(before["geometry"]["home_system"]["reason"], "not-sampled");
    host.click_geometry("home");
    let home = host.cli(&["ui-snapshot"]);
    let sample = &home["geometry"]["home_system"];
    assert_eq!(sample["scope"], "minicon-host-process");
    assert_eq!(sample["source"], "os-process-metrics");
    assert_eq!(sample["status"], "measured");
    assert!(sample["observed_at_unix_ms"].as_u64().unwrap() > 0);
    assert!(sample["cpu_time_ms"].as_u64().is_some());
    assert!(sample["resident_bytes"].as_u64().unwrap() > 0);
    assert_eq!(sample["reason"], Value::Null);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["home_system"],
        *sample
    );
    thread::sleep(Duration::from_millis(30));
    host.click_geometry("home_system_refresh");
    let refreshed = host.cli(&["ui-snapshot"]);
    assert_eq!(refreshed["geometry"]["home_active"], true);
    assert!(
        refreshed["geometry"]["home_system"]["observed_at_unix_ms"]
            .as_u64()
            .unwrap()
            > sample["observed_at_unix_ms"].as_u64().unwrap()
    );
    println!(
        "Home process card capture: {}",
        host.capture_home_client("home-process-card-wide.png")
            .display()
    );
    host.click_geometry_row("home_list_row_rects", 0);
    host.click_geometry("home_focus_tab");
    thread::sleep(Duration::from_millis(30));
    host.click_geometry("home");
    let next = host.cli(&["ui-snapshot"]);
    assert!(
        next["geometry"]["home_system"]["observed_at_unix_ms"]
            .as_u64()
            .unwrap()
            > refreshed["geometry"]["home_system"]["observed_at_unix_ms"]
                .as_u64()
                .unwrap()
    );
    host.cli(&["resize-window", "--width", "500", "--height", "650"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    let narrow = loop {
        let snapshot = host.cli(&["ui-snapshot"]);
        if snapshot["geometry"]["frame"]["width"] == 500 {
            break snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "narrow Home process card did not settle"
        );
        thread::sleep(Duration::from_millis(25));
    };
    let system = &narrow["geometry"]["home_system_refresh"];
    let fleet = &narrow["geometry"]["home_fleet"];
    assert!(
        system["y"].as_u64().unwrap() + system["height"].as_u64().unwrap()
            <= fleet["y"].as_u64().unwrap()
    );
    assert!(
        !narrow["geometry"]["home_list_row_ids"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    thread::sleep(Duration::from_millis(250));
    println!(
        "Home process card narrow capture: {}",
        host.capture_home_client("home-process-card-narrow.png")
            .display()
    );
    host.close_and_wait();
}

#[cfg(windows)]
#[test]
#[ignore = "requires a native desktop; run the explicit Home gateway projection court"]
fn native_home_gateway_is_unknown_until_cached_runtime_and_routes_to_harness() {
    let reservation = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = reservation.local_addr().unwrap().port();
    let root = minicon_binary()
        .parent()
        .unwrap()
        .join("browser-court")
        .join(unique_suffix())
        .join("config");
    harness_synthetic_profile(
        &root,
        port,
        "synthetic-home-gateway-key",
        "synthetic-unused-home-token",
    );
    drop(reservation);
    let mut host = BrowserGui::spawn_with_config(false, None, None, Some(root));
    host.click_geometry("home");
    let unknown = host.cli(&["ui-snapshot"]);
    let gateway = &unknown["geometry"]["home_gateway"];
    assert_eq!(gateway["source"], "harness-runtime-view");
    assert_eq!(gateway["status"], "unknown");
    assert_eq!(gateway["reason"], "not-loaded");
    assert_eq!(gateway["observed_at_unix_ms"], Value::Null);
    assert_eq!(gateway["port"], Value::Null);
    assert_eq!(gateway["action"], "open-harness");
    host.click_geometry("home_gateway_action");
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["harness_workspace"]["active"],
        true
    );
    host.click_geometry("settings");
    host.click_geometry("settings_harness_route");
    let loaded = wait_harness(&host, |value| value["busy"] == false);
    assert_eq!(loaded["listener"], "stopped");
    host.click_geometry("settings");
    host.click_geometry("harness_entry");
    let observed = host.cli(&["ui-snapshot"]);
    assert_eq!(observed["geometry"]["home_active"], true);
    let gateway = &observed["geometry"]["home_gateway"];
    assert_eq!(gateway["status"], "stopped");
    assert_eq!(gateway["reason"], Value::Null);
    assert_eq!(gateway["port"], Value::Null);
    assert!(gateway["observed_at_unix_ms"].as_u64().unwrap() > 0);
    let stopped_at = gateway["observed_at_unix_ms"].as_u64().unwrap();
    host.click_geometry("home_gateway_action");
    open_harness(&host);
    click_harness(&host, "listener_actions", 0);
    let listening = wait_harness(&host, |state| state["listener"] == "listening");
    assert_eq!(listening["actual_port"], port);
    {
        use std::io::{Read, Write};
        let mut client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        write!(client, "GET /v1/models HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer synthetic-home-gateway-key\r\nConnection: close\r\n\r\n").unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    }
    host.click_geometry("settings");
    host.click_geometry("harness_entry");
    let running = host.cli(&["ui-snapshot"]);
    assert_eq!(running["geometry"]["home_active"], true);
    let gateway = &running["geometry"]["home_gateway"];
    assert_eq!(gateway["status"], "listening");
    assert_eq!(gateway["port"], port);
    assert_eq!(gateway["reason"], Value::Null);
    assert!(gateway["observed_at_unix_ms"].as_u64().unwrap() >= stopped_at);
    host.click_geometry("home_gateway_action");
    open_harness(&host);
    click_harness(&host, "listener_actions", 1);
    wait_harness(&host, |state| state["listener"] == "stopped");
    host.click_geometry("settings");
    host.click_geometry("harness_entry");
    let stopped = host.cli(&["ui-snapshot"]);
    assert_eq!(stopped["geometry"]["home_active"], true);
    let gateway = &stopped["geometry"]["home_gateway"];
    assert_eq!(gateway["status"], "stopped");
    assert_eq!(gateway["port"], Value::Null);
    assert!(gateway["observed_at_unix_ms"].as_u64().unwrap() >= stopped_at);
    host.cli(&["resize-window", "--width", "500", "--height", "650"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.cli(&["ui-snapshot"])["geometry"]["frame"]["width"] != 500 {
        assert!(
            Instant::now() < deadline,
            "narrow gateway card did not settle"
        );
        thread::sleep(Duration::from_millis(25));
    }
    thread::sleep(Duration::from_millis(250));
    println!(
        "Home gateway narrow: {}",
        host.capture_home_client("home-gateway-narrow.png")
            .display()
    );
    host.close_and_wait();
}

#[cfg(windows)]
#[test]
#[ignore = "requires a native desktop; run the explicit Home keyboard court"]
fn native_home_keyboard_navigation_reveals_exact_tab_ids() {
    let mut host = BrowserGui::spawn(false, None, None);
    let first = host.cli(&["list-tabs"])["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut ids = vec![first.clone()];
    for _ in 0..8 {
        ids.push(host.cli(&["new-tab"])["id"].as_str().unwrap().to_owned());
    }
    host.click_geometry("home");
    let capacity = host.cli(&["ui-snapshot"])["geometry"]["home_list_row_ids"]
        .as_array()
        .unwrap()
        .len();
    assert!(capacity > 0 && capacity < ids.len());

    host.cli(&["send-ui-keys", "End"]);
    let last = host.cli(&["ui-snapshot"]);
    assert_eq!(last["geometry"]["home_selected_id"], ids[8]);
    assert!(last["geometry"]["home_list_start"].as_u64().unwrap() > 0);
    assert!(
        last["geometry"]["home_list_row_ids"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(ids[8]))
    );
    assert_eq!(last["geometry"]["home_active"], true);

    host.cli(&["send-ui-keys", "PageUp"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["home_selected_id"],
        ids[8 - capacity]
    );
    host.cli(&["send-ui-keys", "PageDown"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["home_selected_id"],
        ids[8]
    );
    host.cli(&["send-ui-keys", "Home"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["home_selected_id"],
        first
    );
    host.cli(&["send-ui-keys", "Down"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["home_selected_id"],
        ids[1]
    );
    host.cli(&["send-ui-keys", "Up"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["home_selected_id"],
        first
    );
    host.cli(&["send-ui-keys", "Escape"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["home_selected_id"],
        Value::Null
    );
    host.cli(&["send-ui-keys", "End", "Enter"]);
    assert_eq!(host.cli(&["ui-snapshot"])["geometry"]["home_active"], false);
    let listed = host.cli(&["list-tabs"]);
    assert_eq!(listed["tabs"][8]["id"], ids[8]);
    assert_eq!(listed["tabs"][8]["active"], true);
    host.close_and_wait();
}

#[cfg(windows)]
#[test]
#[ignore = "requires a native desktop; run the explicit 256-tab Home court"]
fn native_home_256_mixed_tabs_keep_exact_overflow_targets() {
    let mut host = BrowserGui::spawn_with_exiting_console();
    let first = host.cli(&["list-tabs"])["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut middle = String::new();
    for index in 1..255 {
        let id = host.cli(&["new-tab"])["id"].as_str().unwrap().to_owned();
        if index == 127 {
            middle = id;
        }
    }
    let web = host.open_browser_directly()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let listed = host.cli(&["list-tabs"]);
    assert_eq!(listed["tabs"].as_array().unwrap().len(), 256);
    assert_eq!(listed["tabs"][0]["id"], first);
    assert_eq!(listed["tabs"][127]["id"], middle);
    assert_eq!(listed["tabs"][255]["id"], web);

    host.click_geometry("home");
    let home = host.cli(&["ui-snapshot"]);
    assert_eq!(home["geometry"]["home_tab_count"], 256);
    assert_eq!(home["geometry"]["home_visible_count"], 256);
    assert_eq!(home["geometry"]["home_list_scrollbar_visible"], true);
    let capacity = home["geometry"]["home_list_row_ids"]
        .as_array()
        .unwrap()
        .len();
    assert!(capacity > 0 && capacity < 256);
    host.click_geometry("home_mode_action");
    let visual = host.cli(&["ui-snapshot"]);
    assert_eq!(visual["geometry"]["home_mode"], "visual-preview");
    assert_eq!(visual["geometry"]["home_list_scrollbar_visible"], true);
    host.cli(&["send-ui-keys", "End"]);
    let visual_end = host.cli(&["ui-snapshot"]);
    assert_eq!(visual_end["geometry"]["home_selected_id"], web);
    let web_index = visual_end["geometry"]["home_list_row_ids"]
        .as_array()
        .unwrap()
        .iter()
        .position(|id| *id == web)
        .expect("Visual overflow reveals the exact Web @ID");
    host.click_geometry_row("home_list_row_rects", web_index);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["home_selected_id"],
        web
    );
    host.click_geometry("home_focus_tab");
    assert_eq!(host.cli(&["list-tabs"])["tabs"][255]["active"], true);
    host.click_geometry("home");
    host.click_geometry("home_mode_action");
    assert_eq!(host.cli(&["ui-snapshot"])["geometry"]["home_mode"], "list");
    host.click_geometry("home_sort_action");
    let sorted = host.cli(&["ui-snapshot"]);
    assert_eq!(sorted["geometry"]["home_sort"], "label");
    assert_eq!(sorted["geometry"]["home_list_row_ids"][0], web);
    assert_eq!(sorted["geometry"]["home_list_row_ids"][1], first);
    host.click_geometry("home_sort_action");
    assert_eq!(host.cli(&["ui-snapshot"])["geometry"]["home_sort"], "tree");
    host.cli(&["send-ui-keys", "End"]);
    let end = host.cli(&["ui-snapshot"]);
    assert_eq!(end["geometry"]["home_selected_id"], web);
    assert!(
        end["geometry"]["home_list_row_ids"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(web))
    );
    assert!(end["geometry"]["home_list_start"].as_u64().unwrap() > 0);
    host.click_geometry("home_focus_tab");
    let focused = host.cli(&["list-tabs"]);
    assert_eq!(focused["tabs"][255]["id"], web);
    assert_eq!(focused["tabs"][255]["active"], true);

    let over_cap = invoke(&host.exe, &host.endpoint, &["new-tab"]);
    assert!(!over_cap.status.success(), "257th tab must be refused");
    assert_eq!(
        host.cli(&["list-tabs"])["tabs"].as_array().unwrap().len(),
        256
    );
    host.click_geometry("new_web");
    let refused_web = host.cli(&["ui-snapshot"]);
    assert_eq!(refused_web["active"], web);
    assert!(
        refused_web["host_notice"]
            .as_str()
            .unwrap_or_default()
            .contains("browser tab limit reached")
    );
    assert_eq!(
        host.cli(&["list-tabs"])["tabs"].as_array().unwrap().len(),
        256
    );

    host.click_geometry("home");
    host.click_geometry("home_new_web");
    let refused_home_web = host.cli(&["ui-snapshot"]);
    assert_eq!(refused_home_web["geometry"]["home_active"], true);
    assert_eq!(refused_home_web["active"], web);
    assert!(
        refused_home_web["host_notice"]
            .as_str()
            .unwrap_or_default()
            .contains("browser tab limit reached")
    );
    host.click_geometry("home_search");
    assert_eq!(
        host.cli(&["send-ui-ime", "commit", &middle])["route"],
        "home_search"
    );
    let searched = host.cli(&["ui-snapshot"]);
    assert_eq!(searched["geometry"]["home_visible_count"], 1);
    assert_eq!(searched["geometry"]["home_list_row_ids"][0], middle);
    host.click_geometry_row("home_list_row_rects", 0);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["home_selected_id"],
        middle
    );
    host.cli(&["close-tab", "--target", &middle]);
    let stale = host.cli(&["ui-snapshot"]);
    assert_eq!(stale["geometry"]["home_selected_id"], middle);
    assert_eq!(stale["geometry"]["home_selected_stale"], true);
    assert_eq!(stale["geometry"]["home_tab_count"], 255);
    host.click_geometry("home_focus_tab");
    assert_eq!(host.cli(&["ui-snapshot"])["geometry"]["home_active"], true);
    host.click_geometry("home_new_web");
    let recovered = host.cli(&["list-tabs"]);
    assert_eq!(recovered["tabs"].as_array().unwrap().len(), 256);
    assert_eq!(recovered["tabs"][255]["id"], "@257");
    assert_eq!(recovered["tabs"][255]["active"], true);
    assert!(
        recovered["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tab| tab["id"] == web)
    );
    assert_eq!(host.cli(&["ui-snapshot"])["host_notice"], Value::Null);
    host.close_and_wait();
}

#[cfg(windows)]
#[test]
#[ignore = "requires a native desktop; run the explicit Home scrollbar court"]
fn native_home_list_scrollbar_pages_and_drags_without_retargeting() {
    let mut host = BrowserGui::spawn(false, None, None);
    for _ in 0..16 {
        host.cli(&["new-tab"]);
    }
    host.click_geometry("home");
    let before = host.cli(&["ui-snapshot"]);
    let geometry = &before["geometry"];
    assert_eq!(geometry["home_list_scrollbar_visible"], true);
    println!(
        "Home scrollbar capture: {}",
        host.capture_home_client("home-list-scrollbar.png")
            .display()
    );
    let scale = geometry["scale_permille"].as_u64().unwrap() as f64 / 1000.0;
    let track = &geometry["home_list_scrollbar"];
    let x = ((track["x"].as_u64().unwrap() + track["width"].as_u64().unwrap() / 2) as f64 / scale)
        .round()
        .to_string();
    let bottom = ((track["y"].as_u64().unwrap() + track["height"].as_u64().unwrap() - 2) as f64
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
        &bottom,
    ]);
    let paged = host.cli(&["ui-snapshot"]);
    assert!(paged["geometry"]["home_list_start"].as_u64().unwrap() > 0);
    assert_eq!(paged["geometry"]["home_selected_id"], Value::Null);

    let thumb = &paged["geometry"]["home_list_scrollbar_thumb"];
    let thumb_y = ((thumb["y"].as_u64().unwrap() + thumb["height"].as_u64().unwrap() / 2) as f64
        / scale)
        .round()
        .to_string();
    let top = ((track["y"].as_u64().unwrap() + 2) as f64 / scale)
        .round()
        .to_string();
    for (action, button, row) in [
        ("press", "left", thumb_y.as_str()),
        ("move", "none", top.as_str()),
        ("release", "left", top.as_str()),
    ] {
        host.cli(&[
            "send-ui-mouse",
            "--action",
            action,
            "--button",
            button,
            "--column",
            &x,
            "--row",
            row,
        ]);
    }
    let returned = host.cli(&["ui-snapshot"]);
    assert_eq!(returned["geometry"]["home_list_start"], 0);
    let first = returned["geometry"]["home_list_row_ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    host.click_geometry_row("home_list_row_rects", 0);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["home_selected_id"],
        first
    );
    host.click_geometry("home_mode_action");
    let visual = host.cli(&["ui-snapshot"]);
    assert_eq!(visual["geometry"]["home_mode"], "visual-preview");
    assert_eq!(visual["geometry"]["home_list_scrollbar_visible"], true);
    host.cli(&[
        "send-ui-mouse",
        "--action",
        "click",
        "--button",
        "left",
        "--column",
        &x,
        "--row",
        &bottom,
    ]);
    let visual_paged = host.cli(&["ui-snapshot"]);
    assert!(
        visual_paged["geometry"]["home_list_start"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(visual_paged["geometry"]["home_selected_id"], first);
    let visual_thumb = &visual_paged["geometry"]["home_list_scrollbar_thumb"];
    let visual_thumb_y = ((visual_thumb["y"].as_u64().unwrap()
        + visual_thumb["height"].as_u64().unwrap() / 2) as f64
        / scale)
        .round()
        .to_string();
    for (action, button, row) in [
        ("press", "left", visual_thumb_y.as_str()),
        ("move", "none", top.as_str()),
        ("release", "left", top.as_str()),
    ] {
        host.cli(&[
            "send-ui-mouse",
            "--action",
            action,
            "--button",
            button,
            "--column",
            &x,
            "--row",
            row,
        ]);
    }
    assert_eq!(host.cli(&["ui-snapshot"])["geometry"]["home_list_start"], 0);
    host.close_and_wait();
}

#[cfg(windows)]
#[test]
#[ignore = "requires a native desktop; run the explicit Home exit attention court"]
fn native_home_child_exit_attention_inspects_exact_tab_and_resolves_on_close() {
    let mut host = BrowserGui::spawn(false, None, None);
    let exited = host.cli(&["list-tabs"])["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let survivor = host.cli(&["new-tab"])["id"].as_str().unwrap().to_owned();
    host.cli(&["send-text", "--target", &exited, "exit 7\r"]);
    let completed = host.cli(&["wait-tab-exit", "--target", &exited]);
    assert_eq!(completed["child_exit_code"], 7);
    host.click_geometry("home");
    let before = host.cli(&["ui-snapshot"]);
    let items = before["geometry"]["home_attention_items"]
        .as_array()
        .unwrap();
    assert_eq!(
        before["geometry"]["home_attention_coverage"],
        "terminal-child-exit-only"
    );
    assert_eq!(before["geometry"]["home_attention_status"], "observed");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["key"], format!("terminal-child-exit:{exited}"));
    assert_eq!(items[0]["source"], "terminal-child");
    assert_eq!(items[0]["severity"], "info");
    assert_eq!(items[0]["tab_id"], exited);
    assert_eq!(items[0]["child_exit_code"], 7);
    assert!(items[0]["observed_at_unix_ms"].as_u64().unwrap() > 0);
    assert_eq!(items[0]["action"], "inspect-tab");
    println!(
        "Home exit attention capture: {}",
        host.capture_home_client("home-exit-attention.png")
            .display()
    );
    host.cli(&["resize-window", "--width", "500", "--height", "650"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.cli(&["ui-snapshot"])["geometry"]["frame"]["width"] != 500 {
        assert!(Instant::now() < deadline, "narrow Home did not settle");
        thread::sleep(Duration::from_millis(25));
    }
    thread::sleep(Duration::from_millis(250));
    println!(
        "Home narrow exit attention capture: {}",
        host.capture_home_client("home-exit-attention-narrow.png")
            .display()
    );
    host.click_geometry("home_attention_action");
    let selected = host.cli(&["ui-snapshot"]);
    assert_eq!(selected["geometry"]["home_active"], true);
    assert_eq!(selected["geometry"]["home_selected_id"], exited);
    host.cli(&["close-tab", "--target", &exited]);
    let resolved = host.cli(&["ui-snapshot"]);
    assert_eq!(resolved["geometry"]["home_active"], true);
    assert!(
        resolved["geometry"]["home_attention_items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(resolved["geometry"]["home_selected_stale"], true);
    assert_eq!(
        resolved["geometry"]["home_attention_status"],
        "none-observed"
    );
    println!(
        "Home quiet attention capture: {}",
        host.capture_home_client("home-attention-none.png")
            .display()
    );
    assert_eq!(host.cli(&["list-tabs"])["tabs"][0]["id"], survivor);
    host.close_and_wait();
}

#[cfg(windows)]
#[test]
#[ignore = "requires a native desktop; run the explicit Home kind-filter court"]
fn native_home_kind_filter_keeps_exact_rows_and_global_exit_attention() {
    let mut host = BrowserGui::spawn(false, None, None);
    let exited = host.cli(&["list-tabs"])["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let console = host.cli(&["new-tab"])["id"].as_str().unwrap().to_owned();
    let web = host.open_browser_directly()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    host.cli(&["send-text", "--target", &exited, "exit 7\r"]);
    assert_eq!(
        host.cli(&["wait-tab-exit", "--target", &exited])["child_exit_code"],
        7
    );
    host.click_geometry("home");
    let all = host.cli(&["ui-snapshot"]);
    assert_eq!(all["geometry"]["home_kind_filter"], "all");
    assert_eq!(all["geometry"]["home_visible_count"], 3);
    assert_eq!(all["geometry"]["home_tab_count"], 3);
    host.click_geometry("home_filter_action");
    let consoles = host.cli(&["ui-snapshot"]);
    assert_eq!(consoles["geometry"]["home_kind_filter"], "console");
    assert_eq!(consoles["geometry"]["home_visible_count"], 2);
    assert_eq!(consoles["geometry"]["home_list_row_ids"][0], exited);
    assert_eq!(consoles["geometry"]["home_list_row_ids"][1], console);
    host.click_geometry("home_filter_action");
    let webs = host.cli(&["ui-snapshot"]);
    assert_eq!(webs["geometry"]["home_kind_filter"], "web");
    assert_eq!(webs["geometry"]["home_visible_count"], 1);
    assert_eq!(webs["geometry"]["home_list_row_ids"][0], web);
    assert_eq!(
        webs["geometry"]["home_attention_items"][0]["tab_id"],
        exited
    );
    println!(
        "Home filtered capture: {}",
        host.capture_home_client("home-filter-web.png").display()
    );
    host.cli(&["resize-window", "--width", "500", "--height", "650"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.cli(&["ui-snapshot"])["geometry"]["frame"]["width"] != 500 {
        assert!(
            Instant::now() < deadline,
            "narrow filtered Home did not settle"
        );
        thread::sleep(Duration::from_millis(25));
    }
    thread::sleep(Duration::from_millis(250));
    println!(
        "Home narrow filtered capture: {}",
        host.capture_home_client("home-filter-web-narrow.png")
            .display()
    );
    host.click_geometry("home_attention_action");
    let hidden_inspector = host.cli(&["ui-snapshot"]);
    assert_eq!(hidden_inspector["geometry"]["home_selected_id"], exited);
    assert_eq!(hidden_inspector["geometry"]["home_kind_filter"], "web");
    assert_eq!(
        hidden_inspector["geometry"]["home_selected_hidden_by_filter"],
        true
    );
    assert_eq!(
        hidden_inspector["geometry"]["home_inspector_action"],
        "reveal"
    );
    assert_eq!(hidden_inspector["geometry"]["home_focus_enabled"], false);
    println!(
        "Home filtered reveal capture: {}",
        host.capture_home_client("home-filter-reveal-narrow.png")
            .display()
    );
    host.click_geometry("home_reveal_tab");
    let revealed = host.cli(&["ui-snapshot"]);
    assert_eq!(revealed["geometry"]["home_active"], true);
    assert_eq!(revealed["geometry"]["home_kind_filter"], "all");
    assert_eq!(revealed["geometry"]["home_selected_id"], exited);
    assert_eq!(
        revealed["geometry"]["home_selected_hidden_by_filter"],
        false
    );
    assert_eq!(revealed["geometry"]["home_inspector_action"], "focus");
    assert!(
        revealed["geometry"]["home_list_row_ids"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(exited))
    );
    host.click_geometry("home_filter_action");
    host.click_geometry("home_filter_action");
    host.click_geometry_row("home_list_row_rects", 0);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["home_selected_id"],
        web
    );
    host.click_geometry("home_filter_action");
    let restored = host.cli(&["ui-snapshot"]);
    assert_eq!(restored["geometry"]["home_kind_filter"], "all");
    assert_eq!(restored["geometry"]["home_visible_count"], 3);
    assert_eq!(restored["geometry"]["home_selected_id"], web);
    assert_eq!(
        restored["geometry"]["home_selected_hidden_by_filter"],
        false
    );
    host.click_geometry("home_filter_action");
    host.click_geometry("home_filter_action");
    host.cli(&["close-tab", "--target", &web]);
    let empty_web = host.cli(&["ui-snapshot"]);
    assert_eq!(empty_web["geometry"]["home_kind_filter"], "web");
    assert_eq!(empty_web["geometry"]["home_visible_count"], 0);
    assert!(
        empty_web["geometry"]["home_list_row_ids"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        empty_web["geometry"]["home_attention_items"][0]["tab_id"],
        exited
    );
    assert_eq!(empty_web["geometry"]["home_inspector_action"], "disabled");
    assert_eq!(empty_web["geometry"]["home_reveal_tab"], Value::Null);
    host.click_geometry("home_focus_tab");
    assert_eq!(host.cli(&["ui-snapshot"])["geometry"]["home_active"], true);
    host.close_and_wait();
}

#[cfg(windows)]
#[test]
#[ignore = "requires a native desktop; run the explicit Home sort court"]
fn native_home_label_sort_changes_view_without_changing_tab_tree() {
    let mut host = BrowserGui::spawn(false, None, None);
    let first = host.cli(&["list-tabs"])["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let second = host.cli(&["new-tab"])["id"].as_str().unwrap().to_owned();
    let web = host.open_browser_directly()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    host.click_geometry("home");
    let tree = host.cli(&["ui-snapshot"]);
    assert_eq!(tree["geometry"]["home_sort"], "tree");
    assert_eq!(tree["geometry"]["home_list_row_ids"][0], first);
    assert_eq!(tree["geometry"]["home_list_row_ids"][1], second);
    assert_eq!(tree["geometry"]["home_list_row_ids"][2], web);

    host.click_geometry("home_sort_action");
    let sorted = host.cli(&["ui-snapshot"]);
    assert_eq!(sorted["geometry"]["home_sort"], "label");
    assert_eq!(sorted["geometry"]["home_list_row_ids"][0], web);
    assert_eq!(sorted["geometry"]["home_list_row_ids"][1], first);
    assert_eq!(sorted["geometry"]["home_list_row_ids"][2], second);
    host.cli(&["resize-window", "--width", "500", "--height", "650"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    let narrow = loop {
        let snapshot = host.cli(&["ui-snapshot"]);
        if snapshot["geometry"]["frame"]["width"] == 500 {
            break snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "narrow sorted Home did not settle"
        );
        thread::sleep(Duration::from_millis(25));
    };
    let sort = &narrow["geometry"]["home_sort_action"];
    let filter = &narrow["geometry"]["home_filter_action"];
    assert!(sort["width"].as_u64().unwrap() > 0);
    assert!(
        sort["x"].as_u64().unwrap() + sort["width"].as_u64().unwrap()
            < filter["x"].as_u64().unwrap()
    );
    thread::sleep(Duration::from_millis(250));
    println!(
        "Home sorted narrow capture: {}",
        host.capture_home_client("home-sort-narrow.png").display()
    );
    host.click_geometry_row("home_list_row_rects", 0);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["home_selected_id"],
        web
    );
    host.click_geometry("home_focus_tab");
    assert_eq!(host.cli(&["list-tabs"])["tabs"][2]["active"], true);
    host.click_geometry("home");
    let persisted = host.cli(&["ui-snapshot"]);
    assert_eq!(persisted["geometry"]["home_sort"], "label");
    assert_eq!(persisted["geometry"]["home_list_row_ids"][0], web);
    let tree_unchanged = host.cli(&["list-tabs"]);
    assert_eq!(tree_unchanged["tabs"][0]["id"], first);
    assert_eq!(tree_unchanged["tabs"][1]["id"], second);
    assert_eq!(tree_unchanged["tabs"][2]["id"], web);

    host.click_geometry("home_filter_action");
    let consoles = host.cli(&["ui-snapshot"]);
    assert_eq!(consoles["geometry"]["home_kind_filter"], "console");
    assert_eq!(consoles["geometry"]["home_sort"], "label");
    assert_eq!(consoles["geometry"]["home_list_row_ids"][0], first);
    assert_eq!(consoles["geometry"]["home_list_row_ids"][1], second);
    host.click_geometry("home_sort_action");
    let restored = host.cli(&["ui-snapshot"]);
    assert_eq!(restored["geometry"]["home_sort"], "tree");
    assert_eq!(restored["geometry"]["home_list_row_ids"][0], first);
    host.close_and_wait();
}

#[cfg(windows)]
#[test]
#[ignore = "requires a native desktop; run the explicit Home search court"]
fn native_home_search_name_and_exact_id_keep_global_attention() {
    let mut host = BrowserGui::spawn(false, None, None);
    let exited = host.cli(&["list-tabs"])["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let other = host.cli(&["new-tab"])["id"].as_str().unwrap().to_owned();
    let web = host.open_browser_directly()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    host.cli(&["send-text", "--target", &exited, "exit 7\r"]);
    host.cli(&["wait-tab-exit", "--target", &exited]);
    host.click_geometry("home");
    host.click_geometry("home_search");
    assert_eq!(
        host.cli(&["send-ui-ime", "commit", "BROWSER"])["route"],
        "home_search"
    );
    let by_name = host.cli(&["ui-snapshot"]);
    assert_eq!(by_name["geometry"]["home_search_focused"], true);
    assert_eq!(by_name["geometry"]["home_visible_count"], 1);
    assert_eq!(by_name["geometry"]["home_list_row_ids"][0], web);
    assert_eq!(by_name["geometry"]["home_tab_count"], 3);
    assert_eq!(
        by_name["geometry"]["home_attention_items"][0]["tab_id"],
        exited
    );
    println!(
        "Home search capture: {}",
        host.capture_home_client("home-search-name.png").display()
    );
    host.cli(&["resize-window", "--width", "500", "--height", "650"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.cli(&["ui-snapshot"])["geometry"]["frame"]["width"] != 500 {
        assert!(
            Instant::now() < deadline,
            "narrow search Home did not settle"
        );
        thread::sleep(Duration::from_millis(25));
    }
    thread::sleep(Duration::from_millis(250));
    println!(
        "Home narrow search capture: {}",
        host.capture_home_client("home-search-name-narrow.png")
            .display()
    );
    host.click_geometry("home_attention_action");
    let hidden = host.cli(&["ui-snapshot"]);
    assert_eq!(hidden["geometry"]["home_selected_id"], exited);
    assert_eq!(hidden["geometry"]["home_selected_hidden_by_filter"], true);
    host.click_geometry("home_search");
    host.cli(&["send-ui-keys", "Ctrl+a"]);
    host.cli(&["send-ui-ime", "commit", &other]);
    let by_id = host.cli(&["ui-snapshot"]);
    assert_eq!(by_id["geometry"]["home_visible_count"], 1);
    assert_eq!(by_id["geometry"]["home_list_row_ids"][0], other);
    assert_eq!(by_id["geometry"]["home_selected_hidden_by_filter"], true);
    host.click_geometry("home_filter_action");
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["home_visible_count"],
        1
    );
    host.click_geometry("home_filter_action");
    let no_match = host.cli(&["ui-snapshot"]);
    assert_eq!(no_match["geometry"]["home_visible_count"], 0);
    assert_eq!(
        no_match["geometry"]["home_attention_items"][0]["tab_id"],
        exited
    );
    host.cli(&["send-ui-keys", "Ctrl+f", "Escape"]);
    let cleared = host.cli(&["ui-snapshot"]);
    assert_eq!(cleared["geometry"]["home_search_focused"], false);
    assert_eq!(cleared["geometry"]["home_search_query_bytes"], 0);
    assert_eq!(cleared["geometry"]["home_kind_filter"], "web");
    assert_eq!(cleared["geometry"]["home_list_row_ids"][0], web);
    assert_eq!(cleared["geometry"]["home_inspector_action"], "reveal");
    host.cli(&["send-ui-keys", "Enter"]);
    let keyboard_revealed = host.cli(&["ui-snapshot"]);
    assert_eq!(keyboard_revealed["geometry"]["home_active"], true);
    assert_eq!(keyboard_revealed["geometry"]["home_kind_filter"], "all");
    assert_eq!(keyboard_revealed["geometry"]["home_selected_id"], exited);
    assert_eq!(
        keyboard_revealed["geometry"]["home_inspector_action"],
        "focus"
    );
    assert!(
        keyboard_revealed["geometry"]["home_list_row_ids"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(exited))
    );
    host.close_and_wait();
}

#[cfg(windows)]
#[test]
#[ignore = "requires a native desktop; run the explicit @0 fleet wheel court"]
fn native_home_fleet_wheel_keeps_scrolled_row_target_exact() {
    use std::ffi::c_void;
    type Hwnd = *mut c_void;
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    struct Find {
        pid: u32,
        window: Hwnd,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(callback: unsafe extern "system" fn(Hwnd, isize) -> i32, data: isize)
        -> i32;
        fn GetWindowThreadProcessId(window: Hwnd, pid: *mut u32) -> u32;
        fn IsWindowVisible(window: Hwnd) -> i32;
        fn ClientToScreen(window: Hwnd, point: *mut Point) -> i32;
        fn PostMessageW(window: Hwnd, message: u32, wparam: usize, lparam: isize) -> i32;
    }
    unsafe extern "system" fn visit(window: Hwnd, data: isize) -> i32 {
        let find = unsafe { &mut *(data as *mut Find) };
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(window, &mut pid) };
        if pid == find.pid && unsafe { IsWindowVisible(window) } != 0 {
            find.window = window;
            0
        } else {
            1
        }
    }
    let mut host = BrowserGui::spawn(false, None, None);
    for _ in 0..16 {
        host.cli(&["new-tab"]);
    }
    host.click_geometry("home");
    let before = host.cli(&["ui-snapshot"]);
    let geometry = &before["geometry"];
    let visible = geometry["home_list_row_ids"].as_array().unwrap();
    assert!(
        visible.len() < 17 && !visible.is_empty(),
        "the native viewport must overflow: {geometry}"
    );
    let fleet = &geometry["home_fleet"];
    let mut point = Point {
        x: (fleet["x"].as_i64().unwrap() + fleet["width"].as_i64().unwrap() / 2) as i32,
        y: (fleet["y"].as_i64().unwrap() + fleet["height"].as_i64().unwrap() / 2) as i32,
    };
    let mut find = Find {
        pid: host.gui.child.id(),
        window: std::ptr::null_mut(),
    };
    unsafe { EnumWindows(visit, &mut find as *mut Find as isize) };
    assert!(!find.window.is_null(), "owned MiniCon window");
    assert_ne!(unsafe { ClientToScreen(find.window, &mut point) }, 0);
    let wheel_down = (-120i16 as u16 as usize) << 16;
    let packed = (((point.y as u16 as u32) << 16) | point.x as u16 as u32) as isize;
    assert_ne!(
        unsafe { PostMessageW(find.window, 0x020A, wheel_down, packed) },
        0
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    let after = loop {
        let current = host.cli(&["ui-snapshot"]);
        if current["geometry"]["home_list_start"].as_u64().unwrap_or(0) > 0 {
            break current;
        }
        assert!(
            Instant::now() < deadline,
            "Home wheel did not scroll: {current}"
        );
        thread::sleep(Duration::from_millis(25));
    };
    let target = after["geometry"]["home_list_row_ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    host.click_geometry_row("home_list_row_rects", 0);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["home_selected_id"],
        target
    );
    host.click_geometry("home_focus_tab");
    let listed = host.cli(&["list-tabs"]);
    assert!(
        listed["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tab| { tab["id"] == target && tab["active"] == true }),
        "scrolled row retargeted: {listed}"
    );
    host.close_and_wait();
}

#[test]
#[ignore = "requires a native desktop; run the explicit direct creation court"]
fn native_header_buttons_create_terminal_web_and_open_harness_directly() {
    let mut host = BrowserGui::spawn(false, None, None);
    let original = host.cli(&["list-tabs"])["tabs"].as_array().unwrap().len();
    host.click_geometry("new_root");
    let tabs = host.cli(&["list-tabs"]);
    assert_eq!(tabs["tabs"].as_array().unwrap().len(), original + 1);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["new_tab_menu_open"],
        false
    );

    host.click_geometry("new_web");
    let tabs = host.cli(&["list-tabs"]);
    assert_eq!(tabs["tabs"].as_array().unwrap().len(), original + 2);
    assert_eq!(tabs["tabs"][original + 1]["kind"], "browser");
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["new_tab_menu_open"],
        false
    );

    host.click_geometry("sidebar_toggle");
    let collapsed = host.cli(&["ui-snapshot"]);
    assert!(collapsed["geometry"]["new_root"]["width"].as_u64().unwrap() > 0);
    assert!(collapsed["geometry"]["new_web"]["width"].as_u64().unwrap() > 0);
    host.click_geometry("new_root");
    assert_eq!(
        host.cli(&["list-tabs"])["tabs"].as_array().unwrap().len(),
        original + 3
    );
    host.click_geometry("new_web");
    assert_eq!(
        host.cli(&["list-tabs"])["tabs"].as_array().unwrap().len(),
        original + 4
    );
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["new_tab_menu_open"],
        false
    );

    host.click_geometry("harness_entry");
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["harness_workspace"]["active"],
        true
    );
    assert_eq!(
        host.cli(&["list-tabs"])["tabs"].as_array().unwrap().len(),
        original + 4
    );
    host.close_and_wait();
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
    host.open_browser_directly();
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
    host.open_browser_directly();
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
    host.open_browser_directly();
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

fn host_editor_state(host: &BrowserGui, name: &str) -> Value {
    host.cli(&["ui-snapshot"])["geometry"][name].clone()
}

fn click_editor_end(host: &BrowserGui, target: &str) {
    let snapshot = host.cli(&["ui-snapshot"]);
    let geometry = &snapshot["geometry"];
    let rect = &geometry[target];
    let scale = geometry["scale_permille"].as_u64().unwrap() as f64 / 1000.0;
    let x = ((rect["x"].as_u64().unwrap() + rect["width"].as_u64().unwrap() - 4) as f64 / scale)
        .round()
        .to_string();
    let y = ((rect["y"].as_u64().unwrap() + rect["height"].as_u64().unwrap() / 2) as f64 / scale)
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
}

#[test]
fn host_browser_editors_replace_selected_text_with_bounded_atomic_edits() {
    // No network or usable browser engine is needed to exercise host editors.
    let mut host = BrowserGui::spawn(true, None, None);
    host.open_browser_directly();
    for (name, target, limit) in [
        ("browser_address_editor", "browser_address", 8192),
        ("browser_proxy_editor", "browser_proxy", 2048),
        ("web_proxy_candidate_editor", "web_proxy_input", 2048),
    ] {
        if name == "web_proxy_candidate_editor" {
            host.open_web_settings();
        }
        host.click_geometry(target);
        host.cli(&["send-ui-keys", "Ctrl+a"]);
        host.cli(&["send-ui-ime", "commit", &"a".repeat(limit)]);
        assert_eq!(
            host_editor_state(&host, name)["text"]
                .as_str()
                .unwrap()
                .len(),
            limit
        );
        host.cli(&["send-ui-keys", "Ctrl+a"]);
        host.cli(&["send-ui-keys", "z"]);
        assert_eq!(
            host_editor_state(&host, name)["text"],
            "z",
            "{name}: keyboard replacement at the bound"
        );
        host.cli(&["send-ui-keys", "Ctrl+a"]);
        host.cli(&["send-ui-ime", "commit", &"a".repeat(limit)]);
        host.cli(&["send-ui-keys", "Ctrl+a"]);
        host.cli(&["send-ui-ime", "commit", "短"]);
        let replaced = host_editor_state(&host, name);
        assert_eq!(
            replaced["text"], "短",
            "{name}: IME replacement at the bound"
        );
        assert_eq!(replaced["caret"], 3);
        assert!(replaced["selection"].is_null());

        host.cli(&["send-ui-keys", "Ctrl+a"]);
        host.cli(&["send-ui-ime", "commit", &"a".repeat(limit)]);
        host.cli(&["send-ui-keys", "Home"]);
        host.cli(&["send-ui-keys", "Right"]);
        host.cli(&["send-ui-keys", "Shift+Right"]);
        let before = host_editor_state(&host, name);
        assert_eq!(before["selection"], serde_json::json!([1, 2]));
        host.cli(&["send-ui-ime", "commit", "中字"]);
        let rejected = host_editor_state(&host, name);
        assert_eq!(
            rejected["text"], before["text"],
            "{name}: oversized text changed the draft"
        );
        assert_eq!(
            rejected["caret"], before["caret"],
            "{name}: oversized text moved the caret"
        );
        assert_eq!(
            rejected["selection"], before["selection"],
            "{name}: oversized text cut the selection"
        );

        host.cli(&["send-ui-keys", "Ctrl+a"]);
        host.cli(&["send-ui-ime", "commit", "a中z"]);
        host.cli(&["send-ui-keys", "Home"]);
        host.cli(&["send-ui-keys", "Right"]);
        host.cli(&["send-ui-keys", "Shift+Right"]);
        assert_eq!(
            host_editor_state(&host, name)["selection"],
            serde_json::json!([1, 4])
        );
        host.cli(&["send-ui-ime", "commit", "XY"]);
        let partial = host_editor_state(&host, name);
        assert_eq!(partial["text"], "aXYz");
        assert_eq!(partial["caret"], 3);
        assert!(partial["selection"].is_null());

        host.cli(&["send-ui-keys", "Ctrl+a"]);
        host.cli(&["send-ui-ime", "commit", "http://127.0.0.1:65535"]);
        host.cli(&["send-ui-ime", "preedit", "ni"]);
        host.cli(&["send-ui-keys", "Enter"]);
        let composing = host_editor_state(&host, name);
        assert_eq!(
            composing["focused"], true,
            "{name}: composition Enter changed ownership"
        );
        assert_eq!(
            composing["text"], "http://127.0.0.1:65535",
            "{name}: composition Enter submitted the draft"
        );
        assert_eq!(composing["preedit"], "ni");
        if name == "browser_address_editor" {
            host.click_geometry("browser_proxy");
        } else if name == "browser_proxy_editor" {
            host.cli(&["send-ui-keys", "Ctrl+l"]);
        } else {
            host.click_geometry("settings_console_route");
        }
        let blurred = host_editor_state(&host, name);
        assert_eq!(blurred["focused"], false);
        assert_eq!(blurred["preedit"], "", "{name}: blur retained a preedit");
        assert_eq!(
            blurred["text"], composing["text"],
            "{name}: blur discarded its draft"
        );
        if name == "web_proxy_candidate_editor" {
            host.click_geometry("settings_web_route");
        }
        host.click_geometry(target);
        assert_eq!(
            host_editor_state(&host, name)["text"],
            composing["text"],
            "{name}: refocus replaced its draft"
        );

        host.cli(&["send-ui-keys", "Ctrl+a"]);
        host.cli(&["send-ui-ime", "commit", "xy"]);
        click_editor_end(&host, target);
        host.cli(&["send-ui-ime", "commit", "a"]);
        host.cli(&["send-ui-ime", "commit", "b"]);
        let typed = host_editor_state(&host, name);
        assert_eq!(
            typed["text"], "xyab",
            "{name}: a collapsed click anchor replaced the previous character"
        );
        assert!(typed["selection"].is_null());
    }
    host.close_and_wait();
}

#[test]
#[ignore = "requires a native desktop/WebView runtime; run the explicit editor composition court"]
fn native_browser_editor_composition_requires_a_separate_submit() {
    let mut host = BrowserGui::spawn(false, None, None);
    host.open_browser_directly();
    let origin = LocalPreviewServer::start("127.0.0.1:0".parse().unwrap());
    let first = format!("http://{}/first", origin.address);
    let second = format!("http://{}/second", origin.address);
    host.cli(&["send-ui-keys", "Ctrl+l"]);
    host.cli(&["send-ui-ime", "commit", &first]);
    host.cli(&["send-ui-keys", "Enter"]);
    host.wait_browser(&first, "ready", Some("Local preview ready"));
    host.cli(&["send-ui-keys", "Ctrl+l"]);
    host.cli(&["send-ui-ime", "commit", &second]);
    host.cli(&["send-ui-ime", "preedit", "ni"]);
    host.cli(&["send-ui-keys", "Enter"]);
    assert_eq!(host.browser()["browser"]["url"], first);
    assert_eq!(
        host_editor_state(&host, "browser_address_editor")["preedit"],
        "ni"
    );
    host.cli(&["send-ui-ime", "disabled"]);
    host.cli(&["send-ui-keys", "Enter"]);
    host.wait_browser(&second, "ready", Some("Local preview ready"));

    let proxy = LocalPreviewServer::start_proxy();
    let endpoint = format!("http://{}", proxy.address);
    host.click_geometry("browser_proxy");
    host.cli(&["send-ui-keys", "Ctrl+a"]);
    host.cli(&["send-ui-ime", "commit", &endpoint]);
    host.cli(&["send-ui-ime", "preedit", "ni"]);
    host.cli(&["send-ui-keys", "Enter"]);
    assert_eq!(
        host_editor_state(&host, "browser_proxy_editor")["focused"],
        true
    );
    assert_eq!(
        proxy.requests.load(Ordering::Relaxed),
        0,
        "composition confirmation applied a proxy"
    );
    host.cli(&["send-ui-ime", "disabled"]);
    host.cli(&["send-ui-keys", "Enter"]);
    let remote = "http://preview.minicon.invalid/editor";
    host.cli(&["send-ui-keys", "Ctrl+l"]);
    host.cli(&["send-ui-ime", "commit", remote]);
    host.cli(&["send-ui-keys", "Enter"]);
    host.wait_browser(remote, "ready", Some("Proxy preview"));
    assert!(proxy.requests.load(Ordering::Relaxed) > 0);

    host.open_web_settings();
    host.click_geometry("web_proxy_input");
    host.cli(&["send-ui-ime", "commit", &endpoint]);
    host.cli(&["send-ui-ime", "preedit", "ni"]);
    host.cli(&["send-ui-keys", "Enter"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["web_proxy_options_count"],
        0
    );
    assert_eq!(
        host_editor_state(&host, "web_proxy_candidate_editor")["text"],
        endpoint
    );
    host.cli(&["send-ui-ime", "disabled"]);
    host.cli(&["send-ui-keys", "Enter"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["web_proxy_options_count"],
        1
    );
    host.close_and_wait();
}

#[test]
fn host_browser_menus_keyboard_entry_traversal_and_single_activation() {
    let mut host = BrowserGui::spawn(true, None, None);
    let browser_id = host.open_browser_directly()["id"].clone();
    host.cli(&["send-ui-keys", "Ctrl+l"]);
    host.cli(&["send-ui-keys", "Shift+Tab"]);
    host.cli(&["send-ui-keys", "Enter"]);
    let opened = host.cli(&["ui-snapshot"]);
    assert_eq!(
        opened["geometry"]["new_tab_menu_open"], false,
        "keyboard activation of Terminal must not open a menu"
    );
    assert_eq!(
        host.cli(&["list-tabs"])["tabs"].as_array().unwrap().len(),
        3,
        "the focused Terminal button must create exactly one tab"
    );
    host.cli(&["select-tab", "--target", browser_id.as_str().unwrap()]);
    assert_eq!(
        host.browser()["id"],
        browser_id,
        "selecting Browser must preserve its root"
    );
    assert_eq!(
        host.cli(&["list-tabs"])["tabs"].as_array().unwrap().len(),
        3
    );
    host.cli(&["send-ui-keys", "Ctrl+l"]);
    host.cli(&["send-ui-keys", "Tab"]);
    assert_eq!(
        host_editor_state(&host, "browser_proxy_editor")["focused"],
        true
    );
    host.cli(&["send-ui-keys", "Tab"]);
    host.cli(&["send-ui-keys", "Space"]);
    let proxy = host.cli(&["ui-snapshot"]);
    assert_eq!(proxy["geometry"]["browser_proxy_menu_open"], true);
    assert_eq!(proxy["geometry"]["proxy_menu_highlight"], 0);
    assert_eq!(
        proxy["geometry"]["browser_proxy_option_rows"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    host.cli(&["send-ui-keys", "Down"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["proxy_menu_highlight"],
        0,
        "Down must clamp to the only configured-behavior row"
    );
    host.cli(&["send-ui-keys", "Escape"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["browser_proxy_menu_open"],
        false
    );
    host.cli(&["send-ui-keys", "Shift+Tab"]);
    assert_eq!(
        host_editor_state(&host, "browser_proxy_editor")["focused"],
        true,
        "dismissal must restore the trigger as the keyboard traversal owner"
    );
    host.cli(&["send-ui-keys", "Tab", "Space", "Tab"]);
    let dismissed = host.cli(&["ui-snapshot"]);
    assert_eq!(
        dismissed["geometry"]["browser_proxy_menu_open"], false,
        "Tab in an open proxy menu must dismiss it before any header traversal"
    );
    assert_eq!(dismissed["geometry"]["harness_workspace"]["active"], false);
    host.cli(&["send-ui-keys", "Shift+Tab"]);
    assert_eq!(
        host_editor_state(&host, "browser_proxy_editor")["focused"],
        true,
        "menu Tab dismissal retains the proxy trigger as traversal owner"
    );
    host.cli(&["send-ui-keys", "Shift+Tab"]);
    assert_eq!(
        host_editor_state(&host, "browser_address_editor")["focused"],
        true
    );
    host.cli(&["send-ui-keys", "Shift+Tab"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["browser_chrome_trigger"],
        "new"
    );
    host.cli(&["send-ui-keys", "Shift+Tab"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["harness_workspace"]["header_focus"],
        "settings",
        "reverse traversal from New must reach Settings"
    );
    host.cli(&["send-ui-keys", "Shift+Tab"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["harness_workspace"]["header_focus"],
        "robot"
    );
    host.cli(&["send-ui-keys", "Shift+Tab"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["browser_chrome_trigger"],
        "proxy"
    );
    host.cli(&["send-ui-keys", "Tab"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["harness_workspace"]["header_focus"],
        "robot"
    );
    host.cli(&["send-ui-keys", "Tab"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["harness_workspace"]["header_focus"],
        "settings"
    );
    host.cli(&["send-ui-keys", "Tab"]);
    let cycled = host.cli(&["ui-snapshot"]);
    assert_eq!(
        cycled["geometry"]["browser_chrome_trigger"], "new",
        "forward header exit must reach New rather than loop back to proxy"
    );
    assert!(cycled["geometry"]["harness_workspace"]["header_focus"].is_null());
    assert_eq!(
        host.cli(&["list-tabs"])["tabs"].as_array().unwrap().len(),
        3,
        "traversal must not activate a header or menu action"
    );
    host.close_and_wait();
}

#[test]
fn host_browser_proxy_menu_counts_keep_readable_rows_and_reveal_the_last_choice() {
    let mut host = BrowserGui::spawn(true, None, None);
    host.open_browser_directly();
    let mut saved = 0;
    for desired in [0usize, 1, 8] {
        if saved < desired {
            host.open_web_settings();
            while saved < desired {
                host.click_geometry("web_proxy_input");
                host.cli(&["send-ui-keys", "Ctrl+a"]);
                let endpoint = format!("http://127.0.0.1:{}", 35000 + saved);
                host.cli(&["send-ui-ime", "commit", &endpoint]);
                host.cli(&["send-ui-keys", "Enter"]);
                saved += 1;
                assert_eq!(
                    host.cli(&["ui-snapshot"])["geometry"]["web_proxy_options_count"],
                    saved
                );
            }
            host.cli(&["send-ui-keys", "Escape"]);
        }
        for (width, height) in [(540, 300), (1000, 700)] {
            host.cli(&[
                "resize-window",
                "--width",
                &width.to_string(),
                "--height",
                &height.to_string(),
            ]);
            host.click_geometry("browser_proxy_dropdown");
            let opened = host.cli(&["ui-snapshot"]);
            let geometry = &opened["geometry"];
            assert_eq!(geometry["browser_proxy_menu_open"], true);
            let rows = geometry["browser_proxy_option_rows"].as_array().unwrap();
            assert!(!rows.is_empty());
            let scale = geometry["scale_permille"].as_u64().unwrap() as f64 / 1000.0;
            // This contract assertion also detects the released nine-row
            // shrink-to-fit defect without relying on new observation keys.
            for row in rows {
                let actual = row["height"].as_u64().unwrap() as f64 / scale;
                assert!(
                    (actual - 32.0).abs() <= 1.0,
                    "proxy choices must retain a readable 32-DIP target, got {actual}"
                );
            }
            assert!(rows.len() <= desired + 1);
            if desired <= 1 {
                assert_eq!(rows.len(), desired + 1);
            }
            let menu = &geometry["browser_proxy_menu"];
            assert_eq!(
                menu["height"].as_u64().unwrap(),
                rows.iter()
                    .map(|row| row["height"].as_u64().unwrap())
                    .sum::<u64>()
                    + (8.0 * scale).round() as u64,
                "menu height must follow the actual visible choices with no unused choice rows"
            );
            for _ in 0..desired + 2 {
                host.cli(&["send-ui-keys", "Down"]);
            }
            let moved = host.cli(&["ui-snapshot"]);
            assert_eq!(moved["geometry"]["proxy_menu_highlight"], desired);
            let visible = moved["geometry"]["browser_proxy_option_indices"]
                .as_array()
                .unwrap();
            assert!(
                visible
                    .iter()
                    .any(|index| index.as_u64() == Some(desired as u64)),
                "keyboard movement must reveal the last saved choice"
            );
            if desired == 8 && height == 300 {
                assert!(moved["geometry"]["proxy_menu_scroll"].as_u64().unwrap() > 0);
            }
            host.cli(&["resize-window", "--width", "760", "--height", "420"]);
            let resized = host.cli(&["ui-snapshot"]);
            assert_eq!(
                resized["geometry"]["proxy_menu_highlight"], desired,
                "resize changed the user's highlighted action"
            );
            assert!(
                resized["geometry"]["browser_proxy_option_indices"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|index| index.as_u64() == Some(desired as u64))
            );
            host.cli(&["send-ui-keys", "Escape"]);
            assert_eq!(
                host.cli(&["ui-snapshot"])["geometry"]["browser_proxy_menu_open"],
                false
            );
        }
    }
    host.close_and_wait();
}

#[test]
fn host_browser_menu_dismiss_restores_the_original_editor_selection() {
    let mut host = BrowserGui::spawn(true, None, None);
    host.open_browser_directly();
    host.cli(&["send-ui-keys", "Ctrl+l"]);
    host.cli(&["send-ui-ime", "commit", "a中z"]);
    host.cli(&["send-ui-keys", "Home"]);
    host.cli(&["send-ui-keys", "Right"]);
    host.cli(&["send-ui-keys", "Shift+Right"]);
    let original = host_editor_state(&host, "browser_address_editor");
    assert_eq!(original["selection"], serde_json::json!([1, 4]));
    host.cli(&["send-ui-ime", "preedit", "ni"]);
    host.click_geometry("browser_proxy_dropdown");
    let menu = host.cli(&["ui-snapshot"]);
    assert_eq!(menu["geometry"]["menu_return_focus"], "address");
    assert_eq!(
        host_editor_state(&host, "browser_address_editor")["focused"],
        false
    );
    host.cli(&["send-ui-keys", "Escape"]);
    let restored = host_editor_state(&host, "browser_address_editor");
    for key in ["text", "caret", "selection"] {
        assert_eq!(
            restored[key], original[key],
            "proxy menu lost the previous field {key}"
        );
    }
    assert_eq!(restored["focused"], true);
    assert_eq!(restored["preedit"], "");
    host.cli(&["send-ui-ime", "commit", "Q"]);
    assert_eq!(
        host_editor_state(&host, "browser_address_editor")["text"],
        "aQz",
        "after dismissal the next edit must replace the original selected UTF-8 range"
    );
    host.close_and_wait();
}

#[cfg(windows)]
#[test]
#[ignore = "requires interactive foreground activation and WebView2; run the explicit native focus court"]
fn native_browser_menu_reactivation_restores_the_page_focus_without_background_activation() {
    use std::ffi::c_void;
    type Hwnd = *mut c_void;
    #[repr(C)]
    struct GuiInfo {
        size: u32,
        flags: u32,
        active: Hwnd,
        focus: Hwnd,
        capture: Hwnd,
        menu: Hwnd,
        move_size: Hwnd,
        caret: Hwnd,
        rect: [i32; 4],
    }
    struct Find {
        pid: u32,
        window: Hwnd,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(callback: unsafe extern "system" fn(Hwnd, isize) -> i32, data: isize)
        -> i32;
        fn GetWindowThreadProcessId(window: Hwnd, pid: *mut u32) -> u32;
        fn IsWindowVisible(window: Hwnd) -> i32;
        fn GetForegroundWindow() -> Hwnd;
        fn SetForegroundWindow(window: Hwnd) -> i32;
        fn GetGUIThreadInfo(thread: u32, info: *mut GuiInfo) -> i32;
        fn IsChild(parent: Hwnd, child: Hwnd) -> i32;
        fn AttachThreadInput(from: u32, to: u32, attach: i32) -> i32;
        fn PeekMessageW(message: *mut c_void, window: Hwnd, min: u32, max: u32, remove: u32)
        -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThreadId() -> u32;
    }
    unsafe extern "system" fn visit(window: Hwnd, data: isize) -> i32 {
        let find = unsafe { &mut *(data as *mut Find) };
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(window, &mut pid) };
        if pid == find.pid && unsafe { IsWindowVisible(window) } != 0 {
            find.window = window;
            0
        } else {
            1
        }
    }
    fn owned_window(pid: u32) -> Hwnd {
        let mut find = Find {
            pid,
            window: std::ptr::null_mut(),
        };
        unsafe { EnumWindows(visit, &mut find as *mut Find as isize) };
        assert!(
            !find.window.is_null(),
            "the owned court GUI must have a visible native window"
        );
        find.window
    }
    fn focus() -> Hwnd {
        let mut info: GuiInfo = unsafe { std::mem::zeroed() };
        info.size = std::mem::size_of::<GuiInfo>() as u32;
        assert_ne!(
            unsafe { GetGUIThreadInfo(0, &mut info) },
            0,
            "observe actual foreground keyboard focus"
        );
        info.focus
    }
    struct InputAttachment {
        from: u32,
        to: u32,
    }
    impl Drop for InputAttachment {
        fn drop(&mut self) {
            assert_ne!(
                unsafe { AttachThreadInput(self.from, self.to, 0) },
                0,
                "detach only the owned court input queues"
            );
        }
    }
    fn activate_owned(target: Hwnd, first: Hwnd, second: Hwnd) {
        assert!(target == first || target == second);
        // The test runner loses foreground eligibility once its first owned
        // GUI has activated. Temporarily join only these two owned GUI queues;
        // never attach to an unrelated foreground application or inject keys.
        let current = unsafe { GetCurrentThreadId() };
        let mut message = [0usize; 8];
        unsafe { PeekMessageW(message.as_mut_ptr().cast(), std::ptr::null_mut(), 0, 0, 0) };
        let mut attachments = Vec::new();
        for window in [first, second] {
            let thread = unsafe { GetWindowThreadProcessId(window, std::ptr::null_mut()) };
            assert_ne!(thread, 0);
            if current != thread
                && !attachments
                    .iter()
                    .any(|attachment: &InputAttachment| attachment.to == thread)
            {
                assert_ne!(
                    unsafe { AttachThreadInput(current, thread, 1) },
                    0,
                    "attach the test queue to an owned GUI queue"
                );
                attachments.push(InputAttachment {
                    from: current,
                    to: thread,
                });
            }
        }
        assert_ne!(
            unsafe { SetForegroundWindow(target) },
            0,
            "activate an owned court GUI through its attached input queue"
        );
        // RAII detaches on both success and panic; product code must perform
        // keyboard focus restoration itself, without a fixture SetFocus call.
        drop(attachments);
    }
    struct RestoreForeground(Hwnd);
    impl Drop for RestoreForeground {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { SetForegroundWindow(self.0) };
            }
        }
    }
    let _restore = RestoreForeground(unsafe { GetForegroundWindow() });
    let mut host = BrowserGui::spawn(false, None, None);
    let mut other = BrowserGui::spawn(true, None, None);
    let first = owned_window(host.gui.child.id());
    let second = owned_window(other.gui.child.id());
    let wait = |predicate: &dyn Fn() -> bool, label: &str| {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !predicate() {
            assert!(Instant::now() < deadline, "{label}");
            thread::sleep(Duration::from_millis(20));
        }
    };
    activate_owned(first, first, second);
    wait(
        &|| unsafe { GetForegroundWindow() == first },
        "activate browser court GUI",
    );
    host.open_browser_directly();
    let origin = LocalPreviewServer::start("127.0.0.1:0".parse().unwrap());
    let url = format!("http://{}/focus", origin.address);
    host.cli(&["send-ui-keys", "Ctrl+l"]);
    host.cli(&["send-ui-ime", "commit", &url]);
    host.cli(&["send-ui-keys", "Enter"]);
    host.wait_browser(&url, "ready", Some("Local preview ready"));
    wait(
        &|| {
            let focused = focus();
            focused != first && unsafe { IsChild(first, focused) } != 0
        },
        "the real native page must own keyboard focus before opening its menu",
    );
    host.click_geometry("browser_proxy_dropdown");
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["browser_proxy_menu_open"],
        true
    );
    wait(
        &|| focus() == first,
        "the open host menu must own the real window focus",
    );
    activate_owned(second, first, second);
    wait(
        &|| unsafe { GetForegroundWindow() == second },
        "activate the other owned court GUI",
    );
    wait(
        &|| host.cli(&["ui-snapshot"])["geometry"]["browser_proxy_menu_open"] == false,
        "app deactivation must dismiss the menu",
    );
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        assert_eq!(
            unsafe { GetForegroundWindow() },
            second,
            "a background menu close stole foreground activation"
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["menu_return_focus"],
        "page"
    );
    activate_owned(first, first, second);
    wait(
        &|| {
            let focused = focus();
            focused != first && unsafe { IsChild(first, focused) } != 0
        },
        "reactivation must return actual keyboard focus to the embedded page",
    );
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["menu_return_focus"],
        "none"
    );
    host.close_and_wait();
    other.close_and_wait();
}

fn harness_snapshot(host: &BrowserGui) -> Value {
    host.cli(&["ui-snapshot"])["geometry"]["harness_settings"].clone()
}

fn wait_harness(host: &BrowserGui, condition: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        let state = harness_snapshot(host);
        if condition(&state) {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "Harness condition timed out: {state}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn open_harness(host: &BrowserGui) -> Value {
    if host.cli(&["ui-snapshot"])["settings_open"] != true {
        host.click_geometry("settings");
    }
    host.click_geometry("settings_harness_route");
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["settings_section"],
        "harness"
    );
    // Busy is an operation state, not proof that account/configuration loading
    // succeeded. Never corrupt a destination for a save test before this gate.
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        let snapshot = host.cli(&["ui-snapshot"]);
        let metadata = &snapshot["geometry"]["harness_workspace"]["metadata"];
        let state = &snapshot["geometry"]["harness_settings"];
        assert_ne!(
            metadata, "unavailable",
            "Harness fixture initialization failed: {state}"
        );
        if metadata == "loaded" && state["busy"] == false {
            return state.clone();
        }
        assert!(
            Instant::now() < deadline,
            "Harness fixture did not finish loading: {snapshot}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn harness_orientation_links_do_not_start_login_listener_or_key_creation() {
    let mut host = BrowserGui::spawn(true, None, None);
    let initial = open_harness(&host);
    assert_eq!(initial["section"], "accounts");
    assert_eq!(initial["next_step"], "connect_account");
    assert_eq!(
        initial["configured_clients"], 0,
        "the empty default is not a configured client"
    );
    assert_eq!(initial["scroll"], 0, "first use must begin at the overview");
    let first_row = &initial["rows"][0];
    assert_eq!(first_row["id"], "overview_0");
    let shot = host
        .gui
        .screenshot
        .with_file_name("orientation.png")
        .to_string_lossy()
        .into_owned();
    host.cli(&["screenshot-pane", "--output", &shot]);
    host.cli(&["send-ui-keys", "Tab"]);
    assert_eq!(harness_snapshot(&host)["overview_focused"], true);
    host.cli(&["send-ui-keys", "Enter"]);
    let next = harness_snapshot(&host);
    assert!(
        next["scroll"].as_u64().unwrap() > 0,
        "Next must reveal the form body"
    );
    assert!(
        next["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == "accounts")
    );
    for (link, destination) in [
        ("overview_allocations", "model"),
        ("overview_monitoring", "monitoring"),
        ("overview_service", "local_api"),
        ("overview_accounts", "accounts"),
    ] {
        click_harness(&host, link, 0);
        let state = harness_snapshot(&host);
        assert!(
            state["rows"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == destination),
            "link {link} must reveal {destination}: {state}"
        );
        assert_eq!(state["next_step"], "connect_account");
        assert_eq!(state["login_active"], false);
        assert_eq!(state["listener"], "stopped");
        assert_eq!(state["local_key_exists"], false);
        assert!(state["accounts"].as_array().unwrap().is_empty());
        if link == "overview_service" {
            host.cli(&["send-ui-keys", "Tab", "Shift+Tab"]);
            assert_eq!(harness_snapshot(&host)["port_editor"]["focused"], true);
            host.cli(&["send-ui-keys", "Tab"]);
            assert_eq!(
                harness_snapshot(&host)["overview_focused"],
                true,
                "the service form must wrap to the overview"
            );
        } else if link == "overview_monitoring" {
            host.cli(&["send-ui-keys", "Shift+Tab"]);
            assert_eq!(
                harness_snapshot(&host)["overview_focused"],
                true,
                "reverse traversal from monitoring must return to visible section links"
            );
        }
    }
    host.click_geometry("settings");
    let reopened = open_harness(&host);
    assert_eq!(reopened["next_step"], "connect_account");
    assert_eq!(reopened["login_active"], false);
    assert_eq!(reopened["listener"], "stopped");
    host.close_and_wait();
}

fn click_harness(host: &BrowserGui, id: &str, column: usize) {
    let snapshot = host.cli(&["ui-snapshot"]);
    let state = &snapshot["geometry"]["harness_settings"];
    if state["navigation"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["id"] == id)
    {
        if state["navigation_mode"] == "selector" && state["section_picker_open"] == false {
            host.click_rect(
                &state["section_selector"],
                "Harness section selector",
                &snapshot["geometry"],
            );
        }
        let snapshot = host.cli(&["ui-snapshot"]);
        let nav = snapshot["geometry"]["harness_settings"]["navigation"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == id)
            .unwrap();
        assert_eq!(nav["visible"], true);
        host.click_rect(&nav["rect"], id, &snapshot["geometry"]);
        return;
    }
    if !id.starts_with("overview_") {
        let (section, link) = if id.starts_with("monitoring") {
            ("monitoring", "overview_monitoring")
        } else if matches!(
            id,
            "port" | "listener" | "listener_actions" | "local_api" | "status"
        ) {
            ("service", "overview_service")
        } else if id.starts_with("model")
            || id.starts_with("connection")
            || id.starts_with("key_")
            || id.starts_with("recipe")
            || id.starts_with("probe_")
            || matches!(id, "connect" | "openai_base_url" | "anthropic_base_url")
        {
            ("clients", "overview_allocations")
        } else {
            ("accounts", "overview_accounts")
        };
        if harness_snapshot(host)["section"] != section {
            click_harness(host, link, 0);
            assert_eq!(harness_snapshot(host)["section"], section);
        }
    }
    // Find a published target by navigating the real panel. The court never
    // re-derives product row order, position, count or hit geometry.
    for _ in 0..80 {
        let snapshot = host.cli(&["ui-snapshot"]);
        let geometry = &snapshot["geometry"];
        let harness = &geometry["harness_settings"];
        if harness["scroll"] == 0 {
            break;
        }
        host.click_rect(&harness["previous"], "Harness Previous", geometry);
    }
    for _ in 0..80 {
        let snapshot = host.cli(&["ui-snapshot"]);
        let geometry = &snapshot["geometry"];
        let harness = &geometry["harness_settings"];
        if let Some(row) = harness["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == id)
        {
            host.click_rect(&row["cells"][column], id, geometry);
            return;
        }
        host.click_rect(&harness["next"], "Harness Next", geometry);
    }
    panic!("Harness target {id} never became reachable");
}

pub(super) fn harness_sections_preserve_drafts_and_hide_unselected_controls() {
    fn form_ids(host: &BrowserGui) -> std::collections::BTreeSet<String> {
        let mut ids = std::collections::BTreeSet::new();
        for _ in 0..80 {
            let snapshot = host.cli(&["ui-snapshot"]);
            let geometry = &snapshot["geometry"];
            let state = &geometry["harness_settings"];
            if state["scroll"] == 0 {
                break;
            }
            host.click_rect(&state["previous"], "Harness Previous", geometry);
        }
        for _ in 0..80 {
            let snapshot = host.cli(&["ui-snapshot"]);
            let geometry = &snapshot["geometry"];
            let state = &geometry["harness_settings"];
            ids.extend(
                state["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| row["id"].as_str().unwrap().to_owned()),
            );
            host.click_rect(&state["next"], "Harness Next", geometry);
            if harness_snapshot(host)["scroll"] == state["scroll"] {
                return ids;
            }
        }
        panic!("isolated empty Harness form did not reach its end");
    }
    let mut host = BrowserGui::spawn(true, None, None);
    open_harness(&host);
    for (link, section, present, absent) in [
        (
            "overview_accounts",
            "accounts",
            "connect_label",
            ["model_input", "port", "monitoring"],
        ),
        (
            "overview_allocations",
            "clients",
            "model_input",
            ["connect_label", "port", "monitoring"],
        ),
        (
            "overview_monitoring",
            "monitoring",
            "monitoring",
            ["connect_label", "port", "model_input"],
        ),
        (
            "overview_service",
            "service",
            "port",
            ["connect_label", "model_input", "monitoring"],
        ),
    ] {
        click_harness(&host, link, 0);
        assert_eq!(harness_snapshot(&host)["section"], section);
        let ids = form_ids(&host);
        assert!(
            ids.contains(present),
            "missing {section} control {present}: {ids:?}"
        );
        for hidden in absent {
            assert!(
                !ids.contains(hidden),
                "hidden {hidden} remains presented in {section}"
            );
        }
    }
    click_harness(&host, "model_input", 0);
    host.cli(&["send-ui-keys", "Ctrl+a"]);
    host.cli(&["send-ui-ime", "commit", "section-model-draft"]);
    click_harness(&host, "port", 0);
    host.cli(&["send-ui-keys", "Ctrl+a"]);
    host.cli(&["send-ui-ime", "commit", "34567"]);
    click_harness(&host, "connect_label", 0);
    host.cli(&["send-ui-ime", "commit", "Account draft"]);
    click_harness(&host, "overview_allocations", 0);
    let clients = harness_snapshot(&host);
    assert_eq!(clients["model_editor"]["text"], "section-model-draft");
    assert_eq!(clients["port_editor"]["focused"], false);
    assert_eq!(clients["connect_draft"]["label_focused"], false);
    host.click_geometry("settings");
    let reopened = open_harness(&host);
    assert_eq!(reopened["section"], "clients");
    assert_eq!(reopened["model_editor"]["text"], "section-model-draft");
    click_harness(&host, "overview_service", 0);
    assert_eq!(harness_snapshot(&host)["port_editor"]["text"], "34567");
    host.cli(&["resize-window", "--width", "420", "--height", "300"]);
    click_harness(&host, "overview_accounts", 0);
    click_harness(&host, "connect_label", 0);
    let accounts = harness_snapshot(&host);
    let input = accounts["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "connect_label")
        .unwrap();
    assert_eq!(input["cells"].as_array().unwrap().len(), 1);
    assert_eq!(
        input["cells"][0]["width"], input["rect"]["width"],
        "account editor must use the full readable row"
    );
    assert_eq!(
        accounts["connect_draft"]["label_bytes"],
        "Account draft".len()
    );
    assert_eq!(accounts["login_active"], false);
    assert_eq!(accounts["listener"], "stopped");
    assert_eq!(accounts["local_key_exists"], false);
    assert_eq!(
        accounts["port"], 28789,
        "navigation cannot save the port draft"
    );
    assert_eq!(
        accounts["model"], "",
        "navigation cannot apply the model draft"
    );
    let shot = host
        .gui
        .screenshot
        .with_file_name("sections-narrow.png")
        .to_string_lossy()
        .into_owned();
    host.cli(&["screenshot-pane", "--output", &shot]);
    host.close_and_wait();
}

#[test]
fn host_harness_account_save_feedback_stays_with_its_draft() {
    let root = minicon_binary()
        .parent()
        .unwrap()
        .join("browser-court")
        .join(unique_suffix())
        .join("config");
    harness_synthetic_profile(
        &root,
        28789,
        "synthetic-feedback-key",
        "synthetic-feedback-token",
    );
    harness_synthetic_pool(&root);
    let mut host = BrowserGui::spawn_with_config(true, None, None, Some(root));
    open_harness(&host);
    click_harness(&host, "port", 0);
    host.cli(&["send-ui-keys", "Ctrl+a"]);
    host.cli(&["send-ui-ime", "commit", "1"]);
    assert_eq!(harness_snapshot(&host)["port_editor"]["text"], "1");
    click_harness(&host, "port", 1);
    let port_error = harness_snapshot(&host)["status"].clone();
    assert_eq!(port_error, "Enter a port from 1024 to 65535.");
    click_harness(&host, "account_0", 0);
    click_harness(&host, "account_login_proxy", 0);
    host.cli(&["send-ui-keys", "Ctrl+a"]);
    host.cli(&["send-ui-ime", "commit", "invalid-private-proxy"]);
    click_harness(&host, "account_edit_actions", 0);
    assert_eq!(
        harness_snapshot(&host)["account_save_state"],
        "invalid_login_proxy"
    );
    click_harness(&host, "account_save_feedback", 0);
    let shot = host
        .gui
        .screenshot
        .with_file_name("account-save-feedback.png")
        .to_string_lossy()
        .into_owned();
    host.cli(&["screenshot-pane", "--output", &shot]);
    click_harness(&host, "account_1", 0);
    assert_eq!(harness_snapshot(&host)["account_save_state"], "idle");
    click_harness(&host, "account_0", 0);
    assert_eq!(
        harness_snapshot(&host)["account_save_state"],
        "invalid_login_proxy"
    );
    click_harness(&host, "account_login_proxy", 0);
    host.cli(&["send-ui-keys", "Ctrl+a"]);
    host.cli(&["send-ui-keys", "Backspace"]);
    click_harness(&host, "account_label", 0);
    host.cli(&["send-ui-keys", "Ctrl+a"]);
    host.cli(&["send-ui-ime", "commit", "Saved feedback account"]);
    click_harness(&host, "account_edit_actions", 0);
    let saved = wait_harness(&host, |value| value["account_save_state"] == "saved");
    assert_eq!(saved["account_draft_dirty"], false);
    assert_eq!(
        saved["status"], port_error,
        "account save cannot erase another operation's failure"
    );
    click_harness(&host, "account_label", 0);
    host.cli(&["send-ui-ime", "commit", " later"]);
    assert_eq!(
        harness_snapshot(&host)["account_save_state"],
        "saved_with_draft"
    );
    click_harness(&host, "account_edit_actions", 1);
    let reverted = harness_snapshot(&host);
    assert_eq!(reverted["account_save_state"], "idle");
    assert_eq!(reverted["status"], port_error);
    assert_eq!(reverted["listener"], "stopped");
    assert_eq!(reverted["login_active"], false);
    let public = serde_json::to_string(&reverted).unwrap();
    for private in [
        "invalid-private-proxy",
        "Saved feedback account",
        "synthetic-feedback-token",
    ] {
        assert!(!public.contains(private));
    }
    host.close_and_wait();
}

fn harness_private_file(path: &std::path::Path, bytes: &[u8]) {
    let parent = path.parent().unwrap();
    fs::create_dir_all(parent).unwrap();
    minicon_platform::filesystem::protect_private_directory(parent).unwrap();
    minicon_platform::filesystem::write_private_atomic(path, bytes).unwrap();
}

fn harness_synthetic_profile(root: &std::path::Path, port: u16, key: &str, token: &str) {
    // This private synthetic binding qualifies local UI/listener behavior.
    // No upstream request or provider authorization evidence is claimed.
    let harness = root.join("harness");
    harness_private_file(
        &harness.join("gateway.json"),
        &serde_json::to_vec(&serde_json::json!({
            "version":1,"port":port,"selected_account":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "model":"deepseek/fixture-model","identity":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "models":[{"id":"deepseek/fixture-model","model_id":"fixture-model",
                "provider":"deepseek","protocol":"Messages",
                "url":"https://api.deepseek.com/anthropic/v1/messages","headers":[]}]
        }))
        .unwrap(),
    );
    harness_private_file(&harness.join("accounts/accounts.json"), br#"{"version":1,"accounts":[{"id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","provider":"deepseek","label":"Synthetic court","state":"AccountReady"}]}"#);
    harness_private_file(
        &harness.join("accounts/credentials/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json"),
        &serde_json::to_vec(&serde_json::json!({"version":1,"provider":"deepseek",
            "access_token":token,"refresh_token":null,"expires_unix_ms":null,"organization":null}))
        .unwrap(),
    );
    harness_private_file(&harness.join("client-key"), key.as_bytes());
}

fn harness_synthetic_pool(root: &std::path::Path) {
    let base = root.join("harness/accounts");
    let ids = [
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "cccccccccccccccccccccccccccccccc",
    ];
    let accounts: Vec<_> = ids.iter().enumerate().map(|(i, id)| serde_json::json!({
        "id": id, "provider": if i == 2 { "opencode" } else { "deepseek" },
        "label": format!("Synthetic account {i}"), "state": if i == 2 { "CredentialStored" } else { "AccountReady" },
        "preferences": { "context_id": id, "service_generation": id, "enabled": i != 1,
            "login": if i == 1 { serde_json::json!({"Proxy":"http://127.0.0.1:19199"}) } else { serde_json::json!("Direct") }, "service": "Direct" }
    })).collect();
    harness_private_file(
        &base.join("accounts.json"),
        &serde_json::to_vec(&serde_json::json!({"version":1,"accounts":accounts})).unwrap(),
    );
    for (i, id) in ids.iter().enumerate().skip(1) {
        harness_private_file(&base.join("credentials").join(format!("{id}.json")), &serde_json::to_vec(&serde_json::json!({
            "version":1,"provider":if i==2 {"opencode"} else {"deepseek"},"access_token":"synthetic-unused-token",
            "refresh_token":null,"expires_unix_ms":null,"organization":null
        })).unwrap());
    }
}

#[test]
fn host_harness_account_removal_reviews_cancel_and_exact_confirmation() {
    let root = minicon_binary()
        .parent()
        .unwrap()
        .join("browser-court")
        .join(unique_suffix())
        .join("config");
    harness_synthetic_profile(&root, 28789, "removal-local-key", "removal-unused-token");
    harness_synthetic_pool(&root);
    let mut host = BrowserGui::spawn_with_config(true, None, None, Some(root));
    let before = open_harness(&host);
    click_harness(&host, "account_2", 1);
    let menu = harness_snapshot(&host);
    assert_eq!(menu["account_menu"], "cccccccccccccccccccccccccccccccc");
    assert!(
        menu["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["active_operations"] == 0),
        "loaded idle accounts need actual zero samples, not unknown or a missing projection"
    );
    assert_eq!(menu["account_removal"]["state"], "closed");
    assert_eq!(menu["accounts"], before["accounts"]);
    assert_eq!(menu["login_active"], false);
    host.cli(&["send-ui-keys", "Enter"]);
    assert_eq!(harness_snapshot(&host)["account_menu"], Value::Null);
    // Escape restores the named Actions target; Enter can reopen without a mouse.
    host.cli(&["send-ui-keys", "Enter"]);
    assert_eq!(
        harness_snapshot(&host)["account_menu"],
        menu["account_menu"]
    );
    host.cli(&["send-ui-keys", "Escape"]);
    host.cli(&["send-ui-keys", "Enter"]);
    let menu_shot = host
        .gui
        .screenshot
        .with_file_name("account-actions-menu.png");
    host.cli(&["screenshot-pane", "--output", &menu_shot.to_string_lossy()]);
    host.cli(&["send-ui-keys", "Tab"]);
    host.cli(&["send-ui-keys", "Enter"]);
    let toggled = harness_snapshot(&host);
    assert_eq!(toggled["account_menu"], Value::Null);
    assert_eq!(toggled["account_draft_dirty"], true);
    assert_eq!(
        toggled["pool"], before["pool"],
        "toggle cannot save account policy implicitly"
    );
    click_harness(&host, "account_0", 0);
    assert_eq!(harness_snapshot(&host)["account_draft_dirty"], false);
    click_harness(&host, "account_2", 1);
    assert_eq!(harness_snapshot(&host)["account_draft_dirty"], true);
    click_harness(&host, "account_menu_reconnect", 0);
    assert_eq!(harness_snapshot(&host)["login_active"], false);
    assert_eq!(
        harness_snapshot(&host)["account_menu"],
        menu["account_menu"]
    );
    click_harness(&host, "account_menu_close", 0);
    click_harness(&host, "account_edit_actions", 1);
    assert_eq!(harness_snapshot(&host)["account_draft_dirty"], false);
    click_harness(&host, "account_2", 1);
    click_harness(&host, "account_menu_remove", 0);
    let review = harness_snapshot(&host);
    assert_eq!(
        review["accounts"], before["accounts"],
        "first click must never remove"
    );
    assert_eq!(review["account_removal"]["state"], "review");
    assert_eq!(review["account_removal"]["affected_clients"], 0);
    assert_eq!(review["login_active"], false);
    assert_eq!(review["listener"], "stopped");
    assert!(!review.to_string().contains("Synthetic account"));
    host.cli(&["send-ui-keys", "Enter"]);
    assert_eq!(
        harness_snapshot(&host)["account_removal"]["state"],
        "closed"
    );
    click_harness(&host, "account_0", 1);
    click_harness(&host, "account_menu_remove", 0);
    assert_eq!(
        harness_snapshot(&host)["account_removal"]["affected_clients"],
        1
    );
    let shot = host
        .gui
        .screenshot
        .with_file_name("account-removal-review.png");
    host.cli(&["screenshot-pane", "--output", &shot.to_string_lossy()]);
    host.cli(&["send-ui-keys", "Escape"]);
    assert_eq!(
        harness_snapshot(&host)["account_removal"]["state"],
        "closed"
    );
    assert_eq!(host.cli(&["ui-snapshot"])["settings_open"], true);
    click_harness(&host, "account_2", 1);
    click_harness(&host, "account_menu_remove", 0);
    click_harness(&host, "overview_service", 0);
    click_harness(&host, "overview_accounts", 0);
    assert_eq!(
        harness_snapshot(&host)["account_removal"]["state"],
        "closed"
    );
    click_harness(&host, "account_2", 1);
    click_harness(&host, "account_menu_remove", 0);
    let snapshot = host.cli(&["ui-snapshot"]);
    host.click_rect(
        &snapshot["geometry"]["harness_settings"]["close"],
        "close review",
        &snapshot["geometry"],
    );
    assert_eq!(open_harness(&host)["account_removal"]["state"], "closed");
    click_harness(&host, "account_2", 1);
    click_harness(&host, "account_menu_remove", 0);
    assert_eq!(
        harness_snapshot(&host)["account_removal"]["affected_clients"],
        0
    );
    // OpenCode has no remote logout endpoint: this synthetic removal is local.
    click_harness(&host, "account_removal_confirm", 0);
    let after = wait_harness(&host, |h| {
        h["accounts"].as_array().is_some_and(|a| a.len() == 2) && h["login_active"] == false
    });
    assert_eq!(
        after["accounts"],
        Value::Array(before["accounts"].as_array().unwrap()[..2].to_vec())
    );
    assert_eq!(after["selected_account"], before["selected_account"]);
    assert_eq!(after["listener"], "stopped");
    host.close_and_wait();
}

#[test]
fn host_harness_pool_states_and_exact_account_monitoring_preserve_bindings() {
    let root = minicon_binary()
        .parent()
        .unwrap()
        .join("browser-court")
        .join(unique_suffix())
        .join("config");
    harness_synthetic_profile(
        &root,
        28789,
        "synthetic-pool-local-key",
        "synthetic-pool-token",
    );
    harness_synthetic_pool(&root);
    let ids = [
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "cccccccccccccccccccccccccccccccc",
    ];
    let mut host = BrowserGui::spawn_with_config(true, None, None, Some(root));
    let before = open_harness(&host);
    assert_eq!(
        before["pool"],
        serde_json::json!({"previously_tested":1,"stored":1,"disabled":1,"policy_unknown":0,"reading_attention":0,"readings_unknown":3})
    );
    click_harness(&host, "account_1", 0);
    let selected = harness_snapshot(&host);
    assert_eq!(selected["account_focus"], ids[1]);
    click_harness(&host, "account_detail_0", 0);
    let shot = host
        .gui
        .screenshot
        .with_file_name("pool-account.png")
        .to_string_lossy()
        .into_owned();
    host.cli(&["screenshot-pane", "--output", &shot]);
    click_harness(&host, "account_monitoring", 0);
    let after = harness_snapshot(&host);
    assert_eq!(after["section"], "monitoring");
    assert_eq!(
        after["monitoring"]["selected_account"], ids[1],
        "the link must select the same account, not the first account"
    );
    assert_eq!(after["monitoring"]["pending"], Value::Null);
    assert_eq!(after["pool"], before["pool"]);
    assert_eq!(after["selected_account"], before["selected_account"]);
    assert_eq!(after["model"], before["model"]);
    assert_eq!(after["listener"], "stopped");
    assert_eq!(after["login_active"], false);
    let serialized = serde_json::to_string(&after).unwrap();
    for private in [
        "synthetic-pool-token",
        "synthetic-unused-token",
        "Synthetic account",
        "http://127.0.0.1:19199",
    ] {
        assert!(
            !serialized.contains(private),
            "public pool observation disclosed private account data"
        );
    }
    host.close_and_wait();
}

#[test]
fn host_harness_dashboard_reuses_account_state_and_targets_named_client() {
    let root = minicon_binary()
        .parent()
        .unwrap()
        .join("browser-court")
        .join(unique_suffix())
        .join("config");
    harness_synthetic_profile(
        &root,
        28789,
        "synthetic-dashboard-key",
        "synthetic-dashboard-token",
    );
    let gateway = root.join("harness/gateway.json");
    let mut config: Value = serde_json::from_slice(&fs::read(&gateway).unwrap()).unwrap();
    config["version"] = 2.into();
    config["profiles"] = serde_json::json!([{"id":"dddddddddddddddddddddddddddddddd","label":"Named work","enabled":true,"default_model":"work",
        "allocations":[{"alias":"work","account":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","route":"deepseek/fixture-model"}]}]);
    harness_private_file(&gateway, &serde_json::to_vec(&config).unwrap());
    let registry = root.join("harness/accounts/accounts.json");
    let mut accounts: Value = serde_json::from_slice(&fs::read(&registry).unwrap()).unwrap();
    accounts["accounts"][0]["preferences"] = serde_json::json!({"context_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "service_generation":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","enabled":false,"login":"Direct","service":"Direct"});
    harness_private_file(&registry, &serde_json::to_vec(&accounts).unwrap());
    let mut host = BrowserGui::spawn_with_config(true, None, None, Some(root));
    let before = open_harness(&host);
    assert_eq!(before["pool"]["disabled"], 1);
    host.click_geometry("settings");
    host.click_geometry("harness_entry");
    let snapshot = host.cli(&["ui-snapshot"]);
    host.click_rect(
        &snapshot["geometry"]["harness_workspace"]["navigation"][1],
        "Accounts navigation",
        &snapshot["geometry"],
    );
    let snapshot = host.cli(&["ui-snapshot"]);
    let workspace = &snapshot["geometry"]["harness_workspace"];
    assert!(
        workspace["cards"][0]["lines"][0]
            .as_str()
            .unwrap()
            .contains("Disabled")
    );
    host.click_rect(
        &workspace["navigation"][2],
        "Clients navigation",
        &snapshot["geometry"],
    );
    let snapshot = host.cli(&["ui-snapshot"]);
    let workspace = &snapshot["geometry"]["harness_workspace"];
    let card = workspace["cards"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "client:dddddddddddddddddddddddddddddddd")
        .expect("named applied client card");
    assert_eq!(card["title"], "Named work");
    assert!(
        card["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line.as_str().unwrap().contains("fixture-model"))
    );
    host.click_rect(&card["rect"], "Named client card", &snapshot["geometry"]);
    let shot = host
        .gui
        .screenshot
        .with_file_name("dashboard-clients.png")
        .to_string_lossy()
        .into_owned();
    host.cli(&["screenshot-pane", "--output", &shot]);
    host.click_rect(
        &workspace["settings"],
        "Configure selected client",
        &snapshot["geometry"],
    );
    let after = wait_harness(&host, |s| {
        s["connection_profile"] == "dddddddddddddddddddddddddddddddd"
    });
    assert_eq!(after["section"], "clients");
    assert_eq!(after["selected_account"], before["selected_account"]);
    assert_eq!(after["model"], before["model"]);
    assert_eq!(after["listener"], "stopped");
    assert_eq!(after["login_active"], false);
    assert!(
        !serde_json::to_string(&after)
            .unwrap()
            .contains("synthetic-dashboard-token")
    );
    host.close_and_wait();
}

#[test]
fn host_harness_explanations_reflow_through_the_same_pager_and_inert_hits() {
    fn scan(host: &BrowserGui, require_wrapped: bool) -> usize {
        use std::collections::{BTreeMap, BTreeSet};
        for _ in 0..60 {
            let snapshot = host.cli(&["ui-snapshot"]);
            let h = &snapshot["geometry"]["harness_settings"];
            if h["scroll"] == 0 {
                break;
            }
            host.click_rect(&h["previous"], "Harness Previous", &snapshot["geometry"]);
        }
        assert_eq!(harness_snapshot(host)["scroll"], 0);
        let mut coverage: BTreeMap<String, (usize, BTreeSet<usize>)> = BTreeMap::new();
        let mut hit_continuation = false;
        for _ in 0..60 {
            let snapshot = host.cli(&["ui-snapshot"]);
            let h = &snapshot["geometry"]["harness_settings"];
            for row in h["rows"].as_array().unwrap() {
                let text = &row["text_line"];
                if text.is_null() {
                    continue;
                }
                assert_eq!(text["fits"], true, "clipped explanation: {row}");
                let source = text["source"].as_str().unwrap().to_owned();
                let count = text["line_count"].as_u64().unwrap() as usize;
                let (expected, seen) = coverage.entry(source).or_insert((count, BTreeSet::new()));
                assert_eq!(*expected, count);
                seen.insert(text["line"].as_u64().unwrap() as usize);
                if text["line"].as_u64().unwrap() > 0 && !hit_continuation {
                    let before = harness_snapshot(host);
                    host.click_rect(
                        &row["rect"],
                        "read-only continuation",
                        &snapshot["geometry"],
                    );
                    let after = harness_snapshot(host);
                    for field in [
                        "section",
                        "scroll",
                        "listener",
                        "login_active",
                        "local_key_exists",
                        "model",
                        "port",
                    ] {
                        assert_eq!(
                            after[field], before[field],
                            "text dispatched action: {field}"
                        );
                    }
                    hit_continuation = true;
                }
            }
            host.click_rect(&h["next"], "Harness Next", &snapshot["geometry"]);
            if harness_snapshot(host)["scroll"] == h["scroll"] {
                break;
            }
        }
        let mut total = 0;
        for (source, (count, seen)) in &coverage {
            assert_eq!(
                *seen,
                (0..*count).collect(),
                "missing explanation lines: {source}"
            );
            total += count;
        }
        assert!(
            !require_wrapped || hit_continuation,
            "narrow form must expose actual wrapped lines"
        );
        total
    }
    let mut host = BrowserGui::spawn(true, None, None);
    open_harness(&host);
    host.cli(&["resize-window", "--width", "300", "--height", "380"]);
    click_harness(&host, "overview_accounts", 0);
    let narrow_lines = scan(&host, true);
    let shot = host
        .gui
        .screenshot
        .with_file_name("explanations-narrow.png");
    host.cli(&["screenshot-pane", "--output", &shot.to_string_lossy()]);
    host.cli(&["resize-window", "--width", "1000", "--height", "700"]);
    click_harness(&host, "overview_accounts", 0);
    let wide_lines = scan(&host, false);
    assert!(
        narrow_lines > wide_lines,
        "reflow must follow the actual available width: narrow {narrow_lines}, wide {wide_lines}"
    );
    click_harness(&host, "model_input", 0);
    host.cli(&["send-ui-ime", "commit", "reflow-draft-中"]);
    host.cli(&["resize-window", "--width", "300", "--height", "380"]);
    host.cli(&["send-ui-ime", "commit", "-retained"]);
    assert_eq!(
        harness_snapshot(&host)["model_editor"]["text"],
        "reflow-draft-中-retained"
    );
    let snapshot = host.cli(&["ui-snapshot"]);
    assert!(
        snapshot["geometry"]["harness_settings"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == "model_input"),
        "focused editor must remain visible after reflow"
    );
    host.close_and_wait();
}

#[test]
fn host_harness_persistent_navigation_keeps_picker_input_and_drafts_local() {
    fn resized(host: &BrowserGui, width: &str, mode: &str) -> Value {
        host.cli(&["resize-window", "--width", width, "--height", "700"]);
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let snapshot = host.cli(&["ui-snapshot"]);
            if snapshot["geometry"]["harness_settings"]["navigation_mode"] == mode {
                return snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "navigation did not adapt: {snapshot}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
    let mut host = BrowserGui::spawn(true, None, None);
    open_harness(&host);
    resized(&host, "1000", "rail");
    click_harness(&host, "model_input", 0);
    host.cli(&["send-ui-keys", "Ctrl+a"]);
    host.cli(&["send-ui-ime", "commit", "navigation-draft-中"]);
    let wide = host.cli(&["ui-snapshot"]);
    let nav = &wide["geometry"]["harness_settings"]["navigation"];
    assert!(nav.as_array().unwrap().iter().all(|n| n["visible"] == true));
    let shot = host.gui.screenshot.with_file_name("navigation-rail.png");
    host.cli(&["screenshot-pane", "--output", &shot.to_string_lossy()]);
    let narrow = resized(&host, "420", "selector");
    let h = &narrow["geometry"]["harness_settings"];
    assert!(
        h["navigation"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["visible"] == false)
    );
    host.click_rect(
        &h["section_selector"],
        "section selector",
        &narrow["geometry"],
    );
    assert_eq!(harness_snapshot(&host)["section_picker_open"], true);
    host.cli(&["send-ui-ime", "commit", "must-not-edit-hidden-field"]);
    assert_eq!(
        harness_snapshot(&host)["model_editor"]["text"],
        "navigation-draft-中"
    );
    // Four menu items wrap within the picker, never entering the form below it.
    for _ in 0..4 {
        host.cli(&["send-ui-keys", "Tab"]);
    }
    let menu = host.cli(&["ui-snapshot"]);
    assert!(
        menu["geometry"]["harness_settings"]["navigation"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["id"] == "overview_allocations" && n["focused"] == true)
    );
    let shot = host.gui.screenshot.with_file_name("navigation-picker.png");
    host.cli(&["screenshot-pane", "--output", &shot.to_string_lossy()]);
    let scroll = harness_snapshot(&host)["scroll"].clone();
    host.click_rect(
        &menu["geometry"]["harness_settings"]["next"],
        "dismiss picker over pager",
        &menu["geometry"],
    );
    assert_eq!(harness_snapshot(&host)["section_picker_open"], false);
    assert_eq!(harness_snapshot(&host)["scroll"], scroll);
    host.cli(&["send-ui-keys", "Tab"]);
    host.cli(&["send-ui-keys", "Enter"]);
    assert_eq!(harness_snapshot(&host)["section_picker_open"], true);
    host.cli(&["send-ui-keys", "Down"]);
    host.cli(&["send-ui-keys", "Enter"]);
    let chosen = harness_snapshot(&host);
    assert_eq!(chosen["section"], "monitoring");
    assert_eq!(chosen["section_picker_open"], false);
    let snapshot = host.cli(&["ui-snapshot"]);
    host.click_rect(
        &snapshot["geometry"]["harness_settings"]["section_selector"],
        "section selector",
        &snapshot["geometry"],
    );
    host.cli(&["send-ui-keys", "Escape"]);
    let dismissed = host.cli(&["ui-snapshot"]);
    assert_eq!(dismissed["settings_open"], true);
    assert_eq!(harness_snapshot(&host)["section_picker_open"], false);
    assert_eq!(harness_snapshot(&host)["section_selector_focused"], true);
    host.cli(&["send-ui-keys", "Enter"]);
    assert_eq!(harness_snapshot(&host)["section_picker_open"], true);
    let reopened = resized(&host, "1000", "rail");
    assert_eq!(
        reopened["geometry"]["harness_settings"]["section_picker_open"],
        false
    );
    let final_state = harness_snapshot(&host);
    assert_eq!(final_state["section"], "monitoring");
    assert_eq!(final_state["model_editor"]["text"], "navigation-draft-中");
    assert_eq!(final_state["listener"], "stopped");
    assert_eq!(final_state["login_active"], false);
    assert_eq!(final_state["local_key_exists"], false);
    host.close_and_wait();
}

#[test]
fn host_harness_responsive_panel_close_preserves_drafts_and_running_service() {
    fn settle(host: &BrowserGui, wide: bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let snapshot = host.cli(&["ui-snapshot"]);
            let g = &snapshot["geometry"];
            let scale = g["scale_permille"].as_u64().unwrap() as f64 / 1000.0;
            let width = g["frame"]["width"].as_u64().unwrap();
            let panel = &g["harness_settings"]["panel"];
            let expected = ((760.0 * scale).round() as u64).min(width);
            let logical = width as f64 / scale;
            if panel["width"].as_u64() == Some(expected)
                && ((wide && logical >= 800.0) || (!wide && logical <= 500.0))
            {
                return snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "Harness width failed to settle: {snapshot}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
    fn service_responds(port: u16) {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        write!(stream, "GET /v1/models HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer synthetic-responsive-key\r\nConnection: close\r\n\r\n").unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        assert!(
            reply.starts_with("HTTP/1.1 200"),
            "closing Settings interrupted the local service: {reply}"
        );
    }
    let reservation = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = reservation.local_addr().unwrap().port();
    let root = minicon_binary()
        .parent()
        .unwrap()
        .join("browser-court")
        .join(unique_suffix())
        .join("config");
    harness_synthetic_profile(
        &root,
        port,
        "synthetic-responsive-key",
        "synthetic-unused-responsive-token",
    );
    drop(reservation);
    let mut host = BrowserGui::spawn_with_config(true, None, None, Some(root));
    let before = open_harness(&host);
    host.cli(&["resize-window", "--width", "1000", "--height", "700"]);
    let wide = settle(&host, true);
    let g = &wide["geometry"];
    let close = &g["harness_settings"]["close"];
    for route in [
        "settings_console_route",
        "settings_web_route",
        "settings_harness_route",
    ] {
        assert!(
            g[route]["x"].as_u64().unwrap() + g[route]["width"].as_u64().unwrap()
                <= close["x"].as_u64().unwrap()
        );
    }
    click_harness(&host, "model_input", 0);
    host.cli(&["send-ui-keys", "Ctrl+a"]);
    host.cli(&["send-ui-ime", "commit", "responsive-model-draft"]);
    click_harness(&host, "port", 0);
    let draft_port = if port == 34567 { "34568" } else { "34567" };
    host.cli(&["send-ui-keys", "Ctrl+a"]);
    host.cli(&["send-ui-ime", "commit", draft_port]);
    click_harness(&host, "listener_actions", 0);
    wait_harness(&host, |s| s["listener"] == "listening");
    let open = host.cli(&["ui-snapshot"]);
    let shot = host
        .gui
        .screenshot
        .with_file_name("responsive-wide.png")
        .to_string_lossy()
        .into_owned();
    host.cli(&["screenshot-pane", "--output", &shot]);
    host.click_rect(
        &open["geometry"]["harness_settings"]["close"],
        "Harness Close",
        &open["geometry"],
    );
    assert_eq!(host.cli(&["ui-snapshot"])["settings_open"], false);
    service_responds(port);
    let reopened = open_harness(&host);
    assert_eq!(reopened["port_editor"]["text"], draft_port);
    assert_eq!(reopened["model_editor"]["text"], "responsive-model-draft");
    assert_eq!(reopened["selected_account"], before["selected_account"]);
    assert_eq!(reopened["model"], before["model"]);
    host.cli(&["resize-window", "--width", "420", "--height", "300"]);
    let narrow = settle(&host, false);
    host.click_rect(
        &narrow["geometry"]["harness_settings"]["close"],
        "Harness Close narrow",
        &narrow["geometry"],
    );
    assert_eq!(host.cli(&["ui-snapshot"])["settings_open"], false);
    service_responds(port);
    open_harness(&host);
    click_harness(&host, "listener_actions", 1);
    wait_harness(&host, |s| s["listener"] == "stopped");
    host.close_and_wait();
}

#[test]
fn host_harness_routes_preserve_independent_drafts_and_scroll_readable_controls() {
    let mut host = BrowserGui::spawn(true, None, None);
    host.cli(&["send-ui-keys", "Ctrl+Shift+I"]);
    host.cli(&["send-ui-ime", "commit", "terminal draft survives Harness"]);
    assert!(
        harness_snapshot(&host).is_null(),
        "runtime must remain lazy before the first Harness entry"
    );
    let browser = host.open_browser_directly();
    assert_eq!(host.cli(&["ui-snapshot"])["active"], browser["id"]);
    host.open_web_settings();
    host.click_geometry("web_proxy_input");
    host.cli(&["send-ui-ime", "commit", "http://127.0.0.1:22123"]);
    let initial = open_harness(&host);
    assert_eq!(initial["listener"], "stopped");
    assert_eq!(initial["login_active"], false);
    assert_eq!(initial["local_key_exists"], false);
    assert_eq!(initial["accounts"], serde_json::json!([]));
    assert_eq!(initial["model"], "");
    let ignored = host.cli(&["send-ui-ime", "commit", "no field owns this text"]);
    assert_eq!(ignored["route"], "harness_settings");
    assert_eq!(harness_snapshot(&host)["model"], "");
    click_harness(&host, "model_input", 0);
    let model_input = host.cli(&["send-ui-ime", "commit", "provider/中-draft"]);
    assert_eq!(model_input["route"], "harness_model");
    host.cli(&["send-ui-ime", "preedit", "composing"]);
    host.cli(&["send-ui-keys", "Enter"]);
    assert!(
        harness_snapshot(&host)["status"].is_null(),
        "preedit Enter must not apply a model"
    );
    host.click_geometry("settings_console_route");
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["settings_section"],
        "console"
    );
    host.click_geometry("settings_web_route");
    assert_eq!(
        host_editor_state(&host, "web_proxy_candidate_editor")["text"],
        "http://127.0.0.1:22123"
    );
    open_harness(&host);
    let retained = harness_snapshot(&host);
    assert_eq!(retained["model_editor"]["text"], "provider/中-draft");
    assert_eq!(retained["model_editor"]["preedit"], "");
    assert_eq!(
        host.cli(&["ui-snapshot"])["composer_text"],
        "terminal draft survives Harness"
    );
    host.cli(&["resize-window", "--width", "420", "--height", "300"]);
    click_harness(&host, "key_actions", 0);
    let saved = wait_harness(&host, |state| state["local_key_exists"] == true);
    assert!(
        saved["scroll"].as_u64().unwrap() > 0,
        "short windows must scroll to reach connection controls"
    );
    let snapshot = host.cli(&["ui-snapshot"]);
    let geometry = &snapshot["geometry"];
    let scale = geometry["scale_permille"].as_u64().unwrap() as f64 / 1000.0;
    let harness = &geometry["harness_settings"];
    let rows = harness["rows"].as_array().unwrap();
    assert!(!rows.is_empty());
    for row in rows {
        assert!(
            (row["rect"]["height"].as_u64().unwrap() as f64 / scale - 36.0).abs() <= 1.0,
            "Harness rows must retain readable targets after resize"
        );
        assert!(
            row["rect"]["y"].as_u64().unwrap() + row["rect"]["height"].as_u64().unwrap()
                <= harness["previous"]["y"].as_u64().unwrap()
        );
    }
    assert_eq!(
        harness["listener"], "stopped",
        "creating a local key must not start a listener"
    );
    host.close_and_wait();
}

#[test]
fn host_harness_failed_port_save_retains_the_prior_port_and_editable_draft() {
    let mut host = BrowserGui::spawn(true, None, None);
    let initial = open_harness(&host);
    let old_port = initial["port"].clone();
    // After loading succeeds, replace the configuration destination with a
    // directory. This forces the real atomic publish to fail on every OS.
    let target = host.config_root.join("harness/gateway.json");
    if target.is_file() {
        fs::remove_file(&target).unwrap();
    }
    fs::create_dir(&target).unwrap();
    click_harness(&host, "port", 0);
    host.cli(&["send-ui-keys", "Ctrl+a"]);
    host.cli(&["send-ui-ime", "commit", "30127"]);
    assert_eq!(
        harness_snapshot(&host)["port_editor"]["text"],
        "30127",
        "the save-failure fixture must first enter the real port correction"
    );
    click_harness(&host, "port", 1);
    let failed = wait_harness(&host, |state| {
        state["status"]
            .as_str()
            .is_some_and(|text| text.contains("Storage"))
    });
    assert_eq!(
        failed["port"], old_port,
        "failed save must not publish a new configured port"
    );
    assert_eq!(
        failed["port_editor"]["text"], "30127",
        "failed save must retain the user's correction"
    );
    fs::remove_dir(&target).unwrap();
    click_harness(&host, "port", 1);
    let saved = wait_harness(&host, |state| state["port"] == 30127);
    assert_eq!(saved["port_editor"]["text"], "30127");
    host.close_and_wait();
}

#[test]
fn host_harness_port_collision_reports_error_and_retries_without_changing_identity() {
    let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = occupied.local_addr().unwrap().port();
    let root = minicon_binary()
        .parent()
        .unwrap()
        .join("browser-court")
        .join(unique_suffix())
        .join("config");
    harness_synthetic_profile(
        &root,
        port,
        "synthetic-local-collision-key",
        "synthetic-upstream-not-authorized",
    );
    let mut host = BrowserGui::spawn_with_config(true, None, None, Some(root));
    let before = open_harness(&host);
    assert_eq!(before["port"], port);
    assert_eq!(before["listener"], "stopped");
    click_harness(&host, "listener_actions", 0);
    let failed = wait_harness(&host, |state| state["listener"] == "error");
    assert_eq!(
        failed["actual_port"],
        Value::Null,
        "bind conflict must never claim Listening"
    );
    assert!(!failed["status"].is_null());
    assert_eq!(failed["selected_account"], before["selected_account"]);
    assert_eq!(failed["model"], before["model"]);
    drop(occupied);
    click_harness(&host, "listener_actions", 0);
    let listening = wait_harness(&host, |state| state["listener"] == "listening");
    assert_eq!(
        listening["actual_port"], port,
        "retry must bind the explicitly configured port"
    );
    click_harness(&host, "listener_actions", 1);
    wait_harness(&host, |state| {
        state["listener"] == "stopped" && state["actual_port"].is_null()
    });
    let rebound =
        std::net::TcpListener::bind(("127.0.0.1", port)).expect("Stop must release the listener");
    drop(rebound);
    host.close_and_wait();
}

#[test]
fn host_harness_public_observations_redact_local_and_upstream_credentials() {
    let reservation = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = reservation.local_addr().unwrap().port();
    let root = minicon_binary()
        .parent()
        .unwrap()
        .join("browser-court")
        .join(unique_suffix())
        .join("config");
    let key = "SYNTHETIC-LOCAL-KEY-MUST-STAY-PRIVATE";
    let token = "SYNTHETIC-UPSTREAM-GRANT-MUST-STAY-PRIVATE";
    harness_synthetic_profile(&root, port, key, token);
    let mut host = BrowserGui::spawn_with_config(true, None, None, Some(root));
    let state = open_harness(&host);
    assert_eq!(state["local_key_exists"], true);
    for response in [host.cli(&["ui-snapshot"]), host.cli(&["list-tabs"])] {
        let encoded = response.to_string();
        for secret in [key, token] {
            assert!(
                !encoded.contains(secret),
                "public observation leaked a private credential"
            );
        }
    }
    click_harness(&host, "key_actions", 0);
    wait_harness(&host, |state| {
        state["status"]
            .as_str()
            .is_some_and(|text| text.contains("InvalidAction"))
    });
    assert_eq!(
        fs::read_to_string(host.config_root.join("harness/client-key")).unwrap(),
        key,
        "Create must not silently rotate an existing key"
    );
    click_harness(&host, "key_actions", 2);
    wait_harness(&host, |state| state["local_key_exists"] == false);
    assert!(!host.config_root.join("harness/client-key").exists());
    let trace = fs::read_to_string(&host._trace.0).unwrap();
    for secret in [key, token] {
        assert!(
            !trace.contains(secret),
            "host trace leaked a private credential"
        );
    }
    host.close_and_wait();
}

#[test]
fn host_browser_authorization_queries_are_private_in_public_observations() {
    let mut host = BrowserGui::spawn(true, None, None);
    host.open_browser_directly();
    for base in [
        "https://platform.deepseek.com/dsh/authorize",
        "https://platform.deepseek.com/oauth/authorize",
        "https://opencode.ai/console/device",
        "http://127.0.0.1:19999/oauth/callback",
        "http://127.0.0.1:19999/minicon/oauth",
    ] {
        let url = format!(
            "{base}?state=SYNTHETIC-STATE-PRIVATE&code=SYNTHETIC-CODE-PRIVATE#SYNTHETIC-FRAGMENT-PRIVATE"
        );
        host.cli(&["send-ui-keys", "Ctrl+l"]);
        host.cli(&["send-ui-keys", "Ctrl+a"]);
        host.cli(&["send-ui-ime", "commit", &url]);
        let draft = host_editor_state(&host, "browser_address_editor");
        assert_eq!(draft["text"], base);
        assert_eq!(draft["redacted"], true);
        host.cli(&["send-ui-keys", "Enter"]);
        for response in [host.cli(&["ui-snapshot"]), host.cli(&["list-tabs"])] {
            let encoded = response.to_string();
            for secret in [
                "SYNTHETIC-STATE-PRIVATE",
                "SYNTHETIC-CODE-PRIVATE",
                "SYNTHETIC-FRAGMENT-PRIVATE",
            ] {
                assert!(
                    !encoded.contains(secret),
                    "authorization query/fragment leaked in public browser observation"
                );
            }
        }
    }
    let normal = "http://127.0.0.1:19999/search?q=ordinary#result";
    host.cli(&["send-ui-keys", "Ctrl+l"]);
    host.cli(&["send-ui-keys", "Ctrl+a"]);
    host.cli(&["send-ui-ime", "commit", normal]);
    assert_eq!(
        host_editor_state(&host, "browser_address_editor")["text"],
        normal,
        "ordinary web query/fragment observations must retain their existing behavior"
    );
    host.close_and_wait();
}

pub(super) fn h0_harness_destination_preserves_console_and_web_drafts_and_observes_only() {
    fn first_visible_card(snapshot: &Value) -> &Value {
        let workspace = &snapshot["geometry"]["harness_workspace"];
        let scroll = workspace["scroll"]
            .as_u64()
            .unwrap_or_else(|| panic!("missing published scroll: {snapshot}"));
        let cards = workspace["cards"]
            .as_array()
            .unwrap_or_else(|| panic!("missing published cards: {snapshot}"));
        let (index, card) = cards
            .iter()
            .enumerate()
            .find(|(_, card)| {
                card["rect"]["width"]
                    .as_u64()
                    .is_some_and(|width| width > 0)
                    && card["rect"]["height"]
                        .as_u64()
                        .is_some_and(|height| height > 0)
            })
            .unwrap_or_else(|| panic!("no visible dashboard card: {snapshot}"));
        assert_eq!(
            u64::try_from(index).unwrap(),
            scroll,
            "published scroll and first visible card disagree: card={card}, snapshot={snapshot}"
        );
        card
    }

    fn card_pixels(
        path: &Path,
        snapshot: &Value,
        card: &Value,
        from_right: bool,
    ) -> ([u8; 3], [u8; 3]) {
        let rect = &card["rect"];
        let coordinate = |field: &str| {
            rect[field]
                .as_u64()
                .unwrap_or_else(|| panic!("invalid card {field}: card={card}, snapshot={snapshot}"))
        };
        let (left, top, width, height) = (
            coordinate("x"),
            coordinate("y"),
            coordinate("width"),
            coordinate("height"),
        );
        // Guard dimensions before any inset subtraction. Hidden rows publish
        // zero rectangles and must never silently become a pixel probe.
        assert!(
            width >= 4 && height >= 3,
            "card cannot contain the required inset: card={card}, snapshot={snapshot}"
        );
        let decoder = png::Decoder::new(fs::File::open(path).unwrap());
        let mut reader = decoder.read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size()];
        let image = reader.next_frame(&mut pixels).unwrap();
        let channels = match image.color_type {
            png::ColorType::Rgb => 3,
            png::ColorType::Rgba => 4,
            other => panic!("unexpected dashboard PNG {other:?}: card={card}, snapshot={snapshot}"),
        };
        let (image_width, image_height) = (u64::from(image.width), u64::from(image.height));
        assert!(
            left <= image_width
                && top <= image_height
                && width <= image_width - left
                && height <= image_height - top,
            "card exceeds PNG {}x{}: card={card}, snapshot={snapshot}",
            image.width,
            image.height
        );
        let x = if from_right {
            left + width - 4
        } else {
            left + 2
        };
        let y = top + 2;
        let gap = top + height + 2;
        assert!(
            x < image_width && y < image_height && gap < image_height,
            "card/gap probe outside PNG {}x{}: x={x}, y={y}, gap={gap}, card={card}, snapshot={snapshot}",
            image.width,
            image.height
        );
        let sample = |row: u64| {
            let start = usize::try_from(row)
                .ok()
                .and_then(|row| row.checked_mul(image.width as usize))
                .and_then(|offset| offset.checked_add(x as usize))
                .and_then(|offset| offset.checked_mul(channels))
                .filter(|start| start.checked_add(3).is_some_and(|end| end <= pixels.len()))
                .unwrap_or_else(|| {
                    panic!("invalid PNG sample offset: row={row}, card={card}, snapshot={snapshot}")
                });
            <[u8; 3]>::try_from(&pixels[start..start + 3]).unwrap()
        };
        (sample(y), sample(gap))
    }
    let mut host = BrowserGui::spawn(true, None, None);
    host.cli(&["send-ui-keys", "Ctrl+Shift+I"]);
    host.cli(&["send-ui-ime", "commit", "h0-console-draft"]);
    let original = host.cli(&["ui-snapshot"]);
    host.click_geometry("harness_entry");
    let overview = host.cli(&["ui-snapshot"]);
    assert_eq!(overview["geometry"]["harness_workspace"]["active"], true);
    assert_eq!(overview["active"], original["active"]);
    // A real Console tab is still alive: require dashboard raster, not only state.
    let shot = host.gui.screenshot.to_string_lossy().into_owned();
    host.cli(&["screenshot-pane", "--output", &shot]);
    let card = first_visible_card(&overview);
    let first_card_pixels = card_pixels(&host.gui.screenshot, &overview, card, false);
    assert_ne!(
        first_card_pixels.0, first_card_pixels.1,
        "live Console must display the dashboard card/gap pixels, not its unchanged sidebar: card={card}, snapshot={overview}"
    );
    let same_tab_card = card_pixels(&host.gui.screenshot, &overview, card, true);
    let discarded = host.cli(&["perf-stats"])["discarded_capture_frames"]
        .as_u64()
        .unwrap();
    host.cli(&[
        "screenshot-pane",
        "--target",
        original["active"].as_str().unwrap(),
        "--output",
        &shot,
    ]);
    assert_ne!(
        card_pixels(&host.gui.screenshot, &overview, card, true).0,
        same_tab_card.0,
        "an explicit underlying active Console still captures its pane"
    );
    assert_eq!(
        host.cli(&["perf-stats"])["discarded_capture_frames"],
        discarded + 1,
        "scratch capture must never present over the visible Dashboard"
    );
    assert_eq!(host.cli(&["ui-snapshot"])["active"], original["active"]);
    host.cli(&["resize-window", "--width", "540", "--height", "300"]);
    host.cli(&["send-ui-keys", "PageDown", "PageDown", "PageDown"]);
    let small = host.cli(&["ui-snapshot"]);
    let requested_scroll = small["geometry"]["harness_workspace"]["scroll"]
        .as_u64()
        .unwrap() as usize;
    assert_eq!(requested_scroll, 3, "small viewport scroll: {small}");
    let small_height = small["geometry"]["harness_workspace"]["body"]["height"]
        .as_u64()
        .unwrap();
    host.cli(&["resize-window", "--width", "1400", "--height", "900"]);
    // The resize response acknowledges a request; native window bounds may
    // differ. Wait for an enlarged, settled observed frame before clicking.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last_frame = Value::Null;
    let mut stable_since = Instant::now();
    let resized = loop {
        let snapshot = host.cli(&["ui-snapshot"]);
        let frame = &snapshot["geometry"]["frame"];
        if frame != &last_frame {
            last_frame = frame.clone();
            stable_since = Instant::now();
        }
        if snapshot["geometry"]["harness_workspace"]["body"]["height"]
            .as_u64()
            .unwrap()
            > small_height
            && stable_since.elapsed() >= Duration::from_millis(100)
        {
            break snapshot;
        }
        assert!(Instant::now() < deadline, "resize observation: {snapshot}");
        thread::sleep(Duration::from_millis(20));
    };
    let workspace = &resized["geometry"]["harness_workspace"];
    let cards = workspace["cards"].as_array().unwrap();
    let capacity = workspace["visible_capacity"].as_u64().unwrap() as usize;
    assert!(
        capacity > 0,
        "enlarged viewport must show a card: {resized}"
    );
    let expected_scroll = requested_scroll.min(cards.len().saturating_sub(capacity));
    assert_eq!(
        workspace["scroll"], expected_scroll,
        "scroll must clamp to actual capacity without an empty tail: {resized}"
    );
    let visible: Vec<_> = cards
        .iter()
        .enumerate()
        .filter(|(_, card)| {
            card["rect"]["width"].as_u64().unwrap() > 0
                && card["rect"]["height"].as_u64().unwrap() > 0
        })
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        visible,
        (expected_scroll..(expected_scroll + capacity).min(cards.len())).collect::<Vec<_>>(),
        "visible card rows must be contiguous and fill the actual capacity: {resized}"
    );
    let first = first_visible_card(&resized);
    host.click_rect(&first["rect"], "first_visible_card", &resized["geometry"]);
    let selected = host.cli(&["ui-snapshot"]);
    assert_eq!(
        selected["geometry"]["harness_workspace"]["selected"], first["id"],
        "painted first card and hit owner must agree after resize: before={resized}, after={selected}"
    );
    host.cli(&["resize-window", "--width", "1000", "--height", "700"]);
    assert!(
        !host.config_root.join("harness").exists(),
        "entry must not instantiate an account/listener runtime"
    );
    host.cli(&["send-ui-keys", "Ctrl+Shift+I"]);
    host.cli(&["send-ui-ime", "commit", "must-not-reach-hidden-console"]);
    host.cli(&["send-ui-keys", "Escape"]);
    let held = host.cli(&["ui-snapshot"]);
    assert_eq!(held["composer_text"], "h0-console-draft");
    assert_eq!(
        held["geometry"]["harness_workspace"]["active"], true,
        "bare Escape does not leave the destination"
    );
    host.click_rect(
        &held["geometry"]["harness_workspace"]["back"],
        "harness_back",
        &held["geometry"],
    );
    let returned = host.cli(&["ui-snapshot"]);
    assert_eq!(returned["active"], original["active"]);
    assert_eq!(returned["composer_text"], "h0-console-draft");
    assert_eq!(returned["composer_focused"], true);
    host.cli(&["send-ui-keys", "Tab"]);
    assert_eq!(host.cli(&["send-ui-keys", "Ctrl+Shift+I"])["sent_keys"], 1);
    let header_ime = host.cli(&["send-ui-ime", "commit", "H0_HEADER_MUST_NOT_TYPE"]);
    assert_eq!(header_ime["route"], "harness_header");
    host.cli(&["send-ui-keys", "a"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["composer_text"],
        "h0-console-draft"
    );
    host.cli(&["send-ui-keys", "Enter"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["geometry"]["harness_workspace"]["active"],
        true
    );
    host.cli(&["send-ui-keys", "Enter"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["composer_focused"],
        true,
        "keyboard entry/back restores the composer owner"
    );
    host.open_browser_directly();
    host.cli(&["send-ui-keys", "Ctrl+l"]);
    host.cli(&["send-ui-keys", "Ctrl+a"]);
    host.cli(&[
        "send-ui-ime",
        "commit",
        "https://h0-draft.fixture.invalid/unsubmitted",
    ]);
    let web = host.cli(&["ui-snapshot"]);
    host.click_geometry("harness_entry");
    let active = host.cli(&["ui-snapshot"]);
    let card = first_visible_card(&active);
    host.cli(&["screenshot-pane", "--output", &shot]);
    let visible_card = card_pixels(&host.gui.screenshot, &active, card, true);
    assert_ne!(
        visible_card.0, visible_card.1,
        "Browser entry must display dashboard card/gap pixels: card={card}, snapshot={active}"
    );
    host.cli(&[
        "screenshot-pane",
        "--target",
        original["active"].as_str().unwrap(),
        "--output",
        &shot,
    ]);
    let captured = host.cli(&["ui-snapshot"]);
    assert_eq!(
        captured["active"], web["active"],
        "capturing another pane must restore the active Browser"
    );
    assert_eq!(captured["geometry"]["harness_workspace"]["active"], true);
    let explicit_pane = card_pixels(&host.gui.screenshot, &captured, card, true);
    assert_ne!(
        explicit_pane.0, visible_card.0,
        "an explicit Console target must capture its pane, not the Dashboard"
    );
    host.click_rect(
        &active["geometry"]["harness_workspace"]["navigation"][2],
        "harness_clients",
        &active["geometry"],
    );
    let clients = host.cli(&["ui-snapshot"]);
    assert_eq!(
        clients["geometry"]["harness_workspace"]["section"],
        "Clients"
    );
    fs::create_dir_all(host.config_root.join("harness/gateway.json")).unwrap();
    host.click_rect(
        &clients["geometry"]["harness_workspace"]["settings"],
        "harness_settings",
        &clients["geometry"],
    );
    let metadata_deadline = Instant::now() + Duration::from_secs(10);
    while host.cli(&["ui-snapshot"])["geometry"]["harness_workspace"]["metadata"] != "unavailable" {
        assert!(
            Instant::now() < metadata_deadline,
            "failed private metadata read was not published unavailable"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let settings = host.cli(&["ui-snapshot"]);
    assert_eq!(settings["geometry"]["settings_section"], "harness");
    host.click_geometry("settings_console_route");
    host.cli(&["send-ui-keys", "Ctrl+Shift+I"]);
    assert_eq!(
        host.cli(&["send-ui-ime", "commit", "H0_SETTINGS_MUST_NOT_TYPE"])["route"],
        "harness_settings_shell"
    );
    assert_eq!(
        host.cli(&["ui-snapshot"])["composer_text"],
        "h0-console-draft"
    );
    host.cli(&["send-ui-keys", "Escape"]);
    let resumed = host.cli(&["ui-snapshot"]);
    assert_eq!(resumed["settings_open"], false);
    assert_eq!(
        resumed["geometry"]["harness_workspace"]["section"],
        "Clients"
    );
    host.click_rect(
        &resumed["geometry"]["harness_workspace"]["back"],
        "harness_back",
        &resumed["geometry"],
    );
    let returned = host.cli(&["ui-snapshot"]);
    assert_eq!(returned["active"], web["active"]);
    assert_eq!(
        returned["geometry"]["browser_address_editor"]["text"],
        web["geometry"]["browser_address_editor"]["text"]
    );
    assert_eq!(returned["geometry"]["address_focused"], true);
    host.cli(&[
        "select-tab",
        "--target",
        original["active"].as_str().unwrap(),
    ]);
    host.cli(&["send-ui-keys", "Ctrl+Shift+I", "Tab", "Tab", "Enter"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["settings_open"],
        true,
        "header Settings must open its real drawer"
    );
    host.cli(&["send-ui-keys", "Escape"]);
    let drawer_return = host.cli(&["ui-snapshot"]);
    assert_eq!(drawer_return["settings_open"], false);
    assert_eq!(
        drawer_return["composer_focused"], true,
        "closing the drawer must restore its invoking composer owner"
    );
    assert_eq!(drawer_return["composer_text"], "h0-console-draft");
    host.cli(&["send-ui-ime", "commit", "H0_DRAWER_INPUT"]);
    assert_eq!(
        host.cli(&["ui-snapshot"])["composer_text"],
        "h0-console-draftH0_DRAWER_INPUT",
        "post-drawer input must reach the restored composer, not the PTY"
    );
    host.cli(&["send-text", "exit\r"]);
    host.cli(&[
        "wait-tab-exit",
        "--target",
        original["active"].as_str().unwrap(),
        "--timeout-ms",
        "10000",
    ]);
    host.click_geometry("harness_entry");
    assert_eq!(
        host.cli(&["send-ui-ime", "commit", "H0_EXITED_MUST_NOT_TYPE"])["route"],
        "harness_workspace"
    );
    let retained = host.cli(&["ui-snapshot"]);
    host.click_rect(
        &retained["geometry"]["harness_workspace"]["back"],
        "harness_back",
        &retained["geometry"],
    );
    let tabs = host.cli(&["list-tabs"]);
    for tab in tabs["tabs"].as_array().unwrap() {
        host.cli(&["close-tab", "--target", tab["id"].as_str().unwrap()]);
    }
    assert_eq!(host.cli(&["ui-snapshot"])["workspace_empty"], true);
    host.click_geometry("harness_entry");
    let empty = host.cli(&["ui-snapshot"]);
    assert_eq!(empty["geometry"]["harness_workspace"]["active"], true);
    host.cli(&["screenshot-pane", "--output", &shot]);
    let decoder = png::Decoder::new(fs::File::open(&host.gui.screenshot).unwrap());
    let reader = decoder.read_info().unwrap();
    assert!(reader.info().width > 0 && reader.info().height > 0);
    assert_eq!(host.cli(&["ui-snapshot"])["active"], Value::Null);
    assert_eq!(
        host.cli(&["send-ui-ime", "commit", "H0_EMPTY_MUST_NOT_TYPE"])["route"],
        "harness_workspace"
    );
    host.click_rect(
        &empty["geometry"]["harness_workspace"]["back"],
        "harness_back",
        &empty["geometry"],
    );
    assert_eq!(host.cli(&["ui-snapshot"])["workspace_empty"], true);
    host.close_and_wait();
}
