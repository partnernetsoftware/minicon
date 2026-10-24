//! Bounded process-lifetime loopback HTTP. Product routes and credentials belong
//! to the consumer. Handlers and cancellation hooks must cooperate with the
//! context and keep their own work bounded; this is not a plugin executor.

use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const POLL: Duration = Duration::from_millis(50);
const HEADER_BYTES: usize = 16 * 1024;
const BODY_BYTES: usize = 16 * 1024 * 1024;
const RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const INPUT_TIMEOUT: Duration = Duration::from_secs(15);
const OUTPUT_IDLE_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Failure {
    BadRequest,
    HeaderTooLarge,
    BodyTooLarge,
    UnsupportedFraming,
    InputTimeout,
    OutputTimeout,
    ResponseTooLarge,
    QueueFull,
    QueueTimeout,
    Cancelled,
    Internal,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "loopback HTTP: {self:?}")
    }
}
impl std::error::Error for Failure {}

#[derive(Debug)]
pub struct RequestHead {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub content_length: usize,
}

impl RequestHead {
    /// Duplicate authentication/Host headers are deliberately not collapsed.
    pub fn header(&self, name: &str) -> Result<Option<&str>, Failure> {
        let mut values = self
            .headers
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case(name));
        let first = values.next().map(|(_, value)| value.as_str());
        if values.next().is_some() {
            return Err(Failure::BadRequest);
        }
        Ok(first)
    }
}

#[derive(Debug)]
pub struct Request {
    pub head: RequestHead,
    pub body: Vec<u8>,
}

pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

struct CancelState {
    cancelled: AtomicBool,
    finished: AtomicBool,
    hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

#[derive(Clone)]
pub struct RequestContext(Arc<CancelState>);

impl RequestContext {
    fn new() -> Self {
        Self(Arc::new(CancelState {
            cancelled: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            hook: Mutex::new(None),
        }))
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    pub fn cancel(&self) {
        if !self.0.cancelled.swap(true, Ordering::AcqRel) {
            let hook = self
                .0
                .hook
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            if let Some(hook) = hook {
                hook();
            }
        }
    }

    /// Register the real upstream socket cancellation owner. It may race Stop
    /// or disconnect; registering after cancellation still invokes the hook.
    pub fn on_cancel(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        *self.0.hook.lock().unwrap_or_else(|e| e.into_inner()) = Some(hook.clone());
        if self.is_cancelled() {
            hook();
        }
    }
}

pub trait Handler: Send + Sync + 'static {
    /// Runs before allocating/reading a body, allowing credential rejection
    /// without admitting an unauthenticated multi-megabyte request.
    fn admit(&self, head: &RequestHead) -> Result<(), Response>;
    fn handle(
        &self,
        request: Request,
        context: &RequestContext,
        reply: &mut Reply,
    ) -> Result<(), Failure>;
    fn failure(&self, failure: Failure) -> Response;
}

pub struct Reply {
    stream: TcpStream,
    context: RequestContext,
    deadline: Instant,
    started: bool,
    written: usize,
}

impl Reply {
    pub fn started(&self) -> bool {
        self.started
    }

    pub fn start(&mut self, status: u16, content_type: &str) -> Result<(), Failure> {
        self.start_head(status, content_type, None)
    }

    fn start_head(
        &mut self,
        status: u16,
        content_type: &str,
        length: Option<usize>,
    ) -> Result<(), Failure> {
        if self.started
            || !(200..600).contains(&status)
            || content_type.len() > 128
            || content_type.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
        {
            return Err(Failure::Internal);
        }
        let length =
            length.map_or_else(String::new, |bytes| format!("Content-Length: {bytes}\r\n"));
        let head = format!(
            "HTTP/1.1 {status} Response\r\nContent-Type: {content_type}\r\n{length}Connection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\r\n"
        );
        self.write_bytes(head.as_bytes())?;
        self.started = true;
        Ok(())
    }

    pub fn write(&mut self, body: &[u8]) -> Result<(), Failure> {
        if !self.started {
            return Err(Failure::Internal);
        }
        let next = self
            .written
            .checked_add(body.len())
            .ok_or(Failure::ResponseTooLarge)?;
        if next > RESPONSE_BYTES {
            self.context.cancel();
            return Err(Failure::ResponseTooLarge);
        }
        self.write_bytes(body)?;
        self.written = next;
        Ok(())
    }

