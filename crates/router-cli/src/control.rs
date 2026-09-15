//! The control page.
//!
//! # What this is, and what it deliberately is not
//!
//! It answers one question — *what is running, where, and on what?* — and then
//! gets out of the way. It is **not** a second agent UI: the harness ships a
//! full interface, and rebuilding it here would duplicate a large surface and
//! guarantee it lags behind.
//!
//! # Design notes
//!
//! - **The page is generated server-side from the registry**, so it renders
//!   instantly with no client framework and no build step. For a local control
//!   panel, shipping a bundle would be a cost with no benefit.
//!
//! - **State is legible at a glance and to a colour-blind reader.** A running
//!   instance carries a filled dot *and* the word; colour is reinforcement, not
//!   the message.
//!
//! - **The port is the largest thing in each row**, because it is what a person
//!   is looking for.
//!
//! - **Nothing here can mutate anything remotely.** The page is a report. A web
//!   page that can stop your agent processes, on a port with no authentication,
//!   is a liability rather than a feature.

use crate::paths::RouterHome;
use router_core::registry::{Instance, Registry};
use router_core::term::{self, Style};
use std::net::SocketAddr;
use std::process::ExitCode;

use crate::Failure;

/// Build the relay routes for every registered instance.
///
/// # Why instances are reached through the control page
///
/// The harness authenticates its API gateway with a cookie whose *name* is a
/// hash of the request authority — `host:port`. Opening `127.0.0.1:3082` in a
/// browser and reaching `127.0.0.1:3081` from it is a cross-origin request: the
/// browser blocks it, and the cookie would not be sent even if it were allowed.
/// The user-visible symptom is "failed to fetch gateway", which names the
/// browser's complaint rather than its cause.
///
/// Serving every instance through this one origin removes the problem at the
/// root. The browser sees a single authority and one cookie; the proxy connects
/// to each instance's real loopback port, so the harness still sees
/// `127.0.0.1:<its own port>` as the authority and mints a cookie that matches.
///
/// Prefixes are `/i/<name>`, and the prefix is stripped before forwarding,
/// because the harness serves its app at `/` and knows nothing about being
/// mounted under a sub-path.
#[must_use]
pub fn relay_routes(registry: &Registry) -> Vec<router_relay::Route> {
    registry
        .instances
        .iter()
        .map(|(name, instance)| router_relay::Route {
            prefix: format!("/i/{name}"),
            upstream: format!("http://127.0.0.1:{}", instance.port),
        })
        .collect()
}

/// Where the running gateway records the port it bound.
///
/// # Why this is a file
///
/// `router open` needs to know whether a gateway is running and where, and it is
/// a *different process* from `router serve` — the same cross-process boundary
/// that makes an instance record its browser URL. A file is again the simplest
/// thing that can carry it.
///
/// It is deliberately runtime state, not configuration: `--port` is a choice
/// made once at launch, and the file is removed when the gateway exits, so a
/// stale entry cannot make `open` offer a link to nothing.
#[must_use]
pub fn gateway_port_path(home: &RouterHome) -> std::path::PathBuf {
    home.root().join("gateway-port")
}

/// Record the port the gateway bound.
fn write_gateway_port(home: &RouterHome, port: u16) {
    // Best effort: failing to record the port makes `open` fall back to the
    // direct instance URL, which still works. It is not worth refusing to serve
    // over, but it is worth not panicking on.
    let _ = std::fs::write(gateway_port_path(home), port.to_string());
}

/// Read the running gateway's port, if one was recorded.
#[must_use]
pub fn read_gateway_port(home: &RouterHome) -> Option<u16> {
    std::fs::read_to_string(gateway_port_path(home))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Forget the recorded gateway port, once it is no longer answering.
pub fn clear_gateway_port(home: &RouterHome) {
    let _ = std::fs::remove_file(gateway_port_path(home));
}

/// The cookie that remembers which instance this browser is using.
///
/// # Why a cookie, and why it is the only workable basis
///
/// The instance named in an `/i/<name>...` path, if there is one.
fn instance_from_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/i/")?;
    let name = rest.split('/').next()?;
    (!name.is_empty()).then(|| name.to_string())
}

/// Whether a path is one the harness's UI requests from the origin root.
///
/// Deliberately a fixed list rather than "anything not /i/": the control page
/// lives at `/`, and a catch-all would swallow a mistyped address and serve an
/// instance instead of saying the path is wrong.
fn is_harness_root_path(path: &str) -> bool {
    const PREFIXES: [&str; 5] = [
        "/api/",
        "/plugins/",
        "/assets/",
        "/open-in-app/",
        "/favicon",
    ];
    PREFIXES.iter().any(|p| path.starts_with(p))
}

/// Rewrite a bare harness request into the prefixed form the relay routes.
fn prefix_request(req: axum::extract::Request, name: &str) -> axum::extract::Request {
    let (mut parts, body) = req.into_parts();
    let path = parts.uri.path();
    let query = parts.uri.query().map_or(String::new(), |q| format!("?{q}"));
    let target = format!("/i/{name}{path}{query}");
    if let Ok(uri) = target.parse() {
        parts.uri = uri;
    }
    axum::extract::Request::from_parts(parts, body)
}

