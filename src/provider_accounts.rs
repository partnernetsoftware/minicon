//! Bounded account persistence for PRD 02.31.01. Call from a worker thread.
//! The injected root isolates profiles/tests; no environment credential import.

use crate::provider_auth::{Provider, Secret, TokenGrant, TokenLifecycle};
use minicon_platform::filesystem;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_ACCOUNTS: usize = 16;
const MAX_STORE_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum AccountState {
    CredentialStored,
    AccountReady,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AccountSummary {
    pub id: String,
    pub provider: Provider,
    pub label: String,
    pub state: AccountState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreError {
    InvalidRoot,
    InvalidAccount,
    Missing,
    Capacity,
    Storage,
    Corrupt,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "provider account: {self:?}")
    }
}

impl std::error::Error for StoreError {}

#[derive(Deserialize, Serialize)]
struct Registry {
    version: u8,
    accounts: Vec<AccountSummary>,
}

// Deliberately not Debug: only this file, inside the private directory, contains
// raw credentials. Account metadata and coordinator events never serialize it.
#[derive(Deserialize, Serialize)]
struct Credential {
    version: u8,
    provider: Provider,
    access_token: String,
    refresh_token: Option<String>,
    expires_unix_ms: Option<u64>,
    organization: Option<String>,
}

pub struct AccountStore {
    root: PathBuf,
    mutation: Mutex<()>,
}

impl AccountStore {
    /// No filesystem operation occurs here. All methods below can perform I/O.
    pub fn new(root: PathBuf) -> Result<Self, StoreError> {
        if !root.is_absolute() {
            return Err(StoreError::InvalidRoot);
        }
        Ok(Self {
            root,
            mutation: Mutex::new(()),
        })
    }

    pub fn list(&self) -> Result<Vec<AccountSummary>, StoreError> {
        let _guard = self.mutation.lock().map_err(|_| StoreError::Storage)?;
        self.read_registry()
    }

    pub fn insert(&self, label: &str, grant: TokenGrant) -> Result<AccountSummary, StoreError> {
        if !valid_label(label) {
            return Err(StoreError::InvalidAccount);
        }
        let _guard = self.mutation.lock().map_err(|_| StoreError::Storage)?;
        let mut accounts = self.read_registry()?;
        if accounts.len() >= MAX_ACCOUNTS {
            return Err(StoreError::Capacity);
        }
        self.prepare()?;
        let bytes = minicon_platform::entropy::secure_random_array::<16>()
            .map_err(|_| StoreError::Storage)?;
        let id: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let summary = AccountSummary {
            id,
            provider: grant.provider,
            label: label.to_owned(),
            state: AccountState::CredentialStored,
        };
        let path = self.credential_path(&summary.id)?;
        if path.try_exists().map_err(|_| StoreError::Storage)? {
            return Err(StoreError::Storage);
        }
        let credential = encode_grant(grant)?;
        let encoded = serde_json::to_vec(&credential).map_err(|_| StoreError::Storage)?;
        filesystem::write_private_atomic(&path, &encoded).map_err(|_| StoreError::Storage)?;
        accounts.push(summary.clone());
        if let Err(error) = self.write_registry(accounts) {
            let _ = std::fs::remove_file(path);
            return Err(error);
        }
        Ok(summary)
    }

    pub fn load_grant(&self, id: &str) -> Result<TokenGrant, StoreError> {
        let _guard = self.mutation.lock().map_err(|_| StoreError::Storage)?;
        let accounts = self.read_registry()?;
        let summary = accounts
            .iter()
            .find(|account| account.id == id)
            .ok_or(StoreError::Missing)?;
        self.read_grant(id, summary.provider)
    }

    /// Refresh cannot recreate an account removed while its HTTP request ran.
    /// The existence/provider check and private atomic replacement share the
    /// same mutation lock as removal. Existing readiness and metadata survive.
    pub fn replace_grant_if_present(&self, id: &str, grant: TokenGrant) -> Result<(), StoreError> {
        let _guard = self.mutation.lock().map_err(|_| StoreError::Storage)?;
        let accounts = self.read_registry()?;
        let account = accounts
            .iter()
            .find(|account| account.id == id)
            .ok_or(StoreError::Missing)?;
        if account.provider != grant.provider {
            return Err(StoreError::InvalidAccount);
        }
        let path = self.credential_path(id)?;
        let credential = encode_grant(grant)?;
        let encoded = serde_json::to_vec(&credential).map_err(|_| StoreError::Storage)?;
        self.prepare()?;
        filesystem::write_private_atomic(&path, &encoded).map_err(|_| StoreError::Storage)
    }

