//! Public-CLI evidence for the Windows native browser adapter (PRD 25 / 26).
use super::*;

struct BrowserGui {
    gui: OwnedGui,
    endpoint: String,
    exe: PathBuf,
    _trace: HostTrace,
}

impl BrowserGui {
    fn spawn(missing_runtime: bool, http_proxy: Option<&str>, https_proxy: Option<&str>) -> Self {
        let exe = minicon_binary();
        let suffix = unique_suffix();
        let endpoint = control_endpoint(&suffix);
        let root = exe.parent().unwrap().join("browser-court").join(suffix);
        fs::create_dir_all(&root).unwrap();
        let trace = HostTrace(root.join("host.log"));
        let mut command = Command::new(&exe);
        command
            .args([
                "--no-activate",
                "--control",
                &endpoint,
                "-e",
                "cmd.exe",
                "/d",
                "/k",
            ])
            .env("MINICON_CONFIG_DIR", root.join("config"))
            .env("WEBVIEW2_USER_DATA_FOLDER", root.join("profile"))
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
}

#[test]
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
#[ignore = "requires WebView2 runtime; run the explicit native Windows browser court"]
fn native_webview2_navigation_uses_the_browser_proxy() {
    for https_override in [false, true] {
        proxy_navigation(https_override);
    }
}

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
