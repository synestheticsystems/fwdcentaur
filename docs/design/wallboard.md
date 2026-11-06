# The wallboard: one project's graph on a dedicated display

## What it is for

A team that works from a shared graph wants the state of the work in view without anyone opening
it: on a monitor on the wall, all day, with no keyboard, the tasks in progress, what waits on what,
and what changed while nobody was looking. The wallboard is that display. It reads one project from
a daemon that already runs and writes nothing; when the daemon is out of reach it keeps its last
view, and it joins again when the daemon answers.

Its client is open and built and installed by its operator; hosted wallboard URLs, the enrollment
of a display, official builds and device packages are what is sold (`../DECISIONS.md`, D144,
open-source the wallboard client). `deploy/wallboard/README.md` is the manual route: what to install,
how to build, configure, connect and run it, and what to do when something is wrong.

## Shape

- **The page is an ephemeral Ankurah node**, as the web UI is (`web-ui.md`), with no durable node of
  its own. `crates/fwd-web` built with its `wallboard` feature, from `crates/fwd-web/wallboard.html`,
  mounts the wallboard in place of the app. It starts one node for the page's whole life on the
  project its configuration names (`session.rs`, `wallboard_node`), whose connector retries while no
  daemon answers, and waits for it to join with no deadline (`wallboard_context`). It asks for no
  `/health`, lists no projects, acts as no principal and never reloads for a new daemon build.
- **The launcher**, `deploy/wallboard/wallboard.py`, beside the display, turns one configuration file,
  `wallboard.json`, into a Caddy configuration and replaces itself with Caddy. Caddy serves the page
  and its assets, an exact list taken when it starts; serves the settings at `/wallboard-config.json`;
  and passes on one WebSocket, the configured project's, to the daemon, rewriting its Host and
  Origin to the daemon's own (`daemon.md`, the Host and Origin checks). Everything else answers 404.
  The page reaches the daemon only through it, on loopback, directly or through an SSH tunnel.
- **Rust** draws: `wallboard.rs` fetches the configuration and mounts the display, and works out each
  card's availability from the prerequisites it watches; `wallboard_settings.rs` holds the typed
  settings; the renderer, `wallboard_render.rs`, and the packing, `wallboard_layout.rs`, are child
  modules of `graph.rs` and `layout.rs`, so that they draw with the Graph page's own code. The
  renderer gives the runtime each frame as SVG markup, with the settings revision it was drawn under.
- **JavaScript** presents: `wallboard-runtime.mjs` owns the live and replay state, the timers, the
  observers and the DOM. `wallboard-history.mjs` owns the witnessed history and the reading of
  snapshots, `wallboard-sound.mjs` the audio, `wallboard-theme.mjs` the palettes and the solar timer,
  and `wallboard-diagnostics.mjs` the diagnostics; the runtime disposes of all of them. The runtime
  lays out with the renderer's own packing, which Rust hands it, when cards that are leaving still
  need their places and when it fits the graph to the screen. SunCalc (`suncalc.mjs`, BSD-2-Clause,
  `SUNCALC.md`) computes sunrise and sunset.
- **The build**, `scripts/build-wallboard.sh`, writes two modules the runtime imports,
  `wallboard-contract.mjs` (the settings contract) and `wallboard-build.mjs` (what this build is),
  and the bundle's `build-info.json`: the commit it was built from and `content_sha256`, a hash of
  every file the bundle is built from, committed or not, so that any change to them makes another
  build (`scripts/wallboard_identity.py`). Those files are `crates/fwd-core` and `crates/fwd-web`;
  the root `Cargo.toml`, whose workspace settings and dependencies `fwd-core` inherits;
  `deploy/wallboard`; the build script and `wallboard_identity.py` themselves; and
  `.cargo/config.toml`, `rust-toolchain.toml` and `LICENSE`. Its test checks that they hold every
  manifest and the lockfile Cargo reads for the build, as `cargo metadata` names them, and that a
  dependency feature `fwd-core` inherits from the root manifest changes the hash. The build never
  builds or replaces the daemon or the page the daemon embeds, and it builds in a target directory
  of its own, `crates/fwd-web/target/wallboard`: trunk copies every module it finds in its target
  directory into the page it builds, so a shared one would carry the wallboard's modules into the
  daemon's page.

### What the renderer and the packing take from the core

The renderer (`wallboard_render.rs`) and the packing (`wallboard_layout.rs`) are child modules of
`graph.rs` and `layout.rs`, so they use private items there. Each imports them in one `use super::{…}`
list, so renaming one in the core fails to compile there:

