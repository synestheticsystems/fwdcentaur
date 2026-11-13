//! `fwd prime`: what an agent holds, in one command that the agent runs, or that a hook or a skill
//! runs for it as its context starts over (a new session, a compaction) with no round trip of the
//! agent's own. The working set is read by `fwd_core::working_set`, and printed from that one
//! structure as the text an agent reads or as the JSON a panel draws, within a budget of characters
//! that leaves out the least important records first and says what it left out.

use crate::{
    act_as, autostart, format_of, lead_guidance, print_value, project_here, runs_in, scope_for_checkout,
    unbound_notice, App, Cli, Command, Format,
};
use ankurah::EntityId;
use anyhow::{Context as _, Result};
use clap::Parser;
use fwd_core::daemon;
use fwd_core::model::*;
use fwd_core::project::{NamedProject, Paths};
use fwd_core::project_graph::{Followed, InProjectView, ProjectGraph};
use fwd_core::rules::ScopeHere;
use fwd_core::subject;
use fwd_core::working_set::{self, Holder, Holding, Limits, SectionKind};
use serde_json::{json, Value};
use std::future::Future;
use std::io::Write;
use std::time::Duration;

/// How many ready tasks `fwd prime` lists, so that what it puts into an agent's context stays
/// short; a list it cut says so ([`cut_line`]).
const PRIMED_READY: usize = 10;
/// How many unread notifications `fwd prime` lists, for the same reason.
const PRIMED_UNREAD: usize = 20;
const LIMITS: Limits = Limits { ready: PRIMED_READY, unread: PRIMED_UNREAD };

/// The budget of `--format hook` when `--max-chars` gives none: Claude Code keeps the first 10,000
/// characters of a SessionStart hook's output, and a longer one would lose its end, where the line
/// saying what was left out stands.
const HOOK_CHARS: usize = 10_000;

/// What the text ends with, before the line saying what was left out.
const NEXT: &str = "Use `fwd task next --claim` to take work, `fwd inbox pop` to read notifications.";

/// The two ways an agent no principal is bound to comes to hold something.
const NAME_YOURSELF: &str = "fwd principal add <name> --role <role>";
const SUCCEED: &str = "fwd principal add <name> --succeeds <old name>";

/// `fwd prime` for the agent `agent`, a command's subject: the stamp's, the hook's, or the own
/// thread of the session `--session` names (`session`). It runs from any directory, joining the
/// daemon through the directory's project, else through any project the daemon hosts, since every
/// project is in its one node. While no principal is bound to the agent, the hook of a session's own
/// thread (`thread_hook`) reads for the principal `--as` or FWD_AS names, as every hook of that
/// thread acts.
pub(crate) async fn for_agent(
    cli: &Cli,
    paths: Paths,
    format: Format,
    agent: &str,
    session: Option<&str>,
    thread_hook: bool,
) -> Result<()> {
    let Command::Prime { max_chars, include, timeout, .. } = &cli.command else {
        unreachable!("run for fwd prime alone")
    };
    let wanted = wanted(include);
    let here = project_here(cli, &paths)?;
    let client = match &here {
        Some(resolved) => daemon::ensure_daemon(&paths, &resolved.project, autostart(cli)?).await?,
        None => {
            let running = daemon::ensure_running(&paths, &autostart(cli)?).await?;
            let hosted =
                running.projects.into_iter().find(|p| p.error.is_none()).context("the daemon hosts no project")?;
            daemon::ensure_daemon(&paths, &NamedProject { id: hosted.id, name: hosted.name }, autostart(cli)?).await?
        }
    };
    let read = async {
        let graph = &client.graph;
        let mut holdings = working_set::read(graph, Holder::Agent(agent), &wanted, LIMITS).await?;
        if let (true, true, Some(name)) = (holdings.is_empty(), thread_hook && here.is_some(), &cli.act_as) {
            let p: PrincipalView = graph.get(act_as(graph, name).await?).await?;
            if p.active()? {
                holdings = working_set::read(graph, Holder::Principal(p.id()), &wanted, LIMITS).await?;
            }
        }
        let scope = match (&cli.scope, &here) {
            (Some(named), _) => Some(runs_in::scope_named(graph, &[], named).await?),
            (None, Some(resolved)) => match scope_for_checkout(&client, resolved).await? {
                ScopeHere::One(scope) => Some(scope),
                ScopeHere::None | ScopeHere::Several { .. } => None,
            },
            (None, None) => None,
        };
        // What the agent is told beside it is its principal's in the project the command runs in.
        let here = here.as_ref().map(|_| graph.record());
        let said = match holdings.iter().find(|h| Some(h.project.record) == here) {
            Some(h) => Said::to(graph, h.principal, scope).await?,
            None => Said::default(),
        };
        let primer = Primer::new(graph, holdings, Some(agent), here, scope, client.port, session).await?;
        anyhow::Ok((primer, said))
    };
    let (primer, said) = client.while_connected(read).await?;
    if primer.holdings.is_empty() && format == Format::Hook && timeout.is_none() {
        // As every hook of an agent that has not named itself prints nothing.
        return Ok(());
    }
    match primer.answer(format, *max_chars, &said.lines()) {
        Answer::Text(text) => println!("{text}"),
        Answer::Json(value, cut) => {
            print_value(Format::Json, &value);
            cut.into_iter().for_each(|cut| eprintln!("{cut}"));
        }
    }
    if format != Format::Hook {
        said.lines().iter().for_each(|line| eprintln!("fwd: {line}"));
    }
    Ok(())
}

