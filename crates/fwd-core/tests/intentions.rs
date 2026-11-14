//! Intention records (`fwd_core::intentions`) on one store in memory holding two projects, alpha
//! and beta: an intention holds the words of whoever it was written down for, and only they change
//! them or end it; a principal assigns an intention of any project to itself, and reads back what is
//! assigned to it; an agent takes one off itself alone, and a person off anyone.

use ankurah::{policy::DEFAULT_CONTEXT, EntityId, Node, PermissiveAgent};
use ankurah_commons::intention_graph::IntentionStatus;
use ankurah_commons::records::{AttributionView, ClosureView, LastEditView};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::intentions::{self, Parts};
use fwd_core::model::*;
use fwd_core::ops;
use fwd_core::project_graph::{Followed, InProjectView, ProjectGraph, Whose};
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

/// An intention titled `title` that `by` records in alpha, in `said_by`'s words.
async fn intention(w: &World, by: EntityId, title: &str, said_by: Option<EntityId>) -> Result<(PlacementView, Parts)> {
    intentions::create(&w.alpha, by, title, format!("{title}, in the speaker's words"), said_by).await
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

    let for_an_agent = intentions::create(&w.alpha, w.tide, "t", String::new(), Some(w.reef)).await;
    assert!(refusal(for_an_agent).contains("is an agent"));
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

/// The ids of the intentions `assigned` names, in its order.
fn ids(assigned: &[(EntityId, Followed<(PlacementView, Parts)>)]) -> Vec<EntityId> {
    assigned.iter().map(|(id, _)| *id).collect()
}

#[tokio::test]
async fn an_intention_assigned_twice_is_assigned_once_and_unassigning_it_takes_it_off() -> Result<()> {
    let w = world().await?;
    let it = intention(&w, w.ines, "Readers keep their place", None).await?;
    let first = intentions::assign(&w.alpha, w.tide, &it, w.tide).await?;
    assert_eq!((first.kind()?, first.status()?), (RelationKind::AssignedTo, RelationStatus::Accepted));
    let again = intentions::assign(&w.alpha, w.tide, &it, w.tide).await?;
    assert_eq!(again.id(), first.id(), "assigning twice keeps one");
    assert_eq!(ids(&intentions::assigned_to(&w.alpha, w.tide).await?), vec![it.0.id()]);

    assert_eq!(intentions::unassign(&w.alpha, w.tide, &it, w.tide).await?, 1);
    assert!(intentions::assigned_to(&w.alpha, w.tide).await?.is_empty(), "taken off");
    assert_eq!(intentions::unassign(&w.alpha, w.tide, &it, w.tide).await?, 0, "assigned no more");
    Ok(())
}

#[tokio::test]
async fn a_principal_assigns_an_intention_to_itself_alone() -> Result<()> {
    let w = world().await?;
    let it = intention(&w, w.ines, "Readers keep their place", None).await?;
    let to_omar = intentions::assign(&w.alpha, w.tide, &it, w.omar).await;
    assert!(refusal(to_omar).contains("leave out --to"));
    assert!(intentions::assigned_to(&w.alpha, w.omar).await?.is_empty());
    Ok(())
}

/// An agent takes an intention off another principal not at all, a refusal saying why, and a
/// person takes one off anyone; `unlink` takes none off, refusing as `link` does.
#[tokio::test]
async fn an_agent_takes_an_intention_off_itself_alone_and_a_person_off_anyone() -> Result<()> {
    let w = world().await?;
    let it = intention(&w, w.ines, "Readers keep their place", None).await?;
    intentions::assign(&w.alpha, w.tide, &it, w.tide).await?;
    intentions::assign(&w.alpha, w.omar, &it, w.omar).await?;
    let by_tide = intentions::unassign(&w.alpha, w.tide, &it, w.omar).await;
    assert!(refusal(by_tide).contains("is for that principal or a person to decide"));
    let unlinked = ops::unlink(&w.alpha, w.omar, RelationKind::AssignedTo, it.0.id(), w.tide, "done").await;
    assert!(refusal(unlinked).contains("`fwd intention assign <intention>`, not linked"));
    assert_eq!(ids(&intentions::assigned_to(&w.alpha, w.omar).await?), vec![it.0.id()], "still Omar's");

    assert_eq!(intentions::unassign(&w.alpha, w.ines, &it, w.tide).await?, 1, "a person takes it off an agent");
    assert!(intentions::assigned_to(&w.alpha, w.tide).await?.is_empty());
    Ok(())
}

/// beta's intention, assigned from alpha to alpha's agent: the assignment lives with the
/// intention, in beta, and alpha reads it back named with beta, then takes it off there.
#[tokio::test]
async fn an_intention_of_another_project_is_assigned_where_it_lives_and_read_back_with_its_project() -> Result<()> {
    let w = world().await?;
    let theirs = intentions::create(&w.beta, w.reef, "Builds stay under a minute", String::new(), None).await?;
    let assigned = intentions::assign(&w.alpha, w.tide, &theirs, w.tide).await?;
    assert_eq!(assigned.project_id()?, w.beta.record(), "with the intention, in beta");
    match &intentions::assigned_to(&w.alpha, w.tide).await?[..] {
        [(id, Followed::Held { project: Whose::Other(p), .. })] => {
            assert_eq!((*id, p.name.as_str()), (theirs.0.id(), "beta"))
        }
        other => panic!("beta's intention, read from alpha: {other:?}"),
    }
    assert_eq!(intentions::unassign(&w.alpha, w.tide, &theirs, w.tide).await?, 1, "taken off in beta");
    Ok(())
}

/// An accepted `assigned_to` from `intention` to `principal`, made at `created`, written as a write
/// from another node leaves one, so that the order of two does not rest on a clock's resolution.
async fn assigned_at(w: &World, intention: EntityId, principal: EntityId, created: i64) -> Result<()> {
    let trx = w.alpha.begin();
    trx.create(&Relation {
        project: w.alpha.project(),
        kind: RelationKind::AssignedTo,
        from_id: intention,
        to_id: principal,
        status: RelationStatus::Accepted,
        created_by: ankurah::property::Ref::new(principal),
        created,
        via: ankurah::property::Json::new(serde_json::json!({})),
        note: None,
        position: None,
        closed_by: None,
        closed: None,
        closed_reason: None,
    })
    .await?;
    trx.commit().await?;
    Ok(())
}

/// What is assigned to impl-tide: the first assigned first, one two relations assign listed once,
/// one since fulfilled among them, and neither what is assigned to Omar nor what was taken off.
#[tokio::test]
async fn the_intentions_assigned_to_a_principal_come_once_each_the_first_assigned_first_ended_ones_included(
) -> Result<()> {
    let w = world().await?;
    let first = intention(&w, w.ines, "first", None).await?;
    let second = intention(&w, w.ines, "second", None).await?;
    let omars = intention(&w, w.ines, "Omar's", None).await?;
    let dropped = intention(&w, w.ines, "dropped", None).await?;
    assigned_at(&w, second.0.id(), w.tide, 10).await?;
    assigned_at(&w, first.0.id(), w.tide, 20).await?;
    assigned_at(&w, first.0.id(), w.tide, 30).await?;
    assigned_at(&w, omars.0.id(), w.omar, 5).await?;
    intentions::assign(&w.alpha, w.tide, &dropped, w.tide).await?;
    intentions::unassign(&w.alpha, w.tide, &dropped, w.tide).await?;
    intentions::close(&w.alpha, w.ines, &second, IntentionStatus::Fulfilled, None, None).await?;

    assert_eq!(ids(&intentions::assigned_to(&w.alpha, w.tide).await?), vec![second.0.id(), first.0.id()]);
    Ok(())
}
