//! A project's record in the daemon's store (`fwd_core::project_graph`): the daemon makes one per
//! project as it first hosts it, and finds that one again at every later start, with the name the
//! index gives; a client finds it by the project's UUID, and a UUID the store holds no record of is
//! refused. And the reads a project's graph makes beyond its own records: an id a record holds,
//! followed into whichever project its record is in; the relations that touch a record, whichever
//! project made them, which only that project changes; and the ids of every project, and the former
//! ids that start with what someone types, which an id someone types may name.

use ankurah::{policy::DEFAULT_CONTEXT, Context, Node, PermissiveAgent};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::model::ProjectView;
use fwd_core::project_graph::{ProjectGraph, ProjectGraphError};
use std::sync::Arc;

/// A context on a store in memory.
async fn store() -> Result<Context> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    Ok(node.context_async(DEFAULT_CONTEXT).await?)
}

/// Every project record the store holds, by name.
async fn named(ctx: &Context) -> Result<Vec<String>> {
    let mut names =
        ctx.fetch::<ProjectView>("uuid != ''").await?.iter().map(|p| p.name()).collect::<Result<Vec<_>, _>>()?;
    names.sort();
    Ok(names)
}

#[tokio::test]
async fn the_daemon_makes_one_record_per_project_and_finds_it_again_under_the_indexs_name() -> Result<()> {
    let ctx = store().await?;
    let uuid = uuid::Uuid::new_v4();
    let first = ProjectGraph::ensure(ctx.clone(), uuid, "fwdcentaur").await?;
    let again = ProjectGraph::ensure(ctx.clone(), uuid, "fwd centaur").await?;
    assert_eq!(first.project(), again.project(), "a later start finds the record the first made");
    assert_eq!(named(&ctx).await?, vec!["fwd centaur"], "one record, under the name the index has now");
    ProjectGraph::ensure(ctx.clone(), uuid::Uuid::new_v4(), "another").await?;
    assert_eq!(named(&ctx).await?, vec!["another", "fwd centaur"]);

    again.rename("fwd").await?;
    assert_eq!(named(&ctx).await?, vec!["another", "fwd"], "a rename while the daemon runs");
    Ok(())
}

#[tokio::test]
async fn a_client_finds_the_project_by_its_uuid_and_no_project_the_store_has_no_record_of() -> Result<()> {
    let ctx = store().await?;
    let uuid = uuid::Uuid::new_v4();
    let hosted = ProjectGraph::ensure(ctx.clone(), uuid, "fwdcentaur").await?;
    let opened = ProjectGraph::open(ctx.clone(), uuid).await?;
    assert_eq!(opened.project(), hosted.project());
    let unknown = uuid::Uuid::new_v4();
    let refused = ProjectGraph::open(ctx, unknown).await.map(|g| g.project()).expect_err("no record");
    assert!(matches!(refused, ProjectGraphError::NotHosted(id) if id == unknown), "{refused:?}");
    Ok(())
}

/// Two projects' graphs over one store in memory.
async fn two() -> Result<(ProjectGraph, ProjectGraph)> {
    let ctx = store().await?;
    let alpha = ProjectGraph::ensure(ctx.clone(), uuid::Uuid::new_v4(), "alpha").await?;
    let beta = ProjectGraph::ensure(ctx, uuid::Uuid::new_v4(), "beta").await?;
    Ok((alpha, beta))
}

/// A task of `graph`'s project, made by `by`, titled `title`.
async fn task(graph: &ProjectGraph, by: ankurah::EntityId, title: &str) -> Result<ankurah::EntityId> {
    fwd_core::ops::create_task(graph, by, fwd_core::ops::CreateTask { title: title.into(), ..Default::default() }).await
}

