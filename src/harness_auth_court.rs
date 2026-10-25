//! Dedicated native context/live-page court. Owner PRD28/31.01.02.
#![cfg(feature = "native-auth-court")]
use super::*;
use crate::native_auth_fixture::{Action, Notice, Recipe, Worker};
#[cfg(test)]
use crate::native_auth_fixture::{WindowsProfileAclMode, fresh_acl_control_directory};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::atomic::Ordering as CourtOrdering;
use std::time::Duration;
#[cfg(test)]
mod profile_acl_tests {
    use super::*;

    #[test]
    fn alternate_acl_policy_requires_an_explicit_supported_fixture_mode() {
        assert!(matches!(
            WindowsProfileAclMode::read(None),
            Ok(WindowsProfileAclMode::OwnerOnly)
        ));
        assert!(matches!(
            WindowsProfileAclMode::read(Some(&serde_json::json!("owner-only"))),
            Ok(WindowsProfileAclMode::OwnerOnly)
        ));
        for value in [
            serde_json::json!(null),
            serde_json::json!(true),
            serde_json::json!("inherit"),
        ] {
            assert!(WindowsProfileAclMode::read(Some(&value)).is_err());
        }
        let alternate = WindowsProfileAclMode::read(Some(&serde_json::json!("inherit-default")));
        assert_eq!(alternate.is_ok(), cfg!(windows));
    }

