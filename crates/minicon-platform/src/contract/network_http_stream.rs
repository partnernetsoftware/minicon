//! Bounded streaming exchange data. HTTP/SSE interpretation belongs to callers.

use std::{path::PathBuf, time::Duration};

use super::network_http::{NetworkHttpError, NetworkHttpRequest};

pub const NETWORK_HTTP_STREAM_DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// Cumulative streamed bytes, independent of the buffered API's allocation cap.
pub const NETWORK_HTTP_STREAM_MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
pub const NETWORK_HTTP_STREAM_MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;

/// The explicit native executable must dispatch `--platform-http-resolve`
/// through `run_resolver_helper` before initializing a GUI or other service.
/// No provider credentials or URL are sent to this resolver process.
#[derive(Clone, Debug)]
pub struct NetworkHttpStreamRequest {
    pub request: NetworkHttpRequest,
    pub resolver_executable: PathBuf,
    pub idle_timeout: Duration,
}

impl NetworkHttpStreamRequest {
    pub fn new(request: NetworkHttpRequest, resolver_executable: impl Into<PathBuf>) -> Self {
        Self {
            request,
            resolver_executable: resolver_executable.into(),
            idle_timeout: NETWORK_HTTP_STREAM_DEFAULT_IDLE_TIMEOUT,
        }
    }

    #[must_use]
    pub fn idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkHttpStreamHead {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkHttpStreamControl {
    Continue,
    Stop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkHttpStreamOutcome {
    pub body_bytes: usize,
    /// The consumer ended the exchange; this is not a complete response.
    pub stopped: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NetworkHttpStreamError {
    Cancelled,
    Http(NetworkHttpError),
}

impl From<NetworkHttpError> for NetworkHttpStreamError {
    fn from(error: NetworkHttpError) -> Self {
        Self::Http(error)
    }
}

impl std::fmt::Display for NetworkHttpStreamError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("network HTTP stream cancelled"),
            Self::Http(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for NetworkHttpStreamError {}
