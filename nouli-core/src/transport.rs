//! Cancellable single-attempt transport. The evaluator owns all retry loops.

pub mod retry;

use std::collections::BTreeMap;
use std::{fmt, time::Duration};

use reqwest::{
    Client,
    header::{HeaderMap, HeaderName, HeaderValue, RETRY_AFTER},
};
use serde_json::Value;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    codec::{self, InputFormat},
    config::{ExecutionSettings, ProviderProfile, resolve::Credential},
};
use retry::RetryAfter;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TransportError {
    #[error("invalid transport settings")]
    Settings,
    #[error("invalid provider endpoint")]
    Endpoint,
    #[error("cannot construct HTTP transport")]
    Build,
    #[error("provider attempt canceled")]
    Canceled,
    #[error("provider attempt timed out")]
    Timeout,
    #[error("provider connection failed")]
    Connect,
    #[error("provider response body failed")]
    Body,
    #[error("provider response was truncated")]
    Truncated,
    #[error("provider response exceeds {limit} bytes")]
    Oversize { limit: usize },
    #[error("provider authentication failed (HTTP {status})")]
    Authentication { status: u16 },
    #[error("provider rate limited the attempt")]
    RateLimited { retry_after: Option<RetryAfter> },
    #[error("provider context capacity exceeded")]
    Capacity,
    #[error("provider capability is unsupported")]
    Capability,
    #[error("provider contract validation failed")]
    Contract,
    #[error("provider returned HTTP {status}")]
    Status {
        status: u16,
        retry_after: Option<RetryAfter>,
    },
}

/// Contents are intentionally omitted from Debug and all error diagnostics.
pub struct HttpResponse {
    pub status: u16,
    pub headers: HeaderMap,
    body: Vec<u8>,
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("body_bytes", &self.body.len())
            .finish_non_exhaustive()
    }
}

impl HttpResponse {
    /// Decodes the response body as JSON.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Contract`] if the body is not valid JSON.
    pub fn json(&self) -> Result<Value, TransportError> {
        codec::decode_value(&self.body, InputFormat::Json).map_err(|_| TransportError::Contract)
    }
}

pub struct HttpTransport {
    client: Client,
    timeout: Duration,
    max_response_bytes: usize,
}

/// Appends protocol path segments without dropping a reverse proxy's prefix.
///
/// # Errors
///
/// Returns [`TransportError::Endpoint`] if the profile is invalid, its base URL
/// cannot carry path segments, or the request path or a Cloudflare model ID
/// contains an empty, `.`, or `..` segment (or a `?`, `#`, or `\` in the
/// request path).
pub fn endpoint(profile: &ProviderProfile, path: &str) -> Result<Url, TransportError> {
    profile.validate().map_err(|_| TransportError::Endpoint)?;
    let mut url = Url::parse(&profile.base_url).map_err(|_| TransportError::Endpoint)?;
    let path = profile.connection.request_path.as_deref().unwrap_or(path);
    let path = path.trim_matches('/');
    if path.is_empty()
        || path.split('/').any(|part| {
            part.is_empty() || part == "." || part == ".." || part.contains(['?', '#', '\\'])
        })
    {
        return Err(TransportError::Endpoint);
    }
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|()| TransportError::Endpoint)?;
        segments.pop_if_empty();
        for part in path.split('/').filter(|_| {
            profile.connection.binding != Some(crate::config::HttpBinding::CloudflareWorkersAi)
        }) {
            segments.push(part);
        }
    }
    if profile.connection.binding == Some(crate::config::HttpBinding::CloudflareWorkersAi) {
        let mut parts = url
            .path_segments_mut()
            .map_err(|()| TransportError::Endpoint)?;
        // Cloudflare model IDs use slash-separated path components.
        parts.pop_if_empty();
        for segment in profile
            .connection
            .request_path
            .as_deref()
            .unwrap_or("run")
            .split('/')
        {
            parts.push(segment);
        }
        for part in profile.model.split('/') {
            if part.is_empty() || part == "." || part == ".." {
                return Err(TransportError::Endpoint);
            }
            parts.push(part);
        }
    }
    Ok(url)
}

fn network_error(error: &reqwest::Error) -> TransportError {
    if error.is_timeout() {
        TransportError::Timeout
    } else if error.is_connect() {
        TransportError::Connect
    } else {
        TransportError::Body
    }
}

