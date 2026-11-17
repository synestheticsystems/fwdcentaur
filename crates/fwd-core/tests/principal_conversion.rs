//! `fwd admin migrate`'s move of principals from fwd's own model onto the Commons'
//! `identity.Principal` (`fwd_core::conversion::principals::convert`), on a store the build before
//! it wrote, then opened as fwd opens one (`fwd_core::durable`): each principal keeps its id and its
//! old component, gains the Commons' principal and fwd's `Role` holding what it held, each subject
//! that acted as it an identity binding, and, retired, `Closed`; every command finds it as before,
//! and prints it as before; a second run changes nothing; and one that would lose part of what it
//! holds stops the run before anything is written.

use ankurah::model::Mutable;
use ankurah::policy::DEFAULT_CONTEXT;
use ankurah::property::Ref;
use ankurah::{Context, EntityId};
use ankurah_commons::records::{ClosedView, CreatedView};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::conversion::principals::{self, before_d261, Converted};
use fwd_core::durable;
use fwd_core::model::*;
use fwd_core::ops;
use fwd_core::project_graph::{InProjectView, ProjectGraph};
use fwd_core::subject;
use serde_json::{json, Value};
use std::path::PathBuf;
use tempfile::TempDir;

/// A store in a folder of its own, with its project alpha's id.
struct Store {
    _dir: TempDir,
    path: PathBuf,
    project: uuid::Uuid,
}

impl Store {
    /// A new store, its node's context and alpha's graph on it, for writing what the build before
    /// the move wrote.
    async fn new() -> Result<(Store, Context, ProjectGraph)> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("fwd.sqlite");
        let node = durable::open(SqliteStorageEngine::open(&path).await?).await?;
        let project = uuid::Uuid::new_v4();
        let ctx = node.context_async(DEFAULT_CONTEXT).await?;
        let alpha = ProjectGraph::ensure(ctx.clone(), project, "alpha").await?;
        Ok((Store { _dir: dir, path, project }, ctx, alpha))
    }

    /// The project's graph on the store opened again, as fwd opens one.
    async fn opened(&self) -> Result<ProjectGraph> {
        let node = durable::open(SqliteStorageEngine::open(&self.path).await?).await?;
        Ok(ProjectGraph::open(node.context_async(DEFAULT_CONTEXT).await?, self.project).await?)
    }
}

/// A principal as the build before the move recorded one, every optional field none and the
/// principal at work, for a test to give what it holds.
fn principal(graph: &ProjectGraph, kind: PrincipalKind, handle: &str) -> before_d261::Principal {
    before_d261::Principal {
        project: graph.project(),
        kind,
        handle: handle.into(),
        display_name: handle.into(),
        auth_subject: None,
        agent_role: None,
        active: true,
        member: None,
        subject: None,
        succeeded_by: None,
        serves: None,
        last_active: None,
    }
}

/// Writes `principal` as the build before the move did; its id.
async fn written(graph: &ProjectGraph, principal: before_d261::Principal) -> Result<EntityId> {
    let trx = graph.begin();
    let made = trx.create(&principal).await?.id();
    trx.commit().await?;
    Ok(made)
}

/// A principal whose role the build before 2026-09-29 stored as a word outside the fixed set, as
/// it is stored: the same collection as fwd's old model, with the role as text.
mod with_old_role {
    use ankurah::{Model, Ref};
    use fwd_core::model::{PrincipalKind, Project};
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
        pub agent_role: Option<String>,
        pub active: bool,
    }
}

/// A principal written with no handle, which no build wrote but a store can hold: the same
/// collection as fwd's old model, with that field left out.
mod without_a_handle {
    use ankurah::{Model, Ref};
    use fwd_core::model::{PrincipalKind, Project};
    use serde::{Deserialize, Serialize};

    #[derive(Model, Debug, Clone, Serialize, Deserialize)]
    pub struct Principal {
        pub project: Ref<Project>,
        #[active_type(LWW)]
        pub kind: PrincipalKind,
        #[active_type(LWW)]
        pub display_name: String,
        pub active: bool,
    }
}