    /// Only an inference consumer with a successful real grant-backed request
    /// calls this. Login initialization/token persistence does not make it ready.
    pub fn mark_ready(&self, id: &str) -> Result<AccountSummary, StoreError> {
        let _guard = self.mutation.lock().map_err(|_| StoreError::Storage)?;
        let mut accounts = self.read_registry()?;
        let account = accounts
            .iter_mut()
            .find(|account| account.id == id)
            .ok_or(StoreError::Missing)?;
        self.read_grant(id, account.provider)?;
        account.state = AccountState::AccountReady;
        let summary = account.clone();
        self.write_registry(accounts)?;
        Ok(summary)
    }

    /// Local removal happens before the coordinator attempts remote revocation.
    /// Return only the removed grant to that worker, never to GUI events.
    pub fn remove(&self, id: &str) -> Result<Option<TokenGrant>, StoreError> {
        let _guard = self.mutation.lock().map_err(|_| StoreError::Storage)?;
        let mut accounts = self.read_registry()?;
        let index = accounts
            .iter()
            .position(|account| account.id == id)
            .ok_or(StoreError::Missing)?;
        // A missing/corrupt token must not trap the user in an account they
        // cannot disconnect. Remote revocation is unavailable in that case.
        let grant = self.read_grant(id, accounts[index].provider).ok();
        self.prepare()?;
        accounts.remove(index);
        self.write_registry(accounts)?;
        match std::fs::remove_file(self.credential_path(id)?) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(StoreError::Storage),
        }
        Ok(grant)
    }

    fn prepare(&self) -> Result<(), StoreError> {
        // Refuse pre-existing links before creating a missing descendant. The
        // platform repeats ancestry validation when protecting/private-opening.
        for ancestor in self.root.ancestors() {
            match std::fs::symlink_metadata(ancestor) {
                Ok(metadata) if filesystem::metadata_is_link_like(&metadata) => {
                    return Err(StoreError::InvalidRoot);
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(StoreError::Storage),
            }
        }
        std::fs::create_dir_all(&self.root).map_err(|_| StoreError::Storage)?;
        filesystem::protect_private_directory(&self.root).map_err(|_| StoreError::Storage)?;
        let credentials = self.root.join("credentials");
        std::fs::create_dir_all(&credentials).map_err(|_| StoreError::Storage)?;
        filesystem::protect_private_directory(&credentials).map_err(|_| StoreError::Storage)
    }

    fn credential_path(&self, id: &str) -> Result<PathBuf, StoreError> {
        if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(StoreError::InvalidAccount);
        }
        Ok(self.root.join("credentials").join(format!("{id}.json")))
    }

    fn read_registry(&self) -> Result<Vec<AccountSummary>, StoreError> {
        let bytes = match filesystem::read_private_bounded(
            &self.root.join("accounts.json"),
            MAX_STORE_BYTES,
        ) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(_) => return Err(StoreError::Storage),
        };
        let registry: Registry = serde_json::from_slice(&bytes).map_err(|_| StoreError::Corrupt)?;
        if registry.version != 1 || registry.accounts.len() > MAX_ACCOUNTS {
            return Err(StoreError::Corrupt);
        }
        for (index, account) in registry.accounts.iter().enumerate() {
            self.credential_path(&account.id)
                .map_err(|_| StoreError::Corrupt)?;
            if !valid_label(&account.label)
                || registry.accounts[..index]
                    .iter()
                    .any(|other| other.id == account.id)
            {
                return Err(StoreError::Corrupt);
            }
        }
        Ok(registry.accounts)
    }

    fn write_registry(&self, accounts: Vec<AccountSummary>) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(&Registry {
            version: 1,
            accounts,
        })
        .map_err(|_| StoreError::Storage)?;
        if bytes.len() > MAX_STORE_BYTES {
            return Err(StoreError::Capacity);
        }
        filesystem::write_private_atomic(&self.root.join("accounts.json"), &bytes)
            .map_err(|_| StoreError::Storage)
    }

    fn read_grant(&self, id: &str, provider: Provider) -> Result<TokenGrant, StoreError> {
        let bytes = filesystem::read_private_bounded(&self.credential_path(id)?, MAX_STORE_BYTES)
            .map_err(|_| StoreError::Storage)?;
        let credential: Credential =
            serde_json::from_slice(&bytes).map_err(|_| StoreError::Corrupt)?;
        if credential.version != 1 || credential.provider != provider {
            return Err(StoreError::Corrupt);
        }
        let lifecycle = match (credential.refresh_token, credential.expires_unix_ms) {
            (None, None) if provider == Provider::DeepSeek => TokenLifecycle::OpaqueUntilRejected,
            (Some(refresh), Some(expires)) if provider == Provider::OpenCode => {
                let remaining = Duration::from_millis(expires.saturating_sub(unix_ms()?));
                TokenLifecycle::ExpiringRefresh {
                    refresh_token: Secret::new(&refresh).map_err(|_| StoreError::Corrupt)?,
                    expires_at: Instant::now()
                        .checked_add(remaining)
                        .ok_or(StoreError::Corrupt)?,
                }
            }
            _ => return Err(StoreError::Corrupt),
        };
        Ok(TokenGrant {
            provider,
            access_token: Secret::new(&credential.access_token).map_err(|_| StoreError::Corrupt)?,
            lifecycle,
            organization: credential.organization,
        })
    }
}