/// `fwd prime` run as the principal `app` acts as: the person, the principal `--as` names, or the
/// thread an MCP call comes from, which is an agent and is read as one.
pub(crate) async fn for_app(app: &App, max_chars: Option<usize>, include: &[Section]) -> Result<()> {
    let graph = app.graph();
    let holder = match &app.subject {
        Some(agent) => Holder::Agent(agent),
        None => Holder::Principal(app.me),
    };
    let holdings = working_set::read(graph, holder, &wanted(include), LIMITS).await?;
    let said = Said::to(graph, app.me, app.scope).await?;
    let primer = Primer::new(graph, holdings, None, Some(graph.record()), app.scope, app.daemon_port, None).await?;
    match primer.answer(app.format, max_chars, &said.lines()) {
        Answer::Text(text) => println!("{text}"),
        Answer::Json(value, cut) => {
            app.print(value)?;
            cut.into_iter().for_each(|cut| app.note(cut));
        }
    }
    // The notice of a triage agent that serves nothing is said after every command
    // (`say_unbound`); the lead's guidance only here.
    if let (Some(guidance), false) = (said.guidance, app.format == Format::Hook) {
        app.note(format!("fwd: {guidance}"));
    }
    Ok(())
}

/// A part of what `fwd prime` prints, which `--include` selects: a caller that keeps the text where
/// any change costs it, such as a system prompt, takes the parts that change only when what the
/// agent holds does, and leaves out those that change with each notification and each task filed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Section {
    Questions,
    Assigned,
    Unread,
    Ready,
}

/// The sections of a working set `include` selects: every one when it names none.
fn wanted(include: &[Section]) -> Vec<SectionKind> {
    let selected = |kind: &SectionKind| {
        include.contains(&match kind {
            SectionKind::AskedOfYou | SectionKind::YouAsked => Section::Questions,
            SectionKind::Assigned => Section::Assigned,
            SectionKind::Unread => Section::Unread,
            SectionKind::Ready => Section::Ready,
        })
    };
    SectionKind::ALL.into_iter().filter(|kind| include.is_empty() || selected(kind)).collect()
}

/// What `fwd prime` tells the principal of the project it runs in beside what it holds, as its
/// other commands tell it: a lead's guidance on making a triage agent (`lead_guidance`), and the
/// notice of a triage agent that serves nothing (`unbound_notice`).
#[derive(Default)]
struct Said {
    guidance: Option<String>,
    unbound: Option<String>,
}

impl Said {
    async fn to(graph: &ProjectGraph, principal: EntityId, scope: Option<EntityId>) -> Result<Said> {
        Ok(Said {
            guidance: lead_guidance(graph, principal, scope).await?,
            unbound: unbound_notice(graph, principal).await?,
        })
    }

    fn lines(&self) -> Vec<String> {
        self.guidance.iter().chain(&self.unbound).cloned().collect()
    }
}

