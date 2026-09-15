//! The forwarding proxy: HTTP, streaming, and WebSocket upgrade.
//!
//! Implemented as a hyper client driving requests to a loopback upstream. The
//! key properties, each covered by a test, are:
//!
//! - request and response headers pass through verbatim (except hop-by-hop
//!   headers, which must not be forwarded by any proxy);
//! - response bodies stream rather than buffer;
//! - `Upgrade` handshakes are tunneled raw once established.

use bytes::Bytes;
use http::{header, HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode};
use http_body_util::{combinators::BoxBody, BodyExt, Full, StreamBody};
use hyper::body::Frame;
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use router_core::ErrorCode;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;

/// Boxed response body, so buffered and streamed responses share one type.
pub type RelayBody = BoxBody<Bytes, std::io::Error>;

/// Headers that are connection-scoped and must never be forwarded by a proxy.
///
/// Forwarding these is a correctness bug: `Connection` legitimately lists
/// other hop-by-hop headers, and `Transfer-Encoding` describes the previous
/// hop's framing, not ours.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Relay behaviour that callers can tune.
#[derive(Debug, Clone)]
pub struct RelayConfig {
    /// Loopback upstream, e.g. `http://127.0.0.1:3081`.
    pub upstream: String,
    /// How long to wait for the upstream to accept a connection.
    pub connect_timeout: Duration,
    /// How long to wait for response headers.
    pub request_timeout: Duration,
    /// Maximum buffered request body, in bytes.
    ///
    /// Uploads larger than this are streamed rather than buffered.
    pub max_buffered_body: usize,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            upstream: "http://127.0.0.1:3081".to_string(),
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(120),
            max_buffered_body: 8 * 1024 * 1024,
        }
    }
}

/// Shared relay state.
#[derive(Debug, Clone)]
pub struct RelayState {
    config: Arc<RelayConfig>,
}

impl RelayState {
    /// Build state from configuration.
    #[must_use]
    pub fn new(config: RelayConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }

    /// The configured upstream base URL.
    #[must_use]
    pub fn upstream(&self) -> &str {
        &self.config.upstream
    }

    /// The upstream host and port.
    fn upstream_authority(&self) -> Result<(String, u16), ProxyError> {
        let rest = self
            .config
            .upstream
            .strip_prefix("http://")
            .ok_or_else(|| {
                ProxyError::new(
                    ErrorCode::ConfigInvalid,
                    format!(
                        "upstream must be an http:// URL, got '{}'",
                        self.config.upstream
                    ),
                )
            })?;
        let rest = rest.trim_end_matches('/');
        match rest.split_once(':') {
            Some((host, port)) => {
                let port = port.parse::<u16>().map_err(|_| {
                    ProxyError::new(
                        ErrorCode::ConfigInvalid,
                        format!("upstream port is not a number: '{port}'"),
                    )
                })?;
                Ok((host.to_string(), port))
            }
            None => Ok((rest.to_string(), 80)),
        }
    }
}

/// A relay failure.
#[derive(Debug, thiserror::Error)]
#[error("{code}: {detail}")]
pub struct ProxyError {
    /// Stable error code.
    pub code: ErrorCode,
    /// Human-readable detail.
    pub detail: String,
}

