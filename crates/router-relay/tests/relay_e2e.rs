//! End-to-end relay tests against a real upstream server.
//!
//! The unit tests in `proxy.rs` cover header filtering in isolation. These
//! tests prove the properties that actually matter for the product, and that
//! no amount of unit testing can establish:
//!
//! - the relay forwards to a live upstream and returns its response;
//! - **streaming responses are not buffered** — the property the live agent UI
//!   depends on, and the one a naive proxy implementation silently breaks;
//! - the root redirect and its headers survive intact;
//! - an unreachable upstream produces a diagnostic, not a hang.
//!
//! Each test starts a real HTTP server on an ephemeral loopback port, points
//! the relay at it, and drives the relay with a real client.

use axum::body::Body;
use axum::routing::get;
use axum::Router;
use http::StatusCode;
use router_relay::{relay_router, RelayConfig, RelayState};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Start a server and return its address plus a shutdown handle.
async fn spawn_upstream(app: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    // Give the listener a moment to start accepting.
    tokio::time::sleep(Duration::from_millis(50)).await;
    addr
}

/// Start the relay pointed at `upstream`, returning its address.
async fn spawn_relay(upstream: SocketAddr) -> SocketAddr {
    let state = RelayState::new(RelayConfig {
        upstream: format!("http://{upstream}"),
        ..RelayConfig::default()
    });
    let app = relay_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    addr
}

/// Perform a raw HTTP/1.1 GET and return the complete response bytes.
///
/// Deliberately raw rather than using a client crate: the tests need to see
/// exactly what arrives on the wire, including chunked framing and headers,
/// which a high-level client would normalize away.
async fn raw_get(addr: SocketAddr, path: &str, extra_headers: &[(&str, &str)]) -> String {
    raw_get_with_host(addr, path, extra_headers, &addr.to_string()).await
}

/// As [`raw_get`], but with an explicit `Host` value.
///
/// `extra_headers` must not contain `Host`; this parameter is the only way to
/// set it, so a test cannot accidentally emit two conflicting Host headers and
/// silently assert nothing.
async fn raw_get_with_host(
    addr: SocketAddr,
    path: &str,
    extra_headers: &[(&str, &str)],
    host: &str,
) -> String {
    assert!(
        !extra_headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("host")),
        "pass Host via the host parameter, not extra_headers"
    );

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (k, v) in extra_headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();

    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut buf)).await;
    String::from_utf8_lossy(&buf).to_string()
}

#[tokio::test]
async fn forwards_a_simple_get() {
    let upstream =
        spawn_upstream(Router::new().route("/hello", get(|| async { "hello from upstream" })))
            .await;
    let relay = spawn_relay(upstream).await;

    let resp = raw_get(relay, "/hello", &[]).await;
    assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
    assert!(resp.contains("hello from upstream"), "got: {resp}");
}

#[tokio::test]
async fn preserves_upstream_status_codes() {
    // A non-2xx must not be rewritten; the harness uses 401/403 meaningfully.
    let upstream = spawn_upstream(
        Router::new().route("/denied", get(|| async { (StatusCode::FORBIDDEN, "nope") })),
    )
    .await;
    let relay = spawn_relay(upstream).await;

    let resp = raw_get(relay, "/denied", &[]).await;
    assert!(resp.starts_with("HTTP/1.1 403"), "got: {resp}");
    assert!(resp.contains("nope"), "got: {resp}");
}

#[tokio::test]
async fn preserves_custom_response_headers() {
    // The token exchange relies on Set-Cookie and Location surviving.
    let upstream = spawn_upstream(Router::new().route(
        "/redirect",
        get(|| async {
            (
                StatusCode::FOUND,
                [
                    ("location", "/"),
                    ("set-cookie", "dsh_session=abc; Path=/; HttpOnly"),
                ],
                "",
            )
        }),
    ))
    .await;
    let relay = spawn_relay(upstream).await;

    let resp = raw_get(relay, "/redirect", &[]).await;
    assert!(resp.contains("HTTP/1.1 302"), "got: {resp}");
    assert!(
        resp.to_ascii_lowercase().contains("location: /"),
        "Location must survive: {resp}"
    );
    assert!(
        resp.to_ascii_lowercase()
            .contains("set-cookie: dsh_session=abc"),
        "Set-Cookie must survive: {resp}"
    );
}

