//! Ruum Cloud gateway HTTP client (`ureq` + `rustls`), device registration
//! (`PoW` + Ed25519), JWT lifecycle management, entitlement/billing/redemption
//! endpoints, and virtual model discovery.

use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(test)]
use crate::ai_cli::CommandRunner;
use crate::ai_cli::TimedCommandRunner;
use crate::cloud_identity::{
    CloudIdentityEnv, CloudIdentityOs, collect_device_anchors,
    derive_device_id, sign_cloud_request, signing_key_public_b64,
    solve_pow_challenge,
};
use crate::credentials::LazyCredentialStore;
use crate::i18n::{Locale, format_cloud_error_message};
use crate::storage::{
    CloudState, DEFAULT_CLOUD_GATEWAY_BASE_URL, StorageDb,
    validate_cloud_base_url,
};

/// Runtime environment variable overriding the Ruum Cloud gateway base URL.
pub const ENV_CLOUD_BASE_URL: &str = "RUUM_CLOUD_BASE_URL";

/// HTTP header carrying the desktop client version (`X-Ruum-Client-Version`).
pub const HEADER_CLIENT_VERSION: &str = "X-Ruum-Client-Version";

/// HTTP header carrying the server request trace identifier (`X-Trace-ID`).
pub const HEADER_TRACE_ID: &str = "X-Trace-ID";

/// TCP/TLS connect timeout for Ruum Cloud control-plane requests.
pub const CLOUD_CONNECT_TIMEOUT: Duration = Duration::from_mins(1);

/// Read/write/global timeout for Ruum Cloud control-plane requests.
pub const CLOUD_CONTROL_TIMEOUT: Duration = Duration::from_mins(2);

/// Read/write/global timeout for Ruum Cloud billing checkout requests.
pub const CLOUD_BILLING_TIMEOUT: Duration = Duration::from_mins(2);

#[cfg(not(test))]
const BILLING_RETRY_BACKOFF: Duration = Duration::from_secs(1);
#[cfg(test)]
const BILLING_RETRY_BACKOFF: Duration = Duration::from_millis(10);

/// Refresh short-lived JWTs proactively when within 60 seconds of expiry.
pub const JWT_REFRESH_SKEW_SECS: u64 = 60;

/// Structured error returned by Ruum Cloud control-plane and inference APIs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudApiError {
    pub http_status: Option<u16>,
    pub code: Option<String>,
    pub message: String,
    pub trace_id: Option<String>,
    pub retry_after_secs: Option<u64>,
    pub is_timeout: bool,
}

impl CloudApiError {
    /// Construct a client-side or synthetic [`CloudApiError`].
    #[must_use]
    pub fn new(
        http_status: Option<u16>,
        code: Option<&str>,
        message: impl Into<String>,
        trace_id: Option<&str>,
    ) -> Self {
        Self {
            http_status,
            code: code.map(ToOwned::to_owned),
            message: message.into(),
            trace_id: trace_id.map(ToOwned::to_owned),
            retry_after_secs: None,
            is_timeout: matches!(http_status, Some(408 | 504)),
        }
    }

    /// Return `true` when the device has been permanently revoked (`403 AUTH_DEVICE_REVOKED`).
    #[must_use]
    pub fn is_revoked(&self) -> bool {
        self.code.as_deref() == Some("AUTH_DEVICE_REVOKED")
            || self.http_status == Some(403)
                && self.code.as_deref() == Some("AUTH_DEVICE_REVOKED")
    }

    /// Return `true` when an HTTP 401 should trigger a single JWT refresh + retry.
    #[must_use]
    pub fn is_token_retryable(&self) -> bool {
        self.http_status == Some(401)
            && matches!(
                self.code.as_deref(),
                None | Some("AUTH_TOKEN_EXPIRED" | "AUTH_UNAUTHORIZED")
            )
    }

    /// Return `true` when online billing is unavailable or disabled.
    #[must_use]
    pub fn is_billing_unavailable(&self) -> bool {
        matches!(
            self.code.as_deref(),
            Some("BILLING_UNAVAILABLE" | "BILLING_PRODUCT_DISABLED")
        )
    }

    /// Format a localized user-facing error message preserving `[Trace: ...]`.
    #[must_use]
    pub fn format_localized(&self, locale: Locale) -> String {
        format_cloud_error_message(
            self.code.as_deref(),
            self.http_status,
            &self.message,
            self.trace_id.as_deref(),
            self.is_timeout,
            locale,
        )
    }
}

impl fmt::Display for CloudApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref code) = self.code {
            write!(f, "{code}: {}", self.message)?;
        } else {
            write!(f, "{}", self.message)?;
        }
        if let Some(ref tid) = self.trace_id {
            write!(f, " [Trace: {tid}]")?;
        }
        Ok(())
    }
}

impl std::error::Error for CloudApiError {}

/// Entitlement status snapshot returned by `GET /v1/entitlement/status`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementSnapshot {
    pub subject_id: String,
    pub device_id: String,
    pub is_paid: bool,
    pub source: String,
    pub decision: String,
    #[serde(default)]
    pub trial_remaining: u64,
    #[serde(default)]
    pub valid_from_secs: Option<u64>,
    #[serde(default)]
    pub valid_until_secs: Option<u64>,
    #[serde(default)]
    pub alipay_valid_until_secs: Option<u64>,
    #[serde(default)]
    pub grant_valid_until_secs: Option<u64>,
    #[serde(default)]
    pub fup_zone: String,
    #[serde(default)]
    pub daily_cost_micro_cny: u64,
    #[serde(default)]
    pub daily_budget_micro_cny: u64,
    #[serde(default)]
    pub monthly_cost_micro_cny: u64,
    #[serde(default)]
    pub monthly_budget_micro_cny: u64,
    #[serde(default)]
    pub synced_at_secs: u64,
}

impl EntitlementSnapshot {
    /// Return `true` when an Alipay duration pass is currently active at
    /// `now_secs` (never true when `decision == "DENY"`).
    #[must_use]
    pub fn is_alipay_active_at(&self, now_secs: u64) -> bool {
        if self.decision.eq_ignore_ascii_case("DENY") {
            return false;
        }
        if let Some(until) = self.alipay_valid_until_secs {
            return until > now_secs;
        }
        self.is_paid
            && self.source.eq_ignore_ascii_case("alipay")
            && self.valid_until_secs.is_none_or(|u| u > now_secs)
    }

    /// Return `true` when any paid entitlement (Alipay or redemption grant) is
    /// still valid at `now_secs`.
    #[must_use]
    pub fn is_effectively_paid_at(&self, now_secs: u64) -> bool {
        if self.decision.eq_ignore_ascii_case("DENY") {
            return false;
        }
        let has_explicit_expiry = self.alipay_valid_until_secs.is_some()
            || self.grant_valid_until_secs.is_some();
        self.is_alipay_active_at(now_secs)
            || self.grant_valid_until_secs.is_some_and(|u| u > now_secs)
            || (!has_explicit_expiry
                && self.is_paid
                && self.valid_until_secs.is_none_or(|u| u > now_secs))
    }

    /// Return `true` when the subject previously held an Alipay pass that is
    /// no longer active at `now_secs`.
    #[must_use]
    pub fn has_expired_alipay_history_at(&self, now_secs: u64) -> bool {
        !self.is_alipay_active_at(now_secs)
            && (self.alipay_valid_until_secs.is_some_and(|u| u <= now_secs)
                || (self.source.eq_ignore_ascii_case("alipay")
                    && self.valid_until_secs.is_some_and(|u| u <= now_secs)))
    }
}

/// Virtual model entry returned by `GET /v1/models` (bare `OpenAI` list).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudVirtualModel {
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub created: Option<u64>,
    #[serde(default)]
    pub owned_by: Option<String>,
}

/// Select the preferred virtual model if `available == true`, otherwise the
/// first available model in the list, or `None` if zero models are available.
#[must_use]
pub fn select_best_available_model<'a>(
    models: &'a [CloudVirtualModel],
    preferred_id: Option<&str>,
) -> Option<&'a CloudVirtualModel> {
    if let Some(pref) = preferred_id.map(str::trim).filter(|s| !s.is_empty())
        && let Some(found) = models.iter().find(|m| m.id == pref && m.available)
    {
        return Some(found);
    }
    models.iter().find(|m| m.available)
}

/// Pre-payment daily/monthly usage policy returned by `GET /v1/billing/plans`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePolicy {
    pub daily_budget_micro_cny: u64,
    pub monthly_budget_micro_cny: u64,
    pub estimated_daily_analyses: u64,
    pub estimated_monthly_analyses: u64,
    pub soft_fallback_percent: u64,
    pub reset_timezone: String,
}

/// Billing SKU option returned by `/v1/billing/plans` and `/v1/billing/purchase`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChargingOption {
    pub sku_id: String,
    pub display_name: String,
    pub price: String,
    #[serde(default)]
    pub duration_period: Option<String>,
    #[serde(default)]
    pub usage_policy: Option<UsagePolicy>,
}

/// Billing plans snapshot returned by `GET /v1/billing/plans`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingPlansSnapshot {
    pub capability_status: String,
    #[serde(default)]
    pub product_name: Option<String>,
    #[serde(default)]
    pub billing_mode: Option<String>,
    #[serde(default)]
    pub charging_options: Vec<ChargingOption>,
    #[serde(default)]
    pub usage_policy: Option<UsagePolicy>,
}

impl BillingPlansSnapshot {
    /// Return `true` only when `capabilityStatus == "ENABLED"` and at least one
    /// SKU option is present.
    #[must_use]
    pub fn is_available(&self) -> bool {
        self.capability_status.eq_ignore_ascii_case("ENABLED")
            && !self.charging_options.is_empty()
    }

    /// Return a `BILLING_UNAVAILABLE` [`CloudApiError`] when billing is not
    /// enabled or has no charging options.
    pub fn ensure_available(&self) -> Result<(), CloudApiError> {
        if self.is_available() {
            Ok(())
        } else {
            Err(CloudApiError::new(
                Some(503),
                Some("BILLING_UNAVAILABLE"),
                "Online billing is currently unavailable",
                None,
            ))
        }
    }

    /// Return the single shared [`UsagePolicy`] if every SKU resolves to the
    /// same policy, or `None` when per-SKU policies differ or none exist.
    #[must_use]
    pub fn uniform_usage_policy(&self) -> Option<&UsagePolicy> {
        if self.charging_options.is_empty() {
            return self.usage_policy.as_ref();
        }
        let mut iter = self.charging_options.iter().map(|opt| {
            opt.usage_policy.as_ref().or(self.usage_policy.as_ref())
        });
        let first = iter.next().flatten()?;
        if iter.all(|pol| pol == Some(first)) {
            Some(first)
        } else {
            None
        }
    }
}

/// Purchase order snapshot returned by `POST /v1/billing/purchase`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurchaseOrderSnapshot {
    pub decision: String,
    #[serde(rename = "outRequestNo", alias = "purchaseId")]
    pub purchase_id: String,
    #[serde(default)]
    pub purchase_url: Option<String>,
    #[serde(default)]
    pub purchase_qr_code: Option<String>,
    #[serde(rename = "expireAtSecs", alias = "expiresAt", default)]
    pub expires_at: Option<u64>,
    #[serde(default)]
    pub valid_until_secs: Option<u64>,
    #[serde(default)]
    pub charging_options: Vec<ChargingOption>,
}

