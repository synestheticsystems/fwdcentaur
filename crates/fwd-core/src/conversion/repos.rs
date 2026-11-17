//! `fwd admin migrate`, the part that moves a project's repositories from fwd's own model onto the
//! Commons' `git.Repository` (D223), field for field, each placed in its project by a `Placement`.

use super::moves;
use crate::model::{Placement, PlacementView};
use crate::project_graph::ProjectGraph;
use ankurah::model::Mutable;
use ankurah::{EntityId, Model};
use ankurah_commons::git::{Repository, RepositoryView};
use anyhow::Result;

/// fwd's own model of a repository, before it moved onto the Commons' (D261): what [`convert`]
/// reads, and what `fwd-store-merge` writes from the stores of before the daemon's one store, which
/// a run of the migration then moves. The struct keeps the model's name, `Repo`, by which the node
/// binds it to its definition, and is `no_ffi`, since no binding needs it.
pub mod before_d261 {
    use crate::model::Project;
    use ankurah::property::Json;
    use ankurah::{Model, Ref};
    use serde::{Deserialize, Serialize};

    /// A registered repository, keyed by its upstream (`remote_urls`).
    #[derive(Model, Debug, Clone, Serialize, Deserialize)]
    #[model(no_ffi)]
    pub struct Repo {
        pub project: Ref<Project>,
        #[active_type(LWW)]
        pub name: String,
        /// `["git@github.com:org/x.git", ...]`
        pub remote_urls: Json,
        #[active_type(LWW)]
        pub default_branch: String,
    }
}

/// One repository [`convert`] moved: its id and its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Converted {
    pub repository: EntityId,
    pub name: String,
}

/// Moves the project's repositories onto the Commons' `git.Repository`, adding to each entity and
/// keeping its id, which every anchor and scope that names the repository holds: its name, its
/// remote URLs and its default branch as they are, and a `Placement` in its project, which the
/// Commons' model does not name. fwd's own model stays on the entity, since Ankurah cannot take a
/// component off one yet. Each repository moves as [`super`] says: one whose entity carries a
/// `Placement` already, which the move would write over, refuses the run, and one that carries
/// `Repository` has moved.
pub async fn convert(graph: &ProjectGraph) -> Result<Vec<Converted>> {
    let mut checked = Vec::new();
    for old in unmoved(graph).await? {
        let gets = match graph.component_of::<PlacementView>(&old).await? {
            Some(_) => Err("it carries `placement` already, which the move would write over".to_string()),
            None => Ok(Repository {
                name: old.name()?,
                remote_urls: old.remote_urls()?,
                default_branch: old.default_branch()?,
            }),
        };
        checked.push((old.id(), gets.map(|repository| (old, repository))));
    }
    let mut converted = Vec::with_capacity(checked.len());
    for (old, repository) in moves("repository", checked)? {
        let trx = graph.begin();
        let edited = trx.edit(&old)?;
        let record = Mutable::entity(&*edited);
        trx.add(record, &Placement { project: graph.project() }).await?;
        trx.add(record, &repository).await?;
        trx.commit().await?;
        converted.push(Converted { repository: old.id(), name: repository.name });
    }
    Ok(converted)
}

/// Whether the project holds a repository [`convert`] has not moved yet. No read finds one until it
/// has, so registering a checkout of its upstream then would make a second record for it, which the
/// move would keep beside the first.
pub async fn any_unmoved(graph: &ProjectGraph) -> Result<bool> {
    Ok(!unmoved(graph).await?.is_empty())
}

/// The project's repositories of fwd's own model that carry no `Repository` yet. A node that never
/// defined that model holds none, and is not made to define it by reading them.
async fn unmoved(graph: &ProjectGraph) -> Result<Vec<before_d261::RepoView>> {
    if graph.models_labelled(before_d261::Repo::descriptor().label).await?.is_empty() {
        return Ok(Vec::new());
    }
    let mut unmoved = Vec::new();
    for old in graph.all::<before_d261::RepoView>().await? {
        if graph.component_of::<RepositoryView>(&old).await?.is_none() {
            unmoved.push(old);
        }
    }
    Ok(unmoved)
}
