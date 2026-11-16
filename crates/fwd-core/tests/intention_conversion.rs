//! `fwd admin migrate`'s move of intentions written on the Commons' models of 84ea084 onto those of
//! a9ea175 (`fwd_core::intentions::convert`), on one store in memory: each intention keeps its id,
//! its old components, its title, its words as its description, its status as `Closed`, its speaker
//! on a quotation, and its assignments; a second run changes nothing; and an intention that would
//! lose part of what it holds stops the run before anything is written.

use ankurah::model::Mutable;
use ankurah::property::{Json, Ref};
use ankurah::{policy::DEFAULT_CONTEXT, EntityId};
use ankurah_commons::cognition::IntentionView;
use ankurah_commons::records::{Attribution, ClosedView, CreatedView, EditedView};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::durable;
use fwd_core::intentions::{self, before_d229, Outcome, Parts, Quoted};
use fwd_core::model::*;
use fwd_core::ops;
use fwd_core::project_graph::{Followed, ProjectGraph};

/// When the intentions here were written down, and when the one reworded was reworded.
const WRITTEN: i64 = 1_700_000_000_000;
const REWORDED: i64 = WRITTEN + 3_600_000;
const FULFILLED: i64 = WRITTEN + 7_200_000;

/// alpha's person Ines and its agent impl-tide.
struct World {
    alpha: ProjectGraph,
    ines: EntityId,
    tide: EntityId,
}

async fn world() -> Result<World> {
    let node = durable::open(SqliteStorageEngine::open_in_memory().await?).await?;
    let alpha = ProjectGraph::ensure(node.context_async(DEFAULT_CONTEXT).await?, uuid::Uuid::new_v4(), "alpha").await?;
    let ines = ops::ensure_principal(&alpha, "ines", "Ines", PrincipalKind::Human, None).await?;
    let tide = ops::ensure_principal(&alpha, "impl-tide", "impl-tide", PrincipalKind::Agent, None).await?;
    Ok(World { alpha, ines, tide })
}

/// What an intention held on the Commons' models of 84ea084: its record, written down by `by`; its
/// speaker, when someone else's words; its last rewording, by whom and when; and how it closed.
struct Old {
    intention: before_d229::Intention,
    by: EntityId,
    speaker: Option<EntityId>,
    reworded: Option<(EntityId, i64)>,
    closure: Option<before_d229::Closure>,
}

/// Writes `old` into alpha as this build's predecessor wrote an intention, with an `assigned_to`
/// relation to impl-tide; its id.
async fn written(w: &World, old: Old) -> Result<EntityId> {
    let trx = w.alpha.begin();
    let made = trx.create_placed(&old.intention).await?;
    let record = Mutable::entity(&*made);
    trx.add(record, &before_d229::Creation { created_by: Ref::new(old.by), created: WRITTEN }).await?;
    if let Some(speaker) = old.speaker {
        trx.add(record, &Attribution { speaker: Some(Ref::new(speaker)) }).await?;
    }
    if let Some((by, at)) = old.reworded {
        trx.add(record, &before_d229::LastEdit { edited: at, edited_by: Some(Ref::new(by)) }).await?;
    }
    if let Some(closure) = &old.closure {
        trx.add(record, closure).await?;
    }
    let id = made.id();
    trx.create(&Relation {
        project: w.alpha.project(),
        kind: RelationKind::AssignedTo,
        from_id: id,
        to_id: w.tide,
        status: RelationStatus::Accepted,
        created_by: Ref::new(w.tide),
        created: WRITTEN,
        via: Json::new(serde_json::json!({})),
        note: None,
        position: None,
        closed_by: None,
        closed: None,
        closed_reason: None,
    })
    .await?;
    trx.commit().await?;
    Ok(id)
}

fn old(title: &str, status: &str) -> before_d229::Intention {
    before_d229::Intention {
        title: title.into(),
        words: format!("{title}, in the speaker's words"),
        status: status.into(),
    }
}

/// The quotations that say whose words the intention `id` holds.
async fn quotations(w: &World, id: EntityId) -> Result<Vec<(PlacementView, Quoted)>> {
    let mut found = Vec::new();
    for r in w.alpha.relations_to(id, Some(RelationKind::RelatesTo)).await? {
        found.push(w.alpha.get_placed(r.from_id()?).await?);
    }
    Ok(found)
}

/// Its new components, read as a list reads it, beside those of 84ea084, which stay.
async fn moved(w: &World, id: EntityId) -> Result<(PlacementView, Parts)> {
    let record: (PlacementView, Parts) = w.alpha.get_placed(id).await?;
    assert!(w.alpha.component_of::<before_d229::IntentionView>(&record.0).await?.is_some(), "the old ones stay");
    Ok(record)
}

