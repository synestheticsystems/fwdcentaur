#!/usr/bin/env bash
# Runs the VS Code extension's suite (`npm test` in extensions/vscode) with VS Code on a virtual
# display, so that no window opens on a person's desktop and nothing takes the focus. VS Code has
# no headless mode on macOS, so by default the suite runs in a Linux container under Xvfb:
#
#   scripts/test-extension-headless.sh          the suite in a container (Docker), from any checkout
#   scripts/test-extension-headless.sh --here   the suite in this checkout under xvfb-run, on Linux:
#                                               what the container runs, and what CI runs
#
# Either way it builds what the suite needs first (the page, which the `fwd` binary embeds and the
# extension bundles; the `fwd` binary; the extension host's node), then runs `xvfb-run -a npm
# test`, and exits with the suite's exit code; the output ends with the suite's own.
#
# The run ends in this order (extensions/vscode/.vscode-test.mjs, npm test's runner): the suite
# checks that no `fwd` call the extension started still runs; VS Code exits, and with it the
# extension host; then the runner stops the scratch daemon, makes a `fwd --no-autostart` call right
# after the stop, which the CLI refuses, and fails the run if `ps` still finds a scratch daemon.
# The stop comes last because a `fwd` call starts a daemon when none runs: on CI, when the suite
# stopped the daemon before the extension host had exited, a call the window made afterwards
# started a new one (task lK-_4TEE).
#
# When VS Code exits before the suite has started, no test has run: the runner, once it has stopped
# the scratch daemon as above, exits with 75, and this script launches the suite again, with a new
# scratch project and daemon, twice at most, saying so in the output; when the third launch ends so
# as well, the script exits with 75. On CI, VS Code exited so when its window did not answer for 15
# seconds while it loaded, a load that took under a second on some runners and more than 15 seconds
# on others (task EyLMg55J). A launch that got as far as the suite is never launched again, so that
# no failing test runs a second time.
#
# The container sees this checkout read-only, and git's object store read-only, so that the build
# stamps the commit the checkout is on, which the suite checks. While crates/fwd-host takes
# ankurah-storage-sqlite-node by path (../ankurah-storage-node-sqlite beside the checkout), it also
# sees that Ankurah worktree read-only: $ANKURAH_STORAGE_NODE_SQLITE, else the one beside this
# checkout, else the one beside the main checkout when this checkout is a worktree of it. The
# container copies the files git lists here (tracked, and untracked but not ignored) into a copy of
# its own, writing only the files whose content changed, so that cargo, which goes by file times,
# rebuilds what changed and nothing else, also when the runs come from different checkouts. That
# copy and the build outputs in it (the target folders, node_modules, the VS Code build that
# test-electron downloads) stay on the named volume fwd-extension-suite-work, and cargo's registry
# and the downloads of the tools (wasm-bindgen, wasm-opt) on fwd-extension-suite-home; removing
# both (`docker volume rm fwd-extension-suite-work fwd-extension-suite-home`) makes the next run
# start from nothing. The runs share those volumes, so one runs at a time: the container is named
# fwd-extension-suite, and a run that finds it running waits for it to finish.
#
# On Daniel's machine (Apple silicon, Docker Desktop with 16 CPUs), the first run from nothing (no
# image, no volumes; the base image pulled already) took 10 minutes: the image 20 seconds, the page
# a minute and a half, `fwd` 30 seconds, the host's node 6 minutes (5 of them wasm-opt), the VS Code
# download (316 MB) 40 seconds and the suite under a minute. A second run took 63 seconds.
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"