/// Supporter redemption response returned by `POST /v1/entitlement/redeem`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedeemResult {
    pub redeemed_duration_period: String,
    #[serde(default)]
    pub previous_valid_until_secs: Option<u64>,
    pub new_valid_until_secs: u64,
    pub entitlement: EntitlementSnapshot,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub expires_at_secs: u64,
}

/// Generate a client-side idempotency key (`req_<uuid>`) for purchase orders.
#[must_use]
pub fn generate_purchase_request_no() -> String {
    format!("req_{}", uuid::Uuid::new_v4().simple())
}

/// Normalize a user-entered supporter redemption code by stripping surrounding
/// and internal whitespace and uppercasing ASCII letters without silently
/// altering non-empty invalid characters.
#[must_use]
pub fn normalize_redemption_code_input(raw: &str) -> String {
    raw.split_whitespace()
        .collect::<String>()
        .to_ascii_uppercase()
}

/// Resolve the Ruum Cloud gateway base URL using an explicit override chain.
///
/// Order: `env_override` (`RUUM_CLOUD_BASE_URL`) -> `explicit_override` ->
/// `state.gateway_base_url` -> [`DEFAULT_CLOUD_GATEWAY_BASE_URL`].
pub fn resolve_cloud_base_url_with(
    env_override: Option<&str>,
    explicit_override: Option<&str>,
    state: Option<&CloudState>,
) -> Result<String, CloudApiError> {
    let raw = env_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| explicit_override.map(str::trim).filter(|s| !s.is_empty()))
        .or_else(|| {
            state
                .map(|s| s.gateway_base_url.trim())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or(DEFAULT_CLOUD_GATEWAY_BASE_URL);
    validate_cloud_base_url(raw).map_err(|e| {
        CloudApiError::new(
            None,
            Some("CLOUD_INVALID_BASE_URL"),
            e.to_string(),
            None,
        )
    })
}

/// Resolve the Ruum Cloud gateway base URL from `RUUM_CLOUD_BASE_URL`,
/// optional `cloud_state`, and [`DEFAULT_CLOUD_GATEWAY_BASE_URL`].
pub fn resolve_cloud_base_url(
    state: Option<&CloudState>,
) -> Result<String, CloudApiError> {
    let env_val = std::env::var(ENV_CLOUD_BASE_URL).ok();
    resolve_cloud_base_url_with(env_val.as_deref(), None, state)
}

/// Synchronous Ruum Cloud HTTP gateway client backed by `ureq` + `rustls`.
#[derive(Clone)]
pub struct CloudClient {
    db: Arc<StorageDb>,
    cred_store: LazyCredentialStore,
    base_url_override: Option<String>,
    identity_env: CloudIdentityEnv,
    agent: ureq::Agent,
    billing_agent: ureq::Agent,
    #[cfg(test)]
    test_runner: Option<Arc<dyn CommandRunner>>,
}

impl fmt::Debug for CloudClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CloudClient")
            .field("db_path", &self.db.db_path())
            .field("base_url_override", &self.base_url_override)
            .finish_non_exhaustive()
    }
}

fn jwt_refresh_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

impl CloudClient {
    /// Construct a [`CloudClient`] sharing `db` and `cred_store`.
    #[must_use]
    pub fn new(db: Arc<StorageDb>, cred_store: LazyCredentialStore) -> Self {
        Self {
            db,
            cred_store,
            base_url_override: None,
            identity_env: CloudIdentityEnv::current(),
            agent: build_control_agent(),
            billing_agent: build_billing_agent(),
            #[cfg(test)]
            test_runner: None,
        }
    }

    /// Override the gateway base URL after validating it via
    /// [`validate_cloud_base_url`].
    pub fn with_base_url(
        mut self,
        url: impl Into<String>,
    ) -> Result<Self, CloudApiError> {
        let validated = validate_cloud_base_url(&url.into()).map_err(|e| {
            CloudApiError::new(
                None,
                Some("CLOUD_INVALID_BASE_URL"),
                e.to_string(),
                None,
            )
        })?;
        self.base_url_override = Some(validated);
        Ok(self)
    }

    /// Resolve the active validated gateway base URL.
    pub fn resolved_base_url(&self) -> Result<String, CloudApiError> {
        let state = self.db.load_cloud_state().map_err(|e| {
            CloudApiError::new(
                None,
                Some("CLOUD_STORAGE_ERROR"),
                e.to_string(),
                None,
            )
        })?;
        let env_val = std::env::var(ENV_CLOUD_BASE_URL).ok();
        resolve_cloud_base_url_with(
            env_val.as_deref(),
            self.base_url_override.as_deref(),
            Some(&state),
        )
    }

    /// Ensure the device has an Ed25519 signing key, is registered with the
    /// gateway, and holds a valid JWT; triggers `/v1/entitlement/status`
    /// sync immediately on reinstall recovery (`!is_new_subject`).
    pub fn ensure_device_registered(
        &self,
    ) -> Result<CloudState, CloudApiError> {
        let state = self.load_state()?;
        let has_key = self
            .cred_store
            .has_cloud_signing_key()
            .map_err(storage_err)?;
        if state.device_id.is_some() && has_key {
            let now = now_unix_secs();
            if let Some(ref tok) = state.access_token
                && !tok.trim().is_empty()
                && state.token_expires_at_secs
                    > now.saturating_add(JWT_REFRESH_SKEW_SECS)
            {
                return Ok(state);
            }
            return self.refresh_access_token();
        }
        self.register_device_internal(true)
    }

    /// Refresh the short-lived JWT via `POST /v1/auth/token`, performing `PoW`
    /// re-registration once on `404 DEVICE_NOT_FOUND` or `401 AUTH_UNAUTHORIZED`,
    /// and never retrying `403 AUTH_DEVICE_REVOKED`.
    pub fn refresh_access_token(&self) -> Result<CloudState, CloudApiError> {
        let _guard = jwt_refresh_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.refresh_access_token_unlocked()
    }

    fn refresh_access_token_unlocked(
        &self,
    ) -> Result<CloudState, CloudApiError> {
        let state = self.load_state()?;
        let has_key = self
            .cred_store
            .has_cloud_signing_key()
            .map_err(storage_err)?;
        if !has_key || state.device_id.is_none() {
            return self.register_device_internal(true);
        }

        let signing_key = self
            .cred_store
            .load_or_create_cloud_signing_key()
            .map_err(storage_err)?;
        let device_id = match state.device_id {
            Some(id) if !id.trim().is_empty() => id,
            _ => derive_device_id(&signing_key_public_b64(&signing_key))
                .map_err(storage_err)?,
        };

        let body_bytes = b"{}";
        let ts = now_unix_secs();
        let nonce = uuid::Uuid::new_v4().to_string();
        let auth_headers = sign_cloud_request(
            &signing_key,
            &device_id,
            "POST",
            "/v1/auth/token",
            ts,
            &nonce,
            body_bytes,
        );

        match self.post_envelope_signed::<TokenResponse>(
            "/v1/auth/token",
            &auth_headers,
            body_bytes,
        ) {
            Ok(tok) => {
                let mut updated = self.load_state()?;
                if let Some(dev) =
                    tok.device_id.filter(|s| !s.trim().is_empty())
                {
                    updated.device_id = Some(dev);
                }
                if let Some(sub) =
                    tok.subject_id.filter(|s| !s.trim().is_empty())
                {
                    updated.subject_id = Some(sub);
                }
                updated.access_token = Some(tok.access_token);
                updated.token_expires_at_secs = tok.expires_at_secs;
                updated.updated_at_secs = now_unix_secs();
                self.save_state(&updated)?;
                Ok(updated)
            }
            Err(err) if err.is_revoked() => Err(err),
            Err(err) if is_reregister_recoverable(&err) => {
                self.register_device_internal(true)
            }
            Err(err) => Err(err),
        }
    }

    /// Return a valid JWT, refreshing proactively when empty or within 60s of
    /// expiry under a single-flight mutex to avoid concurrent refresh storms.
    pub fn ensure_valid_jwt(&self) -> Result<String, CloudApiError> {
        let state = self.load_state()?;
        let has_key = self
            .cred_store
            .has_cloud_signing_key()
            .map_err(storage_err)?;
        if has_key && state.device_id.is_some() {
            let now = now_unix_secs();
            if let Some(tok) = state.access_token
                && !tok.trim().is_empty()
                && state.token_expires_at_secs
                    > now.saturating_add(JWT_REFRESH_SKEW_SECS)
            {
                return Ok(tok);
            }
            let _guard = jwt_refresh_lock()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Another thread may have already refreshed while we waited on the mutex.
            let latest = self.load_state()?;
            let now = now_unix_secs();
            if latest.device_id.is_some()
                && let Some(tok) = latest.access_token
                && !tok.trim().is_empty()
                && latest.token_expires_at_secs
                    > now.saturating_add(JWT_REFRESH_SKEW_SECS)
            {
                return Ok(tok);
            }
            let refreshed = self.refresh_access_token_unlocked()?;
            return refreshed.access_token.ok_or_else(|| {
                CloudApiError::new(
                    None,
                    Some("AUTH_TOKEN_MISSING"),
                    "cloud token refresh returned no access token",
                    None,
                )
            });
        }
        let _guard = jwt_refresh_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let latest = self.load_state()?;
        let now = now_unix_secs();
        if latest.device_id.is_some()
            && let Some(tok) = latest.access_token
            && !tok.trim().is_empty()
            && latest.token_expires_at_secs
                > now.saturating_add(JWT_REFRESH_SKEW_SECS)
        {
            return Ok(tok);
        }
        let registered = self.register_device_internal(true)?;
        registered.access_token.ok_or_else(|| {
            CloudApiError::new(
                None,
                Some("AUTH_TOKEN_MISSING"),
                "device registration returned no access token",
                None,
            )
        })
    }

    /// Execute `op(&jwt)`, refreshing the JWT once on `401 AUTH_TOKEN_EXPIRED` /
    /// `AUTH_UNAUTHORIZED` (never on `403 AUTH_DEVICE_REVOKED`).
    pub fn with_fresh_jwt_retry<T>(
        &self,
        mut op: impl FnMut(&str) -> Result<T, CloudApiError>,
    ) -> Result<T, CloudApiError> {
        let jwt = self.ensure_valid_jwt()?;
        match op(&jwt) {
            Ok(val) => Ok(val),
            Err(err) if err.is_token_retryable() && !err.is_revoked() => {
                let refreshed = self.refresh_access_token()?;
                let new_jwt = refreshed.access_token.ok_or_else(|| {
                    CloudApiError::new(
                        None,
                        Some("AUTH_TOKEN_MISSING"),
                        "cloud token refresh returned no access token",
                        None,
                    )
                })?;
                op(&new_jwt)
            }
            Err(err) => Err(err),
        }
    }

    /// Fetch `GET /v1/entitlement/status` (with `?refresh=1` when `refresh` is
    /// true) and update `cloud_state.entitlement_json`.
    pub fn fetch_entitlement(
        &self,
        refresh: bool,
    ) -> Result<EntitlementSnapshot, CloudApiError> {
        let path = if refresh {
            "/v1/entitlement/status?refresh=1"
        } else {
            "/v1/entitlement/status"
        };
        let snapshot: EntitlementSnapshot =
            self.with_fresh_jwt_retry(|jwt| {
                self.bearer_json_envelope("GET", path, jwt, None, &[])
            })?;
        let now = now_unix_secs();
        let mut state = self.load_state()?;
        state.subject_id = Some(snapshot.subject_id.clone());
        if !snapshot.device_id.trim().is_empty() {
            state.device_id = Some(snapshot.device_id.clone());
        }
        state.entitlement_json = serde_json::to_string(&snapshot).ok();
        state.last_synced_at_secs = now;
        state.updated_at_secs = now;
        self.save_state(&state)?;
        Ok(snapshot)
    }