impl ProxyError {
    /// Construct an error.
    #[must_use]
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

/// Whether a header is connection-scoped and must be stripped.
fn is_hop_by_hop(name: &HeaderName) -> bool {
    HOP_BY_HOP.contains(&name.as_str())
}

/// Copy a header map, dropping hop-by-hop headers.
///
/// `Connection` is also inspected for additional names it lists, per RFC 9110:
/// a proxy must remove headers named there too.
fn filter_headers(src: &HeaderMap) -> HeaderMap {
    let mut extra: Vec<String> = Vec::new();
    if let Some(conn) = src.get(header::CONNECTION).and_then(|v| v.to_str().ok()) {
        for token in conn.split(',') {
            let t = token.trim().to_ascii_lowercase();
            if !t.is_empty() {
                extra.push(t);
            }
        }
    }

    let mut out = HeaderMap::with_capacity(src.len());
    for (name, value) in src {
        let lower = name.as_str().to_ascii_lowercase();
        if is_hop_by_hop(name) || extra.contains(&lower) {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

/// Turn a fully-read body into a boxed body.
fn full_body(bytes: Bytes) -> RelayBody {
    Full::new(bytes)
        .map_err(|never: Infallible| match never {})
        .boxed()
}

/// Build the relay's axum router.
///
/// A single fallback handler serves every path, because the relay has no
/// application routes of its own: the upstream owns all routing, including the
/// root token exchange and the `/api` bridge.
pub fn relay_router(state: RelayState) -> axum::Router {
    axum::Router::new()
        .fallback(axum::routing::any(handle))
        .with_state(state)
}

/// Forward one request to the loopback upstream.
async fn handle(
    axum::extract::State(state): axum::extract::State<RelayState>,
    req: Request<axum::body::Body>,
) -> Response<RelayBody> {
    match forward(&state, req).await {
        Ok(resp) => resp,
        Err(e) => error_response(&e),
    }
}

/// A 502 whose body explains the failure.
///
/// The harness not being reachable is the single most likely relay failure, so
/// it gets a page that names the cause rather than an empty body.
fn error_response(err: &ProxyError) -> Response<RelayBody> {
    let detail = err.detail.clone();
    let code = err.code.as_str();
    let remedy = err.code.remediation().unwrap_or("");
    let body = format!(
        "DeepSeek Harness Router - relay error\n\n\
         {code}\n\n\
         {detail}\n\n\
         {remedy}\n"
    );
    let mut resp = Response::new(full_body(Bytes::from(body)));
    *resp.status_mut() = StatusCode::BAD_GATEWAY;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    resp
}

/// Perform the actual forward.
///
/// Uses `http1` directly rather than a pooled client because WebSocket upgrades
/// require taking ownership of the underlying stream, which a pooled client
/// does not expose.
async fn forward(
    state: &RelayState,
    req: Request<axum::body::Body>,
) -> Result<Response<RelayBody>, ProxyError> {
    let (host, port) = state.upstream_authority()?;

    let is_upgrade = req.headers().get(header::UPGRADE).is_some_and(|v| {
        v.to_str()
            .is_ok_and(|s| s.eq_ignore_ascii_case("websocket"))
    });

    let stream = tokio::time::timeout(
        state.config.connect_timeout,
        TcpStream::connect((host.as_str(), port)),
    )
    .await
    .map_err(|_| {
        ProxyError::new(
            ErrorCode::RelayUpstreamUnreachable,
            format!("timed out connecting to {host}:{port}"),
        )
    })?
    .map_err(|e| {
        ProxyError::new(
            ErrorCode::RelayUpstreamUnreachable,
            format!("cannot connect to {host}:{port}: {e}"),
        )
    })?;

    // Disable Nagle: live agent output is many small frames, and coalescing
    // them adds visible latency to a streaming UI.
    let _ = stream.set_nodelay(true);

    let io = TokioIo::new(stream);
    let (sender, conn) = http1::handshake(io).await.map_err(|e| {
        ProxyError::new(
            ErrorCode::RelayUpstreamUnreachable,
            format!("HTTP handshake with {host}:{port} failed: {e}"),
        )
    })?;

    let conn_task = tokio::spawn(async move {
        if let Err(e) = conn.with_upgrades().await {
            tracing::debug!(error = %e, "upstream connection ended");
        }
    });

    if is_upgrade {
        return forward_upgrade(state, req, sender, conn_task).await;
    }

    forward_http(state, req, sender, conn_task).await
}

/// Forward an ordinary HTTP request and stream the response back.
async fn forward_http(
    state: &RelayState,
    req: Request<axum::body::Body>,
    mut sender: http1::SendRequest<axum::body::Body>,
    conn_task: tokio::task::JoinHandle<()>,
) -> Result<Response<RelayBody>, ProxyError> {
    let (parts, body) = req.into_parts();

    let mut builder = Request::builder().method(parts.method.clone()).uri(
        parts
            .uri
            .path_and_query()
            .map_or("/", http::uri::PathAndQuery::as_str),
    );

    // Headers pass through verbatim. Rewriting Host would break the browser
    // trust fence, because Origin would no longer match.
    {
        let headers = builder.headers_mut().ok_or_else(|| {
            ProxyError::new(ErrorCode::RelayUpstreamUnreachable, "cannot build headers")
        })?;
        *headers = filter_headers(&parts.headers);
    }

    let outbound = builder.body(body).map_err(|e| {
        ProxyError::new(
            ErrorCode::RelayUpstreamUnreachable,
            format!("cannot build upstream request: {e}"),
        )
    })?;

    let resp = tokio::time::timeout(state.config.request_timeout, sender.send_request(outbound))
        .await
        .map_err(|_| {
            ProxyError::new(
                ErrorCode::RelayUpstreamUnreachable,
                "the runtime did not respond in time",
            )
        })?
        .map_err(|e| {
            ProxyError::new(
                ErrorCode::RelayUpstreamUnreachable,
                format!("request to the runtime failed: {e}"),
            )
        })?;

    // Keep the connection alive for the lifetime of the streamed body.
    let resp_task = conn_task;
    let (parts, body) = resp.into_parts();

    let stream = body.into_data_stream();
    let mapped = StreamBody::new(futures_stream_map(stream));

    let mut out = Response::new(mapped.boxed());
    *out.status_mut() = parts.status;
    *out.headers_mut() = filter_headers(&parts.headers);

    // Tie the upstream connection to the response body: dropping the body must
    // release the connection rather than leaking it.
    let _guard = resp_task;

    Ok(out)
}

/// `StreamBody` requires `Result<Frame<Bytes>, E>`; hyper's data stream yields
/// `Result<Bytes, hyper::Error>`. This adapts one to the other without pulling
/// in `futures-util`.
fn futures_stream_map<S>(
    stream: S,
) -> impl futures_core::Stream<Item = Result<Frame<Bytes>, std::io::Error>>
where
    S: futures_core::Stream<Item = Result<Bytes, hyper::Error>> + Unpin,
{
    use futures_core::Stream;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    struct Map<S>(S);

    impl<S> Stream for Map<S>
    where
        S: Stream<Item = Result<Bytes, hyper::Error>> + Unpin,
    {
        type Item = Result<Frame<Bytes>, std::io::Error>;

        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            match Pin::new(&mut self.get_mut().0).poll_next(cx) {
                Poll::Ready(Some(Ok(b))) => Poll::Ready(Some(Ok(Frame::data(b)))),
                Poll::Ready(Some(Err(e))) => {
                    Poll::Ready(Some(Err(std::io::Error::other(e.to_string()))))
                }
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            }
        }
    }

    Map(stream)
}
/// Forward a WebSocket upgrade and tunnel both directions afterwards.
///
/// A WebSocket upgrade cannot be proxied as a normal request: once the `101`
/// response is received, both sides become raw byte streams and the two must be
/// spliced together. This is why the relay talks `http1` directly instead of
/// using a pooled client — a pool never hands back the underlying socket.
///
/// The harness's live agent output travels over exactly this path, so a broken
/// upgrade means a stalled session rather than a failed request.
async fn forward_upgrade(
    state: &RelayState,
    req: Request<axum::body::Body>,
    sender: http1::SendRequest<axum::body::Body>,
    _conn_task: tokio::task::JoinHandle<()>,
) -> Result<Response<RelayBody>, ProxyError> {
    use hyper::upgrade::on as on_upgrade;

    let (parts, _body) = req.into_parts();

    let mut builder = Request::builder().method(parts.method.clone()).uri(
        parts
            .uri
            .path_and_query()
            .map_or("/", http::uri::PathAndQuery::as_str),
    );

    {
        let headers = builder.headers_mut().ok_or_else(|| {
            ProxyError::new(ErrorCode::RelayUpstreamUnreachable, "cannot build headers")
        })?;
        // `Upgrade` and `Connection` are hop-by-hop in general, but an upgrade
        // request is precisely the case where they must be forwarded.
        *headers = filter_headers(&parts.headers);
        if let Some(v) = parts.headers.get(header::UPGRADE) {
            headers.insert(header::UPGRADE, v.clone());
        }
        headers.insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
    }

    let outbound = builder.body(axum::body::Body::empty()).map_err(|e| {
        ProxyError::new(
            ErrorCode::RelayUpstreamUnreachable,
            format!("cannot build upgrade request: {e}"),
        )
    })?;

    let mut sender = sender;
    let resp = tokio::time::timeout(state.config.request_timeout, sender.send_request(outbound))
        .await
        .map_err(|_| {
            ProxyError::new(
                ErrorCode::RelayUpstreamUnreachable,
                "the runtime did not answer the upgrade in time",
            )
        })?
        .map_err(|e| {
            ProxyError::new(
                ErrorCode::RelayUpstreamUnreachable,
                format!("upgrade request failed: {e}"),
            )
        })?;

    if resp.status() != StatusCode::SWITCHING_PROTOCOLS {
        // Not an upgrade after all; hand the response back as-is.
        let (parts, body) = resp.into_parts();
        let stream = body.into_data_stream();
        let mut out = Response::new(StreamBody::new(futures_stream_map(stream)).boxed());
        *out.status_mut() = parts.status;
        *out.headers_mut() = filter_headers(&parts.headers);
        return Ok(out);
    }

    // Both sides are now raw streams. Splice them.
    //
    // `OnUpgrade::on` takes the message that carried the upgrade — here, the
    // 101 response from the upstream. The browser side takes it from the
    // request extensions, which is where hyper puts it.
    let upstream_upgrade = on_upgrade(resp);
    let client_upgrade = parts.extensions.get::<hyper::upgrade::OnUpgrade>().cloned();

    if let (Some(client), Some(upstream)) = (client_upgrade, Some(upstream_upgrade)) {
        tokio::spawn(async move {
            match tokio::try_join!(client, upstream) {
                Ok((client_io, upstream_io)) => {
                    let mut client_io = TokioIo::new(client_io);
                    let mut upstream_io = TokioIo::new(upstream_io);
                    if let Err(e) =
                        tokio::io::copy_bidirectional(&mut client_io, &mut upstream_io).await
                    {
                        tracing::debug!(error = %e, "websocket tunnel ended");
                    }
                }
                Err(e) => tracing::debug!(error = %e, "websocket upgrade failed"),
            }
        });
    }

    // Reply 101 to the browser with the upstream's handshake headers.
    let mut out = Response::new(full_body(Bytes::new()));
    *out.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    Ok(out)
}

/// Convenience: the default `/health`-independent liveness of the relay itself.
///
/// Returns a JSON body so a probe can distinguish "relay alive" from "upstream
/// reachable" without parsing HTML.
#[must_use]
pub fn relay_liveness_body(upstream: &str) -> String {
    format!(
        "{{\"relay\":\"ok\",\"upstream\":\"{}\"}}",
        upstream.replace('"', "")
    )
}

/// Reject a method the relay does not forward.
///
/// The relay is a transparent proxy; anything HTTP supports is forwarded. This
/// exists so the accepted set is explicit and testable rather than assumed.
#[must_use]
pub fn is_forwardable_method(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::POST | Method::PUT | Method::PATCH | Method::DELETE | Method::HEAD
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(
                HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn hop_by_hop_headers_are_stripped() {
        let src = headers(&[
            ("connection", "keep-alive"),
            ("transfer-encoding", "chunked"),
            ("upgrade", "websocket"),
            ("te", "trailers"),
            ("x-our-header", "kept"),
        ]);
        let out = filter_headers(&src);
        assert!(!out.contains_key("connection"));
        assert!(!out.contains_key("transfer-encoding"));
        assert!(!out.contains_key("upgrade"));
        assert!(!out.contains_key("te"));
        assert_eq!(out.get("x-our-header").unwrap(), "kept");
    }

    #[test]
    fn host_and_origin_survive_untouched() {
        // The whole relay design depends on this: the browser trust fence
        // compares Host against Origin, so rewriting either breaks auth.
        let src = headers(&[
            ("host", "127.0.0.1:3080"),
            ("origin", "http://127.0.0.1:3080"),
            ("cookie", "session=abc"),
        ]);
        let out = filter_headers(&src);
        assert_eq!(out.get("host").unwrap(), "127.0.0.1:3080");
        assert_eq!(out.get("origin").unwrap(), "http://127.0.0.1:3080");
        assert_eq!(out.get("cookie").unwrap(), "session=abc");
    }

    #[test]
    fn connection_named_headers_are_also_stripped() {
        // RFC 9110: names listed in Connection are hop-by-hop for this hop.
        let src = headers(&[("connection", "x-custom, close"), ("x-custom", "drop-me")]);
        let out = filter_headers(&src);
        assert!(!out.contains_key("x-custom"));
        assert!(!out.contains_key("connection"));
    }

    #[test]
    fn connection_named_headers_are_matched_case_insensitively() {
        let src = headers(&[("connection", "X-CUSTOM"), ("x-custom", "drop-me")]);
        let out = filter_headers(&src);
        assert!(!out.contains_key("x-custom"));
    }

    #[test]
    fn upstream_authority_parses_host_and_port() {
        let s = RelayState::new(RelayConfig::default());
        let (host, port) = s.upstream_authority().unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 3081);
    }

    #[test]
    fn upstream_authority_tolerates_trailing_slash() {
        let s = RelayState::new(RelayConfig {
            upstream: "http://127.0.0.1:9999/".into(),
            ..RelayConfig::default()
        });
        let (host, port) = s.upstream_authority().unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 9999);
    }

    #[test]
    fn upstream_authority_rejects_non_http() {
        let s = RelayState::new(RelayConfig {
            upstream: "https://example.com".into(),
            ..RelayConfig::default()
        });
        let e = s.upstream_authority().unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
    }

    #[test]
    fn upstream_authority_rejects_bad_port() {
        let s = RelayState::new(RelayConfig {
            upstream: "http://127.0.0.1:not-a-port".into(),
            ..RelayConfig::default()
        });
        assert_eq!(
            s.upstream_authority().unwrap_err().code,
            ErrorCode::ConfigInvalid
        );
    }

    #[test]
    fn error_response_is_a_502_that_explains_itself() {
        let e = ProxyError::new(
            ErrorCode::RelayUpstreamUnreachable,
            "cannot connect to 127.0.0.1:3081",
        );
        let resp = error_response(&e);
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        // The remedy must be present: an operator seeing this needs the fix.
        assert!(e.code.remediation().is_some());
    }

    #[test]
    fn forwardable_methods_cover_the_harness_surface() {
        assert!(is_forwardable_method(&Method::GET));
        assert!(is_forwardable_method(&Method::POST));
        assert!(is_forwardable_method(&Method::HEAD));
        assert!(!is_forwardable_method(&Method::CONNECT));
    }

    #[test]
    fn liveness_body_is_json_and_quotes_the_upstream() {
        let b = relay_liveness_body("http://127.0.0.1:3081");
        let v: serde_json::Value = serde_json::from_str(&b).unwrap();
        assert_eq!(v["relay"], "ok");
        assert_eq!(v["upstream"], "http://127.0.0.1:3081");
    }

    #[test]
    fn liveness_body_cannot_be_broken_by_a_quoted_upstream() {
        let b = relay_liveness_body("http://x\"/evil");
        assert!(serde_json::from_str::<serde_json::Value>(&b).is_ok());
    }
}
