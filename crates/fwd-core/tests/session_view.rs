//! Reading the person's view of a session from a node (`session_view::read`), over two projects of
//! one store in memory: whose groups it holds and in what order whichever project it is read
//! through, the person it is for resolved once and strictly in each project, the questions they
//! answered, and the records a text cites named, never shown by id. How a read becomes a document
//! is tested beside it (src/session_view/tests.rs).

use ankurah::property::{Json, Ref};
use ankurah::{policy::DEFAULT_CONTEXT, EntityId, Node, PermissiveAgent};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::model::*;
use fwd_core::ops::{self, CreateTask, MessageContext};
use fwd_core::project_graph::ProjectGraph;
use fwd_core::session_view::{self, Document, Reader};
use serde_json::{json, Value};
use std::sync::Arc;

const PAGE: &str = "http://127.0.0.1:4242/";

/// alpha and beta over one store in memory, each with its person `tester`; the session `s1`'s own
/// thread is `lead-sam` in both, and its subagent `a1` is `impl-sam` in alpha.
struct Two {
    alpha: ProjectGraph,
    beta: ProjectGraph,
    tester: EntityId,
    their_tester: EntityId,
    lead: EntityId,
}

async fn two() -> Result<Two> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    let ctx = node.context_async(DEFAULT_CONTEXT).await?;
    let alpha = ProjectGraph::ensure(ctx.clone(), uuid::Uuid::new_v4(), "alpha").await?;
    let beta = ProjectGraph::ensure(ctx, uuid::Uuid::new_v4(), "beta").await?;
    let tester = ops::ensure_principal(&alpha, "tester", "tester", PrincipalKind::Human, None).await?;
    let their_tester = ops::ensure_principal(&beta, "tester", "tester", PrincipalKind::Human, None).await?;
    let agent = |graph: &ProjectGraph, name: &'static str, subject: &'static str| {
        let graph = graph.clone();
        async move { ops::add_principal(&graph, name, Some(AgentRole::Lead), None, None, None, Some(subject)).await }
    };
    // In beta first, so that a read that kept the order things were made in would put beta first.
    agent(&beta, "lead-sam", "claude-code:s1/main").await?;
    let lead = agent(&alpha, "lead-sam", "claude-code:s1/main").await?;
    ops::add_principal(&alpha, "impl-sam", Some(AgentRole::Implementer), None, None, None, Some("claude-code:s1/a1"))
        .await?;
    Ok(Two { alpha, beta, tester, their_tester, lead })
}

/// The view of `s1` for `reader`, read through `graph`, as JSON.
async fn view(graph: &ProjectGraph, reader: Reader<'_>) -> Result<Value> {
    let document: Document = session_view::read(graph, "s1", reader).await?.document(PAGE);
    Ok(serde_json::to_value(document)?)
}

fn asking(to: EntityId) -> MessageContext {
    MessageContext { to: Some(to.to_base64()), ..Default::default() }
}

/// The rows of the list `section` of the group `g`.
fn rows<'a>(json: &'a Value, g: usize, section: &str) -> Vec<&'a Value> {
    json["principals"][g]["sections"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["section"] == section)
        .flat_map(|s| s["records"].as_array().into_iter().flatten())
        .collect()
}

/// The session's own thread's groups come first, by project name, then its subagent's, whichever
/// project the view is read through.
#[tokio::test]
async fn the_groups_stand_in_one_order_whichever_project_the_view_is_read_through() -> Result<()> {
    let t = two().await?;
    for graph in [&t.beta, &t.alpha] {
        let json = view(graph, Reader::Identity("tester")).await?;
        let groups: Vec<String> = json["principals"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|g| format!("{} {}", g["title"].as_str().unwrap_or(""), g["tags"][1]["text"].as_str().unwrap_or("")))
            .collect();
        assert_eq!(groups, ["lead-sam alpha", "lead-sam beta", "impl-sam alpha"], "{json}");
    }
    Ok(())
}

/// The person is the one person of their name in each project: what is asked of them there waits
/// on them, and its Answer acts as that principal, by its id. Where two people share the name, what
/// is asked of either is not counted as theirs and the document says so; a name only an agent
/// has, or an id that names an agent, is said on the document and offers nothing to answer; an id,
/// or its start, that names a person reads as their name does.
#[tokio::test]
async fn the_person_is_resolved_once_and_strictly_in_each_project() -> Result<()> {
    let t = two().await?;
    let in_alpha =
        ops::post_message(&t.alpha, t.lead, "Should the primer name the page?", Priority::Now, asking(t.tester))
            .await?;
    // A second person named tester in beta, which the commands would never make, but a store can hold.
    let trx = t.beta.begin();
    let twin = trx
        .create(&Principal {
            project: t.beta.project(),
            kind: PrincipalKind::Human,
            handle: "tester".into(),
            display_name: "tester".into(),
            auth_subject: None,
            agent_role: None,
            active: true,
            member: Some(true),
            subject: None,
            succeeded_by: None,
            serves: None,
            last_active: None,
        })
        .await?
        .id();
    trx.commit().await?;
    let beta_lead =
        ops::principals(&t.beta).await?.into_iter().find(|p| p.handle().ok().as_deref() == Some("lead-sam"));
    let beta_lead = beta_lead.map(|p| p.id()).expect("lead-sam in beta");
    ops::post_message(&t.beta, beta_lead, "Which of you is it?", Priority::Now, asking(t.their_tester)).await?;

    let json = view(&t.alpha, Reader::Identity("tester")).await?;
    let waiting = rows(&json, 0, "waits_on_you");
    assert_eq!(waiting.len(), 1, "{json}");
    assert_eq!(waiting[0]["id"], in_alpha.to_base64());
    assert_eq!(waiting[0]["actions"][0]["args"][3], t.tester.to_base64());
    let others = rows(&json, 1, "waits_on_others");
    assert_eq!(others.len(), 1, "a question to one of two people named tester is no one's here: {json}");
    assert_eq!(
        json["lines"],
        json!(["More than one person is named tester in beta: what they are asked there is not counted as waiting on you, and none of it can be answered here."])
    );
    assert!(twin != t.their_tester);

    let by_id = view(&t.alpha, Reader::Named(&t.tester.to_base64()[..8])).await?;
    assert_eq!(by_id["principals"], json["principals"], "an id's start names the same person");

    let agent = view(&t.alpha, Reader::Named("impl-sam")).await?;
    assert_eq!(
        agent["lines"],
        json!(["impl-sam names an agent, and the view is a person's, so nothing here waits on you and nothing can be answered."])
    );
    assert!(rows(&agent, 0, "waits_on_you").is_empty());
    let agent_id = view(&t.alpha, Reader::Named(&t.lead.to_base64())).await?;
    assert_eq!(
        agent_id["lines"],
        json!(["`--as` names an agent, and the view is a person's, so nothing here waits on you and nothing can be answered."])
    );
    let nobody = view(&t.alpha, Reader::Identity("nobody")).await?;
    assert_eq!(
        nobody["lines"],
        json!(["no person is named nobody in any project fwd holds, so nothing here waits on you and nothing can be answered."])
    );
    Ok(())
}

