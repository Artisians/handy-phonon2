//! Public-client Sign in with ChatGPT OAuth/OIDC support.
//!
//! This module never opens a browser, persists credentials, logs a response, or
//! activates an account. The caller owns those actions. Persist `new_host_id()`
//! before the first attempt; bind the listener before opening `authorization_url`.
//! After `await_callback`, save the issued client ID BEFORE `exchange`, even if
//! the exchange later fails. Save a successful token bundle atomically in OS-
//! protected storage and serialize refresh + persistence for each registration.
//!
//! Protocol: https://developers.openai.com/siwc/token-sharing-open-source/sign-in

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use rand::{rngs::OsRng, RngCore};
use reqwest::{Client, StatusCode, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::{timeout, Instant},
};

pub const ISSUER: &str = "https://auth.openai.com";
pub const RESOURCE: &str = "https://api.openai.com/v1";
pub const AUTHORIZE_ENDPOINT: &str = "https://auth.openai.com/api/accounts/authorize";
pub const TOKEN_ENDPOINT: &str = "https://auth.openai.com/api/accounts/oauth/token";
pub const DISCOVERY_ENDPOINT: &str = "https://auth.openai.com/.well-known/openid-configuration";
pub const REQUESTED_SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const DYNAMIC_CLIENT_ID: &str = "dynamic_agent_client";
const CALLBACK_PATH: &str = "/auth/callback";
const MAX_HTTP_HEADERS: usize = 16 * 1024;
const MAX_JSON_BODY: usize = 1024 * 1024;
const MAX_TOKEN_LEN: usize = 64 * 1024;
const CLOCK_SKEW_SECONDS: u64 = 30;

/// A new opaque per-installation host identifier. Persist once, before OAuth.
pub fn new_host_id() -> String {
    uuid::Uuid::new_v4().urn().to_string()
}

/// Display-only information from a signature-validated ID token. This is never
/// a workspace identifier; keep it paired with its issued client ID.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    pub issuer: String,
    pub subject: String,
    pub email: Option<String>,
    pub name: Option<String>,
}

/// Secret credential material, deliberately without Debug. Never send this
/// structure to a webview, write ordinary settings, or include it in a log.
#[derive(Clone, Serialize, Deserialize)]
pub struct TokenBundle {
    pub client_id: String,
    pub account: Account,
    pub access_token: String,
    /// Empty only when offline access was not granted. Never reuse an old
    /// refresh token after a successful refresh response omits its replacement.
    pub refresh_token: String,
    pub id_token: String,
    pub token_type: String,
    pub expires_at: u64,
    pub earliest_refresh_at: Option<u64>,
    pub scopes: Vec<String>,
    pub saved_at: u64,
    pub nonce: String,
}

impl TokenBundle {
    /// Both the resource invocation and ChatGPT-plan grant are needed.
    pub fn has_plan_scope(&self) -> bool {
        self.scopes
            .iter()
            .any(|scope| scope == "chatgpt.tokens.use.direct")
            && self.scopes.iter().any(|scope| scope == "resource.invoke")
    }

    pub fn needs_refresh(&self, now: u64, skew_seconds: u64) -> bool {
        self.expires_at <= now.saturating_add(skew_seconds)
    }

    pub fn can_refresh(&self, now: u64) -> bool {
        !self.refresh_token.is_empty()
            && self
                .earliest_refresh_at
                .is_none_or(|earliest| now >= earliest)
    }
}

/// Hints and identity for one saved registration, including after sign-out.
/// Retain the expected subject across sign-out; email is not an identity key.
pub struct Reauthorization<'a> {
    pub client_id: &'a str,
    pub expected_subject: Option<&'a str>,
    pub id_token_hint: Option<&'a str>,
    pub login_hint: Option<&'a str>,
}

/// All errors are deliberately closed, non-secret categories. Network errors,
/// URLs, OAuth descriptions, response bodies and token contents are discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    Configuration,
    ListenerUnavailable,
    CallbackTimeout,
    InvalidCallback,
    InvalidState,
    AccessDenied,
    AuthorizationFailed,
    ClientMismatch,
    RegistrationIncomplete,
    RegistrationNotPersisted,
    Network,
    ServiceUnavailable,
    RateLimited,
    InvalidResponse,
    InvalidToken,
    IdentityMismatch,
    MissingIdentityScope,
    InvalidGrant,
    InvalidRefreshToken,
    TokenExpired,
    RefreshTokenExpired,
    RefreshTokenInvalidated,
    RefreshTokenReused,
    InvalidClient,
    RefreshUnavailable,
    RefreshTooEarly,
    Clock,
}

impl AuthError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Configuration => "chatgpt_configuration",
            Self::ListenerUnavailable => "chatgpt_callback_unavailable",
            Self::CallbackTimeout => "chatgpt_login_timeout",
            Self::InvalidCallback => "chatgpt_invalid_callback",
            Self::InvalidState => "chatgpt_invalid_state",
            Self::AccessDenied => "access_denied",
            Self::AuthorizationFailed => "chatgpt_authorization_failed",
            Self::ClientMismatch => "chatgpt_client_mismatch",
            Self::RegistrationIncomplete => "chatgpt_registration_incomplete",
            Self::RegistrationNotPersisted => "chatgpt_registration_not_persisted",
            Self::Network => "chatgpt_network",
            Self::ServiceUnavailable => "chatgpt_service_unavailable",
            Self::RateLimited => "chatgpt_rate_limited",
            Self::InvalidResponse => "chatgpt_invalid_response",
            Self::InvalidToken => "chatgpt_invalid_token",
            Self::IdentityMismatch => "chatgpt_identity_mismatch",
            Self::MissingIdentityScope => "chatgpt_missing_identity_scope",
            Self::InvalidGrant => "invalid_grant",
            Self::InvalidRefreshToken => "invalid_refresh_token",
            Self::TokenExpired => "token_expired",
            Self::RefreshTokenExpired => "refresh_token_expired",
            Self::RefreshTokenInvalidated => "refresh_token_invalidated",
            Self::RefreshTokenReused => "refresh_token_reused",
            Self::InvalidClient => "invalid_client",
            Self::RefreshUnavailable => "chatgpt_refresh_unavailable",
            Self::RefreshTooEarly => "chatgpt_refresh_too_early",
            Self::Clock => "chatgpt_clock",
        }
    }

    pub fn is_terminal_refresh(&self) -> bool {
        matches!(
            self,
            Self::InvalidGrant
                | Self::InvalidRefreshToken
                | Self::TokenExpired
                | Self::RefreshTokenExpired
                | Self::RefreshTokenInvalidated
                | Self::RefreshTokenReused
        )
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Network | Self::ServiceUnavailable | Self::RateLimited
        )
    }

    pub fn user_message(&self) -> &'static str {
        match self {
            Self::ListenerUnavailable => {
                "The local sign-in callback could not be started. Try again."
            }
            Self::CallbackTimeout => "ChatGPT sign-in timed out. Start sign-in again.",
            Self::AccessDenied => {
                "ChatGPT authorization was declined. Your current account was not changed."
            }
            Self::InvalidState | Self::InvalidCallback => {
                "The sign-in response could not be verified. Start sign-in again."
            }
            Self::ClientMismatch | Self::IdentityMismatch => {
                "The sign-in response does not match the selected ChatGPT account."
            }
            Self::RegistrationIncomplete => {
                "ChatGPT registration did not return an issued client ID. Try signing in again."
            }
            Self::RegistrationNotPersisted => {
                "The ChatGPT registration must be saved before sign-in can finish."
            }
            Self::Network => "ChatGPT could not be reached. Check your connection and try again.",
            Self::ServiceUnavailable => {
                "ChatGPT sign-in is temporarily unavailable. Try again later."
            }
            Self::RateLimited => "ChatGPT sign-in is temporarily rate limited. Try again later.",
            Self::InvalidClient | Self::Configuration => {
                "The ChatGPT sign-in configuration was not accepted."
            }
            Self::RefreshUnavailable => "Sign in to ChatGPT again to renew access.",
            Self::RefreshTooEarly => "ChatGPT access cannot be refreshed yet. Try again later.",
            Self::Clock => "The system clock could not be read. Check your date and time settings.",
            Self::MissingIdentityScope => {
                "ChatGPT did not grant the identity permission needed to finish sign-in."
            }
            error if error.is_terminal_refresh() => {
                "Your ChatGPT session has ended. Sign in again."
            }
            _ => "The ChatGPT sign-in response could not be verified. Try signing in again.",
        }
    }
}