#[tokio::test]
async fn following_an_id_reads_its_record_in_whichever_project_with_that_projects_name() -> Result<()> {
    use fwd_core::model::{FormerId, PrincipalKind, TaskView};
    use fwd_core::project_graph::{Followed, Whose};
    let (alpha, beta) = two().await?;
    let person = fwd_core::ops::ensure_principal(&beta, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let theirs = task(&beta, person, "Retry on disconnect").await?;
    let ours = task(&alpha, person, "Cap the retry delay").await?;

    match alpha.follow::<TaskView>(theirs).await? {
        Followed::Held { record, project: Whose::Other(p), found } => {
            assert_eq!((record, p.name.as_str(), found.title()?), (theirs, "beta", "Retry on disconnect".into()));
        }
        other => panic!("beta's task, followed from alpha: {other:?}"),
    }
    assert!(matches!(alpha.follow::<TaskView>(ours).await?, Followed::Held { project: Whose::This, .. }));
    let nothing = ankurah::EntityId::random();
    assert!(
        matches!(alpha.follow::<TaskView>(nothing).await?, Followed::NotHeld),
        "an id the node holds no record for"
    );
    assert!(matches!(alpha.follow::<TaskView>(person).await?, Followed::NotHeld), "a record of another model");

    // An id beta's task had before it was renumbered, which beta keeps, names the task it is now.
    let former = ankurah::EntityId::random();
    let trx = beta.begin();
    trx.create(&FormerId { project: beta.project(), former: former.to_base64(), record: theirs }).await?;
    trx.commit().await?;
    assert!(matches!(alpha.follow::<TaskView>(former).await?, Followed::Held { record, .. } if record == theirs));
    assert!(matches!(alpha.follow_any(former).await?, Followed::Held { record, .. } if record == theirs));
    Ok(())
}

#[tokio::test]
async fn a_records_relations_are_read_by_its_id_whichever_project_made_them_and_a_list_by_project() -> Result<()> {
    use fwd_core::model::{PrincipalKind, RelationKind, RelationView};
    use fwd_core::ops;
    use fwd_core::project_graph::ProjectGraphError;
    let (alpha, beta) = two().await?;
    let person = ops::ensure_principal(&alpha, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let ours = task(&alpha, person, "Cap the retry delay").await?;
    let theirs = task(&beta, person, "Retry on disconnect").await?;
    let made_in_beta =
        ops::link(&beta, person, RelationKind::DependsOn, theirs, ours, None, false, serde_json::json!({})).await?;

    let to_ours = alpha.relations_to(ours, Some(RelationKind::DependsOn)).await?;
    assert_eq!(to_ours.iter().map(ankurah::View::id).collect::<Vec<_>>(), vec![made_in_beta]);
    assert_eq!(alpha.relations_from(theirs, None).await?.len(), 1, "read on beta's task through alpha's graph too");
    assert_eq!(alpha.relation(made_in_beta).await?.from_id()?, theirs);
    let listed: Vec<RelationView> = alpha.all().await?;
    assert!(listed.is_empty(), "alpha's list of relations holds those alpha made: {}", listed.len());
    assert_eq!(beta.all::<RelationView>().await?.len(), 1);

    // It is beta's record: beta's transactions change it, and alpha's refuse it.
    assert!(!alpha.owns(&to_ours[0]) && beta.owns(&to_ours[0]));
    let trx = alpha.begin();
    let refused = trx.edit(&to_ours[0]).map(|_| ()).expect_err("beta's relation");
    assert!(matches!(refused, ProjectGraphError::OfAnotherProject { record, .. } if record == made_in_beta));
    Ok(())
}

#[tokio::test]
async fn the_ids_a_typed_id_may_name_are_every_projects_with_their_project() -> Result<()> {
    use fwd_core::model::{PrincipalKind, TaskView};
    use fwd_core::project_graph::Whose;
    let (alpha, beta) = two().await?;
    let person = fwd_core::ops::ensure_principal(&alpha, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let ours = task(&alpha, person, "Cap").await?;
    let theirs = task(&beta, person, "Retry").await?;
    let every = alpha.ids_in_every_project::<TaskView>().await?;
    let (alpha_record, beta_record) = (*alpha.project(), *beta.project());
    assert!(every.contains(&(ours, alpha_record)) && every.contains(&(theirs, beta_record)), "{every:?}");
    let named = alpha.projects_named(&[alpha_record, beta_record]).await?;
    assert_eq!(named[0], (alpha_record, Whose::This));
    assert!(matches!(&named[1], (record, Whose::Other(p)) if *record == beta_record && p.name == "beta"));
    Ok(())
}

/// The former ids a typed prefix may name are read as a range of their text, from the prefix to
/// the text whose last character comes after the prefix's, which holds exactly the former ids that
/// start with the prefix, in either project, whichever character of an id's alphabet the prefix ends
/// with: the store orders text by its bytes, where `-` comes before the digits, the digits before
/// the capitals, `_` after them and the small letters last.
#[tokio::test]
async fn the_former_ids_a_prefix_may_name_are_those_of_every_project_that_start_with_it() -> Result<()> {
    use fwd_core::model::FormerId;
    let (alpha, beta) = two().await?;
    let formers = [
        "Retry-a", "Retry_b", "Retryz", "RetryZ", "Retry9", "Retrz", "Retra", "RetrY", "Retr-", "Retr_", "Retr9x",
        "Retr0",
    ];
    for (i, former) in formers.iter().enumerate() {
        let graph = if i % 2 == 0 { &alpha } else { &beta };
        let trx = graph.begin();
        let kept =
            FormerId { project: graph.project(), former: former.to_string(), record: ankurah::EntityId::random() };
        trx.create(&kept).await?;
        trx.commit().await?;
    }
    for (prefix, expected) in [
        ("Retry", vec!["Retry-a", "Retry9", "RetryZ", "Retry_b", "Retryz"]),
        ("Retrz", vec!["Retrz"]),
        ("Retr_", vec!["Retr_"]),
        ("Retr-", vec!["Retr-"]),
        ("Retr9", vec!["Retr9x"]),
        ("RetrY", vec!["RetrY"]),
        ("Retq", vec![]),
    ] {
        let mut found =
            alpha.former_ids_starting(prefix).await?.iter().map(|f| f.former()).collect::<Result<Vec<_>, _>>()?;
        found.sort();
        assert_eq!(found, expected, "the former ids that start with `{prefix}`");
    }
    assert!(alpha.former_ids_starting("Retré").await?.is_empty(), "no id's text holds a character outside ASCII");
    Ok(())
}
