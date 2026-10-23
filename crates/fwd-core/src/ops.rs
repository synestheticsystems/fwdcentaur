//! Operations over a project: what the CLI, the daemon's services and tests call. Each
//! operation runs the `rules` pre-checks, then writes one transaction. Who may do what is policy,
//! applied by the caller (docs/design/data-model.md, "Rights"), except where a rule says whose a
//! thing is: only a message's author, or an agent that succeeded its author, edits it
//! (`rules::can_edit_message`), and only a task's creator and its assignee take a step off its
//! checklist (`rules::can_remove_step`).

use crate::model::*;
use crate::naming;
use crate::query::{with_id, with_ids, with_string};
use crate::references::Named;
use crate::rules::{self, DecisionMove, Edge, Readiness, TaskSnapshot, Verb};
use crate::subject;
use ankurah::property::{Json, Ref};
use ankurah::{Context, EntityId};
use anyhow::{anyhow, bail, Context as _, Result};
use serde_json::{json, Value};

fn now() -> i64 {
    crate::now_ms()
}

/// An empty Yrs text has no stored value, which Ankurah reports as `PropertyError::Missing`;
/// for prose fields that means "empty", not "absent".
pub fn text(read: Result<String, ankurah::property::PropertyError>) -> Result<String> {
    match read {
        Ok(s) => Ok(s),
        Err(ankurah::property::PropertyError::Missing) => Ok(String::new()),
        Err(e) => Err(e.into()),
    }
}

// ---------------------------------------------------------------------------------------------
// Principals

/// The words a role is selected by, for the sentences that say how to select one.
pub const ROLE_WORDS: &str = "lead|triage|implementer|reviewer";

/// What a triage agent answers first for, as its principal records it ([`Principal::serves`]), so
/// that each scope's questions have one front line of their own, since one triage agent for a
/// large project would take every question asked of no one in it (docs/DECISIONS.md, D122: "it
/// should be effectively per scope"; docs/design/agents-and-inbox.md, "Roles"): the scopes it
/// serves, and whether it is endorsed to serve the whole project, which takes what no scope's own
/// triage agent does. A scope has at most one triage agent, and a project at most one that serves
/// it whole, among the principals that are not retired ([`one_triage_per_scope`]). Nothing for
/// every other principal, and for a triage agent that serves nothing, which nothing reaches for
/// its role until it takes a scope or is endorsed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Serving {
    /// The scopes it serves, in the order it took them.
    pub scopes: Vec<EntityId>,
    /// Endorsed to serve the whole project (`--project-wide`), which suits only a small project
    /// with small scopes (Daniel, 2026-09-30).
    pub project_wide: bool,
}

impl Serving {
    /// What `p` serves, as its `serves` records it; nothing where it records none, or text that
    /// does not read as what [`Serving::stored`] writes.
    pub fn of(p: &PrincipalView) -> Serving {
        let stored: Value = match p.serves() {
            Ok(Some(text)) => serde_json::from_str(&text).unwrap_or(Value::Null),
            _ => return Serving::default(),
        };
        let scopes = stored["scopes"].as_array().into_iter().flatten();
        Serving {
            scopes: scopes.filter_map(|s| s.as_str()?.parse().ok()).collect(),
            project_wide: stored["project_wide"].as_bool() == Some(true),
        }
    }

    /// Whether it serves nothing: no scope, and not the whole project.
    pub fn is_nothing(&self) -> bool {
        self.scopes.is_empty() && !self.project_wide
    }

    /// Whether it serves `scope` as that scope's own triage agent.
    pub fn serves_scope(&self, scope: EntityId) -> bool {
        self.scopes.contains(&scope)
    }

    /// What `serves` records for it, as JSON text: none for nothing.
    fn stored(&self) -> Option<String> {
        if self.is_nothing() {
            return None;
        }
        let mut v = json!({});
        if !self.scopes.is_empty() {
            v["scopes"] = json!(self.scopes.iter().map(|s| s.to_base64()).collect::<Vec<_>>());
        }
        if self.project_wide {
            v["project_wide"] = json!(true);
        }
        Some(v.to_string())
    }

    /// It with `what` added: a scope it does not serve yet, or the whole project.
    fn with(mut self, what: Serves) -> Serving {
        match what {
            Serves::Scope(scope) | Serves::Found(scope) if !self.scopes.contains(&scope) => self.scopes.push(scope),
            Serves::Scope(_) | Serves::Found(_) => {}
            Serves::ProjectWide => self.project_wide = true,
        }
        self
    }

    /// It with `what` taken away: a scope, or the endorsement to serve the whole project.
    fn without(mut self, what: Serves) -> Serving {
        match what {
            Serves::Scope(scope) | Serves::Found(scope) => self.scopes.retain(|s| *s != scope),
            Serves::ProjectWide => self.project_wide = false,
        }
        self
    }

    /// It and `other` together, as a successor that held triage already carries on its own job and
    /// its predecessor's: the scopes of both, and the whole project where either serves it.
    fn and(self, other: Serving) -> Serving {
        let project_wide = other.project_wide;
        let mut both = other.scopes.into_iter().fold(self, |both, scope| both.with(Serves::Scope(scope)));
        both.project_wide |= project_wide;
        both
    }
}

/// What a command gives a triage agent to serve, as it takes triage or later (`fwd principal
/// scope`), so that a triage agent never answers for the whole project by default (Daniel,
/// 2026-09-30, on D122: "Something like ankurah for instance is a MASSIVE project, and having one
/// triage principle for that would be terrible").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Serves {
    /// A scope the command names (`--scope <id>` typed on it, or `fwd principal scope add`).
    Scope(EntityId),
    /// The scope the command's checkout finds, or FWD_SCOPE names, which an agent taking triage
    /// serves only where it would serve nothing else: one that serves scopes already takes more
    /// by naming them, so that running `fwd principal add` again in another checkout, to come back
    /// or to change its display name, adds nothing it did not ask for.
    Found(EntityId),
    /// The whole project, by a deliberate endorsement (`--project-wide`), which suits only a small
    /// project with small scopes.
    ProjectWide,
}

/// The triage agent that answers first for something, and as what ([`front_line`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrontLine {
    pub agent: EntityId,
    /// The scope it serves, where it answers as that scope's own triage agent; none where it
    /// answers as the one that serves the whole project.
    pub scope: Option<EntityId>,
}

/// Of `principals`, the triage agent that answers first for `scope`, or for what concerns no scope
/// when `scope` is none (docs/design/agents-and-inbox.md, "Roles"): the agent that serves the scope,
/// else the one endorsed to serve the whole project; none where neither exists, when a question
/// waits for one and the rest reaches no triage agent. Should two serve one scope, or two the whole
/// project (two clients binding at once), the first by handle, then by id, so that every reader of
/// the graph picks the same one, the page as the daemon.
pub fn front_line(principals: &[PrincipalView], scope: Option<EntityId>) -> Option<FrontLine> {
    let first = |serves: &dyn Fn(&Serving) -> bool| {
        principals
            .iter()
            .filter(|p| holds(p, AgentRole::Triage) && serves(&Serving::of(p)))
            .map(|p| (p.handle().unwrap_or_default(), p.id()))
            .min()
            .map(|(_, id)| id)
    };
    if let Some(agent) = scope.and_then(|scope| first(&|s: &Serving| s.serves_scope(scope))) {
        return Some(FrontLine { agent, scope });
    }
    first(&|s: &Serving| s.project_wide).map(|agent| FrontLine { agent, scope: None })
}

/// Of `principals`, the triage agents that answer first for what concerns `scopes`: each scope's
/// own, else the one that serves the whole project ([`front_line`]), each agent once, in the order
/// of the scopes; for what concerns no scope, the one that serves the whole project.
pub fn front_lines(principals: &[PrincipalView], scopes: &[EntityId]) -> Vec<FrontLine> {
    let each: Vec<Option<EntityId>> =
        if scopes.is_empty() { vec![None] } else { scopes.iter().copied().map(Some).collect() };
    let mut found: Vec<FrontLine> = Vec::new();
    for scope in each {
        if let Some(f) = front_line(principals, scope).filter(|f| found.iter().all(|g| g.agent != f.agent)) {
            found.push(f);
        }
    }
    found
}

/// The triage agent that answers first for `scope` ([`front_line`]), read from the graph.
pub async fn triage_for(ctx: &Context, scope: Option<EntityId>) -> Result<Option<FrontLine>> {
    Ok(front_line(&holding(ctx, AgentRole::Triage).await?, scope))
}

/// The triage agents that answer first for what concerns `scopes`, such as the scopes a task is
/// worked in ([`front_lines`]), read from the graph: each scope's own, else the one that serves
/// the whole project, each agent once; for no scope, the one that serves the whole project.
pub async fn triage_for_scopes(ctx: &Context, scopes: &[EntityId]) -> Result<Vec<FrontLine>> {
    Ok(front_lines(&holding(ctx, AgentRole::Triage).await?, scopes))
}

/// What `p` answers first for as a triage agent: what it serves while it holds triage and is not
/// retired ([`holds`]), none otherwise, so that the routing service tells a change of who answers
/// first for something from a change that leaves it as it was.
pub fn triage_standing(p: &PrincipalView) -> Option<Serving> {
    holds(p, AgentRole::Triage).then(|| Serving::of(p))
}

/// Whether `p` holds triage and serves nothing, as the triage agents of before 2026-09-30 do until
/// their leads bind or endorse them, so that nothing reaches it for its role and fwd tells it so on
/// each of its commands (`guidance::unbound`).
pub fn serves_nothing_yet(p: &PrincipalView) -> bool {
    triage_standing(p).is_some_and(|s| s.is_nothing())
}

/// The sentence that refuses triage to the agent named `handle`, which would serve nothing: a
/// triage agent serves a scope, or by a deliberate endorsement the whole project, and never the
/// whole project by default (Daniel, 2026-09-30, on D122).
pub fn serves_nothing(handle: &str) -> String {
    format!(
        "an agent that selects triage serves a scope, and `@{handle}` would serve none, since the command names none and its checkout finds none, or several: name one, `--scope <id>`, or run the command in a checkout of that scope (bound with `fwd scope attach-dir <id>`, or on a branch it alone names); or endorse it to serve the whole project, `--project-wide`, which suits only a small project with small scopes"
    )
}

/// Finds the principal of `kind` that has the name `handle` (an agent that shares a person's name
/// is not that person), creating it when none has. A principal made here does not belong to the
/// project yet (`member` false): a command that writes work into the project makes it belong
/// ([`join`]), and one that only reads leaves it outside, so that a session that only looked at a
/// project's graph is offered by none of its pickers (docs/design/agents-and-inbox.md, "Who belongs
/// to a project"). The CLI makes only the person this way, by the handle `identity.toml` keeps; an
/// agent names itself with [`add_principal`]. Two or more of `kind` sharing the name are refused,
/// listed, as a typed name is ([`naming::refused`]).
pub async fn ensure_principal(
    ctx: &Context,
    handle: &str,
    display_name: &str,
    kind: PrincipalKind,
    role: Option<AgentRole>,
) -> Result<EntityId> {
    let found = principals(ctx).await?;
    match naming::by_name(&found, handle, Some(kind)).as_slice() {
        [] => {}
        [one] => return Ok(one.id()),
        several => return Err(naming::refused(ctx, handle, several, false).await?.into()),
    }
    let trx = ctx.begin();
    let id = trx
        .create(&Principal {
            kind,
            handle: handle.to_string(),
            display_name: display_name.to_string(),
            auth_subject: None,
            agent_role: role,
            active: true,
            member: Some(false),
            subject: None,
            succeeded_by: None,
            serves: None,
            last_active: None,
        })
        .await?
        .id();
    trx.commit().await?;
    Ok(id)
}

/// Every principal the project's graph holds, whether it belongs or not.
pub async fn principals(ctx: &Context) -> Result<Vec<PrincipalView>> {
    Ok(ctx.fetch("kind = 'human' OR kind = 'agent'").await?)
}

/// Whether a picker offers `p`: it is active (not retired) and belongs to the project, or was
/// recorded before membership was (2026-09-29), which belongs while it is active until `fwd admin
/// migrate` records whether it does (docs/design/agents-and-inbox.md, "Who belongs to a project").
pub fn belongs(p: &PrincipalView) -> bool {
    p.active().unwrap_or(false) && p.member().ok().flatten() != Some(false)
}

/// The role `p` holds, when it holds one this build reads: none for a person, for an agent that has
/// not selected one, and for a stored word outside the set (`supervisor`, before `fwd admin
/// migrate`).
pub fn role_of(p: &PrincipalView) -> Option<AgentRole> {
    p.agent_role().ok().flatten()
}

/// Whether `p` is an agent that holds no role, whose writes are refused until it selects one.
pub fn lacks_role(p: &PrincipalView) -> Result<bool> {
    Ok(p.kind()? == PrincipalKind::Agent && role_of(p).is_none())
}

/// The command an agent named `handle` selects a role with, for the sentences that refuse it.
pub fn select_role_command(handle: &str) -> String {
    format!("`fwd principal role {handle} {ROLE_WORDS}`")
}

/// Whether `p` holds `role` so that the role counts: it selected the role and is not retired.
pub fn holds(p: &PrincipalView, role: AgentRole) -> bool {
    // `active` is read here rather than in a predicate: a boolean literal in a predicate did not
    // match entities that arrived over the wire (Ankurah 0.9.3).
    role_of(p) == Some(role) && p.active().unwrap_or(false)
}

/// The principals the graph holds that selected `role`, retired or not.
async fn holding(ctx: &Context, role: AgentRole) -> Result<Vec<PrincipalView>> {
    Ok(ctx.fetch(format!("agent_role = '{role}'").as_str()).await?)
}

/// Refuses `serving`, what the principal named `handle` is to serve as a triage agent, where
/// another principal that is not retired holds triage and serves one of its scopes, or serves the
/// whole project when `serving` is to: a scope has one triage agent, and a project one that serves
/// it whole (docs/DECISIONS.md, D122), so that each scope's questions have one front line. The
/// sentence names the holder and the commands that free the scope, or withdraw the endorsement, or
/// retire it. `besides` never count: the principal itself, and the predecessor a succession
/// retires, whose scopes its successor takes.
async fn one_triage_per_scope(ctx: &Context, serving: &Serving, handle: &str, besides: &[EntityId]) -> Result<()> {
    let others: Vec<(String, EntityId, Serving)> = holding(ctx, AgentRole::Triage)
        .await?
        .iter()
        .filter(|p| holds(p, AgentRole::Triage) && !besides.contains(&p.id()))
        .map(|p| (p.handle().unwrap_or_default(), p.id(), Serving::of(p)))
        .collect();
    let first = |serves: &dyn Fn(&Serving) -> bool| {
        others.iter().filter(|(_, _, s)| serves(s)).map(|(handle, id, _)| (handle.clone(), *id)).min()
    };
    for &scope in &serving.scopes {
        if let Some((held, _)) = first(&|s: &Serving| s.serves_scope(scope)) {
            bail!(
                "the scope {} has a triage agent already, @{held}, and a scope has one: free the scope first (`fwd principal scope remove {} --for {held}`), or retire @{held} (`fwd principal retire {held}`), for `@{handle}` to serve it",
                scope_named(ctx, scope).await,
                short_id(scope)
            );
        }
    }
    if serving.project_wide {
        if let Some((held, _)) = first(&|s: &Serving| s.project_wide) {
            bail!(
                "@{held} serves the whole project already, and a project has one triage agent that serves it whole: withdraw its endorsement first (`fwd principal scope project-wide --withdraw --for {held}`), or retire it (`fwd principal retire {held}`), for `@{handle}` to serve the whole project"
            );
        }
    }
    Ok(())
}

/// What the principal `p` (none for one being made) is to serve once it holds the role it takes
/// (`taking`, where the command gives or brings back a role, else the role it holds): as a triage
/// agent, what it served as one already, with `serves` added, the scope its checkout finds only
/// where it would serve nothing else ([`Serves::Found`]); in any other role, nothing, since what an
/// agent serves is its triage's, and selecting another role frees it. A scope for an agent that
/// will not hold triage is where its session works, not something to serve, and is left alone;
/// `--project-wide` for one is refused. Refused too where the principal takes triage it did not hold
/// and would serve nothing ([`serves_nothing`]), so that no agent is made or given triage that
/// nothing would reach; one that holds triage already and serves nothing, as the triage agents of
/// before 2026-09-30 do until their leads bind them, stays so.
fn serving_after(
    p: Option<&PrincipalView>,
    taking: Option<AgentRole>,
    serves: Option<Serves>,
    handle: &str,
) -> Result<Serving> {
    let held = p.and_then(role_of);
    let role = taking.or(held);
    if role != Some(AgentRole::Triage) {
        if serves == Some(Serves::ProjectWide) {
            let holding = role.map_or("no role".to_string(), |r| r.to_string());
            bail!(
                "only a triage agent serves the whole project, and `@{handle}` would hold {holding}: select triage with it, `--role triage --project-wide`"
            );
        }
        return Ok(Serving::default());
    }
    let kept = match (held, p) {
        (Some(AgentRole::Triage), Some(p)) => Serving::of(p),
        _ => Serving::default(),
    };
    let serving = match serves {
        Some(Serves::Found(_)) if !kept.is_nothing() => kept,
        Some(what) => kept.with(what),
        None => kept,
    };
    if serving.is_nothing() && held != Some(AgentRole::Triage) {
        bail!(serves_nothing(handle));
    }
    Ok(serving)
}

/// Refuses lead, when `role` is lead, for the principal named `handle`, which is bound to `subject`
/// or is being bound to it, while another principal that is not retired holds lead and is bound to
/// an agent of the same Claude Code session: a session has one lead, its own thread, and the agents
/// a lead dispatches select implementer, reviewer or triage, while a project has as many leads as
/// sessions coordinate its work (Daniel, 2026-09-30: "it's one lead per worktree/top level agent
/// session, not per project"; docs/DECISIONS.md, D119; docs/design/agents-and-inbox.md, "Roles").
/// `besides` never count: the principal itself, and the predecessor a succession retires, whose
/// successor takes its lead as before. A principal with no subject, made where the fwd plugin
/// stamped no command (by a script, a typed `--as`, another harness's agent), is not checked, since
/// fwd cannot tell its session from another's.
async fn one_lead_per_session(
    ctx: &Context,
    role: Option<AgentRole>,
    subject: Option<&str>,
    handle: &str,
    besides: &[EntityId],
) -> Result<()> {
    let (Some(AgentRole::Lead), Some(subject)) = (role, subject.filter(|s| subject::session_of(s).is_some())) else {
        return Ok(());
    };
    let leads = holding(ctx, AgentRole::Lead).await?;
    let held = subject::of_same_session(&leads, subject)
        .into_iter()
        .filter(|p| holds(p, AgentRole::Lead) && !besides.contains(&p.id()))
        .map(|p| (p.handle().unwrap_or_default(), p.id()))
        .min();
    if let Some((held, _)) = held {
        bail!(
            "{} has a lead already, @{held}, and a session has one lead, its own thread: an agent a lead dispatches selects implementer, reviewer or triage instead; `@{handle}` may hold lead in this session once @{held} holds another role or is retired",
            subject::session_words(subject)
        );
    }
    Ok(())
}

/// Makes `principal` belong to the project, for a command that writes work into it or for the
/// person `fwd init` records (docs/design/agents-and-inbox.md, "Who belongs to a project"): true when
/// it did not yet say so. A retired principal stays retired, since only `fwd principal add`, run as
/// that principal, brings one back ([`add_principal`]).
pub async fn join(ctx: &Context, principal: EntityId) -> Result<bool> {
    let p: PrincipalView = ctx.get(principal).await?;
    if !p.active()? || p.member()? == Some(true) {
        return Ok(false);
    }
    let trx = ctx.begin();
    let m = p.edit(&trx)?;
    m.member()?.set(&Some(true))?;
    trx.commit().await?;
    Ok(true)
}

/// `fwd principal add`: how an agent names itself and selects its role, `role`, which a new agent
/// must give (docs/design/agents-and-inbox.md, "Roles"). The principal belongs to the project from
/// then on. `name` is read as every typed name is ([`naming::principal_named`]): a name none has
/// makes a new agent, and a name two or more principals share is refused, listing them. A name one
/// principal has already is that principal's alone: the command names it only for a caller that
/// acts as it, `acting` (the principal bound to the command's subject; without one, the principal
/// `--as` or `FWD_AS` names, else the person the CLI acts as), bringing it back if it was retired
/// and giving it `role` when one is given; any other caller is refused, told whose the name is and
/// to choose one no other principal of the project has, so that a second agent never becomes the
/// first agent's user by choosing its name (docs/DECISIONS.md, D110 and D112). A person is never
/// made here, since a person's name comes with their own `fwd init`, and a person takes no role.
///
/// An agent that takes triage serves what `serves` gives as it does (docs/DECISIONS.md, D122): the
/// scope `--scope` names, else the one its checkout finds, or the whole project by a deliberate
/// endorsement (`--project-wide`); one that held triage already keeps what it served, and adds a
/// scope only where the command names it ([`serving_after`]). Triage is refused to an agent that
/// would serve nothing, and where another principal that is not retired serves one of its scopes,
/// or the whole project when it is to ([`one_triage_per_scope`]); lead where another that is not
/// retired holds it bound to an agent of the same Claude Code session ([`one_lead_per_session`]);
/// each naming that principal: for the role given, or, for a retired principal brought back without
/// one, the role it held and what it served, which count again once it is back. Another role given
/// to a triage agent frees what it served.
///
/// `subject` is the Claude Code agent that runs the command, when the fwd plugin stamped it
/// ([`subject`]; docs/design/agents-and-inbox.md, "An agent's identity"): the agent the command
/// makes is bound to it, so that the agent's later commands act as it. A subject binds one
/// principal, so an agent bound already, which `acting` then is, is refused a new name.
pub async fn add_principal(
    ctx: &Context,
    name: &str,
    role: Option<AgentRole>,
    serves: Option<Serves>,
    display_name: Option<&str>,
    acting: Option<EntityId>,
    subject: Option<&str>,
) -> Result<EntityId> {
    let name = naming::bare(name);
    if name.is_empty() {
        bail!("a name cannot be blank");
    }
    let existing = naming::principal_named(ctx, name, None).await?;
    if let Some(p) = existing.as_ref().filter(|p| acting != Some(p.id())) {
        let handle = p.handle()?;
        // Where the command carries a subject, a principal acts as itself by being bound to it, and
        // an agent from before the plugin is taken over by binding it, not by naming it again.
        let (acts_as_itself, bind) = match (subject, acting, p.kind()?, p.subject().ok().flatten()) {
            (Some(_), None, PrincipalKind::Agent, None) => (
                " (from the Claude Code agent bound to it)",
                format!(
                    "; if it is yours from before the fwd plugin, take it over, once: `fwd --as {handle} principal bind {handle}`"
                ),
            ),
            (Some(_), ..) => (" (from the Claude Code agent bound to it)", String::new()),
            (None, ..) => (" (`--as` or FWD_AS naming it)", String::new()),
        };
        bail!(
            "the name `{handle}` is taken: {} has it in this project, and only that principal, acting as itself{acts_as_itself}, adds itself again, to come back or to change its role; choose a name no other principal of this project has, as every agent does (a role and a qualifier reads well: `implementer-replication`; docs/DECISIONS.md, D110){bind}",
            naming::described(ctx, p).await?
        );
    }
    if let (Some(subject), None, Some(bound)) = (subject, &existing, acting) {
        let bound = at_handle(ctx, bound).await;
        let bound = bound.trim_start_matches('@');
        bail!(
            "this command runs as {}, which is @{bound} already, and a subject binds one principal, so an agent names itself once: to select another role, `fwd principal role {bound} <role>`; an agent relaunched to carry on another's job names itself once and succeeds it, `fwd principal succeed <its name>`",
            subject::words(subject)
        );
    }
    // The name the sentences give: the principal's own, when the command named it by its id.
    let handle = match &existing {
        Some(p) => p.handle()?,
        None => name.to_string(),
    };
    match &existing {
        Some(p) if p.kind()? == PrincipalKind::Human => {
            if role.is_some() {
                bail!("`@{handle}` is a person, and a role is an agent's: a person belongs to the project without one");
            }
        }
        Some(p) => {
            if role.is_none() && role_of(p).is_none() {
                bail!(
                    "`@{handle}` holds no role, and every agent selects one: `fwd principal add {handle} --role {ROLE_WORDS}`"
                );
            }
        }
        None => {
            if role.is_none() {
                bail!("an agent selects its role as it names itself: `fwd principal add {handle} --role {ROLE_WORDS}`");
            }
        }
    }
    // A retired principal brought back holds its stored role again, and serves what it served,
    // which count toward the triage agent of each scope and the one lead of its session as a role
    // given now does. Its session is that of the subject it is bound to, or, for an agent made now,
    // of the command's.
    let coming_back = existing.as_ref().filter(|p| !p.active().unwrap_or(false));
    let taking = role.or_else(|| coming_back.and_then(role_of));
    let serving = serving_after(existing.as_ref(), taking, serves, &handle)?;
    let itself: Vec<EntityId> = existing.iter().map(|p| p.id()).collect();
    one_triage_per_scope(ctx, &serving, &handle, &itself).await?;
    let bound = match &existing {
        Some(p) => p.subject().ok().flatten(),
        None => subject.map(str::to_string),
    };
    one_lead_per_session(ctx, taking, bound.as_deref(), &handle, &itself).await?;
    let trx = ctx.begin();
    let id = match existing {
        Some(p) => {
            let m = p.edit(&trx)?;
            m.active()?.set(&true)?;
            m.member()?.set(&Some(true))?;
            // Back, it hears what reaches it again, rather than its successor.
            if p.succeeded_by()?.is_some() {
                m.succeeded_by()?.set(&None)?;
            }
            if let Some(role) = role {
                m.agent_role()?.set(&Some(role))?;
            }
            if serving != Serving::of(&p) {
                m.serves()?.set(&serving.stored())?;
            }
            if let Some(name) = display_name {
                m.display_name()?.set(&name.to_string())?;
            }
            p.id()
        }
        None => trx
            .create(&Principal {
                kind: PrincipalKind::Agent,
                handle: handle.clone(),
                display_name: display_name.unwrap_or(&handle).to_string(),
                auth_subject: None,
                agent_role: role,
                active: true,
                member: Some(true),
                subject: subject.map(str::to_string),
                succeeded_by: None,
                serves: serving.stored(),
                // Naming itself is the agent's first write. The command that names it acts as
                // another principal, or as none, so the stamp that follows each command that
                // writes (`dormancy::wrote`) never reaches the agent it makes.
                last_active: Some(now()),
            })
            .await?
            .id(),
    };
    trx.commit().await?;
    Ok(id)
}

/// `fwd principal bind`: the Claude Code agent `subject` takes over the principal `name` names, a
/// principal from before the fwd plugin that no agent is bound to (`claude`, a pilot's lead), so
/// that the agent's commands act as it from then on (docs/design/agents-and-inbox.md, "An agent's
/// identity"). It happens once and deliberately, the agent saying whom it takes over twice: only
/// when the `--as` typed on the command line names the same principal (`named_by_as`). Refused for
/// a name no principal has, and for a subject bound already, naming its principal; for a person,
/// who acts from their own terminal; for a principal a successor took over, naming the successor;
/// for a principal another agent's subject is bound to, which an agent relaunched to carry on its
/// job succeeds instead; and for a lead, while another lead is bound to an agent of the subject's
/// session ([`one_lead_per_session`]). It sets nothing but the subject: a retired principal stays
/// retired until `fwd principal add`, run as it, brings it back.
pub async fn bind_principal(
    ctx: &Context,
    name: &str,
    subject: &str,
    named_by_as: Option<EntityId>,
) -> Result<EntityId> {
    let name = naming::bare(name);
    let Some(p) = naming::principal_named(ctx, name, None).await? else {
        bail!(
            "no principal of this project is named `{name}`, so there is none to take over: an agent new to the project names itself, `fwd principal add {name} --role {ROLE_WORDS}`"
        );
    };
    let handle = p.handle()?;
    if let Some(bound) = subject::principal_of(ctx, subject).await? {
        bail!(
            "this command runs as {}, which is @{} already, and a subject binds one principal",
            subject::words(subject),
            bound.handle()?
        );
    }
    if named_by_as != Some(p.id()) {
        bail!(
            "binding makes this Claude Code agent act as @{handle} from now on, so it is said twice: `fwd --as {handle} principal bind {handle}`"
        );
    }
    if p.kind()? == PrincipalKind::Human {
        bail!(
            "`@{handle}` is a person, and a person acts from their own terminal, never as a Claude Code agent: an agent names itself, `fwd principal add <name> --role {ROLE_WORDS}`"
        );
    }
    if let Some(successor) = p.succeeded_by()?.map(EntityId::from) {
        let successor = at_handle(ctx, successor).await;
        bail!(
            "`@{handle}` was succeeded by {successor}, who carries on its job: an agent relaunched to carry it on names itself as the successor of {successor}, `fwd principal add <name> --succeeds {}`",
            successor.trim_start_matches('@')
        );
    }
    if let Some(other) = p.subject().ok().flatten() {
        bail!(
            "`@{handle}` is bound to another Claude Code agent ({other}), and a principal is bound to one: an agent relaunched to carry on its job names itself as its successor, `fwd principal add <name> --succeeds {handle}`"
        );
    }
    // A lead bound here counts toward the one lead of the subject's session; a retired one counts
    // once `fwd principal add` brings it back, which checks it then.
    let counted = holds(&p, AgentRole::Lead).then_some(AgentRole::Lead);
    one_lead_per_session(ctx, counted, Some(subject), &handle, &[p.id()]).await?;
    let trx = ctx.begin();
    let m = p.edit(&trx)?;
    m.subject()?.set(&Some(subject.to_string()))?;
    trx.commit().await?;
    Ok(p.id())
}