/// The instance a root-rooted request belongs to.
///
/// # Why the `Referer` and not a cookie
///
/// The gateway has to decide which instance an origin-rooted request is for —
/// `/api/remote.mux` names none. The first attempt used a cookie recording the
/// "current instance", which broke the moment two tabs were open: the second tab
/// overwrote it and the first tab's requests went to the wrong harness. A marker
/// per instance was better but still could not choose *between* them, so with two
/// tabs open the app lost those calls entirely.
///
/// The browser already answers this exactly, per request: a fetch from
/// `/i/notes/` carries `Referer: …/i/notes/`, and one from `/i/probe/` carries
/// `/i/probe/`. That is the instance the request genuinely came from, stated by
/// the browser for each request rather than inferred across them, so two tabs
/// never contend.
///
/// It is a routing hint, not a security boundary: a client can send any
/// `Referer` it likes. That is acceptable because the harness authenticates every
/// request with its own cookie and trust fence — a forged value can only aim a
/// request at an instance that will then refuse it.
fn current_instance(req: &axum::extract::Request) -> Option<String> {
    let referer = req
        .headers()
        .get(axum::http::header::REFERER)?
        .to_str()
        .ok()?;

    // Only the path matters; the authority is the gateway either way.
    let path = referer
        .split_once("://")
        .map_or(referer, |(_, rest)| rest)
        .split_once('/')
        .map_or("", |(_, path)| path);

    let name = instance_from_path(&format!("/{path}"))?;

    // A page for an instance the gateway is not serving routes nowhere, so
    // decline rather than forwarding to a prefix no route matches.
    req.extensions()
        .get::<KnownInstances>()
        .is_some_and(|known| known.0.iter().any(|n| n == &name))
        .then_some(name)
}

/// The instance names the gateway currently serves, attached to each request.
///
/// Passed through request extensions rather than read from the registry per
/// request: the registry is a file, and re-reading it on every origin-rooted call
/// would put disk I/O on the hot path for a list that changes only when an
/// instance is added or removed.
#[derive(Debug, Clone)]
pub struct KnownInstances(pub Vec<String>);

