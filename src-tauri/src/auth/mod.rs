use crate::{
    secret_store,
    storage::{AuthSession, SavingsReportEvent, Storage},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::process::Command;
use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex as StdMutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::async_runtime::Mutex as AsyncMutex;
use tauri::{
    webview::NewWindowResponse, AppHandle, Emitter, Manager, State, WebviewUrl,
    WebviewWindowBuilder, WindowEvent,
};
use tokio::sync::Notify;
use tokio::time::Instant as TokioInstant;
use url::Url;

pub(crate) mod profiles;

const SERVICE_NAME: &str = "Halv";
const LEGACY_SERVICE_NAME: &str = "TokenSaver";
const KEYCHAIN_CHATGPT_ACCOUNT: &str = "chatgpt-app-tokens";
const KEYCHAIN_API_KEY_ACCOUNT: &str = "openai-api-key";
const KEYCHAIN_ANTHROPIC_API_KEY_ACCOUNT: &str = "anthropic-api-key";

const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const DEFAULT_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEFAULT_ORIGINATOR: &str = "codex_cli_rs";
const SCOPE: &str = "openid profile email offline_access api.connectors.read api.connectors.invoke";

const ENV_CLIENT_ID_OVERRIDE: &str = "CODEX_APP_SERVER_LOGIN_CLIENT_ID";
const ENV_REFRESH_URL_OVERRIDE: &str = "CODEX_REFRESH_TOKEN_URL_OVERRIDE";
const ENV_ORIGINATOR_OVERRIDE: &str = "CODEX_INTERNAL_ORIGINATOR_OVERRIDE";

const DEFAULT_CALLBACK_PORT: u16 = 1455;
const FALLBACK_CALLBACK_PORT: u16 = 1457;
const CALLBACK_PATH: &str = "/auth/callback";
const BIND_RETRIES: usize = 10;
const BIND_RETRY_DELAY: Duration = Duration::from_millis(200);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const CALLBACK_POLL_INTERVAL: Duration = Duration::from_millis(100);
const ACCOUNT_CALLBACK_READ_TIMEOUT: Duration = Duration::from_millis(500);
const ACCESS_REFRESH_WINDOW: ChronoDuration = ChronoDuration::minutes(5);
const ACCESS_REFRESH_FALLBACK_AGE: ChronoDuration = ChronoDuration::days(8);

const OPENAI_AUTH_CLAIM: &str = "https://api.openai.com/auth";
const OPENAI_PROFILE_CLAIM: &str = "https://api.openai.com/profile";
const ENV_HALV_API_URL: &str = "HALV_API_URL";
#[cfg(debug_assertions)]
const ENV_HALV_BYPASS_ACCOUNT: &str = "HALV_BYPASS_ACCOUNT";
#[cfg(debug_assertions)]
const DEFAULT_HALV_API_URL: &str = "http://127.0.0.1:8787";
// Production API host; change this single value if the deployed domain differs.
#[cfg(not(debug_assertions))]
const DEFAULT_HALV_API_URL: &str = "https://api.halv.ai";
const HALV_API_TIMEOUT: Duration = Duration::from_secs(10);
const OAUTH_PROVIDER_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);
// The server accepts 500 events and 64 KiB per request. 200 events keep the
// body far below the byte limit, also with the optional dollar fields.
const SAVINGS_REPORT_BATCH_SIZE: usize = 200;
const SAVINGS_REPORT_IDLE_INTERVAL: Duration = Duration::from_secs(30);
const SAVINGS_REPORT_STATE_INTERVAL: Duration = Duration::from_secs(2);
const SAVINGS_REPORT_INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const SAVINGS_REPORT_MAX_BACKOFF: Duration = Duration::from_secs(60);
const BILLING_CHECKOUT_EVENT: &str = "billing-checkout-event";
const BILLING_CHECKOUT_WINDOW_LABEL: &str = "halv-checkout";
// The web site hosts the Turnstile widget on an https page. Turnstile accepts
// only hostnames that the widget allows, and the app webview origins
// (tauri://localhost, http://tauri.localhost) are not such hostnames. The
// page runs in its own window without IPC access, so the main webview does not
// load third-party script.
const EMAIL_CHALLENGE_PAGE_URL: &str = "https://halv.ai/auth/challenge";
// After the widget passes, the page goes to this path with the token in the
// `token` query parameter. The window stops that navigation and keeps the
// token.
const EMAIL_CHALLENGE_DONE_PATH: &str = "/auth/challenge/done";
const EMAIL_CHALLENGE_WINDOW_LABEL: &str = "halv-email-challenge";
// A Turnstile token is valid for 5 minutes.
const EMAIL_CHALLENGE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const TURNSTILE_HOST: &str = "challenges.cloudflare.com";
const TURNSTILE_SITE_KEY_MAX_BYTES: usize = 128;
const TURNSTILE_TOKEN_MAX_BYTES: usize = 2048;
// The server answers with these codes when a code request needs a new
// Turnstile challenge.
const CHALLENGE_REQUIRED_CODES: [&str; 2] = ["challenge_required", "challenge_failed"];

static LOGIN_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static ACCOUNT_LOGIN_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static SAVINGS_REPORTER_STARTED: AtomicBool = AtomicBool::new(false);
static ACTIVE_ACCOUNT_OAUTH_CANCEL: StdMutex<Option<Arc<AtomicBool>>> = StdMutex::new(None);
static ACTIVE_CHATGPT_OAUTH_CANCEL: StdMutex<Option<Arc<AtomicBool>>> = StdMutex::new(None);
#[cfg(debug_assertions)]
static ACCOUNT_BYPASS_LOGGED: AtomicBool = AtomicBool::new(false);

type AuthResult<T> = Result<T, AuthError>;

#[derive(Debug)]
enum AuthError {
    Cancelled,
    Message(String),
    PermanentRefresh {
        message: String,
        failed_refresh_token: String,
    },
    TransientRefresh(String),
}

impl AuthError {
    fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    fn permanent_refresh(
        message: impl Into<String>,
        failed_refresh_token: impl Into<String>,
    ) -> Self {
        Self::PermanentRefresh {
            message: message.into(),
            failed_refresh_token: failed_refresh_token.into(),
        }
    }

    fn transient_refresh(message: impl Into<String>) -> Self {
        Self::TransientRefresh(message.into())
    }
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::Cancelled => formatter.write_str("Sign-in cancelled."),
            AuthError::Message(message) | AuthError::TransientRefresh(message) => {
                formatter.write_str(message)
            }
            AuthError::PermanentRefresh { message, .. } => formatter.write_str(message),
        }
    }
}

impl std::error::Error for AuthError {}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum CredentialAuthStatus {
    SignedOut {},
    Chatgpt {
        source: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        email: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        plan_type: Option<String>,
        account_id: String,
        access_token_valid: bool,
    },
    ApiKey {
        masked: String,
    },
    AnthropicApiKey {
        source: String,
        masked: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnthropicCredentialStatus {
    pub configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub masked: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct AuthBearer {
    pub token: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    pub is_api_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AppTokens {
    id_token: String,
    access_token: String,
    refresh_token: String,
    account_id: Option<String>,
    last_refresh: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct CodexAuthJson {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auth_mode: Option<String>,
    #[serde(rename = "OPENAI_API_KEY", default)]
    openai_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tokens: Option<CodexTokens>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_refresh: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_identity: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    personal_access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bedrock_api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct CodexTokens {
    id_token: String,
    access_token: String,
    refresh_token: String,
    account_id: Option<String>,
}

#[derive(Debug, Clone)]
struct BorrowedCodexTokens {
    id_token: String,
    access_token: String,
    account_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct IdTokenClaims {
    account_id: Option<String>,
    email: Option<String>,
    plan_type: Option<String>,
}

#[derive(Debug, Clone)]
struct PkceFlow {
    verifier: String,
    challenge: String,
    state: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HalvOAuthProvider {
    Google,
    Apple,
}

impl HalvOAuthProvider {
    fn parse(provider: &str) -> Result<Self, String> {
        match provider.trim().to_ascii_lowercase().as_str() {
            "google" => Ok(Self::Google),
            "apple" => Ok(Self::Apple),
            _ => Err("Unsupported sign-in provider. Choose Google or Apple.".to_string()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::Apple => "apple",
        }
    }
}

#[derive(Debug, Serialize)]
struct AuthCodeExchangeRequest<'a> {
    grant_type: &'static str,
    code: &'a str,
    redirect_uri: &'a str,
    client_id: &'a str,
    code_verifier: &'a str,
}

#[derive(Debug, Deserialize)]
struct AuthCodeExchangeResponse {
    id_token: String,
    access_token: String,
    refresh_token: String,
}

#[derive(Debug, Serialize)]
struct RefreshRequest<'a> {
    client_id: &'a str,
    grant_type: &'static str,
    refresh_token: &'a str,
}

impl<'a> RefreshRequest<'a> {
    fn new(client_id: &'a str, refresh_token: &'a str) -> Self {
        Self {
            client_id,
            grant_type: "refresh_token",
            refresh_token,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshFailureKind {
    Permanent,
    Transient,
}

struct CallbackServer {
    receiver: mpsc::Receiver<Result<String, String>>,
    cancel: Arc<AtomicBool>,
    flow: CallbackFlow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallbackFlow {
    Chatgpt,
    HalvAccount,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CallbackError {
    Provider(String),
    MissingCode,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SavingsSummary {
    #[serde(default)]
    pub period: String,
    #[serde(default, alias = "tokens_saved")]
    pub tokens_saved: i64,
    #[serde(default, alias = "cap_tokens")]
    pub cap_tokens: Option<u64>,
    #[serde(default)]
    pub exhausted: bool,
    #[serde(
        default,
        alias = "all_time_tokens_saved",
        skip_serializing_if = "Option::is_none"
    )]
    pub all_time_tokens_saved: Option<i64>,
    #[serde(
        default,
        alias = "all_time_savings_usd",
        skip_serializing_if = "Option::is_none"
    )]
    pub all_time_savings_usd: Option<f64>,
    /// Dollars saved with the methods of ADR 0015, from servers that know
    /// them. Old servers leave these fields out.
    #[serde(
        default,
        alias = "all_time_v2_usd",
        skip_serializing_if = "Option::is_none"
    )]
    pub all_time_v2_usd: Option<f64>,
    #[serde(
        default,
        alias = "period_v2_usd",
        skip_serializing_if = "Option::is_none"
    )]
    pub period_v2_usd: Option<f64>,
    /// UNIX seconds of the first event with a dollar method.
    #[serde(default, alias = "v2_since", skip_serializing_if = "Option::is_none")]
    pub v2_since: Option<i64>,
    #[serde(
        default,
        alias = "all_time_routing_net_usd",
        skip_serializing_if = "Option::is_none"
    )]
    pub all_time_routing_net_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entitlements {
    #[serde(default, alias = "trial_confirmed")]
    pub trial_confirmed: bool,
    #[serde(default, alias = "payment_confirmed")]
    pub payment_confirmed: bool,
    /// Unix seconds when the current free trial ends (ADR 0018).
    #[serde(default, alias = "trial_end", skip_serializing_if = "Option::is_none")]
    pub trial_end: Option<i64>,
    pub beta: bool,
    #[serde(alias = "billing_required")]
    pub billing_required: bool,
    #[serde(alias = "can_use_app")]
    pub can_use_app: bool,
    #[serde(
        default,
        alias = "offline_grace_hours",
        skip_serializing_if = "Option::is_none"
    )]
    pub offline_grace_hours: Option<u32>,
    pub plan: String,
    pub message: Option<String>,
    #[serde(alias = "checkout_url")]
    pub checkout_url: Option<String>,
    #[serde(default, alias = "billing_portal_url")]
    pub billing_portal_url: Option<String>,
    #[serde(
        default,
        alias = "billing_portal_available",
        skip_serializing_if = "Option::is_none"
    )]
    pub billing_portal_available: Option<bool>,
    #[serde(
        default,
        alias = "payment_issue",
        skip_serializing_if = "Option::is_none"
    )]
    pub payment_issue: Option<String>,
    #[serde(default)]
    pub savings: SavingsSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referral: Option<Referral>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Referral {
    pub code: String,
    pub link: String,
    #[serde(alias = "trial_days")]
    pub trial_days: u32,
    pub invited: u32,
    pub rewarded: u32,
    #[serde(default, alias = "claimed_code")]
    pub claimed_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub signed_in: bool,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub server_url: String,
    pub entitlements: Option<Entitlements>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub referral: Option<Referral>,
    pub synced_at: Option<String>,
    pub first_launch_at: String,
    pub free_turns_used: u64,
    #[serde(skip_serializing_if = "is_false")]
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RequestCodeResult {
    pub ok: bool,
    #[serde(default, alias = "dev_code", skip_serializing_if = "Option::is_none")]
    pub dev_code: Option<String>,
    /// True when the server sent no code and asks for a Turnstile challenge.
    #[serde(default, skip_serializing_if = "is_false")]
    pub challenge_required: bool,
}

#[derive(Debug, Serialize)]
struct RequestCodeRequest<'a> {
    email: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    turnstile_token: Option<&'a str>,
}

#[derive(Debug, PartialEq, Eq)]
enum EmailChallengeNavigation {
    Allow,
    /// The page passed the challenge and sent this token.
    Done(String),
    Deny,
}

#[derive(Debug, Serialize)]
struct VerifyRequest<'a> {
    email: &'a str,
    code: &'a str,
}

#[derive(Debug, Serialize)]
struct OAuthExchangeRequest<'a> {
    code: &'a str,
    verifier: &'a str,
}

#[derive(Debug, Deserialize)]
struct VerifyResponse {
    token: String,
    user: HalvUser,
}

#[derive(Debug, Deserialize)]
struct MeResponse {
    user: HalvUser,
    entitlements: Entitlements,
    #[serde(default)]
    referral: Option<Referral>,
}

#[derive(Debug, Serialize)]
struct ClaimReferralRequest<'a> {
    code: &'a str,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimReferralResponse {
    pub ok: bool,
    #[serde(alias = "trial_days")]
    pub trial_days: u32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all(serialize = "snake_case", deserialize = "camelCase"))]
pub struct PublishReceiptAgent {
    pub label: String,
    pub tokens_saved: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all(serialize = "snake_case", deserialize = "camelCase"))]
pub struct PublishReceiptPayload {
    pub period_start: String,
    pub period_end: String,
    pub tokens_saved: u64,
    pub savings_usd: f64,
    pub percent_saved: Option<f64>,
    pub agents: Vec<PublishReceiptAgent>,
    pub png_base64: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PublishReceiptResponse {
    pub id: String,
    pub url: String,
}

#[derive(Debug, Serialize)]
struct RouteTaskPayload<'a> {
    brief: &'a str,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct RouteChoice {
    pub choice: String,
    pub confidence: f64,
    pub probabilities: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct RouteUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct RouteDecision {
    pub schema: String,
    pub provider: String,
    pub model: String,
    pub complexity: RouteChoice,
    pub needs_reasoning: f64,
    pub risky: f64,
    pub usage: RouteUsage,
    pub latency_ms: u64,
}

#[derive(Debug, Deserialize)]
struct HalvUser {
    #[allow(dead_code)]
    id: String,
    email: String,
    plan: String,
}

#[derive(Debug, Deserialize)]
struct HealthResponse {
    #[serde(default)]
    oauth_providers: Vec<String>,
    #[serde(default)]
    turnstile_site_key: Option<String>,
}

#[derive(Debug, Serialize)]
struct SavingsReportRequest<'a> {
    events: &'a [SavingsReportEvent],
}

/// The reply to a savings batch. A new server lists the ids it refused in
/// `rejected` (and the ids it stored in `accepted`, which the app does not
/// read). An old server sends only the summary.
#[derive(Debug, Default, Deserialize)]
struct SavingsReportResponse {
    #[serde(flatten)]
    summary: SavingsSummary,
    #[serde(default)]
    rejected: Vec<SavingsRejectedEvent>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct SavingsRejectedEvent {
    id: String,
    #[serde(default)]
    reason: String,
}

/// The state of the savings report queue, for Settings > System.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SavingsQueueStatus {
    pub unreported: u64,
    pub rejected: u64,
    /// The last error of the reporter, until a round succeeds.
    pub last_error: Option<String>,
}

