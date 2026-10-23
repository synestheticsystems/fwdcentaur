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
use fwd_core::routing;
use std::sync::Arc;

const HOUR: i64 = 60 * 60 * 1000;

async fn context() -> Result<Context> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    Ok(node.context_async(DEFAULT_CONTEXT).await?)
}

/// Whether principal `id` shows as dormant at `now`, read from the graph.
async fn dormant(ctx: &Context, id: EntityId, now: i64) -> Result<bool> {
    Ok(dormancy::is_dormant(&ctx.get::<PrincipalView>(id).await?, now))
}

/// Records `at` as the time of principal `id`'s last write, as though it had last written then.
async fn last_wrote(ctx: &Context, id: EntityId, at: i64) -> Result<()> {
    let p: PrincipalView = ctx.get(id).await?;
    let trx = ctx.begin();
    p.edit(&trx)?.last_active()?.set(&Some(at))?;
    trx.commit().await?;
    Ok(())
}

/// An agent (`implementer`) named `handle`, made as `fwd principal add` makes one.
async fn agent(ctx: &Context, handle: &str) -> Result<EntityId> {
    ops::add_principal(ctx, handle, Some(AgentRole::Implementer), None, None, None, None).await
}

/// The principal as a build before 2026-09-30 wrote it, with no `last_active`: the same collection
/// as `fwd_core::model::Principal`, so what it writes is what a graph of then holds.
mod before {
    use ankurah::Model;
    use fwd_core::model::{AgentRole, PrincipalKind};
    use serde::{Deserialize, Serialize};

    #[derive(Model, Debug, Clone, Serialize, Deserialize)]
    pub struct Principal {
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
    pub async fn made(ctx: &ankurah::Context, handle: &str, kind: PrincipalKind) -> anyhow::Result<ankurah::EntityId> {
        let trx = ctx.begin();
        let id = trx
            .create(&Principal {
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
    let ctx = context().await?;
    let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    ops::join(&ctx, daniel).await?;
    let quiet = agent(&ctx, "impl-quiet").await?;
    let now = fwd_core::now_ms();
    assert!(!dormant(&ctx, quiet, now).await?, "naming itself is its first write");

    last_wrote(&ctx, quiet, now - 13 * HOUR).await?;
    assert!(dormant(&ctx, quiet, now).await?, "thirteen hours after its last write, it is dormant");
    last_wrote(&ctx, quiet, now - 11 * HOUR).await?;
    assert!(!dormant(&ctx, quiet, now).await?, "eleven hours after, it is not yet");
    last_wrote(&ctx, quiet, now - 13 * HOUR).await?;

    // Dormant decides nothing: it still belongs, its name still names it, and a question to it
    // still reaches it.
    let p = ctx.get::<PrincipalView>(quiet).await?;
    assert!(ops::belongs(&p), "a picker still offers it");
    assert_eq!(naming::principal_named(&ctx, "impl-quiet", None).await?.map(|p| p.id()), Some(quiet));
    let to = MessageContext { to: Some(quiet.to_base64()), ..Default::default() };
    let asked = ops::post_message(&ctx, daniel, "still there?", Priority::Next, to).await?;
    let plan = routing::plan_for_message(&ctx, &ctx.get::<MessageView>(asked).await?).await?;
    assert!(
        plan.iter().any(|p| p.recipient == quiet && p.notification_type == NotificationType::QuestionAsked),
        "a question addressed to a dormant agent reaches it"
    );

    // It writes, and each client stamps the write, which makes it active again.
    let reply = MessageContext { reply_to: Some(asked), ..Default::default() };
    ops::post_message(&ctx, quiet, "here", Priority::Next, reply).await?;
    dormancy::wrote(&ctx, quiet).await?;
    assert!(!dormant(&ctx, quiet, fwd_core::now_ms()).await?, "its next write brings it back");
    Ok(())
}

#[tokio::test]
async fn a_person_is_never_dormant_nor_a_retired_agent_and_an_agent_from_before_the_stamp_is() -> Result<()> {
    let ctx = context().await?;
    let later = fwd_core::now_ms() + 100 * HOUR;
    let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    assert!(!dormant(&ctx, daniel, later).await?, "a person no write has stamped");
    dormancy::wrote(&ctx, daniel).await?;
    assert!(!dormant(&ctx, daniel, later).await?, "a person quiet for a hundred hours");
    let old_person = before::made(&ctx, "old-person", PrincipalKind::Human).await?;
    assert!(!dormant(&ctx, old_person, later).await?, "a person from before the stamp");

    let old_agent = before::made(&ctx, "impl-old", PrincipalKind::Agent).await?;
    let p = ctx.get::<PrincipalView>(old_agent).await?;
    assert_eq!(p.last_active()?, None, "an agent from before the stamp records no write");
    assert!(dormancy::is_dormant(&p, fwd_core::now_ms()), "so it is dormant until its next write");
    dormancy::wrote(&ctx, old_agent).await?;
    assert!(!dormant(&ctx, old_agent, fwd_core::now_ms()).await?, "which brings it back");

    let finished = agent(&ctx, "impl-finished").await?;
    last_wrote(&ctx, finished, fwd_core::now_ms() - 13 * HOUR).await?;
    ops::retire_principal(&ctx, "impl-finished").await?;
    assert!(!dormant(&ctx, finished, fwd_core::now_ms()).await?, "retired, it is no picker's, dormant or not");
    Ok(())
}

#[tokio::test]
async fn an_agent_that_names_itself_as_a_successor_is_active_from_its_first_write() -> Result<()> {
    let ctx = context().await?;
    let old = agent(&ctx, "impl-first").await?;
    last_wrote(&ctx, old, fwd_core::now_ms() - 13 * HOUR).await?;
    let done = ops::add_successor(&ctx, "impl-second", None, None, None, None, None, "impl-first").await?;
    let now = fwd_core::now_ms();
    assert!(!dormant(&ctx, done.successor, now).await?, "the successor named itself just now");
    assert!(!dormant(&ctx, old, now).await?, "and the one it succeeded is retired, so no picker's");
    Ok(())
}

#[tokio::test]
async fn the_mention_search_offers_the_agents_at_work_before_the_dormant_ones() -> Result<()> {
    let ctx = context().await?;
    let quiet = agent(&ctx, "impl-a").await?;
    let busy = agent(&ctx, "impl-b").await?;
    let busier = agent(&ctx, "impl-c").await?;
    let now = fwd_core::now_ms();
    last_wrote(&ctx, quiet, now - 13 * HOUR).await?;
    let principals = ops::principals(&ctx).await?;
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
