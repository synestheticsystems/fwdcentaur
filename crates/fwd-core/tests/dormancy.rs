//! Which agents show as dormant: an agent whose last write is more than twelve hours old, which the
//! pickers and `fwd principal list` list below the others, and which is active again at its next
//! write; never a person, and never enforced (docs/DECISIONS.md, D123; `fwd_core::dormancy`).

use ankurah::{policy::DEFAULT_CONTEXT, Context, EntityId, Node, PermissiveAgent};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::dormancy;
use fwd_core::model::*;
use fwd_core::naming;
use fwd_core::ops::{self, MessageContext};
use fwd_core::project_graph::ProjectGraph;
use fwd_core::routing;
use std::sync::Arc;

const HOUR: i64 = 60 * 60 * 1000;

/// A project alone in a store in memory: the node's context, through which a test writes a record
/// as an older build wrote it, and the project's graph.
async fn older_project() -> Result<(Context, ProjectGraph)> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    let ctx = node.context_async(DEFAULT_CONTEXT).await?;
    let graph = ProjectGraph::ensure(ctx.clone(), uuid::Uuid::new_v4(), "test").await?;
    Ok((ctx, graph))
}

/// The graph of a project alone in a store in memory.
async fn project() -> Result<ProjectGraph> {
    Ok(older_project().await?.1)
}

/// Whether principal `id` shows as dormant at `now`, read from the graph.
async fn dormant(graph: &ProjectGraph, id: EntityId, now: i64) -> Result<bool> {
    Ok(dormancy::is_dormant(&graph.get::<PrincipalView>(id).await?, now))
}

/// Records `at` as the time of principal `id`'s last write, as though it had last written then.
async fn last_wrote(graph: &ProjectGraph, id: EntityId, at: i64) -> Result<()> {
    let p: PrincipalView = graph.get(id).await?;
    let trx = graph.begin();
    trx.edit(&p)?.last_active()?.set(&Some(at))?;
    trx.commit().await?;
    Ok(())
}

/// An agent (`implementer`) named `handle`, made as `fwd principal add` makes one.
async fn agent(graph: &ProjectGraph, handle: &str) -> Result<EntityId> {
    ops::add_principal(graph, handle, Some(AgentRole::Implementer), None, None, None, None).await
}

/// The principal as a build before 2026-09-30 wrote it, with no `last_active`, and with the project
/// it names: the same collection as `fwd_core::model::Principal`, so what it writes is what a graph
/// of then holds.
mod before {
    use ankurah::{Context, Model, Ref};
    use fwd_core::model::{AgentRole, PrincipalKind, Project};
    use serde::{Deserialize, Serialize};

    #[derive(Model, Debug, Clone, Serialize, Deserialize)]
    pub struct Principal {
        pub project: Ref<Project>,
        #[active_type(LWW)]
        pub kind: PrincipalKind,
        #[active_type(LWW)]
        pub handle: String,
        #[active_type(LWW)]
        pub display_name: String,
        #[active_type(LWW)]
        pub auth_subject: Option<String>,
        #[active_type(LWW)]
        pub agent_role: Option<AgentRole>,
        pub active: bool,
        #[active_type(LWW)]
        pub member: Option<bool>,
    }

    /// A principal of `kind` named `handle`, belonging to the project, as that build made one.
    pub async fn made(
        ctx: &Context,
        project: Ref<Project>,
        handle: &str,
        kind: PrincipalKind,
    ) -> anyhow::Result<ankurah::EntityId> {
        let trx = ctx.begin();
        let id = trx
            .create(&Principal {
                project,
                kind,
                handle: handle.into(),
                display_name: handle.into(),
                auth_subject: None,
                agent_role: (kind == PrincipalKind::Agent).then_some(AgentRole::Implementer),
                active: true,
                member: Some(true),
            })
            .await?
            .id();
        trx.commit().await?;
        Ok(id)
    }
}