/// `fwd admin bind`: the person at this machine binds the lead `name` names to `subject`, a Codex
/// thread's (`subject::codex`), so that the calls that thread makes to the daemon's MCP endpoint
/// act as that lead from then on. It is the one, narrow way to carry a lead from before automatic
/// attachment, which worked by `--as` and is bound to no thread (lead-codex), over to the thread that
/// does its job now, keeping its name, its history and its inbox; an agent never chooses another
/// principal to act as. Refused for a subject that is not a Codex thread's, for a name no principal
/// has, for anyone but an agent that holds lead, is not retired and was not succeeded, for one bound
/// to a subject already, and for a subject a principal that is not retired is bound to (the one the
/// thread was made as on its first call that needed one, or named itself as), naming it: nothing is
/// retired or replaced here. The person retires that one first if it is not the thread's; a retired
/// principal bound to the subject is unbound as the lead is bound, its work staying attributed to it. Lead is refused while another lead is bound to a thread of
/// the subject's session ([`one_lead_per_session`]).
pub async fn admin_bind(ctx: &Context, name: &str, subject: &str) -> Result<EntityId> {
    if !subject.starts_with("codex:") || subject::session_of(subject).is_none() {
        bail!(
            "`{subject}` is not a Codex thread's subject, `codex:<session id>/<thread id>`, which `whoami` over the daemon's MCP endpoint gives the thread that calls it; a Claude Code agent takes over a principal from before the plugin itself, with `fwd --as <name> principal bind <name>`"
        );
    }
    let name = naming::bare(name);
    let Some(p) = naming::principal_named(ctx, name, None).await? else {
        bail!("no principal of this project is named `{name}`, so there is none to bind");
    };
    let handle = p.handle()?;
    if !holds(&p, AgentRole::Lead) {
        bail!(
            "`@{handle}` is not a lead that is at work (an agent that holds lead and is not retired): `fwd admin bind` carries only a lead from before automatic attachment over to its thread; any other agent names itself from its thread, `principal_add <name> --role <role>`, or succeeds another, `principal_add <name> --succeeds <its name>`"
        );
    }
    if let Some(successor) = p.succeeded_by()?.map(EntityId::from) {
        bail!("`@{handle}` was succeeded by {}, who carries on its job", at_handle(ctx, successor).await);
    }
    match p.subject().ok().flatten() {
        Some(bound) if bound == subject => return Ok(p.id()),
        Some(other) => {
            bail!("`@{handle}` is bound to {} already, and a principal is bound to one", subject::words(&other))
        }
        None => {}
    }
    // Every principal bound to the subject: one at work refuses the bind; a retired one, such as
    // the stub a thread was made as on its first call and the person then retired, is unbound in
    // the same transaction, so that the subject names the lead alone afterwards.
    let held: Vec<PrincipalView> = principals(ctx)
        .await?
        .into_iter()
        .filter(|q| q.id() != p.id() && q.subject().ok().flatten().as_deref() == Some(subject))
        .collect();
    if let Some(q) = held.iter().find(|q| q.active().unwrap_or(false)) {
        bail!(
            "{} is @{} already, and a subject binds one principal: nothing was bound; retire @{} first if it is not the thread's (`fwd principal retire {}`), then bind again",
            subject::words(subject),
            q.handle()?,
            q.handle()?,
            q.id().to_base64()
        );
    }
    one_lead_per_session(ctx, Some(AgentRole::Lead), Some(subject), &handle, &[p.id()]).await?;
    let trx = ctx.begin();
    for q in &held {
        q.edit(&trx)?.subject()?.set(&None)?;
    }
    p.edit(&trx)?.subject()?.set(&Some(subject.to_string()))?;
    trx.commit().await?;
    Ok(p.id())
}

/// What `fwd principal succeed`, or `fwd principal add --succeeds`, did: the principal that carries
/// on the job, the one it succeeded, the role that moved with the job, and how many unread
/// notifications moved to the successor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Succession {
    pub successor: EntityId,
    pub old: EntityId,
    pub role: Option<AgentRole>,
    pub moved: usize,
}

/// `fwd principal succeed`: `successor`, the principal the command acts as, takes over the job of
/// the principal `name` names, as an agent relaunched to carry on another's does (a lead after a
/// handoff, the triage agent after a session limit) with a user of its own, so that the graph keeps
/// which agent did what and nothing waits in an inbox no agent reads (docs/DECISIONS.md, D114;
/// docs/design/agents-and-inbox.md, "An agent's identity"). In one transaction: the old principal
/// is retired with the successor recorded on it (`succeeded_by`), so that routing delivers to the
/// successor whatever would reach it (`routing::Successors`); its role moves to the successor,
/// replacing the successor's own, and with triage the scopes it served, besides those the successor
/// served as a triage agent itself ([`heir_serving`]), so that each scope's one triage agent, and a
/// session's one lead, are counted on the successor; and its unread notifications move to the
/// successor's inbox, each recording whom it was for (`for_id`). Refused for a successor that is a
/// person, is retired, or is the old principal itself; for an old principal that is a person, or
/// that a successor took over already, naming it; where another principal that is not retired
/// serves a scope the successor would serve, or the whole project when it would
/// ([`one_triage_per_scope`]); and where the role is lead and another lead is bound to an agent of
/// the successor's session ([`one_lead_per_session`]). An agent not yet named names itself and
/// succeeds in one step instead ([`add_successor`]).
pub async fn succeed_principal(ctx: &Context, name: &str, successor: EntityId) -> Result<Succession> {
    let heir: PrincipalView = ctx.get(successor).await?;
    let heir_handle = heir.handle()?;
    if heir.kind()? == PrincipalKind::Human {
        bail!(
            "`@{heir_handle}` is a person, and a person succeeds no agent: an agent relaunched to carry on another's job names itself as its successor, `fwd principal add <name> --succeeds <its name>`"
        );
    }
    if !heir.active()? {
        bail!(
            "`@{heir_handle}` is retired, and a successor carries on a job: an agent relaunched to carry it on names itself anew, as its successor"
        );
    }
    let old = predecessor(ctx, name, Some(successor), |before| format!("`fwd principal succeed {before}`")).await?;
    let role = role_of(&old);
    // Its own role stays where the old principal held none.
    let serving = heir_serving(Some(&heir), &old, role.or_else(|| role_of(&heir)), None, &heir_handle)?;
    one_triage_per_scope(ctx, &serving, &heir_handle, &[old.id(), successor]).await?;
    one_lead_per_session(ctx, role, heir.subject().ok().flatten().as_deref(), &heir_handle, &[old.id(), successor])
        .await?;
    let unread = unread_of(ctx, &old).await?;
    let trx = ctx.begin();
    let m = heir.edit(&trx)?;
    if role.is_some() {
        m.agent_role()?.set(&role)?;
    }
    if serving != Serving::of(&heir) {
        m.serves()?.set(&serving.stored())?;
    }
    hand_over(&trx, &old, successor, &unread)?;
    trx.commit().await?;
    Ok(Succession { successor, old: old.id(), role, moved: unread.len() })
}

/// What the successor `heir` (none for one being made) of `old` serves once it holds `role`: as a
/// triage agent, the scopes `old` served, and the whole project where `old` served it, besides what
/// it served itself as one, with `serves` added as [`serving_after`] adds it (the scope a command's
/// checkout finds only where it would serve nothing else), so that succession moves a triage
/// agent's scopes with its role; in any other role, nothing. Refused where the successor takes
/// triage that neither it nor `old` held and would serve nothing, as an agent taking triage anew is
/// ([`serves_nothing`]); the successor of a triage agent that serves nothing, as the triage agents of
/// before 2026-09-30 do until their leads bind them, serves nothing too.
fn heir_serving(
    heir: Option<&PrincipalView>,
    old: &PrincipalView,
    role: Option<AgentRole>,
    serves: Option<Serves>,
    handle: &str,
) -> Result<Serving> {
    if role != Some(AgentRole::Triage) {
        if serves == Some(Serves::ProjectWide) {
            bail!(
                "only a triage agent serves the whole project, and `@{handle}` would hold {}: `--project-wide` goes with triage",
                role.map_or("no role".to_string(), |r| r.to_string())
            );
        }
        return Ok(Serving::default());
    }
    let triage_of = |p: &PrincipalView| (role_of(p) == Some(AgentRole::Triage)).then(|| Serving::of(p));
    let own = heir.and_then(triage_of);
    let carried = triage_of(old);
    let held = own.is_some() || carried.is_some();
    let kept = own.unwrap_or_default().and(carried.unwrap_or_default());
    let serving = match serves {
        Some(Serves::Found(_)) if !kept.is_nothing() => kept,
        Some(what) => kept.with(what),
        None => kept,
    };
    if serving.is_nothing() && !held {
        bail!(serves_nothing(handle));
    }
    Ok(serving)
}

/// `fwd principal add <name> --succeeds <old>`: an agent relaunched to carry on another's job names
/// itself and succeeds it in one step (docs/DECISIONS.md, D114), holding the role its predecessor
/// held, and with triage the scopes it served, so that a lead or a triage agent needs no other role
/// in between: each scope's one triage agent, and a session's one lead, are counted on the
/// successor, never on the predecessor it retires.
/// It makes the agent `name`, bound to `subject` when the command carries one, and hands `old`'s
/// job to it as [`succeed_principal`] does, in one transaction. `role`, when given, must be the
/// role `old` holds, and is the successor's when `old` holds none; a successor that takes triage so,
/// anew, serves what `serves` gives ([`heir_serving`]). Refused for a name a principal has already
/// (the caller's own, which succeeds with `fwd principal succeed`, or another's), for an agent
/// bound already, since a subject binds one principal, and as [`succeed_principal`] refuses.
#[allow(clippy::too_many_arguments)]
pub async fn add_successor(
    ctx: &Context,
    name: &str,
    role: Option<AgentRole>,
    serves: Option<Serves>,
    display_name: Option<&str>,
    acting: Option<EntityId>,
    subject: Option<&str>,
    old: &str,
) -> Result<Succession> {
    let name = naming::bare(name);
    if name.is_empty() {
        bail!("a name cannot be blank");
    }
    if let Some(p) = naming::principal_named(ctx, name, None).await? {
        let handle = p.handle()?;
        if acting == Some(p.id()) {
            bail!(
                "`@{handle}` is a principal already, the one this command acts as: it carries on another's job with `fwd principal succeed {}`",
                naming::bare(old)
            );
        }
        bail!(
            "the name `{handle}` is taken: {} has it in this project; a relaunched agent names itself with a name no other principal of this project has (docs/DECISIONS.md, D110)",
            naming::described(ctx, &p).await?
        );
    }
    if let (Some(subject), Some(bound)) = (subject, acting) {
        let bound = at_handle(ctx, bound).await;
        bail!(
            "this command runs as {}, which is {bound} already, and a subject binds one principal: it carries on another's job with `fwd principal succeed <its name>`",
            subject::words(subject)
        );
    }
    let old = predecessor(ctx, old, None, |before| format!("`fwd principal add {name} --succeeds {before}`")).await?;
    let old_handle = old.handle()?;
    let held = role_of(&old);
    let role = match (role, held) {
        (Some(given), Some(held)) if given != held => bail!(
            "`@{name}` carries on `@{old_handle}`'s job, whose role is {held}: leave `--role` off, or give {held}"
        ),
        (given, held) => held.or(given).ok_or_else(|| {
            anyhow!(
                "an agent selects its role as it names itself, and `@{old_handle}` holds none to carry on: `fwd principal add {name} --role {ROLE_WORDS} --succeeds {old_handle}`"
            )
        })?,
    };
    let serving = heir_serving(None, &old, Some(role), serves, name)?;
    one_triage_per_scope(ctx, &serving, name, &[old.id()]).await?;
    one_lead_per_session(ctx, Some(role), subject, name, &[old.id()]).await?;
    let unread = unread_of(ctx, &old).await?;
    let trx = ctx.begin();
    let heir = trx
        .create(&Principal {
            kind: PrincipalKind::Agent,
            handle: name.to_string(),
            display_name: display_name.unwrap_or(name).to_string(),
            auth_subject: None,
            agent_role: Some(role),
            active: true,
            member: Some(true),
            subject: subject.map(str::to_string),
            succeeded_by: None,
            serves: serving.stored(),
            // As for an agent that names itself anew (`add_principal`).
            last_active: Some(now()),
        })
        .await?
        .id();
    hand_over(&trx, &old, heir, &unread)?;
    trx.commit().await?;
    Ok(Succession { successor: heir, old: old.id(), role: held, moved: unread.len() })
}

/// The principal `name` names, whose job a successor, `heir` when it is a principal already, is to
/// carry on: refused for a name no principal has, for `heir` itself, for a person, and for a
/// principal a successor took over already, naming that successor and the command, `how`, by which
/// the caller would succeed it instead.
async fn predecessor(
    ctx: &Context,
    name: &str,
    heir: Option<EntityId>,
    how: impl Fn(&str) -> String,
) -> Result<PrincipalView> {
    let name = naming::bare(name);
    let Some(old) = naming::principal_named(ctx, name, None).await? else {
        bail!("no principal of this project is named `{name}`, so there is none to succeed");
    };
    let old_handle = old.handle()?;
    if Some(old.id()) == heir {
        bail!("`@{old_handle}` is the principal this command acts as, and an agent does not succeed itself");
    }
    if old.kind()? == PrincipalKind::Human {
        bail!(
            "`@{old_handle}` is a person, and a person is never succeeded: succession hands an agent's job to the agent relaunched to carry it on"
        );
    }
    if let Some(before) = old.succeeded_by()?.map(EntityId::from) {
        let before_handle = at_handle(ctx, before).await;
        if Some(before) == heir {
            bail!("{before_handle} succeeded `@{old_handle}` already");
        }
        bail!(
            "`@{old_handle}` was succeeded by {before_handle} already, who carries on its job: succeed {before_handle} instead, {}",
            how(before_handle.trim_start_matches('@'))
        );
    }
    Ok(old)
}

/// The notifications `old` has not read, which move to its successor.
async fn unread_of(ctx: &Context, old: &PrincipalView) -> Result<Vec<NotificationView>> {
    Ok(ctx.fetch(with_id("recipient = ? AND status = 'unread'", old.id())?).await?)
}

/// Hands `old`'s job to `heir` in `trx`: `old` is retired with `heir` as its successor and holds
/// no role, and serves nothing, any more, since its role and its scopes move (the caller gives them
/// to `heir`), so that one brought back selects a role and takes scopes anew; and its `unread`
/// notifications move to `heir`, each recording whom it was for (`for_id`), the one change of a
/// notification's recipient fwd makes.
fn hand_over(
    trx: &ankurah::transaction::Transaction,
    old: &PrincipalView,
    heir: EntityId,
    unread: &[NotificationView],
) -> Result<()> {
    let m = old.edit(trx)?;
    m.active()?.set(&false)?;
    m.succeeded_by()?.set(&Some(Ref::new(heir)))?;
    if role_of(old).is_some() {
        m.agent_role()?.set(&None)?;
    }
    if old.serves().ok().flatten().is_some() {
        m.serves()?.set(&None)?;
    }
    for n in unread {
        let mut originator: Value = n.originator()?.into();
        if originator.get("for_id").is_none() {
            originator["for_id"] = json!(old.id().to_base64());
        }
        let e = n.edit(trx)?;
        e.recipient()?.set(&Ref::new(heir))?;
        e.originator()?.set(&Json::new(originator))?;
    }
    Ok(())
}

/// `fwd principal role`: the agent `name` names ([`naming::principal_named`]) holds `role` from now
/// on, and with triage serves what `serves` gives as [`add_principal`] has it serve
/// ([`serving_after`]), while another role frees what it served. Refused for a person, for a name
/// no principal of the project has (naming the command that adds it), for a name two or more
/// principals share (listing them), for triage that would serve nothing, or a scope, or the whole
/// project, that another principal that is not retired serves ([`one_triage_per_scope`]), and for
/// lead while another that is not retired holds it bound to an agent of the principal's own Claude
/// Code session ([`one_lead_per_session`]).
pub async fn set_role(ctx: &Context, name: &str, role: AgentRole, serves: Option<Serves>) -> Result<EntityId> {
    let name = naming::bare(name);
    let Some(p) = naming::principal_named(ctx, name, None).await? else {
        bail!(
            "no principal of this project is named `{name}`: an agent names itself and selects its role at once, `fwd principal add {name} --role {role}`"
        );
    };
    let handle = p.handle()?;
    if p.kind()? == PrincipalKind::Human {
        bail!("`@{handle}` is a person, and a role is an agent's");
    }
    let serving = serving_after(Some(&p), Some(role), serves, &handle)?;
    one_triage_per_scope(ctx, &serving, &handle, &[p.id()]).await?;
    one_lead_per_session(ctx, Some(role), p.subject().ok().flatten().as_deref(), &handle, &[p.id()]).await?;
    let trx = ctx.begin();
    let m = p.edit(&trx)?;
    m.agent_role()?.set(&Some(role))?;
    if serving != Serving::of(&p) {
        m.serves()?.set(&serving.stored())?;
    }
    trx.commit().await?;
    Ok(p.id())
}

/// `fwd principal scope add` and `fwd principal scope project-wide`: the triage agent `name` names
/// serves `what` besides what it served, a scope, or by a deliberate endorsement the whole project,
/// which suits only a small project with small scopes (docs/DECISIONS.md, D122), so that a lead
/// binds its triage agent to its scopes, or endorses the one triage agent of a small project, the
/// triage agents of before 2026-09-30 among them, which serve nothing until then. Refused for a
/// principal that does not hold triage, naming the command that selects it, and where another
/// principal that is not retired serves the scope, or the whole project ([`one_triage_per_scope`]);
/// a scope or an endorsement it has already changes nothing.
pub async fn serve(ctx: &Context, name: &str, what: Serves) -> Result<EntityId> {
    let (p, handle) = triage_named(ctx, name, what).await?;
    let serving = Serving::of(&p).with(what);
    one_triage_per_scope(ctx, &serving, &handle, &[p.id()]).await?;
    set_serving(ctx, &p, &serving).await
}

/// `fwd principal scope remove` and `fwd principal scope project-wide --withdraw`: the triage agent
/// `name` names serves `what` no more, freeing the scope for another triage agent, or the
/// endorsement for another agent that serves the whole project; what concerns it then reaches the
/// triage agent that answers first for it without this one, and a question asked of it waits where
/// none does (docs/design/agents-and-inbox.md, rule 5). An agent left serving nothing holds triage
/// still, and nothing reaches it for its role. Refused for a principal that does not hold triage,
/// and for a scope, or an endorsement, it does not have, saying what it serves.
pub async fn stop_serving(ctx: &Context, name: &str, what: Serves) -> Result<EntityId> {
    let (p, handle) = triage_named(ctx, name, what).await?;
    let before = Serving::of(&p);
    let after = before.clone().without(what);
    if after == before {
        let what = match what {
            Serves::Scope(scope) | Serves::Found(scope) => format!("the scope {}", scope_named(ctx, scope).await),
            Serves::ProjectWide => "the whole project".to_string(),
        };
        bail!("`@{handle}` does not serve {what}: it serves {}", serving_words(ctx, &before).await);
    }
    set_serving(ctx, &p, &after).await
}

/// The triage agent `name` names, for a command that changes what it serves (`what`), with its
/// handle: refused for a name no principal has, for a person, and for an agent that does not hold
/// triage, since only a triage agent serves a scope or the whole project.
async fn triage_named(ctx: &Context, name: &str, what: Serves) -> Result<(PrincipalView, String)> {
    let name = naming::bare(name);
    let Some(p) = naming::principal_named(ctx, name, None).await? else {
        bail!("no principal of this project is named `{name}`");
    };
    let handle = p.handle()?;
    if role_of(&p) != Some(AgentRole::Triage) {
        let holds = match (p.kind()?, role_of(&p)) {
            (PrincipalKind::Human, _) => "is a person".to_string(),
            (_, Some(role)) => format!("holds {role}"),
            (_, None) => "holds no role".to_string(),
        };
        let with = match what {
            Serves::ProjectWide => "--project-wide".to_string(),
            Serves::Scope(scope) | Serves::Found(scope) => format!("--scope {}", short_id(scope)),
        };
        bail!(
            "`@{handle}` {holds}, and only a triage agent serves a scope or the whole project: an agent selects triage with what it serves, `fwd principal role {handle} triage {with}`"
        );
    }
    Ok((p, handle))
}

/// Records on `p` that it serves `serving`, in one transaction.
async fn set_serving(ctx: &Context, p: &PrincipalView, serving: &Serving) -> Result<EntityId> {
    if *serving != Serving::of(p) {
        let trx = ctx.begin();
        p.edit(&trx)?.serves()?.set(&serving.stored())?;
        trx.commit().await?;
    }
    Ok(p.id())
}

/// The first eight characters of `id`, which name it wherever an id is typed while no other id
/// starts with them.
fn short_id(id: EntityId) -> String {
    id.to_base64().chars().take(8).collect()
}

/// A scope as the sentences name it: its title in quotes and the first eight characters of its id,
/// `"Replication" (Ab3dEf12)`.
async fn scope_named(ctx: &Context, scope: EntityId) -> String {
    let title = ctx.get::<ScopeView>(scope).await.ok().and_then(|s| s.title().ok()).unwrap_or_default();
    format!("\"{title}\" ({})", short_id(scope))
}

/// What `serving` serves, in words for a sentence: `the scopes "Replication" (Ab3dEf12) and
/// "Storage" (Cd4eFg56)`, `the whole project`, both, or `nothing`.
pub async fn serving_words(ctx: &Context, serving: &Serving) -> String {
    let mut parts = Vec::new();
    if !serving.scopes.is_empty() {
        let mut named = Vec::new();
        for &scope in &serving.scopes {
            named.push(scope_named(ctx, scope).await);
        }
        let noun = if named.len() == 1 { "the scope" } else { "the scopes" };
        parts.push(format!("{noun} {}", and_list(&named)));
    }
    if serving.project_wide {
        parts.push("the whole project".to_string());
    }
    if parts.is_empty() {
        "nothing".to_string()
    } else {
        parts.join(" and ")
    }
}

/// `items` joined for a sentence: `a`, `a and b`, `a, b and c`.
fn and_list(items: &[String]) -> String {
    match items.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// `fwd principal retire`: the principal `name` names ([`naming::principal_named`]) stops belonging
/// to the project. It stays, so that what it did is still attributed to it and it can still be
/// named, and no picker offers it; its later writes are refused, naming `fwd principal add`, which,
/// run as it, brings it back (task Wbkkwpyq; `refuse_retired` in crates/fwd/src/main.rs). A retired
/// triage agent no longer counts toward the scopes it served, nor toward the one agent a project
/// may have that serves it whole, nor a retired lead toward the one its session may have, so
/// retiring a triage agent leaves its scopes to the agent that serves the whole project, or
/// without one until an agent takes them; it keeps what it served, which counts again once it is
/// brought back.
pub async fn retire_principal(ctx: &Context, name: &str) -> Result<EntityId> {
    let name = naming::bare(name);
    let p = naming::principal_named(ctx, name, None)
        .await?
        .ok_or_else(|| anyhow!("no one in this project has the handle `@{name}`"))?;
    let trx = ctx.begin();
    let m = p.edit(&trx)?;
    m.active()?.set(&false)?;
    trx.commit().await?;
    Ok(p.id())
}

/// `fwd principal rename`: the agent `principal`, the one the command acts as, takes the name
/// `name`, so that an agent whose name turned out to be shared is named by a name again rather than
/// by its id alone. It keeps its id, which everything that refers to it stores (what it made and
/// said, its inbox, its role, the Claude Code agent bound to it), so all of that stays its own and
/// everything that shows its name shows the new one (docs/design/agents-and-inbox.md, "A
/// principal's name"). Its display name follows where it was the old name, as `fwd principal add`
/// sets it when given none. Refused for a blank name; for a person, whose name is the handle `fwd
/// init` recorded on this machine, by which the CLI finds them; and for a name that would name
/// another principal of the project as well, by the rule every typed name is read by
/// ([`naming::resolve`]): one that another principal has, listed as `fwd principal add` lists
/// the holder of a taken name, since a name no other principal of the project has is the practice
/// (docs/DECISIONS.md, D110), and one that is, or starts, another principal's id.
pub async fn rename_principal(ctx: &Context, principal: EntityId, name: &str) -> Result<EntityId> {
    let name = naming::bare(name);
    if name.is_empty() {
        bail!("a name cannot be blank");
    }
    let p: PrincipalView = ctx.get(principal).await?;
    let old = p.handle()?;
    if p.kind()? == PrincipalKind::Human {
        bail!(
            "`@{old}` is a person, whose name is the handle `fwd init` recorded on this machine, by which every command finds them: `fwd principal rename` renames the agent the command acts as"
        );
    }
    let found = principals(ctx).await?;
    let others: Vec<&PrincipalView> =
        naming::by_name(&found, name, None).into_iter().filter(|q| q.id() != principal).collect();
    if !others.is_empty() {
        let has = if others.len() == 1 { "has" } else { "have" };
        bail!(
            "the name `{name}` is taken: {} {has} it in this project; choose a name no other principal of this project has, as every agent does (a role and a qualifier reads well: `implementer-replication`; docs/DECISIONS.md, D110)",
            described_all(ctx, &others).await?
        );
    }
    // No other principal has the name; one whose id it is, or starts, is named by it all the same.
    let by_id: Vec<&PrincipalView> = match naming::resolve(&found, name, None) {
        naming::Named::One(q) => vec![q],
        naming::Named::Started(several) => several,
        naming::Named::NoOne | naming::Named::Shared(_) => Vec::new(),
    };
    let by_id: Vec<&PrincipalView> = by_id.into_iter().filter(|q| q.id() != principal).collect();
    if !by_id.is_empty() {
        bail!(
            "`{name}` names {} by its id, wherever a name is typed; choose a name that starts no principal's id and that no other principal of this project has (a role and a qualifier reads well: `implementer-replication`)",
            described_all(ctx, &by_id).await?
        );
    }
    if name == old {
        return Ok(principal);
    }
    let trx = ctx.begin();
    let m = p.edit(&trx)?;
    m.handle()?.set(&name.to_string())?;
    if p.display_name()? == old {
        m.display_name()?.set(&name.to_string())?;
    }
    trx.commit().await?;
    Ok(principal)
}

/// `principals` as a refusal lists them for a reader who must tell each from the others
/// ([`naming::described`]), in the order of their ids, as [`naming::refusal`] lists them:
/// `Ab3dEf12 (implementer, in Replication) and Cd4eFg56 (reviewer, in no scope)`.
async fn described_all(ctx: &Context, principals: &[&PrincipalView]) -> Result<String> {
    let mut principals = principals.to_vec();
    principals.sort_by_key(|p| p.id().to_base64());
    let mut listed = Vec::with_capacity(principals.len());
    for p in principals {
        listed.push(naming::described(ctx, p).await?);
    }
    Ok(and_list(&listed))
}

/// The project's people: every human principal that belongs to it (one in v1, which is
/// single-user), who rule on proposed decisions.
pub async fn people(ctx: &Context) -> Result<Vec<EntityId>> {
    let found: Vec<PrincipalView> = ctx.fetch("kind = 'human'").await?;
    Ok(found.iter().filter(|p| belongs(p)).map(|p| p.id()).collect())
}

/// The person a name or an id names, with or without its `@`, for naming whose ruling an agent
/// records (`fwd decision accept --decided-by`). Only a person rules, so an agent that shares the
/// person's name is not a candidate ([`naming::principal_named`] of a person), and two people
/// sharing it are refused, listed. A name that no principal has, or that names only agents, is
/// refused with a sentence saying so.
pub async fn person_by_handle(ctx: &Context, name: &str) -> Result<EntityId> {
    let name = naming::bare(name);
    if let Some(p) = naming::principal_named(ctx, name, Some(PrincipalKind::Human)).await? {
        return Ok(p.id());
    }
    match naming::resolve(&principals(ctx).await?, name, None) {
        naming::Named::NoOne => bail!("no one in this project has the handle `@{name}`"),
        _ => bail!(
            "`@{name}` is an agent, not a person: a ruling recorded on someone's word names the person who gave it"
        ),
    }
}

/// `@handle` for a principal, or the start of its id when it cannot be read: how a refusal names
/// a person.
async fn at_handle(ctx: &Context, principal: EntityId) -> String {
    match ctx.get::<PrincipalView>(principal).await.ok().and_then(|p| p.handle().ok()) {
        Some(handle) => format!("@{handle}"),
        None => principal.to_base64().chars().take(8).collect(),
    }
}

// ---------------------------------------------------------------------------------------------
// Relations

/// The relations of `kind` that hold for the rules (`rules::holds`): the accepted ones, and for
/// steps the checked ones as well.
pub async fn holding_edges(ctx: &Context, kind: RelationKind) -> Result<Vec<Edge>> {
    let mut views: Vec<RelationView> = ctx.fetch(format!("kind = '{kind}' AND status = 'accepted'").as_str()).await?;
    if kind == RelationKind::StepOf {
        views.extend(ctx.fetch::<RelationView>(format!("kind = '{kind}' AND status = 'resolved'").as_str()).await?);
    }
    views.iter().map(edge_of).collect()
}

pub fn edge_of(v: &RelationView) -> Result<Edge> {
    Ok(Edge { id: v.id(), kind: v.kind()?, from: v.from_id()?, to: v.to_id()?, status: v.status()? })
}

pub async fn relations_from(ctx: &Context, from: EntityId, kind: Option<RelationKind>) -> Result<Vec<RelationView>> {
    let pred = match kind {
        Some(k) => format!("from_id = ? AND kind = '{k}'"),
        None => "from_id = ?".to_string(),
    };
    Ok(ctx.fetch(with_id(&pred, from)?).await?)
}

pub async fn relations_to(ctx: &Context, to: EntityId, kind: Option<RelationKind>) -> Result<Vec<RelationView>> {
    let pred = match kind {
        Some(k) => format!("to_id = ? AND kind = '{k}'"),
        None => "to_id = ?".to_string(),
    };
    Ok(ctx.fetch(with_id(&pred, to)?).await?)
}

/// What one end of a relation may be ([`Join`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Anything,
    /// An entity of one of these kinds.
    OneOf(&'static [Kind]),
}

impl End {
    /// Whether an entity of `kind` may be this end; none is an id that names nothing here, which
    /// only an end that may be anything takes.
    fn takes(self, kind: Option<Kind>) -> bool {
        match self {
            End::Anything => true,
            End::OneOf(kinds) => kind.is_some_and(|k| kinds.contains(&k)),
        }
    }

    /// `anything`, `a task`, `a task or a message`, `a task, a relation or a scope`.
    fn words(self) -> String {
        match self {
            End::Anything => "anything".to_string(),
            End::OneOf(kinds) => {
                let each: Vec<String> = kinds.iter().map(|k| one(*k)).collect();
                match each.split_last() {
                    Some((last, [])) => last.clone(),
                    Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
                    None => String::new(),
                }
            }
        }
    }
}

/// One way a relation of a kind joins two entities: what its `from` may be, and what its `to` may
/// be ([`joins`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Join {
    pub from: End,
    pub to: End,
}

