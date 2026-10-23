//! What fwd tells a lead about creating a triage subagent, and what it tells a triage agent that
//! serves nothing, kept once here so that every place fwd gives them says the same words, and so
//! that the skills cite one text (Daniel, 2026-09-29: "giving automatic guidance on how and why to
//! best create a triage subagent"; docs/design/agents-and-inbox.md, "Lead"). The CLI prints them on
//! standard error, or in a hook's context when its output is the hook's JSON: the guidance when an
//! agent selects lead where no triage agent answers first for its scope, and whenever a lead's
//! inbox, or a question asked of no one, meets such a scope; the notice on every command of a
//! triage agent that serves nothing. Triage is per scope since 2026-09-30 (docs/DECISIONS.md,
//! D122), so the guidance names the scope it is printed for.

/// Why a scope needs a triage agent, and how a lead of it makes one, a Sonnet subagent that
/// annotates and relays (docs/DECISIONS.md, D129): `scope`, the scope the lead works in, as its
/// title and the first eight characters of its id, which no triage agent serves while no agent
/// serves the whole project either; none where the lead's checkout finds no scope and no agent
/// serves the whole project, when nothing answers first for what concerns no scope.
pub fn triage_subagent(scope: Option<(&str, &str)>) -> String {
    let (lacks, take) = match scope {
        Some((title, id)) => (
            format!("The scope \"{title}\" ({id}) has no triage agent, and no agent serves the whole project."),
            format!("--scope {id}"),
        ),
        None => (
            "This command works in no scope (no --scope, and no checkout that finds one), and no agent serves the whole project, so no triage agent answers for what concerns no scope.".to_string(),
            "--scope <scope id>".to_string(),
        ),
    };
    format!(
        "{lacks} The questions asked of no one in particular, comments on tasks and on code, proposals and reviews reach the triage agent of the scope they concern, else the one that serves the whole project; without one the questions wait on the Questions page, listed under their scope, and a lead that answers them itself fills its context with first-line traffic. A triage agent is a Sonnet agent, which annotates and relays rather than judging the work (docs/DECISIONS.md, D129). From Claude Code, spawn it as a background Sonnet subagent, briefed with the fwd-triage skill (integrations/claude-code/skills/fwd-triage in the fwdcentaur checkout) and told the project and a checkout of the scope to work in; a Codex lead launches a persistent Claude Sonnet agent through a route the person has authorized, else asks the person to start one, and never puts a model of its own harness in its place. Its first command names it, with a name no other principal of this project has, and selects triage for the scope, `fwd principal add <its name> --role triage {take}` (without --scope it takes the scope its checkout finds), which binds that principal to the agent that runs it, so that it acts as itself from then on (in Claude Code by the fwd plugin's stamp on its commands; over MCP by the thread ids Codex attaches to each call), and tells it of the questions that waited there. A small project with small scopes may instead endorse one triage agent for the whole project, `--project-wide` in place of --scope; a large one keeps one per scope. A triage agent relaunched later names itself as the old one's successor, `fwd principal add <its name> --succeeds <old name>`, which moves its scopes and its mail to it."
    )
}

/// The one line fwd prints on every command of the triage agent named `handle` while it serves
/// nothing, as the triage agents of before 2026-09-30 do until their leads bind or endorse them
/// (docs/DECISIONS.md, D122: the migration binds nothing), so that it learns why nothing reaches it
/// for its role, and can tell its lead, which the two commands the line names are for. Nothing
/// records that it was told: the line stands while the condition holds, as the guidance does.
pub fn unbound(handle: &str) -> String {
    format!(
        "@{handle} holds triage and serves no scope, so nothing reaches it for its role: bind it to the scope it triages, `fwd principal scope add <scope id> --for {handle}`, or, in a small project with small scopes only, endorse it for the whole project, `fwd principal scope project-wide --for {handle}`."
    )
}
