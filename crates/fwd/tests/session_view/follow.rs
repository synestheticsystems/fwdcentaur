//! `fwd session view --follow`: the document again whenever what it shows changes, an empty line
//! while the stream is healthy, the daemon lost and found again, a read that fails tried again,
//! and the ends: another build, and a reader gone. The tests read the stream line by line, each line
//! within a deadline of the test's own, so that what they wait for is ordered by the stream itself,
//! never by a sleep. One limit: a first document is no barrier for the first results of every live
//! read the stream opens, so a late first result can read the view again after a change a test
//! made and stand in for a live read that did not hear it; each test that follows a part of the
//! view changes it alone, by a writer whose writes reach no other live read.

use super::*;
use std::io::{BufRead, BufReader, Read};
use std::net::TcpListener;
use std::process::{Child, ExitStatus};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

/// A running `fwd session view --follow`, or a shell that runs one, its standard output read line
/// by line by a thread of its own. It is killed when the test ends.
struct Stream {
    child: Child,
    lines: Receiver<String>,
}

impl Stream {
    fn start(s: &Scratch) -> Result<Stream> {
        Stream::of(s.command(s.dir.path(), None, &["session", "view", SESSION, "--follow"]))
    }

    fn of(mut command: Command) -> Result<Stream> {
        let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        let out = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let (sent, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                if sent.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Stream { child, lines })
    }

    /// The next line the stream prints, within `limit`.
    fn line(&self, limit: Duration) -> Result<String> {
        self.lines.recv_timeout(limit).map_err(|why| anyhow!("no line within {}s: {why}", limit.as_secs()))
    }

    /// The next document the stream prints, within `limit`, past any empty line.
    fn document(&self, limit: Duration) -> Result<Value> {
        let until = std::time::Instant::now() + limit;
        loop {
            let left = until.saturating_duration_since(std::time::Instant::now());
            let line = self.line(left)?;
            if !line.is_empty() {
                return Ok(serde_json::from_str(&line)?);
            }
        }
    }

    /// The lines left, once standard output closes within `limit`.
    fn rest(&self, limit: Duration) -> Result<Vec<String>> {
        let until = std::time::Instant::now() + limit;
        let mut rest = Vec::new();
        loop {
            match self.lines.recv_timeout(until.saturating_duration_since(std::time::Instant::now())) {
                Ok(line) => rest.push(line),
                Err(RecvTimeoutError::Disconnected) => return Ok(rest),
                Err(RecvTimeoutError::Timeout) => {
                    return Err(anyhow!("the stream had not ended {}s on: {rest:?}", limit.as_secs()))
                }
            }
        }
    }

