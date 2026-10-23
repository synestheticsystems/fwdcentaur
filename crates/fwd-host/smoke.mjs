// Smoke test for the extension host's node (crates/fwd-host) against a scratch daemon: the node
// joins a project the daemon hosts, over its WebSocket and with node:sqlite storage; a live query
// hears the notification a CLI comment causes; the node reads the comment's anchor and replies;
// the CLI's inbox shows the reply; the node says when its socket closes as the daemon stops; and
// closing the host lets this process exit on its own.
//
// Every `fwd` here runs with FWD_RUNTIME_DIR and FWD_DATA_DIR under a new temporary directory,
// so it never reaches the default data directory or a daemon already running on this machine.
//
// Usage: node crates/fwd-host/smoke.mjs   (the CLI is $FWD_BIN, or else target/debug/fwd in this
// repository: build it with `cargo build -p fwd`)
// Build the package first: (cd crates/fwd-host && wasm-pack build --target nodejs --dev)
// Needs Node.js 22.5 or later (node:sqlite, a global WebSocket); VS Code runs extensions on 24.

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const host = createRequire(import.meta.url)("./pkg/fwd_host.js");

const FWD = process.env.FWD_BIN ?? fileURLToPath(new URL("../../target/debug/fwd", import.meta.url));
if (!fs.existsSync(FWD)) {
  console.log(`FAIL no fwd binary at ${FWD}: build it with \`cargo build -p fwd\`, or set FWD_BIN`);
  process.exit(1);
}
const T = fs.mkdtempSync(path.join(os.tmpdir(), "fwd-host-smoke-"));
const REPO = path.join(T, "repo");
const env = {
  ...process.env,
  FWD_RUNTIME_DIR: path.join(T, "run"),
  FWD_DATA_DIR: path.join(T, "data"),
  // The daemon publishes its status for sutra; a scratch daemon must not appear on the real dashboard.
  FWD_DEV_RUNNER_DIR: path.join(T, "dev-runner"),
};
// Settings a caller's shell may carry that would point `fwd` at another project, principal,
// scope or daemon binary.
for (const name of ["FWD_PROJECT", "FWD_AS", "FWD_SCOPE", "FWD_HANDLE", "FWD_DAEMON_BIN", "FWD_DEV_ORIGIN"]) {
  delete env[name];
}

