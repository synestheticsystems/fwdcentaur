//! Reading the person's view of a session from a node (`session_view::read`), over two projects of
//! one store in memory: whose groups it holds and in what order whichever project it is read
//! through, the person it is for resolved once and strictly in each project, the questions they
//! answered, the records a text cites named, never shown by id, and the session's active
//! intentions. How a read becomes a document is tested beside it (src/session_view/tests.rs).

use ankurah::property::{Json, Ref};
use ankurah::{policy::DEFAULT_CONTEXT, Context, EntityId, Node, PermissiveAgent};
use ankurah_commons::records::ClosedView;
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::decision_log::date;
use fwd_core::intentions;
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
    /// The node's context, through which a test writes what fwd's own writes never make.
    node: Context,
    alpha: ProjectGraph,
    beta: ProjectGraph,
    tester: EntityId,
    their_tester: EntityId,
    lead: EntityId,
    their_lead: EntityId,
    subagent: EntityId,
}

async fn two() -> Result<Two> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    let ctx = node.context_async(DEFAULT_CONTEXT).await?;
    let alpha = ProjectGraph::ensure(ctx.clone(), uuid::Uuid::new_v4(), "alpha").await?;
    let beta = ProjectGraph::ensure(ctx.clone(), uuid::Uuid::new_v4(), "beta").await?;
    let tester = ops::ensure_principal(&alpha, "tester", "tester", PrincipalKind::Human, None).await?;
    let their_tester = ops::ensure_principal(&beta, "tester", "tester", PrincipalKind::Human, None).await?;
    let agent = |graph: &ProjectGraph, name: &'static str, subject: &'static str| {
        let graph = graph.clone();
        async move { ops::add_principal(&graph, name, Some(AgentRole::Lead), None, None, None, Some(subject)).await }
    };
    // In beta first, so that a read that kept the order things were made in would put beta first.
    let their_lead = agent(&beta, "lead-sam", "claude-code:s1/main").await?;
    let lead = agent(&alpha, "lead-sam", "claude-code:s1/main").await?;
    let subagent = ops::add_principal(
        &alpha,
        "impl-sam",
        Some(AgentRole::Implementer),
        None,
        None,
        None,
        Some("claude-code:s1/a1"),
    )
    .await?;
    Ok(Two { node: ctx, alpha, beta, tester, their_tester, lead, their_lead, subagent })
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
/// its page; one that cites an intention names it by its title, with no link, since the page has
/// no intention; and a reference that names nothing fwd holds is named without an id: no id shows
/// in any text of the document.
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
    let intention = intentions::create(&t.alpha, t.tester, "Readers keep their place", String::new(), None).await?;
    let intention = intention.0.id();
    let about = format!("Is [[{}]] still what you want from the page, or has it changed?", intention.to_base64());
    ops::post_message(&t.alpha, t.lead, &about, Priority::Now, asking(t.tester)).await?;
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
    let wanted =
        waiting.iter().find(|r| r["title"].as_str().is_some_and(|t| t.starts_with("Is"))).expect("the intention's");
    assert_eq!(
        wanted["lines"],
        json!(["Is Readers keep their place still what you want from the page, or has it changed?"])
    );
    for row in &waiting {
        let shown = format!("{} {}", row["title"], row["lines"]);
        let link = format!("#/tasks/{}", task.to_base64());
        let without_links = shown.replace(&link, "");
        let ids = [task, gone, intention].map(|id| id.to_base64()[..8].to_string());
        assert!(ids.iter().all(|id| !without_links.contains(id)), "{shown}");
    }
    Ok(())
}