impl fmt::Display for AuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.user_message())
    }
}
impl std::error::Error for AuthError {}

/// A single-use authorization attempt. Dropping the attempt or the future
/// returned by `await_callback` closes its loopback socket and cancels it.
/// Deliberately no Debug: the URL can contain a retained ID token hint.
pub struct LoginAttempt {
    listener: TcpListener,
    authorization_url: String,
    redirect_uri: String,
    state: String,
    nonce: String,
    verifier: String,
    expected_client_id: Option<String>,
    expected_subject: Option<String>,
}

impl LoginAttempt {
    /// Binds only 127.0.0.1 at a fresh OS-selected port, before exposing a URL.
    /// `host_id` must already have been persisted by the caller.
    pub async fn bind(
        host_id: &str,
        previous: Option<Reauthorization<'_>>,
    ) -> Result<Self, AuthError> {
        if !valid_host_id(host_id) {
            return Err(AuthError::Configuration);
        }
        if let Some(previous) = &previous {
            if !valid_client_id(previous.client_id)
                || previous
                    .expected_subject
                    .is_some_and(|subject| !valid_identifier(subject))
                || previous
                    .id_token_hint
                    .is_some_and(|token| !valid_secret(token))
                || previous
                    .login_hint
                    .is_some_and(|email| email.len() > 320 || email.chars().any(char::is_control))
            {
                return Err(AuthError::Configuration);
            }
        }
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| AuthError::ListenerUnavailable)?;
        let address = listener
            .local_addr()
            .map_err(|_| AuthError::ListenerUnavailable)?;
        let redirect_uri = format!("http://127.0.0.1:{}{CALLBACK_PATH}", address.port());
        let state = random_value()?;
        let nonce = random_value()?;
        let verifier = random_value()?;
        let challenge = pkce_challenge(&verifier);
        let mut url = Url::parse(AUTHORIZE_ENDPOINT).map_err(|_| AuthError::Configuration)?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair(
                    "client_id",
                    previous.as_ref().map_or(DYNAMIC_CLIENT_ID, |p| p.client_id),
                )
                .append_pair("ext_agent_host_id", host_id)
                .append_pair("response_type", "code")
                .append_pair("redirect_uri", &redirect_uri)
                .append_pair("scope", REQUESTED_SCOPES)
                .append_pair("resource", RESOURCE)
                .append_pair("state", &state)
                .append_pair("nonce", &nonce)
                .append_pair("code_challenge_method", "S256")
                .append_pair("code_challenge", &challenge);
            match &previous {
                None => {
                    query.append_pair("agent_name_hint", "Handy Phonon");
                }
                Some(previous) => {
                    if let Some(hint) = previous.id_token_hint {
                        query.append_pair("id_token_hint", hint);
                    }
                    if let Some(hint) = previous.login_hint {
                        query.append_pair("login_hint", hint);
                    }
                }
            }
        }
        Ok(Self {
            listener,
            authorization_url: url.into(),
            redirect_uri,
            state,
            nonce,
            verifier,
            expected_client_id: previous.as_ref().map(|p| p.client_id.to_owned()),
            expected_subject: previous.and_then(|p| p.expected_subject.map(str::to_owned)),
        })
    }

    /// Sensitive URL: use only with the system browser, never logs or analytics.
    pub fn authorization_url(&self) -> &str {
        &self.authorization_url
    }

    /// Request consent only for an explicit user action to enable plan usage;
    /// normal sign-in must not repeatedly force consent.
    pub fn request_plan_consent(&mut self) -> Result<(), AuthError> {
        let mut url = Url::parse(&self.authorization_url).map_err(|_| AuthError::Configuration)?;
        if !url.query_pairs().any(|(key, _)| key == "prompt") {
            url.query_pairs_mut().append_pair("prompt", "consent");
        }
        self.authorization_url = url.into();
        Ok(())
    }

    /// Accept one state-bound callback. Malformed requests and requests with an
    /// unrelated state get a generic response and do not consume the attempt.
    /// A valid callback closes the listener before token exchange (no replay).
    pub async fn await_callback(self, limit: Duration) -> Result<PendingExchange, AuthError> {
        let deadline = Instant::now() + limit;
        loop {
            if Instant::now() >= deadline {
                return Err(AuthError::CallbackTimeout);
            }
            let (mut stream, peer) = timeout(
                deadline.saturating_duration_since(Instant::now()),
                self.listener.accept(),
            )
            .await
            .map_err(|_| AuthError::CallbackTimeout)?
            .map_err(|_| AuthError::ListenerUnavailable)?;
            if !peer.ip().is_loopback() {
                continue;
            }
            let read_limit = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(3));
            let request = match timeout(
                read_limit,
                read_callback_request(&mut stream, &self.redirect_uri),
            )
            .await
            {
                Ok(Ok(request)) => request,
                _ => {
                    send_callback_response(&mut stream, false).await;
                    continue;
                }
            };
            let result = parse_callback(&request, &self.state, self.expected_client_id.as_deref());
            match result {
                Err(AuthError::InvalidState | AuthError::InvalidCallback) => {
                    send_callback_response(&mut stream, false).await;
                }
                Err(error) => {
                    send_callback_response(&mut stream, false).await;
                    return Err(error);
                }
                Ok(callback) => {
                    send_callback_response(&mut stream, true).await;
                    return Ok(PendingExchange {
                        client_id: callback.client_id,
                        code: callback.code,
                        redirect_uri: self.redirect_uri,
                        verifier: self.verifier,
                        nonce: self.nonce,
                        expected_subject: self.expected_subject,
                    });
                }
            }
        }
    }
}

/// A validated, single-use code. Persist `client_id()` before exchanging it.
/// No Clone/Debug: ownership prevents accidental code reuse and logging.
pub struct PendingExchange {
    client_id: String,
    code: String,
    redirect_uri: String,
    verifier: String,
    nonce: String,
    expected_subject: Option<String>,
}
impl PendingExchange {
    pub fn client_id(&self) -> &str {
        &self.client_id
    }
}

/// HTTP client restricted to documented OpenAI endpoints. Redirects are disabled
/// so discovery or an HTTP redirect cannot transmit tokens to another origin.
pub struct OAuthClient {
    http: Client,
}

impl OAuthClient {
    pub fn new() -> Result<Self, AuthError> {
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .user_agent("Handy-Phonon")
            .build()
            .map_err(|_| AuthError::Configuration)?;
        Ok(Self { http })
    }

