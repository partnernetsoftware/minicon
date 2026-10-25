//! Shared bounded native-fixture protocol; test-only consumers, owner PRD28/32.
use minicon_web as browser;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering as CourtOrdering},
    mpsc as court_channel,
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
const HTTP_LIMIT: usize = 65_536;
const MANIFEST_LIMIT: u64 = 32_768;
// Only this non-default court may omit its initial fixture UDF rewrite.
// Credential publication and production AccountStore policies are unchanged.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowsProfileAclMode {
    OwnerOnly,
    InheritDefault,
}

impl WindowsProfileAclMode {
    pub(crate) fn read(value: Option<&serde_json::Value>) -> Result<Self, &'static str> {
        match value {
            None => Ok(Self::OwnerOnly),
            Some(serde_json::Value::String(value)) if value == "owner-only" => Ok(Self::OwnerOnly),
            Some(serde_json::Value::String(value))
                if value == "inherit-default" && cfg!(windows) =>
            {
                Ok(Self::InheritDefault)
            }
            _ => Err("native-court-profile-acl-mode-invalid"),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::OwnerOnly => "owner-only",
            Self::InheritDefault => "inherit-default",
        }
    }
}

// The control accepts only this manifest's fresh exact A/B directories.
// It never accepts an arbitrary existing profile as a relaxed-policy target.
pub(crate) fn fresh_acl_control_directory(
    output: &Path,
    directory: &Path,
    name: &str,
) -> Result<(), &'static str> {
    if !matches!(name, "a" | "b") || directory != output.join("profiles").join(name) {
        return Err("native-court-profile-acl-control-outside-fixture");
    }
    for ancestor in directory.ancestors() {
        let metadata = std::fs::symlink_metadata(ancestor)
            .map_err(|_| "native-court-profile-acl-control-unavailable")?;
        if !metadata.is_dir() || minicon_platform::filesystem::metadata_is_link_like(&metadata) {
            return Err("native-court-profile-acl-control-link-like");
        }
    }
    if std::fs::read_dir(directory)
        .map_err(|_| "native-court-profile-acl-control-unavailable")?
        .next()
        .is_some()
    {
        return Err("native-court-profile-acl-control-not-empty");
    }
    Ok(())
}

pub(crate) struct Recipe {
    pub(crate) address: SocketAddr,
    pub(crate) token: String,
    pub(crate) run: String,
    pub(crate) pages: std::collections::HashMap<String, String>,
    pub(crate) a: browser::PageContext,
    pub(crate) b: browser::PageContext,
    pub(crate) output: PathBuf,
    pub(crate) timeout: Duration,
    pub(crate) profile_acl_mode: WindowsProfileAclMode,
}

fn hex16(value: &str) -> Result<[u8; 16], &'static str> {
    if value.len() != 32 || !value.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("native-court-identity-invalid");
    }
    let mut bytes = [0; 16];
    for (index, item) in bytes.iter_mut().enumerate() {
        *item = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| "native-court-identity-invalid")?;
    }
    Ok(bytes)
}

