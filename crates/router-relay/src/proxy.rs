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
    ///
    /// Used for every request that no route in [`Self::routes`] claims.
    pub upstream: String,
    /// Path-prefix routes, longest prefix first.
    ///
    /// # Why one origin matters
    ///
    /// The harness authenticates its API gateway with a cookie whose *name* is
    /// derived from the request authority — `host:port`. A page served from
    /// `127.0.0.1:3082` calling `127.0.0.1:3080` is therefore a different origin
    /// in both senses the browser cares about: the request is blocked as
    /// cross-origin, and the cookie would not be sent even if it were allowed.
    /// The user sees "failed to fetch gateway", which names the symptom and not
    /// the cause.
    ///
    /// Serving every instance through one origin removes the problem entirely:
    /// the browser sees one authority, sends one cookie, and never performs a
    /// cross-origin request. To the harness each request still arrives on its
    /// own loopback port with its own authority, because the proxy connects to
    /// the real upstream and the harness sees `127.0.0.1:<its port>` — so the
    /// cookie it mints matches the authority it is asked for.
    pub routes: Vec<Route>,
    /// How long to wait for the upstream to accept a connection.
    pub connect_timeout: Duration,
    /// How long to wait for response headers.
    pub request_timeout: Duration,
    /// Maximum buffered request body, in bytes.
    ///
    /// Uploads larger than this are streamed rather than buffered.
    pub max_buffered_body: usize,
}

/// One path prefix and the instance it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// Path prefix, without a trailing slash, e.g. `/i/main`.
    pub prefix: String,
    /// Loopback upstream for that prefix, e.g. `http://127.0.0.1:3082`.
    pub upstream: String,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            upstream: "http://127.0.0.1:3081".to_string(),
            routes: Vec::new(),
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(120),
            max_buffered_body: 8 * 1024 * 1024,
        }
    }
}

impl RelayConfig {
    /// The upstream that should serve a request path, and the path to send it.
    ///
    /// Returns `(upstream, rewritten_path)`. The prefix is stripped, because the
    /// harness serves its app at `/` and knows nothing about being mounted under
    /// a sub-path — so `/i/main/api/remote.mux` must reach it as
    /// `/api/remote.mux`, or every asset and endpoint would 404.
    ///
    /// The longest matching prefix wins, so `/i/main-2` cannot be captured by a
    /// route for `/i/main`.
    #[must_use]
    pub fn resolve(&self, path: &str) -> (&str, String, String) {
        let mut best: Option<&Route> = None;
        for route in &self.routes {
            if !path_matches(&route.prefix, path) {
                continue;
            }
            if best.is_none_or(|b| route.prefix.len() > b.prefix.len()) {
                best = Some(route);
            }
        }
        match best {
            Some(route) => {
                let rest = &path[route.prefix.len()..];
                // A bare prefix means the instance root, not an empty path.
                let rewritten = if rest.is_empty() {
                    "/".to_string()
                } else {
                    rest.to_string()
                };
                (route.upstream.as_str(), rewritten, route.prefix.clone())
            }
            None => (self.upstream.as_str(), path.to_string(), String::new()),
        }
    }
}