    /// `persisted_client_id` must be read from a successful persistence result,
    /// not merely copied from the callback. Failure keeps the registration so a
    /// new code can use its issued ID rather than creating another registration.
    pub async fn exchange(
        &self,
        pending: PendingExchange,
        persisted_client_id: &str,
    ) -> Result<TokenBundle, AuthError> {
        if pending.client_id != persisted_client_id {
            return Err(AuthError::RegistrationNotPersisted);
        }
        // Fetch validation keys before exchanging a single-use authorization code.
        let jwks = self.jwks().await?;
        let response = self
            .http
            .post(TOKEN_ENDPOINT)
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", pending.client_id.as_str()),
                ("code", pending.code.as_str()),
                ("code_verifier", pending.verifier.as_str()),
                ("redirect_uri", pending.redirect_uri.as_str()),
                ("resource", RESOURCE),
            ])
            .send()
            .await
            .map_err(|_| AuthError::Network)?;
        let received_at = unix_now()?;
        let tokens: TokenResponse = decode_response(response).await?;
        finish_exchange(tokens, pending, &jwks, received_at)
    }

    /// Returns a complete replacement bundle. The caller MUST serialize this
    /// call with other refreshes and atomically persist it before using it.
    /// Never automatically retry an ambiguous network failure with a rotating
    /// refresh token: a request may have succeeded before the connection failed.
    pub async fn refresh(&self, previous: &TokenBundle) -> Result<TokenBundle, AuthError> {
        if !valid_client_id(&previous.client_id) || previous.account.issuer != ISSUER {
            return Err(AuthError::Configuration);
        }
        if previous.refresh_token.is_empty() {
            return Err(AuthError::RefreshUnavailable);
        }
        if !valid_secret(&previous.refresh_token) {
            return Err(AuthError::InvalidToken);
        }
        if !previous.can_refresh(unix_now()?) {
            return Err(AuthError::RefreshTooEarly);
        }
        // Obtain keys first so a transient discovery failure cannot discard a
        // freshly rotated refresh token that we have not yet persisted.
        let jwks = self.jwks().await?;
        let response = self
            .http
            .post(TOKEN_ENDPOINT)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", previous.client_id.as_str()),
                ("refresh_token", previous.refresh_token.as_str()),
                ("resource", RESOURCE),
                // Intentionally no scope: keep the existing grant.
            ])
            .send()
            .await
            .map_err(|_| AuthError::Network)?;
        let received_at = unix_now()?;
        let tokens: TokenResponse = decode_response(response).await?;
        finish_refresh(tokens, previous, &jwks, received_at)
    }

    /// Attempt server-side revocation before the caller clears local tokens.
    /// A failure means remote revocation was NOT confirmed. Local sign-out can
    /// still proceed, retaining only the account/client mapping and host ID.
    pub async fn revoke(&self, previous: &TokenBundle) -> Result<(), AuthError> {
        if previous.refresh_token.is_empty() {
            // No supported renewable-session token is available. Do not claim
            // remote revocation without having received a revocation response.
            return Err(AuthError::RefreshUnavailable);
        }
        if !valid_client_id(&previous.client_id) || !valid_secret(&previous.refresh_token) {
            return Err(AuthError::Configuration);
        }
        let discovery = self.discovery().await?;
        let endpoint = discovery
            .revocation_endpoint
            .ok_or(AuthError::Configuration)?;
        validate_openai_endpoint(&endpoint)?;
        let response = self
            .http
            .post(endpoint)
            .form(&[
                ("token", previous.refresh_token.as_str()),
                ("token_type_hint", "refresh_token"),
                ("client_id", previous.client_id.as_str()),
            ])
            .send()
            .await
            .map_err(|_| AuthError::Network)?;
        if response.status() == StatusCode::OK {
            return Ok(());
        }
        Err(response_error(response).await)
    }

    async fn discovery(&self) -> Result<Discovery, AuthError> {
        let response = self
            .http
            .get(DISCOVERY_ENDPOINT)
            .send()
            .await
            .map_err(|_| AuthError::Network)?;
        let discovery: Discovery = decode_response(response).await?;
        if discovery.issuer != ISSUER {
            return Err(AuthError::Configuration);
        }
        validate_openai_endpoint(&discovery.jwks_uri)?;
        if let Some(endpoint) = &discovery.revocation_endpoint {
            validate_openai_endpoint(endpoint)?;
        }
        Ok(discovery)
    }

    async fn jwks(&self) -> Result<Jwks, AuthError> {
        let discovery = self.discovery().await?;
        let response = self
            .http
            .get(discovery.jwks_uri)
            .send()
            .await
            .map_err(|_| AuthError::Network)?;
        let jwks: Jwks = decode_response(response).await?;
        if jwks.keys.is_empty() || jwks.keys.len() > 100 {
            return Err(AuthError::InvalidResponse);
        }
        Ok(jwks)
    }
}

#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    jwks_uri: String,
    revocation_endpoint: Option<String>,
}
#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}
#[derive(Deserialize)]
struct Jwk {
    kid: Option<String>,
    kty: String,
    alg: Option<String>,
    #[serde(rename = "use")]
    key_use: Option<String>,
    key_ops: Option<Vec<String>>,
    n: Option<String>,
    e: Option<String>,
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    token_type: String,
    expires_in: u64,
    scope: Option<String>,
    earliest_refresh_at: Option<u64>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}
impl Audience {
    fn contains(&self, expected: &str) -> bool {
        match self {
            Self::One(audience) => audience == expected,
            Self::Many(audiences) => audiences.iter().any(|a| a == expected),
        }
    }
    fn multiple(&self) -> bool {
        matches!(self, Self::Many(audiences) if audiences.len() > 1)
    }
}
#[derive(Deserialize)]
struct IdClaims {
    iss: String,
    sub: String,
    aud: Audience,
    exp: u64,
    iat: u64,
    nonce: Option<String>,
    azp: Option<String>,
    client_id: Option<String>,
    email: Option<String>,
    name: Option<String>,
    at_hash: Option<String>,
}

enum NonceCheck<'a> {
    Required(&'a str),
    IfPresent(&'a str),
}

fn validate_id_token(
    token: &str,
    jwks: &Jwks,
    client_id: &str,
    nonce_check: NonceCheck<'_>,
    expected_subject: Option<&str>,
    access_token: &str,
    now: u64,
) -> Result<Account, AuthError> {
    if !valid_secret(token) || !valid_client_id(client_id) {
        return Err(AuthError::InvalidToken);
    }
    let header = decode_header(token).map_err(|_| AuthError::InvalidToken)?;
    if header.alg != Algorithm::RS256 {
        return Err(AuthError::InvalidToken);
    }
    let kid = header.kid.ok_or(AuthError::InvalidToken)?;
    let mut matching = jwks
        .keys
        .iter()
        .filter(|key| key.kid.as_deref() == Some(&kid));
    let key = matching.next().ok_or(AuthError::InvalidToken)?;
    if matching.next().is_some()
        || key.kty != "RSA"
        || key.alg.as_deref().is_some_and(|alg| alg != "RS256")
        || key.key_use.as_deref().is_some_and(|use_| use_ != "sig")
        || key
            .key_ops
            .as_ref()
            .is_some_and(|ops| !ops.iter().any(|op| op == "verify"))
    {
        return Err(AuthError::InvalidToken);
    }
    let decoding_key = DecodingKey::from_rsa_components(
        key.n.as_deref().ok_or(AuthError::InvalidToken)?,
        key.e.as_deref().ok_or(AuthError::InvalidToken)?,
    )
    .map_err(|_| AuthError::InvalidToken)?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[client_id]);
    validation.set_required_spec_claims(&["iss", "sub", "aud", "exp", "iat"]);
    validation.leeway = CLOCK_SKEW_SECONDS;
    validation.validate_nbf = true;
    let claims = decode::<IdClaims>(token, &decoding_key, &validation)
        .map_err(|_| AuthError::InvalidToken)?
        .claims;
    if claims.iss != ISSUER
        || !claims.aud.contains(client_id)
        || !valid_identifier(&claims.sub)
        || claims.exp <= now
        || claims.iat > now.saturating_add(CLOCK_SKEW_SECONDS)
        || claims.exp <= claims.iat
    {
        return Err(AuthError::InvalidToken);
    }
    if claims
        .azp
        .as_deref()
        .is_some_and(|party| party != client_id)
        || (claims.aud.multiple() && claims.azp.as_deref() != Some(client_id))
        || claims
            .client_id
            .as_deref()
            .is_some_and(|id| id != client_id)
    {
        return Err(AuthError::ClientMismatch);
    }
    let (nonce, require_nonce) = match nonce_check {
        NonceCheck::Required(nonce) => (nonce, true),
        NonceCheck::IfPresent(nonce) => (nonce, false),
    };
    if (require_nonce && claims.nonce.is_none())
        || claims
            .nonce
            .as_deref()
            .is_some_and(|actual| !constant_time_equal(actual.as_bytes(), nonce.as_bytes()))
    {
        return Err(AuthError::InvalidToken);
    }
    if expected_subject.is_some_and(|expected| expected != claims.sub) {
        return Err(AuthError::IdentityMismatch);
    }
    if let Some(expected_hash) = claims.at_hash {
        let digest = Sha256::digest(access_token.as_bytes());
        if !constant_time_equal(
            expected_hash.as_bytes(),
            URL_SAFE_NO_PAD.encode(&digest[..16]).as_bytes(),
        ) {
            return Err(AuthError::InvalidToken);
        }
    }
    Ok(Account {
        issuer: claims.iss,
        subject: claims.sub,
        email: sanitized_display(claims.email, 320),
        name: sanitized_display(claims.name, 256),
    })
}

