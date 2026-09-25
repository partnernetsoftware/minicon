//! The `harness` backend wire layer: how one bounded task reaches a model and
//! how that model's tool calls reach `harness`'s two tools.
//!
//! Design of record: `prd/PRD_02_31_v0_2_horizon.md` ("harness -- detail"),
//! sequencing in `plan/plan-v0.2.0.md` (H4, H5).
//!
//! Split from `harness.rs` on purpose. The two tools' bounds (H2, H3) are the
//! product's security surface and are complete; everything here is the
//! transport and the turn loop, which is where a backend's own wire quirks
//! live. Keeping them in separate files keeps a wire-format fix from touching
//! a bound.

/// One HTTP round trip, so the turn loop can be tested without a network.
///
/// Deliberately this small: `harness` posts one JSON body and reads one JSON
/// body back. Anything richer would be a general HTTP client, which MiniCon
/// does not have and does not want.
pub trait Transport {
    /// POSTs `body` as `application/json` and returns the response body.
    ///
    /// A non-2xx status is an `Err` carrying the status and as much of the
    /// body as the endpoint sent, since that is where an API states why it
    /// refused (a bad key, a rejected model name).
    fn post_json(&self, url: &str, bearer: &str, body: &str) -> Result<String, String>;
}
