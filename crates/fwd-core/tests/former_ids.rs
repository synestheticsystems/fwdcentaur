//! Former ids (`fwd_core::former_ids`): an id a record had before it was renumbered names the
//! record wherever an id is taken, whole or as a prefix, beside its current id; a prefix that names
//! two records is refused with both listed; and a text written before the renumbering, which
//! stores the former id, reads as naming the record.

use ankurah::{policy::DEFAULT_CONTEXT, EntityId, Node, PermissiveAgent};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::model::*;
use fwd_core::naming::{self, Refused};
use fwd_core::ops::{self, CreateScope, CreateTask, Kind, MessageContext};
use fwd_core::project_graph::ProjectGraph;
use std::sync::Arc;

/// The graph of a project alone in a store in memory.
async fn project() -> Result<ProjectGraph> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    let ctx = node.context_async(DEFAULT_CONTEXT).await?;
    Ok(ProjectGraph::ensure(ctx, uuid::Uuid::new_v4(), "test").await?)
}

/// Records that `record` had the id `former`, as the move that renumbers records writes it.
async fn renumbered(graph: &ProjectGraph, former: &str, record: EntityId) -> Result<()> {
    let trx = graph.begin();
    trx.create(&FormerId { project: graph.project(), former: former.into(), record }).await?;
    trx.commit().await?;
    Ok(())
}

/// A task titled `title`, made by `by`.
async fn task(graph: &ProjectGraph, by: EntityId, title: &str) -> Result<EntityId> {
    ops::create_task(graph, by, CreateTask { title: title.into(), ..Default::default() }).await
}

/// An id that starts with `start`, as a former id people cited might.
fn starting(start: &str) -> String {
    format!("{start}{}", &EntityId::random().to_base64()[start.len()..])
}