/// What `fwd prime` prints, before it is fitted to its budget.
struct Primer {
    holdings: Vec<Holding>,
    /// The agent read for, which the line for an agent no principal is bound to names.
    agent: Option<String>,
    /// The project a command run here without `--project` runs in, by its record; none outside
    /// every project.
    here: Option<EntityId>,
    /// The scope the command works in: the record of its project, and its title.
    scope: Option<(EntityId, String)>,
    /// The page's address, as `fwd ui --no-open` prints it without a project, for a panel that
    /// links to what it draws.
    page: String,
    /// The session `--session` named, which the command that prints the rest names too.
    session: Option<String>,
}

/// How many of each section's records are shown: for each holding, for each of its sections.
type Shown = Vec<Vec<usize>>;

/// What `fwd prime` prints: text, or JSON with the line saying what was left out, which is said
/// beside it.
enum Answer {
    Text(String),
    Json(Value, Option<String>),
}

/// One section's records left out.
struct Cut {
    what: String,
    shown: usize,
    of: usize,
    command: String,
}

impl Primer {
    async fn new(
        graph: &ProjectGraph,
        holdings: Vec<Holding>,
        agent: Option<&str>,
        here: Option<EntityId>,
        scope: Option<EntityId>,
        port: u16,
        session: Option<&str>,
    ) -> Result<Primer> {
        let scope = match scope {
            Some(scope) => match graph.follow::<ScopeView>(scope).await? {
                Followed::Held { found, .. } => Some((found.project_id()?, found.title()?)),
                Followed::NotHeld => None,
            },
            None => None,
        };
        Ok(Primer {
            holdings,
            agent: agent.map(str::to_string),
            here,
            scope,
            page: format!("http://127.0.0.1:{port}/"),
            session: session.map(str::to_string),
        })
    }

    /// What to print in `format`, fitted to `max_chars`, or with `--format hook` to [`HOOK_CHARS`];
    /// a hook's text ends with `said` before its last lines, as other commands say it beside their
    /// answer.
    fn answer(&self, format: Format, max_chars: Option<usize>, said: &[String]) -> Answer {
        let budget = max_chars.or((format == Format::Hook).then_some(HOOK_CHARS));
        // Counted as printed, its last newline with it.
        let size = |out: &str| out.chars().count() + 1;
        match format {
            Format::Json => {
                let pretty = |shown: &Shown| serde_json::to_string_pretty(&self.json(shown)).unwrap_or_default();
                let shown = self.fit(budget, |shown| size(&pretty(shown)));
                Answer::Json(self.json(&shown), cut_line(&self.cuts(&shown)))
            }
            Format::Text | Format::Hook => {
                let said = if format == Format::Hook { said } else { &[] };
                let shown = self.fit(budget, |shown| size(&self.text(shown, said)));
                Answer::Text(self.text(&shown, said))
            }
        }
    }

    /// How many of each section's records fit in `budget` characters as `measure` counts what they
    /// print: one record at a time is left out, from the end of the least important section with
    /// any left, the last holding's first, until the rest fits or nothing is left to leave out.
    fn fit(&self, budget: Option<usize>, measure: impl Fn(&Shown) -> usize) -> Shown {
        let mut shown: Shown =
            self.holdings.iter().map(|h| h.sections.iter().map(|s| s.records.len()).collect()).collect();
        let Some(budget) = budget else { return shown };
        // Every holding has the same sections, those `--include` selects.
        let sections = shown.first().map_or(0, Vec::len);
        let order: Vec<(usize, usize)> =
            (0..sections).rev().flat_map(|s| (0..shown.len()).rev().map(move |h| (h, s))).collect();
        while measure(&shown) > budget {
            let Some(&(h, s)) = order.iter().find(|(h, s)| shown[*h][*s] > 0) else { break };
            shown[h][s] -= 1;
        }
        shown
    }

    /// The text: for each holding, who the agent is there, then each section with records shown,
    /// one line each; then `said`, how to go on, and what was left out.
    fn text(&self, shown: &Shown, said: &[String]) -> String {
        if self.holdings.is_empty() {
            return self.unbound_line();
        }
        let mut lines = Vec::new();
        for (h, counts) in self.holdings.iter().zip(shown) {
            let scope = match &self.scope {
                Some((project, title)) if *project == h.project.record => format!(", in the scope {title}"),
                _ => String::new(),
            };
            lines.push(format!("fwdcentaur context for {} ({}) in {}{scope}.", h.handle, role_word(h), h.project.name));
            for (section, &n) in h.sections.iter().zip(counts).filter(|(_, n)| **n > 0) {
                lines.push(format!("{} ({}):", header(section.kind), section.total));
                lines.extend(section.records[..n].iter().map(|r| format!("{} {}", r.id.to_base64(), r.title)));
            }
        }
        lines.extend(said.iter().cloned());
        lines.push(NEXT.to_string());
        lines.extend(cut_line(&self.cuts(shown)));
        lines.join("\n")
    }

