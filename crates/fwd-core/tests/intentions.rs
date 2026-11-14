//! Intention records (`fwd_core::intentions`) on one store in memory holding two projects, alpha
//! and beta: an intention records whose words it holds, and only they change them.

use ankurah::{policy::DEFAULT_CONTEXT, EntityId, Node, PermissiveAgent};
use ankurah_commons::intention_graph::IntentionStatus;
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::intentions::{self, NewIntention};
use fwd_core::model::*;
use fwd_core::ops;
use fwd_core::project_graph::ProjectGraph;
use std::sync::Arc;

/// alpha's people, Ines and Omar, and its agent impl-tide; beta's agent impl-reef.
struct World {
    alpha: ProjectGraph,
    beta: ProjectGraph,
    ines: EntityId,
    omar: EntityId,
    tide: EntityId,
    reef: EntityId,
}

async fn world() -> Result<World> {
    let node = Node::new_durable(Arc::new(SqliteStorageEngine::open_in_memory().await?), PermissiveAgent::new());
    node.system.create().await?;
    let ctx = node.context_async(DEFAULT_CONTEXT).await?;
    let alpha = ProjectGraph::ensure(ctx.clone(), uuid::Uuid::new_v4(), "alpha").await?;
    let beta = ProjectGraph::ensure(ctx, uuid::Uuid::new_v4(), "beta").await?;
    let person = |graph, handle: &'static str| ops::ensure_principal(graph, handle, handle, PrincipalKind::Human, None);
    let agent = |graph, handle: &'static str| ops::ensure_principal(graph, handle, handle, PrincipalKind::Agent, None);
    let (ines, omar) = (person(&alpha, "ines").await?, person(&alpha, "omar").await?);
    let (tide, reef) = (agent(&alpha, "impl-tide").await?, agent(&beta, "impl-reef").await?);
    Ok(World { alpha, beta, ines, omar, tide, reef })
}

/// An intention titled `title` that `by` records in `graph`'s project, in `said_by`'s words.
async fn intention(graph: &ProjectGraph, by: EntityId, title: &str, said_by: Option<EntityId>) -> Result<EntityId> {
    let words = format!("{title}, in the speaker's words");
    let new = NewIntention { title: title.into(), words, said_by, status: IntentionStatus::Committed };
    intentions::create(graph, by, new).await
}

/// The error `result` failed with, as text.
fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
    format!("{:#}", result.expect_err("refused"))
}

#[tokio::test]
async fn an_intention_records_whose_words_it_holds_and_only_they_change_or_close_it() -> Result<()> {
    let w = world().await?;
    let theirs = intention(&w.alpha, w.tide, "Readers keep their place", Some(w.ines)).await?;
    let own = intention(&w.alpha, w.tide, "Builds stay under a minute", None).await?;
    let read = intentions::intention(&w.alpha, theirs).await?;
    assert_eq!((read.authorship.written_by, read.authorship.speaker), (w.tide, w.ines), "written down for Ines");
    assert_eq!(intentions::intention(&w.alpha, own).await?.authorship.speaker, w.tide, "in its writer's words");
    let said_by_an_agent = NewIntention {
        title: "t".into(),
        words: String::new(),
        said_by: Some(w.reef),
        status: IntentionStatus::Wished,
    };
    assert!(refusal(intentions::create(&w.alpha, w.tide, said_by_an_agent).await).contains("is an agent"));

    let same = (Some(read.title.as_str()), Some(read.words.as_str()));
    assert!(!intentions::reword(&w.alpha, w.ines, theirs, same.0, same.1, None).await?, "nothing changed");
    let words = Some("the list stays where I left it");
    assert!(refusal(intentions::reword(&w.alpha, w.tide, theirs, None, words, None).await).contains("only they change"));
    assert!(intentions::reword(&w.alpha, w.tide, theirs, None, words, Some(w.ines)).await?, "in her words");
    let reworded = intentions::intention(&w.alpha, theirs).await?;
    assert_eq!(reworded.words, "the list stays where I left it");
    assert_eq!(reworded.authorship.edited.and_then(|e| e.by), Some(w.tide), "who wrote the change down");
    intentions::reword(&w.alpha, w.ines, theirs, Some("Readers keep their places"), None, None).await?;
    assert_eq!(intentions::intention(&w.alpha, theirs).await?.title, "Readers keep their places", "its title too");

    let withdrawn = intentions::close(&w.alpha, w.ines, theirs, IntentionStatus::Withdrawn, None, None).await;
    assert!(refusal(withdrawn).contains("--reason"));
    let by_omar = intentions::close(&w.alpha, w.omar, theirs, IntentionStatus::Fulfilled, None, None).await;
    assert!(refusal(by_omar).contains("only they change"));
    intentions::close(&w.alpha, w.ines, theirs, IntentionStatus::Fulfilled, Some("shipped".into()), None).await?;
    let closed = intentions::intention(&w.alpha, theirs).await?;
    assert_eq!(
        (closed.status, closed.closed.and_then(|c| c.reason)),
        (IntentionStatus::Fulfilled, Some("shipped".into()))
    );
    let again = intentions::close(&w.alpha, w.ines, theirs, IntentionStatus::Withdrawn, Some("no".into()), None).await;
    assert!(refusal(again).contains("fulfilled already"));
    assert!(
        refusal(intentions::reword(&w.alpha, w.ines, theirs, None, Some("more"), None).await).contains("not reworded")
    );
    assert!(intentions::intentions(&w.beta).await?.is_empty(), "beta's list holds beta's alone");
    Ok(())
}
