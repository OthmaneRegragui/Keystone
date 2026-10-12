//! Cloudflare Turnstile verification for unauthenticated browser endpoints.
//!
//! The browser widget only *proves* to Cloudflare that a human is present; the
//! widget output is a token that carries no trust on its own. This module is
//! the only place that turns that token into a decision: it calls Cloudflare's
//! `siteverify` endpoint with the server-side secret and reports success or
//! failure. Nothing else in the app is allowed to treat a token as valid.
//!
//! Behaviour notes:
//! * The keys live in the `admin_settings` table (managed from the admin
//!   settings screen) and are read on each check, so an admin can enable or
//!   rotate them without restarting the server. Disabled unless the admin
//!   enabled the toggle AND both keys are present, so a default install is
//!   untouched.
//! * Fail-closed. A network error, a non-2xx status, an unparsable body, or an
//!   explicit `success: false` all reject the request. Failing open would turn
//!   a Cloudflare outage into an unprotected login endpoint.
//! * Tokens are single-use and short-lived (5 minutes), so the browser must
//!   re-solve the widget after every rejected attempt. See `login.html`.

use std::net::IpAddr;
use std::time::Duration;

use serde::Deserialize;
use sqlx::PgPool;

use crate::config::CaptchaConfig;
use crate::db::repos::settings::AdminSettingRepository;
use crate::error::{AppError, AppResult};

/// Cloudflare's public `siteverify` endpoint.
pub const DEFAULT_VERIFY_URL: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

/// Verification is one HTTP round-trip on the login path; a slow or hanging
/// endpoint must not hold the request open, so the client has a hard timeout.
const REQUEST_TIMEOUT_SECS: u64 = 5;

/// Upper bound on the token we forward to Cloudflare. Real Turnstile tokens are
/// well under 2 KiB; the cap keeps a hostile client from turning the login
/// endpoint into an arbitrary-size outbound request.
const MAX_TOKEN_LEN: usize = 4096;

/// Shape of Cloudflare's `siteverify` response. Unknown/extra fields are
/// ignored, and every field is optional so a truncated or unexpected body
/// still deserializes (and is then rejected by `success: false`).
#[derive(Debug, Deserialize)]
struct VerifyResponse {
    #[serde(default)]
    success: bool,
    #[serde(default, rename = "error-codes")]
    error_codes: Vec<String>,
    #[serde(default)]
    hostname: Option<String>,
}

/// Turnstile verification.
///
/// Holds a long-lived [`reqwest::Client`] so TLS connections to Cloudflare are
/// pooled across login attempts instead of being rebuilt per request. The keys
/// are deliberately *not* cached here: they are admin-editable at runtime, and
/// a stale cached key would be a silent security hole.
pub struct CaptchaService {
    client: reqwest::Client,
    verify_url: String,
}