/// The JSON every command printed for `old` before the move (`ops::json_of_principal` as it was):
/// what each principal must print the same after it.
fn printed_before(old: &before_d261::PrincipalView) -> Result<Value> {
    let role = old.agent_role().ok().flatten();
    let active = old.active()?;
    let serves: Value = match old.serves()? {
        Some(text) => serde_json::from_str(&text)?,
        None => Value::Null,
    };
    Ok(json!({
        "id": old.id().to_base64(),
        "kind": old.kind()?.to_string(),
        "handle": old.handle()?,
        "display_name": old.display_name()?,
        "agent_role": role.map(|r| r.to_string()),
        "active": active,
        "member": old.member()?,
        "belongs": active && old.member()? != Some(false),
        "subject": old.subject()?,
        "succeeded_by": old.succeeded_by()?.map(|s| EntityId::from(s).to_base64()),
        "serves": (role == Some(AgentRole::Triage)).then(|| json!({
            "scopes": serves["scopes"].as_array().cloned().unwrap_or_default(),
            "project_wide": serves["project_wide"].as_bool() == Some(true),
        })),
        "last_active": old.last_active()?,
    }))
}

/// The JSON every command prints for each of `ids`, as it reads the principal now, its session read
/// for one principal and for a list alike.
async fn prints(graph: &ProjectGraph, ids: &[EntityId]) -> Result<Vec<Value>> {
    let sessions = subject::sessions_bound(graph).await?;
    let mut out = Vec::new();
    for id in ids {
        let one = subject::session_bound_to(graph, *id).await?;
        assert_eq!(sessions.get(id), one.as_ref(), "read for a list as for one principal");
        out.push(ops::json_of_principal(&graph.principal(*id).await?, one.as_deref())?);
    }
    Ok(out)
}

/// The binding of `subject` in the project: the principal it binds, who made it, and the migration
/// that made it.
async fn binding(graph: &ProjectGraph, subject: &str) -> Result<(EntityId, EntityId, Option<String>)> {
    let found = subject::bindings_of(graph, subject).await?;
    let [(placement, binding)] = &found[..] else { panic!("one binding of {subject}: {}", found.len()) };
    assert_eq!(placement.project_id()?, graph.record(), "placed in the principal's project");
    let created: CreatedView = graph.component_of(binding).await?.expect("made by someone");
    let migrated: Option<MigratedView> = graph.component_of(binding).await?;
    Ok((EntityId::from(binding.principal()?), *created.by()?, migrated.map(|m| m.migration()).transpose()?))
}

