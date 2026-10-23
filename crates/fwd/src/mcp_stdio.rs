//! `fwd mcp stdio`: the daemon's MCP endpoint (`/mcp`, crates/fwd/src/mcp.rs) offered over standard
//! input and output, for a host that starts its MCP servers as processes. The Codex plugin
//! (integrations/codex) starts it so that a Codex thread reaches the daemon without a URL in its
//! configuration: the daemon listens on whatever port it got at its last start, which only its port
//! file knows, and a URL fixed at install would stop working at the next restart that moved it.
//!
//! It is a relay and nothing more. Each message the host writes, one JSON-RPC message per line, goes
//! to the daemon in a POST of its own, as the host wrote it, byte for byte: the caller's identity
//! travels in the request's `params._meta` (the Codex thread, the root of its session tree and the
//! turn's metadata, which the host sets and the model cannot), and the daemon reads it there, so
//! the relay never parses, rewrites or drops it. The tools, their arguments, their errors and who
//! the caller is are all the daemon's; nothing here knows a tool's name.
//!
//! The endpoint is stateless and answers each POST with one JSON body, so that is all the relay
//! reads: a body framed by `Content-Length`, or by the connection closing, within fixed bounds. An
//! event stream, a chunked body or a session id would mean an endpoint this relay was not written
//! for, and the call is answered with an error that says so rather than read half-way.
//!
//! Each request the host sends gets exactly one answer under its id: the daemon's, or, when the
//! daemon gave none (it could not be reached, it closed the connection, it accepted the request
//! without answering it, or it answered something else), an error from the relay. A notification,
//! or the host's answer to a request, is owed nothing, and a failure to deliver one goes to
//! standard error, as do the relay's other logs, since standard output carries only the protocol.
//!
//! Before each message the relay finds the daemon as every CLI command does
//! (`fwd_core::daemon::ensure_running`): the port file names the daemon, which must answer its
//! `/health` as the instance the file names, and is started when none answers unless
//! `--no-autostart` is given. So a daemon that restarted, on its old port or another, costs the
//! host nothing, and a port the daemon left, which another program or a scratch daemon took since,
//! is never written to. A message goes again only when the connection to the daemon could not be
//! made, so that not a byte of it went out; one whose connection broke after that is answered with
//! an error saying that it may have been made, as a CLI command that lost its daemon while writing
//! says (`daemon::Lost`), since sending a write twice could make it twice.
//!
//! When standard input ends, the relay waits a few seconds for the calls still in flight, then
//! leaves them: the daemon finishes or abandons each on its own side either way.

use anyhow::{anyhow, bail, Context as _, Result};
use fwd_core::daemon::{self, Autostart};
use fwd_core::project::Paths;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

/// How long a POST waits for the daemon to accept its connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long the relay waits for the calls in flight once standard input has ended.
const GRACE: Duration = Duration::from_secs(5);
/// The longest status or header line the relay reads, and the most header lines.
const MAX_LINE: usize = 8 * 1024;
const MAX_HEADERS: usize = 64;
/// The largest body the relay reads.
const MAX_BODY: usize = 64 * 1024 * 1024;
/// The endpoint's path on the daemon.
const ENDPOINT: &str = "/mcp";
/// The JSON-RPC error code of a message the relay could not deliver or whose answer it lost.
const RELAY_ERROR: i64 = -32603;

/// Relays standard input to the daemon's `/mcp` and its answers to standard output until standard
/// input ends, then waits, for a while, for the messages still in flight.
pub async fn run(paths: Paths, autostart: Autostart) -> Result<()> {
    let (out, writing) = writer(tokio::io::stdout());
    let relay = Arc::new(Relay { daemon: Daemon::Machine { paths, autostart }, out, grace: GRACE });
    relay.serve(BufReader::new(tokio::io::stdin())).await?;
    drop(relay);
    writing.await.context("writing to standard output")?
}

/// Where the relay finds the daemon's port.
enum Daemon {
    /// This machine's daemon, found, checked and started as the CLI does.
    Machine { paths: Paths, autostart: Autostart },
    /// The tests' ports, one per lookup, the last one repeated.
    #[cfg(test)]
    Ports(std::sync::Mutex<std::collections::VecDeque<u16>>),
}