- From `graph.rs`: `Edge` (its fields `first`, `then`, `kind` and `proposed`, and `Edge::of`),
  `also_part_of`, `implied` and `path`.
- From `layout.rs`: `Group` (its fields `task`, `before`, `after`, `chains`, `task_height` and `gap`,
  and its methods `new`, `columns_by_tie`, `order_auto`, `needs`, `pulled`, `place`, `places` and
  `crossings`); `Placed` and `Line`, built field by field; `urgency`, `numbered`, `without_loops`,
  `columns_of`, `groups_of` and `spread_ends_with`; the constants `PAD`, `W` and `GX`; and the
  public `Layout`, `Task` and `Drawing`.

A field or method cannot be named in a `use`, so a rename of one fails where the wallboard uses it.
Their tests are child modules too, and use the Graph page's test helpers `id`, `task`, `waits` and
`part_of` and its `plan`, and the layout's test helpers `open`, `task` and `crossings_drawn`.
Everything else the wallboard uses is crate-public: `store`'s standing queries and lookups, with
`open_with_predicate`; `session`'s project, client and context, with `wallboard_node` and
`wallboard_context`; `fmt`'s labels; and `clock`'s `keep_time` and `comes_due`.

## Configuration

`deploy/wallboard/wallboard-contract.json` is the settings contract: the types, defaults, enums and
limits of every setting. It is a small format of its own, not JSON Schema. The build turns it into a
JavaScript module; Python and Rust read the JSON. All three run the cases in
`deploy/wallboard/settings-cases.json`, each named for its scenario. Known settings are checked
strictly; unknown keys are ignored, a retired replay setting among them, so that the host can keep
its own keys beside the settings. A missing `schema_version` means version 1, and another version is
refused. `theme_schedule: null` means a palette chosen by hand, and an enabled schedule needs both
coordinates.

Rust holds the settings in two parts. The task selection, the statuses, chooses what the store's
task query selects: the store binds it when it opens, for the page's life, and refuses a second,
different predicate rather than ignore it. Only the presentation part changes while the page runs.
The launcher stays the boundary for the project, the addresses and anything about transport; the
wallboard keeps no records of its own and takes no settings from the network.

The runtime receives every frame with the complete settings revision Rust drew it under, through
`update`. `readRevision` validates a revision without changing anything, and `applySettings` applies
it whole. A change of palette, schedule, sound or replay keeps the runtime, its cards and its
history; a change of screen fitting lays the same cards out again. A change of width, text size or
row limit needs a newly drawn frame and starts a new history, since the cards change shape: one rule,
`historyContext` in `wallboard-history.mjs`, names what a history's frames share, and so what counts
as a change of shape. A change of project or statuses is refused: it needs a new query and a quiet
new start, never a reading as tasks arriving and leaving. So a frame drawn under a refused revision is
never shown; the last view stays, marked STALE, while the connection's state still follows. A
revision is compared with the last one applied as text before it is validated again. An accepted
revision applies only with a frame that can be read: an unreadable frame is the graph's failure, not
the settings', so diagnostics file it under `issues.graph`, none of the revision applies, and the
next readable frame brings it again.

## What it shows

Cards show a task's title, about the first 100 characters of its WHAT, its scopes, its status and
assignee, its priority as a lozenge, the number of its messages, and its parent, if any, on a line
of its own. Within each dependency column, higher-priority cards sit above lower; a change of
priority moves the card, its arrows attached, and among cards of one priority the layout keeps
crossings and long lines few. Relations to tasks not shown never add cards. `max_rows` caps a
column, and more cards make more columns rather than leaving any out. `text_scale_percent` scales
the text and both card dimensions together, so a card shows as much of its task as before; footer
rows past the first and a parent line make a card taller. With `fit_to_screen`, the layout tries
shorter, wider arrangements and keeps one only when it makes the cards visibly larger, keeping the
familiar one on a near tie; a resize lays out again with the same code, through a debounced
observer, and records no change.

A card faded to half its opacity is blocked by an unfinished accepted prerequisite or held, and says
which, Blocked or On hold, with its status and priority unchanged. A cancelled or rejected
prerequisite reads Check dependency, and one that cannot be read Dependency unknown. Availability
comes from exact-ID queries of the prerequisites of the cards shown, which close when no card needs
them, so the display never loads the backlog of finished tasks; a proposed dependency blocks nothing.

