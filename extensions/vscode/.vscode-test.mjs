// The headless test (`npm test`): one automated VS Code run against a scratch project, never a
// person's. This file runs in the test runner's own process before VS Code starts. It makes a git
// repository with one commit and an origin remote, runs `fwd init` there (which starts a scratch
// daemon), names two agents, one to comment as and one that holds triage for the whole project (the
// scratch project has no scope for one to serve), points the extension's `fwd.path` at the same
// binary through the repository's workspace settings, and opens VS Code on the repository.
// Every `fwd`, here and inside VS Code, sees FWD_RUNTIME_DIR and FWD_DATA_DIR under a new
// temporary directory, so nothing reaches the default data directory or a daemon already running.
// The binary is $FWD_BIN, or else target/debug/fwd in this repository (`cargo build -p fwd`).

import { defineConfig } from '@vscode/test-cli';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const FWD = process.env.FWD_BIN ?? fileURLToPath(new URL('../../target/debug/fwd', import.meta.url));
if (!fs.existsSync(FWD)) throw new Error(`no fwd binary at ${FWD}: build it with \`cargo build -p fwd\`, or set FWD_BIN`);
const T = fs.mkdtempSync(path.join(os.tmpdir(), 'fwd-vscode-test-'));
const REPO = path.join(T, 'repo');
// Set on this process, so VS Code, its extension host and every `fwd` they run inherit them.
process.env.FWD_RUNTIME_DIR = path.join(T, 'run');
process.env.FWD_DATA_DIR = path.join(T, 'data');
// The daemon publishes its status for sutra; a scratch daemon must not appear on the real dashboard.
process.env.FWD_DEV_RUNNER_DIR = path.join(T, 'dev-runner');
// The file the extension host writes as the suite starts (src/test/started.ts). When VS Code exits
// before it exists, no test has run.
const STARTED = path.join(T, 'suite-started');
process.env.FWD_SUITE_STARTED = STARTED;
// Settings a caller's shell may carry that would point `fwd` at another project, principal,
// scope or daemon binary.
for (const name of ['FWD_PROJECT', 'FWD_AS', 'FWD_SCOPE', 'FWD_HANDLE', 'FWD_DAEMON_BIN', 'FWD_DEV_ORIGIN']) {
  delete process.env[name];
}

const fwd = (...args) => execFileSync(FWD, args, { cwd: REPO, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], timeout: 30_000 });
const git = (...args) => execFileSync('git', args, { cwd: REPO, stdio: 'ignore', timeout: 30_000 });
/** The `ps` lines of daemons serving the scratch runtime directory. */
const scratchDaemons = () =>
  execFileSync('ps', ['-Ao', 'pid,command'], { encoding: 'utf8' })
    .split('\n')
    .filter((line) => line.includes(`--runtime-dir ${process.env.FWD_RUNTIME_DIR}`));