#[tokio::test]
async fn an_agent_quiet_for_thirteen_hours_is_dormant_and_its_next_write_brings_it_back() -> Result<()> {
    let graph = project().await?;
    let daniel = ops::ensure_principal(&graph, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    ops::join(&graph, daniel).await?;
    let quiet = agent(&graph, "impl-quiet").await?;
    let now = fwd_core::now_ms();
    assert!(!dormant(&graph, quiet, now).await?, "naming itself is its first write");

    last_wrote(&graph, quiet, now - 13 * HOUR).await?;
    assert!(dormant(&graph, quiet, now).await?, "thirteen hours after its last write, it is dormant");
    last_wrote(&graph, quiet, now - 11 * HOUR).await?;
    assert!(!dormant(&graph, quiet, now).await?, "eleven hours after, it is not yet");
    last_wrote(&graph, quiet, now - 13 * HOUR).await?;

    // Dormant decides nothing: it still belongs, its name still names it, and a question to it
    // still reaches it.
    let p = graph.get::<PrincipalView>(quiet).await?;
    assert!(ops::belongs(&p), "a picker still offers it");
    assert_eq!(naming::principal_named(&graph, "impl-quiet", None).await?.map(|p| p.id()), Some(quiet));
    let to = MessageContext { to: Some(quiet.to_base64()), ..Default::default() };
    let asked = ops::post_message(&graph, daniel, "still there?", Priority::Next, to).await?;
    let plan = routing::plan_for_message(&graph, &graph.get::<MessageView>(asked).await?).await?;
    assert!(
        plan.iter().any(|p| p.recipient == quiet && p.notification_type == NotificationType::QuestionAsked),
        "a question addressed to a dormant agent reaches it"
    );

    // It writes, and each client stamps the write, which makes it active again.
    let reply = MessageContext { reply_to: Some(asked), ..Default::default() };
    ops::post_message(&graph, quiet, "here", Priority::Next, reply).await?;
    dormancy::wrote(&graph, quiet).await?;
    assert!(!dormant(&graph, quiet, fwd_core::now_ms()).await?, "its next write brings it back");
    Ok(())
}

#[tokio::test]
async fn a_person_is_never_dormant_nor_a_retired_agent_and_an_agent_from_before_the_stamp_is() -> Result<()> {
    let (ctx, graph) = older_project().await?;
    let later = fwd_core::now_ms() + 100 * HOUR;
    let daniel = ops::ensure_principal(&graph, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    assert!(!dormant(&graph, daniel, later).await?, "a person no write has stamped");
    dormancy::wrote(&graph, daniel).await?;
    assert!(!dormant(&graph, daniel, later).await?, "a person quiet for a hundred hours");
    let old_person = before::made(&ctx, graph.project(), "old-person", PrincipalKind::Human).await?;
    assert!(!dormant(&graph, old_person, later).await?, "a person from before the stamp");

    let old_agent = before::made(&ctx, graph.project(), "impl-old", PrincipalKind::Agent).await?;
    let p = graph.get::<PrincipalView>(old_agent).await?;
    assert_eq!(p.last_active()?, None, "an agent from before the stamp records no write");
    assert!(dormancy::is_dormant(&p, fwd_core::now_ms()), "so it is dormant until its next write");
    dormancy::wrote(&graph, old_agent).await?;
    assert!(!dormant(&graph, old_agent, fwd_core::now_ms()).await?, "which brings it back");

    let finished = agent(&graph, "impl-finished").await?;
    last_wrote(&graph, finished, fwd_core::now_ms() - 13 * HOUR).await?;
    ops::retire_principal(&graph, "impl-finished").await?;
    assert!(!dormant(&graph, finished, fwd_core::now_ms()).await?, "retired, it is no picker's, dormant or not");
    Ok(())
}

#[tokio::test]
async fn an_agent_that_names_itself_as_a_successor_is_active_from_its_first_write() -> Result<()> {
    let graph = project().await?;
    let old = agent(&graph, "impl-first").await?;
    last_wrote(&graph, old, fwd_core::now_ms() - 13 * HOUR).await?;
    let done = ops::add_successor(&graph, "impl-second", None, None, None, None, None, "impl-first").await?;
    let now = fwd_core::now_ms();
    assert!(!dormant(&graph, done.successor, now).await?, "the successor named itself just now");
    assert!(!dormant(&graph, old, now).await?, "and the one it succeeded is retired, so no picker's");
    Ok(())
}

#[tokio::test]
async fn the_mention_search_offers_the_agents_at_work_before_the_dormant_ones() -> Result<()> {
    let graph = project().await?;
    let quiet = agent(&graph, "impl-a").await?;
    let busy = agent(&graph, "impl-b").await?;
    let busier = agent(&graph, "impl-c").await?;
    let now = fwd_core::now_ms();
    last_wrote(&graph, quiet, now - 13 * HOUR).await?;
    let principals = ops::principals(&graph).await?;
    let offered = |most: usize| -> Vec<EntityId> {
        naming::mentionable(&principals, "impl", most, now).iter().map(|p| p.id()).collect()
    };
    assert_eq!(offered(8), vec![busy, busier, quiet], "the dormant agent after the others, whatever its name");
    assert_eq!(offered(2), vec![busy, busier], "and out of the first offers");
    assert_eq!(
        naming::mentionable(&principals, "impl-a", 8, now).iter().map(|p| p.id()).collect::<Vec<_>>(),
        vec![quiet],
        "still offered where its name is typed"
    );
    Ok(())
}
