//! `fwd session view`: the person's view of a Claude Code session, for a pane beside the chat, as
//! one JSON document on one line (`fwd_core::session_view` makes it), and with `--follow` again
//! whenever it changes ([`follow`]). It reads every project of the node, so it runs from any
//! directory and joins the daemon through any project it hosts, and it reads for the person,
//! whatever agent runs it.

mod follow;

pub(crate) use follow::Ended;

use crate::{autostart, Cli, Format, Identity};
use anyhow::{bail, Context as _, Result};
use fwd_core::daemon::{self, Autostart, Client};
use fwd_core::project::{NamedProject, Paths};
use fwd_core::session_view::{self, Document, Held, Reader};
use fwd_core::subject;
use std::io::Write;

/// Prints the view of the session `session` once, or with `follow` for as long as the process
/// runs, which then ends only by itself ([`follow::run`], [`Ended`]).
pub(crate) async fn print(cli: &Cli, paths: Paths, session: &str, follow: bool) -> Result<()> {
    let viewing = Viewing::of(cli, &paths, session)?;
    let autostart = autostart(cli)?;
    if follow {
        let Err(ended) = follow::run(viewing, paths, autostart).await;
        return Err(ended);
    }
    let (client, _) = join(&paths, &autostart).await?;
    let held = client.while_connected(viewing.read(&client)).await?;
    let line = line_of(&held.document(&page(&client)));
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}").and_then(|()| out.flush()).context("writing the view on standard output")
}

/// Whose view is read, and for whom.
struct Viewing {
    session: String,
    /// Who the view is for, as the command names them.
    reader: Named,
}

/// Who the view is for, as the command names them ([`Reader`]).
enum Named {
    Identity(String),
    Typed(String),
}

impl Viewing {
    /// The view the command line asks for: a Claude Code session's id, which is refused when it is
    /// none, read for the principal --as or FWD_AS names, else for the person this machine's
    /// identity names ([`identity`]). The view is JSON alone, so another `--format` is refused
    /// rather than ignored.
    fn of(cli: &Cli, paths: &Paths, session: &str) -> Result<Viewing> {
        subject::claude_code(session, subject::MAIN)?;
        if let Some(format) = cli.format.filter(|format| *format != Format::Json) {
            bail!("`fwd session view` prints JSON alone, for a pane to draw; `--format {format:?}` is refused: leave it off");
        }
        let reader = match &cli.act_as {
            Some(typed) => Named::Typed(typed.clone()),
            None => Named::Identity(identity(paths)?),
        };
        Ok(Viewing { session: session.to_string(), reader })
    }

    /// What the view is drawn from, as the daemon `client` joined has it now.
    async fn read(&self, client: &Client) -> Result<Held> {
        let reader = match &self.reader {
            Named::Identity(handle) => Reader::Identity(handle),
            Named::Typed(typed) => Reader::Named(typed),
        };
        session_view::read(&client.graph, &self.session, reader).await
    }
}

/// The name this machine's identity file gives the person, read strictly: a file that is not there
/// or cannot be read is an error naming it, never the login name in its place, which could be
/// another person's.
fn identity(paths: &Paths) -> Result<String> {
    let path = paths.data_dir.join("identity.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => bail!(
            "`fwd session view` reads the person it is for from {}, which does not exist: `fwd init --handle <your name>` records it, or `--as <name>` names the person",
            path.display()
        ),
        Err(error) => return Err(error).with_context(|| format!("reading the person's identity from {}", path.display())),
    };
    let identity: Identity =
        toml::from_str(&text).with_context(|| format!("reading the person's identity from {}", path.display()))?;
    Ok(identity.handle)
}

/// The page's address on the daemon `client` joined, as `fwd ui --no-open` prints it without a
/// project.
fn page(client: &Client) -> String {
    format!("http://127.0.0.1:{}/", client.port)
}

/// Joins the daemon, started first when `autostart` says so, through the first project it hosts,
/// with the build it says it runs: the view reads every project, so it is the same whichever it
/// joins through.
async fn join(paths: &Paths, autostart: &Autostart) -> Result<(Client, Option<String>)> {
    let running = daemon::ensure_running(paths, autostart).await?;
    let hosted = running.projects.into_iter().find(|p| p.error.is_none()).context("the daemon hosts no project")?;
    let project = NamedProject { id: hosted.id, name: hosted.name };
    Ok((daemon::ensure_daemon(paths, &project, autostart.clone()).await?, running.build))
}

/// The document as the view prints it: one line, whatever its text holds.
fn line_of(document: &Document) -> String {
    serde_json::to_string(document).expect("the view's document always serializes: its keys are strings")
}