impl Daemon {
    async fn port(&self) -> Result<u16> {
        match self {
            Daemon::Machine { paths, autostart } => Ok(daemon::ensure_running(paths, autostart).await?.pf.port),
            #[cfg(test)]
            Daemon::Ports(ports) => {
                let mut ports = ports.lock().unwrap();
                Ok(if ports.len() > 1 { ports.pop_front().unwrap() } else { ports[0] })
            }
        }
    }
}

struct Relay {
    daemon: Daemon,
    out: mpsc::UnboundedSender<Vec<u8>>,
    grace: Duration,
}

/// The daemon's answer to one POST.
#[derive(Debug)]
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// Why a POST got no reply.
enum Failed {
    /// No connection to the daemon: not a byte of the message went out, so it can go again.
    Unsent(anyhow::Error),
    /// Something went wrong after the connection was made: the daemon may have acted on it.
    Unanswered(anyhow::Error),
}

impl Relay {
    /// Reads the host's lines until standard input ends, each relayed on a task of its own, so a
    /// slow tool call holds up no other.
    async fn serve(self: &Arc<Self>, mut input: impl AsyncBufRead + Unpin) -> Result<()> {
        let mut in_flight = JoinSet::new();
        let mut line = Vec::new();
        loop {
            line.clear();
            if input.read_until(b'\n', &mut line).await.context("reading standard input")? == 0 {
                break;
            }
            // The line as the host wrote it, but for the newline that frames it.
            let end = line.iter().rposition(|b| !matches!(b, b'\n' | b'\r')).map_or(0, |i| i + 1);
            if line[..end].iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let relay = self.clone();
            let message = line[..end].to_vec();
            in_flight.spawn(async move { relay.relay(message).await });
            while in_flight.try_join_next().is_some() {}
        }
        let drained = async { while in_flight.join_next().await.is_some() {} };
        if tokio::time::timeout(self.grace, drained).await.is_err() {
            eprintln!(
                "fwd mcp stdio: the host closed its input with {} call(s) still in flight; leaving them to the daemon",
                in_flight.len()
            );
            in_flight.shutdown().await;
        }
        Ok(())
    }

    /// Delivers one message. A request gets exactly one line under its id; anything else gets
    /// nothing, its failures going to standard error.
    async fn relay(&self, message: Vec<u8>) {
        let id = match serde_json::from_slice::<Value>(&message) {
            // A request: a method and an id. JSON-RPC answers a line it cannot parse with a null id.
            Ok(value) => match (value.get("method"), value.get("id")) {
                (Some(_), Some(id)) => Some(id.clone()),
                _ => None,
            },
            Err(e) => {
                self.emit(&error_answer(&Value::Null, -32700, &format!("a line from the host is not JSON: {e}")));
                return;
            }
        };
        let delivered = self.deliver(&message).await;
        match id {
            Some(id) => {
                let answer = delivered.and_then(|reply| answer_to(&id, &reply));
                self.emit(&answer.unwrap_or_else(|e| error_answer(&id, RELAY_ERROR, &format!("{e:#}"))));
            }
            None => {
                if let Err(e) = delivered.and_then(|reply| accepted(&reply)) {
                    eprintln!("fwd mcp stdio: {e:#}");
                }
            }
        }
    }

    /// POSTs the message to the daemon the port file names, found again, once, when its port took
    /// no connection, which means that nothing was sent.
    async fn deliver(&self, message: &[u8]) -> Result<Reply> {
        let port = self.daemon.port().await.context("finding the fwd daemon")?;
        let unsent = match post(port, message).await {
            Ok(reply) => return Ok(reply),
            Err(Failed::Unanswered(e)) => return Err(e.context(MAY_HAVE_BEEN_MADE)),
            Err(Failed::Unsent(e)) => e,
        };
        tracing::debug!(port, error = %unsent, "the daemon took no connection; finding it again");
        let port = self.daemon.port().await.context("finding the fwd daemon again")?;
        match post(port, message).await {
            Ok(reply) => Ok(reply),
            Err(Failed::Unanswered(e)) => Err(e.context(MAY_HAVE_BEEN_MADE)),
            Err(Failed::Unsent(e)) => Err(e.context("the fwd daemon could not be reached")),
        }
    }

