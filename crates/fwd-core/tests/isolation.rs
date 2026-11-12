//! Two projects in one node, as the daemon keeps every project it hosts (docs/DECISIONS.md, D135),
//! holding look-alike records: the same handles, titles, scope names and decision numbers. A
//! project is a grouping and the filter of every list, not a wall (D176, D181): every list and every
//! live query of one project holds that project's records alone, in the shapes the page, the
//! wallboard and `fwd watch` open them, before or after it was opened, and routing in one project
//! tells that project's principals alone; while an id someone types, an id a record holds and a
//! record's relations reach a record of the other project, and what they give says which project it
//! is in. One project's graph changes none of the other's records.

use ankurah::changes::ChangeSet;
use ankurah::error::RetrievalError;
use ankurah::property::{Json, Ref};
use ankurah::signals::{Get, Subscribe};
use ankurah::{policy::DEFAULT_CONTEXT, Context, EntityId, LiveQuery, Node, PermissiveAgent, View};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::model::*;
use fwd_core::ops::{self, CreateDecision, CreateScope, CreateTask, Kind, MessageContext};
use fwd_core::project_graph::{Followed, InProjectView, ProjectGraph, ProjectGraphError, Whose};
use fwd_core::query::with_id;
use fwd_core::{decision_log, naming, routing};
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Two projects in one store in memory, each by its graph, and the node's context, through which a
/// test writes what no graph would.
async fn two_projects_and_the_node() -> Result<(Context, ProjectGraph, ProjectGraph)> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    let ctx = node.context_async(DEFAULT_CONTEXT).await?;
    let alpha = ProjectGraph::ensure(ctx.clone(), uuid::Uuid::new_v4(), "alpha").await?;
    let beta = ProjectGraph::ensure(ctx.clone(), uuid::Uuid::new_v4(), "beta").await?;
    Ok((ctx, alpha, beta))
}

/// Two projects in one store in memory, each by its graph.
async fn two_projects() -> Result<(ProjectGraph, ProjectGraph)> {
    let (_, alpha, beta) = two_projects_and_the_node().await?;
    Ok((alpha, beta))
}

/// What one project holds once [`populate`] has run in it.
struct Held {
    daniel: EntityId,
    alice: EntityId,
    triage: EntityId,
    scope: EntityId,
    retry: EntityId,
    cap: EntityId,
    decision: EntityId,
    question: EntityId,
    anchor: EntityId,
    /// An id `retry` had before it was renumbered.
    cited: EntityId,
}

