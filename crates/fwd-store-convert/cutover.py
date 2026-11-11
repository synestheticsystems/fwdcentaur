#!/usr/bin/env python3
"""Moves a machine's fwd from a build pinned at Ankurah f8e85d2 to one pinned at 5d8a000b
(`forward`), or back (`back`), so that no daemon of either build serves the data folder between the
old daemon's stop and the new one's start.

Other sessions keep running fwd commands throughout, and a command that finds no daemon starts
one. A command starts a daemon only while it holds the bootstrap lock in the runtime folder, and a
daemon serves only while it holds the lifetime lock there (both are flock locks on files fwd keeps
there). So this script takes the bootstrap lock before it stops the daemon and keeps it until the
next daemon answers: meanwhile every command waits 10 s for that lock and fails, starting nothing.
It takes the lifetime lock as soon as the old daemon has exited and keeps it until it starts the
next daemon itself: meanwhile no daemon of either build, however started, can serve the folder.
The folders are exchanged and the binary installed by renames, while both locks are held.

forward:
   0. confirm that the daemon answering through the runtime folder serves the data folder (its
      command line names it), and read the projects it hosts and each one's counts
   1. take the bootstrap lock (no command can start a daemon from here on)
   2. stop the daemon; `fwd daemon stop` returns once its process has exited
   3. take the lifetime lock (no daemon can serve the folder from here on)
   4. back up the data folder with the installed fwd (`fwd admin backup`), the safety copy
   5. convert the folder as it is on disk now into <data>.5d8a000b (fwd-store-convert), then
      check that no file other than the stores changed meanwhile (a command can write the project
      index or the anchors' cache without a daemon); if one did, stop with nothing switched
   6. exchange the folders in one rename; the old one then rests as <data>.f8e85d2
   7. install the staged fwd by a rename; the old binary rests as <installed>.f8e85d2
   8. release the lifetime lock and start the new daemon; commands reach it once it answers
   9. check that it hosts exactly the projects the old one did, none with an error, and that no
      project's counts shrank
  10. release the bootstrap lock

back: the same steps with the resting folder and binary, without the backup and the conversion; the
converted folder then rests as <data>.5d8a000b. What was written after the cutover is only there.

A failure, SIGINT, SIGTERM or SIGHUP up to step 8 undoes whatever was switched and ends the daemon
this script started, so the next command starts the previous build's daemon on the previous
folder. Once the new daemon has answered, clients may have written to it, so a failure in step 9 is
reported and the new pair is left in place; going back is the person's decision (`cutover.py back`).
README.md says what a SIGKILL can leave at each step and how to finish or undo it by hand.
"""

import argparse
import ctypes
import fcntl
import hashlib
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from contextlib import contextmanager
from pathlib import Path

OLD, NEW = "f8e85d2", "5d8a000b"
RENAME_SWAP = 0x2  # renamex_np(2): exchange the two paths in one step
LOCK_WAIT = 60  # seconds to wait for a launcher to let go of the bootstrap lock
START_WAIT = 120  # seconds the daemon it starts has to open every project and answer
STOPPING = {signal.SIGINT, signal.SIGTERM, signal.SIGHUP}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("direction", choices=["forward", "back"])
    parser.add_argument("--data-dir", type=Path, default=default_data_dir())
    parser.add_argument("--runtime-dir", type=Path, default=default_runtime_dir())
    parser.add_argument("--installed", type=Path, default=Path.home() / ".cargo/bin/fwd", help="the fwd other sessions run")
    parser.add_argument("--staged", type=Path, help="forward: the fwd built at Ankurah 5d8a000b, to install")
    parser.add_argument("--converter", type=Path, help="forward: the fwd-store-convert binary")
    parser.add_argument("--backups", type=Path, help="forward: the folder `fwd admin backup` writes into")
    args = parser.parse_args()
    machine = Machine(args.data_dir.absolute(), args.runtime_dir.absolute(), args.installed.absolute())
    if args.direction == "forward":
        if not (args.staged and args.converter and args.backups):
            parser.error("forward needs --staged, --converter and --backups")
        forward(machine, args.staged.absolute(), args.converter.absolute(), args.backups.absolute())
    else:
        back(machine)


