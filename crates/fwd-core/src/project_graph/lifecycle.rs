//! Closing a record and reopening it with the Commons' `Closed` on the record's own entity, as a
//! principal is retired and brought back, an identity binding undone, and an intention fulfilled,
//! withdrawn and reopened. A component cannot be taken off an entity, so a record reopened carries
//! its `Closed` still, every field cleared, and closing it again fills that in.

use super::{InProjectView, ProjectGraph, ProjectGraphError, ProjectTransaction};
use ankurah::error::RetrievalError;
use ankurah::model::Mutable;
use ankurah::View;
use ankurah_commons::records::{Closed, ClosedView};

/// Whether the record `record` is a view of is closed: it carries the Commons' `Closed` with a
/// time. A record reopened carries it with every field cleared, and is open.
pub fn closed(record: &impl View) -> Result<bool, RetrievalError> {
    match record.entity().cast::<ClosedView>() {
        Ok(closed) => Ok(closed.closed()?.is_some()),
        Err(RetrievalError::MissingComponent { .. }) => Ok(false),
        Err(e) => Err(e),
    }
}

impl ProjectGraph {
    /// Whether the record `record` is a view of is closed, as [`closed`] reads it, for a reader that
    /// does not know whether the node has bound `Closed` yet: a view of a model the node has not
    /// bound reads nothing, so this binds it first.
    pub async fn is_closed(&self, record: &impl View) -> Result<bool, RetrievalError> {
        self.ctx.resolve_model_id::<Closed>().await?;
        closed(record)
    }
}

impl ProjectTransaction {
    /// Closes the record `placing` names the project of, as `closing` says, in this transaction: its
    /// `Closed` added, or, on a record reopened, filled in.
    pub async fn close<P: InProjectView>(&self, placing: &P, closing: &Closed) -> Result<(), ProjectGraphError> {
        match placing.entity().cast::<ClosedView>() {
            Ok(_) => {
                let m = self.edit_placed::<Closed>(placing)?;
                m.closed()?.set(&closing.closed)?;
                m.by()?.set(&closing.by)?;
                m.outcome()?.set(&closing.outcome)?;
                m.reason()?.set(&closing.reason)?;
            }
            Err(RetrievalError::MissingComponent { .. }) => {
                let m = self.edit_placed::<P::Model>(placing)?;
                self.add(Mutable::entity(&*m), closing).await?;
            }
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    /// Reopens the record `placing` names the project of: every field of its `Closed` cleared. A
    /// record that is not closed is left as it is.
    pub fn reopen(&self, placing: &impl InProjectView) -> Result<(), ProjectGraphError> {
        if !closed(placing)? {
            return Ok(());
        }
        let m = self.edit_placed::<Closed>(placing)?;
        m.closed()?.set(&None)?;
        m.by()?.set(&None)?;
        m.outcome()?.set(&None)?;
        m.reason()?.set(&None)?;
        Ok(())
    }
}