/// The same records in any project: a person, an implementer and a triage agent; a scope; two
/// tasks, one waiting on the other and claimed; D1; a question to the implementer, which routing
/// tells her of, and a comment on a task; a highlight that is a step; and a former id.
async fn populate(graph: &ProjectGraph) -> Result<Held> {
    let daniel = ops::ensure_principal(graph, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    ops::join(graph, daniel).await?;
    let alice = ops::add_principal(graph, "impl-alice", Some(AgentRole::Implementer), None, None, None, None).await?;
    let triage = ops::add_principal(
        graph,
        "triage-bot",
        Some(AgentRole::Triage),
        Some(ops::Serves::ProjectWide),
        None,
        None,
        None,
    )
    .await?;
    let scope =
        ops::create_scope(graph, daniel, CreateScope { title: "Replication".into(), ..Default::default() }).await?;
    let task = |title: &str| CreateTask { title: title.into(), ..Default::default() };
    let cap = ops::create_task(graph, daniel, task("Cap the retry delay")).await?;
    let retry = ops::create_task(graph, daniel, CreateTask { after: vec![cap], ..task("Retry on disconnect") }).await?;
    ops::claim(graph, alice, retry, scope).await?;
    let ruling = CreateDecision { title: "Cap retries at 30s".into(), decided_by: Some(daniel), ..Default::default() };
    let decision = ops::create_decision(graph, daniel, ruling).await?;
    let to_alice = MessageContext { to: Some(alice.to_base64()), ..Default::default() };
    let question = ops::post_message(graph, daniel, "who takes the TLS part?", Priority::Now, to_alice).await?;
    let asked: MessageView = graph.get(question).await?;
    routing::apply(graph, &routing::plan_for_message(graph, &asked).await?).await?;
    let on_task = MessageContext { task_id: Some(retry), ..Default::default() };
    ops::post_message(graph, alice, "look at the backoff", Priority::Next, on_task).await?;
    let anchor = anchor_in(graph, daniel).await?;
    ops::add_step(graph, alice, retry, anchor).await?;
    let cited = EntityId::random();
    let trx = graph.begin();
    trx.create(&FormerId { project: graph.project(), former: cited.to_base64(), record: retry }).await?;
    trx.commit().await?;
    Ok(Held { daniel, alice, triage, scope, retry, cap, decision, question, anchor, cited })
}

/// A repository and a highlight in it, the same in every project.
async fn anchor_in(graph: &ProjectGraph, by: EntityId) -> Result<EntityId> {
    let trx = graph.begin();
    let repo = trx
        .create(&Repo {
            project: graph.project(),
            name: "fwdcentaur".into(),
            remote_urls: Json::new(json!(["git@github.com:synestheticsystems/fwdcentaur.git"])),
            default_branch: "main".into(),
        })
        .await?
        .id();
    let anchor = trx
        .create(&CodeAnchor {
            project: graph.project(),
            repo: Ref::new(repo),
            path: "src/net/ws.rs".into(),
            anchor_key: Some("fwd-anchor:1:00".into()),
            span_key: None,
            basis_blob: None,
            head_blob: None,
            index_blob: None,
            nearest_blob: None,
            basis_patch: None,
            blob_sha: None,
            commit_sha: None,
            side: DiffSide::Worktree,
            start_line: 0,
            end_line: 0,
            whole_file: Some(true),
            anchored_text: String::new(),
            context_before: String::new(),
            context_after: String::new(),
            pieces: Json::new(json!([])),
            symbol_path: None,
            created_by: Ref::new(by),
            created: fwd_core::now_ms(),
        })
        .await?
        .id();
    trx.commit().await?;
    Ok(anchor)
}

/// The ids of every record `graph` holds, of every model.
async fn every_id(graph: &ProjectGraph) -> Result<BTreeSet<EntityId>> {
    fn ids<V: View>(views: Vec<V>) -> Vec<EntityId> {
        views.iter().map(View::id).collect()
    }
    let mut all = BTreeSet::new();
    all.extend(ids(graph.all::<PrincipalView>().await?));
    all.extend(ids(graph.all::<RepoView>().await?));
    all.extend(ids(graph.all::<TaskView>().await?));
    all.extend(ids(graph.all::<RelationView>().await?));
    all.extend(ids(graph.all::<CodeAnchorView>().await?));
    all.extend(ids(graph.all::<MessageView>().await?));
    all.extend(ids(graph.all::<NotificationView>().await?));
    all.extend(ids(graph.all::<ScopeView>().await?));
    all.extend(ids(graph.all::<DecisionView>().await?));
    all.extend(ids(graph.all::<FormerIdView>().await?));
    Ok(all)
}

/// Fails, naming `read`, when `found` holds any of `others`.
fn none_of(others: &BTreeSet<EntityId>, read: &str, found: impl IntoIterator<Item = EntityId>) {
    let leaked: Vec<EntityId> = found.into_iter().filter(|id| others.contains(id)).collect();
    assert!(leaked.is_empty(), "{read} holds the other project's records: {leaked:?}");
}

fn ids_of<V: View>(views: &[V]) -> Vec<EntityId> {
    views.iter().map(View::id).collect()
}

#[tokio::test]
async fn every_list_of_one_project_holds_its_own_records_and_none_of_the_others() -> Result<()> {
    let (alpha, beta) = two_projects().await?;
    let a = populate(&alpha).await?;
    populate(&beta).await?;
    let others = every_id(&beta).await?;
    assert!(every_id(&alpha).await?.is_disjoint(&others));
    assert_eq!(every_id(&alpha).await?.len(), others.len(), "the two hold look-alike records");

    // Each read below finds this project's record, so that none passes by finding nothing.
    let principals = ops::principals(&alpha).await?;
    none_of(&others, "principals", ids_of(&principals));
    assert_eq!(principals.len(), 3);
    assert_eq!(ops::people(&alpha).await?, vec![a.daniel]);
    assert_eq!(ops::person_by_handle(&alpha, "dnorman").await?, a.daniel, "a handle the other project shares too");
    let alice = naming::principal_named(&alpha, "impl-alice", None).await?.map(|p| p.id());
    assert_eq!(alice, Some(a.alice), "a name two projects share is no shared name in either");
    assert_eq!(ops::triage_for(&alpha, None).await?.map(|f| f.agent), Some(a.triage));
    assert_eq!(ops::resolve_scope(&alpha, "Replication").await?, a.scope, "a scope name the other project shares too");
    let tasks: Vec<TaskView> = alpha.all().await?;
    none_of(&others, "every task", ids_of(&tasks));
    assert_eq!(tasks.len(), 2);
    let ready = ops::ready_tasks(&alpha, None).await?;
    assert_eq!(ids_of(&ready), vec![a.cap]);
    let open: Vec<TaskView> = alpha.fetch("status = 'open'").await?;
    none_of(&others, "the open tasks", ids_of(&open));
    let relations: Vec<RelationView> = alpha.all().await?;
    none_of(&others, "every relation", ids_of(&relations));
    let dependencies: Vec<RelationView> = alpha.fetch("kind = 'depends_on'").await?;
    assert_eq!(ids_of(&dependencies).len(), 1);
    let about = ops::messages_about(&alpha, "task", a.retry).await?;
    none_of(&others, "the messages on a task", ids_of(&about));
    assert_eq!(about.len(), 1);
    let inbox = ops::inbox(&alpha, a.alice, false).await?;
    none_of(&others, "an inbox", ids_of(&inbox));
    assert_eq!(inbox.len(), 1, "the question to her, and no other");
    let decisions = ops::decisions(&alpha).await?;
    assert_eq!(ids_of(&decisions), vec![a.decision]);
    let log = decision_log::entries(&alpha).await?;
    assert_eq!(log.len(), 1, "the decision log holds this project's ruling alone");
    none_of(&others, "the anchors at a path", ids_of(&ops::anchors_in_path(&alpha, "src/net/ws.rs").await?));
    let planned = routing::plan_for_waiting_questions(&alpha).await?;
    none_of(&others, "the waiting questions", planned.iter().map(|p| p.recipient));
    Ok(())
}

/// Fails unless `project` is beta, as the graph of alpha names it.
fn beta_named(project: &Whose) {
    assert!(matches!(project, Whose::Other(p) if p.name == "beta"), "{project:?}");
}

#[tokio::test]
async fn a_typed_id_a_held_id_and_a_records_relations_reach_the_other_project_and_name_it() -> Result<()> {
    let (alpha, beta) = two_projects().await?;
    let a = populate(&alpha).await?;
    let b = populate(&beta).await?;

    // An id someone types, whole, by its start, or as a former id whole or as a prefix, names
    // beta's record, and says it is beta's; D1 is alpha's own, since a number is its project's.
    let start = |id: EntityId| id.to_base64()[..8].to_string();
    for (named, record) in [
        (b.retry.to_base64(), b.retry),
        (start(b.retry), b.retry),
        (start(b.question), b.question),
        (b.cited.to_base64(), b.retry),
        (start(b.cited), b.retry),
    ] {
        let found = ops::locate(&alpha, &named, ops::ANY).await?;
        assert_eq!(found.id, record, "`{named}` names beta's record");
        beta_named(&found.project);
    }
    assert_eq!(ops::locate(&alpha, &start(a.retry), ops::ANY).await?.project, Whose::This);
    assert_eq!(ops::resolve_id(&alpha, "D1", &[Kind::Decision]).await?, a.decision, "D1 is alpha's own D1");
    assert_eq!(ops::resolve_id(&alpha, &a.cited.to_base64(), ops::ANY).await?, a.retry, "alpha's own former id");

    // An id a record holds is followed into beta, which every reader names beside the record.
    for id in [b.daniel, b.retry, b.question, b.decision, b.scope, b.anchor] {
        assert!(ops::kind_of(&alpha, id).await.is_some(), "{id}");
        let (_, name) = ops::describe(&alpha, id).await.expect("described");
        assert!(name.ends_with(" in beta"), "{name}");
        assert_eq!(ops::reference_json(&alpha, id).await["project"], "beta");
    }
    assert!(matches!(alpha.follow::<TaskView>(b.retry).await?, Followed::Held { project: Whose::Other(_), .. }));
    // Named in a text: alpha's message names beta's task, and a handle both projects have names
    // alpha's principal, since a handle is its project's.
    let posted =
        ops::post_message(&alpha, a.daniel, &format!("see [[{}]]", start(b.retry)), Priority::Next, Default::default())
            .await?;
    assert_eq!(ops::text(alpha.get::<MessageView>(posted).await?.content())?, format!("see [[{}]]", b.retry));
    let posted = ops::post_message(&alpha, a.daniel, "ask @impl-alice", Priority::Next, Default::default()).await?;
    let stored = ops::text(alpha.get::<MessageView>(posted).await?.content())?;
    assert_eq!(stored, format!("ask [[{}]]", a.alice), "a handle both projects have names alpha's");

    // A relation made in alpha joins alpha's task to beta's, and shows on beta's task wherever
    // its relations are read.
    let link = ops::link(&alpha, a.daniel, RelationKind::DependsOn, a.cap, b.cap, None, false, json!({})).await?;
    assert!(ids_of(&ops::relations_to(&beta, b.cap, None).await?).contains(&link));
    assert!(ids_of(&ops::relations_from(&alpha, a.cap, None).await?).contains(&link));
    let dependency = ops::dependencies(&alpha, a.cap).await?;
    assert!(
        matches!(dependency.as_slice(), [fwd_core::rules::Dependency::Elsewhere { project, .. }] if project == "beta")
    );
    Ok(())
}

#[tokio::test]
async fn one_projects_graph_changes_none_of_the_others_records() -> Result<()> {
    let (alpha, beta) = two_projects().await?;
    let a = populate(&alpha).await?;
    let b = populate(&beta).await?;

    // Read by its id through alpha's graph, beta's record is not alpha's to change: a command whose
    // subject it is runs in beta.
    let refused = alpha.get::<TaskView>(b.retry).await.expect_err("beta's task");
    assert!(matches!(refused, RetrievalError::EntityNotFound(id) if id == b.retry), "{refused:?}");
    let theirs: TaskView = beta.get(b.retry).await?;
    let trx = alpha.begin();
    let edited = trx.edit(&theirs).map(|_| ()).expect_err("alpha's transaction edits none of beta's records");
    assert!(matches!(edited, ProjectGraphError::OfAnotherProject { record, .. } if record == b.retry), "{edited:?}");
    let made = trx.create(&FormerId { project: beta.project(), former: "x".repeat(43), record: b.retry }).await;
    assert!(matches!(made, Err(ProjectGraphError::OtherProject { .. })), "{made:?}");
    let named = alpha.ensure_own(&theirs).await.expect_err("named");
    assert!(named.to_string().contains("project `beta`"), "{named}");
    // And operations on beta's records through alpha's graph change nothing of beta's.
    assert!(ops::transition(&alpha, a.daniel, b.retry, fwd_core::rules::Verb::Cancel, Some("no")).await.is_err());
    assert!(ops::assign(&alpha, a.daniel, b.cap, Some(a.alice)).await.is_err());
    assert!(
        ops::add_step(&alpha, a.daniel, b.retry, a.anchor).await.is_ok(),
        "alpha's step on beta's task is alpha's relation"
    );
    assert_eq!(beta.get::<TaskView>(b.retry).await?.status()?, TaskStatus::InProgress);
    assert_eq!(ops::assignee_of(&beta.get::<TaskView>(b.cap).await?)?, None);
    // A relation beta made, read on alpha's side, is refused there, naming beta.
    let from_beta = ops::link(&beta, b.daniel, RelationKind::RelatesTo, b.cap, a.cap, None, false, json!({})).await?;
    let unlinked =
        ops::unlink(&alpha, a.daniel, RelationKind::RelatesTo, b.cap, a.cap, "no").await.expect_err("beta's");
    assert!(unlinked.to_string().contains("project `beta`"), "{unlinked}");
    assert_eq!(beta.relation(from_beta).await?.status()?, RelationStatus::Accepted);
    Ok(())
}

/// How long a test waits for an arrival before it fails as one that never came.
const NEVER: std::time::Duration = std::time::Duration::from_secs(60);

/// Every id a live query's change sets have named, and each as it arrives.
struct Heard {
    named: Arc<Mutex<Vec<EntityId>>>,
    arrived: mpsc::UnboundedReceiver<EntityId>,
    _guard: ankurah::signals::SubscriptionGuard,
}

impl Heard {
    fn on<V: View + Clone + Send + Sync + 'static>(query: &LiveQuery<V>) -> Heard {
        let named = Arc::new(Mutex::new(Vec::new()));
        let (tx, arrived) = mpsc::unbounded_channel();
        let kept = named.clone();
        let _guard = query.subscribe(move |changes: ChangeSet<V>| {
            for change in &changes.changes {
                let id = change.entity().id();
                kept.lock().expect("heard").push(id);
                let _ = tx.send(id);
            }
        });
        Heard { named, arrived, _guard }
    }

    /// Returns once the query has named `id`; fails the test, naming `id`, when a minute passes
    /// first. The minute only detects an arrival that never comes: what orders the test is the
    /// arrival itself.
    async fn until(&mut self, id: EntityId) {
        let arrived = async { while self.arrived.recv().await != Some(id) {} };
        if tokio::time::timeout(NEVER, arrived).await.is_err() {
            panic!("the live query never named {id} within {}s", NEVER.as_secs());
        }
    }

    fn named(&self) -> Vec<EntityId> {
        self.named.lock().expect("heard").clone()
    }
}

