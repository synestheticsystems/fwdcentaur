//! Records built on the Commons' models, which name no project: each is placed in one by the
//! [`Placement`] component on its own entity, and read and written through the project's graph
//! under the same rule as fwd's own models. A list reads the record's components with its
//! `Placement` as one tuple whose selection carries the project; a record read by its id is the
//! project's, or followed into whichever project it is in; a transaction places what it makes in
//! its own project and changes only that project's records.

use super::{equals, selecting, Followed, InProjectView, ProjectGraph, ProjectGraphError, ProjectTransaction};
use crate::model::{Placement, PlacementView};
use ankurah::ankql::ast::{Parsed, Predicate};
use ankurah::entity::LocalTrxEntity;
use ankurah::error::RetrievalError;
use ankurah::model::{ComponentViews, MutableBorrow};
use ankurah::property::PropertyError;
use ankurah::{EntityId, MatchArgs, Model, Value, View};
use ankurah_commons::intention_graph::ReadingView;

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

    /// Every record of the project's placed with `R`'s components.
    pub async fn all_placed<R: ComponentViews>(&self) -> Result<Vec<(PlacementView, R)>, RetrievalError> {
        self.ctx.fetch(self.everything()).await
    }

    /// The id of every record in the node placed with `R`'s components, with the record of its
    /// project, as [`ProjectGraph::ids_in_every_project`] reads fwd's own models: what an id
    /// someone types may name (D181).
    pub async fn placed_ids_in_every_project<R: ComponentViews>(
        &self,
    ) -> Result<Vec<(EntityId, EntityId)>, ProjectGraphError> {
        let every: Vec<(PlacementView, R)> = self.ctx.fetch(selecting(Predicate::True)).await?;
        Ok(every.iter().map(|(p, _)| Ok((p.id(), p.project_id()?))).collect::<Result<_, PropertyError>>()?)
    }

    /// The readings of the record `of`, whichever project placed them: a record's readings show on
    /// it wherever they were written, as its relations do (D176).
    pub async fn readings_of(&self, of: EntityId) -> Result<Vec<(PlacementView, ReadingView)>, RetrievalError> {
        self.ctx.fetch(selecting(equals("of", Value::EntityId(of)))).await
    }

    /// The component `V` of the placed record `placement` is on; none when the record does not
    /// carry it, as a record carries `LastEdit` only once edited and `Attribution` only when
    /// someone else wrote its words down.
    pub async fn component_of<V: View>(&self, placement: &PlacementView) -> Result<Option<V>, RetrievalError> {
        // A view of a model the node has not bound is refused before it looks at the record.
        self.ctx.resolve_model_id::<V::Model>().await?;
        match V::from_entity(placement.entity().clone()) {
            Ok(component) => Ok(Some(component)),
            Err(RetrievalError::MissingComponent { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The project's placed record `id`, read through its `Placement` and `R`'s components. A
    /// record of another project is refused as [`ProjectGraph::get`] refuses it, and one that lacks
    /// a component of `R` is not found.
    pub async fn get_placed<R: ComponentViews>(&self, id: EntityId) -> Result<(PlacementView, R), ProjectGraphError> {
        let placement: PlacementView = self.get(id).await?;
        Ok(self.placed(placement.id()).await?.ok_or(RetrievalError::EntityNotFound(id))?)
    }

    /// The placed record `id` that a record holds, wherever in the node it is, as
    /// [`ProjectGraph::follow`] reads one; one that lacks a component of `R` is not held as `R`.
    pub async fn follow_placed<R: ComponentViews>(
        &self,
        id: EntityId,
    ) -> Result<Followed<(PlacementView, R)>, ProjectGraphError> {
        let Followed::Held { record, project, .. } = self.follow::<PlacementView>(id).await? else {
            return Ok(Followed::NotHeld);
        };
        Ok(match self.placed(record).await? {
            Some(found) => Followed::Held { record, project, found },
            None => Followed::NotHeld,
        })
    }

    /// The record `id` read through its `Placement` and `R`'s components, wherever it is; none
    /// when it lacks one of them. Read by a fetch, which binds each component the node has not
    /// bound yet, as a view read from the record alone does not.
    async fn placed<R: ComponentViews>(&self, id: EntityId) -> Result<Option<(PlacementView, R)>, RetrievalError> {
        let found: Vec<(PlacementView, R)> = self.ctx.fetch(selecting(equals("id", Value::EntityId(id)))).await?;
        Ok(found.into_iter().next())
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