    pub fn send(&mut self, response: Response) -> Result<(), Failure> {
        self.start_head(
            response.status,
            response.content_type,
            Some(response.body.len()),
        )?;
        self.write(&response.body)
    }

    fn write_bytes(&mut self, mut bytes: &[u8]) -> Result<(), Failure> {
        let mut progress = Instant::now();
        while !bytes.is_empty() {
            if self.context.is_cancelled() {
                return Err(Failure::Cancelled);
            }
            if Instant::now() >= self.deadline || progress.elapsed() >= OUTPUT_IDLE_TIMEOUT {
                self.context.cancel();
                return Err(Failure::OutputTimeout);
            }
            match self.stream.write(bytes) {
                Ok(0) => {
                    self.context.cancel();
                    return Err(Failure::Cancelled);
                }
                Ok(count) => {
                    bytes = &bytes[count..];
                    progress = Instant::now();
                }
                Err(e) if retryable(&e) => thread::sleep(Duration::from_millis(1)),
                Err(_) => {
                    self.context.cancel();
                    return Err(Failure::Cancelled);
                }
            }
        }
        Ok(())
    }
}

fn retryable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}

pub struct Server {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Server {
    /// Port zero is available for isolated fixtures. The product validates its
    /// configured nonzero port before calling this process-local mechanism.
    pub fn start(port: u16, handler: Arc<dyn Handler>) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        let address = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = thread::Builder::new()
            .name("loopback-http".into())
            .spawn(move || {
                serve(listener, stopping, handler);
            })?;
        Ok(Self {
            address,
            stop,
            worker: Some(worker),
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// The owned handler must return after cancellation. Native model I/O and
    /// response writes have finite polling/cleanup bounds; no worker is detached.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Job {
    context: RequestContext,
    worker: JoinHandle<()>,
}

fn serve(listener: TcpListener, stopping: Arc<AtomicBool>, handler: Arc<dyn Handler>) {
    let mut jobs: Vec<Job> = Vec::new();
    let mut queued: VecDeque<(TcpStream, Instant)> = VecDeque::new();
    while !stopping.load(Ordering::Acquire) {
        let mut index = 0;
        while index < jobs.len() {
            if jobs[index].worker.is_finished() {
                let _ = jobs.swap_remove(index).worker.join();
            } else {
                index += 1;
            }
        }
        while let Some((_, admitted)) = queued.front() {
            if stopping.load(Ordering::Acquire) {
                break;
            }
            if admitted.elapsed() < Duration::from_secs(15) {
                break;
            }
            if let Some((stream, _)) = queued.pop_front() {
                reject(stream, &*handler, Failure::QueueTimeout);
            }
        }
        if !stopping.load(Ordering::Acquire)
            && let Ok((stream, peer)) = listener.accept()
        {
            // Explicitly reset accepted sockets: Windows can inherit the
            // listener's nonblocking mode, unlike the Unix fixture assumption.
            if peer.ip().is_loopback() && stream.set_nonblocking(false).is_ok() {
                if queued.len() < 16 {
                    queued.push_back((stream, Instant::now()));
                } else {
                    reject(stream, &*handler, Failure::QueueFull);
                }
            }
        }
        while jobs.len() < 8 && !stopping.load(Ordering::Acquire) {
            let Some((stream, admitted)) = queued.pop_front() else {
                break;
            };
            if admitted.elapsed() >= Duration::from_secs(15) {
                reject(stream, &*handler, Failure::QueueTimeout);
                continue;
            }
            let context = RequestContext::new();
            let running = context.clone();
            let handler = handler.clone();
            match thread::Builder::new()
                .name("loopback-request".into())
                .spawn(move || {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        handle_connection(stream, running.clone(), handler);
                    }));
                    running.cancel();
                    running.0.finished.store(true, Ordering::Release);
                }) {
                Ok(worker) => jobs.push(Job { context, worker }),
                Err(_) => context.cancel(),
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    drop(listener);
    drop(queued);
    for job in &jobs {
        job.context.cancel();
    }
    for job in jobs {
        let _ = job.worker.join();
    }
}

fn reject(mut stream: TcpStream, handler: &dyn Handler, failure: Failure) {
    // Consume at most a small pending header before closing a queued socket.
    // Closing with even an ordinary SDK header unread can make Windows reset
    // the connection before the overload response is delivered. Never drain an
    // unbounded body, allocate its claimed length or admit the rejected work.
    let _ = stream.set_read_timeout(Some(POLL));
    let _ = stream.set_write_timeout(Some(POLL));
    let deadline = Instant::now() + Duration::from_millis(100);
    let mut headers = Vec::new();
    let mut chunk = [0; 1024];
    while headers.len() < HEADER_BYTES && Instant::now() < deadline {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => {
                headers.extend_from_slice(&chunk[..count]);
                if headers.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
            Err(error) if retryable(&error) => {}
            Err(_) => break,
        }
    }
    let mut reply = Reply {
        stream,
        context: RequestContext::new(),
        deadline: Instant::now() + Duration::from_millis(100),
        started: false,
        written: 0,
    };
    let _ = reply.send(handler.failure(failure));
    let _ = reply.stream.shutdown(Shutdown::Write);
}

fn handle_connection(mut stream: TcpStream, context: RequestContext, handler: Arc<dyn Handler>) {
    if stream.set_read_timeout(Some(POLL)).is_err() || stream.set_write_timeout(Some(POLL)).is_err()
    {
        return;
    }
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let request = read_request(&mut stream, &context, &*handler);
    let mut reply = Reply {
        stream,
        context: context.clone(),
        deadline,
        started: false,
        written: 0,
    };
    let request = match request {
        Ok(request) => request,
        Err(response) => {
            let _ = reply.send(response);
            return;
        }
    };
    // A finite monitor observes SDK disconnects even while the upstream has not
    // produced its first event. It owns no parser/body and is joined below.
    let monitor = reply.stream.try_clone().ok().and_then(|peer| {
        let observing = context.clone();
        thread::Builder::new()
            .name("loopback-peer".into())
            .spawn(move || {
                let mut byte = [0];
                while !observing.0.finished.load(Ordering::Acquire) && !observing.is_cancelled() {
                    if Instant::now() >= deadline {
                        observing.cancel();
                        break;
                    }
                    match peer.peek(&mut byte) {
                        Ok(0) => {
                            observing.cancel();
                            break;
                        }
                        // This listener serves exactly one framed request and
                        // closes the connection. Trailing/pipelined bytes must
                        // not indefinitely hide an already-disconnected peer.
                        Ok(_) => {
                            observing.cancel();
                            break;
                        }
                        Err(e) if retryable(&e) => {}
                        Err(_) => {
                            observing.cancel();
                            break;
                        }
                    }
                }
            })
            .ok()
    });
    if monitor.is_none() {
        let _ = reply.send(handler.failure(Failure::Internal));
        return;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        handler.handle(request, &context, &mut reply)
    }));
    let failure = match result {
        Ok(Err(failure)) => Some(failure),
        Err(_) => Some(Failure::Internal),
        _ => None,
    };
    if let Some(failure) = failure
        && !reply.started
    {
        let _ = reply.send(handler.failure(failure));
    }
    context.0.finished.store(true, Ordering::Release);
    let _ = reply.stream.shutdown(Shutdown::Both);
    if let Some(monitor) = monitor {
        let _ = monitor.join();
    }
}

fn read_request(
    stream: &mut TcpStream,
    context: &RequestContext,
    handler: &dyn Handler,
) -> Result<Request, Response> {
    let mut read = || -> Result<Request, ReadFailure> {
        let deadline = Instant::now() + INPUT_TIMEOUT;
        let mut bytes = Vec::new();
        let mut chunk = [0; 1024];
        let header_end = loop {
            if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                break index + 4;
            }
            if bytes.len() >= HEADER_BYTES {
                return Err(ReadFailure::Protocol(Failure::HeaderTooLarge));
            }
            let count = read_chunk(stream, context, deadline, &mut chunk)?;
            bytes.extend_from_slice(&chunk[..count]);
        };
        if header_end > HEADER_BYTES {
            return Err(ReadFailure::Protocol(Failure::HeaderTooLarge));
        }
        let mut headers = [httparse::EMPTY_HEADER; 32];
        let mut parsed = httparse::Request::new(&mut headers);
        match parsed.parse(&bytes[..header_end]) {
            Ok(httparse::Status::Complete(_)) => {}
            _ => return Err(ReadFailure::Protocol(Failure::BadRequest)),
        }
        let head = RequestHead {
            method: parsed
                .method
                .ok_or(ReadFailure::Protocol(Failure::BadRequest))?
                .into(),
            path: parsed
                .path
                .ok_or(ReadFailure::Protocol(Failure::BadRequest))?
                .into(),
            headers: parsed
                .headers
                .iter()
                .map(|header| {
                    std::str::from_utf8(header.value)
                        .map(|value| (header.name.to_owned(), value.to_owned()))
                        .map_err(|_| ReadFailure::Protocol(Failure::BadRequest))
                })
                .collect::<Result<_, _>>()?,
            content_length: 0,
        };
        if head
            .header("transfer-encoding")
            .map_err(ReadFailure::Protocol)?
            .is_some()
        {
            return Err(ReadFailure::Protocol(Failure::UnsupportedFraming));
        }
        let length = head
            .header("content-length")
            .map_err(ReadFailure::Protocol)?
            .unwrap_or("0");
        if length.is_empty() || !length.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(ReadFailure::Protocol(Failure::BadRequest));
        }
        let content_length = length
            .parse::<usize>()
            .map_err(|_| ReadFailure::Protocol(Failure::BodyTooLarge))?;
        if content_length > BODY_BYTES {
            return Err(ReadFailure::Protocol(Failure::BodyTooLarge));
        }
        let head = RequestHead {
            content_length,
            ..head
        };
        handler.admit(&head).map_err(ReadFailure::Rejected)?;
        let mut body = bytes[header_end..].to_vec();
        if body.len() > content_length {
            return Err(ReadFailure::Protocol(Failure::BadRequest));
        }
        while body.len() < content_length {
            let remaining = (content_length - body.len()).min(chunk.len());
            let count = read_chunk(stream, context, deadline, &mut chunk[..remaining])?;
            body.extend_from_slice(&chunk[..count]);
        }
        Ok(Request { head, body })
    };
    read().map_err(|failure| match failure {
        ReadFailure::Protocol(failure) => handler.failure(failure),
        ReadFailure::Rejected(response) => response,
    })
}