/// What a relation of `kind` joins, each way it may, as the table of relations in
/// docs/design/data-model.md gives it: what every page reads from a relation rests on its ends
/// being of these kinds (a task's scope is the target of its `worked_in`), so [`link`] refuses
/// any other. `relates_to` joins anything to anything.
pub fn joins(kind: RelationKind) -> &'static [Join] {
    use End::{Anything, OneOf};
    const TASK: End = OneOf(&[Kind::Task]);
    const MESSAGE: End = OneOf(&[Kind::Message]);
    const DECISION: End = OneOf(&[Kind::Decision]);
    const TASK_OR_MESSAGE: End = OneOf(&[Kind::Task, Kind::Message]);
    match kind {
        RelationKind::DependsOn | RelationKind::SubtaskOf | RelationKind::Duplicates => {
            &[Join { from: TASK, to: TASK }]
        }
        RelationKind::MotivatedBy | RelationKind::DiscoveredFrom => &[Join { from: TASK, to: TASK_OR_MESSAGE }],
        RelationKind::Supersedes => &[Join { from: TASK, to: TASK }, Join { from: DECISION, to: DECISION }],
        RelationKind::AnchoredAt => &[Join { from: TASK, to: OneOf(&[Kind::Anchor]) }],
        RelationKind::WorkedIn | RelationKind::AssignedTo => &[Join { from: TASK, to: OneOf(&[Kind::Scope]) }],
        RelationKind::StepOf => &[Join { from: Anything, to: TASK }],
        RelationKind::Concerns => &[Join { from: MESSAGE, to: OneOf(&[Kind::Task, Kind::Relation, Kind::Scope]) }],
        RelationKind::Cites => &[
            Join { from: OneOf(&[Kind::Task, Kind::Message, Kind::Decision]), to: DECISION },
            Join { from: DECISION, to: TASK_OR_MESSAGE },
        ],
        RelationKind::Mention => &[Join { from: MESSAGE, to: Anything }],
        RelationKind::RelatesTo => &[Join { from: Anything, to: Anything }],
    }
}

/// The relation kinds [`link`] makes, in the order the table of relations in
/// docs/design/data-model.md gives them: every kind but `mention`, which posting a message's text
/// makes, and `assigned_to`, which nothing makes any more.
pub const LINKED: &[RelationKind] = &[
    RelationKind::DependsOn,
    RelationKind::SubtaskOf,
    RelationKind::MotivatedBy,
    RelationKind::DiscoveredFrom,
    RelationKind::RelatesTo,
    RelationKind::Supersedes,
    RelationKind::Duplicates,
    RelationKind::AnchoredAt,
    RelationKind::WorkedIn,
    RelationKind::StepOf,
    RelationKind::Concerns,
    RelationKind::Cites,
];

/// What a relation of `kind` joins, as `fwd link --help` and its refusals say it: `a task to a
/// scope`; `a task to a task, or a decision to a decision`.
pub fn joins_words(kind: RelationKind) -> String {
    joins(kind).iter().map(|j| format!("{} to {}", j.from.words(), j.to.words())).collect::<Vec<_>>().join(", or ")
}

/// One entity of `kind`, as a sentence names it: `a task`, `an anchor`.
fn one(kind: Kind) -> String {
    let name = kind.name();
    let article = if name.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" };
    format!("{article} {name}")
}

/// The sentence refusing a relation of `kind` from `from` to `to`, each an id with the kind of
/// what it names here (none: nothing), when no way [`joins`] gives for the kind takes the two;
/// none when one does. It says what the kind joins and what the two are, or which of them names
/// nothing in the project: "`worked_in` joins a task to a scope, not a task to a decision".
pub fn ends_refused(
    kind: RelationKind,
    from: (EntityId, Option<Kind>),
    to: (EntityId, Option<Kind>),
) -> Option<String> {
    if joins(kind).iter().any(|j| j.from.takes(from.1) && j.to.takes(to.1)) {
        return None;
    }
    let joined = joins_words(kind);
    let nothing: Vec<String> = [from, to]
        .iter()
        .filter(|(_, what)| what.is_none())
        .map(|(id, _)| format!("`{}`", id.to_base64().chars().take(8).collect::<String>()))
        .collect();
    Some(match (nothing.as_slice(), from.1, to.1) {
        ([], Some(from), Some(to)) => format!("`{kind}` joins {joined}, not {} to {}", one(from), one(to)),
        ([id], ..) => format!("`{kind}` joins {joined}, and {id} names nothing in this project"),
        (both, ..) => format!("`{kind}` joins {joined}, and {} name nothing in this project", both.join(" and ")),
    })
}

/// Refuses a relation of `kind` from `from` to `to` whose ends are not of kinds it joins
/// ([`ends_refused`]), reading what each id names in the project, a notification included.
async fn check_ends(ctx: &Context, kind: RelationKind, from: EntityId, to: EntityId) -> Result<()> {
    if joins(kind).iter().all(|j| j.from == End::Anything && j.to == End::Anything) {
        return Ok(());
    }
    match ends_refused(kind, (from, end_kind(ctx, from).await), (to, end_kind(ctx, to).await)) {
        Some(sentence) => bail!("refused: {sentence}"),
        None => Ok(()),
    }
}

/// What `id` names in the project, as a relation's end, a notification included: none when it
/// names nothing here.
async fn end_kind(ctx: &Context, id: EntityId) -> Option<Kind> {
    match kind_of(ctx, id).await {
        Some(kind) => Some(kind),
        None => ctx.get::<NotificationView>(id).await.is_ok().then_some(Kind::Notification),
    }
}

/// Creates a relation after the pre-checks: its ends must be of kinds its kind joins ([`joins`]),
/// and a `depends_on`, `subtask_of` or `step_of` must close no cycle. `via` records what caused
/// the assertion. A `cites` relation is made once: citing again answers the relation that already
/// stands, and citing without `propose` accepts a citation that was only proposed, taking the new
/// note when one is given (so `fwd cite` after `fwd link ... --propose` neither leaves the
/// citation pending nor drops its note). A decision is superseded only through
/// [`supersede_decision`], which also marks it Superseded. A step is made once too, at the end of
/// its task's checklist ([`add_step`]); a task is worked in each of its scopes once, by one
/// `worked_in`, and in any number of scopes (docs/DECISIONS.md, D136, "A task may be worked in
/// several scopes"; until 2026-10-03 a second scope was refused); a mention is made only by posting
/// the text that holds it, and `assigned_to` not at all any more.
#[allow(clippy::too_many_arguments)]
pub async fn link(
    ctx: &Context,
    by: EntityId,
    kind: RelationKind,
    from: EntityId,
    to: EntityId,
    note: Option<&str>,
    propose: bool,
    via: Value,
) -> Result<EntityId> {
    match kind {
        RelationKind::AssignedTo => bail!(
            "`assigned_to` is no longer made: who does a task is its assignee (`fwd task assign <task> @handle`), and the scope the work happens in is `worked_in` (docs/DECISIONS.md, \"Assignment is an optional field on the task, never an edge\", 2026-09-28)"
        ),
        RelationKind::Mention => bail!(
            "a mention is written in a message's text (`@handle`, `[[<id>]]`, `[[D<number>]]`) and made when the message is posted; it is not linked"
        ),
        RelationKind::Cites if from == to => bail!("a decision cannot cite itself"),
        _ => {}
    }
    check_ends(ctx, kind, from, to).await?;
    if kind == RelationKind::StepOf {
        let already = relations_from(ctx, from, Some(RelationKind::StepOf)).await?;
        if let Some(r) = already.iter().find(|r| r.to_id().ok() == Some(to) && step_stands(r)) {
            return Ok(r.id());
        }
    }
    if kind == RelationKind::Cites {
        let already = relations_from(ctx, from, Some(RelationKind::Cites)).await?;
        if let Some(r) = already.iter().find(|r| r.to_id().ok() == Some(to) && standing(r)) {
            if !propose && r.status()? == RelationStatus::Proposed {
                let trx = ctx.begin();
                let m = r.edit(&trx)?;
                m.status()?.set(&RelationStatus::Accepted)?;
                if let Some(note) = note {
                    m.note()?.set(&Some(note.to_string()))?;
                }
                trx.commit().await?;
            }
            return Ok(r.id());
        }
    }
    if kind == RelationKind::Supersedes && ctx.get::<DecisionView>(to).await.is_ok() {
        bail!(
            "a decision is superseded with `fwd decision supersede <old> <new> --note \"...\"`, which also marks it superseded and records why"
        );
    }
    if rules::must_be_acyclic(kind) {
        let edges = holding_edges(ctx, kind).await?;
        if let Some(cycle) = rules::would_close_cycle(kind, from, to, &edges) {
            bail!(
                "refused: {kind} {from} -> {to} would close a cycle: {}",
                cycle.iter().map(|id| id.to_base64()).collect::<Vec<_>>().join(" -> ")
            );
        }
    }
    if kind == RelationKind::WorkedIn {
        let existing = relations_from(ctx, from, Some(RelationKind::WorkedIn)).await?;
        let accepted = |r: &&RelationView| r.status().ok() == Some(RelationStatus::Accepted);
        if let Some(same) = existing.iter().filter(accepted).find(|r| r.to_id().ok() == Some(to)) {
            return Ok(same.id());
        }
    }
    let position = match kind {
        RelationKind::StepOf => Some(next_position(ctx, to).await?),
        _ => None,
    };
    let status = if propose { RelationStatus::Proposed } else { RelationStatus::Accepted };
    let trx = ctx.begin();
    let id =
        trx.create(&new_relation(by, kind, (from, to), status, via, note.map(str::to_string), position)).await?.id();
    trx.commit().await?;
    Ok(id)
}

/// A relation of `kind` from the first of `ends` to the second, with `status`, that `by` makes
/// now, as [`link`] writes one, and `add_highlight_step` the step of a new highlight.
fn new_relation(
    by: EntityId,
    kind: RelationKind,
    (from, to): (EntityId, EntityId),
    status: RelationStatus,
    via: Value,
    note: Option<String>,
    position: Option<String>,
) -> Relation {
    Relation {
        kind,
        from_id: from,
        to_id: to,
        status,
        created_by: Ref::new(by),
        created: now(),
        via: Json::new(via),
        note,
        position,
        closed_by: None,
        closed: None,
        closed_reason: None,
    }
}

/// Retracts every `from -kind-> to` relation that holds (accepted, or a checked step) with a
/// reason. Returns how many. A supersession of one decision by another is refused: retracting it
/// would leave the old decision Superseded with no successor, and nothing moves a superseded
/// decision back, so a supersession is never undone; a new decision supersedes the successor
/// instead.
pub async fn unlink(
    ctx: &Context,
    by: EntityId,
    kind: RelationKind,
    from: EntityId,
    to: EntityId,
    reason: &str,
) -> Result<usize> {
    if kind == RelationKind::Supersedes {
        if let (Ok(old), Ok(new)) = (ctx.get::<DecisionView>(to).await, ctx.get::<DecisionView>(from).await) {
            let (old, new) = (old.number()?, new.number()?);
            bail!(
                "a supersession is not undone: D{old} stays superseded by D{new}. To change course, record a new decision and supersede D{new} with it (`fwd decision supersede D{new} <the new one> --note \"why\"`)"
            );
        }
    }
    let candidates = relations_from(ctx, from, Some(kind)).await?;
    let mut n = 0;
    let trx = ctx.begin();
    for r in candidates.iter().filter(|r| r.to_id().ok() == Some(to)) {
        if !rules::holds(kind, r.status()?) {
            continue;
        }
        let m = r.edit(&trx)?;
        m.status()?.set(&RelationStatus::Retracted)?;
        m.closed_by()?.set(&Some(Ref::new(by)))?;
        m.closed()?.set(&Some(now()))?;
        m.closed_reason()?.set(&Some(reason.to_string()))?;
        n += 1;
    }
    trx.commit().await?;
    Ok(n)
}

/// Changes the scopes a task is worked in, in one write, as the scopes on its panel do (task
/// oLv1sJ3v, "Edit a task's scopes on the page as removable tags with a [+] scope search"): each
/// scope of `add` the task is not worked in comes to be one, by an accepted `worked_in` that `by`
/// makes, and each of `take_off` it is worked in is taken off, its `worked_in` retracted with who
/// took it off and when. So moving a task is taking one scope off and adding another, and a task
/// may end in no scope (docs/DECISIONS.md, D136, "A task may be worked in several scopes"). A scope
/// already as asked is left as it is, so that a change written beside another client's change to
/// other scopes keeps both. A scope to add that is not a scope of the project, and one both added
/// and taken off, are refused before anything is written, the first as `link` refuses it. Nothing
/// tells anyone: an accepted `worked_in` and a retraction reach no one
/// (`routing::plan_for_relation`), as attaching a decision to a scope tells no one (D93).
pub async fn change_scopes(
    ctx: &Context,
    by: EntityId,
    task: EntityId,
    add: &[EntityId],
    take_off: &[EntityId],
) -> Result<()> {
    if let Some(both) = add.iter().find(|s| take_off.contains(s)) {
        bail!("refused: scope {} is both added to the task and taken off it", both.to_base64());
    }
    ctx.get::<TaskView>(task).await.context("that task")?;
    let accepted: Vec<RelationView> = relations_from(ctx, task, Some(RelationKind::WorkedIn))
        .await?
        .into_iter()
        .filter(|r| r.status().ok() == Some(RelationStatus::Accepted))
        .collect();
    let worked_in = |scope: &EntityId| accepted.iter().any(|r| r.to_id().ok() == Some(*scope));
    let mut adding: Vec<EntityId> = Vec::new();
    for scope in add.iter().filter(|s| !worked_in(s)) {
        if !adding.contains(scope) {
            check_ends(ctx, RelationKind::WorkedIn, task, *scope).await?;
            adding.push(*scope);
        }
    }
    let leaving: Vec<&RelationView> =
        accepted.iter().filter(|r| r.to_id().is_ok_and(|scope| take_off.contains(&scope))).collect();
    if adding.is_empty() && leaving.is_empty() {
        return Ok(());
    }
    let t = now();
    let via = json!({"actor_id": by.to_base64()});
    let trx = ctx.begin();
    for scope in adding {
        trx.create(&new_relation(
            by,
            RelationKind::WorkedIn,
            (task, scope),
            RelationStatus::Accepted,
            via.clone(),
            None,
            None,
        ))
        .await?;
    }
    for r in leaving {
        let m = r.edit(&trx)?;
        m.status()?.set(&RelationStatus::Retracted)?;
        m.closed_by()?.set(&Some(Ref::new(by)))?;
        m.closed()?.set(&Some(t))?;
    }
    trx.commit().await?;
    Ok(())
}