#[derive(Debug, Serialize)]
struct BillingCheckoutRequest<'a> {
    plan: &'a str,
    interval: &'a str,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BillingCurrency {
    Usd,
    Brl,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct BillingPlanPrices {
    pub month: u64,
    pub year: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct BillingPricePlans {
    pub basic: BillingPlanPrices,
    pub unlimited: BillingPlanPrices,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct BillingPrices {
    pub currency: BillingCurrency,
    pub plans: BillingPricePlans,
}

#[derive(Debug, Deserialize)]
struct BillingUrlResponse {
    url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum BillingCheckoutOutcome {
    Success,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct BillingCheckoutEvent {
    outcome: BillingCheckoutOutcome,
}

#[derive(Debug, Clone)]
struct CheckoutNavigationPolicy;

#[derive(Debug)]
enum HalvApiError {
    Unauthorized,
    Recoverable(String),
    Message(String),
}

#[tauri::command]
pub async fn auth_request_code(
    storage: State<'_, Storage>,
    email: String,
    challenge_token: Option<String>,
) -> Result<RequestCodeResult, String> {
    let email = normalize_email(&email)?;
    let challenge_token = normalize_challenge_token(challenge_token.as_deref())?;
    let _guard = halv_account_lock().lock().await;
    let server_url = auth_server_url(&storage).map_err(|error| error.to_string())?;
    let response = api_client()
        .post(api_url(&server_url, "/v1/auth/request-code"))
        .json(&RequestCodeRequest {
            email: &email,
            turnstile_token: challenge_token.as_deref(),
        })
        .send()
        .await
        .map_err(|error| format!("Could not request sign-in code: {error}"))?;
    let status = response.status();
    let body = read_response_body(response, "sign-in code response").await?;
    parse_request_code_response(status, &body)
}

fn parse_request_code_response(
    status: StatusCode,
    body: &str,
) -> Result<RequestCodeResult, String> {
    if status.is_success() {
        return serde_json::from_str(body)
            .map_err(|error| format!("Sign-in code response was not valid JSON: {error}"));
    }
    if server_error_code(body).is_some_and(|code| CHALLENGE_REQUIRED_CODES.contains(&code.as_str()))
    {
        return Ok(RequestCodeResult {
            ok: false,
            dev_code: None,
            challenge_required: true,
        });
    }
    Err(http_error_message(
        status,
        body,
        "Could not request sign-in code",
    ))
}

fn server_error_code(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body)
        .ok()?
        .get("code")?
        .as_str()
        .map(str::to_string)
}

fn normalize_challenge_token(token: Option<&str>) -> Result<Option<String>, String> {
    let Some(token) = token.map(str::trim).filter(|token| !token.is_empty()) else {
        return Ok(None);
    };
    if !is_valid_challenge_token(token) {
        return Err("The security check returned an invalid token.".to_string());
    }
    Ok(Some(token.to_string()))
}

fn is_valid_challenge_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= TURNSTILE_TOKEN_MAX_BYTES
        && token.bytes().all(|byte| byte.is_ascii_graphic())
}

fn is_valid_turnstile_site_key(site_key: &str) -> bool {
    !site_key.is_empty()
        && site_key.len() <= TURNSTILE_SITE_KEY_MAX_BYTES
        && site_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Show the Turnstile challenge for email sign-in and return its token. The
/// client calls this command after `auth_request_code` returns
/// `challengeRequired`, and then sends the token with a new code request.
#[tauri::command]
pub async fn auth_email_challenge(
    app: AppHandle,
    storage: State<'_, Storage>,
) -> Result<String, String> {
    let server_url = auth_server_url(&storage).map_err(|error| error.to_string())?;
    let site_key = fetch_turnstile_site_key(&server_url).await?;
    let page_url = email_challenge_page_url(&site_key)?;
    run_email_challenge_window(&app, page_url).await
}

async fn fetch_turnstile_site_key(server_url: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(OAUTH_PROVIDER_DISCOVERY_TIMEOUT)
        .build()
        .map_err(|error| format!("Could not prepare the security check: {error}"))?;
    let response = client
        .get(api_url(server_url, "/v1/health"))
        .send()
        .await
        .map_err(|error| format!("Could not load the security check: {error}"))?;
    let status = response.status();
    let body = read_response_body(response, "security check response").await?;
    if !status.is_success() {
        return Err(http_error_message(
            status,
            &body,
            "Could not load the security check",
        ));
    }
    parse_turnstile_site_key(&body)
}

fn parse_turnstile_site_key(body: &str) -> Result<String, String> {
    let response: HealthResponse = serde_json::from_str(body)
        .map_err(|error| format!("Security check response was not valid JSON: {error}"))?;
    response
        .turnstile_site_key
        .filter(|site_key| is_valid_turnstile_site_key(site_key))
        .ok_or_else(|| "The server did not offer a security check.".to_string())
}

fn email_challenge_page_url(site_key: &str) -> Result<Url, String> {
    if !is_valid_turnstile_site_key(site_key) {
        return Err("The server sent an invalid security check key.".to_string());
    }
    let mut url = Url::parse(EMAIL_CHALLENGE_PAGE_URL)
        .map_err(|error| format!("Could not build the security check URL: {error}"))?;
    url.query_pairs_mut().append_pair("sitekey", site_key);
    Ok(url)
}

// Decide one navigation in the challenge window. The window can show Halv
// pages and the Turnstile frames only. Turnstile frames start as
// about:blank or about:srcdoc, and on macOS and Linux frame navigations also
// pass through this check.
fn email_challenge_navigation(url: &Url) -> EmailChallengeNavigation {
    match url.scheme() {
        "about" if matches!(url.path(), "blank" | "srcdoc") => EmailChallengeNavigation::Allow,
        "https" => {
            let Some(host) = url.host_str() else {
                return EmailChallengeNavigation::Deny;
            };
            if host.eq_ignore_ascii_case(TURNSTILE_HOST) {
                return EmailChallengeNavigation::Allow;
            }
            if !is_halv_host(host) {
                return EmailChallengeNavigation::Deny;
            }
            if url.path().trim_end_matches('/') != EMAIL_CHALLENGE_DONE_PATH {
                return EmailChallengeNavigation::Allow;
            }
            url.query_pairs()
                .find(|(name, _)| name == "token")
                .map(|(_, token)| token.into_owned())
                .filter(|token| is_valid_challenge_token(token))
                .map_or(
                    EmailChallengeNavigation::Deny,
                    EmailChallengeNavigation::Done,
                )
        }
        _ => EmailChallengeNavigation::Deny,
    }
}

type EmailChallengeSender = Arc<StdMutex<Option<tokio::sync::oneshot::Sender<Option<String>>>>>;

fn finish_email_challenge(sender: &EmailChallengeSender, token: Option<String>) {
    let sender = sender
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(sender) = sender {
        let _ = sender.send(token);
    }
}

async fn run_email_challenge_window(app: &AppHandle, page_url: Url) -> Result<String, String> {
    if let Some(window) = app.get_webview_window(EMAIL_CHALLENGE_WINDOW_LABEL) {
        let _ = window.set_focus();
        return Err("The security check is already open.".to_string());
    }

    let (sender, receiver) = tokio::sync::oneshot::channel();
    let sender: EmailChallengeSender = Arc::new(StdMutex::new(Some(sender)));
    let navigation_sender = Arc::clone(&sender);
    let window = WebviewWindowBuilder::new(
        app,
        EMAIL_CHALLENGE_WINDOW_LABEL,
        WebviewUrl::External(page_url),
    )
    .title("Security check · Halv")
    .inner_size(440.0, 560.0)
    .min_inner_size(360.0, 460.0)
    .resizable(true)
    .closable(true)
    .center()
    .on_navigation(
        move |candidate| match email_challenge_navigation(candidate) {
            EmailChallengeNavigation::Allow => true,
            EmailChallengeNavigation::Done(token) => {
                finish_email_challenge(&navigation_sender, Some(token));
                false
            }
            EmailChallengeNavigation::Deny => false,
        },
    )
    .on_new_window(|_, _| NewWindowResponse::Deny)
    .build()
    .map_err(|error| format!("Could not open the security check: {error}"))?;

    let close_sender = Arc::clone(&sender);
    window.on_window_event(move |event| {
        if matches!(
            event,
            WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed
        ) {
            finish_email_challenge(&close_sender, None);
        }
    });

    let outcome = tokio::time::timeout(EMAIL_CHALLENGE_TIMEOUT, receiver).await;
    // The window can be closed already. Then close() fails, and that is fine.
    let _ = window.close();
    match outcome {
        Ok(Ok(Some(token))) => Ok(token),
        Ok(Ok(None)) | Ok(Err(_)) => Err("The security check was cancelled.".to_string()),
        Err(_) => Err("The security check timed out.".to_string()),
    }
}

#[tauri::command]
pub async fn auth_verify(
    storage: State<'_, Storage>,
    email: String,
    code: String,
) -> Result<AuthStatus, String> {
    let email = normalize_email(&email)?;
    let code = normalize_code(&code)?;
    let _guard = halv_account_lock().lock().await;
    let server_url = auth_server_url(&storage).map_err(|error| error.to_string())?;
    let response = api_client()
        .post(api_url(&server_url, "/v1/auth/verify"))
        .json(&VerifyRequest {
            email: &email,
            code: &code,
        })
        .send()
        .await
        .map_err(|error| format!("Could not verify sign-in code: {error}"))?;
    let status = response.status();
    let body = read_response_body(response, "verify response").await?;

    if !status.is_success() {
        return Err(http_error_message(
            status,
            &body,
            "Could not verify sign-in code",
        ));
    }

    let verify_response = parse_login_response(&body, "Verify")?;
    complete_account_login(
        &storage,
        &server_url,
        verify_response,
        "The server rejected the new session. Please request a fresh code.",
    )
    .await
}

#[tauri::command]
pub async fn auth_oauth_login(
    storage: State<'_, Storage>,
    provider: String,
) -> Result<AuthStatus, String> {
    let provider = HalvOAuthProvider::parse(&provider)?;
    let _login_guard = AccountLoginInFlightGuard::acquire().map_err(|error| error.to_string())?;
    let cancel = Arc::new(AtomicBool::new(false));
    let _cancel_guard = AccountOAuthCancelGuard::activate(Arc::clone(&cancel));
    let server_url = auth_server_url(&storage).map_err(|error| error.to_string())?;
    let flow = PkceFlow::generate();
    let (listener, port) = tauri::async_runtime::spawn_blocking(bind_account_callback_listener)
        .await
        .map_err(|error| format!("Account sign-in callback bind task failed: {error}"))?
        .map_err(|error| error.to_string())?;
    let start_url = build_halv_oauth_start_url(&server_url, provider, &flow, port)?;
    let callback_server = spawn_callback_server_with_cancel(
        listener,
        flow.state.clone(),
        CallbackFlow::HalvAccount,
        Arc::clone(&cancel),
    )
    .map_err(|error| error.to_string())?;

    ensure_oauth_not_cancelled(&cancel).map_err(|error| error.to_string())?;

    if let Err(error) = tauri_plugin_opener::open_url(&start_url, None::<&str>) {
        callback_server.cancel.store(true, Ordering::SeqCst);
        return Err(format!(
            "Could not open the browser for account sign-in: {error}"
        ));
    }

    let code = wait_for_callback(callback_server)
        .await
        .map_err(|error| error.to_string())?;
    ensure_oauth_not_cancelled(&cancel).map_err(|error| error.to_string())?;
    let _guard = halv_account_lock().lock().await;
    ensure_oauth_not_cancelled(&cancel).map_err(|error| error.to_string())?;
    let login_response = exchange_halv_oauth_code(&server_url, &code, &flow.verifier).await?;
    ensure_oauth_not_cancelled(&cancel).map_err(|error| error.to_string())?;
    complete_account_login(
        &storage,
        &server_url,
        login_response,
        "The server rejected the new session. Please try signing in again.",
    )
    .await
}

#[tauri::command]
pub fn auth_oauth_cancel() {
    for active in [&ACTIVE_ACCOUNT_OAUTH_CANCEL, &ACTIVE_CHATGPT_OAUTH_CANCEL] {
        let active = active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cancel) = active.as_ref() {
            cancel.store(true, Ordering::SeqCst);
        }
    }
}

#[tauri::command]
pub async fn auth_oauth_providers(storage: State<'_, Storage>) -> Result<Vec<String>, String> {
    let server_url = auth_server_url(&storage).map_err(|error| error.to_string())?;
    let client = reqwest::Client::builder()
        .timeout(OAUTH_PROVIDER_DISCOVERY_TIMEOUT)
        .build()
        .map_err(|error| format!("Could not prepare sign-in provider discovery: {error}"))?;
    let response = client
        .get(api_url(&server_url, "/v1/health"))
        .send()
        .await
        .map_err(|error| format!("Could not discover sign-in providers: {error}"))?;
    let status = response.status();
    let body = read_response_body(response, "sign-in provider response").await?;

    if !status.is_success() {
        return Err(http_error_message(
            status,
            &body,
            "Could not discover sign-in providers",
        ));
    }

    parse_oauth_providers_response(&body)
}

async fn complete_account_login(
    storage: &Storage,
    server_url: &str,
    login_response: VerifyResponse,
    rejected_session_message: &str,
) -> Result<AuthStatus, String> {
    let VerifyResponse { token, user } = login_response;
    let session = AuthSession {
        server_url: server_url.to_string(),
        token: Some(token.clone()),
        email: Some(user.email),
        plan: Some(user.plan),
        entitlements_json: None,
        synced_at: None,
    };
    storage
        .save_auth_session(&session)
        .map_err(|error| error.to_string())?;
    wake_savings_reporter();

    match fetch_me(server_url, &token).await {
        Ok(me) => {
            if !save_me_response(storage, server_url, &token, me)
                .map_err(|error| error.to_string())?
            {
                return local_auth_status(storage, false).map_err(|error| error.to_string());
            }
            local_auth_status(storage, false).map_err(|error| error.to_string())
        }
        Err(HalvApiError::Unauthorized) => {
            storage
                .clear_auth_session_if_current(server_url, Some(&token))
                .map_err(|error| error.to_string())?;
            wake_savings_reporter();
            Err(rejected_session_message.to_string())
        }
        Err(HalvApiError::Recoverable(message)) | Err(HalvApiError::Message(message)) => {
            Err(message)
        }
    }
}

// The status poll and the focus refresh call this often. A sync command runs
// on the app main thread, which also delivers the keys to the webview.
#[tauri::command]
pub async fn auth_status(storage: State<'_, Storage>) -> Result<AuthStatus, String> {
    local_auth_status(&storage, false).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn auth_refresh_entitlements(storage: State<'_, Storage>) -> Result<AuthStatus, String> {
    if account_bypass_active() {
        return local_auth_status(&storage, false).map_err(|error| error.to_string());
    }

    let _guard = halv_account_lock().lock().await;
    let Some(session) = storage
        .get_auth_session()
        .map_err(|error| error.to_string())?
    else {
        return local_auth_status(&storage, false).map_err(|error| error.to_string());
    };
    let Some(token) = session.token.clone() else {
        return local_auth_status(&storage, false).map_err(|error| error.to_string());
    };
    let server_url = normalize_server_url(&session.server_url)
        .map_err(|error| format!("Stored server URL is invalid: {error}"))?;

    match fetch_me(&server_url, &token).await {
        Ok(me) => {
            if !save_me_response(&storage, &server_url, &token, me)
                .map_err(|error| error.to_string())?
            {
                return local_auth_status(&storage, false).map_err(|error| error.to_string());
            }
            local_auth_status(&storage, false).map_err(|error| error.to_string())
        }
        Err(HalvApiError::Unauthorized) => {
            storage
                .clear_auth_session_if_current(&server_url, Some(&token))
                .map_err(|error| error.to_string())?;
            wake_savings_reporter();
            local_auth_status(&storage, false).map_err(|error| error.to_string())
        }
        Err(HalvApiError::Recoverable(message)) | Err(HalvApiError::Message(message)) => {
            eprintln!("Keeping cached Halv account entitlements after refresh failure: {message}");
            local_auth_status(&storage, true).map_err(|error| error.to_string())
        }
    }
}

#[tauri::command]
pub async fn auth_signout(storage: State<'_, Storage>) -> Result<AuthStatus, String> {
    if account_bypass_active() {
        return local_auth_status(&storage, false).map_err(|error| error.to_string());
    }

    let _guard = halv_account_lock().lock().await;
    if let Some(session) = storage
        .get_auth_session_for_sign_out()
        .map_err(|error| error.to_string())?
    {
        let token = session.token.as_deref();
        if let Some(token) = token {
            let _ = api_client()
                .post(api_url(&session.server_url, "/v1/auth/signout"))
                .bearer_auth(token)
                .send()
                .await;
        }
        storage
            .clear_auth_session_if_current(&session.server_url, token)
            .map_err(|error| error.to_string())?;
        wake_savings_reporter();
    }
    local_auth_status(&storage, false).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn auth_open_checkout(url: String) -> Result<(), String> {
    let checkout_url = normalize_checkout_url(&url)?;
    open_checkout_url(&checkout_url)
}

#[tauri::command]
pub async fn billing_lifetime_offer(
    storage: State<'_, Storage>,
) -> Result<serde_json::Value, String> {
    let (server_url, token) = current_halv_session(&storage)?;
    let response = api_client()
        .get(api_url(&server_url, "/v1/billing/lifetime"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("Could not load lifetime offer: {e}"))?;
    let status = response.status();
    let body = read_response_body(response, "lifetime offer").await?;
    if !status.is_success() {
        return Err(http_error_message(
            status,
            &body,
            "Could not load lifetime offer",
        ));
    }
    serde_json::from_str(&body).map_err(|e| format!("Invalid lifetime offer: {e}"))
}

#[tauri::command]
pub async fn billing_prices(storage: State<'_, Storage>) -> Result<BillingPrices, String> {
    let server_url = auth_server_url(&storage).map_err(|error| error.to_string())?;
    let server_url = normalize_server_url(&server_url)
        .map_err(|error| format!("Stored server URL is invalid: {error}"))?;
    let response = api_client()
        .get(api_url(&server_url, "/v1/billing/prices"))
        .send()
        .await
        .map_err(|error| format!("Could not load billing prices: {error}"))?;
    let status = response.status();
    let body = read_response_body(response, "billing prices").await?;
    if !status.is_success() {
        return Err(http_error_message(
            status,
            &body,
            "Could not load billing prices",
        ));
    }
    serde_json::from_str(&body)
        .map_err(|error| format!("Billing prices response was not valid JSON: {error}"))
}

#[tauri::command]
pub async fn billing_checkout(
    app: AppHandle,
    storage: State<'_, Storage>,
    plan: String,
    interval: String,
) -> Result<(), String> {
    let url = create_billing_checkout_url(&storage, &plan, &interval).await?;
    open_checkout_window(&app, &url)
}

#[tauri::command]
pub async fn billing_checkout_browser(
    storage: State<'_, Storage>,
    plan: String,
    interval: String,
) -> Result<(), String> {
    let url = create_billing_checkout_url(&storage, &plan, &interval).await?;
    open_checkout_url(&url)
}

#[tauri::command]
pub async fn billing_portal(storage: State<'_, Storage>) -> Result<(), String> {
    let _guard = halv_account_lock().lock().await;
    let (server_url, token) = current_halv_session(&storage)?;
    let response = api_client()
        .post(api_url(&server_url, "/v1/billing/portal"))
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|error| format!("Could not open billing portal: {error}"))?;
    let url = parse_billing_url_response(response, "Could not open billing portal").await?;
    open_checkout_url(&url)
}

#[tauri::command]
pub async fn account_savings_queue_status(
    storage: State<'_, Storage>,
) -> Result<SavingsQueueStatus, String> {
    let counts = storage
        .savings_queue_counts()
        .map_err(|error| error.to_string())?;
    Ok(SavingsQueueStatus {
        unreported: counts.unreported,
        rejected: counts.rejected,
        last_error: savings_reporter_last_error()
            .lock()
            .ok()
            .and_then(|error| error.clone()),
    })
}

#[tauri::command]
pub async fn account_claim_referral(
    storage: State<'_, Storage>,
    code: String,
) -> Result<ClaimReferralResponse, String> {
    let code = code.trim().to_ascii_uppercase();
    let _guard = halv_account_lock().lock().await;
    let (server_url, token) = current_halv_session(&storage)?;
    let response = api_client()
        .post(api_url(&server_url, "/v1/referrals/claim"))
        .bearer_auth(&token)
        .json(&ClaimReferralRequest { code: &code })
        .send()
        .await
        .map_err(|error| format!("Could not apply referral code: {error}"))?;
    let status = response.status();
    let body = read_response_body(response, "referral claim response").await?;
    if !status.is_success() {
        return Err(http_error_message(
            status,
            &body,
            "Could not apply referral code",
        ));
    }
    serde_json::from_str(&body)
        .map_err(|error| format!("Referral claim response was not valid JSON: {error}"))
}

#[tauri::command]
pub async fn account_publish_receipt(
    storage: State<'_, Storage>,
    payload: PublishReceiptPayload,
) -> Result<PublishReceiptResponse, String> {
    let _guard = halv_account_lock().lock().await;
    let (server_url, token) = current_halv_session(&storage)?;
    let response = api_client()
        .post(api_url(&server_url, "/v1/receipts"))
        .bearer_auth(&token)
        .json(&payload)
        .send()
        .await
        .map_err(|error| format!("Could not publish receipt: {error}"))?;
    let status = response.status();
    let body = read_response_body(response, "receipt response").await?;
    if !status.is_success() {
        return Err(http_error_message(
            status,
            &body,
            "Could not publish receipt",
        ));
    }
    serde_json::from_str(&body)
        .map_err(|error| format!("Receipt response was not valid JSON: {error}"))
}

/// Halv sends a task brief only after the user agreed to brief sharing (ADR 0023).
fn require_brief_sharing(sharing: crate::preferences::BriefSharing) -> Result<(), String> {
    if sharing == crate::preferences::BriefSharing::Allowed {
        Ok(())
    } else {
        Err("Brief sharing is off. Halv sends no task brief.".to_string())
    }
}

#[tauri::command]
pub async fn route_task(
    storage: State<'_, Storage>,
    preferences: State<'_, crate::preferences::PreferencesManager>,
    brief: String,
) -> Result<RouteDecision, String> {
    let brief = brief.trim();
    if brief.is_empty() {
        return Err("Task brief is empty.".to_string());
    }
    require_brief_sharing(preferences.get()?.brief_sharing)?;
    let _guard = halv_account_lock().lock().await;
    let (server_url, token) = current_halv_session(&storage)?;
    let response = api_client()
        .post(api_url(&server_url, "/v1/route"))
        .bearer_auth(&token)
        .json(&RouteTaskPayload { brief })
        .send()
        .await
        .map_err(|error| format!("Could not route task: {error}"))?;
    let status = response.status();
    let body = read_response_body(response, "route response").await?;
    if !status.is_success() {
        return Err(http_error_message(status, &body, "Could not route task"));
    }
    serde_json::from_str(&body)
        .map_err(|error| format!("Route response was not valid JSON: {error}"))
}

#[tauri::command]
pub async fn auth_credential_status() -> CredentialAuthStatus {
    credential_auth_status_inner().await
}

#[tauri::command]
pub async fn auth_login_chatgpt(
    storage: State<'_, Storage>,
) -> Result<CredentialAuthStatus, String> {
    let status = login_chatgpt_inner()
        .await
        .map_err(|error| error.to_string())?;
    if let Err(error) = profiles::ensure_chatgpt_oauth_profile(&storage) {
        eprintln!("ChatGPT sign-in could not update the credential profile: {error}");
    }
    Ok(status)
}

#[tauri::command]
pub async fn auth_logout() -> Result<CredentialAuthStatus, String> {
    clear_keychain_secret(KEYCHAIN_CHATGPT_ACCOUNT).map_err(|error| error.to_string())?;
    clear_keychain_secret(KEYCHAIN_API_KEY_ACCOUNT).map_err(|error| error.to_string())?;
    Ok(credential_auth_status_inner().await)
}

#[tauri::command]
pub async fn auth_set_api_key(api_key: String) -> Result<CredentialAuthStatus, String> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return Err("API key cannot be empty".to_string());
    }

    set_keychain_secret(KEYCHAIN_API_KEY_ACCOUNT, trimmed).map_err(|error| error.to_string())?;
    Ok(credential_auth_status_inner().await)
}

#[tauri::command]
pub async fn auth_clear_api_key() -> Result<CredentialAuthStatus, String> {
    clear_api_key_secret().map_err(|error| error.to_string())?;
    Ok(credential_auth_status_inner().await)
}

#[tauri::command]
pub async fn auth_anthropic_status() -> AnthropicCredentialStatus {
    anthropic_credential_status()
}

#[tauri::command]
pub async fn auth_set_anthropic_api_key(
    api_key: String,
) -> Result<AnthropicCredentialStatus, String> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return Err("Anthropic API key cannot be empty".to_string());
    }
    set_keychain_secret(KEYCHAIN_ANTHROPIC_API_KEY_ACCOUNT, trimmed)
        .map_err(|error| error.to_string())?;
    Ok(anthropic_credential_status())
}