/// A person signed in with an account, an agent bound to its session, a triage agent bound to a
/// Codex thread and serving the whole project, an agent another succeeded and retired, an agent
/// bound to its session and signed in with an account as well, one whose
/// role is the old word `supervisor`, and one recorded before membership and dormancy were, as the
/// build before the move wrote them: each moves keeping its id and its old component; prints what
/// it printed; its subjects bind it, made by itself at the move and marked as the move's, and a
/// command carrying one acts as it; the retired one is closed at the move, by no one, and marked
/// so; and after a second run, which moves nothing, and a reopen, each prints the same.
#[tokio::test]
async fn each_principal_moves_whole_and_every_command_finds_and_prints_it_as_before() -> Result<()> {
    let (store, ctx, before) = Store::new().await?;
    let person = written(
        &before,
        before_d261::Principal {
            display_name: "Ines Marr".into(),
            auth_subject: Some("github:4401".into()),
            member: Some(true),
            last_active: Some(1_790_000_000_000),
            ..principal(&before, PrincipalKind::Human, "ines")
        },
    )
    .await?;
    let tide = written(
        &before,
        before_d261::Principal {
            agent_role: Some(AgentRole::Implementer),
            member: Some(true),
            subject: Some("claude-code:s1/a1".into()),
            last_active: Some(1_790_000_100_000),
            ..principal(&before, PrincipalKind::Agent, "impl-tide")
        },
    )
    .await?;
    let reef = written(
        &before,
        before_d261::Principal {
            agent_role: Some(AgentRole::Triage),
            member: Some(true),
            subject: Some("codex:t9/t9".into()),
            serves: Some(json!({"project_wide": true}).to_string()),
            ..principal(&before, PrincipalKind::Agent, "triage-reef")
        },
    )
    .await?;
    let gone = written(
        &before,
        before_d261::Principal {
            active: false,
            member: Some(false),
            succeeded_by: Some(Ref::new(tide)),
            ..principal(&before, PrincipalKind::Agent, "impl-gone")
        },
    )
    .await?;
    let both = written(
        &before,
        before_d261::Principal {
            agent_role: Some(AgentRole::Reviewer),
            member: Some(true),
            subject: Some("claude-code:s1/a3".into()),
            auth_subject: Some("github:5502".into()),
            ..principal(&before, PrincipalKind::Agent, "reviewer-both")
        },
    )
    .await?;
    let bare = written(&before, principal(&before, PrincipalKind::Agent, "impl-bare")).await?;
    let trx = ctx.begin();
    let old_role = trx
        .create(&with_old_role::Principal {
            project: before.project(),
            kind: PrincipalKind::Agent,
            handle: "supervisor".into(),
            display_name: "supervisor".into(),
            agent_role: Some("supervisor".into()),
            active: true,
        })
        .await?
        .id();
    trx.commit().await?;
    let mut ids = vec![person, tide, reef, gone, both, bare, old_role];
    ids.sort();
    drop(before);

    let alpha = store.opened().await?;
    assert!(ops::principals(&alpha).await?.is_empty(), "no principal reads as one before the move");
    let mut printed = Vec::new();
    for id in &ids {
        printed.push(printed_before(&alpha.get::<before_d261::PrincipalView>(*id).await?)?);
    }
    assert_eq!(printed[ids.iter().position(|id| *id == old_role).unwrap()]["agent_role"], Value::Null);

    let mut converted = principals::convert(&alpha).await?;
    converted.sort_by_key(|c| c.principal);
    let moved: Vec<(EntityId, String, bool, usize)> =
        converted.iter().map(|c| (c.principal, c.handle.clone(), c.retired, c.bindings.len())).collect();
    let mut expected = vec![
        (person, "ines".to_string(), false, 1),
        (tide, "impl-tide".to_string(), false, 1),
        (reef, "triage-reef".to_string(), false, 1),
        (gone, "impl-gone".to_string(), true, 0),
        (both, "reviewer-both".to_string(), false, 2),
        (bare, "impl-bare".to_string(), false, 0),
        (old_role, "supervisor".to_string(), false, 0),
    ];
    expected.sort();
    assert_eq!(moved, expected);

    // Every command prints each as before, and finds it as before.
    assert_eq!(prints(&alpha, &ids).await?, printed);
    for id in [person, tide, reef, gone, both, bare, old_role] {
        let old: before_d261::PrincipalView = alpha.get(id).await?;
        assert_eq!(old.project()?, alpha.project(), "the old component stays");
    }
    assert_eq!(ops::person_by_handle(&alpha, "ines").await?, person);
    assert_eq!(subject::caller(&alpha, "claude-code:s1/a1", None).await?, Some(tide));
    assert_eq!(subject::caller(&alpha, "codex:t9/t9", None).await?, Some(reef));
    assert_eq!(ops::triage_for(&alpha, None).await?.map(|f| f.agent), Some(reef));
    assert_eq!(subject::principal_of(&alpha, "github:4401").await?.map(|(role, _)| role.id()), Some(person));

    // The subjects bind them, made by each itself as the move states them, and marked as the move's.
    assert_eq!(binding(&alpha, "github:4401").await?, (person, person, Some("principal_auth_subject".into())));
    assert_eq!(binding(&alpha, "claude-code:s1/a1").await?, (tide, tide, Some("principal_subject".into())));
    assert_eq!(binding(&alpha, "codex:t9/t9").await?, (reef, reef, Some("principal_subject".into())));
    assert_eq!(binding(&alpha, "claude-code:s1/a3").await?, (both, both, Some("principal_subject".into())));
    assert_eq!(binding(&alpha, "github:5502").await?, (both, both, Some("principal_auth_subject".into())));
    assert_eq!(subject::caller(&alpha, "claude-code:s1/a3", None).await?, Some(both), "its account aside");

    // The retired one is closed at the move, by no one, and the mark says the time is the move's.
    let (retired, _) = alpha.principal(gone).await?;
    let closed: ClosedView = alpha.component_of(&retired).await?.expect("closed");
    assert!(closed.closed()?.is_some() && closed.by()?.is_none());
    let mark: MigratedView = alpha.component_of(&retired).await?.expect("marked");
    assert_eq!(mark.migration()?, "principal_active");
    let at_work: Option<MigratedView> = alpha.component_of(&alpha.get::<RoleView>(tide).await?).await?;
    assert!(at_work.is_none(), "a principal at work is moved, nothing made, and carries no mark");

    assert_eq!(principals::convert(&alpha).await?, Vec::<Converted>::new(), "a second run");
    drop(alpha);
    assert_eq!(prints(&store.opened().await?, &ids).await?, printed, "and after a reopen");
    Ok(())
}

