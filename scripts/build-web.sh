#!/usr/bin/env bash
# Build the web UI (crates/fwd-web, Leptos -> wasm) into crates/fwd-web/dist, which the next
# `cargo build` of `fwd` embeds (crates/fwd/build.rs, crates/fwd/src/serve.rs). Extra arguments
# go to trunk. Needs trunk (`cargo install trunk --locked`) and the wasm32 target.
set -euo pipefail
cd "$(dirname "$0")/../crates/fwd-web"
if ! rustup target list --installed | grep -q '^wasm32-unknown-unknown$'; then
    rustup target add wasm32-unknown-unknown
fi
if ! command -v trunk >/dev/null; then
    echo "trunk is not installed: cargo install trunk --locked" >&2
    exit 1
fi
trunk build --release "$@"
echo "built crates/fwd-web/dist; now: cargo build -p fwd"