enum ReadFailure {
    Protocol(Failure),
    Rejected(Response),
}

fn read_chunk(
    stream: &mut TcpStream,
    context: &RequestContext,
    deadline: Instant,
    chunk: &mut [u8],
) -> Result<usize, ReadFailure> {
    loop {
        if context.is_cancelled() {
            return Err(ReadFailure::Protocol(Failure::Cancelled));
        }
        if Instant::now() >= deadline {
            return Err(ReadFailure::Protocol(Failure::InputTimeout));
        }
        match stream.read(chunk) {
            Ok(0) => return Err(ReadFailure::Protocol(Failure::Cancelled)),
            Ok(count) => return Ok(count),
            Err(error) if retryable(&error) => {}
            Err(_) => return Err(ReadFailure::Protocol(Failure::Cancelled)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::AtomicUsize, mpsc};

    struct Fixture {
        handled: Arc<AtomicUsize>,
        running: Option<mpsc::SyncSender<()>>,
        cancelled: Option<mpsc::SyncSender<()>>,
    }

    impl Handler for Fixture {
        fn admit(&self, head: &RequestHead) -> Result<(), Response> {
            if head.header("authorization").ok().flatten() == Some("Bearer synthetic-local-key") {
                Ok(())
            } else {
                Err(Response {
                    status: 401,
                    content_type: "application/json",
                    body: b"{}".to_vec(),
                })
            }
        }
        fn handle(
            &self,
            _: Request,
            context: &RequestContext,
            reply: &mut Reply,
        ) -> Result<(), Failure> {
            self.handled.fetch_add(1, Ordering::AcqRel);
            if let Some(cancelled) = &self.cancelled {
                let cancelled = cancelled.clone();
                context.on_cancel(Arc::new(move || {
                    let _ = cancelled.try_send(());
                }));
            }
            reply.start(200, "text/event-stream")?;
            reply.write(b"data: first\n\n")?;
            if let Some(running) = &self.running {
                let _ = running.try_send(());
                let deadline = Instant::now() + Duration::from_secs(3);
                while !context.is_cancelled() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(5));
                }
                if !context.is_cancelled() {
                    return Err(Failure::Internal);
                }
            }
            Ok(())
        }
        fn failure(&self, failure: Failure) -> Response {
            let status = match failure {
                Failure::BodyTooLarge => 413,
                Failure::QueueFull | Failure::QueueTimeout => 503,
                _ => 400,
            };
            Response {
                status,
                content_type: "application/json",
                body: b"{}".to_vec(),
            }
        }
    }

    fn connect(server: &Server, request: &[u8]) -> TcpStream {
        let mut stream = TcpStream::connect(server.address()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream.write_all(request).unwrap();
        stream
    }

    fn fixture(
        running: Option<mpsc::SyncSender<()>>,
        cancelled: Option<mpsc::SyncSender<()>>,
    ) -> (Server, Arc<AtomicUsize>) {
        let handled = Arc::new(AtomicUsize::new(0));
        let server = Server::start(
            0,
            Arc::new(Fixture {
                handled: handled.clone(),
                running,
                cancelled,
            }),
        )
        .unwrap();
        assert!(server.address().ip().is_loopback());
        (server, handled)
    }

    #[test]
    fn authentication_rejects_before_reading_or_allocating_the_claimed_body() {
        let (mut server, handled) = fixture(None, None);
        // Deliberately do not send the body. Admission must return immediately,
        // rather than waiting for the request-body deadline.
        let mut client = connect(&server, b"POST /v1/messages HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 16000000\r\nAuthorization: Bearer wrong\r\n\r\n");
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 401 "), "{response}");
        assert_eq!(handled.load(Ordering::Acquire), 0);
        server.stop();
    }

    #[test]
    fn duplicate_lengths_chunked_input_and_oversized_bodies_are_rejected() {
        let (mut server, handled) = fixture(None, None);
        for (headers, status) in [
            ("Content-Length: 0\r\nContent-Length: 1\r\n", 400),
            ("Transfer-Encoding: chunked\r\n", 400),
            ("Content-Length: 16777217\r\n", 413),
        ] {
            let request = format!(
                "POST /v1/messages HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer synthetic-local-key\r\n{headers}\r\n"
            );
            let mut client = connect(&server, request.as_bytes());
            let mut response = String::new();
            client.read_to_string(&mut response).unwrap();
            assert!(
                response.starts_with(&format!("HTTP/1.1 {status} ")),
                "{response}"
            );
        }
        assert_eq!(handled.load(Ordering::Acquire), 0);
        server.stop();
    }

    #[test]
    fn disconnect_before_completion_cancels_the_owned_upstream_and_stop_joins() {
        disconnect_case(false);
        disconnect_case(true);
    }

    fn disconnect_case(trailing_byte: bool) {
        let (running, entered) = mpsc::sync_channel(1);
        let (cancelled, observed) = mpsc::sync_channel(1);
        let (mut server, handled) = fixture(Some(running), Some(cancelled));
        let mut client = connect(&server, b"POST /v1/messages HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nAuthorization: Bearer synthetic-local-key\r\n\r\n");
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut received = Vec::new();
        let mut bytes = [0; 256];
        while !received.ends_with(b"data: first\n\n") {
            let count = client.read(&mut bytes).unwrap();
            assert!(count > 0);
            received.extend_from_slice(&bytes[..count]);
        }
        // Receiving the first event while the handler still waits also proves
        // that the response is not buffered until completion.
        if trailing_byte {
            client.write_all(b"x").unwrap();
        }
        drop(client);
        observed
            .recv_timeout(Duration::from_secs(2))
            .expect("peer disconnect must cancel upstream before Stop");
        let before = Instant::now();
        server.stop();
        assert!(before.elapsed() < Duration::from_secs(1));
        assert_eq!(handled.load(Ordering::Acquire), 1);
    }

    #[test]
    fn eight_active_and_sixteen_queued_connections_reject_the_next_admission() {
        let (running, entered) = mpsc::sync_channel(8);
        let (mut server, handled) = fixture(Some(running), None);
        let request = b"POST /v1/messages HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nAuthorization: Bearer synthetic-local-key\r\n\r\n";
        let mut clients = Vec::new();
        for _ in 0..8 {
            clients.push(connect(&server, request));
        }
        for _ in 0..8 {
            entered.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        for _ in 0..16 {
            clients.push(connect(&server, request));
        }
        let mut overflow = connect(&server, request);
        let mut response = String::new();
        overflow.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 503 "), "{response}");
        assert_eq!(handled.load(Ordering::Acquire), 8);
        server.stop();
        drop(clients);
    }
}
