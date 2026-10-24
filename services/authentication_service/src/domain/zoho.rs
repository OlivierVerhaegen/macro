//! Owner-bound Zoho Mail linking and serialized access-token refresh
//! (Olive Trees fork). Mirrors the Microsoft flow: the grant is stored per
//! reconnect generation and only the email domain adopts it into a mailbox.

use chrono::{DateTime, Duration, Utc};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use uuid::Uuid;
use zeroize::Zeroizing;
use zoho_client::{ZohoDataCenter, ZohoRefreshToken};

use crate::domain::microsoft::token::{
    EncryptedMicrosoftToken, MicrosoftRefreshToken, MicrosoftTokenCipher,
};

#[cfg(test)]
mod test;

/// Envelopes for Zoho tokens never decrypt as another secret kind.
pub const ZOHO_TOKEN_PURPOSE: &str = "zoho-refresh-token";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ZohoAuthError {
    #[error("a professional subscription is required to link an additional inbox")]
    PaymentRequired,
    #[error("Zoho linking is not configured")]
    NotConfigured,
    #[error("this Zoho linking attempt has expired or was already used")]
    InvalidAttempt,
    #[error("too many Zoho linking attempts")]
    TooManyAttempts,
    #[error("Zoho authorization requires reconnecting")]
    ReauthorizationRequired,
    #[error("Zoho authorization is temporarily busy; retry shortly")]
    Busy,
    #[error("Zoho authorization is temporarily unavailable")]
    Unavailable,
    #[error("the Zoho mailbox identity could not be verified")]
    InvalidIdentity,
    #[error("this mailbox is already connected to another owner")]
    OwnershipConflict,
}

/// Secret state is deliberately neither Debug nor serializable to the client.
pub struct ZohoLinkAttempt {
    pub id: Uuid,
    pub owner: Uuid,
    pub data_center: ZohoDataCenter,
    pub redirect_uri: String,
    pub return_uri: Option<String>,
    pub verifier: Zeroizing<String>,
    pub expires_at: DateTime<Utc>,
}

pub struct ZohoLinkStart {
    pub id: Uuid,
    pub authorization_url: String,
}

/// The provider adapter reads the identity from the Mail API with the new token.
pub struct VerifiedZohoGrant {
    pub zuid: String,
    pub account_id: String,
    pub email: String,
    /// The IMAP and SMTP login, which can differ from the primary `email`.
    pub mailbox_address: String,
    /// Every address the account may send as, primary included.
    pub send_addresses: Vec<String>,
    pub scopes: Vec<String>,
    pub refresh_token: ZohoRefreshToken,
}

pub struct StoredZohoGrant {
    pub id: Uuid,
    pub generation: i64,
    pub revision: i64,
    pub owner: String,
    pub email: String,
    pub data_center: ZohoDataCenter,
    pub account_id: String,
    pub scopes: Vec<String>,
    pub envelope: EncryptedMicrosoftToken,
}

/// Verified, non-secret mailbox binding for the email domain's initializer.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CompletedZohoGrant {
    pub grant_id: Uuid,
    pub generation: i64,
    pub owner: Uuid,
    pub email: String,
    pub data_center: ZohoDataCenter,
    pub zuid: String,
    pub account_id: String,
    pub scopes: Vec<String>,
    /// Every address the account may send as, as Zoho reported at consent.
    pub send_addresses: Vec<String>,
    /// The IMAP and SMTP login; `None` for grants stored before it was kept.
    pub mailbox_address: Option<String>,
}

pub struct RefreshedZohoToken {
    pub access_token: Zeroizing<String>,
    pub expires_in: u64,
}

/// An access token plus what its consumer needs to address the mailbox.
pub struct ZohoAccessGrant {
    token: Zeroizing<String>,
    pub scopes: Vec<String>,
    pub data_center: ZohoDataCenter,
    pub account_id: String,
    pub email: String,
}

impl ZohoAccessGrant {
    pub fn as_str(&self) -> &str {
        self.token.as_str()
    }
}

