//! Fixed official account metadata endpoints, separate from inference admission.
use super::*;
use crate::provider_login::LoginMetadata;
#[cfg(test)]
use crate::provider_monitoring::Reading;
use crate::provider_monitoring::{
    self as monitor, Authority, DiskView, MonitorError, MonitorSnapshot,
};
use std::sync::atomic::{AtomicU64, Ordering};

impl Gateway {
    pub(crate) fn monitor_disk_view(&self) -> Option<DiskView> {
        self.usage.get().and_then(usage::UsageHistory::monitor_view)
    }
    pub(crate) fn submit_monitor_snapshot(
        &self,
        snapshot: Arc<MonitorSnapshot>,
    ) -> Result<(), MonitorError> {
        self.usage
            .get()
            .ok_or(MonitorError::Storage)?
            .submit_monitor(snapshot)
    }
    /// The caller owns one finite job. Register account/Stop cancellation but
    /// do NOT borrow a model lease, profile/session, usage purpose, or capacity.
    #[cfg(test)]
    pub(crate) fn read_account_metadata(
        &self,
        account: &str,
        requested: &Authority,
        metadata: &LoginMetadata,
        cancel: &Arc<NetworkHttpCancel>,
        stop: &AtomicU64,
        epoch: u64,
    ) -> Result<Reading, MonitorError> {
        self.read_account_metadata_retry(account, requested, metadata, cancel, stop, epoch)
            .result
    }
    pub(crate) fn read_account_metadata_retry(
        &self,
        account: &str,
        requested: &Authority,
        metadata: &LoginMetadata,
        cancel: &Arc<NetworkHttpCancel>,
        stop: &AtomicU64,
        epoch: u64,
    ) -> monitor::MetadataRead {
        let mut retry_after = monitor::RetryAfter::Missing;
        let mut grant_stamp = None;
        let result = (|| {
            let still_running = || {
                if cancel.is_cancelled() || stop.load(Ordering::Acquire) != epoch {
                    Err(MonitorError::Cancelled)
                } else {
                    Ok(())
                }
            };
            still_running()?;
            let initial = {
                let mut state = self.state.lock().map_err(|_| MonitorError::Storage)?;
                if state.disabled.contains(account) {
                    return Err(MonitorError::NeedsAuth);
                }
                let snapshot = self.store.snapshot(account).map_err(monitor::store_error)?;
                if !snapshot.preferences.enabled {
                    return Err(MonitorError::Disabled);
                }
                if !requested.matches(&snapshot) {
                    return Err(MonitorError::StaleAuthority);
                }
                state.active.retain(|a| a.cancel.strong_count() != 0);
                state.active.push(Active {
                    account: account.to_owned(),
                    profile: None,
                    cancel: Arc::downgrade(cancel),
                });
                snapshot
            };
            let mut budget = CatalogueBudget {
                deadline: Instant::now() + Duration::from_secs(monitor::HTTP_SECONDS),
                response_bytes: 0,
            };
            let initial_grant = self
                .store
                .load_grant_for_authority(account, &initial.generation, &initial.organization)
                .map_err(monitor::store_error)?;
            grant_stamp = Some(monitor::GrantStamp::capture(&initial_grant));
            // Reuse the exact refresh mutex/HTTP/parse/CAS implementation while
            // retaining metadata outcomes. No raw response or grant is projected.
            let mut refresh_failure = None;
            self.grant_budgeted_with_refresh_error(
                account,
                cancel,
                &initial.preferences.service,
                &initial.generation,
                Some(&mut budget),
                |response, error| {
                    retry_after = monitor::parse_retry_after(&response.headers, monitor::now_ms());
                    refresh_failure = Some(metadata_refresh_error(error));
                    ServerError::Upstream
                },
            )
            .map_err(|error| refresh_failure.unwrap_or_else(|| metadata_error(error)))?;
            still_running()?;
            let current = self.store.snapshot(account).map_err(monitor::store_error)?;
            if !requested.matches(&current) {
                return Err(MonitorError::StaleAuthority);
            }
            let grant = self
                .store
                .load_grant_for_authority(account, &current.generation, &current.organization)
                .map_err(monitor::store_error)?;
            if grant.provider != current.summary.provider {
                return Err(MonitorError::StaleAuthority);
            }
            grant_stamp = Some(monitor::GrantStamp::capture(&grant));
            let request = metadata_request(&grant, metadata)?;
            // Recheck the account gate immediately before sending, using lock order
            // Gateway -> Store. No application lock remains held during HTTP.
            {
                let state = self.state.lock().map_err(|_| MonitorError::Storage)?;
                if state.disabled.contains(account) {
                    return Err(MonitorError::NeedsAuth);
                }
                self.store
                    .with_snapshot(account, |s| requested.matches(&s))
                    .map_err(monitor::store_error)?
                    .then_some(())
                    .ok_or(MonitorError::StaleAuthority)?;
                still_running()?;
            }
            let response = budget
                .collect(
                    self,
                    request,
                    cancel,
                    monitor::RESPONSE_BYTES,
                    &initial.preferences.service,
                )
                .map_err(|e| {
                    if budget.remaining().is_err() {
                        MonitorError::Deadline
                    } else {
                        metadata_error(e)
                    }
                })?;
            still_running()?;
            budget.remaining().map_err(|_| MonitorError::Deadline)?;
            retry_after = monitor::parse_retry_after(&response.headers, monitor::now_ms());
            let reading = monitor::parse_response(
                current.summary.provider,
                &response,
                monitor::now_ms().ok_or(MonitorError::Malformed)?,
            )?;
            self.store
                .with_snapshot(account, |s| requested.matches(&s))
                .map_err(monitor::store_error)?
                .then_some(())
                .ok_or(MonitorError::StaleAuthority)?;
            Ok(reading)
        })();
        monitor::MetadataRead {
            result,
            retry_after,
            grant_stamp,
        }
    }
}
fn metadata_refresh_error(error: provider_auth::AuthError) -> MonitorError {
    use provider_auth::AuthError;
    match error {
        AuthError::HttpStatus(429) => MonitorError::RateLimited,
        AuthError::HttpStatus(403) | AuthError::AuthorizationRejected => MonitorError::Forbidden,
        AuthError::HttpStatus(401) | AuthError::NeedsLogin | AuthError::Expired => {
            MonitorError::NeedsAuth
        }
        AuthError::InvalidResponse | AuthError::ResponseTooLarge => MonitorError::Malformed,
        AuthError::Cancelled | AuthError::StaleAttempt => MonitorError::Cancelled,
        _ => MonitorError::Network,
    }
}
fn metadata_error(error: ServerError) -> MonitorError {
    match error {
        ServerError::Cancelled => MonitorError::Cancelled,
        ServerError::MissingAccount => MonitorError::MissingAccount,
        ServerError::NeedsLogin => MonitorError::NeedsAuth,
        ServerError::BindingChanged => MonitorError::StaleAuthority,
        ServerError::Storage => MonitorError::Storage,
        ServerError::Busy => MonitorError::Busy,
        _ => MonitorError::Network,
    }
}
fn metadata_request(
    grant: &TokenGrant,
    metadata: &LoginMetadata,
) -> Result<NetworkHttpRequest, MonitorError> {
    let request = match grant.provider {
        Provider::DeepSeek => {
            let platform = if cfg!(target_os = "windows") {
                "desktop-win"
            } else if cfg!(target_os = "macos") {
                "desktop-mac"
            } else {
                "desktop-linux"
            };
            let locale = if metadata
                .locale
                .split(['-', '_'])
                .next()
                .is_some_and(|v| v.eq_ignore_ascii_case("zh"))
            {
                "zh_CN"
            } else {
                "en_US"
            };
            provider_auth::deepseek_summary_request(
                &AuthClient::available(Provider::DeepSeek),
                grant,
                &provider_auth::DeepSeekMetadata {
                    version: &metadata.version,
                    platform,
                    locale,
                    timezone_offset_seconds: metadata.timezone_offset_seconds,
                },
            )
            .map_err(|_| MonitorError::InvalidAction)?
        }
        Provider::OpenCode => {
            let org = grant
                .organization
                .as_deref()
                .filter(|o| provider_auth::valid_organization_id(o))
                .ok_or(MonitorError::NeedsOrganization)?;
            NetworkHttpRequest::new(
                NetworkHttpMethod::Get,
                "https://opencode.ai/zen/go/v1/usage",
            )
            .header(
                "Authorization",
                format!("Bearer {}", grant.access_token.expose()),
            )
            .header("x-opencode-org-id", org)
            .header("Accept", "application/json")
        }
    }
    .timeout(Duration::from_secs(monitor::HTTP_SECONDS))
    .max_redirects(0);
    minicon_platform::network_http::validate(&request).map_err(|_| MonitorError::InvalidAction)?;
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_accounts::AccountState;
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
    };
    struct Fixture {
        root: PathBuf,
        gateway: Arc<Gateway>,
        accounts: Vec<String>,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("minicon-monitor-{}", random_id().unwrap()));
            std::fs::create_dir_all(&root).unwrap();
            let store = Arc::new(AccountStore::new(root.join("accounts")).unwrap());
            let mut accounts = vec![];
            for token in ["synthetic-a", "synthetic-b"] {
                accounts.push(
                    store
                        .insert(
                            "Synthetic metadata fixture",
                            TokenGrant {
                                provider: Provider::DeepSeek,
                                access_token: Secret::new(token).unwrap(),
                                lifecycle: TokenLifecycle::OpaqueUntilRejected,
                                organization: None,
                            },
                        )
                        .unwrap()
                        .id,
                );
            }
            let gateway = Gateway::new(store, Config::fresh().unwrap(), None).unwrap();
            Self {
                root,
                gateway,
                accounts,
            }
        }
        fn authority(&self, id: &str) -> Authority {
            Authority::capture(&self.gateway.store.snapshot(id).unwrap())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn metadata() -> LoginMetadata {
        LoginMetadata {
            version: env!("CARGO_PKG_VERSION").into(),
            locale: "zh-TW".into(),
            timezone_offset_seconds: 0,
        }
    }
    fn peer(listener: &TcpListener) -> (TcpStream, String) {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut peer = loop {
            match listener.accept() {
                Ok((p, _)) => break p,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "controlled metadata request never arrived"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("controlled accept failed: {e}"),
            }
        };
        peer.set_nonblocking(false).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
        let mut bytes = vec![];
        let mut b = [0];
        while !bytes.ends_with(b"\r\n\r\n") {
            assert!(bytes.len() < 8192);
            peer.read_exact(&mut b).unwrap();
            bytes.push(b[0]);
        }
        (peer, String::from_utf8(bytes).unwrap())
    }
    fn reply(mut peer: TcpStream, status: u16, body: &str) {
        write!(
            peer,
            "HTTP/1.1 {status} fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    }
    fn start(
        f: &Fixture,
        id: &str,
        stop: Arc<AtomicU64>,
    ) -> JoinHandle<Result<Reading, MonitorError>> {
        let gateway = f.gateway.clone();
        let id = id.to_owned();
        let authority = f.authority(&id);
        std::thread::spawn(move || {
            gateway.read_account_metadata(
                &id,
                &authority,
                &metadata(),
                &Arc::new(NetworkHttpCancel::new()),
                &stop,
                0,
            )
        })
    }
    use std::thread::JoinHandle;
    const WALLET: &str = r#"{"code":0,"data":{"biz_code":0,"biz_data":{"normal_wallets":[{"currency":"CNY","balance":"1.25"}],"bonus_wallets":[]}}}"#;

    fn expiring_go_account(f: &Fixture) -> String {
        let id = f
            .gateway
            .store
            .insert(
                "Synthetic expiring metadata",
                TokenGrant {
                    provider: Provider::OpenCode,
                    access_token: Secret::new("synthetic-expiring-access").unwrap(),
                    lifecycle: TokenLifecycle::ExpiringRefresh {
                        refresh_token: Secret::new("synthetic-expiring-refresh").unwrap(),
                        expires_at: Instant::now() + Duration::from_secs(5),
                    },
                    organization: Some("org_selected".into()),
                },
            )
            .unwrap()
            .id;
        f.gateway.store.mark_ready(&id).unwrap();
        id
    }
    fn start_rich(f: &Fixture, id: &str) -> JoinHandle<monitor::MetadataRead> {
        let gateway = f.gateway.clone();
        let authority = f.authority(id);
        let id = id.to_owned();
        std::thread::spawn(move || {
            gateway.read_account_metadata_retry(
                &id,
                &authority,
                &metadata(),
                &Arc::new(NetworkHttpCancel::new()),
                &AtomicU64::new(0),
                0,
            )
        })
    }
    fn refresh_peer(listener: &TcpListener) -> (TcpStream, String) {
        let (mut connection, headers) = peer(listener);
        let length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        assert!(length > 0 && length <= 4096);
        // Drain the synthetic POST body before closing; unread bytes may cause
        // a socket reset instead of delivering the controlled response.
        let mut body = vec![0; length];
        connection.read_exact(&mut body).unwrap();
        (connection, headers)
    }
    #[test]
    fn provider_monitor_actual_refresh_preserves_typed_failures_and_retry_after() {
        for (status, body, expected) in [
            (429, "{}", MonitorError::RateLimited),
            (503, "{}", MonitorError::Network),
            (200, "not-json", MonitorError::Malformed),
            (400, r#"{"error":"invalid_grant"}"#, MonitorError::NeedsAuth),
            (400, r#"{"error":"expired_token"}"#, MonitorError::NeedsAuth),
            (400, r#"{"error":"access_denied"}"#, MonitorError::Forbidden),
            (403, "{}", MonitorError::Forbidden),
            (401, "{}", MonitorError::NeedsAuth),
        ] {
            let f = Fixture::new();
            let id = expiring_go_account(&f);
            let before = f.gateway.store.snapshot(&id).unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            f.gateway.fixture_catalogue_upstream(format!(
                "http://{}/refresh-only",
                listener.local_addr().unwrap()
            ));
            let worker = start_rich(&f, &id);
            let (mut connection, headers) = refresh_peer(&listener);
            assert!(headers.starts_with("POST /refresh-only "));
            write!(connection, "HTTP/1.1 {status} fixture\r\nRetry-After: 1200\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            drop(connection);
            let read = worker.join().unwrap();
            assert_eq!(
                read.result,
                Err(expected),
                "refresh outcome collapsed for status {status}"
            );
            assert!(
                matches!(read.retry_after,monitor::RetryAfter::NotBefore(until) if until >= monitor::now_ms().unwrap()+1_190_000),
                "refresh Retry-After did not survive the shared mapper"
            );
            assert!(
                read.grant_stamp.is_some(),
                "refresh rejection lost the actually used grant identity"
            );
            assert!(
                matches!(listener.accept(),Err(e)if e.kind()==std::io::ErrorKind::WouldBlock),
                "metadata GET was sent after refresh failed"
            );
            assert_eq!(
                f.gateway.store.snapshot(&id).unwrap(),
                before,
                "metadata refresh failure changed inference authority"
            );
            assert!(!f.gateway.state.lock().unwrap().disabled.contains(&id));
        }
    }
    #[test]
    fn provider_monitor_actual_refresh_success_uses_new_grant_for_go_get() {
        let f = Fixture::new();
        let id = expiring_go_account(&f);
        let before = f.gateway.store.snapshot(&id).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        f.gateway.fixture_catalogue_upstream(format!(
            "http://{}/refresh-then-go",
            listener.local_addr().unwrap()
        ));
        let worker = start_rich(&f, &id);
        let (connection, headers) = refresh_peer(&listener);
        assert!(headers.starts_with("POST /refresh-then-go "));
        reply(
            connection,
            200,
            r#"{"access_token":"synthetic-refreshed-access","refresh_token":"synthetic-refreshed-refresh","expires_in":3600}"#,
        );
        let (connection, headers) = peer(&listener);
        reply(
            connection,
            200,
            r#"{"usage":{"rolling":{"status":"ok","percent":1,"resetsAt":"2026-10-26T01:00:00Z"},"weekly":{"status":"ok","percent":2,"resetsAt":"2026-10-26T01:00:00Z"},"monthly":{"status":"ok","percent":3,"resetsAt":"2026-10-26T01:00:00Z"}}}"#,
        );
        let read = worker.join().unwrap();
        // Assert the immutable actually sent headers after joining the worker;
        // an intended original-assert RED must not detach an in-flight request.
        assert!(headers.starts_with("GET /refresh-then-go "));
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("authorization: bearer synthetic-refreshed-access"),
            "metadata GET reused the old access grant after refresh"
        );
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("x-opencode-org-id: org_selected")
        );
        assert!(
            read.result.is_ok(),
            "successful shared refresh failed to reach Go metadata"
        );
        let after = f.gateway.store.snapshot(&id).unwrap();
        assert_eq!(after.generation, before.generation);
        assert_eq!(after.organization, before.organization);
        assert_eq!(after.summary.state, AccountState::AccountReady);
    }

    #[test]
    fn provider_monitor_actual_two_accounts_do_not_use_model_capacity_or_mark_ready() {
        let f = Fixture::new();
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        f.gateway.fixture_catalogue_upstream(format!(
            "http://{}/metadata",
            upstream.local_addr().unwrap()
        ));
        for (i, id) in f.accounts.iter().enumerate() {
            let worker = start(&f, id, Arc::new(AtomicU64::new(0)));
            let (peer, headers) = peer(&upstream);
            assert!(headers.starts_with("GET /metadata "));
            assert!(headers.to_ascii_lowercase().contains(if i == 0 {
                "x-dsh-auth-token: synthetic-a"
            } else {
                "x-dsh-auth-token: synthetic-b"
            }));
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("x-client-bundle-id: minicon")
            );
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("x-client-locale: zh_cn")
            );
            assert!(
                f.gateway.state.lock().unwrap().leases.is_empty(),
                "metadata consumed inference capacity"
            );
            reply(peer, 200, WALLET);
            assert!(worker.join().unwrap().is_ok());
            assert_eq!(
                f.gateway.store.snapshot(id).unwrap().summary.state,
                AccountState::CredentialStored,
                "wallet read marked inference Ready"
            );
        }
    }
    #[test]
    fn provider_monitor_actual_late_reconnect_rejects_wallet_and_preserves_other_account() {
        let f = Fixture::new();
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        f.gateway.fixture_catalogue_upstream(format!(
            "http://{}/metadata",
            upstream.local_addr().unwrap()
        ));
        let id = &f.accounts[0];
        let before = f.gateway.store.snapshot(id).unwrap();
        let other = f.gateway.store.snapshot(&f.accounts[1]).unwrap();
        let worker = start(&f, id, Arc::new(AtomicU64::new(0)));
        let (peer, _) = peer(&upstream);
        f.gateway
            .store
            .reconnect_if_current(
                id,
                &before.generation,
                TokenGrant {
                    provider: Provider::DeepSeek,
                    access_token: Secret::new("synthetic-new").unwrap(),
                    lifecycle: TokenLifecycle::OpaqueUntilRejected,
                    organization: None,
                },
            )
            .unwrap();
        reply(peer, 200, WALLET);
        assert_eq!(worker.join().unwrap(), Err(MonitorError::StaleAuthority));
        assert_eq!(f.gateway.store.snapshot(&f.accounts[1]).unwrap(), other);
    }
    #[test]
    fn provider_monitor_actual_403_and_stop_are_metadata_only() {
        let f = Fixture::new();
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        f.gateway.fixture_catalogue_upstream(format!(
            "http://{}/metadata",
            upstream.local_addr().unwrap()
        ));
        let id = &f.accounts[0];
        f.gateway.store.mark_ready(id).unwrap();
        let worker = start(&f, id, Arc::new(AtomicU64::new(0)));
        let (connection, _) = peer(&upstream);
        reply(connection, 403, "{}");
        assert_eq!(worker.join().unwrap(), Err(MonitorError::Forbidden));
        assert!(
            !f.gateway.state.lock().unwrap().disabled.contains(id),
            "metadata permission failure disabled inference"
        );
        assert_eq!(
            f.gateway.store.snapshot(id).unwrap().summary.state,
            AccountState::AccountReady
        );
        let stop = Arc::new(AtomicU64::new(0));
        let worker = start(&f, id, stop.clone());
        let (peer, _) = peer(&upstream);
        stop.store(1, Ordering::Release);
        reply(peer, 200, WALLET);
        assert_eq!(worker.join().unwrap(), Err(MonitorError::Cancelled));
    }
    #[test]
    fn provider_monitor_fixed_go_contract_uses_chosen_org_and_distinct_header() {
        let grant = TokenGrant {
            provider: Provider::OpenCode,
            access_token: Secret::new("synthetic-go").unwrap(),
            lifecycle: TokenLifecycle::ExpiringRefresh {
                refresh_token: Secret::new("synthetic-refresh").unwrap(),
                expires_at: Instant::now() + Duration::from_secs(3600),
            },
            organization: Some("org_selected".into()),
        };
        let request = metadata_request(&grant, &metadata()).unwrap();
        assert_eq!(request.url, "https://opencode.ai/zen/go/v1/usage");
        assert!(
            request
                .headers
                .iter()
                .any(|(k, v)| k == "x-opencode-org-id" && v == "org_selected")
        );
        assert!(!request.headers.iter().any(|(k, _)| k == "x-org-id"));
    }
}