    /// The JSON a panel draws: the page's address, and for each principal its project, its name
    /// and its role, its records shown in the text's order, and for each section how many are shown
    /// of how many, with the command that prints the rest when some are left out.
    fn json(&self, shown: &Shown) -> Value {
        if self.holdings.is_empty() {
            let fix = format!("{NAME_YOURSELF}, or {SUCCEED} when this session carries on another's work");
            return json!({"state": "unbound", "page": self.page, "fix": fix});
        }
        let principals: Vec<Value> = self
            .holdings
            .iter()
            .zip(shown)
            .map(|(h, counts)| {
                let records: Vec<Value> = h
                    .sections
                    .iter()
                    .zip(counts)
                    .flat_map(|(s, &n)| s.records[..n].iter())
                    .map(|r| json!({"id": r.id.to_base64(), "kind": r.kind.to_string(), "title": r.title}))
                    .collect();
                let sections: Vec<Value> = h
                    .sections
                    .iter()
                    .zip(counts)
                    .map(|(s, &n)| {
                        let rest = (n < s.total).then(|| self.rest(h, s.kind, s.total));
                        json!({"section": s.kind.to_string(), "shown": n, "of": s.total, "rest": rest})
                    })
                    .collect();
                json!({
                    "project": {"id": h.project.uuid.to_string(), "name": h.project.name},
                    "name": h.handle,
                    "role": role_word(h),
                    "records": records,
                    "sections": sections,
                })
            })
            .collect();
        json!({"state": "bound", "page": self.page, "principals": principals})
    }

    /// The one line for an agent no principal is bound to, which says how it comes to hold work.
    fn unbound_line(&self) -> String {
        let whom = self.agent.as_deref().map_or_else(|| "this agent".to_string(), subject::words);
        format!(
            "fwd: nothing to show: no principal is bound to {whom} in any project; name yourself with `{NAME_YOURSELF}`, or `{SUCCEED}` when this session carries on another's work"
        )
    }

    /// Each section with records left out, of each holding, in the text's order; the project is
    /// named when the agent holds work in several.
    fn cuts(&self, shown: &Shown) -> Vec<Cut> {
        let several = self.holdings.len() > 1;
        let mut cuts = Vec::new();
        for (h, counts) in self.holdings.iter().zip(shown) {
            for (s, &n) in h.sections.iter().zip(counts).filter(|(s, n)| **n < s.total) {
                let what =
                    if several { format!("{} in {}", noun(s.kind), h.project.name) } else { noun(s.kind).into() };
                cuts.push(Cut { what, shown: n, of: s.total, command: self.rest(h, s.kind, s.total) });
            }
        }
        cuts
    }

    /// The command that prints the `total` records of the section `kind` of `h`, run where this
    /// one was: in another project than the one found here, with `--project`. The task list is
    /// given a limit of every task it is to list, since its own default would cut a long one again;
    /// no command lists the questions but this one, without the budget that left them out.
    fn rest(&self, h: &Holding, kind: SectionKind, total: usize) -> String {
        let project = if self.here == Some(h.project.record) {
            String::new()
        } else {
            format!(" {}", runs_in::project_flag(&h.project.name))
        };
        match kind {
            SectionKind::AskedOfYou | SectionKind::YouAsked => match &self.session {
                Some(session) => format!("fwd prime --session {session} --format text"),
                None => "fwd prime --format text".to_string(),
            },
            SectionKind::Assigned => format!("fwd{project} task list --mine --limit {total}"),
            SectionKind::Unread => format!("fwd{project} inbox --unread"),
            SectionKind::Ready => format!("fwd{project} task list --ready --limit {total}"),
        }
    }
}

/// What a holding's principal is, as `fwd principal list --format text` says it.
fn role_word(h: &Holding) -> String {
    match (h.kind, h.role) {
        (PrincipalKind::Human, _) => "person".to_string(),
        (_, Some(role)) => role.to_string(),
        (_, None) => "agent".to_string(),
    }
}