A status row above the graph says CONNECTING until the first graph, then LIVE, SYNCING while it
catches up, OFFLINE with the last view while the connection is down, STALE when a frame could not be
drawn or settings were refused, and during replay the age and the counts of the changes it plays.

### Live changes

Changes arriving together are batched: 250 ms after the last, at most 1.2 s after the first. Then a
wave of color crosses the background in 800 ms, the cards lifting two pixels as it passes, their
arrows attached, and the batch appears. A new card pulses for six seconds. A card that leaves keeps
its last content, pulses for four seconds with a countdown along its foot, then fades with its arrows
in 450 ms; it keeps its place in the layout until it has gone, so no new card moves under it. An
edited card glows, a halo outside its edges in the palette's color, for three seconds. A card leaves
when its task leaves the chosen statuses or is given a not-before date still to come, so leaving does
not say the task is done. A dependency added, retracted or accepted edits both its cards, whatever else the batch holds. New
changes never restart a cue already playing. Under the browser's reduced-motion setting the wave,
the lift, the movement and the pulsing stop, and quiet highlights remain.

A cold start is quiet: the page waits for the graph to settle, 750 ms after the last frame and at
most 2.5 s after the first, then draws it without cues, since it cannot tell what changed before it
was watching. A reconnect waits the same way, then shows what changed meanwhile as one batch, with
its cues and no sound. A frame that arrives while the page is hidden applies at once, with no wave,
cue or sound.

With `sound.enabled`, each live batch plays one of three bundled cues, synthesized for the wallboard
(`deploy/wallboard/render-sounds.py` reproduces them): a rising bloom for arrivals, a soft settling
sound for departures, and a shorter brushed tap for edits and mixed batches. One cue plays per batch,
with four seconds between cues; a cue that cannot play in time is dropped, never queued or played
late. Replay, loading, a hidden page and reconnect catch-up stay silent, audio rests between cues,
and a failure of audio never stops the graph.

### Replay

Every `replay.interval_seconds`, the wallboard replays the newest changes it witnessed: snapshots it
saved itself, stamped when it saw them, in this browser's local storage. It is not an Ankurah history:
task timestamps cannot rebuild an old WHAT, priority or status. The history keeps at most 120 batches
and 2 MiB, with no age limit, so a quiet display can replay days back and a busy one only minutes.
Changes seen within 30 seconds of each other, by one build, make one group. Replay plays at most five
groups, oldest first, for four seconds each, and stops within 23 seconds, back at the current graph;
groups with no net change are skipped, and a fresh live change ends it at once. In replay a departing
card fades over three seconds, with no hold first, and arrival highlights finish in three seconds,
so that each step settles before the next.

A history belongs to one project, set of statuses, card width, row limit and text size; a change to
any of them starts a new one, and the history saved for any other is removed at that origin. Saved
snapshots are read back as data: only the SVG vocabulary the renderer writes survives, never a
script, event handler, style or outside resource. Full or disabled storage leaves the display and an
in-memory history working.

### Themes

Each palette covers the whole page: background, cards, text, arrows, edit glows, arrival and
departure cues, the wave, the status row and message counts. The five dark palettes have black
backgrounds and light arrows; the three light ones have light backgrounds, dark text and arrows, and
deeper glows.

| Theme | Cards and priority lozenges |
| --- | --- |
| `dark-aubergine` (default) | Aubergine P1, oat and brass P2, warm charcoal P3 |
| `charcoal` | Widely separated warm charcoal shades, brighter at higher priority, with amber, mauve and stone lozenges |
| `oxblood-copper` | Wine P1, copper P2, warm brown-gray P3 |
| `indigo-saffron` | Indigo P1, warm amber P2, warm gray P3 |
| `graphite-coral` | Widely separated graphite shades, brighter at higher priority, with coral, sand and stone lozenges |
| `linen-indigo` | Warm linen background, indigo P1, apricot P2, warm stone P3 |
| `porcelain-coral` | Pale porcelain background, coral P1, sand P2, neutral gray P3 |
| `sand-plum` | Light sand background, plum P1, oat P2, taupe P3 |

`theme_schedule` switches to its day palette at the local sunrise and its night palette at sunset.
SunCalc works out both as instants, refraction included, from the configured coordinates, so time
zones and daylight saving cannot move them, and nothing asks for a location or the weather. The
palette applies as soon as the configuration loads, before the graph does, and switches at each event
with no reload and no new history. One timer checks the clock at least once a minute, and a page
coming back into view checks at once. Under a polar day or night the palette stays, with no invented
sunrise.

