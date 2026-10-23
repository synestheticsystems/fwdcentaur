//! What a typed name names: a principal's name is a convenience label that several principals of a
//! project may share, and one rule reads every name typed for a principal, within the project,
//! refusing a name several share with each of them listed by id, what it is and the scopes its work
//! is in, never guessing (docs/design/agents-and-inbox.md, "A principal's name"; docs/DECISIONS.md,
//! D107 and D110).

use ankurah::{policy::DEFAULT_CONTEXT, Context, EntityId, Node, PermissiveAgent};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::model::*;
use fwd_core::naming::{self, Named, Refused};
use fwd_core::ops::{self, CreateScope, CreateTask, MessageContext};
use fwd_core::routing;
use std::sync::Arc;

async fn context() -> Result<Context> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    Ok(node.context_async(DEFAULT_CONTEXT).await?)
}

/// A principal named `handle` made beside any other of that name, as two clients naming themselves
/// at once can leave, or a person's `fwd init` after an agent took the person's name: nothing
/// forbids a second principal a name, and `fwd principal add` names the one it finds.
async fn made(ctx: &Context, handle: &str, kind: PrincipalKind, role: Option<AgentRole>) -> Result<EntityId> {
    let trx = ctx.begin();
    let id = trx
        .create(&Principal {
            kind,
            handle: handle.into(),
            display_name: handle.into(),
            auth_subject: None,
            agent_role: role,
            active: true,
            member: Some(true),
            subject: None,
            succeeded_by: None,
            serves: None,
            // Stamped as `fwd principal add` stamps the agent it names.
            last_active: Some(fwd_core::now_ms()),
        })
        .await?
        .id();
    trx.commit().await?;
    Ok(id)
}

/// The first eight characters of an id, as a refusal lists a principal.
fn short(id: EntityId) -> String {
    id.to_base64().chars().take(8).collect()
}

/// The refusal a name met, as its sentence.
fn refused(result: Result<Option<PrincipalView>>) -> String {
    let e = result.expect_err("a name several principals share is refused");
    e.downcast_ref::<Refused>().map(|r| r.0.clone()).unwrap_or_else(|| panic!("not a refusal of a shared name: {e:#}"))
}

/// Two candidates as a refusal lists them, in the order of their ids.
fn two(a: (EntityId, &str), b: (EntityId, &str)) -> String {
    let (first, second) = if a.0.to_base64() < b.0.to_base64() { (a, b) } else { (b, a) };
    format!("{}: {}; {}: {}", short(first.0), first.1, short(second.0), second.1)
}

async fn named(ctx: &Context, name: &str) -> Result<Option<EntityId>> {
    Ok(naming::principal_named(ctx, name, None).await?.map(|p| p.id()))
}

#[tokio::test]
async fn a_name_one_principal_has_names_it_as_its_id_does() -> Result<()> {
    let ctx = context().await?;
    let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let alice = ops::add_principal(&ctx, "impl-alice", Some(AgentRole::Implementer), None, None, None, None).await?;
    assert_eq!(named(&ctx, "impl-alice").await?, Some(alice));
    assert_eq!(named(&ctx, "@impl-alice").await?, Some(alice), "the at-sign is optional");
    assert_eq!(named(&ctx, " impl-alice ").await?, Some(alice), "and so are the spaces around it");
    assert_eq!(named(&ctx, &alice.to_base64()).await?, Some(alice), "its whole id names it");
    assert_eq!(named(&ctx, &short(alice)).await?, Some(alice), "and so do its first eight characters");
    assert_eq!(named(&ctx, "Impl-Alice").await?, None, "a name is matched as it is written");
    // Every command that takes a name reads it so: the principal commands take the id as well.
    assert_eq!(ops::set_role(&ctx, &short(alice), AgentRole::Reviewer, None).await?, alice);
    assert_eq!(
        ops::add_principal(&ctx, &alice.to_base64(), Some(AgentRole::Implementer), None, None, Some(alice), None)
            .await?,
        alice
    );
    let principals = ops::principals(&ctx).await?;
    assert!(!naming::shows_scopes(&principals, "impl-alice"), "a name no other principal has shows bare in a picker");

    // The name is impl-alice's alone: whoever else adds it, a person or an agent naming itself
    // (acting as no principal yet), is told whose it is and to choose another, and no second
    // principal is made, so a second agent never becomes the first one's user (D110, D112).
    let taken = format!(
        "the name `impl-alice` is taken: {} (implementer, in no scope) has it in this project, and only that principal, acting as itself (`--as` or FWD_AS naming it), adds itself again, to come back or to change its role; choose a name no other principal of this project has, as every agent does (a role and a qualifier reads well: `implementer-replication`; docs/DECISIONS.md, D110)",
        short(alice)
    );
    for acting in [Some(daniel), None] {
        let added = ops::add_principal(&ctx, "@impl-alice", Some(AgentRole::Reviewer), None, None, acting, None).await;
        assert_eq!(added.unwrap_err().to_string(), taken, "acting as {acting:?}");
    }
    let added = ops::add_principal(&ctx, &short(alice), None, None, None, Some(daniel), None).await;
    assert_eq!(added.unwrap_err().to_string(), taken, "named by its id, it is still hers");
    assert_eq!(naming::by_name(&ops::principals(&ctx).await?, "impl-alice", None).len(), 1, "no second");
    assert_eq!(ops::role_of(&ctx.get::<PrincipalView>(alice).await?), Some(AgentRole::Implementer), "nor a new role");
    Ok(())
}