impl CaptchaService {
    pub fn new(config: &CaptchaConfig) -> Self {
        let verify_url = match config.turnstile_verify_url.trim() {
            "" => DEFAULT_VERIFY_URL.to_string(),
            url => url.to_string(),
        };

        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
                // Redirects are never expected here; refusing them stops a
                // compromised/misconfigured verify URL from bouncing the secret
                // somewhere else.
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_default(),
            verify_url,
        }
    }

    pub fn verify_url(&self) -> &str {
        &self.verify_url
    }

    /// Whether the captcha is enforced right now.
    ///
    /// Requires the admin toggle *and* both keys: a site key without a secret
    /// would render a widget the server can never validate (locking everyone
    /// out), and a secret without a site key would break logins for no reason.
    pub async fn is_enabled(&self, pool: &PgPool) -> AppResult<bool> {
        if !AdminSettingRepository::get_bool(pool, "captcha_enabled").await? {
            return Ok(false);
        }
        Ok(self.keys(pool).await?.is_complete())
    }

    /// The public site key, safe to embed in the login page. Empty when the
    /// captcha is not fully configured, so a half-configured instance never
    /// renders a widget it cannot validate.
    pub async fn site_key(&self, pool: &PgPool) -> AppResult<String> {
        if !AdminSettingRepository::get_bool(pool, "captcha_enabled").await? {
            return Ok(String::new());
        }
        let keys = self.keys(pool).await?;
        Ok(if keys.is_complete() { keys.site_key } else { String::new() })
    }

    async fn keys(&self, pool: &PgPool) -> AppResult<AdminCaptchaKeys> {
        let keys = AdminSettingRepository::get_captcha_keys(pool).await?;
        Ok(AdminCaptchaKeys::new(keys.site_key, keys.secret_key))
    }

    /// Enforce the captcha for an unauthenticated request.
    ///
    /// No-op when the captcha is disabled. Otherwise the token must be present
    /// and must verify against Cloudflare. `remote_ip` is optional and only
    /// used as an additional signal; it is omitted rather than guessed when
    /// unknown.
    pub async fn enforce(
        &self,
        pool: &PgPool,
        token: Option<&str>,
        remote_ip: Option<IpAddr>,
    ) -> AppResult<()> {
        if !AdminSettingRepository::get_bool(pool, "captcha_enabled").await? {
            return Ok(());
        }
        let keys = self.keys(pool).await?;
        self.enforce_with(&keys, token, remote_ip).await
    }

    /// The actual verification, with the keys already resolved.
    ///
    /// Split out so the pre-flight rules (token present, size cap, key
    /// completeness) are unit-testable without a database or a network.
    async fn enforce_with(
        &self,
        keys: &AdminCaptchaKeys,
        token: Option<&str>,
        remote_ip: Option<IpAddr>,
    ) -> AppResult<()> {
        if !keys.is_complete() {
            // The admin enabled the toggle but has not finished entering keys.
            // Treat it as off rather than locking every user out of login.
            return Ok(());
        }

        let token = token.unwrap_or("").trim();
        if token.is_empty() {
            return Err(AppError::Validation(
                "please complete the verification challenge".into(),
            ));
        }
        if token.len() > MAX_TOKEN_LEN {
            return Err(AppError::Validation("verification token is invalid".into()));
        }

        let mut form = vec![("secret", keys.secret_key.as_str()), ("response", token)];
        let remoteip = remote_ip.map(|ip| ip.to_string());
        if let Some(remoteip) = remoteip.as_deref() {
            form.push(("remoteip", remoteip));
        }

        let response = self
            .client
            .post(&self.verify_url)
            .form(&form)
            .send()
            .await
            .map_err(|e| {
                // Fail closed, but say why in the logs: this is the difference
                // between "Cloudflare rejected the token" and "we could not
                // reach Cloudflare", and only the latter is an outage.
                tracing::error!(error = %e, "captcha verification request failed");
                AppError::Validation(
                    "verification is temporarily unavailable, please try again".into(),
                )
            })?;

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        Self::interpret(status, &body)
    }

    /// Turn a `siteverify` response into a decision.
    ///
    /// Split out from [`CaptchaService::enforce`] so the decision table can be
    /// unit-tested without any network access.
    fn interpret(status: reqwest::StatusCode, body: &str) -> AppResult<()> {
        if !status.is_success() {
            tracing::error!(%status, "captcha verification endpoint returned an error status");
            return Err(AppError::Validation(
                "verification is temporarily unavailable, please try again".into(),
            ));
        }

        let parsed: VerifyResponse = serde_json::from_str(body).map_err(|e| {
            tracing::error!(error = %e, "could not parse the captcha verification response");
            AppError::Validation(
                "verification is temporarily unavailable, please try again".into(),
            )
        })?;

        if parsed.success {
            return Ok(());
        }

        tracing::info!(
            error_codes = ?parsed.error_codes,
            hostname = ?parsed.hostname,
            "captcha rejected"
        );
        Err(AppError::Validation(
            "verification failed, please try again".into(),
        ))
    }
}

/// Captcha keys as configured by an admin, trimmed.
#[derive(Debug, Clone, Default)]
struct AdminCaptchaKeys {
    site_key: String,
    secret_key: String,
}

impl AdminCaptchaKeys {
    /// Trim on the way in. The admin API trims too, but a key stored before a
    /// trim was introduced (or edited straight in the database) must not be able
    /// to turn a blank value into a "configured" one via surrounding spaces.
    fn new(site_key: String, secret_key: String) -> Self {
        Self {
            site_key: site_key.trim().to_string(),
            secret_key: secret_key.trim().to_string(),
        }
    }