fn finish_exchange(
    tokens: TokenResponse,
    pending: PendingExchange,
    jwks: &Jwks,
    now: u64,
) -> Result<TokenBundle, AuthError> {
    let scopes = validate_token_response(&tokens, None)?;
    let id_token = tokens.id_token.as_deref().ok_or(AuthError::InvalidToken)?;
    let account = validate_id_token(
        id_token,
        jwks,
        &pending.client_id,
        NonceCheck::Required(&pending.nonce),
        pending.expected_subject.as_deref(),
        &tokens.access_token,
        now,
    )?;
    build_bundle(
        tokens,
        pending.client_id,
        account,
        pending.nonce,
        scopes,
        now,
        None,
    )
}

fn finish_refresh(
    tokens: TokenResponse,
    previous: &TokenBundle,
    jwks: &Jwks,
    now: u64,
) -> Result<TokenBundle, AuthError> {
    let scopes = validate_token_response(&tokens, Some(&previous.scopes))?;
    // Token rotation is mandatory on successful refresh; never silently keep an
    // old token (it may already be invalidated server-side).
    if tokens
        .refresh_token
        .as_deref()
        .is_none_or(|token| !valid_secret(token))
    {
        return Err(AuthError::InvalidResponse);
    }
    let account = if let Some(id_token) = &tokens.id_token {
        validate_id_token(
            id_token,
            jwks,
            &previous.client_id,
            NonceCheck::IfPresent(&previous.nonce),
            Some(&previous.account.subject),
            &tokens.access_token,
            now,
        )?
    } else {
        previous.account.clone()
    };
    build_bundle(
        tokens,
        previous.client_id.clone(),
        account,
        previous.nonce.clone(),
        scopes,
        now,
        Some(&previous.id_token),
    )
}

fn validate_token_response(
    tokens: &TokenResponse,
    previous_scopes: Option<&[String]>,
) -> Result<Vec<String>, AuthError> {
    if !tokens.token_type.eq_ignore_ascii_case("Bearer")
        || !valid_secret(&tokens.access_token)
        || tokens.expires_in == 0
        || tokens.expires_in > 24 * 60 * 60
        || tokens
            .id_token
            .as_deref()
            .is_some_and(|token| !valid_secret(token))
        || tokens
            .refresh_token
            .as_deref()
            .is_some_and(|token| !valid_secret(token))
    {
        return Err(AuthError::InvalidResponse);
    }
    let scopes = match &tokens.scope {
        Some(scope) => parse_scopes(scope)?,
        None => previous_scopes.ok_or(AuthError::InvalidResponse)?.to_vec(),
    };
    if !scopes.iter().any(|scope| scope == "openid") {
        return Err(AuthError::MissingIdentityScope);
    }
    if scopes.iter().any(|scope| scope == "offline_access") && tokens.refresh_token.is_none() {
        return Err(AuthError::InvalidResponse);
    }
    Ok(scopes)
}

fn build_bundle(
    tokens: TokenResponse,
    client_id: String,
    account: Account,
    nonce: String,
    scopes: Vec<String>,
    now: u64,
    previous_id_token: Option<&str>,
) -> Result<TokenBundle, AuthError> {
    let expires_at = now
        .checked_add(tokens.expires_in)
        .ok_or(AuthError::InvalidResponse)?;
    Ok(TokenBundle {
        client_id,
        account,
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token.unwrap_or_default(),
        id_token: tokens
            .id_token
            .or_else(|| previous_id_token.map(str::to_owned))
            .ok_or(AuthError::InvalidToken)?,
        token_type: "Bearer".to_owned(),
        expires_at,
        earliest_refresh_at: tokens.earliest_refresh_at,
        scopes,
        saved_at: now,
        nonce,
    })
}

fn parse_scopes(value: &str) -> Result<Vec<String>, AuthError> {
    if value.len() > 4096
        || value.bytes().any(|b| {
            !(b == 0x20 || b == 0x21 || (0x23..=0x5b).contains(&b) || (0x5d..=0x7e).contains(&b))
        })
    {
        return Err(AuthError::InvalidResponse);
    }
    let mut scopes: Vec<String> = value
        .split(' ')
        .filter(|scope| !scope.is_empty())
        .map(str::to_owned)
        .collect();
    scopes.sort_unstable();
    scopes.dedup();
    Ok(scopes)
}

struct Callback {
    code: String,
    client_id: String,
}
fn parse_callback(
    target: &str,
    expected_state: &str,
    expected_client_id: Option<&str>,
) -> Result<Callback, AuthError> {
    if target.len() > MAX_HTTP_HEADERS || target.contains('#') || !target.is_ascii() {
        return Err(AuthError::InvalidCallback);
    }
    let (path, query) = target.split_once('?').ok_or(AuthError::InvalidCallback)?;
    if path != CALLBACK_PATH || !valid_percent_encoding(query) {
        return Err(AuthError::InvalidCallback);
    }
    let url =
        Url::parse(&format!("http://127.0.0.1{target}")).map_err(|_| AuthError::InvalidCallback)?;
    let mut params = HashMap::new();
    for (name, value) in url.query_pairs() {
        if params
            .insert(name.into_owned(), value.into_owned())
            .is_some()
        {
            return Err(AuthError::InvalidCallback);
        }
    }
    let state = params.get("state").ok_or(AuthError::InvalidState)?;
    if !constant_time_equal(state.as_bytes(), expected_state.as_bytes()) {
        return Err(AuthError::InvalidState);
    }
    if let Some(error) = params.get("error") {
        if params.contains_key("code") {
            return Err(AuthError::InvalidCallback);
        }
        return Err(if error == "access_denied" {
            AuthError::AccessDenied
        } else {
            AuthError::AuthorizationFailed
        });
    }
    let code = params
        .remove("code")
        .filter(|code| valid_secret(code))
        .ok_or(AuthError::InvalidCallback)?;
    let returned_client = params.remove("client_id");
    let client_id = match (expected_client_id, returned_client) {
        (Some(expected), Some(returned)) if returned != expected => {
            return Err(AuthError::ClientMismatch)
        }
        (Some(expected), _) => expected.to_owned(),
        (None, Some(returned)) if valid_client_id(&returned) => returned,
        _ => return Err(AuthError::RegistrationIncomplete),
    };
    if !valid_client_id(&client_id) {
        return Err(AuthError::RegistrationIncomplete);
    }
    Ok(Callback { code, client_id })
}