#[tokio::test]
async fn a_name_no_principal_has_names_no_one_and_each_command_says_how_one_is_made() -> Result<()> {
    let ctx = context().await?;
    ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    assert_eq!(named(&ctx, "nobody").await?, None);
    assert_eq!(named(&ctx, "abc").await?, None, "too short to start an id");
    let principals = ops::principals(&ctx).await?;
    assert!(matches!(naming::resolve(&principals, "nobody", None), Named::NoOne));
    assert_eq!(
        ops::set_role(&ctx, "nobody", AgentRole::Implementer, None).await.unwrap_err().to_string(),
        "no principal of this project is named `nobody`: an agent names itself and selects its role at once, `fwd principal add nobody --role implementer`"
    );
    assert_eq!(
        ops::retire_principal(&ctx, "nobody").await.unwrap_err().to_string(),
        "no one in this project has the handle `@nobody`"
    );
    assert_eq!(
        ops::person_by_handle(&ctx, "nobody").await.unwrap_err().to_string(),
        "no one in this project has the handle `@nobody`"
    );
    Ok(())
}

#[tokio::test]
async fn a_name_two_principals_share_is_refused_listing_each_by_id_role_and_the_scopes_of_its_work() -> Result<()> {
    let ctx = context().await?;
    let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let one = ops::add_principal(&ctx, "implementer", Some(AgentRole::Implementer), None, None, None, None).await?;
    let other = made(&ctx, "implementer", PrincipalKind::Agent, Some(AgentRole::Reviewer)).await?;

    // Each one's scopes come from its work: `one` made Replication and works a task there; `other`
    // made Anchoring and comments on it.
    let replication =
        ops::create_scope(&ctx, one, CreateScope { title: "Replication".into(), ..Default::default() }).await?;
    let anchoring =
        ops::create_scope(&ctx, other, CreateScope { title: "Anchoring".into(), ..Default::default() }).await?;
    let task = ops::create_task(&ctx, daniel, CreateTask { title: "Replicate".into(), ..Default::default() }).await?;
    ops::claim(&ctx, one, task, replication).await?;
    let on_scope = MessageContext { scope_id: Some(anchoring), ..Default::default() };
    ops::post_message(&ctx, other, "the anchors drift", Priority::Later, on_scope).await?;
    let loaded = naming::Loaded::of(&ctx, &[one, other]).await?;
    assert_eq!(naming::scopes_of(one, &loaded.work()), vec![replication]);
    assert_eq!(naming::scopes_of(other, &loaded.work()), vec![anchoring]);
    assert_eq!(naming::scopes_of(daniel, &loaded.work()), vec![replication], "the task he made is worked there");

    let expected = format!(
        "2 principals of this project are named `implementer` ({}); give one's id, or its first eight characters, in place of the name",
        two((one, "implementer, in Replication"), (other, "reviewer, in Anchoring"))
    );
    assert_eq!(refused(naming::principal_named(&ctx, "@implementer", None).await), expected);
    let principals = ops::principals(&ctx).await?;
    assert!(matches!(naming::resolve(&principals, "implementer", None), Named::Shared(ref s) if s.len() == 2));
    assert!(naming::shows_scopes(&principals, "implementer"), "a picker shows the scopes beside a repeated name");
    assert!(!naming::shows_scopes(&principals, "dnorman"));

    // The id names one of them, whole or by its first eight characters, wherever a name is taken.
    assert_eq!(named(&ctx, &other.to_base64()).await?, Some(other));
    assert_eq!(named(&ctx, &short(one)).await?, Some(one));
    // Every command refuses the name, and `principal add` makes no third principal of it.
    for refusal in [
        ops::set_role(&ctx, "implementer", AgentRole::Implementer, None).await.unwrap_err(),
        ops::retire_principal(&ctx, "implementer").await.unwrap_err(),
        ops::add_principal(&ctx, "implementer", Some(AgentRole::Implementer), None, None, None, None)
            .await
            .unwrap_err(),
    ] {
        assert_eq!(refusal.to_string(), expected);
    }
    assert_eq!(naming::by_name(&ops::principals(&ctx).await?, "implementer", None).len(), 2);

    // A mention by the shared name refuses the post, listing them; the id's start names one.
    let posted = ops::post_message(&ctx, daniel, "ping @implementer", Priority::Later, MessageContext::default()).await;
    assert_eq!(posted.unwrap_err().to_string(), format!("refused: {expected}"));
    let written = format!("ping @{}", short(other));
    let m = ops::post_message(&ctx, daniel, &written, Priority::Later, MessageContext::default()).await?;
    let message: MessageView = ctx.get(m).await?;
    assert_eq!(message.content()?, format!("ping [[{}]]", other.to_base64()), "stored by id");

    // A question whose `to` holds the shared handle, as an older build wrote one, asks neither.
    let to_handle = MessageContext { to: Some("implementer".into()), ..Default::default() };
    let asked = ops::post_message(&ctx, daniel, "which of you?", Priority::Later, to_handle).await?;
    let plan = routing::plan_for_message(&ctx, &ctx.get::<MessageView>(asked).await?).await?;
    assert!(
        plan.iter().all(|p| p.notification_type != NotificationType::QuestionAsked),
        "a handle two principals share addresses neither: {:?}",
        plan.iter().map(|p| (p.recipient, p.notification_type)).collect::<Vec<_>>()
    );

    // The mention search offers the principals that belong whose name starts with what is typed,
    // and writes a shared name's mention by the start of the id, which posting reads back.
    let offered: Vec<EntityId> =
        naming::mentionable(&principals, "IMPL", 8, fwd_core::now_ms()).iter().map(|p| p.id()).collect();
    let mut both = vec![one, other];
    both.sort();
    assert_eq!(offered, both, "without regard to case, by name, then by id");
    let view = |id: EntityId| principals.iter().find(|p| p.id() == id).expect("a principal of the project");
    assert_eq!(naming::mention_of(&principals, view(other)), format!("@{}", short(other)));
    assert_eq!(naming::mention_of(&principals, view(daniel)), "@dnorman");

    // A principal in many scopes is listed with three of them and a count of the rest.
    let several = naming::by_name(&principals, "implementer", None);
    let scopes = |_| ["A", "B", "C", "D", "E"].map(String::from).to_vec();
    assert!(naming::refusal("implementer", &several, false, scopes).contains(": implementer, in A, B, C and 2 more"));
    Ok(())
}

