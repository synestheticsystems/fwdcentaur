# The Linux machine scripts/test-extension-headless.sh runs the VS Code extension's suite on, so
# that the suite's VS Code window opens on a virtual display (Xvfb) inside a container instead of
# on a person's desktop. The script builds it from this file and rust-toolchain.toml alone, so the
# image holds the toolchain the repository pins, and moving the pin rebuilds the image.
#
# Node 24 is the Node that VS Code runs extensions on (docs/design/vscode-extension.md); wasm-pack
# and trunk are the versions the extension job in .github/workflows/ci.yml installs.
FROM node:24-bookworm

# xvfb-run (the xvfb package) and the xauth it calls; `ps`, which the suite asks whether a scratch
# daemon is left; rsync, which copies the checkout in; and the libraries the Linux build of VS Code
# (an Electron app) loads, as VS Code's guide to running extension tests in CI lists them.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      xvfb xauth procps rsync \
      libnss3 libgtk-3-0 libasound2 libxss1 libgbm1 libxshmfence1 libdrm2 libxkbfile1 libsecret-1-0 \
 && rm -rf /var/lib/apt/lists/*

# Rust as rust-toolchain.toml pins it: the channel, its components and the wasm32 target.
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
COPY rust-toolchain.toml /usr/local/pin/
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal --default-toolchain none \
 && cd /usr/local/pin && rustup toolchain install \
 && chown -R node:node /usr/local/rustup /usr/local/cargo

ARG WASM_PACK=0.14.0
ARG TRUNK=0.21.14
RUN arch="$(uname -m)" \
 && curl -sSfL "https://github.com/wasm-bindgen/wasm-pack/releases/download/v${WASM_PACK}/wasm-pack-v${WASM_PACK}-${arch}-unknown-linux-musl.tar.gz" \
    | tar -xz -C /usr/local/bin --strip-components=1 "wasm-pack-v${WASM_PACK}-${arch}-unknown-linux-musl/wasm-pack" \
 && curl -sSfL "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK}/trunk-${arch}-unknown-linux-gnu.tar.gz" \
    | tar -xz -C /usr/local/bin trunk

# The folders the named volumes mount on, owned by the user the run runs as, so that a new volume
# starts out writable by that user: the copy of the checkout, and the home folder the base image has
# (cargo's registry, the wasm-bindgen and wasm-opt builds wasm-pack and trunk download, npm's cache).
RUN mkdir -p /work/fwdcentaur && chown node:node /work/fwdcentaur
USER node
ENV CARGO_HOME=/home/node/.cargo
WORKDIR /work/fwdcentaur
