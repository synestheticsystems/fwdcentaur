# The wallboard, built and run by hand

The wallboard shows one project's task graph on a dedicated monitor, all day, with no one at the
keyboard: the work in the statuses you choose, what waits on what, and what changed recently. This
directory holds what runs beside the display: the launcher (`wallboard.py`), its settings contract,
an example configuration, an example systemd unit and the sound cues, with the wallboard's checks in
`checks/`. The browser page itself is the web crate built with its `wallboard` feature
(`crates/fwd-web`).

The page is an ephemeral Ankurah browser node, like the web UI the daemon serves. It reads one
project from a daemon that already runs, through a proxy the launcher configures, and writes
nothing. It keeps its last view while the daemon is out of reach and joins again when the daemon
answers. No hosted service is involved. How it works and why: [docs/design/wallboard.md](../../docs/design/wallboard.md).

Everything here is open: the client, the settings contract, the launcher and this manual route.
Hosted wallboard URLs, the enrollment of a display, official builds and device packages are the
paid convenience (`docs/DECISIONS.md`, D144, open-source the wallboard client).

## What you need

To build, on any machine with a checkout of this repository:

- Git and Python 3.
- Rust at the version `rust-toolchain.toml` pins, with the `wasm32-unknown-unknown` target: rustup
  installs both from that file.
