//! Host Copilot OAuth authorization and account discovery.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use http::{HeaderValue, Uri};
use tokio::sync::Mutex;

pub const PLACEHOLDER: &str = "copilot-aegis-placeholder";

#[derive(Debug, thiserror::Error)]
pub enum CopilotError {
    #[error("Copilot returned an invalid token response")]
    InvalidResponse,
    #[error("Copilot token request failed; check connectivity and try again")]
    Transport,
    #[error("Copilot token request returned HTTP {0}; verify that the selected GitHub token supports Copilot and the account has Copilot access")]
    Status(u16),
    #[error("GitHub device authorization was denied or expired; run host Copilot login again")]
    DeviceAuthorization,
    #[error("the GitHub account does not have Copilot chat access")]
    Entitlement,
    #[error("Copilot OAuth token is missing; run host Copilot login")]
    MissingOauth,
}

#[derive(Clone)]
pub struct CopilotSession {
    pub token: String,
    pub endpoint: String,
}

impl std::fmt::Debug for CopilotSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CopilotSession")
            .field("token", &"[REDACTED]")
            .field("endpoint", &self.endpoint)
            .finish()
    }
}

impl CopilotSession {
    pub fn account(token: &str, value: serde_json::Value) -> Result<Self, CopilotError> {
        if value.get("chat_enabled").and_then(|value| value.as_bool()) != Some(true) {
            return Err(CopilotError::Entitlement);
        }
        if token.is_empty() {
            return Err(CopilotError::InvalidResponse);
        }
        HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| CopilotError::InvalidResponse)?;
        let endpoint = match value.pointer("/endpoints/api") {
            None => "https://api.githubcopilot.com",
            Some(value) => value.as_str().ok_or(CopilotError::InvalidResponse)?,
        };
        let endpoint = endpoint.trim_end_matches('/');
        let uri: Uri = endpoint
            .parse()
            .map_err(|_| CopilotError::InvalidResponse)?;
        let authority = uri.authority().ok_or(CopilotError::InvalidResponse)?;
        let host = uri.host().ok_or(CopilotError::InvalidResponse)?;
        if uri.scheme_str() != Some("https")
            || !is_copilot_host(host)
            || authority.as_str().contains('@')
            || uri.port_u16().is_some_and(|port| port != 443)
            || uri.path() != "/"
            || uri.query().is_some()
        {
            return Err(CopilotError::InvalidResponse);
        }
        Ok(Self {
            token: token.to_string(),
            endpoint: endpoint.to_string(),
        })
    }
}

pub fn poll_response(
    value: serde_json::Value,
    interval: &mut u64,
) -> Result<Option<String>, CopilotError> {
    if let Some(token) = value
        .get("access_token")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
    {
        return Ok(Some(token.to_string()));
    }
    match value.get("error").and_then(|value| value.as_str()) {
        Some("authorization_pending") => Ok(None),
        Some("slow_down") => {
            *interval = value
                .get("interval")
                .and_then(|value| value.as_u64())
                .filter(|value| *value > *interval)
                .unwrap_or(interval.saturating_add(5));
            Ok(None)
        }
        _ => Err(CopilotError::DeviceAuthorization),
    }
}

pub async fn device_login() -> Result<String, CopilotError> {
    const CLIENT_ID: &str = "Ov23li8tweQw6odWQebz";
    let device = crate::proxy::host_json(
        "POST",
        "https://github.com/login/device/code",
        None,
        Some(serde_json::json!({"client_id":CLIENT_ID,"scope":"read:user"})),
    )
    .await?;
    let code = device
        .get("device_code")
        .and_then(|value| value.as_str())
        .ok_or(CopilotError::InvalidResponse)?;
    let user_code = device
        .get("user_code")
        .and_then(|value| value.as_str())
        .ok_or(CopilotError::InvalidResponse)?;
    let url = device
        .get("verification_uri")
        .and_then(|value| value.as_str())
        .filter(|url| *url == "https://github.com/login/device")
        .ok_or(CopilotError::InvalidResponse)?;
    let lifetime = device
        .get("expires_in")
        .and_then(|value| value.as_u64())
        .filter(|value| *value > 0)
        .ok_or(CopilotError::InvalidResponse)?;
    let mut interval = device
        .get("interval")
        .and_then(|value| value.as_u64())
        .unwrap_or(5)
        .max(1);
    eprintln!("Authorize GitHub Copilot at {url}\nEnter code: {user_code}");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(lifetime);
    loop {
        let response = tokio::time::timeout_at(deadline, async {
            tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
            crate::proxy::host_json(
                "POST",
                "https://github.com/login/oauth/access_token",
                None,
                Some(serde_json::json!({"client_id":CLIENT_ID,"device_code":code,
                    "grant_type":"urn:ietf:params:oauth:grant-type:device_code"})),
            )
            .await
        })
        .await
        .map_err(|_| CopilotError::DeviceAuthorization)??;
        if let Some(token) = poll_response(response, &mut interval)? {
            discover_account(&token).await?;
            return Ok(token);
        }
    }
}

pub async fn discover_account(token: &str) -> Result<CopilotSession, CopilotError> {
    let account = crate::proxy::host_json(
        "GET",
        "https://api.github.com/copilot_internal/user",
        Some(token),
        None,
    )
    .await?;
    CopilotSession::account(token, account)
}

