//! `fwd session view`: the person's view of a Claude Code session, for a pane beside the chat, as
//! one JSON document on one line (`fwd_core::session_view` makes it), and with `--follow` again
//! whenever it changes ([`follow`]). It reads every project of the node, so it runs from any
//! directory and joins the daemon through any project it hosts, and it reads for the person,
//! whatever agent runs it.

mod follow;

use crate::{autostart, load_identity, Cli, Format};
use anyhow::{bail, Context as _, Result};
use fwd_core::daemon::{self, Autostart, Client};
use fwd_core::project::{NamedProject, Paths};
use fwd_core::session_view::{self, Document, Held};
use fwd_core::subject;
use std::io::Write;

/// Prints the view of the session `session` once, or with `follow` for as long as the process
/// runs, which then ends only by itself ([`follow::run`]).
pub(crate) async fn print(cli: &Cli, paths: Paths, session: &str, follow: bool) -> Result<()> {
    let viewing = Viewing::of(cli, &paths, session)?;
    let autostart = autostart(cli)?;
    if follow {
        let Err(error) = follow::run(viewing, paths, autostart).await;
        crate::fail(error, 1)
    }
    let client = join(&paths, &autostart).await?;
    let held = client.while_connected(viewing.read(&client)).await?;
    write_line(&line_of(&held.document(&page(&client))))
}

/// Whose view is read, and for whom.
struct Viewing {
    session: String,
    /// The name of the person the view is for: what waits on them, and as whom its actions act.
    person: String,
}

impl Viewing {
    /// The view the command line asks for: a Claude Code session's id, which is refused when it is
    /// none, read for the principal --as or FWD_AS names, else for the person this machine's
    /// identity names. The view is JSON alone, so another `--format` is refused rather than
    /// ignored.
    fn of(cli: &Cli, paths: &Paths, session: &str) -> Result<Viewing> {
        subject::claude_code(session, subject::MAIN)?;
        if let Some(format) = cli.format.filter(|format| *format != Format::Json) {
            bail!("`fwd session view` prints JSON alone, for a pane to draw; `--format {format:?}` is refused: leave it off");
        }
        let person = match &cli.act_as {
            Some(name) => name.clone(),
            None => load_identity(paths)?.handle,
        };
        Ok(Viewing { session: session.to_string(), person })
    }

    /// What the view is drawn from, as the daemon `client` joined has it now.
    async fn read(&self, client: &Client) -> Result<Held> {
        session_view::read(&client.graph, &self.session, &self.person).await
    }
}

/// The page's address on the daemon `client` joined, as `fwd ui --no-open` prints it without a
/// project.
fn page(client: &Client) -> String {
    format!("http://127.0.0.1:{}/", client.port)
}

/// Joins the daemon, started first when `autostart` says so, through the first project it hosts:
/// the view reads every project, so it is the same whichever it joins through.
async fn join(paths: &Paths, autostart: &Autostart) -> Result<Client> {
    let running = daemon::ensure_running(paths, autostart).await?;
    let hosted = running.projects.into_iter().find(|p| p.error.is_none()).context("the daemon hosts no project")?;
    daemon::ensure_daemon(paths, &NamedProject { id: hosted.id, name: hosted.name }, autostart.clone()).await
}

/// Writes `line` and a newline on standard output, flushed, so that a reader that reads line by
/// line has it whole at once.
fn write_line(line: &str) -> Result<()> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}").and_then(|()| out.flush()).context("writing the view on standard output")
}

/// The document as the view prints it: one line, whatever its text holds.
fn line_of(document: &Document) -> String {
    serde_json::to_string(document).expect("the view's document always serializes: its keys are strings")
}