impl Recipe {
    pub(crate) fn read(path: &Path) -> Result<Self, &'static str> {
        let mut raw = Vec::new();
        std::fs::File::open(path)
            .map_err(|_| "native-court-manifest-unreadable")?
            .take(MANIFEST_LIMIT + 1)
            .read_to_end(&mut raw)
            .map_err(|_| "native-court-manifest-unreadable")?;
        if raw.len() as u64 > MANIFEST_LIMIT {
            return Err("native-court-manifest-bound");
        }
        let json: serde_json::Value =
            serde_json::from_slice(&raw).map_err(|_| "native-court-manifest-invalid")?;
        if json["schema"] != 1 || json["test_only"] != true {
            return Err("native-court-manifest-invalid");
        }
        let text = |name: &str| {
            json[name]
                .as_str()
                .map(str::to_owned)
                .ok_or("native-court-manifest-invalid")
        };
        let origin = text("origin")?;
        let address: SocketAddr = origin
            .strip_prefix("http://")
            .ok_or("native-court-origin-invalid")?
            .parse()
            .map_err(|_| "native-court-origin-invalid")?;
        if address.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
            || address.port() == 0
            || origin != format!("http://{address}")
        {
            return Err("native-court-origin-invalid");
        }
        let run = text("run")?;
        let token = text("driver_token")?;
        hex16(&run)?;
        hex16(&token)?;
        let output = PathBuf::from(text("output")?)
            .canonicalize()
            .map_err(|_| "native-court-output-invalid")?;
        // The caller supplies an isolated process environment before launching
        // this test artifact. Never mutate process-global environment here.
        for name in ["MINICON_CONFIG_DIR", "MINICON_BROWSER_PROFILE_ROOT"] {
            let expected = json["launch_environment"][name]
                .as_str()
                .ok_or("native-court-environment-invalid")?;
            let expected_path = Path::new(expected);
            let leaf = if name == "MINICON_CONFIG_DIR" {
                "config"
            } else {
                "ordinary-profile"
            };
            if expected_path.file_name() != Some(std::ffi::OsStr::new(leaf))
                || expected_path
                    .parent()
                    .and_then(|parent| parent.canonicalize().ok())
                    .as_ref()
                    != Some(&output)
            {
                return Err("native-court-environment-outside-output");
            }
            if std::env::var_os(name).as_deref() != Some(std::ffi::OsStr::new(expected)) {
                return Err("native-court-environment-not-isolated");
            }
        }
        for name in [
            "WEBVIEW2_USER_DATA_FOLDER",
            "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
            "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
            "MINICON_BROWSER_HTTP_PROXY",
            "MINICON_BROWSER_HTTPS_PROXY",
            "MINICON_BROWSER_NO_PROXY",
        ] {
            if std::env::var_os(name).is_some_and(|value| !value.is_empty()) {
                return Err("native-court-environment-override");
            }
        }
        let helper = std::env::var_os("MINICON_BROWSER_BIN");
        match (
            json.get("helper").and_then(serde_json::Value::as_str),
            helper.as_deref(),
        ) {
            (None, None) if !cfg!(target_os = "linux") => {}
            (Some(expected), Some(actual)) if cfg!(target_os = "linux") => {
                let expected = Path::new(expected);
                let actual = Path::new(actual);
                if !expected.is_absolute()
                    || !actual.is_absolute()
                    || !expected.is_file()
                    || !actual.is_file()
                    || expected.canonicalize().ok() != actual.canonicalize().ok()
                {
                    return Err("native-court-helper-identity-mismatch");
                }
            }
            _ => return Err("native-court-helper-not-configured"),
        }
        let profile_acl_mode = WindowsProfileAclMode::read(json.get("windows_profile_acl_mode"))?;
        let profile = |name: &str| -> Result<browser::PageContext, &'static str> {
            let value = &json["profiles"][name];
            let directory = PathBuf::from(
                value["directory"]
                    .as_str()
                    .ok_or("native-court-profile-invalid")?,
            );
            let directory = directory
                .canonicalize()
                .map_err(|_| "native-court-profile-invalid")?;
            if !directory.starts_with(&output) || directory == output {
                return Err("native-court-profile-outside-output");
            }
            if profile_acl_mode == WindowsProfileAclMode::InheritDefault {
                fresh_acl_control_directory(&output, &directory, name)?;
            } else {
                minicon_platform::filesystem::protect_private_directory(&directory)
                    .map_err(|_| "native-court-profile-not-private")?;
            }
            let context = browser::PageContext {
                profile: minicon_web::PageProfile {
                    data_directory: directory,
                    store_identifier: hex16(
                        value["store_identifier"]
                            .as_str()
                            .ok_or("native-court-profile-invalid")?,
                    )?,
                },
                proxy: browser::BrowserProxy::default(),
            };
            context
                .validate()
                .map_err(|_| "native-court-profile-invalid")?;
            Ok(context)
        };
        let a = profile("a")?;
        let b = profile("b")?;
        if a.profile.data_directory == b.profile.data_directory
            || a.profile.store_identifier == b.profile.store_identifier
        {
            return Err("native-court-profile-identities-shared");
        }
        let mut pages = std::collections::HashMap::new();
        for phase in ["ordinary", "a", "b", "a-reopen"] {
            let url = json["pages"][phase]
                .as_str()
                .ok_or("native-court-page-invalid")?;
            let prefix = format!("{origin}/page/{run}/{phase}/");
            let ticket = url
                .strip_prefix(&prefix)
                .ok_or("native-court-page-invalid")?;
            hex16(ticket)?;
            pages.insert(phase.to_owned(), url.to_owned());
        }
        let seconds = json["timeout_seconds"]
            .as_f64()
            .filter(|value| value.is_finite() && *value > 0.0 && *value <= 180.0)
            .ok_or("native-court-deadline-invalid")?;
        Ok(Self {
            address,
            token,
            run,
            pages,
            a,
            b,
            output,
            timeout: Duration::from_secs_f64(seconds),
            profile_acl_mode,
        })
    }
}

#[derive(Clone)]
pub(crate) struct Action {
    pub(crate) seq: u64,
    pub(crate) kind: String,
    pub(crate) phase: String,
    pub(crate) url: Option<String>,
}

