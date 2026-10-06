//! One account-bound cleanup session. Secrets never cross the Tauri boundary.
use crate::chatgpt_auth::{self, LoginAttempt, OAuthClient, Reauthorization, TokenBundle};
use crate::chatgpt_responses::{self, ChatGptError, ChatGptModel};
use crate::chatgpt_store::CredentialStore;
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct ChatGptAccount {
    pub email: Option<String>,
    pub name: Option<String>,
}
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct ChatGptStatus {
    pub connected: bool,
    pub plan_usage_enabled: bool,
    pub plan_usage_welcome_seen: bool,
    pub login_pending: bool,
    pub account: Option<ChatGptAccount>,
    pub models: Vec<ChatGptModel>,
    pub selected_model: Option<String>,
    pub service_tier: String,
    pub last_service_tier: Option<String>,
    pub last_error: Option<ChatGptError>,
    pub transcript_sharing_acknowledged: bool,
}
#[derive(Serialize, Deserialize)]
struct SavedSession {
    host_id: String,
    client_id: Option<String>,
    subject: Option<String>,
    tokens: Option<TokenBundle>,
    selected_model: Option<String>,
    service_tier: String,
    transcript_sharing_acknowledged: bool,
    #[serde(default)]
    plan_usage_welcome_seen: bool,
}
impl Default for SavedSession {
    fn default() -> Self {
        Self {
            host_id: chatgpt_auth::new_host_id(),
            client_id: None,
            subject: None,
            tokens: None,
            selected_model: None,
            service_tier: "default".into(),
            transcript_sharing_acknowledged: false,
            plan_usage_welcome_seen: false,
        }
    }
}
#[derive(Default)]
struct SessionState {
    saved: Option<SavedSession>,
    models: Vec<ChatGptModel>,
    last_service_tier: Option<String>,
    last_error: Option<ChatGptError>,
}