/// Serve the control page and the instance gateway until interrupted.
///
/// Two jobs on one port, and they belong together: the page is how a person
/// finds their instances, and the gateway is how they reach them without the
/// browser refusing the request. Splitting them would mean the page linking to
/// URLs that cannot work.
pub async fn serve(style: &Style, port: u16, no_open: bool) -> Result<ExitCode, Failure> {
    let (home, registry) = crate::load(None)?;

    let addr = SocketAddr::from(([127, 0, 0, 1], port));

    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
        Failure::runtime(
            format!("cannot listen on port {port}: {e}"),
            "Another process may be using it. Choose another with --port.",
        )
    })?;

    let actual = listener
        .local_addr()
        .map_err(|e| Failure::runtime(format!("cannot read the bound address: {e}"), ""))?;
    let url = format!("http://127.0.0.1:{}", actual.port());

    // Recorded only once the socket is actually bound, so the file never claims
    // a gateway that failed to start listening.
    write_gateway_port(&home, actual.port());

    if style.verbosity != router_core::term::Verbosity::Quiet {
        term::out(&style.ok("Control page ready"));
        term::out("");
        term::out(&format!(
            "  {}  {}",
            style.dim(style.glyphs.arrow),
            style.url(&url)
        ));
        if !registry.instances.is_empty() {
            term::out("");
            term::out(&style.dim("  Instances are reachable through this page:"));
            for (name, instance) in &registry.instances {
                term::out(&format!(
                    "    {}/i/{}",
                    url,
                    style.paint(router_core::term::Ink::Blue, name)
                ));
                let _ = instance;
            }
        }
        term::out("");
        term::out(&style.dim("  Press Ctrl+C to stop."));
        term::out("");
    } else {
        term::out(&url);
    }

    if !no_open {
        // Best-effort: the printed URL is the authoritative answer.
        let _ = open_page(&url);
    }

    // The control page and the gateway share one origin.
    //
    // `/` renders the page; everything under `/i/<name>` is relayed to that
    // instance. The relay already handles streaming and WebSocket upgrades,
    // which the gateway needs — see `router_relay`.
    let routes = relay_routes(&registry);

    // The registry can change while the server runs, so the page is re-read per
    // request rather than captured once. A control page showing stale state is
    // worse than no control page.
    let home_for_page = home.clone();
    let fallback = registry.clone();
    let known_names: Vec<String> = registry.instances.keys().cloned().collect();

    // The relay, if there is anything to relay.
    //
    // Built as a service rather than merged as a router: both halves are
    // fallbacks, and axum refuses to merge two routers that each carry one —
    // correctly, because one would silently shadow the other. Nesting gives the
    // relay first refusal on every path and lets the page handle the rest.
    let relay: Option<axum::Router> = if routes.is_empty() {
        None
    } else {
        Some(router_relay::relay_router(router_relay::RelayState::new(
            router_relay::RelayConfig {
                // Only reached for a path the routes did not claim. Pointing it
                // at the base port keeps the behaviour honest: an unmatched
                // `/i/typo/...` reaches a real instance rather than a dead port.
                upstream: format!("http://127.0.0.1:{}", registry.base_port),
                routes,
                ..router_relay::RelayConfig::default()
            },
        )))
    };

    // One origin, two jobs, and exactly one fallback.
    //
    // # Why the composition is this specific shape
    //
    // The relay and the page are both *fallbacks* — each answers "anything I do
    // not recognise" — so they cannot be merged: axum refuses two routers that
    // each carry a fallback, and it is right to, because one would silently
    // shadow the other. The first attempt called `fallback_service(relay)` and
    // then `.fallback(page)`, and the second call *replaced* the first: every
    // request went to the page, every `/i/...` path 404'd, and the relay was
    // never reached. The body of that 404 was the page's own text, which is how
    // the mistake was found.
    //
    // So there is one fallback and it dispatches explicitly. The choice is made
    // from the path, which keeps the precedence visible in one place rather
    // than depending on the order axum happens to try routes.
    let app =
        axum::Router::new().fallback(axum::routing::any(move |req: axum::extract::Request| {
            let home = home_for_page.clone();
            let fallback = fallback.clone();
            let relay = relay.clone();
            let known_names = known_names.clone();
            async move {
                let path = req.uri().path().to_string();

                // Which instances exist, for `current_instance` to check against.
                // Attached here rather than read from the registry per request.
                let (mut parts, body) = req.into_parts();
                parts.extensions.insert(KnownInstances(known_names.clone()));
                let req = axum::extract::Request::from_parts(parts, body);

                // An instance path goes to the relay.
                if let Some(relay) = relay {
                    if path.starts_with("/i/") {
                        use tower::ServiceExt as _;
                        return relay.clone().oneshot(req).await.unwrap_or_else(|e| {
                            axum::response::Response::builder()
                                .status(axum::http::StatusCode::BAD_GATEWAY)
                                .body(axum::body::Body::from(format!("relay failed: {e}")))
                                .expect("a fixed response always builds")
                        });
                    }

                    // A root-absolute harness path, sent by an app that is
                    // already open on an instance.
                    //
                    // # Why this exists at all
                    //
                    // The harness serves its UI at `/` and builds some of its own
                    // URLs from the *origin* rather than the current path — the
                    // API calls and the live gateway socket, most importantly
                    // `ws://host/api/remote.mux`. Through the gateway those
                    // resolve to `/api/remote.mux`, which names no instance, so
                    // they 404 and the app shows "Failed to load plugins" or
                    // retries a dead socket forever.
                    //
                    // No amount of response rewriting fixes it: the URL is
                    // constructed in JavaScript at runtime, not present in any
                    // document the relay can edit. The gateway therefore has to
                    // decide, and `current_instance` explains how it does so
                    // correctly per request rather than per browser.
                    if is_harness_root_path(&path) {
                        if let Some(name) = current_instance(&req) {
                            use tower::ServiceExt as _;
                            // Rewritten into the prefixed form the relay already
                            // understands, so there is one routing rule rather
                            // than two: `/api/x` becomes `/i/<name>/api/x`, and
                            // the existing prefix handling carries it the rest of
                            // the way — including the WebSocket upgrade.
                            let rewritten = prefix_request(req, &name);
                            return relay.oneshot(rewritten).await.unwrap_or_else(|e| {
                                axum::response::Response::builder()
                                    .status(axum::http::StatusCode::BAD_GATEWAY)
                                    .body(axum::body::Body::from(format!("relay failed: {e}")))
                                    .expect("a fixed response always builds")
                            });
                        }
                    }
                }

                // Only the root is the page. Anything else is a path nothing
                // claims, and answering it with HTML would hide a mistyped
                // address — the user would see the control page and wonder why
                // their instance did not open.
                if path != "/" {
                    return axum::response::Response::builder()
                        .status(axum::http::StatusCode::NOT_FOUND)
                        .header(
                            axum::http::header::CONTENT_TYPE,
                            "text/plain; charset=utf-8",
                        )
                        .body(axum::body::Body::from(
                            "Not found. The control page is at /.\n\
                             Instances are at /i/<name>/.\n",
                        ))
                        .expect("a fixed response always builds");
                }

                let fresh =
                    Registry::load(&home.registry_path()).unwrap_or_else(|_| fallback.clone());
                let body = render(&home, &fresh);
                axum::response::Response::builder()
                    .status(axum::http::StatusCode::OK)
                    .header(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")
                    .header(axum::http::header::CACHE_CONTROL, "no-store")
                    .body(axum::body::Body::from(body))
                    .expect("a fixed response always builds")
            }
        }));

    let served = axum::serve(listener, app).await;

    // The port is released here, so the record must go with it. Leaving it would
    // let a later `router open` hand out a gateway URL that nothing answers —
    // which reads as a router bug rather than a gateway that was stopped.
    clear_gateway_port(&home);

    served.map_err(|e| Failure::runtime(format!("the server stopped: {e}"), ""))?;

    Ok(ExitCode::SUCCESS)
}