#[tauri::command]
pub async fn auth_clear_anthropic_api_key() -> Result<AnthropicCredentialStatus, String> {
    clear_keychain_secret(KEYCHAIN_ANTHROPIC_API_KEY_ACCOUNT).map_err(|error| error.to_string())?;
    Ok(anthropic_credential_status())
}

fn anthropic_credential_status() -> AnthropicCredentialStatus {
    match get_keychain_secret(KEYCHAIN_ANTHROPIC_API_KEY_ACCOUNT) {
        Ok(Some(api_key)) => AnthropicCredentialStatus {
            configured: true,
            masked: Some(mask_secret(&api_key)),
        },
        _ => AnthropicCredentialStatus {
            configured: false,
            masked: None,
        },
    }
}

pub(crate) fn auth_get_anthropic_api_key() -> Result<String, String> {
    get_keychain_secret(KEYCHAIN_ANTHROPIC_API_KEY_ACCOUNT)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Set an Anthropic API key in Settings to use Claude".to_string())
}

pub(crate) fn api_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(HALV_API_TIMEOUT)
            .build()
            .expect("Halv API client should build")
    })
}

fn savings_reporter_notify() -> &'static Notify {
    static NOTIFY: OnceLock<Notify> = OnceLock::new();
    NOTIFY.get_or_init(Notify::new)
}

/// The last error of the reporter, for support. A successful round clears
/// it.
fn savings_reporter_last_error() -> &'static std::sync::Mutex<Option<String>> {
    static LAST_ERROR: OnceLock<std::sync::Mutex<Option<String>>> = OnceLock::new();
    LAST_ERROR.get_or_init(|| std::sync::Mutex::new(None))
}

fn set_savings_reporter_last_error(error: Option<String>) {
    if let Ok(mut last_error) = savings_reporter_last_error().lock() {
        *last_error = error;
    }
}

/// Wakes the reporter after account or queue state changes without waiting on
/// storage or network work.
pub fn wake_savings_reporter() {
    savings_reporter_notify().notify_one();
}

/// Starts the single process-wide savings reporter.
pub fn spawn_savings_reporter(app: AppHandle) {
    if SAVINGS_REPORTER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        run_savings_reporter(app).await;
    });
}

async fn run_savings_reporter(app: AppHandle) {
    let mut retry_at = TokioInstant::now();
    let mut backoff = SAVINGS_REPORT_INITIAL_BACKOFF;

    loop {
        // Settle finished JEV chains first, so their routing events join this
        // batch (ADR 0016).
        if let Err(error) = app.state::<Storage>().settle_routing_chains() {
            eprintln!("Could not settle JEV routing savings: {error}");
        }
        let session = match app.state::<Storage>().get_auth_session() {
            Ok(session) => session,
            Err(error) => {
                eprintln!("Could not read Halv account for savings reporting: {error}");
                wait_for_savings_reporter(SAVINGS_REPORT_STATE_INTERVAL).await;
                continue;
            }
        };
        // The Headroom proxy is local work, so it never depends on the plan
        // of the account (ADR 0021). The reporter never pauses it.

        let Some(session) = session else {
            retry_at = TokioInstant::now();
            backoff = SAVINGS_REPORT_INITIAL_BACKOFF;
            wait_for_savings_reporter(SAVINGS_REPORT_IDLE_INTERVAL).await;
            continue;
        };
        if session.token.is_none() {
            retry_at = TokioInstant::now();
            backoff = SAVINGS_REPORT_INITIAL_BACKOFF;
            wait_for_savings_reporter(SAVINGS_REPORT_IDLE_INTERVAL).await;
            continue;
        }

        let events = match app
            .state::<Storage>()
            .unreported_savings(SAVINGS_REPORT_BATCH_SIZE)
        {
            Ok(events) => events,
            Err(error) => {
                eprintln!("Could not read queued savings reports: {error}");
                wait_for_savings_reporter(SAVINGS_REPORT_STATE_INTERVAL).await;
                continue;
            }
        };
        let refresh_expired_period = savings_period_refresh_required(&session);
        if events.is_empty() && !refresh_expired_period {
            retry_at = TokioInstant::now();
            backoff = SAVINGS_REPORT_INITIAL_BACKOFF;
            wait_for_savings_reporter(SAVINGS_REPORT_IDLE_INTERVAL).await;
            continue;
        }

        let now = TokioInstant::now();
        if now < retry_at {
            wait_for_savings_reporter(
                retry_at
                    .duration_since(now)
                    .min(SAVINGS_REPORT_STATE_INTERVAL),
            )
            .await;
            continue;
        }

        let _guard = halv_account_lock().lock().await;
        let current_session = match app.state::<Storage>().get_auth_session() {
            Ok(Some(current_session)) if current_session.token.is_some() => current_session,
            Ok(_) => continue,
            Err(error) => {
                eprintln!("Could not re-check Halv account for savings reporting: {error}");
                continue;
            }
        };
        let current_events = match app
            .state::<Storage>()
            .unreported_savings(SAVINGS_REPORT_BATCH_SIZE)
        {
            Ok(events) => events,
            Err(error) => {
                eprintln!("Could not re-read queued savings reports: {error}");
                continue;
            }
        };
        if current_events.is_empty() && !savings_period_refresh_required(&current_session) {
            continue;
        }
        let token = current_session
            .token
            .as_deref()
            .expect("signed-in session checked above");
        let server_url = match normalize_server_url(&current_session.server_url) {
            Ok(server_url) => server_url,
            Err(error) => {
                eprintln!("Stored server URL is invalid for savings reporting: {error}");
                retry_at = TokioInstant::now() + backoff;
                backoff = next_savings_backoff(backoff);
                continue;
            }
        };

        let delivery = if current_events.is_empty() {
            // Only the expired period needs a fresh summary.
            match post_savings_batch(&server_url, token, &current_events).await {
                Ok(response) => SavingsDelivery {
                    summary: Some(response.summary),
                    ..SavingsDelivery::default()
                },
                Err(error) => SavingsDelivery {
                    retry_error: Some(error.to_string()),
                    ..SavingsDelivery::default()
                },
            }
        } else {
            deliver_savings_events(&current_events, |chunk| {
                let server_url = server_url.as_str();
                async move { post_savings_batch(server_url, token, &chunk).await }
            })
            .await
        };
        if let Some(summary) = delivery.summary.clone() {
            if let Err(error) =
                cache_savings_summary(&app.state::<Storage>(), &current_session, summary)
            {
                eprintln!("Could not update cached savings entitlements: {error}");
            }
        }
        for (event_id, reason) in &delivery.rejected {
            eprintln!("The server refused savings report `{event_id}`: {reason}");
        }
        let storage = app.state::<Storage>();
        if let Err(error) = storage
            .mark_savings_reported(&delivery.delivered)
            .and_then(|()| storage.mark_savings_rejected(&delivery.rejected))
        {
            eprintln!("Could not mark savings reports delivered: {error}");
            set_savings_reporter_last_error(Some(error.to_string()));
            retry_at = TokioInstant::now() + backoff;
            backoff = next_savings_backoff(backoff);
            continue;
        }
        if let Some(error) = delivery.retry_error {
            eprintln!("Could not report token savings: {error}");
            set_savings_reporter_last_error(Some(error));
            retry_at = TokioInstant::now() + backoff;
            backoff = next_savings_backoff(backoff);
            continue;
        }
        set_savings_reporter_last_error(None);
        wake_savings_reporter();
        retry_at = TokioInstant::now();
        backoff = SAVINGS_REPORT_INITIAL_BACKOFF;
    }
}

/// The result of one delivery round of queued savings reports.
#[derive(Debug, Default)]
struct SavingsDelivery {
    delivered: Vec<String>,
    /// Event ids that the server refused one by one, with the server error.
    rejected: Vec<(String, String)>,
    summary: Option<SavingsSummary>,
    /// Set when the round stopped on an error that a later retry can fix.
    retry_error: Option<String>,
}

#[derive(Debug)]
enum SavingsPostError {
    /// The server refused the request body (HTTP 400, 413 or 422).
    Payload(String),
    /// A network, authentication, rate limit or server error.
    Retry(String),
}

impl std::fmt::Display for SavingsPostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Payload(message) | Self::Retry(message) => formatter.write_str(message),
        }
    }
}

/// The reason the server refuses an event, by the rules the app knows.
/// The queue splits large samples, so this catches only old rows.
fn local_savings_rejection(event: &SavingsReportEvent) -> Option<&'static str> {
    if event.tokens_saved > crate::storage::MAX_TOKENS_SAVED_PER_REPORT_EVENT {
        return Some("tokens_saved is above the per-event limit");
    }
    if event.tokens_saved < 0 {
        return Some("tokens_saved is negative");
    }
    if !event.savings_usd.is_finite() || event.savings_usd < 0.0 {
        return Some("savings_usd is not a finite non-negative number");
    }
    None
}

/// Sends the events in order. A new server answers 200 and lists the ids
/// it stored and the ids it refused; each refused id becomes a dead letter.
/// An old server refuses the whole batch with 400. Then the events that
/// break a known rule become dead letters and the rest go again. When no
/// event breaks a known rule, the batch splits in two halves and each half
/// goes again, until a single refused event becomes a dead letter. So one
/// bad event cannot block the queue. Any other error stops the round and
/// keeps the remaining events in the queue.
async fn deliver_savings_events<F, Fut>(
    events: &[SavingsReportEvent],
    mut post: F,
) -> SavingsDelivery
where
    F: FnMut(Vec<SavingsReportEvent>) -> Fut,
    Fut: std::future::Future<Output = Result<SavingsReportResponse, SavingsPostError>>,
{
    let mut delivery = SavingsDelivery::default();
    let mut pending: Vec<Vec<usize>> = vec![(0..events.len()).collect()];
    while let Some(indexes) = pending.pop() {
        if indexes.is_empty() {
            continue;
        }
        let chunk: Vec<SavingsReportEvent> =
            indexes.iter().map(|&index| events[index].clone()).collect();
        match post(chunk).await {
            Ok(response) => {
                // An id in `rejected` is a dead letter. Every other id of
                // the chunk is delivered: the server answered 200 for it,
                // whether it lists the id in `accepted` or not.
                let refused: std::collections::HashMap<&str, &str> = response
                    .rejected
                    .iter()
                    .map(|event| (event.id.as_str(), event.reason.as_str()))
                    .collect();
                for &index in &indexes {
                    let id = &events[index].id;
                    match refused.get(id.as_str()) {
                        Some(reason) => delivery.rejected.push((id.clone(), (*reason).to_string())),
                        None => delivery.delivered.push(id.clone()),
                    }
                }
                delivery.summary = Some(response.summary);
            }
            Err(SavingsPostError::Payload(reason)) => {
                let (offenders, rest): (Vec<usize>, Vec<usize>) = indexes
                    .iter()
                    .partition(|&&index| local_savings_rejection(&events[index]).is_some());
                if !offenders.is_empty() {
                    for index in offenders {
                        let event = &events[index];
                        let rule = local_savings_rejection(event).unwrap_or("refused");
                        delivery
                            .rejected
                            .push((event.id.clone(), format!("{reason}: {rule}")));
                    }
                    pending.push(rest);
                } else if indexes.len() > 1 {
                    let middle = indexes.len() / 2;
                    pending.push(indexes[middle..].to_vec());
                    pending.push(indexes[..middle].to_vec());
                } else {
                    delivery
                        .rejected
                        .push((events[indexes[0]].id.clone(), reason));
                }
            }
            Err(SavingsPostError::Retry(error)) => {
                delivery.retry_error = Some(error);
                break;
            }
        }
    }
    delivery
}

fn savings_post_error(status: StatusCode, message: String) -> SavingsPostError {
    if matches!(
        status,
        StatusCode::BAD_REQUEST | StatusCode::PAYLOAD_TOO_LARGE | StatusCode::UNPROCESSABLE_ENTITY
    ) {
        SavingsPostError::Payload(message)
    } else {
        SavingsPostError::Retry(message)
    }
}

async fn wait_for_savings_reporter(duration: Duration) {
    tokio::select! {
        _ = tokio::time::sleep(duration) => {}
        _ = savings_reporter_notify().notified() => {}
    }
}

fn next_savings_backoff(current: Duration) -> Duration {
    current
        .checked_mul(2)
        .unwrap_or(SAVINGS_REPORT_MAX_BACKOFF)
        .min(SAVINGS_REPORT_MAX_BACKOFF)
}

fn cached_entitlements(session: &AuthSession) -> Option<Entitlements> {
    session
        .entitlements_json
        .as_deref()
        .and_then(|value| serde_json::from_str(value).ok())
}

fn savings_period_refresh_required(session: &AuthSession) -> bool {
    cached_entitlements(session).is_some_and(|entitlements| {
        entitlements.plan == "basic"
            && entitlements.savings.exhausted
            && entitlements.savings.period != Utc::now().format("%Y-%m").to_string()
    })
}

async fn post_savings_batch(
    server_url: &str,
    token: &str,
    events: &[SavingsReportEvent],
) -> Result<SavingsReportResponse, SavingsPostError> {
    let response = api_client()
        .post(api_url(server_url, "/v1/usage/savings"))
        .bearer_auth(token)
        .json(&SavingsReportRequest { events })
        .send()
        .await
        .map_err(|error| SavingsPostError::Retry(format!("request failed: {error}")))?;
    let status = response.status();
    let body = read_response_body(response, "savings response")
        .await
        .map_err(SavingsPostError::Retry)?;
    if !status.is_success() {
        return Err(savings_post_error(
            status,
            http_error_message(status, &body, "Savings report failed"),
        ));
    }
    serde_json::from_str(&body).map_err(|error| {
        SavingsPostError::Retry(format!("Savings response was not valid JSON: {error}"))
    })
}

fn cache_savings_summary(
    storage: &Storage,
    session: &AuthSession,
    savings: SavingsSummary,
) -> Result<bool, String> {
    let Some(mut entitlements) = cached_entitlements(session) else {
        return Ok(false);
    };
    let token = session
        .token
        .as_deref()
        .ok_or_else(|| "Halv account is signed out".to_string())?;
    entitlements.savings = savings;
    let entitlements_json = serde_json::to_string(&entitlements)
        .map_err(|error| format!("failed to encode entitlements: {error}"))?;
    let updated = storage
        .update_auth_entitlements_if_current(&session.server_url, token, &entitlements_json)
        .map_err(|error| error.to_string())?;
    if updated {
        wake_savings_reporter();
    }
    Ok(updated)
}

fn halv_account_lock() -> &'static AsyncMutex<()> {
    static LOCK: OnceLock<AsyncMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| AsyncMutex::new(()))
}

#[cfg(debug_assertions)]
fn account_bypass_active() -> bool {
    let active = account_bypass_active_from(env::var(ENV_HALV_BYPASS_ACCOUNT).ok().as_deref());
    if active && !ACCOUNT_BYPASS_LOGGED.swap(true, Ordering::Relaxed) {
        eprintln!("HALV_BYPASS_ACCOUNT active: account gating disabled.");
    }
    active
}

#[cfg(not(debug_assertions))]
fn account_bypass_active() -> bool {
    // Release builds must never honor the developer account bypass.
    false
}

#[cfg(any(test, debug_assertions))]
fn account_bypass_active_from(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        let value = value.trim();
        value == "1" || value.eq_ignore_ascii_case("true")
    })
}

fn account_bypass_status(free_turns_used: u64, first_launch_at: String) -> AuthStatus {
    AuthStatus {
        signed_in: true,
        email: Some("dev@halv.local".to_string()),
        plan: Some("dev".to_string()),
        server_url: default_halv_api_url().to_string(),
        entitlements: Some(Entitlements {
            trial_confirmed: false,
            payment_confirmed: false,
            trial_end: None,
            beta: true,
            billing_required: false,
            can_use_app: true,
            offline_grace_hours: None,
            plan: "dev".to_string(),
            message: None,
            checkout_url: None,
            billing_portal_url: None,
            billing_portal_available: None,
            payment_issue: None,
            savings: SavingsSummary::default(),
            referral: None,
        }),
        referral: None,
        synced_at: Some(Utc::now().to_rfc3339()),
        first_launch_at,
        free_turns_used,
        stale: false,
    }
}

fn default_halv_api_url() -> &'static str {
    static DEFAULT_URL: OnceLock<String> = OnceLock::new();
    DEFAULT_URL.get_or_init(|| {
        env::var(ENV_HALV_API_URL)
            .ok()
            .and_then(|value| normalize_server_url(&value).ok())
            .unwrap_or_else(|| DEFAULT_HALV_API_URL.to_string())
    })
}

/// The server URL for Halv requests. A signed-in session gives its URL: for
/// a token in the keychain, the URL that the keychain item binds. Without a
/// session, the URL is the default. The step does not use the URL column of
/// a signed-out row: any process of the same user can change it, and the next
/// sign-in would then send the email code or the OAuth flow to that host
/// (issue #50).
pub(crate) fn auth_server_url(storage: &Storage) -> Result<String, crate::storage::StorageError> {
    Ok(storage
        .get_auth_session()?
        .filter(|session| session.token.is_some())
        .map(|session| session.server_url)
        .unwrap_or_else(|| default_halv_api_url().to_string()))
}

/// Validates and normalizes a Halv server URL: https, or http to a loopback
/// host for a development server.
pub(crate) fn normalize_server_url(url: &str) -> Result<String, String> {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("Server URL cannot be empty".to_string());
    }
    let parsed = Url::parse(trimmed).map_err(|error| format!("Invalid server URL: {error}"))?;
    match parsed.scheme() {
        "https" => Ok(trimmed.to_string()),
        "http" if is_literal_loopback_host(&parsed) => Ok(trimmed.to_string()),
        "http" => Err(
            "Server URL must use https:// unless it targets localhost or a loopback IP address"
                .to_string(),
        ),
        _ => Err("Server URL must start with http:// or https://".to_string()),
    }
}

fn is_literal_loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

fn normalize_checkout_url(url: &str) -> Result<String, String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("Checkout URL cannot be empty".to_string());
    }
    let parsed = Url::parse(trimmed).map_err(|error| format!("Invalid checkout URL: {error}"))?;
    match parsed.scheme() {
        "http" | "https" => Ok(trimmed.to_string()),
        _ => Err("Checkout URL must start with http:// or https://".to_string()),
    }
}

impl CheckoutNavigationPolicy {
    fn new(checkout_url: &str) -> Result<(Self, Url), String> {
        let parsed = Url::parse(checkout_url)
            .map_err(|error| format!("Invalid Stripe checkout URL: {error}"))?;
        parsed
            .host_str()
            .filter(|host| is_stripe_host(host))
            .ok_or_else(|| "Checkout did not return a Stripe URL.".to_string())?;
        if parsed.scheme() != "https" {
            return Err("Stripe checkout URL must use https://".to_string());
        }
        Ok((Self, parsed))
    }