pub(crate) enum Notice {
    Action(Action),
    Complete(serde_json::Value),
    Failed(&'static str),
}

// All HTTP runs on this owned worker. GUI callbacks never do socket I/O.
fn request(
    address: SocketAddr,
    token: &str,
    path: &str,
    body: Option<&[u8]>,
) -> Result<serde_json::Value, &'static str> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut socket = TcpStream::connect_timeout(&address, Duration::from_millis(500))
        .map_err(|_| "native-court-fixture-connect")?;
    socket
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|_| "native-court-fixture-io")?;
    let payload = body.unwrap_or_default();
    let method = if body.is_some() { "POST" } else { "GET" };
    let header = format!(
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nX-Court-Token: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    socket
        .write_all(header.as_bytes())
        .and_then(|_| socket.write_all(payload))
        .map_err(|_| "native-court-fixture-io")?;
    let mut bytes = Vec::new();
    let mut chunk = [0; 2048];
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("native-court-fixture-timeout")?;
        socket
            .set_read_timeout(Some(remaining))
            .map_err(|_| "native-court-fixture-io")?;
        let count = socket
            .read(&mut chunk)
            .map_err(|_| "native-court-fixture-io")?;
        if count == 0 {
            break;
        }
        if bytes.len() + count > HTTP_LIMIT {
            return Err("native-court-fixture-response-bound");
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    let split = bytes
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .ok_or("native-court-fixture-response-invalid")?;
    if split > 8192 {
        return Err("native-court-fixture-header-bound");
    }
    let headers = std::str::from_utf8(&bytes[..split])
        .map_err(|_| "native-court-fixture-response-invalid")?;
    if !headers
        .lines()
        .next()
        .is_some_and(|line| line.starts_with("HTTP/1.1 200 "))
    {
        return Err("native-court-fixture-rejected");
    }
    let lengths: Vec<_> = headers
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .collect();
    if lengths.len() != 1
        || lengths[0].1.trim().parse::<usize>().ok() != Some(bytes.len() - split - 4)
        || headers
            .lines()
            .any(|line| line.to_ascii_lowercase().starts_with("transfer-encoding:"))
    {
        return Err("native-court-fixture-framing-invalid");
    }
    serde_json::from_slice(&bytes[split + 4..]).map_err(|_| "native-court-fixture-json-invalid")
}

pub(crate) struct Worker {
    pub(crate) notices: court_channel::Receiver<Notice>,
    pub(crate) acknowledgements: court_channel::SyncSender<u64>,
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) overflow: Arc<AtomicBool>,
    pub(crate) handle: Option<JoinHandle<()>>,
}

impl Worker {
    pub(crate) fn start(recipe: &Recipe, wake: impl Fn() + Send + 'static) -> Self {
        let (send, notices) = court_channel::sync_channel(8);
        let (acknowledgements, receive) = court_channel::sync_channel::<u64>(8);
        let stop = Arc::new(AtomicBool::new(false));
        let overflow = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker_overflow = overflow.clone();
        let address = recipe.address;
        let token = recipe.token.clone();
        let run = recipe.run.clone();
        let deadline = Instant::now() + recipe.timeout;
        let handle = std::thread::spawn(move || {
            let emit = |notice| {
                if send.try_send(notice).is_err() {
                    worker_overflow.store(true, CourtOrdering::Release);
                }
                wake();
            };
            let result = (|| -> Result<(), &'static str> {
                let mut last = None;
                while !worker_stop.load(CourtOrdering::Acquire) && Instant::now() < deadline {
                    while let Ok(seq) = receive.try_recv() {
                        request(
                            address,
                            &token,
                            "/driver/ack",
                            Some(serde_json::json!({"seq":seq}).to_string().as_bytes()),
                        )?;
                    }
                    let view = request(address, &token, "/driver/status", None)?;
                    if view["run"].as_str() != Some(run.as_str()) {
                        return Err("native-court-run-identity-mismatch");
                    }
                    match view["state"].as_str() {
                        Some("failed") => return Err("native-court-page-observation-failed"),
                        Some("observations-complete") => {
                            let receipt = request(address, &token, "/driver/receipt", None)?;
                            emit(Notice::Complete(receipt));
                            return Ok(());
                        }
                        Some("observing") => {}
                        _ => return Err("native-court-fixture-state-invalid"),
                    }
                    if let Some(seq) = view["action"]["seq"]
                        .as_u64()
                        .filter(|seq| last != Some(*seq))
                    {
                        let action = &view["action"];
                        emit(Notice::Action(Action {
                            seq,
                            kind: action["kind"]
                                .as_str()
                                .ok_or("native-court-action-invalid")?
                                .to_owned(),
                            phase: action["phase"]
                                .as_str()
                                .ok_or("native-court-action-invalid")?
                                .to_owned(),
                            url: action["url"].as_str().map(str::to_owned),
                        }));
                        last = Some(seq);
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                if worker_stop.load(CourtOrdering::Acquire) {
                    Ok(())
                } else {
                    Err("native-court-deadline-expired")
                }
            })();
            if let Err(code) = result {
                emit(Notice::Failed(code));
            }
        });
        Self {
            notices,
            acknowledgements,
            stop,
            overflow,
            handle: Some(handle),
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, CourtOrdering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
