//! Selected applied-profile key UI. Secrets are borrowed only by the explicit sink.
use super::*;
use provider_runtime::{ClientKeyCopy, ProfileKeyOperation, RuntimeError};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FeedbackKind {
    Waiting,
    Created,
    Revoked,
    Copied,
    ClipboardFailed,
    NotCopied,
    Busy,
    Refused,
    ApplyFirst,
    CreateFirst,
    Disabled,
    RecipeCopied,
    RecipeFailed,
}
impl FeedbackKind {
    fn text(self) -> &'static str {
        match self {
            Self::Waiting => "Key operation pending",
            Self::Created => "Key created; not copied",
            Self::Revoked => "Key revoked",
            Self::Copied => "Key copied",
            Self::ClipboardFailed => "Copy failed: clipboard",
            Self::NotCopied => "Not copied: intent changed",
            Self::Busy => "Busy; click again later",
            Self::Refused => "Key operation refused",
            Self::ApplyFirst => "Apply profile first",
            Self::CreateFirst => "Create profile key first",
            Self::Disabled => "Apply enabled profile first",
            Self::RecipeCopied => "Setup copied; no launch",
            Self::RecipeFailed => "Setup copy failed",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Created => "created",
            Self::Revoked => "revoked",
            Self::Copied => "copied",
            Self::ClipboardFailed => "clipboard_failed",
            Self::NotCopied => "not_copied",
            Self::Busy => "busy",
            Self::Refused => "refused",
            Self::ApplyFirst => "apply_first",
            Self::CreateFirst => "create_first",
            Self::Disabled => "disabled",
            Self::RecipeCopied => "recipe_copied",
            Self::RecipeFailed => "recipe_failed",
        }
    }
}
struct Feedback {
    profile: String,
    request: Option<u64>,
    kind: FeedbackKind,
    intent: u64,
}
struct CopyIntent {
    profile: String,
    request: u64,
    selection: u64,
    intent: u64,
}
struct PendingKey {
    request: u64,
    operation: ProfileKeyOperation,
    selection: u64,
    intent: u64,
}
#[derive(Default)]
pub(super) struct KeyUi {
    selected: Option<String>,
    selection: u64,
    copy: Option<CopyIntent>,
    intent: u64,
    pending: BTreeMap<String, PendingKey>,
    seen: BTreeMap<String, u64>,
    feedback: Option<Feedback>,
}
impl KeyUi {
    fn report(&mut self, profile: &str, request: Option<u64>, kind: FeedbackKind) {
        self.feedback = Some(Feedback {
            profile: profile.into(),
            request,
            kind,
            intent: self.intent,
        });
    }
    pub(super) fn invalidate_copy(&mut self) {
        if let Some(intent) = self.copy.take() {
            if self.intent == intent.intent
                && self.feedback.as_ref().is_some_and(|feedback| {
                    feedback.intent == intent.intent
                        && feedback.profile == intent.profile
                        && feedback.request == Some(intent.request)
                })
            {
                self.report(
                    &intent.profile,
                    Some(intent.request),
                    FeedbackKind::NotCopied,
                );
            }
            // A late preparation receipt cannot restore Waiting after blur.
            self.intent = self.intent.saturating_add(1);
        }
    }
    fn select(&mut self, selected: Option<&str>) {
        if self.selected.as_deref() != selected {
            self.invalidate_copy();
            self.selection = self.selection.saturating_add(1);
            self.intent = self.intent.saturating_add(1);
            self.selected = selected.map(str::to_owned);
        }
    }
    fn begin_intent(&mut self) {
        self.invalidate_copy();
        self.intent = self.intent.saturating_add(1);
    }
    fn observe(&mut self, view: &View) {
        self.seen
            .retain(|profile, _| view.profile_keys.iter().any(|row| &row.profile == profile));
        self.pending
            .retain(|profile, _| view.profile_keys.iter().any(|row| &row.profile == profile));
        for receipt in &view.profile_key_receipts {
            if self.seen.get(&receipt.profile) == Some(&receipt.request) {
                continue;
            }
            if self
                .pending
                .get(&receipt.profile)
                .map(|entry| entry.request)
                != Some(receipt.request)
            {
                continue;
            }
            let pending = self
                .pending
                .remove(&receipt.profile)
                .expect("matched pending key");
            self.seen.insert(receipt.profile.clone(), receipt.request);
            let kind = match receipt.result {
                Err(RuntimeError::Busy) => FeedbackKind::Busy,
                Err(RuntimeError::Cancelled) => FeedbackKind::NotCopied,
                Err(_) => FeedbackKind::Refused,
                Ok(()) => match receipt.operation {
                    ProfileKeyOperation::Create => FeedbackKind::Created,
                    ProfileKeyOperation::Revoke => FeedbackKind::Revoked,
                    ProfileKeyOperation::Copy => FeedbackKind::Waiting,
                },
            };
            // Success for Copy is only preparation. The real sink below owns
            // Copied/ClipboardFailed; repeated View delivery cannot overwrite it.
            if pending.selection == self.selection
                && pending.intent == self.intent
                && self.selected.as_deref() == Some(receipt.profile.as_str())
                && self.feedback.as_ref().is_some_and(|feedback| {
                    feedback.intent == pending.intent
                        && feedback.request == Some(receipt.request)
                        && feedback.profile == receipt.profile
                })
            {
                self.report(&receipt.profile, Some(receipt.request), kind);
            }
            if receipt.result.is_err()
                && self.copy.as_ref().is_some_and(|intent| {
                    intent.request == receipt.request && intent.profile == receipt.profile
                })
            {
                self.copy = None;
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum RecipeKind {
    Worker,
    Pi,
}

fn short_profile(profile: &str) -> &str {
    if profile == "default" {
        profile
    } else {
        profile.get(..8).unwrap_or(profile)
    }
}

fn shell_literal(value: &str, windows_shell: bool) -> String {
    if windows_shell {
        format!("'{}'", value.replace('\'', "''"))
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

impl HarnessSettings {
    fn selected_key_profile(&self) -> Option<&str> {
        self.forms.selected.as_deref().or_else(|| {
            self.view
                .profile_keys
                .iter()
                .any(|row| row.profile == "default")
                .then_some("default")
        })
    }
    fn key_target_applied(&self, profile: &str) -> bool {
        self.view
            .profile_keys
            .iter()
            .any(|row| row.profile == profile)
            && (profile == "default"
                || self
                    .view
                    .connection_profiles
                    .iter()
                    .any(|row| row.id == profile && row.applied.is_some()))
    }
    pub(super) fn sync_key_ui(&mut self) {
        let selected = self.selected_key_profile().map(str::to_owned);
        self.keys.select(selected.as_deref());
        self.keys.observe(&self.view);
    }
    pub(super) fn key_action(&mut self, operation: ProfileKeyOperation) {
        self.sync_key_ui();
        self.keys.begin_intent();
        let profile = self.selected_key_profile().unwrap_or("default").to_owned();
        if !self.key_target_applied(&profile) {
            self.keys.report(&profile, None, FeedbackKind::ApplyFirst);
            return;
        }
        let request = match self
            .runtime
            .as_ref()
            .ok_or(RuntimeError::WorkerUnavailable)
            .and_then(|runtime| runtime.next_profile_key_request())
        {
            Ok(request) => request,
            Err(_) => {
                self.keys.report(&profile, None, FeedbackKind::Refused);
                return;
            }
        };
        let result = self
            .runtime
            .as_mut()
            .ok_or(RuntimeError::WorkerUnavailable)
            .and_then(|runtime| {
                // Preserve the canonical setup entry points; they translate
                // into the same targeted request sequence inside Runtime.
                let action = if profile == "default" {
                    match operation {
                        ProfileKeyOperation::Create => Action::CreateClientKey,
                        ProfileKeyOperation::Copy => Action::CopyClientKey,
                        ProfileKeyOperation::Revoke => Action::RevokeClientKey,
                    }
                } else {
                    Action::ProfileKey {
                        request,
                        profile: profile.clone(),
                        operation,
                    }
                };
                runtime.act(action)
            });
        match result {
            Ok(()) => {
                self.keys.pending.insert(
                    profile.clone(),
                    PendingKey {
                        request,
                        operation,
                        selection: self.keys.selection,
                        intent: self.keys.intent,
                    },
                );
                self.keys
                    .report(&profile, Some(request), FeedbackKind::Waiting);
                if operation == ProfileKeyOperation::Copy {
                    self.keys.copy = Some(CopyIntent {
                        profile,
                        request,
                        selection: self.keys.selection,
                        intent: self.keys.intent,
                    });
                }
            }
            Err(error) => self.keys.report(
                &profile,
                Some(request),
                if error == RuntimeError::Busy {
                    FeedbackKind::Busy
                } else {
                    FeedbackKind::Refused
                },
            ),
        }
    }
    pub(super) fn refuse_unreadable_key(&mut self) {
        self.keys.begin_intent();
        let profile = self.selected_key_profile().unwrap_or("default").to_owned();
        self.keys.report(&profile, None, FeedbackKind::Refused);
        self.error =
            Some("Enlarge the window to read the complete key action before clicking.".into());
    }
    pub(super) fn consume_key_copy(
        &mut self,
        copy: ClientKeyCopy,
        sink: impl FnOnce(&str) -> bool,
    ) {
        self.sync_key_ui();
        // An old A effect must never consume the newer B intent or its feedback.
        if !self
            .keys
            .copy
            .as_ref()
            .is_some_and(|intent| intent.profile == copy.profile && intent.request == copy.request)
        {
            return;
        }
        let intent = self.keys.copy.take();
        let accepted = intent.is_some_and(|intent| {
            intent.profile == copy.profile
                && intent.request == copy.request
                && intent.selection == self.keys.selection
                && intent.intent == self.keys.intent
                && self.selected_key_profile() == Some(copy.profile.as_str())
        }) && self.key_target_applied(&copy.profile)
            && self.view.profile_keys.iter().any(|row| {
                row.profile == copy.profile && row.exists && row.epoch == copy.key_epoch
            });
        if !accepted {
            self.keys
                .report(&copy.profile, Some(copy.request), FeedbackKind::NotCopied);
            return;
        }
        let copied = sink(copy.expose());
        self.keys.report(
            &copy.profile,
            Some(copy.request),
            if copied {
                FeedbackKind::Copied
            } else {
                FeedbackKind::ClipboardFailed
            },
        );
    }
    pub(super) fn key_state_label(&self) -> String {
        let Some(profile) = self.selected_key_profile() else {
            return FeedbackKind::ApplyFirst.text().into();
        };
        if !self.key_target_applied(profile) {
            return FeedbackKind::ApplyFirst.text().into();
        }
        self.view
            .profile_keys
            .iter()
            .find(|row| row.profile == profile)
            .map_or_else(
                || "Key state unavailable".into(),
                |row| format!("[{}] Key: {:?}", short_profile(profile), row.state),
            )
    }
    pub(super) fn selected_key_durability_unconfirmed(&self) -> bool {
        self.selected_key_profile()
            .and_then(|profile| {
                self.view
                    .profile_keys
                    .iter()
                    .find(|row| row.profile == profile)
            })
            .is_some_and(|row| row.durability_unconfirmed)
    }
    pub(super) fn key_feedback_label(&self) -> String {
        self.keys.feedback.as_ref().map_or_else(
            || "Copy key is explicit; templates contain no key.".into(),
            |feedback| {
                format!(
                    "[{}] {}",
                    short_profile(&feedback.profile),
                    feedback.kind.text()
                )
            },
        )
    }
    pub(super) fn key_applied_label(&self) -> String {
        self.selected_applied_connection().map_or_else(
            || FeedbackKind::ApplyFirst.text().into(),
            |(_, applied)| {
                applied
                    .allocations
                    .iter()
                    .find(|row| row.alias == applied.default_alias)
                    .map_or_else(
                        || "Applied default alias unavailable".into(),
                        |row| format!("Applied alias: {} / model: {}", row.alias, row.model),
                    )
            },
        )
    }
    fn selected_applied_connection(
        &self,
    ) -> Option<(&str, &provider_runtime::ConnectionDraftInput)> {
        let selected = self.selected_key_profile()?;
        self.view
            .connection_profiles
            .iter()
            .find(|row| row.id == selected)
            .and_then(|row| row.applied.as_ref())
            .map(|applied| (selected, applied))
    }
    pub(super) fn endpoint_label(&self, openai: bool) -> String {
        let running =
            self.view.listener == ListenerState::Listening && self.view.actual_port.is_some();
        let port = if running {
            self.view.actual_port.unwrap_or(self.view.port)
        } else {
            self.view.port
        };
        format!(
            "{} {}: http://127.0.0.1:{port}{}",
            if running {
                "Running"
            } else {
                "Stopped/configured"
            },
            if openai { "Chat" } else { "Messages" },
            if openai { "/v1" } else { "" }
        )
    }
    fn connection_template(
        &self,
        kind: RecipeKind,
        windows_shell: bool,
    ) -> Result<String, FeedbackKind> {
        let (profile, applied) = self
            .selected_applied_connection()
            .ok_or(FeedbackKind::ApplyFirst)?;
        if !applied.enabled {
            return Err(FeedbackKind::Disabled);
        }
        if !self
            .view
            .profile_keys
            .iter()
            .any(|row| row.profile == profile && row.exists)
        {
            return Err(FeedbackKind::CreateFirst);
        }
        let allocation = applied
            .allocations
            .iter()
            .find(|row| {
                row.alias == applied.default_alias && row.account.is_some() && !row.model.is_empty()
            })
            .ok_or(FeedbackKind::ApplyFirst)?;
        let running =
            self.view.listener == ListenerState::Listening && self.view.actual_port.is_some();
        let port = if running {
            self.view.actual_port.unwrap_or(self.view.port)
        } else {
            self.view.port
        };
        let state = if running {
            "Listener running at its actual bound port."
        } else {
            "Listener stopped. Endpoint is configured only; explicitly Start before use."
        };
        let setup = if windows_shell {
            "Remove-Item Env:MINICON_LOCAL_API_KEY -ErrorAction SilentlyContinue\n$mcKey = Read-Host 'Paste the selected profile key' -AsSecureString\n$env:MINICON_LOCAL_API_KEY = [System.Net.NetworkCredential]::new('', $mcKey).Password\n"
        } else {
            "unset MINICON_LOCAL_API_KEY\nread -r -s -p 'Paste the selected profile key: ' MINICON_LOCAL_API_KEY\nprintf '\\n'\nexport MINICON_LOCAL_API_KEY\n"
        };
        let cleanup = if windows_shell {
            "Remove-Item Env:MINICON_LOCAL_API_KEY\nRemove-Variable mcKey\n"
        } else {
            "unset MINICON_LOCAL_API_KEY\n"
        };
        let header = format!(
            "# Selected applied profile: {profile}; default alias: {}\n# {state}\n# Copy THIS profile's key separately, then paste into the private prompt.\n# Environment applies to this shell/its children only; sharing a key shares its allocation.\n",
            allocation.alias
        );
        Ok(match kind {
            RecipeKind::Worker => format!(
                "{header}{setup}minicon harness --root '<project-path>' --task '<task>' --gateway http://127.0.0.1:{port}/v1 --client-key-env MINICON_LOCAL_API_KEY --allow-cmd '<explicit-command>'\n{cleanup}# Replace every <placeholder>; no task is launched by copying this template.\n"
            ),
            RecipeKind::Pi => {
                // Same pinned API/compatibility options as the existing Pi 1.4 court.
                // Capability placeholders intentionally avoid inventing account/model limits.
                let config = serde_json::json!({"providers":{"minicon":{
                    "baseUrl":format!("http://127.0.0.1:{port}/v1"),"api":"openai-completions","apiKey":"$MINICON_LOCAL_API_KEY","authHeader":true,
                    "headers":{"x-minicon-session":"$MINICON_PI_SESSION"},
                    "compat":{"supportsStore":false,"supportsDeveloperRole":false,"supportsReasoningEffort":false,"supportsStrictMode":false,"supportsUsageInStreaming":true,"maxTokensField":"max_tokens","sendSessionAffinityHeaders":true,"sessionAffinityFormat":"openai"},
                    "models":[{"id":allocation.alias,"name":"Selected MiniCon applied alias","reasoning":false,"input":["text"],"contextWindow":"<actual-context-limit-number>","maxTokens":"<chosen-output-limit-number>"}]
                }}});
                let session = if windows_shell {
                    "$env:MINICON_PI_SESSION = '<fresh-conversation-id>'\n"
                } else {
                    "export MINICON_PI_SESSION='<fresh-conversation-id>'\n"
                };
                let end = if windows_shell {
                    "Remove-Item Env:MINICON_PI_SESSION\n"
                } else {
                    "unset MINICON_PI_SESSION\n"
                };
                format!(
                    "{header}# Pi 1.4.0 manual template; scoped compatibility, not all clients/models.\n{setup}{session}# Manually merge this provider into Pi models.json. Replace numeric capability placeholders with verified model limits.\n{}\n# Run explicitly after replacing all placeholders:\npi --provider minicon --model {}\n{cleanup}{end}",
                    serde_json::to_string_pretty(&config).expect("finite recipe JSON"),
                    shell_literal(&allocation.alias, windows_shell)
                )
            }
        })
    }
    pub(super) fn copy_connection_template(
        &mut self,
        kind: RecipeKind,
        sink: impl FnOnce(&str) -> bool,
    ) {
        self.sync_key_ui();
        self.keys.begin_intent();
        let profile = self.selected_key_profile().unwrap_or("default").to_owned();
        if !self.key_target_applied(&profile) {
            self.keys.report(&profile, None, FeedbackKind::ApplyFirst);
            return;
        }
        let admission = self
            .runtime
            .as_ref()
            .ok_or(RuntimeError::WorkerUnavailable)
            .and_then(|runtime| runtime.profile_key_handoff_ready(&profile));
        if self.forms.publication_request().is_some() || admission.is_err() {
            self.keys.report(
                &profile,
                None,
                if admission == Err(RuntimeError::Busy)
                    || self.forms.publication_request().is_some()
                {
                    FeedbackKind::Busy
                } else if admission == Err(RuntimeError::MissingClientKey) {
                    FeedbackKind::CreateFirst
                } else {
                    FeedbackKind::Refused
                },
            );
            return;
        }
        match self.connection_template(kind, cfg!(windows)) {
            Ok(template) => {
                let copied = sink(&template);
                self.keys.report(
                    &profile,
                    None,
                    if copied {
                        FeedbackKind::RecipeCopied
                    } else {
                        FeedbackKind::RecipeFailed
                    },
                );
            }
            Err(kind) => self.keys.report(&profile, None, kind),
        }
    }
    pub(super) fn key_snapshot(&self) -> json::JsonValue {
        let profile = self.selected_key_profile();
        let metadata =
            profile.and_then(|id| self.view.profile_keys.iter().find(|row| row.profile == id));
        let pending = profile.and_then(|id| self.keys.pending.get(id));
        json::object(vec![
            ("profile", profile.map_or(json::JsonValue::Null, Into::into)),
            (
                "applied",
                profile.is_some_and(|id| self.key_target_applied(id)).into(),
            ),
            (
                "exists",
                metadata.map_or(json::JsonValue::Null, |row| row.exists.into()),
            ),
            (
                "state",
                metadata.map_or(json::JsonValue::Null, |row| {
                    format!("{:?}", row.state).into()
                }),
            ),
            (
                "durability_unconfirmed",
                metadata.map_or(json::JsonValue::Null, |row| {
                    row.durability_unconfirmed.into()
                }),
            ),
            (
                "pending",
                pending.map_or(json::JsonValue::Null, |pending| {
                    json::object(vec![
                        ("request", pending.request.into()),
                        ("operation", format!("{:?}", pending.operation).into()),
                    ])
                }),
            ),
            (
                "feedback",
                self.keys
                    .feedback
                    .as_ref()
                    .map_or(json::JsonValue::Null, |feedback| {
                        json::object(vec![
                            ("profile", feedback.profile.as_str().into()),
                            (
                                "request",
                                feedback.request.map_or(json::JsonValue::Null, Into::into),
                            ),
                            ("kind", feedback.kind.name().into()),
                        ])
                    }),
            ),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const CUSTOM: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const DRAFT: &str = "cccccccccccccccccccccccccccccccc";

    fn with_ui(test: impl FnOnce(&mut HarnessSettings, &std::path::Path)) {
        with_configuration(true, test);
    }
    fn with_configuration(
        applied: bool,
        test: impl FnOnce(&mut HarnessSettings, &std::path::Path),
    ) {
        use crate::provider_server::{
            ClientAllocation, ClientProfile, Config, ModelRoute, WireProtocol,
        };
        let root = std::env::temp_dir().join(format!(
            "minicon-k2-{}",
            crate::provider_server::random_id().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        minicon_platform::filesystem::protect_private_directory(&root).unwrap();
        let mut config = Config::fresh().unwrap();
        config.version = 2;
        config.selected_account = Some("a".repeat(32));
        config.model = "deepseek/k2-fixture".into();
        config.models = vec![ModelRoute {
            id: config.model.clone(),
            model_id: "k2-fixture".into(),
            provider: Provider::DeepSeek,
            protocol: WireProtocol::Messages,
            url: "https://api.deepseek.com/anthropic/v1/messages".into(),
            headers: vec![],
        }];
        config.profiles = vec![ClientProfile {
            id: CUSTOM.into(),
            label: "Synthetic custom".into(),
            enabled: true,
            default_model: "work".into(),
            allocations: vec![ClientAllocation {
                alias: "work".into(),
                account: "a".repeat(32),
                route: config.model.clone(),
            }],
        }];
        config.validate().unwrap();
        if applied {
            minicon_platform::filesystem_publish::write_private_atomic(
                &root.join("gateway.json"),
                &serde_json::to_vec(&config).unwrap(),
            )
            .unwrap();
        }
        let runtime = provider_runtime::Runtime::new(
            root.clone(),
            crate::provider_login::LoginMetadata {
                version: "synthetic-k2".into(),
                locale: "en".into(),
                timezone_offset_seconds: 0,
            },
            std::sync::Arc::new(|| {}),
        )
        .unwrap();
        let mut settings = HarnessSettings {
            runtime: Some(runtime),
            ..Default::default()
        };
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let effects = settings.runtime.as_mut().unwrap().poll();
            assert!(
                effects.is_empty(),
                "opening settings unexpectedly produced a secret/effect"
            );
            settings.view = settings.runtime.as_ref().unwrap().view().clone();
            if settings.view.load_state == provider_runtime::LoadState::Loaded
                && !settings.view.busy
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "synthetic local runtime load deadline"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        settings.sync_connection_forms();
        test(&mut settings, &root);
        drop(settings);
        std::fs::remove_dir_all(root).unwrap();
    }
    fn collect(settings: &mut HarnessSettings) -> Vec<Effect> {
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        let mut output = Vec::new();
        loop {
            output.extend(settings.runtime.as_mut().unwrap().poll());
            settings.view = settings.runtime.as_ref().unwrap().view().clone();
            settings.sync_connection_forms();
            if settings.keys.pending.is_empty() {
                return output;
            }
            assert!(Instant::now() < deadline, "explicit key receipt deadline");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    fn copy_effect(effects: Vec<Effect>) -> ClientKeyCopy {
        assert_eq!(
            effects.len(),
            1,
            "one explicit Copy must prepare exactly one effect"
        );
        match effects.into_iter().next().unwrap() {
            Effect::CopyClientKey(copy) => copy,
            _ => panic!("explicit Copy produced unrelated effect"),
        }
    }
    fn create(settings: &mut HarnessSettings) {
        settings.activate(Row::KeyActions, 0);
        assert_eq!(
            settings.keys.pending.len(),
            1,
            "real UI Create was not accepted"
        );
        assert!(
            collect(settings).is_empty(),
            "Create automatically copied its key"
        );
    }

    #[test]
    fn k2_canonical_default_keeps_key_setup_before_any_applied_model_or_listener() {
        with_configuration(false, |settings, root| {
            assert!(settings.view.connection_profiles.iter().all(|row| {
                row.applied
                    .as_ref()
                    .is_none_or(|applied| applied.allocations.is_empty())
            }));
            create(settings);
            assert!(root.join("client-key").exists());
            settings.activate(Row::KeyActions, 1);
            let copy = copy_effect(collect(settings));
            let mut calls = 0;
            settings.consume_key_copy(copy, |_| {
                calls += 1;
                true
            });
            assert_eq!(calls, 1);
            settings.copy_connection_template(RecipeKind::Worker, |_| {
                panic!("unconfigured worker recipe copied")
            });
            assert_eq!(
                settings.keys.feedback.as_ref().unwrap().kind,
                FeedbackKind::ApplyFirst
            );
            settings.activate(Row::KeyActions, 2);
            assert!(collect(settings).is_empty());
            assert!(!root.join("client-key").exists());
            assert_eq!(settings.view.listener, ListenerState::Stopped);
            assert!(settings.view.connection_check_active.is_none());
        });
    }

    #[test]
    fn k2_actual_selected_custom_key_actions_preserve_default_and_do_not_retarget() {
        with_ui(|settings, root| {
            create(settings);
            let default = std::fs::read(root.join("client-key")).unwrap();
            settings.focus_connection(CUSTOM);
            create(settings);
            let custom_path = root.join(format!("client-key-{CUSTOM}"));
            assert!(
                custom_path.exists(),
                "selected custom Create did not publish its own private file"
            );
            let custom = std::fs::read(&custom_path).unwrap();
            assert!(custom != default, "profiles shared a generated key");
            settings.activate(Row::KeyActions, 1);
            let effect = copy_effect(collect(settings));
            let mut sink = Vec::new();
            settings.consume_key_copy(effect, |value| {
                sink.extend_from_slice(value.as_bytes());
                true
            });
            assert!(
                sink == custom,
                "actual Copy sink received a different profile's key"
            );
            assert!(matches!(
                settings.keys.feedback.as_ref().unwrap().kind,
                FeedbackKind::Copied
            ));
            settings.activate(Row::KeyActions, 2);
            assert!(collect(settings).is_empty());
            assert!(
                !custom_path.exists(),
                "selected Revoke left the actual private file"
            );
            assert!(
                std::fs::read(root.join("client-key")).unwrap() == default,
                "custom action changed default key bytes"
            );
            assert_eq!(settings.view.listener, ListenerState::Stopped);
            assert!(settings.view.connection_check_active.is_none());
        });
    }

    #[test]
    fn k2_actual_clipboard_failure_survives_repeated_preparation_receipts_and_retry_is_explicit() {
        with_ui(|settings, _| {
            create(settings);
            settings.activate(Row::KeyActions, 1);
            let effect = copy_effect(collect(settings));
            let mut calls = 0;
            settings.consume_key_copy(effect, |_| {
                calls += 1;
                false
            });
            assert_eq!(calls, 1);
            assert_eq!(
                settings.keys.feedback.as_ref().unwrap().kind,
                FeedbackKind::ClipboardFailed
            );
            for _ in 0..3 {
                settings.sync_key_ui();
            }
            assert_eq!(calls, 1, "poll replayed clipboard delivery");
            assert_eq!(
                settings.keys.feedback.as_ref().unwrap().kind,
                FeedbackKind::ClipboardFailed,
                "backend prepared success hid clipboard failure"
            );
            settings.activate(Row::KeyActions, 1);
            let retry = copy_effect(collect(settings));
            settings.consume_key_copy(retry, |_| {
                calls += 1;
                true
            });
            assert_eq!(calls, 2);
            assert_eq!(
                settings.keys.feedback.as_ref().unwrap().kind,
                FeedbackKind::Copied
            );
        });
    }

    #[test]
    fn k2_switch_aba_and_stale_a_effect_cannot_take_new_b_copy_intent() {
        with_ui(|settings, root| {
            create(settings);
            settings.activate(Row::KeyActions, 1);
            let old = copy_effect(collect(settings));
            settings.focus_connection(CUSTOM);
            settings.focus_connection("default");
            let mut calls = 0;
            settings.consume_key_copy(old, |_| {
                calls += 1;
                true
            });
            assert_eq!(calls, 0, "A->B->A restored an obsolete Copy authorization");
            settings.activate(Row::KeyActions, 1);
            let stale_a = copy_effect(collect(settings));
            settings.activate(Row::KeyActions, 1);
            let stale_after_failure = copy_effect(collect(settings));
            settings.focus_connection(CUSTOM);
            create(settings);
            settings.activate(Row::KeyActions, 1);
            let new_b = copy_effect(collect(settings));
            let current_request = settings.keys.copy.as_ref().unwrap().request;
            settings.consume_key_copy(stale_a, |_| {
                calls += 1;
                true
            });
            assert_eq!(calls, 0);
            assert!(
                settings
                    .keys
                    .copy
                    .as_ref()
                    .is_some_and(|intent| intent.request == current_request),
                "stale A consumed B's intent"
            );
            let expected = std::fs::read(root.join(format!("client-key-{CUSTOM}"))).unwrap();
            settings.consume_key_copy(new_b, |value| {
                calls += 1;
                assert!(value.as_bytes() == expected);
                false
            });
            assert_eq!(calls, 1);
            settings.consume_key_copy(stale_after_failure, |_| {
                calls += 1;
                true
            });
            assert_eq!(calls, 1);
            assert_eq!(
                settings.keys.feedback.as_ref().unwrap().kind,
                FeedbackKind::ClipboardFailed,
                "stale A effect overwrote B clipboard failure"
            );
            assert_eq!(settings.keys.feedback.as_ref().unwrap().profile, CUSTOM);
        });
    }

    #[test]
    fn k2_late_copy_and_receipt_cannot_overwrite_newer_busy_feedback() {
        with_ui(|settings, _| {
            create(settings);
            settings.activate(Row::KeyActions, 1);
            let old = copy_effect(collect(settings));
            settings.activate(Row::KeyActions, 1);
            settings.activate(Row::KeyActions, 1); // unresolved prior request: real Runtime Busy
            assert_eq!(
                settings.keys.feedback.as_ref().unwrap().kind,
                FeedbackKind::Busy
            );
            let newer_request = settings.keys.feedback.as_ref().unwrap().request;
            settings.consume_key_copy(old, |_| panic!("stale effect reached clipboard"));
            let effects = collect(settings);
            for effect in effects {
                if let Effect::CopyClientKey(copy) = effect {
                    settings.consume_key_copy(copy, |_| panic!("Busy intent replayed clipboard"));
                }
            }
            assert_eq!(
                settings.keys.feedback.as_ref().unwrap().kind,
                FeedbackKind::Busy,
                "older result overwrote newer Busy"
            );
            assert_eq!(
                settings.keys.feedback.as_ref().unwrap().request,
                newer_request
            );
        });
    }

    #[test]
    fn k2_close_reopen_between_polls_and_blur_reject_actual_prepared_copy() {
        with_ui(|settings, _| {
            create(settings);
            settings.activate(Row::KeyActions, 1);
            invalidate_key_for_settings_route(Some(settings), false);
            invalidate_key_for_settings_route(Some(settings), true);
            let effect = copy_effect(collect(settings));
            let mut calls = 0;
            settings.consume_key_copy(effect, |_| {
                calls += 1;
                true
            });
            assert_eq!(
                calls, 0,
                "close->reopen between polls authorized an old Copy"
            );
            assert_eq!(
                settings.keys.feedback.as_ref().unwrap().kind,
                FeedbackKind::NotCopied,
                "late preparation receipt overwrote close intent"
            );
            settings.activate(Row::KeyActions, 1);
            let effect = copy_effect(collect(settings));
            settings.blur();
            settings.consume_key_copy(effect, |_| {
                calls += 1;
                true
            });
            assert_eq!(calls, 0, "blur did not revoke Copy authorization");
        });
    }

    #[test]
    fn k2_draft_only_key_and_recipe_clicks_explain_apply_without_orphan_or_sink() {
        with_ui(|settings, root| {
            let mut draft = settings.view.connection_profiles[0].clone();
            draft.id = DRAFT.into();
            draft.applied = None;
            settings.view.connection_profiles.push(draft);
            settings.sync_connection_forms();
            settings.focus_connection(DRAFT);
            for column in 0..3 {
                settings.activate(Row::KeyActions, column);
                assert_eq!(
                    settings.keys.feedback.as_ref().unwrap().kind,
                    FeedbackKind::ApplyFirst
                );
            }
            settings.copy_connection_template(RecipeKind::Worker, |_| {
                panic!("draft-only recipe reached sink")
            });
            assert_eq!(
                settings.keys.feedback.as_ref().unwrap().kind,
                FeedbackKind::ApplyFirst
            );
            assert!(settings.keys.pending.is_empty());
            assert!(!root.join(format!("client-key-{DRAFT}")).exists());
            assert!(!root.join("client-key").exists());
            assert!(settings.view.connection_check_active.is_none());
        });
    }

    #[test]
    fn k2_recipe_and_public_metadata_use_applied_alias_actual_port_and_no_secret_or_dirty_model() {
        with_ui(|settings, root| {
            settings.focus_connection(CUSTOM);
            create(settings);
            let secret =
                std::fs::read_to_string(root.join(format!("client-key-{CUSTOM}"))).unwrap();
            settings.forms.current_mut().unwrap().model_mut().text = "unapplied-model".into();
            let form = settings.forms.current_mut().unwrap();
            form.model_mut().caret = 3;
            form.model_mut().anchor = Some(1);
            form.edit_model();
            form.default_alias.text = "unapplied-alias".into();
            form.allocations.get_mut(&0).unwrap().alias.text = "unapplied-alias".into();
            settings.view.listener = ListenerState::Listening;
            settings.view.actual_port = Some(42229);
            settings.view.port = 42228;
            let mut template = String::new();
            settings.copy_connection_template(RecipeKind::Worker, |value| {
                template = value.into();
                true
            });
            assert!(
                template.contains("127.0.0.1:42229/v1"),
                "recipe used configured instead of actual listening port"
            );
            assert!(template.contains(CUSTOM) && template.contains("default alias: work"));
            assert!(
                !template.contains("unapplied-alias"),
                "recipe used dirty alias rather than applied identity"
            );
            assert_eq!(settings.forms.current().unwrap().model().caret, 3);
            assert_eq!(settings.forms.current().unwrap().model().anchor, Some(1));
            assert!(
                settings.key_applied_label().contains("k2-fixture")
                    && !settings.key_applied_label().contains("unapplied-model")
            );
            assert!(!template.contains("unapplied-model") && !template.contains(&secret));
            assert!(
                template.contains("--client-key-env MINICON_LOCAL_API_KEY")
                    && template.contains("<project-path>")
                    && template.contains("<explicit-command>")
            );
            let observation = format!("{:?}", settings.key_snapshot());
            assert!(
                !observation.contains(&secret)
                    && !observation.contains("key_epoch")
                    && !observation.contains("unapplied-model")
            );
            settings.view.listener = ListenerState::Stopped;
            let stopped = settings
                .connection_template(RecipeKind::Worker, false)
                .unwrap();
            assert!(stopped.contains("Listener stopped") && stopped.contains("127.0.0.1:42228/v1"));
            let pi = settings.connection_template(RecipeKind::Pi, false).unwrap();
            assert!(
                pi.contains("openai-completions")
                    && pi.contains("$MINICON_LOCAL_API_KEY")
                    && pi.contains("$MINICON_PI_SESSION")
            );
            assert!(
                pi.contains("--model 'work'")
                    && pi.contains("actual-context-limit-number")
                    && !pi.contains(&secret)
            );
            assert_eq!(
                settings.keys.feedback.as_ref().unwrap().kind,
                FeedbackKind::RecipeCopied
            );
            settings.copy_connection_template(RecipeKind::Worker, |_| false);
            assert_eq!(
                settings.keys.feedback.as_ref().unwrap().kind,
                FeedbackKind::RecipeFailed
            );
        });
    }

    #[test]
    fn k2_pi_shell_literal_preserves_applied_alias_without_turning_it_into_a_command() {
        with_ui(|settings, _| {
            settings.focus_connection(CUSTOM);
            create(settings);
            let applied = settings
                .view
                .connection_profiles
                .iter_mut()
                .find(|row| row.id == CUSTOM)
                .unwrap()
                .applied
                .as_mut()
                .unwrap();
            applied.default_alias = "work'$(touch-x)".into();
            applied.allocations[0].alias = applied.default_alias.clone();
            let bash = settings.connection_template(RecipeKind::Pi, false).unwrap();
            let powershell = settings.connection_template(RecipeKind::Pi, true).unwrap();
            assert!(
                bash.contains("--model 'work'\"'\"'$(touch-x)'"),
                "bash recipe did not quote the actual alias"
            );
            assert!(
                powershell.contains("--model 'work''$(touch-x)'"),
                "PowerShell recipe did not quote the actual alias"
            );
        });
    }
}
