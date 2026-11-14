//! Intention records and readings (`fwd_core::intentions`) on one store in memory holding two
//! projects, alpha and beta: an intention records whose words it holds and is reworded and closed;
//! a party has one reading of a record; a reading of a reading answers the revision it read; the
//! intention's speaker alone confirms an agent's reading, until the reading changes; what a
//! principal works under reads in the order to discuss it; and the two relation kinds join only
//! the records they are for.

use ankurah::property::{Json, Ref};
use ankurah::{policy::DEFAULT_CONTEXT, EntityId, Node, PermissiveAgent};
use ankurah_commons::intention_graph::{IntentionStatus, Stance};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::intentions::readings::{self, ReadingChanged, SetReading};
use fwd_core::intentions::works_under::{works_under, Standing};
use fwd_core::intentions::{self, NewIntention};
use fwd_core::model::*;
use fwd_core::ops::{self, CreateTask, Kind};
use fwd_core::project_graph::{ProjectGraph, Whose};
use serde_json::json;
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

/// `by`'s reading of `of`, in its own name.
async fn read(graph: &ProjectGraph, by: EntityId, of: EntityId, words: &str, stance: Stance) -> Result<EntityId> {
    let spec = SetReading { of, words: words.into(), stance, said_by: None, as_read: None };
    readings::set(graph, by, spec).await
}

/// Returns once the wall clock has passed `ms`: a confirmation is judged by the millisecond times
/// records carry, so a write in the same millisecond as the one before would read as simultaneous.
fn past(ms: i64) {
    while fwd_core::now_ms() <= ms {
        std::hint::spin_loop();
    }
}

/// The error `result` failed with, as text.
fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
    format!("{:#}", result.expect_err("refused"))
}

#[tokio::test]
async fn an_intention_records_whose_words_it_holds_and_is_reworded_then_closed_once() -> Result<()> {
    let w = world().await?;
    let theirs = intention(&w.alpha, w.tide, "Readers keep their place", Some(w.ines)).await?;
    let own = intention(&w.alpha, w.tide, "Builds stay under a minute", None).await?;
    let read = intentions::intention(&w.alpha, theirs).await?;
    assert_eq!((read.authorship.written_by, read.authorship.speaker), (w.tide, w.ines), "written down for Ines");
    assert_eq!(intentions::intention(&w.alpha, own).await?.authorship.speaker, w.tide, "in its writer's words");
    assert_eq!((read.status, read.authorship.edited), (IntentionStatus::Committed, None));

    let same = (Some(read.title.as_str()), Some(read.words.as_str()));
    assert!(!intentions::reword(&w.alpha, w.ines, theirs, same.0, same.1).await?, "nothing changed");
    assert_eq!(intentions::intention(&w.alpha, theirs).await?.authorship.edited, None, "and no edit recorded");
    assert!(intentions::reword(&w.alpha, w.ines, theirs, None, Some("the list stays where I left it")).await?);
    let reworded = intentions::intention(&w.alpha, theirs).await?;
    assert_eq!(
        (reworded.title.as_str(), reworded.words.as_str()),
        ("Readers keep their place", "the list stays where I left it")
    );
    assert_eq!(reworded.authorship.edited.and_then(|e| e.by), Some(w.ines), "who reworded it");
    assert!(refusal(intentions::reword(&w.alpha, w.ines, theirs, Some("  "), None).await).contains("blank"));

    let closing =
        NewIntention { title: "Done".into(), words: String::new(), said_by: None, status: IntentionStatus::Fulfilled };
    assert!(refusal(intentions::create(&w.alpha, w.tide, closing).await).contains("afterwards"));
    intentions::close(&w.alpha, w.ines, theirs, IntentionStatus::Fulfilled, Some("shipped".into())).await?;
    let closed = intentions::intention(&w.alpha, theirs).await?;
    let by_and_why = closed.closed.map(|c| (c.by, c.reason));
    assert_eq!((closed.status, by_and_why), (IntentionStatus::Fulfilled, Some((Some(w.ines), Some("shipped".into())))));
    let again = intentions::close(&w.alpha, w.ines, theirs, IntentionStatus::Withdrawn, None).await;
    assert!(refusal(again).contains("fulfilled already"));

    let listed: Vec<EntityId> = intentions::intentions(&w.alpha).await?.iter().map(|i| i.id).collect();
    let mut both = vec![theirs, own];
    both.sort();
    assert_eq!(listed, both);
    assert!(intentions::intentions(&w.beta).await?.is_empty(), "beta's list holds beta's alone");
    Ok(())
}

