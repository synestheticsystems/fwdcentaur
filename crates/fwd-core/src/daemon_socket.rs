//! The path at which a client joins the daemon's node, in one place for the daemon that serves it
//! (crates/fwd/src/serve.rs) and every client that joins it: the CLI ([`crate::daemon`]) and the
//! page (crates/fwd-web). The wallboard's launcher, in Python, names it once itself
//! (deploy/wallboard/wallboard.py).
//!
//! The path names what the socket reaches, the daemon's one store, so that no client built for the
//! daemons before it gets there. Those daemons kept a store per project and served each project's
//! socket at `/p/<project>/ws`. A client built for them speaks the same Ankurah wire, but none of
//! its reads names a project: joined to the one store, its `fwd watch` would print every project's
//! notifications, its page would show every project's records, and its writes would make records
//! that name no project. The daemon answers that old path with a refusal (`refuse_older_client` in
//! serve.rs), so such a client fails to join instead.

/// The base path a client hands Ankurah's WebSocket connector for `project`'s socket into the
/// daemon's one store; the connector appends `/ws` to it.
pub fn base_path(project: impl std::fmt::Display) -> String {
    format!("/p/{project}/one-store")
}
