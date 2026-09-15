//! The loopback relay.
//!
//! # Why this exists
//!
//! The harness refuses to bind `0.0.0.0` — it rejects that at startup for
//! safety, and its own documentation says the HTTP server "carries no TLS,
//! authentication, or origin policy of its own". But publishing a container
//! port requires listening on a non-loopback address *inside* the container.
//!
//! Those two facts conflict, and this relay resolves it: the harness keeps its
//! loopback bind, and the relay — the only component that ever sees a
//! non-loopback socket — forwards to it.
//!
//! # The two things it must not break
//!
//! 1. **Headers pass through unchanged.** The harness's browser-trust fence
//!    compares `Host` and `Origin` against loopback and its trusted-host list.
//!    Rewriting `Host` to `127.0.0.1` would break the ordinary case, because
//!    the browser's `Origin` would then no longer match.
//! 2. **Streaming stays streaming.** The UI depends on server-sent events and
//!    a WebSocket for live agent output. Buffering a response would turn a
//!    live session into a stalled one.
//!
//! This is why the relay exists rather than a workaround.

#![deny(missing_docs)]
#![deny(clippy::all)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

pub mod proxy;

pub use proxy::{relay_router, RelayConfig, RelayState};