#[tokio::test]
async fn a_party_has_one_reading_of_a_record_which_a_second_set_rewords_wherever_it_was_placed() -> Result<()> {
    let w = world().await?;
    let it = intention(&w.alpha, w.ines, "Readers keep their place", None).await?;
    let first = read(&w.alpha, w.tide, it, "the list keeps its scroll", Stance::Agree).await?;
    let unchanged = readings::readings_of(&w.alpha, it).await?;
    assert_eq!(read(&w.alpha, w.tide, it, "the list keeps its scroll", Stance::Agree).await?, first);
    assert_eq!(readings::readings_of(&w.alpha, it).await?, unchanged, "a set that changes nothing writes nothing");

    assert_eq!(read(&w.alpha, w.tide, it, "which list?", Stance::Question).await?, first, "rewords it");
    let mine = readings::readings_of(&w.alpha, it).await?;
    assert_eq!(mine.len(), 1);
    assert_eq!((mine[0].stance, mine[0].authorship.edited.and_then(|e| e.by)), (Some(Stance::Question), Some(w.tide)));
    assert_ne!(mine[0].revision, unchanged[0].revision);

    let theirs = read(&w.beta, w.reef, it, "the reader's scroll", Stance::Agree).await?;
    let all = readings::readings_of(&w.alpha, it).await?;
    let placed: Vec<(EntityId, EntityId)> = all.iter().map(|r| (r.id, r.project)).collect();
    assert!(placed.contains(&(theirs, w.beta.record())), "placed in beta, read from alpha: {placed:?}");
    assert_eq!(all.len(), 2);
    Ok(())
}

#[tokio::test]
async fn a_reading_of_a_reading_answers_the_revision_it_read() -> Result<()> {
    let w = world().await?;
    let it = intention(&w.alpha, w.ines, "Readers keep their place", None).await?;
    let agents = read(&w.alpha, w.tide, it, "the list keeps its scroll", Stance::Agree).await?;
    let revision = readings::readings_of(&w.alpha, it).await?[0].revision.clone();
    let correction = |as_read: Option<String>| SetReading {
        of: agents,
        words: "the scroll and the selection".into(),
        stance: Stance::Disagree,
        said_by: None,
        as_read,
    };

    assert!(refusal(readings::set(&w.alpha, w.ines, correction(None)).await).contains("--as-read"));
    let stale = readings::set(&w.alpha, w.ines, correction(Some("000000000000".into()))).await;
    assert!(stale.expect_err("stale").is::<ReadingChanged>());
    readings::set(&w.alpha, w.ines, correction(Some(revision.clone()))).await?;
    assert_eq!(readings::readings_of(&w.alpha, agents).await?[0].party(), w.ines);
    let on_an_intention = SetReading { as_read: Some(revision), ..correction(None) };
    let on_an_intention = SetReading { of: it, ..on_an_intention };
    assert!(refusal(readings::set(&w.alpha, w.omar, on_an_intention).await).contains("reads an intention"));
    Ok(())
}

#[tokio::test]
async fn the_speaker_alone_confirms_an_agents_reading_until_the_reading_changes() -> Result<()> {
    let w = world().await?;
    let it = intention(&w.alpha, w.tide, "Readers keep their place", Some(w.ines)).await?;
    ops::link(&w.alpha, w.tide, RelationKind::WorksUnder, w.tide, it, None, false, json!({})).await?;
    let agents = read(&w.alpha, w.tide, it, "the list keeps its scroll", Stance::Agree).await?;
    let revision = || async { Ok::<_, anyhow::Error>(readings::readings_of(&w.alpha, it).await?[0].revision.clone()) };
    let standing = || async { Ok::<_, anyhow::Error>(works_under(&w.alpha, w.tide).await?.intentions[0].clone()) };

    let by_omar = readings::confirm(&w.alpha, w.omar, agents, revision().await?, None, None).await;
    assert!(refusal(by_omar).contains("only the intention's speaker, ines"));
    let wordless = readings::confirm(&w.alpha, w.tide, agents, revision().await?, None, Some(w.ines)).await;
    assert!(refusal(wordless).contains("their own words"));
    assert_eq!(standing().await?.standing(), Standing::Unconfirmed);

    readings::confirm(&w.alpha, w.ines, agents, revision().await?, None, None).await?;
    let confirmed = standing().await?.confirmation.expect("confirmed by Ines");
    assert_eq!((confirmed.by, standing().await?.standing()), (w.ines, Standing::Confirmed));

    past(confirmed.at);
    read(&w.alpha, w.tide, it, "the list keeps its scroll and its selection", Stance::Agree).await?;
    assert_eq!(standing().await?.confirmation, None, "reworded since it was confirmed");

    let in_her_words = Some("yes, both".to_string());
    readings::confirm(&w.alpha, w.tide, agents, revision().await?, in_her_words, Some(w.ines)).await?;
    assert_eq!(standing().await?.confirmation.map(|c| c.by), Some(w.ines), "written down for her by the agent");
    Ok(())
}