/// A principal whose handle cannot be read, and one whose entity carries a `Role` already, which
/// the move would write over, stop the run, each named with why; and the principal that would move
/// whole does not move either.
#[tokio::test]
async fn a_principal_that_would_lose_part_of_what_it_holds_stops_the_run_before_anything_is_written() -> Result<()> {
    let (store, ctx, before) = Store::new().await?;
    let whole = written(&before, principal(&before, PrincipalKind::Agent, "impl-whole")).await?;
    let trx = ctx.begin();
    let nameless = trx
        .create(&without_a_handle::Principal {
            project: before.project(),
            kind: PrincipalKind::Agent,
            display_name: "nameless".into(),
            active: true,
        })
        .await?
        .id();
    trx.commit().await?;
    let roled = written(&before, principal(&before, PrincipalKind::Agent, "impl-roled")).await?;
    let old: before_d261::PrincipalView = before.get(roled).await?;
    let trx = before.begin();
    let edited = trx.edit(&old)?;
    let role = Role {
        project: before.project(),
        agent_role: None,
        member: None,
        succeeded_by: None,
        serves: None,
        last_active: None,
    };
    trx.add(Mutable::entity(&*edited), &role).await?;
    trx.commit().await?;
    drop(before);
    let alpha = store.opened().await?;

    let refused = format!("{:#}", principals::convert(&alpha).await.expect_err("refused"));
    assert!(refused.contains(&format!("{} (its `handle` cannot be read", nameless.to_base64())), "{refused}");
    assert!(refused.contains(&format!("{} (it carries `role` already", roled.to_base64())), "{refused}");
    assert!(!refused.contains(&whole.to_base64()), "{refused}");
    let old: before_d261::PrincipalView = alpha.get(whole).await?;
    assert!(alpha.component_of::<PrincipalView>(&old).await?.is_none(), "nothing written");
    assert!(alpha.component_of::<RoleView>(&old).await?.is_none());
    Ok(())
}