/// One live query a client opens: what it is, what its change sets have named, and what it holds.
struct Opened {
    name: &'static str,
    heard: Heard,
    holds: Box<dyn Fn() -> Vec<EntityId> + Send>,
}

impl Opened {
    fn on<V: InProjectView + Clone + Send + Sync + 'static>(name: &'static str, query: LiveQuery<V>) -> Opened {
        let heard = Heard::on(&query);
        Opened { name, heard, holds: Box::new(move || query.get().iter().map(View::id).collect()) }
    }
}

/// The live queries of one project the clients open: each page's standing query on every
/// collection, the Inbox's and a reader's own messages, the wallboard's tasks, its query of a
/// prerequisite by id, and `fwd watch`'s unread notifications. The tasks query comes first.
struct Standing(Vec<Opened>);

impl Standing {
    async fn open(graph: &ProjectGraph, reader: EntityId, prerequisite: EntityId) -> Result<Standing> {
        let inbox = with_id("recipient = ? AND status != 'dismissed'", reader)?;
        let wallboard = "created > 0 AND (status = 'open' OR status = 'in_progress' OR status = 'proposed')";
        let by_id = format!("id = '{}'", prerequisite.to_base64());
        let unread = with_id("recipient = ? AND status = 'unread'", reader)?;
        Ok(Standing(vec![
            Opened::on("tasks", graph.query_all::<TaskView>()?),
            Opened::on("relations", graph.query_all::<RelationView>()?),
            Opened::on("scopes", graph.query_all::<ScopeView>()?),
            Opened::on("principals", graph.query_all::<PrincipalView>()?),
            Opened::on("messages", graph.query_all::<MessageView>()?),
            Opened::on("anchors", graph.query_all::<CodeAnchorView>()?),
            Opened::on("decisions", graph.query_all::<DecisionView>()?),
            Opened::on("former ids", graph.query_all::<FormerIdView>()?),
            Opened::on("the inbox", graph.query::<NotificationView>(inbox)?),
            Opened::on("a reader's messages", graph.query::<MessageView>(with_id("author = ?", reader)?)?),
            Opened::on("the wallboard's tasks", graph.query::<TaskView>(wallboard)?),
            Opened::on("a prerequisite by id", graph.query::<TaskView>(by_id.as_str())?),
            Opened::on("fwd watch", graph.query::<NotificationView>(unread)?),
        ]))
    }