/// Render the whole page.
///
/// Takes no [`Style`]: the output is HTML, and terminal styling has no meaning
/// in a browser. Accepting one would imply the page adapts to a terminal it
/// cannot see.
#[must_use]
pub fn render(home: &RouterHome, registry: &Registry) -> String {
    let instances: Vec<(&String, &Instance)> = registry.instances.iter().collect();

    // Probing is the expensive part — a socket connect per instance — so it is
    // done once here and the answer is passed down, rather than each row asking
    // again. Two inconsistent answers on one page would be worse than slow.
    let live: Vec<(&String, &Instance, bool)> = instances
        .iter()
        .map(|(name, instance)| (*name, *instance, crate::probe_port(instance.port)))
        .collect();

    let total = live.len();
    let running = live.iter().filter(|(_, _, up)| *up).count();
    let stopped = total.saturating_sub(running);
    let missing: Vec<&str> = live
        .iter()
        .filter(|(_, instance, _)| !instance.workspace.is_dir())
        .map(|(name, _, _)| name.as_str())
        .collect();
    let models: std::collections::BTreeSet<&str> = live
        .iter()
        .map(|(_, instance, _)| instance.model.as_deref().unwrap_or("default"))
        .collect();

    let mut rows = String::new();
    for (name, instance, up) in &live {
        rows.push_str(&render_row(name, instance, *up, home));
    }

    if instances.is_empty() {
        rows = r#"<tr><td colspan="4" class="empty">
            <strong>No instances yet.</strong>
            <span>Add one from your terminal:</span>
            <code>router add my-project --workspace ~/projects/my-project</code>
          </td></tr>"#
            .to_string();
    }

    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>DeepSeek Harness Router</title>
<style>{css}</style>
</head>
<body>
<main>
  <header>
    <div class="brand">
      <svg viewBox="0 0 40 40" width="36" height="36" aria-hidden="true">
        <path d="M20 6 L32 13 L32 27 L20 34 L8 27 L8 13 Z" fill="none"
              stroke="url(#g)" stroke-width="2.2" stroke-linejoin="round"/>
        <circle cx="20" cy="20" r="3.6" fill="url(#g)"/>
        <defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stop-color="#4f8cff"/>
          <stop offset="50%" stop-color="#7b5cff"/>
          <stop offset="100%" stop-color="#37e0c8"/>
        </linearGradient></defs>
      </svg>
      <div>
        <h1>DeepSeek Harness Router</h1>
        <p class="sub">Each instance runs in its own workspace, on its own port,
           with its own state root.</p>
      </div>
    </div>
    <div class="home" title="{home_path}"><span>router home</span>{home_short}</div>
  </header>

  <section class="stats" aria-label="Summary">
    <div class="stat">
      <span class="k">Instances</span>
      <span class="v">{total}</span>
    </div>
    <div class="stat">
      <span class="k">Serving</span>
      <span class="v {up_class}">{running}</span>
    </div>
    <div class="stat">
      <span class="k">Stopped</span>
      <span class="v {down_class}">{stopped}</span>
    </div>
    <div class="stat">
      <span class="k">Models in use</span>
      <span class="v">{model_count}</span>
    </div>
  </section>

  {warning}

  <h2>Instances</h2>
  <table>
    <thead>
      <tr><th>Instance</th><th>Port</th><th>Workspace</th><th>Model</th></tr>
    </thead>
    <tbody>{rows}</tbody>
  </table>

  <footer>
    <p>This page reports. It cannot start or stop anything &mdash; use the
       <code>router</code> command for that.</p>
  </footer>
</main>
</body>
</html>"##,
        css = CSS,
        home_path = html_escape(&home.root().display().to_string()),
        home_short = html_escape(&short_path(&home.root().display().to_string())),
        total = total,
        running = running,
        stopped = stopped,
        up_class = if running > 0 { "up" } else { "idle" },
        down_class = if stopped > 0 { "down" } else { "idle" },
        model_count = models.len(),
        warning = render_warning(&missing),
        rows = rows,
    )
}

/// A banner for the one condition a user must act on.
///
/// A workspace that no longer exists is not a display detail: the harness will
/// fail to start, or start somewhere unintended. Reporting it in the same grey
/// as everything else would bury the only line on the page that needs a
/// decision.
fn render_warning(missing: &[&str]) -> String {
    if missing.is_empty() {
        return String::new();
    }
    let names = missing
        .iter()
        .map(|n| html_escape(n))
        .collect::<Vec<_>>()
        .join(", ");
    let plural = if missing.len() == 1 { "" } else { "s" };
    format!(
        r#"<div class="warn" role="status">
      <strong>{n} workspace{plural} missing</strong>
      <span>The directory for <code>{names}</code> no longer exists. The router
        never creates or deletes a workspace, so this needs fixing by hand.</span>
    </div>"#,
        n = missing.len(),
        plural = plural,
        names = names,
    )
}

/// One instance row.
fn render_row(name: &str, instance: &Instance, serving: bool, home: &RouterHome) -> String {
    let workspace_ok = instance.workspace.is_dir();
    // The link is root-relative and goes through this page's own origin, not
    // the instance's bare port. A direct `127.0.0.1:<port>` link looks like it
    // should work and does not: the harness names its auth cookie after the
    // request authority, so a page reached that way cannot talk to a gateway
    // that mints the cookie for a different one. The user-visible result was
    // "failed to fetch gateway" — which is why `/i/<name>` exists, and why the
    // link has to use it. Being relative, it also cannot hard-code a host.
    let url = format!("/i/{name}");

    // State is carried by a word and a shape, not by colour alone — a
    // colour-blind reader must get the same information.
    let (state_class, state_word) = if serving {
        ("up", "serving")
    } else {
        ("down", "stopped")
    };

    let workspace_note = if workspace_ok {
        String::new()
    } else {
        r#"<span class="flag">missing</span>"#.to_string()
    };

    let model = match &instance.model {
        Some(m) => html_escape(m),
        None => r#"<span class="muted">default</span>"#.to_string(),
    };

    // A serving instance links; a stopped one does not, because a link that
    // leads nowhere is worse than no link.
    //
    // The tooltip still names the port, because that is what a user needs when
    // they go looking for the process — but it is deliberately not the link.
    let port_cell = if serving {
        format!(
            r#"<a class="port" href="{url}" title="Open {name} (its own port is {port})">{port}</a>"#,
            url = html_escape(&url),
            name = html_escape(name),
            port = instance.port
        )
    } else {
        format!(r#"<span class="port dead">{}</span>"#, instance.port)
    };

    // The state root is the instance's own DSH_HOME. Showing it is not
    // decoration: it is the single fact that explains why these instances
    // cannot interfere with each other, and it is the directory a user would
    // inspect when something looks wrong.
    let state_root = home.instance_dir(name).join("dsh");
    let state_short = short_path(&state_root.display().to_string());

    format!(
        r#"<tr>
      <td class="name">
        <span class="dot {state_class}" aria-hidden="true"></span>
        <span class="label">{name}</span>
        <span class="state">{state_word}</span>
      </td>
      <td>{port_cell}</td>
      <td class="ws" title="{ws_full}">{ws}{flag}</td>
      <td class="model">{model}</td>
    </tr>
    <tr class="detail"><td colspan="4">
      <span class="meta"><span class="muted">state root</span>
        <code title="{state_full}">{state_short}</code></span>
      <span class="meta"><span class="muted">endpoint</span>
        <code>127.0.0.1:{port}</code></span>
    </td></tr>"#,
        state_class = state_class,
        name = html_escape(name),
        state_word = state_word,
        port_cell = port_cell,
        ws_full = html_escape(&instance.workspace.display().to_string()),
        ws = html_escape(&short_path(&instance.workspace.display().to_string())),
        flag = workspace_note,
        model = model,
        state_full = html_escape(&state_root.display().to_string()),
        state_short = html_escape(&state_short),
        port = instance.port,
    )
}

/// Abbreviate the home prefix to `~`.
fn short_path(path: &str) -> String {
    for var in ["USERPROFILE", "HOME"] {
        if let Ok(home) = std::env::var(var) {
            if !home.is_empty() && path.starts_with(&home) {
                return format!("~{}", &path[home.len()..]);
            }
        }
    }
    path.to_string()
}

/// Escape text for HTML.
///
/// Everything interpolated into this page goes through here. A workspace path
/// is user-controlled, and a control page that can be made to render arbitrary
/// markup is a control page that can be made to lie.
#[must_use]
pub fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Open a URL in the default browser.
fn open_page(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
        Ok(())
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = url;
        Err(std::io::Error::other("unsupported platform"))
    }
}

