//! Each agent a user of its own (docs/design/agents-and-inbox.md, "An agent's identity";
//! docs/DECISIONS.md, D112 and D118): a command the fwd plugin stamped names the Claude Code agent
//! that runs it, its subject, and acts as the principal bound to it; an agent binds its principal
//! as it names itself, or takes over one from before the plugin, once; a typed `--as` names that
//! principal or is refused; and an agent that has not named itself is told how to.

use ankurah::{policy::DEFAULT_CONTEXT, Context, EntityId, Node, PermissiveAgent};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::model::*;
use fwd_core::ops::{self, CreateScope, CreateTask, MessageContext};
use fwd_core::routing;
use fwd_core::subject;
use std::sync::Arc;

async fn context() -> Result<Context> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    Ok(node.context_async(DEFAULT_CONTEXT).await?)
}

/// The subject of `agent` in the session `s1`, as the plugin's stamp names it.
fn agent(agent: &str) -> String {
    subject::claude_code("s1", agent).expect("a subject")
}

/// What an operation refused with, as its sentence.
fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
    format!("{:#}", result.expect_err("refused"))
}

async fn subject_of(ctx: &Context, principal: EntityId) -> Result<Option<String>> {
    Ok(ctx.get::<PrincipalView>(principal).await?.subject()?)
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

/// The triage agent that answers first for `scope`, or for what concerns no scope when it is none
/// (`ops::front_line`).
async fn front(ctx: &Context, scope: Option<EntityId>) -> Result<Option<EntityId>> {
    Ok(ops::triage_for(ctx, scope).await?.map(|f| f.agent))
}

/// What `principal` serves as a triage agent.
async fn serving(ctx: &Context, principal: EntityId) -> Result<ops::Serving> {
    Ok(ops::Serving::of(&ctx.get::<PrincipalView>(principal).await?))
}

/// Naming itself binds the agent: the principal `fwd principal add` makes carries the command's
/// subject, and every later command with that subject acts as it.
#[tokio::test]
async fn an_agent_that_names_itself_is_bound_and_acts_as_itself_from_then_on() -> Result<()> {
    let ctx = context().await?;
    let a1 = agent("a1");
    assert_eq!(subject::caller(&ctx, &a1, None).await?, None, "until it names itself, it acts as no principal");
    let alice =
        ops::add_principal(&ctx, "impl-alice", Some(AgentRole::Implementer), None, None, None, Some(&a1)).await?;
    assert_eq!(subject_of(&ctx, alice).await?, Some(a1.clone()));
    assert_eq!(subject::caller(&ctx, &a1, None).await?, Some(alice));
    assert_eq!(ops::json_of_principal(&ctx.get(alice).await?)?["subject"], a1.as_str(), "the JSON says so");
    // Without a subject, `add` binds nothing, as before the plugin.
    let bob = ops::add_principal(&ctx, "impl-bob", Some(AgentRole::Implementer), None, None, None, None).await?;
    assert_eq!(subject_of(&ctx, bob).await?, None);
    // A subject binds one principal: the bound agent is refused a second name, and told how to
    // change its role or hand over its job instead.
    let second = refusal(
        ops::add_principal(&ctx, "impl-alice-2", Some(AgentRole::Reviewer), None, None, Some(alice), Some(&a1)).await,
    );
    assert!(
        second
            .contains("which is @impl-alice already, and a subject binds one principal, so an agent names itself once")
            && second.contains("`fwd principal role impl-alice <role>`")
            && second.contains("`fwd principal succeed <its name>`"),
        "{second}"
    );
    assert!(ops::principals(&ctx).await?.iter().all(|p| p.handle().unwrap() != "impl-alice-2"), "none was made");
    // Adding itself again, run as itself, selects another role and binds nothing new.
    assert_eq!(
        ops::add_principal(&ctx, "impl-alice", Some(AgentRole::Reviewer), None, None, Some(alice), Some(&a1)).await?,
        alice
    );
    assert_eq!(ctx.get::<PrincipalView>(alice).await?.agent_role()?, Some(AgentRole::Reviewer));
    Ok(())
}

/// Where a command carries a subject, the subject names the agent: a typed `--as` that names the
/// bound principal is let be, and one that names another, or any while the agent has not named
/// itself, is refused, since an agent acts only as itself.
#[tokio::test]
async fn the_subject_names_the_agent_and_a_typed_as_for_another_is_refused() -> Result<()> {
    let ctx = context().await?;
    ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let a1 = agent("a1");
    let alice =
        ops::add_principal(&ctx, "impl-alice", Some(AgentRole::Implementer), None, None, None, Some(&a1)).await?;
    assert_eq!(subject::caller(&ctx, &a1, Some("impl-alice")).await?, Some(alice), "its own name");
    assert_eq!(subject::caller(&ctx, &a1, Some(&alice.to_base64())).await?, Some(alice), "or its id");
    let other = refusal(subject::caller(&ctx, &a1, Some("dnorman")).await);
    assert_eq!(
        other,
        "`--as dnorman` asks to act as another principal, but this command runs as the Claude Code agent claude-code:s1/a1, whose principal is @impl-alice, and an agent acts only as itself: leave `--as` off"
    );
    let none = refusal(subject::caller(&ctx, &a1, Some("nobody")).await);
    assert!(none.starts_with("`--as nobody` asks to act as another principal"), "{none}");
    // An agent that has not named itself is refused any `--as`, and told how to name itself or take
    // over a principal from before the plugin.
    let a2 = agent("a2");
    let unnamed = refusal(subject::caller(&ctx, &a2, Some("impl-alice")).await);
    assert!(
        unnamed.starts_with("this command runs as the Claude Code agent claude-code:s1/a2, which has not named itself in this project, and an agent acts only as itself, so `--as impl-alice` is refused")
            && unnamed.contains("`fwd principal add <name> --role lead|triage|implementer|reviewer`")
            && unnamed.contains("`fwd --as <its name> principal bind <its name>`"),
        "{unnamed}"
    );
    Ok(())
}

/// A principal from before the plugin is taken over once and deliberately: `bind` sets the subject
/// only when `--as` names the same principal, and never for a person, a principal another agent is
/// bound to, or a subject bound already.
#[tokio::test]
async fn bind_takes_over_a_principal_from_before_the_plugin_once_and_on_purpose() -> Result<()> {
    let ctx = context().await?;
    let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let claude = ops::add_principal(&ctx, "claude", Some(AgentRole::Lead), None, None, None, None).await?;
    let main = agent("main");
    // Said once is not enough: `--as` must name it too.
    let once = refusal(ops::bind_principal(&ctx, "claude", &main, None).await);
    assert_eq!(
        once,
        "binding makes this Claude Code agent act as @claude from now on, so it is said twice: `fwd --as claude principal bind claude`"
    );
    let elsewhere = refusal(ops::bind_principal(&ctx, "claude", &main, Some(daniel)).await);
    assert_eq!(elsewhere, once, "`--as` naming someone else is no better");
    assert_eq!(subject_of(&ctx, claude).await?, None, "nothing was bound");
    assert_eq!(ops::bind_principal(&ctx, "claude", &main, Some(claude)).await?, claude);
    assert_eq!(subject::caller(&ctx, &main, None).await?, Some(claude), "its commands act as claude from now on");
    // Once: the subject is bound, and so is the principal.
    let again = refusal(ops::bind_principal(&ctx, "claude", &main, Some(claude)).await);
    assert_eq!(again, "this command runs as the Claude Code agent claude-code:s1/main, which is @claude already, and a subject binds one principal");
    let taken = refusal(ops::bind_principal(&ctx, "claude", &agent("a9"), Some(claude)).await);
    assert!(
        taken.starts_with("`@claude` is bound to another Claude Code agent (claude-code:s1/main)")
            && taken.contains("`fwd principal add <name> --succeeds claude`"),
        "{taken}"
    );
    // A person is never bound; a name no principal has is no one to take over.
    let person = refusal(ops::bind_principal(&ctx, "dnorman", &agent("a3"), Some(daniel)).await);
    assert!(person.starts_with("`@dnorman` is a person, and a person acts from their own terminal"), "{person}");
    let nobody = refusal(ops::bind_principal(&ctx, "ghost", &agent("a3"), None).await);
    assert!(
        nobody.starts_with("no principal of this project is named `ghost`, so there is none to take over"),
        "{nobody}"
    );
    // A name another principal has, given to `add` by an agent that has not named itself, says it
    // may be taken over, when it is an agent no agent is bound to.
    let lead2 = ops::add_principal(&ctx, "pilot-lead", Some(AgentRole::Implementer), None, None, None, None).await?;
    let taken_name = refusal(
        ops::add_principal(&ctx, "pilot-lead", Some(AgentRole::Implementer), None, None, None, Some(&agent("a4")))
            .await,
    );
    assert!(
        taken_name.starts_with("the name `pilot-lead` is taken")
            && taken_name.ends_with("if it is yours from before the fwd plugin, take it over, once: `fwd --as pilot-lead principal bind pilot-lead`"),
        "{taken_name}"
    );
    assert_eq!(subject_of(&ctx, lead2).await?, None);
    Ok(())
}

/// The principals bound to a session's agents are found by the session, which is how a command that
/// carries no stamp is known to come from a session whose agents act as themselves.
#[tokio::test]
async fn a_sessions_bound_agents_are_found_by_the_session() -> Result<()> {
    let ctx = context().await?;
    let lead =
        ops::add_principal(&ctx, "claude", Some(AgentRole::Lead), None, None, None, Some(&agent("main"))).await?;
    let sub =
        ops::add_principal(&ctx, "impl-a", Some(AgentRole::Implementer), None, None, None, Some(&agent("a1"))).await?;
    ops::add_principal(&ctx, "elsewhere", Some(AgentRole::Implementer), None, None, None, Some("claude-code:s2/main"))
        .await?;
    let principals = ops::principals(&ctx).await?;
    let mut found: Vec<EntityId> = subject::of_session(&principals, "s1").iter().map(|p| p.id()).collect();
    found.sort();
    let mut expected = vec![lead, sub];
    expected.sort();
    assert_eq!(found, expected);
    assert!(subject::of_session(&principals, "s3").is_empty());
    let refusal = subject::unstamped("s1", "claude");
    assert!(
        refusal.starts_with("this command runs in the Claude Code session s1, whose agents act as themselves (@claude is bound to one of them), but it carries no agent's stamp")
            && refusal.contains("run fwd on its own"),
        "{refusal}"
    );
    let unnamed = subject::unnamed(&agent("a7"));
    assert!(
        unnamed.starts_with("this command runs as the Claude Code agent claude-code:s1/a7, which has not named itself in this project, and an agent writes, and reads its own inbox and its own tasks, only as itself")
            && unnamed.contains("`fwd principal add <name> --role lead|triage|implementer|reviewer`"),
        "{unnamed}"
    );
    Ok(())
}

/// Two principals bound to one subject (two `add`s of one agent at once): the one that is not
/// retired acts; two that are not are refused, listed by id, never guessed.
#[tokio::test]
async fn a_subject_two_principals_carry_is_the_active_ones_or_refused() -> Result<()> {
    let ctx = context().await?;
    let a1 = agent("a1");
    let first = ops::add_principal(&ctx, "impl-one", Some(AgentRole::Implementer), None, None, None, Some(&a1)).await?;
    // The race: a second made as though the first were not there yet.
    let trx = ctx.begin();
    let second = trx
        .create(&Principal {
            kind: PrincipalKind::Agent,
            handle: "impl-two".into(),
            display_name: "impl-two".into(),
            auth_subject: None,
            agent_role: Some(AgentRole::Implementer),
            active: true,
            member: Some(true),
            subject: Some(a1.clone()),
            succeeded_by: None,
            serves: None,
            last_active: Some(fwd_core::now_ms()),
        })
        .await?
        .id();
    trx.commit().await?;
    let both = refusal(subject::caller(&ctx, &a1, None).await);
    assert!(
        both.starts_with("2 principals of this project are bound to the Claude Code agent claude-code:s1/a1")
            && both.contains(&first.to_base64()[..8])
            && both.contains(&second.to_base64()[..8]),
        "{both}"
    );
    ops::retire_principal(&ctx, &second.to_base64()).await?;
    assert_eq!(subject::caller(&ctx, &a1, None).await?, Some(first));
    Ok(())
}

/// A relaunched agent succeeds its predecessor (D114): the old principal is retired with its
/// successor recorded, its role moves to the successor, so that the one lead of the successor's
/// session is the successor, and its unread notifications move to the successor's inbox, each
/// saying whom it was for; brought back, the old principal selects a role again and hears what
/// reaches it.
#[tokio::test]
async fn a_successor_takes_the_old_principals_role_and_its_unread_notifications() -> Result<()> {
    let ctx = context().await?;
    let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let old = ops::add_principal(&ctx, "lead-1", Some(AgentRole::Lead), None, None, None, Some(&agent("main"))).await?;
    let waiting = ops::create_notification(
        &ctx,
        old,
        NotificationType::QuestionAsked,
        Priority::Now,
        serde_json::json!({"type": "Message", "id": old.to_base64()}),
        serde_json::json!({"actor_id": daniel.to_base64(), "why": "addressee"}),
    )
    .await?;
    let read = ops::create_notification(
        &ctx,
        old,
        NotificationType::Replied,
        Priority::Next,
        serde_json::json!({"type": "Message", "id": old.to_base64()}),
        serde_json::json!({"actor_id": daniel.to_base64(), "why": "parent_author"}),
    )
    .await?;
    ops::set_notification_status(&ctx, &[read], NotificationStatus::Read).await?;
    // The relaunched lead, in a session of its own, names itself with another role and succeeds
    // lead-1 (a lead of another session, it could also have selected lead beside lead-1: D119).
    let s2 = "claude-code:s2/main";
    let new = ops::add_principal(&ctx, "lead-2", Some(AgentRole::Reviewer), None, None, None, Some(s2)).await?;
    let done = ops::succeed_principal(&ctx, "lead-1", new).await?;
    assert_eq!(done, ops::Succession { successor: new, old, role: Some(AgentRole::Lead), moved: 1 });
    let old_view: PrincipalView = ctx.get(old).await?;
    assert!(!old_view.active()?, "the old principal is retired");
    assert_eq!(old_view.succeeded_by()?.map(EntityId::from), Some(new));
    assert_eq!(old_view.agent_role()?, None, "its role moved");
    assert_eq!(ctx.get::<PrincipalView>(new).await?.agent_role()?, Some(AgentRole::Lead), "to the successor");
    // The successor is its session's one lead: another agent of that session is refused lead.
    let third = refusal(
        ops::add_principal(&ctx, "lead-3", Some(AgentRole::Lead), None, None, None, Some("claude-code:s2/a1")).await,
    );
    assert!(third.starts_with("the Claude Code session s2 has a lead already, @lead-2"), "{third}");
    // The unread notification moved, saying whom it was for; the one read stays where it was.
    let moved: NotificationView = ctx.get(waiting).await?;
    assert_eq!(EntityId::from(moved.recipient()?), new);
    let originator: serde_json::Value = moved.originator()?.into();
    assert_eq!(originator["for_id"], old.to_base64());
    assert_eq!(EntityId::from(ctx.get::<NotificationView>(read).await?.recipient()?), old);
    // The one that stayed is the old principal's: its successor is refused it as anyone else is
    // (task AfhuiHYS), and its own, the one that moved, it dismisses.
    let theirs = refusal(ops::mark_notifications(&ctx, new, &[read], NotificationStatus::Dismissed).await);
    assert!(
        theirs.starts_with(&format!("notification {} is @lead-1's, not @lead-2's", &read.to_base64()[..8])),
        "{theirs}"
    );
    ops::mark_notifications(&ctx, new, &[waiting], NotificationStatus::Dismissed).await?;
    // Brought back, run as itself, it selects a role again, and its successor no longer takes
    // what reaches it.
    let bare = refusal(ops::add_principal(&ctx, "lead-1", None, None, None, Some(old), Some(&agent("main"))).await);
    assert!(bare.contains("holds no role, and every agent selects one"), "{bare}");
    ops::add_principal(&ctx, "lead-1", Some(AgentRole::Reviewer), None, None, Some(old), Some(&agent("main"))).await?;
    let back: PrincipalView = ctx.get(old).await?;
    assert!(back.active()? && back.succeeded_by()?.is_none());
    Ok(())
}

/// Routing delivers to the successor whatever it would deliver to the principal it succeeded: a
/// question asked of it, a mention of it, a reply in its thread, a comment on a task assigned to it,
/// a task assigned to it; each notification says whom it was for, and the inbox's sentence says so.
/// A successor succeeded in turn passes it on, and never hears of what it did itself.
#[tokio::test]
async fn routing_delivers_what_reaches_a_succeeded_principal_to_its_successor() -> Result<()> {
    let ctx = context().await?;
    let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let old = ops::add_principal(
        &ctx,
        "triage-1",
        Some(AgentRole::Triage),
        Some(ops::Serves::ProjectWide),
        None,
        None,
        Some(&agent("t1")),
    )
    .await?;
    let task = ops::create_task(&ctx, daniel, CreateTask { title: "Retry on disconnect".into(), ..Default::default() })
        .await?;
    ops::assign(&ctx, daniel, task, Some(old)).await?;
    let on_task = || MessageContext { task_id: Some(task), ..Default::default() };
    let thread = ops::post_message(&ctx, old, "I took it", Priority::Next, on_task()).await?;
    let new =
        ops::add_principal(&ctx, "triage-2", Some(AgentRole::Reviewer), None, None, None, Some(&agent("t2"))).await?;
    ops::succeed_principal(&ctx, "triage-1", new).await?;
    assert_eq!(front(&ctx, None).await?, Some(new), "the successor serves the whole project in its place");

    let heard = |planned: &[routing::Planned]| -> Vec<(EntityId, NotificationType, String)> {
        let mut out: Vec<_> = planned
            .iter()
            .map(|p| (p.recipient, p.notification_type, p.originator["for_id"].as_str().unwrap_or("").to_string()))
            .collect();
        out.sort();
        out
    };
    let for_old = old.to_base64();
    // A question asked of the old principal, by its id and by its name.
    for to in [old.to_base64(), "triage-1".to_string()] {
        let mc = MessageContext { to: Some(to), ..Default::default() };
        let asked = ops::post_message(&ctx, daniel, "still on it?", Priority::Now, mc).await?;
        let planned = routing::plan_for_message(&ctx, &ctx.get::<MessageView>(asked).await?).await?;
        assert_eq!(heard(&planned), vec![(new, NotificationType::QuestionAsked, for_old.clone())]);
    }
    // A mention of it.
    let mention =
        ops::post_message(&ctx, daniel, "@triage-1 have a look", Priority::Next, MessageContext::default()).await?;
    let planned = routing::plan_for_message(&ctx, &ctx.get::<MessageView>(mention).await?).await?;
    assert_eq!(heard(&planned), vec![(new, NotificationType::Mentioned, for_old.clone())]);
    // A reply in its thread on its task: one notification, the reply's, as for the old principal.
    let mc = MessageContext { reply_to: Some(thread), ..on_task() };
    let reply = ops::post_message(&ctx, daniel, "thanks", Priority::Next, mc).await?;
    let planned = routing::plan_for_message(&ctx, &ctx.get::<MessageView>(reply).await?).await?;
    assert_eq!(heard(&planned), vec![(new, NotificationType::Replied, for_old.clone())]);
    // A comment on the task assigned to it.
    let comment = ops::post_message(&ctx, daniel, "back off", Priority::Next, on_task()).await?;
    let planned = routing::plan_for_message(&ctx, &ctx.get::<MessageView>(comment).await?).await?;
    assert_eq!(heard(&planned), vec![(new, NotificationType::TaskCommented, for_old.clone())]);
    // The inbox's sentence says whom it was for, then how to answer the comment.
    let made = routing::apply(&ctx, &planned).await?;
    let n: NotificationView = ctx.get(made[0]).await?;
    let said = fwd_core::reason::plain(&fwd_core::reason::gather(&ctx, &n, Some(new)).await?);
    let why = "dnorman commented on Retry on disconnect, which is assigned to you (for @triage-1, whom you succeeded)";
    assert_eq!(said, format!("{why}; reply with fwd reply {}", &n.id().to_base64()[..8]));
    // Read by anyone else, as `fwd inbox --for` and the page show another principal's inbox, it
    // says why alone: the notification is the successor's to reply through, not the reader's.
    assert_eq!(fwd_core::reason::plain(&fwd_core::reason::gather(&ctx, &n, Some(daniel)).await?), why);
    // A task assigned to it now.
    let other = ops::create_task(&ctx, daniel, CreateTask { title: "Jitter".into(), ..Default::default() }).await?;
    ops::assign(&ctx, daniel, other, Some(old)).await?;
    let planned = routing::plan_for_assignee(&ctx, &ctx.get::<TaskView>(other).await?, None, Some(daniel)).await?;
    assert_eq!(heard(&planned), vec![(new, NotificationType::TaskAssigned, for_old.clone())]);
    // The successor never hears of what it did itself, on the old principal's task.
    let own = ops::post_message(&ctx, new, "picking this up", Priority::Next, on_task()).await?;
    let planned = routing::plan_for_message(&ctx, &ctx.get::<MessageView>(own).await?).await?;
    assert!(planned.iter().all(|p| p.recipient != new), "{planned:?}");
    // Succeeded in turn, the successor passes it on to its own successor, still for the first.
    let third =
        ops::add_principal(&ctx, "triage-3", Some(AgentRole::Reviewer), None, None, None, Some(&agent("t3"))).await?;
    ops::succeed_principal(&ctx, "triage-2", third).await?;
    let mc = MessageContext { to: Some(old.to_base64()), ..Default::default() };
    let asked = ops::post_message(&ctx, daniel, "anyone?", Priority::Now, mc).await?;
    let planned = routing::plan_for_message(&ctx, &ctx.get::<MessageView>(asked).await?).await?;
    assert_eq!(heard(&planned), vec![(third, NotificationType::QuestionAsked, for_old)]);
    Ok(())
}

/// An agent relaunched to carry on another's job edits the messages its predecessor wrote, so that
/// an answer a retired agent gave is corrected where it stands by the agent that carries on its
/// work (task sVCUXcEg; docs/DECISIONS.md, D114, a relaunched agent's user succeeds its
/// predecessor's): the successor edits it, and so does the successor of that successor, each edit
/// saying who made it (`edited_by`), while the message stays its author's; a person or another
/// agent is refused, told to reply or to ask the agent at work in the author's place; and whoever
/// an edit names anew hears of it from the agent that made the edit, which hears nothing of its
/// own edit, not even as its predecessor's successor.
#[tokio::test]
async fn a_successor_edits_the_messages_of_the_principal_it_succeeded_and_no_one_else_does() -> Result<()> {
    let ctx = context().await?;
    let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let alice =
        ops::add_principal(&ctx, "impl-alice", Some(AgentRole::Implementer), None, None, None, Some(&agent("a1")))
            .await?;
    let old = ops::add_principal(
        &ctx,
        "triage-1",
        Some(AgentRole::Triage),
        Some(ops::Serves::ProjectWide),
        None,
        None,
        Some(&agent("t1")),
    )
    .await?;
    let task =
        ops::create_task(&ctx, daniel, CreateTask { title: "Say what the primer is".into(), ..Default::default() })
            .await?;
    let on_task = MessageContext { task_id: Some(task), ..Default::default() };
    let answer = ops::post_message(&ctx, old, "the primer is the README", Priority::Next, on_task).await?;
    let new =
        ops::add_principal(&ctx, "triage-2", Some(AgentRole::Reviewer), None, None, None, Some(&agent("t2"))).await?;
    ops::succeed_principal(&ctx, "triage-1", new).await?;

    // Another agent is refused, and told to reply or to ask the agent at work in the author's
    // place; so is a person.
    let stranger = refusal(ops::edit_message(&ctx, alice, answer, "mine now").await);
    assert_eq!(
        stranger,
        "only its author edits a message, or an agent that succeeded its author, and this one is @triage-1's, whom @triage-2 succeeded; reply to it instead, or ask @triage-2 to edit it"
    );
    let person = refusal(ops::edit_message(&ctx, daniel, answer, "mine now").await);
    assert_eq!(person, stranger);
    let untouched: MessageView = ctx.get(answer).await?;
    assert_eq!(
        (untouched.edited()?, untouched.edited_by()?.map(EntityId::from)),
        (None, None),
        "a refused edit writes nothing"
    );

    // The successor corrects it where it stands; the message stays its author's and says who
    // edited it.
    ops::edit_message(&ctx, new, answer, "the primer is `docs/using-fwd.md`").await?;
    let edited: MessageView = ctx.get(answer).await?;
    assert_eq!(ops::text(edited.content())?, "the primer is `docs/using-fwd.md`");
    assert_eq!(EntityId::from(edited.author()?), old);
    assert!(edited.edited()?.is_some());
    assert_eq!(edited.edited_by()?.map(EntityId::from), Some(new));
    assert_eq!(ops::json_of_message(&edited)?["edited_by"], new.to_base64());

    // Succeeded in turn, the successor passes the right on: the successor of its successor edits
    // the message too, and a refusal names the agent at work now.
    let third =
        ops::add_principal(&ctx, "triage-3", Some(AgentRole::Reviewer), None, None, None, Some(&agent("t3"))).await?;
    ops::succeed_principal(&ctx, "triage-2", third).await?;
    let stranger = refusal(ops::edit_message(&ctx, alice, answer, "mine now").await);
    assert!(
        stranger.ends_with(
            "this one is @triage-1's, whom @triage-3 succeeded; reply to it instead, or ask @triage-3 to edit it"
        ),
        "{stranger}"
    );
    ops::edit_message(&ctx, third, answer, "@impl-alice, @triage-1 meant `docs/using-fwd.md`").await?;
    assert_eq!(ctx.get::<MessageView>(answer).await?.edited_by()?.map(EntityId::from), Some(third));

    // Alice, named anew, hears of it from the agent that made the edit. The predecessor, named
    // too, would reach its last successor, which made the edit, and so reaches no one.
    let mut told = Vec::new();
    for r in ops::relations_from(&ctx, answer, Some(RelationKind::Mention)).await? {
        let planned = routing::plan_for_relation(&ctx, &r).await?;
        let heard: Vec<(EntityId, EntityId, String)> = planned
            .iter()
            .map(|p| (r.to_id().unwrap(), p.recipient, p.originator["actor_id"].as_str().unwrap_or("").to_string()))
            .collect();
        told.extend(heard);
        if r.to_id()? == alice {
            let made = routing::apply(&ctx, &planned).await?;
            let n: NotificationView = ctx.get(made[0]).await?;
            let said = fwd_core::reason::plain(&fwd_core::reason::gather(&ctx, &n, Some(alice)).await?);
            let short = &n.id().to_base64()[..8];
            assert_eq!(
                said,
                format!("triage-3 mentioned you in a comment on Say what the primer is; reply with fwd reply {short}")
            );
        }
    }
    assert_eq!(told, vec![(alice, alice, third.to_base64())]);
    Ok(())
}

/// Succession is refused where it would not hand an agent's job to another agent: a person on
/// either side, an agent succeeding itself, one succeeded already, and a triage another holds. A
/// lead's job passes to its successor while other leads hold lead, a project having one per
/// session that coordinates its work (D119).
#[tokio::test]
async fn succession_is_refused_where_it_would_not_hand_one_agents_job_to_another() -> Result<()> {
    let ctx = context().await?;
    let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let old = ops::add_principal(&ctx, "lead-1", Some(AgentRole::Lead), None, None, None, None).await?;
    let new = ops::add_principal(&ctx, "lead-2", Some(AgentRole::Reviewer), None, None, None, None).await?;
    let person = refusal(ops::succeed_principal(&ctx, "lead-1", daniel).await);
    assert!(person.starts_with("`@dnorman` is a person, and a person succeeds no agent"), "{person}");
    let of_person = refusal(ops::succeed_principal(&ctx, "dnorman", new).await);
    assert!(of_person.starts_with("`@dnorman` is a person, and a person is never succeeded"), "{of_person}");
    let itself = refusal(ops::succeed_principal(&ctx, "lead-2", new).await);
    assert_eq!(itself, "`@lead-2` is the principal this command acts as, and an agent does not succeed itself");
    let nobody = refusal(ops::succeed_principal(&ctx, "ghost", new).await);
    assert_eq!(nobody, "no principal of this project is named `ghost`, so there is none to succeed");
    // Retired, with another agent holding lead since, lead-1's job still passes to its successor,
    // who leads beside that agent.
    ops::retire_principal(&ctx, "lead-1").await?;
    let lead_x = ops::add_principal(&ctx, "lead-x", Some(AgentRole::Lead), None, None, None, None).await?;
    ops::succeed_principal(&ctx, "lead-1", new).await?;
    for lead in [lead_x, new] {
        assert_eq!(ctx.get::<PrincipalView>(lead).await?.agent_role()?, Some(AgentRole::Lead));
    }
    let twice = refusal(ops::succeed_principal(&ctx, "lead-1", new).await);
    assert_eq!(twice, "@lead-2 succeeded `@lead-1` already");
    let late = ops::add_principal(&ctx, "lead-3", Some(AgentRole::Reviewer), None, None, None, None).await?;
    let taken = refusal(ops::succeed_principal(&ctx, "lead-1", late).await);
    assert!(
        taken
            .starts_with("`@lead-1` was succeeded by @lead-2 already, who carries on its job: succeed @lead-2 instead"),
        "{taken}"
    );
    // Retired, with another agent serving the whole project since, triage-1's endorsement is that
    // agent's: a project has one triage agent that serves it whole.
    ops::add_principal(&ctx, "triage-1", Some(AgentRole::Triage), Some(ops::Serves::ProjectWide), None, None, None)
        .await?;
    ops::retire_principal(&ctx, "triage-1").await?;
    ops::add_principal(&ctx, "triage-x", Some(AgentRole::Triage), Some(ops::Serves::ProjectWide), None, None, None)
        .await?;
    let held = refusal(ops::succeed_principal(&ctx, "triage-1", late).await);
    assert_eq!(
        held,
        "@triage-x serves the whole project already, and a project has one triage agent that serves it whole: withdraw its endorsement first (`fwd principal scope project-wide --withdraw --for triage-x`), or retire it (`fwd principal retire triage-x`), for `@lead-3` to serve the whole project"
    );
    // A principal a successor took over is not taken over by binding it either.
    let bound = refusal(ops::bind_principal(&ctx, "lead-1", &agent("a5"), Some(old)).await);
    assert!(bound.starts_with("`@lead-1` was succeeded by @lead-2, who carries on its job"), "{bound}");
    Ok(())
}

/// A relaunched agent names itself as its predecessor's successor in one step (`fwd principal add
/// <name> --succeeds <old>`): it holds the role its predecessor held, a lead's or a triage agent's
/// included, with the scopes a triage agent served, which the one-triage-agent-per-scope rule, and
/// the one lead of a session, count on the successor and not on the predecessor it retires, and it
/// takes over the predecessor's job as `succeed` hands it over.
#[tokio::test]
async fn an_agent_names_itself_as_its_predecessors_successor_in_one_step() -> Result<()> {
    let ctx = context().await?;
    let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let old = ops::add_principal(&ctx, "lead-1", Some(AgentRole::Lead), None, None, None, Some(&agent("main"))).await?;
    let waiting = ops::create_notification(
        &ctx,
        old,
        NotificationType::QuestionAsked,
        Priority::Now,
        serde_json::json!({"type": "Message", "id": old.to_base64()}),
        serde_json::json!({"actor_id": daniel.to_base64(), "why": "addressee"}),
    )
    .await?;
    // Named lead, in a session of its own, while lead-1 still holds lead.
    let s2 = "claude-code:s2/main";
    let done = ops::add_successor(&ctx, "lead-2", Some(AgentRole::Lead), None, None, None, Some(s2), "lead-1").await?;
    let new = done.successor;
    assert_eq!(done, ops::Succession { successor: new, old, role: Some(AgentRole::Lead), moved: 1 });
    let made: PrincipalView = ctx.get(new).await?;
    assert_eq!(
        (made.handle()?, made.agent_role()?, made.active()?, made.member()?, made.subject()?),
        ("lead-2".to_string(), Some(AgentRole::Lead), true, Some(true), Some(s2.to_string())),
        "named, holding lead, belonging, and bound to its agent"
    );
    let old_view: PrincipalView = ctx.get(old).await?;
    assert!(!old_view.active()? && old_view.agent_role()?.is_none());
    assert_eq!(old_view.succeeded_by()?.map(EntityId::from), Some(new));
    assert_eq!(EntityId::from(ctx.get::<NotificationView>(waiting).await?.recipient()?), new, "its mail moved");
    // Without --role, the successor takes the role its predecessor held, and what it served: the
    // scopes move, and the predecessor serves nothing any more.
    let replication =
        ops::create_scope(&ctx, daniel, CreateScope { title: "Replication".into(), ..Default::default() }).await?;
    let first =
        ops::add_principal(&ctx, "triage-1", Some(AgentRole::Triage), Some(ops::Serves::ProjectWide), None, None, None)
            .await?;
    ops::serve(&ctx, "triage-1", ops::Serves::Scope(replication)).await?;
    let triage = ops::add_successor(&ctx, "triage-2", None, None, None, None, None, "triage-1").await?;
    assert_eq!(front(&ctx, None).await?, Some(triage.successor));
    assert_eq!(front(&ctx, Some(replication)).await?, Some(triage.successor));
    assert_eq!(serving(&ctx, first).await?, ops::Serving::default(), "the predecessor's scopes moved");
    // Another role than the one it carries on is refused, and nothing is made.
    let other =
        refusal(ops::add_successor(&ctx, "lead-x", Some(AgentRole::Reviewer), None, None, None, None, "lead-2").await);
    assert_eq!(other, "`@lead-x` carries on `@lead-2`'s job, whose role is lead: leave `--role` off, or give lead");
    assert!(ops::principals(&ctx).await?.iter().all(|p| p.handle().unwrap() != "lead-x"), "none was made");
    // A name taken is refused: the caller's own, which succeeds with `succeed`, or another's.
    let own = refusal(ops::add_successor(&ctx, "lead-2", None, None, None, Some(new), None, "triage-2").await);
    assert_eq!(
        own,
        "`@lead-2` is a principal already, the one this command acts as: it carries on another's job with `fwd principal succeed triage-2`"
    );
    let taken = refusal(ops::add_successor(&ctx, "triage-2", None, None, None, None, None, "lead-2").await);
    assert!(taken.starts_with("the name `triage-2` is taken"), "{taken}");
    // An agent bound already has its principal: a subject binds one.
    let bound = refusal(ops::add_successor(&ctx, "lead-3", None, None, None, Some(new), Some(s2), "lead-2").await);
    assert!(
        bound.starts_with("this command runs as the Claude Code agent claude-code:s2/main, which is @lead-2 already"),
        "{bound}"
    );
    // A predecessor taken over already is refused, naming its successor and the command for it.
    let twice = refusal(ops::add_successor(&ctx, "lead-4", None, None, None, None, None, "lead-1").await);
    assert_eq!(
        twice,
        "`@lead-1` was succeeded by @lead-2 already, who carries on its job: succeed @lead-2 instead, `fwd principal add lead-4 --succeeds lead-2`"
    );
    // Where another principal serves a scope the predecessor served, since it retired, naming it.
    ops::retire_principal(&ctx, "triage-2").await?;
    ops::add_principal(
        &ctx,
        "triage-x",
        Some(AgentRole::Triage),
        Some(ops::Serves::Scope(replication)),
        None,
        None,
        None,
    )
    .await?;
    let held = refusal(ops::add_successor(&ctx, "triage-3", None, None, None, None, None, "triage-2").await);
    assert!(held.starts_with("the scope \"Replication\"") && held.contains("@triage-x"), "{held}");
    Ok(())
}

/// Succession moves a triage agent's scopes onto the successor besides those the successor served
/// as a triage agent itself; the successor of a triage agent that serves nothing, as the migration
/// leaves those from before 2026-09-30, serves nothing too, and is not refused; one that takes
/// triage anew by `--role`, from a predecessor that held none, serves the scope its command gives
/// and is refused with none.
#[tokio::test]
async fn succession_moves_a_triage_agents_scopes() -> Result<()> {
    let ctx = context().await?;
    let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let scope = |title: &str| {
        let (ctx, title) = (ctx.clone(), title.to_string());
        async move { ops::create_scope(&ctx, daniel, CreateScope { title, ..Default::default() }).await }
    };
    let (a, b) = (scope("Replication").await?, scope("Storage").await?);
    ops::add_principal(&ctx, "triage-a", Some(AgentRole::Triage), Some(ops::Serves::Scope(a)), None, None, None)
        .await?;
    let heir =
        ops::add_principal(&ctx, "triage-b", Some(AgentRole::Triage), Some(ops::Serves::Scope(b)), None, None, None)
            .await?;
    // triage-b carries on triage-a's job as well as its own: it serves both scopes.
    ops::succeed_principal(&ctx, "triage-a", heir).await?;
    assert_eq!(serving(&ctx, heir).await?.scopes, vec![b, a]);
    assert_eq!((front(&ctx, Some(a)).await?, front(&ctx, Some(b)).await?), (Some(heir), Some(heir)));
    // A triage agent serving nothing is succeeded by one serving nothing.
    let old =
        ops::ensure_principal(&ctx, "triage-old", "triage-old", PrincipalKind::Agent, Some(AgentRole::Triage)).await?;
    let next = ops::add_successor(&ctx, "triage-next", None, None, None, None, None, "triage-old").await?;
    assert!(ops::serves_nothing_yet(&ctx.get::<PrincipalView>(next.successor).await?));
    let _ = old;
    // Triage taken anew by a successor of an agent that held no role needs a scope.
    ops::ensure_principal(&ctx, "impl-old", "impl-old", PrincipalKind::Agent, None).await?;
    let none = refusal(
        ops::add_successor(&ctx, "triage-c", Some(AgentRole::Triage), None, None, None, None, "impl-old").await,
    );
    assert_eq!(none, ops::serves_nothing("triage-c"));
    let c = scope("Docs").await?;
    let taken = ops::add_successor(
        &ctx,
        "triage-c",
        Some(AgentRole::Triage),
        Some(ops::Serves::Found(c)),
        None,
        None,
        None,
        "impl-old",
    )
    .await?;
    assert_eq!(front(&ctx, Some(c)).await?, Some(taken.successor));
    Ok(())
}

/// A project has a lead per session or worktree that coordinates its work, and a Claude Code
/// session has one, its own thread (docs/DECISIONS.md, D119): lead is refused to a principal bound
/// to an agent of a session whose lead is bound already, by `add`, `role`, `bind`, `add
/// --succeeds`, `succeed` and a retired lead's coming back, the sentence naming that lead; an agent
/// of another session, and a principal with no subject, whose session fwd cannot tell, lead beside
/// it. A successor of the session's lead takes lead as before, since the predecessor it retires no
/// longer counts, nor does a retired lead.
#[tokio::test]
async fn a_session_has_one_lead_and_a_project_one_per_session() -> Result<()> {
    let ctx = context().await?;
    let lead_1 =
        ops::add_principal(&ctx, "lead-1", Some(AgentRole::Lead), None, None, None, Some(&agent("main"))).await?;
    let lead_2 =
        ops::add_principal(&ctx, "lead-2", Some(AgentRole::Lead), None, None, None, Some("claude-code:s2/main"))
            .await?;
    let pilot = ops::add_principal(&ctx, "pilot", Some(AgentRole::Lead), None, None, None, None).await?;
    assert_eq!(leads(&ctx).await?, sorted([lead_1, lead_2, pilot]), "a lead per session, and one with none");
    // An agent of s1 is refused lead as it names itself, and nothing is made.
    let named =
        refusal(ops::add_principal(&ctx, "impl-a1", Some(AgentRole::Lead), None, None, None, Some(&agent("a1"))).await);
    assert_eq!(
        named,
        "the Claude Code session s1 has a lead already, @lead-1, and a session has one lead, its own thread: an agent a lead dispatches selects implementer, reviewer or triage instead; `@impl-a1` may hold lead in this session once @lead-1 holds another role or is retired"
    );
    assert!(ops::principals(&ctx).await?.iter().all(|p| p.handle().unwrap() != "impl-a1"), "none was made");
    // Named an implementer, it is refused lead by `role`.
    let a1 =
        ops::add_principal(&ctx, "impl-a1", Some(AgentRole::Implementer), None, None, None, Some(&agent("a1"))).await?;
    let by_role = refusal(ops::set_role(&ctx, "impl-a1", AgentRole::Lead, None).await);
    assert!(by_role.starts_with("the Claude Code session s1 has a lead already, @lead-1,"), "{by_role}");
    // An agent of s1 does not take over a lead from before the plugin by binding it.
    let by_bind = refusal(ops::bind_principal(&ctx, "pilot", &agent("a2"), Some(pilot)).await);
    assert!(by_bind.starts_with("the Claude Code session s1 has a lead already, @lead-1,"), "{by_bind}");
    assert_eq!(subject_of(&ctx, pilot).await?, None, "nothing was bound");
    // Nor does it carry on another session's lead, named already or naming itself.
    let by_succeeding = refusal(ops::succeed_principal(&ctx, "lead-2", a1).await);
    assert!(by_succeeding.starts_with("the Claude Code session s1 has a lead already, @lead-1,"), "{by_succeeding}");
    let by_naming =
        refusal(ops::add_successor(&ctx, "lead-2b", None, None, None, None, Some(&agent("a3")), "lead-2").await);
    assert!(by_naming.starts_with("the Claude Code session s1 has a lead already, @lead-1,"), "{by_naming}");
    let untouched: PrincipalView = ctx.get(lead_2).await?;
    assert!(untouched.active()? && untouched.succeeded_by()?.is_none(), "lead-2 carries on its own job");
    // A successor of s1's lead takes lead in s1: the predecessor it retires no longer counts.
    let heir = ops::add_successor(&ctx, "lead-1b", None, None, None, None, Some(&agent("a3")), "lead-1").await?;
    assert_eq!(heir.role, Some(AgentRole::Lead));
    assert_eq!(leads(&ctx).await?, sorted([heir.successor, lead_2, pilot]));
    // Retired, s1's lead no longer counts, so another agent of s1 selects lead; the retired one is
    // then refused lead as it comes back, and may come back with another role.
    ops::retire_principal(&ctx, "lead-1b").await?;
    assert_eq!(ops::set_role(&ctx, "impl-a1", AgentRole::Lead, None).await?, a1);
    let back =
        refusal(ops::add_principal(&ctx, "lead-1b", None, None, None, Some(heir.successor), Some(&agent("a3"))).await);
    assert!(back.starts_with("the Claude Code session s1 has a lead already, @impl-a1,"), "{back}");
    assert!(!ctx.get::<PrincipalView>(heir.successor).await?.active()?, "it stays retired");
    ops::add_principal(
        &ctx,
        "lead-1b",
        Some(AgentRole::Reviewer),
        None,
        None,
        Some(heir.successor),
        Some(&agent("a3")),
    )
    .await?;
    // An agent of another session, bound to a lead from before the plugin, leads beside s1's.
    assert_eq!(ops::bind_principal(&ctx, "pilot", "claude-code:s3/main", Some(pilot)).await?, pilot);
    assert_eq!(leads(&ctx).await?, sorted([a1, lead_2, pilot]));
    Ok(())
}