#[tokio::test]
async fn what_a_principal_works_under_reads_what_to_discuss_first_then_unread_unconfirmed_and_confirmed() -> Result<()>
{
    let w = world().await?;
    let mut ids = Vec::new();
    for title in ["questioned", "unread", "unconfirmed", "confirmed", "elsewhere", "retracted"] {
        let (graph, by) = if title == "elsewhere" { (&w.beta, w.reef) } else { (&w.alpha, w.ines) };
        let id = intention(graph, by, title, None).await?;
        ops::link(&w.alpha, w.tide, RelationKind::WorksUnder, w.tide, id, None, false, json!({})).await?;
        ids.push(id);
    }
    let [questioned, unread, unconfirmed, confirmed, elsewhere, retracted] = ids[..] else { unreachable!() };
    ops::unlink(&w.alpha, w.tide, RelationKind::WorksUnder, w.tide, retracted, "no longer").await?;
    ops::link(&w.alpha, w.omar, RelationKind::WorksUnder, w.omar, unread, None, false, json!({})).await?;
    // A relation to an id the node holds no intention by, as one whose intention is not held here.
    let gone = EntityId::random();
    let trx = w.alpha.begin();
    trx.create(&Relation {
        project: w.alpha.project(),
        kind: RelationKind::WorksUnder,
        from_id: w.tide,
        to_id: gone,
        status: RelationStatus::Accepted,
        created_by: Ref::new(w.tide),
        created: 1,
        via: Json::new(json!({})),
        note: None,
        position: None,
        closed_by: None,
        closed: None,
        closed_reason: None,
    })
    .await?;
    trx.commit().await?;

    read(&w.alpha, w.tide, questioned, "which list?", Stance::Question).await?;
    read(&w.alpha, w.tide, unconfirmed, "as I read it", Stance::Agree).await?;
    let to_confirm = read(&w.alpha, w.tide, confirmed, "as I read it", Stance::Agree).await?;
    read(&w.alpha, w.tide, elsewhere, "not this", Stance::Disagree).await?;
    let revision = readings::readings_of(&w.alpha, confirmed).await?[0].revision.clone();
    readings::confirm(&w.alpha, w.ines, to_confirm, revision, None, None).await?;
    let read_at = readings::readings_of(&w.alpha, unconfirmed).await?[0].authorship.changed();
    past(read_at);
    intentions::reword(&w.alpha, w.ines, unconfirmed, None, Some("said again")).await?;

    let held = works_under(&w.alpha, w.tide).await?;
    let mut discuss = vec![questioned, elsewhere];
    discuss.sort();
    let order: Vec<EntityId> = held.intentions.iter().map(|i| i.intention.id).collect();
    assert_eq!(order, [discuss, vec![unread, unconfirmed, confirmed]].concat());
    let standings: Vec<Standing> = held.intentions.iter().map(|i| i.standing()).collect();
    let expected =
        [Standing::ToDiscuss, Standing::ToDiscuss, Standing::Unread, Standing::Unconfirmed, Standing::Confirmed];
    assert_eq!(standings, expected);
    let of = |id| held.intentions.iter().find(|i| i.intention.id == id).expect("listed");
    assert!(matches!(&of(elsewhere).project, Whose::Other(p) if p.name == "beta"), "named with its project");
    assert!(of(unconfirmed).reworded_since_reading && !of(confirmed).reworded_since_reading);
    assert_eq!(held.not_held, vec![gone]);
    Ok(())
}

#[tokio::test]
async fn works_under_joins_a_principal_to_an_intention_and_serves_a_task_or_an_intention_to_one() -> Result<()> {
    let w = world().await?;
    let broad = intention(&w.alpha, w.ines, "Readers keep their place", None).await?;
    let narrow = intention(&w.alpha, w.ines, "The list keeps its scroll", None).await?;
    let task = ops::create_task(&w.alpha, w.ines, CreateTask { title: "Save the scroll".into(), ..Default::default() })
        .await?;
    let link = |kind, from, to| ops::link(&w.alpha, w.tide, kind, from, to, None, false, json!({}));

    let under = link(RelationKind::WorksUnder, w.tide, broad).await?;
    assert_eq!(link(RelationKind::WorksUnder, w.tide, broad).await?, under, "made once");
    assert!(refusal(link(RelationKind::WorksUnder, task, broad).await).contains("not a task to an intention"));
    assert!(refusal(link(RelationKind::WorksUnder, w.tide, task).await).contains("not a principal to a task"));
    link(RelationKind::Serves, task, narrow).await?;
    link(RelationKind::Serves, narrow, broad).await?;
    assert!(refusal(link(RelationKind::Serves, task, task).await).contains("not a task to a task"));

    assert_eq!(ops::describe(&w.alpha, broad).await?, Some((Kind::Intention, "Readers keep their place".into())));
    let start = &broad.to_base64()[..8];
    assert_eq!(ops::resolve_id(&w.alpha, start, &[Kind::Intention]).await?, broad, "this project's");
    assert_eq!(ops::resolve_id(&w.beta, start, &[Kind::Intention]).await?, broad, "from another project");
    let reading = read(&w.alpha, w.tide, broad, "as I read it", Stance::Agree).await?;
    assert_eq!(ops::resolve_id(&w.alpha, &reading.to_base64()[..8], &[Kind::Reading]).await?, reading);
    Ok(())
}