fn encode_grant(grant: TokenGrant) -> Result<Credential, StoreError> {
    let (refresh_token, expires_unix_ms) = match grant.lifecycle {
        TokenLifecycle::OpaqueUntilRejected if grant.provider == Provider::DeepSeek => (None, None),
        TokenLifecycle::ExpiringRefresh {
            refresh_token,
            expires_at,
        } if grant.provider == Provider::OpenCode => {
            let remaining = u64::try_from(
                expires_at
                    .saturating_duration_since(Instant::now())
                    .as_millis(),
            )
            .map_err(|_| StoreError::Corrupt)?;
            (
                Some(refresh_token.expose().to_owned()),
                Some(
                    unix_ms()?
                        .checked_add(remaining)
                        .ok_or(StoreError::Corrupt)?,
                ),
            )
        }
        _ => return Err(StoreError::Corrupt),
    };
    Ok(Credential {
        version: 1,
        provider: grant.provider,
        access_token: grant.access_token.expose().to_owned(),
        refresh_token,
        expires_unix_ms,
        organization: grant.organization,
    })
}

fn unix_ms() -> Result<u64, StoreError> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| StoreError::Storage)?
            .as_millis(),
    )
    .map_err(|_| StoreError::Storage)
}

fn valid_label(label: &str) -> bool {
    !label.trim().is_empty() && label.len() <= 128 && !label.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let entropy = minicon_platform::entropy::secure_random_array::<16>().unwrap();
            let suffix: String = entropy.iter().map(|byte| format!("{byte:02x}")).collect();
            let root = std::env::temp_dir().join(format!("minicon-provider-accounts-{suffix}"));
            std::fs::create_dir_all(&root).unwrap();
            Self(std::fs::canonicalize(root).unwrap())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn grant(secret: &str) -> TokenGrant {
        TokenGrant {
            provider: Provider::DeepSeek,
            access_token: Secret::new(secret).unwrap(),
            lifecycle: TokenLifecycle::OpaqueUntilRejected,
            organization: None,
        }
    }

    #[test]
    fn profile_roots_and_secret_files_are_isolated_and_login_is_not_ready() {
        let fixture = Fixture::new();
        let first = AccountStore::new(fixture.0.join("first")).unwrap();
        let second = AccountStore::new(fixture.0.join("second")).unwrap();
        let account = first
            .insert("Synthetic", grant("synthetic-account-secret"))
            .unwrap();
        assert_eq!(account.state, AccountState::CredentialStored);
        assert!(second.list().unwrap().is_empty());
        let metadata = std::fs::read_to_string(first.root.join("accounts.json")).unwrap();
        assert!(!metadata.contains("synthetic-account-secret"));
        assert_eq!(
            first.load_grant(&account.id).unwrap().access_token.expose(),
            "synthetic-account-secret"
        );
        assert_eq!(
            first.mark_ready(&account.id).unwrap().state,
            AccountState::AccountReady
        );
        let removed = first.remove(&account.id).unwrap().unwrap();
        assert_eq!(removed.access_token.expose(), "synthetic-account-secret");
        assert!(first.list().unwrap().is_empty());
        assert!(!first.credential_path(&account.id).unwrap().exists());
        assert_eq!(
            first.load_grant("../../escape").unwrap_err(),
            StoreError::Missing
        );
    }

    #[test]
    fn corrupt_metadata_is_not_silently_replaced_and_expiring_grants_round_trip() {
        let fixture = Fixture::new();
        let store = AccountStore::new(fixture.0.clone()).unwrap();
        let account = store
            .insert(
                "OpenCode synthetic",
                TokenGrant {
                    provider: Provider::OpenCode,
                    access_token: Secret::new("synthetic-access").unwrap(),
                    lifecycle: TokenLifecycle::ExpiringRefresh {
                        refresh_token: Secret::new("synthetic-refresh").unwrap(),
                        expires_at: Instant::now() + Duration::from_secs(600),
                    },
                    organization: Some("org_synthetic".into()),
                },
            )
            .unwrap();
        let restored = store.load_grant(&account.id).unwrap();
        assert!(matches!(
            restored.lifecycle,
            TokenLifecycle::ExpiringRefresh { .. }
        ));
        assert_eq!(restored.organization.as_deref(), Some("org_synthetic"));
        filesystem::write_private_atomic(&store.root.join("accounts.json"), b"broken").unwrap();
        assert_eq!(
            store.insert("Other", grant("synthetic-other")).unwrap_err(),
            StoreError::Corrupt
        );
        assert_eq!(
            std::fs::read(store.root.join("accounts.json")).unwrap(),
            b"broken"
        );
        assert!(AccountStore::new(std::path::Path::new("relative").to_owned()).is_err());
    }

    #[test]
    fn corrupt_credential_can_be_removed_without_claiming_remote_revocation() {
        let fixture = Fixture::new();
        let store = AccountStore::new(fixture.0.clone()).unwrap();
        let account = store
            .insert("Synthetic", grant("synthetic-to-remove"))
            .unwrap();
        filesystem::write_private_atomic(&store.credential_path(&account.id).unwrap(), b"broken")
            .unwrap();
        assert!(store.remove(&account.id).unwrap().is_none());
        assert!(store.list().unwrap().is_empty());
        assert!(!store.credential_path(&account.id).unwrap().exists());
    }

    #[test]
    fn refresh_replacement_preserves_identity_and_cannot_resurrect_removed_account() {
        let fixture = Fixture::new();
        let store = AccountStore::new(fixture.0.clone()).unwrap();
        let account = store.insert("Synthetic", grant("old-token")).unwrap();
        let summary = store.mark_ready(&account.id).unwrap();
        store
            .replace_grant_if_present(&account.id, grant("rotated-token"))
            .unwrap();
        assert_eq!(store.list().unwrap(), vec![summary.clone()]);
        assert_eq!(
            store.load_grant(&account.id).unwrap().access_token.expose(),
            "rotated-token"
        );

        let mut wrong_provider = grant("wrong-provider-token");
        wrong_provider.provider = Provider::OpenCode;
        wrong_provider.lifecycle = TokenLifecycle::ExpiringRefresh {
            refresh_token: Secret::new("wrong-provider-refresh").unwrap(),
            expires_at: Instant::now() + Duration::from_secs(600),
        };
        assert_eq!(
            store.replace_grant_if_present(&account.id, wrong_provider),
            Err(StoreError::InvalidAccount)
        );
        assert_eq!(
            store.load_grant(&account.id).unwrap().access_token.expose(),
            "rotated-token"
        );

        store.remove(&account.id).unwrap();
        assert_eq!(
            store.replace_grant_if_present(&account.id, grant("late-refresh-token")),
            Err(StoreError::Missing)
        );
        assert!(store.list().unwrap().is_empty());
        assert!(!store.credential_path(&account.id).unwrap().exists());
    }
}