// However the run ends, the scratch daemon is stopped here, once VS Code has exited, and `ps` is
// asked whether any is left. The order matters, since a `fwd` call starts a daemon when none runs.
// This runs as the runner's process exits: test-electron has seen VS Code's process exit, which
// VS Code does only after its extension host has exited (its log says "Extension host with pid N
// exited" first), and the suite, the last thing the host ran, checked that no `fwd` call the
// extension started still ran; so nothing of the window calls `fwd` after this stop. When the
// suite stopped the daemon itself, before VS Code had ended, a call the window made after the stop
// started a new daemon, which failed this check on CI (run 36757718566, task lK-_4TEE). The calls
// the extension makes by itself also carry --no-autostart, and end as it deactivates, so that none
// starts a daemon after a stop; a call right after this stop checks that the CLI refuses such a
// call. The scratch directory goes when everything passed and is kept, and named, otherwise.
// When VS Code exited before the suite started and the scratch daemon stopped cleanly, no test ran,
// and the runner exits with NO_TEST_RAN, on which scripts/test-extension-headless.sh launches the
// suite again: on CI, VS Code exited so when its window did not answer for 15 seconds while it
// loaded (task EyLMg55J). Any other failure exits with 1, so that no failing test runs again.
const NO_TEST_RAN = 75; // sysexits.h's EX_TEMPFAIL: a failure that may pass when tried again
let launching = false; // set once the scratch project is ready and test-cli goes on to launch VS Code
process.on('exit', (code) => {
  const say = (line) => fs.writeSync(1, `${line}\n`); // stdout is asynchronous on macOS pipes
  const before = scratchDaemons();
  say(`ps before the stop: ${before.length} scratch daemon(s)${before.map((line) => `\n  ${line.trim()}`).join('')}`);
  try {
    execFileSync(FWD, ['daemon', 'stop'], { cwd: REPO, stdio: 'ignore', timeout: 15_000 });
  } catch {}
  for (let tries = 0; scratchDaemons().length && tries < 10; tries++) execFileSync('sleep', ['0.5']);
  // The call the extension makes by itself to place a document's highlights (src/comments.ts,
  // `placeAll`), right after the stop: no daemon answers it, and the CLI starts none.
  let late = '';
  if (before.length) {
    try {
      fwd('--no-autostart', 'anchor', 'list', '--path', 'src/lib.rs', '--against', 'worktree');
      late = 'a daemon answered it';
    } catch (e) {
      if (!String(e.stderr).includes('autostart is off')) late = `it failed otherwise: ${String(e.stderr || e.message).trim()}`;
    }
    say(late ? `FAIL a fwd call with --no-autostart right after the stop: ${late}` : 'a fwd call with --no-autostart right after the stop: refused');
  }
  const left = scratchDaemons();
  say(left.length ? `FAIL a scratch daemon is still running: ${left.join('; ')}` : 'ps after the stop: no scratch daemon is running');
  if (code === 0 && !left.length && !late) {
    fs.rmSync(T, { recursive: true, force: true });
  } else {
    say(`(scratch directory kept: ${T})`);
    process.exitCode = 1;
    if (code !== 0 && launching && !fs.existsSync(STARTED) && !left.length && !late) {
      say(`the suite never started, so no test ran (the runner exits with ${NO_TEST_RAN})`);
      process.exitCode = NO_TEST_RAN;
    }
  }
});

fs.mkdirSync(path.join(REPO, 'src'), { recursive: true });
git('init', '-q', '-b', 'main');
git('config', 'user.email', 't@e.com');
git('config', 'user.name', 't');
git('remote', 'add', 'origin', 'git@example.com:t/vscode.git');
fs.writeFileSync(path.join(REPO, 'src', 'lib.rs'), 'fn one() {}\nfn two() {}\nfn three() {}\n');
git('add', 'src/lib.rs');
git('-c', 'commit.gpgsign=false', 'commit', '-q', '-m', 'init');
fwd('init', '--name', 'scratch', '--handle', 'tester', '--display-name', 'Tester');
// Two agents name themselves, as an agent does from its own session: one that comments from the
// CLI, and one that holds triage and so answers the questions asked of no one. A triage agent
// serves a scope, and the scratch project has none, so this one is endorsed for the whole project,
// as a small project may endorse one (docs/DECISIONS.md, D122).
fwd('--as', 'impl-alice', 'principal', 'add', 'impl-alice', '--role', 'implementer');
fwd('--as', 'triage-bot', 'principal', 'add', 'triage-bot', '--role', 'triage', '--project-wide');
fs.mkdirSync(path.join(REPO, '.vscode'));
fs.writeFileSync(path.join(REPO, '.vscode', 'settings.json'), JSON.stringify({ 'fwd.path': FWD }, null, 2));
launching = true;

export default defineConfig({
  files: 'out/test/**/*.test.js',
  workspaceFolder: REPO,
  // So that the window, as it loads, does not wait for a software OpenGL to start. The window waits
  // for VS Code's GPU process as it loads, and with no GPU (Xvfb has none) that process first loads
  // and starts Mesa's software OpenGL, with the LLVM library it compiles through; with --disable-gpu
  // it draws without OpenGL and loads none of it. On CI, VS Code exited before the suite when its
  // window did not answer for 15 seconds while it loaded (task EyLMg55J); in the container, the GPU
  // process stopped for 20 seconds as it started made VS Code exit the same way, with the same line
  // ("CodeWindow: detected unresponsive") 16 seconds after the window opened, --disable-gpu or not.
  launchArgs: ['--disable-gpu'],
  // started.js tells the exit handler above that the suite started.
  mocha: { timeout: 60_000, preload: './out/test/started.js' },
});