fn status_error(status: u16, retry_after: Option<RetryAfter>, body: &[u8]) -> TransportError {
    if matches!(status, 401 | 403) {
        return TransportError::Authentication { status };
    }
    if status == 429 {
        return TransportError::RateLimited { retry_after };
    }
    if status == 413 {
        return TransportError::Capacity;
    }
    // Only recognized native codes affect classification. Never expose server
    // messages, arbitrary error bodies, or secrets in normalized diagnostics.
    let code = codec::decode_value(body, InputFormat::Json)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/code")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
    match code.as_deref() {
        Some("context_length_exceeded" | "context_too_large" | "context_limit_exceeded") => {
            TransportError::Capacity
        }
        Some("unsupported_model" | "unsupported_capability" | "unsupported_question_type") => {
            TransportError::Capability
        }
        _ if matches!(status, 400 | 422) => TransportError::Contract,
        _ => TransportError::Status {
            status,
            retry_after,
        },
    }
}

impl HttpTransport {
    /// Builds a transport that enforces the attempt timeout and response size
    /// limit from `settings`, without retries, redirects, or proxies.
    ///
    /// # Errors
    ///
    /// - Returns [`TransportError::Settings`] if the settings are invalid or the
    ///   timeout overflows the clock.
    /// - Returns [`TransportError::Build`] if the HTTP client cannot be built.
    pub fn new(settings: &ExecutionSettings) -> Result<Self, TransportError> {
        settings.validate().map_err(|_| TransportError::Settings)?;
        let timeout = Duration::from_secs(settings.attempt_timeout_seconds);
        if std::time::Instant::now().checked_add(timeout).is_none() {
            return Err(TransportError::Settings);
        }
        let client = Client::builder()
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| TransportError::Build)?;
        Ok(Self {
            client,
            timeout,
            max_response_bytes: settings.max_response_bytes,
        })
    }

    /// Sends one cancellable JSON POST attempt to the profile's endpoint.
    ///
    /// # Errors
    ///
    /// - Returns [`TransportError::Endpoint`] if the endpoint cannot be built,
    ///   and [`TransportError::Settings`] if a header or credential is not a
    ///   valid HTTP header.
    /// - Returns [`TransportError::Canceled`] if `cancel` fires first, and
    ///   [`TransportError::Timeout`] if the attempt exceeds its timeout.
    /// - Returns [`TransportError::Connect`], [`TransportError::Body`],
    ///   [`TransportError::Truncated`], or [`TransportError::Oversize`] if the
    ///   exchange fails or the response body is incomplete or too large.
    /// - Returns a status classification such as
    ///   [`TransportError::Authentication`], [`TransportError::RateLimited`],
    ///   [`TransportError::Capacity`], [`TransportError::Capability`],
    ///   [`TransportError::Contract`], or [`TransportError::Status`] for a
    ///   non-2xx response.
    #[tracing::instrument(skip_all, name = "provider_attempt")]
    pub async fn post(
        &self,
        profile: &ProviderProfile,
        path: &str,
        body: &Value,
        credential: Option<&Credential>,
        cancel: &CancellationToken,
    ) -> Result<HttpResponse, TransportError> {
        self.post_with_headers(
            profile,
            path,
            body,
            credential,
            &BTreeMap::default(),
            cancel,
        )
        .await
    }

    /// Sends one cancellable JSON POST attempt with additional sensitive
    /// headers.
    ///
    /// # Errors
    ///
    /// - Returns [`TransportError::Endpoint`] if the endpoint cannot be built,
    ///   and [`TransportError::Settings`] if a header or credential is not a
    ///   valid HTTP header.
    /// - Returns [`TransportError::Canceled`] if `cancel` fires first, and
    ///   [`TransportError::Timeout`] if the attempt exceeds its timeout.
    /// - Returns [`TransportError::Connect`], [`TransportError::Body`],
    ///   [`TransportError::Truncated`], or [`TransportError::Oversize`] if the
    ///   exchange fails or the response body is incomplete or too large.
    /// - Returns a status classification such as
    ///   [`TransportError::Authentication`], [`TransportError::RateLimited`],
    ///   [`TransportError::Capacity`], [`TransportError::Capability`],
    ///   [`TransportError::Contract`], or [`TransportError::Status`] for a
    ///   non-2xx response.
    #[allow(clippy::too_many_arguments)]
    pub async fn post_with_headers(
        &self,
        profile: &ProviderProfile,
        path: &str,
        body: &Value,
        credential: Option<&Credential>,
        headers: &std::collections::BTreeMap<String, Credential>,
        cancel: &CancellationToken,
    ) -> Result<HttpResponse, TransportError> {
        let url = endpoint(profile, path)?;
        let exchange = self.exchange(url, body, credential, profile, headers);
        tokio::select! {
            biased;
            ()=cancel.cancelled()=>Err(TransportError::Canceled),
            result=tokio::time::timeout(self.timeout,exchange)=>result.unwrap_or(Err(TransportError::Timeout)),
        }
    }

    async fn exchange(
        &self,
        url: Url,
        body: &Value,
        credential: Option<&Credential>,
        profile: &ProviderProfile,
        headers: &std::collections::BTreeMap<String, Credential>,
    ) -> Result<HttpResponse, TransportError> {
        let mut response = self
            .client
            .post(url)
            .json(body)
            .headers(request_headers(profile, headers, credential)?)
            .send()
            .await
            .map_err(|error| network_error(&error))?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let bytes = self.read_body(&mut response).await?;
        tracing::debug!(
            status,
            body_bytes = bytes.len(),
            "provider attempt received"
        );
        if !(200..300).contains(&status) {
            let retry_after = headers
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(RetryAfter::parse);
            return Err(status_error(status, retry_after, &bytes));
        }
        Ok(HttpResponse {
            status,
            headers,
            body: bytes,
        })
    }

    /// Reads the whole response body, rejecting it as soon as it exceeds the
    /// size limit, and as truncated when it falls short of a declared length.
    async fn read_body(&self, response: &mut reqwest::Response) -> Result<Vec<u8>, TransportError> {
        let oversize = TransportError::Oversize {
            limit: self.max_response_bytes,
        };
        let expected = response.content_length();
        if expected.is_some_and(|length| length > self.max_response_bytes as u64) {
            return Err(oversize);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            if expected.is_some() && !error.is_timeout() {
                TransportError::Truncated
            } else {
                network_error(&error)
            }
        })? {
            if chunk.len() > self.max_response_bytes - bytes.len() {
                return Err(oversize);
            }
            bytes.extend_from_slice(&chunk);
        }
        if expected.is_some_and(|length| length != bytes.len() as u64) {
            return Err(TransportError::Truncated);
        }
        Ok(bytes)
    }
}