    /// The stream's exit, once its standard output closes within `limit`, the lines it printed
    /// before, and what it said on standard error.
    fn exit(mut self, limit: Duration) -> Result<(ExitStatus, Vec<String>, String)> {
        let rest = self.rest(limit)?;
        let status = self.child.wait()?;
        let mut said = String::new();
        if let Some(mut err) = self.child.stderr.take() {
            err.read_to_string(&mut said)?;
        }
        Ok((status, rest, said))
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A copy of this build of fwd that says it is another, its build's name changed by one character
/// wherever the binary holds it, and signed again where the system asks a binary to be; with this
/// build's name and the copy's.
fn another_build(s: &Scratch) -> Result<(PathBuf, String, String)> {
    let version = String::from_utf8(s.command(s.dir.path(), None, &["--version"]).output()?.stdout)?;
    let ours = version.trim().strip_prefix("fwd ").ok_or_else(|| anyhow!("no build in {version:?}"))?.to_string();
    let mut theirs = ours.clone();
    let last = theirs.pop().ok_or_else(|| anyhow!("an empty build"))?;
    theirs.push(if last == '0' { '1' } else { '0' });
    let mut bytes = std::fs::read(env!("CARGO_BIN_EXE_fwd"))?;
    let mut found = 0;
    let mut at = 0;
    while let Some(start) = bytes[at..].windows(ours.len()).position(|w| w == ours.as_bytes()) {
        let start = at + start;
        bytes[start..start + ours.len()].copy_from_slice(theirs.as_bytes());
        found += 1;
        at = start + ours.len();
    }
    assert!(found > 0, "the binary holds its build's name");
    let path = s.dir.path().join("fwd-another-build");
    std::fs::write(&path, bytes)?;
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
    if cfg!(target_os = "macos") {
        let signed = Command::new("codesign").args(["--force", "--sign", "-"]).arg(&path).output()?;
        assert!(signed.status.success(), "{}", String::from_utf8_lossy(&signed.stderr));
    }
    Ok((path, ours, theirs))
}

/// The port the scratch daemon listens on.
fn port(s: &Scratch) -> Result<u64> {
    let status = s.command(s.dir.path(), None, &["daemon", "status"]).output()?;
    let status: Value = serde_json::from_slice(&status.stdout)?;
    status["port"].as_u64().ok_or_else(|| anyhow!("no port in {status}"))
}

const SOON: Duration = Duration::from_secs(30);

/// The stream prints the view at once, and again when what it shows changes, from another
/// command's write, with no timer in the path: a change that leaves the view as it was prints
/// nothing, so the next document after one is the next that differs; and the person's answer,
/// pressed from the view, takes the question out of what waits on them.
#[test]
fn the_view_is_printed_again_when_what_it_shows_changes_and_only_then() -> Result<()> {
    let s = Scratch::new()?;
    s.json(&s.alpha, Some("main"), &["principal", "add", "lead-sam", "--role", "lead"])?;
    let stream = Stream::start(&s)?;
    let first = stream.document(SOON)?;
    assert_eq!(first["principals"][0]["title"], "lead-sam", "{first}");
    assert_eq!(list(&first["principals"][0], "waits_on_you"), None);

    // The agent's comment on a task no one holds stamps it as having written, which the view's
    // read of the session's principals hears, but changes nothing the view shows.
    let elsewhere = s.made(&s.alpha, None, &["task", "create", "Name the page"])?;
    s.made(&s.alpha, Some("main"), &["comment", &elsewhere, "Not mine to do."])?;
    let question = s.made(&s.alpha, Some("main"), &["ask", "--to", "tester", "Should the primer name the page?"])?;
    let asked = stream.document(SOON)?;
    assert_eq!(titles(&asked["principals"][0], "waits_on_you"), ["Should the primer name the page?"], "{asked}");

    let row = &list(&asked["principals"][0], "waits_on_you").expect("waits on you")["records"][0];
    assert_eq!(row["id"], json!(question));
    let args: Vec<String> = serde_json::from_value(row["actions"][0]["args"].clone())?;
    let pressed = s.press(&args, "No: the pane links there.")?;
    assert!(pressed.status.success(), "{}", String::from_utf8_lossy(&pressed.stderr));
    let answered = stream.document(SOON)?;
    assert_eq!(list(&answered["principals"][0], "waits_on_you"), None, "{answered}");
    Ok(())
}

/// Each part of what the view is made from is followed, each change here reaching the stream
/// through one live read alone, the person making it, whose writes stamp no principal of the
/// session: the title of a task assigned to the agent (the tasks assigned to it), a comment on it
/// (its unread notifications), the text of a question asked of it (the questions asked of it), a
/// question it asked, resolved (the questions it asked); then a subagent naming itself (the
/// session's principals), and a project made after the stream started, where the agent then names
/// itself (the projects, then that project's principals of the session).
#[test]
fn each_part_of_what_the_view_is_made_from_is_followed() -> Result<()> {
    let s = Scratch::new()?;
    s.json(&s.alpha, Some("main"), &["principal", "add", "lead-sam", "--role", "lead"])?;
    let task = s.made(&s.alpha, None, &["task", "create", "Retry on disconnect"])?;
    s.json(&s.alpha, None, &["task", "assign", &task, "lead-sam"])?;
    let asked_of = s.made(&s.alpha, None, &["ask", "--to", "lead-sam", "How long a wait?"])?;
    let asked = s.made(&s.alpha, Some("main"), &["ask", "--to", "tester", "Should the primer name the page?"])?;
    // The assignment and the person's question, so that the stream starts after routing is done.
    s.told(&s.alpha, "main", 2)?;
    let stream = Stream::start(&s)?;
    let first = stream.document(SOON)?;
    assert_eq!(titles(&first["principals"][0], "holds"), ["How long a wait?", "Retry on disconnect"], "{first}");

    s.json(&s.alpha, None, &["task", "amend", &task, "--title", "Retry once on disconnect"])?;
    let renamed = stream.document(SOON)?;
    assert_eq!(
        titles(&renamed["principals"][0], "holds"),
        ["How long a wait?", "Retry once on disconnect"],
        "{renamed}"
    );

    s.made(&s.alpha, None, &["comment", &task, "Cap the wait at ten seconds."])?;
    let told = stream.document(SOON)?;
    assert_eq!(told["principals"][0]["tags"][2]["text"], "3 unread", "{told}");

    s.json(&s.alpha, None, &["message", "edit", &asked_of, "How long a wait at most?"])?;
    let edited = stream.document(SOON)?;
    assert_eq!(
        titles(&edited["principals"][0], "holds"),
        ["How long a wait at most?", "Retry once on disconnect"],
        "{edited}"
    );

    s.json(&s.alpha, None, &["message", "resolve", &asked])?;
    let resolved = stream.document(SOON)?;
    assert_eq!(list(&resolved["principals"][0], "waits_on_you"), None, "{resolved}");

    s.json(&s.alpha, Some("a1"), &["principal", "add", "impl-sam", "--role", "implementer"])?;
    let named = stream.document(SOON)?;
    let groups = |document: &Value| -> Vec<String> {
        document["principals"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|g| format!("{} {}", g["title"], g["tags"][1]["text"]))
            .collect()
    };
    assert_eq!(groups(&named), [r#""lead-sam" "alpha""#, r#""impl-sam" "alpha""#], "{named}");

    let gamma = s.dir.path().join("gamma");
    std::fs::create_dir_all(&gamma)?;
    git(&gamma, &["init", "-q", "-b", "main"])?;
    git(&gamma, &["remote", "add", "origin", "git@example.com:t/gamma.git"])?;
    s.json(&gamma, None, &["init", "--name", "gamma", "--handle", "tester"])?;
    s.json(&gamma, Some("main"), &["principal", "add", "lead-sam", "--role", "lead"])?;
    let joined = stream.document(SOON)?;
    assert_eq!(
        groups(&joined),
        [r#""lead-sam" "alpha""#, r#""lead-sam" "gamma""#, r#""impl-sam" "alpha""#],
        "{joined}"
    );
    Ok(())
}

/// While the stream is healthy and nothing changes, an empty line comes every 30 seconds. With
/// the daemon stopped, the last document comes again saying the daemon is out of reach, with no
/// page and the principals it showed; with the daemon started again, the fresh document follows.
#[test]
fn the_stream_beats_while_healthy_and_follows_the_daemon_out_of_reach_and_back() -> Result<()> {
    let s = Scratch::new()?;
    s.json(&s.alpha, Some("main"), &["principal", "add", "lead-sam", "--role", "lead"])?;
    let task = s.made(&s.alpha, None, &["task", "create", "Retry on disconnect"])?;
    s.json(&s.alpha, None, &["task", "assign", &task, "lead-sam"])?;
    let stream = Stream::start(&s)?;
    let first = stream.document(SOON)?;
    assert_eq!(titles(&first["principals"][0], "holds"), ["Retry on disconnect"], "{first}");
    assert_eq!(stream.line(Duration::from_secs(45))?, "", "the heartbeat, and nothing else");

    stop(&s)?;
    let lost = stream.document(SOON)?;
    assert_eq!(lost["page"], Value::Null, "{lost}");
    assert_eq!(
        lost["lines"],
        json!([
            "fwd's daemon is out of reach; this is what it last said, and it will show again when the daemon answers."
        ])
    );
    assert_eq!(lost["principals"], first["principals"]);

    let started = s.command(s.dir.path(), None, &["daemon", "start"]).output()?;
    assert!(started.status.success(), "{}", String::from_utf8_lossy(&started.stderr));
    let back = stream.document(Duration::from_secs(60))?;
    assert!(back["page"].as_str().is_some_and(|page| page.starts_with("http://127.0.0.1:")), "{back}");
    assert_eq!((&back["lines"], &back["principals"]), (&json!([]), &first["principals"]));
    Ok(())
}

/// A daemon that answers again, after it was out of reach, running another build than the stream
/// ends the stream, with exit 0 and a line saying why, so that whatever runs it starts the build
/// now on the machine.
#[test]
fn a_daemon_that_answers_running_another_build_ends_the_stream() -> Result<()> {
    let s = Scratch::new()?;
    s.json(&s.alpha, Some("main"), &["principal", "add", "lead-sam", "--role", "lead"])?;
    let stream = Stream::start(&s)?;
    stream.document(SOON)?;
    stop(&s)?;
    stream.document(SOON)?;

    // A daemon of another build: its port file, and a port that answers its health check and its
    // project list as that daemon, and nothing else.
    let other = TcpListener::bind("127.0.0.1:0")?;
    let port = other.local_addr()?.port();
    let instance = "another-build";
    let health = json!({"ok": true, "pid": std::process::id(), "instance_id": instance, "build": "0.0.1-1.gfeedbee"});
    std::thread::spawn(move || {
        for stream in other.incoming().flatten() {
            let mut request = [0; 1024];
            let mut stream = stream;
            let read = stream.read(&mut request).unwrap_or(0);
            let path = String::from_utf8_lossy(&request[..read]).split_whitespace().nth(1).unwrap_or("").to_string();
            let (status, body) = match path.as_str() {
                "/health" => ("200 OK", health.to_string()),
                "/projects" => ("200 OK", "[]".to_string()),
                _ => ("404 Not Found", String::new()),
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    let port_file =
        json!({"port": port, "pid": std::process::id(), "instance_id": instance, "started": 0, "bind": "loopback"});
    std::fs::write(s.dir.path().join("run").join("daemon.ws"), port_file.to_string())?;

    let (status, _, said) = stream.exit(Duration::from_secs(60))?;
    assert_eq!(status.code(), Some(0), "{said}");
    assert!(said.contains("the daemon now runs build 0.0.1-1.gfeedbee"), "{said}");
    std::fs::remove_file(s.dir.path().join("run").join("daemon.ws"))?;
    Ok(())
}

/// The first join checks the build: a stream of another build than the daemon's ends at once, with
/// exit 0 and the line saying so, having printed nothing.
#[test]
fn a_stream_of_another_build_than_the_daemons_ends_at_its_first_join() -> Result<()> {
    let s = Scratch::new()?;
    let (other, ours, theirs) = another_build(&s)?;
    let stream = Stream::of(s.command_of(&other, s.dir.path(), None, &["session", "view", SESSION, "--follow"]))?;
    let (status, printed, said) = stream.exit(SOON)?;
    assert_eq!((status.code(), printed.len()), (Some(0), 0), "{said}");
    assert_eq!(
        said,
        format!("fwd: the daemon now runs build {ours}, not this view's {theirs}; start `fwd session view --follow` again\n")
    );
    Ok(())
}

/// The common case of an install: the daemon restarts on the port it had, as another build, and
/// the socket comes back before the daemon counts as lost. The stream ends then, with exit 0 and the
/// line saying so, having printed no document saying the daemon was out of reach.
#[test]
fn a_daemon_restarted_on_its_port_as_another_build_ends_the_stream() -> Result<()> {
    let s = Scratch::new()?;
    s.json(&s.alpha, Some("main"), &["principal", "add", "lead-sam", "--role", "lead"])?;
    let (other, ours, theirs) = another_build(&s)?;
    stop(&s)?;
    let started = s.command_of(&other, s.dir.path(), None, &["daemon", "start"]).output()?;
    assert!(started.status.success(), "{}", String::from_utf8_lossy(&started.stderr));
    let before = port(&s)?;
    let stream = Stream::of(s.command_of(&other, s.dir.path(), None, &["session", "view", SESSION, "--follow"]))?;
    stream.document(SOON)?;
    stop(&s)?;
    let restarted = s.command(s.dir.path(), None, &["daemon", "start"]).output()?;
    assert!(restarted.status.success(), "{}", String::from_utf8_lossy(&restarted.stderr));
    assert_eq!(port(&s)?, before, "the daemon came back on its port");
    let (status, printed, said) = stream.exit(SOON)?;
    assert_eq!(status.code(), Some(0), "{said}");
    assert!(said.contains(&format!("fwd: the daemon now runs build {ours}, not this view's {theirs}")), "{said}");
    assert!(printed.iter().all(|line| !line.contains("out of reach")), "{printed:?}");
    Ok(())
}

/// A reader gone ends the stream quietly, with exit 0 and nothing on standard error, at its next
/// write: a document after a change, or an empty line at the heartbeat.
#[test]
fn a_reader_gone_ends_the_stream_quietly_at_its_next_write() -> Result<()> {
    let s = Scratch::new()?;
    s.json(&s.alpha, Some("main"), &["principal", "add", "lead-sam", "--role", "lead"])?;
    let script = r#""$0" session view "$1" --follow 2> "$2" | head -n 1; echo "exit ${PIPESTATUS[0]}""#;
    let piped = |n: usize| -> Result<(Stream, PathBuf)> {
        let said = s.dir.path().join(format!("said-{n}"));
        let words = ["-c", script, env!("CARGO_BIN_EXE_fwd"), SESSION, said.to_str().unwrap_or_default()];
        Ok((Stream::of(s.command_of(Path::new("bash"), s.dir.path(), None, &words))?, said))
    };
    // A document: the reader takes the first and is gone, and a change makes another.
    let (document, said) = piped(0)?;
    document.document(SOON)?;
    s.made(&s.alpha, Some("main"), &["ask", "--to", "tester", "Should the primer name the page?"])?;
    assert_eq!(document.line(SOON)?, "exit 0");
    assert_eq!(std::fs::read_to_string(&said)?, "");
    // An empty line: nothing changes, and the heartbeat finds no reader.
    let (beat, said) = piped(1)?;
    beat.document(SOON)?;
    assert_eq!(beat.line(Duration::from_secs(45))?, "exit 0");
    assert_eq!(std::fs::read_to_string(&said)?, "");
    Ok(())
}

/// A stream whose parent process is gone while the daemon is out of reach, so that it has nothing
/// to write, ends at its next attempt to join the daemon again, closing what it wrote to.
#[test]
fn a_stream_whose_parent_is_gone_during_an_outage_ends() -> Result<()> {
    let s = Scratch::new()?;
    s.json(&s.alpha, Some("main"), &["principal", "add", "lead-sam", "--role", "lead"])?;
    let script = r#""$0" session view "$1" --follow 2> /dev/null & wait"#;
    let words = ["-c", script, env!("CARGO_BIN_EXE_fwd"), SESSION];
    let mut parent = Stream::of(s.command_of(Path::new("bash"), s.dir.path(), None, &words))?;
    parent.document(SOON)?;
    stop(&s)?;
    while !parent.document(SOON)?["page"].is_null() {}
    parent.child.kill()?;
    parent.child.wait()?;
    // The stream alone holds the pipe now: its end is the pipe's.
    parent.rest(Duration::from_secs(60))?;
    Ok(())
}

/// A read that fails with the daemon in reach is said, with no id, and tried again after waits that
/// double, with no empty line while it fails; mended, the view comes again. Here a project's record
/// that cannot be read fails it.
#[tokio::test]
async fn a_read_that_fails_is_tried_again_and_the_stream_is_not_healthy_meanwhile() -> Result<()> {
    let s = Scratch::new()?;
    s.json(&s.alpha, Some("main"), &["principal", "add", "lead-sam", "--role", "lead"])?;
    let client = s.client(&s.alpha).await?;
    let ctx = client.node.context_async(ankurah::policy::DEFAULT_CONTEXT).await?;
    let trx = ctx.begin();
    let broken = trx.create(&fwd_core::model::Project { uuid: "not a uuid".into(), name: "broken".into() }).await?.id();
    trx.commit().await?;
    let stream = Stream::start(&s)?;
    let failed = stream.document(SOON)?;
    assert_eq!(
        failed["lines"],
        json!(["fwd could not read what this session holds: a field of a record it reads could not be read. This is what it last read; it tries again."])
    );
    assert!(stream.line(Duration::from_secs(35)).is_err(), "no empty line, and no other document, while it fails");
    let record: fwd_core::model::ProjectView = ctx.get(broken).await?;
    let trx = ctx.begin();
    record.edit(&trx)?.uuid()?.replace(&uuid::Uuid::new_v4().to_string())?;
    trx.commit().await?;
    let mended = stream.document(SOON)?;
    assert_eq!(mended["principals"][0]["title"], "lead-sam", "{mended}");
    Ok(())
}

/// Stops the scratch daemon, as a person does to install a new build.
fn stop(s: &Scratch) -> Result<()> {
    let stopped = s.command(s.dir.path(), None, &["daemon", "stop"]).output()?;
    match String::from_utf8(stopped.stdout)?.trim() {
        "stopped" => Ok(()),
        said => Err(anyhow!("the daemon did not stop: {said} {}", String::from_utf8_lossy(&stopped.stderr))),
    }
}
