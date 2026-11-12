//! The live reads of a page that shows one project (`ProjectGraph::follow_live`,
//! `relations_made_elsewhere_live`, `query_projects`), over two projects of one node: a page lists
//! alpha's records and names what they hold the ids of, which may be beta's. Each read holds the
//! records of the ids it is given, in either project, follows a change made in the other project,
//! and gives up an id it is no longer given; the relations beta made that point at an alpha record
//! are read by that record's id, and alpha's own relations are not among them.

use ankurah::signals::Peek;
use ankurah::{policy::DEFAULT_CONTEXT, EntityId, LiveQuery, Node, PermissiveAgent, View};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::model::*;
use fwd_core::ops::{self, CreateTask};
use fwd_core::project_graph::{InProjectView, ProjectGraph};
use fwd_core::rules::Verb;
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::Arc;

/// alpha and beta over one store in memory, each with its person.
struct Two {
    alpha: ProjectGraph,
    beta: ProjectGraph,
    daniel: EntityId,
    their_daniel: EntityId,
}

async fn two() -> Result<Two> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    let ctx = node.context_async(DEFAULT_CONTEXT).await?;
    let alpha = ProjectGraph::ensure(ctx.clone(), uuid::Uuid::new_v4(), "alpha").await?;
    let beta = ProjectGraph::ensure(ctx, uuid::Uuid::new_v4(), "beta").await?;
    let daniel = ops::ensure_principal(&alpha, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let their_daniel = ops::ensure_principal(&beta, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    Ok(Two { alpha, beta, daniel, their_daniel })
}

async fn task(graph: &ProjectGraph, by: EntityId, title: &str) -> Result<EntityId> {
    ops::create_task(graph, by, CreateTask { title: title.into(), ..Default::default() }).await
}

async fn link(
    graph: &ProjectGraph,
    by: EntityId,
    kind: RelationKind,
    from: EntityId,
    to: EntityId,
) -> Result<EntityId> {
    ops::link(graph, by, kind, from, to, None, false, json!({})).await
}

fn ids(of: &[EntityId]) -> BTreeSet<EntityId> {
    of.iter().copied().collect()
}

/// The ids of what `query` holds now.
fn held<V: InProjectView + Clone + 'static>(query: &LiveQuery<V>) -> BTreeSet<EntityId> {
    query.peek().iter().map(View::id).collect()
}

#[tokio::test]
async fn a_live_follow_holds_the_records_of_the_ids_it_is_given_in_either_project_and_follows_their_changes(
) -> Result<()> {
    let t = two().await?;
    let here = task(&t.alpha, t.daniel, "Use the connector").await?;
    let unnamed = task(&t.alpha, t.daniel, "Something no one names").await?;
    let far = task(&t.beta, t.their_daniel, "Ship the connector").await?;
    let nothing = EntityId::from_bytes([7; 32]);

    let follow = t.alpha.follow_live::<TaskView>(&ids(&[here, far, nothing]))?;
    follow.live().wait_initialized().await?;
    assert_eq!(held(follow.live()), ids(&[here, far]), "beta's task with alpha's; an id the node lacks is absent");
    assert!(!held(follow.live()).contains(&unnamed), "a record no one names is not read");

    // Done in beta: the query alpha's page reads it through hears it.
    ops::transition(&t.beta, t.their_daniel, far, Verb::Done, None).await?;
    let status = follow.live().peek().iter().find(|v| v.id() == far).map(|v| v.status()).transpose()?;
    assert_eq!(status, Some(TaskStatus::Done));

    // Given other ids, it holds their records alone.
    follow.follow_ids(&ids(&[far]))?;
    follow.live().wait_initialized().await?;
    assert_eq!(held(follow.live()), ids(&[far]));
    follow.follow_ids(&BTreeSet::new())?;
    follow.live().wait_initialized().await?;
    assert!(held(follow.live()).is_empty(), "given no id, it holds nothing");
    Ok(())
}

#[tokio::test]
async fn the_relations_another_project_made_that_touch_a_record_are_read_live_by_its_id() -> Result<()> {
    let t = two().await?;
    let ours = task(&t.alpha, t.daniel, "Ship the connector").await?;
    let also_ours = task(&t.alpha, t.daniel, "Document the connector").await?;
    let theirs = task(&t.beta, t.their_daniel, "Use the connector").await?;
    let made_there = link(&t.beta, t.their_daniel, RelationKind::DependsOn, theirs, ours).await?;
    // Alpha's own relation to the same record is on alpha's list, not in this read.
    link(&t.alpha, t.daniel, RelationKind::RelatesTo, also_ours, ours).await?;

    let elsewhere = t.alpha.relations_made_elsewhere_live(&ids(&[ours]))?;
    elsewhere.live().wait_initialized().await?;
    assert_eq!(held(elsewhere.live()), ids(&[made_there]));

    // One beta makes later arrives, from beta's task too.
    let later = link(&t.beta, t.their_daniel, RelationKind::RelatesTo, theirs, also_ours).await?;
    assert_eq!(held(elsewhere.live()), ids(&[made_there]), "it touches a record the read was not given");
    elsewhere.follow_ids(&ids(&[ours, also_ours]))?;
    elsewhere.live().wait_initialized().await?;
    assert_eq!(held(elsewhere.live()), ids(&[made_there, later]));
    let another = link(&t.beta, t.their_daniel, RelationKind::RelatesTo, theirs, ours).await?;
    assert_eq!(held(elsewhere.live()), ids(&[made_there, later, another]), "made after the read opened");

    // Read for beta's task, from alpha's graph: its own relations, which beta made.
    elsewhere.follow_ids(&ids(&[theirs]))?;
    elsewhere.live().wait_initialized().await?;
    assert_eq!(held(elsewhere.live()), ids(&[made_there, later, another]));
    Ok(())
}

#[tokio::test]
async fn the_projects_query_names_every_project_of_the_node_as_it_changes() -> Result<()> {
    let t = two().await?;
    let projects = t.alpha.query_projects()?;
    projects.wait_initialized().await?;
    let names =
        || -> Result<BTreeSet<String>> { Ok(projects.peek().iter().map(|p| p.name()).collect::<Result<_, _>>()?) };
    assert_eq!(names()?, ["alpha", "beta"].map(String::from).into());
    t.beta.rename("beta two").await?;
    assert_eq!(names()?, ["alpha", "beta two"].map(String::from).into());
    Ok(())
}
