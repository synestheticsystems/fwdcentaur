#!/usr/bin/env bash
# Build the wallboard (docs/design/wallboard.md): crates/fwd-web with its `wallboard` feature, from
# wallboard.html, into the bundle in deploy/wallboard: dist/, build-info.json and LICENSE beside the
# launcher. It builds neither the daemon nor the page the daemon embeds (scripts/build-web.sh).
# Extra arguments go to trunk. Needs git, python3, trunk and the wasm32 target.
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
bundle="$repo/deploy/wallboard"
if ! command -v trunk >/dev/null; then
    echo "trunk is not installed: cargo install trunk --locked" >&2
    exit 1
fi

# Before trunk runs, the two modules the runtime imports that the build writes: the settings contract
# as JavaScript, and what this build is (scripts/wallboard_identity.py), which build-info.json records
# for the bundle too.
python3 - "$bundle/wallboard-contract.json" "$repo/crates/fwd-web/src/wallboard-contract.mjs" <<'PY'
import json
from pathlib import Path
import sys

contract = json.loads(Path(sys.argv[1]).read_text())
Path(sys.argv[2]).write_text('// Generated from deploy/wallboard/wallboard-contract.json by scripts/build-wallboard.sh.\n'
                             f'export const contract = {json.dumps(contract)};\n')
PY
python3 "$repo/scripts/wallboard_identity.py" "$repo"

cd "$repo/crates/fwd-web"
# A target directory of its own: trunk keeps wasm-bindgen's output in the target directory and copies
# every module it finds there into the page it builds, so the page the daemon embeds would otherwise
# carry this build's modules.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo/crates/fwd-web/target}/wallboard"
NO_COLOR=true trunk build wallboard.html --release=true --dist wallboard-dist "$@"
rm -rf "$bundle/dist"
cp -R wallboard-dist "$bundle/dist"
cp -R "$bundle/sounds" "$bundle/dist/"
# wasm-bindgen copies only the modules Rust imports; the modules they import, and SunCalc's license,
# go beside them.
for module in "$bundle"/dist/snippets/fwd-web-*/src/wallboard-runtime.mjs; do
    cp src/wallboard-*.mjs src/suncalc.mjs src/LICENSE-suncalc.txt src/SUNCALC.md "$(dirname "$module")/"
done
cp "$repo/LICENSE" "$bundle/LICENSE"
printf 'built the wallboard bundle in %s\n' "$bundle"