/// Three intentions: one written down for Ines, one she reworded through impl-tide, and one in her
/// own words that she fulfilled. Each comes through with what it held, its speaker on a quotation
/// recorded by whoever last wrote its words down, when they did; and still assigned.
#[tokio::test]
async fn each_intention_moves_whole_keeping_its_id_and_a_second_run_changes_nothing() -> Result<()> {
    let w = world().await?;
    let for_ines = written(
        &w,
        Old {
            intention: old("Readers keep their place", "open"),
            by: w.tide,
            speaker: Some(w.ines),
            reworded: None,
            closure: None,
        },
    )
    .await?;
    let reworded = Old {
        intention: old("Builds stay under a minute", "open"),
        by: w.tide,
        speaker: Some(w.ines),
        reworded: Some((w.tide, REWORDED)),
        closure: None,
    };
    let reworded = written(&w, reworded).await?;
    let closure = before_d229::Closure {
        closed: Some(FULFILLED),
        closed_by: Some(Ref::new(w.ines)),
        reason: Some("shipped".into()),
        reason_message: None,
    };
    let fulfilled = Old {
        intention: old("Name the page", "fulfilled"),
        by: w.ines,
        speaker: None,
        reworded: None,
        closure: Some(closure),
    };
    let fulfilled = written(&w, fulfilled).await?;

    let converted = intentions::convert(&w.alpha, w.ines).await?;
    let mut added: Vec<(EntityId, Vec<&str>)> = converted.iter().map(|c| (c.intention, c.added.clone())).collect();
    added.sort();
    let mut expected = vec![
        (for_ines, vec!["cognition.intention", "records.created"]),
        (reworded, vec!["cognition.intention", "records.created", "records.edited"]),
        (fulfilled, vec!["cognition.intention", "records.created", "records.closed"]),
    ];
    expected.sort();
    assert_eq!(added, expected);

    let (placement, (intention, created)) = moved(&w, for_ines).await?;
    assert_eq!(
        (intention.title()?, intention.description()?),
        ("Readers keep their place".into(), "Readers keep their place, in the speaker's words".into())
    );
    assert_eq!((*created.by()?, created.created()?.unix_millis()), (w.tide, WRITTEN));
    assert_eq!(intentions::outcome(w.alpha.component_of::<ClosedView>(&placement).await?.as_ref())?, None);
    let [(_, (quotation, recorded))] = &quotations(&w, for_ines).await?[..] else { panic!("one quotation") };
    assert_eq!(quotation.text()?, "Readers keep their place, in the speaker's words");
    assert_eq!((*recorded.by()?, recorded.created()?.unix_millis(), quotation.said()?), (w.tide, WRITTEN, None));
    let speaker = intentions::whose_words(&w.alpha, for_ines).await?.and_then(|(speaker, _)| speaker);
    assert_eq!(speaker, Some(w.ines));

    let (placement, _) = moved(&w, reworded).await?;
    let edited = w.alpha.component_of::<EditedView>(&placement).await?.expect("reworded");
    assert_eq!((edited.edited()?.unix_millis(), edited.by()?.map(|by| *by)), (REWORDED, Some(w.tide)));
    let (speaker, recorded) = intentions::whose_words(&w.alpha, reworded).await?.expect("quoted");
    let recorded = (*recorded.by()?, recorded.created()?.unix_millis());
    assert_eq!((speaker, recorded), (Some(w.ines), (w.tide, REWORDED)), "as last written down");

    let (placement, _) = moved(&w, fulfilled).await?;
    let closed = w.alpha.component_of::<ClosedView>(&placement).await?.expect("fulfilled");
    assert_eq!(intentions::outcome(Some(&closed))?, Some(Outcome::Fulfilled));
    let closed = (closed.closed()?.map(|at| at.unix_millis()), closed.by()?.map(|by| *by), closed.reason()?);
    assert_eq!(closed, (Some(FULFILLED), Some(w.ines), Some("shipped".into())));
    let [(_, (quotation, recorded))] = &quotations(&w, fulfilled).await?[..] else { panic!("one quotation") };
    assert_eq!((*recorded.by()?, quotation.said()?), (w.ines, Some(recorded.created()?)), "her own, said then");

    let assigned: Vec<EntityId> = intentions::assigned_to(&w.alpha, w.tide)
        .await?
        .into_iter()
        .filter_map(|(id, followed)| matches!(followed, Followed::Held { .. }).then_some(id))
        .collect();
    assert_eq!(assigned.len(), 3, "still assigned: {assigned:?}");

    assert_eq!(intentions::convert(&w.alpha, w.ines).await?, vec![], "a second run");
    assert_eq!(quotations(&w, for_ines).await?.len(), 1, "and no second quotation");
    Ok(())
}

/// A closure that names the message giving its reason has no place in `Closed`, so the run stops
/// naming that intention, and the other, which would move whole, does not move either.
#[tokio::test]
async fn an_intention_that_would_lose_part_of_what_it_holds_stops_the_run_before_anything_is_written() -> Result<()> {
    let w = world().await?;
    let whole = written(
        &w,
        Old {
            intention: old("Readers keep their place", "open"),
            by: w.ines,
            speaker: None,
            reworded: None,
            closure: None,
        },
    )
    .await?;
    let closure = before_d229::Closure {
        closed: Some(FULFILLED),
        closed_by: Some(Ref::new(w.ines)),
        reason: Some("see the thread".into()),
        reason_message: Some(EntityId::from_bytes([7; 32])),
    };
    let losing = Old {
        intention: old("Name the page", "withdrawn"),
        by: w.ines,
        speaker: None,
        reworded: None,
        closure: Some(closure),
    };
    let losing = written(&w, losing).await?;

    let refused = format!("{:#}", intentions::convert(&w.alpha, w.ines).await.expect_err("refused"));
    assert!(refused.contains(&losing.to_base64()) && refused.contains("message giving the reason"), "{refused}");
    assert!(!refused.contains(&whole.to_base64()), "{refused}");
    let placement: PlacementView = w.alpha.get(whole).await?;
    assert!(w.alpha.component_of::<IntentionView>(&placement).await?.is_none(), "nothing written");
    assert!(w.alpha.component_of::<CreatedView>(&placement).await?.is_none());
    assert!(quotations(&w, whole).await?.is_empty());
    Ok(())
}