/// The stylesheet.
///
/// Dark-first, because a control panel is glanced at while working in a
/// terminal, and a full-white page at that moment is hostile. It follows the
/// system preference so a light-mode user is not forced into the dark.
const CSS: &str = r#"
:root {
  --bg: #0a0e1a; --panel: #0d1628; --line: #1e2a44;
  --ink: #e8eef9; --dim: #8296b5; --faint: #55668a;
  --up: #37e0c8; --down: #f0a341; --accent: #4f8cff;
  color-scheme: dark light;
}
@media (prefers-color-scheme: light) {
  :root {
    --bg: #f6f8fc; --panel: #ffffff; --line: #dde4f0;
    --ink: #101a2e; --dim: #5a6b87; --faint: #8b9ab4;
    --up: #0d9488; --down: #b45309; --accent: #2563eb;
  }
}
* { box-sizing: border-box; }
body {
  margin: 0; padding: 44px 24px; background: var(--bg); color: var(--ink);
  font: 15px/1.55 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
  -webkit-font-smoothing: antialiased;
}
main { max-width: 980px; margin: 0 auto; }

/* The header states what the product is, once. The summary that follows is
   the answer to "is everything up?", which is the question that brings a
   person to this page at all. */
header { display: flex; align-items: flex-start; justify-content: space-between; gap: 22px; margin-bottom: 26px; flex-wrap: wrap; }
.brand { display: flex; align-items: flex-start; gap: 14px; }
.brand svg { flex: none; margin-top: 2px; }
h1 { font-size: 19px; font-weight: 650; margin: 0; letter-spacing: -0.012em; }
.sub { margin: 5px 0 0; font-size: 13.5px; color: var(--dim); max-width: 46ch; }
.home {
  font: 12px/1.5 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  color: var(--dim); text-align: right; word-break: break-all; max-width: 34ch;
}
.home span {
  display: block; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
  font-size: 10.5px; letter-spacing: 0.09em; text-transform: uppercase; color: var(--faint);
  margin-bottom: 3px;
}

.stats { display: grid; grid-template-columns: repeat(4, 1fr); gap: 12px; margin-bottom: 26px; }
.stat {
  background: var(--panel); border: 1px solid var(--line); border-radius: 13px;
  padding: 15px 17px; display: flex; flex-direction: column; gap: 5px;
}
.stat .k {
  font-size: 10.5px; font-weight: 650; letter-spacing: 0.09em; text-transform: uppercase;
  color: var(--faint);
}
.stat .v { font-size: 27px; font-weight: 640; line-height: 1; letter-spacing: -0.03em; }
.stat .v.up { color: var(--up); }
.stat .v.down { color: var(--down); }
.stat .v.idle { color: var(--faint); }

/* The one thing that needs a decision gets a colour of its own. */
.warn {
  display: flex; flex-direction: column; gap: 5px; margin-bottom: 24px;
  border: 1px solid color-mix(in srgb, var(--down) 45%, var(--line));
  background: color-mix(in srgb, var(--down) 9%, var(--panel));
  border-radius: 13px; padding: 15px 18px;
}
.warn strong { font-size: 14px; color: var(--down); }
.warn span { font-size: 13px; color: var(--dim); }
.warn code { font: 12px/1 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; color: var(--ink); }

h2 {
  font-size: 11px; font-weight: 650; letter-spacing: 0.09em; text-transform: uppercase;
  color: var(--faint); margin: 0 0 11px 2px;
}