/// Builds the attempt's headers, all marked sensitive: the profile's literal
/// headers, then the resolved extra headers, then the credential, each later
/// one replacing an earlier header of the same name.
fn request_headers(
    profile: &ProviderProfile,
    headers: &std::collections::BTreeMap<String, Credential>,
    credential: Option<&Credential>,
) -> Result<HeaderMap, TransportError> {
    let mut resolved = HeaderMap::new();
    for (name, value) in &profile.connection.headers {
        let (name, value) = sensitive_header(name, value)?;
        resolved.insert(name, value);
    }
    for (name, value) in headers {
        let (name, value) = sensitive_header(name, value.expose())?;
        resolved.insert(name, value);
    }
    if let Some(credential) = credential {
        let (name, value) = authorization_header(profile, credential)?;
        resolved.insert(name, value);
    }
    Ok(resolved)
}

/// The credential header: `Authorization: Bearer <key>` unless the profile
/// names another header or scheme. An empty scheme sends the bare key.
fn authorization_header(
    profile: &ProviderProfile,
    credential: &Credential,
) -> Result<(HeaderName, HeaderValue), TransportError> {
    let scheme = profile
        .connection
        .auth_scheme
        .as_deref()
        .unwrap_or("Bearer");
    let separator = if profile.connection.auth_scheme.as_deref() == Some("") {
        ""
    } else {
        " "
    };
    let name = profile
        .connection
        .auth_header
        .as_deref()
        .unwrap_or("Authorization");
    sensitive_header(name, &format!("{scheme}{separator}{}", credential.expose()))
}