fn header(kind: SectionKind) -> &'static str {
    match kind {
        SectionKind::AskedOfYou => "Questions asked of you",
        SectionKind::YouAsked => "Questions you asked",
        SectionKind::Assigned => "Assigned to you",
        SectionKind::Unread => "Unread notifications",
        SectionKind::Ready => "Ready to start",
    }
}

fn noun(kind: SectionKind) -> &'static str {
    match kind {
        SectionKind::AskedOfYou => "questions asked of you",
        SectionKind::YouAsked => "questions you asked",
        SectionKind::Assigned => "tasks assigned to you",
        SectionKind::Unread => "unread notifications",
        SectionKind::Ready => "ready tasks",
    }
}

/// The line that ends what `fwd prime` prints when it left records out, as `fwd task list` says
/// when its limit cut it: how many of how many of each section it shows, and the commands that
/// print the rest, each named once; none when it left nothing out.
fn cut_line(cuts: &[Cut]) -> Option<String> {
    if cuts.is_empty() {
        return None;
    }
    let shown: Vec<String> = cuts.iter().map(|c| format!("{} of {} {}", c.shown, c.of, c.what)).collect();
    let mut commands: Vec<String> = Vec::new();
    for command in cuts.iter().map(|c| format!("`{}`", c.command)) {
        if !commands.contains(&command) {
            commands.push(command);
        }
    }
    let verb = if commands.len() == 1 { "lists" } else { "list" };
    Some(format!("fwd: {} shown; {} {verb} the rest", and_list(&shown), and_list(&commands)))
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(items: &[String]) -> String {
    match items {
        [rest @ .., last] if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => items.join(""),
    }
}

/// The deadline `fwd prime --timeout` gives, and whether its one line is JSON: read from the
/// command line before the command runs, so that it bounds all of it, the wait for the daemon
/// first.
pub(crate) struct Bound {
    within: Duration,
    json: bool,
}

pub(crate) fn bound() -> Option<Bound> {
    let cli = Cli::try_parse().ok()?;
    let Command::Prime { timeout: Some(seconds), .. } = cli.command else { return None };
    Some(Bound { within: Duration::from_secs(seconds), json: format_of(&cli) == Format::Json })
}

/// Runs `ran`, a `fwd prime` given `--timeout`, and ends the process with exit 0 whatever becomes of
/// it, so that a hook or a skill that runs it neither fails nor waits past the deadline: an error,
/// and a deadline that passes first, are said in one line, as JSON with `--format json`. The
/// process ends at once, without shutting the runtime down, which can wait for good on a thread
/// left inside Ankurah (`watch::end` says how).
pub(crate) async fn within(bound: Bound, ran: impl Future<Output = Result<()>>) -> ! {
    let why = match tokio::time::timeout(bound.within, ran).await {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(format!("{error:#}").replace('\n', " ")),
        Err(_) => Some(format!("it did not finish within {}s", bound.within.as_secs())),
    };
    match why {
        Some(why) if bound.json => println!("{}", json!({"state": "unavailable", "reason": why})),
        Some(why) => println!("fwd: nothing to show: {why}"),
        None => {}
    }
    let _ = std::io::stdout().flush();
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fwd_core::project_graph::ProjectRecord;
    use fwd_core::working_set::{self, Record, RecordKind};

    /// A holding in `project`, whose sections hold, in order, the first of each pair of records of
    /// the second.
    fn holding(project: u8, counts: [(usize, usize); 5]) -> Holding {
        let sections = SectionKind::ALL
            .iter()
            .zip(counts)
            .map(|(kind, (records, total))| working_set::Section {
                kind: *kind,
                total,
                records: (0..records)
                    .map(|i| Record {
                        id: EntityId::from_bytes([i as u8; 32]),
                        kind: RecordKind::Task,
                        title: format!("{kind} {i}"),
                    })
                    .collect(),
            })
            .collect();
        let name = ["alpha", "beta"][usize::from(project)].to_string();
        Holding {
            project: ProjectRecord { record: EntityId::from_bytes([100 + project; 32]), uuid: uuid::Uuid::nil(), name },
            principal: EntityId::from_bytes([200; 32]),
            handle: "impl-a".into(),
            kind: PrincipalKind::Agent,
            role: Some(AgentRole::Implementer),
            sections,
        }
    }

    /// A primer of `holdings`, run in the first one's project.
    fn primer(holdings: Vec<Holding>) -> Primer {
        let here = holdings.first().map(|h| h.project.record);
        Primer { holdings, agent: None, here, scope: None, page: "http://127.0.0.1:1/".into(), session: None }
    }

    /// Over its budget, `fwd prime` leaves out one record at a time from the least important end:
    /// every holding's ready tasks first, the last holding's before the first's, then the unread
    /// notifications, then the assigned tasks, then the questions; and its last line names each
    /// section it cut, of how many, and the commands that print the rest.
    #[test]
    fn a_budget_leaves_out_the_least_important_records_first_and_the_last_line_says_which() {
        let p = primer(vec![
            holding(0, [(1, 1), (1, 1), (1, 1), (2, 2), (3, 3)]),
            holding(1, [(0, 0), (0, 0), (0, 0), (0, 0), (2, 2)]),
        ]);
        // A measure that counts the records shown, so that a budget of n keeps n of them.
        let records = |shown: &Shown| shown.iter().flatten().sum::<usize>();
        let kept = [
            [[1, 1, 1, 2, 3], [0, 0, 0, 0, 2]],
            [[1, 1, 1, 2, 3], [0, 0, 0, 0, 1]],
            [[1, 1, 1, 2, 3], [0, 0, 0, 0, 0]],
            [[1, 1, 1, 2, 2], [0, 0, 0, 0, 0]],
            [[1, 1, 1, 2, 1], [0, 0, 0, 0, 0]],
            [[1, 1, 1, 2, 0], [0, 0, 0, 0, 0]],
            [[1, 1, 1, 1, 0], [0, 0, 0, 0, 0]],
            [[1, 1, 1, 0, 0], [0, 0, 0, 0, 0]],
            [[1, 1, 0, 0, 0], [0, 0, 0, 0, 0]],
            [[1, 0, 0, 0, 0], [0, 0, 0, 0, 0]],
            [[0, 0, 0, 0, 0], [0, 0, 0, 0, 0]],
        ];
        for (budget, kept) in (0..=10).rev().zip(kept) {
            assert_eq!(p.fit(Some(budget), records), kept.map(Vec::from), "a budget of {budget} records");
        }
        // With nothing left to leave out, what is left is printed whatever its size.
        assert_eq!(p.fit(Some(0), |_| 100), [vec![0; 5], vec![0; 5]]);

        let text = p.text(&kept[3].map(Vec::from).into(), &[]);
        assert_eq!(
            text.lines().last(),
            Some(
                "fwd: 2 of 3 ready tasks in alpha and 0 of 2 ready tasks in beta shown; `fwd task list --ready --limit 3` and `fwd --project 'beta' task list --ready --limit 2` list the rest"
            ),
            "{text}"
        );
    }

    /// The last line names each section cut and each command that prints the rest once, `fwd task
    /// list` given a limit of every task it is to list, as its own default would cut a long list
    /// again; a text that left nothing out ends with how to go on, however near its caps it came.
    #[test]
    fn the_last_line_names_each_section_cut_and_each_command_once() {
        let p = primer(vec![holding(0, [(0, 0), (0, 0), (0, 0), (20, 20), (10, 10)])]);
        let text = p.text(&p.fit(None, |_| 0), &[]);
        assert_eq!(text.lines().last(), Some(NEXT), "{text}");
        let p = primer(vec![holding(0, [(1, 1), (1, 1), (0, 0), (20, 31), (10, 60)])]);
        assert_eq!(
            cut_line(&p.cuts(&p.fit(None, |_| 0))).as_deref(),
            Some("fwd: 20 of 31 unread notifications and 10 of 60 ready tasks shown; `fwd inbox --unread` and `fwd task list --ready --limit 60` list the rest")
        );
        assert_eq!(
            cut_line(&p.cuts(&vec![vec![0, 0, 0, 20, 10]])).as_deref(),
            Some("fwd: 0 of 1 questions asked of you, 0 of 1 questions you asked, 20 of 31 unread notifications and 10 of 60 ready tasks shown; `fwd prime --format text`, `fwd inbox --unread` and `fwd task list --ready --limit 60` list the rest")
        );
    }
}
