//! Former ids (`fwd_core::former_ids`): an id a record had before it was renumbered names the
//! record wherever an id is taken, whole or as a prefix, beside its current id; a prefix that names
//! two records is refused with both listed; and a text written before the renumbering, which
//! stores the former id, reads as naming the record.

use ankurah::{policy::DEFAULT_CONTEXT, EntityId, Node, PermissiveAgent};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::model::*;
use fwd_core::ops::{self, CreateTask, Kind, MessageContext};
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
    assert_eq!(ops::render_references(&graph, &text).await, "as [Retry on disconnect] says");
    let references = ops::references_json(&graph, &text).await;
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
