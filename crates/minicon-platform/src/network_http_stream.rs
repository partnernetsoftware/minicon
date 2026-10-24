//! One bounded HTTP exchange with synchronous chunk delivery and real socket
//! cancellation. Uses the existing ureq HTTP parser and target TLS provider.
//! No reader/resolver/watchdog thread is detached (the exchange creates none).
//! Host lookup runs in a contained, killable native helper, never ureq's
//! DefaultResolver, whose timed lookup detaches an OS resolver thread.

use crate::{
    contained_process::ContainedHeadlessCommand,
    network_http::{self, NetworkHttpError, NetworkHttpErrorKind},
};
use std::{
    fmt,
    io::{self, Read, Write},
    net::{IpAddr, Shutdown, SocketAddr, TcpStream, ToSocketAddrs},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use ureq::{
    config::Config,
    unversioned::{
        resolver::{ResolvedSocketAddrs, Resolver},
        transport::{
            Buffers, ConnectionDetails, Connector, Either, LazyBuffers, NextTimeout, Transport,
        },
    },
};

pub use crate::contract::network_http_stream::{
    NETWORK_HTTP_STREAM_DEFAULT_IDLE_TIMEOUT, NETWORK_HTTP_STREAM_MAX_REQUEST_BYTES,
    NETWORK_HTTP_STREAM_MAX_RESPONSE_BYTES, NetworkHttpStreamControl, NetworkHttpStreamError,
    NetworkHttpStreamHead, NetworkHttpStreamOutcome, NetworkHttpStreamRequest,
};

const IO_POLL: Duration = Duration::from_millis(100);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(2);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);
const RESOLVER_MAX_HOST: usize = 253;
const RESOLVER_MAX_OUTPUT: usize = 1024;
const RESOLVER_MAX_ADDRESSES: usize = 16;
const RESOLVER_MAGIC: &[u8; 4] = b"NHR1";

#[derive(Debug, Default)]
struct CancelInner {
    cancelled: Arc<AtomicBool>,
    in_use: AtomicBool,
    socket: Mutex<Option<TcpStream>>,
}

/// One exchange's cloneable cancellation owner. Cancellation shuts down the
/// actual TCP socket underneath TLS; it is sticky and cannot be reset.
#[derive(Clone, Debug, Default)]
pub struct NetworkHttpCancel(Arc<CancelInner>);

impl NetworkHttpCancel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Observes an invocation-owned flag during bounded native socket polls.
    /// Calling `cancel` additionally shuts down the registered socket at once.
    pub fn with_flag(cancelled: Arc<AtomicBool>) -> Self {
        Self(Arc::new(CancelInner {
            cancelled,
            ..CancelInner::default()
        }))
    }

    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        self.shutdown();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    fn shutdown(&self) {
        if let Some(socket) = self
            .0
            .socket
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
        {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }

    fn register(&self, stream: &TcpStream) -> io::Result<()> {
        let mut socket = self
            .0
            .socket
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *socket = Some(stream.try_clone()?);
        if self.is_cancelled() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        Ok(())
    }
}

struct ExchangeOwner(NetworkHttpCancel);
impl Drop for ExchangeOwner {
    fn drop(&mut self) {
        self.0.shutdown();
        self.0
            .0
            .socket
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        self.0.0.in_use.store(false, Ordering::Release);
    }
}

#[derive(Debug)]
struct Exchange {
    cancel: NetworkHttpCancel,
    deadline: Instant,
    idle_timeout: Duration,
    last_progress: Mutex<Instant>,
    cleanup_failed: AtomicBool,
}

impl Exchange {
    fn check(&self) -> Result<(), ureq::Error> {
        if self.cancel.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "HTTP exchange cancelled",
            )
            .into());
        }
        let now = Instant::now();
        if now >= self.deadline {
            return Err(ureq::Error::Timeout(ureq::Timeout::Global));
        }
        if now.duration_since(
            *self
                .last_progress
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        ) >= self.idle_timeout
        {
            return Err(ureq::Error::Timeout(ureq::Timeout::RecvBody));
        }
        Ok(())
    }

    fn progress(&self) {
        *self
            .last_progress
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Instant::now();
    }

    fn remaining(&self) -> Duration {
        let now = Instant::now();
        self.deadline.saturating_duration_since(now).min(
            (*self
                .last_progress
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                + self.idle_timeout)
                .saturating_duration_since(now),
        )
    }
}

fn failure(kind: NetworkHttpErrorKind, detail: &str) -> NetworkHttpError {
    NetworkHttpError::new(kind, detail)
}

