//! fwd-core: the Ankurah models and rules shared by the daemon (hosting a project's durable
//! node), the CLI (an ephemeral node) and the web UI (an ephemeral node in the browser).
//! Everything with semantics lives here so clients, the daemon and tests run one implementation.
//! See docs/design/data-model.md.
//!
//! Features: `native` (default) adds the daemon client, the project directories and git-driven
//! anchoring; `wasm` compiles the rest for the browser (crates/fwd-web).

pub mod anchor_name;
#[cfg(feature = "native")]
pub mod anchoring;
#[cfg(feature = "native")]
pub mod backup;
#[cfg(feature = "native")]
pub mod daemon;
pub mod decision_log;
pub mod dormancy;
pub mod file_mention;
pub mod guidance;
pub mod labels;
pub mod model;
pub mod naming;
pub mod ops;
pub mod position;
#[cfg(feature = "native")]
pub mod project;
pub mod query;
pub mod reason;
pub mod references;
pub mod routing;
pub mod rules;
#[cfg(feature = "native")]
pub mod sidecar;
pub mod subject;

/// Milliseconds since the Unix epoch, wall clock. Informational only: Ankurah's causal ordering
/// decides merges, not these timestamps.
#[cfg(not(target_arch = "wasm32"))]
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// The browser has no `SystemTime`; `Date.now()` is the same clock.
#[cfg(target_arch = "wasm32")]
pub fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}