def default_data_dir():
    return Path(os.environ.get("FWD_DATA_DIR") or Path.home() / "Library/Application Support/fwd")


def default_runtime_dir():
    return Path(os.environ.get("FWD_RUNTIME_DIR") or Path(tempfile.gettempdir()) / f"fwd-{os.getuid()}")


class Failure(Exception):
    pass


class Machine:
    """The data folder, the runtime folder and the installed binary this run switches over."""

    def __init__(self, data, runtime, installed):
        self.data, self.runtime, self.installed = data, runtime, installed
        self.env = {k: v for k, v in os.environ.items() if k not in ("FWD_PROJECT", "FWD_AS", "FWD_SCOPE")}
        self.env.update(FWD_DATA_DIR=str(data), FWD_RUNTIME_DIR=str(runtime))

    def resting(self, path, pin):
        return path.with_name(f"{path.name}.{pin}")

    def fwd(self, *args, check=True):
        done = run([str(self.installed), *args], env=self.env)
        if check and done.returncode != 0:
            raise Failure(f"`fwd {' '.join(args)}` failed: {first_line(done.stderr)}")
        return done

    def status(self):
        """The answering daemon's status (pid and hosted projects), or None; starts nothing."""
        done = self.fwd("daemon", "status", check=False)
        return json.loads(done.stdout) if done.returncode == 0 and done.stdout.startswith("{") else None

    def counts(self, projects):
        """Per project: tasks, relations and decisions, as the answering daemon reads them."""
        counts = {}
        for project in sorted(projects):
            graph = json.loads(self.fwd("--project", project, "graph").stdout)
            decisions = json.loads(self.fwd("--project", project, "decision", "list").stdout)
            counts[project] = (len(graph["tasks"]), len(graph["relations"]), len(decisions))
        return counts

    def start_daemon(self):
        """Starts `fwd daemon serve` as a launcher does (crates/fwd-core/src/daemon.rs,
        spawn_daemon): its own session, its output appended to the folder's daemon.log."""
        with open(self.data / "daemon.log", "ab") as log:
            return subprocess.Popen(
                [str(self.installed), "daemon", "serve", "--runtime-dir", str(self.runtime), "--data-dir", str(self.data)],
                env=self.env, stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True, close_fds=True,
                preexec_fn=unblock_stopping_signals,
            )

    def await_answer(self, progress):
        """The status of the daemon `progress` records once it answers, recorded as answered in the
        same uninterrupted step that recognises the answer; a failure when it exits or does not
        answer in time."""
        daemon = progress.daemon
        deadline = time.monotonic() + START_WAIT
        while time.monotonic() < deadline:
            if daemon.poll() is not None:
                raise Failure(f"the daemon it started exited with status {daemon.returncode}; see {self.data / 'daemon.log'}")
            with uninterrupted():
                status = self.status()
                if status is not None and status.get("pid") == daemon.pid:
                    progress.answered = True
                    return status
            time.sleep(0.2)
        raise Failure(f"the daemon it started did not answer within {START_WAIT} s; see {self.data / 'daemon.log'}")


class Lock:
    """An flock lock on one of the files fwd's daemon and launchers lock (crates/fwd-core,
    `Paths::bootstrap_lock` and `Paths::lifetime_lock`). The descriptor is not inherited, so no
    process this script starts holds it. Taking a lock this object holds already does nothing: a
    second descriptor's flock would conflict with the first, even in this process."""

    def __init__(self, path, name):
        self.path, self.name = path, name
        self.fd = None

    def take(self, wait):
        if self.fd is not None:
            return
        fd = os.open(self.path, os.O_RDWR | os.O_CREAT, 0o644)
        deadline = time.monotonic() + wait
        while True:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                self.fd = fd
                say(f"holding the {self.name} lock")
                return
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    os.close(fd)
                    raise Failure(f"the {self.name} lock at {self.path} is held by another process")
                time.sleep(0.1)

    def release(self):
        if self.fd is not None:
            fcntl.flock(self.fd, fcntl.LOCK_UN)
            os.close(self.fd)
            self.fd = None
            say(f"released the {self.name} lock")