const fwd = (...args) => execFileSync(FWD, args, { cwd: REPO, env, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
// `--format json` goes first so that an entity id can follow `--`: ids are base64url, and one in
// 64 begins with `-`, which the CLI's argument parser otherwise reads as a flag.
const fwdJson = (...args) => JSON.parse(fwd("--format", "json", ...args));
const git = (...args) => execFileSync("git", args, { cwd: REPO, stdio: "ignore" });
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
/** The first truthy answer of `probe`, asked every 100 ms for up to `ms`; undefined if none. */
async function until(probe, ms) {
  for (const deadline = Date.now() + ms; ; await sleep(100)) {
    const answer = await probe();
    if (answer || Date.now() >= deadline) return answer || undefined;
  }
}
/** What a promise rejected with, or "resolved" when it did not reject. */
const rejection = (promise) => promise.then(() => "resolved", (error) => error);

let passed = 0;
let failed = 0;
const show = (value) => (typeof value === "string" ? value : JSON.stringify(value));
function check(what, got, expected) {
  if (show(got) === show(expected)) {
    passed++;
    console.log(`  ok   ${what}`);
  } else {
    failed++;
    console.log(`  FAIL ${what}: expected [${show(expected)}] got [${show(got)}]`);
  }
}

let daemonRunning = false;
let closedAt;

async function main() {
  console.log("== setup ==");
  fs.mkdirSync(path.join(REPO, "src"), { recursive: true });
  git("init", "-q", "-b", "main");
  git("config", "user.email", "t@e.com");
  git("config", "user.name", "t");
  git("remote", "add", "origin", "git@example.com:t/host.git");
  fs.writeFileSync(path.join(REPO, "src", "lib.rs"), "fn one() {}\nfn two() {}\nfn three() {}\n");
  git("add", "src/lib.rs");
  git("commit", "-q", "-m", "init");
  fwd("init", "--name", "scratch", "--handle", "tester", "--display-name", "Tester");
  daemonRunning = true;
  fwd("--as", "impl-alice", "principal", "add", "impl-alice", "--role", "implementer"); // the second principal, who comments from the CLI: `--as` makes none
  const status = JSON.parse(fwd("daemon", "status"));
  const url = `ws://127.0.0.1:${status.port}/p/${status.project}`;

  console.log("== the host's node joins the daemon ==");
  const joining = Date.now();
  const h = await host.Host.connect(url, 8000);
  console.log(`  (joined in ${Date.now() - joining} ms as node ${h.nodeId()})`);
  const ctx = h.context();
  const me = await host.default_principal(ctx);
  check("the default principal is the project's person", me.handle, "tester");
  check("principal_named finds the CLI's second principal", (await host.principal_named(ctx, "impl-alice"))?.handle, "impl-alice");
  check("principal_named answers null for a name nobody has", await host.principal_named(ctx, "nobody"), null);

  console.log("== a live query hears what the CLI caused ==");
  const unread = host.Notification.query(ctx, "status = 'unread'");
  const added = [];
  let firstAddedAt;
  const guard = unread.subscribe((changes) => {
    for (const view of changes.added) {
      added.push(view.id.to_base64());
      firstAddedAt ??= Date.now();
    }
  }, false);
  check("the live query loads", await until(() => unread.loaded, 5000), true);
  const comment = fwdJson("--as", "impl-alice", "comment", "--path", "src/lib.rs", "--lines", "1-2", "hello from the CLI, @tester");
  const commented = Date.now();
  check("the subscription hears a notification added", await until(() => added.length > 0, 5000), true);
  if (firstAddedAt) console.log(`  (heard ${firstAddedAt - commented} ms after the CLI returned)`);
  const mine = await host.inbox(ctx, me.id, true);
  check("the tester's unread inbox holds the mention", mine.map((n) => n.type).join(","), "mentioned");
  check("the subscription heard that notification", added.includes(mine[0]?.id), true);
  check("the mention points at the CLI's message", mine[0]?.originator?.message_id, comment.id);

  console.log("== the host reads the anchor and replies ==");
  const anchors = await host.anchors_in_path(ctx, "src/lib.rs");
  check("anchors_in_path finds the comment's anchor", anchors.map((a) => a.id).join(","), comment.anchor);
  const replyText = "a reply from the extension host";
  const replyId = await host.post_message(ctx, me.id, replyText, "next", { reply_to: comment.id });
  const heard = await until(
    () => fwdJson("--as", "impl-alice", "inbox", "--unread").find((n) => n.type === "replied" && n.message?.id === replyId),
    5000,
  );
  check("the comment's author sees the reply in `fwd inbox`", heard?.message?.content, replyText);
  const thread = await host.messages_about(ctx, "anchor", comment.anchor);
  check("messages_about(anchor) returns the comment and the reply", thread.map((m) => m.id).join(","), `${comment.id},${replyId}`);
  check("the reply is in the comment's thread", thread[1]?.thread, comment.id);
  check(
    "post_message refuses a context field it does not know",
    await rejection(host.post_message(ctx, me.id, "x", "next", { replyTo: comment.id })),
    "`context` has no field `replyTo`: it takes task_id, anchor_id, scope_id, to and reply_to",
  );
  await host.set_notification_status(ctx, [mine[0].id], "read");
  check("set_notification_status marks the mention read", (await host.inbox(ctx, me.id, true)).length, 0);
  await host.resolve_message(ctx, me.id, comment.id);
  check("resolve_message resolves the comment", (await host.messages_about(ctx, "anchor", comment.anchor))[0]?.status, "resolved");

  console.log("== task verbs ==");
  const task = fwdJson("task", "create", "Smoke task", "--why", "to exercise claim and transition");
  const scope = fwdJson("scope", "create", "Smoke scope", "--intention", "to exercise claim");
  await host.claim(ctx, me.id, task.id, scope.id);
  check("claim starts the task", fwdJson("task", "show", "--", task.id).status, "in_progress");
  check(
    "transition refuses start, which is claim",
    await rejection(host.transition(ctx, me.id, task.id, "start")),
    "A task starts with `claim`, which also assigns it to a scope.",
  );
  check("transition submits the task", await host.transition(ctx, me.id, task.id, "submit", "the smoke ran"), "done");

  console.log("== the node hears its socket close ==");
  const socket = h.connectionState();
  const now = socket.peek;
  check("the node's socket is connected", now.value(), "Connected");
  now.free();
  const states = [];
  const watch = socket.subscribe((state) => {
    states.push(state.value());
    state.free();
  });
  socket.free();
  check("the daemon stops", fwd("daemon", "stop").trim(), "stopped");
  daemonRunning = false;
  check("the node says its socket is no longer connected", await until(() => states.length > 0 && !states.includes("Connected"), 5000), true);
  watch.free();

  console.log("== closing ==");
  guard.free();
  unread.free();
  ctx.free();
  h.close();
  closedAt = Date.now();
  check("a closed host refuses a context", (() => { try { h.context(); return "no error"; } catch (error) { return error; } })(), "This host is closed.");
}

try {
  await main();
} catch (error) {
  failed++;
  console.log(`  FAIL ${error?.stack ?? error}`);
} finally {
  if (daemonRunning) {
    try {
      fwd("daemon", "stop");
    } catch {}
  }
}
console.log(`== ${passed} passed, ${failed} failed ==`);
if (failed > 0) {
  console.log(`(scratch directory kept: ${T})`);
  process.exit(1);
}
fs.rmSync(T, { recursive: true, force: true });
// Nothing keeps the process alive after `close()` if it exits on its own; the watchdog timer does
// not count (unref), and reports what still does.
process.on("beforeExit", () => console.log(`== the process exits on its own, ${Date.now() - closedAt} ms after close() ==`));
setTimeout(() => {
  console.log(`FAIL still alive 20 s after close(), held by: ${process.getActiveResourcesInfo().join(", ")}`);
  process.exit(1);
}, 20_000).unref();
