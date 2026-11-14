//! `fwd session view --follow`: the document again whenever what it shows changes, an empty line
//! while the stream is healthy, the daemon lost and found again, and the end when the daemon runs
//! another build. The tests read the stream line by line, each line within a deadline of the
//! test's own, so that what they wait for is ordered by the stream itself, never by a sleep.

use super::*;
use std::io::{BufRead, BufReader, Read};
use std::net::TcpListener;
use std::process::{Child, ExitStatus};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

/// A running `fwd session view --follow`, its standard output read line by line by a thread of its
/// own. It is killed when the test ends.
struct Stream {
    child: Child,
    lines: Receiver<String>,
}

impl Stream {
    fn start(s: &Scratch) -> Result<Stream> {
        let mut command = s.command(s.dir.path(), None, &["session", "view", SESSION, "--follow"]);
        let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
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

    /// The stream's exit, once its standard output closes within `limit`, and what it said on
    /// standard error.
    fn exit(mut self, limit: Duration) -> Result<(ExitStatus, String)> {
        let until = std::time::Instant::now() + limit;
        loop {
            match self.lines.recv_timeout(until.saturating_duration_since(std::time::Instant::now())) {
                Ok(_) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(anyhow!("the stream had not ended {}s on", limit.as_secs()))
                }
            }
        }
        let status = self.child.wait()?;
        let mut said = String::new();
        if let Some(mut err) = self.child.stderr.take() {
            err.read_to_string(&mut said)?;
        }
        Ok((status, said))
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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

    // An agent of no session naming itself changes the project's principals, which the view reads,
    // but not the view.
    s.json(&s.alpha, None, &["--as", "impl-elsewhere", "principal", "add", "impl-elsewhere", "--role", "implementer"])?;
    let question = s.made(&s.alpha, Some("main"), &["ask", "--to", "tester", "Should the primer name the page?"])?;
    let asked = stream.document(SOON)?;
    assert_eq!(titles(&asked["principals"][0], "waits_on_you"), ["Should the primer name the page?"], "{asked}");
    assert_eq!(asked["principals"].as_array().map(Vec::len), Some(1), "the other agent is not this session's");

    let row = &list(&asked["principals"][0], "waits_on_you").expect("waits on you")["records"][0];
    assert_eq!(row["id"], json!(question));
    let args: Vec<String> = serde_json::from_value(row["actions"][0]["args"].clone())?;
    let pressed = s.press(&args, "No: the pane links there.")?;
    assert!(pressed.status.success(), "{}", String::from_utf8_lossy(&pressed.stderr));
    let answered = stream.document(SOON)?;
    assert_eq!(list(&answered["principals"][0], "waits_on_you"), None, "{answered}");
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

/// A daemon that answers again running another build than the stream ends the stream, with exit 0
/// and a line saying why, so that whatever runs it starts the build now on the machine.
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

    let (status, said) = stream.exit(Duration::from_secs(60))?;
    assert_eq!(status.code(), Some(0), "{said}");
    assert!(said.contains("the daemon now runs build 0.0.1-1.gfeedbee"), "{said}");
    std::fs::remove_file(s.dir.path().join("run").join("daemon.ws"))?;
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
