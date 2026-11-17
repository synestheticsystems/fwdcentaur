//! Principals, each the Commons' `identity.Principal` with fwd's `Role` in one project beside it on
//! the same entity (`model::Role`): the role names the project, so the graph reads a project's
//! principals by it, under the same rule as every other record, and hands each out as the views of
//! the two, `(RoleView, PrincipalView)`. A principal read by its id is the project's, or followed
//! into its own. Whether a principal is retired is read off the same entity, from the Commons'
//! `Closed`, which it carries only once retired ([`closed`]).

use super::{Followed, InProjectView, ProjectGraph, ProjectGraphError, ProjectTransaction};
use crate::model::{Principal, PrincipalView, RoleView};
use ankurah::ankql::ast::Parsed;
use ankurah::error::RetrievalError;
use ankurah::model::Mutable;
use ankurah::{Context, EntityId, MatchArgs, View};
use ankurah_commons::records::{Closed, ClosedView};

/// Binds the components every reader of a principal reads off its role, `identity.Principal` and
/// `Closed`, so that a principal named by a role alone, as the page's lists and the routing
/// service's standing query hold one, is read whole ([`principal_parts`], [`closed`]): a view of a
/// model the node has not bound reads nothing. A client's graph and the routing service's bind them
/// as they open, and every read here that hands out a principal binds them first.
pub(super) async fn bind_principal_parts(ctx: &Context) -> Result<(), RetrievalError> {
    ctx.resolve_model_id::<Principal>().await?;
    ctx.resolve_model_id::<Closed>().await?;
    Ok(())
}

/// The parts of the principal `role` is the role of: the role and the Commons' principal on its
/// entity.
pub fn principal_parts(role: RoleView) -> Result<(RoleView, PrincipalView), RetrievalError> {
    let principal = role.entity().cast::<PrincipalView>()?;
    Ok((role, principal))
}

/// Whether the record `record` is a view of is closed: it carries the Commons' `Closed` with a
/// time, as a retired principal and an undone identity binding do, and a record reopened carries it
/// with every field cleared.
pub fn closed(record: &impl View) -> Result<bool, RetrievalError> {
    match record.entity().cast::<ClosedView>() {
        Ok(closed) => Ok(closed.closed()?.is_some()),
        Err(RetrievalError::MissingComponent { .. }) => Ok(false),
        Err(e) => Err(e),
    }
}

impl ProjectGraph {
    /// Whether the record `record` is a view of is closed, as [`closed`] reads it, for a reader that
    /// does not know whether the node has bound `Closed` yet: this binds it first.
    pub async fn is_closed(&self, record: &impl View) -> Result<bool, RetrievalError> {
        let closing: Option<ClosedView> = self.component_of(record).await?;
        Ok(match closing {
            Some(closing) => closing.closed()?.is_some(),
            None => false,
        })
    }

    /// Every principal of the project, retired or not.
    pub async fn principals(&self) -> Result<Vec<(RoleView, PrincipalView)>, RetrievalError> {
        bind_principal_parts(&self.ctx).await?;
        self.ctx.fetch(self.everything()).await
    }

    /// The project's principals that `args` selects, by the properties of their role and of the
    /// Commons' principal.
    pub async fn fetch_principals(
        &self,
        args: impl TryInto<MatchArgs<Parsed>, Error = impl Into<RetrievalError>>,
    ) -> Result<Vec<(RoleView, PrincipalView)>, RetrievalError> {
        let args = args.try_into().map_err(Into::into)?;
        bind_principal_parts(&self.ctx).await?;
        self.ctx.fetch(self.scoped(args)).await
    }

    /// The project's principal `id`. A principal of another project is refused as
    /// [`ProjectGraph::get`] refuses a record of it.
    pub async fn principal(&self, id: EntityId) -> Result<(RoleView, PrincipalView), ProjectGraphError> {
        bind_principal_parts(&self.ctx).await?;
        Ok(principal_parts(self.get(id).await?)?)
    }

    /// The principal `id` that a record holds, wherever in the node it is, as
    /// [`ProjectGraph::follow`] reads a record.
    pub async fn follow_principal(
        &self,
        id: EntityId,
    ) -> Result<Followed<(RoleView, PrincipalView)>, ProjectGraphError> {
        bind_principal_parts(&self.ctx).await?;
        Ok(match self.follow::<RoleView>(id).await? {
            Followed::Held { record, project, found } => {
                Followed::Held { record, project, found: principal_parts(found)? }
            }
            Followed::NotHeld => Followed::NotHeld,
        })
    }
}

impl ProjectTransaction {
    /// Closes the record `placing` names the project of, as `closing` says, in this transaction, as
    /// a principal is retired and an identity binding undone: its `Closed` added, or, on a record
    /// reopened, which carries it with every field cleared, filled in.
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

    /// Reopens the record `placing` names the project of, as a retired principal is brought back:
    /// every field of its `Closed` cleared, as the Commons reopen a record, since a component cannot
    /// be taken off an entity. A record that is not closed is left as it is.
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
