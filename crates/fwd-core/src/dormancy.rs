//! Which agents the pickers and `fwd principal list` show as dormant, so that the users left behind
//! by finished agents that missed their closeout do not crowd the agents at work out of every
//! picker (docs/DECISIONS.md, D123). Nothing retires a user by time, since fwd cannot tell a quiet
//! agent from a gone one; instead an agent that has written nothing to the graph for twelve hours
//! is listed below the others, under a heading of its own.
//!
//! Dormant is an overlay for display, derived from the principal's last write and the reader's
//! clock as the task overlays are derived (docs/design/data-model.md, "Availability overlays"), and
//! never enforced: a dormant agent is still offered and still addressable, routing still reaches
//! it, it still writes, and its next write makes it active again. People are never dormant.
//!
//! The rule is here, [`is_dormant`], so that the page's pickers, the extension's mention search and
//! `fwd principal list` agree on it; so is the stamp every client makes after it writes,
//! [`wrote`], which the rule reads back as the principal's `last_active`.

use crate::model::*;
use ankurah::{Context, EntityId};
use anyhow::Result;

/// How long an agent goes without writing before it shows as dormant: twelve hours (D123).
pub const DORMANT_AFTER_MS: i64 = 12 * 60 * 60 * 1000;

/// Whether `p` shows as dormant at `now`, the reader's clock in milliseconds since the epoch: an
/// agent that is not retired and whose last write is more than twelve hours old, or that has
/// recorded no write at all, as every agent recorded before 2026-09-30 has not until its next one.
/// A person is never dormant, and neither is a retired agent, which no picker offers.
pub fn is_dormant(p: &PrincipalView, now: i64) -> bool {
    quiet(p.kind().ok(), p.active().unwrap_or(false), p.last_active().ok().flatten(), now)
}

/// [`is_dormant`], on the three fields it reads.
fn quiet(kind: Option<PrincipalKind>, active: bool, last_active: Option<i64>, now: i64) -> bool {
    kind == Some(PrincipalKind::Agent) && active && last_active.is_none_or(|at| now - at > DORMANT_AFTER_MS)
}

/// Records that `principal` wrote to the graph just now, by this machine's clock, in its
/// `last_active`, which is what keeps an agent at work out of the dormant list and brings a dormant
/// one back at its next write. Every client runs it after each of its writes succeeds but those
/// that mark notifications read or dismissed: the CLI after every command that writes work into the
/// project or changes a principal (not after marking notifications, nor after `fwd admin migrate`),
/// and the page and the extension after each of their other writes.
pub async fn wrote(ctx: &Context, principal: EntityId) -> Result<()> {
    let p: PrincipalView = ctx.get(principal).await?;
    let trx = ctx.begin();
    p.edit(&trx)?.last_active()?.set(&Some(crate::now_ms()))?;
    trx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 60 * 60 * 1000;
    const NOW: i64 = 1_790_800_000_000;

    #[test]
    fn an_agent_is_dormant_once_its_last_write_is_more_than_twelve_hours_old() {
        let agent = |last: Option<i64>| quiet(Some(PrincipalKind::Agent), true, last, NOW);
        assert!(!agent(Some(NOW - 11 * HOUR)), "eleven hours quiet: active");
        assert!(!agent(Some(NOW - 12 * HOUR)), "twelve hours to the millisecond: active still");
        assert!(agent(Some(NOW - 12 * HOUR - 1)), "a millisecond more: dormant");
        assert!(agent(Some(NOW - 13 * HOUR)), "thirteen hours quiet: dormant");
        assert!(agent(None), "no write recorded, as for an agent recorded before 2026-09-30: dormant");
        assert!(!agent(Some(NOW + HOUR)), "a write stamped by a clock ahead of the reader's: active");
    }

    #[test]
    fn a_person_a_retired_agent_and_a_principal_of_no_kind_read_are_never_dormant() {
        assert!(!quiet(Some(PrincipalKind::Human), true, None, NOW), "a person with no write recorded");
        assert!(!quiet(Some(PrincipalKind::Human), true, Some(NOW - 100 * HOUR), NOW), "a person long quiet");
        assert!(!quiet(Some(PrincipalKind::Agent), false, Some(NOW - 13 * HOUR), NOW), "a retired agent");
        assert!(!quiet(None, true, None, NOW), "a principal whose kind cannot be read");
    }
}