#[cfg_attr(test, mockall::automock)]
#[async_trait::async_trait]
pub trait ZohoGrantRepository: Send + Sync {
    /// Expire abandoned link attempts, then delete grants no mailbox, custodian
    /// or remaining attempt references.
    async fn collect_expired_grants(
        &self,
        older_than: DateTime<Utc>,
        limit: i64,
    ) -> Result<u64, ZohoAuthError>;
    async fn connection_facts(
        &self,
        owner: Uuid,
        reconnect: Option<Uuid>,
    ) -> Result<email::domain::inbox_entitlement::InboxConnectionFacts, ZohoAuthError>;
    /// Enforces the pending-attempt cap atomically for this owner.
    async fn begin_link(&self, attempt: &ZohoLinkAttempt) -> Result<(), ZohoAuthError>;
    /// Atomically claims an unexpired attempt exactly once.
    async fn claim_link(&self, id: Uuid) -> Result<ZohoLinkAttempt, ZohoAuthError>;
    async fn abandon_link(&self, id: Uuid) -> Result<(), ZohoAuthError>;
    /// Persists a new grant generation and marks the attempt completed in one
    /// transaction. A different Zoho user behind the same address is refused.
    async fn complete_link(
        &self,
        attempt: &ZohoLinkAttempt,
        identity: &VerifiedZohoGrant,
        envelope: &EncryptedMicrosoftToken,
        grant_id: Uuid,
    ) -> Result<(), ZohoAuthError>;
    async fn completed_grant(
        &self,
        attempt: Uuid,
        owner: Uuid,
    ) -> Result<CompletedZohoGrant, ZohoAuthError>;
    /// Resolves an active IMAP link's exact grant generation.
    async fn active_grant(
        &self,
        link_id: Uuid,
        generation: i64,
        sync_generation: i64,
    ) -> Result<StoredZohoGrant, ZohoAuthError>;
    async fn acquire_refresh(
        &self,
        grant: &StoredZohoGrant,
        lease: Uuid,
    ) -> Result<bool, ZohoAuthError>;
    /// Compare-and-swap on generation, revision and lease owner.
    async fn finish_refresh(
        &self,
        grant: &StoredZohoGrant,
        lease: Uuid,
    ) -> Result<bool, ZohoAuthError>;
    async fn release_refresh(
        &self,
        grant: &StoredZohoGrant,
        lease: Uuid,
        revoke: bool,
    ) -> Result<(), ZohoAuthError>;
}

#[cfg_attr(test, mockall::automock)]
#[async_trait::async_trait]
pub trait ZohoIdentityProvider: Send + Sync {
    fn authorize(&self, attempt: &ZohoLinkAttempt) -> Result<String, ZohoAuthError>;
    async fn exchange(
        &self,
        attempt: &ZohoLinkAttempt,
        code: &str,
    ) -> Result<VerifiedZohoGrant, ZohoAuthError>;
    async fn refresh(
        &self,
        data_center: ZohoDataCenter,
        token: &ZohoRefreshToken,
    ) -> Result<RefreshedZohoToken, ZohoAuthError>;
}

/// Object-safe because the API context holds it as `Arc<dyn ZohoAuth>`.
#[async_trait::async_trait]
pub trait ZohoAuth: Send + Sync {
    async fn start_link(
        &self,
        owner: Uuid,
        data_center: ZohoDataCenter,
        redirect_uri: String,
        return_uri: Option<String>,
        reconnect: Option<Uuid>,
    ) -> Result<ZohoLinkStart, ZohoAuthError>;
    /// Return destinations come from server state, never the callback's state.
    async fn complete_link(&self, id: Uuid, code: &str) -> Result<Option<String>, ZohoAuthError>;
    async fn completed_grant(
        &self,
        attempt: Uuid,
        owner: Uuid,
    ) -> Result<CompletedZohoGrant, ZohoAuthError>;
    async fn access_token(
        &self,
        link_id: Uuid,
        generation: i64,
        sync_generation: i64,
        force_refresh: bool,
    ) -> Result<ZohoAccessGrant, ZohoAuthError>;
}

/// Credential retention must not depend on the owner signing in again.
pub async fn collect_expired_grants(
    repository: &impl ZohoGrantRepository,
) -> Result<u64, ZohoAuthError> {
    const BATCH_SIZE: i64 = 256;
    repository
        .collect_expired_grants(Utc::now() - Duration::hours(24), BATCH_SIZE)
        .await
}

fn has_required_scopes(scopes: &[String]) -> bool {
    zoho_client::scopes::MAILBOX.iter().all(|required| {
        scopes
            .iter()
            .any(|scope| scope.eq_ignore_ascii_case(required))
    })
}

struct CachedToken {
    value: Zeroizing<String>,
    expires_at: DateTime<Utc>,
}

pub struct ZohoAuthService<R, P> {
    repo: R,
    provider: P,
    cipher: Arc<dyn MicrosoftTokenCipher>,
    cache: Mutex<HashMap<(Uuid, i64), CachedToken>>,
}