async fn read_callback_request(
    stream: &mut TcpStream,
    redirect_uri: &str,
) -> Result<String, AuthError> {
    let mut bytes = Vec::with_capacity(2048);
    let mut buffer = [0u8; 1024];
    loop {
        let size = stream
            .read(&mut buffer)
            .await
            .map_err(|_| AuthError::InvalidCallback)?;
        if size == 0 {
            return Err(AuthError::InvalidCallback);
        }
        bytes.extend_from_slice(&buffer[..size]);
        if bytes.len() > MAX_HTTP_HEADERS {
            return Err(AuthError::InvalidCallback);
        }
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let request = std::str::from_utf8(&bytes).map_err(|_| AuthError::InvalidCallback)?;
    let mut lines = request.split("\r\n");
    let mut request_line = lines.next().ok_or(AuthError::InvalidCallback)?.split(' ');
    let method = request_line.next();
    let target = request_line.next().ok_or(AuthError::InvalidCallback)?;
    let version = request_line.next();
    if method != Some("GET")
        || !matches!(version, Some("HTTP/1.0" | "HTTP/1.1"))
        || request_line.next().is_some()
    {
        return Err(AuthError::InvalidCallback);
    }
    let expected_host = redirect_uri
        .strip_prefix("http://")
        .and_then(|uri| uri.strip_suffix(CALLBACK_PATH))
        .ok_or(AuthError::Configuration)?;
    let mut host_seen = false;
    for line in lines {
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').ok_or(AuthError::InvalidCallback)?;
        if name.eq_ignore_ascii_case("host") {
            if host_seen || value.trim() != expected_host {
                return Err(AuthError::InvalidCallback);
            }
            host_seen = true;
        }
        if name.eq_ignore_ascii_case("transfer-encoding")
            || (name.eq_ignore_ascii_case("content-length") && value.trim() != "0")
        {
            return Err(AuthError::InvalidCallback);
        }
    }
    if !host_seen {
        return Err(AuthError::InvalidCallback);
    }
    Ok(target.to_owned())
}

async fn send_callback_response(stream: &mut TcpStream, accepted: bool) {
    // Static, non-reflective page: never echo a callback, state, code or error.
    // Acceptance here is not token validation; do not claim sign-in succeeded.
    let (status, body) = if accepted {
        ("200 OK", "Sign-in response received. Return to Handy Phonon to finish signing in. You may close this tab.")
    } else {
        (
            "400 Bad Request",
            "This sign-in response could not be accepted. Return to Handy Phonon.",
        )
    };
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nPragma: no-cache\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nX-Content-Type-Options: nosniff\r\n\r\n{body}", body.len());
    let _ = timeout(
        Duration::from_millis(500),
        stream.write_all(response.as_bytes()),
    )
    .await;
}

fn random_value() -> Result<String, AuthError> {
    let mut bytes = [0u8; 32];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| AuthError::Configuration)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}