class Progress:
    """What this run has switched so far, each flag set in the same uninterrupted step as the change
    it records (see `uninterrupted`), so that the undo does exactly what is needed."""

    def __init__(self):
        self.exchanged = False  # the data folder's path names the incoming folder
        self.rested = False  # the displaced folder has its resting name
        self.installed = False  # the installed path names the binary for the incoming folder
        self.daemon = None  # the daemon this run started
        self.answered = False  # that daemon has answered, so clients may have written to it


@contextmanager
def uninterrupted():
    """Holds SIGINT, SIGTERM and SIGHUP back while one change and the flag that records it are
    made; they are delivered, and raise, right after."""
    signal.pthread_sigmask(signal.SIG_BLOCK, STOPPING)
    try:
        yield
    finally:
        signal.pthread_sigmask(signal.SIG_UNBLOCK, STOPPING)


def stop_by_signal(signum, _frame):
    raise Failure(f"received {signal.Signals(signum).name}")


def forward(machine, staged, converter, backups):
    converted, old_rest = machine.resting(machine.data, NEW), machine.resting(machine.data, OLD)
    old_binary = machine.resting(machine.installed, OLD)
    for path in (converted, old_rest, old_binary):
        if path.exists():
            raise SystemExit(f"{path} exists already; move it aside first")
    for binary in (staged, converter):
        if run([str(binary), "--version"]).returncode != 0:
            raise SystemExit(f"{binary} does not run")

    def install(progress):
        incoming = machine.installed.with_name(f"{machine.installed.name}.incoming")
        shutil.copy2(staged, incoming)
        try:
            os.link(machine.installed, old_binary)
            with uninterrupted():
                os.replace(incoming, machine.installed)
                progress.installed = True
        except BaseException:
            for leftover in (incoming, old_binary):
                if leftover.exists() and not progress.installed:
                    leftover.unlink()
            raise

    switch(
        machine,
        back_up=lambda: back_up(machine, backups),
        make_incoming=lambda: convert(converter, machine.data, converted),
        install=install,
        uninstall=lambda: os.replace(old_binary, machine.installed),
        resting_folder=old_rest,
        incoming_folder=converted,
    )


def back(machine):
    old_rest, converted = machine.resting(machine.data, OLD), machine.resting(machine.data, NEW)
    old_binary, new_binary = machine.resting(machine.installed, OLD), machine.resting(machine.installed, NEW)
    for path in (old_rest, old_binary):
        if not path.exists():
            raise SystemExit(f"{path} is missing, so there is nothing to go back to")
    for path in (converted, new_binary):
        if path.exists():
            raise SystemExit(f"{path} exists already; move it aside first")

    def install(progress):
        os.link(machine.installed, new_binary)
        try:
            with uninterrupted():
                os.replace(old_binary, machine.installed)
                progress.installed = True
        except BaseException:
            if not progress.installed:
                new_binary.unlink()
            raise

    def uninstall():
        os.link(machine.installed, old_binary)
        os.replace(new_binary, machine.installed)

    switch(
        machine,
        back_up=lambda: None,
        make_incoming=lambda: None,
        install=install,
        uninstall=uninstall,
        resting_folder=converted,
        incoming_folder=old_rest,
    )


def back_up(machine, backups):
    say(f"backing up {machine.data} into {backups}")
    machine.fwd("admin", "backup", "--to", str(backups), "--keep", "100000")


def convert(converter, data, converted):
    say(f"converting {data} into {converted}")
    done = run([str(converter), "--from", str(data), "--to", str(converted)])
    for line in done.stdout.splitlines():
        say(line)
    if done.returncode != 0:
        raise Failure(f"the conversion failed: {first_line(done.stderr) or 'a store was refused'}")


