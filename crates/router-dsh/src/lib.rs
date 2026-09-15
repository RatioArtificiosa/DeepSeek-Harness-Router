//! # router-dsh
//!
//! The `DeepSeek Harness` adapter: process supervision, readiness detection, and
//! the harness-specific knowledge the rest of the system must not have.
//!
//! # The adapter rule
//!
//! **This is the only crate permitted to know how the harness works.** No other
//! crate may encode harness-specific behaviour, because the harness is in
//! developer preview and breaking changes are expected. Confining that coupling
//! here is what makes an upstream change a localized edit rather than a rewrite
//! (which keeps an upstream change a localized edit).
//!
//! ```
//! use router_dsh::readiness::{classify_line, OutputSignal};
//!
//! // The harness announces readiness with a URL line.
//! assert!(matches!(
//!     classify_line("dsh web: http://127.0.0.1:3081"),
//!     OutputSignal::Ready { .. }
//! ));
//! ```

#![deny(missing_docs)]
#![deny(clippy::all)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

pub mod readiness;
pub mod state;
pub mod supervisor;

pub use readiness::{classify_line, OutputSignal};
pub use state::{RuntimeFailure, RuntimeState, RuntimeStatus};
pub use supervisor::{Supervisor, SupervisorConfig};