#[tokio::test]
async fn forwards_the_host_header_unchanged() {
    // The harness's browser-trust fence compares Host against Origin. If the
    // relay rewrote Host, the ordinary localhost case would start returning
    // 403 — the single most likely way to break the product.
    let upstream = spawn_upstream(Router::new().route(
        "/echo-host",
        get(|headers: http::HeaderMap| async move {
            headers
                .get("host")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("<none>")
                .to_string()
        }),
    ))
    .await;
    let relay = spawn_relay(upstream).await;

    let resp = raw_get_with_host(relay, "/echo-host", &[], "example.test:3080").await;
    assert!(
        resp.contains("example.test:3080"),
        "the upstream must see the browser's Host, not the relay's: {resp}"
    );
    assert!(
        !resp.contains(&relay.to_string()),
        "the relay must not substitute its own authority: {resp}"
    );
}

#[tokio::test]
async fn streams_without_buffering() {
    // THE critical property. The live agent UI is a server-sent event stream;
    // a relay that buffers would hold every update until the stream closed,
    // turning a live session into a frozen one.
    //
    // The upstream sends one chunk, then waits well beyond the test's read
    // window before sending the next. A buffering relay returns nothing; a
    // streaming relay returns the first chunk promptly.
    let upstream = spawn_upstream(Router::new().route(
        "/stream",
        get(|| async {
            let stream = async_stream::stream! {
                yield Ok::<_, std::io::Error>(axum::body::Bytes::from("chunk-one\n"));
                tokio::time::sleep(Duration::from_millis(1500)).await;
                yield Ok(axum::body::Bytes::from("chunk-two\n"));
            };
            axum::response::Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(stream))
                .unwrap()
        }),
    ))
    .await;
    let relay = spawn_relay(upstream).await;

    // Open the request manually so we can read incrementally.
    let mut stream = tokio::net::TcpStream::connect(relay).await.unwrap();
    stream
        .write_all(
            format!("GET /stream HTTP/1.1\r\nHost: {relay}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();

    let mut first = vec![0u8; 4096];
    let n = tokio::time::timeout(Duration::from_millis(700), stream.read(&mut first))
        .await
        .expect("the first chunk must arrive before the second is sent")
        .unwrap();
    let text = String::from_utf8_lossy(&first[..n]);

    assert!(
        text.contains("chunk-one"),
        "first chunk must be delivered promptly, got: {text}"
    );
    assert!(
        !text.contains("chunk-two"),
        "the second chunk must not have been sent yet"
    );
}

#[tokio::test]
async fn reports_an_unreachable_upstream_instead_of_hanging() {
    // Port 1 on loopback is reserved and refuses connections.
    let relay = spawn_relay("127.0.0.1:1".parse().unwrap()).await;
    let resp = raw_get(relay, "/", &[]).await;

    assert!(
        resp.starts_with("HTTP/1.1 502"),
        "expected a 502, got: {resp}"
    );
    assert!(
        resp.contains("RELAY_UPSTREAM_UNREACHABLE"),
        "the diagnostic must name the cause: {resp}"
    );
}

#[tokio::test]
async fn forwards_post_bodies() {
    let upstream = spawn_upstream(Router::new().route(
        "/echo",
        axum::routing::post(|body: String| async move { format!("got:{body}") }),
    ))
    .await;
    let relay = spawn_relay(upstream).await;

    let mut stream = tokio::net::TcpStream::connect(relay).await.unwrap();
    let body = "prompt=hello";
    let req = format!(
        "POST /echo HTTP/1.1\r\nHost: {relay}\r\nContent-Type: text/plain\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).await.unwrap();

    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut buf)).await;
    let resp = String::from_utf8_lossy(&buf);
    assert!(resp.contains("got:prompt=hello"), "got: {resp}");
}

#[tokio::test]
async fn strips_hop_by_hop_headers_end_to_end() {
    let upstream = spawn_upstream(Router::new().route(
        "/echo-headers",
        get(|headers: http::HeaderMap| async move {
            // Report whether a hop-by-hop header arrived.
            let leaked = ["connection", "keep-alive", "te"]
                .iter()
                .filter(|h| headers.contains_key(**h))
                .count();
            format!("leaked:{leaked}")
        }),
    ))
    .await;
    let relay = spawn_relay(upstream).await;

    let resp = raw_get(
        relay,
        "/echo-headers",
        &[("TE", "trailers"), ("Keep-Alive", "timeout=5")],
    )
    .await;
    assert!(
        resp.contains("leaked:0"),
        "hop-by-hop headers leaked: {resp}"
    );
}
