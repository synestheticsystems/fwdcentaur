//! `fwd admin migrate`, the part that moves a project's principals from fwd's own model onto the
//! Commons' `identity.Principal` (D261), with fwd's [`Role`] in the project beside it, the subjects
//! that act as each as `identity.IdentityBinding` records, and a retired one closed. It runs before
//! the migration finds whom it acts as, since every command finds its caller among the principals
//! moved and their bindings (`fwd`'s `admin migrate`).

use super::moves;
use crate::model::{Migrated, Principal, Role, RoleView};
use crate::project_graph::ProjectGraph;
use crate::subject;
use ankurah::model::Mutable;
use ankurah::property::{PropertyError, Ref};
use ankurah::{EntityId, Model};
use ankurah_commons::property::Timestamp;
use ankurah_commons::records::Closed;
use anyhow::{bail, Result};

/// fwd's own model of a principal, before it moved onto the Commons' (D261): what [`convert`] and
/// the older parts of `fwd admin migrate` read (`ops::migrate_roles`), what [`refuse_unmoved`]
/// looks for, and what `fwd-store-merge` writes from the stores of before the daemon's one store,
/// which a run of the migration then moves. The struct keeps the model's name, `Principal`, by
/// which the node binds it to its definition, and is `no_ffi`, since no binding needs it.
pub mod before_d261 {
    use crate::model::{AgentRole, PrincipalKind, Project};
    use ankurah::{Model, Ref};
    use serde::{Deserialize, Serialize};

    /// A person or an agent identity, someone's part in one project.
    #[derive(Model, Debug, Clone, Serialize, Deserialize)]
    #[model(no_ffi)]
    pub struct Principal {
        pub project: Ref<Project>,
        #[active_type(LWW)]
        pub kind: PrincipalKind,
        #[active_type(LWW)]
        pub handle: String,
        #[active_type(LWW)]
        pub display_name: String,
        /// "github:<numeric id>", captured at `fwd init`.
        #[active_type(LWW)]
        pub auth_subject: Option<String>,
        #[active_type(LWW)]
        pub agent_role: Option<AgentRole>,
        pub active: bool,
        #[active_type(LWW)]
        pub member: Option<bool>,
        /// The Claude Code agent or Codex thread whose commands act as it.
        #[active_type(LWW)]
        pub subject: Option<String>,
        pub succeeded_by: Option<Ref<Principal>>,
        #[active_type(LWW)]
        pub serves: Option<String>,
        #[active_type(LWW)]
        pub last_active: Option<i64>,
    }
}

/// What a binding the conversion made from a principal's `subject` says it came from, in its
/// `Migrated`.
const FROM_SUBJECT: &str = "principal_subject";

/// What a binding the conversion made from a principal's `auth_subject` says it came from.
const FROM_AUTH_SUBJECT: &str = "principal_auth_subject";

/// What a principal retired before the conversion says of its `Closed`, whose time is the
/// conversion's, since fwd never recorded when a principal was retired (the map's doubt 10).
const FROM_ACTIVE: &str = "principal_active";

/// One principal [`convert`] moved: its id, its handle, whether it was retired, and the identity
/// bindings it made for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Converted {
    pub principal: EntityId,
    pub handle: String,
    pub retired: bool,
    pub bindings: Vec<EntityId>,
}