    fn is_complete(&self) -> bool {
        !self.site_key.is_empty() && !self.secret_key.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    fn service() -> CaptchaService {
        CaptchaService::new(&CaptchaConfig::default())
    }

    fn keys(site: &str, secret: &str) -> AdminCaptchaKeys {
        AdminCaptchaKeys::new(site.to_string(), secret.to_string())
    }

    #[test]
    fn default_verify_url_is_used() {
        assert_eq!(service().verify_url(), DEFAULT_VERIFY_URL);
    }

    #[test]
    fn explicit_verify_url_is_used() {
        let service = CaptchaService::new(&CaptchaConfig {
            turnstile_verify_url: "http://127.0.0.1:9/siteverify".to_string(),
        });
        assert_eq!(service.verify_url(), "http://127.0.0.1:9/siteverify");
    }

    #[test]
    fn keys_are_trimmed_and_completeness_requires_both() {
        let trimmed = keys("  site-key  ", "  secret-key  ");
        assert_eq!(trimmed.site_key, "site-key");
        assert_eq!(trimmed.secret_key, "secret-key");
        assert!(trimmed.is_complete());
    }

    #[test]
    fn incomplete_keys_are_reported() {
        // A site key with no secret would render a widget nothing can validate,
        // and a secret with no site key would break every login for nothing.
        assert!(!keys("site", "").is_complete());
        assert!(!keys("", "secret").is_complete());
        assert!(!keys("", "").is_complete());
        assert!(!keys("  ", "  ").is_complete());
    }

    #[tokio::test]
    async fn incomplete_config_accepts_anything_without_network() {
        // Admin flipped the toggle but has not entered both keys yet: login must
        // keep working, and nothing may be sent anywhere.
        let service = service();
        assert!(service.enforce_with(&keys("site", ""), None, None).await.is_ok());
        assert!(service.enforce_with(&keys("", ""), None, None).await.is_ok());
    }

    #[tokio::test]
    async fn complete_config_requires_a_token_without_network() {
        // Unreachable verifier: if the missing token were forwarded, the
        // request would fail as a network error ("temporarily unavailable")
        // instead of the missing-challenge 422 asserted below.
        let service = CaptchaService::new(&CaptchaConfig {
            turnstile_verify_url: "http://127.0.0.1:9/siteverify".to_string(),
        });

        for token in [None, Some(""), Some("   ")] {
            let err = service
                .enforce_with(&keys("site", "secret"), token, None)
                .await
                .unwrap_err();
            assert_eq!(err.status_code(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);
            assert!(err.to_string().contains("complete the verification challenge"));
        }
    }

    #[tokio::test]
    async fn oversized_tokens_are_rejected_without_network() {
        let huge = "x".repeat(MAX_TOKEN_LEN + 1);
        let err = service()
            .enforce_with(&keys("site", "secret"), Some(&huge), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("token is invalid"));
    }

    #[tokio::test]
    async fn verification_fails_closed_when_the_verifier_is_unreachable() {
        let service = CaptchaService::new(&CaptchaConfig {
            turnstile_verify_url: "http://127.0.0.1:9/siteverify".to_string(),
        });
        let err = service
            .enforce_with(&keys("site", "secret"), Some("token"), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("temporarily unavailable"));
    }

    #[test]
    fn interpret_accepts_a_successful_response() {
        let body = r#"{"success":true,"hostname":"files.example.com"}"#;
        assert!(CaptchaService::interpret(StatusCode::OK, body).is_ok());
    }

    #[test]
    fn interpret_rejects_a_failed_response() {
        let body = r#"{"success":false,"error-codes":["invalid-input-response"]}"#;
        let err = CaptchaService::interpret(StatusCode::OK, body).unwrap_err();
        assert!(err.to_string().contains("verification failed"));
    }

    #[test]
    fn interpret_rejects_expired_and_spent_tokens() {
        for code in ["timeout-or-duplicate", "invalid-input-response"] {
            let body = format!(r#"{{"success":false,"error-codes":["{code}"]}}"#);
            assert!(CaptchaService::interpret(StatusCode::OK, &body).is_err());
        }
    }

    #[test]
    fn interpret_rejects_an_error_status() {
        let err = CaptchaService::interpret(
            StatusCode::INTERNAL_SERVER_ERROR,
            r#"{"success":true}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("temporarily unavailable"));
    }

    #[test]
    fn interpret_rejects_unparsable_bodies() {
        for body in ["", "not json", "<html>502 Bad Gateway</html>"] {
            assert!(CaptchaService::interpret(StatusCode::OK, body).is_err());
        }
    }

    #[test]
    fn interpret_rejects_a_body_without_success_field() {
        // A truncated/empty JSON object must not be read as a pass.
        assert!(CaptchaService::interpret(StatusCode::OK, "{}").is_err());
    }
}