if [ "${1:-}" = --here ]; then
    if ! command -v xvfb-run >/dev/null; then
        echo "--here needs xvfb-run (Linux, the xvfb package); elsewhere run this script without it" >&2
        exit 2
    fi
    cd "$repo"
    bash scripts/build-web.sh
    cargo build -p fwd
    # The extension host's node. After cargo, wasm-pack passes the library through wasm-bindgen and
    # wasm-opt, which takes minutes, so it runs only when cargo has built the library again since
    # pkg/ was made from it (package.json is the last file wasm-pack writes there).
    cd crates/fwd-host
    cargo build --lib --release --target wasm32-unknown-unknown
    lib=target/wasm32-unknown-unknown/release/fwd_host.wasm
    if ! [ -f "$lib" ] || ! [ pkg/package.json -nt "$lib" ]; then
        wasm-pack build --target nodejs
    fi
    cd ../../extensions/vscode
    npm install --no-audit --no-fund
    # The suite, launched again, twice at most, when VS Code exited before it started (the runner's
    # exit code 75; the top of this file says why).
    for launch in 1 2 3; do
        status=0
        xvfb-run -a npm test || status=$?
        if [ $status -ne 75 ] || [ $launch -eq 3 ]; then exit $status; fi
        echo "launch $launch of 3 ended before the suite started, so no test ran: launching the suite again"
        if [ "${GITHUB_ACTIONS:-}" = true ]; then
            echo "::warning title=Suite launched again::launch $launch of 3 ended before the suite started, so no test ran"
        fi
    done
fi

if [ "${1:-}" = --in-container ]; then
    # The container's first step: bring its copy of the checkout up to date with /src, from the
    # names on stdin (NUL-separated), then run the suite in the copy. $2 is the checkout's commit.
    cd /work/fwdcentaur
    [ -d .git ] || git -c init.defaultBranch=main init -q
    echo /src-objects >.git/objects/info/alternates # the checkout's commits, read where they are
    cat >.git/copied-files.new
    exec </dev/null
    # Without --times, a file whose content differs is written with the current time, so cargo
    # rebuilds what it feeds; a file whose content is the same (--checksum) is left as it is.
    rsync --links --perms --checksum --from0 --files-from=.git/copied-files.new /src/ ./
    # A file an earlier run copied that this run does not list has left the checkout, or is ignored.
    if [ -f .git/copied-files ]; then
        LC_ALL=C comm -z -23 <(LC_ALL=C sort -z .git/copied-files) <(LC_ALL=C sort -z .git/copied-files.new) |
            xargs -0 -r rm -f --
    fi
    mv .git/copied-files.new .git/copied-files
    # The copy is on the checkout's commit, with that commit's tree as its index, so the version the
    # build stamps names the commit, and says whether the files differ from it, as it would there.
    git update-ref HEAD "$2"
    git read-tree HEAD
    # The compiled tests: one compiled from a test file that only another checkout has would run too.
    rm -rf extensions/vscode/out
    exec bash scripts/test-extension-headless.sh --here
fi

if ! command -v docker >/dev/null; then
    echo "needs Docker: the docker command, and its daemon running" >&2
    exit 2
fi
cd "$repo"
image=fwd-extension-suite
mounts=(
    --mount "type=bind,src=$repo,dst=/src,readonly"
    --mount "type=bind,src=$(git rev-parse --path-format=absolute --git-path objects),dst=/src-objects,readonly"
    --mount "type=volume,src=fwd-extension-suite-work,dst=/work/fwdcentaur"
    --mount "type=volume,src=fwd-extension-suite-home,dst=/home/node"
)
if grep -q '"\.\./\.\./\.\./ankurah-storage-node-sqlite/' crates/fwd-host/Cargo.toml; then
    main="$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")"
    sibling=
    for dir in "${ANKURAH_STORAGE_NODE_SQLITE:-}" "$repo/../ankurah-storage-node-sqlite" "$main/../ankurah-storage-node-sqlite"; do
        if [ -n "$dir" ] && [ -d "$dir/storage/sqlite-node" ]; then
            sibling="$(cd "$dir" && pwd -P)"
            break
        fi
    done
    if [ -z "$sibling" ]; then
        echo "crates/fwd-host takes ankurah-storage-sqlite-node by path from ../ankurah-storage-node-sqlite, and" >&2
        echo "no such folder is beside this checkout or beside $main: set ANKURAH_STORAGE_NODE_SQLITE to it" >&2
        exit 2
    fi
    mounts+=(--mount "type=bind,src=$sibling,dst=/work/ankurah-storage-node-sqlite,readonly")
