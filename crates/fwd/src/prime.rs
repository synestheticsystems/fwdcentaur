//! `fwd prime`: what an agent holds, in one command that the agent runs, or that a hook or a skill
//! runs for it as its context starts over (a new session, a compaction) with no round trip of the
//! agent's own. Its active intentions come first, read from the intentions assigned to its
//! principals (`fwd_core::intentions::assigned_to`); then the working set, read by
//! `fwd_core::working_set`. Both are printed as the text an agent reads or as the JSON a panel
//! draws, within a budget of characters that leaves out the least important records first, never an
//! intention, and says what it left out.

use crate::{
    autostart, format_of, lead_guidance, project_here, runs_in, scope_for_checkout, thread_hook_stand_in,
    unbound_notice, App, Cli, Command, Format, Output,
};
use ankurah::EntityId;
use ankurah_commons::records::ClosedView;
use anyhow::{Context as _, Result};
use clap::Parser;
use fwd_core::daemon;
use fwd_core::intentions::{self, Parts};
use fwd_core::model::*;
use fwd_core::project::{self, NamedProject, Paths};
use fwd_core::project_graph::{Followed, InProjectView, ProjectGraph, Whose};
use fwd_core::route::page_path;
use fwd_core::rules::{short_id, ScopeHere};
use fwd_core::subject;
use fwd_core::working_set::{self, Addressee, Asking, Extent, Holder, Holding, Record, RecordKind, SectionKind};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::future::Future;
use std::io::Write;
use std::sync::{Mutex, PoisonError, TryLockError};
use std::time::Duration;
use uuid::Uuid;

/// How many ready tasks `fwd prime` lists, so that what it puts into an agent's context stays
/// short; a list it cut says so ([`cut_line`]).
const PRIMED_READY: usize = 10;
/// How many unread notifications `fwd prime` lists, for the same reason.
const PRIMED_UNREAD: usize = 20;

/// The budget of `--format hook` when `--max-chars` gives none: Claude Code keeps the first 10,000
/// characters of a SessionStart hook's output, and a longer one would lose its end, where the line
/// saying what was left out stands.
const HOOK_CHARS: usize = 10_000;

/// The two ways an agent no principal is bound to comes to hold something.
const NAME_YOURSELF: &str = "fwd principal add <name> --role <role>";
const SUCCEED: &str = "fwd principal add <name> --succeeds <old name>";

/// `fwd prime` for the agent `agent`, a command's subject: the stamp's, the hook's, or the own
/// thread of the session `--session` names (`session`). It runs from any directory, joining the
/// daemon through the directory's project, else through any project the daemon hosts, since every
/// project is in its one node, and it reads the ready tasks of the project the command runs in
/// alone. While no principal of that project is bound to the agent, the hook of a session's own
/// thread (`thread_hook`) reads for the principal `--as` or FWD_AS names, as every hook of that
/// thread acts.
pub(crate) async fn for_agent(
    cli: &Cli,
    paths: Paths,
    format: Format,
    agent: &str,
    session: Option<&str>,
    thread_hook: bool,
    typed_as: Option<&str>,
) -> Result<()> {
    let Command::Prime { max_chars, include, timeout, .. } = &cli.command else {
        unreachable!("run for fwd prime alone")
    };
    let resolved = project_here(cli, &paths)?;
    let directory = directory_project(&paths);
    let client = match &resolved {
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
        let here = resolved.as_ref().map(|_| graph.record());
        let extent = extent(include, here);
        let mut holdings = working_set::read(graph, Holder::Agent(agent), &extent).await?;
        if let (true, Some(_)) = (holdings.is_empty(), here) {
            if let Some(Some(stand_in)) = thread_hook_stand_in(cli, graph, agent, thread_hook).await? {
                // A retired principal acts for no one, as every hook of it does.
                if graph.get::<PrincipalView>(stand_in).await?.active()? {
                    holdings = working_set::read(graph, Holder::Principal(stand_in), &extent).await?;
                }
            }
        }
        let scope = match (&cli.scope, &resolved) {
            (Some(named), _) => Some(runs_in::scope_named(graph, &[], named).await?),
            (None, Some(resolved)) => match scope_for_checkout(&client, resolved).await? {
                ScopeHere::One(scope) => Some(scope),
                ScopeHere::None | ScopeHere::Several { .. } => None,
            },
            (None, None) => None,
        };
        // What the agent is told beside it is its principal's in the project the command runs in.
        let notices = match holdings.iter().find(|h| Some(h.project.record) == here) {
            Some(h) => Notices::to(graph, h.principal, scope).await?,
            None => Notices::default(),
        };
        let project = resolved.as_ref().map(|r| (r.project.id, r.project.name.as_str()));
        let rest = prime_command(project, directory, typed_as, session);
        let primer = Primer::new(graph, format, holdings, Some(agent), directory, scope, client.port, rest).await?;
        anyhow::Ok((primer, notices))
    };
    let (primer, notices) = client.while_connected(read).await?;
    if primer.holdings.is_empty() && format == Format::Hook && timeout.is_none() {
        // As every hook of an agent that has not named itself prints nothing.
        return Ok(());
    }
    match primer.answer(*max_chars, &notices.lines()) {
        Answer::Text(text) => write_answer(&text)?,
        Answer::Json(value, cut) => {
            write_answer(&pretty(&value))?;
            cut.iter().for_each(|cut| say(cut));
        }
    }
    if format != Format::Hook {
        notices.lines().iter().for_each(|line| say(&format!("fwd: {line}")));
    }
    Ok(())
}

/// `fwd prime` run as the principal `app` acts as: the person, the principal `--as` names, or the
/// thread an MCP call comes from, which is an agent and is read as one.
pub(crate) async fn for_app(app: &App, max_chars: Option<usize>, include: &[Section]) -> Result<()> {
    let graph = app.graph();
    let holdings = working_set::read(graph, app.holder(), &extent(include, Some(graph.record()))).await?;
    let notices = Notices::to(graph, app.me, app.scope).await?;
    // An MCP call runs in the daemon, whose directory is no caller's.
    let directory = match app.out {
        Output::Captured(_) => None,
        Output::Stdout => directory_project(&app.paths),
    };
    let rest = prime_command(Some((app.project.id, &app.project.name)), directory, app.typed_as.as_deref(), None);
    let primer = Primer::new(graph, app.format, holdings, None, directory, app.scope, app.daemon_port, rest).await?;
    match (primer.answer(max_chars, &notices.lines()), &app.out) {
        (Answer::Text(text), _) => write_answer(&text)?,
        (Answer::Json(value, cut), Output::Captured(_)) => {
            app.print(value)?;
            cut.into_iter().for_each(|cut| app.note(cut));
        }
        (Answer::Json(value, cut), Output::Stdout) => {
            write_answer(&pretty(&value))?;
            cut.iter().for_each(|cut| say(cut));
        }
    }
    // The notice of a triage agent that serves nothing is said after every command
    // (`say_unbound`); the lead's guidance only here.
    if let (Some(guidance), false) = (notices.guidance, app.format == Format::Hook) {
        app.note(format!("fwd: {guidance}"));
    }
    Ok(())
}

