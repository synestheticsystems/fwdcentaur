//! The node's catalog of model definitions, which is no project's: what a migration reads where
//! binding a model by its label would not reach the definition it needs, as when the node minted a
//! definition for itself under a label that a kept mint also uses (`intentions::convert`).

use super::{equals, selecting, ProjectGraph};
use ankurah::core::schema::catalog::{SysModelPropertyRowView, SysModelRowView, SysPropertyRowView};
use ankurah::error::RetrievalError;
use ankurah::{EntityId, Value, View};

impl ProjectGraph {
    /// The id of every model definition the node holds under `label`. Two can share a label, one
    /// the node minted for itself and one a kept mint gave it, and a model bound by its label
    /// reaches only one of them.
    pub async fn models_labelled(&self, label: &str) -> Result<Vec<EntityId>, RetrievalError> {
        let found: Vec<SysModelRowView> =
            self.ctx.fetch(selecting(equals("label", Value::String(label.to_string())))).await?;
        Ok(found.iter().map(View::id).collect())
    }

    /// The id of the property named `name` among those of the model definition `model`; none when
    /// it has no property by that name.
    pub async fn property_named(&self, model: EntityId, name: &str) -> Result<Option<EntityId>, RetrievalError> {
        let members: Vec<SysModelPropertyRowView> =
            self.ctx.fetch(selecting(equals("model", Value::EntityId(model)))).await?;
        for member in members {
            let property: SysPropertyRowView = self.ctx.get(member.property()?).await?;
            if property.name()? == name {
                return Ok(Some(property.id()));
            }
        }
        Ok(None)
    }
}