    fn emit(&self, message: &Value) {
        // Serialized again so that a body the daemon split over lines is one line here; the
        // receiving end is gone only when standard output is, which `run` reports.
        let _ = self.out.send(serde_json::to_vec(message).expect("a JSON value serializes"));
    }
}

const MAY_HAVE_BEEN_MADE: &str =
    "the connection to the fwd daemon broke after this call was sent, so it may have been made; look before making it again";

/// The daemon's answer to the request with this id, from its reply: the message carrying that id,
/// or the reason there is none.
fn answer_to(id: &Value, reply: &Reply) -> Result<Value> {
    if reply.status == 404 {
        bail!(
            "the running fwd daemon has no MCP endpoint, so it predates it: restart it with a build that has one (`fwd daemon stop`, then any fwd command)"
        );
    }
    let body = json_body(reply)?;
    let Some(body) = body else {
        bail!("the fwd daemon accepted this request ({}) but sent no answer to it", reply.status);
    };
    let messages = match body {
        Value::Array(batch) => batch,
        one => vec![one],
    };
    let mut refusal = None;
    for message in messages {
        if message.get("id") == Some(id) && (message.get("result").is_some() || message.get("error").is_some()) {
            return Ok(message);
        }
        if message.get("id") == Some(&Value::Null) {
            refusal = message.pointer("/error/message").and_then(Value::as_str).map(str::to_string);
        }
    }
    match refusal {
        Some(why) => bail!("the fwd daemon refused the call: {why}"),
        None => bail!("the fwd daemon's answer ({}) carried no answer to this request", reply.status),
    }
}

/// Succeeds when the daemon took a notification or the host's answer to a request.
fn accepted(reply: &Reply) -> Result<()> {
    match reply.status {
        200..=299 => Ok(()),
        status => bail!("the fwd daemon refused a notification ({status}): {}", first_line(&reply.body)),
    }
}

/// The reply's JSON body, none when it is empty; an event stream, or a body that is not JSON, is
/// an error naming it.
fn json_body(reply: &Reply) -> Result<Option<Value>> {
    if reply.header("content-type").is_some_and(|t| t.trim_start().starts_with("text/event-stream")) {
        bail!("the fwd daemon answered with an event stream, which this relay does not read; update fwd");
    }
    if reply.body.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    match serde_json::from_slice(&reply.body) {
        Ok(value) => Ok(Some(value)),
        Err(_) if !(200..=299).contains(&reply.status) => {
            bail!("the fwd daemon refused the call ({}): {}", reply.status, first_line(&reply.body))
        }
        Err(e) => bail!("the fwd daemon's answer is not JSON: {e}"),
    }
}

fn error_answer(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).lines().next().unwrap_or("").trim().chars().take(200).collect()
}

/// The single writer of standard output: each message a line, flushed at once, in the order the
/// relay's tasks finish.
fn writer(
    mut output: impl AsyncWrite + Unpin + Send + 'static,
) -> (mpsc::UnboundedSender<Vec<u8>>, tokio::task::JoinHandle<Result<()>>) {
    let (out, mut lines) = mpsc::unbounded_channel::<Vec<u8>>();
    let writing = tokio::spawn(async move {
        while let Some(mut line) = lines.recv().await {
            line.push(b'\n');
            output.write_all(&line).await?;
            output.flush().await?;
        }
        Ok(())
    });
    (out, writing)
}

/// One POST of `message` to the daemon's `/mcp` over HTTP/1.1, the connection closed after it,
/// with no `Origin`, which the daemon's endpoint refuses as a web page's.
async fn post(port: u16, message: &[u8]) -> Result<Reply, Failed> {
    let stream = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(("127.0.0.1", port))).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => return Err(Failed::Unsent(anyhow!("connecting to port {port}: {e}"))),
        Err(_) => {
            return Err(Failed::Unsent(anyhow!("port {port} took no connection within {}s", CONNECT_TIMEOUT.as_secs())))
        }
    };
    let head = format!(
        "POST {ENDPOINT} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        message.len()
    );
    let mut stream = BufReader::new(stream);
    let sent = async {
        stream.get_mut().write_all(head.as_bytes()).await?;
        stream.get_mut().write_all(message).await?;
        stream.get_mut().flush().await
    };
    sent.await.map_err(|e| Failed::Unanswered(anyhow!("sending to port {port}: {e}")))?;
    read_reply(&mut stream).await.map_err(Failed::Unanswered)
}