/// What a working set is read for `include` (every section when it names none), with the ready
/// tasks of `here`, the project the command runs in, by its record.
fn extent(include: &[Section], here: Option<EntityId>) -> Extent {
    let selected = |kind: &SectionKind| {
        include.contains(&match kind {
            SectionKind::AskedOfYou | SectionKind::YouAsked => Section::Questions,
            SectionKind::Assigned => Section::Assigned,
            SectionKind::Unread => Section::Unread,
            SectionKind::Ready => Section::Ready,
        })
    };
    Extent {
        sections: SectionKind::ALL.into_iter().filter(|kind| include.is_empty() || selected(kind)).collect(),
        ready_in: here,
        ready: PRIMED_READY,
        unread: PRIMED_UNREAD,
    }
}

/// The project the directory the command runs in finds, without `--project`: where the commands
/// the text names run unless they name another. It only decides whether a command the text names
/// needs `--project`, so a directory that finds none, or not one alone, is none, and every command
/// is named with its project; it never fails the read.
fn directory_project(paths: &Paths) -> Option<Uuid> {
    let here = std::env::current_dir().ok()?;
    project::resolve(paths, &here, None).ok().map(|resolved| resolved.project.id)
}

/// The command that prints the questions and the assigned tasks whole: this one, without a budget,
/// reading the same working set, with `--project` whenever the project it runs in (`project`) is
/// not the one the directory finds, the `--as` its caller typed, and the session it named.
fn prime_command(
    project: Option<(Uuid, &str)>,
    directory: Option<Uuid>,
    typed_as: Option<&str>,
    session: Option<&str>,
) -> String {
    let mut words = vec!["fwd".to_string()];
    if let Some((_, name)) = project.filter(|(id, _)| directory != Some(*id)) {
        words.push(runs_in::project_flag(name));
    }
    if let Some(name) = typed_as {
        words.push(format!("--as '{}'", name.replace('\'', r"'\''")));
    }
    words.push("prime".to_string());
    if let Some(session) = session {
        words.push(format!("--session {session}"));
    }
    words.push("--format text".to_string());
    words.join(" ")
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

/// What `fwd prime` tells the principal of the project it runs in beside what it holds, as its
/// other commands tell it: a lead's guidance on making a triage agent (`lead_guidance`), and the
/// notice of a triage agent that serves nothing (`unbound_notice`).
#[derive(Default)]
struct Notices {
    guidance: Option<String>,
    unbound: Option<String>,
}

impl Notices {
    async fn to(graph: &ProjectGraph, principal: EntityId, scope: Option<EntityId>) -> Result<Notices> {
        Ok(Notices {
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
    /// The format it was read for, which its intentions are printed in.
    format: Format,
    /// The intentions assigned to the holdings' principals, each once: the first holding's first,
    /// each holding's in the order they were assigned.
    intentions: Vec<PrintedIntention>,
    /// The ids of the intentions assigned to them that could not be read, or of the assignments
    /// that could not be ([`intentions::assigned_to`]).
    unreadable_intentions: Vec<EntityId>,
    holdings: Vec<Holding>,
    /// The agent read for, which the line for an agent no principal is bound to names.
    agent: Option<String>,
    /// The project the directory the command runs in finds ([`directory_project`]): a command the
    /// text names for another project's records carries `--project`.
    directory: Option<Uuid>,
    /// The scope the command works in: the record of its project, and its title.
    scope: Option<(EntityId, String)>,
    /// The page's address, as `fwd ui --no-open` prints it without a project, for a panel that
    /// links to what it draws.
    page: String,
    /// The command that prints the questions and the assigned tasks whole ([`prime_command`]).
    rest_of_prime: String,
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
    /// What `fwd prime` prints in `format` of `holdings`, read through `graph`.
    #[allow(clippy::too_many_arguments)]
    async fn new(
        graph: &ProjectGraph,
        format: Format,
        holdings: Vec<Holding>,
        agent: Option<&str>,
        directory: Option<Uuid>,
        scope: Option<EntityId>,
        port: u16,
        rest_of_prime: String,
    ) -> Result<Primer> {
        let scope = match scope {
            Some(scope) => match graph.follow::<ScopeView>(scope).await? {
                Followed::Held { found, .. } => Some((found.project_id()?, found.title()?)),
                Followed::NotHeld => None,
            },
            None => None,
        };
        let (intentions, unreadable_intentions) = assigned_intentions(graph, format, &holdings, directory).await?;
        Ok(Primer {
            format,
            intentions,
            unreadable_intentions,
            holdings,
            agent: agent.map(str::to_string),
            directory,
            scope,
            page: format!("http://127.0.0.1:{port}/"),
            rest_of_prime,
        })
    }

    /// What to print, fitted to `max_chars`, or with `--format hook` to [`HOOK_CHARS`]; a hook's
    /// text ends with `notices` before its last lines, as other commands say them beside their
    /// answer. Which intentions keep their words is chosen first, against the budget with every
    /// other record left out, since every record is left out before any intention's words; the
    /// records then fill what room those words leave. Where leaving out every record still leaves
    /// it longer than the budget, its last line says so, with both numbers.
    fn answer(&self, max_chars: Option<usize>, notices: &[String]) -> Answer {
        let budget = max_chars.or((self.format == Format::Hook).then_some(HOOK_CHARS));
        // Counted as printed, its last newline with it.
        let size = |out: &str| out.chars().count() + 1;
        let none: Shown = self.holdings.iter().map(|h| vec![0; h.sections.len()]).collect();
        match self.format {
            Format::Pane => unreachable!("run_cli refuses --format pane for every command but reply"),
            Format::Json => {
                // The JSON holds the words of an intention since ended too.
                let most = self.intentions.len();
                let words = self.fit_words(budget, most, |words| size(&pretty(&self.json(&none, words))));
                let shown = self.fit(budget, |shown| size(&pretty(&self.json(shown, words))));
                let json = self.json(&shown, words);
                let over = budget.map(|b| (size(&pretty(&json)), b)).filter(|(length, b)| length > b);
                Answer::Json(json, self.last_line(&shown, words, over))
            }
            Format::Text | Format::Hook => {
                let notices = if self.format == Format::Hook { notices } else { &[] };
                let most = self.active();
                let words = self.fit_words(budget, most, |words| size(&self.text(&none, words, notices, None)));
                let shown = self.fit(budget, |shown| size(&self.text(shown, words, notices, None)));
                let text = match budget.filter(|b| size(&self.text(&shown, words, notices, None)) > *b) {
                    // The line states the length of the text that holds it, so its count is found
                    // by writing it until it states its own length.
                    Some(b) => {
                        let mut stated = 0;
                        loop {
                            let text = self.text(&shown, words, notices, Some((stated, b)));
                            if size(&text) == stated {
                                break text;
                            }
                            stated = size(&text);
                        }
                    }
                    None => self.text(&shown, words, notices, None),
                };
                Answer::Text(text)
            }
        }
    }

    /// How many of each section's records fit in `budget` characters as `measure` counts what they
    /// print: one record at a time is left out, from the end of the least important section with
    /// any left, the last holding's first, until the rest fits or nothing is left to leave out. The
    /// active intentions are in no section, and none is left out.
    fn fit(&self, budget: Option<usize>, measure: impl Fn(&Shown) -> usize) -> Shown {
        let mut shown: Shown =
            self.holdings.iter().map(|h| h.sections.iter().map(|s| s.records.len()).collect()).collect();
        let Some(budget) = budget else { return shown };
        // By kind, since holdings need not have the same sections: the ready tasks are read in one.
        let order: Vec<(usize, usize)> = SectionKind::ALL
            .iter()
            .rev()
            .flat_map(|kind| {
                self.holdings.iter().enumerate().rev().filter_map(move |(h, holding)| {
                    holding.sections.iter().position(|s| s.kind == *kind).map(|s| (h, s))
                })
            })
            .collect();
        while measure(&shown) > budget {
            let Some(&(h, s)) = order.iter().find(|(h, s)| shown[*h][*s] > 0) else { break };
            shown[h][s] -= 1;
        }
        shown
    }

    /// How many of the intentions keep their words, of the first `most` in the order they keep them
    /// ([`Primer::keeping`]), in `budget` characters as `measure` counts what prints with that
    /// many: all `most`, unless that is longer than the budget, when the last lose theirs first and
    /// every title stays.
    fn fit_words(&self, budget: Option<usize>, most: usize, measure: impl Fn(usize) -> usize) -> usize {
        let mut words = most;
        if let Some(budget) = budget {
            while words > 0 && measure(words) > budget {
                words -= 1;
            }
        }
        words
    }

    /// How many of the intentions are active: assigned and not ended.
    fn active(&self) -> usize {
        self.intentions.iter().filter(|i| i.active).count()
    }

    /// Each intention, in the order they print, with whether it keeps its words when `words` of
    /// them do: the active ones keep theirs first, in that order, then those since ended, whose
    /// words only the JSON holds, so that a budget leaves out an ended intention's words before an
    /// active one's.
    fn keeping(&self, words: usize) -> impl Iterator<Item = (&PrintedIntention, bool)> {
        let active = self.active();
        let (mut actives, mut ended) = (0, 0);
        self.intentions.iter().map(move |i| {
            let place = if i.active {
                actives += 1;
                actives - 1
            } else {
                ended += 1;
                active + ended - 1
            };
            (i, place < words)
        })
    }

    /// The text: the active intentions, the first `words` of them with their words
    /// ([`Primer::keeping`]); then for each holding, who the agent is there, then each section with
    /// records shown, one line each, a record of another project than its principal's naming that
    /// project, and what of it could not be read; then `notices`, how to go on, and the last line:
    /// what was left out, and, given `over` (the text's length and the budget), that it is longer
    /// than the budget.
    fn text(&self, shown: &Shown, words: usize, notices: &[String], over: Option<(usize, usize)>) -> String {
        if self.holdings.is_empty() {
            return [Some(self.unbound_line()), self.last_line(shown, words, over)]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("\n");
        }
        let mut lines = self.intention_lines(words);
        for (h, counts) in self.holdings.iter().zip(shown) {
            let scope = match &self.scope {
                Some((project, title)) if *project == h.project.record => format!(", in the scope {title}"),
                _ => String::new(),
            };
            lines.push(format!("fwdcentaur context for {} ({}) in {}{scope}.", h.handle, role_word(h), h.project.name));
            for (section, &n) in h.sections.iter().zip(counts) {
                if n > 0 {
                    lines.push(format!("{} ({}):", header(section.kind), section.total));
                    lines.extend(section.records[..n].iter().map(|r| record_line(h, section.kind, r)));
                }
                if let Some(first) = section.unreadable.iter().min() {
                    lines.push(format!(
                        "{} {} could not be read, the first {}",
                        section.unreadable.len(),
                        noun(section.kind),
                        short_id(first)
                    ));
                }
            }
        }
        lines.extend(notices.iter().cloned());
        lines.extend(self.next());
        lines.extend(self.last_line(shown, words, over));
        lines.join("\n")
    }

    /// The active intentions as the text gives them, before everything else: a heading that counts
    /// those not ended, then each intention, the first `words` of the active ones with their words,
    /// one since ended in one line; then what could not be read. Nothing when none is assigned.
    fn intention_lines(&self, words: usize) -> Vec<String> {
        let mut lines = Vec::new();
        if !self.intentions.is_empty() {
            lines.push(format!("Active Intentions ({}):", self.active()));
            for (i, keeps) in self.keeping(words) {
                let Printed::Text { lines: before, words: said } = &i.printed else {
                    unreachable!("a primer read for text prints its intentions as text")
                };
                lines.push(if keeps { format!("{before}{said}") } else { before.clone() });
            }
        }
        if let Some(first) = self.unreadable_intentions.iter().min() {
            let n = self.unreadable_intentions.len();
            lines.push(format!("{n} intentions assigned to you could not be read, the first {}", short_id(first)));
        }
        lines
    }

    /// The active intentions as the JSON gives them: each one, never left out, the first `words` of
    /// them with their words ([`Primer::keeping`]) and the others' words null.
    fn intentions_json(&self, words: usize) -> Value {
        let records: Vec<Value> = self
            .keeping(words)
            .map(|(i, keeps)| {
                let Printed::Json(record) = &i.printed else {
                    unreachable!("a primer read for JSON prints its intentions as JSON")
                };
                let mut record = record.clone();
                if !keeps {
                    record["words"] = Value::Null;
                }
                record
            })
            .collect();
        let unreadable = self
            .unreadable_intentions
            .iter()
            .min()
            .map(|first| json!({"count": self.unreadable_intentions.len(), "first": first.to_base64()}));
        json!({
            "section": "active_intentions",
            "shown": records.len(),
            "of": records.len(),
            "rest": (words < self.active()).then(|| self.rest_of_prime.clone()),
            "unreadable": unreadable,
            "records": records,
        })
    }

    /// How to go on from the sections included: taking work from the ready tasks, and reading the
    /// unread notifications; none when neither is included.
    fn next(&self) -> Option<String> {
        let has = |kind| self.holdings.iter().any(|h| h.sections.iter().any(|s| s.kind == kind));
        let ways: Vec<&str> = [
            (SectionKind::Ready, "`fwd task next --claim` to take work"),
            (SectionKind::Unread, "`fwd inbox pop` to read notifications"),
        ]
        .into_iter()
        .filter(|(kind, _)| has(*kind))
        .map(|(_, way)| way)
        .collect();
        (!ways.is_empty()).then(|| format!("Use {}.", ways.join(", ")))
    }

    /// The JSON a panel draws: the page's address; the section of the active intentions, the first
    /// `words` of them with their words; and for each principal its project, its name and its role
    /// and its sections, each with how many records it shows of how many, the command that prints
    /// the rest when some are left out, how many could not be read and the first of them, and its
    /// records shown, in the text's order, each with its path on the page and, when it is another
    /// than the principal's, its project.
    fn json(&self, shown: &Shown, words: usize) -> Value {
        if self.holdings.is_empty() {
            let fix = format!("{NAME_YOURSELF}, or {SUCCEED} when this session carries on another's work");
            return json!({"state": "unbound", "page": self.page, "fix": fix});
        }
        let principals: Vec<Value> = self
            .holdings
            .iter()
            .zip(shown)
            .map(|(h, counts)| {
                let sections: Vec<Value> = h
                    .sections
                    .iter()
                    .zip(counts)
                    .map(|(s, &n)| {
                        let records: Vec<Value> = s.records[..n].iter().map(|r| record_json(h, s.kind, r)).collect();
                        let unreadable = s
                            .unreadable
                            .iter()
                            .min()
                            .map(|first| json!({"count": s.unreadable.len(), "first": first.to_base64()}));
                        json!({
                            "section": s.kind.to_string(),
                            "shown": n,
                            "of": s.total,
                            "rest": (n < s.total).then(|| self.rest(h, s.kind, s.total + s.listed_elsewhere)),
                            "unreadable": unreadable,
                            "records": records,
                        })
                    })
                    .collect();
                json!({
                    "project": {"id": h.project.uuid.to_string(), "name": h.project.name},
                    "name": h.handle,
                    "role": role_word(h),
                    "sections": sections,
                })
            })
            .collect();
        json!({"state": "bound", "page": self.page, "sections": [self.intentions_json(words)], "principals": principals})
    }

    /// The one line for an agent no principal is bound to, which says how it comes to hold work.
    fn unbound_line(&self) -> String {
        let whom = self.agent.as_deref().map_or_else(|| "this agent".to_string(), subject::words);
        format!(
            "fwd: nothing to show: no principal is bound to {whom} in any project; name yourself with `{NAME_YOURSELF}`, or `{SUCCEED}` when this session carries on another's work"
        )
    }

    /// The last line: the active intentions whose words were left out, and each section with
    /// records left out, of each holding, in the text's order, the project named when the agent
    /// holds work in several, and the commands that print the rest, each named once; then, given
    /// `over` (a length and the budget), that the text is longer than its budget although every
    /// record that can be left out is. None when neither is so.
    fn last_line(&self, shown: &Shown, words: usize, over: Option<(usize, usize)>) -> Option<String> {
        let several = self.holdings.len() > 1;
        let mut cuts = Vec::new();
        if words < self.active() {
            let what = "active intentions with their words".to_string();
            cuts.push(Cut { what, shown: words, of: self.active(), command: self.rest_of_prime.clone() });
        }
        for (h, counts) in self.holdings.iter().zip(shown) {
            for (s, &n) in h.sections.iter().zip(counts).filter(|(s, n)| **n < s.total) {
                let what =
                    if several { format!("{} in {}", noun(s.kind), h.project.name) } else { noun(s.kind).into() };
                let command = self.rest(h, s.kind, s.total + s.listed_elsewhere);
                cuts.push(Cut { what, shown: n, of: s.total, command });
            }
        }
        let over = over.map(|(length, budget)| {
            format!("at {length} characters this is longer than the budget of {budget}, which leaves out records alone")
        });
        match (cut_line(&cuts), over) {
            (Some(cut), Some(over)) => Some(format!("{cut}; {over}")),
            (Some(cut), None) => Some(cut),
            (None, Some(over)) => Some(format!("fwd: {over}")),
            (None, None) => None,
        }
    }

    /// The command that prints the records of the section `kind` of `h`, of which a whole list
    /// holds `whole`, for the agent the text is for, whose own commands are stamped: the questions
    /// and the assigned tasks by this command without the budget that left them out, wherever they
    /// are; the notifications and the ready tasks by the command that lists them in their project,
    /// with `--project` when that is not the one the directory finds, the task list given a limit of
    /// every task it lists, since its own default would cut a long one again.
    fn rest(&self, h: &Holding, kind: SectionKind, whole: usize) -> String {
        let project = if self.directory == Some(h.project.uuid) {
            String::new()
        } else {
            format!(" {}", runs_in::project_flag(&h.project.name))
        };
        match kind {
            SectionKind::AskedOfYou | SectionKind::YouAsked | SectionKind::Assigned => self.rest_of_prime.clone(),
            SectionKind::Unread => format!("fwd{project} inbox --unread"),
            SectionKind::Ready => format!("fwd{project} task list --ready --limit {whole}"),
        }
    }
}

/// An intention assigned to one of the agent's principals, as prime prints it in the format it was
/// read for, rendered once from its views.
struct PrintedIntention {
    /// Whether it is active, neither fulfilled nor withdrawn, as its status read when it was
    /// rendered, so that the heading's count and what it prints agree.
    active: bool,
    printed: Printed,
}

/// What an intention prints, in the format prime was read for.
enum Printed {
    /// Its lines, then its words, which a budget leaves out last of all; for one since ended, the
    /// one line saying so, and no words.
    Text { lines: String, words: String },
    /// Its record, as `fwd intention show` gives it with a record's path and project, its words
    /// among its fields.
    Json(Value),
}

impl PrintedIntention {
    /// The intention `record` assigned to `h`'s principal, read in `there`, its principal's
    /// project's graph, as one of `project`, as it prints in `format`. In the text, its short id
    /// and title, its project when that is not the principal's, then whose words its description
    /// holds and the day they were written down, when a quotation names its speaker, then the
    /// description; for one since ended, that it is still assigned and the command that takes it
    /// off, run from `directory`'s project. In the JSON, its record.
    async fn of(
        there: &ProjectGraph,
        format: Format,
        h: &Holding,
        directory: Option<Uuid>,
        project: &Whose,
        record: &(PlacementView, Parts),
    ) -> Result<PrintedIntention> {
        let (placement, (intention, _)) = record;
        let ended = intentions::outcome(there.component_of::<ClosedView>(placement).await?.as_ref())?;
        let active = ended.is_none();
        if format == Format::Json {
            let mut json = crate::intentions::intention_json(there, record).await?;
            json["path"] = Value::Null;
            json["project"] = match project {
                Whose::Other(p) => json!({"id": p.uuid.to_string(), "name": p.name}),
                Whose::This => Value::Null,
            };
            return Ok(PrintedIntention { active, printed: Printed::Json(json) });
        }
        let short = short_id(&placement.id());
        let title = intention.title()?.split_whitespace().collect::<Vec<_>>().join(" ");
        let elsewhere = match project {
            Whose::Other(p) => format!(" in {}", p.name),
            Whose::This => String::new(),
        };
        let printed = if active {
            let spoken = match crate::intentions::spoken(there, record).await? {
                Some((speaker, day, _)) => format!("\n  @{speaker}, {day}"),
                None => String::new(),
            };
            let lines = format!("{short} {title}{elsewhere}{spoken}");
            Printed::Text { lines, words: crate::intentions::words_after(&intention.description()?) }
        } else {
            let flag = if directory == Some(h.project.uuid) {
                String::new()
            } else {
                format!(" {}", runs_in::project_flag(&h.project.name))
            };
            let status = intentions::status(ended);
            let lines = format!(
                "{short} {title}{elsewhere} [{status}]: still assigned to you; `fwd{flag} intention unassign {short}` takes it off"
            );
            Printed::Text { lines, words: String::new() }
        };
        Ok(PrintedIntention { active, printed })
    }
}

/// The intentions assigned to the principals of `holdings`, each once, as prime prints them in
/// `format`: the first holding's first and each holding's in the order they were assigned, each
/// named with its project when that is not its principal's; and the ids of those that could not be
/// read, which, as every record prime cannot read, are not listed but counted.
async fn assigned_intentions(
    graph: &ProjectGraph,
    format: Format,
    holdings: &[Holding],
    directory: Option<Uuid>,
) -> Result<(Vec<PrintedIntention>, Vec<EntityId>)> {
    let mut listed = HashSet::new();
    let (mut printed, mut unreadable) = (Vec::new(), Vec::new());
    for h in holdings {
        let there = graph.graph_of_project(&h.project);
        for (id, assigned) in intentions::assigned_to(&there, h.principal).await? {
            if !listed.insert(id) {
                continue;
            }
            match assigned {
                Followed::Held { project, found, .. } => {
                    match PrintedIntention::of(&there, format, h, directory, &project, &found).await {
                        Ok(intention) => printed.push(intention),
                        Err(_) => unreadable.push(id),
                    }
                }
                Followed::NotHeld => unreadable.push(id),
            }
        }
    }
    Ok((printed, unreadable))
}

/// A record of `h`'s section `kind` as one line: its short id, the form the page shows and every
/// command takes, its title ([`primed_title`]), and the project it is in when that is not `h`'s.
fn record_line(h: &Holding, kind: SectionKind, r: &Record) -> String {
    let elsewhere =
        if r.project.record == h.project.record { String::new() } else { format!(" in {}", r.project.name) };
    format!("{} {}{elsewhere}", short_id(&r.id), primed_title(kind, r))
}

/// A record of `h`'s section `kind` as the JSON gives it: its whole id, its kind, its title as its
/// line has it ([`primed_title`]), its path on the page (null where the page has no route of its own
/// for it), and the project it is in when that is not `h`'s (null otherwise).
fn record_json(h: &Holding, kind: SectionKind, r: &Record) -> Value {
    let project = (r.project.record != h.project.record)
        .then(|| json!({"id": r.project.uuid.to_string(), "name": r.project.name}));
    json!({
        "id": r.id.to_base64(),
        "kind": r.kind.to_string(),
        "title": primed_title(kind, r),
        "path": r.route.as_ref().map(|route| page_path(r.project.uuid, route)),
        "project": project,
    })
}

/// What a record's line says of it after its id, in the section `kind`: a question asked of the
/// agent by whom it is from, one it asked by whom it asks, each before its first words; a task
/// assigned to it with its status in brackets, and a ready task by its title alone; a notification
/// by its sentence. Its whitespace is collapsed once it is composed, so that a name or a text that
/// holds newlines, or no text at all, makes one line with no space left at its end.
fn primed_title(kind: SectionKind, r: &Record) -> String {
    let line = match &r.kind {
        RecordKind::Ask(Asking::Of { from }) => format!("from @{}: {}", from.handle, r.title),
        RecordKind::Ask(Asking::By { to: Addressee::Principal(to) }) => format!("to @{}: {}", to.handle, r.title),
        RecordKind::Ask(Asking::By { to: Addressee::Name(name) }) => format!("to @{name}: {}", r.title),
        RecordKind::Ask(Asking::By { to: Addressee::NoOne }) => format!("to no one in particular: {}", r.title),
        RecordKind::Task { status: Some(status) } if kind == SectionKind::Assigned => format!("{} [{status}]", r.title),
        RecordKind::Task { .. } | RecordKind::Notification => r.title.clone(),
    };
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `value` as `--format json` prints it.
fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("a JSON value always serializes: its keys are strings")
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

/// What the last line says when records were left out, as `fwd task list` says when its limit cut
/// it: how many of how many of each section it shows, and the commands that print the rest, each
/// named once; none when nothing was left out.
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

/// Whether `fwd prime`'s answer has been written, held while it is written, so that the line a
/// deadline prints comes neither inside an answer nor after one.
static ANSWERED: Mutex<bool> = Mutex::new(false);

/// Writes `text`, `fwd prime`'s answer, on standard output.
fn write_answer(text: &str) -> Result<()> {
    let mut answered = ANSWERED.lock().unwrap_or_else(PoisonError::into_inner);
    *answered = true;
    let mut out = std::io::stdout().lock();
    writeln!(out, "{text}").and_then(|()| out.flush()).context("writing the answer on standard output")
}

/// Says `line` on standard error; a line that cannot be written has no reader left, and costs the
/// command nothing.
fn say(line: &str) {
    let _ = writeln!(std::io::stderr(), "{line}");
}

/// The deadline `fwd prime --timeout` gives, and whether its one line is JSON: read from the
/// command line before the command runs, so that it bounds all of it.
pub(crate) struct Deadline {
    within: Duration,
    json: bool,
}

pub(crate) fn deadline() -> Option<Deadline> {
    let cli = Cli::try_parse().ok()?;
    let Command::Prime { timeout: Some(seconds), .. } = cli.command else { return None };
    Some(Deadline { within: Duration::from_secs(seconds), json: format_of(&cli) == Format::Json })
}

/// Runs `ran`, a `fwd prime` given `--timeout`, and ends the process with exit 0 whatever becomes of
/// it, so that a hook or a skill that runs it neither fails nor waits past the deadline. A thread of
/// its own ends the process at the deadline, whatever the command is doing then, reading its
/// standard input or a checkout's git files included, which no future's timeout could interrupt,
/// and writing its answer too: an answer being written then is not waited for. An error, and the
/// deadline passing first, are said in one line, as JSON with `--format json`, unless the answer
/// was written. The process ends at once, without shutting the runtime down, which can wait for
/// good on a thread left inside Ankurah (`watch::end` says how).
pub(crate) async fn within(deadline: Deadline, ran: impl Future<Output = Result<()>>) -> ! {
    let Deadline { within, json } = deadline;
    std::thread::spawn(move || {
        std::thread::sleep(within);
        let answered = match ANSWERED.try_lock() {
            Ok(answered) => Some(answered),
            Err(TryLockError::Poisoned(answered)) => Some(answered.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        };
        if let Some(mut answered) = answered {
            nothing_to_show(&mut answered, json, &format!("it did not finish within {}s", within.as_secs()));
        }
        std::process::exit(0)
    });
    if let Err(error) = ran.await {
        let mut answered = ANSWERED.lock().unwrap_or_else(PoisonError::into_inner);
        nothing_to_show(&mut answered, json, &format!("{error:#}"));
    }
    std::process::exit(0)
}

/// Says in one line why there is nothing to show, unless the answer was written (`answered`); a
/// line that cannot be written has no reader left.
fn nothing_to_show(answered: &mut bool, json: bool, why: &str) {
    if *answered {
        return;
    }
    *answered = true;
    let why = why.split_whitespace().collect::<Vec<_>>().join(" ");
    let line = if json {
        json!({"state": "unavailable", "reason": why}).to_string()
    } else {
        format!("fwd: nothing to show: {why}")
    };
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}").and_then(|()| out.flush());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fwd_core::project_graph::ProjectRecord;
    use fwd_core::route::Route;
    use fwd_core::working_set::Section;

    fn project(n: u8) -> ProjectRecord {
        let name = ["alpha", "beta"][usize::from(n)].to_string();
        ProjectRecord { record: EntityId::from_bytes([100 + n; 32]), uuid: Uuid::from_bytes([n + 1; 16]), name }
    }

    /// A holding in `project` with a section of each `kinds`, each holding the first of its pair of
    /// records of the second.
    fn holding(project_n: u8, sections: &[(SectionKind, usize, usize)]) -> Holding {
        let sections = sections
            .iter()
            .map(|&(kind, records, total)| Section {
                kind,
                total,
                listed_elsewhere: 0,
                unreadable: Vec::new(),
                records: (0..records)
                    .map(|i| Record {
                        id: EntityId::from_bytes([i as u8; 32]),
                        kind: RecordKind::Task { status: Some(TaskStatus::Open) },
                        title: format!("{kind} {i}"),
                        text: None,
                        created: None,
                        project: project(project_n),
                        route: Some(Route::task(EntityId::from_bytes([i as u8; 32]))),
                    })
                    .collect(),
            })
            .collect();
        Holding {
            project: project(project_n),
            principal: EntityId::from_bytes([200; 32]),
            handle: "impl-a".into(),
            kind: PrincipalKind::Agent,
            role: Some(AgentRole::Implementer),
            subject: None,
            sections,
        }
    }

    /// A primer of `holdings`, read for the text, run in the first one's project's directory.
    fn primer(holdings: Vec<Holding>) -> Primer {
        let directory = holdings.first().map(|h| h.project.uuid);
        Primer {
            format: Format::Text,
            intentions: Vec::new(),
            unreadable_intentions: Vec::new(),
            holdings,
            agent: None,
            directory,
            scope: None,
            page: "http://127.0.0.1:1/".into(),
            rest_of_prime: "fwd prime --format text".into(),
        }
    }

    use SectionKind::*;

    /// Over its budget, `fwd prime` leaves out one record at a time from the least important end:
    /// the ready tasks first, then every holding's unread notifications, the last holding's before
    /// the first's, then the assigned tasks, then the questions, though the holdings do not have the
    /// same sections; and its last line names each section it cut, of how many, and the commands
    /// that print the rest, with `--project` for a project the directory does not find.
    #[test]
    fn a_budget_leaves_out_the_least_important_records_first_and_the_last_line_says_which() {
        let p = primer(vec![
            holding(0, &[(AskedOfYou, 1, 1), (YouAsked, 1, 1), (Assigned, 1, 1), (Unread, 1, 1), (Ready, 2, 2)]),
            holding(1, &[(AskedOfYou, 0, 0), (YouAsked, 0, 0), (Assigned, 0, 0), (Unread, 2, 2)]),
        ]);
        // A measure that counts the records shown, so that a budget of n keeps n of them.
        let records = |shown: &Shown| shown.iter().flatten().sum::<usize>();
        let kept: [(Vec<usize>, Vec<usize>); 9] = [
            (vec![1, 1, 1, 1, 2], vec![0, 0, 0, 2]),
            (vec![1, 1, 1, 1, 1], vec![0, 0, 0, 2]),
            (vec![1, 1, 1, 1, 0], vec![0, 0, 0, 2]),
            (vec![1, 1, 1, 1, 0], vec![0, 0, 0, 1]),
            (vec![1, 1, 1, 1, 0], vec![0, 0, 0, 0]),
            (vec![1, 1, 1, 0, 0], vec![0, 0, 0, 0]),
            (vec![1, 1, 0, 0, 0], vec![0, 0, 0, 0]),
            (vec![1, 0, 0, 0, 0], vec![0, 0, 0, 0]),
            (vec![0, 0, 0, 0, 0], vec![0, 0, 0, 0]),
        ];
        for (budget, (a, b)) in (0..=8).rev().zip(kept.clone()) {
            assert_eq!(p.fit(Some(budget), records), [a, b], "a budget of {budget} records");
        }
        let text = p.text(&vec![kept[3].0.clone(), kept[3].1.clone()], 0, &[], None);
        assert_eq!(
            text.lines().last(),
            Some(
                "fwd: 0 of 2 ready tasks in alpha and 1 of 2 unread notifications in beta shown; `fwd task list --ready --limit 2` and `fwd --project 'beta' inbox --unread` list the rest"
            ),
            "{text}"
        );
    }

    /// The last line names each section cut and each command that prints the rest once, the task
    /// list given a limit of every task it lists, the agent's own ready tasks listed as assigned
    /// among them; a text that left nothing out ends with how to go on, and with neither the ready
    /// tasks nor the notifications included, with its last record.
    #[test]
    fn the_last_line_names_each_section_cut_and_each_command_once() {
        let p = primer(vec![holding(0, &[(Unread, 20, 20), (Ready, 10, 10)])]);
        let text = p.text(&p.fit(None, |_| 0), 0, &[], None);
        assert_eq!(
            text.lines().last(),
            Some("Use `fwd task next --claim` to take work, `fwd inbox pop` to read notifications.")
        );
        let mut held = primer(vec![holding(0, &[(AskedOfYou, 1, 1), (Assigned, 1, 1)])]);
        let text = held.text(&held.fit(None, |_| 0), 0, &[], None);
        assert!(text.lines().last().is_some_and(|line| line.ends_with("assigned 0 [open]")), "{text}");

        held.holdings = vec![holding(0, &[(AskedOfYou, 1, 1), (YouAsked, 1, 1), (Unread, 20, 31), (Ready, 10, 60)])];
        held.holdings[0].sections[3].listed_elsewhere = 2;
        let text = held.text(&vec![vec![0, 0, 20, 10]], 0, &[], None);
        assert_eq!(
            text.lines().last(),
            Some("fwd: 0 of 1 questions asked of you, 0 of 1 questions you asked, 20 of 31 unread notifications and 10 of 60 ready tasks shown; `fwd prime --format text`, `fwd inbox --unread` and `fwd task list --ready --limit 62` list the rest")
        );
    }

    /// At the hook's budget the text, counted in characters as Claude Code keeps them, fits in
    /// 10,000, its last line saying what was left out; where leaving out every record is not
    /// enough, the last line says by how much, the length it states being the text's own.
    #[test]
    fn the_hook_fits_its_budget_and_says_when_leaving_out_records_is_not_enough() {
        let mut p = Primer { format: Format::Hook, ..primer(vec![holding(0, &[(Assigned, 400, 400)])]) };
        for r in &mut p.holdings[0].sections[0].records {
            r.title = format!("{} {}", r.title, "é".repeat(40));
        }
        let Answer::Text(text) = p.answer(None, &[]) else { panic!("text") };
        assert!(text.chars().count() < HOOK_CHARS, "{} characters", text.chars().count());
        assert!(text
            .lines()
            .last()
            .is_some_and(|line| line.starts_with("fwd: ") && line.contains("of 400 tasks assigned to you shown")));

        p.format = Format::Text;
        let Answer::Text(text) = p.answer(Some(30), &[]) else { panic!("text") };
        let length = text.chars().count() + 1;
        assert!(
            text.lines().last().is_some_and(|line| line.ends_with(&format!(
                "; at {length} characters this is longer than the budget of 30, which leaves out records alone"
            ))),
            "{text}"
        );

        // The one line for an agent no principal is bound to says so too.
        let unbound = Primer { holdings: Vec::new(), agent: Some("claude-code:s1/main".into()), ..primer(Vec::new()) };
        let Answer::Text(text) = unbound.answer(Some(30), &[]) else { panic!("text") };
        let length = text.chars().count() + 1;
        assert!(text.starts_with("fwd: nothing to show: no principal is bound to"), "{text}");
        assert_eq!(
            text.lines().last(),
            Some(
                format!(
                    "fwd: at {length} characters this is longer than the budget of 30, which leaves out records alone"
                )
                .as_str()
            )
        );
    }

    /// The `n`th intention assigned to the agent, with `words`, active or since fulfilled, as it
    /// prints in `format`: in the text, one since fulfilled is its one line.
    fn assigned(n: u8, words: &str, active: bool, format: Format) -> PrintedIntention {
        let id = EntityId::from_bytes([n; 32]);
        let printed = match (format, active) {
            (Format::Json, _) => Printed::Json(
                json!({"id": id.to_base64(), "kind": "intention", "title": format!("intention {n}"), "words": words}),
            ),
            (_, true) => Printed::Text {
                lines: format!("{} intention {n}\n  @ines, 2026-11-14", short_id(&id)),
                words: crate::intentions::words_after(words),
            },
            (_, false) => {
                Printed::Text { lines: format!("{} intention {n} [fulfilled]", short_id(&id)), words: String::new() }
            }
        };
        PrintedIntention { active, printed }
    }

    /// `primer` read for `format`, holding `intentions`, each as it prints in `format`.
    fn with_intentions(p: Primer, format: Format, intentions: &[(u8, &str, bool)]) -> Primer {
        let intentions = intentions.iter().map(|&(n, words, active)| assigned(n, words, active, format)).collect();
        Primer { format, intentions, ..p }
    }

    /// Over its budget prime leaves out every other record before an intention's words, then the
    /// last intentions' words first, and never a title; the last line says how many kept theirs
    /// and names the command that prints them, and the JSON gives the others' words as null.
    #[test]
    fn a_budget_leaves_out_the_last_intentions_words_after_every_record_and_never_a_title() {
        let words: Vec<String> = (1..=3).map(|n| format!("{n}{}", " word".repeat(60))).collect();
        let three: Vec<(u8, &str, bool)> = (1..=3).map(|n| (n, words[usize::from(n) - 1].as_str(), true)).collect();
        let read_for = |format| with_intentions(primer(vec![holding(0, &[(Unread, 2, 2)])]), format, &three);
        let cut = "fwd: 2 of 3 active intentions with their words and 0 of 2 unread notifications shown; `fwd prime --format text` and `fwd inbox --unread` list the rest";

        let p = read_for(Format::Text);
        let budget = p.text(&vec![vec![0]], 2, &[], None).chars().count() + 1;
        let Answer::Text(text) = p.answer(Some(budget), &[]) else { panic!("text") };
        for n in 1..=3 {
            assert!(text.contains(&format!("intention {n}\n")), "every title stays: {text}");
        }
        assert!(
            text.contains("\n    1 word") && text.contains("\n    2 word") && !text.contains("\n    3 word"),
            "{text}"
        );
        assert_eq!(text.lines().last(), Some(cut));

        let p = read_for(Format::Json);
        let budget = pretty(&p.json(&vec![vec![0]], 2)).chars().count() + 1;
        let Answer::Json(json, said) = p.answer(Some(budget), &[]) else { panic!("JSON") };
        let section = &json["sections"][0];
        assert_eq!(
            (&section["section"], &section["rest"]),
            (&json!("active_intentions"), &json!("fwd prime --format text"))
        );
        let words: Vec<bool> = (0..3).map(|i| section["records"][i]["words"].is_null()).collect();
        assert_eq!(words, [false, false, true]);
        assert_eq!((&json["principals"][0]["sections"][0]["shown"], said.as_deref()), (&json!(0), Some(cut)));
    }

    /// Which intentions keep their words is chosen first, and the records fill the room the words
    /// left out leave: at a budget that holds the first intention's words and both notifications,
    /// but not the second's words, prime prints the first's words and both notifications, in the
    /// text and in the JSON.
    #[test]
    fn the_records_fill_the_room_the_words_left_out_leave() {
        let words: Vec<String> = (1..=2).map(|n| format!("{n}{}", " word".repeat(200))).collect();
        let two = [(1, words[0].as_str(), true), (2, words[1].as_str(), true)];
        let read_for = |format| {
            let mut p = with_intentions(primer(vec![holding(0, &[(Unread, 2, 2)])]), format, &two);
            for r in &mut p.holdings[0].sections[0].records {
                r.title = format!("{} {}", r.title, "on the retry ".repeat(4));
            }
            p
        };

        let p = read_for(Format::Text);
        let expected = p.text(&vec![vec![2]], 1, &[], None);
        let Answer::Text(text) = p.answer(Some(expected.chars().count() + 1), &[]) else { panic!("text") };
        assert_eq!(text, expected);

        let p = read_for(Format::Json);
        let expected = p.json(&vec![vec![2]], 1);
        let Answer::Json(json, _) = p.answer(Some(pretty(&expected).chars().count() + 1), &[]) else { panic!("JSON") };
        assert_eq!(json, expected);
    }

    /// The JSON holds the words of an intention since ended too, and a budget leaves those out
    /// before any active intention's: with a fulfilled intention whose words alone pass the budget,
    /// the active one keeps its words, the fulfilled one's are null, and the JSON fits; the text,
    /// which gives an ended intention one line, keeps the active one's words too.
    #[test]
    fn an_ended_intentions_words_are_left_out_before_an_active_ones() {
        let fulfilled = "fulfilled ".repeat(2_000);
        let both = [(1, fulfilled.as_str(), false), (2, "the list stays", true)];
        let read_for = |format| with_intentions(primer(vec![holding(0, &[])]), format, &both);

        let Answer::Json(json, said) = read_for(Format::Json).answer(Some(10_000), &[]) else { panic!("JSON") };
        let records = &json["sections"][0]["records"];
        assert_eq!((&records[0]["words"], &records[1]["words"]), (&Value::Null, &json!("the list stays")));
        assert!(pretty(&json).chars().count() < 10_000, "{} characters", pretty(&json).chars().count());
        assert_eq!(said, None, "no active intention's words are left out");

        let Answer::Text(text) = read_for(Format::Text).answer(Some(10_000), &[]) else { panic!("text") };
        assert!(text.contains("intention 2\n  @ines, 2026-11-14:\n    the list stays\n"), "{text}");
    }

    /// A record of another project than its principal's names that project on its line and in the
    /// JSON, which gives its path in that project; what could not be read is said by count and by
    /// the least of its ids, so that the same set always names the same one.
    #[test]
    fn a_record_of_another_project_names_it_and_what_could_not_be_read_is_said() {
        let mut p = primer(vec![holding(0, &[(Assigned, 1, 1)])]);
        let moved = &mut p.holdings[0].sections[0];
        moved.records[0].project = project(1);
        moved.unreadable = vec![EntityId::from_bytes([9; 32]), EntityId::from_bytes([8; 32])];
        let shown = p.fit(None, |_| 0);
        let text = p.text(&shown, 0, &[], None);
        let id = EntityId::from_bytes([0; 32]);
        assert!(text.contains(&format!("{} assigned 0 [open] in beta\n", short_id(&id))), "{text}");
        assert!(
            text.contains(&format!(
                "2 tasks assigned to you could not be read, the first {}",
                short_id(&EntityId::from_bytes([8; 32]))
            )),
            "{text}"
        );
        let json = p.json(&shown, 0);
        let section = &json["principals"][0]["sections"][0];
        assert_eq!(section["records"][0]["project"]["name"], "beta");
        assert_eq!(section["records"][0]["path"], format!("?p={}#/tasks/{}", project(1).uuid, id.to_base64()));
        assert_eq!(section["unreadable"], json!({"count": 2, "first": EntityId::from_bytes([8; 32]).to_base64()}));
    }
}