#[tokio::test]
async fn a_retired_principal_is_named_still_and_counts_among_those_sharing_its_name() -> Result<()> {
    let ctx = context().await?;
    ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let old = ops::add_principal(&ctx, "builder", Some(AgentRole::Implementer), None, None, None, None).await?;
    ops::retire_principal(&ctx, "builder").await?;
    assert_eq!(named(&ctx, "builder").await?, Some(old), "retired, it is still named");
    // Its name stays its own: another caller cannot bring it back, nor take the name.
    let taken = ops::add_principal(&ctx, "builder", Some(AgentRole::Implementer), None, None, None, None).await;
    let taken = taken.unwrap_err().to_string();
    let whose = format!("the name `builder` is taken: {} (implementer, retired, in no scope) has it", short(old));
    assert!(taken.starts_with(&whose), "{taken}");
    assert_eq!(
        ops::add_principal(&ctx, "builder", None, None, None, Some(old), None).await?,
        old,
        "principal add, acting as it, brings it back"
    );
    ops::retire_principal(&ctx, "builder").await?;

    let new = made(&ctx, "builder", PrincipalKind::Agent, Some(AgentRole::Implementer)).await?;
    let principals = ops::principals(&ctx).await?;
    let offered: Vec<EntityId> =
        naming::mentionable(&principals, "build", 8, fwd_core::now_ms()).iter().map(|p| p.id()).collect();
    assert_eq!(offered, vec![new], "the mention search offers only principals that belong");
    assert_eq!(
        refused(naming::principal_named(&ctx, "builder", None).await),
        format!(
            "2 principals of this project are named `builder` ({}); give one's id, or its first eight characters, in place of the name",
            two((old, "implementer, retired, in no scope"), (new, "implementer, in no scope"))
        ),
        "never the active one in its place"
    );
    Ok(())
}