    /// The query named `name`.
    fn named(&mut self, name: &str) -> &mut Opened {
        self.0.iter_mut().find(|opened| opened.name == name).expect("a query of that name")
    }

    /// Fails when any query holds, or has ever named, one of `others`.
    fn holds_none_of(&self, others: &BTreeSet<EntityId>) {
        for opened in &self.0 {
            none_of(others, &format!("{}, as it holds now", opened.name), (opened.holds)());
            none_of(others, &format!("{}, in any change set", opened.name), opened.heard.named());
        }
    }
}

#[tokio::test]
async fn a_live_query_of_one_project_never_hears_of_the_others_records_before_or_after_it_opened() -> Result<()> {
    let (alpha, beta) = two_projects().await?;
    let a = populate(&alpha).await?;
    let b = populate(&beta).await?;
    let mut mine = Standing::open(&alpha, a.alice, b.cap).await?;
    let mut theirs = Standing::open(&beta, b.alice, b.cap).await?;

    // After the queries opened: beta gains a task, a question to its implementer, a ruling and a
    // former id, all as alpha's would be; each reaches beta's own queries first.
    let task =
        ops::create_task(&beta, b.daniel, CreateTask { title: "Retry on disconnect".into(), ..Default::default() })
            .await?;
    let to_alice = MessageContext { to: Some(b.alice.to_base64()), ..Default::default() };
    let question = ops::post_message(&beta, b.daniel, "who takes the TLS part?", Priority::Now, to_alice).await?;
    let asked: MessageView = beta.get(question).await?;
    let told = routing::apply(&beta, &routing::plan_for_message(&beta, &asked).await?).await?;
    let ruling =
        CreateDecision { title: "Cap retries at 30s".into(), decided_by: Some(b.daniel), ..Default::default() };
    ops::create_decision(&beta, b.daniel, ruling).await?;
    theirs.named("tasks").heard.until(task).await;
    for notification in &told {
        theirs.named("the inbox").heard.until(*notification).await;
    }

    // Then alpha gains a task; once its queries have it, they have heard of beta's writes or never
    // will.
    let marker =
        ops::create_task(&alpha, a.daniel, CreateTask { title: "Cap the retry delay".into(), ..Default::default() })
            .await?;
    mine.named("tasks").heard.until(marker).await;
    let others = every_id(&beta).await?;
    assert!(others.contains(&task) && others.contains(&question));
    mine.holds_none_of(&others);
    theirs.holds_none_of(&every_id(&alpha).await?);
    Ok(())
}

