//! Explicit loopback model transport for the existing file/exec worker.
//! Product contract: prd/PRD_02_31_01_provider_gateway.md.

use crate::harness_wire::{HARNESS_HTTP_MAX_RESPONSE_BYTES, HARNESS_HTTP_TIMEOUT, Transport};
use minicon_platform::{
    network_http::{NetworkHttpMethod, NetworkHttpRequest},
    network_http_stream::{
        self, NetworkHttpCancel, NetworkHttpStreamControl, NetworkHttpStreamRequest,
    },
};
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Binding {
    pub gateway: String,
    pub profile: String,
    pub provider: String,
    pub account: String,
    pub model: String,
    #[serde(default = "legacy_model_alias")]
    pub alias: String,
    pub generation: String,
    pub session: String,
}

impl Binding {
    pub fn changed_field(&self, other: &Self) -> Option<&'static str> {
        [
            ("gateway", &self.gateway, &other.gateway),
            ("profile", &self.profile, &other.profile),
            ("provider", &self.provider, &other.provider),
            ("account", &self.account, &other.account),
            ("model", &self.model, &other.model),
            ("alias", &self.alias, &other.alias),
            ("generation", &self.generation, &other.generation),
            ("session", &self.session, &other.session),
        ]
        .into_iter()
        .find_map(|(name, previous, current)| (previous != current).then_some(name))
    }
}

#[derive(serde::Deserialize)]
struct RemoteBinding {
    profile: String,
    provider: String,
    account: String,
    model: String,
    #[serde(default = "legacy_model_alias")]
    alias: String,
    generation: String,
}

fn legacy_model_alias() -> String {
    "minicon-selected".into()
}

pub fn validate_url(value: &str) -> Result<String, String> {
    let port = value
        .strip_prefix("http://127.0.0.1:")
        .and_then(|rest| rest.strip_suffix("/v1"))
        .filter(|port| !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|port| port.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .ok_or_else(|| "harness: --gateway requires http://127.0.0.1:<port>/v1".to_owned())?;
    Ok(format!("http://127.0.0.1:{port}/v1"))
}

pub fn validate_env_name(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().enumerate().all(|(i, byte)| {
            byte == b'_' || byte.is_ascii_alphabetic() || (i != 0 && byte.is_ascii_digit())
        })
    {
        return Err("harness: --client-key-env requires an environment variable name".into());
    }
    Ok(())
}

pub fn new_session_id() -> Result<String, String> {
    let bytes = minicon_platform::entropy::secure_random_array::<16>()
        .map_err(|_| "harness: cannot create local session identity".to_owned())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub struct GatewayTransport {
    pub base: String,
    pub session: String,
    pub cancelled: Arc<AtomicBool>,
}

impl GatewayTransport {
    pub fn binding(&self, key: &str, timeout: Duration) -> Result<Binding, String> {
        let bytes = self.exchange(
            NetworkHttpMethod::Get,
            &format!("{}/minicon/binding", self.base),
            key,
            None,
            timeout,
        )?;
        let binding: RemoteBinding = serde_json::from_slice(&bytes)
            .map_err(|_| "harness: invalid gateway binding response".to_owned())?;
        for field in [
            &binding.profile,
            &binding.provider,
            &binding.account,
            &binding.model,
            &binding.alias,
            &binding.generation,
        ] {
            if field.is_empty() || field.len() > 512 || field.chars().any(char::is_control) {
                return Err("harness: invalid gateway binding identity".into());
            }
        }
        Ok(Binding {
            gateway: self.base.clone(),
            profile: binding.profile,
            provider: binding.provider,
            account: binding.account,
            model: binding.model,
            alias: binding.alias,
            generation: binding.generation,
            session: self.session.clone(),
        })
    }

    fn exchange(
        &self,
        method: NetworkHttpMethod,
        url: &str,
        key: &str,
        body: Option<&str>,
        timeout: Duration,
    ) -> Result<Vec<u8>, String> {
        // The transport can reach only the explicitly validated loopback base.
        if !url.starts_with(&format!("{}/", self.base)) {
            return Err("harness: gateway request escaped its base URL".into());
        }
        let mut request = NetworkHttpRequest::new(method, url)
            .header("Authorization", format!("Bearer {key}"))
            .header("Accept", "application/json")
            .header("x-minicon-session", &self.session)
            .max_redirects(0)
            .max_response_bytes(HARNESS_HTTP_MAX_RESPONSE_BYTES)
            .timeout(timeout.min(HARNESS_HTTP_TIMEOUT));
        if let Some(body) = body {
            request = request
                .header("Content-Type", "application/json")
                .body(body.as_bytes().to_vec());
        }
        let resolver = std::env::current_exe()
            .map_err(|_| "harness: native HTTP resolver executable unavailable".to_owned())?;
        let request = NetworkHttpStreamRequest::new(request, resolver);
        let cancel = NetworkHttpCancel::with_flag(self.cancelled.clone());
        let mut status = 0;
        let mut bytes = Vec::new();
        network_http_stream::request_stream(
            &request,
            &cancel,
            |head| {
                status = head.status;
                NetworkHttpStreamControl::Continue
            },
            |chunk| {
                bytes.extend_from_slice(chunk);
                NetworkHttpStreamControl::Continue
            },
        )
        .map_err(|error| format!("harness: local gateway exchange failed: {error}"))?;
        if !(200..300).contains(&status) {
            // Never echo an untrusted error body into logs or shell output.
            return Err(format!("harness: local gateway answered HTTP {status}"));
        }
        Ok(bytes)
    }
}

impl Transport for GatewayTransport {
    fn post_json(
        &self,
        url: &str,
        bearer: &str,
        body: &str,
        headers: &[(&str, &str)],
    ) -> Result<String, String> {
        self.post_json_with_timeout(url, bearer, body, headers, HARNESS_HTTP_TIMEOUT)
    }

    fn post_json_with_timeout(
        &self,
        url: &str,
        bearer: &str,
        body: &str,
        _headers: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<String, String> {
        let bytes = self.exchange(NetworkHttpMethod::Post, url, bearer, Some(body), timeout)?;
        String::from_utf8(bytes).map_err(|_| "harness: gateway response is not UTF-8".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_accepts_only_literal_loopback_and_plain_variable_names() {
        assert_eq!(
            validate_url("http://127.0.0.1:28789/v1").unwrap(),
            "http://127.0.0.1:28789/v1"
        );
        for value in [
            "http://localhost:28789/v1",
            "https://127.0.0.1:1/v1",
            "http://127.0.0.1:0/v1",
            "http://127.0.0.1:1/v1/../x",
            "http://127.0.0.1:1@evil/v1",
            "http://127.0.0.1:1/v1?x",
        ] {
            assert!(validate_url(value).is_err(), "{value}");
        }
        for value in ["", "1KEY", "KEY=value", "KEY\n"] {
            assert!(validate_env_name(value).is_err());
        }
        validate_env_name("MINICON_LOCAL_API_KEY").unwrap();
    }
}