pub async fn decide_relation(
    ctx: &Context,
    by: EntityId,
    relation: EntityId,
    approve: bool,
    reason: Option<&str>,
) -> Result<()> {
    let r: RelationView = ctx.get(relation).await?;
    if r.status()? != RelationStatus::Proposed {
        bail!("relation {relation} is {}, not proposed", r.status()?);
    }
    if approve && rules::must_be_acyclic(r.kind()?) {
        let edges = holding_edges(ctx, r.kind()?).await?;
        if let Some(cycle) = rules::would_close_cycle(r.kind()?, r.from_id()?, r.to_id()?, &edges) {
            bail!(
                "refused: approving would close a cycle: {}",
                cycle.iter().map(|id| id.to_base64()).collect::<Vec<_>>().join(" -> ")
            );
        }
    }
    let trx = ctx.begin();
    let m = r.edit(&trx)?;
    if approve {
        m.status()?.set(&RelationStatus::Accepted)?;
    } else {
        m.status()?.set(&RelationStatus::Rejected)?;
        m.closed_by()?.set(&Some(Ref::new(by)))?;
        m.closed()?.set(&Some(now()))?;
        m.closed_reason()?.set(&reason.map(str::to_string))?;
    }
    trx.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Steps

/// Whether a `step_of` relation still stands: proposed, accepted or checked, neither retracted nor
/// rejected.
fn step_stands(r: &RelationView) -> bool {
    matches!(r.status(), Ok(RelationStatus::Proposed | RelationStatus::Accepted | RelationStatus::Resolved))
}

/// The position after the last of `task`'s steps, for a step added at the end of its checklist.
async fn next_position(ctx: &Context, task: EntityId) -> Result<String> {
    let last = relations_to(ctx, task, Some(RelationKind::StepOf))
        .await?
        .iter()
        .filter(|r| step_stands(r))
        .filter_map(|r| r.position().ok().flatten())
        .filter(|p| crate::position::is_key(p))
        .max();
    crate::position::after(last.as_deref())
}

/// The steps of `task`, as its checklist lists them: its `step_of` relations that are accepted or
/// checked, by position, then by when they were made, then by id; a step with no position (made
/// by a client that did not give one) after those with one.
pub async fn steps(ctx: &Context, task: EntityId) -> Result<Vec<RelationView>> {
    let mut found: Vec<RelationView> = relations_to(ctx, task, Some(RelationKind::StepOf))
        .await?
        .into_iter()
        .filter(|r| r.status().is_ok_and(|s| rules::holds(RelationKind::StepOf, s)))
        .collect();
    found.sort_by_key(|r| {
        let position = r.position().ok().flatten();
        (position.is_none(), position, r.created().unwrap_or(0), r.id())
    });
    Ok(found)
}

/// Makes `step` (a highlight, a message, a task or anything else) a step of `task`, at the end of
/// its checklist: a `step_of` relation from the step to the task. A task that would become a step
/// of itself, however far round, is refused; an entity that is already a step of the task answers
/// the step that stands.
pub async fn add_step(ctx: &Context, by: EntityId, task: EntityId, step: EntityId) -> Result<EntityId> {
    link(ctx, by, RelationKind::StepOf, step, task, None, false, step_via(by)).await
}

/// What a step records of what made it: who added it.
fn step_via(by: EntityId) -> Value {
    json!({"actor_id": by.to_base64()})
}

/// Checks the step `edge` (`check`), moving its relation to Resolved with `by` and the time as its
/// closing fields, or unchecks it, moving it back to Accepted and clearing them. A step that is a
/// highlight takes the first thread on the highlight with it, as `rules::move_step_and_thread`
/// says (docs/DECISIONS.md, D99, "Checking a step resolves its highlight's first thread, and
/// resolving that thread checks the step; each control says it does both"): checking resolves
/// that thread's first message, with `by` and the time as who resolved it and when, unchecking sets
/// it back to active, and the highlight's steps of other tasks move with them, in the same
/// transaction. A step that is a message or a task, or a highlight no one has started a thread on,
/// moves alone. Checking a checked step, or unchecking an unchecked one, leaves the step as it is.
/// Returns what moved. Like a check before D99, it tells no one.
pub async fn check_step(ctx: &Context, by: EntityId, edge: EntityId, check: bool) -> Result<rules::Changes> {
    let r: RelationView = ctx.get(edge).await.context("that step")?;
    let from = r.from_id()?;
    let highlight = r.kind()? == RelationKind::StepOf && ctx.get::<CodeAnchorView>(from).await.is_ok();
    let (messages, mut relations) =
        if highlight { highlight_parts(ctx, from).await? } else { (Vec::new(), Vec::new()) };
    if !relations.iter().any(|x| x.id() == edge) {
        relations.push(r.clone());
    }
    let on_highlight = messages.iter().map(thread_message).collect::<Result<Vec<_>>>()?;
    let steps = relations.iter().map(step_edge).collect::<Result<Vec<_>>>()?;
    let moved = step_edge(&r)?;
    let changes = rules::move_step_and_thread(rules::Moved::Step(&moved), check, &on_highlight, &steps)?;
    write_changes(ctx, by, &changes, &messages, &relations).await?;
    Ok(changes)
}

/// Takes the step `edge` off `task`'s checklist: for a step that no longer belongs there, such as
/// one whose work moved to other tasks or one added by mistake, which checking would record as
/// done (task 9dvGW0EI, "Take a step off a checklist and retire its highlight in one command: fwd
/// step remove", after a review of 172 steps was replaced by smaller ones). Its `step_of` relation
/// is retracted with `by`, the time and `reason` in its closing fields, as [`unlink`] retracts
/// one, so no checklist lists it, and what the step is (a highlight, a message, a task) stays as
/// it was. With `retire`, a step that is a highlight has its threads resolved too
/// (`rules::retire_highlight`), each with `by` and the time as who resolved it and when, so the
/// editor stops drawing it while its page still lists the threads. One transaction writes it all.
/// Only the task's creator and its assignee take a step off (`rules::can_remove_step`). Refused
/// with a sentence, writing nothing: a relation that is not a step on `task`'s checklist, anyone
/// but the task's creator and its assignee, a blank reason, and a retire of a step that is not a
/// highlight. Returns what the retire moved, nothing without one.
///
/// It tells no one, as an unlink and a resolve tell no one today: it retracts a relation and
/// moves statuses, and adds nothing for routing, which reacts only to what is added.
pub async fn remove_step(
    ctx: &Context,
    by: EntityId,
    task: EntityId,
    edge: EntityId,
    reason: &str,
    retire: bool,
) -> Result<rules::Retired> {
    let view: TaskView = ctx.get(task).await.context("that task")?;
    let title = view.title()?;
    let r: RelationView = ctx.get(edge).await.context("that step")?;
    if r.kind()? != RelationKind::StepOf || r.to_id()? != task {
        bail!("that is not a step on the checklist of \"{title}\"");
    }
    match r.status()? {
        status if rules::holds(RelationKind::StepOf, status) => {}
        RelationStatus::Retracted => bail!("that step was taken off the checklist of \"{title}\" already"),
        status => bail!("that step is {status}, so it is not on the checklist of \"{title}\""),
    }
    let (creator, assignee) = (EntityId::from(view.created_by()?), assignee_of(&view)?);
    if rules::can_remove_step(creator, assignee, by).is_err() {
        let ask = match assignee.filter(|a| *a != creator) {
            Some(a) => format!("{} or {}", at_handle(ctx, creator).await, at_handle(ctx, a).await),
            None => at_handle(ctx, creator).await,
        };
        bail!("only the creator and the assignee of \"{title}\" take a step off its checklist; ask {ask}");
    }
    let reason = reason.trim();
    if reason.is_empty() {
        bail!("say why the step comes off the checklist: the reason is kept on the step");
    }
    let step = r.from_id()?;
    let (messages, relations, retired) = if retire {
        if ctx.get::<CodeAnchorView>(step).await.is_err() {
            let what = match describe(ctx, step).await {
                Some((kind, name)) => format!("a {} ({name})", kind.name()),
                None => "not a highlight".to_string(),
            };
            bail!("only a highlight is retired, and this step is {what}; take it off without retiring it");
        }
        let (messages, relations) = highlight_parts(ctx, step).await?;
        let on_highlight = messages.iter().map(thread_message).collect::<Result<Vec<_>>>()?;
        let steps = relations.iter().map(step_edge).collect::<Result<Vec<_>>>()?;
        // Resolving the first thread checks the steps the highlight still is on other tasks'
        // checklists, as `fwd message resolve` on that thread would, since a highlight's steps and
        // its first thread are one state (docs/DECISIONS.md, D99, "Checking a step resolves its
        // highlight's first thread, and resolving that thread checks the step; each control says
        // it does both"). Retiring checks them rather than leave a checklist and the thread apart;
        // the CLI and the page say which steps it checked.
        let retired = rules::retire_highlight(&on_highlight, &steps, edge)?;
        (messages, relations, retired)
    } else {
        (Vec::new(), Vec::new(), rules::Retired::default())
    };
    let t = now();
    let trx = ctx.begin();
    let m = r.edit(&trx)?;
    m.status()?.set(&RelationStatus::Retracted)?;
    m.closed_by()?.set(&Some(Ref::new(by)))?;
    m.closed()?.set(&Some(t))?;
    m.closed_reason()?.set(&Some(reason.to_string()))?;
    for id in &retired.messages {
        set_message_status(&trx, &messages, *id, MessageStatus::Resolved, by, t)?;
    }
    for (id, status) in &retired.steps {
        set_step_status(&trx, &relations, *id, *status, by, t)?;
    }
    trx.commit().await?;
    Ok(retired)
}

// ---------------------------------------------------------------------------------------------
// A step and its highlight's first thread

/// A message as `rules::move_step_and_thread` reads it.
pub fn thread_message(m: &MessageView) -> Result<rules::ThreadMessage> {
    Ok(rules::ThreadMessage {
        id: m.id(),
        answers: m.thread()?.is_some() || m.reply_to()?.is_some(),
        status: m.status()?,
        created: m.created()?,
    })
}

/// A relation as `rules::move_step_and_thread` reads it.
pub fn step_edge(r: &RelationView) -> Result<rules::StepEdge> {
    Ok(rules::StepEdge { id: r.id(), kind: r.kind()?, status: r.status()? })
}

/// The first message of the first thread on the highlight `anchor` (`rules::first_thread`), which
/// moves with the highlight's steps; none while no one has started a thread on it.
pub async fn first_thread(ctx: &Context, anchor: EntityId) -> Result<Option<MessageView>> {
    let messages = messages_about(ctx, "anchor", anchor).await?;
    let on_highlight = messages.iter().map(thread_message).collect::<Result<Vec<_>>>()?;
    let first = rules::first_thread(&on_highlight);
    Ok(messages.into_iter().find(|m| Some(m.id()) == first))
}

/// The messages on the highlight `anchor` and its `step_of` relations: what moves together when one
/// of them moves.
async fn highlight_parts(ctx: &Context, anchor: EntityId) -> Result<(Vec<MessageView>, Vec<RelationView>)> {
    Ok((messages_about(ctx, "anchor", anchor).await?, relations_from(ctx, anchor, Some(RelationKind::StepOf)).await?))
}

/// Writes `changes` in one transaction: each message it resolves gets `by` and the time as who
/// resolved it and when, and each step it checks as who checked it and when; a message set back to
/// active, and a step unchecked, forget both. `messages` and `relations` hold the views it names.
async fn write_changes(
    ctx: &Context,
    by: EntityId,
    changes: &rules::Changes,
    messages: &[MessageView],
    relations: &[RelationView],
) -> Result<()> {
    if changes.is_empty() {
        return Ok(());
    }
    let t = now();
    let trx = ctx.begin();
    if let Some((id, status)) = changes.message {
        set_message_status(&trx, messages, id, status, by, t)?;
    }
    for (id, status) in &changes.steps {
        set_step_status(&trx, relations, *id, *status, by, t)?;
    }
    trx.commit().await?;
    Ok(())
}

/// Moves the message `id`, one of `messages`, to `status` in `trx`: resolved, with `by` and the
/// time `t` as who resolved it and when, or back to active, forgetting both.
fn set_message_status(
    trx: &ankurah::transaction::Transaction,
    messages: &[MessageView],
    id: EntityId,
    status: MessageStatus,
    by: EntityId,
    t: i64,
) -> Result<()> {
    let view = messages.iter().find(|m| m.id() == id).ok_or_else(|| anyhow!("message {id} is not loaded"))?;
    let m = view.edit(trx)?;
    let resolved = status == MessageStatus::Resolved;
    m.status()?.set(&status)?;
    m.resolved_by()?.set(&resolved.then(|| Ref::new(by)))?;
    m.resolved()?.set(&resolved.then_some(t))?;
    Ok(())
}

/// Moves the step `id`, one of `relations`, to `status` in `trx`: checked, with `by` and the time
/// `t` as who checked it and when, or unchecked, forgetting both.
fn set_step_status(
    trx: &ankurah::transaction::Transaction,
    relations: &[RelationView],
    id: EntityId,
    status: RelationStatus,
    by: EntityId,
    t: i64,
) -> Result<()> {
    let view = relations.iter().find(|r| r.id() == id).ok_or_else(|| anyhow!("step {id} is not loaded"))?;
    let r = view.edit(trx)?;
    let checked = status == RelationStatus::Resolved;
    r.status()?.set(&status)?;
    r.closed_by()?.set(&checked.then(|| Ref::new(by)))?;
    r.closed()?.set(&checked.then_some(t))?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Tasks

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateTask {
    pub title: String,
    pub what: String,
    pub why: String,
    pub done_when: String,
    pub propose: bool,
    pub needs_review: bool,
    pub priority: Option<i64>,
    pub labels: Vec<String>,
    /// `depends_on` targets.
    pub after: Vec<EntityId>,
    /// `subtask_of` target.
    pub under: Option<EntityId>,
    /// `discovered_from` target (a task or a message).
    pub from: Option<EntityId>,
    /// `motivated_by` target (a task or a message).
    pub motivated_by: Option<EntityId>,
    /// The scope the task is made in, which the command that makes it works in (`fwd task create`'s
    /// `--scope`, else the scope its checkout finds; the page's scope selector): the `worked_in`
    /// target. One, since a command works in one scope; a task comes to be worked in more
    /// afterwards (`fwd link <task> worked_in <scope>`, the scopes on its panel), as it may since
    /// D136 ("A task may be worked in several scopes"). Until 2026-10-03 the task's context
    /// recorded it too (`scope_id`), which nothing reads now.
    pub scope: Option<EntityId>,
}

/// Makes a task, with the relations its spec names from it: `depends_on` each of `after`,
/// `subtask_of` `under`, `discovered_from` `from`, `motivated_by` `motivated_by` and `worked_in`
/// `scope`, so that a task made in a scope is found there and started only there from the start,
/// where until 2026-09-30 it was worked in none until someone started it (task cToJvD22). Each
/// relation is refused, before anything is written, where [`link`] would refuse it: a target not
/// of a kind its relation joins, or one that names nothing in the project, with the sentence `link`
/// gives ([`ends_refused`]; task iDJpHjiB), and a dependency or a parent that would close a cycle.
pub async fn create_task(ctx: &Context, by: EntityId, spec: CreateTask) -> Result<EntityId> {
    // Each label in the form it is stored in, each once; a blank one, or one holding a comma, is
    // refused before anything is written.
    let labels = crate::labels::list(&spec.labels)?;
    let mut rels: Vec<(RelationKind, EntityId)> = spec.after.iter().map(|a| (RelationKind::DependsOn, *a)).collect();
    if let Some(u) = spec.under {
        rels.push((RelationKind::SubtaskOf, u));
    }
    if let Some(f) = spec.from {
        rels.push((RelationKind::DiscoveredFrom, f));
    }
    if let Some(m) = spec.motivated_by {
        rels.push((RelationKind::MotivatedBy, m));
    }
    if let Some(s) = spec.scope {
        rels.push((RelationKind::WorkedIn, s));
    }
    // Each relation starts from the task made here, which `probe` stands for in the checks: one
    // whose target is not of a kind it joins is refused as `link` refuses it, before anything is
    // written.
    let probe = EntityId::random();
    for (kind, to) in &rels {
        if let Some(sentence) = ends_refused(*kind, (probe, Some(Kind::Task)), (*to, end_kind(ctx, *to).await)) {
            bail!("refused: {sentence}");
        }
    }
    // Pre-check every dependency before writing anything.
    let dep_edges = holding_edges(ctx, RelationKind::DependsOn).await?;
    let sub_edges = holding_edges(ctx, RelationKind::SubtaskOf).await?;
    for after in &spec.after {
        if rules::would_close_cycle(RelationKind::DependsOn, probe, *after, &dep_edges).is_some() {
            bail!("refused: depending on {after} would close a cycle");
        }
    }
    if let Some(under) = spec.under {
        if rules::would_close_cycle(RelationKind::SubtaskOf, probe, under, &sub_edges).is_some() {
            bail!("refused: nesting under {under} would close a cycle");
        }
    }
    let mut context = json!({});
    if let Some(under) = spec.under {
        context["parent_id"] = json!(under.to_base64());
    }
    let t = now();
    let trx = ctx.begin();
    let task = trx
        .create(&Task {
            title: spec.title.clone(),
            what: spec.what.clone(),
            why: spec.why.clone(),
            done_when: spec.done_when.clone(),
            status: if spec.propose { TaskStatus::Proposed } else { TaskStatus::Open },
            needs_review: spec.needs_review,
            hold: None,
            assignee: None,
            priority: spec.priority,
            labels: Json::new(json!(labels)),
            context: Json::new(context),
            created_by: Ref::new(by),
            created: t,
            updated: t,
            closed: None,
            closed_reason: None,
        })
        .await?
        .id();
    let via = json!({"actor_id": by.to_base64()});
    for (kind, to) in rels {
        trx.create(&Relation {
            kind,
            from_id: task,
            to_id: to,
            status: RelationStatus::Accepted,
            created_by: Ref::new(by),
            created: t,
            via: Json::new(via.clone()),
            note: None,
            position: None,
            closed_by: None,
            closed: None,
            closed_reason: None,
        })
        .await?;
    }
    trx.commit().await?;
    Ok(task)
}

/// The accepted `depends_on` targets of a task with their statuses (`None` when unavailable).
pub async fn dependencies(ctx: &Context, task: EntityId) -> Result<Vec<(EntityId, Option<TaskStatus>)>> {
    let rels = relations_from(ctx, task, Some(RelationKind::DependsOn)).await?;
    let mut out = Vec::new();
    for r in rels {
        if r.status()? != RelationStatus::Accepted {
            continue;
        }
        let target: EntityId = r.to_id()?;
        let status = match ctx.get::<TaskView>(target).await {
            Ok(t) => t.status().ok(),
            Err(_) => None,
        };
        out.push((target, status));
    }
    Ok(out)
}

pub async fn readiness(ctx: &Context, task: &TaskView) -> Result<Readiness> {
    let deps = dependencies(ctx, task.id()).await?;
    Ok(rules::readiness(&TaskSnapshot { id: task.id(), status: task.status()?, hold: task.hold()? }, &deps))
}

/// The scopes a task is worked in: the targets of its accepted `worked_in` relations, any number of
/// them (docs/DECISIONS.md, D136, "A task may be worked in several scopes", 2026-10-03), none of
/// them primary, in the order the task came to be worked in them (each relation's `created`, then
/// its id), so that every reader lists them alike. They are also the scopes whose triage agents
/// answer first for what concerns the task: a comment on it, its proposal, its review, its becoming
/// ready while nobody holds it (D122). None for a task worked in no scope, for which the triage
/// agent that serves the whole project answers. Until 2026-10-03 a task had one scope, and routing
/// read, for a task worked in none, the scope its context recorded it was made in (`scope_id`),
/// which nothing reads now: `fwd admin migrate` gave the tasks made in a scope before 2026-09-30,
/// which only their context placed there, a `worked_in` to it ([`migrate_recorded_scopes`]).
pub async fn worked_in(ctx: &Context, task: EntityId) -> Result<Vec<EntityId>> {
    Ok(scopes_in(task, &relations_from(ctx, task, Some(RelationKind::WorkedIn)).await?))
}

/// The scopes `task` is worked in, read from `relations`, which may hold any relations, those of
/// other kinds, from other entities or not accepted being passed over: the targets of the task's
/// accepted `worked_in` relations, and of an accepted `assigned_to`, that relation's name until
/// 2026-09-28, which a graph `fwd admin migrate` has not carried over still holds; each scope once,
/// in the order the task came to be worked in them (each relation's `created`, then its id). The
/// one rule [`worked_in`] and the page order a task's scopes by, so that they list them alike.
pub fn scopes_in(task: EntityId, relations: &[RelationView]) -> Vec<EntityId> {
    let mut found: Vec<(i64, EntityId, EntityId)> = relations
        .iter()
        .filter(|r| {
            r.from_id().ok() == Some(task)
                && matches!(r.kind(), Ok(RelationKind::WorkedIn | RelationKind::AssignedTo))
                && r.status().ok() == Some(RelationStatus::Accepted)
        })
        .filter_map(|r| Some((r.created().ok()?, r.id(), r.to_id().ok()?)))
        .collect();
    found.sort();
    let mut scopes: Vec<EntityId> = Vec::with_capacity(found.len());
    for (_, _, scope) in found {
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }
    scopes
}

/// Who a task is assigned to, if anyone; a task written before the field existed has no one.
pub fn assignee_of(task: &TaskView) -> Result<Option<EntityId>> {
    Ok(task.assignee()?.map(EntityId::from))
}

/// The key in a task's `context` naming who last set its assignee (a principal id), so routing
/// can leave them out of the notification the assignment causes. `fwd admin migrate` writes
/// [`MIGRATED`] there instead, for an assignee it carried over from an `assigned_to` relation.
pub const ASSIGNED_BY_KEY: &str = "assigned_by";

/// What [`ASSIGNED_BY_KEY`] holds for an assignment the migration carried over, which is no news
/// to anyone: the assignee heard of it when the relation was made.
pub const MIGRATED: &str = "migration";

/// The frontier: ready tasks, highest priority first, those assigned to `me` before unassigned
/// ones. Tasks assigned to someone else are left out.
pub async fn ready_tasks(ctx: &Context, me: Option<EntityId>) -> Result<Vec<TaskView>> {
    let open: Vec<TaskView> = ctx.fetch("status = 'open'").await?;
    let mut ranked: Vec<(u8, i64, TaskView)> = Vec::new();
    for t in open {
        if !readiness(ctx, &t).await?.is_ready() {
            continue;
        }
        let rank = match assignee_of(&t)? {
            None => 1,
            Some(a) if Some(a) == me => 0,
            Some(_) => continue,
        };
        ranked.push((rank, t.priority()?.unwrap_or(9), t));
    }
    ranked.sort_by_key(|(rank, prio, t)| (*rank, *prio, t.created().unwrap_or(0)));
    Ok(ranked.into_iter().map(|(_, _, t)| t).collect())
}

/// The scopes a task is worked in ([`worked_in`]), each with what the start rule needs to name it.
async fn scopes_worked_in(ctx: &Context, task: EntityId) -> Result<Vec<rules::ScopeSnapshot>> {
    let mut out = Vec::new();
    for id in worked_in(ctx, task).await? {
        let scope = ctx.get::<ScopeView>(id).await.ok();
        out.push(rules::ScopeSnapshot {
            id,
            title: scope.as_ref().and_then(|s| s.title().ok()).unwrap_or_default(),
            status: scope.as_ref().and_then(|s| s.status().ok()),
        });
    }
    Ok(out)
}

/// The task `fwd task next` offers `me`, who works in `scope`: the first task of the frontier
/// ([`ready_tasks`]) that [`start`] would let them start there. A task none of whose scopes is
/// `scope` is left for whoever works in them, so that a claim never ends in the refusal a start
/// from the wrong scope gets.
pub async fn next_task(ctx: &Context, me: EntityId, scope: Option<EntityId>) -> Result<Option<TaskView>> {
    for t in ready_tasks(ctx, Some(me)).await? {
        if rules::can_start_in(scope, &scopes_worked_in(ctx, t.id()).await?).is_ok() {
            return Ok(Some(t));
        }
    }
    Ok(None)
}

/// Starts a task for `by`, in one transaction after the start pre-checks: Open to InProgress, and
/// `by` its assignee (docs/DECISIONS.md, "Assignment is an optional field on the task, never an
/// edge", 2026-09-28: "a task should assign when started, provided the identity of the starting
/// party is known"). A start never moves a task out of the scopes someone chose for it: with
/// `scope`, the caller's active scope, a task worked in no scope comes to be worked in it, a task
/// worked in it (among other scopes or alone) starts as it is, and a task none of whose scopes is
/// it is refused, naming them and how to switch to one (`rules::can_start_in`). Without one (a
/// person need not have a scope) the task's scope relations stay as they are. Until 2026-10-03 the
/// start also wrote the caller's scope into the task's context (`scope_id`), which routing read
/// for a task worked in no scope; nothing reads it now, and the `worked_in` a start makes records
/// where the task was started, by whom and when.
pub async fn start(ctx: &Context, by: EntityId, task: EntityId, scope: Option<EntityId>) -> Result<()> {
    let view: TaskView = ctx.get(task).await?;
    match rules::can_start(view.status()?, assignee_of(&view)?, by) {
        Err(rules::RuleError::AssignedToAnother { assignee }) => bail!(
            "task is assigned to {}; only its assignee starts it (`fwd task assign <task> @<you>` first)",
            at_handle(ctx, assignee).await
        ),
        other => other?,
    }
    let worked_in = scopes_worked_in(ctx, task).await?;
    rules::can_start_in(scope, &worked_in)?;
    let next = rules::transition(view.status()?, Verb::Start, view.needs_review()?)?;
    let t = now();
    let trx = ctx.begin();
    let m = view.edit(&trx)?;
    m.status()?.set(&next)?;
    m.updated()?.set(&t)?;
    m.assignee()?.set(&Some(Ref::new(by)))?;
    let mut context: Value = view.context()?.into();
    context[ASSIGNED_BY_KEY] = json!(by.to_base64());
    m.context()?.set(&Json::new(context))?;
    if let Some(scope) = scope {
        // The start rule let it through, so the task is worked in this scope already, among its
        // scopes, or in none.
        if worked_in.is_empty() {
            trx.create(&Relation {
                kind: RelationKind::WorkedIn,
                from_id: task,
                to_id: scope,
                status: RelationStatus::Accepted,
                created_by: Ref::new(by),
                created: t,
                via: Json::new(json!({"actor_id": by.to_base64()})),
                note: None,
                position: None,
                closed_by: None,
                closed: None,
                closed_reason: None,
            })
            .await?;
        }
    }
    trx.commit().await?;
    Ok(())
}

/// [`start`] in a scope, the form the page and the extension host call with the scope they offer.
pub async fn claim(ctx: &Context, by: EntityId, task: EntityId, scope: EntityId) -> Result<()> {
    start(ctx, by, task, Some(scope)).await
}

/// Sets a task's assignee to `assignee` (a principal), or clears it with none; routing tells the
/// new assignee, unless it is `by` (agents-and-inbox.md, rule 7). Setting the assignee it already
/// has changes nothing.
pub async fn assign(ctx: &Context, by: EntityId, task: EntityId, assignee: Option<EntityId>) -> Result<()> {
    let view: TaskView = ctx.get(task).await.context("that task")?;
    if let Some(a) = assignee {
        ctx.get::<PrincipalView>(a).await.map_err(|_| anyhow!("{a} is not a person or an agent of this project"))?;
    }
    if assignee_of(&view)? == assignee {
        return Ok(());
    }
    let trx = ctx.begin();
    let m = view.edit(&trx)?;
    m.assignee()?.set(&assignee.map(Ref::new))?;
    let mut context: Value = view.context()?.into();
    context[ASSIGNED_BY_KEY] = json!(by.to_base64());
    m.context()?.set(&Json::new(context))?;
    m.updated()?.set(&now())?;
    trx.commit().await?;
    Ok(())
}

/// What `fwd admin migrate` changed: the tasks it gave an assignee (with the scope whose maker that
/// assignee is), and the relations it renamed from `assigned_to` to `worked_in`. Until 2026-10-03
/// it also retracted every accepted one but the newest, a task being worked in one scope at a time
/// then (docs/DECISIONS.md, D136, "A task may be worked in several scopes").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Migration {
    /// (task, assignee, scope)
    pub assigned: Vec<(EntityId, EntityId, EntityId)>,
    pub renamed: Vec<EntityId>,
}

impl Migration {
    pub fn is_empty(&self) -> bool {
        self.assigned.is_empty() && self.renamed.is_empty()
    }
}

/// Carries a graph from the days of assignment as a relation over to the assignee field
/// (docs/DECISIONS.md, "Assignment is an optional field on the task, never an edge", 2026-09-28):
/// every `assigned_to` relation, whatever its status, is renamed `worked_in`, which is what it now
/// means, so that a task is worked in each scope it was assigned to, as it may be in several
/// (D136, "A task may be worked in several scopes"; until 2026-10-03 the run kept the newest
/// accepted one and retracted the others). A task with an accepted `assigned_to`, no accepted
/// `worked_in` already and no assignee gets whoever made the scope of the newest of them as its
/// assignee (its owner, which was always whoever made it, until scopes lost their owner: D108).
/// One transaction per task, so a run that stops halfway leaves each task whole; a second run finds
/// nothing to do. The assignee it sets names [`MIGRATED`] as who assigned it, so routing tells no
/// one of it.
pub async fn migrate_assignments(ctx: &Context) -> Result<Migration> {
    let legacy: Vec<RelationView> = ctx.fetch("kind = 'assigned_to'").await?;
    let mut by_task: Vec<(EntityId, Vec<RelationView>)> = Vec::new();
    for r in legacy {
        let task = r.from_id()?;
        match by_task.iter_mut().find(|(t, _)| *t == task) {
            Some((_, rels)) => rels.push(r),
            None => by_task.push((task, vec![r])),
        }
    }
    let mut done = Migration::default();
    for (task, mut rels) in by_task {
        // Newest first: the newest accepted relation names the scope whose maker becomes the
        // assignee.
        rels.sort_by_key(|r| std::cmp::Reverse((r.created().unwrap_or(0), r.id())));
        let already = relations_from(ctx, task, Some(RelationKind::WorkedIn))
            .await?
            .iter()
            .any(|r| r.status().ok() == Some(RelationStatus::Accepted));
        let newest =
            if already { None } else { rels.iter().find(|r| r.status().ok() == Some(RelationStatus::Accepted)) };
        let view = ctx.get::<TaskView>(task).await.ok();
        let maker = match (&view, newest) {
            (Some(v), Some(r)) if assignee_of(v)?.is_none() => {
                let scope = r.to_id()?;
                ctx.get::<ScopeView>(scope)
                    .await
                    .ok()
                    .and_then(|s| s.created_by().ok())
                    .map(|by| (EntityId::from(by), scope))
            }
            _ => None,
        };
        let trx = ctx.begin();
        for r in &rels {
            r.edit(&trx)?.kind()?.set(&RelationKind::WorkedIn)?;
            done.renamed.push(r.id());
        }
        if let (Some(v), Some((maker, scope))) = (&view, maker) {
            let m = v.edit(&trx)?;
            m.assignee()?.set(&Some(Ref::new(maker)))?;
            let mut context: Value = v.context()?.into();
            context[ASSIGNED_BY_KEY] = json!(MIGRATED);
            m.context()?.set(&Json::new(context))?;
            done.assigned.push((task, maker, scope));
        }
        trx.commit().await?;
    }
    Ok(done)
}

/// The key a task's `context` named the scope it was made or started in by, until 2026-10-03:
/// what [`migrate_recorded_scopes`] reads, and nothing else.
const RECORDED_SCOPE_KEY: &str = "scope_id";

/// `fwd admin migrate`, the part that gives a task made in a scope before 2026-09-30, and never
/// started, the `worked_in` its making did not write: until then making a task recorded the scope
/// only in the task's context (`scope_id`), which routing read for a task worked in no scope until
/// 2026-10-03, when it stopped (docs/DECISIONS.md, D136, "A task may be worked in several scopes";
/// [`worked_in`]). Each task whose context names a scope the graph holds and which has no
/// `worked_in` relation of any status comes to be worked in that scope, by an accepted relation
/// that `by` makes, its `via` naming the migration; a task with a `worked_in` relation, even one
/// taken off since, keeps the scopes it has, so a scope someone took off stays off. Returns each
/// task with the scope it now is worked in. One transaction per task; a second run finds nothing
/// to do. An accepted `worked_in` tells no one (`routing::plan_for_relation`).
pub async fn migrate_recorded_scopes(ctx: &Context, by: EntityId) -> Result<Vec<(EntityId, EntityId)>> {
    let mut placed = Vec::new();
    for task in ctx.fetch::<TaskView>("created > 0").await? {
        let context: Value = task.context()?.into();
        let recorded = context.get(RECORDED_SCOPE_KEY).and_then(Value::as_str).and_then(|s| s.parse().ok());
        let Some(scope) = recorded else { continue };
        if ctx.get::<ScopeView>(scope).await.is_err()
            || !relations_from(ctx, task.id(), Some(RelationKind::WorkedIn)).await?.is_empty()
        {
            continue;
        }
        let via = json!({"actor_id": by.to_base64(), "migrated": RECORDED_SCOPE_KEY});
        let relation =
            new_relation(by, RelationKind::WorkedIn, (task.id(), scope), RelationStatus::Accepted, via, None, None);
        let trx = ctx.begin();
        trx.create(&relation).await?;
        trx.commit().await?;
        placed.push((task.id(), scope));
    }
    Ok(placed)
}

/// The word the role triage was stored as until 2026-09-29, `supervisor` (docs/DECISIONS.md, D106):
/// what [`migrate_roles`] carries over, and what a question addressed to that role held in `to`.
const OLD_ROLE_WORD: &str = "supervisor";

/// What `fwd admin migrate` did to carry a project's graph over from the supervisor role to the
/// fixed roles, and to record who belongs to the project ([`migrate_roles`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoleMigration {
    /// The principal that held `supervisor` and holds triage now: of those that held it, the one
    /// that wrote most recently. None when no principal held it, or when an agent held triage
    /// already.
    pub triage: Option<EntityId>,
    /// The other principals that held `supervisor`: they hold no role now, and are named for a
    /// person to retire, since each is refused its next write until it selects a role.
    pub roles_cleared: Vec<EntityId>,
    /// The questions addressed to the supervisor role (`to` holding `supervisor`), asked of the
    /// project now (`asks`).
    pub readdressed: Vec<EntityId>,
    /// The principals the run recorded as belonging to the project.
    pub members: Vec<EntityId>,
    /// The agents whose only things in the graph are messages (with the mentions and the
    /// highlights their messages made), with how many messages each wrote: they acted in the
    /// project without belonging to it, and the run records them as not belonging, for a person to
    /// retire, so that a later message of theirs does not make them belong, or to add.
    pub visitors: Vec<(EntityId, usize)>,
    /// The agents with nothing of theirs in the graph: they only read it, and the run records them
    /// as not belonging.
    pub readers: Vec<EntityId>,
}

/// `fwd admin migrate`, the part that carries a graph over from the supervisor role to the fixed
/// roles and records who belongs to the project (docs/design/agents-and-inbox.md, "Roles" and "Who
/// belongs to a project"; docs/design/cli.md, "Maintenance"). It translates the stored role word
/// and gives no other role: an agent without one selects its own. In order:
///
/// - Of the principals whose stored role is `supervisor`, the one that wrote most recently holds
///   triage: the same principal under its own handle, so its history and its inbox stay its own.
///   It serves nothing, as every triage agent from before 2026-09-30 does, so nothing reaches it
///   for its role until its lead binds it to a scope or endorses it for the whole project
///   (docs/DECISIONS.md, D122; [`serve`]). The others hold no role, and are named for retirement.
///   Where an agent holds triage already, every one of them holds no role. `implementer` is left
///   as it is.
/// - Every question addressed to the role (`to` holding `supervisor`) is asked of the project
///   instead (`asks`), one transaction each.
/// - Each principal that does not say whether it belongs is recorded: a person belongs, the
///   principal given triage belongs, an agent belongs when it did work in the project (a task it
///   made or holds, a relation it made that is not a mention, a decision it filed or ruled on, a
///   scope it made), and an agent does not when its only things there are messages or when it has
///   none.
///
/// Nothing is recorded twice, so a second run changes nothing.
pub async fn migrate_roles(ctx: &Context) -> Result<RoleMigration> {
    let mut done = RoleMigration::default();
    // The stored role word, which this build's `AgentRole` does not read.
    let held: Vec<PrincipalView> = ctx.fetch(format!("agent_role = '{OLD_ROLE_WORD}'").as_str()).await?;
    let mut latest: Option<(i64, EntityId)> = None;
    let triage_held = holding(ctx, AgentRole::Triage).await?.iter().any(|p| holds(p, AgentRole::Triage));
    if !triage_held {
        for p in &held {
            let at = (last_wrote(ctx, p.id()).await?, p.id());
            if latest.is_none_or(|l| at > l) {
                latest = Some(at);
            }
        }
    }
    for p in &held {
        let trx = ctx.begin();
        let m = p.edit(&trx)?;
        if Some(p.id()) == latest.map(|(_, id)| id) {
            m.agent_role()?.set(&Some(AgentRole::Triage))?;
            done.triage = Some(p.id());
        } else {
            m.agent_role()?.set(&None)?;
            done.roles_cleared.push(p.id());
        }
        trx.commit().await?;
    }
    let messages: Vec<MessageView> = ctx.fetch("created > 0").await?;
    for m in messages {
        let context: Value = m.context()?.into();
        if context.get("to").and_then(Value::as_str) != Some(OLD_ROLE_WORD) {
            continue;
        }
        let mut asked = context.clone();
        if let Some(fields) = asked.as_object_mut() {
            fields.remove("to");
            fields.insert(ASKS_KEY.to_string(), json!(ASKS_PROJECT));
        }
        let trx = ctx.begin();
        let edit = m.edit(&trx)?;
        edit.context()?.set(&Json::new(asked))?;
        trx.commit().await?;
        done.readdressed.push(m.id());
    }
    for p in principals(ctx).await? {
        if p.member()?.is_some() {
            continue;
        }
        let id = p.id();
        let belongs = p.kind()? == PrincipalKind::Human || Some(id) == done.triage || did_work(ctx, id).await?;
        let trx = ctx.begin();
        let m = p.edit(&trx)?;
        m.member()?.set(&Some(belongs))?;
        trx.commit().await?;
        if belongs {
            done.members.push(id);
        } else {
            match ctx.fetch::<MessageView>(with_id("author = ?", id)?).await?.len() {
                0 => done.readers.push(id),
                written => done.visitors.push((id, written)),
            }
        }
    }
    Ok(done)
}

