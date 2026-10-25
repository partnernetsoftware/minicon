//! Opt-in, value-free CookieManager observation. Owner: PRD 28.01.
//! Every method and interface drop stays on the owning WebView2 STA.
use std::{
    cell::RefCell,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use webview2_com::{
    GetCookiesCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2_2, ICoreWebView2CookieList, ICoreWebView2CookieManager,
        ICoreWebView2GetCookiesCompletedHandler,
    },
};
use windows_core::{Interface, PCWSTR};
use wry::{WebView, WebViewExtWindows};

const COOKIE_LIMIT: u32 = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCookieMetadataStatus {
    Observed,
    Unsupported,
    InvalidUri,
    ContextUnavailable,
    CallFailed,
    CallbackFailed,
    PropertyUnavailable,
    CountBound,
    TimedOut,
    Cancelled,
}
impl NativeCookieMetadataStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Unsupported => "unsupported",
            Self::InvalidUri => "invalid-uri",
            Self::ContextUnavailable => "context-unavailable",
            Self::CallFailed => "call-failed",
            Self::CallbackFailed => "callback-failed",
            Self::PropertyUnavailable => "property-unavailable",
            Self::CountBound => "count-bound",
            Self::TimedOut => "timed-out",
            Self::Cancelled => "cancelled",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NativeCookieMetadataCounts {
    pub total: u32,
    pub session: u32,
    pub persistent: u32,
    pub expiry_future: u32,
    pub expiry_elapsed: u32,
    pub expiry_unknown: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeCookieMetadataSummary {
    pub status: NativeCookieMetadataStatus,
    pub counts: Option<NativeCookieMetadataCounts>,
}
impl NativeCookieMetadataSummary {
    pub fn unavailable(status: NativeCookieMetadataStatus) -> Self {
        Self {
            status,
            counts: None,
        }
    }
}

struct ProbeState {
    generation: u64,
    deadline: Instant,
    active: bool,
    result: Option<NativeCookieMetadataSummary>,
}
impl ProbeState {
    fn expire(&mut self, now: Instant) {
        if self.active && now >= self.deadline {
            self.active = false;
            self.result = Some(NativeCookieMetadataSummary::unavailable(
                NativeCookieMetadataStatus::TimedOut,
            ));
        }
    }
    fn can_read(&mut self, generation: u64, now: Instant) -> bool {
        self.expire(now);
        self.active && self.generation == generation
    }
    fn finish(&mut self, generation: u64, now: Instant, result: NativeCookieMetadataSummary) {
        if self.can_read(generation, now) {
            self.active = false;
            self.result = Some(result);
        }
    }
    fn cancel(&mut self) {
        if self.active {
            self.active = false;
            self.result = Some(NativeCookieMetadataSummary::unavailable(
                NativeCookieMetadataStatus::Cancelled,
            ));
        }
    }
}

/// No WebView, cookie objects, URI or COM interfaces are exposed to the caller.
/// This Rc-backed token is not Send. Cancel/drop never waits for an SDK callback.
pub struct NativeCookieMetadataProbe {
    state: Rc<RefCell<ProbeState>>,
    manager: Option<ICoreWebView2CookieManager>,
    handler: Option<ICoreWebView2GetCookiesCompletedHandler>,
}
impl NativeCookieMetadataProbe {
    pub fn feature_name() -> &'static str {
        "minicon-web/native-profile-diagnostics"
    }
    pub(super) fn unavailable(status: NativeCookieMetadataStatus, deadline: Instant) -> Self {
        Self {
            state: Rc::new(RefCell::new(ProbeState {
                generation: 0,
                deadline,
                active: false,
                result: Some(NativeCookieMetadataSummary::unavailable(status)),
            })),
            manager: None,
            handler: None,
        }
    }
    pub(super) fn start(
        webview: &WebView,
        uri: &str,
        generation: u64,
        deadline: Instant,
        wake: impl Fn() + 'static,
    ) -> Self {
        if !valid_uri(uri) {
            return Self::unavailable(NativeCookieMetadataStatus::InvalidUri, deadline);
        }
        if Instant::now() >= deadline {
            return Self::unavailable(NativeCookieMetadataStatus::TimedOut, deadline);
        }
        let manager = unsafe {
            webview
                .webview()
                .cast::<ICoreWebView2_2>()
                .and_then(|view| view.CookieManager())
        };
        let Ok(manager) = manager else {
            return Self::unavailable(NativeCookieMetadataStatus::Unsupported, deadline);
        };
        let state = Rc::new(RefCell::new(ProbeState {
            generation,
            deadline,
            active: true,
            result: None,
        }));
        let weak = Rc::downgrade(&state);
        let handler = GetCookiesCompletedHandler::create(Box::new(move |error, list| {
            // The weak state and no captured host/manager prevent a reference cycle.
            let Some(state) = weak.upgrade() else {
                return Ok(());
            };
            let result = catch_unwind(AssertUnwindSafe(|| {
                if !state.borrow_mut().can_read(generation, Instant::now()) {
                    return;
                }
                let summary = if error.is_err() {
                    NativeCookieMetadataSummary::unavailable(NativeCookieMetadataStatus::CallFailed)
                } else if let Some(list) = list {
                    summarize(&ComReader(list), unix_now(), deadline)
                } else {
                    NativeCookieMetadataSummary::unavailable(
                        NativeCookieMetadataStatus::PropertyUnavailable,
                    )
                };
                state
                    .borrow_mut()
                    .finish(generation, Instant::now(), summary);
            }));
            if result.is_err()
                && let Ok(mut state) = state.try_borrow_mut()
            {
                state.finish(
                    generation,
                    Instant::now(),
                    NativeCookieMetadataSummary::unavailable(
                        NativeCookieMetadataStatus::CallbackFailed,
                    ),
                );
            }
            // Neither callback body nor the waker may unwind across COM.
            let _ = catch_unwind(AssertUnwindSafe(&wake));
            Ok(())
        }));
        // Set all callback-visible state before GetCookies; do not hold a borrow
        // through the COM call, which may complete synchronously.
        let mut probe = Self {
            state,
            manager: Some(manager),
            handler: Some(handler),
        };
        let wide: Vec<u16> = uri.encode_utf16().chain(Some(0)).collect();
        if unsafe {
            probe
                .manager
                .as_ref()
                .unwrap()
                .GetCookies(PCWSTR(wide.as_ptr()), probe.handler.as_ref().unwrap())
        }
        .is_err()
        {
            probe.state.borrow_mut().finish(
                generation,
                Instant::now(),
                NativeCookieMetadataSummary::unavailable(NativeCookieMetadataStatus::CallFailed),
            );
            probe.release_completed();
        }
        probe
    }
    pub fn poll(&mut self, now: Instant) -> Option<NativeCookieMetadataSummary> {
        self.state.borrow_mut().expire(now);
        let result = self.state.borrow().result;
        if result.is_some() {
            self.release_completed();
        }
        result
    }
    pub fn cancel(&mut self) {
        self.state.borrow_mut().cancel();
        self.release_completed();
    }
    fn release_completed(&mut self) {
        self.handler = None;
        self.manager = None;
    }
}
impl Drop for NativeCookieMetadataProbe {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn valid_uri(uri: &str) -> bool {
    if uri.is_empty()
        || uri.len() > 8192
        || uri.contains('\0')
        || crate::http_url_at(uri, 0).as_deref() != Some(uri)
    {
        return false;
    }
    let Some(rest) = uri.strip_prefix("http://127.0.0.1:") else {
        return false;
    };
    let Some((port, path)) = rest.split_once('/') else {
        return false;
    };
    !path.is_empty()
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|port| port != 0)
}
fn unix_now() -> Option<f64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs_f64())
}
trait MetadataReader {
    fn count(&self) -> Result<u32, ()>;
    fn metadata(&self, index: u32) -> Result<(bool, f64), ()>;
}
struct ComReader(ICoreWebView2CookieList);
impl MetadataReader for ComReader {
    fn count(&self) -> Result<u32, ()> {
        let mut count = 0;
        unsafe { self.0.Count(&mut count) }.map_err(|_| ())?;
        Ok(count)
    }
    fn metadata(&self, index: u32) -> Result<(bool, f64), ()> {
        let cookie = unsafe { self.0.GetValueAtIndex(index) }.map_err(|_| ())?;
        let mut session = windows_core::BOOL::default();
        let mut expires = 0.0;
        unsafe { cookie.IsSession(&mut session) }.map_err(|_| ())?;
        unsafe { cookie.Expires(&mut expires) }.map_err(|_| ())?;
        Ok((session.as_bool(), expires))
    }
}
fn summarize(
    reader: &impl MetadataReader,
    now: Option<f64>,
    deadline: Instant,
) -> NativeCookieMetadataSummary {
    let Ok(total) = reader.count() else {
        return NativeCookieMetadataSummary::unavailable(
            NativeCookieMetadataStatus::PropertyUnavailable,
        );
    };
    if total > COOKIE_LIMIT {
        return NativeCookieMetadataSummary::unavailable(NativeCookieMetadataStatus::CountBound);
    }
    let mut counts = NativeCookieMetadataCounts {
        total,
        ..Default::default()
    };
    for index in 0..total {
        if Instant::now() >= deadline {
            return NativeCookieMetadataSummary::unavailable(NativeCookieMetadataStatus::TimedOut);
        }
        let Ok((session, expires)) = reader.metadata(index) else {
            return NativeCookieMetadataSummary::unavailable(
                NativeCookieMetadataStatus::PropertyUnavailable,
            );
        };
        if session {
            counts.session += 1;
        } else {
            counts.persistent += 1;
            match now {
                Some(now) if expires.is_finite() && expires >= 0.0 => {
                    if expires > now {
                        counts.expiry_future += 1;
                    } else {
                        counts.expiry_elapsed += 1;
                    }
                }
                _ => counts.expiry_unknown += 1,
            }
        }
    }
    NativeCookieMetadataSummary {
        status: NativeCookieMetadataStatus::Observed,
        counts: Some(counts),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, time::Duration};
    struct Reader {
        rows: Vec<Result<(bool, f64), ()>>,
        count: u32,
        calls: Cell<u32>,
    }
    impl MetadataReader for Reader {
        fn count(&self) -> Result<u32, ()> {
            Ok(self.count)
        }
        fn metadata(&self, index: u32) -> Result<(bool, f64), ()> {
            self.calls.set(self.calls.get() + 1);
            self.rows.get(index as usize).copied().unwrap_or(Err(()))
        }
    }
    #[test]
    fn cookie_metadata_is_bounded_and_never_turns_failure_into_zero() {
        let deadline = Instant::now() + Duration::from_secs(1);
        let reader = Reader {
            rows: vec![],
            count: COOKIE_LIMIT + 1,
            calls: Cell::new(0),
        };
        assert_eq!(
            summarize(&reader, Some(100.0), deadline).status,
            NativeCookieMetadataStatus::CountBound
        );
        assert_eq!(reader.calls.get(), 0);
        let reader = Reader {
            rows: vec![
                Ok((true, -1.0)),
                Ok((false, 101.0)),
                Ok((false, 99.0)),
                Ok((false, f64::NAN)),
            ],
            count: 4,
            calls: Cell::new(0),
        };
        assert_eq!(
            summarize(&reader, Some(100.0), deadline).counts,
            Some(NativeCookieMetadataCounts {
                total: 4,
                session: 1,
                persistent: 3,
                expiry_future: 1,
                expiry_elapsed: 1,
                expiry_unknown: 1
            })
        );
        let reader = Reader {
            rows: vec![Err(())],
            count: 1,
            calls: Cell::new(0),
        };
        assert_eq!(
            summarize(&reader, Some(100.0), deadline),
            NativeCookieMetadataSummary::unavailable(
                NativeCookieMetadataStatus::PropertyUnavailable
            )
        );
        let reader = Reader {
            rows: vec![],
            count: 0,
            calls: Cell::new(0),
        };
        assert_eq!(
            summarize(&reader, Some(100.0), deadline)
                .counts
                .unwrap()
                .total,
            0
        );
    }
    #[test]
    fn cookie_query_state_refuses_late_or_cancelled_completion() {
        let now = Instant::now();
        let observed = NativeCookieMetadataSummary {
            status: NativeCookieMetadataStatus::Observed,
            counts: Some(Default::default()),
        };
        let mut state = ProbeState {
            generation: 9,
            deadline: now + Duration::from_secs(1),
            active: true,
            result: None,
        };
        state.finish(8, now, observed);
        assert!(state.result.is_none());
        state.cancel();
        state.finish(9, now, observed);
        assert_eq!(
            state.result.unwrap().status,
            NativeCookieMetadataStatus::Cancelled
        );
        let mut state = ProbeState {
            generation: 9,
            deadline: now,
            active: true,
            result: None,
        };
        assert!(!state.can_read(9, now));
        state.finish(9, now, observed);
        assert_eq!(
            state.result.unwrap().status,
            NativeCookieMetadataStatus::TimedOut
        );
    }
    #[test]
    fn cookie_query_never_enumerates_a_whole_profile() {
        assert!(valid_uri("http://127.0.0.1:12345/page/run/a/ticket"));
        for uri in [
            "",
            "http://127.0.0.1/",
            "http://127.0.0.1:0/page",
            "http://localhost:12345/page",
            "https://provider.invalid/",
            "http://127.0.0.1:12345/",
        ] {
            assert!(!valid_uri(uri));
        }
    }
}