#[tokio::test]
async fn a_former_id_names_its_record_whole_and_as_a_prefix_beside_its_current_id() -> Result<()> {
    let graph = project().await?;
    let daniel = ops::ensure_principal(&graph, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let retry = task(&graph, daniel, "Retry on disconnect").await?;
    let cited = EntityId::random().to_base64();
    renumbered(&graph, &cited, retry).await?;

    assert_eq!(ops::resolve_id(&graph, &cited, ops::ANY).await?, retry, "whole");
    assert_eq!(ops::resolve_id(&graph, &cited[..8], ops::ANY).await?, retry, "its first eight characters");
    assert_eq!(ops::resolve_id(&graph, &retry.to_base64()[..8], ops::ANY).await?, retry, "the current id still does");
    let refused = ops::resolve_id(&graph, &cited[..8], &[Kind::Message]).await.expect_err("a task is no message");
    assert!(refused.to_string().contains("no message has an id starting with"), "{refused}");
    Ok(())
}

#[tokio::test]
async fn a_prefix_that_names_two_records_is_refused_with_both_listed_and_one_that_names_one_twice_is_not() -> Result<()>
{
    let graph = project().await?;
    let daniel = ops::ensure_principal(&graph, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let (retry, cap) =
        (task(&graph, daniel, "Retry on disconnect").await?, task(&graph, daniel, "Cap the delay").await?);
    // The task `cap` once had an id that starts as `retry`'s id does now.
    let shared = retry.to_base64()[..8].to_string();
    let cited = starting(&shared);
    renumbered(&graph, &cited, cap).await?;
    let refused = ops::resolve_id(&graph, &shared, ops::ANY).await.expect_err("two tasks start so");
    let said = refused.to_string();
    assert!(said.contains("starts the ids of 2 entities"), "{said}");
    assert!(said.contains(&format!("task {}", &retry.to_base64()[..12])), "{said}");
    assert!(said.contains(&format!("(formerly {})", &cited[..12])), "{said}");
    assert_eq!(ops::resolve_id(&graph, &cited[..10], ops::ANY).await?, cap, "a longer prefix names one");

    // Renumbered twice, `retry` has two former ids with one start: that start names it alone.
    let twice = starting("Zz9Zz9Zz");
    renumbered(&graph, &twice, retry).await?;
    renumbered(&graph, &starting("Zz9Zz9Zz"), retry).await?;
    assert_eq!(ops::resolve_id(&graph, "Zz9Zz9Zz", ops::ANY).await?, retry);
    assert_eq!(ops::resolve_id(&graph, &twice, ops::ANY).await?, retry);
    Ok(())
}

#[tokio::test]
async fn a_text_names_a_record_by_a_former_id_whether_written_before_the_renumbering_or_after() -> Result<()> {
    let graph = project().await?;
    let daniel = ops::ensure_principal(&graph, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let retry = task(&graph, daniel, "Retry on disconnect").await?;
    let cited = EntityId::random();
    renumbered(&graph, &cited.to_base64(), retry).await?;

    // Written after: the reference is read through the former id, and stored as the record's id.
    let on_task = MessageContext { task_id: Some(retry), ..Default::default() };
    let typed = format!("see [[{}]] and [[{}]]", cited.to_base64(), &cited.to_base64()[..8]);
    let posted = ops::post_message(&graph, daniel, &typed, Priority::Next, on_task.clone()).await?;
    let stored = ops::text(graph.get::<MessageView>(posted).await?.content())?;
    assert_eq!(stored, format!("see [[{retry}]] and [[{retry}]]"));

    // Written before: the text stores the former id as it was, and reads as naming the record.
    let before = format!("as [[{cited}]] says");
    let trx = graph.begin();
    let old = trx
        .create(&Message {
            project: graph.project(),
            author: ankurah::Ref::new(daniel),
            created: 1,
            content: before.clone(),
            priority: Priority::Next,
            status: MessageStatus::Active,
            task: Some(ankurah::Ref::new(retry)),
            anchor: None,
            scope: None,
            decision: None,
            reply_to: None,
            thread: None,
            context: ankurah::property::Json::new(serde_json::json!({})),
            resolved_by: None,
            resolved: None,
            edited: None,
            edited_by: None,
        })
        .await?
        .id();
    trx.commit().await?;
    let text = ops::text(graph.get::<MessageView>(old).await?.content())?;
    assert_eq!(text, before, "the words stay as they were written");
    assert_eq!(ops::render_references(&graph, &text).await?, "as [Retry on disconnect] says");
    let references = ops::references_json(&graph, &text).await?;
    assert_eq!(
        references,
        vec![serde_json::json!({
            "id": retry.to_base64(),
            "former_id": cited.to_base64(),
            "kind": "task",
            "name": "Retry on disconnect",
        })]
    );
    Ok(())
}

/// A principal named by a former id, whole or by its start: `--as`, `?as=` and `#/inbox?for=` all
/// read a name so (`naming::principal_named`). A handle comes first, and a start that a principal's
/// current id and another's former id share names neither.
#[tokio::test]
async fn a_principal_is_named_by_a_former_id_after_its_handle_and_a_shared_start_names_none() -> Result<()> {
    let graph = project().await?;
    let daniel = ops::ensure_principal(&graph, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let alice = ops::add_principal(&graph, "impl-alice", Some(AgentRole::Implementer), None, None, None, None).await?;
    let cited = EntityId::random().to_base64();
    renumbered(&graph, &cited, alice).await?;
    let named = |name: String| {
        let graph = graph.clone();
        async move { anyhow::Ok(naming::principal_named(&graph, &name, None).await?.map(|p| p.id())) }
    };
    assert_eq!(named(cited.clone()).await?, Some(alice), "whole");
    assert_eq!(named(cited[..8].to_string()).await?, Some(alice), "its start");
    assert_eq!(named(format!("@{}", &cited[..8])).await?, Some(alice), "with an at-sign, as `--as` is typed");
    // A handle that is also the start of a former id names the principal that has the handle.
    let shadow = ops::add_principal(&graph, "Zz9Zz9", Some(AgentRole::Reviewer), None, None, None, None).await?;
    renumbered(&graph, &starting("Zz9Zz9"), alice).await?;
    assert_eq!(named("Zz9Zz9".into()).await?, Some(shadow), "the handle comes first");
    // Daniel once had an id that starts as Alice's current id does: that start names neither.
    renumbered(&graph, &starting(&alice.to_base64()[..8]), daniel).await?;
    let refused = named(alice.to_base64()[..8].to_string()).await.expect_err("two principals start so");
    assert!(refused.downcast_ref::<Refused>().is_some(), "{refused}");
    Ok(())
}

/// A scope named by a former id, whole or by its start (`fwd decision attach`, `fwd decision list
/// --attached`, the page's box for a task's scopes), and a start that two scopes' current and
/// former ids share is refused with both listed; one that two former ids of one scope share names
/// that scope.
#[tokio::test]
async fn a_scope_is_named_by_a_former_id_and_a_start_two_scopes_share_names_neither() -> Result<()> {
    let graph = project().await?;
    let daniel = ops::ensure_principal(&graph, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let scope = |title: &str| {
        let (graph, title) = (graph.clone(), title.to_string());
        async move { ops::create_scope(&graph, daniel, CreateScope { title, ..Default::default() }).await }
    };
    let (hosting, docs) = (scope("Cloud Hosting").await?, scope("Docs").await?);
    let cited = EntityId::random().to_base64();
    renumbered(&graph, &cited, hosting).await?;
    renumbered(&graph, &starting(&cited[..8]), hosting).await?;
    assert_eq!(ops::resolve_scope(&graph, &cited).await?, hosting, "whole");
    assert_eq!(ops::resolve_scope(&graph, &cited[..8]).await?, hosting, "a start two of its former ids share");
    renumbered(&graph, &starting(&docs.to_base64()[..8]), hosting).await?;
    let refused = ops::resolve_scope(&graph, &docs.to_base64()[..8]).await.expect_err("two scopes start so");
    assert!(refused.to_string().contains("starts the ids of 2 entities (scope "), "{refused}");
    // A former id of a task is no scope's.
    let task =
        ops::create_task(&graph, daniel, CreateTask { title: "Not a scope".into(), ..Default::default() }).await?;
    let a_tasks = EntityId::random().to_base64();
    renumbered(&graph, &a_tasks, task).await?;
    assert!(ops::resolve_scope(&graph, &a_tasks).await.is_err(), "a former id of a task names no scope");
    Ok(())
}