/// What `fwd admin migrate` took off the scopes ([`migrate_scopes`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopeMigration {
    /// The scopes that held an owner or a heartbeat, which the run cleared.
    pub cleared: Vec<EntityId>,
}

/// `fwd admin migrate`, the part that takes off each scope the owner and the heartbeat it had until
/// 2026-09-29, when scopes lost their owner (docs/DECISIONS.md, D108, "A scope has no owner: scopes
/// are not assigned to principals"; docs/design/data-model.md, Scope): `owner`, `last_heartbeat`
/// and `heartbeat_ttl_s`, which [`Scope`] no longer declares, so that no stored value still says
/// whose a scope was. Nothing else about a scope changes, and its `created_by` still says who made
/// it. The fields are written through [`before_d108::Scope`], which declares them still, each
/// optional so that it can be set to none; registering it marks them optional in the project's
/// schema catalog too, where the scope's owner and time to live were required. One transaction
/// per scope; a scope that holds none of the three is left alone, so a second run changes nothing.
pub async fn migrate_scopes(ctx: &Context) -> Result<ScopeMigration> {
    let mut done = ScopeMigration::default();
    let scopes: Vec<ScopeView> = ctx.fetch("created > 0").await?;
    for scope in scopes {
        let before: before_d108::ScopeView = ctx.get(scope.id()).await?;
        let held =
            before.owner()?.is_some() || before.last_heartbeat()?.is_some() || before.heartbeat_ttl_s()?.is_some();
        if !held {
            continue;
        }
        let trx = ctx.begin();
        let m = before.edit(&trx)?;
        m.owner()?.set(&None)?;
        m.last_heartbeat()?.set(&None)?;
        m.heartbeat_ttl_s()?.set(&None)?;
        trx.commit().await?;
        done.cleared.push(scope.id());
    }
    Ok(done)
}

/// When `principal` last wrote in the project, for [`migrate_roles`]: the latest time among the
/// messages it wrote and the tasks, relations, decisions and scopes it made; 0 when it wrote
/// nothing.
async fn last_wrote(ctx: &Context, principal: EntityId) -> Result<i64> {
    let said: Vec<MessageView> = ctx.fetch(with_id("author = ?", principal)?).await?;
    let made: Vec<TaskView> = ctx.fetch(with_id("created_by = ?", principal)?).await?;
    let related: Vec<RelationView> = ctx.fetch(with_id("created_by = ?", principal)?).await?;
    let filed: Vec<DecisionView> = ctx.fetch(with_id("author = ?", principal)?).await?;
    let scoped: Vec<ScopeView> = ctx.fetch(with_id("created_by = ?", principal)?).await?;
    let times = said
        .iter()
        .filter_map(|m| m.created().ok())
        .chain(made.iter().filter_map(|t| t.created().ok()))
        .chain(related.iter().filter_map(|r| r.created().ok()))
        .chain(filed.iter().filter_map(|d| d.created().ok()))
        .chain(scoped.iter().filter_map(|s| s.created().ok()));
    Ok(times.max().unwrap_or(0))
}

/// Whether `principal` did work in the project, for [`migrate_roles`]: it made a task or holds one,
/// made a relation that is not a mention (a mention comes with a message), filed or ruled on a
/// decision, or made a scope.
async fn did_work(ctx: &Context, principal: EntityId) -> Result<bool> {
    let made: Vec<TaskView> = ctx.fetch(with_id("created_by = ?", principal)?).await?;
    let holds: Vec<TaskView> = ctx.fetch(with_id("assignee = ?", principal)?).await?;
    if !made.is_empty() || !holds.is_empty() {
        return Ok(true);
    }
    let related: Vec<RelationView> = ctx.fetch(with_id("created_by = ?", principal)?).await?;
    if related.iter().any(|r| r.kind().ok() != Some(RelationKind::Mention)) {
        return Ok(true);
    }
    let filed: Vec<DecisionView> = ctx.fetch(with_id("author = ?", principal)?).await?;
    let ruled: Vec<DecisionView> = ctx.fetch(with_id("decided_by = ?", principal)?).await?;
    let made_scopes: Vec<ScopeView> = ctx.fetch(with_id("created_by = ?", principal)?).await?;
    Ok(!filed.is_empty() || !ruled.is_empty() || !made_scopes.is_empty())
}

/// Applies a status verb. `note` is the evidence for submit, the reason for reject, return,
/// cancel, reopen and rejustify, and the acceptance note for accept. A reason is kept as a comment
/// on the task by `by`, written in the same transaction ([`task_move_comment`]), so that whoever
/// reads the task learns why it moved: until 2026-10-03 the reason for a return, a reopen or a
/// rejustify was asked for and stored nowhere (task jJp3bcF7). A reject's and a cancel's are also
/// the task's `closed_reason`, as before.
pub async fn transition(
    ctx: &Context,
    by: EntityId,
    task: EntityId,
    verb: Verb,
    note: Option<&str>,
) -> Result<TaskStatus> {
    let view: TaskView = ctx.get(task).await?;
    let mut context: Value = view.context()?.into();
    if verb == Verb::Accept {
        if let Some(submitter) =
            context.get("submitted_by").and_then(Value::as_str).and_then(|s| s.parse::<EntityId>().ok())
        {
            rules::can_accept(submitter, by)?;
        }
    }
    let next = rules::transition(view.status()?, verb, view.needs_review()?)?;
    // What the CLI makes mandatory holds for every client: the evidence and the reasons are
    // what a reviewer, a requester or a later reader gets to see.
    let wants = match verb {
        Verb::Submit => Some("the evidence (commits, test runs)"),
        Verb::Return => Some("what should change"),
        Verb::Reject | Verb::Cancel | Verb::Reopen => Some("why"),
        Verb::Rejustify => Some("the new reason"),
        _ => None,
    };
    if let Some(what) = wants {
        if note.map(str::trim).unwrap_or("").is_empty() {
            bail!("{} needs a note saying {what}", format!("{verb:?}").to_lowercase());
        }
    }
    // The reason, read as a post reads its text before the move is made, so that what it names
    // draws by name and a reference that names nothing refuses the move, as a decision's move does.
    let reason = match (task_move_opening(verb), note.map(str::trim).filter(|n| !n.is_empty())) {
        (Some(opening), Some(note)) => Some((opening, read_reason(ctx, by, note).await?)),
        _ => None,
    };
    let t = now();
    match verb {
        Verb::Submit => {
            context["submitted_by"] = json!(by.to_base64());
            context["evidence"] = json!(note.unwrap_or(""));
        }
        Verb::Accept => {
            context["accepted_by"] = json!(by.to_base64());
        }
        // Whoever approved the work is asked to review it when no triage agent can take the
        // review: none serves the task's scopes or the whole project, or the only ones that do
        // submitted it (routing, rule 8).
        Verb::Approve => {
            context["approved_by"] = json!(by.to_base64());
        }
        _ => {}
    }
    let trx = ctx.begin();
    let m = view.edit(&trx)?;
    m.status()?.set(&next)?;
    m.updated()?.set(&t)?;
    m.context()?.set(&Json::new(context))?;
    if matches!(next, TaskStatus::Done | TaskStatus::Cancelled | TaskStatus::Rejected) {
        m.closed()?.set(&Some(t))?;
        m.closed_reason()?.set(&note.map(str::to_string))?;
    } else if verb == Verb::Reopen {
        m.closed()?.set(&None)?;
        m.closed_reason()?.set(&None)?;
    }
    if let Some((opening, reason)) = &reason {
        task_move_comment(&trx, by, task, opening, reason).await?;
    }
    trx.commit().await?;
    Ok(next)
}

/// The words the comment keeping a task move's reason opens with, before its colon and the reason,
/// for the moves whose note is a reason: what should change on a return, why on a reject, a cancel
/// or a reopen, the new reason on a rejustify; its lowercase is the move the comment names in
/// `context.move` ([`MOVE_KEY`]). None for every other move: a submission keeps its evidence in the
/// task's context, which the task's panel and the review rows show, and repeats it nowhere, so
/// that it does not reach the task's reviewer twice (the lead's ruling of 2026-10-03 on task
/// jJp3bcF7).
fn task_move_opening(verb: Verb) -> Option<&'static str> {
    match verb {
        Verb::Return => Some("Returned"),
        Verb::Reject => Some("Rejected"),
        Verb::Cancel => Some("Cancelled"),
        Verb::Reopen => Some("Reopened"),
        Verb::Rejustify => Some("Rejustified"),
        _ => None,
    }
}

/// Records why a task moved, in the move's own transaction `trx`: the comment on `task` by
/// `author`, "Returned: <what should change>", naming the move in `context.move`, with a `mention`
/// relation to each thing the reason names, as a decision's move records its reason
/// ([`decision_comment`]). It is a comment on the task like any other, so routing tells the task's
/// assignee and its scope's triage agent of it (`agents-and-inbox.md`, rule 2), never its author.
async fn task_move_comment(
    trx: &ankurah::transaction::Transaction,
    author: EntityId,
    task: EntityId,
    opening: &str,
    reason: &Reason,
) -> Result<EntityId> {
    let t = now();
    let id = trx
        .create(&Message {
            author: Ref::new(author),
            created: t,
            content: format!("{opening}: {}", reason.text),
            priority: Priority::Next,
            status: MessageStatus::Active,
            task: Some(Ref::new(task)),
            anchor: None,
            scope: None,
            decision: None,
            reply_to: None,
            thread: None,
            context: Json::new(json!({ MOVE_KEY: opening.to_lowercase() })),
            resolved_by: None,
            resolved: None,
            edited: None,
            edited_by: None,
        })
        .await?
        .id();
    write_mentions(trx, author, id, &reason.named, WrittenBy::Post, t).await?;
    Ok(id)
}

#[derive(Debug, Clone, Default)]
pub struct Amend {
    pub title: Option<String>,
    pub what: Option<String>,
    pub why: Option<String>,
    pub why_append: Option<String>,
    pub done_when: Option<String>,
    pub done_when_append: Option<String>,
    pub priority: Option<Option<i64>>,
    pub hold: Option<Option<String>>,
    /// Labels to add, as typed ([`label_task`]).
    pub label: Vec<String>,
    /// Labels to take off, as typed ([`unlabel_task`]).
    pub unlabel: Vec<String>,
}

pub async fn amend(ctx: &Context, task: EntityId, amend: Amend) -> Result<()> {
    let view: TaskView = ctx.get(task).await?;
    // Worked out before the transaction, so that a label refused leaves the task as it was.
    let labels = relabelled(&view, &amend.label, &amend.unlabel)?;
    let trx = ctx.begin();
    let m = view.edit(&trx)?;
    if let Some(v) = amend.title {
        m.title()?.set(&v)?;
    }
    if let Some(v) = amend.what {
        m.what()?.replace(&v)?;
    }
    if let Some(v) = amend.why {
        m.why()?.replace(&v)?;
    }
    if let Some(v) = amend.why_append {
        let current = text(view.why())?;
        m.why()?.replace(&append_paragraph(&current, &v))?;
    }
    if let Some(v) = amend.done_when {
        m.done_when()?.replace(&v)?;
    }
    if let Some(v) = amend.done_when_append {
        let current = text(view.done_when())?;
        m.done_when()?.replace(&append_paragraph(&current, &v))?;
    }
    if let Some(v) = amend.priority {
        m.priority()?.set(&v)?;
    }
    if let Some(v) = amend.hold {
        m.hold()?.set(&v)?;
    }
    if let Some(v) = labels {
        m.labels()?.set(&Json::new(json!(v)))?;
    }
    m.updated()?.set(&now())?;
    trx.commit().await?;
    Ok(())
}

fn append_paragraph(current: &str, addition: &str) -> String {
    if current.trim().is_empty() {
        addition.to_string()
    } else {
        format!("{}\n\n{}", current.trim_end(), addition)
    }
}

// ---------------------------------------------------------------------------------------------
// Labels

/// A task's labels, as they are stored: the words of its `labels` list, in order (`labels.rs`
/// says what a label is). A task whose list is missing has none.
pub fn labels_of(task: &TaskView) -> Result<Vec<String>> {
    let stored: Value = match task.labels() {
        Ok(json) => json.into(),
        Err(ankurah::property::PropertyError::Missing) => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    Ok(stored.as_array().into_iter().flatten().filter_map(|l| l.as_str().map(str::to_string)).collect())
}

/// Gives `task` the label `label`, stored in its one form (`labels::label`: trimmed, single
/// spaces, lower case), so that people and agents can find the task by its kind and filter by it
/// (docs/DECISIONS.md, D97, "A task's kind, such as bug or feature, is a label, not a task type").
/// A blank label, or one holding a comma, is refused; labelling a task with a label it has
/// changes nothing. No principal is recorded against a label, so none is asked for, as [`amend`]
/// asks for none.
pub async fn label_task(ctx: &Context, task: EntityId, label: &str) -> Result<()> {
    change_labels(ctx, task, &[label.to_string()], &[]).await
}

/// Takes the label `label` off `task`, however it was written there (`labels::same`). Taking off a
/// label the task does not have changes nothing.
pub async fn unlabel_task(ctx: &Context, task: EntityId, label: &str) -> Result<()> {
    change_labels(ctx, task, &[], &[label.to_string()]).await
}

/// Adds `add` to the task's labels and takes `remove` off them in one transaction, which also
/// records when the task changed; a change that leaves the labels as they were writes nothing.
async fn change_labels(ctx: &Context, task: EntityId, add: &[String], remove: &[String]) -> Result<()> {
    let view: TaskView = ctx.get(task).await.context("that task")?;
    let Some(labels) = relabelled(&view, add, remove)? else {
        return Ok(());
    };
    let trx = ctx.begin();
    let m = view.edit(&trx)?;
    m.labels()?.set(&Json::new(json!(labels)))?;
    m.updated()?.set(&now())?;
    trx.commit().await?;
    Ok(())
}

/// The labels `task` has once `add` is added and `remove` taken off (`labels::relabel`), or none
/// when that leaves them as they are.
fn relabelled(task: &TaskView, add: &[String], remove: &[String]) -> Result<Option<Vec<String>>> {
    if add.is_empty() && remove.is_empty() {
        return Ok(None);
    }
    let current = labels_of(task)?;
    let next = crate::labels::relabel(&current, add, remove)?;
    Ok((next != current).then_some(next))
}

// ---------------------------------------------------------------------------------------------
// Scopes

/// A new scope, as `fwd scope create` and the Scopes page's panel make one; whoever creates it
/// owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateScope {
    /// Its name: free text for people, anything but a blank.
    pub title: String,
    /// What the work in it is trying to achieve.
    pub intention: String,
    /// The upstream of the repository its branches are in, as `repo_key` writes it (a remote URL
    /// is taken too, and stored as its key).
    pub repo: Option<String>,
    pub branches: Vec<String>,
    pub status: ScopeStatus,
}

impl Default for CreateScope {
    /// An active scope with nothing filled in, which `create_scope` refuses until it has a name.
    fn default() -> Self {
        CreateScope {
            title: String::new(),
            intention: String::new(),
            repo: None,
            branches: Vec::new(),
            status: ScopeStatus::Active,
        }
    }
}

/// Makes a scope, under the rules an edit runs too ([`edit_scope`]): a blank name is refused, and
/// so are branches without their repository. A scope made planned has not started: it starts the
/// first time it becomes anything else. One made done or abandoned ends as it is made. `by` is
/// recorded as who made it, and decides nothing: anyone works in any scope (docs/DECISIONS.md,
/// D108, "A scope has no owner: scopes are not assigned to principals").
pub async fn create_scope(ctx: &Context, by: EntityId, spec: CreateScope) -> Result<EntityId> {
    scope_name(&spec.title)?;
    let (repo, branches) = scope_place(spec.repo, spec.branches)?;
    let t = now();
    let trx = ctx.begin();
    let id = trx
        .create(&Scope {
            status: spec.status,
            title: spec.title,
            intention: spec.intention,
            context: Json::new(json!({"repo": repo, "branches": branches})),
            started: (spec.status != ScopeStatus::Planned).then_some(t),
            ended: scope_closed(spec.status).then_some(t),
            created_by: Ref::new(by),
            created: t,
        })
        .await?
        .id();
    trx.commit().await?;
    Ok(id)
}

/// A change to a scope's fields: each one given replaces what the scope has, and each one left
/// `None` keeps it (`fwd scope rename`, `status` and `intention`, and the Scopes page's panel).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditScope {
    pub title: Option<String>,
    pub intention: Option<String>,
    /// A paragraph added after the intention (after `intention`, when both are given).
    pub intention_append: Option<String>,
    /// The repository its branches are in, as for [`CreateScope`]; `Some(None)` names none.
    pub repo: Option<Option<String>>,
    pub branches: Option<Vec<String>>,
    pub status: Option<ScopeStatus>,
}

impl EditScope {
    /// Whether it changes nothing.
    pub fn is_empty(&self) -> bool {
        *self == EditScope::default()
    }
}

/// Changes a scope's fields in one transaction, under the rules creation runs: a name cannot be
/// blank, and a change to the repository or the branches cannot leave branches without their
/// repository (a scope that already names branches without one keeps them through an edit that
/// touches neither). The repository and the branches live in the scope's context beside whatever
/// else is there; the single `branch` a scope from before 2026-09-25 carries is folded into the
/// list once either changes. A status that closes the scope (done, abandoned) records when it
/// ended and one that reopens it clears that, as reopening a task clears when it closed; the
/// first status past planned records when it started.
pub async fn edit_scope(ctx: &Context, scope: EntityId, edit: EditScope) -> Result<()> {
    if let Some(title) = &edit.title {
        scope_name(title)?;
    }
    let view: ScopeView = ctx.get(scope).await?;
    let mut context: Value = view.context()?.into();
    let place = edit.repo.is_some() || edit.branches.is_some();
    if place {
        if !context.is_object() {
            context = json!({});
        }
        let repo = edit.repo.unwrap_or_else(|| context["repo"].as_str().map(str::to_string));
        let branches = edit.branches.unwrap_or_else(|| scope_branches(&context));
        let (repo, branches) = scope_place(repo, branches)?;
        context["repo"] = json!(repo);
        context["branches"] = json!(branches);
        if let Some(map) = context.as_object_mut() {
            map.remove("branch");
        }
    }
    let intention = match (edit.intention, edit.intention_append) {
        (Some(set), Some(more)) => Some(append_paragraph(&set, &more)),
        (None, Some(more)) => Some(append_paragraph(&text(view.intention())?, &more)),
        (set, None) => set,
    };
    let t = now();
    let trx = ctx.begin();
    let m = view.edit(&trx)?;
    if let Some(v) = edit.title {
        m.title()?.set(&v)?;
    }
    if let Some(v) = intention {
        m.intention()?.replace(&v)?;
    }
    if place {
        m.context()?.set(&Json::new(context))?;
    }
    if let Some(status) = edit.status {
        let was = view.status()?;
        m.status()?.set(&status)?;
        if scope_closed(status) != scope_closed(was) {
            m.ended()?.set(&scope_closed(status).then_some(t))?;
        }
        if status != ScopeStatus::Planned && view.started()?.is_none() {
            m.started()?.set(&Some(t))?;
        }
    }
    trx.commit().await?;
    Ok(())
}

/// Whether a scope with this status has ended.
fn scope_closed(status: ScopeStatus) -> bool {
    matches!(status, ScopeStatus::Done | ScopeStatus::Abandoned)
}

/// Refuses a blank name. A scope's name is free text for people to read: nothing looks a scope up
/// by it, so any other text will do.
fn scope_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("a scope's name cannot be blank");
    }
    Ok(())
}

/// A scope's repository and branches as they are stored: the repository as its upstream's key
/// (`repo_key`, so a remote URL names the repository a checkout reports), each branch trimmed and
/// named once, blanks dropped. Branches without their repository are refused: a session finds its
/// scope by the repository and the branch its checkout is on, and a branch alone matches none.
fn scope_place(repo: Option<String>, branches: Vec<String>) -> Result<(Option<String>, Vec<String>)> {
    let repo = repo.map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).map(|r| repo_key(&r));
    let mut named: Vec<String> = Vec::new();
    for b in branches.iter().map(|b| b.trim()).filter(|b| !b.is_empty()) {
        if !named.iter().any(|n| n == b) {
            named.push(b.to_string());
        }
    }
    if !named.is_empty() && repo.is_none() {
        bail!(
            "a scope that names branches needs the repository they are in (its upstream, such as github.com/org/repo)"
        );
    }
    Ok((repo, named))
}

/// Gives a scope a new name (stored as its `title`); a blank one is refused.
pub async fn rename_scope(ctx: &Context, scope: EntityId, name: &str) -> Result<()> {
    edit_scope(ctx, scope, EditScope { title: Some(name.to_string()), ..EditScope::default() }).await
}

/// The branches a scope names: `branches` (a list, since 2026-09-25) plus the single `branch`
/// scopes created before then carry.
pub fn scope_branches(context: &Value) -> Vec<String> {
    let mut out: Vec<String> =
        context["branches"].as_array().into_iter().flatten().filter_map(|b| b.as_str().map(str::to_string)).collect();
    if let Some(b) = context["branch"].as_str() {
        if !out.iter().any(|x| x == b) {
            out.push(b.to_string());
        }
    }
    out
}

/// Rewrites a scope's context in place (its repository and branches live there).
pub async fn update_scope_context(ctx: &Context, scope: EntityId, edit: impl FnOnce(&mut Value)) -> Result<Value> {
    let view: ScopeView = ctx.get(scope).await?;
    let mut context: Value = view.context()?.into();
    if !context.is_object() {
        context = json!({});
    }
    edit(&mut context);
    let trx = ctx.begin();
    view.edit(&trx)?.context()?.set(&Json::new(context.clone()))?;
    trx.commit().await?;
    Ok(context)
}

/// Moves a scope to `status`, recording when it started and when it ended as [`edit_scope`] does.
pub async fn set_scope_status(ctx: &Context, scope: EntityId, status: ScopeStatus) -> Result<()> {
    edit_scope(ctx, scope, EditScope { status: Some(status), ..EditScope::default() }).await
}

// ---------------------------------------------------------------------------------------------
// Messages and notifications

/// The key in a message's `context` that marks a question asked of no one in particular, with
/// [`ASKS_PROJECT`] as its value: the triage agent of the scope it asks answers it
/// ([`ASKS_SCOPE_KEY`]), else the one that serves the whole project, and while neither exists it
/// waits, listed as asked of that scope, or of the project (docs/design/data-model.md, Message). A
/// reader sees it as it is, `"asks": "project"`, beside the `to` a question asked of someone has
/// instead.
pub const ASKS_KEY: &str = "asks";
/// What [`ASKS_KEY`] holds on a question asked of no one in particular.
pub const ASKS_PROJECT: &str = "project";
/// The key in a message's `context` that names, beside [`ASKS_KEY`], the scope a question asked of
/// no one in particular asks, whose triage agent answers it (docs/DECISIONS.md, D122): the scope
/// `fwd ask --scope` or the page's Ask box names, else a scope of what it is about (of a task's
/// several scopes, the one its asker wrote it in, else the first: `routing::scope_to_ask`), else
/// the scope its asker wrote it in ([`WRITTEN_IN_KEY`]), fixed as it is posted so that the routing
/// service, its catch-up and the Questions page read one answer. Absent where none of these finds a
/// scope, when the triage agent that serves the whole project answers it.
pub const ASKS_SCOPE_KEY: &str = "asks_scope";
/// The key in a message's `context` that names the scope its author wrote it in, the scope the
/// command found (`--scope`, else the one its checkout finds), so that what concerns no scope's work
/// (a comment on code no task is near, a question about nothing) reaches the triage agent of the
/// scope its author works in (docs/DECISIONS.md, D122, "else the asker's scope"). Absent where the
/// command found none.
pub const WRITTEN_IN_KEY: &str = "written_in";
/// The key in a message's `context` that names, by its `step_of` relation's id, the step that the
/// command which added the step wrote the message with as its first comment (`fwd step add <task>
/// --path ... <text>`, [`add_highlight_step`]), so that the step's list of the threads open on its
/// lines leaves the message out and rule 14 reports it as a comment on the step's highlight,
/// whichever of the two routing reaches first (task WTCAd9la): a causal fact its writer records,
/// where the times the step and the message were made with, each from its writer's clock, decide
/// nothing. Absent on every other message.
pub const FIRST_OF_STEP_KEY: &str = "first_of_step";

/// Where a message points. Subjects are typed fields on the model; whom it asks lives in `context`.
#[derive(Debug, Clone, Default)]
pub struct MessageContext {
    pub task_id: Option<EntityId>,
    pub anchor_id: Option<EntityId>,
    pub scope_id: Option<EntityId>,
    pub decision_id: Option<EntityId>,
    /// The id of the one principal the message asks (a handle no other principal of the project has
    /// is read too: `routing::plan_for_message`). A message from before
    /// 2026-09-29 may hold `supervisor`, the role it asked, until `fwd admin migrate` makes it a
    /// question asked of the project (`migrate_roles`).
    pub to: Option<String>,
    /// A question asked of no one in particular, whose triage agent answers it (`"asks": "project"`
    /// in `context`). Never with a `to`.
    pub asks_project: bool,
    /// The scope a question asked of no one in particular asks ([`ASKS_SCOPE_KEY`]): as posted, the
    /// scope the asker names, else none, and [`post_message`] finds it from what the question is
    /// about and where it was written.
    pub asks_scope: Option<EntityId>,
    /// The scope its author wrote it in ([`WRITTEN_IN_KEY`]).
    pub written_in: Option<EntityId>,
    /// The step, by its relation's id, it was written with as that step's first comment
    /// ([`FIRST_OF_STEP_KEY`]).
    pub first_of_step: Option<EntityId>,
    pub reply_to: Option<EntityId>,
}

impl MessageContext {
    pub fn from_view(m: &MessageView) -> Result<Self> {
        let context: Value = m.context()?.into();
        let id_at = |key: &str| context.get(key).and_then(Value::as_str).and_then(|s| s.parse::<EntityId>().ok());
        Ok(Self {
            task_id: m.task()?.map(EntityId::from),
            anchor_id: m.anchor()?.map(EntityId::from),
            scope_id: m.scope()?.map(EntityId::from),
            decision_id: m.decision()?.map(EntityId::from),
            to: context.get("to").and_then(Value::as_str).map(str::to_string),
            asks_project: context.get(ASKS_KEY).and_then(Value::as_str) == Some(ASKS_PROJECT),
            asks_scope: id_at(ASKS_SCOPE_KEY),
            written_in: id_at(WRITTEN_IN_KEY),
            first_of_step: id_at(FIRST_OF_STEP_KEY),
            reply_to: m.reply_to()?.map(EntityId::from),
        })
    }
}