#[tokio::test]
async fn a_person_and_an_agent_sharing_a_name_are_told_apart_only_where_a_command_takes_a_person() -> Result<()> {
    let ctx = context().await?;
    let person = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    ops::join(&ctx, person).await?; // as `fwd init` makes its person belong
    let agent = made(&ctx, "dnorman", PrincipalKind::Agent, Some(AgentRole::Implementer)).await?;
    assert_eq!(
        refused(naming::principal_named(&ctx, "dnorman", None).await),
        format!(
            "2 principals of this project are named `dnorman` ({}); give one's id, or its first eight characters, in place of the name",
            two((person, "person, in no scope"), (agent, "implementer, in no scope"))
        ),
        "--as, --to, --for and task assign take either, so they refuse it"
    );
    // A ruling is a person's, and the CLI's own person is found among people.
    assert_eq!(ops::person_by_handle(&ctx, "@dnorman").await?, person);
    assert_eq!(
        naming::principal_named(&ctx, "dnorman", Some(PrincipalKind::Human)).await?.map(|p| p.id()),
        Some(person)
    );
    assert_eq!(ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?, person);
    // Naming the agent by its id still works where either is taken.
    assert_eq!(ops::set_role(&ctx, &short(agent), AgentRole::Reviewer, None).await?, agent);
    // An agent whose name only an agent has is refused as a decider, as before.
    ops::add_principal(&ctx, "triage-bot", Some(AgentRole::Triage), Some(ops::Serves::ProjectWide), None, None, None)
        .await?;
    let err = ops::person_by_handle(&ctx, "triage-bot").await.unwrap_err().to_string();
    assert!(err.contains("`@triage-bot` is an agent, not a person"), "{err}");
    Ok(())
}