    /// Fetch public `GET /v1/billing/plans` without device auth or keychain
    /// access, retrying once on transient errors, and best-effort cache the
    /// snapshot in `cloud_state`.
    pub fn fetch_billing_plans(
        &self,
    ) -> Result<BillingPlansSnapshot, CloudApiError> {
        let send_once = || {
            let (status, header_trace, raw_body) = self.send_http_with_agent(
                &self.agent,
                "GET",
                "/v1/billing/plans",
                CloudRequestAuth::None,
                None,
                &[],
            )?;
            decode_envelope::<BillingPlansSnapshot>(
                status,
                header_trace.as_deref(),
                &raw_body,
                &[],
            )
        };
        let snapshot = match send_once() {
            Ok(s) => s,
            Err(err) if is_retryable_billing_error(&err) => {
                std::thread::sleep(BILLING_RETRY_BACKOFF);
                send_once()?
            }
            Err(err) => return Err(err),
        };
        if let Ok(mut state) = self.load_state() {
            let now = now_unix_secs();
            state.plans_json = serde_json::to_string(&snapshot).ok();
            state.plans_cached_at_secs = now;
            state.updated_at_secs = now;
            let _ = self.save_state(&state);
        }
        Ok(snapshot)
    }

    /// Return the cached `selected_model_id` from `cloud_state` without
    /// hitting the keychain or network (`None` means Auto).
    #[must_use]
    pub fn cached_selected_model_id(&self) -> Option<String> {
        self.db
            .load_cloud_state()
            .ok()
            .and_then(|s| s.selected_model_id)
    }

    /// Return `true` when cached billing plans are absent or older than
    /// `ttl_secs` at `now_secs`.
    #[must_use]
    pub fn cached_plans_need_refresh(
        &self,
        now_secs: u64,
        ttl_secs: u64,
    ) -> bool {
        let Ok(state) = self.db.load_cloud_state() else {
            return true;
        };
        state
            .plans_json
            .as_deref()
            .is_none_or(|s| s.trim().is_empty())
            || state.plans_cached_at_secs.saturating_add(ttl_secs) <= now_secs
    }

    /// Create a purchase order via `POST /v1/billing/purchase` with a fresh
    /// client-generated `outRequestNo` idempotency key.
    pub fn create_purchase(
        &self,
        sku_id: &str,
    ) -> Result<PurchaseOrderSnapshot, CloudApiError> {
        self.create_purchase_with_request_no(
            sku_id,
            &generate_purchase_request_no(),
        )
    }

    /// Create a purchase order via `POST /v1/billing/purchase` using an
    /// explicit `outRequestNo`, retrying once only on non-timeout transport
    /// errors with the exact same `outRequestNo`.
    pub fn create_purchase_with_request_no(
        &self,
        sku_id: &str,
        out_request_no: &str,
    ) -> Result<PurchaseOrderSnapshot, CloudApiError> {
        let clean_sku = sku_id.trim();
        if clean_sku.is_empty() {
            return Err(CloudApiError::new(
                Some(400),
                Some("BILLING_INVALID_SKU"),
                "SKU identifier is required",
                None,
            ));
        }
        let trimmed_req = out_request_no.trim();
        let fallback_req;
        let clean_req_no = if trimmed_req.is_empty() {
            fallback_req = generate_purchase_request_no();
            fallback_req.as_str()
        } else {
            trimmed_req
        };
        let body = serde_json::to_vec(&serde_json::json!({
            "skuId": clean_sku,
            "outRequestNo": clean_req_no,
        }))
        .map_err(|e| {
            CloudApiError::new(
                None,
                Some("CLOUD_JSON_ERROR"),
                e.to_string(),
                None,
            )
        })?;

        let send_once = || {
            self.with_fresh_jwt_retry(|jwt| {
                self.bearer_json_envelope_with_agent(
                    &self.billing_agent,
                    "POST",
                    "/v1/billing/purchase",
                    jwt,
                    Some(&body),
                    &[],
                )
            })
        };

        match send_once() {
            Ok(order) => Ok(order),
            Err(err) if is_transport_retryable(&err) => {
                std::thread::sleep(BILLING_RETRY_BACKOFF);
                send_once()
            }
            Err(err) => Err(err),
        }
    }

    /// Redeem a supporter code via `POST /v1/entitlement/redeem` and update
    /// `cloud_state` JWT + entitlement without leaking the code in errors.
    pub fn redeem_code(
        &self,
        code: &str,
    ) -> Result<RedeemResult, CloudApiError> {
        let raw_trimmed = code.trim();
        let clean_code = normalize_redemption_code_input(code);
        if clean_code.is_empty() {
            return Err(CloudApiError::new(
                Some(400),
                Some("REDEMPTION_CODE_INVALID"),
                "Redemption code is required",
                None,
            ));
        }
        let body = serde_json::to_vec(&serde_json::json!({
            "code": clean_code,
        }))
        .map_err(|e| {
            CloudApiError::new(
                None,
                Some("CLOUD_JSON_ERROR"),
                e.to_string(),
                None,
            )
        })?;
        let secrets = [clean_code.as_str(), raw_trimmed];
        let res: RedeemResult = self.with_fresh_jwt_retry(|jwt| {
            self.bearer_json_envelope(
                "POST",
                "/v1/entitlement/redeem",
                jwt,
                Some(&body),
                &secrets,
            )
        })?;
        let now = now_unix_secs();
        let mut state = self.load_state()?;
        state.subject_id = Some(res.entitlement.subject_id.clone());
        if !res.entitlement.device_id.trim().is_empty() {
            state.device_id = Some(res.entitlement.device_id.clone());
        }
        if !res.access_token.trim().is_empty() {
            state.access_token = Some(res.access_token.clone());
            state.token_expires_at_secs = res.expires_at_secs;
        }
        state.entitlement_json = serde_json::to_string(&res.entitlement).ok();
        state.last_synced_at_secs = now;
        state.updated_at_secs = now;
        self.save_state(&state)?;
        Ok(res)
    }

    /// Fetch bare `OpenAI` `GET /v1/models` and cache `cloud_state.models_json`.
    pub fn fetch_virtual_models(
        &self,
    ) -> Result<Vec<CloudVirtualModel>, CloudApiError> {
        let parsed: VirtualModelsListResponse =
            self.with_fresh_jwt_retry(|jwt| {
                let secrets = [jwt];
                let (status, header_trace, raw_body) = self
                    .send_http_with_agent(
                        &self.agent,
                        "GET",
                        "/v1/models",
                        CloudRequestAuth::Bearer(jwt),
                        None,
                        &secrets,
                    )?;
                if !(200..300).contains(&status) {
                    return Err(parse_cloud_error_parts(
                        status,
                        header_trace.as_deref(),
                        None,
                        &raw_body,
                        &secrets,
                    ));
                }
                serde_json::from_str(&raw_body).map_err(|e| {
                    CloudApiError::new(
                        Some(status),
                        Some("CLOUD_PROTOCOL_ERROR"),
                        format!("invalid /v1/models response: {e}"),
                        header_trace.as_deref(),
                    )
                })
            })?;
        let models = parsed.data;
        let now = now_unix_secs();
        let mut state = self.load_state()?;
        state.models_json = serde_json::to_string(&models).ok();
        if let Some(ref selected_id) = state.selected_model_id {
            let still_available =
                models.iter().any(|m| m.id == *selected_id && m.available);
            if !still_available {
                state.selected_model_id = None;
            }
        }
        state.updated_at_secs = now;
        self.save_state(&state)?;
        Ok(models)
    }

    fn load_state(&self) -> Result<CloudState, CloudApiError> {
        self.db.load_cloud_state().map_err(storage_err)
    }

    fn save_state(&self, state: &CloudState) -> Result<(), CloudApiError> {
        self.db.save_cloud_state(state).map_err(storage_err)
    }

    fn register_device_internal(
        &self,
        sync_entitlement_on_reinstall: bool,
    ) -> Result<CloudState, CloudApiError> {
        let challenge: ChallengeResponse =
            self.public_post_envelope("/v1/devices/challenge", b"{}")?;
        let pow_solution =
            solve_pow_challenge(&challenge.nonce, challenge.difficulty_bits)
                .map_err(|e| {
                    CloudApiError::new(None, Some("CLOUD_POW_FAILED"), e, None)
                })?;

        let signing_key = self
            .cred_store
            .load_or_create_cloud_signing_key()
            .map_err(storage_err)?;
        let pubkey_b64 = signing_key_public_b64(&signing_key);
        let device_id = derive_device_id(&pubkey_b64).map_err(storage_err)?;
        let anchors = self.collect_anchors()?;

        let os = match self.identity_env.os {
            CloudIdentityOs::MacOs => "macos",
            CloudIdentityOs::Linux => "linux",
            CloudIdentityOs::Windows => "windows",
        };
        let req_payload = RegisterDeviceRequest {
            pubkey_b64,
            hw_anchor_hash: anchors.hw_anchor_hash,
            os_anchor_hash: anchors.os_anchor_hash,
            challenge_nonce: challenge.nonce,
            pow_solution,
            os,
            app_version: env!("CARGO_PKG_VERSION"),
        };
        let body_bytes = serde_json::to_vec(&req_payload).map_err(|e| {
            CloudApiError::new(
                None,
                Some("CLOUD_JSON_ERROR"),
                e.to_string(),
                None,
            )
        })?;
        let ts = now_unix_secs();
        let nonce = uuid::Uuid::new_v4().to_string();
        let auth_headers = sign_cloud_request(
            &signing_key,
            &device_id,
            "POST",
            "/v1/devices/register",
            ts,
            &nonce,
            &body_bytes,
        );

        let reg: RegisterResponse = self.post_envelope_signed(
            "/v1/devices/register",
            &auth_headers,
            &body_bytes,
        )?;

        let now = now_unix_secs();
        let mut state = self.load_state()?;
        state.device_id = Some(if reg.device_id.trim().is_empty() {
            device_id
        } else {
            reg.device_id
        });
        state.subject_id = Some(reg.subject_id);
        state.access_token = Some(reg.access_token);
        state.token_expires_at_secs = reg.expires_at_secs;
        state.updated_at_secs = now;
        self.save_state(&state)?;

        if sync_entitlement_on_reinstall && !reg.is_new_subject {
            let _ = self.fetch_entitlement(false)?;
            return self.load_state();
        }
        Ok(state)
    }

    fn collect_anchors(
        &self,
    ) -> Result<crate::cloud_identity::DeviceAnchors, CloudApiError> {
        #[cfg(test)]
        if let Some(ref runner) = self.test_runner {
            return collect_device_anchors(
                &self.db,
                &self.identity_env,
                runner.as_ref(),
            )
            .map_err(storage_err);
        }
        let runner = TimedCommandRunner::new(Duration::from_secs(5));
        collect_device_anchors(&self.db, &self.identity_env, &runner)
            .map_err(storage_err)
    }

