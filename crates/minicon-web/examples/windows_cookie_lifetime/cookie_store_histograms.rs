//! Example-private, value-free fixed Cookie backend histograms. Owner: PRD 28.01.
//! All COM calls, callback ownership and polling remain on the owning STA.
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::{Rc, Weak},
    time::Instant,
};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2CallDevToolsProtocolMethodCompletedHandler,
    ICoreWebView2CallDevToolsProtocolMethodCompletedHandler_Impl,
};
use windows_core::{HRESULT, PCWSTR};
use wry::{WebView, WebViewExtWindows};

const RESPONSE_UNITS: usize = 4096;
const RESPONSE_BYTES: usize = 8192;
const BUCKETS: usize = 32;
const COUNT: u64 = 1_000_000;
const RANGE: u64 = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Phase {
    BeforeRelease,
    BeforeReopenNavigation,
}
impl Phase {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::BeforeRelease => "a-before-context-release",
            Self::BeforeReopenNavigation => "a-reopen-before-target-navigation",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Name {
    Load,
    Commit,
}
impl Name {
    fn index(self) -> usize {
        match self {
            Self::Load => 0,
            Self::Commit => 1,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Load => "Cookie.LoadProblem",
            Self::Commit => "Cookie.CommitProblem",
        }
    }
    fn request(self) -> &'static str {
        match self {
            Self::Load => r#"{"name":"Cookie.LoadProblem","delta":false}"#,
            Self::Commit => r#"{"name":"Cookie.CommitProblem","delta":false}"#,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Status {
    PositiveBuckets,
    Missing,
    Empty,
    UnsupportedOrFailed,
    ShapeUnavailable,
    ResponseBound,
    TimedOut,
    Cancelled,
    CallbackFailed,
    AuthorityUnavailable,
}
impl Status {
    fn name(self) -> &'static str {
        match self {
            Self::PositiveBuckets => "positive-numeric-buckets",
            Self::Missing => "missing-unknown",
            Self::Empty => "empty-unknown",
            Self::UnsupportedOrFailed => "unsupported-or-failed-unknown",
            Self::ShapeUnavailable => "shape-unavailable-unknown",
            Self::ResponseBound => "response-bound-unknown",
            Self::TimedOut => "timed-out-unknown",
            Self::Cancelled => "cancelled-unknown",
            Self::CallbackFailed => "callback-failed-unknown",
            Self::AuthorityUnavailable => "authority-unavailable-unknown",
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Bucket {
    low: u64,
    high: u64,
    count: u64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Summary {
    status: Status,
    count: Option<u64>,
    sum: Option<u64>,
    buckets: Vec<Bucket>,
}
impl Summary {
    fn unavailable(status: Status) -> Self {
        Self {
            status,
            count: None,
            sum: None,
            buckets: Vec::new(),
        }
    }
    fn public(&self, name: Name) -> Value {
        json!({"name":name.name(),"status":self.status.name(),"count":self.count,"sum":self.sum,
            "buckets":self.buckets.iter().map(|b|json!({"low":b.low,"high":b.high,"count":b.count})).collect::<Vec<_>>()})
    }
}
fn parse(name: Name, raw: &str) -> Summary {
    if raw.len() > RESPONSE_BYTES {
        return Summary::unavailable(Status::ResponseBound);
    }
    let unknown = || Summary::unavailable(Status::ShapeUnavailable);
    let Ok(root) = serde_json::from_str::<Value>(raw) else {
        return unknown();
    };
    let Some(object) = root.as_object() else {
        return unknown();
    };
    let Some(histogram) = object.get("histogram") else {
        return Summary::unavailable(Status::Missing);
    };
    let Some(h) = histogram.as_object() else {
        return unknown();
    };
    if object.len() != 1
        || h.len() != 4
        || h.get("name").and_then(Value::as_str) != Some(name.name())
    {
        return unknown();
    }
    let (Some(count), Some(sum), Some(items)) = (
        h.get("count").and_then(Value::as_u64),
        h.get("sum").and_then(Value::as_u64),
        h.get("buckets").and_then(Value::as_array),
    ) else {
        return unknown();
    };
    if count > COUNT || sum > COUNT * RANGE || items.len() > BUCKETS {
        return unknown();
    }
    let mut buckets = Vec::with_capacity(items.len());
    let mut total = 0_u64;
    let mut minimum = 0_u64;
    let mut maximum = 0_u64;
    let mut previous_high = 0;
    for item in items {
        let Some(b) = item.as_object() else {
            return unknown();
        };
        let (Some(low), Some(high), Some(n)) = (
            b.get("low").and_then(Value::as_u64),
            b.get("high").and_then(Value::as_u64),
            b.get("count").and_then(Value::as_u64),
        ) else {
            return unknown();
        };
        if b.len() != 3 || high <= low || high > RANGE || low < previous_high || n > COUNT {
            return unknown();
        }
        total += n;
        minimum += low * n;
        maximum += (high - 1) * n;
        previous_high = high;
        buckets.push(Bucket {
            low,
            high,
            count: n,
        });
    }
    if total != count || sum < minimum || sum > maximum {
        return unknown();
    }
    if count == 0 || buckets.is_empty() {
        return Summary::unavailable(Status::Empty);
    }
    Summary {
        status: Status::PositiveBuckets,
        count: Some(count),
        sum: Some(sum),
        buckets,
    }
}

// The convenience webview2-com callback converts PCWSTR to String without a
// length bound before invoking a closure. Keep this raw SDK callback instead.
unsafe fn bounded_text(pointer: &PCWSTR) -> Result<String, Status> {
    if pointer.is_null() {
        return Err(Status::Missing);
    }
    for n in 0..=RESPONSE_UNITS {
        // The SDK promises a valid null-terminated callback string. Scan only
        // the finite prefix, then reject without copying an over-limit body.
        if unsafe { *pointer.0.add(n) } == 0 {
            let units = unsafe { std::slice::from_raw_parts(pointer.0, n) };
            let text = String::from_utf16(units).map_err(|_| Status::ShapeUnavailable)?;
            return if text.len() <= RESPONSE_BYTES {
                Ok(text)
            } else {
                Err(Status::ResponseBound)
            };
        }
    }
    Err(Status::ResponseBound)
}
struct State {
    generation: u64,
    deadline: Instant,
    active: bool,
    summaries: [Option<Summary>; 2],
}
impl State {
    fn expire(&mut self, now: Instant) {
        if self.active && now >= self.deadline {
            self.active = false;
            for slot in &mut self.summaries {
                if slot.is_none() {
                    *slot = Some(Summary::unavailable(Status::TimedOut));
                }
            }
        }
    }
    fn accepts(&mut self, generation: u64, now: Instant, name: Name) -> bool {
        self.expire(now);
        self.active && self.generation == generation && self.summaries[name.index()].is_none()
    }
    fn finish(&mut self, generation: u64, now: Instant, name: Name, summary: Summary) {
        if self.accepts(generation, now, name) {
            self.summaries[name.index()] = Some(summary);
            if self.summaries.iter().all(Option::is_some) {
                self.active = false;
            }
        }
    }
    fn cancel(&mut self) {
        self.active = false;
        for slot in &mut self.summaries {
            *slot = Some(Summary::unavailable(Status::Cancelled));
        }
    }
}
#[windows_core::implement(ICoreWebView2CallDevToolsProtocolMethodCompletedHandler)]
struct RawHandler {
    state: Weak<RefCell<State>>,
    generation: u64,
    name: Name,
}
impl ICoreWebView2CallDevToolsProtocolMethodCompletedHandler_Impl for RawHandler_Impl {
    fn Invoke(&self, error: HRESULT, value: &PCWSTR) -> windows_core::Result<()> {
        let Some(state) = self.state.upgrade() else {
            return Ok(());
        };
        let completed = catch_unwind(AssertUnwindSafe(|| {
            if !state
                .borrow_mut()
                .accepts(self.generation, Instant::now(), self.name)
            {
                return;
            }
            let summary = if error.is_err() {
                Summary::unavailable(Status::UnsupportedOrFailed)
            } else {
                match unsafe { bounded_text(value) } {
                    Ok(text) => parse(self.name, &text),
                    Err(status) => Summary::unavailable(status),
                }
            };
            state
                .borrow_mut()
                .finish(self.generation, Instant::now(), self.name, summary);
        }));
        if completed.is_err()
            && let Ok(mut state) = state.try_borrow_mut()
        {
            state.finish(
                self.generation,
                Instant::now(),
                self.name,
                Summary::unavailable(Status::CallbackFailed),
            );
        }
        Ok(())
    }
}
/// Not Send; Weak callbacks hold neither WebView nor environment nor host.
pub(super) struct Probe {
    state: Rc<RefCell<State>>,
    handlers: [Option<ICoreWebView2CallDevToolsProtocolMethodCompletedHandler>; 2],
}
impl Probe {
    pub(super) fn start(view: &WebView, generation: u64, deadline: Instant) -> Self {
        let state = Rc::new(RefCell::new(State {
            generation,
            deadline,
            active: true,
            summaries: [None, None],
        }));
        let mut probe = Self {
            state,
            handlers: [None, None],
        };
        for name in [Name::Load, Name::Commit] {
            if Instant::now() >= deadline {
                probe.state.borrow_mut().expire(Instant::now());
                break;
            }
            let handler: ICoreWebView2CallDevToolsProtocolMethodCompletedHandler = RawHandler {
                state: Rc::downgrade(&probe.state),
                generation,
                name,
            }
            .into();
            // The two requests are fixed literals, no URI/user values are used.
            let parameters: Vec<u16> = name.request().encode_utf16().chain(Some(0)).collect();
            if parameters.len() > 128 {
                probe.state.borrow_mut().finish(
                    generation,
                    Instant::now(),
                    name,
                    Summary::unavailable(Status::ShapeUnavailable),
                );
                continue;
            }
            let result = unsafe {
                view.webview().CallDevToolsProtocolMethod(
                    windows_core::w!("Browser.getHistogram"),
                    PCWSTR(parameters.as_ptr()),
                    &handler,
                )
            };
            if result.is_err() {
                probe.state.borrow_mut().finish(
                    generation,
                    Instant::now(),
                    name,
                    Summary::unavailable(Status::UnsupportedOrFailed),
                );
            }
            probe.handlers[name.index()] = Some(handler);
        }
        probe
    }
    pub(super) fn poll(&mut self, now: Instant) -> Option<Value> {
        self.state.borrow_mut().expire(now);
        let mut state = self.state.borrow_mut();
        if state.active {
            return None;
        }
        let load = state.summaries[0].take()?;
        let commit = state.summaries[1].take()?;
        drop(state);
        self.handlers = [None, None];
        Some(json!({"status":"diagnostic-only-not-a-persistence-verdict",
            "histograms":[load.public(Name::Load),commit.public(Name::Commit)]}))
    }
    pub(super) fn authority_unavailable() -> Value {
        let unknown = Summary::unavailable(Status::AuthorityUnavailable);
        json!({"status":"authority-unavailable-unknown","histograms":[unknown.public(Name::Load),unknown.public(Name::Commit)]})
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        self.state.borrow_mut().cancel();
        self.handlers = [None, None];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    fn payload(name: Name) -> String {
        json!({"histogram":{"name":name.name(),"sum":3,"count":1,"buckets":[{"low":3,"high":4,"count":1}]}}).to_string()
    }
    #[test]
    fn numeric_backend_problem_reduces_without_cookie_or_error_strings() {
        let summary = parse(Name::Load, &payload(Name::Load));
        assert_eq!(summary.status, Status::PositiveBuckets);
        assert_eq!(summary.count, Some(1));
        assert_eq!(
            summary.buckets,
            vec![Bucket {
                low: 3,
                high: 4,
                count: 1
            }]
        );
        let secret = r#"{"histogram":{"name":"private-path","sum":3,"count":1,"buckets":[{"low":3,"high":4,"count":1}]}}"#;
        let refused = parse(Name::Load, secret).public(Name::Load).to_string();
        assert!(!refused.contains("private-path"));
        assert_eq!(parse(Name::Load, secret).status, Status::ShapeUnavailable);
    }
    #[test]
    fn missing_empty_unknown_and_nonfinite_shapes_remain_unknown() {
        assert_eq!(parse(Name::Load, "{}").status, Status::Missing);
        let mut raw: Value = serde_json::from_str(&payload(Name::Load)).unwrap();
        raw["histogram"]["buckets"] = json!([]);
        raw["histogram"]["count"] = json!(0);
        raw["histogram"]["sum"] = json!(0);
        assert_eq!(parse(Name::Load, &raw.to_string()).status, Status::Empty);
        raw["histogram"]["count"] = json!(-1);
        assert_eq!(
            parse(Name::Load, &raw.to_string()).status,
            Status::ShapeUnavailable
        );
        assert_eq!(
            parse(Name::Commit, &payload(Name::Load)).status,
            Status::ShapeUnavailable
        );
    }
    #[test]
    fn response_prefix_is_bounded_before_json_allocation() {
        let large = format!("{}{}", payload(Name::Load), " ".repeat(RESPONSE_BYTES));
        assert_eq!(parse(Name::Load, &large).status, Status::ResponseBound);
        let units: Vec<u16> = std::iter::repeat_n(b' ' as u16, RESPONSE_UNITS + 1)
            .chain(Some(0))
            .collect();
        assert_eq!(
            unsafe { bounded_text(&PCWSTR(units.as_ptr())) },
            Err(Status::ResponseBound)
        );
        let short: Vec<u16> = payload(Name::Load).encode_utf16().chain(Some(0)).collect();
        assert_eq!(
            unsafe { bounded_text(&PCWSTR(short.as_ptr())) }.unwrap(),
            payload(Name::Load)
        );
    }
    #[test]
    fn numeric_bucket_count_and_range_cannot_escape_fixed_bound() {
        let mut raw: Value = serde_json::from_str(&payload(Name::Load)).unwrap();
        raw["histogram"]["buckets"] = Value::Array(
            (0..=BUCKETS)
                .map(|i| json!({"low":i,"high":i+1,"count":1}))
                .collect(),
        );
        raw["histogram"]["count"] = json!(BUCKETS + 1);
        raw["histogram"]["sum"] = json!((0..=BUCKETS).sum::<usize>());
        assert_eq!(
            parse(Name::Load, &raw.to_string()).status,
            Status::ShapeUnavailable
        );
        let mut raw: Value = serde_json::from_str(&payload(Name::Load)).unwrap();
        raw["histogram"]["buckets"] = json!([{"low":3,"high":257,"count":1}]);
        assert_eq!(
            parse(Name::Load, &raw.to_string()).status,
            Status::ShapeUnavailable
        );
        let mut raw: Value = serde_json::from_str(&payload(Name::Load)).unwrap();
        raw["histogram"]["count"] = json!(2);
        assert_eq!(
            parse(Name::Load, &raw.to_string()).status,
            Status::ShapeUnavailable
        );
    }
    #[test]
    fn deadline_and_cancel_reject_late_callback_generation() {
        let now = Instant::now();
        let mut state = State {
            generation: 2,
            deadline: now + Duration::from_secs(1),
            active: true,
            summaries: [None, None],
        };
        state.finish(1, now, Name::Load, parse(Name::Load, &payload(Name::Load)));
        assert!(state.summaries.iter().all(Option::is_none));
        state.expire(now + Duration::from_secs(1));
        state.finish(
            2,
            now + Duration::from_secs(1),
            Name::Load,
            parse(Name::Load, &payload(Name::Load)),
        );
        assert_eq!(
            state.summaries[0],
            Some(Summary::unavailable(Status::TimedOut))
        );
        let mut pending = State {
            generation: 2,
            deadline: now + Duration::from_secs(1),
            active: true,
            summaries: [None, None],
        };
        pending.cancel();
        let cancelled = [
            Some(Summary::unavailable(Status::Cancelled)),
            Some(Summary::unavailable(Status::Cancelled)),
        ];
        assert_eq!(pending.summaries, cancelled);
        pending.finish(
            2,
            now,
            Name::Commit,
            parse(Name::Commit, &payload(Name::Commit)),
        );
        assert_eq!(pending.summaries, cancelled);
    }
    #[test]
    fn duplicate_callback_cannot_replace_the_first_finite_result() {
        let now = Instant::now();
        let mut state = State {
            generation: 2,
            deadline: now + Duration::from_secs(1),
            active: true,
            summaries: [None, None],
        };
        state.finish(2, now, Name::Load, Summary::unavailable(Status::Missing));
        state.finish(2, now, Name::Load, parse(Name::Load, &payload(Name::Load)));
        assert_eq!(state.summaries[0].as_ref().unwrap().status, Status::Missing);
        state.finish(
            2,
            now,
            Name::Commit,
            parse(Name::Commit, &payload(Name::Commit)),
        );
        assert!(!state.active);
    }
}