/// An agent whose name turned out to be shared renames itself (`fwd principal rename`) and is
/// named by a name again: it keeps its id, so its role, the agent bound to it, what it made and
/// said and its inbox stay its own, and what names it by its id shows the new name; the name it
/// left names the other principal alone. A name another principal has, or one that names another
/// by its id, is refused with that principal listed, and a person is not renamed here (task
/// u-nZCXww; docs/design/agents-and-inbox.md, "A principal's name").
#[tokio::test]
async fn an_agent_renames_itself_and_keeps_its_id_and_everything_that_refers_to_it() -> Result<()> {
    let ctx = context().await?;
    let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let subject = "claude-code:s1/a1";
    let one =
        ops::add_principal(&ctx, "implementer", Some(AgentRole::Implementer), None, None, None, Some(subject)).await?;
    let other = made(&ctx, "implementer", PrincipalKind::Agent, Some(AgentRole::Reviewer)).await?;
    let alice =
        ops::add_principal(&ctx, "impl-alice", Some(AgentRole::Implementer), None, Some("Alice"), None, None).await?;

    // Under the shared name `one` made a task and commented on it, Daniel named it by its id, and a
    // question waits unread in its inbox.
    let task = ops::create_task(&ctx, one, CreateTask { title: "Replicate".into(), ..Default::default() }).await?;
    let on_task = MessageContext { task_id: Some(task), ..Default::default() };
    let said = ops::post_message(&ctx, one, "started", Priority::Later, on_task).await?;
    let naming_it = format!("over to @{}", short(one));
    let mention = ops::post_message(&ctx, daniel, &naming_it, Priority::Later, MessageContext::default()).await?;
    let waiting = ops::create_notification(
        &ctx,
        one,
        NotificationType::QuestionAsked,
        Priority::Now,
        serde_json::json!({"type": "Message", "id": mention.to_base64()}),
        serde_json::json!({"actor_id": daniel.to_base64(), "why": "addressee"}),
    )
    .await?;
    refused(naming::principal_named(&ctx, "implementer", None).await);

    assert_eq!(ops::rename_principal(&ctx, one, "@implementer-replication").await?, one);
    assert_eq!(named(&ctx, "implementer-replication").await?, Some(one), "the new name names it");
    assert_eq!(named(&ctx, "implementer").await?, Some(other), "and the old one names the other alone");
    let renamed: PrincipalView = ctx.get(one).await?;
    assert_eq!(renamed.handle()?, "implementer-replication");
    assert_eq!(renamed.display_name()?, "implementer-replication", "the display name, which was the name, follows");
    assert_eq!(ops::role_of(&renamed), Some(AgentRole::Implementer), "its role stays");
    assert_eq!(renamed.subject()?.as_deref(), Some(subject), "and so does the agent bound to it");
    assert_eq!(EntityId::from(ctx.get::<TaskView>(task).await?.created_by()?), one);
    assert_eq!(EntityId::from(ctx.get::<MessageView>(said).await?.author()?), one);
    let unread: Vec<EntityId> = ops::inbox(&ctx, one, true).await?.iter().map(|n| n.id()).collect();
    assert_eq!(unread, vec![waiting], "its inbox is its own still");
    let content = ctx.get::<MessageView>(mention).await?.content()?;
    assert_eq!(ops::render_references(&ctx, &content).await, "over to @implementer-replication");
    let principals = ops::principals(&ctx).await?;
    assert!(!naming::shows_scopes(&principals, "implementer-replication"), "a picker shows it bare");
    assert_eq!(ops::rename_principal(&ctx, one, "implementer-replication").await?, one, "its own name again");

    // A display name given apart from the name stays.
    ops::rename_principal(&ctx, alice, "impl-alice-2").await?;
    assert_eq!(ctx.get::<PrincipalView>(alice).await?.display_name()?, "Alice");

    // A name another principal has is refused, listing it, and so is one that names another by
    // its id; the name stays as it was.
    let refusal = |result: Result<EntityId>| result.expect_err("refused").to_string();
    let choose = "choose a name no other principal of this project has, as every agent does (a role and a qualifier reads well: `implementer-replication`; docs/DECISIONS.md, D110)";
    assert_eq!(
        refusal(ops::rename_principal(&ctx, alice, "implementer").await),
        format!(
            "the name `implementer` is taken: {} (reviewer, in no scope) has it in this project; {choose}",
            short(other)
        )
    );
    let third = made(&ctx, "implementer", PrincipalKind::Agent, Some(AgentRole::Reviewer)).await?;
    let (first, second) = if other.to_base64() < third.to_base64() { (other, third) } else { (third, other) };
    assert_eq!(
        refusal(ops::rename_principal(&ctx, alice, "implementer").await),
        format!(
            "the name `implementer` is taken: {} (reviewer, in no scope) and {} (reviewer, in no scope) have it in this project; {choose}",
            short(first),
            short(second)
        )
    );
    assert_eq!(
        refusal(ops::rename_principal(&ctx, alice, &short(one)).await),
        format!(
            "`{0}` names {0} (implementer, in no scope) by its id, wherever a name is typed; choose a name that starts no principal's id and that no other principal of this project has (a role and a qualifier reads well: `implementer-replication`)",
            short(one)
        )
    );
    assert_eq!(
        refusal(ops::rename_principal(&ctx, daniel, "daniel").await),
        "`@dnorman` is a person, whose name is the handle `fwd init` recorded on this machine, by which every command finds them: `fwd principal rename` renames the agent the command acts as"
    );
    assert_eq!(refusal(ops::rename_principal(&ctx, alice, " @ ").await), "a name cannot be blank");
    assert_eq!(ctx.get::<PrincipalView>(alice).await?.handle()?, "impl-alice-2");
    Ok(())
}
