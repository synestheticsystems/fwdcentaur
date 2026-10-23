//! Who a project's principals are: the roles each agent selects for itself from a fixed set, with
//! one triage per project and a lead per session that coordinates its work (one lead in a session:
//! tests/agents.rs); the triage agent, whom a question asked of no one and every
//! rule that reached the supervisors reach, one recipient; who belongs to the project, whom the
//! page's pickers offer; and `fwd admin migrate`, which carries a graph written before 2026-09-29
//! over from the supervisor role (docs/design/agents-and-inbox.md, "Roles" and "Who belongs to a
//! project"; docs/DECISIONS.md, D106, "Each project has its own triager user in place of the
//! supervisor role, and a project's pickers list only the principals that belong to it").

use ankurah::{policy::DEFAULT_CONTEXT, Context, EntityId, Node, PermissiveAgent};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::model::*;
use fwd_core::naming;
use fwd_core::ops::{self, CreateScope, CreateTask, MessageContext, RoleMigration};
use fwd_core::routing;
use fwd_core::rules::Verb;
use std::sync::Arc;

async fn context() -> Result<Context> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    Ok(node.context_async(DEFAULT_CONTEXT).await?)
}

async fn principal(ctx: &Context, id: EntityId) -> Result<PrincipalView> {
    Ok(ctx.get::<PrincipalView>(id).await?)
}

/// The principals that hold lead and are not retired, in id order.
async fn leads(ctx: &Context) -> Result<Vec<EntityId>> {
    let mut found: Vec<EntityId> =
        ops::principals(ctx).await?.iter().filter(|p| ops::holds(p, AgentRole::Lead)).map(|p| p.id()).collect();
    found.sort();
    Ok(found)
}

/// `ids` in id order, as [`leads`] gives them.
fn sorted<const N: usize>(mut ids: [EntityId; N]) -> Vec<EntityId> {
    ids.sort();
    ids.to_vec()
}

/// Who a plan notifies, and why, in recipient order.
fn told(planned: &[routing::Planned]) -> Vec<(EntityId, NotificationType, String)> {
    let mut out: Vec<_> = planned
        .iter()
        .map(|p| (p.recipient, p.notification_type, p.originator["why"].as_str().unwrap_or("").to_string()))
        .collect();
    out.sort();
    out
}

/// The principal as a build before 2026-09-29 wrote it: its role a word, `supervisor` among them,
/// which this build's `AgentRole` does not read, and nothing said about belonging. It is the same
/// collection as `fwd_core::model::Principal`, so what it writes is what an older graph holds.
mod before {
    use ankurah::Model;
    use serde::{Deserialize, Serialize};

    #[derive(Model, Debug, Clone, Serialize, Deserialize)]
    pub struct Principal {
        #[active_type(LWW)]
        pub kind: fwd_core::model::PrincipalKind,
        #[active_type(LWW)]
        pub handle: String,
        #[active_type(LWW)]
        pub display_name: String,
        #[active_type(LWW)]
        pub auth_subject: Option<String>,
        #[active_type(LWW)]
        pub agent_role: Option<String>,
        pub active: bool,
    }

    /// A principal as a build before 2026-09-29 made it: an agent with `role` as it stored it, or
    /// with no role a person.
    pub async fn made(ctx: &ankurah::Context, handle: &str, role: Option<&str>) -> anyhow::Result<ankurah::EntityId> {
        use fwd_core::model::PrincipalKind;
        let trx = ctx.begin();
        let id = trx
            .create(&Principal {
                kind: if role.is_some() { PrincipalKind::Agent } else { PrincipalKind::Human },
                handle: handle.into(),
                display_name: handle.into(),
                auth_subject: None,
                agent_role: role.map(str::to_string),
                active: true,
            })
            .await?
            .id();
        trx.commit().await?;
        Ok(id)
    }
}

/// The first eight characters of `id`, as the refusals name a scope.
fn short(id: EntityId) -> String {
    id.to_base64().chars().take(8).collect()
}

/// A scope named `title`, made by `by`.
async fn scope(ctx: &Context, by: EntityId, title: &str) -> Result<EntityId> {
    ops::create_scope(ctx, by, CreateScope { title: title.into(), ..Default::default() }).await
}

/// The triage agent that answers first for `scope`, or for what concerns no scope when it is none:
/// the scope's own, else the one that serves the whole project (`ops::front_line`).
async fn front(ctx: &Context, scope: Option<EntityId>) -> Result<Option<EntityId>> {
    Ok(ops::triage_for(ctx, scope).await?.map(|f| f.agent))
}