    #[test]
    fn alternate_acl_fixture_refuses_other_paths_and_nonempty_profiles() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let allocated = std::env::temp_dir().join(format!(
            "minicon-acl-fixture-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&allocated).unwrap();
        let output = allocated.canonicalize().unwrap();
        std::fs::create_dir(output.join("profiles")).unwrap();
        for name in ["a", "b"] {
            let directory = output.join("profiles").join(name);
            std::fs::create_dir(&directory).unwrap();
            assert_eq!(
                fresh_acl_control_directory(&output, &directory, name),
                Ok(())
            );
        }
        let a = output.join("profiles/a");
        assert_eq!(
            fresh_acl_control_directory(&output, &output, "a"),
            Err("native-court-profile-acl-control-outside-fixture")
        );
        assert_eq!(
            fresh_acl_control_directory(&output, &a, "b"),
            Err("native-court-profile-acl-control-outside-fixture")
        );
        std::fs::write(a.join("occupied"), b"synthetic").unwrap();
        assert_eq!(
            fresh_acl_control_directory(&output, &a, "a"),
            Err("native-court-profile-acl-control-not-empty")
        );
        std::fs::remove_file(a.join("occupied")).unwrap();
        std::fs::remove_dir(a).unwrap();
        std::fs::remove_dir(output.join("profiles/b")).unwrap();
        std::fs::remove_dir(output.join("profiles")).unwrap();
        std::fs::remove_dir(output).unwrap();
    }
}

#[cfg(windows)]
fn recipe_auth_action(
    action: &Action,
    pages: &std::collections::HashMap<String, String>,
    seq: u64,
) -> bool {
    let phase = match seq {
        1 => "a",
        2 => "b",
        3 => "a-reopen",
        _ => return false,
    };
    action.kind == "present-auth"
        && action.seq == seq
        && action.phase == phase
        && action
            .url
            .as_deref()
            .is_some_and(|url| pages.get(phase).is_some_and(|expected| expected == url))
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CookieGateStage {
    Armed,
    Sampling,
    Released,
    Discarded,
}

/// Per-action test-driver state; never a product/global navigation policy.
#[cfg(windows)]
pub(super) struct PreNavigationCookieProbe {
    attempt: provider_runtime::AttemptId,
    context: browser::PageContext,
    url: String,
    phase: String,
    measurement_stage: &'static str,
    deadline: Instant,
    stage: CookieGateStage,
    query: Option<minicon_web::NativeCookieMetadataProbe>,
    result: Option<minicon_web::NativeCookieMetadataSummary>,
    reported: bool,
}
#[cfg(windows)]
impl PreNavigationCookieProbe {
    fn arm(
        attempt: provider_runtime::AttemptId,
        context: browser::PageContext,
        url: &str,
        phase: &str,
        court_deadline: Instant,
    ) -> Self {
        Self {
            attempt,
            context,
            url: url.to_owned(),
            phase: phase.to_owned(),
            measurement_stage: "before-target-navigation",
            deadline: court_deadline,
            stage: CookieGateStage::Armed,
            query: None,
            result: None,
            reported: false,
        }
    }
    fn matches(
        &self,
        attempt: Option<provider_runtime::AttemptId>,
        context: Option<&browser::PageContext>,
        url: &str,
    ) -> bool {
        attempt == Some(self.attempt) && context == Some(&self.context) && url == self.url
    }
    fn current(&self, app: &ConApp) -> bool {
        let attempt = app
            .harness_settings
            .as_ref()
            .and_then(|settings| settings.auth_attempt());
        !app.exit
            && app.dismissed_auth_attempt != Some(self.attempt)
            && attempt == Some(self.attempt)
            && app
                .browser_auth
                .as_ref()
                .is_some_and(|auth| auth.attempt == self.attempt && auth.context == self.context)
    }
    fn matches_action(
        &self,
        action: Option<&Action>,
        pages: &std::collections::HashMap<String, String>,
    ) -> bool {
        let Some(action) = action else {
            return false;
        };
        let seq = match self.phase.as_str() {
            "a" => 1,
            "b" => 2,
            "a-reopen" => 3,
            _ => return false,
        };
        recipe_auth_action(action, pages, seq)
            && self.attempt.0 == 100 + seq
            && action.url.as_deref() == Some(self.url.as_str())
    }
    fn before_release_authorized(
        &self,
        queued: &Action,
        pages: &std::collections::HashMap<String, String>,
        last_acknowledged: Option<u64>,
    ) -> bool {
        self.attempt == provider_runtime::AttemptId(101)
            && self.phase == "a"
            && pages.get("a").is_some_and(|url| *url == self.url)
            && last_acknowledged == Some(1)
            && recipe_auth_action(queued, pages, 2)
    }
    fn begin(&mut self, now: Instant) -> bool {
        if self.stage == CookieGateStage::Armed {
            self.deadline = (now + Duration::from_secs(2)).min(self.deadline);
            self.stage = CookieGateStage::Sampling;
            true
        } else {
            false
        }
    }
    fn advance(
        &mut self,
        current: bool,
        now: Instant,
        observed: Option<minicon_web::NativeCookieMetadataSummary>,
    ) -> bool {
        use minicon_web::{
            NativeCookieMetadataStatus as Status, NativeCookieMetadataSummary as Summary,
        };
        if matches!(
            self.stage,
            CookieGateStage::Released | CookieGateStage::Discarded
        ) {
            return false;
        }
        if !current {
            self.stage = CookieGateStage::Discarded;
            self.result = Some(Summary::unavailable(Status::Cancelled));
            self.query = None;
            return false;
        }
        let observed = if now >= self.deadline {
            Some(Summary::unavailable(Status::TimedOut))
        } else {
            observed
        };
        if self.stage == CookieGateStage::Sampling
            && let Some(summary) = observed
        {
            self.result = Some(summary);
            self.stage = CookieGateStage::Released;
            self.query = None;
            return true;
        }
        false
    }
    fn target_ready(
        &self,
        attempt: provider_runtime::AttemptId,
        context: &browser::PageContext,
        url: &str,
    ) -> bool {
        self.stage == CookieGateStage::Released && self.matches(Some(attempt), Some(context), url)
    }
    fn take_public_record(&mut self) -> Option<serde_json::Value> {
        if self.reported {
            return None;
        }
        let summary = self.result?;
        self.reported = true;
        Some(
            serde_json::json!({"phase":self.phase,"stage":self.measurement_stage,"status":summary.status.as_str(),
            "counts":summary.counts.map(|counts| serde_json::json!({"total":counts.total,"session":counts.session,"persistent":counts.persistent,"expiry_future":counts.expiry_future,"expiry_elapsed":counts.expiry_elapsed,"expiry_unknown":counts.expiry_unknown}))}),
        )
    }
}

#[cfg(windows)]
struct BeforeReleaseCookieProbe {
    queued: Option<Action>,
    probe: PreNavigationCookieProbe,
    early: Option<minicon_web::NativeCookieMetadataSummary>,
}

#[cfg(windows)]
impl BeforeReleaseCookieProbe {
    fn install(slot: &mut Option<Self>, value: Self) -> bool {
        if slot.is_some() {
            false
        } else {
            *slot = Some(value);
            true
        }
    }
    fn authorized(
        &self,
        current: bool,
        pages: &std::collections::HashMap<String, String>,
        last_acknowledged: Option<u64>,
    ) -> bool {
        current
            && self.queued.as_ref().is_some_and(|action| {
                self.probe
                    .before_release_authorized(action, pages, last_acknowledged)
            })
    }
    fn finish(
        &mut self,
        current: bool,
        now: Instant,
        observed: Option<minicon_web::NativeCookieMetadataSummary>,
    ) -> Option<Action> {
        if self.probe.advance(current, now, observed) {
            self.queued.take()
        } else {
            None
        }
    }
}

#[cfg(windows)]
impl ConApp {
    /// Called only by the opt-in driver hook immediately before load_url.
    pub(super) fn defer_native_court_cookie_navigation(
        &mut self,
        window: &PixelWindow,
        url: &str,
    ) -> bool {
        let Some(mut probe) = self.native_auth_cookie_probe.take() else {
            return false;
        };
        let current = probe.current(self);
        // Ordinary restore/startup can run while open_harness_auth installs
        // the armed target. Never intercept that unrelated navigation.
        if !current || probe.url != url {
            if probe.stage == CookieGateStage::Sampling || current {
                probe.advance(false, Instant::now(), None);
            }
            self.native_auth_cookie_probe = Some(probe);
            return false;
        }
        if self.native_browser_page.is_none() {
            // Preserve the original failed-create path: diagnostic unavailability
            // must not cause a second host creation/navigation attempt.
            probe.begin(Instant::now());
            probe.advance(
                true,
                Instant::now(),
                Some(minicon_web::NativeCookieMetadataSummary::unavailable(
                    minicon_web::NativeCookieMetadataStatus::ContextUnavailable,
                )),
            );
            self.native_auth_cookie_probe = Some(probe);
            return false;
        }
        let deferred = if matches!(
            probe.stage,
            CookieGateStage::Released | CookieGateStage::Discarded
        ) {
            false
        } else {
            if probe.begin(Instant::now())
                && let Some(page) = self.native_browser_page.as_ref()
            {
                let waker = window.waker();
                probe.query = Some(page.cookie_metadata_for_diagnostics(
                    &probe.context.profile,
                    url,
                    probe.attempt.0,
                    probe.deadline,
                    move || {
                        let _ = waker.wake();
                    },
                ));
            }
            true
        };
        self.native_auth_cookie_probe = Some(probe);
        deferred
    }
}

#[cfg(windows)]
impl CourtApplication {
    fn resume_cookie_navigation(&mut self, window: &PixelWindow) {
        let Some(mut probe) = self.app.native_auth_cookie_probe.take() else {
            return;
        };
        let current = probe.current(&self.app)
            && probe.matches_action(self.pending.as_ref(), &self.recipe.pages);
        let now = Instant::now();
        let observed = if current {
            probe.query.as_mut().and_then(|query| query.poll(now))
        } else {
            None
        };
        let release = probe.advance(current, now, observed);
        if let Some(record) = probe.take_public_record()
            && let Ok(mut records) = self.cookie_observations.lock()
            && records.len() < 4
        {
            records.push(record);
        }
        let url = probe.url.clone();
        self.app.native_auth_cookie_probe = Some(probe);
        if release {
            self.app.launch_page_host(window, &url);
        }
    }
    fn queue_before_context_release(
        &mut self,
        window: &PixelWindow,
        queued: Action,
    ) -> Result<(), PixelWindowError> {
        let url = self.recipe.pages.get("a").unwrap();
        let mut probe = PreNavigationCookieProbe::arm(
            provider_runtime::AttemptId(101),
            self.recipe.a.clone(),
            url,
            "a",
            self.deadline,
        );
        if !probe.current(&self.app)
            || !probe.before_release_authorized(&queued, &self.recipe.pages, self.last_acknowledged)
        {
            return Err(self.fail("native-court-before-release-authority-invalid"));
        }
        probe.measurement_stage = "before-context-release";
        probe.begin(Instant::now());
        let early = if let Some(page) = self.app.native_browser_page.as_ref() {
            let waker = window.waker();
            probe.query = Some(page.cookie_metadata_for_diagnostics(
                &probe.context.profile,
                &probe.url,
                probe.attempt.0,
                probe.deadline,
                move || {
                    let _ = waker.wake();
                },
            ));
            None
        } else {
            Some(minicon_web::NativeCookieMetadataSummary::unavailable(
                minicon_web::NativeCookieMetadataStatus::ContextUnavailable,
            ))
        };
        if !BeforeReleaseCookieProbe::install(
            &mut self.before_release,
            BeforeReleaseCookieProbe {
                queued: Some(queued),
                probe,
                early,
            },
        ) {
            return Err(self.fail("native-court-overlapping-actions"));
        }
        Ok(())
    }
    fn resume_before_context_release(
        &mut self,
        window: &PixelWindow,
    ) -> Result<(), PixelWindowError> {
        let Some(mut queued) = self.before_release.take() else {
            return Ok(());
        };
        let current = queued.authorized(
            queued.probe.current(&self.app) && self.pending.is_none(),
            &self.recipe.pages,
            self.last_acknowledged,
        );
        let now = Instant::now();
        let observed = if current {
            queued.early.take().or_else(|| {
                queued
                    .probe
                    .query
                    .as_mut()
                    .and_then(|query| query.poll(now))
            })
        } else {
            None
        };
        let dispatch = queued.finish(current, now, observed);
        if let Some(record) = queued.probe.take_public_record()
            && let Ok(mut records) = self.cookie_observations.lock()
            && records.len() < 4
        {
            records.push(record);
        }
        if let Some(action) = dispatch {
            // Query refs were dropped by advance before dispatch closes A.
            // The single owned Action is consumed once, with no wrapper retry.
            self.dispatch_action(window, action)?;
        } else if queued.probe.stage == CookieGateStage::Discarded && !self.app.exit {
            return Err(self.fail("native-court-before-release-action-superseded"));
        } else if queued.probe.stage != CookieGateStage::Discarded {
            self.before_release = Some(queued);
        }
        Ok(())
    }
    fn cookie_target_ready(&self, action: &Action, loaded_url: &str) -> bool {
        if action.kind != "present-auth" {
            return true;
        }
        let Some(auth) = self.app.browser_auth.as_ref() else {
            return false;
        };
        action.url.as_deref() == Some(loaded_url)
            && self
                .app
                .native_auth_cookie_probe
                .as_ref()
                .is_some_and(|probe| {
                    probe.current(&self.app)
                        && probe.matches_action(Some(action), &self.recipe.pages)
                        && probe.target_ready(auth.attempt, &auth.context, loaded_url)
                })
    }
}

#[cfg(all(test, windows))]
mod cookie_navigation_tests {
    use super::*;
    use minicon_web::{
        NativeCookieMetadataStatus as Status, NativeCookieMetadataSummary as Summary,
    };
    fn context(label: &str) -> browser::PageContext {
        browser::PageContext {
            profile: minicon_web::PageProfile {
                data_directory: PathBuf::from(label),
                store_identifier: [1; 16],
            },
            proxy: Default::default(),
        }
    }
    fn arm(now: Instant) -> PreNavigationCookieProbe {
        PreNavigationCookieProbe::arm(
            provider_runtime::AttemptId(103),
            context("synthetic-a"),
            "http://127.0.0.1:12345/page/run/a-reopen/ticket",
            "a-reopen",
            now + Duration::from_secs(120),
        )
    }
    #[test]
    fn cookie_gate_waits_for_metadata_and_releases_the_original_target_once() {
        let now = Instant::now();
        let mut probe = arm(now);
        let ctx = context("synthetic-a");
        let url = probe.url.clone();
        assert!(probe.matches(Some(provider_runtime::AttemptId(103)), Some(&ctx), &url));
        assert!(probe.begin(now));
        assert!(!probe.begin(now));
        assert!(!probe.advance(true, now, None));
        assert!(!probe.target_ready(provider_runtime::AttemptId(103), &ctx, "about:blank"));
        assert!(!probe.target_ready(provider_runtime::AttemptId(103), &ctx, &url));
        assert!(probe.advance(true, now, Some(Summary::unavailable(Status::CallFailed))));
        assert!(probe.target_ready(provider_runtime::AttemptId(103), &ctx, &url));
        assert!(!probe.target_ready(provider_runtime::AttemptId(103), &ctx, "about:blank"));
        assert!(!probe.target_ready(provider_runtime::AttemptId(104), &ctx, &url));
        assert!(!probe.target_ready(
            provider_runtime::AttemptId(103),
            &context("synthetic-b"),
            &url
        ));
        assert!(!probe.advance(true, now, Some(Summary::unavailable(Status::Observed))));
        let record = probe.take_public_record().unwrap();
        assert_eq!(record["status"], "call-failed");
        assert_eq!(probe.stage, CookieGateStage::Released);
        assert!(probe.take_public_record().is_none());
        assert!(!record.to_string().contains(&url));
    }
    fn pages() -> std::collections::HashMap<String, String> {
        ["a", "b", "a-reopen"]
            .into_iter()
            .map(|phase| {
                (
                    phase.to_owned(),
                    format!("http://127.0.0.1:12345/page/run/{phase}/ticket"),
                )
            })
            .collect()
    }
    fn action(seq: u64, phase: &str, pages: &std::collections::HashMap<String, String>) -> Action {
        Action {
            seq,
            kind: "present-auth".into(),
            phase: phase.into(),
            url: Some(pages[phase].clone()),
        }
    }
    #[test]
    fn cookie_gate_revalidates_the_independent_pending_action() {
        let now = Instant::now();
        let pages = pages();
        let original = action(3, "a-reopen", &pages);
        let mut changed = Vec::new();
        let mut item = original.clone();
        item.seq = 2;
        changed.push(item);
        let mut item = original.clone();
        item.phase = "b".into();
        changed.push(item);
        let mut item = original.clone();
        item.kind = "ordinary".into();
        changed.push(item);
        let mut item = original.clone();
        item.url = Some(item.url.unwrap().replace(":12345/", ":12346/"));
        changed.push(item);
        for pending in changed {
            let mut probe = arm(now);
            let url = probe.url.clone();
            // Captured attempt/context/URI remain equal: only the independent
            // live pending Action changed. It must still revoke release.
            assert!(probe.matches(
                Some(provider_runtime::AttemptId(103)),
                Some(&context("synthetic-a")),
                &url
            ));
            assert!(!probe.matches_action(Some(&pending), &pages));
            probe.begin(now);
            assert!(!probe.advance(
                probe.matches_action(Some(&pending), &pages),
                now,
                Some(Summary::unavailable(Status::CallFailed))
            ));
            assert_eq!(probe.stage, CookieGateStage::Discarded);
        }
        let probe = arm(now);
        assert!(probe.matches_action(Some(&original), &pages));
        assert!(!probe.matches_action(None, &pages));
        let mut changed_recipe = pages.clone();
        changed_recipe.insert(
            "a-reopen".into(),
            "http://127.0.0.1:12345/page/other/a-reopen/ticket".into(),
        );
        assert!(!probe.matches_action(Some(&original), &changed_recipe));
    }
    #[test]
    fn before_release_consumes_one_authorized_b_action_and_discards_late_work() {
        let now = Instant::now();
        let pages = pages();
        let original = action(2, "b", &pages);
        let make = || {
            let mut probe = PreNavigationCookieProbe::arm(
                provider_runtime::AttemptId(101),
                context("synthetic-a"),
                &pages["a"],
                "a",
                now + Duration::from_secs(120),
            );
            probe.measurement_stage = "before-context-release";
            probe.begin(now);
            BeforeReleaseCookieProbe {
                queued: Some(original.clone()),
                probe,
                early: None,
            }
        };
        let mut slot = None;
        assert!(BeforeReleaseCookieProbe::install(&mut slot, make()));
        let mut second = make();
        second.queued.as_mut().unwrap().url = Some(pages["a"].clone());
        assert!(!BeforeReleaseCookieProbe::install(&mut slot, second));
        assert_eq!(
            slot.as_ref()
                .unwrap()
                .queued
                .as_ref()
                .unwrap()
                .url
                .as_deref(),
            Some(pages["b"].as_str())
        );
        drop(slot);
        let mut queued = make();
        assert!(queued.authorized(true, &pages, Some(1)));
        assert!(!queued.authorized(true, &pages, None));
        assert!(!queued.authorized(false, &pages, Some(1)));
        assert!(queued.finish(true, now, None).is_none());
        assert!(queued.queued.is_some());
        let next = queued
            .finish(true, now, Some(Summary::unavailable(Status::CallFailed)))
            .unwrap();
        assert_eq!(next.seq, 2);
        assert_eq!(next.url.as_deref(), Some(pages["b"].as_str()));
        assert!(queued.queued.is_none());
        assert!(
            queued
                .finish(true, now, Some(Summary::unavailable(Status::Observed)))
                .is_none()
        );
        let record = queued.probe.take_public_record().unwrap();
        assert_eq!(record["phase"], "a");
        assert_eq!(record["stage"], "before-context-release");
        assert_eq!(record["status"], "call-failed");
        assert_eq!(record.as_object().unwrap().len(), 4);
        let mut queued = make();
        queued.queued.as_mut().unwrap().url = Some(pages["a"].clone());
        let allowed = queued.authorized(true, &pages, Some(1));
        assert!(!allowed);
        assert!(queued.finish(allowed, now, None).is_none());
        assert_eq!(queued.probe.stage, CookieGateStage::Discarded);
        assert!(
            queued
                .finish(true, now, Some(Summary::unavailable(Status::Observed)))
                .is_none()
        );
        let mut queued = make();
        let allowed = queued.authorized(false, &pages, Some(1));
        assert!(queued.finish(allowed, now, None).is_none());
        assert!(
            queued
                .finish(true, now, Some(Summary::unavailable(Status::Observed)))
                .is_none()
        );
        let mut queued = make();
        assert!(
            queued
                .finish(true, now + Duration::from_secs(2), None)
                .is_some()
        );
        assert_eq!(
            queued.probe.result.map(|r| r.status),
            Some(Status::TimedOut)
        );
    }

    #[test]
    fn cookie_gate_refuses_changed_authority_timeout_and_late_completion() {
        let now = Instant::now();
        let mut probe = arm(now);
        probe.begin(now);
        assert_eq!(probe.deadline, now + Duration::from_secs(2));
        assert!(!probe.matches(
            Some(provider_runtime::AttemptId(104)),
            Some(&context("synthetic-a")),
            &probe.url
        ));
        assert!(!probe.matches(
            Some(provider_runtime::AttemptId(103)),
            Some(&context("synthetic-b")),
            &probe.url
        ));
        assert!(!probe.matches(
            Some(provider_runtime::AttemptId(103)),
            Some(&context("synthetic-a")),
            "http://127.0.0.1:12346/page/run/a-reopen/ticket"
        ));
        assert!(!probe.advance(false, now, None));
        assert_eq!(
            probe.result.map(|result| result.status),
            Some(Status::Cancelled)
        );
        assert!(!probe.advance(true, now, Some(Summary::unavailable(Status::CallFailed))));
        let mut probe = arm(now);
        probe.begin(now);
        assert!(probe.advance(true, now + Duration::from_secs(2), None));
        assert_eq!(
            probe.result.map(|result| result.status),
            Some(Status::TimedOut)
        );
        assert!(!probe.advance(
            true,
            now + Duration::from_secs(3),
            Some(Summary::unavailable(Status::Observed))
        ));
        let mut probe = PreNavigationCookieProbe::arm(
            provider_runtime::AttemptId(103),
            context("synthetic-a"),
            "http://127.0.0.1:12345/page/run/a/ticket",
            "a",
            now + Duration::from_millis(1),
        );
        probe.begin(now);
        assert_eq!(probe.deadline, now + Duration::from_millis(1));
    }
}

struct CourtApplication {
    app: ConApp,
    recipe: Recipe,
    worker: Option<Worker>,
    pending: Option<Action>,
    #[cfg(windows)]
    before_release: Option<BeforeReleaseCookieProbe>,
    #[cfg(windows)]
    last_acknowledged: Option<u64>,
    browser_id: Option<workspace::TabId>,
    deadline: Instant,
    outcome: Arc<Mutex<Option<Result<(), &'static str>>>>,
    fixture_receipt: Arc<Mutex<Option<serde_json::Value>>>,
    #[cfg(any(windows, target_os = "linux"))]
    diagnostics: Arc<Mutex<Vec<serde_json::Value>>>,
    #[cfg(windows)]
    private_locators: Arc<Mutex<std::collections::BTreeMap<String, serde_json::Value>>>,
    #[cfg(windows)]
    cookie_observations: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl CourtApplication {
    #[cfg(target_os = "linux")]
    fn record_context_observation(&self) {
        let (phase, release) = match &self.app.browser_auth {
            Some(auth) => {
                let phase = match auth.attempt.0 {
                    101 => "a",
                    102 => "b",
                    103 => "a-reopen",
                    _ => return,
                };
                (
                    phase,
                    self.app
                        .auth_context_lifecycle
                        .release_observation(&auth.context.profile),
                )
            }
            None => ("no-auth-presentation", "not-applicable"),
        };
        let observation = serde_json::json!({
            "phase":phase,
            "native_host_present":self.app.browser_page.is_some(),
            "owned_profile_release":release,
            "releases":self.app.auth_context_lifecycle.release_diagnostics(),
        });
        if let Ok(mut records) = self.diagnostics.lock()
            && records.last() != Some(&observation)
            && records.len() < 64
        {
            // Retain finite stage evidence even if a stuck native teardown
            // prevents the final driver receipt from being written.
            eprintln!("linux-native-release-observation: {observation}");
            records.push(observation);
        }
    }
    #[cfg(windows)]
    fn record_context_observation(&self) {
        let Some(auth) = &self.app.browser_auth else {
            return;
        };
        let phase = match auth.attempt.0 {
            101 => "a",
            102 => "b",
            103 => "a-reopen",
            _ => "unrecognized-attempt",
        };
        if matches!(phase, "a" | "b" | "a-reopen")
            && let Some(page) = self.app.native_browser_page.as_ref()
            && let Ok(relative) =
                page.context_profile_relative_directory_for_diagnostics(&auth.context.profile)
            && let Some(relative) = relative.to_str()
            && let Ok(mut locators) = self.private_locators.lock()
        {
            locators.insert(
                phase.to_owned(),
                serde_json::json!({
                    "store_identifier": auth.context.profile.store_identifier,
                    "relative_directory": relative,
                }),
            );
        }
        let observation = serde_json::json!({
            "phase": phase,
            "native_host_present": self.app.native_browser_page.is_some(),
            "persistent_profile_verified": self.app.native_browser_page.as_ref()
                .map(|page| page.context_profile_matches(&auth.context.profile)),
            "persistent_profile_layout": self.app.native_browser_page.as_ref()
                .map(|page| page.context_profile_layout(&auth.context.profile))
                .unwrap_or("unavailable"),
            "owned_profile_release": self.app.auth_context_lifecycle.release_observation(&auth.context.profile),
        });
        if let Ok(mut observations) = self.diagnostics.lock()
            && observations.last() != Some(&observation)
        {
            if observations.len() == 16 {
                observations.remove(0);
            }
            observations.push(observation);
        }
    }

    fn fail(&self, code: &'static str) -> PixelWindowError {
        if let Ok(mut result) = self.outcome.lock() {
            *result = Some(Err(code));
        }
        PixelWindowError::failed("native_auth_court_failed", code)
    }

    fn action(&mut self, window: &PixelWindow, action: Action) -> Result<(), PixelWindowError> {
        #[cfg(windows)]
        {
            if self.before_release.is_some() {
                return Err(self.fail("native-court-overlapping-actions"));
            }
            if action.kind == "present-auth" && action.seq == 2 {
                if self.pending.is_some() {
                    return Err(self.fail("native-court-overlapping-actions"));
                }
                return self.queue_before_context_release(window, action);
            }
        }
        self.dispatch_action(window, action)
    }

    fn dispatch_action(
        &mut self,
        window: &PixelWindow,
        action: Action,
    ) -> Result<(), PixelWindowError> {
        if self.pending.is_some() {
            return Err(self.fail("native-court-overlapping-actions"));
        }
        match action.kind.as_str() {
            "ordinary" if action.seq == 0 && action.phase == "ordinary" => {
                let url = self.recipe.pages.get("ordinary").unwrap();
                if action.url.as_deref() != Some(url.as_str())
                    || !self.app.present_http_link(window, url)
                {
                    return Err(self.fail("native-court-ordinary-navigation-failed"));
                }
                self.browser_id = self.app.workspace.browser_tab();
            }
            "present-auth" if (1..=3).contains(&action.seq) => {
                let expected_phase = ["a", "b", "a-reopen"][(action.seq - 1) as usize];
                let url = self.recipe.pages.get(expected_phase).unwrap();
                if action.phase != expected_phase || action.url.as_deref() != Some(url.as_str()) {
                    return Err(self.fail("native-court-auth-action-invalid"));
                }
                let context = if action.phase == "b" {
                    self.recipe.b.clone()
                } else {
                    self.recipe.a.clone()
                };
                let attempt = provider_runtime::AttemptId(100 + action.seq);
                self.app.harness_settings = Some(
                    harness_settings_ui::HarnessSettings::for_native_auth_court(attempt),
                );
                #[cfg(windows)]
                {
                    self.app.native_auth_cookie_probe = Some(PreNavigationCookieProbe::arm(
                        attempt,
                        context.clone(),
                        url,
                        expected_phase,
                        self.deadline,
                    ));
                }
                self.app.open_harness_auth(window, attempt, context, url);
                if !self
                    .app
                    .browser_auth
                    .as_ref()
                    .is_some_and(|auth| auth.attempt == attempt)
                {
                    return Err(self.fail("native-court-auth-presentation-refused"));
                }
            }
            "close-auth" if action.seq == 4 && action.phase == "ordinary-return" => {
                let id = self
                    .browser_id
                    .ok_or_else(|| self.fail("native-court-browser-missing"))?;
                if self.app.browser_auth.is_none()
                    || self.app.detach_session(id)
                    || self.app.workspace.browser_tab() != Some(id)
                    || self.app.browser_auth.is_some()
                {
                    return Err(self.fail("native-court-first-close-did-not-restore"));
                }
                // Closing auth deliberately returns to Harness. Dismiss that
                // panel before observing the returned ordinary native page.
                self.app.settings_open = false;
                self.app.sync_embedded_page(window);
                self.app.mark_host_ui_full_and_repaint(window);
            }
            _ => return Err(self.fail("native-court-action-invalid")),
        }
        self.pending = Some(action);
        Ok(())
    }

    fn turn(&mut self, window: &PixelWindow) -> Result<(), PixelWindowError> {
        if Instant::now() >= self.deadline {
            return Err(self.fail("native-court-deadline-expired"));
        }
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.overflow.load(CourtOrdering::Acquire))
        {
            return Err(self.fail("native-court-notice-queue-overflow"));
        }
        loop {
            let notice = self
                .worker
                .as_ref()
                .and_then(|worker| worker.notices.try_recv().ok());
            match notice {
                Some(Notice::Action(action)) => self.action(window, action)?,
                Some(Notice::Failed(code)) => return Err(self.fail(code)),
                Some(Notice::Complete(receipt)) => {
                    #[cfg(windows)]
                    if self.before_release.is_some() {
                        return Err(self.fail("native-court-final-presentation-mismatch"));
                    }
                    if self.pending.is_some()
                        || self.app.browser_auth.is_some()
                        || self.app.workspace.browser_tab() != self.browser_id
                    {
                        return Err(self.fail("native-court-final-presentation-mismatch"));
                    }
                    let checks = [
                        "ordinary-initial-storage",
                        "a-initial-storage",
                        "b-initial-storage",
                        "a-reopen-initial-storage",
                        "ordinary-live-dom-cookie-storage-return",
                        "ordinary-one-document-load",
                        "ordinary-native-history-back",
                        "ordinary-native-history-forward",
                    ];
                    if receipt["observation_status"] != "complete"
                        || !checks.iter().all(|name| receipt["checks"][*name] == true)
                    {
                        return Err(self.fail("native-court-fixture-receipt-incomplete"));
                    }
                    if let Ok(mut value) = self.fixture_receipt.lock() {
                        *value = Some(receipt);
                    }
                    if let Ok(mut result) = self.outcome.lock() {
                        *result = Some(Ok(()));
                    }
                    self.app.exit = true;
                }
                None => break,
            }
        }
        #[cfg(windows)]
        self.resume_before_context_release(window)?;
        #[cfg(windows)]
        self.resume_cookie_navigation(window);
        if self.pending.is_some() {
            let ready = self
                .app
                .workspace
                .browser_tab()
                .and_then(|id| self.app.browsers.get(&id))
                .is_some_and(|tab| {
                    #[cfg(windows)]
                    {
                        matches!(&tab.panel().load, browser::PageLoad::Ready { url, .. }
                            if self.cookie_target_ready(self.pending.as_ref().unwrap(), url))
                    }
                    #[cfg(not(windows))]
                    {
                        matches!(tab.panel().load, browser::PageLoad::Ready { .. })
                    }
                });
            #[cfg(target_os = "linux")]
            let ready = ready
                && self.app.content_panel_slot(window).is_some_and(|slot| {
                    self.app.browser_page.as_ref().is_some_and(|child| {
                        self.app
                            .browser_process
                            .as_ref()
                            .is_some_and(|process| process.embedded_ready(slot.parent, child.id()))
                    })
                });
            if ready {
                let seq = self.pending.take().unwrap().seq;
                if self
                    .worker
                    .as_ref()
                    .unwrap()
                    .acknowledgements
                    .try_send(seq)
                    .is_err()
                {
                    return Err(self.fail("native-court-ack-queue-full"));
                }
                #[cfg(windows)]
                {
                    self.last_acknowledged = Some(seq);
                }
            }
        }
        Ok(())
    }
}

impl PixelWindowApplication for CourtApplication {
    fn opened(&mut self, window: &PixelWindow) -> Result<PixelWindowDirective, PixelWindowError> {
        self.app.opened(window)?;
        let waker = window.waker();
        self.worker = Some(Worker::start(&self.recipe, move || {
            let _ = waker.wake();
        }));
        Ok(PixelWindowDirective::Continue)
    }

    fn event(
        &mut self,
        window: &PixelWindow,
        event: PixelWindowEvent,
    ) -> Result<PixelWindowDirective, PixelWindowError> {
        if matches!(event, PixelWindowEvent::CloseRequested)
            && self
                .outcome
                .lock()
                .ok()
                .is_some_and(|value| value.is_none())
        {
            return Err(self.fail("native-court-closed-before-observations"));
        }
        self.app.event(window, event)
    }

    fn render(
        &mut self,
        window: &PixelWindow,
        frame: &mut XrgbPixelFrame<'_>,
    ) -> Result<PixelWindowDirective, PixelWindowError> {
        self.app.render(window, frame)
    }

    fn about_to_wait(
        &mut self,
        window: &PixelWindow,
        now: Instant,
    ) -> Result<PixelWindowDirective, PixelWindowError> {
        let directive = self.app.about_to_wait(window, now)?;
        #[cfg(any(windows, target_os = "linux"))]
        self.record_context_observation();
        self.turn(window)?;
        if self.app.exit {
            return Ok(PixelWindowDirective::Exit);
        }
        let next = (now + Duration::from_millis(100)).min(self.deadline);
        Ok(match directive {
            PixelWindowDirective::Exit => PixelWindowDirective::Exit,
            PixelWindowDirective::Continue => PixelWindowDirective::Continue,
            PixelWindowDirective::WaitUntil(current) => {
                PixelWindowDirective::WaitUntil(current.min(next))
            }
            PixelWindowDirective::Wait => PixelWindowDirective::WaitUntil(next),
            _ => return Err(self.fail("native-court-window-directive-unsupported")),
        })
    }
}

impl Drop for CourtApplication {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            self.before_release = None;
            self.app.native_auth_cookie_probe = None;
        }
        self.worker = None;
        // At most one auth-close restoration, then one ordinary teardown.
        // Invoke the real teardown path rather than dropping live Child handles.
        for _ in 0..2 {
            if let Some(id) = self.app.workspace.browser_tab() {
                self.app.detach_session(id);
            }
        }
    }
}