/// The reply on a connection the daemon closes after it: its status, its headers, and a body
/// framed by `Content-Length` or by the close, each within the relay's bounds.
async fn read_reply(stream: &mut (impl AsyncBufRead + Unpin)) -> Result<Reply> {
    let status_line =
        read_line(stream).await?.ok_or_else(|| anyhow!("the daemon closed the connection without answering"))?;
    let mut words = status_line.split_whitespace();
    let status = match (words.next(), words.next().and_then(|s| s.parse::<u16>().ok())) {
        (Some(version), Some(status)) if version.starts_with("HTTP/1.") => status,
        _ => bail!("the daemon's answer has no status line: {:?}", status_line.chars().take(80).collect::<String>()),
    };
    let mut headers = Vec::new();
    loop {
        let line = read_line(stream).await?.ok_or_else(|| anyhow!("the daemon's answer ended in its headers"))?;
        if line.is_empty() {
            break;
        }
        if headers.len() == MAX_HEADERS {
            bail!("the daemon's answer has more than {MAX_HEADERS} headers");
        }
        let (name, value) = line.split_once(':').ok_or_else(|| anyhow!("a malformed header in the daemon's answer"))?;
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }
    let mut reply = Reply { status, headers, body: Vec::new() };
    if let Some(encoding) = reply.header("transfer-encoding") {
        bail!("the daemon answered with a {encoding} body, which this relay does not read; update fwd");
    }
    match reply.header("content-length") {
        Some(length) => {
            let length: usize = length.parse().map_err(|_| anyhow!("the daemon's Content-Length {length:?}"))?;
            if length > MAX_BODY {
                bail!("the daemon's answer is {length} bytes, more than the {MAX_BODY} this relay reads");
            }
            reply.body = vec![0; length];
            stream.read_exact(&mut reply.body).await.context("the daemon's answer ended before its length")?;
        }
        None => {
            stream.take(MAX_BODY as u64 + 1).read_to_end(&mut reply.body).await?;
            if reply.body.len() > MAX_BODY {
                bail!("the daemon's answer is more than the {MAX_BODY} bytes this relay reads");
            }
        }
    }
    Ok(reply)
}