/// What a principal gets in the move, read from fwd's old model.
struct Additions {
    old: before_d261::PrincipalView,
    principal: Principal,
    role: Role,
    retired: bool,
    /// Each subject that acts as it, with the field it was read from.
    subjects: Vec<(String, &'static str)>,
}

/// Refuses to make a principal, or to bind a subject to one, in a project that holds a principal
/// [`convert`] has not moved yet: no read finds one until it has, and a principal made beside it
/// would stay a second one for good once the move keeps both (a person's command finds or makes the
/// person on every start). It runs only where a command is about to make a principal or a binding.
pub async fn refuse_unmoved(graph: &ProjectGraph) -> Result<()> {
    if !unmoved(graph).await?.is_empty() {
        let project = graph.projects().await?.into_iter().find(|p| p.record == graph.record());
        let name = project.map_or_else(|| graph.record().to_base64(), |p| p.name);
        bail!(
            "the project {name} holds principals that `fwd admin migrate` has not moved onto the Commons' models yet, and makes no principal and binds no session until it has: one made now could be a second record of one it holds already. Run `fwd admin migrate --project \"{name}\"`, then this again"
        );
    }
    Ok(())
}

/// The project's principals of fwd's own model that carry no Commons principal yet, selected by
/// the store, so that a project moved reads none of them. A node that never defined that model
/// holds none, and is not made to define it by reading them.
async fn unmoved(graph: &ProjectGraph) -> Result<Vec<before_d261::PrincipalView>> {
    if graph.models_labelled(before_d261::Principal::descriptor().label).await?.is_empty() {
        return Ok(Vec::new());
    }
    let selecting = format!("NOT memberof('{}')", Principal::descriptor().label);
    Ok(graph.fetch(selecting.as_str()).await?)
}

/// Moves the project's principals onto the Commons' `identity.Principal`, adding to each entity
/// and keeping its id, which every record that names the principal holds: its kind, its handle
/// and its display name as they are; fwd's `Role` with its project, its role, whether it belongs,
/// what it serves, who succeeded it and when it last wrote; for its `subject` and its
/// `auth_subject`, where set, an `IdentityBinding` placed in its project, made now by the
/// principal itself, whose own command or sign-in stated the subject; and, where it was retired,
/// `Closed` at the time of the conversion by no one. What the conversion states for the first time
/// carries `Migrated`, naming the field it was read from. A role word outside the fixed set, which
/// every reader reads as none and which `ops::migrate_roles` carries over before this part runs,
/// moves as none. fwd's own model stays on the entity, since Ankurah cannot take a component off
/// one yet. Each principal moves as [`super`] says: one whose kind, handle or any other field
/// cannot be read, or whose entity carries a `Role` already, which the move would write over,
/// refuses the run, and one that carries the Commons' `Principal` has moved.
pub async fn convert(graph: &ProjectGraph) -> Result<Vec<Converted>> {
    let mut checked = Vec::new();
    for old in unmoved(graph).await? {
        let gets = match graph.component_of::<RoleView>(&old).await? {
            Some(_) => Err("it carries `role` already, which the move would write over".to_string()),
            None => additions(graph, &old),
        };
        checked.push((old.id(), gets));
    }
    let now = Timestamp::from_unix_millis(crate::now_ms());
    let mut converted = Vec::with_capacity(checked.len());
    for m in moves("principal", checked)? {
        let id = m.old.id();
        let trx = graph.begin();
        let edited = trx.edit(&m.old)?;
        let record = Mutable::entity(&*edited);
        trx.add(record, &m.role).await?;
        trx.add(record, &m.principal).await?;
        if m.retired {
            trx.add(record, &Closed { closed: Some(now), by: None, outcome: None, reason: None }).await?;
            trx.add(record, &Migrated { migration: FROM_ACTIVE.to_string() }).await?;
        }
        let mut bindings = Vec::with_capacity(m.subjects.len());
        for (s, from) in &m.subjects {
            bindings.push(subject::bind(&trx, id, s, id, Some(from)).await?);
        }
        trx.commit().await?;
        converted.push(Converted { principal: id, handle: m.principal.handle, retired: m.retired, bindings });
    }
    Ok(converted)
}

/// What the principal `old` gets ([`convert`]), or why it cannot move without losing part of what
/// it holds: the first field that cannot be read.
fn additions(graph: &ProjectGraph, old: &before_d261::PrincipalView) -> Result<Additions, String> {
    fn unread(field: &str) -> impl Fn(PropertyError) -> String + '_ {
        move |e| format!("its `{field}` cannot be read: {e}")
    }
    let mut subjects = Vec::new();
    if let Some(s) = old.subject().map_err(unread("subject"))? {
        subjects.push((s, FROM_SUBJECT));
    }
    if let Some(s) = old.auth_subject().map_err(unread("auth_subject"))? {
        subjects.push((s, FROM_AUTH_SUBJECT));
    }
    Ok(Additions {
        principal: Principal {
            kind: old.kind().map_err(unread("kind"))?,
            handle: old.handle().map_err(unread("handle"))?,
            display_name: old.display_name().map_err(unread("display_name"))?,
        },
        role: Role {
            project: graph.project(),
            // A word outside the set reads as none everywhere (`ops::role_of`).
            agent_role: old.agent_role().ok().flatten(),
            member: old.member().map_err(unread("member"))?,
            succeeded_by: old.succeeded_by().map_err(unread("succeeded_by"))?.map(|s| Ref::new(EntityId::from(s))),
            serves: old.serves().map_err(unread("serves"))?,
            last_active: old.last_active().map_err(unread("last_active"))?,
        },
        retired: !old.active().map_err(unread("active"))?,
        subjects,
        old: old.clone(),
    })
}
