#!/usr/bin/env bash
# Build the wallboard (docs/design/wallboard.md): crates/fwd-web with its `wallboard` feature, from
# wallboard.html, into the bundle in deploy/wallboard: dist/, with its NOTICE.txt, and build-info.json
# and LICENSE beside the launcher. It builds neither the daemon nor the page the daemon embeds
# (scripts/build-web.sh). Extra arguments go to trunk. Needs git, python3, trunk and the wasm32 target.
# WALLBOARD_REMAP_PATHS=1 makes an official build: the builder's paths are taken out of the
# WebAssembly, where its panic messages would otherwise carry them, and the build fails if any is left.
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
bundle="$repo/deploy/wallboard"
if [ "${WALLBOARD_REMAP_PATHS:-}" = 1 ]; then
    # Cargo takes either of these in place of the target's flags, and would drop the remapping and
    # .cargo/config.toml's flags with them, so an official build refuses them before it writes anything.
    for variable in RUSTFLAGS CARGO_ENCODED_RUSTFLAGS; do
        if [ -n "${!variable+set}" ]; then
            echo "WALLBOARD_REMAP_PATHS=1 refuses $variable: Cargo would take it in place of the remapping and of .cargo/config.toml's flags" >&2
            exit 1
        fi
    done
    cargo_home="${CARGO_HOME:-$HOME/.cargo}"
    # Cargo splits these flags on whitespace, and joins them to .cargo/config.toml's for the target.
    for path in "$HOME" "$cargo_home" "$repo"; do
        if [[ "$path" =~ [[:space:]] ]]; then
            echo "WALLBOARD_REMAP_PATHS cannot remap a path with whitespace in it: $path" >&2
            exit 1
        fi
    done
    # The last prefix that matches wins, so the narrower ones come after $HOME.
    export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS="--remap-path-prefix=$HOME=home --remap-path-prefix=$cargo_home=cargo --remap-path-prefix=$repo=fwdcentaur"
fi
if ! command -v trunk >/dev/null; then
    echo "trunk is not installed: cargo install trunk --version 0.21.14 --locked" >&2
    exit 1
fi

# Before trunk runs, the two modules the runtime imports that the build writes: the settings contract
# as JavaScript, and what this build is (scripts/wallboard_identity.py), which build-info.json records
# for the bundle once the page is built.
python3 - "$bundle/wallboard-contract.json" "$repo/crates/fwd-web/src/wallboard/wallboard-contract.mjs" <<'PY'
import json
from pathlib import Path
import sys

contract = json.loads(Path(sys.argv[1]).read_text())
Path(sys.argv[2]).write_text('// Generated from deploy/wallboard/wallboard-contract.json by scripts/build-wallboard.sh.\n'
                             f'export const contract = {json.dumps(contract)};\n')
PY
identity="$(python3 "$repo/scripts/wallboard_identity.py" identify "$repo")"

cd "$repo/crates/fwd-web"
# A target directory of its own: trunk keeps wasm-bindgen's output in the target directory and copies
# every module it finds there into the page it builds, so the page the daemon embeds would otherwise
# carry this build's modules.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo/crates/fwd-web/target}/wallboard"
# wasm-bindgen leaves the modules of earlier builds in its output, and trunk copies all it finds there
# into the page, so it starts empty: the bundle carries this build's modules alone.
rm -rf "$CARGO_TARGET_DIR/wasm-bindgen"
# Locked, so that the build reads the lockfile its identity hashed, as it was hashed.
NO_COLOR=true trunk build wallboard.html --release=true --locked=true --dist wallboard-dist "$@"
if [ "${WALLBOARD_REMAP_PATHS:-}" = 1 ]; then
    # What the remapping is for: the WebAssembly names none of the builder's directories. grep exits 1
    # when it finds none, and 2 when it cannot read the WebAssembly.
    for path in "$HOME" "$cargo_home" "$repo"; do
        LC_ALL=C grep -aqF -- "$path/" wallboard-dist/*.wasm && found=0 || found=$?
        case $found in
            0) echo "the wallboard's WebAssembly still names $path, a directory of its builder" >&2; exit 1 ;;
            1) ;;
            *) exit "$found" ;;
        esac
    done
fi
# The old identity goes with the old dist, so that a build that fails from here on leaves no
# build-info.json rather than an earlier build's.
rm -f "$bundle/build-info.json"
rm -rf "$bundle/dist"
cp -R wallboard-dist "$bundle/dist"
cp -R "$bundle/sounds" "$bundle/dist/"
# wasm-bindgen copies only the modules Rust imports; the modules they import, and SunCalc's license,
# go beside them.
for module in "$bundle"/dist/snippets/fwd-web-*/src/wallboard/wallboard-runtime.mjs; do
    cp src/wallboard/*.mjs src/wallboard/LICENSE-suncalc.txt src/wallboard/SUNCALC.md "$(dirname "$module")/"
done
cp "$repo/LICENSE" "$bundle/LICENSE"
# Who made the bundle, where its source is, and the licenses of what others made in it.
python3 "$repo/scripts/wallboard_notices.py" "$repo" "$bundle/dist/NOTICE.txt"
# Last, the identity, once the page is built and in place, and only for a dist that carries it.
python3 "$repo/scripts/wallboard_identity.py" record "$bundle" "$identity"
printf 'built the wallboard bundle in %s\n' "$bundle"