#[tokio::test]
async fn routing_in_one_project_tells_its_own_principals_and_no_one_in_the_other() -> Result<()> {
    let (alpha, beta) = two_projects().await?;
    let a = populate(&alpha).await?;
    let b = populate(&beta).await?;
    let theirs: BTreeSet<EntityId> = ops::principals(&beta).await?.iter().map(View::id).collect();
    let before = every_id(&beta).await?;

    // A question asked of no one reaches alpha's triage agent; a comment on a claimed task its
    // assignee; a submission for review; a ruling, and its repeal.
    let asked = MessageContext { asks_project: true, ..Default::default() };
    let question = ops::post_message(&alpha, a.daniel, "who takes the release?", Priority::Now, asked).await?;
    let comment = ops::post_message(
        &alpha,
        a.daniel,
        "the backoff doubles",
        Priority::Next,
        MessageContext { task_id: Some(a.retry), ..Default::default() },
    )
    .await?;
    let mut planned = Vec::new();
    for message in [question, comment, a.question] {
        planned.extend(routing::plan_for_message(&alpha, &alpha.get(message).await?).await?);
    }
    let task: TaskView = alpha.get(a.retry).await?;
    planned.extend(routing::plan_for_task_created(&alpha, &task).await?);
    planned.extend(routing::plan_for_waiting_questions(&alpha).await?);
    let ruling = alpha.get::<DecisionView>(a.decision).await?;
    planned.extend(routing::plan_for_decision_created(&alpha, &ruling).await?);
    let recipients: BTreeSet<EntityId> = planned.iter().map(|p| p.recipient).collect();
    assert!(recipients.contains(&a.triage) && recipients.contains(&a.alice), "{recipients:?}");
    assert!(recipients.is_disjoint(&theirs), "routing in alpha told beta's principals: {recipients:?}");
    let made = routing::apply(&alpha, &planned).await?;
    assert!(!made.is_empty());
    assert_eq!(every_id(&beta).await?, before, "nothing of beta's was made or touched");
    assert_eq!(ops::inbox(&beta, b.alice, false).await?.len(), 1, "beta's implementer has her own question alone");
    Ok(())
}

