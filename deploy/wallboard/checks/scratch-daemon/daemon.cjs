// The scratch daemon and the fwd commands that speak to it, under the environment `openEnvironment` isolated.
const fs = require('node:fs'), path = require('node:path');
const { spawnSync, execFileSync } = require('node:child_process');

const PRINCIPAL = 'tester';

function openDaemon(root, binaries, report) {
  const { expect, line } = report;
  // Only fwd gets this home: nothing under the real one can be its default for anything.
  const home = path.join(root, 'home');
  fs.mkdirSync(home);
  const fwd = (binary, args, cwd, { allowFailure = false } = {}) => {
    const result = spawnSync(binary, args, { cwd, env: { ...process.env, HOME: home }, encoding: 'utf8', timeout: 60_000 });
    line('cli.log', `${binary === binaries.daemon ? 'daemon build' : 'client'} ${args.join(' ')} -> exit ${result.status}\n${result.stdout}${result.stderr}`);
    if (result.status !== 0 && !allowFailure) throw Error(`fwd ${args.join(' ')} exited ${result.status}: ${result.stderr}`);
    return result;
  };
  const json = result => JSON.parse(result.stdout);
  const version = binary => fwd(binary, ['--version'], root).stdout.trim();

  // The first command that needs a daemon starts it, so from then on there may be one to stop.
  let first, mayHaveStarted = false;
  return {
    versions: () => ({ daemon: version(binaries.daemon), client: version(binaries.client) }),
    // Exit 7 is the daemon's own "no daemon is answering". Anything else stops the run before it writes.
    assertNoneAnswers() {
      const before = fwd(binaries.daemon, ['daemon', 'status'], root, { allowFailure: true });
      expect('under the isolation variables no daemon answers yet', before.status === 7, { exit: before.status, said: before.stderr.trim() });
      if (before.status !== 7) throw Error('Something answers under the isolation variables; refusing to go on.');
    },
    // The daemon's own account of itself, checked each time against the first.
    expectSameDaemon(where) {
      const status = json(fwd(binaries.daemon, ['daemon', 'status'], root));
      const command = execFileSync('ps', ['-o', 'command=', '-p', String(status.pid)], { encoding: 'utf8' }).trim();
      const found = { pid: status.pid, instance_id: status.instance_id, port: status.port, started: status.started };
      const ours = command.startsWith(`${binaries.daemon} daemon serve`) && command.includes(root);
      if (!first) {
        expect('the scratch daemon is the binary asked for, on this run\'s directories', ours, { command });
        if (!ours) throw Error('The daemon that answers is not this run\'s; refusing to go on.');
        first = found;
      }
      expect(`the scratch daemon is the same process and instance (${where})`, ours && JSON.stringify(found) === JSON.stringify(first), found);
    },
    port: () => first.port,
    stop() {
      if (!mayHaveStarted) return;
      mayHaveStarted = false;
      const stopped = fwd(binaries.daemon, ['daemon', 'stop'], root, { allowFailure: true });
      const after = fwd(binaries.daemon, ['daemon', 'status'], root, { allowFailure: true });
      expect('the scratch daemon stopped, through fwd daemon stop', /stopped/.test(stopped.stdout) && after.status === 7, { stop: stopped.stdout.trim(), statusExit: after.status });
      const daemonLog = path.join(process.env.FWD_DATA_DIR, 'daemon.log');
      const troubled = fs.existsSync(daemonLog) ? fs.readFileSync(daemonLog, 'utf8').split('\n').filter(entry => /panic|poison/i.test(entry)) : [];
      expect('the scratch daemon logged no panic or poisoned lock', troubled.length === 0, troubled.slice(0, 3));
    },
    // A project of its own, in a scratch repository, for one scenario. A write goes through the client
    // built from this checkout when it names a date, which the daemon's build may not know.
    project(name) {
      const repository = path.join(root, name);
      fs.mkdirSync(repository);
      execFileSync('git', ['-c', 'init.defaultBranch=main', 'init', '-q', '.'], { cwd: repository });
      execFileSync('git', ['remote', 'add', 'origin', `git@example.invalid:scratch/${name}.git`], { cwd: repository });
      mayHaveStarted = true;
      const { id } = json(fwd(binaries.daemon, ['init', '--name', `scratch-${name}`, '--handle', PRINCIPAL], repository)).project;
      this.expectSameDaemon(`after ${name} opened`);
      const daemonBuild = (args, options) => fwd(binaries.daemon, ['--as', PRINCIPAL, ...args], repository, options);
      const client = (args, options) => fwd(binaries.client, ['--as', PRINCIPAL, '--no-autostart', ...args], repository, options);
      // The daemon's store, read only: one for every project since the daemon keeps them all in one
      // node (docs/DECISIONS.md, D135), and the project's own for a daemon of an earlier build.
      const oneStore = path.join(process.env.FWD_DATA_DIR, 'store.sqlite');
      const database = fs.existsSync(oneStore) ? oneStore : path.join(process.env.FWD_DATA_DIR, 'projects', id, 'project.sqlite');
      const sql = query => execFileSync('sqlite3', [`file:${database}?mode=ro`, query], { encoding: 'utf8' }).trim();
      return {
        id, daemonBuild,
        task: (title, ...flags) => json(daemonBuild(['task', 'create', title, '--what', `What ${title.toLowerCase()} is`, ...flags])).id,
        taskWithWhat: (title, what) => json(daemonBuild(['task', 'create', title, '--what', what])).id,
        show: task => json(client(['task', 'show', task])),
        start: task => daemonBuild(['task', 'start', task]),
        rename: (task, title) => daemonBuild(['task', 'amend', task, '--title', title]),
        comment: (task, text) => daemonBuild(['comment', task, text]),
        decide: title => daemonBuild(['decision', 'create', title, '--area', 'architecture', '--decision', `${title}, as ruled`]),
        // A highlight on the first line of `file`, which this writes into the repository first.
        highlight: (file, line) => {
          fs.writeFileSync(path.join(repository, file), `${line}\n`);
          return daemonBuild(['anchor', 'create', '--path', file, '--lines', '1-1']);
        },
        date: (task, when) => client(['task', 'amend', task, '--not-before', when]),
        clearDate: task => client(['task', 'amend', task, '--clear-not-before']),
        link: (waiting, awaited) => daemonBuild(['link', waiting, 'depends_on', awaited]),
        propose: (waiting, awaited) => json(daemonBuild(['link', waiting, 'depends_on', awaited, '--propose'])).id,
        accept: relation => daemonBuild(['relation', 'approve', relation]),
        unlink: (waiting, awaited) => daemonBuild(['unlink', waiting, 'depends_on', awaited, '--reason', 'Scratch run retracting it']),
        eventCount: () => Number(sql('select count(*) from _ankurah_event')),
        registered: property => sql(`select count(*) from _ankurah_property where name = '${property}'`) !== '0',
      };
    },
  };
}

module.exports = { openDaemon };