    fn allows(&self, candidate: &Url) -> bool {
        candidate.scheme() == "https"
    }
}

fn is_stripe_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("stripe.com")
        || host
            .to_ascii_lowercase()
            .strip_suffix(".stripe.com")
            .is_some_and(|prefix| !prefix.is_empty())
}

fn is_halv_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("halv.ai")
        || host
            .to_ascii_lowercase()
            .strip_suffix(".halv.ai")
            .is_some_and(|prefix| !prefix.is_empty())
}

fn billing_checkout_outcome(url: &Url) -> Option<BillingCheckoutOutcome> {
    if !url.host_str().is_some_and(is_halv_host) {
        return None;
    }
    match url.path().trim_end_matches('/') {
        "/billing/success" => Some(BillingCheckoutOutcome::Success),
        "/billing/cancel" => Some(BillingCheckoutOutcome::Cancelled),
        _ => None,
    }
}

fn normalize_billing_plan(plan: &str) -> Result<String, String> {
    match plan.trim().to_ascii_lowercase().as_str() {
        "basic" => Ok("basic".to_string()),
        "unlimited" => Ok("unlimited".to_string()),
        "lifetime" => Ok("lifetime".to_string()),
        _ => Err("Billing plan must be `basic`, `unlimited`, or `lifetime`.".to_string()),
    }
}

fn normalize_billing_interval(interval: &str) -> Result<String, String> {
    match interval.trim().to_ascii_lowercase().as_str() {
        "month" => Ok("month".to_string()),
        "year" => Ok("year".to_string()),
        _ => Err("Billing interval must be `month` or `year`.".to_string()),
    }
}

fn current_halv_session(storage: &Storage) -> Result<(String, String), String> {
    let session = storage
        .get_auth_session()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Sign in to your Halv account first.".to_string())?;
    let token = session
        .token
        .ok_or_else(|| "Sign in to your Halv account first.".to_string())?;
    let server_url = normalize_server_url(&session.server_url)
        .map_err(|error| format!("Stored server URL is invalid: {error}"))?;
    Ok((server_url, token))
}

async fn create_billing_checkout_url(
    storage: &Storage,
    plan: &str,
    interval: &str,
) -> Result<String, String> {
    let plan = normalize_billing_plan(plan)?;
    let interval = normalize_billing_interval(interval)?;
    let _guard = halv_account_lock().lock().await;
    let (server_url, token) = current_halv_session(storage)?;
    let response = api_client()
        .post(api_url(&server_url, "/v1/billing/checkout"))
        .bearer_auth(&token)
        .json(&BillingCheckoutRequest {
            plan: &plan,
            interval: &interval,
        })
        .send()
        .await
        .map_err(|error| format!("Could not start checkout: {error}"))?;
    parse_billing_url_response(response, "Could not start checkout").await
}

async fn parse_billing_url_response(
    response: reqwest::Response,
    error_prefix: &str,
) -> Result<String, String> {
    let status = response.status();
    let body = read_response_body(response, "billing response").await?;
    if !status.is_success() {
        return Err(http_error_message(status, &body, error_prefix));
    }
    let response: BillingUrlResponse = serde_json::from_str(&body)
        .map_err(|error| format!("Billing response was not valid JSON: {error}"))?;
    normalize_checkout_url(&response.url)
}

fn emit_billing_checkout_event(app: &AppHandle, outcome: BillingCheckoutOutcome) {
    if let Err(error) = app.emit(BILLING_CHECKOUT_EVENT, BillingCheckoutEvent { outcome }) {
        eprintln!("Could not emit billing checkout completion: {error}");
    }
}

fn open_checkout_window(app: &AppHandle, checkout_url: &str) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(BILLING_CHECKOUT_WINDOW_LABEL) {
        window
            .set_focus()
            .map_err(|error| format!("Could not focus checkout: {error}"))?;
        return Ok(());
    }

    let (navigation_policy, parsed_url) = CheckoutNavigationPolicy::new(checkout_url)?;
    let completion_sent = Arc::new(AtomicBool::new(false));
    let page_completion_sent = completion_sent.clone();
    let page_app = app.clone();

    let window = WebviewWindowBuilder::new(
        app,
        BILLING_CHECKOUT_WINDOW_LABEL,
        WebviewUrl::External(parsed_url),
    )
    .title("Secure checkout · Halv")
    .inner_size(520.0, 760.0)
    .min_inner_size(420.0, 600.0)
    .resizable(true)
    .closable(true)
    .center()
    .on_navigation(move |candidate| navigation_policy.allows(candidate))
    .on_new_window(|_, _| NewWindowResponse::Deny)
    .on_page_load(move |window, payload| {
        let Some(outcome) = billing_checkout_outcome(payload.url()) else {
            return;
        };
        if page_completion_sent.swap(true, Ordering::SeqCst) {
            return;
        }
        emit_billing_checkout_event(&page_app, outcome);
        if let Err(error) = window.close() {
            eprintln!("Could not close completed billing checkout: {error}");
        }
    })
    .build()
    .map_err(|error| format!("Could not open checkout window: {error}"))?;

    let close_app = app.clone();
    window.on_window_event(move |event| {
        if matches!(
            event,
            WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed
        ) && !completion_sent.swap(true, Ordering::SeqCst)
        {
            emit_billing_checkout_event(&close_app, BillingCheckoutOutcome::Cancelled);
        }
    });

    Ok(())
}

fn normalize_email(email: &str) -> Result<String, String> {
    let trimmed = email.trim();
    if trimmed.is_empty() {
        return Err("Email is required".to_string());
    }
    if !trimmed.contains('@') {
        return Err("Enter a valid email address".to_string());
    }
    Ok(trimmed.to_string())
}

fn normalize_code(code: &str) -> Result<String, String> {
    let trimmed = code.trim();
    if trimmed.len() != 6 || !trimmed.chars().all(|character| character.is_ascii_digit()) {
        return Err("Enter the 6-digit code".to_string());
    }
    Ok(trimmed.to_string())
}

pub(crate) fn api_url(server_url: &str, path: &str) -> String {
    format!("{}{}", server_url.trim_end_matches('/'), path)
}

fn build_halv_oauth_start_url(
    server_url: &str,
    provider: HalvOAuthProvider,
    flow: &PkceFlow,
    port: u16,
) -> Result<String, String> {
    let mut url = Url::parse(&api_url(
        server_url,
        &format!("/v1/auth/oauth/{}/start", provider.as_str()),
    ))
    .map_err(|error| format!("Could not build account sign-in URL: {error}"))?;
    let port = port.to_string();
    url.query_pairs_mut()
        .append_pair("state", &flow.state)
        .append_pair("port", &port)
        .append_pair("challenge", &flow.challenge);
    Ok(url.to_string())
}

fn parse_login_response(body: &str, context: &str) -> Result<VerifyResponse, String> {
    serde_json::from_str(body)
        .map_err(|error| format!("{context} response was not valid JSON: {error}"))
}

fn parse_oauth_providers_response(body: &str) -> Result<Vec<String>, String> {
    serde_json::from_str::<HealthResponse>(body)
        .map(|response| response.oauth_providers)
        .map_err(|error| format!("Sign-in provider response was not valid JSON: {error}"))
}

async fn read_response_body(response: reqwest::Response, context: &str) -> Result<String, String> {
    response
        .text()
        .await
        .map_err(|error| format!("Could not read {context}: {error}"))
}

async fn exchange_halv_oauth_code(
    server_url: &str,
    code: &str,
    verifier: &str,
) -> Result<VerifyResponse, String> {
    let response = api_client()
        .post(api_url(server_url, "/v1/auth/oauth/exchange"))
        .json(&OAuthExchangeRequest { code, verifier })
        .send()
        .await
        .map_err(|error| format!("Could not complete account sign-in: {error}"))?;
    let status = response.status();
    let body = read_response_body(response, "account sign-in exchange response").await?;

    if !status.is_success() {
        return Err(http_error_message(
            status,
            &body,
            "Could not complete account sign-in",
        ));
    }

    parse_login_response(&body, "Account sign-in exchange")
}

fn http_error_message(status: StatusCode, body: &str, prefix: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| body.trim().to_string());
    if detail.is_empty() {
        format!("{prefix} (HTTP {status})")
    } else {
        format!("{prefix}: {detail}")
    }
}