## Not-before dates

The renderer leaves out a task that is not yet due (`../DECISIONS.md`, D152, a short-term workaround
until the live query's `IS NULL` path is fixed): `wallboard_tasks` asks `clock::comes_due` inside
the drawing's memo, so that a date arriving draws again. Judge a date only through
`clock::comes_due` or `clock::present`, never `now_ms`, or the clock does not remember the judgment
and cannot find when it stops holding. A started task shows whatever its date.

Keep the task query as it is. The clock finds the dates to come among `store::tasks`, so those tasks
must stay in the query, and a date passing changes nothing a live query could hear. At fwd's Ankurah
pin a live query with `IS NULL` panics the reactor on both the client and the daemon serving it and
poisons its lock, so add no such predicate, no date comparison in a predicate and no
`update_selection`.

`WallboardGraph` starts the clock's checks with `clock::keep_time`, because they read the store, and
it mounts only once the store has opened: a cold start with no daemon answering can wait for the
store for longer than a check takes to come round. The app's `App`, which starts them on the ordinary
page, is not mounted in the wallboard.

A date passing reaches the runtime as an ordinary frame, so it needs no presentation of its own and
is told from a task edit by what the frame says. A card's revision holds nothing about its date, and
states its task's relations whether or not the other card is drawn (`wallboard_relation_facts`): its
parents, and each dependency in either direction. `changes` in `wallboard-history.mjs` takes edits
from revisions alone, and changed arrows only make a frame worth presenting. So a card arriving or
leaving, a chain through it replacing the arrow it implies, and a parent coming and going edit
nobody, while a dependency added, retracted or accepted edits both its cards, whatever else is in the
batch. Availability does not read dates: a task's prerequisites are watched and judged by status,
whether or not their cards are drawn, so a card that waits for a task not yet due still says Blocked,
with no arrow to a card that is not drawn.

The page checks the clock once a minute and when it comes back into view, so a task comes due at
most a minute after its date, as an ordinary arrival, and a task given a later date leaves as an
ordinary departure. The clock does not check while the page is hidden, so a date that passes then
shows when the page comes back into view, with the ordinary cue. During an outage the last view
stands, and a date that passed meanwhile appears in the silent catch-up after the reconnect. Like
any client of this core, the wallboard registers the `not_before` field in the project's catalog
the first time it connects.

A `TaskView` cannot be built natively, so the synthetic checks do not notice if `wallboard_tasks`
stops asking the clock, `keep_time` moves before the store opens, or a revision stops stating
relations. The scratch-daemon test does (Checks, below): run it after changing any of them.

## Diagnostics

The runtime exposes `diagnosticsFor(host)` through its module, and
`runtimeFor(host).diagnostics.snapshot()`. They report the build (`build-info.json`'s two fields), a
digest of the active settings, the connection and loading state, the last times a frame was
received, found valid and applied, the numbers of cards and arrows, one record per failing area with
a reason that holds no content, and timings for reading snapshots, layout, drawing and saving
history. They record no task text, credentials or addresses, and nothing polls them. A malformed
frame keeps the last good graph, marked STALE while connected or OFFLINE when not; failures of audio
and storage never stop the display.

Times in `boot`, `milestones` and `latest` are milliseconds since navigation started
(`performance.timeOrigin`), one clock for Rust's boot and the runtime; the times in `state` are wall
clock.

| Field | Recorded when |
| --- | --- |
| `boot.mounted` | The Rust display mounted: assets loaded and the WASM compiled. |
| `boot.configuration` | The configuration was fetched and validated. |
| `boot.node` | The ephemeral node, its browser cache and its connector existed. |
| `boot.context` | The node's system was ready, its context open and the standing queries registered. |
| `milestones.firstLoaded` | The runtime first saw the daemon's answer to every standing query. |
| `milestones.armed` | The quiet wait at the start ended and live changes began to be presented. |
| `latest.connected`, `latest.disconnected` | The latest changes of connection the runtime saw. |
| `latest.baselineEnded` | The latest end of a quiet wait: at a cold start, after a reconnect, or after a frame the runtime held back while connected. On an armed display, new content is not on screen yet at this mark. |
| `latest.catchUpApplied` | The latest time an armed display applied what a quiet wait held back, after its batch wait and wave; at once if nothing differed, or when a frame of new card shapes replaced it first. Cards may still be moving, and a failed draw does not count. A cold start does not set it: `milestones.armed` presents the first graph. |

`timings.rustRender` counts Rust's recomputations of the drawing and totals their time;
`timings.render` is the DOM update, `snapshot` the reading of SVG, `layout` the fitting and the layout
of departures, and `persistence` the saving of history.

### Measuring a cold start and a reconnect

Sample a real start and a real reconnect in a browser of your own, never the display's. Record the
hardware, the browser's version, `build.content_sha256`, `state.settingsDigest`, `state.nodes` and
`state.edges` with every sample.

- The opt-in live test (Checks, below) starts with its relay down, then interrupts it once. Its final
  JSON holds `diagnostics.coldStart`, sampled once the runtime is armed, `diagnostics.reconnect`,
  sampled once the reconnect's catch-up has applied, and `diagnostics.relay`, the page-clock times at
  which it started and restarted its relay.
- On the display machine, open the wallboard's address in a separately launched browser, wait for
  `.wallboard-graph[data-armed="true"]`, then evaluate:

  ```js
  const runtime = performance.getEntriesByType('resource').map(entry => entry.name)
    .find(name => name.endsWith('/wallboard-runtime.mjs'));
  (await import(runtime)).diagnosticsFor(document.querySelector('.wallboard-graph'));
  ```

The stages are differences between those times:

| Stage | Cold start | Reconnect |
| --- | --- | --- |
| Assets, WASM and mount | `boot.mounted` | |
| Configuration fetch | `boot.configuration − boot.mounted` | |
| Node start: cache and connector | `boot.node − boot.configuration` | |
| Connection and context | `boot.context − boot.node`, or from `relay.started` in the live test, which holds its relay down first | `latest.connected − relay.restarted` |
| First answers to the standing queries | `milestones.firstLoaded − boot.context` | |
| Quiet wait, with any frames still arriving | `milestones.armed − milestones.firstLoaded` | `latest.baselineEnded − latest.connected` |
| Batch wait and wave before the catch-up shows | | `latest.catchUpApplied − latest.baselineEnded` |

These waits are deliberate (Live changes, above). A cold start arms straight after its quiet wait,
with neither batch wait nor wave. A reconnect that found nothing new applies at once, so a last stage
near zero measured no catch-up: label that sample a reconnect without changes. A frame the runtime
held back moves both marks with no reconnection, so check that `latest.connected` belongs to the
reconnect being measured.

These times do not establish Ankurah's convergence. `store::standing_loaded` latches once, when the
daemon has first answered every standing query, and stays true for the page's life, so a reconnect
has no signal that the answers are complete. A reconnect's stages are the connection coming back and
a deliberate quiet wait, and cannot separate Ankurah's catch-up from that wait; they name no
bottleneck. Compare the `timings.rustRender` and `timings.render` totals with the stage they fall
in: on a cold start, the stage that dominates is the one to work on, not the layer busy last. For a
representative update or an idle interval, sample before and after and subtract.

### Profiling the presentation with synthetic data

`deploy/wallboard/profile-browser.cjs` measures the presentation only: the production WASM layout and
runtime, drawing a synthetic graph at 3840×2160 with no Ankurah connection. It reports the cold start
of the presentation, the reconnect's quiet wait, the main thread's idle use and the timing totals,
and its `kind` says that it leaves out Ankurah's synchronization. After `scripts/build-wallboard.sh`
and the check script's `npm ci`:

```sh
PLAYWRIGHT_BROWSERS_PATH=target/wallboard/ms-playwright node deploy/wallboard/profile-browser.cjs
```

It launches its own browser; `TEST_CDP` attaches it to a separately started test browser instead,
never a display's, `LOCAL_PORT` and `TEST_URL` allow forwarding to another host, and
`PROFILE_OUTPUT` saves the report. Draw timings include the layout they cause, so do not add them
together, and the start includes loading, compiling and setting up the fixture. A report holds the
build's identity and counters, never task content. Timings on one machine are a baseline for that
machine, not a claim about another.

## Checks

`scripts/check-wallboard.py` runs every check that needs no daemon, with synthetic data only. It
installs the locked Playwright (`--install-browser` adds its pinned Chromium), runs the web crate's
native tests with the `wallboard` feature, which cover the renderer, packing, settings and
availability, and without it; clippy for wasm32 with and without the feature; the crate's rustfmt
check; the launcher's and the settings contract's Python tests; the build identity's tests
(`scripts/test_wallboard_identity.py`); the build; the runtime's unit tests in Node (diagnostics,
history and change detection, settings, themes and the scratch test's isolation); and the browser
checks, each against the production runtime, most of them on a virtual clock: motion and replay,
membership changes, settings revisions, screen fitting, diagnostics, sound, and the production WASM
layout driving the runtime. Continuous integration runs the same checks, the Rust and Python ones in
its `rust` job and the rest in its `wallboard` job.

Two integration tests stay opt-in, since each needs what the suite never touches.

`deploy/wallboard/test-browser.cjs` reads a live project through a daemon that already runs. It needs
Caddy, the locked Playwright that the check script installs, and the configuration of an existing
project with tasks in the statuses it shows:

```sh
WALLBOARD_CONFIG=/path/to/wallboard.json CADDY=/path/to/caddy \
  PLAYWRIGHT_BROWSERS_PATH=target/wallboard/ms-playwright node deploy/wallboard/test-browser.cjs
```

It runs the launcher on a free port with a relay of its own in front of the configured daemon,
starts with the relay down, then interrupts only that relay: it checks the cold start, the graph kept
through the outage, the reconnect with the same node and its subscriptions sent again, no reload, no
controls, the statuses shown, the fit to the screen in both orientations and the proxy's refusals. It
never starts, stops, configures or writes to the daemon.

`deploy/wallboard/test-scratch-daemon.cjs` drives the built bundle against a daemon it starts itself,
in real time and with real writes. It covers the Rust wiring the synthetic checks cannot reach: the
renderer asking the clock about a task's date, the clock starting only once the store has opened, and
a card's revision stating its task's relations. It takes about twelve minutes:

```sh
bash scripts/build-wallboard.sh
bash scripts/build-web.sh && cargo build -p fwd   # the daemon serves the page the first scenario opens
SCRATCH_DAEMON=target/debug/fwd SCRATCH_CLIENT=target/debug/fwd \
  PLAYWRIGHT_BROWSERS_PATH=target/wallboard/ms-playwright node deploy/wallboard/test-scratch-daemon.cjs
```

`SCRATCH_CLIENT` is `fwd` built from this checkout, which writes the not-before dates.
`SCRATCH_DAEMON` is the `fwd` to run as the daemon: this checkout's too, or the build a display's
daemon runs, to check the wallboard against it. The test also needs `python3`, `git` and `sqlite3`.
`SCRATCH_SCENARIOS` chooses a comma-separated subset of the scenarios in
`deploy/wallboard/scratch/scenarios/`, each of which opens its own project and page: `first-contact`
(a cold start with no daemon answering past one clock check, the clock starting only after the store
opens, and the field's first registration), `dates` (the first load, a real date passing with no
write, setting and clearing a date), `reconnect` (dates during an outage), `page-clock` (a hidden
page, a corrected clock), `membership` (a chain and a parent coming and going) and `relations` (a real
dependency change, batched with an unrelated arrival and with an unrelated departure).

It cannot reach a daemon it did not start. Before anything starts, it removes every inherited
`FWD_*`, `CLAUDE*` and `CODEX_*` variable from its own environment, which everything it starts
inherits, points fwd's runtime, data and dev-runner directories into a new temporary directory,
checks both, and refuses to go on if a daemon answers there. Its one daemon runs on those
directories and stops at the end, and every task it writes is synthetic. A passing run deletes the
temporary directory unless `SCRATCH_KEEP` is set; a failing run keeps it, logs included, and names
it. A small Node server stands in for Caddy, serving the routes `wallboard.py` generates, so real
Caddy stays with the live test. The page is hidden, and its clock corrected, by overriding what the
page reads, not by the operating system.

## Limits kept deliberately

History holds bounded snapshots that the display witnessed. It cannot recover changes it did not
see, and a change of card shape starts a new one; replacing it with history that Ankurah keeps is
its own task. A browser started while the daemon is out of reach waits for data: it is not a durable
replica. The connection runs under the daemon's permissive policy, not an authorization that
enforces reading only on the server. Managing displays remotely and packaging installations are
separate work.

A card's revision means something only within one build, since what it hashes can change. A page
compares live frames only with frames it received itself, and its first frame is a quiet start, so
an upgrade cues nothing. Replay compares saved snapshots with each other. Each history entry records
the build that wrote it, and a replay group holds entries of one build, so no group spans an
upgrade. A saved frame in which only arrows changed replays as "Observed change", naming no card.