/// Validates stream-specific 16 MiB request / 32 MiB cumulative response
/// ceilings and all shared request bounds without opening a socket/helper.
pub fn validate(request: &NetworkHttpStreamRequest) -> Result<(), NetworkHttpError> {
    network_http::validate_bounds(
        &request.request,
        NETWORK_HTTP_STREAM_MAX_REQUEST_BYTES,
        NETWORK_HTTP_STREAM_MAX_RESPONSE_BYTES,
    )?;
    if request.request.max_redirects != 0 {
        return Err(failure(
            NetworkHttpErrorKind::InvalidLimit,
            "streaming requires max_redirects=0; sent requests are never replayed",
        ));
    }
    if request.idle_timeout.is_zero()
        || request.idle_timeout > NETWORK_HTTP_STREAM_DEFAULT_IDLE_TIMEOUT
    {
        return Err(failure(
            NetworkHttpErrorKind::InvalidLimit,
            "stream idle timeout must be above zero and at most 120 seconds",
        ));
    }
    if !request.resolver_executable.is_absolute() {
        return Err(failure(
            NetworkHttpErrorKind::InvalidLimit,
            "resolver executable must be an explicit absolute native path",
        ));
    }
    Ok(())
}

/// Delivers response bytes as they arrive, without parsing SSE or collecting a
/// complete body. The caller must disable redirects (streaming never replays
/// a sent request), and both callbacks must return promptly. A downstream
/// writer must use bounded writes and return Stop on disconnect; arbitrary
/// blocking callback code cannot be safely preempted by this API.
pub fn request_stream(
    request: &NetworkHttpStreamRequest,
    cancel: &NetworkHttpCancel,
    mut on_head: impl FnMut(&NetworkHttpStreamHead) -> NetworkHttpStreamControl,
    mut on_chunk: impl FnMut(&[u8]) -> NetworkHttpStreamControl,
) -> Result<NetworkHttpStreamOutcome, NetworkHttpStreamError> {
    validate(request)?;
    if cancel.is_cancelled() {
        return Err(NetworkHttpStreamError::Cancelled);
    }
    if cancel
        .0
        .in_use
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(failure(
            NetworkHttpErrorKind::InvalidLimit,
            "cancel handle already owns an exchange",
        )
        .into());
    }
    let _owner = ExchangeOwner(cancel.clone());
    let state = Arc::new(Exchange {
        cancel: cancel.clone(),
        deadline: Instant::now() + request.request.timeout,
        idle_timeout: request.idle_timeout,
        last_progress: Mutex::new(Instant::now()),
        cleanup_failed: AtomicBool::new(false),
    });
    let result = exchange(request, &state, &mut on_head, &mut on_chunk);
    if cancel.is_cancelled() && !state.cleanup_failed.load(Ordering::Acquire) {
        return Err(NetworkHttpStreamError::Cancelled);
    }
    result.map_err(NetworkHttpStreamError::Http)
}

fn exchange(
    request: &NetworkHttpStreamRequest,
    state: &Arc<Exchange>,
    on_head: &mut impl FnMut(&NetworkHttpStreamHead) -> NetworkHttpStreamControl,
    on_chunk: &mut impl FnMut(&[u8]) -> NetworkHttpStreamControl,
) -> Result<NetworkHttpStreamOutcome, NetworkHttpError> {
    let input = &request.request;
    let mut config = ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(network_http::TLS_PROVIDER)
                .build(),
        )
        .timeout_global(Some(input.timeout))
        .max_redirects(0)
        .max_redirects_will_error(true)
        .max_idle_connections(0)
        .max_idle_connections_per_host(0)
        .max_response_header_size(network_http::NETWORK_HTTP_MAX_HEADER_BYTES)
        .http_status_as_error(false);
    if let Some(proxy) = &input.proxy {
        let proxy = ureq::Proxy::new(proxy).map_err(|error| {
            failure(
                NetworkHttpErrorKind::Proxy,
                &network_http::redact_detail(&error.to_string()),
            )
        })?;
        config = config.proxy(Some(proxy));
    } else {
        config = config.proxy(None);
    }

    let connector =
        ().chain(ureq::unversioned::transport::ConnectProxyConnector::default())
            .chain(StreamingConnector {
                state: state.clone(),
            });
    #[cfg(windows)]
    let connector = connector.chain(ureq::unversioned::transport::NativeTlsConnector::default());
    #[cfg(unix)]
    let connector = connector.chain(ureq::unversioned::transport::RustlsConnector::default());
    let resolver = HelperResolver {
        executable: request.resolver_executable.clone(),
        state: state.clone(),
        arguments: vec!["--platform-http-resolve".into()],
    };
    let agent = ureq::Agent::with_parts(config.build(), connector, resolver);
    let mut builder = ureq::http::Request::builder()
        .method(input.method.as_str())
        .uri(input.url.as_str());
    for (name, value) in &input.headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    let wire = builder
        .body(input.body.as_slice())
        .map_err(|error| network_http::map_http_error(&error))?;
    state
        .check()
        .map_err(|error| network_http::map_ureq_error(&error))?;
    let mut response = agent
        .run(wire)
        .map_err(|error| network_http::map_ureq_error(&error))?;
    let head = NetworkHttpStreamHead {
        status: response.status().as_u16(),
        headers: response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.to_string(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect(),
    };
    network_http::validate_headers(&head.headers)?;
    if on_head(&head) == NetworkHttpStreamControl::Stop {
        state.cancel.shutdown();
        return Ok(NetworkHttpStreamOutcome {
            body_bytes: 0,
            stopped: true,
        });
    }
    let mut body = response.body_mut().as_reader();
    let mut chunk = [0; 8192];
    let mut total = 0;
    loop {
        state
            .check()
            .map_err(|error| network_http::map_ureq_error(&error))?;
        let remaining = input.max_response_bytes - total;
        let want = chunk.len().min(remaining + 1);
        let count = body
            .read(&mut chunk[..want])
            .map_err(|error| network_http::map_body_read_error(&error))?;
        if count == 0 {
            return Ok(NetworkHttpStreamOutcome {
                body_bytes: total,
                stopped: false,
            });
        }
        if count > remaining {
            return Err(failure(
                NetworkHttpErrorKind::ResponseTooLarge,
                "streamed body exceeds the cumulative response byte limit",
            ));
        }
        total += count;
        if on_chunk(&chunk[..count]) == NetworkHttpStreamControl::Stop {
            state.cancel.shutdown();
            return Ok(NetworkHttpStreamOutcome {
                body_bytes: total,
                stopped: true,
            });
        }
    }
}