/// Invoke from the executable's main thread, before ordinary product arg
/// handling. The default build must not compile or accept this entry point.
pub(super) fn run_application(manifest: &Path) -> Result<(), &'static str> {
    let recipe = Recipe::read(manifest)?;
    let output = recipe.output.clone();
    let profile_acl_mode = recipe.profile_acl_mode.as_str();
    #[cfg(windows)]
    let locator_run = recipe.run.clone();
    #[cfg(windows)]
    let private_locators = Arc::new(Mutex::new(std::collections::BTreeMap::new()));
    #[cfg(windows)]
    let cookie_observations = Arc::new(Mutex::new(Vec::new()));
    let deadline = Instant::now() + recipe.timeout;
    let outcome = Arc::new(Mutex::new(None));
    let receipt = Arc::new(Mutex::new(None));
    #[cfg(any(windows, target_os = "linux"))]
    let diagnostics = Arc::new(Mutex::new(Vec::new()));
    let mut app = ConApp::new(None, None);
    app.no_activate = true;
    let native = CourtApplication {
        app,
        recipe,
        worker: None,
        pending: None,
        #[cfg(windows)]
        before_release: None,
        #[cfg(windows)]
        last_acknowledged: None,
        browser_id: None,
        deadline,
        outcome: outcome.clone(),
        fixture_receipt: receipt.clone(),
        #[cfg(any(windows, target_os = "linux"))]
        diagnostics: diagnostics.clone(),
        #[cfg(windows)]
        private_locators: private_locators.clone(),
        #[cfg(windows)]
        cookie_observations: cookie_observations.clone(),
    };
    let result = run_pixel_window(
        PixelWindowOptions::new("MiniCon native auth court", LogicalSize::new(960.0, 600.0))
            .with_no_activate(true)
            .with_ime_allowed(true),
        Box::new(native),
    );
    // The window has returned. No filesystem publication is added to GUI
    // callbacks, and a failed optional diagnostic never changes the oracle.
    #[cfg(windows)]
    let private_locator_status = (|| {
        let locators = private_locators.lock().ok()?;
        if locators.len() > 3 {
            return None;
        }
        let bytes = serde_json::to_vec(&serde_json::json!({
            "schema": 1, "test_only": true, "run": locator_run, "phases": &*locators,
        }))
        .ok()?;
        if bytes.len() > 16_384 {
            return None;
        }
        minicon_platform::filesystem::write_private_atomic(
            &output.join("native-profile-locator.private.json"),
            &bytes,
        )
        .ok()?;
        Some(())
    })()
    .map_or("unavailable", |_| "written");
    let observed = outcome
        .lock()
        .map_err(|_| "native-court-outcome-lock")?
        .unwrap_or(Err("native-court-observations-not-complete"));
    let unsupported = matches!(&result, Err(PixelWindowError::Unsupported { .. }));
    let failure = if unsupported {
        Some("native-court-native-host-unsupported")
    } else {
        observed.err().or(if result.is_err() {
            Some("native-court-window-failed")
        } else {
            None
        })
    };
    let status = if unsupported {
        "BLOCKED"
    } else if result.is_ok() && observed.is_ok() {
        "native-observations-complete"
    } else {
        "failed"
    };
    let features: Vec<_> = [
        cfg!(feature = "default").then_some("default"),
        cfg!(feature = "experimental-rust-console").then_some("experimental-rust-console"),
        Some("native-auth-court"),
    ]
    .into_iter()
    .flatten()
    .collect();
    #[cfg(windows)]
    let diagnostic_features = vec![minicon_web::NativeCookieMetadataProbe::feature_name()];
    #[cfg(not(windows))]
    let diagnostic_features: Vec<&str> = Vec::new();
    let data = serde_json::json!({
        "kind":"native-auth-court-test-build", "status":status,
        "windows_profile_acl_mode":profile_acl_mode,
        "profile_acl_control_is_production_repair":false,
        "test_only":true, "production_test_build":true,
        "features":features,
        "diagnostic_features":diagnostic_features,
        "os":std::env::consts::OS, "isa":std::env::consts::ARCH,
        "failure_code":failure,
        "production_artifact_qualification":false, "provider_authorization_claimed":false,
        "fixture":receipt.lock().map_err(|_| "native-court-outcome-lock")?.take(),
        "unclaimed":["proxy/no-direct", "delayed embedded_ready", "native map/focus/pixels", "physical input/IME/clipboard", "toolbar-driven history"],
    });
    #[cfg(windows)]
    let data = {
        let mut data = data;
        data["private_profile_locator"] = serde_json::json!(private_locator_status);
        data["windows_cookie_metadata_observations"] = serde_json::json!(
            cookie_observations
                .lock()
                .ok()
                .map(|records| records.clone())
        );
        data["windows_context_observations"] = serde_json::json!(
            diagnostics
                .lock()
                .ok()
                .map(|observations| observations.clone())
        );
        data
    };
    #[cfg(target_os = "linux")]
    let data = {
        let mut data = data;
        data["linux_context_observations"] =
            serde_json::json!(diagnostics.lock().ok().map(|records| records.clone()));
        data
    };
    std::fs::write(
        output.join("native-driver-receipt.json"),
        serde_json::to_vec_pretty(&data).map_err(|_| "native-court-receipt-serialize")?,
    )
    .map_err(|_| "native-court-receipt-write")?;
    if let Some(failure) = failure {
        return Err(failure);
    }
    observed
}