def switch(machine, back_up, make_incoming, install, uninstall, resting_folder, incoming_folder):
    """Steps 0 to 10 for either direction: `make_incoming` makes `incoming_folder` from the data
    folder, which then rests as `resting_folder` while `incoming_folder` takes its place, and
    `install` puts the binary for it in place."""
    status = machine.status()
    if status is None:
        raise SystemExit(
            f"no daemon answers through {machine.runtime}: run any fwd command to start it, so that this script can "
            "confirm which folders it serves (or pass --runtime-dir)"
        )
    serving = run(["ps", "-o", "command=", "-p", str(status["pid"])]).stdout
    if f"--data-dir {machine.data}" not in serving:
        raise SystemExit(f"the daemon (pid {status['pid']}) that answers through {machine.runtime} does not name --data-dir {machine.data}: {serving.strip()}")
    projects = {project["id"] for project in status.get("projects", [])}
    before = machine.counts(projects)

    bootstrap = Lock(machine.runtime / "daemon.bootstrap.lock", "bootstrap")
    lifetime = Lock(machine.runtime / "daemon.lock", "lifetime")
    progress = Progress()
    for stopping in STOPPING:
        signal.signal(stopping, stop_by_signal)
    try:
        bootstrap.take(LOCK_WAIT)
        say("stopping the daemon")
        say(machine.fwd("daemon", "stop").stdout.strip())
        lifetime.take(0)
        back_up()
        snapshot = files_beside_stores(machine.data)
        make_incoming()
        now = files_beside_stores(machine.data)
        changed = sorted(str(path) for path in set(snapshot) | set(now) if snapshot.get(path) != now.get(path))
        if changed:
            raise Failure(f"these files changed during the conversion, so nothing is switched; run it again: {', '.join(changed)}")
        with uninterrupted():
            swap(machine.data, incoming_folder)
            progress.exchanged = True
        with uninterrupted():
            os.rename(incoming_folder, resting_folder)
            progress.rested = True
        say(f"{machine.data} is now the incoming folder; the previous one rests as {resting_folder}")
        install(progress)
        say(f"installed {machine.installed} ({machine.fwd('--version').stdout.strip()})")
        lifetime.release()
        with uninterrupted():
            progress.daemon = machine.start_daemon()
        status = machine.await_answer(progress)
        say(f"the daemon it started (pid {status['pid']}) answers")
        check_projects(machine, status, projects, before)
    except (Failure, OSError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        # A second signal must not cut short what follows.
        signal.pthread_sigmask(signal.SIG_BLOCK, STOPPING)
        if progress.answered:
            # Clients may have written to the new daemon since it answered: undoing now would
            # strand those writes in the resting folder. So the failure is reported, not undone.
            say(f"FAILED AFTER THE SWITCH: {error}")
            say(f"the new pair is in place and serving: {machine.data} with {machine.installed}; clients may already have written to it")
            say("going back is your decision: cutover.py back")
            bootstrap.release()
            sys.exit(1)
        say(f"FAILED: {error}")
        try:
            undo(machine, lifetime, progress, uninstall, resting_folder, incoming_folder)
        except (Failure, OSError, subprocess.SubprocessError) as undo_error:
            say(f"THE UNDO FAILED: {undo_error}")
            say(f"{paths_now(machine)}; README.md says how to finish or undo this by hand")
            lifetime.release()
            bootstrap.release()
            sys.exit(2)
        lifetime.release()
        bootstrap.release()
        say("nothing is switched over; the next fwd command starts the daemon of the installed build on the data folder")
        sys.exit(1)
    bootstrap.release()
    say("done")


def check_projects(machine, status, projects, before):
    hosted = {project["id"] for project in status.get("projects", [])}
    failed = [project for project in status.get("projects", []) if project.get("error")]
    if failed:
        raise Failure(f"the daemon it started could not open {len(failed)} project(s): " + "; ".join(project["error"] for project in failed))
    if hosted != projects:
        raise Failure(f"the daemon it started hosts {len(hosted)} projects, the previous one {len(projects)}: missing {sorted(projects - hosted)}, new {sorted(hosted - projects)}")
    say(f"it hosts the same {len(hosted)} projects, none with an error")
    after = machine.counts(hosted)
    shrank = []
    for project, counts in after.items():
        was = before[project]
        say(f"  {project}: tasks, relations, decisions {counts}" + ("" if was == counts else f" (before the stop: {was})"))
        if any(now < then for now, then in zip(counts, was)):
            shrank.append(project)
    if shrank:
        raise Failure(f"counts shrank in {', '.join(shrank)}")


def undo(machine, lifetime, progress, uninstall, resting_folder, incoming_folder):
    """Undoes what `progress` records, ending the daemon this run started first."""
    if progress.daemon is not None and progress.daemon.poll() is None:
        progress.daemon.terminate()
        try:
            progress.daemon.wait(10)
        except subprocess.TimeoutExpired:
            progress.daemon.kill()
            progress.daemon.wait()
        say(f"ended the daemon it started (pid {progress.daemon.pid})")
    if not progress.exchanged:
        return
    lifetime.take(30)
    if progress.installed:
        uninstall()
        progress.installed = False
    if progress.rested:
        os.rename(resting_folder, incoming_folder)
        progress.rested = False
    swap(machine.data, incoming_folder)
    progress.exchanged = False
    say(f"undone: {machine.data} and {machine.installed} are as they were; the folder that was to replace it is at {incoming_folder}")


def swap(a, b):
    """Exchanges two folders in one step, so the data folder's path never names nothing (a command
    would create an empty data folder there)."""
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.renamex_np(os.fsencode(a), os.fsencode(b), RENAME_SWAP) != 0:
        errno = ctypes.get_errno()
        raise OSError(errno, f"exchanging {a} and {b}: {os.strerror(errno)}")


def files_beside_stores(folder):
    """Every file in the data folder but the project stores, which only a daemon writes, by its
    SHA-256."""
    return {
        path.relative_to(folder): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(folder.rglob("*"))
        if path.is_file() and not (path.name == "project.sqlite" and path.parent.parent.name == "projects")
    }


def paths_now(machine):
    """Which of the names a switch moves things to exist now, and which build is installed."""
    names = [
        machine.resting(machine.data, OLD),
        machine.resting(machine.data, NEW),
        machine.resting(machine.installed, OLD),
        machine.resting(machine.installed, NEW),
        machine.installed.with_name(f"{machine.installed.name}.incoming"),
    ]
    present = [str(name) for name in names if name.exists()]
    try:
        installed = machine.fwd("--version", check=False).stdout.strip() or "a build that does not run"
    except OSError as error:
        installed = f"unreadable ({error})"
    return f"present now: {', '.join(present) or 'none of the resting names'}; {machine.installed} is {installed}"


def run(args, **kwargs):
    return subprocess.run(args, capture_output=True, text=True, preexec_fn=unblock_stopping_signals, **kwargs)


def unblock_stopping_signals():
    """Runs in each child before it executes. A child inherits the signals this script holds back
    at that moment, and keeps them held back for its whole life: the daemon it starts would then
    ignore a plain `kill`, and its own shutdown on SIGTERM would never run."""
    signal.pthread_sigmask(signal.SIG_UNBLOCK, STOPPING)


def first_line(text):
    return next((line for line in text.splitlines() if line.strip()), "")


def say(line):
    """Prints a line of progress, and never raises: an output that closed or filled must not stop
    the undo."""
    now = time.time()
    try:
        print(f"[{time.strftime('%H:%M:%S', time.localtime(now))}.{int(now % 1 * 1000):03d}] {line}", flush=True)
    except (OSError, ValueError):
        pass


if __name__ == "__main__":
    main()