/// Each agent selects its role from the fixed set as it names itself, and may change it; a triage
/// agent serves a scope, or by a deliberate endorsement the whole project, and is refused where it
/// would serve neither, with the two ways out named; among the principals not retired, a scope has
/// one triage agent and a project one that serves it whole, each second refused, naming the one
/// that has it (docs/DECISIONS.md, D122); a project has a lead per session that coordinates its
/// work (docs/DECISIONS.md, D119), so a second lead, by `add` or by `role`, is another lead where
/// fwd cannot tell the two sessions apart, as here, where no agent carries a subject; implementers
/// and reviewers are as many as the work needs; a person takes no role; an agent that holds none is
/// told how to select one.
#[tokio::test]
async fn an_agent_selects_its_role_and_a_scope_has_one_triage_agent() -> Result<()> {
    let ctx = context().await?;
    let refused = ops::add_principal(&ctx, "impl-alice", None, None, None, None, None).await.unwrap_err().to_string();
    assert_eq!(
        refused,
        "an agent selects its role as it names itself: `fwd principal add impl-alice --role lead|triage|implementer|reviewer`"
    );
    let lead = ops::add_principal(&ctx, "claude", Some(AgentRole::Lead), None, None, None, None).await?;
    let alice = ops::add_principal(&ctx, "impl-alice", Some(AgentRole::Implementer), None, None, None, None).await?;
    let bob =
        ops::add_principal(&ctx, "@impl-bob", Some(AgentRole::Implementer), None, Some("Bob's agent"), None, None)
            .await?;
    let b = principal(&ctx, bob).await?;
    assert_eq!(
        (b.handle()?, b.display_name()?, ops::role_of(&b), ops::belongs(&b)),
        ("impl-bob".to_string(), "Bob's agent".to_string(), Some(AgentRole::Implementer), true),
        "an agent added belongs, with the role it selected"
    );
    assert_eq!(leads(&ctx).await?, vec![lead]);
    assert_eq!(front(&ctx, None).await?, None, "no agent holds triage yet");

    // A second lead is another lead.
    let lead_2 = ops::add_principal(&ctx, "lead-2", Some(AgentRole::Lead), None, None, None, None).await?;
    assert_eq!(leads(&ctx).await?, sorted([lead, lead_2]));
    // Triage that would serve nothing is refused, naming the two ways out, and nothing is made.
    let nothing = ops::add_principal(&ctx, "triage-x", Some(AgentRole::Triage), None, None, None, None)
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(nothing, ops::serves_nothing("triage-x"));
    assert!(
        nothing.contains("`--scope <id>`") && nothing.contains("`--project-wide`, which suits only a small project")
    );
    assert!(naming::principal_named(&ctx, "triage-x", None).await?.is_none());
    // A scope has one triage agent: a second for it is refused, naming the one that serves it and
    // how to free the scope; another scope takes another.
    let (replication, storage) = (scope(&ctx, lead, "Replication").await?, scope(&ctx, lead, "Storage").await?);
    let triage = ops::add_principal(
        &ctx,
        "triage-bot",
        Some(AgentRole::Triage),
        Some(ops::Serves::Found(replication)),
        None,
        None,
        None,
    )
    .await?;
    assert_eq!(front(&ctx, Some(replication)).await?, Some(triage));
    assert_eq!(front(&ctx, Some(storage)).await?, None, "a scope no agent serves has no front line");
    assert_eq!(front(&ctx, None).await?, None, "and a scope's agent answers for no other");
    let second = ops::add_principal(
        &ctx,
        "triage-2",
        Some(AgentRole::Triage),
        Some(ops::Serves::Scope(replication)),
        None,
        None,
        None,
    )
    .await
    .unwrap_err()
    .to_string();
    let r = short(replication);
    assert_eq!(
        second,
        format!(
            "the scope \"Replication\" ({r}) has a triage agent already, @triage-bot, and a scope has one: free the scope first (`fwd principal scope remove {r} --for triage-bot`), or retire @triage-bot (`fwd principal retire triage-bot`), for `@triage-2` to serve it"
        )
    );
    let changed = ops::set_role(&ctx, "impl-alice", AgentRole::Triage, Some(ops::Serves::Found(replication)))
        .await
        .unwrap_err()
        .to_string();
    assert!(changed.starts_with("the scope \"Replication\""), "{changed}");
    let triage_2 = ops::add_principal(
        &ctx,
        "triage-2",
        Some(AgentRole::Triage),
        Some(ops::Serves::Scope(storage)),
        None,
        None,
        None,
    )
    .await?;
    assert_eq!(front(&ctx, Some(storage)).await?, Some(triage_2));
    // A project has one triage agent that serves it whole, which answers first for what concerns
    // no scope and for the scopes no agent serves, but not for a scope that has its own.
    let whole = ops::add_principal(
        &ctx,
        "triage-all",
        Some(AgentRole::Triage),
        Some(ops::Serves::ProjectWide),
        None,
        None,
        None,
    )
    .await?;
    assert_eq!(front(&ctx, None).await?, Some(whole));
    let elsewhere = scope(&ctx, lead, "Docs").await?;
    assert_eq!(front(&ctx, Some(elsewhere)).await?, Some(whole));
    assert_eq!(front(&ctx, Some(replication)).await?, Some(triage));
    let again = ops::add_principal(
        &ctx,
        "triage-all-2",
        Some(AgentRole::Triage),
        Some(ops::Serves::ProjectWide),
        None,
        None,
        None,
    )
    .await
    .unwrap_err()
    .to_string();
    assert_eq!(
        again,
        "@triage-all serves the whole project already, and a project has one triage agent that serves it whole: withdraw its endorsement first (`fwd principal scope project-wide --withdraw --for triage-all`), or retire it (`fwd principal retire triage-all`), for `@triage-all-2` to serve the whole project"
    );
    // --project-wide goes with triage alone, and the scope a command finds is where an agent of
    // another role works, which it does not serve.
    let wrong = ops::add_principal(
        &ctx,
        "impl-c",
        Some(AgentRole::Implementer),
        Some(ops::Serves::ProjectWide),
        None,
        None,
        None,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(wrong.starts_with("only a triage agent serves the whole project"), "{wrong}");
    let carol = ops::add_principal(
        &ctx,
        "impl-c",
        Some(AgentRole::Implementer),
        Some(ops::Serves::Found(storage)),
        None,
        None,
        None,
    )
    .await?;
    assert_eq!(ops::Serving::of(&principal(&ctx, carol).await?), ops::Serving::default());
    // Reviewers and implementers are as many as the work needs.
    assert_eq!(ops::set_role(&ctx, "impl-alice", AgentRole::Reviewer, None).await?, alice);
    assert_eq!(ops::role_of(&principal(&ctx, alice).await?), Some(AgentRole::Reviewer));
    ops::add_principal(&ctx, "reviewer-2", Some(AgentRole::Reviewer), None, None, None, None).await?;
    // Selecting the role one already holds changes nothing; a third lead, by `role`, is another
    // lead; and a retired lead no longer counts.
    assert_eq!(ops::set_role(&ctx, "claude", AgentRole::Lead, None).await?, lead);
    assert_eq!(ops::set_role(&ctx, "impl-bob", AgentRole::Lead, None).await?, bob);
    assert_eq!(leads(&ctx).await?, sorted([lead, lead_2, bob]));
    ops::retire_principal(&ctx, "claude").await?;
    assert_eq!(leads(&ctx).await?, sorted([lead_2, bob]));

    // A name no principal has is refused, naming the command that adds it.
    let unknown = ops::set_role(&ctx, "nobody", AgentRole::Implementer, None).await.unwrap_err().to_string();
    assert_eq!(
        unknown,
        "no principal of this project is named `nobody`: an agent names itself and selects its role at once, `fwd principal add nobody --role implementer`"
    );
    // A person takes no role, even acting as themselves.
    let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let person = ops::set_role(&ctx, "dnorman", AgentRole::Lead, None).await.unwrap_err().to_string();
    assert_eq!(person, "`@dnorman` is a person, and a role is an agent's");
    let person = ops::add_principal(&ctx, "dnorman", Some(AgentRole::Lead), None, None, Some(daniel), None)
        .await
        .unwrap_err()
        .to_string();
    assert!(person.contains("is a person, and a role is an agent's"), "{person}");
    // An agent with no role, as one made before the roles were fixed, lacks one until it selects it.
    let old = ops::ensure_principal(&ctx, "impl-old", "impl-old", PrincipalKind::Agent, None).await?;
    assert!(ops::lacks_role(&principal(&ctx, old).await?)?);
    assert_eq!(ops::select_role_command("impl-old"), "`fwd principal role impl-old lead|triage|implementer|reviewer`");
    let bare = ops::add_principal(&ctx, "impl-old", None, None, None, Some(old), None).await.unwrap_err().to_string();
    assert!(bare.contains("holds no role, and every agent selects one"), "{bare}");
    ops::set_role(&ctx, "impl-old", AgentRole::Implementer, None).await?;
    assert!(!ops::lacks_role(&principal(&ctx, old).await?)?);
    assert!(!ops::lacks_role(
        &principal(&ctx, naming::principal_named(&ctx, "dnorman", None).await?.unwrap().id()).await?
    )?);
    Ok(())
}

/// A rule that reached the supervisors reaches one triage agent: should two principals serve the
/// whole project (two clients endorsing at once), the first by handle answers first for it, and the
/// other hears nothing for the role, of a question asked of no one, a comment on a task, a proposal
/// or a review.
#[tokio::test]
async fn a_rule_that_reached_the_supervisors_reaches_one_triage_agent() -> Result<()> {
    let ctx = context().await?;
    let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let alice = ops::add_principal(&ctx, "impl-alice", Some(AgentRole::Implementer), None, None, None, None).await?;
    let triage = ops::add_principal(
        &ctx,
        "triage-bot",
        Some(AgentRole::Triage),
        Some(ops::Serves::ProjectWide),
        None,
        None,
        None,
    )
    .await?;
    // A second principal serving the whole project, as a race could leave it, whose handle sorts
    // first.
    let first = ops::add_principal(&ctx, "a-triage", Some(AgentRole::Implementer), None, None, None, None).await?;
    let trx = ctx.begin();
    let m = principal(&ctx, first).await?.edit(&trx)?;
    m.agent_role()?.set(&Some(AgentRole::Triage))?;
    m.serves()?.set(&Some(serde_json::json!({"project_wide": true}).to_string()))?;
    trx.commit().await?;
    assert_eq!(front(&ctx, None).await?, Some(first), "the first by handle");
    let _ = triage;

    let task =
        ops::create_task(&ctx, daniel, CreateTask { title: "Retry".into(), needs_review: true, ..Default::default() })
            .await?;
    let asked = MessageContext { asks_project: true, task_id: Some(task), ..Default::default() };
    let question = ops::post_message(&ctx, daniel, "who takes it?", Priority::Now, asked).await?;
    let planned = routing::plan_for_message(&ctx, &ctx.get::<MessageView>(question).await?).await?;
    assert_eq!(told(&planned), vec![(first, NotificationType::QuestionAsked, "triage".into())]);
    let comment = ops::post_message(
        &ctx,
        daniel,
        "back off",
        Priority::Next,
        MessageContext { task_id: Some(task), ..Default::default() },
    )
    .await?;
    let planned = routing::plan_for_message(&ctx, &ctx.get::<MessageView>(comment).await?).await?;
    assert_eq!(told(&planned), vec![(first, NotificationType::TaskCommented, "triage".into())]);
    let proposed =
        ops::create_task(&ctx, alice, CreateTask { title: "Jitter".into(), propose: true, ..Default::default() })
            .await?;
    let planned = routing::plan_for_task_created(&ctx, &ctx.get::<TaskView>(proposed).await?).await?;
    assert_eq!(told(&planned), vec![(first, NotificationType::TaskProposed, "triage".into())]);
    let scope =
        ops::create_scope(&ctx, alice, CreateScope { title: "Retry work".into(), ..Default::default() }).await?;
    ops::claim(&ctx, alice, task, scope).await?;
    ops::transition(&ctx, alice, task, Verb::Submit, Some("tests green")).await?;
    let submitted: TaskView = ctx.get(task).await?;
    let planned = routing::plan_for_task_status(&ctx, &submitted, TaskStatus::InProgress, Some(alice)).await?;
    assert_eq!(told(&planned), vec![(first, NotificationType::ReviewRequested, "triage".into())]);
    Ok(())
}

/// A principal a read brings into the graph does not belong; a write makes it belong; retiring
/// hides it and keeps it, its later writes leave it retired, and adding it brings it back. The
/// project's people are those of its humans who belong.
#[tokio::test]
async fn a_principal_belongs_from_its_first_write_or_its_addition_until_it_is_retired() -> Result<()> {
    let ctx = context().await?;
    let reader = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let p = principal(&ctx, reader).await?;
    assert_eq!((p.member()?, ops::belongs(&p)), (Some(false), false), "a read's principal does not belong");
    assert!(ops::people(&ctx).await?.is_empty(), "nor is it one of the project's people");
    assert!(ops::join(&ctx, reader).await?, "its first write makes it belong");
    assert!(ops::belongs(&principal(&ctx, reader).await?));
    assert!(!ops::join(&ctx, reader).await?, "and its next changes nothing");
    assert_eq!(ops::people(&ctx).await?, vec![reader]);

    let agent = ops::add_principal(&ctx, "impl-alice", Some(AgentRole::Implementer), None, None, None, None).await?;
    let task = ops::create_task(&ctx, agent, CreateTask { title: "Mine".into(), ..Default::default() }).await?;
    assert_eq!(ops::retire_principal(&ctx, "@impl-alice").await?, agent);
    let retired = principal(&ctx, agent).await?;
    assert_eq!((retired.active()?, ops::belongs(&retired)), (false, false), "retired, it belongs no more");
    let made: TaskView = ctx.get(task).await?;
    assert_eq!(EntityId::from(made.created_by()?), agent, "and what it did is still attributed to it");
    assert_eq!(
        naming::principal_named(&ctx, "impl-alice", None).await?.map(|p| p.id()),
        Some(agent),
        "and it can be named"
    );
    assert!(!ops::join(&ctx, agent).await?, "a retired principal's later writes leave it retired");
    assert!(!ops::belongs(&principal(&ctx, agent).await?));
    assert_eq!(
        ops::add_principal(&ctx, "impl-alice", None, None, None, Some(agent), None).await?,
        agent,
        "principal add brings it back"
    );
    assert!(ops::belongs(&principal(&ctx, agent).await?));
    Ok(())
}

/// A graph written before 2026-09-29 is carried over once, translating the stored role word and
/// nothing else: of the principals that held `supervisor`, the one that wrote most recently holds
/// triage, under its own handle, serving nothing, so that the rules reach it only once its lead
/// binds it or endorses it (docs/DECISIONS.md, D122: the migration binds nothing); the others hold
/// no role, named for retirement; `implementer` stays; questions to the role are asked of the
/// project; people and the agents that did work belong, while an agent that only wrote messages
/// acted without belonging and one with nothing only read. A second run changes nothing.
#[tokio::test]
async fn a_graph_from_before_the_fixed_roles_is_carried_over_once() -> Result<()> {
    let ctx = context().await?;
    let daniel = before::made(&ctx, "dnorman", None).await?;
    let supervisor = before::made(&ctx, "supervisor", Some("supervisor")).await?;
    let replication = before::made(&ctx, "supervisor-replication", Some("supervisor")).await?;
    let worker = before::made(&ctx, "impl-worker", Some("implementer")).await?;
    let visitor = before::made(&ctx, "impl-visitor", Some("implementer")).await?;
    let reader = before::made(&ctx, "claude", Some("implementer")).await?;
    // Before the migration: the old role reads as none, and every principal belongs while active.
    let old = principal(&ctx, supervisor).await?;
    assert!(old.agent_role().is_err(), "this build does not read the role `supervisor`");
    assert_eq!(ops::json_of_principal(&old)?["agent_role"], serde_json::Value::Null, "and prints it as none");
    assert!(ops::lacks_role(&old)?, "so its writes are refused until the migration");
    assert!(ops::belongs(&old) && ops::belongs(&principal(&ctx, reader).await?));
    assert_eq!(front(&ctx, None).await?, None);

    // What each did, in this order: the old supervisor proposed a task; Ankurah's own triage agent
    // owns a scope, written later; the worker holds a task; the visitor wrote two messages, a
    // question to the role among them.
    let proposed =
        ops::create_task(&ctx, supervisor, CreateTask { title: "Groom".into(), propose: true, ..Default::default() })
            .await?;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    ops::create_scope(&ctx, replication, CreateScope { title: "Replication".into(), ..Default::default() }).await?;
    let held = ops::create_task(&ctx, daniel, CreateTask { title: "Replicate".into(), ..Default::default() }).await?;
    ops::assign(&ctx, daniel, held, Some(worker)).await?;
    let to_role = MessageContext { to: Some("supervisor".into()), task_id: Some(held), ..Default::default() };
    let question = ops::post_message(&ctx, visitor, "who takes this?", Priority::Now, to_role).await?;
    ops::post_message(&ctx, visitor, "and this?", Priority::Next, MessageContext::default()).await?;

    let mut done = ops::migrate_roles(&ctx).await?;
    done.members.sort();
    // The old supervisor did work here, so it belongs until a person retires it, as the migration
    // names it for.
    let mut members = vec![daniel, supervisor, replication, worker];
    members.sort();
    assert_eq!(
        done,
        RoleMigration {
            triage: Some(replication),
            roles_cleared: vec![supervisor],
            readdressed: vec![question],
            members,
            visitors: vec![(visitor, 2)],
            readers: vec![reader],
        }
    );
    let t = principal(&ctx, replication).await?;
    assert_eq!(
        (t.handle()?, ops::role_of(&t), ops::belongs(&t)),
        ("supervisor-replication".to_string(), Some(AgentRole::Triage), true),
        "the same principal, under its own handle"
    );
    assert!(ops::serves_nothing_yet(&t), "serving nothing");
    assert_eq!(front(&ctx, None).await?, None, "so it answers first for nothing");
    let cleared = principal(&ctx, supervisor).await?;
    assert_eq!(
        (cleared.handle()?, ops::role_of(&cleared)),
        ("supervisor".to_string(), None),
        "no role, for retirement"
    );
    assert_eq!(ops::role_of(&principal(&ctx, worker).await?), Some(AgentRole::Implementer), "implementer stays");
    let asked: MessageView = ctx.get(question).await?;
    assert_eq!(
        serde_json::Value::from(asked.context()?),
        serde_json::json!({"asks": "project"}),
        "asked of the project"
    );
    for (who, belongs) in
        [(daniel, true), (supervisor, true), (replication, true), (worker, true), (visitor, false), (reader, false)]
    {
        assert_eq!(ops::belongs(&principal(&ctx, who).await?), belongs, "{who}");
    }
    // The rules reach the principal given triage once its lead endorses it, and not before.
    let comment = ops::post_message(
        &ctx,
        daniel,
        "replicate the notifications too",
        Priority::Next,
        MessageContext { task_id: Some(proposed), ..Default::default() },
    )
    .await?;
    let planned = routing::plan_for_message(&ctx, &ctx.get::<MessageView>(comment).await?).await?;
    assert_eq!(told(&planned), vec![], "serving nothing, it hears nothing for its role");
    ops::serve(&ctx, "supervisor-replication", ops::Serves::ProjectWide).await?;
    let planned = routing::plan_for_message(&ctx, &ctx.get::<MessageView>(comment).await?).await?;
    assert_eq!(told(&planned), vec![(replication, NotificationType::TaskCommented, "triage".into())]);

    let again = ops::migrate_roles(&ctx).await?;
    assert_eq!(again, RoleMigration::default(), "nothing twice");
    Ok(())
}

/// Where an agent holds triage already, every principal that held `supervisor` holds no role; where
/// none held it, no role is given.
#[tokio::test]
async fn the_migration_gives_triage_only_where_no_agent_holds_it_and_makes_no_principal() -> Result<()> {
    let ctx = context().await?;
    before::made(&ctx, "dnorman", None).await?;
    let old = before::made(&ctx, "supervisor", Some("supervisor")).await?;
    let triage = ops::add_principal(
        &ctx,
        "triage-bot",
        Some(AgentRole::Triage),
        Some(ops::Serves::ProjectWide),
        None,
        None,
        None,
    )
    .await?;
    let done = ops::migrate_roles(&ctx).await?;
    assert_eq!((done.triage, done.roles_cleared), (None, vec![old]));
    assert_eq!(front(&ctx, None).await?, Some(triage));

    let ctx = context().await?;
    before::made(&ctx, "dnorman", None).await?;
    let done = ops::migrate_roles(&ctx).await?;
    assert_eq!(done.triage, None, "no principal held the role, and none is made");
    assert_eq!(ops::principals(&ctx).await?.len(), 1);
    Ok(())
}

/// A retired triage agent comes back serving what it served only while no other principal that
/// is not retired serves it, since what it serves counts again once it is back: while another
/// serves its scope, bringing it back is refused, naming that principal, and it stays retired; it
/// may come back with another role, serving nothing, and once no one serves its scope, serving it.
/// A retired lead comes back holding lead beside the project's other leads, one per session that
/// coordinates its work (docs/DECISIONS.md, D119; within one session, tests/agents.rs).
#[tokio::test]
async fn a_retired_triage_agent_comes_back_to_a_scope_no_other_serves_and_a_lead_beside_the_others() -> Result<()> {
    let ctx = context().await?;
    let claude = ops::add_principal(&ctx, "claude", Some(AgentRole::Lead), None, None, None, None).await?;
    ops::retire_principal(&ctx, "claude").await?;
    let lead_x = ops::add_principal(&ctx, "lead-x", Some(AgentRole::Lead), None, None, None, None).await?;
    ops::add_principal(&ctx, "claude", None, None, None, Some(claude), None).await?;
    assert_eq!(leads(&ctx).await?, sorted([claude, lead_x]), "two leads, the one brought back among them");
    // A retired triage agent is refused its scope while another serves it, and stays retired.
    let retry = scope(&ctx, claude, "Retry work").await?;
    let serves_retry = Some(ops::Serves::Scope(retry));
    let triage = ops::add_principal(&ctx, "triage-1", Some(AgentRole::Triage), serves_retry, None, None, None).await?;
    ops::retire_principal(&ctx, "triage-1").await?;
    assert_eq!(front(&ctx, Some(retry)).await?, None, "retired, it answers for its scope no more");
    let triage_x =
        ops::add_principal(&ctx, "triage-x", Some(AgentRole::Triage), serves_retry, None, None, None).await?;
    let back =
        ops::add_principal(&ctx, "triage-1", None, None, None, Some(triage), None).await.unwrap_err().to_string();
    let r = short(retry);
    assert_eq!(
        back,
        format!(
            "the scope \"Retry work\" ({r}) has a triage agent already, @triage-x, and a scope has one: free the scope first (`fwd principal scope remove {r} --for triage-x`), or retire @triage-x (`fwd principal retire triage-x`), for `@triage-1` to serve it"
        )
    );
    assert!(!principal(&ctx, triage).await?.active()?, "it stays retired");
    assert_eq!(
        front(&ctx, Some(retry)).await?,
        Some(triage_x),
        "one triage agent for the scope, not the one brought back"
    );
    // With another role it comes back, serving nothing.
    ops::add_principal(&ctx, "triage-1", Some(AgentRole::Reviewer), None, None, Some(triage), None).await?;
    let p = principal(&ctx, triage).await?;
    assert!(p.active()? && ops::role_of(&p) == Some(AgentRole::Reviewer));
    assert_eq!(ops::Serving::of(&p), ops::Serving::default(), "another role frees what it served");
    // A retired triage agent whose scope no one serves since comes back serving it.
    ops::retire_principal(&ctx, "triage-x").await?;
    assert_eq!(front(&ctx, Some(retry)).await?, None);
    ops::add_principal(&ctx, "triage-x", None, None, None, Some(triage_x), None).await?;
    assert_eq!(front(&ctx, Some(retry)).await?, Some(triage_x));
    Ok(())
}

/// What a triage agent serves changes with `fwd principal scope` (`ops::serve`, `ops::stop_serving`):
/// a lead binds its triage agent to more scopes and frees them, and endorses the one agent that
/// serves a small project whole, or withdraws the endorsement; a triage agent serving nothing, as
/// the migration leaves those from before 2026-09-30, takes a scope so. Refused for an agent that
/// holds no triage, naming the command that selects it, for a scope another serves, naming it, and
/// for what it does not serve, saying what it serves.
#[tokio::test]
async fn a_lead_binds_and_frees_its_triage_agents_scopes_and_endorses_one_for_the_whole_project() -> Result<()> {
    let ctx = context().await?;
    let lead = ops::add_principal(&ctx, "lead-a", Some(AgentRole::Lead), None, None, None, None).await?;
    let (replication, storage) = (scope(&ctx, lead, "Replication").await?, scope(&ctx, lead, "Storage").await?);
    let bot = ops::add_principal(
        &ctx,
        "triage-a",
        Some(AgentRole::Triage),
        Some(ops::Serves::Scope(replication)),
        None,
        None,
        None,
    )
    .await?;
    // It takes a second scope, and answers first for both.
    ops::serve(&ctx, "triage-a", ops::Serves::Scope(storage)).await?;
    assert_eq!(ops::Serving::of(&principal(&ctx, bot).await?).scopes, vec![replication, storage]);
    assert_eq!((front(&ctx, Some(replication)).await?, front(&ctx, Some(storage)).await?), (Some(bot), Some(bot)));
    // Taking one it serves already changes nothing.
    ops::serve(&ctx, "triage-a", ops::Serves::Scope(storage)).await?;
    assert_eq!(ops::Serving::of(&principal(&ctx, bot).await?).scopes, vec![replication, storage]);
    // Another triage agent is refused a scope it serves, naming it.
    let other =
        ops::add_principal(&ctx, "triage-b", Some(AgentRole::Triage), Some(ops::Serves::ProjectWide), None, None, None)
            .await?;
    let taken = ops::serve(&ctx, "triage-b", ops::Serves::Scope(storage)).await.unwrap_err().to_string();
    assert!(taken.starts_with("the scope \"Storage\"") && taken.contains("@triage-a"), "{taken}");
    // Freed, the scope's questions reach the agent that serves the whole project.
    ops::stop_serving(&ctx, "triage-a", ops::Serves::Scope(storage)).await?;
    assert_eq!(front(&ctx, Some(storage)).await?, Some(other));
    let not_served = ops::stop_serving(&ctx, "triage-a", ops::Serves::Scope(storage)).await.unwrap_err().to_string();
    assert_eq!(
        not_served,
        format!(
            "`@triage-a` does not serve the scope \"Storage\" ({}): it serves the scope \"Replication\" ({})",
            short(storage),
            short(replication)
        )
    );
    // The endorsement is withdrawn, and what concerns no scope has no front line then.
    ops::stop_serving(&ctx, "triage-b", ops::Serves::ProjectWide).await?;
    assert_eq!(front(&ctx, None).await?, None);
    assert!(ops::serves_nothing_yet(&principal(&ctx, other).await?), "left serving nothing, it holds triage still");
    let withdrawn = ops::stop_serving(&ctx, "triage-b", ops::Serves::ProjectWide).await.unwrap_err().to_string();
    assert_eq!(withdrawn, "`@triage-b` does not serve the whole project: it serves nothing");
    // A triage agent serving nothing is endorsed, and a second endorsement is refused.
    ops::serve(&ctx, "triage-b", ops::Serves::ProjectWide).await?;
    assert_eq!(front(&ctx, None).await?, Some(other));
    let second = ops::serve(&ctx, "triage-a", ops::Serves::ProjectWide).await.unwrap_err().to_string();
    assert!(second.starts_with("@triage-b serves the whole project already"), "{second}");
    // Only a triage agent serves anything.
    let implementer = ops::add_principal(&ctx, "impl-a", Some(AgentRole::Implementer), None, None, None, None).await?;
    let _ = implementer;
    let refused = ops::serve(&ctx, "impl-a", ops::Serves::Scope(storage)).await.unwrap_err().to_string();
    assert_eq!(
        refused,
        format!(
            "`@impl-a` holds implementer, and only a triage agent serves a scope or the whole project: an agent selects triage with what it serves, `fwd principal role impl-a triage --scope {}`",
            short(storage)
        )
    );
    Ok(())
}

/// The scope a command finds (its checkout's, or FWD_SCOPE's) is what an agent taking triage serves
/// only where it would serve nothing else, so that an agent that serves scopes, coming back or
/// changing its display name in another checkout, takes nothing it did not ask for, while a scope
/// the command names is taken; a triage agent that serves nothing, as the migration leaves those
/// from before 2026-09-30, takes the scope its command finds, and without one stays as it is.
#[tokio::test]
async fn the_scope_a_checkout_finds_is_taken_only_by_a_triage_agent_that_would_serve_nothing() -> Result<()> {
    let ctx = context().await?;
    let lead = ops::add_principal(&ctx, "lead-a", Some(AgentRole::Lead), None, None, None, None).await?;
    let (replication, storage) = (scope(&ctx, lead, "Replication").await?, scope(&ctx, lead, "Storage").await?);
    let bot = ops::add_principal(
        &ctx,
        "triage-a",
        Some(AgentRole::Triage),
        Some(ops::Serves::Found(replication)),
        None,
        None,
        None,
    )
    .await?;
    // Named again from a checkout of another scope, it keeps what it serves.
    ops::add_principal(&ctx, "triage-a", None, Some(ops::Serves::Found(storage)), Some("Triage A"), Some(bot), None)
        .await?;
    assert_eq!(ops::Serving::of(&principal(&ctx, bot).await?).scopes, vec![replication]);
    // A scope named on the command is taken.
    ops::add_principal(&ctx, "triage-a", None, Some(ops::Serves::Scope(storage)), None, Some(bot), None).await?;
    assert_eq!(ops::Serving::of(&principal(&ctx, bot).await?).scopes, vec![replication, storage]);
    // A triage agent serving nothing, as the migration leaves one, takes the scope its checkout
    // finds as it names itself again, and not without one.
    let old =
        ops::ensure_principal(&ctx, "triage-old", "triage-old", PrincipalKind::Agent, Some(AgentRole::Triage)).await?;
    ops::add_principal(&ctx, "triage-old", None, None, None, Some(old), None).await?;
    assert!(ops::serves_nothing_yet(&principal(&ctx, old).await?), "no scope, and it stays serving nothing");
    let docs = scope(&ctx, lead, "Docs").await?;
    ops::add_principal(&ctx, "triage-old", None, Some(ops::Serves::Found(docs)), None, Some(old), None).await?;
    assert_eq!(front(&ctx, Some(docs)).await?, Some(old));
    Ok(())
}
