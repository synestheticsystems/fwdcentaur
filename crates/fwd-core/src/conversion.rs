//! What each part of `fwd admin migrate` that moves a kind of record onto the Commons' models does
//! alike (D223): every record of the kind is checked before anything is written, and one that
//! cannot move without losing part of what it holds stops the run, naming each such record ([`moves`]);
//! then each record moves in a transaction of its own, keeping its id, so that a run that stops
//! halfway leaves each record whole. A record that carries the kind's Commons component has moved
//! already and is not checked again, so a second run changes nothing.

use ankurah::EntityId;
use anyhow::{bail, Result};

/// What each record `checked` lists gets, in order, when every one of them can move; otherwise a
/// refusal naming each record that cannot and why, of the kind `kind` says, before anything is
/// written.
pub(crate) fn moves<A>(kind: &str, checked: Vec<(EntityId, Result<A, String>)>) -> Result<Vec<A>> {
    let mut moves = Vec::with_capacity(checked.len());
    let mut refused = Vec::new();
    for (id, outcome) in checked {
        match outcome {
            Ok(gets) => moves.push(gets),
            Err(why) => refused.push(format!("{} ({why})", id.to_base64())),
        }
    }
    if !refused.is_empty() {
        bail!(
            "no {kind} was moved onto the Commons' new models, since moving these would lose part of what they hold: {}",
            refused.join("; ")
        );
    }
    Ok(moves)
}