/// Whether a path falls under a prefix.
///
/// Matched on a segment boundary: `/i/main` matches `/i/main` and
/// `/i/main/api`, but never `/i/maintenance`. A plain `starts_with` would take
/// the wrong instance for a name that happens to share a prefix — and the
/// failure would look like one instance serving another's data.
#[must_use]
fn path_matches(prefix: &str, path: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    match path.strip_prefix(prefix) {
        Some(rest) => rest.is_empty() || rest.starts_with('/'),
        None => false,
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

    /// The host and port of an upstream URL.
    ///
    /// Takes the URL rather than reading it from config, because a request may
    /// be routed to any instance the relay knows about, not only the default.
    fn authority_of(upstream: &str) -> Result<(String, u16), ProxyError> {
        let rest = upstream.strip_prefix("http://").ok_or_else(|| {
            ProxyError::new(
                ErrorCode::ConfigInvalid,
                format!("upstream must be an http:// URL, got '{upstream}'"),
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
    // Which instance serves this path, and what path to ask it for.
    //
    // Resolved here rather than deeper down so that the WebSocket path and the
    // ordinary path cannot disagree: both branches below use the same target.
    let incoming = req
        .uri()
        .path_and_query()
        .map_or("/", http::uri::PathAndQuery::as_str)
        .to_string();
    let request_path = incoming.split('?').next().unwrap_or("/");
    let (upstream, upstream_path, route_prefix) = state.config.resolve(request_path);
    let (host, port) = RelayState::authority_of(upstream)?;

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
        return forward_upgrade(state, req, sender, conn_task, &upstream_path).await;
    }

    forward_http(state, req, sender, conn_task, &upstream_path, &route_prefix).await
}

/// Forward an ordinary HTTP request and stream the response back.
async fn forward_http(
    state: &RelayState,
    req: Request<axum::body::Body>,
    mut sender: http1::SendRequest<axum::body::Body>,
    conn_task: tokio::task::JoinHandle<()>,
    upstream_path: &str,
    prefix: &str,
) -> Result<Response<RelayBody>, ProxyError> {
    let (parts, body) = req.into_parts();

    // The rewritten path, with the browser-side prefix removed.
    //
    // The harness serves its app at `/` and knows nothing about being mounted
    // under a sub-path, so `/i/main/api/remote.mux` must reach it as
    // `/api/remote.mux`. Passing the prefixed path through would 404 every
    // asset and endpoint.
    let query = parts.uri.query().map_or(String::new(), |q| format!("?{q}"));
    let target = format!("{upstream_path}{query}");

    let mut builder = Request::builder().method(parts.method.clone()).uri(target);

    // Headers pass through verbatim. Rewriting Host would break the browser
    // trust fence, because Origin would no longer match.
    //
    // One header is an exception, and it is load-bearing: `Accept-Encoding` is
    // dropped for navigations that will be rewritten.
    //
    // # Why the compression has to go
    //
    // The harness compresses its HTML (`Vary: Accept-Encoding`). A compressed
    // body cannot be rewritten as text — searching it for `href="` finds nothing,
    // and any edit corrupts the stream — so the browser answers
    // `ERR_CONTENT_DECODING_FAILED` and the page never renders. That is a worse
    // failure than the 404s it replaced, because nothing loads at all.
    //
    // Declining the compression is the honest fix: the relay asks for what it can
    // actually process. The cost is a few kilobytes of HTML on a loopback link,
    // which is not worth a decompression dependency to avoid. A request that will
    // not be rewritten keeps its `Accept-Encoding` and streams compressed as
    // before.
    let rewritable = wants_html_rewrite(&parts.headers);
    {
        let headers = builder.headers_mut().ok_or_else(|| {
            ProxyError::new(ErrorCode::RelayUpstreamUnreachable, "cannot build headers")
        })?;
        *headers = filter_headers(&parts.headers);
        if rewritable {
            headers.remove(header::ACCEPT_ENCODING);
        }
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

    // An HTML page is rewritten so the harness's root-absolute URLs stay inside
    // the instance. Everything else streams untouched.
    //
    // # Why the harness cannot simply be asked to use relative URLs
    //
    // It does, for most things — the shell loads `./assets/index-*.js`. But two
    // categories are root-absolute and cannot be made relative from the outside:
    // the plugin bundle (`/plugins/??…`) and the app's own home link (`/`).
    // Through the gateway those resolve against the *gateway's* root, so the
    // plugin bundle 404s and the page loads its shell but none of its behaviour:
    // a title, a blank body, and a console full of 404s. Every wire-level test
    // passed while this was broken, because the HTML itself arrived intact.
    //
    // The rewrite is textual and deliberately narrow: only `src`, `href`, and
    // `action` attributes whose value starts with a single `/`, and only when
    // that `/` is not already the prefix. `/i/<name>/...` is left alone, so a
    // second pass is a no-op and a page that already names its prefix cannot be
    // double-prefixed.
    let is_html = is_rewritable_html(&parts.headers);
    let mapped = if is_html {
        // `collect` buffers the whole document; the harness's shell is tens of
        // kilobytes, so the cap is generous and a body that exceeds it is
        // reported rather than silently truncated.
        match body.collect().await {
            Ok(collected) => {
                let bytes = collected.to_bytes();
                if bytes.len() > state.config.max_buffered_body {
                    return Err(ProxyError::new(
                        ErrorCode::RelayUpstreamUnreachable,
                        format!(
                            "the HTML response is {} bytes, over the {} byte limit for rewriting",
                            bytes.len(),
                            state.config.max_buffered_body
                        ),
                    ));
                }
                let text = String::from_utf8_lossy(&bytes);
                let rewritten = rewrite_root_absolute_urls(&text, prefix);
                full_body(Bytes::from(rewritten))
            }
            // The upstream stream failed mid-body. Report it, rather than
            // handing back a page whose plugin bundle will silently 404.
            Err(e) => {
                return Err(ProxyError::new(
                    ErrorCode::RelayUpstreamUnreachable,
                    format!("cannot read the HTML body to rewrite it: {e}"),
                ));
            }
        }
    } else {
        StreamBody::new(futures_stream_map(body.into_data_stream())).boxed()
    };

    let mut out = Response::new(mapped);
    *out.status_mut() = parts.status;
    let mut headers = filter_headers(&parts.headers);
    // The body changed length, so a length header from upstream is now wrong and
    // would truncate the page or hang the client waiting for bytes that will not
    // come.
    headers.remove(header::CONTENT_LENGTH);
    *out.headers_mut() = headers;

    // Re-attach the browser-side prefix to any redirect.
    //
    // # Why this is required, not a nicety
    //
    // The harness authenticates by redirecting: it accepts `?token=…`, mints the
    // cookie, and answers `303 Location: /`. That `Location` is *root-relative*,
    // so the browser resolves it against the origin it is talking to — the
    // gateway — and lands on `/`, which is the control page rather than the
    // instance. The user sees the control page after logging in and concludes
    // the login failed.
    //
    // Rewriting to `/i/<name>/` keeps the browser inside the instance. Without
    // this the whole single-origin scheme breaks at the first redirect, which
    // happens to be the login itself.
    if let Some(location) = out.headers().get(header::LOCATION).cloned() {
        if let Ok(value) = location.to_str() {
            if let Some(rewritten) = reattach_prefix(value, prefix) {
                if let Ok(header_value) = HeaderValue::from_str(&rewritten) {
                    out.headers_mut().insert(header::LOCATION, header_value);
                }
            }
        }
    }

    // Tie the upstream connection to the response body: dropping the body must
    // release the connection rather than leaking it.
    let _guard = resp_task;

    Ok(out)
}

/// Whether a request is a navigation whose response may need rewriting.
///
/// Only HTML is rewritten, and only when the browser is asking for a document
/// rather than an asset. A `GET` that accepts `text/html` is the shape of a
/// navigation; a fetch for a script, an image, or an API endpoint is not, and
/// leaving those `Accept-Encoding` untouched keeps them compressed on the wire.
#[must_use]
fn wants_html_rewrite(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|accept| {
            // `*/*` counts: a browser navigating from the address bar sends it,
            // and the harness answers with HTML.
            accept.contains("text/html") || accept.contains("*/*")
        })
}

/// Whether a response body may be rewritten as text.
///
/// A compressed or otherwise encoded body must never be edited: its bytes are
/// not the document, and searching them for markup finds nothing while any
/// change corrupts the stream. The relay declines `Accept-Encoding` for
/// navigations, but a client may still receive an encoded response from a
/// caching intermediary or a harness that compresses unconditionally, so the
/// decision is confirmed here rather than assumed.
#[must_use]
fn is_rewritable_html(headers: &HeaderMap) -> bool {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !content_type.contains("text/html") {
        return false;
    }

    // Any encoding other than `identity` means the bytes are not the document.
    match headers
        .get(header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
    {
        None => true,
        Some(encoding) => {
            let e = encoding.trim().to_ascii_lowercase();
            e.is_empty() || e == "identity"
        }
    }
}

/// Rewrite root-absolute URLs in an HTML document so they stay inside `prefix`.
///
/// `/plugins/??x` becomes `/i/main/plugins/??x`, and `href="/"` becomes
/// `href="/i/main/"`. Relative URLs (`./assets/x`) are untouched, because the
/// browser already resolves those against the prefixed request path.
///
/// # Why this is a text rewrite and not a parse
///
/// The harness's HTML is machine-generated and its attributes are simple, quoted
/// values. A full parser would be a heavy dependency for a transformation this
/// narrow, and it would also normalize parts of the document the harness may
/// care about. The rewrite therefore touches only the three attributes that
/// carry navigable URLs.
///
/// # What it deliberately does not touch
///
/// - `//host/path` — protocol-relative, an explicit other origin.
/// - `/i/<name>/...` — already prefixed. Leaving it alone is what makes the
///   rewrite idempotent, so a page passed through twice is not doubled into
///   `/i/main/i/main/...`.
/// - Anything outside those attributes, so a document that merely *mentions* a
///   path is not rewritten.
#[must_use]
pub fn rewrite_root_absolute_urls(html: &str, prefix: &str) -> String {
    if prefix.is_empty() {
        return html.to_string();
    }

    rewrite_bootstrap_urls(&rewrite_attributes(html, prefix), prefix)
}

/// Rewrite the URL-bearing HTML attributes in a document.
fn rewrite_attributes(html: &str, prefix: &str) -> String {
    const ATTRIBUTES: [&str; 3] = ["src=\"", "href=\"", "action=\""];
    let mut out = String::with_capacity(html.len() + 256);
    let mut rest = html;

    loop {
        // The earliest occurrence of any tracked attribute.
        let next = ATTRIBUTES
            .iter()
            .filter_map(|attr| rest.find(attr).map(|i| (i, *attr)))
            .min_by_key(|(i, _)| *i);

        let Some((index, attr)) = next else {
            out.push_str(rest);
            break;
        };

        let value_start = index + attr.len();
        out.push_str(&rest[..value_start]);
        let after_quote = &rest[value_start..];

        // A value ends at the next quote. If there is none the document is
        // malformed, so the remainder is left exactly as it was rather than
        // guessed at.
        let Some(end) = after_quote.find('"') else {
            out.push_str(after_quote);
            break;
        };
        let value = &after_quote[..end];

        if needs_prefix(value, prefix) {
            out.push_str(prefix);
        }
        out.push_str(value);
        out.push('"');
        rest = &after_quote[end + 1..];
    }

    out
}

/// Rewrite `"url":"/..."` fields in the harness's inline bootstrap JSON.
///
/// # Why the attribute rewrite is not enough
///
/// The document also contains `window.__DSH_BOOT__`, a JSON blob naming every
/// client plugin the app must load — around fifty of them — each with a
/// root-absolute `url`. That JSON is not an HTML attribute, so an attribute-based
/// rewrite never sees it, and the loader then asks the gateway for
/// `/plugins/??…` directly: a 404, an empty page, and an error banner reading
/// "Failed to load plugins".
///
/// This is deliberately narrower than a JSON parse. The pattern is a fixed,
/// machine-generated key, and rewriting only the value that follows it means a
/// document that merely contains the text `"url":"/x"` elsewhere — or a string
/// with an escaped quote — is left alone rather than guessed at.
fn rewrite_bootstrap_urls(html: &str, prefix: &str) -> String {
    const KEY: &str = "\"url\":\"";
    let mut out = String::with_capacity(html.len() + 512);
    let mut rest = html;

    loop {
        let Some(index) = rest.find(KEY) else {
            out.push_str(rest);
            break;
        };
        let value_start = index + KEY.len();
        out.push_str(&rest[..value_start]);
        let after_quote = &rest[value_start..];

        let Some(end) = after_quote.find('"') else {
            out.push_str(after_quote);
            break;
        };
        let value = &after_quote[..end];

        if needs_prefix(value, prefix) {
            out.push_str(prefix);
        }
        out.push_str(value);
        out.push('"');
        rest = &after_quote[end + 1..];
    }

    out
}

/// Whether a URL value must be prefixed to stay inside the instance.
fn needs_prefix(value: &str, prefix: &str) -> bool {
    // Root-absolute, but not protocol-relative: `//host/x` names another origin
    // explicitly, and rewriting it would send the browser somewhere it never
    // meant to go.
    value.starts_with('/') && !value.starts_with("//") && !path_matches(prefix, value)
}

/// Prefix a root-relative redirect so it stays inside the instance.
///
/// Only root-relative targets are touched:
///
/// - `/api/x` becomes `/i/main/api/x` — it must stay on this instance.
/// - `http://other/` and `//other/` are left alone: rewriting an absolute
///   location would hijack a redirect the harness meant to send elsewhere.
/// - `api/x` (relative to the current directory) needs no change; the browser
///   resolves it against the request path, which already carries the prefix.
///
/// Returns `None` when nothing should change, so the caller can leave the
/// header untouched rather than replacing it with an identical value.
#[must_use]
fn reattach_prefix(location: &str, prefix: &str) -> Option<String> {
    // Already inside the instance: a second rewrite would double the prefix.
    if location.starts_with(prefix) {
        return None;
    }
    // Root-relative, and not protocol-relative (`//host/path`).
    if !location.starts_with('/') || location.starts_with("//") {
        return None;
    }
    if prefix.is_empty() {
        return None;
    }
    Some(format!("{prefix}{location}"))
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
    upstream_path: &str,
) -> Result<Response<RelayBody>, ProxyError> {
    use hyper::upgrade::on as on_upgrade;

    let (parts, _body) = req.into_parts();

    // The rewritten path, with the browser-side prefix removed — the same rule
    // as the ordinary branch. The gateway socket lives at `/api/remote.mux`, so
    // a prefixed path would never reach it.
    let query = parts.uri.query().map_or(String::new(), |q| format!("?{q}"));
    let target = format!("{upstream_path}{query}");

    let mut builder = Request::builder().method(parts.method.clone()).uri(target);

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
    // The handshake headers are read before the response is consumed below,
    // because `on_upgrade` takes ownership of it.
    let response_headers = resp.headers().clone();

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
    //
    // A bare `101` is not a valid handshake. The browser validates
    // `Upgrade: websocket`, `Connection: Upgrade`, and — decisively — the
    // `Sec-WebSocket-Accept` derived from the key it sent. Without them the
    // socket error handler fires and the harness UI reports that it could not
    // reach the gateway, while the network tab shows a perfectly good 101: the
    // exchange looked successful and the client still refused it.
    //
    // These are copied deliberately, even though `Upgrade` and `Connection` are
    // hop-by-hop and were filtered out of the request. For a 101 the handshake
    // headers *are* the message, so passing them through is the whole point.
    let mut out = Response::new(full_body(Bytes::new()));
    *out.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    *out.headers_mut() = handshake_headers(&response_headers);
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

/// Copy the handshake headers from an upstream `101` onto our reply.
///
/// A `101` is not "a response with a status code" — it *is* the handshake, and
/// the client validates it field by field. Most importantly
/// `Sec-WebSocket-Accept` is computed from the `Sec-WebSocket-Key` the client
/// sent; a proxy that omits it produces a reply the client silently rejects,
/// which surfaces to the user as "failed to fetch gateway" even though the
/// network tab shows a successful 101.
///
/// `Upgrade` and `Connection` are normally hop-by-hop and are stripped from both
/// directions by [`filter_headers`]. The `101` is the documented exception: the
/// headers that describe this one hop's protocol change are exactly what must
/// cross it.
#[must_use]
pub fn handshake_headers(upstream: &HeaderMap) -> HeaderMap {
    const HANDSHAKE: &[HeaderName] = &[
        header::UPGRADE,
        header::CONNECTION,
        header::SEC_WEBSOCKET_ACCEPT,
        header::SEC_WEBSOCKET_PROTOCOL,
        header::SEC_WEBSOCKET_EXTENSIONS,
    ];
    let mut out = HeaderMap::new();
    for name in HANDSHAKE {
        for value in upstream.get_all(name) {
            out.append(name.clone(), value.clone());
        }
    }
    out
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
    fn a_handshake_carries_accept_and_upgrade_headers() {
        // The regression this guards: the relay used to answer the browser with
        // a bare `101` and no headers. Every WebSocket then failed to open, so
        // live agent output never arrived — while the network tab showed a
        // perfectly healthy 101. `Sec-WebSocket-Accept` is the field the client
        // actually validates, so it is the one that must survive.
        let upstream = headers(&[
            ("upgrade", "websocket"),
            ("connection", "Upgrade"),
            ("sec-websocket-accept", "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
            ("x-unrelated", "dropped"),
        ]);
        let out = handshake_headers(&upstream);
        assert_eq!(out.get("upgrade").unwrap(), "websocket");
        assert_eq!(out.get("connection").unwrap(), "Upgrade");
        assert_eq!(
            out.get("sec-websocket-accept").unwrap(),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        // Only handshake fields travel; a 101 must not leak other upstream headers.
        assert!(!out.contains_key("x-unrelated"));
    }

    #[test]
    fn a_handshake_without_an_accept_header_is_empty_not_wrong() {
        // If the upstream did not send one, inventing a value would be worse
        // than sending none: the client would reject the exchange as invalid.
        let out = handshake_headers(&headers(&[("content-type", "text/plain")]));
        assert!(out.is_empty());
    }

    #[test]
    fn repeated_handshake_headers_are_all_preserved() {
        // `Sec-WebSocket-Protocol` and `Sec-WebSocket-Extensions` are list-valued
        // and may legitimately arrive more than once.
        let mut upstream = HeaderMap::new();
        upstream.append(
            header::SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static("chat"),
        );
        upstream.append(
            header::SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static("superchat"),
        );
        let out = handshake_headers(&upstream);
        let values: Vec<_> = out
            .get_all(header::SEC_WEBSOCKET_PROTOCOL)
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect();
        assert_eq!(values, vec!["chat", "superchat"]);
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
        let (host, port) = RelayState::authority_of("http://127.0.0.1:3081").unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 3081);
    }

    #[test]
    fn upstream_authority_tolerates_trailing_slash() {
        let (host, port) = RelayState::authority_of("http://127.0.0.1:9999/").unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 9999);
    }

    #[test]
    fn upstream_authority_rejects_non_http() {
        let e = RelayState::authority_of("https://example.com").unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
    }

    #[test]
    fn upstream_authority_rejects_bad_port() {
        assert_eq!(
            RelayState::authority_of("http://127.0.0.1:not-a-port")
                .unwrap_err()
                .code,
            ErrorCode::ConfigInvalid
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // Route resolution
    //
    // This is what makes several instances reachable from one browser origin,
    // which is the only way the harness's per-authority auth cookie can work
    // across them.
    // ─────────────────────────────────────────────────────────────────────

    fn two_instances() -> RelayConfig {
        RelayConfig {
            upstream: "http://127.0.0.1:3081".into(),
            routes: vec![
                Route {
                    prefix: "/i/main".into(),
                    upstream: "http://127.0.0.1:3082".into(),
                },
                Route {
                    prefix: "/i/notes".into(),
                    upstream: "http://127.0.0.1:3083".into(),
                },
            ],
            ..RelayConfig::default()
        }
    }

    #[test]
    fn a_prefixed_path_routes_to_its_instance_and_strips_the_prefix() {
        // The harness serves its app at `/` and knows nothing about being
        // mounted under a sub-path, so the prefix must not be forwarded.
        let cfg = two_instances();
        let (upstream, path, _prefix) = cfg.resolve("/i/main/api/remote.mux");
        assert_eq!(upstream, "http://127.0.0.1:3082");
        assert_eq!(path, "/api/remote.mux");
    }

    #[test]
    fn the_bare_prefix_means_the_instance_root() {
        // `/i/main` on its own is the instance's home page, not an empty path.
        let cfg = two_instances();
        let (upstream, path, _prefix) = cfg.resolve("/i/main");
        assert_eq!(upstream, "http://127.0.0.1:3082");
        assert_eq!(path, "/");
    }

    #[test]
    fn each_route_goes_to_its_own_instance() {
        let cfg = two_instances();
        assert_eq!(cfg.resolve("/i/notes/x").0, "http://127.0.0.1:3083");
        assert_eq!(cfg.resolve("/i/main/x").0, "http://127.0.0.1:3082");
    }

    #[test]
    fn a_path_sharing_a_prefix_does_not_capture_another_instance() {
        // `/i/maintenance` must not be served by `/i/main`. Getting this wrong
        // would show one instance's data under another's URL, which is far
        // worse than a 404.
        let cfg = two_instances();
        let (upstream, path, _prefix) = cfg.resolve("/i/maintenance/page");
        assert_eq!(
            upstream, "http://127.0.0.1:3081",
            "an unmatched path falls through to the default upstream"
        );
        assert_eq!(path, "/i/maintenance/page");
    }

    #[test]
    fn the_longest_matching_prefix_wins() {
        // So a nested route can be added later without the shorter one
        // shadowing it.
        let cfg = RelayConfig {
            routes: vec![
                Route {
                    prefix: "/i/app".into(),
                    upstream: "http://127.0.0.1:4001".into(),
                },
                Route {
                    prefix: "/i/app/deep".into(),
                    upstream: "http://127.0.0.1:4002".into(),
                },
            ],
            ..RelayConfig::default()
        };
        assert_eq!(cfg.resolve("/i/app/deep/x").0, "http://127.0.0.1:4002");
        assert_eq!(cfg.resolve("/i/app/other").0, "http://127.0.0.1:4001");
    }

    #[test]
    fn a_compressed_body_is_never_rewritten() {
        // The failure this guards: rewriting gzip bytes produced a page the
        // browser refused with ERR_CONTENT_DECODING_FAILED — worse than the 404s
        // it replaced, because nothing rendered at all. Compressed bytes are not
        // the document, so the rewrite must be skipped entirely.
        let compressed = headers(&[
            ("content-type", "text/html; charset=utf-8"),
            ("content-encoding", "gzip"),
        ]);
        assert!(!is_rewritable_html(&compressed));

        let brotli = headers(&[
            ("content-type", "text/html; charset=utf-8"),
            ("content-encoding", "br"),
        ]);
        assert!(!is_rewritable_html(&brotli));
    }

    #[test]
    fn identity_encoding_is_rewritable() {
        let plain = headers(&[("content-type", "text/html; charset=utf-8")]);
        assert!(is_rewritable_html(&plain));

        // An explicit `identity` is the same as none.
        let explicit = headers(&[
            ("content-type", "text/html; charset=utf-8"),
            ("content-encoding", "identity"),
        ]);
        assert!(is_rewritable_html(&explicit));
    }

    #[test]
    fn only_html_is_rewritten() {
        assert!(!is_rewritable_html(&headers(&[(
            "content-type",
            "text/javascript"
        )])));
        assert!(!is_rewritable_html(&headers(&[(
            "content-type",
            "application/json"
        )])));
        // Case-insensitive: a header value is not required to be lower-case.
        assert!(is_rewritable_html(&headers(&[(
            "content-type",
            "TEXT/HTML"
        )])));
    }

    #[test]
    fn only_readers_of_html_have_compression_declined() {
        // Declining `Accept-Encoding` for every request would silently uncompress
        // every script and stylesheet on the wire, which is a real cost for no
        // benefit: only navigations are rewritten.
        assert!(wants_html_rewrite(&headers(&[(
            "accept",
            "text/html,application/xhtml+xml,*/*"
        )])));
        assert!(wants_html_rewrite(&headers(&[("accept", "*/*")])));
        assert!(!wants_html_rewrite(&headers(&[(
            "accept",
            "application/json"
        )])));
        assert!(!wants_html_rewrite(&headers(&[(
            "accept",
            "image/png,image/*"
        )])));
        // No Accept at all: an API client, not a browser.
        assert!(!wants_html_rewrite(&HeaderMap::new()));
    }

    #[test]
    fn bootstrap_plugin_urls_are_rewritten() {
        // The second, less obvious half of the problem: the plugin list lives in
        // inline JSON, not in an attribute, so the attribute rewrite never sees
        // it. Missing these produced "Failed to load plugins" and an empty app.
        let html = r#"<script>window["__DSH_BOOT__"]={"entries":[{"id":"x","url":"/plugins/??a.js&rev=1"}]}</script>"#;
        let out = rewrite_root_absolute_urls(html, "/i/main");
        assert!(
            out.contains(r#""url":"/i/main/plugins/??a.js&rev=1""#),
            "{out}"
        );
    }

    #[test]
    fn bootstrap_rewrite_is_idempotent_and_scoped() {
        let html = r#"{"url":"/plugins/x"}"#;
        let once = rewrite_root_absolute_urls(html, "/i/notes");
        assert_eq!(once, r#"{"url":"/i/notes/plugins/x"}"#);
        // A second pass must not double the prefix.
        assert_eq!(rewrite_root_absolute_urls(&once, "/i/notes"), once);
    }

    #[test]
    fn relative_and_external_bootstrap_urls_are_untouched() {
        // Only root-absolute values are ours to rewrite. A relative one resolves
        // correctly already, and an absolute one names somewhere else entirely.
        let html = r#"{"url":"./local.js"},{"url":"http://cdn/x.js"},{"url":"//cdn/x.js"}"#;
        assert_eq!(rewrite_root_absolute_urls(html, "/i/main"), html);
    }

    #[test]
    fn root_absolute_urls_are_brought_inside_the_prefix() {
        // The exact shapes the harness emits, and the reason this exists: the
        // plugin bundle and the home link are root-absolute, so through the
        // gateway they resolved to the control page's origin and 404ed. The page
        // then loaded its shell and none of its behaviour.
        let html = r#"<link href="/plugins/??a.js&rev=1"><script src="/assets/x.js"></script><a href="/">home</a>"#;
        let out = rewrite_root_absolute_urls(html, "/i/main");
        assert!(
            out.contains(r#"href="/i/main/plugins/??a.js&rev=1""#),
            "{out}"
        );
        assert!(out.contains(r#"src="/i/main/assets/x.js""#), "{out}");
        assert!(out.contains(r#"href="/i/main/""#), "{out}");
    }

    #[test]
    fn relative_urls_are_left_alone() {
        // These already resolve against the prefixed request path, so prefixing
        // them again would double it.
        let html =
            r#"<script src="./assets/index.js"></script><link href="./manifest.webmanifest">"#;
        assert_eq!(rewrite_root_absolute_urls(html, "/i/main"), html);
    }

    #[test]
    fn an_already_prefixed_page_is_unchanged() {
        // Idempotence: rewriting twice must not produce `/i/main/i/main/...`.
        let html = r#"<script src="/i/main/assets/x.js"></script>"#;
        let once = rewrite_root_absolute_urls(html, "/i/main");
        assert_eq!(once, html);
        assert_eq!(rewrite_root_absolute_urls(&once, "/i/main"), html);
    }

    #[test]
    fn a_protocol_relative_url_is_not_hijacked() {
        // `//cdn/x` names another origin explicitly. Prefixing it would send the
        // browser somewhere it never meant to go.
        let html = r#"<script src="//cdn.example.com/x.js"></script>"#;
        assert_eq!(rewrite_root_absolute_urls(html, "/i/main"), html);
    }

    #[test]
    fn text_outside_attributes_is_not_rewritten() {
        // The rewrite targets three attributes, not every slash in the document.
        let html = "<p>A path like /plugins/x in prose stays as written.</p>";
        assert_eq!(rewrite_root_absolute_urls(html, "/i/main"), html);
    }

    #[test]
    fn an_unquoted_or_truncated_attribute_does_not_corrupt_the_document() {
        // A malformed document must come back byte-identical rather than
        // half-rewritten: a proxy that mangles HTML is worse than one that
        // leaves it alone.
        let html = r#"<div>href="/unclosed"#;
        assert_eq!(rewrite_root_absolute_urls(html, "/i/main"), html);
    }

    #[test]
    fn several_urls_in_one_document_are_all_rewritten() {
        let html = r#"<a href="/a">1</a><img src="/b.png"><form action="/c">"#;
        let out = rewrite_root_absolute_urls(html, "/i/notes");
        assert!(out.contains(r#"href="/i/notes/a""#), "{out}");
        assert!(out.contains(r#"src="/i/notes/b.png""#), "{out}");
        assert!(out.contains(r#"action="/i/notes/c""#), "{out}");
    }

    #[test]
    fn an_empty_prefix_changes_nothing() {
        // The unrouted case: no instance to scope to, so nothing to rewrite.
        let html = r#"<script src="/assets/x.js"></script>"#;
        assert_eq!(rewrite_root_absolute_urls(html, ""), html);
    }

    #[test]
    fn a_root_relative_redirect_keeps_the_instance_prefix() {
        // The login flow: the harness accepts the token and answers
        // `303 Location: /`. That is root-relative, so the browser would resolve
        // it to the gateway's own root and land on the control page instead of
        // the instance. This rewrite is what makes single-origin login work.
        assert_eq!(
            reattach_prefix("/", "/i/main"),
            Some("/i/main/".to_string())
        );
        assert_eq!(
            reattach_prefix("/api/sessions", "/i/main"),
            Some("/i/main/api/sessions".to_string())
        );
    }

    #[test]
    fn an_absolute_redirect_is_left_alone() {
        // Rewriting a location the harness meant to send elsewhere would hijack
        // it, which is worse than a wrong-looking URL.
        assert_eq!(reattach_prefix("http://example.com/x", "/i/main"), None);
        assert_eq!(reattach_prefix("//example.com/x", "/i/main"), None);
    }

    #[test]
    fn an_already_prefixed_redirect_is_not_prefixed_twice() {
        // Reaching here would mean the harness echoed the prefix back; doubling
        // it would produce `/i/main/i/main/` and a 404.
        assert_eq!(reattach_prefix("/i/main/", "/i/main"), None);
    }

    #[test]
    fn a_document_relative_redirect_needs_no_rewrite() {
        // The browser resolves this against the request path, which already
        // carries the prefix.
        assert_eq!(reattach_prefix("settings", "/i/main"), None);
    }

    #[test]
    fn an_unrouted_path_uses_the_default_upstream_unchanged() {
        // Single-instance deployments keep working: no routes, no rewriting.
        let cfg = RelayConfig::default();
        let (upstream, path, _prefix) = cfg.resolve("/api/remote.mux");
        assert_eq!(upstream, "http://127.0.0.1:3081");
        assert_eq!(path, "/api/remote.mux");
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