pub struct ChatGptManager {
    store: Arc<dyn CredentialStore>,
    state: Mutex<SessionState>,
    generation: AtomicU64,
    login_pending: AtomicBool,
}
impl ChatGptManager {
    pub fn new(store: Arc<dyn CredentialStore>) -> Self {
        Self {
            store,
            state: Mutex::new(SessionState::default()),
            generation: AtomicU64::new(0),
            login_pending: AtomicBool::new(false),
        }
    }
    fn load(&self, state: &mut SessionState) -> Result<(), ChatGptError> {
        if state.saved.is_none() {
            state.saved = Some(match self.store.load()? {
                Some(bytes) => serde_json::from_slice(&bytes).map_err(|_| {
                    ChatGptError::new(
                        "credential_store",
                        "Saved ChatGPT credentials could not be read.",
                    )
                })?,
                None => SavedSession::default(),
            });
        }
        Ok(())
    }
    fn save(&self, saved: &SavedSession) -> Result<(), ChatGptError> {
        self.store.save(&serde_json::to_vec(saved).map_err(|_| {
            ChatGptError::new("credential_store", "Could not save ChatGPT credentials.")
        })?)
    }
    fn status_locked(&self, state: &SessionState) -> ChatGptStatus {
        let saved = state.saved.as_ref();
        let tokens = saved.and_then(|s| s.tokens.as_ref());
        ChatGptStatus {
            connected: tokens.is_some(),
            plan_usage_enabled: tokens.is_some_and(TokenBundle::has_plan_scope),
            plan_usage_welcome_seen: saved.is_some_and(|s| s.plan_usage_welcome_seen),
            login_pending: self.login_pending.load(Ordering::SeqCst),
            account: tokens.map(|t| ChatGptAccount {
                email: t.account.email.clone(),
                name: t.account.name.clone(),
            }),
            models: state.models.clone(),
            selected_model: saved.and_then(|s| s.selected_model.clone()),
            service_tier: saved
                .map(|s| s.service_tier.clone())
                .unwrap_or_else(|| "default".into()),
            last_service_tier: state.last_service_tier.clone(),
            last_error: state.last_error.clone().or_else(|| {
                tokens
                    .filter(|t| !t.has_plan_scope())
                    .map(|_| plan_permission())
            }),
            transcript_sharing_acknowledged: saved
                .is_some_and(|s| s.transcript_sharing_acknowledged),
        }
    }
    pub async fn status(&self) -> ChatGptStatus {
        let mut state = self.state.lock().await;
        if let Err(error) = self.load(&mut state) {
            state.last_error = Some(error);
        }
        self.status_locked(&state)
    }
    pub async fn record_error(&self, error: ChatGptError) {
        self.state.lock().await.last_error = Some(error);
    }
    pub async fn acknowledge(&self) -> Result<ChatGptStatus, ChatGptError> {
        let mut state = self.state.lock().await;
        self.load(&mut state)?;
        let saved = state.saved.as_mut().expect("loaded session");
        saved.transcript_sharing_acknowledged = true;
        if let Err(e) = self.save(saved) {
            saved.transcript_sharing_acknowledged = false;
            return Err(e);
        }
        Ok(self.status_locked(&state))
    }
    pub async fn dismiss_welcome(&self) -> Result<ChatGptStatus, ChatGptError> {
        let mut state = self.state.lock().await;
        self.load(&mut state)?;
        let saved = state.saved.as_mut().expect("loaded session");
        let previous = saved.plan_usage_welcome_seen;
        saved.plan_usage_welcome_seen = true;
        if let Err(error) = self.save(saved) {
            saved.plan_usage_welcome_seen = previous;
            return Err(error);
        }
        Ok(self.status_locked(&state))
    }
    pub async fn configure(
        &self,
        model: String,
        tier: String,
    ) -> Result<ChatGptStatus, ChatGptError> {
        if self.login_pending.load(Ordering::SeqCst) {
            return Err(ChatGptError::new(
                "login_pending",
                "Finish or cancel ChatGPT sign-in first.",
            ));
        }
        if !matches!(tier.as_str(), "default" | "fast") {
            return Err(ChatGptError::new(
                "unsupported_tier",
                "Choose Standard or Fast.",
            ));
        }
        let mut state = self.state.lock().await;
        self.load(&mut state)?;
        if !state.models.iter().any(|m| m.slug == model) {
            return Err(ChatGptError::new(
                "invalid_model",
                "Refresh models and select an available model.",
            ));
        }
        let saved = state.saved.as_mut().expect("loaded session");
        let previous = (
            saved.selected_model.replace(model),
            std::mem::replace(&mut saved.service_tier, tier),
        );
        if let Err(e) = self.save(saved) {
            saved.selected_model = previous.0;
            saved.service_tier = previous.1;
            return Err(e);
        }
        self.generation.fetch_add(1, Ordering::SeqCst);
        state.last_error = None;
        Ok(self.status_locked(&state))
    }
    pub async fn begin_login(&self) -> Result<(LoginAttempt, u64), ChatGptError> {
        if self.login_pending.swap(true, Ordering::SeqCst) {
            return Err(ChatGptError::new(
                "login_pending",
                "A ChatGPT sign-in is already open.",
            ));
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let result = async {
            let mut state = self.state.lock().await;
            self.check_generation(generation)?;
            self.load(&mut state)?;
            let saved = state.saved.as_mut().expect("loaded session");
            self.save(saved)?;
            let registration = saved.client_id.as_deref().map(|client_id| Reauthorization {
                client_id,
                expected_subject: saved.subject.as_deref(),
                id_token_hint: saved.tokens.as_ref().map(|t| t.id_token.as_str()),
                login_hint: saved
                    .tokens
                    .as_ref()
                    .and_then(|t| t.account.email.as_deref()),
            });
            let mut attempt = LoginAttempt::bind(&saved.host_id, registration)
                .await
                .map_err(auth_error)?;
            // This command is an explicit sign-in action. Request consent again
            // only for a saved identity whose plan permission was declined.
            if saved.tokens.as_ref().is_some_and(|t| !t.has_plan_scope()) {
                attempt.request_plan_consent().map_err(auth_error)?;
            }
            self.check_generation(generation)?;
            state.last_error = None;
            Ok((attempt, generation))
        }
        .await;
        if result.is_err() && self.generation.load(Ordering::SeqCst) == generation {
            self.login_pending.store(false, Ordering::SeqCst);
        }
        result
    }
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.login_pending.store(false, Ordering::SeqCst);
    }
    pub async fn finish_login(
        &self,
        attempt: LoginAttempt,
        generation: u64,
    ) -> Result<(), ChatGptError> {
        let result = self.finish_login_inner(attempt, generation).await;
        // An old task must never dismiss a newer login or overwrite its error.
        if self.generation.load(Ordering::SeqCst) == generation {
            self.login_pending.store(false, Ordering::SeqCst);
            if let Err(ref error) = result {
                self.record_error(error.clone()).await;
            }
        }
        result
    }
    async fn finish_login_inner(
        &self,
        attempt: LoginAttempt,
        generation: u64,
    ) -> Result<(), ChatGptError> {
        self.check_generation(generation)?;
        let pending = tokio::select! {
            result=attempt.await_callback(Duration::from_secs(180))=>result.map_err(auth_error)?,
            _=async { loop {
                if self.generation.load(Ordering::SeqCst)!=generation { break; }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }}=>return Err(stale()),
        };
        let mut state = self.state.lock().await;
        self.check_generation(generation)?;
        let saved = state.saved.as_mut().expect("login initialized session");
        // Persist registration BEFORE code exchange; never revert to dynamic on failure.
        saved.client_id = Some(pending.client_id().to_owned());
        self.save(saved)?;
        let oauth = OAuthClient::new().map_err(auth_error)?;
        let tokens = oauth
            .exchange(pending, saved.client_id.as_deref().expect("issued client"))
            .await
            .map_err(auth_error)?;
        self.check_generation(generation)?;
        saved.subject = Some(tokens.account.subject.clone());
        saved.tokens = Some(tokens);
        saved.selected_model = None;
        if let Err(error) = self.save(saved) {
            saved.tokens = None;
            return Err(error);
        }
        state.models.clear();
        state.last_service_tier = None;
        let plan = state
            .saved
            .as_ref()
            .and_then(|s| s.tokens.as_ref())
            .is_some_and(TokenBundle::has_plan_scope);
        state.last_error = if plan { None } else { Some(plan_permission()) };
        Ok(())
    }
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
    pub fn check_generation(&self, generation: u64) -> Result<(), ChatGptError> {
        if self.generation.load(Ordering::SeqCst) != generation {
            Err(stale())
        } else {
            Ok(())
        }
    }
    async fn access_token(self: &Arc<Self>) -> Result<(String, u64), ChatGptError> {
        self.access_token_task_with(
            |tokens| async move { OAuthClient::new()?.refresh(&tokens).await },
        )
        .await
    }
    async fn access_token_task_with<F, Fut>(
        self: &Arc<Self>,
        refresh: F,
    ) -> Result<(String, u64), ChatGptError>
    where
        F: FnOnce(TokenBundle) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<TokenBundle, chatgpt_auth::AuthError>>
            + Send
            + 'static,
    {
        let manager = self.clone();
        // The recording pipeline cancels by dropping its future. A rotating
        // token request must finish and persist even if its caller disappears.
        tokio::spawn(async move { manager.access_token_with(refresh).await })
            .await
            .map_err(|_| {
                ChatGptError::new("unavailable", "ChatGPT token refresh was interrupted.")
            })?
    }
    async fn access_token_with<F, Fut>(&self, refresh: F) -> Result<(String, u64), ChatGptError>
    where
        F: FnOnce(TokenBundle) -> Fut,
        Fut: std::future::Future<Output = Result<TokenBundle, chatgpt_auth::AuthError>>,
    {
        let mut state = self.state.lock().await;
        self.load(&mut state)?;
        let generation = self.generation.load(Ordering::SeqCst);
        let saved = state.saved.as_mut().expect("loaded session");
        let tokens = saved.tokens.as_ref().ok_or_else(not_connected)?;
        if !tokens.has_plan_scope() {
            return Err(plan_permission());
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if tokens.needs_refresh(now, 60) && (tokens.can_refresh(now) || tokens.expires_at <= now) {
            let refreshed = match refresh(tokens.clone()).await {
                Ok(tokens) => tokens,
                Err(error) => {
                    if error.is_terminal_refresh() {
                        saved.tokens = None;
                        self.save(saved)?;
                    }
                    return Err(auth_error(error));
                }
            };
            saved.tokens = Some(refreshed);
            // One mutex spans refresh and durable replacement. No second caller
            // can observe/reuse the old rotated refresh token.
            if let Err(e) = self.save(saved) {
                saved.tokens = None;
                return Err(e);
            }
            // Rotation must be durable even if cancellation happened while the
            // server was rotating it. Only its USE is rejected as stale.
            self.check_generation(generation)?;
        }
        let tokens = saved.tokens.as_ref().ok_or_else(not_connected)?;
        if !tokens.has_plan_scope() {
            return Err(plan_permission());
        }
        if tokens.expires_at <= now {
            return Err(ChatGptError::new(
                "auth_expired",
                "Sign in with ChatGPT to renew access.",
            ));
        }
        Ok((tokens.access_token.clone(), generation))
    }
    pub async fn refresh_models(self: &Arc<Self>) -> Result<ChatGptStatus, ChatGptError> {
        let (token, generation) = self.access_token().await?;
        let models =
            chatgpt_responses::fetch_models(&chatgpt_responses::http_client()?, &token).await?;
        let mut state = self.state.lock().await;
        self.check_generation(generation)?;
        let saved = state.saved.as_mut().expect("loaded session");
        if saved
            .selected_model
            .as_ref()
            .is_some_and(|slug| !models.iter().any(|m| &m.slug == slug))
        {
            saved.selected_model = None;
            self.save(saved)?;
        }
        state.models = models;
        state.last_error = None;
        Ok(self.status_locked(&state))
    }
    pub async fn logout(&self) -> Result<bool, ChatGptError> {
        self.logout_with(|tokens| async move { OAuthClient::new()?.revoke(&tokens).await })
            .await
    }
    async fn logout_with<F, Fut>(&self, revoke: F) -> Result<bool, ChatGptError>
    where
        F: FnOnce(TokenBundle) -> Fut,
        Fut: std::future::Future<Output = Result<(), chatgpt_auth::AuthError>>,
    {
        self.cancel();
        let mut state = self.state.lock().await;
        self.load(&mut state)?;
        let saved = state.saved.as_mut().expect("loaded session");
        let tokens = saved.tokens.take();
        saved.service_tier = "default".into();
        saved.selected_model = None;
        saved.transcript_sharing_acknowledged = false;
        // Always attempt local durable invalidation, even if revocation fails.
        let remote_revoked = match tokens {
            Some(tokens) => revoke(tokens).await.is_ok(),
            None => true,
        };
        let persisted = self.save(saved);
        state.models.clear();
        state.last_service_tier = None;
        state.last_error = None;
        persisted?;
        Ok(remote_revoked)
    }
    pub async fn cleanup(
        self: &Arc<Self>,
        instructions: &str,
        text: &str,
    ) -> Result<String, ChatGptError> {
        {
            let mut state = self.state.lock().await;
            self.load(&mut state)?;
            if let Some(error) = state.last_error.as_ref().filter(|e| {
                matches!(
                    e.code.as_str(),
                    "quota"
                        | "not_eligible"
                        | "unsupported_tier"
                        | "unsupported_capability"
                        | "forbidden"
                )
            }) {
                return Err(error.clone());
            }
            if !state
                .saved
                .as_ref()
                .is_some_and(|s| s.transcript_sharing_acknowledged)
            {
                return Err(ChatGptError::new(
                    "sharing_not_acknowledged",
                    "Confirm transcript sharing before using ChatGPT cleanup.",
                ));
            }
        }
        let (token, generation) = self.access_token().await?;
        let client = chatgpt_responses::http_client()?;
        // Entitlement is refreshed with the exact bearer used for this request.
        let models = chatgpt_responses::fetch_models(&client, &token).await?;
        let (model, tier) = {
            let mut state = self.state.lock().await;
            self.check_generation(generation)?;
            state.models = models;
            let saved = state.saved.as_ref().expect("loaded session");
            let model = saved.selected_model.clone().ok_or_else(|| {
                ChatGptError::new("invalid_model", "Select an available ChatGPT model.")
            })?;
            if !state.models.iter().any(|m| m.slug == model) {
                return Err(ChatGptError::new(
                    "invalid_model",
                    "The selected model is no longer available. Refresh and choose another model.",
                ));
            }
            (model, saved.service_tier.clone())
        };
        self.check_generation(generation)?;
        let response =
            chatgpt_responses::cleanup(&client, &token, &model, &tier, instructions, text).await?;
        let mut state = self.state.lock().await;
        self.check_generation(generation)?;
        state.last_service_tier = response.service_tier;
        state.last_error = None;
        Ok(response.text)
    }
}
fn auth_error(error: chatgpt_auth::AuthError) -> ChatGptError {
    use chatgpt_auth::AuthError::*;
    let code = match error {
        Network => "offline",
        CallbackTimeout => "login_timeout",
        AccessDenied => "login_cancelled",
        InvalidCallback
        | InvalidState
        | RegistrationIncomplete
        | RegistrationNotPersisted
        | ListenerUnavailable => "invalid_callback",
        IdentityMismatch | ClientMismatch => "identity_mismatch",
        Configuration | InvalidClient => "invalid_client",
        ServiceUnavailable | RefreshTooEarly => "unavailable",
        RateLimited => "quota",
        MissingIdentityScope => "plan_permission_required",
        InvalidGrant
        | InvalidRefreshToken
        | TokenExpired
        | RefreshTokenExpired
        | RefreshTokenInvalidated
        | RefreshTokenReused
        | RefreshUnavailable => "auth_expired",
        _ => "invalid_token",
    };
    ChatGptError::new(code, error.user_message())
}
fn not_connected() -> ChatGptError {
    ChatGptError::new("not_connected", "Sign in with ChatGPT to use cleanup.")
}
fn plan_permission() -> ChatGptError {
    ChatGptError::new(
        "plan_permission_required",
        "Enable ChatGPT plan usage when signing in.",
    )
}
fn stale() -> ChatGptError {
    ChatGptError::new(
        "stale_result",
        "The ChatGPT account or settings changed. Original text was kept.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct MemoryStore(std::sync::Mutex<Option<Vec<u8>>>);
    impl CredentialStore for MemoryStore {
        fn load(&self) -> Result<Option<Vec<u8>>, ChatGptError> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn save(&self, bytes: &[u8]) -> Result<(), ChatGptError> {
            *self.0.lock().unwrap() = Some(bytes.to_vec());
            Ok(())
        }
    }
    #[tokio::test]
    async fn first_use_requires_explicit_ack_and_no_old_provider_migration() {
        let manager = Arc::new(ChatGptManager::new(Arc::new(MemoryStore::default())));
        assert!(!manager.status().await.transcript_sharing_acknowledged);
        assert_eq!(
            manager
                .cleanup("instructions", "private text")
                .await
                .unwrap_err()
                .code,
            "sharing_not_acknowledged"
        );
        manager.acknowledge().await.unwrap();
        assert_eq!(
            manager
                .cleanup("instructions", "private text")
                .await
                .unwrap_err()
                .code,
            "not_connected"
        );
    }
    #[tokio::test]
    async fn logout_keeps_registration_but_clears_ack_and_selection() {
        let store = Arc::new(MemoryStore::default());
        let manager = Arc::new(ChatGptManager::new(store.clone()));
        manager.acknowledge().await.unwrap();
        {
            let mut state = manager.state.lock().await;
            let saved = state.saved.as_mut().unwrap();
            saved.client_id = Some("issued-test-client".into());
            saved.subject = Some("user-test".into());
            manager.save(saved).unwrap();
        }
        assert!(manager.logout().await.unwrap());
        let status = manager.status().await;
        assert!(!status.connected);
        assert!(!status.transcript_sharing_acknowledged);
        let saved: SavedSession = serde_json::from_slice(&store.load().unwrap().unwrap()).unwrap();
        assert_eq!(saved.client_id.as_deref(), Some("issued-test-client"));
        assert!(saved.tokens.is_none());
    }
    #[tokio::test]
    async fn stale_generation_is_rejected_on_cancel_and_configuration() {
        let manager = Arc::new(ChatGptManager::new(Arc::new(MemoryStore::default())));
        let old = manager.generation.load(Ordering::SeqCst);
        manager.cancel();
        assert_eq!(
            manager.check_generation(old).unwrap_err().code,
            "stale_result"
        );
        assert_eq!(
            manager
                .configure("unlisted".into(), "fast".into())
                .await
                .unwrap_err()
                .code,
            "invalid_model"
        );
    }
    #[tokio::test]
    async fn public_status_has_no_secret_fields() {
        let manager = Arc::new(ChatGptManager::new(Arc::new(MemoryStore::default())));
        let json = serde_json::to_value(manager.status().await).unwrap();
        for key in [
            "tokens",
            "access_token",
            "refresh_token",
            "id_token",
            "client_id",
            "host_id",
            "subject",
        ] {
            assert!(json.get(key).is_none());
        }
    }
    fn fake_tokens(expires_at: u64) -> TokenBundle {
        TokenBundle {
            client_id: "test-issued".into(),
            account: chatgpt_auth::Account {
                issuer: chatgpt_auth::ISSUER.into(),
                subject: "subject".into(),
                email: Some("test@example.invalid".into()),
                name: None,
            },
            access_token: "old-access-secret".into(),
            refresh_token: "old-refresh-secret".into(),
            id_token: "old-id-secret".into(),
            token_type: "Bearer".into(),
            expires_at,
            earliest_refresh_at: None,
            scopes: vec![
                "openid".into(),
                "resource.invoke".into(),
                "chatgpt.tokens.use.direct".into(),
            ],
            saved_at: 1,
            nonce: "nonce".into(),
        }
    }
    async fn seed(manager: &ChatGptManager, tokens: TokenBundle) {
        let mut state = manager.state.lock().await;
        manager.load(&mut state).unwrap();
        let saved = state.saved.as_mut().unwrap();
        saved.client_id = Some(tokens.client_id.clone());
        saved.subject = Some(tokens.account.subject.clone());
        saved.tokens = Some(tokens);
        manager.save(saved).unwrap();
    }
    #[tokio::test]
    async fn concurrent_refresh_is_serialized_and_durable_before_use() {
        let store = Arc::new(MemoryStore::default());
        let manager = Arc::new(ChatGptManager::new(store.clone()));
        seed(&manager, fake_tokens(0)).await;
        let calls = AtomicU64::new(0);
        let refresh = |mut tokens: TokenBundle| async {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(10)).await;
            tokens.access_token = "new-access-secret".into();
            tokens.refresh_token = "new-refresh-secret".into();
            tokens.expires_at = u64::MAX;
            Ok(tokens)
        };
        let (one, two) = tokio::join!(
            manager.access_token_with(refresh),
            manager.access_token_with(refresh)
        );
        assert_eq!(one.unwrap().0, "new-access-secret");
        assert_eq!(two.unwrap().0, "new-access-secret");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let persisted: SavedSession =
            serde_json::from_slice(&store.load().unwrap().unwrap()).unwrap();
        assert_eq!(
            persisted.tokens.unwrap().refresh_token,
            "new-refresh-secret"
        );
    }
    #[tokio::test]
    async fn transient_refresh_keeps_credentials_terminal_refresh_clears_them() {
        let store = Arc::new(MemoryStore::default());
        let manager = Arc::new(ChatGptManager::new(store.clone()));
        seed(&manager, fake_tokens(0)).await;
        assert_eq!(
            manager
                .access_token_with(|_| async { Err(chatgpt_auth::AuthError::Network) })
                .await
                .unwrap_err()
                .code,
            "offline"
        );
        assert!(manager.status().await.connected);
        assert_eq!(
            manager
                .access_token_with(|_| async { Err(chatgpt_auth::AuthError::InvalidGrant) })
                .await
                .unwrap_err()
                .code,
            "auth_expired"
        );
        assert!(!manager.status().await.connected);
        let saved: SavedSession = serde_json::from_slice(&store.load().unwrap().unwrap()).unwrap();
        assert!(saved.tokens.is_none());
        assert_eq!(saved.client_id.as_deref(), Some("test-issued"));
    }
    #[tokio::test]
    async fn local_logout_succeeds_when_remote_revocation_unconfirmed() {
        let store = Arc::new(MemoryStore::default());
        let manager = Arc::new(ChatGptManager::new(store.clone()));
        seed(&manager, fake_tokens(u64::MAX)).await;
        assert!(!manager
            .logout_with(|_| async { Err(chatgpt_auth::AuthError::Network) })
            .await
            .unwrap());
        assert!(!manager.status().await.connected);
        let saved: SavedSession = serde_json::from_slice(&store.load().unwrap().unwrap()).unwrap();
        assert!(saved.tokens.is_none());
    }
    #[tokio::test]
    async fn cancelling_login_closes_listener_without_callback_or_network() {
        let manager = Arc::new(ChatGptManager::new(Arc::new(MemoryStore::default())));
        let (attempt, generation) = manager.begin_login().await.unwrap();
        manager.cancel();
        assert_eq!(
            manager
                .finish_login(attempt, generation)
                .await
                .unwrap_err()
                .code,
            "stale_result"
        );
        assert!(!manager.status().await.login_pending);
    }
    #[tokio::test]
    async fn refresh_hint_does_not_block_still_valid_access_token() {
        let manager = Arc::new(ChatGptManager::new(Arc::new(MemoryStore::default())));
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut tokens = fake_tokens(now + 30);
        tokens.earliest_refresh_at = Some(now + 25);
        seed(&manager, tokens).await;
        assert_eq!(
            manager
                .access_token_with(|_| async { panic!("must not refresh too early") })
                .await
                .unwrap()
                .0,
            "old-access-secret"
        );
    }
    #[tokio::test]
    async fn cancel_while_login_waits_for_session_lock_cannot_restart_login() {
        let manager = Arc::new(ChatGptManager::new(Arc::new(MemoryStore::default())));
        let guard = manager.state.lock().await;
        let pending = manager.begin_login();
        tokio::pin!(pending);
        assert!(
            tokio::time::timeout(Duration::from_millis(5), pending.as_mut())
                .await
                .is_err()
        );
        manager.cancel();
        drop(guard);
        assert!(matches!(pending.await,Err(ChatGptError { code,.. }) if code=="stale_result"));
        assert!(!manager.status().await.login_pending);
    }
    #[tokio::test]
    async fn cancellation_during_refresh_persists_rotation_but_rejects_use() {
        let store = Arc::new(MemoryStore::default());
        let manager = Arc::new(ChatGptManager::new(store.clone()));
        seed(&manager, fake_tokens(0)).await;
        let refresh = |mut tokens: TokenBundle| async {
            manager.cancel();
            tokens.access_token = "rotated-access".into();
            tokens.refresh_token = "rotated-refresh".into();
            tokens.expires_at = u64::MAX;
            Ok(tokens)
        };
        assert_eq!(
            manager.access_token_with(refresh).await.unwrap_err().code,
            "stale_result"
        );
        let saved: SavedSession = serde_json::from_slice(&store.load().unwrap().unwrap()).unwrap();
        assert_eq!(saved.tokens.unwrap().refresh_token, "rotated-refresh");
    }
    #[tokio::test]
    async fn quota_and_eligibility_errors_pause_new_cleanup_until_explicit_recovery() {
        let manager = Arc::new(ChatGptManager::new(Arc::new(MemoryStore::default())));
        manager.acknowledge().await.unwrap();
        manager
            .record_error(ChatGptError::new("quota", "Usage limited"))
            .await;
        assert_eq!(
            manager
                .cleanup("instructions", "text")
                .await
                .unwrap_err()
                .code,
            "quota"
        );
    }
    #[tokio::test]
    async fn limited_permission_and_welcome_survive_restart() {
        let store = Arc::new(MemoryStore::default());
        let manager = Arc::new(ChatGptManager::new(store.clone()));
        let mut tokens = fake_tokens(u64::MAX);
        tokens.scopes = vec!["openid".into()];
        seed(&manager, tokens).await;
        manager.dismiss_welcome().await.unwrap();
        drop(manager);
        let manager = Arc::new(ChatGptManager::new(store));
        let status = manager.status().await;
        assert!(status.connected);
        assert!(!status.plan_usage_enabled);
        assert!(status.plan_usage_welcome_seen);
        assert_eq!(status.last_error.unwrap().code, "plan_permission_required");
        manager.logout_with(|_| async { Ok(()) }).await.unwrap();
        assert!(manager.status().await.plan_usage_welcome_seen);
    }
    #[tokio::test]
    async fn dropping_cleanup_caller_cannot_drop_successful_refresh_rotation() {
        let store = Arc::new(MemoryStore::default());
        let manager = Arc::new(ChatGptManager::new(store.clone()));
        seed(&manager, fake_tokens(0)).await;
        let task = manager.access_token_task_with(|mut tokens| async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            tokens.refresh_token = "survives-caller-cancel".into();
            tokens.expires_at = u64::MAX;
            Ok(tokens)
        });
        assert!(tokio::time::timeout(Duration::from_millis(5), task)
            .await
            .is_err());
        tokio::time::sleep(Duration::from_millis(40)).await;
        let saved: SavedSession = serde_json::from_slice(&store.load().unwrap().unwrap()).unwrap();
        assert_eq!(
            saved.tokens.unwrap().refresh_token,
            "survives-caller-cancel"
        );
    }
}
