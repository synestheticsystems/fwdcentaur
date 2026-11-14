//! Records built on the Commons' models, which name no project: each is placed in one by the
//! [`Placement`] component on its own entity, and read and written through the project's graph
//! under the same rule as fwd's own models. A list reads the record's components with its
//! `Placement` as one tuple whose selection carries the project; a record read by its id is the
//! project's, or followed into whichever project it is in; a transaction places what it makes in
//! its own project and changes only that project's records.

use super::{Followed, InProjectView, ProjectGraph, ProjectGraphError, ProjectTransaction};
use crate::model::{Placement, PlacementView};
use ankurah::ankql::ast::Parsed;
use ankurah::entity::LocalTrxEntity;
use ankurah::error::RetrievalError;
use ankurah::model::{ComponentViews, MutableBorrow};
use ankurah::{EntityId, MatchArgs, Model, View};

impl ProjectGraph {
    /// The project's placed records that `args` selects, each read through its `Placement` and
    /// the components `R` names, every one of which a record must carry to be selected.
    pub async fn fetch_placed<R: ComponentViews>(
        &self,
        args: impl TryInto<MatchArgs<Parsed>, Error = impl Into<RetrievalError>>,
    ) -> Result<Vec<(PlacementView, R)>, RetrievalError> {
        let args = args.try_into().map_err(Into::into)?;
        self.ctx.fetch(self.scoped(args)).await
    }

    /// The project's placed record `id`, read through its `Placement` and `R`'s components. A
    /// record of another project is refused as [`ProjectGraph::get`] refuses it; one that lacks a
    /// component of `R` is the error that names it.
    pub async fn get_placed<R: ComponentViews>(&self, id: EntityId) -> Result<(PlacementView, R), ProjectGraphError> {
        let placement: PlacementView = self.get(id).await?;
        let components = R::views_of(placement.entity().clone())?;
        Ok((placement, components))
    }

    /// The placed record `id` that a record holds, wherever in the node it is, as
    /// [`ProjectGraph::follow`] reads one; one that lacks a component of `R` is not held as `R`.
    pub async fn follow_placed<R: ComponentViews>(
        &self,
        id: EntityId,
    ) -> Result<Followed<(PlacementView, R)>, ProjectGraphError> {
        let Followed::Held { record, project, found } = self.follow::<PlacementView>(id).await? else {
            return Ok(Followed::NotHeld);
        };
        Ok(match R::views_of(found.entity().clone()) {
            Ok(components) => Followed::Held { record, project, found: (found, components) },
            Err(RetrievalError::MissingComponent { .. }) => Followed::NotHeld,
            Err(e) => return Err(e.into()),
        })
    }
}

impl ProjectTransaction {
    /// Makes a record of `model` placed in the transaction's project. Its other components are
    /// added with [`ProjectTransaction::add`] before its id is first asked for, so that the event
    /// that makes the record carries them all.
    pub async fn create_placed<'rec, 'trx: 'rec, M: Model>(
        &'trx self,
        model: &M,
    ) -> Result<MutableBorrow<'rec, M::Mutable>, ProjectGraphError> {
        let made = self.trx.create(model).await?;
        self.add(ankurah::model::Mutable::entity(&*made), &Placement { project: self.project() }).await?;
        Ok(made)
    }

    /// Adds `component` to `record`, a record this transaction makes or changes, which must not
    /// carry it yet: a component's values once carried are changed through its own view.
    pub async fn add<M: Model>(&self, record: &LocalTrxEntity, component: &M) -> Result<(), ProjectGraphError> {
        let model = self.ctx.resolve_model_id::<M>().await.map_err(RetrievalError::from)?;
        if record.has_membership(&model) {
            return Err(ProjectGraphError::Carried { label: M::descriptor().label });
        }
        Ok(component.initialize_new_entity(record, model, record.system_epoch())?)
    }

    /// The component `M` of the placed record `placement` is on, to change in this transaction. A
    /// record of another project is refused as [`ProjectTransaction::edit`] refuses it.
    pub fn edit_placed<'rec, 'trx: 'rec, M: Model>(
        &'trx self,
        placement: &PlacementView,
    ) -> Result<MutableBorrow<'rec, M::Mutable>, ProjectGraphError> {
        match placement.project_id()? {
            project if project == self.project => Ok(self.trx.edit::<M>(placement.entity())?),
            project => Err(ProjectGraphError::OfAnotherProject { record: placement.id(), project }),
        }
    }
}