async fn fetch_me(server_url: &str, token: &str) -> Result<MeResponse, HalvApiError> {
    let response = api_client()
        .get(api_url(server_url, "/v1/me"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|error| {
            HalvApiError::Recoverable(format!("Could not refresh account status: {error}"))
        })?;
    let status = response.status();
    let body = response.text().await.map_err(|error| {
        HalvApiError::Recoverable(format!("Could not read account status response: {error}"))
    })?;

    if status == StatusCode::UNAUTHORIZED {
        return Err(HalvApiError::Unauthorized);
    }
    if !status.is_success() {
        return Err(HalvApiError::Recoverable(http_error_message(
            status,
            &body,
            "Could not refresh account status",
        )));
    }

    serde_json::from_str(&body).map_err(|error| {
        HalvApiError::Message(format!(
            "Account status response was not valid JSON: {error}"
        ))
    })
}

fn save_me_response(
    storage: &Storage,
    server_url: &str,
    token: &str,
    response: MeResponse,
) -> Result<bool, crate::storage::StorageError> {
    let MeResponse {
        user,
        mut entitlements,
        referral,
    } = response;
    entitlements.referral = referral;
    let entitlements_json = serde_json::to_string(&entitlements).map_err(|error| {
        crate::storage::StorageError::Validation(format!("failed to encode entitlements: {error}"))
    })?;
    let updated = storage.update_auth_session_if_current(
        server_url,
        token,
        &user.email,
        &user.plan,
        &entitlements_json,
        &Utc::now().to_rfc3339(),
    )?;
    if updated {
        wake_savings_reporter();
    }
    Ok(updated)
}

fn local_auth_status(
    storage: &Storage,
    stale: bool,
) -> Result<AuthStatus, crate::storage::StorageError> {
    local_auth_status_with_bypass(storage, stale, account_bypass_active())
}

fn local_auth_status_with_bypass(
    storage: &Storage,
    stale: bool,
    bypass_active: bool,
) -> Result<AuthStatus, crate::storage::StorageError> {
    let free_turns_used = storage.free_turns_used()?;
    let first_launch_at = storage.first_launch_at()?;
    if bypass_active {
        return Ok(account_bypass_status(free_turns_used, first_launch_at));
    }

    let session = storage.get_auth_session()?;
    Ok(status_from_session(
        session,
        stale,
        free_turns_used,
        first_launch_at,
    ))
}

fn status_from_session(
    session: Option<AuthSession>,
    stale: bool,
    free_turns_used: u64,
    first_launch_at: String,
) -> AuthStatus {
    let server_url = session
        .as_ref()
        .map(|session| session.server_url.clone())
        .unwrap_or_else(|| default_halv_api_url().to_string());
    let signed_in = session
        .as_ref()
        .and_then(|session| session.token.as_ref())
        .is_some();
    let email = session.as_ref().and_then(|session| session.email.clone());
    let plan = session.as_ref().and_then(|session| session.plan.clone());
    let entitlements = session
        .as_ref()
        .and_then(|session| session.entitlements_json.as_deref())
        .and_then(|value| serde_json::from_str::<Entitlements>(value).ok());
    let referral = entitlements
        .as_ref()
        .and_then(|entitlements| entitlements.referral.clone());
    let synced_at = session
        .as_ref()
        .and_then(|session| session.synced_at.clone());

    AuthStatus {
        signed_in,
        email,
        plan,
        server_url,
        entitlements,
        referral,
        synced_at,
        first_launch_at,
        free_turns_used,
        stale,
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(target_os = "macos")]
fn open_checkout_url(url: &str) -> Result<(), String> {
    Command::new("open")
        .arg(url)
        .spawn()
        .map_err(|error| format!("Could not open checkout: {error}"))?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn open_checkout_url(url: &str) -> Result<(), String> {
    // rundll32 is a console program; the browser it opens shows its own window.
    let mut command = Command::new("rundll32");
    command.args(["url.dll,FileProtocolHandler", url]);
    crate::process::hide_console(&mut command)
        .spawn()
        .map_err(|error| format!("Could not open checkout: {error}"))?;
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_checkout_url(url: &str) -> Result<(), String> {
    Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map_err(|error| format!("Could not open checkout: {error}"))?;
    Ok(())
}

#[allow(dead_code)]
pub(crate) async fn auth_get_bearer() -> Result<AuthBearer, String> {
    auth_get_bearer_inner()
        .await
        .map_err(|error| error.to_string())
}

async fn credential_auth_status_inner() -> CredentialAuthStatus {
    if let Some(tokens) = load_app_tokens_or_absent("auth_status") {
        match ensure_app_access_token(tokens.clone()).await {
            Ok(refreshed) => {
                if let Some(status) = chatgpt_status(
                    "app",
                    &refreshed.id_token,
                    &refreshed.access_token,
                    refreshed.account_id.clone(),
                ) {
                    return status;
                }
            }
            Err(AuthError::PermanentRefresh {
                failed_refresh_token,
                ..
            }) => {
                let _ = clear_app_tokens_if_refresh_token_matches(&failed_refresh_token);
            }
            Err(AuthError::Cancelled)
            | Err(AuthError::TransientRefresh(_))
            | Err(AuthError::Message(_)) => {
                if let Some(status) = chatgpt_status(
                    "app",
                    &tokens.id_token,
                    &tokens.access_token,
                    tokens.account_id.clone(),
                ) {
                    return status;
                }
            }
        }
    }

    if let Some(tokens) = load_unexpired_codex_tokens() {
        return chatgpt_status(
            "codex-cli",
            &tokens.id_token,
            &tokens.access_token,
            Some(tokens.account_id),
        )
        .unwrap_or(CredentialAuthStatus::SignedOut {});
    }

    if let Ok(Some(api_key)) = get_keychain_secret(KEYCHAIN_API_KEY_ACCOUNT) {
        return CredentialAuthStatus::ApiKey {
            masked: mask_secret(&api_key),
        };
    }

    if let Ok(Some(api_key)) = get_keychain_secret(KEYCHAIN_ANTHROPIC_API_KEY_ACCOUNT) {
        return CredentialAuthStatus::AnthropicApiKey {
            source: "api-key".to_string(),
            masked: mask_secret(&api_key),
        };
    }

    CredentialAuthStatus::SignedOut {}
}

#[allow(dead_code)]
async fn auth_get_bearer_inner() -> AuthResult<AuthBearer> {
    if let Some(tokens) = load_app_tokens_or_absent("auth_get_bearer") {
        match ensure_app_access_token(tokens).await {
            Ok(tokens) => {
                let claims = parse_id_token_claims(&tokens.id_token);
                let account_id = claims.account_id.or(tokens.account_id).ok_or_else(|| {
                    AuthError::message("ChatGPT access token is missing account id")
                })?;
                return Ok(AuthBearer {
                    token: tokens.access_token,
                    source: "app".to_string(),
                    account_id: Some(account_id),
                    is_api_key: false,
                });
            }
            Err(AuthError::PermanentRefresh {
                failed_refresh_token,
                ..
            }) => {
                clear_app_tokens_if_refresh_token_matches(&failed_refresh_token)?;
            }
            Err(error) => return Err(error),
        }
    }

    if let Some(tokens) = load_unexpired_codex_tokens() {
        return Ok(AuthBearer {
            token: tokens.access_token,
            source: "codex-cli".to_string(),
            account_id: Some(tokens.account_id),
            is_api_key: false,
        });
    }

    if let Some(api_key) = get_keychain_secret(KEYCHAIN_API_KEY_ACCOUNT)? {
        return Ok(AuthBearer {
            token: api_key,
            source: "api-key".to_string(),
            account_id: None,
            is_api_key: true,
        });
    }

    Err(AuthError::message(
        "Not connected. Connect ChatGPT or set an API key.",
    ))
}

async fn login_chatgpt_inner() -> AuthResult<CredentialAuthStatus> {
    let _guard = LoginInFlightGuard::acquire()?;
    login_chatgpt_flow().await
}

async fn login_chatgpt_flow() -> AuthResult<CredentialAuthStatus> {
    let flow = PkceFlow::generate();
    let (listener, port) = tauri::async_runtime::spawn_blocking(bind_callback_listener)
        .await
        .map_err(|error| AuthError::message(format!("OAuth bind task failed: {error}")))??;
    let redirect_uri = redirect_uri(port);
    let authorize_url = build_authorize_url(&flow, &redirect_uri, &[])?;
    let callback_server =
        spawn_callback_server(listener, flow.state.clone(), CallbackFlow::Chatgpt)?;
    let _cancel_guard = ChatgptOAuthCancelGuard::activate(Arc::clone(&callback_server.cancel));

    if let Err(error) = tauri_plugin_opener::open_url(&authorize_url, None::<&str>) {
        callback_server.cancel.store(true, Ordering::SeqCst);
        return Err(AuthError::message(format!(
            "Failed to open browser for ChatGPT login: {error}"
        )));
    }

    let code = wait_for_callback(callback_server).await?;
    let token_response = exchange_authorization_code(&code, &redirect_uri, &flow.verifier).await?;
    let claims = parse_id_token_claims(&token_response.id_token);
    let account_id = claims
        .account_id
        .ok_or_else(|| AuthError::message("ChatGPT id_token did not include chatgpt_account_id"))?;

    let tokens = AppTokens {
        id_token: token_response.id_token,
        access_token: token_response.access_token,
        refresh_token: token_response.refresh_token,
        account_id: Some(account_id),
        last_refresh: Utc::now(),
    };
    store_app_tokens(&tokens)?;

    Ok(credential_auth_status_inner().await)
}

struct LoginInFlightGuard;

impl LoginInFlightGuard {
    fn acquire() -> AuthResult<Self> {
        LOGIN_IN_FLIGHT
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| Self)
            .map_err(|_| AuthError::message("A ChatGPT login is already in progress"))
    }
}

impl Drop for LoginInFlightGuard {
    fn drop(&mut self) {
        LOGIN_IN_FLIGHT.store(false, Ordering::SeqCst);
    }
}

struct AccountLoginInFlightGuard;

impl AccountLoginInFlightGuard {
    fn acquire() -> AuthResult<Self> {
        ACCOUNT_LOGIN_IN_FLIGHT
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| Self)
            .map_err(|_| AuthError::message("An account sign-in is already in progress"))
    }
}

impl Drop for AccountLoginInFlightGuard {
    fn drop(&mut self) {
        ACCOUNT_LOGIN_IN_FLIGHT.store(false, Ordering::SeqCst);
    }
}

struct AccountOAuthCancelGuard {
    cancel: Arc<AtomicBool>,
}

impl AccountOAuthCancelGuard {
    fn activate(cancel: Arc<AtomicBool>) -> Self {
        let mut active = ACTIVE_ACCOUNT_OAUTH_CANCEL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *active = Some(Arc::clone(&cancel));
        Self { cancel }
    }
}

impl Drop for AccountOAuthCancelGuard {
    fn drop(&mut self) {
        let mut active = ACTIVE_ACCOUNT_OAUTH_CANCEL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active
            .as_ref()
            .is_some_and(|cancel| Arc::ptr_eq(cancel, &self.cancel))
        {
            *active = None;
        }
    }
}

struct ChatgptOAuthCancelGuard {
    cancel: Arc<AtomicBool>,
}

impl ChatgptOAuthCancelGuard {
    fn activate(cancel: Arc<AtomicBool>) -> Self {
        let mut active = ACTIVE_CHATGPT_OAUTH_CANCEL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *active = Some(Arc::clone(&cancel));
        Self { cancel }
    }
}

impl Drop for ChatgptOAuthCancelGuard {
    fn drop(&mut self) {
        let mut active = ACTIVE_CHATGPT_OAUTH_CANCEL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active
            .as_ref()
            .is_some_and(|cancel| Arc::ptr_eq(cancel, &self.cancel))
        {
            *active = None;
        }
    }
}

fn ensure_oauth_not_cancelled(cancel: &AtomicBool) -> AuthResult<()> {
    if cancel.load(Ordering::SeqCst) {
        Err(AuthError::Cancelled)
    } else {
        Ok(())
    }
}

impl PkceFlow {
    fn generate() -> Self {
        let verifier = random_base64url_64_bytes();
        let challenge = pkce_challenge(&verifier);
        let state = random_base64url_32_bytes();
        Self {
            verifier,
            challenge,
            state,
        }
    }
}

fn random_base64url_64_bytes() -> String {
    let bytes: [u8; 64] = rand::random();
    URL_SAFE_NO_PAD.encode(bytes)
}

fn random_base64url_32_bytes() -> String {
    let bytes: [u8; 32] = rand::random();
    URL_SAFE_NO_PAD.encode(bytes)
}

fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn build_authorize_url(
    flow: &PkceFlow,
    redirect_uri: &str,
    allowed_workspace_ids: &[String],
) -> AuthResult<String> {
    build_authorize_url_with_config(
        flow,
        redirect_uri,
        &client_id(),
        &originator(),
        allowed_workspace_ids,
    )
}

fn build_authorize_url_with_config(
    flow: &PkceFlow,
    redirect_uri: &str,
    client_id: &str,
    originator: &str,
    allowed_workspace_ids: &[String],
) -> AuthResult<String> {
    let mut url = Url::parse(AUTHORIZE_URL)
        .map_err(|error| AuthError::message(format!("Invalid authorize URL: {error}")))?;
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("scope", SCOPE)
            .append_pair("code_challenge", &flow.challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("id_token_add_organizations", "true")
            .append_pair("codex_cli_simplified_flow", "true")
            .append_pair("state", &flow.state)
            .append_pair("originator", originator);

        if !allowed_workspace_ids.is_empty() {
            query.append_pair("allowed_workspace_id", &allowed_workspace_ids.join(","));
        }
    }
    Ok(url.to_string())
}

async fn exchange_authorization_code(
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> AuthResult<AuthCodeExchangeResponse> {
    let client_id = client_id();
    let request = AuthCodeExchangeRequest {
        grant_type: "authorization_code",
        code,
        redirect_uri,
        client_id: &client_id,
        code_verifier: verifier,
    };

    let response = reqwest::Client::new()
        .post(TOKEN_URL)
        .form(&request)
        .send()
        .await
        .map_err(|error| {
            AuthError::message(format!("ChatGPT code exchange request failed: {error}"))
        })?;

    let status = response.status();
    let body = response.text().await.map_err(|error| {
        AuthError::message(format!(
            "ChatGPT code exchange response read failed: {error}"
        ))
    })?;

    if !status.is_success() {
        return Err(AuthError::message(format!(
            "ChatGPT code exchange failed with HTTP {status}: {body}"
        )));
    }

    serde_json::from_str(&body).map_err(|error| {
        AuthError::message(format!(
            "ChatGPT code exchange response was not the expected token JSON: {error}"
        ))
    })
}

async fn ensure_app_access_token(tokens: AppTokens) -> AuthResult<AppTokens> {
    let now = Utc::now();
    if !access_token_needs_refresh(&tokens.access_token, tokens.last_refresh, now) {
        return Ok(tokens);
    }

    let _guard = refresh_lock().lock().await;
    let current = load_app_tokens_or_absent("refresh re-check").unwrap_or(tokens);
    let now = Utc::now();
    if !access_token_needs_refresh(&current.access_token, current.last_refresh, now) {
        return Ok(current);
    }

    refresh_app_tokens_for_account(current, KEYCHAIN_CHATGPT_ACCOUNT).await
}

async fn ensure_app_access_token_for_account(
    tokens: AppTokens,
    account: &str,
) -> AuthResult<AppTokens> {
    let now = Utc::now();
    if !access_token_needs_refresh(&tokens.access_token, tokens.last_refresh, now) {
        return Ok(tokens);
    }

    let _guard = refresh_lock().lock().await;
    let current = load_app_tokens_from_account(account)?.unwrap_or(tokens);
    let now = Utc::now();
    if !access_token_needs_refresh(&current.access_token, current.last_refresh, now) {
        return Ok(current);
    }

    refresh_app_tokens_for_account(current, account).await
}

fn refresh_lock() -> &'static AsyncMutex<()> {
    static LOCK: OnceLock<AsyncMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| AsyncMutex::new(()))
}

async fn refresh_app_tokens_for_account(
    mut tokens: AppTokens,
    account: &str,
) -> AuthResult<AppTokens> {
    let client_id = client_id();
    let failed_refresh_token = tokens.refresh_token.clone();
    let request = RefreshRequest::new(&client_id, &tokens.refresh_token);
    let response = reqwest::Client::new()
        .post(refresh_token_url())
        .json(&request)
        .send()
        .await
        .map_err(|error| {
            AuthError::transient_refresh(format!("ChatGPT token refresh request failed: {error}"))
        })?;

    let status = response.status();
    let body = response.text().await.map_err(|error| {
        AuthError::transient_refresh(format!(
            "ChatGPT token refresh response read failed: {error}"
        ))
    })?;

    if !status.is_success() {
        let classification = classify_refresh_failure(status, &body);
        let message = format!("ChatGPT token refresh failed with HTTP {status}: {body}");
        return match classification {
            RefreshFailureKind::Permanent => {
                Err(AuthError::permanent_refresh(message, failed_refresh_token))
            }
            RefreshFailureKind::Transient => Err(AuthError::transient_refresh(message)),
        };
    }

    let refresh_response: RefreshResponse = serde_json::from_str(&body).map_err(|error| {
        AuthError::transient_refresh(format!(
            "ChatGPT token refresh response was not valid JSON: {error}"
        ))
    })?;

    let now = Utc::now();
    if let Some(refresh_token) = refresh_response
        .refresh_token
        .filter(|refresh_token| !refresh_token.is_empty())
    {
        tokens.refresh_token = refresh_token;
        tokens.last_refresh = now;
        store_app_tokens_for_account(&tokens, account)?;
    }

    if let Some(id_token) = refresh_response.id_token {
        tokens.id_token = id_token;
    }
    if let Some(access_token) = refresh_response.access_token {
        tokens.access_token = access_token;
    }
    let claims = parse_id_token_claims(&tokens.id_token);
    if claims.account_id.is_some() {
        tokens.account_id = claims.account_id;
    }
    tokens.last_refresh = now;
    store_app_tokens_for_account(&tokens, account)?;

    if access_token_needs_refresh(&tokens.access_token, tokens.last_refresh, now) {
        return Err(AuthError::transient_refresh(
            "ChatGPT token refresh did not return a usable access token",
        ));
    }

    Ok(tokens)
}

fn classify_refresh_failure(status: StatusCode, body: &str) -> RefreshFailureKind {
    match refresh_error_code(body).as_deref() {
        Some("refresh_token_expired")
        | Some("refresh_token_reused")
        | Some("refresh_token_invalidated") => RefreshFailureKind::Permanent,
        _ if status == StatusCode::UNAUTHORIZED => RefreshFailureKind::Permanent,
        _ => RefreshFailureKind::Transient,
    }
}

fn refresh_error_code(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let code = value
        .get("error")
        .and_then(|error| {
            error
                .get("code")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
        })
        .or_else(|| value.get("code").and_then(Value::as_str))?;
    Some(code.to_ascii_lowercase())
}

fn access_token_needs_refresh(
    access_token: &str,
    last_refresh: DateTime<Utc>,
    now: DateTime<Utc>,
) -> bool {
    match jwt_exp(access_token) {
        Some(exp) => exp <= (now + ACCESS_REFRESH_WINDOW).timestamp(),
        None => last_refresh < now - ACCESS_REFRESH_FALLBACK_AGE,
    }
}

fn access_token_unexpired(access_token: &str, now: DateTime<Utc>) -> bool {
    jwt_exp(access_token)
        .map(|exp| exp > now.timestamp())
        .unwrap_or(false)
}

fn jwt_exp(jwt: &str) -> Option<i64> {
    jwt_payload_value(jwt)?.get("exp")?.as_i64().or_else(|| {
        jwt_payload_value(jwt)?
            .get("exp")?
            .as_u64()
            .map(|value| value as i64)
    })
}

fn parse_id_token_claims(id_token: &str) -> IdTokenClaims {
    let Some(payload) = jwt_payload_value(id_token) else {
        return IdTokenClaims::default();
    };

    let auth_claim = payload.get(OPENAI_AUTH_CLAIM);
    let profile_claim = payload.get(OPENAI_PROFILE_CLAIM);

    let account_id = auth_claim
        .and_then(|claim| claim.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let plan_type = auth_claim
        .and_then(|claim| claim.get("chatgpt_plan_type"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let email = payload
        .get("email")
        .and_then(Value::as_str)
        .or_else(|| {
            profile_claim
                .and_then(|claim| claim.get("email"))
                .and_then(Value::as_str)
        })
        .map(ToOwned::to_owned);

    IdTokenClaims {
        account_id,
        email,
        plan_type,
    }
}

fn jwt_payload_value(jwt: &str) -> Option<Value> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn chatgpt_status(
    source: &str,
    id_token: &str,
    access_token: &str,
    account_id_fallback: Option<String>,
) -> Option<CredentialAuthStatus> {
    let claims = parse_id_token_claims(id_token);
    let account_id = claims.account_id.or(account_id_fallback)?;
    Some(CredentialAuthStatus::Chatgpt {
        source: source.to_string(),
        email: claims.email,
        plan_type: claims.plan_type,
        account_id,
        access_token_valid: access_token_unexpired(access_token, Utc::now()),
    })
}

fn load_app_tokens() -> AuthResult<Option<AppTokens>> {
    load_app_tokens_from_account(KEYCHAIN_CHATGPT_ACCOUNT)
}

fn load_app_tokens_from_account(account: &str) -> AuthResult<Option<AppTokens>> {
    let Some(secret) = get_keychain_secret(account)? else {
        return Ok(None);
    };
    let tokens = serde_json::from_str(&secret).map_err(|error| {
        AuthError::message(format!("Stored ChatGPT tokens are not valid JSON: {error}"))
    })?;
    Ok(Some(tokens))
}

fn load_app_tokens_or_absent(context: &str) -> Option<AppTokens> {
    match load_app_tokens() {
        Ok(tokens) => tokens,
        Err(error) => {
            eprintln!("Ignoring stored ChatGPT tokens during {context}: {error}");
            None
        }
    }
}

fn store_app_tokens(tokens: &AppTokens) -> AuthResult<()> {
    store_app_tokens_for_account(tokens, KEYCHAIN_CHATGPT_ACCOUNT)
}

fn store_app_tokens_for_account(tokens: &AppTokens, account: &str) -> AuthResult<()> {
    let secret = serde_json::to_string(tokens).map_err(|error| {
        AuthError::message(format!("Failed to serialize ChatGPT tokens: {error}"))
    })?;
    set_keychain_secret(account, &secret)
}

fn get_keychain_secret(account: &str) -> AuthResult<Option<String>> {
    get_keychain_secret_with(
        account,
        read_keychain_secret,
        write_keychain_secret,
        delete_keychain_secret,
    )
}

fn get_keychain_secret_with<R, W, D>(
    account: &str,
    mut read: R,
    mut write: W,
    mut delete: D,
) -> AuthResult<Option<String>>
where
    R: FnMut(&str, &str) -> AuthResult<Option<String>>,
    W: FnMut(&str, &str, &str) -> AuthResult<()>,
    D: FnMut(&str, &str) -> AuthResult<()>,
{
    if let Some(secret) = read(SERVICE_NAME, account)? {
        return Ok(Some(secret));
    }

    let Some(secret) = read(LEGACY_SERVICE_NAME, account)? else {
        return Ok(None);
    };

    if let Err(error) = write(SERVICE_NAME, account, &secret) {
        eprintln!(
            "Keychain migration could not store legacy {LEGACY_SERVICE_NAME}/{account} entry under {SERVICE_NAME}/{account}: {error}"
        );
        return Ok(Some(secret));
    }

    if let Err(error) = delete(LEGACY_SERVICE_NAME, account) {
        eprintln!(
            "Keychain migration could not delete legacy {LEGACY_SERVICE_NAME}/{account} entry: {error}"
        );
    }

    Ok(Some(secret))
}

/// Reads one keychain entry. On Windows, a large secret comes from its
/// sealed file (see `secret_store`).
fn read_keychain_secret(service: &str, account: &str) -> AuthResult<Option<String>> {
    secret_store::read_secret(service, account).map_err(AuthError::message)
}

fn set_keychain_secret(account: &str, secret: &str) -> AuthResult<()> {
    write_keychain_secret(SERVICE_NAME, account, secret)
}

/// Writes one keychain entry. On Windows, a secret above the credential
/// blob limit goes to a sealed file, because the credential manager rejects
/// it (see `secret_store`).
fn write_keychain_secret(service: &str, account: &str, secret: &str) -> AuthResult<()> {
    secret_store::write_secret(service, account, secret).map_err(AuthError::message)
}

fn clear_keychain_secret(account: &str) -> AuthResult<()> {
    let result = delete_keychain_secret(SERVICE_NAME, account);
    if let Err(error) = delete_keychain_secret(LEGACY_SERVICE_NAME, account) {
        eprintln!(
            "Ignoring failure while clearing legacy {LEGACY_SERVICE_NAME}/{account} keychain entry: {error}"
        );
    }
    result
}

/// Deletes one keychain entry and its sealed file, when one exists.
fn delete_keychain_secret(service: &str, account: &str) -> AuthResult<()> {
    secret_store::delete_secret(service, account).map_err(AuthError::message)
}

/// The keychain store of the Halv session token: the service `Halv`, one
/// account per session (issue #50). The value binds the token to its server
/// URL (see `storage::session_token`). Test builds use an in-memory store.
#[cfg_attr(test, allow(dead_code))]
pub(crate) struct KeychainSessionTokenStore;

impl crate::storage::session_token::SessionTokenStore for KeychainSessionTokenStore {
    fn read(&self, account: &str) -> Result<Option<String>, String> {
        read_keychain_secret(SERVICE_NAME, account).map_err(|error| error.to_string())
    }

    fn write(&self, account: &str, value: &str) -> Result<(), String> {
        set_keychain_secret(account, value).map_err(|error| error.to_string())
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        delete_keychain_secret(SERVICE_NAME, account).map_err(|error| error.to_string())
    }
}

fn clear_api_key_secret() -> AuthResult<()> {
    clear_api_key_secret_with(clear_keychain_secret)
}

fn clear_api_key_secret_with<F>(clear: F) -> AuthResult<()>
where
    F: FnOnce(&str) -> AuthResult<()>,
{
    clear(KEYCHAIN_API_KEY_ACCOUNT)
}

fn clear_app_tokens_if_refresh_token_matches(failed_refresh_token: &str) -> AuthResult<()> {
    clear_app_tokens_if_refresh_token_matches_for_account(
        KEYCHAIN_CHATGPT_ACCOUNT,
        failed_refresh_token,
    )
}

fn clear_app_tokens_if_refresh_token_matches_for_account(
    account: &str,
    failed_refresh_token: &str,
) -> AuthResult<()> {
    match load_app_tokens_from_account(account) {
        Ok(Some(tokens))
            if should_clear_app_tokens_after_refresh_failure(
                Some(&tokens),
                failed_refresh_token,
            ) =>
        {
            clear_keychain_secret(account)
        }
        Ok(_) => Ok(()),
        Err(error) => {
            eprintln!(
                "Not clearing stored ChatGPT tokens in {account} after refresh failure because the current entry could not be read: {error}"
            );
            Ok(())
        }
    }
}

fn should_clear_app_tokens_after_refresh_failure(
    stored_tokens: Option<&AppTokens>,
    failed_refresh_token: &str,
) -> bool {
    stored_tokens
        .map(|tokens| tokens.refresh_token == failed_refresh_token)
        .unwrap_or(false)
}

fn load_unexpired_codex_tokens() -> Option<BorrowedCodexTokens> {
    let path = codex_auth_path()?;
    let contents = fs::read_to_string(path).ok()?;
    borrowed_codex_tokens_from_str(&contents, Utc::now())
        .ok()
        .flatten()
}

fn borrowed_codex_tokens_from_str(
    contents: &str,
    now: DateTime<Utc>,
) -> serde_json::Result<Option<BorrowedCodexTokens>> {
    let auth_json: CodexAuthJson = serde_json::from_str(contents)?;
    let mode_is_chatgpt = match auth_json.auth_mode.as_deref() {
        Some(mode) => mode.eq_ignore_ascii_case("chatgpt"),
        None => {
            auth_json.tokens.is_some()
                && !has_non_empty_credential(auth_json.openai_api_key.as_ref())
                && !has_non_empty_credential(auth_json.personal_access_token.as_ref())
                && !has_non_empty_credential(auth_json.bedrock_api_key.as_ref())
        }
    };
    if !mode_is_chatgpt {
        return Ok(None);
    }

    let Some(tokens) = auth_json.tokens else {
        return Ok(None);
    };

    // Codex refresh tokens are single-use and rotating. Halv may borrow
    // an unexpired access token from ~/.codex/auth.json, but must never use its
    // refresh_token or write auth.json, or it can invalidate the user's Codex CLI
    // session.
    if !access_token_unexpired(&tokens.access_token, now) {
        return Ok(None);
    }

    let claims = parse_id_token_claims(&tokens.id_token);
    let Some(account_id) = claims.account_id.or(tokens.account_id) else {
        return Ok(None);
    };

    Ok(Some(BorrowedCodexTokens {
        id_token: tokens.id_token,
        access_token: tokens.access_token,
        account_id,
    }))
}

fn has_non_empty_credential(value: Option<&String>) -> bool {
    value.map(|value| !value.trim().is_empty()).unwrap_or(false)
}

fn codex_auth_path() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(PathBuf::from))
        .map(|home| home.join(".codex").join("auth.json"))
}

fn bind_account_callback_listener() -> AuthResult<(TcpListener, u16)> {
    for _ in 0..3 {
        let listener = bind_callback_port(0).map_err(|error| {
            AuthError::message(format!(
                "Failed to bind account sign-in callback on an ephemeral port: {error}"
            ))
        })?;
        let port = listener.local_addr().map_err(|error| {
            AuthError::message(format!(
                "Failed to read account sign-in callback port: {error}"
            ))
        })?;
        if !matches!(port.port(), DEFAULT_CALLBACK_PORT | FALLBACK_CALLBACK_PORT) {
            return Ok((listener, port.port()));
        }
    }

    Err(AuthError::message(
        "Could not reserve a separate callback port for account sign-in",
    ))
}

fn bind_callback_listener() -> AuthResult<(TcpListener, u16)> {
    match bind_callback_port(DEFAULT_CALLBACK_PORT) {
        Ok(listener) => Ok((listener, DEFAULT_CALLBACK_PORT)),
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
            let listener = bind_callback_port(FALLBACK_CALLBACK_PORT).map_err(|fallback_error| {
                AuthError::message(format!(
                    "Failed to bind OAuth callback on ports {DEFAULT_CALLBACK_PORT} and {FALLBACK_CALLBACK_PORT}: {fallback_error}"
                ))
            })?;
            Ok((listener, FALLBACK_CALLBACK_PORT))
        }
        Err(error) => Err(AuthError::message(format!(
            "Failed to bind OAuth callback on port {DEFAULT_CALLBACK_PORT}: {error}"
        ))),
    }
}

fn bind_callback_port(port: u16) -> io::Result<TcpListener> {
    let mut last_error = None;
    for attempt in 0..BIND_RETRIES {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                listener.set_nonblocking(true)?;
                return Ok(listener);
            }
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                last_error = Some(error);
                if attempt + 1 < BIND_RETRIES {
                    thread::sleep(BIND_RETRY_DELAY);
                }
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::AddrInUse, "OAuth callback port is in use")
    }))
}

fn redirect_uri(port: u16) -> String {
    format!("http://localhost:{port}{CALLBACK_PATH}")
}

fn spawn_callback_server(
    listener: TcpListener,
    expected_state: String,
    flow: CallbackFlow,
) -> AuthResult<CallbackServer> {
    spawn_callback_server_with_cancel(
        listener,
        expected_state,
        flow,
        Arc::new(AtomicBool::new(false)),
    )
}

