//! The ids a project's records had before they were renumbered (`model::FormerId`), kept so that
//! what people cited for months still finds the record: a commit message, a memory file or a
//! message's text that names a task by its old id, whole or by its first characters. Every place
//! that takes an id reads these beside the current ids: the command line's id resolver
//! (`ops::name_to_id`), the reference parser (`ops::resolve_reference`) and the reading of a stored
//! text's references (`ops::describe`), and the page's router and lozenges.

use crate::model::FormerIdView;
use crate::project_graph::ProjectGraph;
use crate::query::with_string;
use ankurah::EntityId;
use anyhow::Result;

/// A project's former ids, each with the record it names now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FormerIds(Vec<(String, EntityId)>);

impl FormerIds {
    /// Every former id of the project `graph` holds.
    pub async fn of(graph: &ProjectGraph) -> Result<FormerIds> {
        Ok(FormerIds::from_views(&graph.all::<FormerIdView>().await?))
    }

    /// The record the former id `whole` of the project `graph` holds names now, when it is one.
    pub async fn named_in(graph: &ProjectGraph, whole: &str) -> Result<Option<EntityId>> {
        let found: Vec<FormerIdView> = graph.fetch(with_string("former = ?", whole)?).await?;
        Ok(FormerIds::from_views(&found).named(whole))
    }

    /// The former ids among `views`, as the page's standing query holds them; one that cannot be
    /// read is left out.
    pub fn from_views(views: &[FormerIdView]) -> FormerIds {
        FormerIds(views.iter().filter_map(|v| Some((v.former().ok()?, v.record().ok()?))).collect())
    }

    /// The record the former id `whole` names now.
    pub fn named(&self, whole: &str) -> Option<EntityId> {
        self.0.iter().find(|(former, _)| former == whole).map(|(_, record)| *record)
    }

    /// The former ids that start with `prefix`, each with the record it names now.
    pub fn starting<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = (&'a str, EntityId)> + 'a {
        self.0
            .iter()
            .filter(move |(former, _)| former.starts_with(prefix))
            .map(|(former, record)| (former.as_str(), *record))
    }
}