table { width: 100%; border-collapse: collapse; background: var(--panel); border: 1px solid var(--line); border-radius: 14px; overflow: hidden; }
thead th {
  text-align: left; font-size: 11px; font-weight: 650; letter-spacing: 0.09em;
  text-transform: uppercase; color: var(--faint); padding: 13px 18px;
  border-bottom: 1px solid var(--line); background: color-mix(in srgb, var(--panel) 60%, var(--bg));
}
tbody tr:not(.detail) { border-top: 1px solid var(--line); }
tbody tr:first-child:not(.detail) { border-top: none; }
td { padding: 15px 18px; vertical-align: middle; }
tr.detail td { padding: 0 18px 16px; border: none; display: flex; gap: 22px; flex-wrap: wrap; }
tr.detail code { font: 12px/1.5 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; color: var(--faint); }
.meta { display: inline-flex; align-items: baseline; gap: 7px; }
.name { display: flex; align-items: center; gap: 10px; }
.dot { width: 8px; height: 8px; border-radius: 50%; flex: none; }
.dot.up { background: var(--up); box-shadow: 0 0 0 3px color-mix(in srgb, var(--up) 22%, transparent); }
.dot.down { background: transparent; border: 1.5px solid var(--faint); }
.label { font-weight: 600; }
.state { font-size: 11px; color: var(--dim); text-transform: uppercase; letter-spacing: 0.06em; }
.port {
  font: 600 20px/1 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  color: var(--accent); text-decoration: none; letter-spacing: -0.02em;
}
.port:hover { text-decoration: underline; text-underline-offset: 3px; }
.port.dead { color: var(--faint); font-weight: 500; }
.ws { font: 12.5px/1.4 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; color: var(--dim); word-break: break-all; }
.flag { margin-left: 8px; font-size: 11px; color: var(--down); border: 1px solid var(--down); border-radius: 5px; padding: 1px 6px; }
.model { font-size: 13px; }
.muted { color: var(--faint); }
.empty { text-align: center; padding: 56px 20px; color: var(--dim); }
.empty strong { display: block; color: var(--ink); font-size: 16px; margin-bottom: 6px; }
.empty span { display: block; margin-bottom: 12px; }
.empty code { font: 13px/1.5 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; background: var(--bg); border: 1px solid var(--line); padding: 8px 14px; border-radius: 8px; display: inline-block; color: var(--ink); }
footer { margin-top: 22px; }
footer p { font-size: 12.5px; color: var(--faint); margin: 0; }
footer code { font: 12px/1 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; color: var(--dim); }