fn spawn_callback_server_with_cancel(
    listener: TcpListener,
    expected_state: String,
    flow: CallbackFlow,
    cancel: Arc<AtomicBool>,
) -> AuthResult<CallbackServer> {
    let (sender, receiver) = mpsc::channel();
    let thread_cancel = Arc::clone(&cancel);

    thread::Builder::new()
        .name("halv-oauth-callback".to_string())
        .spawn(move || {
            let started_at = Instant::now();
            while started_at.elapsed() < LOGIN_TIMEOUT && !thread_cancel.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _address)) => {
                        let _ = stream.set_nonblocking(false);
                        if let Some(result) =
                            handle_callback_stream(&mut stream, &expected_state, flow)
                        {
                            let _ = sender.send(result);
                            break;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(CALLBACK_POLL_INTERVAL);
                    }
                    Err(error) => {
                        let _ =
                            sender.send(Err(format!("OAuth callback listener failed: {error}")));
                        break;
                    }
                }
            }
        })
        .map_err(|error| {
            AuthError::message(format!("Failed to start OAuth callback server: {error}"))
        })?;

    Ok(CallbackServer {
        receiver,
        cancel,
        flow,
    })
}

async fn wait_for_callback(callback_server: CallbackServer) -> AuthResult<String> {
    let cancel = Arc::clone(&callback_server.cancel);
    let flow = callback_server.flow;
    let receiver = callback_server.receiver;
    tauri::async_runtime::spawn_blocking(move || {
        wait_for_callback_result(receiver, cancel, flow, LOGIN_TIMEOUT)
    })
    .await
    .map_err(|error| AuthError::message(format!("OAuth callback wait task failed: {error}")))?
}

fn wait_for_callback_result(
    receiver: mpsc::Receiver<Result<String, String>>,
    cancel: Arc<AtomicBool>,
    flow: CallbackFlow,
    timeout: Duration,
) -> AuthResult<String> {
    let started_at = Instant::now();

    loop {
        ensure_oauth_not_cancelled(&cancel)?;

        let Some(remaining) = timeout.checked_sub(started_at.elapsed()) else {
            cancel.store(true, Ordering::SeqCst);
            return Err(callback_timeout_error(flow));
        };
        if remaining.is_zero() {
            cancel.store(true, Ordering::SeqCst);
            return Err(callback_timeout_error(flow));
        }

        match receiver.recv_timeout(remaining.min(CALLBACK_POLL_INTERVAL)) {
            Ok(Ok(code)) => {
                ensure_oauth_not_cancelled(&cancel)?;
                return Ok(code);
            }
            Ok(Err(error)) => return Err(AuthError::message(error)),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                ensure_oauth_not_cancelled(&cancel)?;
                return Err(AuthError::message(match flow {
                    CallbackFlow::Chatgpt => {
                        "OAuth callback server stopped before receiving a code"
                    }
                    CallbackFlow::HalvAccount => {
                        "Account sign-in callback stopped before receiving a code. Please try again."
                    }
                }));
            }
        }
    }
}

fn callback_timeout_error(flow: CallbackFlow) -> AuthError {
    AuthError::message(match flow {
        CallbackFlow::Chatgpt => "Timed out waiting for ChatGPT login",
        CallbackFlow::HalvAccount => "Timed out waiting for account sign-in. Please try again.",
    })
}

fn handle_callback_stream(
    stream: &mut TcpStream,
    expected_state: &str,
    flow: CallbackFlow,
) -> Option<Result<String, String>> {
    let _ = stream.set_nonblocking(false);
    let read_timeout = match flow {
        CallbackFlow::Chatgpt => Duration::from_secs(5),
        CallbackFlow::HalvAccount => ACCOUNT_CALLBACK_READ_TIMEOUT,
    };
    let _ = stream.set_read_timeout(Some(read_timeout));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let mut buffer = [0_u8; 8192];
    let bytes_read = match stream.read(&mut buffer) {
        Ok(0) => {
            let _ = write_callback_response(stream, "404 Not Found", "Not found.");
            return None;
        }
        Ok(bytes_read) => bytes_read,
        Err(_) => {
            let _ = write_callback_response(stream, "404 Not Found", "Not found.");
            return None;
        }
    };

    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
    let request_line = request.lines().next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();

    if method != "GET" {
        let _ = write_callback_response(stream, "404 Not Found", "Not found.");
        return None;
    }

    let url = match Url::parse(&format!("http://localhost{target}")) {
        Ok(url) => url,
        Err(_) => {
            let _ = write_callback_response(stream, "404 Not Found", "Not found.");
            return None;
        }
    };

    match callback_result_from_url(&url, expected_state) {
        None => {
            let _ = write_callback_response(stream, "404 Not Found", "Not found.");
            None
        }
        Some(Ok(code)) => {
            let _ = write_callback_response(stream, "200 OK", "You can close this tab.");
            Some(Ok(code))
        }
        Some(Err(error)) => {
            let browser_message = match &error {
                CallbackError::Provider(_) => "Login was not completed.",
                CallbackError::MissingCode => "Missing authorization code.",
            };
            let _ = write_callback_response(stream, "400 Bad Request", browser_message);
            Some(Err(callback_error_message(flow, &error)))
        }
    }
}

fn callback_result_from_url(
    url: &Url,
    expected_state: &str,
) -> Option<Result<String, CallbackError>> {
    if url.path() != CALLBACK_PATH {
        return None;
    }

    let query: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();
    if query.get("state").map(String::as_str) != Some(expected_state) {
        return None;
    }
    if let Some(error) = query.get("error") {
        return Some(Err(CallbackError::Provider(error.clone())));
    }

    query
        .get("code")
        .filter(|code| !code.is_empty())
        .cloned()
        .map(Ok)
        .or(Some(Err(CallbackError::MissingCode)))
}

fn callback_error_message(flow: CallbackFlow, error: &CallbackError) -> String {
    match (flow, error) {
        (CallbackFlow::Chatgpt, CallbackError::Provider(error)) => {
            format!("ChatGPT login returned OAuth error: {error}")
        }
        (CallbackFlow::Chatgpt, CallbackError::MissingCode) => {
            "OAuth callback did not include an authorization code".to_string()
        }
        (CallbackFlow::HalvAccount, CallbackError::Provider(error)) => match error.as_str() {
            "access_denied" => "Sign-in was cancelled.".to_string(),
            "invalid_response" => {
                "The sign-in provider returned an invalid response. Please try again.".to_string()
            }
            "provider_error" => {
                "The sign-in provider could not complete sign-in. Please try again.".to_string()
            }
            "server_error" => {
                "The account server could not complete sign-in. Please try again.".to_string()
            }
            _ => "Account sign-in could not be completed. Please try again.".to_string(),
        },
        (CallbackFlow::HalvAccount, CallbackError::MissingCode) => {
            "Account sign-in did not return an authorization code. Please try again.".to_string()
        }
    }
}

fn write_callback_response(stream: &mut TcpStream, status: &str, message: &str) -> io::Result<()> {
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Halv</title></head><body>{message}</body></html>"
    );
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn client_id() -> String {
    env_override(ENV_CLIENT_ID_OVERRIDE).unwrap_or_else(|| DEFAULT_CLIENT_ID.to_string())
}

fn refresh_token_url() -> String {
    env_override(ENV_REFRESH_URL_OVERRIDE).unwrap_or_else(|| TOKEN_URL.to_string())
}

fn originator() -> String {
    env_override(ENV_ORIGINATOR_OVERRIDE).unwrap_or_else(|| DEFAULT_ORIGINATOR.to_string())
}