- [Trunk](https://trunkrs.dev) 0.21.14: `cargo install trunk --version 0.21.14 --locked`. Its first
  build downloads the wasm-bindgen and wasm-opt versions it needs into its own cache; leave
  `TRUNK_OFFLINE` unset, or it cannot use them.

To run the checks as well: Node 22 or newer, with npm. The browser checks use Playwright 1.62.1, as
`checks/package-lock.json` locks it, and the Chromium build that Playwright version pins.

To run the wallboard, on the machine attached to the display:

- Python 3 and [Caddy](https://caddyserver.com) 2. The launcher starts its own unprivileged Caddy,
  so a Caddy installed as a system service (Debian's `caddy` package starts one) can be stopped and
  disabled.
- A browser that can run full screen or as a kiosk, such as Chromium.
- No Rust, Node, `fwd` binary or daemon. The built assets are WebAssembly and JavaScript, the same
  on every architecture: a Raspberry Pi runs what a laptop built.

And a daemon that hosts the project, reachable from the display machine: on the same machine, or
on another one through an SSH tunnel (Connecting to the daemon, below).

## Building

From the repository's root:

```sh
bash scripts/build-wallboard.sh
```

It builds `crates/fwd-web` with the `wallboard` feature, from `crates/fwd-web/wallboard.html`, and
assembles the bundle here: `dist/` (the page, its WebAssembly, its JavaScript modules, the sounds,
and `NOTICE.txt`, which names who made the bundle, where its source is, and the licenses of what
others made in it, with their texts), `build-info.json` (the commit it was built from and a hash of
everything it was built from) and `LICENSE`, beside the launcher. It writes `build-info.json` last,
and only for a `dist/` that carries the identity it records, so a build that fails leaves the bundle
it found or a `dist/` with no `build-info.json`, never a page under another build's identity. It
builds locked, never updating the lockfile, and builds neither the daemon nor the page the daemon
embeds (`scripts/build-web.sh`).
`WALLBOARD_REMAP_PATHS=1` builds as an official build does: it keeps the paths of your home,
Cargo's and the checkout out of the WebAssembly, which a bundle built for others to run should not
name, and fails if any is left. It refuses `RUSTFLAGS` and `CARGO_ENCODED_RUSTFLAGS`, which Cargo
would take in place of the remapping.

To check it, with synthetic data only and never a daemon:

```sh
python3 scripts/check-wallboard.py --install-browser
```

The first run installs the locked Playwright and downloads its Chromium into
`target/wallboard/ms-playwright`, never Playwright's user-wide cache; later runs can leave out
`--install-browser`. `CHROME=/path/to/chromium` runs the browser checks in another Chromium. The
script builds the bundle itself, and writes its report to `target/wallboard/checks.json` as it
goes: `passed` stays false until the last check has passed. What each check covers is in
[docs/design/wallboard.md](../../docs/design/wallboard.md), Checks.

## Configuring

Copy `wallboard.example.json` to `wallboard.json` beside it and edit the copy. The launcher reads
the `wallboard.json` beside itself, or the file `--config` names, and serves the `dist/` beside that
file. The copy is ignored by Git: it names a real project, so keep it out of the repository, and
keep it when you update the wallboard.

| Field | Meaning |
| --- | --- |
| `project_id` | Required. The UUID of the project to show, across all its scopes. In one of the project's repositories, `fwd daemon status` prints it as `project`. |
| `statuses` | The statuses to show, from `proposed`, `open`, `in_progress` and `in_review`; `["in_progress"]` by default. Done, cancelled and rejected tasks are never shown. |
| `max_rows` | The most cards in one column, from 3 to 16; 8 by default. More cards make more columns, so no task is dropped. |
| `node_width` | A card's width at 100% text size, from 240 to 640; 360 by default. |
| `text_scale_percent` | The text size, from 80 to 160; 100 by default. Cards, padding and badges grow with the text, so a card shows as much of its task as before. |
| `fit_to_screen` | `false` by default. When `true`, the wallboard tries shorter, wider arrangements, at most `max_rows` a column, and uses one when it makes the cards larger on the screen. |
| `theme` | The palette: `dark-aubergine` (the default), `charcoal`, `oxblood-copper`, `indigo-saffron` or `graphite-coral` on black, or `linen-indigo`, `porcelain-coral` or `sand-plum` on a light background. The operating system's light or dark setting does not change it. |
| `theme_schedule` | A light palette by day and a dark one by night, switched at the local sunrise and sunset; below. Off by default. |
| `replay.enabled` | Replay of the recent changes this display saw, every so often; `true` by default. |
| `replay.interval_seconds` | How often replay runs, from 60 to 3600 seconds; 180 by default. |
| `sound.enabled` | A short sound for each batch of live changes; `false` by default. Below. |
| `sound.volume` | Its level from 0 to 100; 30 by default. The machine's and the monitor's own volume apply as well. |
| `listen.host`, `listen.port` | Required. Where the launcher serves the page, `127.0.0.1:3000` in the example. The host must be a loopback address. |
| `upstream.host`, `upstream.port` | Required. Where the launcher's proxy reaches the daemon: an IP address and a port, normally on loopback, the daemon's own or a tunnel's (below). |
| `daemon_origin` | Required. The daemon's own address, `http://127.0.0.1:<port>`, as the daemon knows it. The proxy presents it as the Host and Origin of the one connection it passes on, which the daemon checks. |

The example's port, 12345, stands in for the daemon's: `fwd daemon status` on the daemon's machine
prints it as `port`. The launcher has no default for the four required fields, and stops with a
sentence naming one that is missing. Unknown keys are ignored, a missing `schema_version` means
version 1, and a value the contract does not allow stops the launcher with the field it refused.

For the day and night palettes, give the display's position; city-level precision is enough. These
coordinates are Greenwich's:

```json
"theme_schedule": {
  "enabled": true,
  "latitude": 51.48,
  "longitude": 0,
  "day_theme": "linen-indigo",
  "night_theme": "indigo-saffron"
}
```

`day_theme` is one of the three light palettes and `night_theme` one of the five dark ones. A
schedule that is enabled without both coordinates is refused rather than guessed. The times come
from the SunCalc library bundled with the page, with no location or weather service.

## Connecting to the daemon

The daemon listens on loopback, on the port it had last time when that port is free and on another
otherwise. It runs until it is stopped, with `fwd daemon stop` or a signal, and a restart may give
it another port, so check `fwd daemon status` after one and correct `upstream`, or the tunnel, and
`daemon_origin`. The wallboard never starts it, so start it with `fwd daemon start`, or any `fwd`
command, on its machine.

On the same machine as the daemon, `upstream` and `daemon_origin` name the same port:
`"upstream": {"host": "127.0.0.1", "port": <port>}` and `"daemon_origin": "http://127.0.0.1:<port>"`.

On another machine, forward a port on the display machine's loopback to the daemon's, from the
daemon's machine:

```sh
ssh -NT -o ExitOnForwardFailure=yes -o ServerAliveInterval=30 -o ServerAliveCountMax=3 \
  -R 127.0.0.1:<upstream port>:127.0.0.1:<daemon port> <user>@<display host>
```

`upstream` names the forwarded port on the display machine and `daemon_origin` the daemon's own,
which can differ. The daemon's machine needs no SSH server and no listener on the network. Do not
add `-g` or forward to a non-loopback address: the daemon's policy permits any client that reaches
it, so the proxy and the tunnel stay on loopback.

## Running

Check the configuration, then run the launcher, from this directory:

```sh
python3 wallboard.py --check
python3 wallboard.py
```

It validates `wallboard.json`, writes `runtime/caddy.json` beside it and replaces itself with
Caddy (`--caddy` names another Caddy binary). Caddy serves the page and its assets, serves the
settings at `/wallboard-config.json`, and passes on exactly one WebSocket, the configured project's,
with the Host and Origin the daemon expects. Every other path, the daemon's control routes, MCP and
project list included, answers 404, and so does a request naming another host or a WebSocket from
another origin.

Then open `http://127.0.0.1:3000`, or your `listen` address, in the browser, full screen or as a
kiosk. For sound, Chromium needs `--autoplay-policy=no-user-gesture-required`, since the wallboard
has no control to click to allow audio:

```sh
chromium --kiosk --autoplay-policy=no-user-gesture-required http://127.0.0.1:3000
```

To run the launcher as a service, copy `dist/` and the files `wallboard.py`, `settings.py`,
`wallboard-contract.json`, `build-info.json`, `wallboard-launcher.service`, `README.md` and
`LICENSE` to `/srv/wallboard/current`, with your `wallboard.json`, owned by the user who runs the
display, and install `wallboard-launcher.service` as a user unit there:

```sh
cd /srv/wallboard/current
python3 wallboard.py --check
mkdir -p ~/.config/systemd/user
cp wallboard-launcher.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now wallboard-launcher.service
loginctl enable-linger "$USER"   # so that it starts at boot, before anyone logs in
```

To update, build again and copy the whole `dist/` tree and the files `wallboard.py`, `settings.py`,
`wallboard-contract.json`, `build-info.json`, `README.md` and `LICENSE` over the installed ones,
keeping `wallboard.json`. Then restart `wallboard-launcher.service`, which serves only the files it
found when it started, and reload the page. A configuration change needs the same restart and
reload.

## When something is wrong

- **"Waiting for graph…" stays.** The page has its configuration but no answer from the daemon.
  Check that the daemon runs (`fwd daemon status` on its machine), that `upstream` and
  `daemon_origin` name its current port, and that the tunnel is up when there is one. The page
  joins on its own once the daemon answers.
- **The page does not load.** The launcher is not running, or refused its configuration:
  `python3 wallboard.py --check` says why, and `journalctl --user -u wallboard-launcher.service`
  shows the service's log.
- **OFFLINE, Last view.** The connection dropped after the wallboard had joined. It keeps the same
  page, node and last view and retries, at intervals growing from half a second to ten seconds; no
  reload is needed. A browser restarted while the daemon is out of reach waits for it: the page
  keeps no copy of the graph that outlives it. A daemon whose project was set up again, rather than
  out of reach for a while, can need the page reloaded.
- **STALE, Last view.** A frame of the graph could not be drawn, or a settings change was refused;
  the last good view stays until a frame can be. `diagnosticsFor` names the failure with no task
  content ([docs/design/wallboard.md](../../docs/design/wallboard.md), Diagnostics).
- **No sound.** Set `sound.enabled`, start Chromium with the autoplay flag above, and check the
  machine's output: on a Raspberry Pi with PipeWire, `wpctl status` lists the outputs, `wpctl
  set-default <id>` chooses the HDMI one (the default may be the headphone jack) and `wpctl
  get-volume @DEFAULT_AUDIO_SINK@` reads its level. The monitor must accept audio over HDMI and have
  a working speaker.
- **A pointer shows.** The page hides it over every element. Under labwc, the compositor of
  Raspberry Pi OS's desktop, this rule in the session's `rc.xml` hides it before the page loads too;
  reload labwc's configuration and restart the browser:

  ```xml
  <windowRules>
    <windowRule identifier="chromium*">
      <action name="HideCursor" />
    </windowRule>
  </windowRules>
  ```

- **Old changes replay.** Replay plays back what this browser saw, kept in its local storage, which
  can hold task titles and excerpts. Clearing the site's data in the browser removes it.