/// Posts a message, the one path the CLI, the page and the extension all post through. Its inline
/// references (`@handle`, `[[<id or its start>]]`, `[[D<number>]]`, and a file at a version,
/// `[[docs/cli.md@HEAD]]`; `references.rs`, `file_mention.rs`) are resolved first, and a reference
/// that names nothing, or a start of an id that several ids share, refuses the post with a
/// sentence naming it, so a typo is caught before anyone reads it. The message is stored with
/// every reference as `[[<full id>]]` (a file as its whole-file highlight's, with the version it
/// names: `[[<full id>@<version>]]`), in one transaction with a `mention` relation from the
/// message to each thing it names ([`write_mentions`]).
pub async fn post_message(
    ctx: &Context,
    author: EntityId,
    content: &str,
    priority: Priority,
    mc: MessageContext,
) -> Result<EntityId> {
    let post = Post::prepare(ctx, author, content, priority, mc).await?;
    let trx = ctx.begin();
    let id = post.write(&trx).await?;
    trx.commit().await?;
    Ok(id)
}

/// So that acting on a notification settles it (task TdwRqMeQ, "Mark a notification read when the
/// agent replies to it, forwards it or dismisses it by its id, as the primer says it does"): posts
/// a message as [`post_message`] does, and the transaction that writes the message marks the
/// notification `settles` read, so that the two land together or not at all and a watch armed
/// next does not print the notification again. `fwd reply` and `fwd forward` post through it,
/// passing the notification when they are given its id rather than its message's. Until
/// 2026-09-30 a transaction of its own marked the notification after the post, and a failure
/// between the two left the message posted and the notification unread. A notification read
/// already is left as it is, and so is one dismissed, which marking it read would bring back into
/// the recipient's inbox. A notification addressed to another principal than `author` is refused
/// before anything is posted, naming whose it is and the id of the message behind it, by which
/// `author` answers the message without settling anything ([`own_notification`]; task AfhuiHYS).
pub async fn post_settling(
    ctx: &Context,
    author: EntityId,
    content: &str,
    priority: Priority,
    mc: MessageContext,
    settles: Option<EntityId>,
) -> Result<EntityId> {
    let mut unread = None;
    if let Some(n) = settles {
        let view: NotificationView = ctx.get(n).await.context("the notification being acted on")?;
        let message = mc.reply_to.map_or_else(
            || "the message's id".to_string(),
            |m| format!("its id, {},", m.to_base64().chars().take(8).collect::<String>()),
        );
        let instead = format!(
            "nothing was posted; to answer the message behind it without settling anything, give {message} in place of the notification's"
        );
        own_notification(ctx, &view, author, &instead).await?;
        if view.status()? == NotificationStatus::Unread {
            unread = Some(view);
        }
    }
    let post = Post::prepare(ctx, author, content, priority, mc).await?;
    let trx = ctx.begin();
    let id = post.write(&trx).await?;
    if let Some(view) = unread {
        view.edit(&trx)?.status()?.set(&NotificationStatus::Read)?;
    }
    trx.commit().await?;
    Ok(id)
}

/// A message [`post_message`] has checked and not yet written: its text with every reference
/// resolved, what it names, what it is about and whom it asks. A command that writes something
/// with the message, a new highlight it is on (`post_on_highlight`) or the step that highlight
/// becomes (`add_highlight_step`), prepares it first, so that a text refused for naming nothing
/// leaves nothing behind, and then writes it in the transaction of the rest.
struct Post {
    author: EntityId,
    stored: String,
    named: Vec<EntityId>,
    priority: Priority,
    mc: MessageContext,
    thread: Option<EntityId>,
    context: Value,
}

impl Post {
    /// Resolves the references in `content` and what the message is about, refusing it as
    /// [`post_message`] does, and writes nothing.
    async fn prepare(
        ctx: &Context,
        author: EntityId,
        content: &str,
        priority: Priority,
        mut mc: MessageContext,
    ) -> Result<Post> {
        let found = crate::references::find(content);
        let named = resolve_references(ctx, author, &found).await?;
        let stored = crate::references::rewrite(content, &found, &named);
        let named = ids_of(&named);
        let mut thread = None;
        // A reply inherits its thread root and its subject so participants and topics are one query away.
        if let Some(reply_to) = mc.reply_to {
            let parent: MessageView = ctx.get(reply_to).await.context("the message being replied to")?;
            thread = Some(parent.thread()?.map(EntityId::from).unwrap_or(reply_to));
            let parent_mc = MessageContext::from_view(&parent)?;
            mc.task_id = mc.task_id.or(parent_mc.task_id);
            mc.anchor_id = mc.anchor_id.or(parent_mc.anchor_id);
            mc.scope_id = mc.scope_id.or(parent_mc.scope_id);
            mc.decision_id = mc.decision_id.or(parent_mc.decision_id);
        }
        if mc.to.is_some() && mc.asks_project {
            bail!("a message asks one principal or the project, not both");
        }
        if mc.asks_scope.is_some() && !mc.asks_project {
            bail!("a message asks a scope only as a question asked of no one in particular, with no `to`");
        }
        // The scope a question asked of no one asks, fixed as it is posted: the one its asker
        // named, else of the scopes what it is about concerns the asker's own, else the first,
        // else the one it was written in (`routing::scope_to_ask`).
        if mc.asks_project && mc.asks_scope.is_none() {
            mc.asks_scope = crate::routing::scope_to_ask(ctx, &mc).await?;
        }
        let mut context = json!({});
        if let Some(to) = &mc.to {
            context["to"] = json!(to);
        }
        if mc.asks_project {
            context[ASKS_KEY] = json!(ASKS_PROJECT);
        }
        if let Some(scope) = mc.asks_scope {
            context[ASKS_SCOPE_KEY] = json!(scope.to_base64());
        }
        if let Some(scope) = mc.written_in {
            context[WRITTEN_IN_KEY] = json!(scope.to_base64());
        }
        if let Some(step) = mc.first_of_step {
            context[FIRST_OF_STEP_KEY] = json!(step.to_base64());
        }
        Ok(Post { author, stored, named, priority, mc, thread, context })
    }

    /// Marks the message as the first comment of the step whose relation is `edge`
    /// ([`FIRST_OF_STEP_KEY`]), for a message prepared before the step was made, as
    /// [`add_highlight_step`] prepares its comment so that a refused text writes nothing.
    #[cfg(feature = "native")]
    fn first_of_step(&mut self, edge: EntityId) {
        self.mc.first_of_step = Some(edge);
        self.context[FIRST_OF_STEP_KEY] = json!(edge.to_base64());
    }

    /// Writes the message in `trx`, with a `mention` relation to each thing it names.
    async fn write(self, trx: &ankurah::transaction::Transaction) -> Result<EntityId> {
        let Post { author, stored, named, priority, mc, thread, context } = self;
        let t = now();
        let id = trx
            .create(&Message {
                author: Ref::new(author),
                created: t,
                content: stored,
                priority,
                status: MessageStatus::Active,
                task: mc.task_id.map(Ref::new),
                anchor: mc.anchor_id.map(Ref::new),
                scope: mc.scope_id.map(Ref::new),
                decision: mc.decision_id.map(Ref::new),
                reply_to: mc.reply_to.map(Ref::new),
                thread: thread.map(Ref::new),
                context: Json::new(context),
                resolved_by: None,
                resolved: None,
                edited: None,
                edited_by: None,
            })
            .await?
            .id();
        write_mentions(trx, author, id, &named, WrittenBy::Post, t).await?;
        Ok(id)
    }
}

/// The key in a `mention` relation's `via` that says an edit of the message wrote it, rather than
/// the post: routing tells the person or agent such a relation names that the message names them
/// now (`routing::plan_for_relation`), while those a post names are told with the post.
pub const EDITED_KEY: &str = "edited";

/// Why an edit retracts a `mention` relation, kept on the relation, whose `closed_by` says who
/// made the edit: the author, or an agent that succeeded the author.
const NO_LONGER_NAMED: &str = "the message's text no longer names it: an edit of the text removed the reference";

/// Replaces the text of `message` with `content`, as `by`, so that a message sent with a mistake
/// in it is corrected where it stands instead of answered by a second message, which would split
/// its discussion across two threads. `by` is its author or an agent that succeeded its author
/// (`rules::can_edit_message`), so that an agent relaunched to carry on the author's job corrects
/// what its predecessor wrote (task sVCUXcEg); the agents that succeeded the author are read from
/// the graph's principals, as routing reads them (`routing::Successors::chain`). Anyone else is
/// refused with a sentence naming the author and, when an agent succeeded it, the agent that
/// carries on its job now, whom to ask. Nothing else about the message changes: its id, its thread
/// and its place in it (when it was created) stay, `edited` records when the text changed and
/// `edited_by` who changed it, and the text it had stays in the entity's history. The new text is
/// read as a post's is ([`post_message`]): each inline reference is resolved and stored as
/// `[[<full id>]]`, and one that names nothing refuses the edit with the same sentence, writing
/// nothing. In the same transaction the message's `mention` relations are brought to what the new
/// text names ([`write_mentions`]): a reference to what the text named before keeps its relation,
/// which takes the reference's new place among the references; a reference to anything else gets
/// a relation that `via` marks as written by the edit ([`EDITED_KEY`]), which routing turns into
/// `Mentioned` for a person or an agent the message did not name before; and a relation to what
/// the text no longer names is retracted with the reason, which tells no one. A blank text is
/// refused, and a text that stores as the message's own changes nothing, not even `edited`.
pub async fn edit_message(ctx: &Context, by: EntityId, message: EntityId, content: &str) -> Result<()> {
    let view: MessageView = ctx.get(message).await.context("the message to edit")?;
    let author: EntityId = view.author()?.into();
    let successors = crate::routing::Successors::load(ctx).await?.chain(author);
    match rules::can_edit_message(author, &successors, by) {
        Err(rules::RuleError::NeitherAuthorNorSuccessor { author, successor }) => {
            let author = at_handle(ctx, author).await;
            let (written_by, ask) = match successor {
                Some(successor) => {
                    let successor = at_handle(ctx, successor).await;
                    (format!("{author}'s, whom {successor} succeeded"), successor)
                }
                None => (format!("{author}'s"), author),
            };
            bail!(
                "only its author edits a message, or an agent that succeeded its author, and this one is {written_by}; reply to it instead, or ask {ask} to edit it"
            )
        }
        other => other?,
    }
    if content.trim().is_empty() {
        bail!("a message's text cannot be blank; write what it should say instead");
    }
    let found = crate::references::find(content);
    let named = resolve_references(ctx, by, &found).await?;
    let stored = crate::references::rewrite(content, &found, &named);
    let named = ids_of(&named);
    if stored == text(view.content())? {
        return Ok(());
    }
    let before = relations_from(ctx, message, Some(RelationKind::Mention)).await?;
    let t = now();
    let trx = ctx.begin();
    let m = view.edit(&trx)?;
    m.content()?.replace(&stored)?;
    m.edited()?.set(&Some(t))?;
    m.edited_by()?.set(&Some(Ref::new(by)))?;
    write_mentions(&trx, by, message, &named, WrittenBy::Edit { before }, t).await?;
    trx.commit().await?;
    Ok(())
}

/// What wrote a message's text, which [`write_mentions`] marks on each `mention` relation it adds,
/// since routing tells whom a post names with the post, and whom an edit names anew from the
/// relation the edit wrote ([`EDITED_KEY`]).
enum WrittenBy {
    /// The text a message is created with, by a post or by a decision's or a task's move, which
    /// writes its reason as a comment: the message has no relations yet.
    Post,
    /// An edit of the message's text; `before` is the message's `mention` relations as the edit
    /// found them, retracted ones among them.
    Edit { before: Vec<RelationView> },
}

/// So that routing, which reads a message's `mention` relations and never its text
/// (docs/design/agents-and-inbox.md, rule 13), tells each person or agent the text names, and the
/// page lists the message as "mentioned in" on each thing it names: brings the relations from
/// `message` to what its text names, in the transaction `trx` that writes the text. Every
/// `mention` relation is written here, for a post, an edit, and the comment a decision's move
/// writes. `named` is what the text's references name, in the order the text places them; `by`
/// writes the relations at `t`. A reference keeps a standing relation to what it names, which
/// moves to the reference's place among the references (the first is 1) and keeps the rest of its
/// `via`; a reference that keeps none gets a new relation, whose `via` records that it was written
/// inline, its place and, for an edit, that the edit wrote it; and a relation that no reference
/// keeps is retracted with the reason, which tells no one. A message being created has no
/// relations to keep.
async fn write_mentions(
    trx: &ankurah::transaction::Transaction,
    by: EntityId,
    message: EntityId,
    named: &[EntityId],
    written_by: WrittenBy,
    t: i64,
) -> Result<()> {
    let (before, edited) = match written_by {
        WrittenBy::Post => (Vec::new(), false),
        WrittenBy::Edit { before } => (before, true),
    };
    // Each reference keeps the first standing relation to what it names, in the order the text
    // placed them; the relations no reference keeps are what the text no longer names.
    let mut unkept: Vec<RelationView> =
        before.into_iter().filter(|r| r.status().ok() == Some(RelationStatus::Accepted)).collect();
    unkept.sort_by_key(|r| {
        let ordinal = mention_ordinal(r);
        (ordinal.is_none(), ordinal, r.created().unwrap_or(0))
    });
    for (n, to) in named.iter().enumerate() {
        let ordinal = n as u64 + 1;
        match unkept.iter().position(|r| r.to_id().ok() == Some(*to)) {
            Some(i) => {
                let kept = unkept.remove(i);
                if mention_ordinal(&kept) != Some(ordinal) {
                    let mut via: Value = kept.via()?.into();
                    if !via.is_object() {
                        via = json!({"inline": true});
                    }
                    via["ordinal"] = json!(ordinal);
                    kept.edit(trx)?.via()?.set(&Json::new(via))?;
                }
            }
            None => {
                let via = if edited {
                    json!({"inline": true, "ordinal": ordinal, EDITED_KEY: true})
                } else {
                    json!({"inline": true, "ordinal": ordinal})
                };
                trx.create(&Relation {
                    kind: RelationKind::Mention,
                    from_id: message,
                    to_id: *to,
                    status: RelationStatus::Accepted,
                    created_by: Ref::new(by),
                    created: t,
                    via: Json::new(via),
                    note: None,
                    position: None,
                    closed_by: None,
                    closed: None,
                    closed_reason: None,
                })
                .await?;
            }
        }
    }
    for gone in &unkept {
        let r = gone.edit(trx)?;
        r.status()?.set(&RelationStatus::Retracted)?;
        r.closed_by()?.set(&Some(Ref::new(by)))?;
        r.closed()?.set(&Some(t))?;
        r.closed_reason()?.set(&Some(NO_LONGER_NAMED.to_string()))?;
    }
    Ok(())
}

/// A `mention` relation's place among its message's references (the first is 1), as its `via`
/// records it.
pub fn mention_ordinal(r: &RelationView) -> Option<u64> {
    let via: Value = r.via().ok()?.into();
    via.get("ordinal").and_then(Value::as_u64)
}

/// The ids of what `named` names, in order, as the `mention` relations beside a text point at them
/// ([`write_mentions`]).
fn ids_of(named: &[Named]) -> Vec<EntityId> {
    named.iter().map(|n| n.id).collect()
}

/// What each of `found` names, in order; refused with a sentence naming every reference that names
/// nothing, and why, every handle that two or more principals share, listing them
/// ([`naming::Refused`]), and every file named where no checkout is read
/// (`file_mention::NoCheckout`). A file named in the text is read in the checkout first, and
/// becomes its whole-file highlight, found or made by `by`, only once every reference of the text
/// reads, so that a text refused writes nothing to the graph (`file_mention.rs`).
async fn resolve_references(ctx: &Context, by: EntityId, found: &[crate::references::Found]) -> Result<Vec<Named>> {
    use crate::file_mention::{self, Bracketed};
    let mut named: Vec<Option<Named>> = Vec::with_capacity(found.len());
    let mut files = Vec::new();
    let mut unresolved = Vec::new();
    let mut shared = Vec::new();
    let mut unread = Vec::new();
    for f in found {
        let file = match &f.written {
            crate::references::Written::Bracketed(inner) => file_mention::read(inner),
            crate::references::Written::Handle(_) => None,
        };
        let outcome = match file {
            Some(Bracketed::File { path, revision }) => match file_mention::read_file(path, revision) {
                Ok(read) => {
                    files.push((named.len(), read));
                    Ok(None)
                }
                Err(why) => Err(why),
            },
            Some(Bracketed::Highlight { id, version }) => file_mention::highlight_at(ctx, id, version)
                .await
                .map(|(id, version)| Some(Named { id, version: Some(version) })),
            None => resolve_reference(ctx, &f.written).await.map(|id| Some(Named::entity(id))),
        };
        match outcome {
            Ok(n) => named.push(n),
            Err(why) => {
                named.push(None);
                if why.downcast_ref::<file_mention::NoCheckout>().is_some() {
                    unread.push(format!("`{}`", f.written.as_typed()));
                } else if let Some(refused) = why.downcast_ref::<naming::Refused>() {
                    shared.push(refused.to_string());
                } else {
                    unresolved.push(format!("`{}` ({why})", f.written.as_typed()));
                }
            }
        }
    }
    let nothing = match unresolved.as_slice() {
        [] => None,
        [one] => Some(format!("{one} names nothing; correct it, or put it in backticks to keep it as text")),
        several => Some(format!(
            "{} references name nothing: {}; correct them, or put them in backticks to keep them as text",
            several.len(),
            several.join(", ")
        )),
    };
    let no_checkout = match unread.as_slice() {
        [] => None,
        [one] => Some(format!("{one} names a file, {}", file_mention::NO_CHECKOUT)),
        several => Some(format!("{} name files, {}", several.join(", "), file_mention::NO_CHECKOUT)),
    };
    let refusals: Vec<String> = shared.into_iter().chain(nothing).chain(no_checkout).collect();
    if !refusals.is_empty() {
        bail!("refused: {}", refusals.join("; "));
    }
    for (at, read) in files {
        let id = file_mention::highlight(ctx, by, &read).await?;
        named[at] = Some(Named { id, version: Some(read.version) });
    }
    named.into_iter().collect::<Option<Vec<Named>>>().ok_or_else(|| anyhow!("a reference of the text was left unread"))
}

/// The entity one inline reference names: a principal by handle, or anything by its full id (which
/// must name something), the start of its id, or `D<number>`, under the command line's prefix
/// rules. The error says why nothing was found, in words for the refusal to quote; a handle that
/// two or more principals of the project share is refused, listing them ([`naming::Refused`]).
async fn resolve_reference(ctx: &Context, written: &crate::references::Written) -> Result<EntityId> {
    use crate::references::Written;
    match written {
        // An at-sign names a person by handle, and failing that anything by its id or the start of
        // one, the way the brackets do: the at-sign is what a writer reaches for first (Daniel,
        // 2026-09-28, typing `@<task id>` on the page and being refused).
        Written::Handle(handle) => match naming::by_name(&principals(ctx).await?, handle, None).as_slice() {
            [one] => Ok(one.id()),
            [] => match handle.parse::<EntityId>() {
                Ok(id) if kind_of(ctx, id).await.is_some() => Ok(id),
                _ => match resolve_id(ctx, handle, MENTIONABLE).await {
                    Ok(id) => Ok(id),
                    Err(_) => bail!("no one has the handle `{handle}`, and no id starts with it"),
                },
            },
            several => Err(naming::refused(ctx, handle, several, false).await?.into()),
        },
        Written::Bracketed(inner) if inner.is_empty() => bail!("the brackets are empty"),
        Written::Bracketed(inner) => match inner.parse::<EntityId>() {
            Ok(id) => match kind_of(ctx, id).await {
                Some(_) => Ok(id),
                None => bail!("nothing in this project has that id"),
            },
            Err(_) => resolve_id(ctx, inner, MENTIONABLE).await,
        },
    }
}

/// The kinds an inline reference may name: every kind a command line names, and people.
pub const MENTIONABLE: &[Kind] =
    &[Kind::Task, Kind::Message, Kind::Decision, Kind::Scope, Kind::Anchor, Kind::Relation, Kind::Principal];

/// Which of the [`MENTIONABLE`] kinds `id` is, if any.
pub async fn kind_of(ctx: &Context, id: EntityId) -> Option<Kind> {
    for kind in MENTIONABLE {
        let found = match kind {
            Kind::Task => ctx.get::<TaskView>(id).await.is_ok(),
            Kind::Message => ctx.get::<MessageView>(id).await.is_ok(),
            Kind::Decision => ctx.get::<DecisionView>(id).await.is_ok(),
            Kind::Scope => ctx.get::<ScopeView>(id).await.is_ok(),
            Kind::Anchor => ctx.get::<CodeAnchorView>(id).await.is_ok(),
            Kind::Relation => ctx.get::<RelationView>(id).await.is_ok(),
            Kind::Principal => ctx.get::<PrincipalView>(id).await.is_ok(),
            Kind::Notification => false,
        };
        if found {
            return Some(*kind);
        }
    }
    None
}

/// What something is and a name to show for it, as a reference or a step reads: a person as
/// `@handle`, a task and a scope by title, a decision as `D12 (title)`, code as `path:lines` and a
/// whole file as its path, a message by its first words, a relation by its kind. None when `id`
/// names nothing here.
pub async fn describe(ctx: &Context, id: EntityId) -> Option<(Kind, String)> {
    let kind = kind_of(ctx, id).await?;
    let name = match kind {
        Kind::Task => ctx.get::<TaskView>(id).await.ok()?.title().ok()?,
        Kind::Decision => {
            let d = ctx.get::<DecisionView>(id).await.ok()?;
            format!("D{} ({})", d.number().ok()?, d.title().ok()?)
        }
        Kind::Scope => ctx.get::<ScopeView>(id).await.ok()?.title().ok()?,
        Kind::Anchor => {
            let a = ctx.get::<CodeAnchorView>(id).await.ok()?;
            crate::anchor_name::place(&a.path().ok()?, crate::anchor_name::lines(lines_of(&a).ok()?))
        }
        Kind::Message => {
            let m = ctx.get::<MessageView>(id).await.ok()?;
            let words = crate::reason::first_words(&text(m.content()).unwrap_or_default(), crate::reason::NAME_WORDS);
            format!("“{words}”")
        }
        Kind::Relation => format!("a {} relation", ctx.get::<RelationView>(id).await.ok()?.kind().ok()?),
        Kind::Principal => format!("@{}", ctx.get::<PrincipalView>(id).await.ok()?.handle().ok()?),
        Kind::Notification => return None,
    };
    Some((kind, name))
}

/// The references of a stored message text, in order, as the JSON of a message lists them: each
/// thing's id, kind and name ([`reference_json`]), and for a file mention the version it names
/// ([`named_json`]).
pub async fn references_json(ctx: &Context, content: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for (_, named) in crate::references::stored_named(content) {
        out.push(named_json(ctx, &named).await);
    }
    out
}

/// What [`describe`] says of the thing a reference names, and of a file mention the file at the
/// version it names (`docs/cli.md e13b8e2`, `anchor_name::AnchorName::file_at`), since the
/// whole-file highlight it names serves every version that holds the same bytes.
pub async fn describe_named(ctx: &Context, named: &Named) -> Option<(Kind, String)> {
    let (kind, name) = describe(ctx, named.id).await?;
    match &named.version {
        Some(version) if kind == Kind::Anchor => {
            let a = ctx.get::<CodeAnchorView>(named.id).await.ok()?;
            Some((kind, crate::anchor_name::AnchorName::file_at(&a.path().ok()?, version).text()))
        }
        _ => Some((kind, name)),
    }
}

/// One reference of a stored text for a reader who holds only the text: as [`reference_json`]
/// gives the thing it names, named as [`describe_named`] names it, with the version a file mention
/// names as the text stores it (`version`: `worktree`, `index` or a commit's id).
pub async fn named_json(ctx: &Context, named: &Named) -> Value {
    let (kind, name) = match describe_named(ctx, named).await {
        Some((kind, name)) => (kind.name(), name),
        None => ("unknown", named.id.to_base64()),
    };
    let mut v = json!({"id": named.id.to_base64(), "kind": kind, "name": name});
    if let Some(version) = &named.version {
        v["version"] = json!(version.as_str());
    }
    v
}

/// One thing named for a reader who holds only its id, as a message's references and a batch's
/// steps list it: its id, kind and name ([`describe`]); something that cannot be read (gone, or
/// not synced here) has kind `unknown` and its id as its name.
pub async fn reference_json(ctx: &Context, id: EntityId) -> Value {
    let (kind, name) = match describe(ctx, id).await {
        Some((kind, name)) => (kind.name(), name),
        None => ("unknown", id.to_base64()),
    };
    json!({"id": id.to_base64(), "kind": kind, "name": name})
}

/// A stored message text for reading in a terminal: a person as `@handle`, anything else as its
/// name in brackets, a file mention with the version it names (`references.rs`,
/// [`describe_named`]).
pub async fn render_references(ctx: &Context, content: &str) -> String {
    let mut names = Vec::new();
    for (_, named) in crate::references::stored_named(content) {
        if let Some((kind, name)) = describe_named(ctx, &named).await {
            names.push((named, if kind == Kind::Principal { name } else { format!("[{name}]") }));
        }
    }
    crate::references::render_named(content, |named| {
        names.iter().find(|(n, _)| n == named).map(|(_, name)| name.clone())
    })
}

/// Resolves `message`, with `by` and the time as who resolved it and when. The first message of the
/// first thread on a highlight that is a step checks the step with it, on every task's checklist
/// the highlight is on, in the same transaction, as `rules::move_step_and_thread` says
/// (docs/DECISIONS.md, D99, "Checking a step resolves its highlight's first thread, and resolving
/// that thread checks the step; each control says it does both"); any other message moves alone.
/// A resolved message stays as it was resolved. Returns what moved; it tells no one.
pub async fn resolve_message(ctx: &Context, by: EntityId, message: EntityId) -> Result<rules::Changes> {
    move_message(ctx, by, message, true).await
}

/// Sets a resolved `message` back to active, forgetting who resolved it and when: the opposite of
/// [`resolve_message`], which unchecks with it the steps that resolving it checks (D99, above).
pub async fn unresolve_message(ctx: &Context, by: EntityId, message: EntityId) -> Result<rules::Changes> {
    move_message(ctx, by, message, false).await
}

/// Resolves `message` (`resolve`) or sets it back to active, with the steps of its highlight when
/// it is the first message of the highlight's first thread.
async fn move_message(ctx: &Context, by: EntityId, message: EntityId, resolve: bool) -> Result<rules::Changes> {
    let view: MessageView = ctx.get(message).await.context("that message")?;
    let (mut messages, relations) = match view.anchor()? {
        Some(anchor) => highlight_parts(ctx, anchor.into()).await?,
        None => (Vec::new(), Vec::new()),
    };
    if !messages.iter().any(|m| m.id() == message) {
        messages.push(view.clone());
    }
    let on_highlight = messages.iter().map(thread_message).collect::<Result<Vec<_>>>()?;
    let steps = relations.iter().map(step_edge).collect::<Result<Vec<_>>>()?;
    let moved = thread_message(&view)?;
    let changes = rules::move_step_and_thread(rules::Moved::Message(&moved), resolve, &on_highlight, &steps)?;
    write_changes(ctx, by, &changes, &messages, &relations).await?;
    Ok(changes)
}

/// Messages whose `field` (`task`, `anchor`, `scope`, `decision`, `thread` or `reply_to`) points
/// at `id`, oldest first.
pub async fn messages_about(ctx: &Context, field: &str, id: EntityId) -> Result<Vec<MessageView>> {
    if !matches!(field, "task" | "anchor" | "scope" | "decision" | "thread" | "reply_to") {
        bail!("messages are looked up by task, anchor, scope, decision, thread or reply_to, not `{field}`");
    }
    let mut found: Vec<MessageView> = ctx.fetch(with_id(&format!("{field} = ?"), id)?).await?;
    found.sort_by_key(|m| m.created().unwrap_or(0));
    Ok(found)
}

pub async fn create_notification(
    ctx: &Context,
    recipient: EntityId,
    notification_type: NotificationType,
    priority: Priority,
    target: Value,
    originator: Value,
) -> Result<EntityId> {
    let trx = ctx.begin();
    let id = trx
        .create(&Notification {
            recipient: Ref::new(recipient),
            notification_type,
            status: NotificationStatus::Unread,
            priority,
            created: now(),
            updated: None,
            target: Json::new(target),
            originator: Json::new(originator),
        })
        .await?
        .id();
    trx.commit().await?;
    Ok(id)
}

