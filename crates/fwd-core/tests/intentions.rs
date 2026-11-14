//! Intention records (`fwd_core::intentions`) on one store in memory holding two projects, alpha
//! and beta: an intention holds the words of whoever it was written down for, and only they change
//! them or end it.

use ankurah::{policy::DEFAULT_CONTEXT, EntityId, Node, PermissiveAgent};
use ankurah_commons::intention_graph::{Intention, IntentionStatus};
use ankurah_commons::records::{AttributionView, ClosureView, LastEditView};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::intentions::{self, Parts};
use fwd_core::model::*;
use fwd_core::ops;
use fwd_core::project_graph::ProjectGraph;
use std::sync::Arc;

/// alpha's people, Ines and Omar, and its agent impl-tide; beta's agent impl-reef.
struct World {
    alpha: ProjectGraph,
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
    Ok(World { alpha, ines, omar, tide, reef })
}

/// A committed intention titled `title` that `by` records in alpha, in `said_by`'s words.
async fn intention(w: &World, by: EntityId, title: &str, said_by: Option<EntityId>) -> Result<(PlacementView, Parts)> {
    let words = format!("{title}, in the speaker's words");
    let new = Intention { title: title.into(), words, status: IntentionStatus::Committed };
    intentions::create(&w.alpha, by, new, said_by).await
}

/// Whose words the intention `record` holds.
async fn speaker(w: &World, (placement, (_, creation)): &(PlacementView, Parts)) -> Result<EntityId> {
    let attribution = w.alpha.component_of::<AttributionView>(placement).await?;
    Ok(intentions::speaker(creation, attribution.as_ref())?)
}

/// The error `result` failed with, as text.
fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
    format!("{:#}", result.expect_err("refused"))
}

#[tokio::test]
async fn an_intention_written_down_for_a_person_holds_their_words_and_an_agent_is_never_named_its_speaker() -> Result<()>
{
    let w = world().await?;
    let theirs = intention(&w, w.tide, "Readers keep their place", Some(w.ines)).await?;
    let (_, (made, creation)) = &theirs;
    assert_eq!(made.title()?, "Readers keep their place", "the views of the record made");
    assert_eq!((*creation.created_by()?, speaker(&w, &theirs).await?), (w.tide, w.ines), "written down for Ines");
    let own = intention(&w, w.tide, "Builds stay under a minute", None).await?;
    assert_eq!(speaker(&w, &own).await?, w.tide, "in its writer's words");

    let for_an_agent = Intention { title: "t".into(), words: String::new(), status: IntentionStatus::Wished };
    assert!(refusal(intentions::create(&w.alpha, w.tide, for_an_agent, Some(w.reef)).await).contains("is an agent"));
    Ok(())
}

#[tokio::test]
async fn only_the_speaker_or_someone_writing_down_their_words_rewords_or_closes_an_intention() -> Result<()> {
    let w = world().await?;
    let theirs = intention(&w, w.tide, "Readers keep their place", Some(w.ines)).await?;
    let (placement, (it, _)) = &theirs;
    let (title, words) = (it.title()?, it.words()?);
    let same = intentions::reword(&w.alpha, w.ines, &theirs, Some(&title), Some(&words), None).await?;
    assert!(!same, "nothing changed");
    assert!(w.alpha.component_of::<LastEditView>(placement).await?.is_none(), "and nothing was written");

    let new_words = Some("the list stays where I left it");
    let by_tide = intentions::reword(&w.alpha, w.tide, &theirs, None, new_words, None).await;
    assert!(refusal(by_tide).contains("only they change"));
    assert!(intentions::reword(&w.alpha, w.tide, &theirs, None, new_words, Some(w.ines)).await?, "in her words");
    assert_eq!(it.words()?, "the list stays where I left it", "the view handed in reads the change");
    let edit = w.alpha.component_of::<LastEditView>(placement).await?.expect("reworded");
    assert_eq!(edit.edited_by()?.map(|by| *by), Some(w.tide), "who wrote the change down");

    let by_omar = intentions::close(&w.alpha, w.omar, &theirs, IntentionStatus::Fulfilled, None, None).await;
    assert!(refusal(by_omar).contains("only they change"));
    Ok(())
}

#[tokio::test]
async fn an_ended_intention_is_neither_reworded_nor_ended_again_and_a_withdrawal_says_why() -> Result<()> {
    let w = world().await?;
    let it = intention(&w, w.ines, "Readers keep their place", None).await?;
    let withdrawn = intentions::close(&w.alpha, w.ines, &it, IntentionStatus::Withdrawn, None, None).await;
    assert!(refusal(withdrawn).contains("--reason"));

    intentions::close(&w.alpha, w.ines, &it, IntentionStatus::Fulfilled, Some("shipped".into()), None).await?;
    let (placement, (view, _)) = &it;
    let closure = w.alpha.component_of::<ClosureView>(placement).await?.expect("closed");
    assert_eq!((view.status()?, closure.reason()?), (IntentionStatus::Fulfilled, Some("shipped".into())));
    let again = intentions::close(&w.alpha, w.ines, &it, IntentionStatus::Withdrawn, Some("no".into()), None).await;
    assert!(refusal(again).contains("fulfilled already"));
    let reworded = intentions::reword(&w.alpha, w.ines, &it, None, Some("more"), None).await;
    assert!(refusal(reworded).contains("not reworded"));
    Ok(())
}