#[derive(Debug)]
struct StreamingConnector {
    state: Arc<Exchange>,
}

impl<In: Transport> Connector<In> for StreamingConnector {
    type Out = Either<In, StreamingTcp>;
    fn connect(
        &self,
        details: &ConnectionDetails,
        chained: Option<In>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        if let Some(transport) = chained {
            return Ok(Some(Either::A(transport)));
        }
        let phase_deadline = Instant::now() + (*details.timeout.after).min(self.state.remaining());
        let mut last_error = None;
        for address in &details.addrs {
            self.state.check()?;
            let timeout = self
                .state
                .remaining()
                .min(phase_deadline.saturating_duration_since(Instant::now()))
                .min(CONNECT_TIMEOUT);
            if timeout.is_zero() {
                return Err(ureq::Error::Timeout(details.timeout.reason));
            }
            match TcpStream::connect_timeout(address, timeout) {
                Ok(stream) => {
                    self.state.check()?;
                    stream.set_nodelay(details.config.no_delay())?;
                    self.state.cancel.register(&stream)?;
                    self.state.progress();
                    return Ok(Some(Either::B(StreamingTcp {
                        stream,
                        buffers: LazyBuffers::new(
                            details.config.input_buffer_size(),
                            details.config.output_buffer_size(),
                        ),
                        state: self.state.clone(),
                    })));
                }
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error
            .unwrap_or_else(|| {
                io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    "resolver returned no usable addresses",
                )
            })
            .into())
    }
}

struct StreamingTcp {
    stream: TcpStream,
    buffers: LazyBuffers,
    state: Arc<Exchange>,
}
impl fmt::Debug for StreamingTcp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StreamingTcp")
    }
}

fn phase_remaining(
    state: &Exchange,
    deadline: Instant,
    reason: ureq::Timeout,
) -> Result<Duration, ureq::Error> {
    state.check()?;
    let remaining = state
        .remaining()
        .min(deadline.saturating_duration_since(Instant::now()));
    if remaining.is_zero() {
        Err(ureq::Error::Timeout(reason))
    } else {
        Ok(remaining.min(IO_POLL))
    }
}