/// The session's active intentions are those assigned to the principals of its own thread, of any
/// project, each once, however many of them hold it, one its principal of beta alone holds among
/// them: each titled by its title, its description alone its line, one with no description with no
/// line, and attributed apart from it to the speaker its quotation names, by name, on the day the
/// words were written down; another project's name a tag, and one since fulfilled saying so in a
/// line after its description and a tag; no path and no action. What is assigned to its subagent is
/// not among them.
#[tokio::test]
async fn the_active_intentions_are_those_assigned_to_the_sessions_own_thread() -> Result<()> {
    let t = two().await?;
    // An attribution names her in plain text: her name's whitespace made single spaces, and
    // nothing escaped as a line's Markdown would escape it.
    let ines = ops::ensure_principal(&t.alpha, "ines", "Ines\tMoreau (design)", PrincipalKind::Human, None).await?;
    let words = "The list stays where I left it.".to_string();
    let ours = intentions::create(&t.alpha, t.lead, "Readers keep their place", words, Some(ines)).await?;
    let theirs = intentions::create(&t.beta, t.their_tester, "Builds stay under a minute", String::new(), None).await?;
    let done = intentions::create(&t.alpha, ines, "Name the page", "A short name.".into(), None).await?;
    let subagents = intentions::create(&t.alpha, ines, "Draw the panel", String::new(), None).await?;
    let betas = intentions::create(&t.beta, t.their_tester, "Logs name their run", String::new(), None).await?;
    for it in [&ours, &theirs, &done] {
        intentions::assign(&t.alpha, t.lead, it, t.lead).await?;
    }
    intentions::assign(&t.beta, t.their_lead, &ours, t.their_lead).await?;
    intentions::assign(&t.beta, t.their_lead, &betas, t.their_lead).await?;
    intentions::assign(&t.alpha, t.subagent, &subagents, t.subagent).await?;
    intentions::close(&t.alpha, ines, &done, intentions::Outcome::Fulfilled, None, None).await?;

    let json = view(&t.alpha, Reader::Identity("tester")).await?;
    let section = &json["sections"][0];
    assert_eq!((&section["section"], &section["of"]), (&json!("active_intentions"), &json!(4)), "{json}");
    let row = |record: &(PlacementView, intentions::Parts)| -> Value {
        let id = json!(record.0.id().to_base64());
        section["records"].as_array().into_iter().flatten().find(|r| r["id"] == id).cloned().unwrap_or(Value::Null)
    };
    let day = date(ours.1 .1.created()?.unix_millis());
    assert_eq!(
        row(&ours),
        json!({
            "id": ours.0.id().to_base64(), "kind": "intention", "title": "Readers keep their place",
            "title_is_cut": false, "tags": [], "lines": ["The list stays where I left it."],
            "attribution": {"by": "Ines Moreau (design)", "at": day}, "path": null, "actions": [],
        })
    );
    assert_eq!(
        (&row(&theirs)["tags"], &row(&theirs)["lines"]),
        (&json!([{"text": "beta", "tone": null, "at": null}]), &json!([]))
    );
    assert_eq!((&row(&betas)["title"], &row(&betas)["tags"]), (&json!("Logs name their run"), &json!([])));
    let closure = t.alpha.component_of::<ClosedView>(&done.0).await?.expect("closed");
    let closed = date(closure.closed()?.expect("when it closed").unix_millis());
    assert_eq!(row(&done)["lines"][1], format!("fulfilled on {closed}"));
    assert_eq!(row(&done)["tags"], json!([{"text": "fulfilled", "tone": null, "at": null}]));
    assert_eq!(row(&subagents), Value::Null, "the subagent's is not the session's");
    Ok(())
}

/// A relation as a writer that left a field out stores one: by a model of the name fwd's relation
/// is read by, with fewer properties.
mod sparse {
    use ankurah::property::{Json, Ref};
    use ankurah::{EntityId, Model};
    use fwd_core::model::{Principal, Project, RelationKind, RelationStatus};
    use serde::{Deserialize, Serialize};

    /// A relation with no creation time.
    #[derive(Model, Debug, Clone, Serialize, Deserialize)]
    #[model(no_ffi)]
    pub struct Relation {
        pub project: Ref<Project>,
        #[active_type(LWW)]
        pub kind: RelationKind,
        pub from_id: EntityId,
        pub to_id: EntityId,
        #[active_type(LWW)]
        pub status: RelationStatus,
        pub created_by: Ref<Principal>,
        pub via: Json,
    }
}

/// An assignment that cannot be followed fails nothing: one of an id that names nothing, and one
/// whose relation has no creation time, as a writer that left the field out stores one, are
/// counted in the section's line, beside the intention that can be read.
#[tokio::test]
async fn an_assignment_that_cannot_be_followed_is_counted_beside_the_rest() -> Result<()> {
    let t = two().await?;
    let ours = intentions::create(&t.alpha, t.lead, "Readers keep their place", String::new(), None).await?;
    intentions::assign(&t.alpha, t.lead, &ours, t.lead).await?;
    let trx = t.alpha.begin();
    trx.create(&Relation {
        project: t.alpha.project(),
        kind: RelationKind::AssignedTo,
        from_id: EntityId::from_bytes([0; 32]),
        to_id: t.lead,
        status: RelationStatus::Accepted,
        created_by: Ref::new(t.lead),
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
    let trx = t.node.begin();
    trx.create(&sparse::Relation {
        project: t.alpha.project(),
        kind: RelationKind::AssignedTo,
        from_id: ours.0.id(),
        to_id: t.lead,
        status: RelationStatus::Accepted,
        created_by: Ref::new(t.lead),
        via: Json::new(json!({})),
    })
    .await?;
    trx.commit().await?;

    let json = view(&t.alpha, Reader::Identity("tester")).await?;
    let section = &json["sections"][0];
    let titles: Vec<&Value> = section["records"].as_array().into_iter().flatten().map(|r| &r["title"]).collect();
    assert_eq!(titles, [&json!("Readers keep their place")], "{json}");
    assert_eq!(section["lines"], json!(["fwd could not read 2 of the intentions assigned to this session."]));
    Ok(())
}