/// While a project holds a principal not moved yet, nothing makes a principal or binds a subject
/// there, since one found missing may be that principal: the person's find-or-make, an agent
/// naming itself, by the name of one not moved or by another, and an agent or a person taking one
/// over are each refused naming the migration, and make nothing; once the move has run, a command whose caller was found before it
/// finished is refused as the agent it is, the person and the agent are found, no second is made,
/// and a new agent is made and bound.
#[tokio::test]
async fn nothing_makes_a_principal_or_a_binding_beside_one_not_moved_yet() -> Result<()> {
    let (_store, _, alpha) = Store::new().await?;
    let person = written(&alpha, principal(&alpha, PrincipalKind::Human, "ines")).await?;
    let agent = written(
        &alpha,
        before_d261::Principal {
            agent_role: Some(AgentRole::Implementer),
            subject: Some("claude-code:s1/a1".into()),
            ..principal(&alpha, PrincipalKind::Agent, "impl-tide")
        },
    )
    .await?;
    let refusal = |result: Result<EntityId>| format!("{:#}", result.expect_err("refused"));
    let migrate = "holds principals that `fwd admin migrate` has not moved onto the Commons' models yet";
    let found = refusal(ops::ensure_principal(&alpha, "ines", "Ines", PrincipalKind::Human, None).await);
    assert!(found.contains("the project alpha") && found.contains(migrate), "{found}");
    assert!(found.contains("Run `fwd admin migrate --project \"alpha\"`, then this again"), "{found}");
    let named = ops::add_principal(
        &alpha,
        "impl-tide",
        Some(AgentRole::Implementer),
        None,
        None,
        None,
        Some("claude-code:s1/a1"),
    );
    assert!(refusal(named.await).contains(migrate));
    let other = ops::add_principal(
        &alpha,
        "impl-new",
        Some(AgentRole::Implementer),
        None,
        None,
        None,
        Some("claude-code:s1/a2"),
    );
    assert!(refusal(other.await).contains(migrate));
    // Taking one over is refused before its name is looked for, so that every check of a binding
    // runs on the principals moved.
    assert!(refusal(ops::bind_principal(&alpha, "impl-tide", "claude-code:s1/a2", None).await).contains(migrate));
    assert!(refusal(ops::admin_bind(&alpha, "impl-tide", "codex:t1/t1", person).await).contains(migrate));
    assert_eq!(subject::caller(&alpha, "claude-code:s1/a1", None).await?, None, "it acts as no one meanwhile");
    assert!(ops::principals(&alpha).await?.is_empty(), "nothing was made");
    assert!(subject::bindings_of(&alpha, "claude-code:s1/a2").await?.is_empty(), "nor bound");

    principals::convert(&alpha).await?;
    // A command whose caller was found before the move finished took the agent for a newcomer:
    // naming itself anew, or as a successor, is refused as for the agent it is, and binds nothing.
    let stale = ops::add_principal(
        &alpha,
        "impl-new",
        Some(AgentRole::Implementer),
        None,
        None,
        None,
        Some("claude-code:s1/a1"),
    );
    assert!(refusal(stale.await).contains("which is @impl-tide already"));
    let succeeding =
        ops::add_successor(&alpha, "impl-next", None, None, None, None, Some("claude-code:s1/a1"), "impl-tide");
    assert!(format!("{:#}", succeeding.await.expect_err("refused")).contains("which is @impl-tide already"));
    assert_eq!(subject::bindings_of(&alpha, "claude-code:s1/a1").await?.len(), 1, "bound once");
    assert_eq!(ops::ensure_principal(&alpha, "ines", "Ines", PrincipalKind::Human, None).await?, person);
    assert_eq!(subject::caller(&alpha, "claude-code:s1/a1", None).await?, Some(agent));
    let again = ops::add_principal(&alpha, "impl-tide", None, None, None, Some(agent), Some("claude-code:s1/a1"));
    assert_eq!(again.await?, agent, "it names itself again as itself");
    assert_eq!(ops::principals(&alpha).await?.len(), 2, "and no second was made");
    let new = ops::add_principal(
        &alpha,
        "impl-new",
        Some(AgentRole::Implementer),
        None,
        None,
        None,
        Some("claude-code:s1/a2"),
    );
    let new = new.await?;
    assert_eq!(subject::caller(&alpha, "claude-code:s1/a2", None).await?, Some(new), "a new agent is made and bound");
    Ok(())
}

/// One agent session with a principal in each of two projects, as the build before the move wrote
/// them: the move in one project binds that project's principal alone, and leaves the other
/// project's as it was, until the move runs there too; then the session acts as each project's own.
#[tokio::test]
async fn a_session_with_a_principal_in_two_projects_moves_with_each_project() -> Result<()> {
    let (_store, ctx, alpha) = Store::new().await?;
    let beta = ProjectGraph::ensure(ctx, uuid::Uuid::new_v4(), "beta").await?;
    let session = "claude-code:s1/a1";
    let agent = |graph: &ProjectGraph| before_d261::Principal {
        agent_role: Some(AgentRole::Implementer),
        subject: Some(session.into()),
        ..principal(graph, PrincipalKind::Agent, "impl-tide")
    };
    let ours = written(&alpha, agent(&alpha)).await?;
    let theirs = written(&beta, agent(&beta)).await?;

    principals::convert(&alpha).await?;
    assert_eq!(subject::caller(&alpha, session, None).await?, Some(ours));
    let old: before_d261::PrincipalView = beta.get(theirs).await?;
    assert!(beta.component_of::<RoleView>(&old).await?.is_none(), "the other project's is as it was");
    assert!(subject::bindings_of(&beta, session).await?.is_empty());
    assert_eq!(subject::caller(&beta, session, None).await?, None);

    principals::convert(&beta).await?;
    assert_eq!(subject::caller(&beta, session, None).await?, Some(theirs));
    assert_eq!(subject::caller(&alpha, session, None).await?, Some(ours), "each project's own");
    Ok(())
}