impl Transport for StreamingTcp {
    fn buffers(&mut self) -> &mut dyn Buffers {
        &mut self.buffers
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        let deadline = Instant::now() + (*timeout.after).min(self.state.remaining());
        let mut sent = 0;
        while sent < amount {
            self.stream.set_write_timeout(Some(phase_remaining(
                &self.state,
                deadline,
                timeout.reason,
            )?))?;
            match self.stream.write(&self.buffers.output()[sent..amount]) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "HTTP socket stopped accepting bytes",
                    )
                    .into());
                }
                Ok(count) => {
                    sent += count;
                    self.state.progress();
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        let deadline = Instant::now() + (*timeout.after).min(self.state.remaining());
        loop {
            self.stream.set_read_timeout(Some(phase_remaining(
                &self.state,
                deadline,
                timeout.reason,
            )?))?;
            match self.stream.read(self.buffers.input_append_buf()) {
                Ok(count) => {
                    self.buffers.input_appended(count);
                    if count != 0 {
                        self.state.progress();
                    }
                    return Ok(count != 0);
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn is_open(&mut self) -> bool {
        !self.state.cancel.is_cancelled()
    }
}

#[derive(Debug)]
struct HelperResolver {
    executable: PathBuf,
    arguments: Vec<String>,
    state: Arc<Exchange>,
}

impl Resolver for HelperResolver {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        _config: &Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        self.state.check()?;
        let host = uri.host().ok_or(ureq::Error::HostNotFound)?;
        let host = host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(host);
        let port = uri
            .port_u16()
            .unwrap_or(if uri.scheme_str() == Some("https") {
                443
            } else {
                80
            });
        let mut result = self.empty();
        if let Ok(address) = host.parse::<IpAddr>() {
            result.push(SocketAddr::new(address, port));
            return Ok(result);
        }
        let frame = resolver_frame(host, port).map_err(|_| ureq::Error::HostNotFound)?;
        let bytes = self.lookup(&frame, (*timeout.after).min(RESOLVE_TIMEOUT))?;
        for address in parse_addresses(&bytes, port)? {
            result.push(address);
        }
        Ok(result)
    }
}

impl HelperResolver {
    fn lookup(&self, frame: &[u8], timeout: Duration) -> Result<Vec<u8>, ureq::Error> {
        self.state.check()?;
        let deadline = Instant::now() + self.state.remaining().min(timeout);
        let mut command = ContainedHeadlessCommand::new(&self.executable);
        command
            .args(self.arguments.iter().cloned())
            .env_clear()
            .pipe_stdin()
            .capture_output();
        for name in ["SystemRoot", "WINDIR", "LANG", "LC_ALL", "LC_CTYPE"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let mut child = command.spawn()?;
        let exchange = (|| -> Result<Vec<u8>, ureq::Error> {
            // The entire frame is < 262 bytes, below the minimum native pipe
            // capacity. It never carries a URL, Authorization or provider key.
            let mut stdin = child
                .take_stdin()
                .ok_or_else(|| io::Error::other("resolver stdin unavailable"))?;
            stdin.write_all(frame)?;
            drop(stdin);
            let mut stdout = child.take_stdout();
            let mut stderr = child.take_stderr();
            let mut output = Vec::new();
            let mut discarded = 0;
            loop {
                self.state.check()?;
                if Instant::now() >= deadline {
                    return Err(ureq::Error::Timeout(ureq::Timeout::Resolve));
                }
                let mut progress = false;
                for (stream, retain) in [(&mut stdout, true), (&mut stderr, false)] {
                    if let Some(source) = stream {
                        let mut chunk = [0; 256];
                        match source.read_timeout(&mut chunk, Duration::ZERO) {
                            Ok(0) => *stream = None,
                            Ok(count) => {
                                progress = true;
                                if retain {
                                    output.extend_from_slice(&chunk[..count]);
                                } else {
                                    discarded += count;
                                }
                                if output.len() > RESOLVER_MAX_OUTPUT
                                    || discarded > RESOLVER_MAX_OUTPUT
                                {
                                    return Err(io::Error::other(
                                        "resolver exceeded its output limit",
                                    )
                                    .into());
                                }
                            }
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    io::ErrorKind::TimedOut
                                        | io::ErrorKind::Interrupted
                                        | io::ErrorKind::WouldBlock
                                ) => {}
                            Err(error) => return Err(error.into()),
                        }
                    }
                }
                if child.exit_observed()? && stdout.is_none() && stderr.is_none() {
                    break;
                }
                if !progress {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            Ok(output)
        })();
        let cleanup = child.terminate_and_wait(CLEANUP_TIMEOUT);
        let exit = child.try_wait();
        drop(child);
        if let Err(error) = cleanup {
            self.state.cleanup_failed.store(true, Ordering::Release);
            return Err(
                io::Error::new(error.kind(), format!("resolver cleanup failed: {error}")).into(),
            );
        }
        let output = exchange?;
        if exit?.and_then(|exit| exit.conventional_code()) != Some(0) {
            return Err(ureq::Error::HostNotFound);
        }
        self.state.progress();
        Ok(output)
    }
}

fn resolver_frame(host: &str, port: u16) -> io::Result<Vec<u8>> {
    if host.is_empty()
        || host.len() > RESOLVER_MAX_HOST
        || port == 0
        || !host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid resolver host/port frame",
        ));
    }
    let mut frame = Vec::with_capacity(8 + host.len());
    frame.extend_from_slice(RESOLVER_MAGIC);
    frame.extend_from_slice(&(host.len() as u16).to_be_bytes());
    frame.extend_from_slice(&port.to_be_bytes());
    frame.extend_from_slice(host.as_bytes());
    Ok(frame)
}

fn parse_addresses(bytes: &[u8], port: u16) -> Result<Vec<SocketAddr>, ureq::Error> {
    let text = std::str::from_utf8(bytes).map_err(|_| ureq::Error::HostNotFound)?;
    let mut result = Vec::new();
    for line in text.lines() {
        if result.len() >= RESOLVER_MAX_ADDRESSES {
            return Err(ureq::Error::HostNotFound);
        }
        let address = line
            .parse::<IpAddr>()
            .map_err(|_| ureq::Error::HostNotFound)?;
        let address = SocketAddr::new(address, port);
        if !result.contains(&address) {
            result.push(address);
        }
    }
    if result.is_empty() {
        return Err(ureq::Error::HostNotFound);
    }
    Ok(result)
}

/// Embedding native binaries dispatch this function for exactly
/// `--platform-http-resolve`, before initializing their normal host. The
/// parent owns the helper's 2-second lookup and 2-second kill/reap deadlines.
/// This process performs the OS lookup synchronously; no resolver thread can
/// survive the contained process's cancellation.
pub fn run_resolver_helper() -> Result<(), NetworkHttpError> {
    resolve_helper_io(&mut io::stdin().lock(), &mut io::stdout().lock()).map_err(|_| {
        failure(
            NetworkHttpErrorKind::HostNotFound,
            "native resolver helper failed",
        )
    })
}

fn resolve_helper_io(input: &mut impl Read, output: &mut impl Write) -> io::Result<()> {
    let mut header = [0; 8];
    input.read_exact(&mut header)?;
    if &header[..4] != RESOLVER_MAGIC {
        return Err(io::Error::other("invalid resolver frame magic"));
    }
    let length = u16::from_be_bytes([header[4], header[5]]) as usize;
    let port = u16::from_be_bytes([header[6], header[7]]);
    if !(1..=RESOLVER_MAX_HOST).contains(&length) {
        return Err(io::Error::other("invalid resolver frame length"));
    }
    let mut host = vec![0; length];
    input.read_exact(&mut host)?;
    let mut trailing = [0];
    if input.read(&mut trailing)? != 0 {
        return Err(io::Error::other("trailing resolver frame bytes"));
    }
    let host =
        std::str::from_utf8(&host).map_err(|_| io::Error::other("non-ASCII resolver host"))?;
    resolver_frame(host, port)?;
    let mut addresses = Vec::new();
    for address in (host, port).to_socket_addrs()?.take(RESOLVER_MAX_ADDRESSES) {
        if !addresses.contains(&address.ip()) {
            addresses.push(address.ip());
        }
    }
    if addresses.is_empty() {
        return Err(io::Error::other("native resolver found no address"));
    }
    for address in addresses {
        writeln!(output, "{address}")?;
    }
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network_http::{NetworkHttpMethod, NetworkHttpRequest};
    use std::{net::TcpListener, sync::mpsc, thread};

    fn listener() -> (TcpListener, String) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/events", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        (listener, url)
    }

    fn accept(listener: TcpListener) -> TcpStream {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    // Windows can inherit the listener's nonblocking mode.
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    return stream;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < until, "stream client did not connect");
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("accept failed: {error}"),
            }
        }
    }

    fn read_head(stream: &mut TcpStream) {
        let mut bytes = Vec::new();
        let mut byte = [0];
        while !bytes.ends_with(b"\r\n\r\n") {
            assert!(bytes.len() < 32768);
            assert_eq!(stream.read(&mut byte).unwrap(), 1);
            bytes.push(byte[0]);
        }
    }

    fn begin_sse(stream: &mut TcpStream) {
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
        stream.flush().unwrap();
    }

    fn chunk(stream: &mut TcpStream, bytes: &[u8]) -> io::Result<()> {
        write!(stream, "{:x}\r\n", bytes.len())?;
        stream.write_all(bytes)?;
        stream.write_all(b"\r\n")?;
        stream.flush()
    }

    fn assert_closed(stream: &mut TcpStream) {
        let mut buffer = [0; 1024];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionReset
                            | io::ErrorKind::ConnectionAborted
                            | io::ErrorKind::BrokenPipe
                    ) =>
                {
                    break;
                }
                Err(error) => panic!("owned upstream socket did not close: {error}"),
            }
        }
    }

    fn request(url: String) -> NetworkHttpStreamRequest {
        NetworkHttpStreamRequest::new(
            NetworkHttpRequest::new(NetworkHttpMethod::Get, url)
                .timeout(Duration::from_secs(5))
                .max_redirects(0),
            std::env::current_exe().unwrap(),
        )
    }

    fn assert_released(cancel: &NetworkHttpCancel) {
        assert!(!cancel.0.in_use.load(Ordering::Acquire));
        assert!(cancel.0.socket.lock().unwrap().is_none());
    }

    #[test]
    fn sse_chunks_arrive_before_the_peer_can_finish_its_body() {
        let (listener, url) = listener();
        let (observed, first_event) = mpsc::sync_channel(1);
        let peer = thread::spawn(move || {
            let mut socket = accept(listener);
            read_head(&mut socket);
            begin_sse(&mut socket);
            chunk(&mut socket, b"data: first\n\n").unwrap();
            first_event
                .recv_timeout(Duration::from_secs(2))
                .expect("first callback must precede response completion");
            chunk(&mut socket, b"data: second\n\n").unwrap();
            socket.write_all(b"0\r\n\r\n").unwrap();
        });
        let cancel = NetworkHttpCancel::new();
        let mut bytes = Vec::new();
        let mut notified = false;
        let outcome = request_stream(
            &request(url),
            &cancel,
            |head| {
                assert_eq!(head.status, 200);
                NetworkHttpStreamControl::Continue
            },
            |chunk| {
                bytes.extend_from_slice(chunk);
                if !notified && bytes.windows(13).any(|part| part == b"data: first\n\n") {
                    observed.send(()).unwrap();
                    notified = true;
                }
                NetworkHttpStreamControl::Continue
            },
        )
        .unwrap();
        peer.join().unwrap();
        assert_eq!(bytes, b"data: first\n\ndata: second\n\n");
        assert_eq!(
            outcome,
            NetworkHttpStreamOutcome {
                body_bytes: bytes.len(),
                stopped: false
            }
        );
        assert_released(&cancel);
    }

    #[test]
    fn stream_body_ceilings_are_independent_of_buffered_eight_mib_limits() {
        let mut input = request("http://127.0.0.1/events".to_owned());
        input.request.max_response_bytes = NETWORK_HTTP_STREAM_MAX_RESPONSE_BYTES;
        validate(&input).expect("32 MiB stream configuration is admitted");
        assert_eq!(
            network_http::validate(&input.request).unwrap_err().kind(),
            NetworkHttpErrorKind::InvalidLimit
        );
        input.request.max_response_bytes += 1;
        assert_eq!(
            validate(&input).unwrap_err().kind(),
            NetworkHttpErrorKind::InvalidLimit
        );
        input.request.max_response_bytes = 0;
        assert_eq!(
            validate(&input).unwrap_err().kind(),
            NetworkHttpErrorKind::InvalidLimit
        );
        input.request.max_response_bytes = NETWORK_HTTP_STREAM_MAX_RESPONSE_BYTES;
        input.request.method = NetworkHttpMethod::Post;
        input.request.body = vec![b'x'; NETWORK_HTTP_STREAM_MAX_REQUEST_BYTES];
        validate(&input).expect("16 MiB stream request is admitted");
        // A separately admissible buffered response cap isolates its original
        // request-body limit from the independent stream response limit.
        input.request.max_response_bytes = network_http::NETWORK_HTTP_MAX_RESPONSE_BYTES;
        assert_eq!(
            network_http::validate(&input.request).unwrap_err().kind(),
            NetworkHttpErrorKind::InvalidLimit
        );
        input.request.body.push(b'x');
        assert_eq!(
            validate(&input).unwrap_err().kind(),
            NetworkHttpErrorKind::InvalidLimit
        );
    }

    #[test]
    fn chunk_stream_crosses_eight_mib_before_the_peer_finishes_its_response() {
        let (listener, url) = listener();
        let (over_cap, observed) = mpsc::sync_channel(1);
        const BLOCK_BYTES: usize = 8192;
        let blocks = network_http::NETWORK_HTTP_MAX_RESPONSE_BYTES / BLOCK_BYTES + 1;
        let expected = blocks * BLOCK_BYTES + b"data: done\n\n".len();
        let peer = thread::spawn(move || {
            let mut socket = accept(listener);
            socket.set_nodelay(true).unwrap();
            read_head(&mut socket);
            begin_sse(&mut socket);
            let mut event = [b'x'; BLOCK_BYTES];
            event[..6].copy_from_slice(b"data: ");
            event[BLOCK_BYTES - 2..].copy_from_slice(b"\n\n");
            for _ in 0..blocks {
                chunk(&mut socket, &event).unwrap();
            }
            // No EOF or final event is available until the consumer confirms
            // real delivery past the buffered API's old eight-MiB ceiling.
            observed
                .recv_timeout(Duration::from_secs(3))
                .expect("stream must deliver beyond eight MiB before completion");
            chunk(&mut socket, b"data: done\n\n").unwrap();
            socket.write_all(b"0\r\n\r\n").unwrap();
        });
        let cancel = NetworkHttpCancel::new();
        let mut input = request(url);
        input.request.max_response_bytes = NETWORK_HTTP_STREAM_MAX_RESPONSE_BYTES;
        input.request.timeout = Duration::from_secs(10);
        let mut received = 0;
        let mut notified = false;
        let result = request_stream(
            &input,
            &cancel,
            |_| NetworkHttpStreamControl::Continue,
            |bytes| {
                received += bytes.len();
                if !notified && received > network_http::NETWORK_HTTP_MAX_RESPONSE_BYTES {
                    over_cap.send(()).unwrap();
                    notified = true;
                }
                NetworkHttpStreamControl::Continue
            },
        );
        let peer_result = peer.join();
        assert_eq!(
            result.expect("streaming must not inherit the buffered byte ceiling"),
            NetworkHttpStreamOutcome {
                body_bytes: expected,
                stopped: false
            }
        );
        peer_result.unwrap();
        assert_eq!(received, expected);
        assert!(notified);
        assert_released(&cancel);
    }

    #[test]
    fn consumer_stop_at_head_or_chunk_closes_the_actual_upstream() {
        for stop_at_head in [false, true] {
            let (listener, url) = listener();
            let peer = thread::spawn(move || {
                let mut socket = accept(listener);
                read_head(&mut socket);
                begin_sse(&mut socket);
                if !stop_at_head {
                    chunk(&mut socket, b"data: stop\n\n").unwrap();
                }
                assert_closed(&mut socket);
            });
            let cancel = NetworkHttpCancel::new();
            let mut chunks = 0;
            let start = Instant::now();
            let outcome = request_stream(
                &request(url),
                &cancel,
                |_| {
                    if stop_at_head {
                        NetworkHttpStreamControl::Stop
                    } else {
                        NetworkHttpStreamControl::Continue
                    }
                },
                |_| {
                    chunks += 1;
                    NetworkHttpStreamControl::Stop
                },
            )
            .unwrap();
            assert!(outcome.stopped);
            assert_eq!(chunks == 0, stop_at_head);
            assert!(start.elapsed() < Duration::from_secs(2));
            peer.join().unwrap();
            assert_released(&cancel);
        }
    }

    #[test]
    fn cancellation_interrupts_a_blocked_body_read_and_joins_the_owner() {
        for external_flag in [false, true] {
            let (listener, url) = listener();
            let (head_sent, ready) = mpsc::sync_channel(1);
            let peer = thread::spawn(move || {
                let mut socket = accept(listener);
                read_head(&mut socket);
                begin_sse(&mut socket);
                head_sent.send(()).unwrap();
                assert_closed(&mut socket);
            });
            let flag = Arc::new(AtomicBool::new(false));
            let cancel = if external_flag {
                NetworkHttpCancel::with_flag(flag.clone())
            } else {
                NetworkHttpCancel::new()
            };
            let child_cancel = cancel.clone();
            let worker = thread::spawn(move || {
                request_stream(
                    &request(url),
                    &child_cancel,
                    |_| NetworkHttpStreamControl::Continue,
                    |_| panic!("cancelled body produced data"),
                )
            });
            ready.recv_timeout(Duration::from_secs(3)).unwrap();
            let start = Instant::now();
            if external_flag {
                flag.store(true, Ordering::Release);
            } else {
                cancel.cancel();
            }
            assert_eq!(
                worker.join().unwrap(),
                Err(NetworkHttpStreamError::Cancelled)
            );
            assert!(start.elapsed() < Duration::from_secs(2));
            peer.join().unwrap();
            assert_released(&cancel);
        }
    }

    #[test]
    fn cancellation_interrupts_the_native_tls_handshake_socket() {
        let (listener, url) = listener();
        let url = url.replacen("http://", "https://", 1);
        let (hello_seen, ready) = mpsc::sync_channel(1);
        let peer = thread::spawn(move || {
            let mut socket = accept(listener);
            let mut byte = [0];
            assert_eq!(socket.read(&mut byte).unwrap(), 1);
            assert_eq!(
                byte[0], 22,
                "native TLS must send a handshake, not plain HTTP"
            );
            hello_seen.send(()).unwrap();
            assert_closed(&mut socket);
        });
        let cancel = NetworkHttpCancel::new();
        let child_cancel = cancel.clone();
        let worker = thread::spawn(move || {
            request_stream(
                &request(url),
                &child_cancel,
                |_| panic!("unnegotiated TLS produced headers"),
                |_| panic!("unnegotiated TLS produced body"),
            )
        });
        ready.recv_timeout(Duration::from_secs(3)).unwrap();
        let start = Instant::now();
        cancel.cancel();
        assert_eq!(
            worker.join().unwrap(),
            Err(NetworkHttpStreamError::Cancelled)
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        peer.join().unwrap();
        assert_released(&cancel);
    }

    #[test]
    fn idle_deadline_stops_a_peer_that_keeps_its_stream_open() {
        let (listener, url) = listener();
        let peer = thread::spawn(move || {
            let mut socket = accept(listener);
            read_head(&mut socket);
            begin_sse(&mut socket);
            chunk(&mut socket, b"data: ready\n\n").unwrap();
            assert_closed(&mut socket);
        });
        let cancel = NetworkHttpCancel::new();
        let start = Instant::now();
        let error = request_stream(
            &request(url).idle_timeout(Duration::from_millis(150)),
            &cancel,
            |_| NetworkHttpStreamControl::Continue,
            |_| NetworkHttpStreamControl::Continue,
        )
        .unwrap_err();
        assert!(
            matches!(error, NetworkHttpStreamError::Http(ref error) if error.kind() == NetworkHttpErrorKind::Timeout)
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        peer.join().unwrap();
        assert_released(&cancel);
    }

    #[test]
    fn global_deadline_does_not_restart_on_stream_progress() {
        let (listener, url) = listener();
        let peer = thread::spawn(move || {
            let mut socket = accept(listener);
            read_head(&mut socket);
            begin_sse(&mut socket);
            let until = Instant::now() + Duration::from_secs(2);
            while Instant::now() < until {
                if chunk(&mut socket, b"data: tick\n\n").is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(40));
            }
            panic!("global deadline did not close the progressing stream");
        });
        let cancel = NetworkHttpCancel::new();
        let mut request = request(url).idle_timeout(Duration::from_millis(200));
        request.request.timeout = Duration::from_millis(400);
        let mut received = 0;
        let start = Instant::now();
        let error = request_stream(
            &request,
            &cancel,
            |_| NetworkHttpStreamControl::Continue,
            |bytes| {
                received += bytes.len();
                NetworkHttpStreamControl::Continue
            },
        )
        .unwrap_err();
        assert!(
            matches!(error, NetworkHttpStreamError::Http(ref error) if error.kind() == NetworkHttpErrorKind::Timeout)
        );
        assert!(received >= 2 * b"data: tick\n\n".len());
        assert!(start.elapsed() < Duration::from_secs(2));
        peer.join().unwrap();
        assert_released(&cancel);
    }

    #[test]
    fn response_byte_limit_accepts_exactly_the_cap_and_rejects_one_more() {
        for length in [8, 9] {
            let (listener, url) = listener();
            let peer = thread::spawn(move || {
                let mut socket = accept(listener);
                read_head(&mut socket);
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
                socket.write_all(&[b'x'; 9][..length]).unwrap();
            });
            let cancel = NetworkHttpCancel::new();
            let mut request = request(url);
            request.request.max_response_bytes = 8;
            let mut received = 0;
            let result = request_stream(
                &request,
                &cancel,
                |_| NetworkHttpStreamControl::Continue,
                |bytes| {
                    received += bytes.len();
                    NetworkHttpStreamControl::Continue
                },
            );
            if length == 8 {
                assert_eq!(
                    result.unwrap(),
                    NetworkHttpStreamOutcome {
                        body_bytes: 8,
                        stopped: false
                    }
                );
            } else {
                assert!(
                    matches!(result, Err(NetworkHttpStreamError::Http(ref error)) if error.kind() == NetworkHttpErrorKind::ResponseTooLarge)
                );
            }
            assert!(received <= 8);
            peer.join().unwrap();
            assert_released(&cancel);
        }
    }

    #[test]
    fn pre_cancelled_stream_has_no_connection_or_callback_effect() {
        let (listener, url) = listener();
        let cancel = NetworkHttpCancel::new();
        cancel.cancel();
        assert_eq!(
            request_stream(
                &request(url),
                &cancel,
                |_| panic!("cancelled head"),
                |_| panic!("cancelled chunk")
            ),
            Err(NetworkHttpStreamError::Cancelled)
        );
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_released(&cancel);
    }

    #[test]
    fn resolver_frame_refuses_urls_credentials_overflow_and_trailing_bytes() {
        for host in [
            "",
            "https://secret.invalid/path",
            "name@secret.invalid",
            "name\n.invalid",
        ] {
            assert!(resolver_frame(host, 443).is_err());
        }
        assert!(resolver_frame(&"a".repeat(254), 443).is_err());
        assert!(resolver_frame("localhost", 0).is_err());
        let mut malformed = resolver_frame("localhost", 443).unwrap();
        malformed.push(b'!');
        assert!(resolve_helper_io(&mut malformed.as_slice(), &mut Vec::new()).is_err());
        assert!(parse_addresses(b"127.0.0.1\nnot-an-ip\n", 443).is_err());
        let addresses = parse_addresses(b"127.0.0.1\n::1\n", 443).unwrap();
        assert_eq!(addresses.len(), 2);
        assert!(addresses.iter().all(|address| address.port() == 443));
    }

    #[test]
    fn slow_resolver_probe() {
        let Some(path) = std::env::args()
            .find_map(|argument| argument.strip_prefix("resolver-ready=").map(PathBuf::from))
        else {
            return;
        };
        let pid = std::process::id();
        let identity = crate::process_observation::start_identity(pid).unwrap();
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, format!("{pid}\n{identity}\n")).unwrap();
        std::fs::rename(temporary, path).unwrap();
        thread::sleep(Duration::from_secs(6));
    }

    #[test]
    fn delayed_resolver_is_cancelled_or_timed_out_and_reaped() {
        for cancel_during_lookup in [false, true] {
            let root = std::env::temp_dir().join(format!(
                "minicon-stream-resolver-{}-{cancel_during_lookup}-{:?}",
                std::process::id(),
                thread::current().id()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let root = root.canonicalize().unwrap();
            let ready = root.join("ready");
            let cancel = NetworkHttpCancel::new();
            let state = Arc::new(Exchange {
                cancel: cancel.clone(),
                deadline: Instant::now() + Duration::from_secs(10),
                idle_timeout: Duration::from_secs(10),
                last_progress: Mutex::new(Instant::now()),
                cleanup_failed: AtomicBool::new(false),
            });
            let resolver = HelperResolver {
                executable: std::env::current_exe().unwrap(),
                state,
                arguments: vec![
                    "--exact".into(),
                    "network_http_stream::tests::slow_resolver_probe".into(),
                    "--nocapture".into(),
                    "--skip".into(),
                    format!("resolver-ready={}", ready.display()),
                ],
            };
            let worker = thread::spawn(move || {
                resolver.lookup(&resolver_frame("localhost", 443).unwrap(), RESOLVE_TIMEOUT)
            });
            let until = Instant::now() + Duration::from_secs(5);
            while !ready.is_file() {
                assert!(
                    Instant::now() < until,
                    "slow resolver never reached its blocking lookup"
                );
                thread::sleep(Duration::from_millis(2));
            }
            let text = std::fs::read_to_string(&ready).unwrap();
            let mut lines = text.lines();
            let pid = lines.next().unwrap().parse().unwrap();
            let identity = lines.next().unwrap();
            let reference = crate::process_reference::ProcessReference::open(pid).unwrap();
            assert_eq!(
                crate::process_observation::verify_identity(pid, identity),
                crate::process_observation::IdentityVerdict::Live
            );
            let start = Instant::now();
            if cancel_during_lookup {
                cancel.cancel();
            }
            let error = worker.join().unwrap().unwrap_err();
            if !cancel_during_lookup {
                assert!(matches!(
                    error,
                    ureq::Error::Timeout(ureq::Timeout::Resolve)
                ));
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            assert_eq!(
                reference
                    .wait_for_exit(Some(Duration::from_secs(1)))
                    .unwrap(),
                crate::process_reference::ProcessWait::Exited
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