fi

# The base image is public, so it is pulled with no credentials when it is missing: inside an
# agent's sandbox, the credential helper Docker Desktop sets up hangs, and the build's own pull with
# it.
base="$(awk '$1 == "FROM" { print $2; exit }' scripts/test-extension-headless.Dockerfile)"
if ! docker image inspect "$base" >/dev/null 2>&1; then
    anonymous="$(mktemp -d)"
    DOCKER_HOST="$(docker context inspect --format '{{.Endpoints.docker.Host}}')" DOCKER_CONFIG="$anonymous" \
        docker pull --quiet "$base" >/dev/null
    rm -rf "$anonymous"
fi
# The image, which docker builds again only when the Dockerfile or the toolchain pin has changed.
if docker image inspect "$image" >/dev/null 2>&1; then
    progress=quiet
else
    progress=auto
    echo "building the image $image, which takes a few minutes the first time" >&2
fi
# The build context, rust-toolchain.toml and the Dockerfile, as a compressed archive: docker reads
# stdin as a Dockerfile unless its first kilobyte shows an archive, and the extended attributes
# macOS's tar records push the first file's header past that.
COPYFILE_DISABLE=1 tar --no-xattrs -czf - rust-toolchain.toml -C scripts test-extension-headless.Dockerfile |
    docker build --progress="$progress" --tag "$image" --file test-extension-headless.Dockerfile - >/dev/null

# The files to copy in, listed once: the tracked ones and the untracked ones git does not ignore,
# leaving out any deleted here.
files=$(mktemp "${TMPDIR:-/tmp}/fwd-extension-suite-files.XXXXXX")
trap 'rm -f "$files"' EXIT
git ls-files -z --cached --others --exclude-standard |
    while IFS= read -r -d '' file; do
        if [ -e "$file" ] || [ -L "$file" ]; then printf '%s\0' "$file"; fi
    done >"$files"

# One run at a time: the container is named, so a second run cannot start while one is in it.
# Wait for a run already there to stop and for docker to remove it (--rm), which frees the name;
# then try to start. Several waiters may wake at once and only one gets the name, so a start
# refused for the name goes back to waiting, for up to an hour in all.
started=$(date +%s)
while :; do
    if docker container inspect fwd-extension-suite >/dev/null 2>&1; then
        echo "waiting for the run already in the container fwd-extension-suite to finish" >&2
        docker wait fwd-extension-suite >/dev/null 2>&1 || true
        for _ in 1 2 3 4 5 6 7 8 9 10; do
            docker container inspect fwd-extension-suite >/dev/null 2>&1 || break
            sleep 1
        done
    fi
    # docker's own refusal comes on stderr before anything runs; the run's stderr follows it there
    # once the container is up, so both are kept in a file and printed after, in order. The status
    # is taken with `||`, since `set -e` would otherwise end the script right here when docker
    # refuses the name or the run fails: before the refusal is tried again, and before the kept
    # stderr is printed.
    refused=$(mktemp "${TMPDIR:-/tmp}/fwd-extension-suite-start.XXXXXX")
    status=0
    docker run --rm --interactive --init --name fwd-extension-suite --shm-size=2g "${mounts[@]}" "$image" \
        bash /src/scripts/test-extension-headless.sh --in-container "$(git rev-parse HEAD)" <"$files" 2>"$refused" ||
        status=$?
    cat "$refused" >&2
    if [ $status -eq 125 ] && grep -q 'container name "/fwd-extension-suite" is already in use' "$refused"; then
        rm -f "$refused"
        if [ $(( $(date +%s) - started )) -gt 3600 ]; then
            echo "gave up after an hour waiting for the container fwd-extension-suite" >&2
            exit 125
        fi
        sleep $((RANDOM % 5 + 1))
        continue
    fi
    rm -f "$refused"
    exit $status
done