    fn public_post_envelope<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &[u8],
    ) -> Result<T, CloudApiError> {
        let (status, header_trace, raw_body) = self.send_http_with_agent(
            &self.agent,
            "POST",
            path,
            CloudRequestAuth::None,
            Some(body),
            &[],
        )?;
        decode_envelope(status, header_trace.as_deref(), &raw_body, &[])
    }

    fn post_envelope_signed<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        auth_headers: &crate::cloud_identity::CloudAuthHeaders,
        body: &[u8],
    ) -> Result<T, CloudApiError> {
        let (status, header_trace, raw_body) = self.send_http_with_agent(
            &self.agent,
            "POST",
            path,
            CloudRequestAuth::Signed(auth_headers),
            Some(body),
            &[],
        )?;
        decode_envelope(status, header_trace.as_deref(), &raw_body, &[])
    }

    fn bearer_json_envelope<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        path: &str,
        jwt: &str,
        body: Option<&[u8]>,
        extra_secrets: &[&str],
    ) -> Result<T, CloudApiError> {
        self.bearer_json_envelope_with_agent(
            &self.agent,
            method,
            path,
            jwt,
            body,
            extra_secrets,
        )
    }

    fn bearer_json_envelope_with_agent<T: for<'de> Deserialize<'de>>(
        &self,
        agent: &ureq::Agent,
        method: &str,
        path: &str,
        jwt: &str,
        body: Option<&[u8]>,
        extra_secrets: &[&str],
    ) -> Result<T, CloudApiError> {
        let mut secrets = Vec::with_capacity(1 + extra_secrets.len());
        secrets.push(jwt);
        secrets.extend_from_slice(extra_secrets);
        let (status, header_trace, raw_body) = self.send_http_with_agent(
            agent,
            method,
            path,
            CloudRequestAuth::Bearer(jwt),
            body,
            &secrets,
        )?;
        decode_envelope(status, header_trace.as_deref(), &raw_body, &secrets)
    }

    fn send_http_with_agent(
        &self,
        agent: &ureq::Agent,
        method: &str,
        path: &str,
        auth: CloudRequestAuth<'_>,
        body: Option<&[u8]>,
        secrets_to_redact: &[&str],
    ) -> Result<(u16, Option<String>, String), CloudApiError> {
        let base = self.resolved_base_url()?;
        let url = format!("{base}{path}");
        let response_res = match method {
            "GET" => {
                let mut req = agent
                    .get(&url)
                    .header(HEADER_CLIENT_VERSION, env!("CARGO_PKG_VERSION"));
                match auth {
                    CloudRequestAuth::None => {}
                    CloudRequestAuth::Bearer(tok) => {
                        req = req
                            .header("Authorization", &format!("Bearer {tok}"));
                    }
                    CloudRequestAuth::Signed(signed) => {
                        for (k, v) in signed.as_pairs() {
                            req = req.header(k, v);
                        }
                    }
                }
                req.call()
            }
            "POST" => {
                let mut req = agent
                    .post(&url)
                    .header(HEADER_CLIENT_VERSION, env!("CARGO_PKG_VERSION"))
                    .header("Content-Type", "application/json");
                match auth {
                    CloudRequestAuth::None => {}
                    CloudRequestAuth::Bearer(tok) => {
                        req = req
                            .header("Authorization", &format!("Bearer {tok}"));
                    }
                    CloudRequestAuth::Signed(signed) => {
                        for (k, v) in signed.as_pairs() {
                            req = req.header(k, v);
                        }
                    }
                }
                req.send(body.unwrap_or(b"{}"))
            }
            other => {
                return Err(CloudApiError::new(
                    None,
                    Some("CLOUD_INVALID_METHOD"),
                    format!("unsupported HTTP method {other}"),
                    None,
                ));
            }
        };

        let mut response = match response_res {
            Ok(resp) => resp,
            Err(ureq::Error::Timeout(reason)) => {
                return Err(CloudApiError {
                    http_status: None,
                    code: Some("COMMON_UPSTREAM_TIMEOUT".to_owned()),
                    message: format!(
                        "Ruum Cloud request timed out: {reason:?}"
                    ),
                    trace_id: None,
                    retry_after_secs: None,
                    is_timeout: true,
                });
            }
            Err(err) => {
                let redacted =
                    redact_secrets(&err.to_string(), secrets_to_redact);
                return Err(CloudApiError::new(
                    None,
                    Some("CLOUD_NETWORK_ERROR"),
                    format!("Ruum Cloud request failed: {redacted}"),
                    None,
                ));
            }
        };

        let status = response.status().as_u16();
        let header_trace = response
            .headers()
            .get(HEADER_TRACE_ID)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        let retry_after = response
            .headers()
            .get("Retry-After")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok());
        let raw_body = response.body_mut().read_to_string().unwrap_or_default();

        if !(200..300).contains(&status) {
            let mut secrets = secrets_to_redact.to_vec();
            if let CloudRequestAuth::Bearer(tok) = auth {
                secrets.push(tok);
            }
            return Err(parse_cloud_error_parts(
                status,
                header_trace.as_deref(),
                retry_after,
                &raw_body,
                &secrets,
            ));
        }

        Ok((status, header_trace, raw_body))
    }

    #[cfg(test)]
    fn with_test_identity(
        mut self,
        env: CloudIdentityEnv,
        runner: Arc<dyn CommandRunner>,
    ) -> Self {
        self.identity_env = env;
        self.test_runner = Some(runner);
        self
    }
}