fn sensitive_header(name: &str, value: &str) -> Result<(HeaderName, HeaderValue), TransportError> {
    let mut value = HeaderValue::from_str(value).map_err(|_| TransportError::Settings)?;
    value.set_sensitive(true);
    let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| TransportError::Settings)?;
    Ok((name, value))
}

#[cfg(test)]
pub(crate) mod test_server;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConnectionSettings;
    use crate::config::ModelSettings;
    use test_server::MockServer;

    fn profile(base_url: String) -> ProviderProfile {
        ProviderProfile {
            connection: ConnectionSettings::default(),
            limits: ModelSettings::default(),
            protocol: crate::config::Protocol::SystemOne,
            base_url,
            model: "fixture".into(),
            api_key_env: None,
            stored_credential: false,
            context_budget_tokens: None,
            context_window_tokens: None,
            reserved_output_tokens: None,
            max_attempts: None,
            attempt_timeout_seconds: None,
        }
    }

    #[tokio::test]
    async fn cloudflare_binding_and_custom_headers_are_explicit_on_any_endpoint() {
        let mut server = MockServer::start().await;
        let mut profile = profile(format!("{}/accounts/demo/ai", server.url));
        profile.model = "@cf/cloudflare/clef".into();
        profile.connection.binding = Some(crate::config::HttpBinding::CloudflareWorkersAi);
        profile.connection.request_path = Some("custom/run".into());
        profile.connection.auth_header = Some("X-Api-Key".into());
        profile.connection.auth_scheme = Some(String::new());
        profile
            .connection
            .headers
            .insert("X-Tenant".into(), "literal".into());
        let headers = std::collections::BTreeMap::from([(
            "x-tenant".into(),
            Credential::new("resolved".into()).unwrap(),
        )]);
        let key = Credential::new("private-key".into()).unwrap();
        let transport = HttpTransport::new(&ExecutionSettings::default()).unwrap();
        let cancel = CancellationToken::new();
        let body = serde_json::json!({"model":"clef"});
        let (result, request) = tokio::join!(
            transport.post_with_headers(
                &profile,
                "v1/systemone",
                &body,
                Some(&key),
                &headers,
                &cancel
            ),
            async {
                let request = server.received().await;
                server.respond(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_vec(),
                );
                request
            }
        );
        result.unwrap();
        let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
        assert!(
            request.starts_with("post /accounts/demo/ai/custom/run/@cf/cloudflare/clef http/1.1")
        );
        assert_eq!(request.matches("x-tenant:").count(), 1);
        assert!(request.contains("x-tenant: resolved"));
        assert!(request.contains("x-api-key: private-key"));
        assert!(!request.contains("authorization:"));
        server.finish().await;
    }
    #[tokio::test]
    async fn actual_connection_failure_maps_to_a_retryable_connect_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let settings = ExecutionSettings::default();
        let transport = HttpTransport::new(&settings).unwrap();
        let result = transport
            .post(
                &profile(format!("http://{address}")),
                "v1/test",
                &Value::Null,
                None,
                &CancellationToken::new(),
            )
            .await;
        let error = result.unwrap_err();
        assert_eq!(error, TransportError::Connect);
        assert!(
            retry::RetryPolicy::new(&settings)
                .unwrap()
                .next_delay(1, &error, std::time::SystemTime::UNIX_EPOCH)
                .is_some()
        );
    }

    #[tokio::test]
    async fn prefix_authorization_and_single_attempt_are_preserved() {
        let mut server = MockServer::start().await;
        let profile = profile(format!("{}/proxy/api", server.url));
        let transport = HttpTransport::new(&ExecutionSettings::default()).unwrap();
        let credential = Credential::new("SECRET_SENTINEL".into()).unwrap();
        let cancel = CancellationToken::new();
        let body = serde_json::json!({"state":"BODY_SENTINEL"});
        let (response, received) = tokio::join!(
            transport.post(&profile, "/v1/systemone", &body, Some(&credential), &cancel),
            async {
                let request = server.received().await;
                server.respond(b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nRetry-After: 7\r\nConnection: close\r\n\r\n".to_vec());
                request
            }
        );
        let request = String::from_utf8(received).unwrap();
        assert!(request.starts_with("POST /proxy/api/v1/systemone HTTP/1.1"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer secret_sentinel")
        );
        let error = response.unwrap_err();
        assert_eq!(
            error,
            TransportError::Status {
                status: 503,
                retry_after: Some(RetryAfter::Delay(Duration::from_secs(7)))
            }
        );
        assert!(!format!("{error:?} {error}").contains("SENTINEL"));
        server.finish().await;
    }

    #[tokio::test]
    async fn bounds_declared_and_streamed_responses_and_distinguishes_truncation() {
        for (wire,expected) in [
            (b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n".as_slice(),TransportError::Oversize {limit:4}),
            (b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n6\r\nabcdef\r\n0\r\n\r\n".as_slice(),TransportError::Oversize {limit:4}),
            (b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nx".as_slice(),TransportError::Truncated),
        ] {
            let mut server=MockServer::start().await;
            let transport=HttpTransport::new(&ExecutionSettings {max_response_bytes:4,..Default::default()}).unwrap();
            server.respond(wire.to_vec());
            let response=transport.post(&profile(server.url.clone()),"v1/test",&Value::Null,None,&CancellationToken::new()).await;
            assert_eq!(response.unwrap_err(),expected);
            server.finish().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_and_cancellation_stop_in_flight_waits() {
        for canceled in [false, true] {
            let mut server = MockServer::start().await;
            let transport = HttpTransport::new(&ExecutionSettings::default()).unwrap();
            let cancel = CancellationToken::new();
            let profile = profile(server.url.clone());
            let (result, ()) = tokio::join!(
                transport.post(&profile, "v1/test", &Value::Null, None, &cancel),
                async {
                    server.received().await;
                    if canceled {
                        cancel.cancel();
                    } else {
                        tokio::time::advance(Duration::from_secs(121)).await;
                    }
                }
            );
            assert_eq!(
                result.unwrap_err(),
                if canceled {
                    TransportError::Canceled
                } else {
                    TransportError::Timeout
                }
            );
            server.finish().await;
        }
    }

    #[tokio::test]
    async fn malformed_json_statuses_and_response_debug_do_not_expose_contents() {
        for (status, body, expected) in [
            (
                401,
                "SECRET_SENTINEL",
                Some(TransportError::Authentication { status: 401 }),
            ),
            (413, "SECRET_SENTINEL", Some(TransportError::Capacity)),
            (
                400,
                r#"{"error":{"code":"context_length_exceeded","message":"SECRET_SENTINEL"}}"#,
                Some(TransportError::Capacity),
            ),
            (
                400,
                r#"{"error":{"code":"unsupported_model"}}"#,
                Some(TransportError::Capability),
            ),
            (422, "SECRET_SENTINEL", Some(TransportError::Contract)),
            (
                429,
                "SECRET_SENTINEL",
                Some(TransportError::RateLimited { retry_after: None }),
            ),
            (200, "SECRET_SENTINEL", None),
        ] {
            let mut server = MockServer::start().await;
            server.respond(format!("HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).into_bytes());
            let transport = HttpTransport::new(&ExecutionSettings::default()).unwrap();
            let result = transport
                .post(
                    &profile(server.url.clone()),
                    "v1/test",
                    &Value::Null,
                    None,
                    &CancellationToken::new(),
                )
                .await;
            assert!(!format!("{result:?}").contains("SECRET_SENTINEL"));
            if let Some(expected) = expected {
                assert_eq!(result.unwrap_err(), expected);
            } else {
                assert_eq!(result.unwrap().json(), Err(TransportError::Contract));
            }
            server.finish().await;
        }
    }

    #[test]
    fn unsafe_endpoint_paths_and_unbounded_timers_fail_before_network() {
        let profile = profile("https://example.org/prefix/".into());
        assert_eq!(
            endpoint(&profile, "/alpha/decisions").unwrap().as_str(),
            "https://example.org/prefix/alpha/decisions"
        );
        for path in ["", "../secret", "https://other.example", "a//b", "a?key=x"] {
            assert_eq!(endpoint(&profile, path), Err(TransportError::Endpoint));
        }
        assert!(matches!(
            HttpTransport::new(&ExecutionSettings {
                attempt_timeout_seconds: u64::MAX,
                ..Default::default()
            }),
            Err(TransportError::Settings)
        ));
    }
}