pub fn is_copilot_host(host: &str) -> bool {
    host == "api.githubcopilot.com" || host.ends_with(".githubcopilot.com")
}

pub type Acquisition = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<CopilotSession, CopilotError>> + Send>>
        + Send
        + Sync,
>;

pub struct CopilotTokens {
    acquire: Acquisition,
    cached: Mutex<Option<CopilotSession>>,
}

impl CopilotTokens {
    pub fn new(acquire: Acquisition) -> Self {
        Self {
            acquire,
            cached: Mutex::new(None),
        }
    }

    pub async fn session(&self) -> Result<CopilotSession, CopilotError> {
        let mut cached = self.cached.lock().await;
        if let Some(session) = cached.as_ref() {
            return Ok(session.clone());
        }
        let session = tokio::time::timeout(std::time::Duration::from_secs(10), (self.acquire)())
            .await
            .map_err(|_| CopilotError::Transport)??;
        *cached = Some(session.clone());
        Ok(session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn oauth_account_discovery_checks_entitlement_and_endpoint() {
        let session = CopilotSession::account(
            "oauth-sentinel",
            serde_json::json!({
                "chat_enabled":true,"endpoints":{"api":"https://api.enterprise.githubcopilot.com"}
            }),
        )
        .unwrap();
        assert_eq!(session.token, "oauth-sentinel");
        assert_eq!(session.endpoint, "https://api.enterprise.githubcopilot.com");
        assert!(!format!("{session:?}").contains("oauth-sentinel"));
        for response in [
            serde_json::json!({"chat_enabled":false}),
            serde_json::json!({}),
            serde_json::json!({"chat_enabled":true,"endpoints":{"api":"https://evil.example"}}),
        ] {
            assert!(CopilotSession::account("oauth-sentinel", response).is_err());
        }
    }

    #[test]
    fn device_polling_handles_pending_backoff_and_denial() {
        let mut interval = 5;
        assert!(poll_response(
            serde_json::json!({"error":"authorization_pending"}),
            &mut interval
        )
        .unwrap()
        .is_none());
        assert!(
            poll_response(serde_json::json!({"error":"slow_down"}), &mut interval)
                .unwrap()
                .is_none()
        );
        assert_eq!(interval, 10);
        assert_eq!(
            poll_response(
                serde_json::json!({"access_token":"oauth-sentinel"}),
                &mut interval
            )
            .unwrap()
            .as_deref(),
            Some("oauth-sentinel")
        );
        for error in ["access_denied", "expired_token", "incorrect_device_code"] {
            assert!(poll_response(serde_json::json!({"error":error}), &mut interval).is_err());
        }
    }

    #[tokio::test]
    async fn lazy_cache_coordinates_account_discovery() {
        let requests = Arc::new(AtomicUsize::new(0));
        let tokens = Arc::new(CopilotTokens::new({
            let requests = requests.clone();
            Arc::new(move || {
                let number = requests.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    tokio::task::yield_now().await;
                    CopilotSession::account(
                        &format!("token-{number}"),
                        serde_json::json!({
                            "chat_enabled":true,"endpoints":{"api":"https://api.business.githubcopilot.com"}
                        }),
                    )
                })
            })
        }));
        assert_eq!(requests.load(Ordering::SeqCst), 0);
        let (first, second) = tokio::join!(tokens.session(), tokens.session());
        assert_eq!(first.unwrap().token, "token-0");
        assert_eq!(second.unwrap().token, "token-0");
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert_eq!(tokens.session().await.unwrap().token, "token-0");
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn acquisition_validates_tokens_and_endpoints_without_disclosure() {
        for endpoint in [
            "http://api.githubcopilot.com",
            "https://evil.example",
            "https://user@api.githubcopilot.com",
            "https://api.githubcopilot.com:444",
            "https://api.githubcopilot.com/path",
        ] {
            let response = serde_json::json!({"chat_enabled":true, "endpoints":{"api":endpoint}});
            let error = CopilotSession::account("secret-sentinel", response).unwrap_err();
            assert!(!format!("{error:?}").contains("secret-sentinel"));
        }
        for token in ["", "secret\r\nsentinel"] {
            assert!(
                CopilotSession::account(token, serde_json::json!({"chat_enabled":true})).is_err()
            );
        }
        let session =
            CopilotSession::account("secret-sentinel", serde_json::json!({"chat_enabled":true}))
                .unwrap();
        assert_eq!(session.endpoint, "https://api.githubcopilot.com");
        assert!(!format!("{session:?}").contains("secret-sentinel"));
    }

    #[tokio::test]
    async fn failed_discovery_is_retried() {
        let requests = Arc::new(AtomicUsize::new(0));
        let tokens = CopilotTokens::new({
            let requests = requests.clone();
            Arc::new(move || {
                let number = requests.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    if number == 0 {
                        return Err(CopilotError::InvalidResponse);
                    }
                    CopilotSession::account("recovered", serde_json::json!({"chat_enabled":true}))
                })
            })
        });
        assert!(tokens.session().await.is_err());
        assert_eq!(tokens.session().await.unwrap().token, "recovered");
    }
}
