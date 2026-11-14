//! The live forms of the reads of what an agent holds (`ProjectGraph::query_unfinished_tasks_assigned_to`,
//! `query_open_questions_by`, `query_open_questions_to`, `query_answers_to`), over two projects of
//! one node: each holds what its read reads, by the principal's or the question's id, wherever the
//! records are, and lets a record go once it no longer matches, so that a view following a session
//! hears of what was added, changed or closed in any project.

use ankurah::signals::Peek;
use ankurah::{policy::DEFAULT_CONTEXT, EntityId, LiveQuery, Node, PermissiveAgent, View};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::model::*;
use fwd_core::ops::{self, CreateTask, MessageContext};
use fwd_core::project_graph::{InProjectView, ProjectGraph};
use fwd_core::rules::Verb;
use std::collections::BTreeSet;
use std::sync::Arc;

/// alpha and beta over one store in memory; `agent` is alpha's agent, `person` beta's person.
struct Two {
    alpha: ProjectGraph,
    beta: ProjectGraph,
    agent: EntityId,
    person: EntityId,
}

async fn two() -> Result<Two> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    let ctx = node.context_async(DEFAULT_CONTEXT).await?;
    let alpha = ProjectGraph::ensure(ctx.clone(), uuid::Uuid::new_v4(), "alpha").await?;
    let beta = ProjectGraph::ensure(ctx, uuid::Uuid::new_v4(), "beta").await?;
    let agent = ops::ensure_principal(&alpha, "lead-sam", "lead-sam", PrincipalKind::Agent, None).await?;
    let person = ops::ensure_principal(&beta, "tester", "tester", PrincipalKind::Human, None).await?;
    Ok(Two { alpha, beta, agent, person })
}

/// The ids of what `query` holds once it has its first results.
async fn held<V: InProjectView + Clone + 'static>(query: &LiveQuery<V>) -> Result<BTreeSet<EntityId>> {
    query.wait_initialized().await?;
    Ok(query.peek().iter().map(View::id).collect())
}

fn ids(of: &[EntityId]) -> BTreeSet<EntityId> {
    of.iter().copied().collect()
}

fn asking(to: EntityId) -> MessageContext {
    MessageContext { to: Some(to.to_base64()), ..Default::default() }
}

/// A task of alpha's assigned to alpha's agent is held by the live read opened through beta's
/// graph, as it is wherever the task is, until it is done; a task no one holds is not.
#[tokio::test]
async fn the_tasks_assigned_to_a_principal_are_followed_wherever_they_are_until_finished() -> Result<()> {
    let t = two().await?;
    let create = |title: &str| CreateTask { title: title.into(), ..Default::default() };
    let task = ops::create_task(&t.alpha, t.agent, create("Cap the backoff")).await?;
    ops::create_task(&t.alpha, t.agent, create("No one's")).await?;
    let assigned = t.beta.query_unfinished_tasks_assigned_to(t.agent)?;
    assert_eq!(held(&assigned).await?, ids(&[]));
    ops::assign(&t.alpha, t.agent, task, Some(t.agent)).await?;
    assert_eq!(held(&assigned).await?, ids(&[task]));
    ops::transition(&t.alpha, t.agent, task, Verb::Start, None).await?;
    ops::transition(&t.alpha, t.agent, task, Verb::Done, None).await?;
    assert_eq!(held(&assigned).await?, ids(&[]), "a finished task leaves");
    Ok(())
}

/// The questions a principal asked and those asked of it by its id are followed wherever they
/// are, until resolved; a message that asks no one is in neither.
#[tokio::test]
async fn the_questions_by_and_to_a_principal_are_followed_wherever_they_are_until_resolved() -> Result<()> {
    let t = two().await?;
    let by = t.alpha.query_open_questions_by(t.agent)?;
    let to = t.alpha.query_open_questions_to(t.agent)?;
    assert_eq!((held(&by).await?, held(&to).await?), (ids(&[]), ids(&[])));
    let asked =
        ops::post_message(&t.alpha, t.agent, "Should the primer name the page?", Priority::Now, asking(t.person))
            .await?;
    let asked_of = ops::post_message(&t.beta, t.person, "How long a wait?", Priority::Now, asking(t.agent)).await?;
    ops::post_message(&t.beta, t.person, "A comment that asks no one.", Priority::Next, MessageContext::default())
        .await?;
    assert_eq!((held(&by).await?, held(&to).await?), (ids(&[asked]), ids(&[asked_of])));
    ops::resolve_message(&t.alpha, t.agent, asked).await?;
    ops::resolve_message(&t.beta, t.person, asked_of).await?;
    assert_eq!((held(&by).await?, held(&to).await?), (ids(&[]), ids(&[])), "a resolved question leaves");
    Ok(())
}

/// The answers to a question are followed wherever they are written: a reply to it, and a reply
/// to a reply in the thread it opens; a message of another thread is not among them.
#[tokio::test]
async fn the_answers_in_a_questions_thread_are_followed_wherever_they_are_written() -> Result<()> {
    let t = two().await?;
    let question =
        ops::post_message(&t.alpha, t.agent, "Should the primer name the page?", Priority::Now, asking(t.person))
            .await?;
    let answers = t.alpha.query_answers_to(question)?;
    assert_eq!(held(&answers).await?, ids(&[]));
    let reply = MessageContext { reply_to: Some(question), ..Default::default() };
    let first = ops::post_message(&t.beta, t.person, "Not yet.", Priority::Next, reply).await?;
    let again = MessageContext { reply_to: Some(first), ..Default::default() };
    let second = ops::post_message(&t.beta, t.person, "No: the pane links there.", Priority::Next, again).await?;
    ops::post_message(&t.beta, t.person, "Another thread.", Priority::Next, MessageContext::default()).await?;
    assert_eq!(held(&answers).await?, ids(&[first, second]));
    Ok(())
}

/// The principals bound to an agent of one Claude Code session, its own thread's and its
/// subagents', are followed in their project, and a principal of another session, even one whose
/// id starts with this one's, of a Codex thread, or of no agent, is not among them.
#[tokio::test]
async fn the_principals_of_a_session_are_followed_and_no_others() -> Result<()> {
    let t = two().await?;
    let add = |name: &'static str, subject: &'static str| {
        let alpha = t.alpha.clone();
        async move { ops::add_principal(&alpha, name, Some(AgentRole::Implementer), None, None, None, Some(subject)).await }
    };
    let bound = t.alpha.query::<PrincipalView>(fwd_core::subject::of_session_selection("s1")?)?;
    assert_eq!(held(&bound).await?, ids(&[]));
    let main = add("lead-s1", "claude-code:s1/main").await?;
    let subagent = add("impl-s1", "claude-code:s1/a1").await?;
    add("impl-s10", "claude-code:s10/main").await?;
    add("impl-codex", "codex:s1/s1").await?;
    assert_eq!(held(&bound).await?, ids(&[main, subagent]));
    Ok(())
}