/// A record moved to the other project, here by a raw write of its `project`, which stands in for
/// the move fwd is to make later, leaves every standing query and every view of the project it left
/// and appears in those of the project it joined: a task, and a comment on it, moved together.
#[tokio::test]
async fn a_record_moved_to_the_other_project_leaves_the_first_and_appears_in_the_second() -> Result<()> {
    let (ctx, alpha, beta) = two_projects_and_the_node().await?;
    let a = populate(&alpha).await?;
    let b = populate(&beta).await?;
    let task = ops::create_task(&alpha, a.daniel, CreateTask { title: "Move me".into(), ..Default::default() }).await?;
    let on_task = MessageContext { task_id: Some(task), ..Default::default() };
    let comment = ops::post_message(&alpha, a.daniel, "and me with it", Priority::Next, on_task).await?;
    let mut mine = Standing::open(&alpha, a.daniel, a.cap).await?;
    let mut theirs = Standing::open(&beta, b.daniel, b.cap).await?;
    mine.named("tasks").heard.until(task).await;
    mine.named("messages").heard.until(comment).await;
    mine.named("a reader's messages").heard.until(comment).await;

    let trx = ctx.begin();
    let moving: TaskView = ctx.get(task).await?;
    moving.edit(&trx)?.project()?.set(&beta.project())?;
    let moving: MessageView = ctx.get(comment).await?;
    moving.edit(&trx)?.project()?.set(&beta.project())?;
    trx.commit().await?;
    theirs.named("tasks").heard.until(task).await;
    theirs.named("messages").heard.until(comment).await;
    // Once alpha's queries have a task made after the move, they have heard of the move or never
    // will.
    let marker = ops::create_task(&alpha, a.daniel, CreateTask { title: "Stay".into(), ..Default::default() }).await?;
    mine.named("tasks").heard.until(marker).await;

    for opened in &mine.0 {
        let held = (opened.holds)();
        assert!(!held.contains(&task) && !held.contains(&comment), "alpha's {} still holds what moved", opened.name);
    }
    assert!(alpha.get::<TaskView>(task).await.is_err() && alpha.get::<MessageView>(comment).await.is_err());
    assert!(ops::messages_about(&alpha, "task", task).await?.is_empty());
    let typed = ops::locate(&alpha, &task.to_base64()[..8], ops::ANY).await?;
    assert_eq!(typed.id, task, "its id, typed in alpha, names it in beta");
    beta_named(&typed.project);
    let theirs_now = |name: &str| (theirs.0.iter().find(|o| o.name == name).expect("a query").holds)();
    assert!(theirs_now("tasks").contains(&task) && theirs_now("messages").contains(&comment));
    assert_eq!(beta.get::<TaskView>(task).await?.title()?, "Move me");
    assert_eq!(ids_of(&ops::messages_about(&beta, "task", task).await?), vec![comment]);
    assert_eq!(ops::resolve_id(&beta, &task.to_base64()[..8], ops::ANY).await?, task);
    Ok(())
}