/// One CRLF- or LF-ended line of at most `MAX_LINE` bytes, without its ending; none at a clean end.
async fn read_line(stream: &mut (impl AsyncBufRead + Unpin)) -> Result<Option<String>> {
    let mut line = Vec::new();
    let n = stream.take(MAX_LINE as u64 + 1).read_until(b'\n', &mut line).await?;
    if n == 0 {
        return Ok(None);
    }
    if line.last() != Some(&b'\n') {
        if line.len() > MAX_LINE {
            bail!("a line of the daemon's answer is longer than {MAX_LINE} bytes");
        }
        bail!("the daemon's answer ended in the middle of a line");
    }
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    String::from_utf8(line).map(Some).map_err(|_| anyhow!("a line of the daemon's answer is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fwd_core::daemon::{Bind, PortFile};
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    /// What the stand-in daemon does with a POST.
    enum Act {
        /// Writes these bytes as its whole answer, then closes.
        Answer(String),
        /// Reads the request, then closes without a byte.
        Close,
        /// Reads the request and never answers.
        Hang,
    }

    /// A stand-in daemon on a port of its own: it answers `/health` as `instance`, `/projects` with
    /// none, and each POST to `/mcp` as `act(n, body)` says, recording the bodies it got.
    async fn stand_in(
        instance: &'static str,
        act: impl Fn(usize, &[u8]) -> Act + Send + Sync + 'static,
    ) -> (u16, Arc<Mutex<Vec<Vec<u8>>>>, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let posts = Arc::new(Mutex::new(Vec::new()));
        let heads = Arc::new(Mutex::new(Vec::new()));
        let (log, head_log, act) = (posts.clone(), heads.clone(), Arc::new(act));
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let (log, head_log, act) = (log.clone(), head_log.clone(), act.clone());
                tokio::spawn(async move {
                    let mut stream = BufReader::new(stream);
                    let (mut head, mut length) = (String::new(), 0);
                    loop {
                        let mut line = String::new();
                        if stream.read_line(&mut line).await.unwrap_or(0) == 0 {
                            return;
                        }
                        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                            length = v.trim().parse().unwrap();
                        }
                        head.push_str(&line);
                        if line == "\r\n" {
                            break;
                        }
                    }
                    let mut body = vec![0; length];
                    stream.read_exact(&mut body).await.unwrap();
                    let reply = if head.starts_with("GET /health") {
                        let health = json!({"pid": 4242, "instance_id": instance}).to_string();
                        Act::Answer(format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{health}", health.len()))
                    } else if head.starts_with("GET /projects") {
                        Act::Answer("HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n[]".into())
                    } else {
                        let n = {
                            let mut log = log.lock().unwrap();
                            log.push(body.clone());
                            head_log.lock().unwrap().push(head);
                            log.len() - 1
                        };
                        act(n, &body)
                    };
                    match reply {
                        Act::Answer(bytes) => {
                            let _ = stream.get_mut().write_all(bytes.as_bytes()).await;
                            let _ = stream.get_mut().shutdown().await;
                        }
                        Act::Close => {}
                        Act::Hang => std::future::pending::<()>().await,
                    }
                });
            }
        });
        (port, posts, heads)
    }

    fn json_answer(body: &str) -> Act {
        Act::Answer(format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        ))
    }

    /// The daemon's answer to a request: its id echoed with `result`.
    fn echo(body: &[u8]) -> Act {
        let request: Value = serde_json::from_slice(body).unwrap();
        if request.get("id").is_none() {
            return Act::Answer("HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\n\r\n".into());
        }
        json_answer(&json!({"jsonrpc": "2.0", "id": request["id"], "result": {"echo": request["params"]}}).to_string())
    }

    /// Runs a relay over `input` against `daemon` and returns the lines it wrote.
    async fn relay_lines_with(daemon: Daemon, input: &str, grace: Duration) -> Vec<Value> {
        let (out_end, mut in_end) = tokio::io::duplex(1 << 20);
        let (out, writing) = writer(out_end);
        let relay = Arc::new(Relay { daemon, out, grace });
        relay.serve(BufReader::new(input.as_bytes())).await.unwrap();
        drop(relay);
        writing.await.unwrap().unwrap();
        let mut text = String::new();
        in_end.read_to_string(&mut text).await.unwrap();
        text.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    async fn relay_lines(port: u16, input: &str) -> Vec<Value> {
        relay_lines_with(Daemon::Ports(Mutex::new(VecDeque::from([port]))), input, GRACE).await
    }

    fn request(id: u64) -> String {
        format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"tools/call\",\"params\":{{}}}}\n")
    }

    /// The one line the relay wrote, which must answer `id` with an error containing `says`.
    fn one_error(lines: &[Value], id: u64, says: &str) {
        assert_eq!(lines.len(), 1, "exactly one line: {lines:?}");
        assert_eq!(lines[0]["id"], json!(id));
        let message = lines[0]["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains(says), "{message:?} should say {says:?}");
    }

    #[tokio::test]
    async fn relays_the_hosts_bytes_and_meta_untouched() {
        let (port, posts, heads) = stand_in("a", |_, body| echo(body)).await;
        let call = r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"whoami","arguments":{"threadId":"MODEL"},"_meta":{"threadId":"T","sessionId":"S","x-codex-turn-metadata":{"thread_id":"T","session_id":"S","turn_id":"U"}}}}"#;
        let note = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let lines = relay_lines(port, &format!("{note}\n\n{call}\r\n")).await;
        assert_eq!(lines.len(), 1, "a notification is owed nothing: {lines:?}");
        assert_eq!(lines[0]["result"]["echo"]["_meta"]["x-codex-turn-metadata"]["turn_id"], json!("U"));
        let posts = posts.lock().unwrap();
        assert!(posts.iter().any(|p| p == call.as_bytes()), "the daemon gets the host's bytes as they were");
        let head = heads
            .lock()
            .unwrap()
            .iter()
            .find(|h| h.contains(&format!("Content-Length: {}", call.len())))
            .unwrap()
            .to_ascii_lowercase();
        assert!(head.starts_with("post /mcp http/1.1\r\n"));
        assert!(head.contains("accept: application/json, text/event-stream"));
        assert!(!head.contains("origin:"), "the daemon refuses a web page's Origin");
    }

    #[tokio::test]
    async fn a_request_the_daemon_accepted_then_dropped_is_answered_once_and_never_sent_again() {
        let (port, posts, _) = stand_in("a", |_, _| Act::Close).await;
        one_error(&relay_lines(port, &request(1)).await, 1, "may have been made");
        assert_eq!(posts.lock().unwrap().len(), 1, "sent once");

        let (port, posts, _) = stand_in("a", |_, _| Act::Answer("HTTP/1.1 200 OK\r\nContent-Le".into())).await;
        one_error(&relay_lines(port, &request(2)).await, 2, "may have been made");
        assert_eq!(posts.lock().unwrap().len(), 1, "sent once");

        let (port, posts, _) =
            stand_in("a", |_, _| Act::Answer("HTTP/1.1 200 OK\r\ncontent-length: 50\r\n\r\n{\"jsonrpc\"".into())).await;
        one_error(&relay_lines(port, &request(3)).await, 3, "ended before its length");
        assert_eq!(posts.lock().unwrap().len(), 1, "sent once");
    }

    #[tokio::test]
    async fn every_request_gets_exactly_one_answer_under_its_id() {
        // Accepted with 202 and no answer.
        let (port, _, _) =
            stand_in("a", |_, _| Act::Answer("HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\n\r\n".into())).await;
        one_error(&relay_lines(port, &request(1)).await, 1, "sent no answer");
        // Refused under a null id.
        let refusal =
            json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32600, "message": "bad request"}}).to_string();
        let (port, _, _) = stand_in("a", move |_, _| json_answer(&refusal)).await;
        one_error(&relay_lines(port, &request(2)).await, 2, "refused the call: bad request");
        // Answered under another id.
        let other = json!({"jsonrpc": "2.0", "id": 99, "result": {}}).to_string();
        let (port, _, _) = stand_in("a", move |_, _| json_answer(&other)).await;
        one_error(&relay_lines(port, &request(3)).await, 3, "carried no answer to this request");
        // A batch that holds the answer among others: only the answer.
        let batch =
            json!([{"jsonrpc": "2.0", "id": 99, "result": {}}, {"jsonrpc": "2.0", "id": 4, "result": {"ok": true}}])
                .to_string();
        let (port, _, _) = stand_in("a", move |_, _| json_answer(&batch)).await;
        assert_eq!(
            relay_lines(port, &request(4)).await,
            vec![json!({"jsonrpc": "2.0", "id": 4, "result": {"ok": true}})]
        );
        // A refusal the daemon words in plain text.
        let (port, _, _) =
            stand_in("a", |_, _| Act::Answer("HTTP/1.1 403 Forbidden\r\ncontent-length: 10\r\n\r\nforbidden\n".into()))
                .await;
        one_error(&relay_lines(port, &request(5)).await, 5, "refused the call (403): forbidden");
        // A daemon from before the endpoint.
        let (port, _, _) =
            stand_in("a", |_, _| Act::Answer("HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n".into())).await;
        one_error(&relay_lines(port, &request(6)).await, 6, "predates it");
        // A line that is not JSON.
        let lines = relay_lines(port, "not json\n").await;
        assert_eq!(lines.len(), 1);
        assert_eq!((lines[0]["id"].clone(), lines[0]["error"]["code"].clone()), (Value::Null, json!(-32700)));
    }

    #[tokio::test]
    async fn framing_it_does_not_read_is_refused_by_name_and_within_bounds() {
        let cases: [(&str, String); 5] = [
            ("event stream", "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\ndata: {}\n\n".into()),
            ("chunked body", "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\nffffffffffffffff\r\n".into()),
            ("more than", format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n", u64::MAX)),
            ("longer than", format!("HTTP/1.1 200 OK\r\nx-long: {}\r\n\r\n", "a".repeat(MAX_LINE + 10))),
            ("no status line", "SPDY nonsense\r\n\r\n".into()),
        ];
        for (says, bytes) in cases {
            let (port, posts, _) = stand_in("a", move |_, _| Act::Answer(bytes.clone())).await;
            one_error(&relay_lines(port, &request(1)).await, 1, says);
            assert_eq!(posts.lock().unwrap().len(), 1, "{says}: sent once");
        }
    }

    #[tokio::test]
    async fn a_port_that_takes_no_connection_is_looked_up_again_and_the_call_goes_once() {
        let dead = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port();
        let (live, posts, _) = stand_in("a", |_, body| echo(body)).await;
        let daemon = Daemon::Ports(Mutex::new(VecDeque::from([dead, live])));
        let lines = relay_lines_with(daemon, &request(1), GRACE).await;
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["id"], json!(1));
        assert!(lines[0].get("result").is_some(), "{lines:?}");
        assert_eq!(posts.lock().unwrap().len(), 1);

        let unreachable = Daemon::Ports(Mutex::new(VecDeque::from([dead])));
        one_error(&relay_lines_with(unreachable, &request(2), GRACE).await, 2, "could not be reached");
    }

    #[tokio::test]
    async fn each_call_goes_to_the_daemon_the_port_file_names_now_not_a_stale_one_still_listening() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths { runtime_dir: dir.path().join("run"), data_dir: dir.path().join("data") };
        paths.ensure().unwrap();
        let (old, old_posts, _) = stand_in("old", |_, body| echo(body)).await;
        let (new, new_posts, _) = stand_in("new", |_, body| echo(body)).await;
        let names = |port, instance: &str| {
            daemon::write_port_file(
                &paths.port_file(),
                &PortFile { port, pid: 4242, instance_id: instance.into(), started: 0, bind: Bind::Loopback },
            )
            .unwrap()
        };
        let (out_end, mut in_end) = tokio::io::duplex(1 << 20);
        let (out, writing) = writer(out_end);
        let relay = Arc::new(Relay {
            daemon: Daemon::Machine { paths: paths.clone(), autostart: Autostart::Never },
            out,
            grace: GRACE,
        });
        names(old, "old");
        relay.serve(BufReader::new(request(1).as_bytes())).await.unwrap();
        // The daemon restarted elsewhere; whatever holds the old port still answers.
        names(new, "new");
        relay.serve(BufReader::new(request(2).as_bytes())).await.unwrap();
        // A port file whose daemon does not answer as the instance it names: no daemon, no call.
        names(old, "gone");
        relay.serve(BufReader::new(request(3).as_bytes())).await.unwrap();
        drop(relay);
        writing.await.unwrap().unwrap();
        let mut text = String::new();
        in_end.read_to_string(&mut text).await.unwrap();
        let lines: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].get("result").is_some() && lines[1].get("result").is_some());
        assert!(lines[2]["error"]["message"].as_str().unwrap().contains("finding the fwd daemon"), "{:?}", lines[2]);
        assert_eq!(old_posts.lock().unwrap().len(), 1, "the stale daemon got the first call only");
        assert_eq!(new_posts.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_relay_ends_soon_after_its_input_ends_even_with_a_call_hung_in_the_daemon() {
        let (port, posts, _) = stand_in("a", |_, _| Act::Hang).await;
        let started = tokio::time::Instant::now();
        let daemon = Daemon::Ports(Mutex::new(VecDeque::from([port])));
        let lines = relay_lines_with(daemon, &request(1), Duration::from_millis(200)).await;
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
        assert!(lines.is_empty(), "the host is gone; nothing is owed: {lines:?}");
        assert_eq!(posts.lock().unwrap().len(), 1);
    }
}