/// Rewrites what a notification says as an item joins its batch (docs/design/agents-and-inbox.md,
/// rule 10): its type, priority, target and originator, and `updated`, set to now, so the batch
/// lists as new. Its recipient, its status and when it was made stay.
pub async fn update_notification(
    ctx: &Context,
    n: &NotificationView,
    notification_type: NotificationType,
    priority: Priority,
    target: Value,
    originator: Value,
) -> Result<()> {
    let trx = ctx.begin();
    let m = n.edit(&trx)?;
    m.notification_type()?.set(&notification_type)?;
    m.priority()?.set(&priority)?;
    m.target()?.set(&Json::new(target))?;
    m.originator()?.set(&Json::new(originator))?;
    m.updated()?.set(&Some(now()))?;
    trx.commit().await?;
    Ok(())
}

/// When a notification last changed what it says: when the last item joined its batch, else when
/// it was made. Inboxes order by it, so a batch that grew lists as new.
pub fn notification_time(n: &NotificationView) -> i64 {
    n.updated().ok().flatten().unwrap_or_else(|| n.created().unwrap_or(0))
}

/// The simplikeys query: everything for a recipient that is not dismissed, unread first, then
/// by priority, then oldest first (by [`notification_time`]).
pub async fn inbox(ctx: &Context, recipient: EntityId, unread_only: bool) -> Result<Vec<NotificationView>> {
    let pred =
        if unread_only { "recipient = ? AND status = 'unread'" } else { "recipient = ? AND status != 'dismissed'" };
    let mut found: Vec<NotificationView> = ctx.fetch(with_ids(pred, &[recipient])?).await?;
    found.sort_by_key(|n| {
        let status_rank = match n.status().unwrap_or(NotificationStatus::Read) {
            NotificationStatus::Unread => 0,
            _ => 1,
        };
        let prio_rank = match n.priority().unwrap_or(Priority::Later) {
            Priority::Now => 0,
            Priority::Next => 1,
            Priority::Later => 2,
        };
        (status_rank, prio_rank, notification_time(n))
    });
    Ok(found)
}

/// Gives the notifications `ids` the status `status` in one transaction, fetching each first:
/// [`set_status_of`] for a caller that holds only their ids, as the VS Code extension does.
pub async fn set_notification_status(ctx: &Context, ids: &[EntityId], status: NotificationStatus) -> Result<()> {
    let mut held = Vec::with_capacity(ids.len());
    for id in ids {
        held.push(ctx.get(*id).await?);
    }
    set_status_of(ctx, &held, status).await
}

/// Gives the notifications `held`, which the caller has read already, the status `status` in one
/// transaction, fetching none of them again: a client that is not the daemon fetches each from the
/// daemon, one round trip at a time, so the page's Mark all read, which fetched them so, left 212
/// notifications unread for longer than anyone waited (task taRWSL2-). The page marks the
/// notifications its inbox query holds, and `fwd inbox pop` those it has just listed. Like
/// [`set_notification_status`], it asks no one whose they are ([`mark_notifications`]).
pub async fn set_status_of(ctx: &Context, held: &[NotificationView], status: NotificationStatus) -> Result<()> {
    let trx = ctx.begin();
    for view in held {
        view.edit(&trx)?.status()?.set(&status)?;
    }
    trx.commit().await?;
    Ok(())
}

/// `fwd read` and `fwd dismiss`: gives the notifications `ids` the status `status` for `by`, the
/// principal the command acts as, in one transaction, after refusing them all when one of them is
/// addressed to another principal, naming whose it is ([`own_notification`]; task AfhuiHYS), so that
/// no principal takes a notification out of another's unread inbox. [`set_notification_status`]
/// and [`set_status_of`] ask no one whose they are, so their callers mark only their own
/// principal's: the page, which marks only the cards of the principal it acts as; the VS Code
/// extension through fwd-host, whose inbox view lists only the notifications of the principal its
/// window acts as; and `fwd inbox pop`, which refuses another principal's inbox before it prints or
/// marks anything.
pub async fn mark_notifications(
    ctx: &Context,
    by: EntityId,
    ids: &[EntityId],
    status: NotificationStatus,
) -> Result<()> {
    let mut views = Vec::with_capacity(ids.len());
    for id in ids {
        let view: NotificationView = ctx.get(*id).await?;
        own_notification(ctx, &view, by, "nothing was marked").await?;
        views.push(view);
    }
    set_status_of(ctx, &views, status).await
}

/// Refuses `by` the notification `view` when it is addressed to another principal
/// ([`rules::can_settle`]), in a sentence naming whose it is and whom the command acts as, then
/// `instead`, what happened or what to do instead.
async fn own_notification(ctx: &Context, view: &NotificationView, by: EntityId, instead: &str) -> Result<()> {
    let recipient: EntityId = view.recipient()?.into();
    match rules::can_settle(view.id(), recipient, by) {
        Err(rules::RuleError::NotTheRecipient { notification, recipient }) => bail!(
            "notification {} is {}'s, not {}'s, and a principal reads, dismisses, replies through or forwards only its own notifications, as the page marks no card of another's read: {instead}",
            notification.to_base64().chars().take(8).collect::<String>(),
            at_handle(ctx, recipient).await,
            at_handle(ctx, by).await,
        ),
        other => Ok(other?),
    }
}

// ---------------------------------------------------------------------------------------------
// Decisions

#[derive(Debug, Clone, Default)]
pub struct CreateDecision {
    pub title: String,
    /// What kind of ruling: architecture, product, process, ux, naming, dependency, or any word.
    pub area: Option<String>,
    pub context: String,
    pub decision: String,
    pub consequences: String,
    /// Filed for a ruling rather than recorded as one given.
    pub propose: bool,
    /// Who ruled, for a ruling recorded as given; the filer when unset.
    pub decided_by: Option<EntityId>,
    /// When it was ruled on, for a ruling recorded after the fact; now when unset.
    pub decided: Option<i64>,
    /// The tasks, messages or decisions it was decided in the context of: one `cites` relation
    /// from the new decision to each, however often it is named.
    pub from: Vec<EntityId>,
    /// The scope whose work it governs; none for a ruling that governs the whole project.
    pub scope: Option<EntityId>,
}

/// Every decision of the project, by number (then by creation, should two share a number).
pub async fn decisions(ctx: &Context) -> Result<Vec<DecisionView>> {
    let mut found: Vec<DecisionView> = ctx.fetch("created > 0").await?;
    found.sort_by_key(|d| (d.number().unwrap_or(0), d.created().unwrap_or(0)));
    Ok(found)
}

/// Files a decision under the next number: accepted and ruled by `decided_by` (else the filer),
/// or proposed for a ruling with `propose`; attached to `scope` when one is given, else governing
/// the whole project. A scope that does not exist is refused before anything is written.
pub async fn create_decision(ctx: &Context, by: EntityId, spec: CreateDecision) -> Result<EntityId> {
    if spec.title.trim().is_empty() {
        bail!("a decision needs a title");
    }
    if spec.propose && (spec.decided_by.is_some() || spec.decided.is_some()) {
        bail!("a proposed decision has not been ruled on yet: --decided-by and --decided record a ruling already given, so they do not go with --propose");
    }
    if let Some(scope) = spec.scope {
        scope_to_attach_to(ctx, scope).await?;
    }
    let mut decided_in: Vec<EntityId> = Vec::new();
    for from in spec.from {
        if !decided_in.contains(&from) {
            decided_in.push(from);
        }
    }
    for from in &decided_in {
        if citable(ctx, *from).await?.is_none() {
            bail!("{from} is not a task, a message or a decision, so a decision cannot have been made in its context");
        }
    }
    let number = rules::next_decision_number(decisions(ctx).await?.iter().filter_map(|d| d.number().ok()));
    let t = now();
    let (status, decided_by, decided) = if spec.propose {
        (DecisionStatus::Proposed, None, None)
    } else {
        (DecisionStatus::Accepted, Some(spec.decided_by.unwrap_or(by)), Some(spec.decided.unwrap_or(t)))
    };
    let trx = ctx.begin();
    let id = trx
        .create(&Decision {
            number,
            title: spec.title.trim().to_string(),
            status,
            area: spec.area.map(|a| a.trim().to_string()).filter(|a| !a.is_empty()),
            scope: spec.scope.map(Ref::new),
            context: spec.context,
            decision: spec.decision,
            consequences: spec.consequences,
            author: Ref::new(by),
            decided_by: decided_by.map(Ref::new),
            created: t,
            decided,
            closed_by: None,
            closed: None,
            closed_reason: None,
            closed_reason_message: None,
        })
        .await?
        .id();
    for from in decided_in {
        trx.create(&Relation {
            kind: RelationKind::Cites,
            from_id: id,
            to_id: from,
            status: RelationStatus::Accepted,
            created_by: Ref::new(by),
            created: t,
            via: Json::new(json!({"actor_id": by.to_base64()})),
            note: None,
            position: None,
            closed_by: None,
            closed: None,
            closed_reason: None,
        })
        .await?;
    }
    trx.commit().await?;
    Ok(id)
}

/// Attaches `decision` to `scope`, the scope whose work the ruling governs, in place of any scope
/// it was attached to (docs/DECISIONS.md, D93, "A decision may be attached to the scope it
/// governs; one attached to none governs the whole project, and its area is its type",
/// 2026-09-28), so that it is listed among the rulings of that scope's work and no other's. The
/// ruling itself is unchanged, so attaching tells nobody, and it asks no permission of its own:
/// whoever may comment on a decision may attach it. `_by` is who attaches it, which nothing
/// records: the scope a ruling governs keeps no history. A scope that does not exist is refused
/// with a sentence saying so; attaching a decision to the scope it governs already changes
/// nothing.
pub async fn attach_decision(ctx: &Context, _by: EntityId, decision: EntityId, scope: EntityId) -> Result<()> {
    scope_to_attach_to(ctx, scope).await?;
    set_decision_scope(ctx, decision, Some(scope)).await
}

/// Detaches `decision` from the scope it was attached to, so that it governs the whole project
/// again; telling nobody and asking no permission of its own, as [`attach_decision`]. A decision
/// attached to no scope stays as it is.
pub async fn detach_decision(ctx: &Context, _by: EntityId, decision: EntityId) -> Result<()> {
    set_decision_scope(ctx, decision, None).await
}

/// Refuses, with a sentence, a scope that a decision cannot be attached to because it does not
/// exist.
async fn scope_to_attach_to(ctx: &Context, scope: EntityId) -> Result<()> {
    if ctx.get::<ScopeView>(scope).await.is_err() {
        bail!("no scope of this project has the id {scope}, so no ruling can be attached to it");
    }
    Ok(())
}

/// Sets the scope a decision governs, none for the whole project, unless it has that one already.
async fn set_decision_scope(ctx: &Context, decision: EntityId, scope: Option<EntityId>) -> Result<()> {
    let view: DecisionView = ctx.get(decision).await.context("that decision")?;
    if governs(&view)? == scope {
        return Ok(());
    }
    let trx = ctx.begin();
    view.edit(&trx)?.scope()?.set(&scope.map(Ref::new))?;
    trx.commit().await?;
    Ok(())
}

/// The scope a decision governs; none for one that governs the whole project, as a decision
/// recorded before decisions had scopes does.
pub fn governs(decision: &DecisionView) -> Result<Option<EntityId>> {
    Ok(decision.scope()?.map(EntityId::from))
}

/// The key in a message's `context` naming the move whose reason the message gives: on a decision
/// (`accepted`, `rejected`, `superseded` or `repealed`), so routing tells a move's reason from any
/// other comment on the decision; on a task (`returned`, `rejected`, `cancelled`, `reopened` or
/// `rejustified`: [`transition`]), so a reader tells why it moved from a comment about the work.
pub const MOVE_KEY: &str = "move";

/// A move's reason as the comment that records it stores it: the note, trimmed, with each inline
/// reference as `[[<full id>]]`, and what the references name, in the order the note names them.
struct Reason {
    text: String,
    named: Vec<EntityId>,
}

/// So that the comment a move writes names what its note names, as any comment does (Daniel,
/// 2026-09-28, of a supersession's note that named his message with an at-sign: "your `@` didn't
/// work"): reads the note as [`post_message`] reads a message's text, before the move is made, a
/// file it names becoming a highlight made by `by`. A reference that names nothing refuses the
/// move with the sentence a post gets.
async fn read_reason(ctx: &Context, by: EntityId, note: &str) -> Result<Reason> {
    let note = note.trim();
    let found = crate::references::find(note);
    let named = resolve_references(ctx, by, &found).await?;
    Ok(Reason { text: crate::references::rewrite(note, &found, &named), named: ids_of(&named) })
}

/// Records why a decision moved, in the move's own transaction `trx`: the comment on `decision` by
/// `author` naming the move `mv`, its text `content` (which holds the reason's), and, as a post
/// writes a message's ([`write_mentions`]), a `mention` relation from the comment to each thing
/// the reason names (`named`), so that the reason's references draw as lozenges and a person it
/// names hears of it.
async fn decision_comment(
    trx: &ankurah::transaction::Transaction,
    author: EntityId,
    decision: EntityId,
    mv: DecisionMove,
    content: String,
    named: &[EntityId],
) -> Result<EntityId> {
    let t = now();
    let id = trx
        .create(&Message {
            author: Ref::new(author),
            created: t,
            content,
            priority: Priority::Next,
            status: MessageStatus::Active,
            task: None,
            anchor: None,
            scope: None,
            decision: Some(Ref::new(decision)),
            reply_to: None,
            thread: None,
            context: Json::new(json!({ MOVE_KEY: mv.participle() })),
            resolved_by: None,
            resolved: None,
            edited: None,
            edited_by: None,
        })
        .await?
        .id();
    write_mentions(trx, author, id, named, WrittenBy::Post, t).await?;
    Ok(id)
}

fn wants_note(mv: DecisionMove, note: &str) -> Result<()> {
    if note.trim().is_empty() {
        bail!("a decision is {} with a note saying why, which is kept as a comment on it", mv.participle());
    }
    Ok(())
}

/// The sentence a move leaves on the decision: the words it opens with ([`move_opening`]), then the
/// reason given: "Rejected: <note>", "Superseded by D2: <note>".
fn move_comment(mv: DecisionMove, successor: Option<i64>, note: &str) -> String {
    format!("{}: {}", move_opening(mv, successor), note.trim())
}

/// The words a move's comment opens with, before the colon and the note: the move, capitalized,
/// and for a supersession the number of the ruling that replaces it (`successor`): "Rejected",
/// "Superseded by D2".
fn move_opening(mv: DecisionMove, successor: Option<i64>) -> String {
    let mut done = mv.participle();
    if let Some(first) = done.get(..1) {
        done = first.to_uppercase() + &done[1..];
    }
    match successor {
        Some(number) => format!("{done} by D{number}"),
        None => done,
    }
}

/// The note in `comment`, the comment the move `mv` wrote: its text after the words the move
/// opened it with ([`move_opening`]) and their colon, or all of it where it does not open with
/// them, as when its author edited them away.
fn move_note(mv: DecisionMove, comment: &str) -> &str {
    let after_opening = comment.strip_prefix(move_opening(mv, None).as_str()).and_then(|rest| match mv {
        // A supersession's opening names the ruling that replaces it, whose number the reader of
        // the comment is not given: any number stands for it.
        DecisionMove::Supersede => {
            let number = rest.strip_prefix(" by D")?;
            let after = number.trim_start_matches(|c: char| c.is_ascii_digit());
            (after.len() < number.len()).then_some(after)
        }
        _ => Some(rest),
    });
    after_opening.and_then(|rest| rest.strip_prefix(": ")).unwrap_or(comment).trim()
}

/// A comment on a decision as [`closing_note`] reads it, which each reader builds from what it
/// holds: the page from its standing query of messages, the exported log from the decision's
/// comments.
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionComment {
    /// When it was written, which picks between two moves that raced.
    pub created: i64,
    /// Its `context`, where the comment a move writes names the move ([`MOVE_KEY`]).
    pub context: Value,
    /// Its text as stored, each reference in it as `[[<full id>]]`.
    pub text: String,
}

impl DecisionComment {
    /// The message `m`, a comment on a decision, as [`closing_note`] reads it.
    pub fn of(m: &MessageView) -> Result<Self> {
        Ok(Self { created: m.created()?, context: m.context()?.into(), text: text(m.content())? })
    }
}

/// So that a decision's record gives why it closed with the names its note named, drawn as
/// lozenges on the page and printed by name in the exported log, where `closed_reason` keeps the
/// note as it was typed (task b6bAAF57, "Show a decision's closing note as its comment, with names
/// drawn, not the raw text"): the note the move that closed a decision in `status` gave in the
/// comment it wrote ([`DecisionMove::closing`]: rejecting, superseding or repealing it), found
/// among `comments`, the decision's comments, by the move its `context.move` names ([`MOVE_KEY`]).
/// The comment keeps each reference as the move resolved it, `[[<full id>]]`, and the note is its
/// text after the words the move opened it with ([`move_note`]). When two moves of the decision
/// raced, both comments stand (docs/design/data-model.md, Decision), and the later one's note is
/// given. None for a decision still open, and for one closed before its moves wrote comments,
/// whose note is its `closed_reason` alone.
pub fn closing_note(status: DecisionStatus, comments: &[DecisionComment]) -> Option<&str> {
    let mv = DecisionMove::closing(status)?;
    let named = mv.participle();
    comments
        .iter()
        .filter(|c| c.context.get(MOVE_KEY).and_then(Value::as_str) == Some(named.as_str()))
        .max_by_key(|c| c.created)
        .map(|c| move_note(mv, &c.text))
}

/// Accepts, rejects or repeals a decision, in one transaction with the comment that records why,
/// whose note is read as a message's text is ([`read_reason`]). Accepting or rejecting makes `by`
/// its decider; rejecting or repealing closes it, its `closed_reason` the note as it was typed,
/// and a repeal names its comment as `closed_reason_message`, the reason its notifications carry.
pub async fn move_decision(
    ctx: &Context,
    by: EntityId,
    decision: EntityId,
    mv: DecisionMove,
    note: &str,
) -> Result<DecisionStatus> {
    move_decision_for(ctx, by, by, decision, mv, note).await
}

/// [`move_decision`] for a ruling on a proposal that someone else gave, so the record names who
/// ruled rather than who wrote it down: `by` makes the move and writes the comment that records
/// why, and `decided_by` (a person, as [`person_by_handle`] finds one) becomes the decider of the
/// proposal it accepts or rejects. A rejection names `decided_by` as the one who closed the
/// proposal too, not `by`, so that everything saying who rejected it names the same person
/// (`fwd decision show` prints both fields; routing reads `closed_by` only for a repeal or a
/// supersession). A repeal rules on no proposal, so it names no decider but `by`.
pub async fn move_decision_for(
    ctx: &Context,
    by: EntityId,
    decided_by: EntityId,
    decision: EntityId,
    mv: DecisionMove,
    note: &str,
) -> Result<DecisionStatus> {
    if mv == DecisionMove::Supersede {
        bail!("a supersession names the decision that replaces it: use supersede_decision");
    }
    let rules_on_a_proposal = matches!(mv, DecisionMove::Accept | DecisionMove::Reject);
    if decided_by != by && !rules_on_a_proposal {
        bail!("only accepting or rejecting a proposal records someone else's ruling, not a repeal");
    }
    let view: DecisionView = ctx.get(decision).await.context("that decision")?;
    let next = rules::decision_move(view.status()?, mv)?;
    wants_note(mv, note)?;
    let reason = read_reason(ctx, by, note).await?;
    let t = now();
    let trx = ctx.begin();
    let m = view.edit(&trx)?;
    m.status()?.set(&next)?;
    if rules_on_a_proposal {
        m.decided_by()?.set(&Some(Ref::new(decided_by)))?;
        m.decided()?.set(&Some(t))?;
    }
    if matches!(mv, DecisionMove::Reject | DecisionMove::Repeal) {
        m.closed_by()?.set(&Some(Ref::new(decided_by)))?;
        m.closed()?.set(&Some(t))?;
        m.closed_reason()?.set(&Some(note.trim().to_string()))?;
    }
    let comment = decision_comment(&trx, by, decision, mv, move_comment(mv, None, &reason.text), &reason.named).await?;
    if mv == DecisionMove::Repeal {
        m.closed_reason_message()?.set(&Some(Ref::new(comment)))?;
    }
    trx.commit().await?;
    Ok(next)
}

/// Replaces the ruling `old` with the ruling `new`, in one transaction: `old` becomes Superseded,
/// `new` gets a `supersedes` relation to it, and a comment on `old` records why, named as its
/// `closed_reason_message`, with the note read as a message's text is ([`read_reason`]).
pub async fn supersede_decision(ctx: &Context, by: EntityId, old: EntityId, new: EntityId, note: &str) -> Result<()> {
    let old_view: DecisionView = ctx.get(old).await.context("the decision being superseded")?;
    let new_view: DecisionView = ctx.get(new).await.context("the decision that supersedes it")?;
    rules::can_supersede(old, new, new_view.status()?)?;
    let next = rules::decision_move(old_view.status()?, DecisionMove::Supersede)?;
    wants_note(DecisionMove::Supersede, note)?;
    let reason = read_reason(ctx, by, note).await?;
    let t = now();
    let trx = ctx.begin();
    let m = old_view.edit(&trx)?;
    m.status()?.set(&next)?;
    m.closed_by()?.set(&Some(Ref::new(by)))?;
    m.closed()?.set(&Some(t))?;
    m.closed_reason()?.set(&Some(note.trim().to_string()))?;
    trx.create(&Relation {
        kind: RelationKind::Supersedes,
        from_id: new,
        to_id: old,
        status: RelationStatus::Accepted,
        created_by: Ref::new(by),
        created: t,
        via: Json::new(json!({"actor_id": by.to_base64()})),
        note: None,
        position: None,
        closed_by: None,
        closed: None,
        closed_reason: None,
    })
    .await?;
    let comment = move_comment(DecisionMove::Supersede, Some(new_view.number()?), &reason.text);
    let comment = decision_comment(&trx, by, old, DecisionMove::Supersede, comment, &reason.named).await?;
    m.closed_reason_message()?.set(&Some(Ref::new(comment)))?;
    trx.commit().await?;
    Ok(())
}

/// What `fwd decision create --from` accepts: the things a `cites` relation joins to a decision
/// ([`joins`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Citable {
    Task,
    Message,
    Decision,
}

/// Whether `id` is a task, a message or a decision, the three things a `cites` relation joins.
pub async fn citable(ctx: &Context, id: EntityId) -> Result<Option<Citable>> {
    Ok(if ctx.get::<DecisionView>(id).await.is_ok() {
        Some(Citable::Decision)
    } else if ctx.get::<TaskView>(id).await.is_ok() {
        Some(Citable::Task)
    } else if ctx.get::<MessageView>(id).await.is_ok() {
        Some(Citable::Message)
    } else {
        None
    })
}

/// Records that `from` (a task, a message or a decision) rests on the ruling `decision`. Citing
/// again answers the relation already there.
pub async fn cite(
    ctx: &Context,
    by: EntityId,
    from: EntityId,
    decision: EntityId,
    note: Option<&str>,
) -> Result<EntityId> {
    if ctx.get::<DecisionView>(decision).await.is_err() {
        bail!("{decision} is not a decision; `fwd cite` names what cites first and the decision second");
    }
    link(ctx, by, RelationKind::Cites, from, decision, note, false, json!({"actor_id": by.to_base64()})).await
}

/// Whether a relation still stands: neither retracted nor rejected.
pub fn standing(r: &RelationView) -> bool {
    matches!(r.status(), Ok(RelationStatus::Accepted | RelationStatus::Proposed))
}

/// Who made a task, a message or a decision: the one to tell when what it rests on changes.
pub async fn author_of(ctx: &Context, id: EntityId) -> Result<Option<EntityId>> {
    if let Ok(t) = ctx.get::<TaskView>(id).await {
        return Ok(Some(t.created_by()?.into()));
    }
    if let Ok(m) = ctx.get::<MessageView>(id).await {
        return Ok(Some(m.author()?.into()));
    }
    if let Ok(d) = ctx.get::<DecisionView>(id).await {
        return Ok(Some(d.author()?.into()));
    }
    Ok(None)
}

// ---------------------------------------------------------------------------------------------
// JSON projections for the CLI

pub fn json_of_task(t: &TaskView, readiness: Option<&Readiness>) -> Result<Value> {
    Ok(json!({
        "id": t.id().to_base64(),
        "title": t.title()?,
        "status": t.status()?.to_string(),
        "what": text(t.what())?,
        "why": text(t.why())?,
        "done_when": text(t.done_when())?,
        "needs_review": t.needs_review()?,
        "assignee": assignee_of(t)?.map(|a| a.to_base64()),
        "hold": t.hold()?,
        "priority": t.priority()?,
        "labels": Value::from(t.labels()?),
        "context": Value::from(t.context()?),
        "created_by": EntityId::from(t.created_by()?).to_base64(),
        "created": t.created()?,
        "updated": t.updated()?,
        "closed": t.closed()?,
        "closed_reason": t.closed_reason()?,
        "readiness": readiness.map(json_of_readiness),
    }))
}

pub fn json_of_readiness(r: &Readiness) -> Value {
    match r {
        Readiness::Ready => json!({"state": "ready"}),
        Readiness::NotReady(reasons) => {
            json!({"state": "not_ready", "reasons": reasons.iter().map(|x| format!("{x:?}")).collect::<Vec<_>>()})
        }
        Readiness::Unknown(reasons) => {
            json!({"state": "unknown", "reasons": reasons.iter().map(|x| format!("{x:?}")).collect::<Vec<_>>()})
        }
    }
}

pub fn json_of_relation(r: &RelationView) -> Result<Value> {
    Ok(json!({
        "id": r.id().to_base64(),
        "kind": r.kind()?.to_string(),
        "from": r.from_id()?.to_base64(),
        "to": r.to_id()?.to_base64(),
        "status": r.status()?.to_string(),
        "note": r.note()?,
        "position": r.position()?,
        "created_by": EntityId::from(r.created_by()?).to_base64(),
        "created": r.created()?,
        "closed_by": r.closed_by()?.map(|p| EntityId::from(p).to_base64()),
        "closed": r.closed()?,
        "closed_reason": r.closed_reason()?,
    }))
}

pub fn json_of_scope(s: &ScopeView) -> Result<Value> {
    Ok(json!({
        "id": s.id().to_base64(),
        "title": s.title()?,
        "status": s.status()?.to_string(),
        "intention": text(s.intention())?,
        "context": Value::from(s.context()?),
        "branches": scope_branches(&Value::from(s.context()?)),
        "started": s.started()?,
        "ended": s.ended()?,
        "created_by": EntityId::from(s.created_by()?).to_base64(),
        "created": s.created()?,
    }))
}

pub fn json_of_message(m: &MessageView) -> Result<Value> {
    Ok(json!({
        "id": m.id().to_base64(),
        "author": EntityId::from(m.author()?).to_base64(),
        "created": m.created()?,
        "content": text(m.content())?,
        "priority": m.priority()?.to_string(),
        "status": m.status()?.to_string(),
        "task": m.task()?.map(|r| EntityId::from(r).to_base64()),
        "anchor": m.anchor()?.map(|r| EntityId::from(r).to_base64()),
        "scope": m.scope()?.map(|r| EntityId::from(r).to_base64()),
        "decision": m.decision()?.map(|r| EntityId::from(r).to_base64()),
        "reply_to": m.reply_to()?.map(|r| EntityId::from(r).to_base64()),
        "thread": m.thread()?.map(|r| EntityId::from(r).to_base64()),
        "context": Value::from(m.context()?),
        "edited": m.edited()?,
        // Who last edited its text: its author, or an agent that succeeded its author.
        "edited_by": m.edited_by()?.map(|p| EntityId::from(p).to_base64()),
    }))
}

pub fn json_of_notification(n: &NotificationView) -> Result<Value> {
    Ok(json!({
        "id": n.id().to_base64(),
        "recipient": EntityId::from(n.recipient()?).to_base64(),
        "type": n.notification_type()?.to_string(),
        "status": n.status()?.to_string(),
        "priority": n.priority()?.to_string(),
        "created": n.created()?,
        "updated": n.updated()?,
        "target": Value::from(n.target()?),
        "originator": Value::from(n.originator()?),
    }))
}