const fn default_none<T>() -> Option<T> {
    None
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiEnvelope<T> {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default = "default_none")]
    data: Option<T>,
    #[serde(default, alias = "trace_id")]
    trace_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChallengeResponse {
    nonce: String,
    difficulty_bits: u8,
    #[serde(default)]
    #[allow(
        dead_code,
        reason = "deserialized from challenge payload for protocol parity"
    )]
    expires_at_secs: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisterDeviceRequest<'a> {
    pubkey_b64: String,
    hw_anchor_hash: String,
    os_anchor_hash: String,
    challenge_nonce: String,
    pow_solution: String,
    os: &'a str,
    app_version: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterResponse {
    device_id: String,
    subject_id: String,
    access_token: String,
    expires_at_secs: u64,
    #[serde(default)]
    #[allow(dead_code, reason = "deserialized from registration response")]
    trial_remaining: u64,
    #[serde(default)]
    is_new_subject: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenResponse {
    access_token: String,
    expires_at_secs: u64,
    #[serde(default)]
    device_id: Option<String>,
    #[serde(default)]
    subject_id: Option<String>,
}

#[derive(Deserialize)]
struct VirtualModelsListResponse {
    #[serde(default)]
    data: Vec<CloudVirtualModel>,
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn storage_err(msg: impl fmt::Display) -> CloudApiError {
    CloudApiError::new(None, Some("CLOUD_STORAGE_ERROR"), msg.to_string(), None)
}

#[derive(Clone, Copy)]
enum CloudRequestAuth<'a> {
    None,
    Bearer(&'a str),
    Signed(&'a crate::cloud_identity::CloudAuthHeaders),
}

fn is_reregister_recoverable(err: &CloudApiError) -> bool {
    matches!(
        (err.http_status, err.code.as_deref()),
        (Some(404), Some("DEVICE_NOT_FOUND") | None)
            | (Some(401), Some("AUTH_UNAUTHORIZED" | "DEVICE_NOT_FOUND"))
    )
}

fn is_transport_retryable(err: &CloudApiError) -> bool {
    err.http_status.is_none()
        && err.code.as_deref() == Some("CLOUD_NETWORK_ERROR")
        && !err.is_timeout
}

fn is_retryable_billing_error(err: &CloudApiError) -> bool {
    if err.http_status.is_some_and(|s| (400..500).contains(&s)) {
        return false;
    }
    if matches!(
        err.code.as_deref(),
        Some("BILLING_PRODUCT_DISABLED" | "COMMON_IDEMPOTENCY_CONFLICT")
    ) {
        return false;
    }
    matches!(
        err.code.as_deref(),
        Some(
            "CLOUD_NETWORK_ERROR"
                | "COMMON_UPSTREAM_TIMEOUT"
                | "BILLING_UNAVAILABLE"
                | "COMMON_SERVICE_UNAVAILABLE"
        )
    ) || matches!(err.http_status, Some(502..=504))
}

fn build_agent_with_timeout(timeout_global: Duration) -> ureq::Agent {
    let cfg = ureq::Agent::config_builder()
        .timeout_connect(Some(CLOUD_CONNECT_TIMEOUT))
        .timeout_global(Some(timeout_global))
        .max_redirects(0)
        .http_status_as_error(false)
        .user_agent(concat!("ruum/", env!("CARGO_PKG_VERSION")))
        .build();
    ureq::Agent::new_with_config(cfg)
}

fn build_control_agent() -> ureq::Agent {
    build_agent_with_timeout(CLOUD_CONTROL_TIMEOUT)
}

fn build_billing_agent() -> ureq::Agent {
    build_agent_with_timeout(CLOUD_BILLING_TIMEOUT)
}

fn redact_secrets(raw: &str, secrets: &[&str]) -> String {
    let mut out = raw.to_owned();
    for raw_sec in secrets {
        let sec = raw_sec.trim();
        if sec.is_empty() {
            continue;
        }
        out = out.replace(sec, "[REDACTED]");
        if sec.len() >= 6 {
            let lower = sec.to_ascii_lowercase();
            let upper = sec.to_ascii_uppercase();
            out = out.replace(&lower, "[REDACTED]");
            out = out.replace(&upper, "[REDACTED]");
        }
    }
    out
}

fn decode_envelope<T: for<'de> Deserialize<'de>>(
    status: u16,
    header_trace: Option<&str>,
    raw_body: &str,
    secrets_to_redact: &[&str],
) -> Result<T, CloudApiError> {
    let env: ApiEnvelope<T> = serde_json::from_str(raw_body).map_err(|e| {
        CloudApiError::new(
            Some(status),
            Some("CLOUD_PROTOCOL_ERROR"),
            redact_secrets(
                &format!("invalid Ruum Cloud envelope: {e}"),
                secrets_to_redact,
            ),
            header_trace,
        )
    })?;
    let trace_id = env
        .trace_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or(header_trace);
    if let Some(data) = env.data {
        return Ok(data);
    }
    let code = env
        .code
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "OK");
    let msg = env
        .message
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("missing response data");
    Err(CloudApiError::new(
        Some(status),
        code.or(Some("CLOUD_PROTOCOL_ERROR")),
        redact_secrets(msg, secrets_to_redact),
        trace_id,
    ))
}

fn parse_cloud_error_parts(
    status: u16,
    header_trace: Option<&str>,
    retry_after_secs: Option<u64>,
    raw_body: &str,
    secrets_to_redact: &[&str],
) -> CloudApiError {
    let parsed = serde_json::from_str::<Value>(raw_body).ok();
    let err_obj = parsed.as_ref().and_then(|v| v.get("error"));

    let code = parsed
        .as_ref()
        .and_then(|v| v.get("code"))
        .and_then(Value::as_str)
        .filter(|c| *c != "OK")
        .or_else(|| err_obj.and_then(|e| e.get("code")).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    let trace_id = parsed
        .as_ref()
        .and_then(|v| v.get("traceId").or_else(|| v.get("trace_id")))
        .and_then(Value::as_str)
        .or_else(|| {
            err_obj
                .and_then(|e| e.get("traceId").or_else(|| e.get("trace_id")))
                .and_then(Value::as_str)
        })
        .or(header_trace)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    let raw_msg = parsed
        .as_ref()
        .and_then(|v| v.get("message"))
        .and_then(Value::as_str)
        .or_else(|| {
            err_obj
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
        })
        .filter(|s| !s.trim().is_empty())
        .map_or_else(
            || {
                if raw_body.trim().is_empty() {
                    format!("HTTP {status}")
                } else {
                    raw_body.to_owned()
                }
            },
            ToOwned::to_owned,
        );

    let message = redact_secrets(&raw_msg, secrets_to_redact);
    let is_timeout = matches!(status, 408 | 504)
        || code.as_deref() == Some("COMMON_UPSTREAM_TIMEOUT");

    CloudApiError {
        http_status: Some(status),
        code,
        message,
        trace_id,
        retry_after_secs,
        is_timeout,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fmt::Write as _;
    use std::io::{self, Read as _, Write as _};
    use std::net::TcpListener;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
    use serde_json::{Value, json};
    use sha2::{Digest as _, Sha256};

    use super::*;
    use crate::ai_cli::CommandOutput;
    use crate::cloud_identity::{CloudIdentityOs, verify_pow_solution};
    use crate::credentials::CREDENTIALS_FILE_NAME;
    use crate::credentials::test_helpers::MemoryRootSecretStore;

    #[derive(Clone, Debug)]
    struct RecordedHttpRequest {
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        raw_body: Vec<u8>,
    }

    impl RecordedHttpRequest {
        fn header(&self, name: &str) -> Option<&str> {
            let target = name.to_ascii_lowercase();
            self.headers
                .iter()
                .find(|(k, _)| k == &target)
                .map(|(_, v)| v.as_str())
        }

        fn json_body(&self) -> Value {
            serde_json::from_slice(&self.raw_body).unwrap_or(Value::Null)
        }
    }

    #[derive(Clone)]
    struct StubResponse {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    }

    impl StubResponse {
        fn json(status: u16, val: &Value) -> Self {
            Self {
                status,
                headers: Vec::new(),
                body: val.to_string(),
            }
        }

        fn with_header(mut self, k: &str, v: &str) -> Self {
            self.headers.push((k.to_owned(), v.to_owned()));
            self
        }
    }

    struct StubServer {
        base_url: String,
        recorded: Arc<Mutex<Vec<RecordedHttpRequest>>>,
        stop: Arc<AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl StubServer {
        fn start(responses: Vec<StubResponse>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let addr = listener.local_addr().unwrap();
            let queue = Arc::new(Mutex::new(VecDeque::from(responses)));
            let recorded = Arc::new(Mutex::new(Vec::new()));
            let recorded_bg = Arc::clone(&recorded);
            let stop = Arc::new(AtomicBool::new(false));
            let stop_bg = Arc::clone(&stop);
            let deadline = Instant::now() + Duration::from_secs(30);

            let handle = std::thread::spawn(move || {
                while !stop_bg.load(Ordering::SeqCst)
                    && Instant::now() < deadline
                {
                    let mut stream = match listener.accept() {
                        Ok((stream, _)) => stream,
                        Err(ref err)
                            if err.kind() == io::ErrorKind::WouldBlock =>
                        {
                            std::thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Err(_) => break,
                    };
                    let _ = stream.set_nonblocking(false);
                    let _ =
                        stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut buf = Vec::new();
                    let mut tmp = [0_u8; 4096];
                    let mut header_end = None;
                    while let Ok(n) = stream.read(&mut tmp) {
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(pos) =
                            buf.windows(4).position(|w| w == b"\r\n\r\n")
                        {
                            header_end = Some(pos + 4);
                            break;
                        }
                    }
                    let Some(hend) = header_end else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&buf[..hend]);
                    let mut lines = head.split("\r\n");
                    let req_line = lines.next().unwrap_or("");
                    let mut parts = req_line.split_whitespace();
                    let method = parts.next().unwrap_or("").to_owned();
                    let path = parts.next().unwrap_or("").to_owned();
                    let mut headers = Vec::new();
                    let mut content_len = 0_usize;
                    for line in lines {
                        if line.is_empty() {
                            continue;
                        }
                        if let Some((k, v)) = line.split_once(':') {
                            let kl = k.trim().to_ascii_lowercase();
                            let val = v.trim().to_owned();
                            if kl == "content-length" {
                                content_len = val.parse::<usize>().unwrap_or(0);
                            }
                            headers.push((kl, val));
                        }
                    }
                    while buf.len() < hend + content_len {
                        match stream.read(&mut tmp) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => buf.extend_from_slice(&tmp[..n]),
                        }
                    }
                    let raw_body =
                        buf[hend..(hend + content_len).min(buf.len())].to_vec();
                    recorded_bg.lock().unwrap().push(RecordedHttpRequest {
                        method,
                        path,
                        headers,
                        raw_body,
                    });

                    let reply =
                        queue.lock().unwrap().pop_front().unwrap_or_else(
                            || {
                                StubResponse::json(
                                    500,
                                    &json!({
                                        "code": "MOCK_EXHAUSTED",
                                        "message": "unexpected request",
                                        "data": null,
                                        "traceId": "tr-exhausted"
                                    }),
                                )
                            },
                        );
                    let mut resp = format!(
                        "HTTP/1.1 {} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                        reply.status,
                        reply.body.len()
                    );
                    for (k, v) in reply.headers {
                        let _ = write!(resp, "{k}: {v}\r\n");
                    }
                    resp.push_str("\r\n");
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.write_all(reply.body.as_bytes());
                    let _ = stream.flush();
                }
            });

            Self {
                base_url: format!("http://{addr}"),
                recorded,
                stop,
                handle: Some(handle),
            }
        }

        fn recorded(&self) -> Vec<RecordedHttpRequest> {
            self.recorded.lock().unwrap().clone()
        }

        fn stop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    impl Drop for StubServer {
        fn drop(&mut self) {
            self.stop();
        }
    }

    struct StaticLinuxIdentityRunner;
    impl CommandRunner for StaticLinuxIdentityRunner {
        fn run(
            &self,
            _program: &Path,
            _args: &[&str],
            _envs: &[(&str, &str)],
        ) -> io::Result<CommandOutput> {
            Ok(CommandOutput {
                status_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    fn linux_test_env(dir: &Path) -> CloudIdentityEnv {
        let dmi = dir.join("dmi");
        std::fs::create_dir_all(&dmi).unwrap();
        std::fs::write(
            dmi.join("product_uuid"),
            "11111111-2222-3333-4444-555555555555\n",
        )
        .unwrap();
        let mid = dir.join("machine-id");
        std::fs::write(&mid, "abcdef0123456789abcdef0123456789\n").unwrap();
        CloudIdentityEnv {
            os: CloudIdentityOs::Linux,
            linux_dmi_dir: dmi,
            linux_machine_id_path: mid.clone(),
            linux_dbus_machine_id_path: mid,
        }
    }

    fn test_client(
        dir: &Path,
        base_url: &str,
    ) -> (CloudClient, Arc<StorageDb>, LazyCredentialStore) {
        let db = Arc::new(StorageDb::open(dir.join("ruum.db")).unwrap());
        let root = Arc::new(MemoryRootSecretStore::new([42_u8; 32]));
        let creds =
            LazyCredentialStore::new(dir.join(CREDENTIALS_FILE_NAME), root);
        let client = CloudClient::new(Arc::clone(&db), creds.clone())
            .with_base_url(base_url)
            .unwrap()
            .with_test_identity(
                linux_test_env(dir),
                Arc::new(StaticLinuxIdentityRunner),
            );
        (client, db, creds)
    }

    fn future_ts() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 900
    }

    #[test]
    fn resolve_cloud_base_url_enforces_https_or_loopback_http() {
        assert!(
            resolve_cloud_base_url_with(
                Some("http://insecure.example.com"),
                None,
                None,
            )
            .is_err()
        );
        assert_eq!(
            resolve_cloud_base_url_with(
                Some("http://127.0.0.1:8787/"),
                None,
                None,
            )
            .unwrap(),
            "http://127.0.0.1:8787"
        );
        assert_eq!(
            resolve_cloud_base_url_with(
                Some("http://localhost:8787"),
                None,
                None,
            )
            .unwrap(),
            "http://localhost:8787"
        );
        assert_eq!(
            resolve_cloud_base_url_with(Some("http://[::1]:8787"), None, None,)
                .unwrap(),
            "http://[::1]:8787"
        );
        assert_eq!(
            resolve_cloud_base_url_with(None, None, None).unwrap(),
            DEFAULT_CLOUD_GATEWAY_BASE_URL
        );
    }

    #[test]
    fn register_solves_pow_verifies_ed25519_sig_and_syncs_entitlement_on_reinstall()
     {
        let temp = tempfile::tempdir().unwrap();
        let exp = future_ts();
        let server = StubServer::start(vec![
            StubResponse::json(
                200,
                &json!({
                    "code": "OK",
                    "message": "success",
                    "data": {
                        "nonce": "chal-nonce-1",
                        "difficultyBits": 8,
                        "expiresAtSecs": exp
                    },
                    "traceId": "tr-chal"
                }),
            ),
            StubResponse::json(
                200,
                &json!({
                    "code": "OK",
                    "message": "success",
                    "data": {
                        "deviceId": "dev_placeholder",
                        "subjectId": "sub_restored",
                        "accessToken": "jwt-reinstalled",
                        "expiresAtSecs": exp,
                        "trialRemaining": 0,
                        "isNewSubject": false
                    },
                    "traceId": "tr-reg"
                }),
            ),
            StubResponse::json(
                200,
                &json!({
                    "code": "OK",
                    "message": "success",
                    "data": {
                        "subjectId": "sub_restored",
                        "deviceId": "dev_placeholder",
                        "isPaid": true,
                        "source": "alipay",
                        "decision": "ALLOW",
                        "trialRemaining": 0,
                        "validFromSecs": 1_700_000_000,
                        "validUntilSecs": exp + 86400,
                        "alipayValidUntilSecs": exp + 86400,
                        "grantValidUntilSecs": null,
                        "fupZone": "green",
                        "dailyCostMicroCny": 100,
                        "dailyBudgetMicroCny": 100_000,
                        "monthlyCostMicroCny": 100,
                        "monthlyBudgetMicroCny": 1_000_000,
                        "syncedAtSecs": exp - 10
                    },
                    "traceId": "tr-ent"
                }),
            ),
        ]);

        let (client, db, creds) = test_client(temp.path(), &server.base_url);
        let state = client.ensure_device_registered().unwrap();
        assert_eq!(state.subject_id.as_deref(), Some("sub_restored"));
        assert_eq!(state.access_token.as_deref(), Some("jwt-reinstalled"));

        // Entitlement synced automatically because isNewSubject == false.
        let cached_state = db.load_cloud_state().unwrap();
        let ent: EntitlementSnapshot = serde_json::from_str(
            cached_state.entitlement_json.as_deref().unwrap(),
        )
        .unwrap();
        assert!(ent.is_paid);
        assert_eq!(ent.source, "alipay");

        let reqs = server.recorded();
        assert_eq!(reqs.len(), 3);
        assert_eq!(reqs[0].method, "POST");
        assert_eq!(reqs[0].path, "/v1/devices/challenge");
        assert_eq!(reqs[1].path, "/v1/devices/register");
        assert_eq!(reqs[2].path, "/v1/entitlement/status");

        // Verify PoW, wire schema, and Ed25519 signature on
        // /v1/devices/register.
        let reg_body = reqs[1].json_body();
        assert_eq!(reg_body["os"].as_str(), Some("linux"));
        assert!(
            matches!(
                reg_body["os"].as_str(),
                Some("macos" | "windows" | "linux")
            ),
            "unexpected os wire value: {reg_body}"
        );
        assert!(
            reg_body.get("platform").is_none(),
            "register payload must not send legacy platform key: {reg_body}"
        );
        assert!(verify_pow_solution(
            reg_body["challengeNonce"].as_str().unwrap(),
            reg_body["powSolution"].as_str().unwrap(),
            8
        ));
        let pub_b64 = reg_body["pubkeyB64"].as_str().unwrap();
        let pub_bytes: [u8; 32] =
            STANDARD.decode(pub_b64).unwrap().try_into().unwrap();
        let vk = VerifyingKey::from_bytes(&pub_bytes).unwrap();
        let ts = reqs[1].header("x-ruum-ts").unwrap();
        let nonce = reqs[1].header("x-ruum-nonce").unwrap();
        let sig_b64 = reqs[1].header("x-ruum-sig").unwrap();
        let body_digest = format!("{:x}", Sha256::digest(&reqs[1].raw_body));
        let canonical =
            format!("POST\n/v1/devices/register\n{ts}\n{nonce}\n{body_digest}");
        let sig =
            Signature::from_slice(&STANDARD.decode(sig_b64).unwrap()).unwrap();
        vk.verify(canonical.as_bytes(), &sig).unwrap();
        assert!(creds.has_cloud_signing_key().unwrap());
    }

    #[test]
    fn auth_token_refresh_handles_404_reregister_and_never_retries_403_revoked()
    {
        let temp = tempfile::tempdir().unwrap();
        let exp = future_ts();
        let server = StubServer::start(vec![
            // 1. /v1/auth/token returns 404 DEVICE_NOT_FOUND
            StubResponse::json(
                404,
                &json!({
                    "code": "DEVICE_NOT_FOUND",
                    "message": "not found",
                    "data": null,
                    "traceId": "tr-404"
                }),
            ),
            // 2. PoW challenge
            StubResponse::json(
                200,
                &json!({
                    "code": "OK",
                    "message": "success",
                    "data": {
                        "nonce": "chal-rereg",
                        "difficultyBits": 6,
                        "expiresAtSecs": exp
                    },
                    "traceId": "tr-c2"
                }),
            ),
            // 3. /v1/devices/register
            StubResponse::json(
                200,
                &json!({
                    "code": "OK",
                    "message": "success",
                    "data": {
                        "deviceId": "dev_1",
                        "subjectId": "sub_1",
                        "accessToken": "jwt-recovered",
                        "expiresAtSecs": exp,
                        "trialRemaining": 3,
                        "isNewSubject": true
                    },
                    "traceId": "tr-r2"
                }),
            ),
            // 4. Subsequent /v1/auth/token returns 403 AUTH_DEVICE_REVOKED
            StubResponse::json(
                403,
                &json!({
                    "code": "AUTH_DEVICE_REVOKED",
                    "message": "revoked",
                    "data": null,
                    "traceId": "tr-403"
                }),
            )
            .with_header("X-Trace-ID", "tr-403"),
        ]);

        let (client, db, creds) = test_client(temp.path(), &server.base_url);
        let sk = creds.load_or_create_cloud_signing_key().unwrap();
        let dev_id = crate::cloud_identity::derive_device_id(
            &crate::cloud_identity::signing_key_public_b64(&sk),
        )
        .unwrap();
        db.save_cloud_state(&CloudState {
            gateway_base_url: server.base_url.clone(),
            device_id: Some(dev_id),
            subject_id: Some("sub_1".to_owned()),
            access_token: Some("jwt-old".to_owned()),
            token_expires_at_secs: 1,
            ..CloudState::default()
        })
        .unwrap();

        let state = client.refresh_access_token().unwrap();
        assert_eq!(state.access_token.as_deref(), Some("jwt-recovered"));
        let reqs = server.recorded();
        assert_eq!(reqs.len(), 3);
        assert_eq!(reqs[0].path, "/v1/auth/token");
        assert_eq!(
            reqs[0].header("x-ruum-client-version"),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(reqs[1].path, "/v1/devices/challenge");
        assert_eq!(reqs[2].path, "/v1/devices/register");

        // Now 403 AUTH_DEVICE_REVOKED must fail immediately with 0 retries.
        let err = client.refresh_access_token().unwrap_err();
        assert_eq!(err.http_status, Some(403));
        assert_eq!(err.code.as_deref(), Some("AUTH_DEVICE_REVOKED"));
        assert_eq!(err.trace_id.as_deref(), Some("tr-403"));
        assert_eq!(server.recorded().len(), 4);
    }

    #[test]
    fn fetch_virtual_models_parses_bare_openai_json_and_selects_best_available()
    {
        let temp = tempfile::tempdir().unwrap();
        let exp = future_ts();
        let server = StubServer::start(vec![StubResponse::json(
            200,
            &json!({
                "object": "list",
                "traceId": "tr-models",
                "data": [
                    {
                        "id": "ruum-pro",
                        "object": "model",
                        "owned_by": "ruum-cloud",
                        "display_name": "Ruum Pro",
                        "description": "Paid flagship",
                        "tier": "paid",
                        "available": false
                    },
                    {
                        "id": "ruum-fast",
                        "object": "model",
                        "owned_by": "ruum-cloud",
                        "display_name": "Ruum Fast",
                        "description": "Trial friendly",
                        "tier": "trial",
                        "available": true
                    }
                ]
            }),
        )]);

        let (client, db, creds) = test_client(temp.path(), &server.base_url);
        let _ = creds.load_or_create_cloud_signing_key().unwrap();
        db.save_cloud_state(&CloudState {
            gateway_base_url: server.base_url.clone(),
            device_id: Some("dev_1".to_owned()),
            subject_id: Some("sub_1".to_owned()),
            access_token: Some("jwt-valid".to_owned()),
            token_expires_at_secs: exp,
            ..CloudState::default()
        })
        .unwrap();

        let models = client.fetch_virtual_models().unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].tier, "paid");
        assert!(!models[0].available);
        assert_eq!(models[0].description.as_deref(), Some("Paid flagship"));

        let chosen =
            select_best_available_model(&models, Some("ruum-pro")).unwrap();
        assert_eq!(chosen.id, "ruum-fast");

        let none_available = vec![models[0].clone()];
        assert!(select_best_available_model(&none_available, None).is_none());
    }

    #[test]
    fn billing_plans_non_enabled_or_empty_is_treated_as_unavailable_and_redeem_hides_secret()
     {
        let temp = tempfile::tempdir().unwrap();
        let exp = future_ts();
        let server = StubServer::start(vec![
            StubResponse::json(
                200,
                &json!({
                    "code": "OK",
                    "message": "success",
                    "data": {
                        "capabilityStatus": "DISABLED",
                        "productName": "Ruum Pro",
                        "billingMode": "TIME_BASED",
                        "chargingOptions": []
                    },
                    "traceId": "tr-plans"
                }),
            ),
            StubResponse::json(
                404,
                &json!({
                    "code": "REDEMPTION_CODE_INVALID",
                    "message": "invalid code",
                    "data": null,
                    "traceId": "tr-redeem-404"
                }),
            )
            .with_header("X-Trace-ID", "tr-redeem-404"),
        ]);

        let (client, db, creds) = test_client(temp.path(), &server.base_url);
        let _ = creds.load_or_create_cloud_signing_key().unwrap();
        db.save_cloud_state(&CloudState {
            gateway_base_url: server.base_url.clone(),
            device_id: Some("dev_1".to_owned()),
            subject_id: Some("sub_1".to_owned()),
            access_token: Some("jwt-valid".to_owned()),
            token_expires_at_secs: exp,
            ..CloudState::default()
        })
        .unwrap();

        let plans = client.fetch_billing_plans().unwrap();
        assert!(!plans.is_available());
        let avail_err = plans.ensure_available().unwrap_err();
        assert!(avail_err.is_billing_unavailable());

        let secret_code = "RUUM-SECRET-CODE-9999";
        let err = client.redeem_code(secret_code).unwrap_err();
        assert_eq!(err.http_status, Some(404));
        assert_eq!(err.code.as_deref(), Some("REDEMPTION_CODE_INVALID"));
        assert_eq!(err.trace_id.as_deref(), Some("tr-redeem-404"));
        assert!(!err.to_string().contains(secret_code));
    }

    #[test]
    fn normalize_redemption_code_input_handles_whitespace_and_case() {
        assert_eq!(normalize_redemption_code_input("   \t\n "), "");
        assert_eq!(
            normalize_redemption_code_input("  ruum-2345-6789-abcd  "),
            "RUUM-2345-6789-ABCD"
        );
        assert_eq!(
            normalize_redemption_code_input("ruum - 2345 - 6789 - abcd"),
            "RUUM-2345-6789-ABCD"
        );
        assert_eq!(normalize_redemption_code_input("bad"), "BAD");
    }

    #[test]
    fn redeem_code_syncs_jwt_expiry_and_entitlement_into_cloud_state() {
        let temp = tempfile::tempdir().unwrap();
        let exp = future_ts();
        let new_exp = exp + 1_800;
        let valid_until = 1_893_456_000_u64;
        let server = StubServer::start(vec![StubResponse::json(
            200,
            &json!({
                "code": "OK",
                "message": "success",
                "data": {
                    "redeemedDurationPeriod": "QUARTER",
                    "previousValidUntilSecs": null,
                    "newValidUntilSecs": valid_until,
                    "entitlement": {
                        "subjectId": "sub_redeemed",
                        "deviceId": "dev_1",
                        "isPaid": true,
                        "source": "redemption",
                        "decision": "ALLOW",
                        "trialRemaining": 0,
                        "validFromSecs": 1_700_000_000_u64,
                        "validUntilSecs": valid_until,
                        "alipayValidUntilSecs": null,
                        "grantValidUntilSecs": valid_until,
                        "fupZone": "green",
                        "dailyCostMicroCny": 0,
                        "dailyBudgetMicroCny": 100_000,
                        "monthlyCostMicroCny": 0,
                        "monthlyBudgetMicroCny": 1_000_000,
                        "syncedAtSecs": exp
                    },
                    "accessToken": "jwt-after-redeem",
                    "expiresAtSecs": new_exp
                },
                "traceId": "tr-redeem-ok"
            }),
        )]);

        let (client, db, creds) = test_client(temp.path(), &server.base_url);
        let _ = creds.load_or_create_cloud_signing_key().unwrap();
        db.save_cloud_state(&CloudState {
            gateway_base_url: server.base_url.clone(),
            device_id: Some("dev_1".to_owned()),
            subject_id: Some("sub_1".to_owned()),
            access_token: Some("jwt-before-redeem".to_owned()),
            token_expires_at_secs: exp,
            ..CloudState::default()
        })
        .unwrap();

        let res = client
            .redeem_code("  ruum - abcd - efgh - ijkm \n")
            .unwrap();
        assert_eq!(res.redeemed_duration_period, "QUARTER");
        assert_eq!(res.new_valid_until_secs, valid_until);
        assert_eq!(res.access_token, "jwt-after-redeem");
        assert_eq!(res.expires_at_secs, new_exp);
        assert!(res.entitlement.is_paid);
        assert_eq!(res.entitlement.source, "redemption");

        let saved = db.load_cloud_state().unwrap();
        assert_eq!(saved.subject_id.as_deref(), Some("sub_redeemed"));
        assert_eq!(saved.access_token.as_deref(), Some("jwt-after-redeem"));
        assert_eq!(saved.token_expires_at_secs, new_exp);
        let ent: EntitlementSnapshot =
            serde_json::from_str(saved.entitlement_json.as_deref().unwrap())
                .unwrap();
        assert!(ent.is_paid);
        assert_eq!(ent.source, "redemption");
        assert_eq!(ent.valid_until_secs, Some(valid_until));

        let reqs = server.recorded();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].method, "POST");
        assert_eq!(reqs[0].path, "/v1/entitlement/redeem");
        assert_eq!(
            reqs[0].json_body()["code"].as_str(),
            Some("RUUM-ABCD-EFGH-IJKM")
        );
    }

    #[test]
    fn redeem_code_rejects_empty_locally_and_never_leaks_code_on_all_errors() {
        let temp = tempfile::tempdir().unwrap();
        let exp = future_ts();
        let raw_secret = "  ruum-leak-test-9999  ";
        let norm_secret = "RUUM-LEAK-TEST-9999";
        let cases = [
            (404_u16, "REDEMPTION_CODE_INVALID", "tr-inv"),
            (409_u16, "REDEMPTION_CODE_ALREADY_USED", "tr-used"),
            (400_u16, "REDEMPTION_CODE_EXPIRED", "tr-exp"),
            (409_u16, "REDEMPTION_CODE_REVOKED", "tr-rev"),
            (429_u16, "REDEMPTION_RATE_LIMITED", "tr-rate"),
        ];
        let responses = cases
            .iter()
            .map(|(status, code, tid)| {
                StubResponse::json(
                    *status,
                    &json!({
                        "code": code,
                        "message": format!(
                            "rejected {} / {}",
                            raw_secret.trim(),
                            norm_secret
                        ),
                        "data": null,
                        "traceId": tid
                    }),
                )
                .with_header("X-Trace-ID", tid)
            })
            .collect();
        let server = StubServer::start(responses);

        let (client, db, creds) = test_client(temp.path(), &server.base_url);
        let _ = creds.load_or_create_cloud_signing_key().unwrap();
        db.save_cloud_state(&CloudState {
            gateway_base_url: server.base_url.clone(),
            device_id: Some("dev_1".to_owned()),
            subject_id: Some("sub_1".to_owned()),
            access_token: Some("jwt-valid".to_owned()),
            token_expires_at_secs: exp,
            ..CloudState::default()
        })
        .unwrap();

        let empty_err = client.redeem_code("   \t\n ").unwrap_err();
        assert_eq!(empty_err.code.as_deref(), Some("REDEMPTION_CODE_INVALID"));
        assert!(server.recorded().is_empty());

        for (status, code, tid) in cases {
            let err = client.redeem_code(raw_secret).unwrap_err();
            assert_eq!(err.http_status, Some(status));
            assert_eq!(err.code.as_deref(), Some(code));
            assert_eq!(err.trace_id.as_deref(), Some(tid));
            let display = err.to_string();
            assert!(!display.contains(raw_secret.trim()));
            assert!(!display.contains(norm_secret));
            for locale in [Locale::En, Locale::Zh, Locale::Ja, Locale::Ko] {
                let loc = err.format_localized(locale);
                assert!(!loc.contains(raw_secret.trim()));
                assert!(!loc.contains(norm_secret));
                assert!(loc.ends_with(&format!("[Trace: {tid}]")));
            }
        }
    }

    #[test]
    fn fetch_billing_plans_is_public_caches_json_and_policy() {
        let temp = tempfile::tempdir().unwrap();
        let server = StubServer::start(vec![StubResponse::json(
            200,
            &json!({
                "code": "OK",
                "message": "success",
                "data": {
                    "capabilityStatus": "ENABLED",
                    "productName": "Ruum Cloud",
                    "billingMode": "TIME_BASED",
                    "usagePolicy": {
                        "dailyBudgetMicroCny": 250_000,
                        "monthlyBudgetMicroCny": 1_800_000,
                        "estimatedDailyAnalyses": 25,
                        "estimatedMonthlyAnalyses": 180,
                        "softFallbackPercent": 70,
                        "resetTimezone": "Asia/Shanghai"
                    },
                    "chargingOptions": [
                        {
                            "skuId": "sku_month",
                            "displayName": "Monthly Pass",
                            "price": "9.90",
                            "durationPeriod": "MONTH"
                        },
                        {
                            "skuId": "sku_year",
                            "displayName": "Yearly Pass",
                            "price": "99.00",
                            "durationPeriod": "YEAR"
                        }
                    ]
                },
                "traceId": "tr-plans-public"
            }),
        )]);

        let (client, db, creds) = test_client(temp.path(), &server.base_url);
        let plans = client.fetch_billing_plans().unwrap();
        assert!(plans.is_available());
        assert_eq!(plans.charging_options.len(), 2);
        let policy = plans.usage_policy.as_ref().unwrap();
        assert_eq!(policy.daily_budget_micro_cny, 250_000);
        assert_eq!(policy.monthly_budget_micro_cny, 1_800_000);
        assert_eq!(policy.estimated_daily_analyses, 25);
        assert_eq!(policy.estimated_monthly_analyses, 180);
        assert_eq!(policy.soft_fallback_percent, 70);
        assert_eq!(policy.reset_timezone, "Asia/Shanghai");
        assert_eq!(plans.uniform_usage_policy(), Some(policy));

        // Must be completely unauthenticated with 0 PoW/keychain calls.
        assert!(!creds.has_cloud_signing_key().unwrap());
        let reqs = server.recorded();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].method, "GET");
        assert_eq!(reqs[0].path, "/v1/billing/plans");
        assert_eq!(reqs[0].header("authorization"), None);

        // Best-effort cached in cloud_state.
        let saved = db.load_cloud_state().unwrap();
        assert!(saved.plans_cached_at_secs > 0);
        let cached: BillingPlansSnapshot =
            serde_json::from_str(saved.plans_json.as_deref().unwrap()).unwrap();
        assert_eq!(cached, plans);
    }

    #[test]
    fn billing_plans_uniform_usage_policy_distinguishes_shared_vs_per_sku() {
        let pol_a = UsagePolicy {
            daily_budget_micro_cny: 250_000,
            monthly_budget_micro_cny: 1_800_000,
            estimated_daily_analyses: 25,
            estimated_monthly_analyses: 180,
            soft_fallback_percent: 70,
            reset_timezone: "Asia/Shanghai".to_owned(),
        };
        let pol_b = UsagePolicy {
            estimated_daily_analyses: 60,
            ..pol_a.clone()
        };
        let opt = |sku: &str, pol: Option<UsagePolicy>| ChargingOption {
            sku_id: sku.to_owned(),
            display_name: sku.to_owned(),
            price: "9.90".to_owned(),
            duration_period: Some("MONTH".to_owned()),
            usage_policy: pol,
        };

        // Shared top-level policy with None per-SKU options.
        let top_only = BillingPlansSnapshot {
            capability_status: "ENABLED".to_owned(),
            product_name: None,
            billing_mode: None,
            charging_options: vec![opt("a", None), opt("b", None)],
            usage_policy: Some(pol_a.clone()),
        };
        assert_eq!(top_only.uniform_usage_policy(), Some(&pol_a));

        // Explicit matching per-SKU policies.
        let all_match = BillingPlansSnapshot {
            charging_options: vec![
                opt("a", Some(pol_a.clone())),
                opt("b", Some(pol_a.clone())),
            ],
            ..top_only.clone()
        };
        assert_eq!(all_match.uniform_usage_policy(), Some(&pol_a));

        // All SKUs override with the same pol_b (even if top-level differs).
        let all_override_same = BillingPlansSnapshot {
            charging_options: vec![
                opt("a", Some(pol_b.clone())),
                opt("b", Some(pol_b.clone())),
            ],
            ..top_only.clone()
        };
        assert_eq!(all_override_same.uniform_usage_policy(), Some(&pol_b));

        // Differing per-SKU policies.
        let diff_skus = BillingPlansSnapshot {
            charging_options: vec![
                opt("a", Some(pol_a.clone())),
                opt("b", Some(pol_b.clone())),
            ],
            usage_policy: None,
            ..top_only.clone()
        };
        assert_eq!(diff_skus.uniform_usage_policy(), None);

        // One SKU overrides with pol_b while another falls back to pol_a.
        let mixed_fallback = BillingPlansSnapshot {
            charging_options: vec![opt("a", Some(pol_b)), opt("b", None)],
            ..top_only.clone()
        };
        assert_eq!(mixed_fallback.uniform_usage_policy(), None);

        // Top-level None with one SKU having policy and another None.
        let partial_no_top = BillingPlansSnapshot {
            charging_options: vec![
                opt("a", Some(pol_a.clone())),
                opt("b", None),
            ],
            usage_policy: None,
            ..top_only.clone()
        };
        assert_eq!(partial_no_top.uniform_usage_policy(), None);

        // Empty options falls back to top-level policy.
        let empty_opts = BillingPlansSnapshot {
            charging_options: Vec::new(),
            ..top_only
        };
        assert_eq!(empty_opts.uniform_usage_policy(), Some(&pol_a));
    }

    #[test]
    fn fetch_virtual_models_keeps_auto_and_resets_locked() {
        let temp = tempfile::tempdir().unwrap();
        let exp = future_ts();
        let models_payload = json!({
            "object": "list",
            "traceId": "tr-models",
            "data": [
                {
                    "id": "ruum-fast",
                    "display_name": "Ruum Fast",
                    "tier": "trial",
                    "available": true
                },
                {
                    "id": "ruum-pro",
                    "display_name": "Ruum Pro",
                    "tier": "paid",
                    "available": false
                }
            ]
        });
        let server = StubServer::start(vec![
            StubResponse::json(200, &models_payload),
            StubResponse::json(200, &models_payload),
            StubResponse::json(200, &models_payload),
            StubResponse::json(200, &models_payload),
        ]);

        let (client, db, creds) = test_client(temp.path(), &server.base_url);
        let _ = creds.load_or_create_cloud_signing_key().unwrap();

        let seed_selected = |selected: Option<&str>| {
            db.save_cloud_state(&CloudState {
                gateway_base_url: server.base_url.clone(),
                device_id: Some("dev_1".to_owned()),
                subject_id: Some("sub_1".to_owned()),
                access_token: Some("jwt-valid".to_owned()),
                token_expires_at_secs: exp,
                selected_model_id: selected.map(ToOwned::to_owned),
                ..CloudState::default()
            })
            .unwrap();
        };

        // 1. None (Auto) must stay None — never overwritten with first model.
        seed_selected(None);
        assert_eq!(client.cached_selected_model_id(), None);
        let _ = client.fetch_virtual_models().unwrap();
        assert_eq!(db.load_cloud_state().unwrap().selected_model_id, None);
        assert_eq!(client.cached_selected_model_id(), None);

        // 2. Valid available selection is preserved.
        seed_selected(Some("ruum-fast"));
        assert_eq!(
            client.cached_selected_model_id().as_deref(),
            Some("ruum-fast")
        );
        let _ = client.fetch_virtual_models().unwrap();
        assert_eq!(
            db.load_cloud_state().unwrap().selected_model_id.as_deref(),
            Some("ruum-fast")
        );

        // 3. Unavailable selection ("ruum-pro") resets to None (Auto).
        seed_selected(Some("ruum-pro"));
        let _ = client.fetch_virtual_models().unwrap();
        assert_eq!(db.load_cloud_state().unwrap().selected_model_id, None);

        // 4. Unknown selection resets to None (Auto).
        seed_selected(Some("locked-id"));
        let _ = client.fetch_virtual_models().unwrap();
        assert_eq!(db.load_cloud_state().unwrap().selected_model_id, None);
    }

    #[test]
    fn create_purchase_timeout_constant_and_request_no_generation() {
        assert_eq!(CLOUD_BILLING_TIMEOUT, Duration::from_mins(2));
        assert_eq!(CLOUD_CONTROL_TIMEOUT, Duration::from_mins(2));
        assert_eq!(CLOUD_CONNECT_TIMEOUT, Duration::from_mins(1));
        let req_a = generate_purchase_request_no();
        let req_b = generate_purchase_request_no();
        assert!(req_a.starts_with("req_"));
        assert_eq!(req_a.len(), 36);
        assert!(req_b.starts_with("req_"));
        assert_ne!(req_a, req_b);
    }

    #[test]
    fn create_purchase_with_request_no_does_not_retry_503_504_or_timeouts() {
        let exp = future_ts();
        for (status, code, req_no) in [
            (503_u16, "BILLING_UNAVAILABLE", "req_no_retry_503"),
            (504_u16, "COMMON_UPSTREAM_TIMEOUT", "req_no_retry_504"),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let server = StubServer::start(vec![
                StubResponse::json(
                    status,
                    &json!({
                        "code": code,
                        "message": "gateway failure",
                        "data": null,
                        "traceId": "tr-no-retry-1"
                    }),
                ),
                StubResponse::json(
                    200,
                    &json!({
                        "code": "OK",
                        "message": "unexpected second attempt",
                        "data": {
                            "decision": "PENDING",
                            "outRequestNo": req_no,
                            "purchaseUrl": "https://qr.alipay.com/test",
                            "purchaseQrCode": "https://qr.alipay.com/test",
                            "expireAtSecs": exp + 900
                        },
                        "traceId": "tr-no-retry-2"
                    }),
                ),
            ]);
            let (client, db, creds) =
                test_client(temp.path(), &server.base_url);
            let _ = creds.load_or_create_cloud_signing_key().unwrap();
            db.save_cloud_state(&CloudState {
                gateway_base_url: server.base_url.clone(),
                device_id: Some("dev_1".to_owned()),
                subject_id: Some("sub_1".to_owned()),
                access_token: Some("jwt-valid".to_owned()),
                token_expires_at_secs: exp,
                ..CloudState::default()
            })
            .unwrap();

            let err = client
                .create_purchase_with_request_no("sku_month", req_no)
                .unwrap_err();
            assert_eq!(err.http_status, Some(status));
            assert_eq!(err.code.as_deref(), Some(code));

            let reqs = server.recorded();
            assert_eq!(reqs.len(), 1, "must not auto-retry {status}");
            assert_eq!(reqs[0].method, "POST");
            assert_eq!(reqs[0].path, "/v1/billing/purchase");
            let body = reqs[0].json_body();
            assert_eq!(body["skuId"].as_str(), Some("sku_month"));
            assert_eq!(body["outRequestNo"].as_str(), Some(req_no));
        }

        let net_err = CloudApiError::new(
            None,
            Some("CLOUD_NETWORK_ERROR"),
            "connection reset",
            None,
        );
        assert!(is_transport_retryable(&net_err));

        let mut net_timeout = net_err;
        net_timeout.is_timeout = true;
        assert!(!is_transport_retryable(&net_timeout));

        let mut client_timeout = CloudApiError::new(
            None,
            Some("COMMON_UPSTREAM_TIMEOUT"),
            "timed out",
            None,
        );
        client_timeout.is_timeout = true;
        assert!(!is_transport_retryable(&client_timeout));
        assert!(!is_retryable_billing_error(&CloudApiError {
            http_status: None,
            code: Some("OTHER_ERROR".to_owned()),
            message: "timed out".to_owned(),
            trace_id: None,
            retry_after_secs: None,
            is_timeout: true,
        }));

        let err_503 = CloudApiError::new(
            Some(503),
            Some("BILLING_UNAVAILABLE"),
            "unavailable",
            None,
        );
        assert!(!is_transport_retryable(&err_503));
    }

    #[test]
    fn entitlement_snapshot_time_aware_helpers_and_plan_ttl() {
        let now = 2_000_000_000_u64;
        let base = EntitlementSnapshot {
            subject_id: "sub_1".to_owned(),
            device_id: "dev_1".to_owned(),
            is_paid: true,
            source: "alipay".to_owned(),
            decision: "ALLOW".to_owned(),
            trial_remaining: 0,
            valid_from_secs: None,
            valid_until_secs: Some(now + 1000),
            alipay_valid_until_secs: Some(now + 1000),
            grant_valid_until_secs: None,
            fup_zone: "green".to_owned(),
            daily_cost_micro_cny: 0,
            daily_budget_micro_cny: 100,
            monthly_cost_micro_cny: 0,
            monthly_budget_micro_cny: 1000,
            synced_at_secs: now,
        };

        // Active Alipay pass.
        assert!(base.is_alipay_active_at(now));
        assert!(base.is_effectively_paid_at(now));
        assert!(!base.has_expired_alipay_history_at(now));

        // Expired Alipay pass even with stale `is_paid: true`.
        let expired_alipay = EntitlementSnapshot {
            valid_until_secs: Some(now - 10),
            alipay_valid_until_secs: Some(now - 10),
            ..base.clone()
        };
        assert!(!expired_alipay.is_alipay_active_at(now));
        assert!(!expired_alipay.is_effectively_paid_at(now));
        assert!(expired_alipay.has_expired_alipay_history_at(now));

        // Pure redemption user (no Alipay pass).
        let pure_redeem = EntitlementSnapshot {
            source: "redemption".to_owned(),
            valid_until_secs: Some(now + 5000),
            alipay_valid_until_secs: None,
            grant_valid_until_secs: Some(now + 5000),
            ..base.clone()
        };
        assert!(!pure_redeem.is_alipay_active_at(now));
        assert!(pure_redeem.is_effectively_paid_at(now));
        assert!(!pure_redeem.has_expired_alipay_history_at(now));

        // Expired Alipay + active redemption grant.
        let expired_alipay_active_grant = EntitlementSnapshot {
            source: "redemption".to_owned(),
            valid_until_secs: Some(now + 5000),
            alipay_valid_until_secs: Some(now - 10),
            grant_valid_until_secs: Some(now + 5000),
            ..base.clone()
        };
        assert!(!expired_alipay_active_grant.is_alipay_active_at(now));
        assert!(expired_alipay_active_grant.is_effectively_paid_at(now));
        assert!(expired_alipay_active_grant.has_expired_alipay_history_at(now));

        // DENY overrides everything.
        let denied = EntitlementSnapshot {
            decision: "DENY".to_owned(),
            ..base.clone()
        };
        assert!(!denied.is_alipay_active_at(now));
        assert!(!denied.is_effectively_paid_at(now));

        // Legacy snapshot without explicit alipay/grant timestamps.
        let legacy_alipay = EntitlementSnapshot {
            alipay_valid_until_secs: None,
            grant_valid_until_secs: None,
            ..base
        };
        assert!(legacy_alipay.is_alipay_active_at(now));
        assert!(legacy_alipay.is_effectively_paid_at(now));
        assert!(!legacy_alipay.has_expired_alipay_history_at(now));

        // `cached_plans_need_refresh` checks empty JSON and TTL.
        let temp = tempfile::tempdir().unwrap();
        let (client, db, _creds) =
            test_client(temp.path(), "http://127.0.0.1:9");
        assert!(client.cached_plans_need_refresh(now, 3600));
        db.save_cloud_state(&CloudState {
            plans_json: Some("{\"chargingOptions\":[]}".to_owned()),
            plans_cached_at_secs: now - 3599,
            ..CloudState::default()
        })
        .unwrap();
        assert!(!client.cached_plans_need_refresh(now, 3600));
        assert!(client.cached_plans_need_refresh(now + 1, 3600));
    }

    #[test]
    fn create_purchase_with_request_no_does_not_retry_400_409_or_disabled() {
        let exp = future_ts();
        for (status, code) in [
            (400_u16, "BILLING_INVALID_SKU"),
            (409_u16, "COMMON_IDEMPOTENCY_CONFLICT"),
            (503_u16, "BILLING_PRODUCT_DISABLED"),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let server = StubServer::start(vec![
                StubResponse::json(
                    status,
                    &json!({
                        "code": code,
                        "message": "non-retryable billing error",
                        "data": null,
                        "traceId": "tr-no-retry"
                    }),
                ),
                StubResponse::json(
                    200,
                    &json!({
                        "code": "OK",
                        "message": "unexpected second attempt",
                        "data": {
                            "decision": "PENDING",
                            "outRequestNo": "req_should_not_send"
                        }
                    }),
                ),
            ]);
            let (client, db, creds) =
                test_client(temp.path(), &server.base_url);
            let _ = creds.load_or_create_cloud_signing_key().unwrap();
            db.save_cloud_state(&CloudState {
                gateway_base_url: server.base_url.clone(),
                device_id: Some("dev_1".to_owned()),
                subject_id: Some("sub_1".to_owned()),
                access_token: Some("jwt-valid".to_owned()),
                token_expires_at_secs: exp,
                ..CloudState::default()
            })
            .unwrap();

            let err = client
                .create_purchase_with_request_no("sku_month", "req_once")
                .unwrap_err();
            assert_eq!(err.http_status, Some(status));
            assert_eq!(err.code.as_deref(), Some(code));
            let reqs = server.recorded();
            assert_eq!(reqs.len(), 1, "must not retry {status} {code}");
            assert_eq!(
                reqs[0].json_body()["outRequestNo"].as_str(),
                Some("req_once")
            );
        }
    }

    #[test]
    fn fetch_billing_plans_retries_once_on_503() {
        let temp = tempfile::tempdir().unwrap();
        let server = StubServer::start(vec![
            StubResponse::json(
                503,
                &json!({
                    "code": "BILLING_UNAVAILABLE",
                    "message": "temporary upstream blip",
                    "data": null,
                    "traceId": "tr-plans-503"
                }),
            ),
            StubResponse::json(
                200,
                &json!({
                    "code": "OK",
                    "message": "success",
                    "data": {
                        "capabilityStatus": "ENABLED",
                        "productName": "Ruum",
                        "billingMode": "ONE_TIME_DURATION",
                        "chargingOptions": [
                            {
                                "skuId": "sku_month",
                                "displayName": "月卡",
                                "price": "2.90",
                                "durationPeriod": "MONTH"
                            }
                        ]
                    },
                    "traceId": "tr-plans-ok"
                }),
            ),
        ]);

        let (client, _db, _creds) = test_client(temp.path(), &server.base_url);
        let plans = client.fetch_billing_plans().unwrap();
        assert!(plans.is_available());
        assert_eq!(plans.charging_options.len(), 1);
        assert_eq!(server.recorded().len(), 2);
    }
}