/// A question the person answered in its thread waits on them no more; one another principal
/// answered still does.
#[tokio::test]
async fn a_question_the_person_answered_waits_on_them_no_more() -> Result<()> {
    let t = two().await?;
    let mine = ops::post_message(&t.alpha, t.lead, "Is the retry capped?", Priority::Now, asking(t.tester)).await?;
    let theirs =
        ops::post_message(&t.alpha, t.lead, "Is the backoff doubled?", Priority::Now, asking(t.tester)).await?;
    let reply = |to| MessageContext { reply_to: Some(to), ..Default::default() };
    ops::post_message(&t.alpha, t.tester, "Yes, at two seconds.", Priority::Next, reply(mine)).await?;
    ops::post_message(&t.alpha, t.lead, "Asking again.", Priority::Next, reply(theirs)).await?;
    let json = view(&t.alpha, Reader::Identity("tester")).await?;
    let waiting: Vec<&Value> = rows(&json, 0, "waits_on_you").into_iter().map(|r| &r["id"]).collect();
    assert_eq!(waiting, [&json!(theirs.to_base64())], "{json}");
    Ok(())
}

/// A question that cites a task names it, in its title by its title and in its text as a link to
/// its page, and a reference that names nothing fwd holds is named without an id: no id shows in
/// any text of the document.
#[tokio::test]
async fn a_cited_record_is_named_and_linked_and_no_id_shows() -> Result<()> {
    let t = two().await?;
    let task = ops::create_task(
        &t.beta,
        t.their_tester,
        CreateTask { title: "Retry on disconnect".into(), ..Default::default() },
    )
    .await?;
    let text = format!(
        "Does [[{}]] cover the page too, or only the command line? I would leave the page out.",
        task.to_base64()
    );
    ops::post_message(&t.alpha, t.lead, &text, Priority::Now, asking(t.tester)).await?;
    // A reference stored before what it names was taken away, as a text written in another store
    // can hold.
    let gone = EntityId::from_bytes([7; 32]);
    let trx = t.alpha.begin();
    trx.create(&Message {
        project: t.alpha.project(),
        author: Ref::new(t.lead),
        content: format!("Was [[{}]] ever finished, and by whom?", gone.to_base64()),
        priority: Priority::Now,
        status: MessageStatus::Active,
        created: 1,
        context: Json::new(json!({"to": t.tester.to_base64()})),
        task: None,
        anchor: None,
        scope: None,
        decision: None,
        reply_to: None,
        thread: None,
        resolved: None,
        resolved_by: None,
        edited: None,
        edited_by: None,
    })
    .await?;
    trx.commit().await?;

    let json = view(&t.alpha, Reader::Identity("tester")).await?;
    let waiting = rows(&json, 0, "waits_on_you");
    let cites = waiting
        .iter()
        .find(|r| r["title"].as_str().is_some_and(|t| t.starts_with("Does")))
        .expect("the citing question");
    assert_eq!(cites["title"], "Does Retry on disconnect cover the page too,…");
    let beta = t.beta.projects().await?.into_iter().find(|p| p.name == "beta").expect("beta").uuid;
    assert_eq!(
        cites["lines"],
        json!([format!("Does [Retry on disconnect]({PAGE}?p={beta}#/tasks/{}) cover the page too, or only the command line? I would leave the page out.", task.to_base64())])
    );
    let missing =
        waiting.iter().find(|r| r["title"].as_str().is_some_and(|t| t.starts_with("Was"))).expect("the other");
    assert_eq!(missing["title"], "Was something fwd cannot find ever finished, and…");
    assert_eq!(missing["lines"], json!(["Was something fwd cannot find ever finished, and by whom?"]));
    for row in &waiting {
        let shown = format!("{} {}", row["title"], row["lines"]);
        let link = format!("#/tasks/{}", task.to_base64());
        let without_links = shown.replace(&link, "");
        assert!(
            !without_links.contains(&task.to_base64()[..8]) && !without_links.contains(&gone.to_base64()[..8]),
            "{shown}"
        );
    }
    Ok(())
}
