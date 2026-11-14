//! Intention records and readings (`fwd_core::intentions`) on one store in memory holding two
//! projects, alpha and beta: an intention records whose words it holds, and only they change them;
//! a party has one standing reading of a record, reworded where it is placed; a reading answers the
//! wording it read; the intention's speaker alone confirms an agent's reading, while it reads as
//! confirmed; what a principal works under reads in the order to discuss it; and the two relation
//! kinds join only the records they are for.

use ankurah::property::{Json, Ref};
use ankurah::{policy::DEFAULT_CONTEXT, EntityId, Node, PermissiveAgent};
use ankurah_commons::intention_graph::{IntentionStatus, Reading, Stance};
use ankurah_commons::records::Creation;
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::intentions::readings::{self, ChangedSinceRead, ReadingRecord, SetReading};
use fwd_core::intentions::works_under::{works_under, Standing, WorkingIntention};
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

/// `by`'s reading of the intention `of`, in its own name.
async fn read(graph: &ProjectGraph, by: EntityId, of: EntityId, words: &str, stance: Stance) -> Result<EntityId> {
    let spec = SetReading { of, words: words.into(), stance, said_by: None, as_read: None };
    readings::set(graph, by, spec).await
}

/// The reading `id` among the readings of `of`.
async fn reading(graph: &ProjectGraph, of: EntityId, id: EntityId) -> Result<ReadingRecord> {
    let all = readings::readings_of(graph, of).await?;
    all.into_iter().find(|r| r.id == id).ok_or_else(|| anyhow::anyhow!("no reading {}", id.to_base64()))
}

/// The one intention `principal` works under.
async fn only(graph: &ProjectGraph, principal: EntityId) -> Result<WorkingIntention> {
    let mut held = works_under(graph, principal).await?.intentions;
    assert_eq!(held.len(), 1, "{held:?}");
    Ok(held.remove(0))
}

/// The error `result` failed with, as text.
fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
    format!("{:#}", result.expect_err("refused"))
}

