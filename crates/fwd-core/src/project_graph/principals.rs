//! Principals, each the Commons' `identity.Principal` with fwd's `Role` in one project beside it on
//! the same entity (`model::Role`): the role names the project, so the graph reads a project's
//! principals by it, under the same rule as every other record, and hands each out as the views of
//! the two, `(RoleView, PrincipalView)`. A principal read by its id is the project's, or followed
//! into its own. Whether a principal is retired is read off the same entity, from the Commons'
//! `Closed`, which it carries only once retired ([`closed`](super::closed)).

use super::{Followed, ProjectGraph, ProjectGraphError};
use crate::model::{Principal, PrincipalView, RoleView};
use ankurah::ankql::ast::Parsed;
use ankurah::error::RetrievalError;
use ankurah::{Context, EntityId, MatchArgs, View};
use ankurah_commons::records::Closed;

/// Binds the components every reader of a principal reads off its role, `identity.Principal` and
/// `Closed`, so that a principal named by a role alone, as the page's lists and the routing
/// service's standing query hold one, is read whole ([`principal_parts`], [`closed`](super::closed)): a view of a
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

impl ProjectGraph {
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