fn env_override(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn mask_secret(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 8 {
        return "****".to_string();
    }
    let prefix: String = chars.iter().take(3).collect();
    let suffix: String = chars
        .iter()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}...{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn route_task_sends_a_brief_only_after_brief_sharing_is_allowed() {
        use crate::preferences::BriefSharing;
        assert!(require_brief_sharing(BriefSharing::Allowed).is_ok());
        assert!(require_brief_sharing(BriefSharing::Unset).is_err());
        assert!(require_brief_sharing(BriefSharing::Declined).is_err());
    }

    #[test]
    fn checkout_payload_has_no_client_selected_currency() {
        let payload = serde_json::to_value(BillingCheckoutRequest {
            plan: "basic",
            interval: "month",
        })
        .unwrap();
        assert_eq!(payload, json!({ "plan": "basic", "interval": "month" }));
    }

    #[test]
    fn route_decision_reads_snake_case_and_writes_camel_case() {
        let server_body = r#"{"schema":"task-tier-v1","provider":"jev","model":"typesafe/jev-1.13-20260917","complexity":{"choice":"standard","confidence":0.78,"probabilities":{"trivial":0.15,"standard":0.78,"hard":0.07}},"needs_reasoning":0.12,"risky":0.03,"usage":{"input_tokens":392,"output_tokens":65},"latency_ms":143}"#;
        let decision: RouteDecision = serde_json::from_str(server_body).unwrap();
        assert_eq!(decision.schema, "task-tier-v1");
        assert_eq!(decision.complexity.choice, "standard");
        assert_eq!(decision.complexity.probabilities["hard"], 0.07);
        assert_eq!(decision.needs_reasoning, 0.12);
        assert_eq!(decision.usage.input_tokens, Some(392));
        assert_eq!(decision.latency_ms, 143);

        assert_eq!(
            serde_json::to_value(&decision).unwrap(),
            json!({
                "schema": "task-tier-v1",
                "provider": "jev",
                "model": "typesafe/jev-1.13-20260917",
                "complexity": {
                    "choice": "standard",
                    "confidence": 0.78,
                    "probabilities": { "trivial": 0.15, "standard": 0.78, "hard": 0.07 }
                },
                "needsReasoning": 0.12,
                "risky": 0.03,
                "usage": { "inputTokens": 392, "outputTokens": 65 },
                "latencyMs": 143
            })
        );
    }

    #[test]
    fn route_task_payload_carries_only_the_brief() {
        let payload = serde_json::to_value(RouteTaskPayload {
            brief: "Add a retry",
        })
        .unwrap();
        assert_eq!(payload, json!({ "brief": "Add a retry" }));
    }

    fn jwt_with_payload(payload: Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        format!("{header}.{payload}.")
    }

    fn app_tokens_with_refresh_token(refresh_token: &str) -> AppTokens {
        AppTokens {
            id_token: "id-token".to_string(),
            access_token: "access-token".to_string(),
            refresh_token: refresh_token.to_string(),
            account_id: Some("acc_test".to_string()),
            last_refresh: Utc::now(),
        }
    }

    fn savings_events(count: usize) -> Vec<SavingsReportEvent> {
        (0..count)
            .map(|index| SavingsReportEvent {
                id: format!("event-{index:03}"),
                tokens_saved: 100,
                savings_usd: 0.01,
                dollars: None,
                created_at: None,
            })
            .collect()
    }

    #[tokio::test]
    async fn one_refused_event_becomes_a_dead_letter_and_the_rest_are_delivered() {
        let events = savings_events(SAVINGS_REPORT_BATCH_SIZE);
        let mut requests = 0;
        let delivery = deliver_savings_events(&events, |chunk| {
            requests += 1;
            async move {
                if chunk.iter().any(|event| event.id == "event-137") {
                    Err(SavingsPostError::Payload(
                        "invalid savings payload".to_string(),
                    ))
                } else {
                    Ok(SavingsReportResponse::default())
                }
            }
        })
        .await;

        assert_eq!(delivery.delivered.len(), SAVINGS_REPORT_BATCH_SIZE - 1);
        assert_eq!(
            delivery.rejected,
            vec![(
                "event-137".to_string(),
                "invalid savings payload".to_string()
            )]
        );
        assert!(delivery.retry_error.is_none());
        assert!(delivery.summary.is_some());
        // The split is a binary search, so one bad event costs few requests.
        assert!(requests <= 2 * 8 + 1, "{requests} requests");
        let mut sorted = delivery.delivered.clone();
        sorted.sort();
        assert_eq!(sorted, delivery.delivered, "events keep their queue order");
    }

    #[tokio::test]
    async fn a_new_server_lists_the_refused_ids_and_the_rest_are_delivered() {
        let mut events = savings_events(3);
        events[1].tokens_saved = 12_000_000;
        let mut requests = 0;
        let delivery = deliver_savings_events(&events, |chunk| {
            requests += 1;
            async move {
                let big: Vec<_> = chunk
                    .iter()
                    .filter(|event| event.tokens_saved > 10_000_000)
                    .collect();
                Ok(SavingsReportResponse {
                    summary: SavingsSummary::default(),
                    rejected: big
                        .iter()
                        .map(|event| SavingsRejectedEvent {
                            id: event.id.clone(),
                            reason: "tokens_saved is above the per-event limit".to_string(),
                        })
                        .collect(),
                })
            }
        })
        .await;

        assert_eq!(requests, 1);
        assert_eq!(
            delivery.delivered,
            vec!["event-000".to_string(), "event-002".to_string()]
        );
        assert_eq!(
            delivery.rejected,
            vec![(
                "event-001".to_string(),
                "tokens_saved is above the per-event limit".to_string()
            )]
        );
        assert!(delivery.retry_error.is_none());
    }

    #[tokio::test]
    async fn an_old_server_refusal_applies_the_local_size_rule_before_a_split() {
        let mut events = savings_events(5);
        events[2].tokens_saved = 12_000_000;
        let mut requests = 0;
        let delivery = deliver_savings_events(&events, |chunk| {
            requests += 1;
            async move {
                if chunk.iter().any(|event| event.tokens_saved > 10_000_000) {
                    Err(SavingsPostError::Payload(
                        "invalid savings payload".to_string(),
                    ))
                } else {
                    Ok(SavingsReportResponse::default())
                }
            }
        })
        .await;

        // One refused request, then the rest in one request: no bisection.
        assert_eq!(requests, 2);
        assert_eq!(
            delivery.delivered,
            vec![
                "event-000".to_string(),
                "event-001".to_string(),
                "event-003".to_string(),
                "event-004".to_string()
            ]
        );
        assert_eq!(
            delivery.rejected,
            vec![(
                "event-002".to_string(),
                "invalid savings payload: tokens_saved is above the per-event limit".to_string()
            )]
        );
    }

    #[test]
    fn the_report_response_parses_old_and_new_servers() {
        let old: SavingsReportResponse =
            serde_json::from_str(r#"{"period":"2026-10","tokens_saved":9,"exhausted":false}"#)
                .unwrap();
        assert_eq!(old.summary.tokens_saved, 9);
        assert!(old.rejected.is_empty());
        let new: SavingsReportResponse = serde_json::from_str(
            r#"{"period":"2026-10","tokens_saved":9,"exhausted":false,
                "accepted":["a"],"rejected":[{"id":"b","reason":"too big"}]}"#,
        )
        .unwrap();
        assert_eq!(new.rejected[0].id, "b");
        assert_eq!(new.rejected[0].reason, "too big");
        let event = SavingsReportEvent {
            id: "timed".to_string(),
            tokens_saved: 1,
            savings_usd: 0.0,
            dollars: None,
            created_at: Some(1_792_000_000),
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap()["created_at"],
            1_792_000_000
        );
    }

    #[test]
    fn a_full_batch_with_dollar_fields_fits_the_server_body_limit() {
        let events = (0..SAVINGS_REPORT_BATCH_SIZE)
            .map(|index| SavingsReportEvent {
                id: format!(
                    "agent-turn:{}:{}",
                    crate::storage::new_uuid_v4(),
                    1_000_000 + index
                ),
                tokens_saved: 9_999_999,
                savings_usd: 123.456_789_012_345,
                dollars: Some(crate::storage::savings_dollars::ReportDollars {
                    kind: "output_estimate".to_string(),
                    net_usd: -123.456_789_012_345,
                    method: "effective_rate".to_string(),
                    price_version: "2026-10-15".to_string(),
                }),
                created_at: Some(1_792_000_000),
            })
            .collect::<Vec<_>>();
        let body = serde_json::to_vec(&SavingsReportRequest { events: &events }).unwrap();
        // The server limit is 64 KiB (server/src/lib.rs SAVINGS_BODY_LIMIT_BYTES).
        assert!(body.len() < 64 * 1024, "{} bytes", body.len());
    }

    #[tokio::test]
    async fn a_too_large_batch_splits_until_each_part_fits() {
        let events = savings_events(8);
        let delivery = deliver_savings_events(&events, |chunk| async move {
            if chunk.len() > 2 {
                Err(SavingsPostError::Payload("HTTP 413".to_string()))
            } else {
                Ok(SavingsReportResponse::default())
            }
        })
        .await;

        assert_eq!(delivery.delivered.len(), 8);
        assert!(delivery.rejected.is_empty());
    }

    #[tokio::test]
    async fn a_retryable_error_keeps_the_events_queued() {
        let events = savings_events(4);
        let delivery = deliver_savings_events(&events, |_| async move {
            Err::<SavingsReportResponse, _>(SavingsPostError::Retry("HTTP 429".to_string()))
        })
        .await;

        assert!(delivery.delivered.is_empty());
        assert!(delivery.rejected.is_empty());
        assert_eq!(delivery.retry_error.as_deref(), Some("HTTP 429"));
    }

    #[test]
    fn only_payload_status_codes_split_the_batch() {
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::PAYLOAD_TOO_LARGE,
            StatusCode::UNPROCESSABLE_ENTITY,
        ] {
            assert!(matches!(
                savings_post_error(status, String::new()),
                SavingsPostError::Payload(_)
            ));
        }
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            assert!(matches!(
                savings_post_error(status, String::new()),
                SavingsPostError::Retry(_)
            ));
        }
    }

    fn test_entitlements(plan: &str, savings: SavingsSummary) -> Entitlements {
        Entitlements {
            trial_confirmed: false,
            payment_confirmed: false,
            trial_end: None,
            beta: false,
            billing_required: false,
            can_use_app: true,
            offline_grace_hours: None,
            plan: plan.to_string(),
            message: None,
            checkout_url: None,
            billing_portal_url: Some("https://billing.example/portal".to_string()),
            billing_portal_available: None,
            payment_issue: None,
            savings,
            referral: None,
        }
    }

    #[test]
    fn billing_portal_capability_survives_server_to_frontend_serialization() {
        for key in ["billingPortalAvailable", "billing_portal_available"] {
            let mut payload = serde_json::json!({"plan": "unlimited", "beta": false, "billingRequired": true, "canUseApp": true, "message": null, "checkoutUrl": null, "billingPortalUrl": null, "savings": {"period": "2026-09", "tokensSaved": 0, "capTokens": null, "exhausted": false}});
            payload[key] = serde_json::json!(true);
            let entitlements: Entitlements = serde_json::from_value(payload).unwrap();
            assert_eq!(entitlements.billing_portal_available, Some(true));
            assert_eq!(
                serde_json::to_value(entitlements).unwrap()["billingPortalAvailable"],
                true
            );
        }
    }

    #[test]
    fn me_response_accepts_optional_referral() {
        let legacy: MeResponse = serde_json::from_value(json!({
            "user": { "id": "user_1", "email": "a@example.com", "plan": "basic" },
            "entitlements": {
                "beta": false,
                "billing_required": false,
                "can_use_app": true,
                "plan": "basic",
                "message": null,
                "checkout_url": null,
                "savings": {}
            }
        }))
        .expect("legacy response should deserialize");
        assert_eq!(legacy.referral, None);

        let current: MeResponse = serde_json::from_value(json!({
            "user": { "id": "user_1", "email": "a@example.com", "plan": "basic" },
            "entitlements": {
                "beta": false,
                "billing_required": false,
                "can_use_app": true,
                "plan": "basic",
                "message": null,
                "checkout_url": null,
                "payment_issue": "past_due",
                "savings": {}
            },
            "referral": {
                "code": "K7PQ2M4X",
                "link": "https://halv.ai/i/K7PQ2M4X",
                "trial_days": 14,
                "invited": 3,
                "rewarded": 1,
                "claimed_code": null
            }
        }))
        .expect("current response should deserialize");
        assert_eq!(
            current.entitlements.payment_issue.as_deref(),
            Some("past_due")
        );
        let referral = current.referral.expect("referral should exist");
        assert_eq!(referral.trial_days, 14);

        let mut entitlements = current.entitlements;
        entitlements.referral = Some(referral);
        let mut session = test_savings_session(Some("token"), "basic", "2026-08", false);
        session.entitlements_json =
            Some(serde_json::to_string(&entitlements).expect("entitlements should serialize"));
        let status =
            status_from_session(Some(session), false, 0, "2026-08-01T12:00:00Z".to_string());
        assert_eq!(
            status.referral.expect("status should expose referral").code,
            "K7PQ2M4X"
        );
    }

    #[test]
    fn publish_receipt_payload_serializes_server_field_names() {
        let payload = PublishReceiptPayload {
            period_start: "2026-08-25".to_string(),
            period_end: "2026-08-31".to_string(),
            tokens_saved: 42,
            savings_usd: 1.25,
            percent_saved: Some(20.0),
            agents: vec![PublishReceiptAgent {
                label: "Codex".to_string(),
                tokens_saved: 42,
            }],
            png_base64: "aGVsbG8=".to_string(),
        };
        let value = serde_json::to_value(payload).expect("payload should serialize");
        assert_eq!(value["period_start"], "2026-08-25");
        assert_eq!(value["tokens_saved"], 42);
        assert_eq!(value["agents"][0]["tokens_saved"], 42);
        assert!(value.get("periodStart").is_none());
    }

    fn test_savings_session(
        token: Option<&str>,
        plan: &str,
        period: &str,
        exhausted: bool,
    ) -> AuthSession {
        let entitlements = test_entitlements(
            plan,
            SavingsSummary {
                period: period.to_string(),
                tokens_saved: 123,
                cap_tokens: Some(1_000),
                exhausted,
                all_time_tokens_saved: None,
                all_time_savings_usd: None,
                ..SavingsSummary::default()
            },
        );
        AuthSession {
            server_url: "https://api.halv.test".to_string(),
            token: token.map(str::to_string),
            email: Some("user@example.com".to_string()),
            plan: Some(plan.to_string()),
            entitlements_json: Some(serde_json::to_string(&entitlements).unwrap()),
            synced_at: Some("2026-08-10T12:00:00Z".to_string()),
        }
    }

    #[test]
    fn server_url_accepts_https_and_preserves_trailing_slash_normalization() {
        assert_eq!(
            normalize_server_url("  https://api.halv.ai///  "),
            Ok("https://api.halv.ai".to_string())
        );
    }

    #[test]
    fn server_url_allows_plain_http_only_for_literal_loopback_hosts() {
        for (input, expected) in [
            ("http://localhost:8787/", "http://localhost:8787"),
            ("http://127.0.0.1:8787/", "http://127.0.0.1:8787"),
            ("http://[::1]:8787/", "http://[::1]:8787"),
        ] {
            assert_eq!(normalize_server_url(input), Ok(expected.to_string()));
        }
    }

    #[test]
    fn server_url_rejects_plain_http_for_remote_and_lan_hosts() {
        for input in [
            "http://api.halv.ai",
            "http://192.168.1.10:8787",
            "http://10.0.0.5",
        ] {
            let error = normalize_server_url(input)
                .expect_err("remote and LAN servers must not receive bearer tokens over HTTP");
            assert!(
                error.contains("must use https://"),
                "unexpected error: {error}"
            );
        }
    }

    #[test]
    fn server_url_rejects_plain_http_localhost_lookalikes() {
        for input in [
            "http://localhost.example.com",
            "http://localhost.evil.test:8787",
        ] {
            let error = normalize_server_url(input)
                .expect_err("localhost lookalikes must not bypass HTTPS enforcement");
            assert!(
                error.contains("must use https://"),
                "unexpected error: {error}"
            );
        }
    }

    #[test]
    fn checkout_navigation_allows_any_https_url() {
        let (policy, _) = CheckoutNavigationPolicy::new(
            "https://checkout.stripe.com/c/pay/cs_test_123#fidkdWxOYHwnPyd1blpxYHZxWjA0",
        )
        .expect("Stripe checkout URL should be accepted");

        for allowed in [
            "https://checkout.stripe.com/c/pay/cs_test_456",
            "https://halv.ai/billing/success",
            "https://www.halv.ai/billing/cancel?session_id=cs_test_123",
            "https://js.stripe.com/v3/controller-with-preconnect.html",
            "https://m.stripe.network/inner.html",
            "https://hooks.stripe.com/three-ds-2-challenge",
            "https://newassets.hcaptcha.com/captcha/v1/challenge",
            "https://payments.example-bank.com/3ds/challenge",
            "https://payments.stripe.com/redirect",
            "https://checkout.stripe.com.evil.test/c/pay/cs_test_123",
            "https://example.com/billing/success",
        ] {
            assert!(
                policy.allows(&Url::parse(allowed).unwrap()),
                "expected navigation to be allowed: {allowed}"
            );
        }

        for blocked in [
            "http://checkout.stripe.com/c/pay/cs_test_123",
            "data:text/html,<p>checkout</p>",
            "ftp://checkout.stripe.com/c/pay/cs_test_123",
        ] {
            assert!(
                !policy.allows(&Url::parse(blocked).unwrap()),
                "expected navigation to be blocked: {blocked}"
            );
        }
    }

    #[test]
    fn checkout_window_requires_a_secure_stripe_url() {
        for blocked in [
            "http://checkout.stripe.com/c/pay/cs_test_123",
            "https://halv.ai/billing/success",
            "https://stripe.example/c/pay/cs_test_123",
        ] {
            assert!(
                CheckoutNavigationPolicy::new(blocked).is_err(),
                "expected checkout URL to be rejected: {blocked}"
            );
        }
    }

    #[test]
    fn checkout_outcome_only_matches_exact_halv_completion_paths() {
        for (url, expected) in [
            (
                "https://halv.ai/billing/success?session_id=cs_test_123",
                Some(BillingCheckoutOutcome::Success),
            ),
            (
                "https://www.halv.ai/billing/cancel/",
                Some(BillingCheckoutOutcome::Cancelled),
            ),
            ("https://halv.ai/billing/successful", None),
            ("https://example.com/billing/success", None),
        ] {
            assert_eq!(
                billing_checkout_outcome(&Url::parse(url).unwrap()),
                expected,
                "unexpected completion outcome for {url}"
            );
        }
    }

    #[test]
    fn pkce_challenge_matches_rfc_known_answer() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            pkce_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );

        let flow = PkceFlow::generate();
        assert_eq!(flow.verifier.len(), 86);
        assert_eq!(flow.challenge, pkce_challenge(&flow.verifier));
    }

    #[test]
    fn authorize_url_query_matches_contract() {
        let flow = PkceFlow {
            verifier: "verifier".to_string(),
            challenge: "challenge".to_string(),
            state: "state-value".to_string(),
        };
        let url = build_authorize_url_with_config(
            &flow,
            "http://localhost:1455/auth/callback",
            "client-id",
            "codex_cli_rs",
            &["workspace-a".to_string(), "workspace-b".to_string()],
        )
        .unwrap();
        let parsed = Url::parse(&url).unwrap();
        let params: HashMap<String, String> = parsed.query_pairs().into_owned().collect();

        assert_eq!(parsed.as_str().split('?').next().unwrap(), AUTHORIZE_URL);
        assert_eq!(params.get("response_type").unwrap(), "code");
        assert_eq!(params.get("client_id").unwrap(), "client-id");
        assert_eq!(
            params.get("redirect_uri").unwrap(),
            "http://localhost:1455/auth/callback"
        );
        assert_eq!(params.get("scope").unwrap(), SCOPE);
        assert_eq!(params.get("code_challenge").unwrap(), "challenge");
        assert_eq!(params.get("code_challenge_method").unwrap(), "S256");
        assert_eq!(params.get("id_token_add_organizations").unwrap(), "true");
        assert_eq!(params.get("codex_cli_simplified_flow").unwrap(), "true");
        assert_eq!(params.get("state").unwrap(), "state-value");
        assert_eq!(params.get("originator").unwrap(), "codex_cli_rs");
        assert_eq!(
            params.get("allowed_workspace_id").unwrap(),
            "workspace-a,workspace-b"
        );
    }

    #[test]
    fn halv_oauth_start_url_matches_server_contract() {
        let flow = PkceFlow {
            verifier: "verifier-value".to_string(),
            challenge: "challenge-value".to_string(),
            state: "state-value".to_string(),
        };
        let url = build_halv_oauth_start_url(
            "https://api.halv.test/",
            HalvOAuthProvider::Google,
            &flow,
            43_210,
        )
        .unwrap();
        let parsed = Url::parse(&url).unwrap();
        let params: HashMap<String, String> = parsed.query_pairs().into_owned().collect();

        assert_eq!(
            parsed.as_str().split('?').next().unwrap(),
            "https://api.halv.test/v1/auth/oauth/google/start"
        );
        assert_eq!(params.get("state").map(String::as_str), Some("state-value"));
        assert_eq!(params.get("port").map(String::as_str), Some("43210"));
        assert_eq!(
            params.get("challenge").map(String::as_str),
            Some("challenge-value")
        );
        assert_eq!(params.len(), 3);
    }

    #[test]
    fn halv_oauth_callback_validates_state_and_maps_errors() {
        let success =
            Url::parse("http://localhost/auth/callback?code=exchange-code&state=expected-state")
                .unwrap();
        assert_eq!(
            callback_result_from_url(&success, "expected-state"),
            Some(Ok("exchange-code".to_string()))
        );
        assert_eq!(callback_result_from_url(&success, "wrong-state"), None);

        let denied =
            Url::parse("http://localhost/auth/callback?error=access_denied&state=expected-state")
                .unwrap();
        let error = callback_result_from_url(&denied, "expected-state")
            .expect("matching callback should finish")
            .expect_err("denied callback should fail");
        assert_eq!(error, CallbackError::Provider("access_denied".to_string()));
        assert_eq!(
            callback_error_message(CallbackFlow::HalvAccount, &error),
            "Sign-in was cancelled."
        );

        let wrong_path =
            Url::parse("http://localhost/not-the-callback?code=exchange-code&state=expected-state")
                .unwrap();
        assert_eq!(
            callback_result_from_url(&wrong_path, "expected-state"),
            None
        );
    }

    #[test]
    fn oauth_cancel_marks_and_clears_active_account_and_chatgpt_flows() {
        let account_cancel = Arc::new(AtomicBool::new(false));
        let chatgpt_cancel = Arc::new(AtomicBool::new(false));
        {
            let _account_guard = AccountOAuthCancelGuard::activate(Arc::clone(&account_cancel));
            let _chatgpt_guard = ChatgptOAuthCancelGuard::activate(Arc::clone(&chatgpt_cancel));
            auth_oauth_cancel();
            assert!(account_cancel.load(Ordering::SeqCst));
            assert!(chatgpt_cancel.load(Ordering::SeqCst));
        }

        let active_account = ACTIVE_ACCOUNT_OAUTH_CANCEL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(active_account.is_none());
        let active_chatgpt = ACTIVE_CHATGPT_OAUTH_CANCEL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(active_chatgpt.is_none());
    }

    #[test]
    fn account_oauth_callback_wait_returns_a_distinct_cancelled_error() {
        let (_sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(true));

        let error =
            wait_for_callback_result(receiver, cancel, CallbackFlow::HalvAccount, LOGIN_TIMEOUT)
                .expect_err("cancelled callback wait should fail");

        assert!(matches!(error, AuthError::Cancelled));
        assert_eq!(error.to_string(), "Sign-in cancelled.");
    }

    #[test]
    fn halv_oauth_exchange_request_and_response_match_contract() {
        let request = serde_json::to_value(OAuthExchangeRequest {
            code: "one-time-code",
            verifier: "pkce-verifier",
        })
        .unwrap();
        assert_eq!(
            request,
            json!({
                "code": "one-time-code",
                "verifier": "pkce-verifier"
            })
        );

        let response = parse_login_response(
            r#"{"token":"session-token","user":{"id":"user-1","email":"person@example.com","plan":"pro"}}"#,
            "Account sign-in exchange",
        )
        .unwrap();
        assert_eq!(response.token, "session-token");
        assert_eq!(response.user.id, "user-1");
        assert_eq!(response.user.email, "person@example.com");
        assert_eq!(response.user.plan, "pro");
    }

    #[test]
    fn oauth_provider_discovery_accepts_new_and_legacy_health_shapes() {
        assert_eq!(
            parse_oauth_providers_response(r#"{"ok":true,"oauth_providers":["google","apple"]}"#)
                .unwrap(),
            vec!["google".to_string(), "apple".to_string()]
        );
        assert_eq!(
            parse_oauth_providers_response(r#"{"ok":true}"#).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn parses_id_token_namespaced_claims() {
        let id_token = jwt_with_payload(json!({
            "email": "top@example.com",
            OPENAI_PROFILE_CLAIM: {
                "email": "profile@example.com"
            },
            OPENAI_AUTH_CLAIM: {
                "chatgpt_account_id": "acc_123",
                "chatgpt_plan_type": "plus",
                "chatgpt_user_id": "user_123"
            }
        }));

        assert_eq!(
            parse_id_token_claims(&id_token),
            IdTokenClaims {
                account_id: Some("acc_123".to_string()),
                email: Some("top@example.com".to_string()),
                plan_type: Some("plus".to_string()),
            }
        );
    }

    #[test]
    fn auth_status_serializes_struct_variant_fields_as_camel_case() {
        let status = CredentialAuthStatus::Chatgpt {
            source: "app".to_string(),
            email: Some("user@example.com".to_string()),
            plan_type: Some("plus".to_string()),
            account_id: "acc_123".to_string(),
            access_token_valid: true,
        };

        let serialized = serde_json::to_value(status).unwrap();

        assert_eq!(
            serialized,
            json!({
                "chatgpt": {
                    "source": "app",
                    "email": "user@example.com",
                    "planType": "plus",
                    "accountId": "acc_123",
                    "accessTokenValid": true
                }
            })
        );
        assert!(serialized["chatgpt"].get("plan_type").is_none());
        assert!(serialized["chatgpt"].get("account_id").is_none());
        assert!(serialized["chatgpt"].get("access_token_valid").is_none());
    }

    #[test]
    fn anthropic_api_key_status_exposes_the_api_key_source() {
        let status = CredentialAuthStatus::AnthropicApiKey {
            source: "api-key".to_string(),
            masked: "sk-ant-...1234".to_string(),
        };

        assert_eq!(
            serde_json::to_value(status).unwrap(),
            json!({
                "anthropicApiKey": {
                    "source": "api-key",
                    "masked": "sk-ant-...1234"
                }
            })
        );
    }

    #[test]
    fn request_code_result_accepts_server_dev_code_and_serializes_camel_case() {
        let result: RequestCodeResult =
            serde_json::from_str(r#"{"ok":true,"dev_code":"123456"}"#).unwrap();

        assert_eq!(
            serde_json::to_value(result).unwrap(),
            json!({
                "ok": true,
                "devCode": "123456"
            })
        );
    }

    #[test]
    fn request_code_request_sends_a_challenge_token_only_when_present() {
        assert_eq!(
            serde_json::to_value(RequestCodeRequest {
                email: "person@example.com",
                turnstile_token: None,
            })
            .unwrap(),
            json!({ "email": "person@example.com" })
        );
        assert_eq!(
            serde_json::to_value(RequestCodeRequest {
                email: "person@example.com",
                turnstile_token: Some("token-1"),
            })
            .unwrap(),
            json!({ "email": "person@example.com", "turnstile_token": "token-1" })
        );
    }

    #[test]
    fn request_code_response_maps_challenge_codes() {
        let sent =
            parse_request_code_response(StatusCode::OK, r#"{"ok":true,"dev_code":"123456"}"#)
                .unwrap();
        assert!(sent.ok);
        assert!(!sent.challenge_required);
        assert_eq!(
            serde_json::to_value(&sent).unwrap(),
            json!({ "ok": true, "devCode": "123456" })
        );

        for (status, code) in [
            (StatusCode::TOO_MANY_REQUESTS, "challenge_required"),
            (StatusCode::FORBIDDEN, "challenge_failed"),
        ] {
            let body = format!(r#"{{"error":"email sign-in is busy","code":"{code}"}}"#);
            let result = parse_request_code_response(status, &body).unwrap();
            assert!(!result.ok, "{code}");
            assert!(result.challenge_required, "{code}");
            assert_eq!(
                serde_json::to_value(&result).unwrap(),
                json!({ "ok": false, "challengeRequired": true })
            );
        }

        let error = parse_request_code_response(
            StatusCode::TOO_MANY_REQUESTS,
            r#"{"error":"code requested too recently"}"#,
        )
        .unwrap_err();
        assert_eq!(
            error,
            "Could not request sign-in code: code requested too recently"
        );
    }

    #[test]
    fn challenge_token_input_is_trimmed_and_bounded() {
        assert_eq!(normalize_challenge_token(None).unwrap(), None);
        assert_eq!(normalize_challenge_token(Some("  ")).unwrap(), None);
        assert_eq!(
            normalize_challenge_token(Some(" token-1 ")).unwrap(),
            Some("token-1".to_string())
        );
        assert!(normalize_challenge_token(Some(&"x".repeat(2049))).is_err());
        assert!(normalize_challenge_token(Some("bad token")).is_err());
    }

    #[test]
    fn turnstile_site_key_comes_from_health() {
        assert_eq!(
            parse_turnstile_site_key(
                r#"{"ok":true,"oauth_providers":[],"turnstile_site_key":"0x4AAAAAAA_key-1"}"#
            )
            .unwrap(),
            "0x4AAAAAAA_key-1"
        );
        for body in [
            r#"{"ok":true,"oauth_providers":[]}"#,
            r#"{"ok":true,"turnstile_site_key":""}"#,
            r#"{"ok":true,"turnstile_site_key":"key&next=https://evil.test"}"#,
        ] {
            assert!(parse_turnstile_site_key(body).is_err(), "{body}");
        }
    }

    #[test]
    fn email_challenge_page_url_names_the_site_key() {
        assert_eq!(
            email_challenge_page_url("0x4AAAAAAA_key-1")
                .unwrap()
                .as_str(),
            "https://halv.ai/auth/challenge?sitekey=0x4AAAAAAA_key-1"
        );
        assert!(email_challenge_page_url("key&x=1").is_err());
    }

    #[test]
    fn email_challenge_navigation_allows_the_widget_and_captures_the_token() {
        for allowed in [
            "https://halv.ai/auth/challenge?sitekey=0x4AAAAAAA",
            "https://www.halv.ai/auth/challenge?sitekey=0x4AAAAAAA",
            "https://challenges.cloudflare.com/cdn-cgi/challenge-platform/h/b/turnstile/if/ov2/av0/rcv/abc",
            "about:blank",
            "about:srcdoc",
        ] {
            assert_eq!(
                email_challenge_navigation(&Url::parse(allowed).unwrap()),
                EmailChallengeNavigation::Allow,
                "{allowed}"
            );
        }

        assert_eq!(
            email_challenge_navigation(
                &Url::parse("https://halv.ai/auth/challenge/done?token=0.abc-DEF_123.xyz").unwrap()
            ),
            EmailChallengeNavigation::Done("0.abc-DEF_123.xyz".to_string())
        );
        assert_eq!(
            email_challenge_navigation(
                &Url::parse("https://www.halv.ai/auth/challenge/done/?token=t1").unwrap()
            ),
            EmailChallengeNavigation::Done("t1".to_string())
        );

        for denied in [
            "https://halv.ai/auth/challenge/done",
            "https://halv.ai/auth/challenge/done?token=",
            "http://halv.ai/auth/challenge?sitekey=0x4AAAAAAA",
            "https://evil.test/auth/challenge/done?token=t1",
            "https://halv.ai.evil.test/auth/challenge",
            "https://challenges.cloudflare.com.evil.test/turnstile",
            "about:config",
            "data:text/html,<p>check</p>",
            "tauri://localhost/",
            "file:///etc/passwd",
        ] {
            assert_eq!(
                email_challenge_navigation(&Url::parse(denied).unwrap()),
                EmailChallengeNavigation::Deny,
                "{denied}"
            );
        }
        let long_token = format!(
            "https://halv.ai/auth/challenge/done?token={}",
            "x".repeat(2049)
        );
        assert_eq!(
            email_challenge_navigation(&Url::parse(&long_token).unwrap()),
            EmailChallengeNavigation::Deny
        );
    }

    #[test]
    fn account_bypass_accepts_only_explicit_values() {
        for value in ["1", "true", "TRUE", " TrUe ", " 1 "] {
            assert!(
                account_bypass_active_from(Some(value)),
                "expected {value:?} to activate the bypass"
            );
        }

        for value in [None, Some(""), Some("0"), Some("false"), Some("yes")] {
            assert!(
                !account_bypass_active_from(value),
                "expected {value:?} to leave the bypass disabled"
            );
        }
    }

    #[test]
    fn account_bypass_status_works_without_persisting_to_empty_database() {
        let path = std::env::temp_dir().join(format!(
            "halv-auth-bypass-{}.sqlite3",
            crate::storage::new_uuid_v4()
        ));
        let storage = Storage::open(&path).expect("empty auth test database should open");

        let regular_status =
            local_auth_status_with_bypass(&storage, false, account_bypass_active_from(None))
                .expect("regular empty-database status should load");
        assert!(!regular_status.signed_in);

        let bypass_status =
            local_auth_status_with_bypass(&storage, true, account_bypass_active_from(Some("true")))
                .expect("bypass status should not require a stored session");
        assert!(bypass_status.signed_in);
        assert_eq!(bypass_status.email.as_deref(), Some("dev@halv.local"));
        assert_eq!(bypass_status.plan.as_deref(), Some("dev"));
        assert_eq!(bypass_status.server_url, default_halv_api_url());
        assert!(!bypass_status.stale);
        assert!(DateTime::parse_from_rfc3339(
            bypass_status
                .synced_at
                .as_deref()
                .expect("bypass status should include a sync timestamp")
        )
        .is_ok());
        assert_eq!(
            bypass_status.entitlements,
            Some(Entitlements {
                trial_confirmed: false,
                payment_confirmed: false,
                trial_end: None,
                beta: true,
                billing_required: false,
                can_use_app: true,
                offline_grace_hours: None,
                plan: "dev".to_string(),
                message: None,
                checkout_url: None,
                billing_portal_url: None,
                billing_portal_available: None,
                payment_issue: None,
                savings: SavingsSummary::default(),
                referral: None,
            })
        );
        assert_eq!(
            storage
                .get_auth_session()
                .expect("auth session should remain readable"),
            None
        );

        drop(storage);
        std::fs::remove_file(path).expect("auth test database should be removed");
    }

    #[test]
    fn local_auth_status_serializes_launch_and_legacy_turn_fields() {
        let path = std::env::temp_dir().join(format!(
            "halv-auth-free-turns-{}.sqlite3",
            crate::storage::new_uuid_v4()
        ));
        let storage = Storage::open(&path).expect("auth test database should open");

        let status = local_auth_status_with_bypass(&storage, false, false)
            .expect("local auth status should load");
        let serialized = serde_json::to_value(status).expect("auth status should serialize");
        assert_eq!(serialized["freeTurnsUsed"], 0);
        assert_eq!(
            serialized["firstLaunchAt"],
            storage.first_launch_at().expect("first launch should load")
        );
        assert!(serialized.get("free_turns_used").is_none());
        assert!(serialized.get("first_launch_at").is_none());

        drop(storage);
        std::fs::remove_file(path).expect("auth test database should be removed");
    }

    #[test]
    fn account_status_parses_cached_paywall_entitlements_and_marks_stale() {
        let status = status_from_session(
            Some(AuthSession {
                server_url: "https://api.halv.test".to_string(),
                token: Some("token-123".to_string()),
                email: Some("user@example.com".to_string()),
                plan: Some("trial".to_string()),
                entitlements_json: Some(
                    r#"{"beta":true,"billingRequired":true,"canUseApp":false,"offlineGraceHours":24,"plan":"trial","message":"Subscription required","checkoutUrl":"https://checkout.example"}"#
                        .to_string(),
                ),
                synced_at: Some("2026-07-08T12:00:00+00:00".to_string()),
            }),
            true,
            3,
            "2026-08-01T12:00:00Z".to_string(),
        );

        let serialized = serde_json::to_value(status).unwrap();
        assert_eq!(
            serialized,
            json!({
                "signedIn": true,
                "email": "user@example.com",
                "plan": "trial",
                "serverUrl": "https://api.halv.test",
                "entitlements": {
                    "trialConfirmed": false,
                    "paymentConfirmed": false,
                    "beta": true,
                    "billingRequired": true,
                    "canUseApp": false,
                    "offlineGraceHours": 24,
                    "plan": "trial",
                    "message": "Subscription required",
                    "checkoutUrl": "https://checkout.example",
                    "billingPortalUrl": null,
                    "savings": {
                        "period": "",
                        "tokensSaved": 0,
                        "capTokens": null,
                        "exhausted": false
                    }
            },
            "syncedAt": "2026-07-08T12:00:00+00:00",
            "firstLaunchAt": "2026-08-01T12:00:00Z",
            "freeTurnsUsed": 3,
                "stale": true
            })
        );
        assert_eq!(serialized["entitlements"]["canUseApp"], false);
    }

    #[test]
    fn savings_contract_parses_snake_case_and_serializes_report_events() {
        let summary: SavingsSummary = serde_json::from_str(
            r#"{"period":"2026-08","tokens_saved":900,"cap_tokens":1000,"exhausted":false}"#,
        )
        .unwrap();
        assert_eq!(summary.tokens_saved, 900);
        assert_eq!(summary.cap_tokens, Some(1_000));
        assert_eq!(summary.all_time_tokens_saved, None);
        assert_eq!(summary.all_time_savings_usd, None);
        let current_summary: SavingsSummary = serde_json::from_str(
            r#"{"period":"2026-08","tokens_saved":900,"cap_tokens":1000,"exhausted":false,"all_time_tokens_saved":1900,"all_time_savings_usd":0.75}"#,
        )
        .unwrap();
        assert_eq!(current_summary.all_time_tokens_saved, Some(1_900));
        assert_eq!(current_summary.all_time_savings_usd, Some(0.75));
        assert_eq!(current_summary.all_time_v2_usd, None);
        let dollar_summary: SavingsSummary = serde_json::from_str(
            r#"{"period":"2026-10","tokens_saved":900,"exhausted":false,"all_time_v2_usd":1.5,"period_v2_usd":0.5,"v2_since":1792000000,"all_time_routing_net_usd":-0.1}"#,
        )
        .unwrap();
        assert_eq!(dollar_summary.all_time_v2_usd, Some(1.5));
        assert_eq!(dollar_summary.period_v2_usd, Some(0.5));
        assert_eq!(dollar_summary.v2_since, Some(1_792_000_000));
        assert_eq!(dollar_summary.all_time_routing_net_usd, Some(-0.1));
        assert_eq!(
            serde_json::to_value(&dollar_summary).unwrap()["allTimeV2Usd"],
            1.5
        );

        let events = vec![SavingsReportEvent {
            id: "message-1".to_string(),
            tokens_saved: 42,
            savings_usd: 0.25,
            dollars: None,
            created_at: None,
        }];
        assert_eq!(
            serde_json::to_value(SavingsReportRequest { events: &events }).unwrap(),
            json!({
                "events": [{
                    "id": "message-1",
                    "tokens_saved": 42,
                    "savings_usd": 0.25
                }]
            })
        );
    }

    #[test]
    fn basic_exhaustion_month_rollover_refreshes_savings() {
        // Exhausted Basic savings never pause the local proxy (ADR 0021).
        // Only the month rollover asks the server for a new period.
        let current_period = Utc::now().format("%Y-%m").to_string();
        let basic = test_savings_session(Some("token"), "basic", &current_period, true);
        assert!(!savings_period_refresh_required(&basic));

        let expired = test_savings_session(Some("token"), "basic", "2000-01", true);
        assert!(savings_period_refresh_required(&expired));

        let unlimited = test_savings_session(Some("token"), "unlimited", "2000-01", true);
        assert!(!savings_period_refresh_required(&unlimited));
    }

    #[test]
    fn savings_report_backoff_caps_at_maximum() {
        assert_eq!(
            next_savings_backoff(SAVINGS_REPORT_INITIAL_BACKOFF),
            Duration::from_secs(2)
        );
        assert_eq!(
            next_savings_backoff(SAVINGS_REPORT_MAX_BACKOFF),
            SAVINGS_REPORT_MAX_BACKOFF
        );
    }

    #[test]
    fn reported_savings_update_only_the_current_cached_session() {
        let path = std::env::temp_dir().join(format!(
            "halv-auth-savings-cache-{}.sqlite3",
            crate::storage::new_uuid_v4()
        ));
        let storage = Storage::open(&path).expect("auth test database should open");
        let session = test_savings_session(Some("token-current"), "basic", "2000-01", false);
        storage
            .save_auth_session(&session)
            .expect("session should save");
        let updated = SavingsSummary {
            period: Utc::now().format("%Y-%m").to_string(),
            tokens_saved: 1_000,
            cap_tokens: Some(1_000),
            exhausted: true,
            all_time_tokens_saved: Some(2_000),
            all_time_savings_usd: Some(0.75),
            ..SavingsSummary::default()
        };

        assert!(cache_savings_summary(&storage, &session, updated.clone())
            .expect("current summary should cache"));
        let stored = storage
            .get_auth_session()
            .expect("session should load")
            .expect("session should exist");
        assert_eq!(cached_entitlements(&stored).unwrap().savings, updated);

        let stale = test_savings_session(Some("token-stale"), "basic", "2000-01", false);
        assert!(
            !cache_savings_summary(&storage, &stale, SavingsSummary::default())
                .expect("stale session should be ignored")
        );

        drop(storage);
        std::fs::remove_file(path).expect("auth test database should be removed");
    }

    #[test]
    fn halv_requests_ignore_a_changed_database_url() {
        let path = std::env::temp_dir().join(format!(
            "halv-auth-server-url-{}.sqlite3",
            crate::storage::new_uuid_v4()
        ));
        let storage = Storage::open(&path).expect("auth test database should open");
        let set_raw_server_url = |server_url: &str| {
            rusqlite::Connection::open(&path)
                .expect("auth test database should open")
                .execute(
                    "UPDATE auth_session SET server_url = ?1 WHERE id = 1",
                    [server_url],
                )
                .expect("server URL should update");
        };
        storage
            .save_auth_session(&test_savings_session(
                Some("token-current"),
                "basic",
                "2000-01",
                false,
            ))
            .expect("session should save");

        // A same-user process points the row at its own host.
        set_raw_server_url("https://attacker.test");
        assert_eq!(
            current_halv_session(&storage),
            Ok((
                "https://api.halv.test".to_string(),
                "token-current".to_string()
            ))
        );
        assert_eq!(
            auth_server_url(&storage).expect("server URL should load"),
            "https://api.halv.test"
        );
        assert_eq!(
            local_auth_status_with_bypass(&storage, false, false)
                .expect("status should load")
                .server_url,
            "https://api.halv.test"
        );

        // Without a session, the next sign-in uses the default URL, not the
        // URL column.
        assert!(storage
            .clear_auth_session_if_current("https://api.halv.test", Some("token-current"))
            .expect("sign-out should clear the session"));
        set_raw_server_url("https://attacker.test");
        assert_eq!(
            auth_server_url(&storage).expect("server URL should load"),
            default_halv_api_url()
        );

        drop(storage);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn account_status_omits_false_stale_flag() {
        let status = status_from_session(None, false, 0, "2026-08-01T12:00:00Z".to_string());
        let serialized = serde_json::to_value(status).unwrap();

        assert_eq!(serialized["signedIn"], false);
        assert_eq!(serialized["serverUrl"], default_halv_api_url());
        assert_eq!(serialized["firstLaunchAt"], "2026-08-01T12:00:00Z");
        assert!(serialized.get("stale").is_none());
    }

    #[test]
    fn codex_auth_json_fixture_round_trips_and_borrows_only_unexpired_access() {
        let access_token = jwt_with_payload(json!({ "exp": 4_102_444_800_i64 }));
        let id_token = jwt_with_payload(json!({
            OPENAI_AUTH_CLAIM: {
                "chatgpt_account_id": "acc_fixture",
                "chatgpt_plan_type": "pro"
            }
        }));
        let fixture = json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": id_token,
                "access_token": access_token,
                "refresh_token": "refresh-secret",
                "account_id": null
            },
            "last_refresh": "2026-07-01T00:00:00Z"
        });
        let fixture_text = serde_json::to_string(&fixture).unwrap();
        let parsed: CodexAuthJson = serde_json::from_str(&fixture_text).unwrap();
        let serialized = serde_json::to_value(parsed).unwrap();
        assert_eq!(serialized, fixture);

        let borrowed = borrowed_codex_tokens_from_str(&fixture_text, Utc::now())
            .unwrap()
            .unwrap();
        assert_eq!(borrowed.account_id, "acc_fixture");
        assert_eq!(borrowed.access_token, fixture["tokens"]["access_token"]);
    }

    #[test]
    fn codex_auth_borrow_accepts_case_insensitive_and_legacy_chatgpt_mode() {
        let access_token = jwt_with_payload(json!({ "exp": 4_102_444_800_i64 }));
        let id_token = jwt_with_payload(json!({}));

        let mixed_case = json!({
            "auth_mode": "ChatGPT",
            "tokens": {
                "id_token": id_token,
                "access_token": access_token,
                "refresh_token": "refresh-secret",
                "account_id": "acc_mixed"
            }
        });
        let borrowed = borrowed_codex_tokens_from_str(
            &serde_json::to_string(&mixed_case).unwrap(),
            Utc::now(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(borrowed.account_id, "acc_mixed");

        let legacy = json!({
            "tokens": {
                "id_token": jwt_with_payload(json!({
                    OPENAI_AUTH_CLAIM: {
                        "chatgpt_account_id": "acc_legacy"
                    }
                })),
                "access_token": jwt_with_payload(json!({ "exp": 4_102_444_800_i64 })),
                "refresh_token": "refresh-secret",
                "account_id": null
            }
        });
        let borrowed =
            borrowed_codex_tokens_from_str(&serde_json::to_string(&legacy).unwrap(), Utc::now())
                .unwrap()
                .unwrap();
        assert_eq!(borrowed.account_id, "acc_legacy");

        let api_key_mode = json!({
            "OPENAI_API_KEY": "sk-test",
            "tokens": {
                "id_token": jwt_with_payload(json!({})),
                "access_token": jwt_with_payload(json!({ "exp": 4_102_444_800_i64 })),
                "refresh_token": "refresh-secret",
                "account_id": "acc_api"
            }
        });
        assert!(borrowed_codex_tokens_from_str(
            &serde_json::to_string(&api_key_mode).unwrap(),
            Utc::now(),
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn refresh_request_body_has_contract_shape_without_scope() {
        let body =
            serde_json::to_value(RefreshRequest::new("client-id", "refresh-secret")).unwrap();
        assert_eq!(
            body,
            json!({
                "client_id": "client-id",
                "grant_type": "refresh_token",
                "refresh_token": "refresh-secret"
            })
        );
        assert!(body.get("scope").is_none());
    }

    #[test]
    fn refresh_failure_classification_matches_contract() {
        assert_eq!(
            classify_refresh_failure(
                StatusCode::BAD_REQUEST,
                r#"{"error":{"code":"refresh_token_reused"}}"#
            ),
            RefreshFailureKind::Permanent
        );
        assert_eq!(
            classify_refresh_failure(
                StatusCode::BAD_REQUEST,
                r#"{"error":"refresh_token_expired"}"#
            ),
            RefreshFailureKind::Permanent
        );
        assert_eq!(
            classify_refresh_failure(
                StatusCode::BAD_REQUEST,
                r#"{"code":"refresh_token_invalidated"}"#
            ),
            RefreshFailureKind::Permanent
        );
        assert_eq!(
            classify_refresh_failure(StatusCode::UNAUTHORIZED, r#"{"error":"unknown"}"#),
            RefreshFailureKind::Permanent
        );
        assert_eq!(
            classify_refresh_failure(StatusCode::INTERNAL_SERVER_ERROR, r#"{"error":"unknown"}"#),
            RefreshFailureKind::Transient
        );
    }

    #[test]
    fn conditional_refresh_failure_clear_spares_rotated_successor_token() {
        let failed_tokens = app_tokens_with_refresh_token("refresh-old");
        let rotated_tokens = app_tokens_with_refresh_token("refresh-new");

        assert!(should_clear_app_tokens_after_refresh_failure(
            Some(&failed_tokens),
            "refresh-old"
        ));
        assert!(!should_clear_app_tokens_after_refresh_failure(
            Some(&rotated_tokens),
            "refresh-old"
        ));
        assert!(!should_clear_app_tokens_after_refresh_failure(
            None,
            "refresh-old"
        ));
    }

    #[test]
    fn auth_clear_api_key_targets_only_api_key_secret() {
        let mut cleared_account = None;

        clear_api_key_secret_with(|account| {
            cleared_account = Some(account.to_string());
            Ok(())
        })
        .expect("clear helper should succeed");

        assert_eq!(cleared_account, Some(KEYCHAIN_API_KEY_ACCOUNT.to_string()));
        assert_ne!(cleared_account, Some(KEYCHAIN_CHATGPT_ACCOUNT.to_string()));
    }

    #[test]
    fn keychain_read_migrates_legacy_service_entry() {
        use std::{cell::RefCell, collections::HashMap};

        let secrets = RefCell::new(HashMap::from([(
            (
                LEGACY_SERVICE_NAME.to_string(),
                KEYCHAIN_CHATGPT_ACCOUNT.to_string(),
            ),
            "legacy-token-json".to_string(),
        )]));
        let reads = RefCell::new(Vec::<(String, String)>::new());
        let writes = RefCell::new(Vec::<(String, String, String)>::new());
        let deletes = RefCell::new(Vec::<(String, String)>::new());

        let secret = get_keychain_secret_with(
            KEYCHAIN_CHATGPT_ACCOUNT,
            |service, account| {
                reads
                    .borrow_mut()
                    .push((service.to_string(), account.to_string()));
                Ok(secrets
                    .borrow()
                    .get(&(service.to_string(), account.to_string()))
                    .cloned())
            },
            |service, account, secret| {
                writes.borrow_mut().push((
                    service.to_string(),
                    account.to_string(),
                    secret.to_string(),
                ));
                secrets.borrow_mut().insert(
                    (service.to_string(), account.to_string()),
                    secret.to_string(),
                );
                Ok(())
            },
            |service, account| {
                deletes
                    .borrow_mut()
                    .push((service.to_string(), account.to_string()));
                secrets
                    .borrow_mut()
                    .remove(&(service.to_string(), account.to_string()));
                Ok(())
            },
        )
        .expect("legacy keychain read should migrate");

        assert_eq!(secret.as_deref(), Some("legacy-token-json"));
        assert_eq!(
            reads.into_inner(),
            vec![
                (
                    SERVICE_NAME.to_string(),
                    KEYCHAIN_CHATGPT_ACCOUNT.to_string()
                ),
                (
                    LEGACY_SERVICE_NAME.to_string(),
                    KEYCHAIN_CHATGPT_ACCOUNT.to_string()
                ),
            ]
        );
        assert_eq!(
            writes.into_inner(),
            vec![(
                SERVICE_NAME.to_string(),
                KEYCHAIN_CHATGPT_ACCOUNT.to_string(),
                "legacy-token-json".to_string()
            )]
        );
        assert_eq!(
            deletes.into_inner(),
            vec![(
                LEGACY_SERVICE_NAME.to_string(),
                KEYCHAIN_CHATGPT_ACCOUNT.to_string()
            )]
        );
        let secrets = secrets.into_inner();
        assert_eq!(
            secrets.get(&(
                SERVICE_NAME.to_string(),
                KEYCHAIN_CHATGPT_ACCOUNT.to_string()
            )),
            Some(&"legacy-token-json".to_string())
        );
        assert!(!secrets.contains_key(&(
            LEGACY_SERVICE_NAME.to_string(),
            KEYCHAIN_CHATGPT_ACCOUNT.to_string()
        )));
    }
}