impl<R, P> ZohoAuthService<R, P> {
    /// `cipher` must be bound to [`ZOHO_TOKEN_PURPOSE`].
    pub fn new(repo: R, provider: P, cipher: Arc<dyn MicrosoftTokenCipher>) -> Self {
        Self {
            repo,
            provider,
            cipher,
            cache: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl<R: ZohoGrantRepository, P: ZohoIdentityProvider> ZohoAuth for ZohoAuthService<R, P> {
    async fn start_link(
        &self,
        owner: Uuid,
        data_center: ZohoDataCenter,
        redirect_uri: String,
        return_uri: Option<String>,
        reconnect: Option<Uuid>,
    ) -> Result<ZohoLinkStart, ZohoAuthError> {
        let facts = self.repo.connection_facts(owner, reconnect).await?;
        if !facts.permits_connection() {
            return Err(ZohoAuthError::PaymentRequired);
        }
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        // OS-backed randomness; no caller-controlled OAuth secrets.
        let verifier = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
        let attempt = ZohoLinkAttempt {
            id: macro_uuid::generate_uuid_v7(),
            owner,
            data_center,
            redirect_uri,
            return_uri,
            verifier: Zeroizing::new(verifier),
            expires_at: Utc::now() + Duration::minutes(10),
        };
        let authorization_url = self.provider.authorize(&attempt)?;
        self.repo.begin_link(&attempt).await?;
        Ok(ZohoLinkStart {
            id: attempt.id,
            authorization_url,
        })
    }

    async fn complete_link(&self, id: Uuid, code: &str) -> Result<Option<String>, ZohoAuthError> {
        let attempt = self.repo.claim_link(id).await?;
        let result = async {
            let identity = self.provider.exchange(&attempt, code).await?;
            if !has_required_scopes(&identity.scopes) {
                return Err(ZohoAuthError::InvalidIdentity);
            }
            if identity.zuid.is_empty()
                || identity.account_id.is_empty()
                || email_validator::normalize_email(&identity.email).is_none()
            {
                return Err(ZohoAuthError::InvalidIdentity);
            }
            // The owner is always the principal that initiated this attempt.
            let envelope = self
                .cipher
                .encrypt(
                    &attempt.owner.to_string(),
                    &identity.email,
                    MicrosoftRefreshToken::new(identity.refresh_token.expose().to_owned()),
                )
                .await
                .map_err(|_| ZohoAuthError::Unavailable)?;
            self.repo
                .complete_link(
                    &attempt,
                    &identity,
                    &envelope,
                    macro_uuid::generate_uuid_v7(),
                )
                .await?;
            Ok(attempt.return_uri.clone())
        }
        .await;
        if result.is_err() {
            let _ = self.repo.abandon_link(id).await;
        }
        result
    }

    async fn completed_grant(
        &self,
        attempt: Uuid,
        owner: Uuid,
    ) -> Result<CompletedZohoGrant, ZohoAuthError> {
        let grant = self.repo.completed_grant(attempt, owner).await?;
        if grant.owner != owner {
            return Err(ZohoAuthError::InvalidAttempt);
        }
        Ok(grant)
    }

    async fn access_token(
        &self,
        link_id: Uuid,
        generation: i64,
        sync_generation: i64,
        force_refresh: bool,
    ) -> Result<ZohoAccessGrant, ZohoAuthError> {
        // Check revocation and generation on every call, including cache hits.
        let grant = self
            .repo
            .active_grant(link_id, generation, sync_generation)
            .await?;
        let token = self.token_for_grant(&grant, force_refresh).await?;
        Ok(ZohoAccessGrant {
            token,
            scopes: grant.scopes,
            data_center: grant.data_center,
            account_id: grant.account_id,
            email: grant.email,
        })
    }
}

impl<R: ZohoGrantRepository, P: ZohoIdentityProvider> ZohoAuthService<R, P> {
    fn cached(&self, key: (Uuid, i64)) -> Result<Option<Zeroizing<String>>, ZohoAuthError> {
        Ok(self
            .cache
            .lock()
            .map_err(|_| ZohoAuthError::Unavailable)?
            .get(&key)
            .filter(|cached| cached.expires_at > Utc::now())
            .map(|cached| cached.value.clone()))
    }

    async fn token_for_grant(
        &self,
        grant: &StoredZohoGrant,
        force_refresh: bool,
    ) -> Result<Zeroizing<String>, ZohoAuthError> {
        let key = (grant.id, grant.generation);
        if force_refresh {
            self.cache
                .lock()
                .map_err(|_| ZohoAuthError::Unavailable)?
                .remove(&key);
        } else if let Some(token) = self.cached(key)? {
            return Ok(token);
        }
        // Zoho throttles refreshes per refresh token; one refresher at a time.
        let lease = macro_uuid::generate_uuid_v7();
        if !self.repo.acquire_refresh(grant, lease).await? {
            return Err(ZohoAuthError::Busy);
        }
        let result = async {
            let refresh = self
                .cipher
                .decrypt(&grant.owner, &grant.email, &grant.envelope)
                .await
                .map_err(|_| ZohoAuthError::Unavailable)?;
            let refreshed = self
                .provider
                .refresh(
                    grant.data_center,
                    &ZohoRefreshToken::new(refresh.as_str().to_owned()),
                )
                .await?;
            if !self.repo.finish_refresh(grant, lease).await? {
                return Err(ZohoAuthError::Busy);
            }
            let lifetime = refreshed.expires_in.min(86_400).saturating_sub(120) as i64;
            let mut cache = self.cache.lock().map_err(|_| ZohoAuthError::Unavailable)?;
            cache.retain(|_, token| token.expires_at > Utc::now());
            if cache.len() >= 10_000 {
                cache.clear();
            }
            cache.insert(
                key,
                CachedToken {
                    value: refreshed.access_token.clone(),
                    expires_at: Utc::now() + Duration::seconds(lifetime),
                },
            );
            Ok(refreshed.access_token)
        }
        .await;
        if let Err(error) = &result {
            self.repo
                .release_refresh(
                    grant,
                    lease,
                    matches!(error, ZohoAuthError::ReauthorizationRequired),
                )
                .await?;
        }
        result
    }
}