fn valid_host_id(value: &str) -> bool {
    value
        .strip_prefix("urn:uuid:")
        .and_then(|uuid| uuid::Uuid::parse_str(uuid).ok())
        .is_some_and(|uuid| !uuid.is_nil())
}
fn valid_client_id(value: &str) -> bool {
    value != DYNAMIC_CLIENT_ID && valid_identifier(value)
}
fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:|".contains(&b))
}
fn valid_secret(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_TOKEN_LEN && value.bytes().all(|b| b.is_ascii_graphic())
}
fn valid_percent_encoding(value: &str) -> bool {
    let mut bytes = value.as_bytes().iter();
    while let Some(byte) = bytes.next() {
        if *byte == b'%'
            && !(bytes.next().is_some_and(u8::is_ascii_hexdigit)
                && bytes.next().is_some_and(u8::is_ascii_hexdigit))
        {
            return false;
        }
    }
    true
}
fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}
fn sanitized_display(value: Option<String>, limit: usize) -> Option<String> {
    value
        .map(|value| {
            value
                .chars()
                .filter(|c| {
                    !c.is_control()
                        && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
                })
                .take(limit)
                .collect::<String>()
                .trim()
                .to_owned()
        })
        .filter(|value| !value.is_empty())
}
fn unix_now() -> Result<u64, AuthError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| AuthError::Clock)
}
fn validate_openai_endpoint(endpoint: &str) -> Result<(), AuthError> {
    let url = Url::parse(endpoint).map_err(|_| AuthError::Configuration)?;
    if url.scheme() != "https"
        || url.host_str() != Some("auth.openai.com")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
    {
        return Err(AuthError::Configuration);
    }
    Ok(())
}
async fn limited_body(mut response: reqwest::Response) -> Result<Vec<u8>, AuthError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_JSON_BODY as u64)
    {
        return Err(AuthError::InvalidResponse);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| AuthError::Network)? {
        if body.len().saturating_add(chunk.len()) > MAX_JSON_BODY {
            return Err(AuthError::InvalidResponse);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
async fn decode_response<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, AuthError> {
    if response.status() != StatusCode::OK {
        return Err(response_error(response).await);
    }
    let body = limited_body(response).await?;
    serde_json::from_slice(&body).map_err(|_| AuthError::InvalidResponse)
}
async fn response_error(response: reqwest::Response) -> AuthError {
    let status = response.status();
    let body = limited_body(response).await.unwrap_or_default();
    classify_error(status, &body)
}
fn classify_error(status: StatusCode, body: &[u8]) -> AuthError {
    // Temporary HTTP failures never invalidate a renewable session, even if an
    // intermediary puts a misleading protocol code in an error response.
    if status == StatusCode::TOO_MANY_REQUESTS {
        return AuthError::RateLimited;
    }
    if status.is_server_error() {
        return AuthError::ServiceUnavailable;
    }
    // Only fixed protocol codes survive this boundary. No arbitrary description,
    // detail, URL or body can accidentally reach logs or frontend error messages.
    let value = serde_json::from_slice::<serde_json::Value>(body).ok();
    let error = value.as_ref().and_then(|value| value.get("error"));
    let code = error.and_then(|error| {
        error
            .as_str()
            .or_else(|| error.get("code").and_then(|code| code.as_str()))
    });
    match code {
        Some("invalid_grant") => AuthError::InvalidGrant,
        Some("invalid_refresh_token") => AuthError::InvalidRefreshToken,
        Some("token_expired") => AuthError::TokenExpired,
        Some("refresh_token_expired") => AuthError::RefreshTokenExpired,
        Some("refresh_token_invalidated") => AuthError::RefreshTokenInvalidated,
        Some("refresh_token_reused") => AuthError::RefreshTokenReused,
        Some("invalid_client") => AuthError::InvalidClient,
        Some("access_denied") => AuthError::AccessDenied,
        _ => AuthError::AuthorizationFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde_json::{json, Value};

    // This fixed test-only key is generated for mock JWT signatures. It has no
    // account, credentials, or access to any service and is never used at runtime.
    const TEST_KEY: &[u8] = br#"-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCFcOQZaWfCEcDL
pPicZ9Gtcs0lnweAlBxg3ke1H0oH/x2QQhaFxskCW0TmzSG3lknGi8+yYufq8Lhm
u2QzQpaSplvmIpTfhuwIA1xTd4lxrkSiBB+N+2/EzzJLbvNx/yi4Jomzzz7C/ErG
EUrWe3AAk5efDNXKkjcXRlLgQF+twwYu8a/v7BJrY6utBGNBxqnENawWERzJhYAJ
WxKGT4W+KU8ELjYmq7Yb905MI6MdhlGzGNammGhN/e/2xYSSpX6kl6pVCUYF+Xh8
E/kQ9IXIguD+218XVvMB+PYjiSSW/abbclaLL4c0qYl8QtVtzh0HcVvOOEc20HM1
2d6NxMqvAgMBAAECggEAC9hZDx5prPL5e7pBrVST6sMhjcDfoBzFph2lHOFRp4MQ
Y0FSkX5zUme6pog4AX1wQBUiEzIvZw4GOGxS+S/kgNEOoE+ainsGEbIGrIwUYch3
5C/cgzR6F+zSiJqpNonRWgNlvtXbOuC9XHalf4OGji6Ly30Or0QqasD901UziQ+B
eBo6SNRVgyzyAF4BeNodkW00yvWIcWVerKKfaSF9fbSfFkp60qNQJJFba89AjItK
5msjTOyXsN+VRLE5GUAhwB+JZi2xRZcP0f4NkKx2N+XGqfxPXih/P9nlcFYt/aFx
ZGxPslADt45PEU7ezLOxPhCgSXqxeL2Q+gL2EyDWYQKBgQC780T5M4Coqijl8ZX8
Q+syi8EMxCnzY7RDyJZLs/hdFzMc6zZVgDyFhUikQbbFd16JyE5q9fJohayaO9I1
YF2+NNcfcCyPhmClV28JBFp1XahVwSx8lrpn6W4pdiURQZw+N/XViPOgAuJIJjmr
jvGlcuHrsV4MiN82iXzYTyxTjwKBgQC1wUQCLiruNbwvC3ROWuFdrHaMhb+r4Eq7
AndemzTUVGcwJKYRI32y+ysF9c0cf1PrqXxCA7DdBRlJv65tsXHuPBPSQsmBvu1+
oPyHBVydnTfEIMt+nLFXABzn6AAlvGvnKMp/cOtwOTErkgR+iIX/2lu3ZXvcwNzl
plC+P4YG4QKBgQCM2ry3MeTbAmMSKOJpoxDx2ZC9G4oA8JjZL8uLQn4AbfGNW61l
mGxC+Gc/SkxKYrJD+gzi1h1sPbnkAK8B941pjbomwm9yxJdLcmIxVMTiLmWIlvPb
Dy71zxgTFIqlCxGoA1JGTJOgOGkS/yq7Kq5oetdbRpqgNDdsbM9WYMdsewKBgDaX
v3q8LU7xuv2SfjPO3mSJme4pemIA89FqMzqqedrRI1F1oKADPg1Vnh2jMCHAKQ/f
D6CwhR5OGsNpHNZ79xGs3/NG9knPdHyVlGRl+uSoxYhWpWj5XdcZBJWvvOOYzfxX
50MSQtWpiBhjOpBbJ4yrJONYSzUKhQ9Bvnz2jaZhAoGAcAasGgvMeYXLMOvZ4bTY
tw6h9/ab0b73/vjZm2CB3YCbNWGrVUcodurzkPOZVjDjJvPG8P+7wb9YQSYBnOx7
owLwbVI/4stkV4Yhy0Vt3C0WzasqeTAxKEL1/iwDKbzuKfg7GMDf02gRu6GgCa4S
f0Y9cYZPgq8Z3jFRgrsTyZ8=
-----END PRIVATE KEY-----
"#;
    const TEST_N: &str = "hXDkGWlnwhHAy6T4nGfRrXLNJZ8HgJQcYN5HtR9KB_8dkEIWhcbJAltE5s0ht5ZJxovPsmLn6vC4ZrtkM0KWkqZb5iKU34bsCANcU3eJca5EogQfjftvxM8yS27zcf8ouCaJs88-wvxKxhFK1ntwAJOXnwzVypI3F0ZS4EBfrcMGLvGv7-wSa2OrrQRjQcapxDWsFhEcyYWACVsShk-FvilPBC42Jqu2G_dOTCOjHYZRsxjWpphoTf3v9sWEkqV-pJeqVQlGBfl4fBP5EPSFyILg_ttfF1bzAfj2I4kklv2m23JWiy-HNKmJfELVbc4dB3FbzjhHNtBzNdnejcTKrw";
    const TEST_E: &str = "AQAB";

    fn jwks() -> Jwks {
        serde_json::from_value(
            json!({"keys": [{"kid":"test-key", "kty":"RSA", "alg":"RS256",
            "use":"sig", "key_ops":["verify"], "n":TEST_N, "e":TEST_E}]}),
        )
        .unwrap()
    }
    fn claims() -> Value {
        let now = unix_now().unwrap();
        json!({"iss":ISSUER, "sub":"test-subject", "aud":"oaiapp_test", "iat":now - 5,
            "exp":now + 3600, "nonce":"test-nonce", "email":"test@example.invalid", "name":"Test"})
    }
    fn sign(claims: &Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test-key".to_owned());
        encode(
            &header,
            claims,
            &EncodingKey::from_rsa_pem(TEST_KEY).unwrap(),
        )
        .unwrap()
    }
    fn validate(claims: &Value) -> Result<Account, AuthError> {
        validate_id_token(
            &sign(claims),
            &jwks(),
            "oaiapp_test",
            NonceCheck::Required("test-nonce"),
            Some("test-subject"),
            "test-access-token",
            unix_now().unwrap(),
        )
    }
    fn token_response() -> TokenResponse {
        TokenResponse {
            access_token: "test-access-token".to_owned(),
            refresh_token: Some("test-refresh-token".to_owned()),
            id_token: Some(sign(&claims())),
            token_type: "Bearer".to_owned(),
            expires_in: 3600,
            scope: Some(REQUESTED_SCOPES.to_owned()),
            earliest_refresh_at: None,
        }
    }
    fn pending() -> PendingExchange {
        PendingExchange {
            client_id: "oaiapp_test".to_owned(),
            code: "test-code".to_owned(),
            redirect_uri: "http://127.0.0.1:12345/auth/callback".to_owned(),
            verifier: "test-verifier".to_owned(),
            nonce: "test-nonce".to_owned(),
            expected_subject: Some("test-subject".to_owned()),
        }
    }
    fn bundle() -> TokenBundle {
        finish_exchange(token_response(), pending(), &jwks(), unix_now().unwrap()).unwrap()
    }
    fn callback(query: &str) -> Result<Callback, AuthError> {
        parse_callback(&format!("/auth/callback?{query}"), "correct-state", None)
    }

    #[test]
    fn pkce_matches_rfc7636_example() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn host_id_is_unique_opaque_uuid() {
        let first = new_host_id();
        assert!(valid_host_id(&first));
        assert!(first.starts_with("urn:uuid:"));
        assert_ne!(first, new_host_id());
        assert!(!valid_host_id("my-computer"));
        assert!(!valid_host_id(
            "urn:uuid:00000000-0000-0000-0000-000000000000"
        ));
    }

    #[tokio::test]
    async fn initial_authorization_binds_listener_and_uses_dynamic_registration() {
        let host = new_host_id();
        let attempt = LoginAttempt::bind(&host, None).await.unwrap();
        let url = Url::parse(attempt.authorization_url()).unwrap();
        let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(params["client_id"], "dynamic_agent_client");
        assert_eq!(params["agent_name_hint"], "Handy Phonon");
        assert_eq!(params["ext_agent_host_id"], host);
        assert_eq!(params["resource"], RESOURCE);
        assert_eq!(params["scope"], REQUESTED_SCOPES);
        assert_eq!(params["code_challenge_method"], "S256");
        assert_eq!(params["code_challenge"], pkce_challenge(&attempt.verifier));
        assert_eq!(params["redirect_uri"], attempt.redirect_uri);
        assert_eq!(attempt.state.len(), 43);
        assert_eq!(attempt.nonce.len(), 43);
        assert_eq!(attempt.verifier.len(), 43);
        assert_ne!(attempt.state, attempt.nonce);
        assert_ne!(attempt.nonce, attempt.verifier);
        let address = attempt.listener.local_addr().unwrap();
        assert_eq!(address.ip(), std::net::Ipv4Addr::LOCALHOST);
        assert!(TcpListener::bind(address).await.is_err());
    }

    #[tokio::test]
    async fn reauthorization_keeps_registration_and_encodes_only_associated_hints() {
        let mut attempt = LoginAttempt::bind(
            &new_host_id(),
            Some(Reauthorization {
                client_id: "oaiapp_test",
                expected_subject: Some("test-subject"),
                id_token_hint: Some("expired.mock.token"),
                login_hint: Some("test+tag@example.invalid"),
            }),
        )
        .await
        .unwrap();
        let url = Url::parse(attempt.authorization_url()).unwrap();
        let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(params["client_id"], "oaiapp_test");
        assert!(!params.contains_key("agent_name_hint"));
        assert_eq!(params["id_token_hint"], "expired.mock.token");
        assert_eq!(params["login_hint"], "test+tag@example.invalid");
        assert!(!params.contains_key("prompt"));
        attempt.request_plan_consent().unwrap();
        attempt.request_plan_consent().unwrap();
        assert_eq!(
            Url::parse(attempt.authorization_url())
                .unwrap()
                .query_pairs()
                .filter(|(key, value)| key == "prompt" && value == "consent")
                .count(),
            1
        );
    }

    #[test]
    fn callback_requires_bound_state_even_for_denial() {
        assert!(matches!(
            callback("code=test&client_id=oaiapp_test"),
            Err(AuthError::InvalidState)
        ));
        assert!(matches!(
            callback("state=wrong&error=access_denied"),
            Err(AuthError::InvalidState)
        ));
        assert!(matches!(
            callback("state=correct-state&error=access_denied"),
            Err(AuthError::AccessDenied)
        ));
    }

    #[test]
    fn callback_rejects_duplicate_encoded_and_ambiguous_parameters() {
        for query in [
            "state=correct-state&state=correct-state&code=test&client_id=oaiapp_test",
            "state=correct-state&st%61te=correct-state&code=test&client_id=oaiapp_test",
            "state=correct-state&code=test&code=second&client_id=oaiapp_test",
            "state=correct-state&code=test&client_id=oaiapp_test&client_id=oaiapp_second",
            "state=correct-state&error=access_denied&code=test",
            "state=correct-state&code=test%xx&client_id=oaiapp_test",
            "state=correct-state&code=test#fragment&client_id=oaiapp_test",
            "state=correct-state&code=test%0A&client_id=oaiapp_test",
        ] {
            assert!(matches!(callback(query), Err(AuthError::InvalidCallback)));
        }
    }

    #[test]
    fn callback_uses_new_issued_id_and_never_dynamic_placeholder() {
        let result =
            callback("state=correct-state&code=test&client_id=oaiapp_test&scope=openid").unwrap();
        assert_eq!(result.client_id, "oaiapp_test");
        assert!(matches!(
            callback("state=correct-state&code=test"),
            Err(AuthError::RegistrationIncomplete)
        ));
        assert!(matches!(
            callback("state=correct-state&code=test&client_id=dynamic_agent_client"),
            Err(AuthError::RegistrationIncomplete)
        ));
    }

    #[test]
    fn returning_callback_cannot_replace_registered_client() {
        let base = "/auth/callback?state=correct-state&code=test";
        assert_eq!(
            parse_callback(base, "correct-state", Some("oaiapp_test"))
                .unwrap()
                .client_id,
            "oaiapp_test"
        );
        assert!(matches!(
            parse_callback(
                &format!("{base}&client_id=oaiapp_other"),
                "correct-state",
                Some("oaiapp_test")
            ),
            Err(AuthError::ClientMismatch)
        ));
        for path in [
            "/callback",
            "/auth/../auth/callback",
            "//auth/callback",
            "/auth/%63allback",
        ] {
            assert!(parse_callback(
                &format!("{path}?state=correct-state&code=test"),
                "correct-state",
                Some("oaiapp_test")
            )
            .is_err());
        }
    }

    #[tokio::test]
    async fn callback_is_one_shot_and_response_never_reflects_secrets() {
        let attempt = LoginAttempt::bind(&new_host_id(), None).await.unwrap();
        let redirect = attempt.redirect_uri.clone();
        let state = attempt.state.clone();
        let address = attempt.listener.local_addr().unwrap();
        let task = tokio::spawn(attempt.await_callback(Duration::from_secs(3)));
        let client = Client::builder().no_proxy().build().unwrap();
        let response = client
            .get(&redirect)
            .query(&[
                ("state", "unrelated"),
                ("code", "secret-code"),
                ("client_id", "oaiapp_test"),
            ])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(!response.text().await.unwrap().contains("secret-code"));
        let response = client
            .get(&redirect)
            .query(&[
                ("state", state.as_str()),
                ("code", "secret-code"),
                ("client_id", "oaiapp_test"),
            ])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let body = response.text().await.unwrap();
        assert!(!body.contains(&state));
        assert!(!body.contains("secret-code"));
        assert_eq!(task.await.unwrap().unwrap().client_id(), "oaiapp_test");
        assert!(TcpStream::connect(address).await.is_err());
    }

    #[tokio::test]
    async fn callback_timeout_or_cancellation_closes_listener() {
        let attempt = LoginAttempt::bind(&new_host_id(), None).await.unwrap();
        let address = attempt.listener.local_addr().unwrap();
        assert!(matches!(
            attempt.await_callback(Duration::from_millis(10)).await,
            Err(AuthError::CallbackTimeout)
        ));
        assert!(TcpStream::connect(address).await.is_err());
        let attempt = LoginAttempt::bind(&new_host_id(), None).await.unwrap();
        let address = attempt.listener.local_addr().unwrap();
        let future = attempt.await_callback(Duration::from_secs(300));
        drop(future);
        assert!(TcpStream::connect(address).await.is_err());
    }

    #[tokio::test]
    async fn wrong_persisted_client_is_rejected_before_any_network_request() {
        let result = OAuthClient::new()
            .unwrap()
            .exchange(pending(), "oaiapp_other")
            .await;
        assert!(matches!(result, Err(AuthError::RegistrationNotPersisted)));
    }

    #[test]
    fn valid_rs256_id_token_produces_display_identity() {
        let account = validate(&claims()).unwrap();
        assert_eq!(account.subject, "test-subject");
        assert_eq!(account.email.as_deref(), Some("test@example.invalid"));
    }

    #[test]
    fn jwt_requires_signature_issuer_audience_expiry_nonce_and_subject() {
        for (field, value) in [
            ("iss", json!("https://untrusted.invalid")),
            ("aud", json!("oaiapp_other")),
            ("exp", json!(unix_now().unwrap() - 100)),
            ("iat", json!(unix_now().unwrap() + 3600)),
            ("nonce", json!("other-nonce")),
        ] {
            let mut value_claims = claims();
            value_claims[field] = value;
            assert!(matches!(
                validate(&value_claims),
                Err(AuthError::InvalidToken)
            ));
        }
        let mut changed = claims();
        changed["sub"] = json!("other-subject");
        assert!(matches!(
            validate(&changed),
            Err(AuthError::IdentityMismatch)
        ));
        for field in ["iss", "sub", "aud", "exp", "iat", "nonce"] {
            let mut missing = claims();
            missing.as_object_mut().unwrap().remove(field);
            assert!(validate(&missing).is_err());
        }
        let token = sign(&claims());
        let mut parts: Vec<String> = token.split('.').map(str::to_owned).collect();
        let mut forged = claims();
        forged["sub"] = json!("forged-subject");
        parts[1] = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&forged).unwrap());
        assert!(matches!(
            validate_id_token(
                &parts.join("."),
                &jwks(),
                "oaiapp_test",
                NonceCheck::Required("test-nonce"),
                None,
                "test-access-token",
                unix_now().unwrap()
            ),
            Err(AuthError::InvalidToken)
        ));
    }

    #[test]
    fn jwt_rejects_algorithm_confusion_unknown_keys_and_duplicate_keys() {
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims(),
            &EncodingKey::from_secret(TEST_N.as_bytes()),
        )
        .unwrap();
        assert!(matches!(
            validate_id_token(
                &token,
                &jwks(),
                "oaiapp_test",
                NonceCheck::Required("test-nonce"),
                None,
                "test-access-token",
                unix_now().unwrap()
            ),
            Err(AuthError::InvalidToken)
        ));
        let mut keys = jwks();
        keys.keys[0].kid = Some("wrong-key".to_owned());
        assert!(validate_id_token(
            &sign(&claims()),
            &keys,
            "oaiapp_test",
            NonceCheck::Required("test-nonce"),
            None,
            "test-access-token",
            unix_now().unwrap()
        )
        .is_err());
        let mut keys = jwks();
        keys.keys.extend(jwks().keys);
        assert!(validate_id_token(
            &sign(&claims()),
            &keys,
            "oaiapp_test",
            NonceCheck::Required("test-nonce"),
            None,
            "test-access-token",
            unix_now().unwrap()
        )
        .is_err());
    }

    #[test]
    fn jwt_checks_authorized_party_client_binding_and_access_token_hash() {
        let mut values = claims();
        values["client_id"] = json!("oaiapp_other");
        assert!(matches!(validate(&values), Err(AuthError::ClientMismatch)));
        let mut values = claims();
        values["azp"] = json!("oaiapp_other");
        assert!(matches!(validate(&values), Err(AuthError::ClientMismatch)));
        let mut values = claims();
        values["aud"] = json!(["oaiapp_test", "another-audience"]);
        assert!(matches!(validate(&values), Err(AuthError::ClientMismatch)));
        values["azp"] = json!("oaiapp_test");
        assert!(validate(&values).is_ok());
        let mut values = claims();
        values["at_hash"] = json!("wrong-access-token");
        assert!(matches!(validate(&values), Err(AuthError::InvalidToken)));
        values["at_hash"] =
            json!(URL_SAFE_NO_PAD.encode(&Sha256::digest(b"test-access-token")[..16]));
        assert!(validate(&values).is_ok());
    }

    #[test]
    fn identity_only_grant_is_retained_without_enabling_inference() {
        let mut response = token_response();
        response.scope = Some("openid profile email".to_owned());
        response.refresh_token = None;
        let tokens = finish_exchange(response, pending(), &jwks(), unix_now().unwrap()).unwrap();
        assert!(!tokens.has_plan_scope());
        assert!(!tokens.can_refresh(unix_now().unwrap()));
        assert_eq!(tokens.account.subject, "test-subject");
        let complete = bundle();
        assert!(complete.has_plan_scope());
        assert!(!complete.needs_refresh(unix_now().unwrap(), 60));
        assert!(complete.needs_refresh(complete.expires_at, 0));
    }

    #[test]
    fn token_envelope_validates_type_scopes_and_lifetimes() {
        let mut response = token_response();
        response.token_type = "MAC".to_owned();
        assert!(matches!(
            validate_token_response(&response, None),
            Err(AuthError::InvalidResponse)
        ));
        let mut response = token_response();
        response.scope = Some("openid\nchatgpt.tokens.use.direct".to_owned());
        assert!(matches!(
            validate_token_response(&response, None),
            Err(AuthError::InvalidResponse)
        ));
        let mut response = token_response();
        response.scope = Some("chatgpt.tokens.use.direct".to_owned());
        assert!(matches!(
            validate_token_response(&response, None),
            Err(AuthError::MissingIdentityScope)
        ));
        let mut response = token_response();
        response.scope = None;
        assert!(matches!(
            validate_token_response(&response, None),
            Err(AuthError::InvalidResponse)
        ));
        let mut response = token_response();
        response.expires_in = 0;
        assert!(matches!(
            validate_token_response(&response, None),
            Err(AuthError::InvalidResponse)
        ));
        let mut response = token_response();
        response.refresh_token = None;
        assert!(matches!(
            validate_token_response(&response, None),
            Err(AuthError::InvalidResponse)
        ));
    }

    #[test]
    fn refresh_rotates_complete_bundle_and_can_retain_omitted_scope_and_id_token() {
        let previous = bundle();
        let mut response = token_response();
        response.scope = None;
        response.id_token = None;
        response.access_token = "replacement-access-token".to_owned();
        response.refresh_token = Some("replacement-refresh-token".to_owned());
        response.earliest_refresh_at = Some(unix_now().unwrap() + 100);
        let updated = finish_refresh(response, &previous, &jwks(), unix_now().unwrap()).unwrap();
        assert_eq!(updated.access_token, "replacement-access-token");
        assert_eq!(updated.refresh_token, "replacement-refresh-token");
        assert_eq!(updated.id_token, previous.id_token);
        assert_eq!(updated.scopes, previous.scopes);
        assert_eq!(updated.client_id, previous.client_id);
        assert!(!updated.can_refresh(unix_now().unwrap()));
    }

    #[test]
    fn refresh_never_reuses_missing_rotation_or_accepts_changed_identity() {
        let previous = bundle();
        let mut response = token_response();
        response.refresh_token = None;
        assert!(matches!(
            finish_refresh(response, &previous, &jwks(), unix_now().unwrap()),
            Err(AuthError::InvalidResponse)
        ));
        let mut response = token_response();
        let mut changed = claims();
        changed["sub"] = json!("other-subject");
        response.id_token = Some(sign(&changed));
        assert!(matches!(
            finish_refresh(response, &previous, &jwks(), unix_now().unwrap()),
            Err(AuthError::IdentityMismatch)
        ));
        let mut response = token_response();
        let mut changed = claims();
        changed["nonce"] = json!("other-nonce");
        response.id_token = Some(sign(&changed));
        assert!(matches!(
            finish_refresh(response, &previous, &jwks(), unix_now().unwrap()),
            Err(AuthError::InvalidToken)
        ));
        let mut response = token_response();
        let mut changed = claims();
        changed.as_object_mut().unwrap().remove("nonce");
        response.id_token = Some(sign(&changed));
        assert!(finish_refresh(response, &previous, &jwks(), unix_now().unwrap()).is_ok());
    }

    #[test]
    fn discovery_cannot_redirect_credentials_to_another_origin() {
        assert!(validate_openai_endpoint("https://auth.openai.com/.well-known/jwks.json").is_ok());
        for endpoint in [
            "http://auth.openai.com/revoke",
            "https://auth.openai.com.evil.invalid/revoke",
            "https://evil.invalid/revoke",
            "https://auth.openai.com:444/revoke",
            "https://user@auth.openai.com/revoke",
            "https://auth.openai.com/revoke#fragment",
            "https://auth.openai.com/revoke?token=x",
            "http://127.0.0.1/revoke",
        ] {
            assert!(matches!(
                validate_openai_endpoint(endpoint),
                Err(AuthError::Configuration)
            ));
        }
    }

    #[test]
    fn errors_are_allowlisted_without_body_or_secret_leaks() {
        let body = br#"{"error":{"code":"not-a-known-code","message":"secret-access-token"},"detail":"private"}"#;
        let error = classify_error(StatusCode::BAD_REQUEST, body);
        assert_eq!(error, AuthError::AuthorizationFailed);
        assert!(!error.to_string().contains("secret"));
        assert!(!error.code().contains("not-a-known-code"));
        assert!(!format!("{error:?}").contains("secret"));
        assert!(
            classify_error(StatusCode::SERVICE_UNAVAILABLE, b"private error body").is_retryable()
        );
        assert!(
            !classify_error(StatusCode::SERVICE_UNAVAILABLE, b"private error body")
                .is_terminal_refresh()
        );
        for code in [
            "invalid_grant",
            "invalid_refresh_token",
            "token_expired",
            "refresh_token_expired",
            "refresh_token_invalidated",
            "refresh_token_reused",
        ] {
            let body = serde_json::to_vec(&json!({"error":code})).unwrap();
            assert!(classify_error(StatusCode::BAD_REQUEST, &body).is_terminal_refresh());
        }
        assert!(!AuthError::InvalidClient.is_terminal_refresh());
        assert!(!classify_error(
            StatusCode::SERVICE_UNAVAILABLE,
            br#"{"error":"invalid_grant"}"#
        )
        .is_terminal_refresh());
        assert!(!classify_error(
            StatusCode::TOO_MANY_REQUESTS,
            br#"{"error":"invalid_grant"}"#
        )
        .is_terminal_refresh());
    }

    #[test]
    fn display_fields_strip_controls_and_tokens_remain_separate() {
        let mut values = claims();
        values["name"] = json!("Test\n\u{202e} Name");
        let account = validate(&values).unwrap();
        assert_eq!(account.name.as_deref(), Some("Test Name"));
        let public = serde_json::to_string(&account).unwrap();
        assert!(!public.contains("access_token"));
        assert!(!public.contains("refresh_token"));
        assert!(!public.contains("id_token"));
    }
}