pub fn json_of_principal(p: &PrincipalView) -> Result<Value> {
    Ok(json!({
        "id": p.id().to_base64(),
        "kind": p.kind()?.to_string(),
        "handle": p.handle()?,
        "display_name": p.display_name()?,
        // A role this build does not know, such as `supervisor` on a principal `fwd admin migrate`
        // has not carried over yet, reads as none.
        "agent_role": p.agent_role().ok().flatten().map(|r| r.to_string()),
        "active": p.active()?,
        "member": p.member()?,
        "belongs": belongs(p),
        // The Claude Code agent whose commands act as it (`subject::claude_code`), when one is bound.
        "subject": p.subject().ok().flatten(),
        // The principal that took over its job (`fwd principal succeed`), whom what reaches it reaches.
        "succeeded_by": p.succeeded_by().ok().flatten().map(|s| EntityId::from(s).to_base64()),
        // What it answers first for as a triage agent (`Serving`): the ids of the scopes it serves
        // and whether it serves the whole project; null for any other principal.
        "serves": (role_of(p) == Some(AgentRole::Triage)).then(|| {
            let serving = Serving::of(p);
            json!({
                "scopes": serving.scopes.iter().map(|s| s.to_base64()).collect::<Vec<_>>(),
                "project_wide": serving.project_wide,
            })
        }),
        // When it last wrote, which `dormancy` reads; null before its first write since 2026-09-30.
        "last_active": p.last_active().ok().flatten(),
    }))
}

pub fn json_of_decision(d: &DecisionView) -> Result<Value> {
    let number = d.number()?;
    Ok(json!({
        "id": d.id().to_base64(),
        "number": number,
        "ref": format!("D{number}"),
        "title": d.title()?,
        "status": d.status()?.to_string(),
        "area": d.area()?,
        "scope": governs(d)?.map(|s| s.to_base64()),
        "context": text(d.context())?,
        "decision": text(d.decision())?,
        "consequences": text(d.consequences())?,
        "author": EntityId::from(d.author()?).to_base64(),
        "decided_by": d.decided_by()?.map(|r| EntityId::from(r).to_base64()),
        "created": d.created()?,
        "decided": d.decided()?,
        "closed_by": d.closed_by()?.map(|r| EntityId::from(r).to_base64()),
        "closed": d.closed()?,
        "closed_reason": d.closed_reason()?,
        "closed_reason_message": d.closed_reason_message()?.map(|r| EntityId::from(r).to_base64()),
    }))
}

pub fn parse_id(s: &str) -> Result<EntityId> {
    s.parse::<EntityId>().map_err(|e| anyhow!("`{s}` is not an entity id: {e}"))
}

// ---------------------------------------------------------------------------------------------
// Naming entities on a command line

/// The collections an id on a command line may name, so a short name is looked for only where
/// the command can use what it finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Task,
    Message,
    Decision,
    Scope,
    Anchor,
    Relation,
    Notification,
    /// A person or an agent: named inline in a message's text, never as a command's id argument.
    Principal,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Task => "task",
            Kind::Message => "message",
            Kind::Decision => "decision",
            Kind::Scope => "scope",
            Kind::Anchor => "anchor",
            Kind::Relation => "relation",
            Kind::Notification => "notification",
            Kind::Principal => "principal",
        }
    }

    /// The ids of every entity of this kind.
    async fn ids(self, ctx: &Context) -> Result<Vec<EntityId>> {
        fn ids<V: ankurah::model::View>(found: Vec<V>) -> Vec<EntityId> {
            found.iter().map(|v| v.id()).collect()
        }
        let all = "created > 0";
        Ok(match self {
            Kind::Task => ids::<TaskView>(ctx.fetch(all).await?),
            Kind::Message => ids::<MessageView>(ctx.fetch(all).await?),
            Kind::Decision => ids::<DecisionView>(ctx.fetch(all).await?),
            Kind::Scope => ids::<ScopeView>(ctx.fetch(all).await?),
            Kind::Anchor => ids::<CodeAnchorView>(ctx.fetch(all).await?),
            Kind::Relation => ids::<RelationView>(ctx.fetch(all).await?),
            Kind::Notification => ids::<NotificationView>(ctx.fetch(all).await?),
            Kind::Principal => ids::<PrincipalView>(ctx.fetch("kind = 'human' OR kind = 'agent'").await?),
        })
    }
}

/// Every kind of entity a command line names by id.
pub const ANY: &[Kind] = &[Kind::Task, Kind::Message, Kind::Decision, Kind::Scope, Kind::Anchor, Kind::Relation];

/// The shortest prefix of an id that a command line takes for the whole id.
pub const MIN_PREFIX: usize = 6;

/// An entity named on a command line: its full id; `D<number>` for a decision; or a prefix of at
/// least six characters of the id of exactly one entity among `kinds`. A full id is taken as it
/// is (the command then says whether it can use it); a prefix that matches several entities is
/// refused with each of them listed, and one that matches none is refused plainly. `D<number>` is
/// read as a decision's number first. With five digits or more it is also long enough to be the
/// start of an id, and it is tried as one when no decision has that number; while one has, it
/// names that decision, which a command that takes no decision refuses.
pub async fn resolve_id(ctx: &Context, s: &str, kinds: &[Kind]) -> Result<EntityId> {
    if let Ok(id) = s.parse::<EntityId>() {
        return Ok(id);
    }
    let numbered: Vec<EntityId> = match rules::decision_number(s) {
        Some(n) => decisions(ctx).await?.iter().filter(|d| d.number().ok() == Some(n)).map(|d| d.id()).collect(),
        None => Vec::new(),
    };
    // The entities of `kinds`, read only when the name can start an id and no decision's number
    // took it.
    let mut found = Vec::new();
    if numbered.is_empty() && may_start_an_id(s) {
        for kind in kinds {
            found.extend(kind.ids(ctx).await?.into_iter().map(|id| (id, *kind)));
        }
    }
    name_to_id(s, kinds, &numbered, &found)
}

/// The entity a name stands for by [`resolve_id`]'s rule, among entities already in hand, as the
/// page's pickers hold them: `numbered` has the decisions with the number a `D<number>` name
/// gives, and `found` each entity the name may start the id of, with its kind (one of a kind not
/// in `kinds` is passed over).
pub fn name_to_id(s: &str, kinds: &[Kind], numbered: &[EntityId], found: &[(EntityId, Kind)]) -> Result<EntityId> {
    if let Ok(id) = s.parse::<EntityId>() {
        return Ok(id);
    }
    let wanted = || kinds.iter().map(|k| k.name()).collect::<Vec<_>>().join(", ");
    if let Some(n) = rules::decision_number(s) {
        match numbered {
            // No decision has the number: long enough to be the start of an id, tried as one below.
            [] if s.len() >= MIN_PREFIX => {}
            _ if !kinds.contains(&Kind::Decision) => bail!("`{s}` names a decision, and this takes a {}", wanted()),
            [one] => return Ok(*one),
            [] => bail!("there is no decision D{n}"),
            several => bail!(
                "D{n} names {} decisions ({}); give one's id",
                several.len(),
                several.iter().map(|d| d.to_base64()).collect::<Vec<_>>().join(", ")
            ),
        }
    }
    if !may_start_an_id(s) {
        bail!("`{s}` is not an entity id: give the whole id, its first {MIN_PREFIX} characters or more, or D<number> for a decision");
    }
    let matches: Vec<(EntityId, Kind)> =
        found.iter().copied().filter(|(id, kind)| kinds.contains(kind) && id.to_base64().starts_with(s)).collect();
    match matches.as_slice() {
        [(id, _)] => Ok(*id),
        [] => bail!("no {} has an id starting with `{s}`", wanted()),
        several => {
            let listed: Vec<String> = several
                .iter()
                .map(|(id, kind)| {
                    format!("{} {}", kind.name(), id.to_base64().chars().take(s.len() + 4).collect::<String>())
                })
                .collect();
            bail!("`{s}` starts the ids of {} entities ({}); give more of the id", several.len(), listed.join(", "))
        }
    }
}

/// The scope a command line names: its whole id, its title (without regard to case or to the
/// spaces around it), or the first six characters or more of its id, for a command that takes a
/// scope a person names by its title as often as by its id (`fwd decision attach D12 "Cloud
/// Hosting"`). A title several scopes share, and a start several ids share, are refused with
/// each of those scopes listed; a name that fits no scope is refused saying so, a whole id among
/// them, so that `fwd decision list --attached` given a mistyped id says so rather than listing
/// nothing.
pub async fn resolve_scope(ctx: &Context, name: &str) -> Result<EntityId> {
    let scopes: Vec<ScopeView> = ctx.fetch("created > 0").await?;
    let held: Vec<(EntityId, String)> = scopes.iter().map(|s| (s.id(), s.title().unwrap_or_default())).collect();
    scope_by_name(name, &held)
}

/// The scope `name` names among `scopes`, each scope's id and title, by [`resolve_scope`]'s rule,
/// so that the page's box for a task's scopes reads a name as the command line does: its whole id,
/// its title (without regard to case or to the spaces around it), or the first six characters or
/// more of its id; a title several share, a start several ids share and a name that fits none are
/// refused with the sentence the command line gives.
pub fn scope_by_name(name: &str, scopes: &[(EntityId, String)]) -> Result<EntityId> {
    if let Ok(id) = name.parse::<EntityId>() {
        if !scopes.iter().any(|(s, _)| *s == id) {
            bail!("no scope of this project has the id {id}");
        }
        return Ok(id);
    }
    let wanted = name.trim();
    let listed = |found: &[&(EntityId, String)], chars: usize| {
        found
            .iter()
            .map(|(id, _)| id.to_base64().chars().take(chars).collect::<String>())
            .collect::<Vec<_>>()
            .join(", ")
    };
    // A title that could not be read is held as a blank, which names nothing.
    let titled: Vec<&(EntityId, String)> = scopes
        .iter()
        .filter(|(_, title)| !title.trim().is_empty() && title.trim().eq_ignore_ascii_case(wanted))
        .collect();
    match titled.as_slice() {
        [(one, _)] => return Ok(*one),
        [] => {}
        several => bail!(
            "{} scopes have the title `{wanted}` ({}); give one's id or its first characters",
            several.len(),
            listed(several, 8)
        ),
    }
    let started: Vec<&(EntityId, String)> = if may_start_an_id(wanted) {
        scopes.iter().filter(|(id, _)| id.to_base64().starts_with(wanted)).collect()
    } else {
        Vec::new()
    };
    match started.as_slice() {
        [(one, _)] => Ok(*one),
        [] => bail!("no scope has the title `{wanted}` or an id starting with it"),
        several => bail!(
            "`{wanted}` starts the ids of {} scopes ({}); give more of the id",
            several.len(),
            listed(several, wanted.len() + 4)
        ),
    }
}

/// Whether a name is long enough, and made of the right characters, to be the start of an id.
pub(crate) fn may_start_an_id(s: &str) -> bool {
    s.len() >= MIN_PREFIX && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

// ---------------------------------------------------------------------------------------------
// Repos and anchors

/// The Repo entity for a repository root: keyed by the upstream remote (`origin`), so every clone
/// and every worktree of one repository share one entity (Daniel, 2026-09-25: repositories are
/// identified by their upstream, never by a worktree or a directory). A repository with no
/// remote has no identity yet and is refused. Created on first use; the name is the directory
/// that first registered it.
#[cfg(feature = "native")]
pub async fn ensure_repo(ctx: &Context, root: &std::path::Path) -> Result<EntityId> {
    let name = root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or("repo".into());
    let remote = crate::anchoring::git(root, &["remote", "get-url", "origin"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let Some(remote) = remote else {
        bail!(
            "the repository at {} has no origin remote; add one (`git remote add origin <url>`) before anchoring code in it: a repository is identified by its upstream, never by its directory",
            root.display()
        );
    };
    let key = repo_key(&remote);
    // Every repository of the project: a handful of entities, matched in Rust because the
    // remote URLs live in a Json list.
    let repos: Vec<RepoView> = ctx.fetch("default_branch != ''").await?;
    for r in &repos {
        let urls: Value = r.remote_urls()?.into();
        if urls.as_array().into_iter().flatten().filter_map(Value::as_str).any(|u| repo_key(u) == key) {
            return Ok(r.id());
        }
    }
    let default_branch = crate::anchoring::git(root, &["symbolic-ref", "--short", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or("main".into());
    let trx = ctx.begin();
    let id = trx.create(&Repo { name, remote_urls: Json::new(json!([remote])), default_branch }).await?.id();
    trx.commit().await?;
    Ok(id)
}

/// The upstream keys a Repo entity answers to (its remote URLs, normalized).
pub fn repo_upstreams(repo: &RepoView) -> Result<Vec<String>> {
    let urls: Value = repo.remote_urls()?.into();
    Ok(urls.as_array().into_iter().flatten().filter_map(Value::as_str).map(repo_key).collect())
}

/// The identity of an upstream: `git@github.com:Org/X.git`, `https://github.com/org/x.git` and
/// `ssh://git@github.com/org/x` all name one repository, so the key is the host (with its port
/// when one is given) and the path, lower-cased, without scheme, user, empty segments or
/// `.git`. A local path used as a remote is kept as given.
pub fn repo_key(url: &str) -> String {
    let s = url.trim();
    let (authority, path) = if let Some((_scheme, rest)) = s.split_once("://") {
        rest.split_once('/').unwrap_or((rest, ""))
    } else if let Some((authority, path)) = s.split_once(':').filter(|(a, _)| !a.contains('/')) {
        (authority, path)
    } else {
        let local = s.trim_end_matches('/');
        return local.strip_suffix(".git").unwrap_or(local).to_string();
    };
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let path = path.split('/').filter(|seg| !seg.is_empty()).collect::<Vec<_>>().join("/");
    let path = path.strip_suffix(".git").unwrap_or(&path).to_string();
    if path.is_empty() {
        host.to_lowercase()
    } else {
        format!("{host}/{path}").to_lowercase()
    }
}

/// A highlight a command puts something on, named by its lines ([`highlight`]): the anchor made
/// already for them, or the anchor to make, which the command writes in the transaction of what it
/// puts on it ([`post_on_highlight`], [`add_highlight_step`]).
#[cfg(feature = "native")]
pub enum Highlight {
    Made(EntityId),
    New(Box<CodeAnchor>),
}

/// The highlight `spec` describes in `repo`: an anchor with the same key (the same lines of the
/// same content at the same path, or the same whole file) is the one made already, the earliest
/// when two machines racing each made one, with the side it was made on; with none, the anchor to
/// make, by `by`, now. Relating such twins is section 3.7 of the robust-anchoring proposal, later.
#[cfg(feature = "native")]
pub async fn highlight(
    ctx: &Context,
    by: EntityId,
    repo: EntityId,
    spec: &crate::anchoring::AnchorSpec,
) -> Result<Highlight> {
    let basis = spec
        .basis_blob
        .as_deref()
        .ok_or_else(|| anyhow!("{} has no basis: anchors are made from what `anchoring::form` records", spec.path))?;
    let key = crate::anchoring::anchor_key(&repo.to_base64(), &spec.path, basis, spec.lines);
    let same: Vec<CodeAnchorView> = ctx.fetch(with_string("anchor_key = ?", &key)?).await?;
    if let Some(first) = same.iter().min_by_key(|a| (a.created().unwrap_or(i64::MAX), a.id())) {
        return Ok(Highlight::Made(first.id()));
    }
    let (start_line, end_line) = spec.lines.unwrap_or((0, 0));
    Ok(Highlight::New(Box::new(CodeAnchor {
        repo: Ref::new(repo),
        path: spec.path.clone(),
        anchor_key: Some(key),
        span_key: spec.lines.map(|_| crate::anchoring::span_key(&spec.anchored_text)),
        basis_blob: spec.basis_blob.clone(),
        head_blob: spec.head_blob.clone(),
        index_blob: spec.index_blob.clone(),
        nearest_blob: spec.nearest_blob.clone(),
        basis_patch: spec.basis_patch.clone(),
        blob_sha: None,
        commit_sha: spec.commit_sha.clone(),
        side: spec.side,
        start_line,
        end_line,
        whole_file: Some(spec.whole_file()),
        anchored_text: spec.anchored_text.clone(),
        context_before: spec.context_before.clone(),
        context_after: spec.context_after.clone(),
        pieces: Json::new(serde_json::to_value(&spec.pieces)?),
        symbol_path: None,
        created_by: Ref::new(by),
        created: now(),
    })))
}

/// The anchor `spec` describes in `repo`, made once ([`highlight`], [`make_highlight`]).
#[cfg(feature = "native")]
pub async fn create_anchor(
    ctx: &Context,
    by: EntityId,
    repo: EntityId,
    spec: &crate::anchoring::AnchorSpec,
) -> Result<EntityId> {
    make_highlight(ctx, highlight(ctx, by, repo, spec).await?).await
}

/// The highlight `on`, made here, alone in its transaction, when it is new (`fwd anchor create`,
/// which puts nothing on it).
#[cfg(feature = "native")]
pub async fn make_highlight(ctx: &Context, on: Highlight) -> Result<EntityId> {
    match on {
        Highlight::Made(id) => Ok(id),
        Highlight::New(anchor) => {
            let trx = ctx.begin();
            let id = trx.create(&*anchor).await?.id();
            trx.commit().await?;
            Ok(id)
        }
    }
}

/// Posts a message on the highlight `on` (`fwd ask --path`, `fwd comment --path`): on one made
/// already as [`post_message`] posts, and on a new one in one transaction with the highlight.
/// Answers the message and the highlight.
#[cfg(feature = "native")]
pub async fn post_on_highlight(
    ctx: &Context,
    author: EntityId,
    content: &str,
    priority: Priority,
    mut mc: MessageContext,
    on: Highlight,
) -> Result<(EntityId, EntityId)> {
    match on {
        Highlight::Made(highlight) => {
            mc.anchor_id = Some(highlight);
            Ok((post_message(ctx, author, content, priority, mc).await?, highlight))
        }
        Highlight::New(anchor) => {
            let mut post = Post::prepare(ctx, author, content, priority, mc).await?;
            // A new highlight and its message are one transaction for atomicity: a message refused
            // leaves no highlight behind, and whoever hears of the highlight hears of its message
            // in the same publication. That also keeps the message out of a race in Ankurah's
            // durable node (task F-Z2QwJe): a live query opened on a new highlight, as the VS Code
            // extension opens one on its messages as soon as it hears of it, could load a message
            // committed apart before the node published it, and the publication then told no live
            // query, the routing service's among them, so no one was told of the message.
            let trx = ctx.begin();
            let highlight = trx.create(&*anchor).await?.id();
            post.mc.anchor_id = Some(highlight);
            let message = post.write(&trx).await?;
            trx.commit().await?;
            Ok((message, highlight))
        }
    }
}

/// Makes the highlight `on` the next step of `task`, with `first`, when given, as its first comment
/// (`fwd step add <task> --path`). On a highlight made already, the step is added as [`add_step`]
/// adds one and then the comment posted, so that the comment's notifications know whose checklist
/// it is on; a new highlight, its step and its comment are written in one transaction. The comment
/// names its step ([`FIRST_OF_STEP_KEY`]), so that the task's people hear of it as the step's
/// first comment whichever of the two routing reaches first, never as a thread the step found on
/// its lines. The comment is checked before anything is written, so a text refused for naming
/// nothing leaves no highlight and no step. Answers the highlight, the step and the comment.
#[cfg(feature = "native")]
pub async fn add_highlight_step(
    ctx: &Context,
    by: EntityId,
    task: EntityId,
    on: Highlight,
    first: Option<&str>,
) -> Result<(EntityId, EntityId, Option<EntityId>)> {
    let post = match first {
        Some(text) => Some(Post::prepare(ctx, by, text, Priority::Next, MessageContext::default()).await?),
        None => None,
    };
    match on {
        Highlight::Made(highlight) => {
            let edge = add_step(ctx, by, task, highlight).await?;
            let message = match post {
                Some(mut post) => {
                    post.mc.anchor_id = Some(highlight);
                    post.first_of_step(edge);
                    let trx = ctx.begin();
                    let id = post.write(&trx).await?;
                    trx.commit().await?;
                    Some(id)
                }
                None => None,
            };
            Ok((highlight, edge, message))
        }
        Highlight::New(anchor) => {
            // What `link` checks of a step, for a highlight that no relation reaches yet: that the
            // step is of a task; a new highlight is a step of nothing yet, and closes no cycle. The
            // highlight, its step and its comment are one transaction, as a new highlight and its
            // message are in `post_on_highlight`, which says why.
            let task_kind = kind_of(ctx, task).await;
            let position = next_position(ctx, task).await?;
            let trx = ctx.begin();
            let highlight = trx.create(&*anchor).await?.id();
            if let Some(sentence) =
                ends_refused(RelationKind::StepOf, (highlight, Some(Kind::Anchor)), (task, task_kind))
            {
                bail!("refused: {sentence}");
            }
            let edge = trx
                .create(&new_relation(
                    by,
                    RelationKind::StepOf,
                    (highlight, task),
                    RelationStatus::Accepted,
                    step_via(by),
                    None,
                    Some(position),
                ))
                .await?
                .id();
            let message = match post {
                Some(mut post) => {
                    post.mc.anchor_id = Some(highlight);
                    post.first_of_step(edge);
                    Some(post.write(&trx).await?)
                }
                None => None,
            };
            trx.commit().await?;
            Ok((highlight, edge, message))
        }
    }
}

/// The anchor's pieces; none on an anchor made before pieces existed.
fn pieces_of(a: &CodeAnchorView) -> Result<Vec<Piece>> {
    match a.pieces() {
        Ok(pieces) => serde_json::from_value(Value::from(pieces))
            .with_context(|| format!("anchor {} holds pieces that do not read as runs of lines", a.id().to_base64())),
        Err(ankurah::property::PropertyError::Missing) => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(feature = "native")]
pub fn spec_of(a: &CodeAnchorView) -> Result<crate::anchoring::AnchorSpec> {
    Ok(crate::anchoring::AnchorSpec {
        path: a.path()?,
        lines: lines_of(a)?,
        basis_blob: a.basis_blob()?,
        head_blob: a.head_blob()?,
        index_blob: a.index_blob()?,
        nearest_blob: a.nearest_blob()?,
        basis_patch: a.basis_patch()?,
        pieces: pieces_of(a)?,
        commit_sha: a.commit_sha()?,
        legacy_blob: a.blob_sha()?,
        anchored_text: text(a.anchored_text())?,
        context_before: text(a.context_before())?,
        context_after: text(a.context_after())?,
        side: a.side()?,
    })
}

/// Resolves an anchor against `target` (`worktree`, `index`, `HEAD`, a ref or a blob id) in the
/// checkout at `repo_root`. An answer for the same anchor, file path and blob comes from `cache`
/// when it holds one, and a new one is kept there (never on the entity). An anchor made before
/// keys existed gains its key and basis when this checkout can supply them (`anchoring::backfill`).
#[cfg(feature = "native")]
pub async fn resolve_anchor(
    ctx: &Context,
    repo_root: &std::path::Path,
    anchor: EntityId,
    target: &str,
    cache: Option<&crate::sidecar::Sidecar>,
) -> Result<crate::anchoring::Resolution> {
    use crate::anchoring::{self, Against};
    let view: CodeAnchorView = ctx.get(anchor).await?;
    let spec = spec_of(&view)?;
    let Some(against) = Against::parse(repo_root, target)? else { return Ok(anchoring::absent(&spec)) };
    let located = anchoring::locate(repo_root, &spec, &against)?;
    let key = view.anchor_key()?.unwrap_or_else(|| format!("id:{}", anchor.to_base64()));
    let cached = match (cache, located.blob.as_deref()) {
        (Some(cache), Some(blob)) => cache.resolution(&key, &located.path, blob).unwrap_or_else(|e| {
            tracing::warn!("reading the anchors' store: {e}");
            None
        }),
        _ => None,
    };
    let mut resolution = match cached {
        Some(r) => r,
        None => {
            let r = anchoring::resolve_located(repo_root, &spec, &located);
            if let (Some(cache), Some(blob)) = (cache, located.blob.as_deref()) {
                if r.cacheable() {
                    if let Err(e) = cache.remember(&key, &located.path, blob, &r) {
                        tracing::warn!("writing the anchors' store: {e}");
                    }
                }
            }
            r
        }
    };
    // The backfill is an extra on the answer: its failure is logged and the answer stands.
    if view.anchor_key()?.is_none() {
        if let Err(e) = backfill_anchor(ctx, repo_root, &view, &spec, &against, &located, &resolution).await {
            tracing::warn!("giving anchor {} a key and basis: {e}", anchor.to_base64());
        }
    }
    resolution.target = Some(anchoring::target_of(&against, &located));
    Ok(resolution)
}

/// Writes what `anchoring::backfill` finds for an anchor of the first cut: its basis, the nearest
/// blob and patch that rebuild it elsewhere, its text's fingerprint, and the key they make.
#[cfg(feature = "native")]
async fn backfill_anchor(
    ctx: &Context,
    repo_root: &std::path::Path,
    view: &CodeAnchorView,
    spec: &crate::anchoring::AnchorSpec,
    against: &crate::anchoring::Against,
    located: &crate::anchoring::Located,
    resolution: &crate::anchoring::Resolution,
) -> Result<()> {
    let Some(fill) = crate::anchoring::backfill(repo_root, spec, against, located, resolution)? else {
        return Ok(());
    };
    let repo = EntityId::from(view.repo()?);
    let key = crate::anchoring::anchor_key(&repo.to_base64(), &spec.path, &fill.basis_blob, spec.lines);
    let trx = ctx.begin();
    let edit = view.edit(&trx)?;
    edit.anchor_key()?.set(&Some(key))?;
    edit.span_key()?.set(&Some(fill.span_key))?;
    edit.basis_blob()?.set(&Some(fill.basis_blob))?;
    edit.nearest_blob()?.set(&fill.nearest_blob)?;
    edit.basis_patch()?.set(&fill.basis_patch)?;
    trx.commit().await?;
    Ok(())
}

/// An anchor's lines as stored (0-based, the end exclusive), or none for an anchor on the whole
/// file, whose stored lines, 0 and 0, name no line of it. An anchor made before whole-file anchors
/// existed names lines.
pub fn lines_of(a: &CodeAnchorView) -> Result<Option<(i64, i64)>> {
    Ok(if a.whole_file()?.unwrap_or(false) { None } else { Some((a.start_line()?, a.end_line()?)) })
}

pub async fn anchors_in_path(ctx: &Context, path: &str) -> Result<Vec<CodeAnchorView>> {
    let mut found: Vec<CodeAnchorView> = ctx.fetch(with_string("path = ?", path)?).await?;
    found.sort_by_key(|a| (a.start_line().unwrap_or(0), a.created().unwrap_or(0)));
    Ok(found)
}

/// An anchor as the CLI and the extension read it. `whole_file` says it has no line range (its
/// `start_line` and `end_line` are then 0); `side` says where its lines were chosen (`worktree`,
/// or the `base` or `modified` side of a diff showing a revision); `pieces` name where each run of
/// its lines stood (`in`: `head`, `index`, `worktree` or `revision`, as `Piece::stage` says);
/// `blob_sha` is set only on anchors of the first cut.
pub fn json_of_anchor(a: &CodeAnchorView) -> Result<Value> {
    let side = a.side()?;
    let pieces: Vec<Value> = pieces_of(a)?
        .iter()
        .map(|p| {
            json!({
                "basis_lines": p.basis_lines,
                "head_lines": p.head_lines,
                "index_lines": p.index_lines,
                "in": p.stage(side),
            })
        })
        .collect();
    Ok(json!({
        "id": a.id().to_base64(),
        "repo": EntityId::from(a.repo()?).to_base64(),
        "path": a.path()?,
        "whole_file": a.whole_file()?.unwrap_or(false),
        "start_line": a.start_line()?,
        "end_line": a.end_line()?,
        "anchor_key": a.anchor_key()?,
        "span_key": a.span_key()?,
        "basis_blob": a.basis_blob()?,
        "head_blob": a.head_blob()?,
        "index_blob": a.index_blob()?,
        "nearest_blob": a.nearest_blob()?,
        "basis_patch_bytes": a.basis_patch()?.map(|p| p.len()),
        "pieces": pieces,
        "commit_sha": a.commit_sha()?,
        "blob_sha": a.blob_sha()?,
        "side": side.to_string(),
        "anchored_text": text(a.anchored_text())?,
        "created_by": EntityId::from(a.created_by()?).to_base64(),
        "created": a.created()?,
    }))
}