/// A reading of `of` that `party` wrote, of the wording `read`, made through the graph's lower
/// layer, as a write that raced another, or answered a wording since changed, leaves one.
async fn written_below(
    graph: &ProjectGraph,
    party: EntityId,
    of: EntityId,
    stance: Stance,
    read: &str,
) -> Result<EntityId> {
    let trx = graph.begin();
    let made = trx.create_placed(&Reading { of, words: "below".into(), stance: Some(stance) }).await?;
    let record = ankurah::model::Mutable::entity(&*made);
    trx.add(record, &Creation { created_by: Ref::new(party), created: 1 }).await?;
    trx.add(record, &WordingRead { revision: read.into() }).await?;
    let id = made.id();
    trx.commit().await?;
    Ok(id)
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
    assert!(refusal(intentions::reword(&w.alpha, w.tide, theirs, None, words, None).await).contains("write a reading"));
    assert!(intentions::reword(&w.alpha, w.tide, theirs, None, words, Some(w.ines)).await?, "in her words");
    let reworded = intentions::intention(&w.alpha, theirs).await?;
    assert_eq!(reworded.words, "the list stays where I left it");
    assert_eq!(reworded.authorship.edited.and_then(|e| e.by), Some(w.tide), "who wrote the change down");
    assert_ne!(reworded.revision, read.revision);
    intentions::reword(&w.alpha, w.ines, theirs, Some("Readers keep their places"), None, None).await?;
    assert_ne!(intentions::intention(&w.alpha, theirs).await?.revision, reworded.revision, "its title too");

    let withdrawn = intentions::close(&w.alpha, w.ines, theirs, IntentionStatus::Withdrawn, None, None).await;
    assert!(refusal(withdrawn).contains("--reason"));
    let by_omar = intentions::close(&w.alpha, w.omar, theirs, IntentionStatus::Fulfilled, None, None).await;
    assert!(refusal(by_omar).contains("write a reading"));
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

#[tokio::test]
async fn a_party_has_one_reading_of_a_record_which_a_second_set_rewords_where_it_is_placed() -> Result<()> {
    let w = world().await?;
    let it = intention(&w.alpha, w.ines, "Readers keep their place", None).await?;
    let first = read(&w.alpha, w.tide, it, "the list keeps its scroll", Stance::Agree).await?;
    let before = reading(&w.alpha, it, first).await?;
    assert_eq!(read(&w.alpha, w.tide, it, "the list keeps its scroll", Stance::Agree).await?, first);
    assert_eq!(reading(&w.alpha, it, first).await?, before, "a set that changes nothing writes nothing");
    assert_eq!(read(&w.alpha, w.tide, it, "which list?", Stance::Question).await?, first, "rewords it");
    let reworded = reading(&w.alpha, it, first).await?;
    assert_eq!(
        (reworded.stance, reworded.authorship.edited.and_then(|e| e.by)),
        (Some(Stance::Question), Some(w.tide))
    );
    assert_ne!(reworded.revision, before.revision);

    // impl-reef, of beta, reads alpha's intention: its reading is placed in beta, and reworded there.
    let theirs = read(&w.beta, w.reef, it, "the reader's scroll", Stance::Agree).await?;
    assert_eq!(read(&w.alpha, w.reef, it, "the reader's scroll and selection", Stance::Agree).await?, theirs);
    let there = reading(&w.alpha, it, theirs).await?;
    assert_eq!((there.project, there.words.as_str()), (w.beta.record(), "the reader's scroll and selection"));

    // A person's reading, reworded by an agent writing down her words, records that agent as editor.
    let hers = read(&w.alpha, w.ines, it, "as I said", Stance::Agree).await?;
    let in_her_words = SetReading {
        of: it,
        words: "as I said it".into(),
        stance: Stance::Agree,
        said_by: Some(w.ines),
        as_read: None,
    };
    readings::set(&w.alpha, w.tide, in_her_words).await?;
    assert_eq!(reading(&w.alpha, it, hers).await?.authorship.edited.and_then(|e| e.by), Some(w.tide));
    assert_eq!(readings::readings_of(&w.alpha, it).await?.len(), 3);
    Ok(())
}

#[tokio::test]
async fn a_reading_answers_the_wording_it_read_and_a_stale_one_is_refused() -> Result<()> {
    let w = world().await?;
    let it = intention(&w.alpha, w.ines, "Readers keep their place", None).await?;
    let wording = intentions::intention(&w.alpha, it).await?.revision;
    let on = |of, as_read: Option<&str>| SetReading {
        of,
        words: "the scroll and the selection".into(),
        stance: Stance::Disagree,
        said_by: None,
        as_read: as_read.map(str::to_string),
    };
    let stale = readings::set(&w.alpha, w.tide, on(it, Some("000000000000"))).await;
    assert!(stale.expect_err("stale").is::<ChangedSinceRead>(), "of an intention");
    let agents = readings::set(&w.alpha, w.tide, on(it, Some(&wording))).await?;
    assert_eq!(reading(&w.alpha, it, agents).await?.read, wording);

    assert!(refusal(readings::set(&w.alpha, w.ines, on(agents, None)).await).contains("--as-read"));
    let stale = readings::set(&w.alpha, w.ines, on(agents, Some("000000000000"))).await;
    assert!(stale.expect_err("stale").is::<ChangedSinceRead>(), "of a reading");
    let revision = reading(&w.alpha, it, agents).await?.revision;
    let correction = readings::set(&w.alpha, w.ines, on(agents, Some(&revision))).await?;
    assert_eq!(reading(&w.alpha, agents, correction).await?.read, revision);
    Ok(())
}

#[tokio::test]
async fn the_speaker_alone_confirms_an_agents_reading_as_it_reads_and_a_rewording_unconfirms_it() -> Result<()> {
    let w = world().await?;
    let it = intention(&w.alpha, w.tide, "Readers keep their place", Some(w.ines)).await?;
    ops::link(&w.alpha, w.tide, RelationKind::WorksUnder, w.tide, it, None, false, json!({})).await?;
    let agents = read(&w.alpha, w.tide, it, "the list keeps its scroll", Stance::Agree).await?;
    let revision = || async { Ok::<_, anyhow::Error>(reading(&w.alpha, it, agents).await?.revision) };

    let by_omar = readings::confirm(&w.alpha, w.omar, agents, revision().await?, None, None).await;
    assert!(refusal(by_omar).contains("only the intention's speaker, ines"));
    let wordless = SetReading {
        of: agents,
        words: " ".into(),
        stance: Stance::Agree,
        said_by: Some(w.ines),
        as_read: Some(revision().await?),
    };
    assert!(refusal(readings::set(&w.alpha, w.tide, wordless).await).contains("their own words"), "through set too");
    assert_eq!(only(&w.alpha, w.tide).await?.standing(), Standing::Unconfirmed);

    readings::confirm(&w.alpha, w.ines, agents, revision().await?, None, None).await?;
    assert_eq!(only(&w.alpha, w.tide).await?.standing(), Standing::Confirmed);
    read(&w.alpha, w.tide, it, "the list keeps its scroll and its selection", Stance::Agree).await?;
    assert_eq!(only(&w.alpha, w.tide).await?.standing(), Standing::Unconfirmed, "reworded since it was confirmed");
    readings::confirm(&w.alpha, w.ines, agents, revision().await?, None, None).await?;
    assert_eq!(only(&w.alpha, w.tide).await?.standing(), Standing::Confirmed, "a second wordless confirmation");
    // Ines rewords what she wants; the agent's account, read of the new wording, is a new revision.
    intentions::reword(&w.alpha, w.ines, it, None, Some("the list keeps its place"), None).await?;
    read(&w.alpha, w.tide, it, "the list keeps its scroll and its selection", Stance::Agree).await?;
    assert_eq!(only(&w.alpha, w.tide).await?.standing(), Standing::Unconfirmed, "she confirmed the old wording");
    readings::confirm(&w.alpha, w.ines, agents, revision().await?, None, None).await?;

    let corrected = SetReading {
        of: agents,
        words: "no, only the scroll".into(),
        stance: Stance::Disagree,
        said_by: Some(w.ines),
        as_read: Some(revision().await?),
    };
    readings::set(&w.alpha, w.tide, corrected).await?;
    let held = only(&w.alpha, w.tide).await?;
    assert_eq!(held.standing(), Standing::ToDiscuss, "her later disagreement passes over her agreement");
    assert_eq!(held.speakers_reading.map(|r| r.words), Some("no, only the scroll".into()));
    Ok(())
}

#[tokio::test]
async fn a_confirmation_of_a_wording_no_longer_standing_does_not_confirm() -> Result<()> {
    let w = world().await?;
    let it = intention(&w.alpha, w.tide, "Readers keep their place", Some(w.ines)).await?;
    ops::link(&w.alpha, w.tide, RelationKind::WorksUnder, w.tide, it, None, false, json!({})).await?;
    let agents = read(&w.alpha, w.tide, it, "the list keeps its scroll", Stance::Agree).await?;
    let checked = reading(&w.alpha, it, agents).await?.revision;
    read(&w.alpha, w.tide, it, "the list keeps its scroll and its selection", Stance::Agree).await?;
    // Ines's agreement, checked against the wording before the rewording and written after it.
    written_below(&w.alpha, w.ines, agents, Stance::Agree, &checked).await?;
    assert_eq!(only(&w.alpha, w.tide).await?.standing(), Standing::Unconfirmed);
    Ok(())
}

#[tokio::test]
async fn a_reading_that_does_not_say_which_wording_it_read_is_refused_as_broken() -> Result<()> {
    let w = world().await?;
    let it = intention(&w.alpha, w.ines, "Readers keep their place", None).await?;
    let trx = w.alpha.begin();
    let made =
        trx.create_placed(&Reading { of: it, words: "as I read it".into(), stance: Some(Stance::Agree) }).await?;
    let record = ankurah::model::Mutable::entity(&*made);
    trx.add(record, &Creation { created_by: Ref::new(w.tide), created: 1 }).await?;
    trx.commit().await?;
    assert!(refusal(readings::readings_of(&w.alpha, it).await).contains("the wording it read"));
    Ok(())
}

#[tokio::test]
async fn two_readings_by_one_party_are_read_as_its_one_standing_reading_by_every_reader() -> Result<()> {
    let w = world().await?;
    let it = intention(&w.alpha, w.tide, "Readers keep their place", Some(w.ines)).await?;
    ops::link(&w.alpha, w.tide, RelationKind::WorksUnder, w.tide, it, None, false, json!({})).await?;
    let wording = intentions::intention(&w.alpha, it).await?.revision;
    let first = read(&w.alpha, w.tide, it, "the list keeps its scroll", Stance::Agree).await?;
    let second = written_below(&w.alpha, w.tide, it, Stance::Question, &wording).await?;
    let standing = first.min(second);
    let other = first.max(second);

    assert_eq!(only(&w.alpha, w.tide).await?.reading.map(|r| r.id), Some(standing), "works_under");
    assert_eq!(read(&w.alpha, w.tide, it, "the list and its selection", Stance::Agree).await?, standing, "set");
    let revision = reading(&w.alpha, it, other).await?.revision;
    let refused = refusal(readings::confirm(&w.alpha, w.ines, other, revision, None, None).await);
    assert!(refused.contains(&standing.to_base64()), "confirm names the standing one: {refused}");
    Ok(())
}

#[tokio::test]
async fn what_a_principal_works_under_reads_ended_first_then_what_to_discuss_reworded_unread_unconfirmed_and_confirmed(
) -> Result<()> {
    let w = world().await?;
    let mut ids = Vec::new();
    let titles = ["ended", "questioned", "disputed", "reworded", "unread", "unconfirmed", "confirmed", "elsewhere"];
    for title in titles.into_iter().chain(["retracted"]) {
        let (graph, by, said_by) =
            if title == "elsewhere" { (&w.beta, w.reef, None) } else { (&w.alpha, w.tide, Some(w.ines)) };
        let id = intention(graph, by, title, said_by).await?;
        ops::link(&w.alpha, w.tide, RelationKind::WorksUnder, w.tide, id, None, false, json!({})).await?;
        ids.push(id);
    }
    let [ended, questioned, disputed, reworded, unread, unconfirmed, confirmed, elsewhere, retracted] = ids[..] else {
        unreachable!()
    };
    ops::unlink(&w.alpha, w.tide, RelationKind::WorksUnder, w.tide, retracted, "no longer").await?;
    ops::link(&w.alpha, w.omar, RelationKind::WorksUnder, w.omar, unread, None, false, json!({})).await?;
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

    for (id, stance) in [
        (ended, Stance::Agree),
        (questioned, Stance::Question),
        (disputed, Stance::Agree),
        (reworded, Stance::Agree),
        (unconfirmed, Stance::Agree),
        (confirmed, Stance::Agree),
        (elsewhere, Stance::Agree),
    ] {
        read(&w.alpha, w.tide, id, "as I read it", stance).await?;
    }
    for (id, stance) in [(confirmed, Stance::Agree), (disputed, Stance::Disagree)] {
        let agents = readings::readings_of(&w.alpha, id).await?[0].clone();
        let words = "her answer".to_string();
        let answer = SetReading { of: agents.id, words, stance, said_by: None, as_read: Some(agents.revision) };
        readings::set(&w.alpha, w.ines, answer).await?;
    }
    intentions::close(&w.alpha, w.ines, ended, IntentionStatus::Withdrawn, Some("dropped".into()), None).await?;
    intentions::reword(&w.alpha, w.ines, reworded, None, Some("said again"), None).await?;

    let held = works_under(&w.alpha, w.tide).await?;
    let mut expected = vec![
        (ended, Standing::Ended),
        (questioned, Standing::ToDiscuss),
        (disputed, Standing::ToDiscuss),
        (reworded, Standing::Reworded),
        (unread, Standing::Unread),
        (unconfirmed, Standing::Unconfirmed),
        (elsewhere, Standing::Unconfirmed),
        (confirmed, Standing::Confirmed),
    ];
    expected.sort_by_key(|(id, standing)| (*standing, *id));
    let listed: Vec<(EntityId, Standing)> = held.intentions.iter().map(|i| (i.intention.id, i.standing())).collect();
    assert_eq!(listed, expected);
    let of = |id| held.intentions.iter().find(|i| i.intention.id == id).expect("listed");
    assert!(matches!(&of(elsewhere).project, Whose::Other(p) if p.name == "beta"), "named with its project");
    assert_eq!(held.not_held, vec![gone]);

    // Read again, its account unchanged, the reworded intention is no longer reworded, and its
    // reading records no edit.
    read(&w.alpha, w.tide, reworded, "as I read it", Stance::Agree).await?;
    let again = works_under(&w.alpha, w.tide).await?;
    let now = again.intentions.iter().find(|i| i.intention.id == reworded).expect("listed");
    assert_eq!(now.standing(), Standing::Unconfirmed);
    assert_eq!(now.reading.as_ref().and_then(|r| r.authorship.edited), None);
    Ok(())
}

#[tokio::test]
async fn works_under_joins_a_principal_to_an_intention_and_serves_a_task_or_an_intention_to_one() -> Result<()> {
    let w = world().await?;
    let broad = intention(&w.alpha, w.ines, "Readers keep their place", None).await?;
    let narrow = intention(&w.alpha, w.ines, "The list keeps its scroll", None).await?;
    let task = ops::create_task(&w.alpha, w.ines, CreateTask { title: "Save the scroll".into(), ..Default::default() })
        .await?;
    let link = |kind, from, to, propose| ops::link(&w.alpha, w.tide, kind, from, to, None, propose, json!({}));

    let proposed = link(RelationKind::WorksUnder, w.tide, broad, true).await?;
    assert_eq!(link(RelationKind::WorksUnder, w.tide, broad, true).await?, proposed, "proposed once");
    assert_eq!(link(RelationKind::WorksUnder, w.tide, broad, false).await?, proposed, "and accepted");
    assert_eq!(w.alpha.relation(proposed).await?.status()?, RelationStatus::Accepted);
    assert!(refusal(link(RelationKind::WorksUnder, task, broad, false).await).contains("not a task to an intention"));
    assert!(refusal(link(RelationKind::WorksUnder, w.tide, task, false).await).contains("not a principal to a task"));
    link(RelationKind::Serves, task, narrow, false).await?;
    link(RelationKind::Serves, narrow, broad, false).await?;
    assert!(refusal(link(RelationKind::Serves, broad, narrow, false).await).contains("cycle"));
    assert!(refusal(link(RelationKind::Serves, broad, broad, false).await).contains("cycle"));
    assert!(refusal(link(RelationKind::Serves, task, task, false).await).contains("not a task to a task"));

    assert_eq!(ops::describe(&w.alpha, broad).await?, Some((Kind::Intention, "Readers keep their place".into())));
    let start = &broad.to_base64()[..8];
    assert_eq!(ops::resolve_id(&w.alpha, start, &[Kind::Intention]).await?, broad, "this project's");
    assert_eq!(ops::resolve_id(&w.beta, start, &[Kind::Intention]).await?, broad, "from another project");
    let reading = read(&w.alpha, w.tide, broad, "as I read it", Stance::Agree).await?;
    assert_eq!(ops::resolve_id(&w.alpha, &reading.to_base64()[..8], &[Kind::Reading]).await?, reading);
    Ok(())
}
