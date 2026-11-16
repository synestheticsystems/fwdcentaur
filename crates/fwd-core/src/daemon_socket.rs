//! The path at which a client joins the daemon's node, in one place for the daemon that serves it
//! (crates/fwd/src/serve.rs) and every client that joins it: the CLI ([`crate::daemon`]) and the
//! page (crates/fwd-web). The wallboard's launcher, in Python, names it once itself
//! (deploy/wallboard/wallboard.py).
//!
//! The path names what the socket reaches, the daemon's one store and its [`GENERATION`], so that
//! no client built for an earlier store gets there. The daemons before the one store kept a store
//! per project and served each project's socket at `/p/<project>/ws`. A client built for them speaks
//! the same Ankurah wire, but none of its reads names a project: joined to the one store, its
//! `fwd watch` would print every project's notifications, its page would show every project's
//! records, and its writes would make records that name no project. The daemon answers each earlier
//! path with a refusal (`OlderClient` in serve.rs), so such a client fails to join instead.

/// The generation of the daemon's one store that a client of this build is made for, which the
/// path of its socket names. A change after which a client of an earlier build would read or write
/// the store wrongly bumps it, and the daemon refuses the paths of the generations before; a build
/// that changes nothing a client relies on keeps it, so clients of other builds still join.
///
/// Generation 2: the store holds the Commons' kept mint (D255), and a definition the node minted
/// for itself can share its label with one of the mint's. A client of an earlier build binds the
/// Commons' models by their labels, so there it reaches the mint's definition, reads by it what an
/// older record holds in the node's own, and registers its own declaration over the mint's.
pub const GENERATION: u32 = 2;

/// The base path a client hands Ankurah's WebSocket connector for `project`'s socket into the
/// daemon's one store; the connector appends `/ws` to it.
pub fn base_path(project: impl std::fmt::Display) -> String {
    format!("/p/{project}/one-store/{GENERATION}")
}