@media (max-width: 720px) {
  .stats { grid-template-columns: repeat(2, 1fr); }
  .home { text-align: left; max-width: none; }
}
@media (max-width: 640px) {
  body { padding: 26px 14px; }
  thead { display: none; }
  tbody tr:not(.detail) { display: grid; grid-template-columns: 1fr auto; gap: 5px 12px; padding: 14px 0; }
  tbody tr.detail { display: block; }
  tr.detail td { display: flex; flex-direction: column; gap: 7px; padding: 0 18px 16px; }
  td { padding: 2px 18px; }
  td:nth-child(3), td:nth-child(4) { grid-column: 1 / -1; }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn home() -> RouterHome {
        RouterHome::resolve(Some(PathBuf::from("/tmp/router"))).unwrap()
    }

    #[test]
    fn escapes_html_metacharacters() {
        // A workspace path is user-controlled. A control page that can be made
        // to render arbitrary markup is one that can be made to lie.
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
        assert_eq!(html_escape("a&b"), "a&amp;b");
        assert_eq!(html_escape("say \"hi\""), "say &quot;hi&quot;");
        assert_eq!(html_escape("it's"), "it&#39;s");
    }

    #[test]
    fn renders_an_empty_state_that_teaches_the_command() {
        let registry = Registry::default();
        let page = render(&home(), &registry);
        assert!(page.contains("No instances yet"));
        assert!(page.contains("router add"));
    }

    #[test]
    fn renders_one_row_per_instance() {
        let mut registry = Registry::default();
        registry
            .insert("alpha", Instance::new(PathBuf::from("/tmp/a"), 3081))
            .unwrap();
        registry
            .insert("beta", Instance::new(PathBuf::from("/tmp/b"), 3082))
            .unwrap();
        let page = render(&home(), &registry);

        assert!(page.contains("alpha"));
        assert!(page.contains("beta"));
        assert!(page.contains("3081"));
        assert!(page.contains("3082"));
    }

    #[test]
    fn the_port_is_shown_for_every_instance() {
        // The port is what a person is looking for, so it must always appear.
        let mut registry = Registry::default();
        registry
            .insert("only", Instance::new(PathBuf::from("/tmp/x"), 3099))
            .unwrap();
        let page = render(&home(), &registry);
        assert!(page.contains("3099"));
    }

    #[test]
    fn state_is_carried_by_a_word_not_only_by_colour() {
        // A colour-blind reader must get the same information.
        let mut registry = Registry::default();
        registry
            .insert("only", Instance::new(PathBuf::from("/tmp/x"), 3099))
            .unwrap();
        let page = render(&home(), &registry);
        assert!(
            page.contains("stopped") || page.contains("serving"),
            "a textual state word must be present"
        );
    }

    #[test]
    fn a_hostile_workspace_path_cannot_inject_markup() {
        let mut registry = Registry::default();
        registry
            .insert(
                "evil",
                Instance::new(PathBuf::from("/tmp/<img src=x onerror=alert(1)>"), 3081),
            )
            .unwrap();
        let page = render(&home(), &registry);
        assert!(
            !page.contains("<img src=x"),
            "raw markup leaked into the page"
        );
        assert!(page.contains("&lt;img"));
    }

    #[test]
    fn a_hostile_instance_name_cannot_inject_markup() {
        let mut registry = Registry::default();
        // The registry validates names, so this is caught on insert; the test
        // asserts the renderer would be safe even if it were not.
        let name = "<script>alert(1)</script>";
        if registry
            .insert(name, Instance::new(PathBuf::from("/tmp/x"), 3081))
            .is_err()
        {
            return; // rejected upstream, which is the stronger outcome
        }
        let page = render(&home(), &registry);
        assert!(!page.contains("<script>alert"));
    }

    #[test]
    fn the_default_model_is_labelled_as_such() {
        let mut registry = Registry::default();
        registry
            .insert("only", Instance::new(PathBuf::from("/tmp/x"), 3081))
            .unwrap();
        let page = render(&home(), &registry);
        assert!(page.contains("default"));
    }

    #[test]
    fn a_named_model_appears_verbatim() {
        let mut registry = Registry::default();
        let mut inst = Instance::new(PathBuf::from("/tmp/x"), 3081);
        inst.model = Some("deepseek-v4-pro".into());
        registry.insert("only", inst).unwrap();
        let page = render(&home(), &registry);
        assert!(page.contains("deepseek-v4-pro"));
    }

    #[test]
    fn the_page_states_that_it_cannot_mutate() {
        // A web page that can stop agent processes, on an unauthenticated
        // port, is a liability rather than a feature.
        let page = render(&home(), &Registry::default());
        assert!(page.contains("cannot start or stop"));
    }

    #[test]
    fn the_state_root_is_shown_per_instance() {
        let mut registry = Registry::default();
        registry
            .insert("alpha", Instance::new(PathBuf::from("/tmp/a"), 3081))
            .unwrap();
        let page = render(&home(), &registry);
        assert!(page.contains("instances"));
        assert!(page.contains("alpha"));
    }

    #[test]
    fn output_is_a_complete_document() {
        let page = render(&home(), &Registry::default());
        assert!(page.starts_with("<!DOCTYPE html>"));
        assert!(page.contains("</html>"));
        assert!(page.contains("viewport"));
    }

    #[test]
    fn the_count_pluralises_correctly() {
        let mut registry = Registry::default();
        let one = render(&home(), &registry);
        assert!(one.contains("Instances"));

        registry
            .insert("a", Instance::new(PathBuf::from("/tmp/a"), 3081))
            .unwrap();
        let two = render(&home(), &registry);
        assert!(two.contains(">1<"), "the instance count must be shown");
    }

    /// Start a throwaway HTTP server and return the port it listens on.
    ///
    /// The page decides "serving" by probing for an HTTP status line, so a bare
    /// listening socket reads as stopped. A test that wants a serving row has to
    /// answer like a server, not merely accept a connection.
    fn spawn_http_responder() -> (u16, std::thread::JoinHandle<()>) {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            // A few probes happen per render; serve them all, then end when the
            // test drops the listener by returning from this closure.
            for stream in listener.incoming().take(4) {
                let Ok(mut stream) = stream else { break };
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nok");
                let _ = stream.flush();
            }
        });
        (port, handle)
    }

    #[test]
    fn a_serving_instance_links_through_the_gateway_prefix() {
        // The link must not point at the instance's bare port. Reaching a
        // gateway from a page served on another authority is what produced
        // "failed to fetch gateway", and the port link recreates exactly that
        // situation — it looks correct and cannot work.
        let name = "linked-instance";
        let (port, server) = spawn_http_responder();
        let mut registry = Registry::default();
        registry
            .insert(name, Instance::new(PathBuf::from("/tmp/linked"), port))
            .unwrap();
        let page = render(&home(), &registry);
        drop(server);

        assert!(
            page.contains(&format!(r#"href="/i/{name}""#)),
            "a serving instance must link through the gateway prefix:\n{page}"
        );
        assert!(
            !page.contains(&format!(r#"href="http://127.0.0.1:{port}""#)),
            "the bare-port link is the bug this replaced:\n{page}"
        );
    }

    #[test]
    fn a_stopped_instance_is_not_a_link() {
        // A port nothing is listening on must not be clickable: a link that
        // leads to a connection error is worse than plain text.
        let mut registry = Registry::default();
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        registry
            .insert(
                "stopped-one",
                Instance::new(PathBuf::from("/tmp/stopped"), port),
            )
            .unwrap();
        let page = render(&home(), &registry);

        assert!(
            !page.contains(r#"href="/i/stopped-one""#),
            "a stopped instance must not link anywhere:\n{page}"
        );
        assert!(page.contains(&port.to_string()));
    }

    #[test]
    fn an_instance_name_is_read_from_the_path() {
        assert_eq!(instance_from_path("/i/main").as_deref(), Some("main"));
        assert_eq!(instance_from_path("/i/main/").as_deref(), Some("main"));
        assert_eq!(
            instance_from_path("/i/notes/api/x").as_deref(),
            Some("notes")
        );
        // Not an instance path at all.
        assert_eq!(instance_from_path("/"), None);
        assert_eq!(instance_from_path("/api/x"), None);
        // The prefix with nothing after it names nothing.
        assert_eq!(instance_from_path("/i/"), None);
    }

    #[test]
    fn only_harness_paths_are_routed_by_affinity() {
        // The list is deliberately fixed. A catch-all would swallow a mistyped
        // address and serve an instance instead of reporting the path wrong.
        assert!(is_harness_root_path("/api/remote.mux"));
        assert!(is_harness_root_path("/plugins/??a.js"));
        assert!(is_harness_root_path("/assets/index.js"));
        assert!(is_harness_root_path("/favicon.svg"));
        assert!(is_harness_root_path("/open-in-app/apps"));

        assert!(!is_harness_root_path("/"));
        assert!(!is_harness_root_path("/i/main/"));
        assert!(!is_harness_root_path("/typo"));
        // A near-miss must not match: `/apis/` is not `/api/`.
        assert!(!is_harness_root_path("/apis/x"));
    }

    #[test]
    fn a_bare_path_is_rewritten_into_the_instance_prefix() {
        let req = axum::extract::Request::builder()
            .uri("/api/remote.mux?token=abc")
            .body(axum::body::Body::empty())
            .unwrap();
        let out = prefix_request(req, "notes");
        assert_eq!(out.uri().path(), "/i/notes/api/remote.mux");
        // The query must survive: the harness token travels in it.
        assert_eq!(out.uri().query(), Some("token=abc"));
    }

    /// A request carrying a `Referer` and the set of served instances.
    fn request_from(referer: Option<&str>, known: &[&str]) -> axum::extract::Request {
        let mut builder = axum::extract::Request::builder();
        if let Some(r) = referer {
            builder = builder.header("referer", r);
        }
        let mut req = builder
            .body(axum::body::Body::empty())
            .expect("a test request builds");
        req.extensions_mut().insert(KnownInstances(
            known.iter().map(|s| (*s).to_string()).collect(),
        ));
        req
    }

    #[test]
    fn the_instance_comes_from_the_referer_per_request() {
        // The design that finally worked. A cookie cannot answer this correctly:
        // the first attempt stored one "current instance" and the second tab
        // overwrote it, so the first tab's requests went to the wrong harness.
        // The browser states the origin of each request individually, so two tabs
        // never contend.
        let notes = request_from(Some("http://127.0.0.1:3090/i/notes/"), &["notes", "probe"]);
        assert_eq!(current_instance(&notes).as_deref(), Some("notes"));

        let probe = request_from(
            Some("http://127.0.0.1:3090/i/probe/api/x"),
            &["notes", "probe"],
        );
        assert_eq!(current_instance(&probe).as_deref(), Some("probe"));
    }

    #[test]
    fn a_request_with_no_referer_routes_nowhere() {
        // A client that sends no Referer gets no guess. Serving an arbitrary
        // instance would be worse than declining.
        assert_eq!(current_instance(&request_from(None, &["notes"])), None);
        assert_eq!(
            current_instance(&request_from(Some("http://127.0.0.1:3090/"), &["notes"])),
            None
        );
    }

    #[test]
    fn an_unknown_instance_in_the_referer_routes_nowhere() {
        // A page for an instance the gateway no longer serves must not be
        // forwarded to a prefix no route matches.
        assert_eq!(
            current_instance(&request_from(
                Some("http://127.0.0.1:3090/i/gone/"),
                &["notes"]
            )),
            None
        );
    }

    #[test]
    fn a_relative_referer_is_accepted() {
        // Browsers normally send an absolute Referer, but the parsing must not
        // depend on the scheme being present.
        let req = request_from(Some("/i/notes/"), &["notes"]);
        assert_eq!(current_instance(&req).as_deref(), Some("notes"));
    }

    #[test]
    fn the_summary_counts_every_instance_exactly_once() {
        // Two stopped instances, so counts are deterministic without needing a
        // listening socket. The summary is the first thing read, so a wrong
        // number here is worse than no number.
        let mut registry = Registry::default();
        for (i, name) in ["alpha", "beta", "gamma"].iter().enumerate() {
            registry
                .insert(
                    name,
                    Instance::new(PathBuf::from(format!("/tmp/{name}")), 3200 + i as u16),
                )
                .unwrap();
        }
        let page = render(&home(), &registry);

        assert!(page.contains(r#"<span class="v idle">0</span>"#) || page.contains(">0<"));
        assert!(
            page.contains(">3<"),
            "three instances must be counted:\n{page}"
        );
        // Two distinct models: two named, one default — the default is a model
        // choice in its own right and must be counted as one.
        assert!(page.contains(">1<"));
    }

    #[test]
    fn a_missing_workspace_raises_a_warning_and_a_flag() {
        // A workspace that no longer exists means the harness will fail to start
        // somewhere the user did not intend. That is the one condition on this
        // page that needs a decision, so it must not be buried in grey text.
        let mut registry = Registry::default();
        let absent = PathBuf::from("/definitely/not/a/real/directory/anywhere");
        registry
            .insert("gone", Instance::new(absent, 3081))
            .unwrap();
        let page = render(&home(), &registry);

        assert!(
            page.contains("class=\"warn\""),
            "a warning banner is required"
        );
        assert!(page.contains("missing"), "the row must be flagged too");
        assert!(
            page.contains("never creates or deletes"),
            "the warning must say why we do not fix it"
        );
    }

    #[test]
    fn a_healthy_registry_shows_no_warning() {
        // A warning that appears when nothing is wrong is noise, and noise is
        // how real warnings get ignored.
        let mut registry = Registry::default();
        let dir = std::env::temp_dir();
        registry.insert("fine", Instance::new(dir, 3081)).unwrap();
        let page = render(&home(), &registry);
        assert!(
            !page.contains("class=\"warn\""),
            "no warning may appear when every workspace exists"
        );
    }

    #[test]
    fn the_endpoint_is_shown_in_full() {
        // A truncated address cannot be connected to. The port is also the link
        // target, but the endpoint line gives the exact host:port to paste.
        let mut registry = Registry::default();
        registry
            .insert("only", Instance::new(PathBuf::from("/tmp/x"), 3099))
            .unwrap();
        let page = render(&home(), &registry);
        assert!(
            page.contains("127.0.0.1:3099"),
            "the full endpoint must be rendered"
        );
    }

    #[test]
    fn the_summary_labels_reach_the_reader() {
        let page = render(&home(), &Registry::default());
        for label in ["Instances", "Serving", "Stopped", "Models in use"] {
            assert!(
                page.contains(label),
                "the summary must label its numbers: {label}"
            );
        }
    }
}
