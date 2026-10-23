# fwdcentaur

A shared, fine-grained graph of the work (tasks, their dependencies and their reasons), the code it touches (annotations anchored to lines that survive the code changing), and the messages between the people and coding agents doing it. A local daemon on Ankurah, a CLI intended for agents, and a VS Code extension.

The CLI and the daemon are one binary, `fwd`, built with stable Rust. The daemon holds each project's graph, applies its rules (which tasks are ready, which moves a task or a decision may make, who may change what) and routes each comment, question and reply to the inbox of the person or agent it concerns; code anchors keep a comment on the lines it was made about while the code changes. Agents work through the CLI, the Claude Code plugin's hooks bringing their inbox into the session and its skills their role. People work in the web UI the daemon serves (the inbox, questions, tasks, graph, scopes and decisions) and in the VS Code extension (the inbox, and comment threads and highlights on the code and in diffs). Nothing is hosted: everything runs on one machine. `docs/DECISIONS.md` says what is established and what is proposed, and the "Implementation notes" at the end of each design doc say how the code deviates from the design.

## Try it

```bash
cargo build --workspace                        # one binary, fwd, lands in target/debug (it spawns itself as the daemon)
export PATH="$PWD/target/debug:$PATH"
cd /path/to/your/repo
fwd init --handle <your github login>          # joins or creates the project (writes .fwd/project.toml), starts the daemon
fwd task create "Retry on disconnect" --why "laptops behind NAT drop" --needs-review
fwd --as impl-alice principal add impl-alice --role implementer   # the agent names itself; each --as impl-alice below acts as it, on local trust
fwd --as impl-alice scope create "Inbox delivery" --intention "get comments to agents mid-task"
fwd --as impl-alice task next --claim          # the implementer takes the ready task
# So that a comment on the code impl-alice works on reaches it, impl-alice anchors its task there: a comment on lines near a task reaches the task's assignee.
fwd --as impl-alice anchor create --path src/net/ws.rs --lines 120-134   # prints the anchor, with its id
fwd --as impl-alice link <task id> anchored_at <anchor id>
fwd comment --path src/net/ws.rs --lines 120-134 "back off, don't hammer"   # anchored to those lines
fwd --as impl-alice inbox                      # the comment arrived (the daemon routed it)
fwd --as impl-alice inbox pop --format hook    # what a Claude Code hook injects
fwd --as impl-alice reply <notification id> "added exponential backoff"
fwd --as impl-alice task submit <task id> --evidence "commit abc, tests green"
fwd inbox                                      # your review request and the reply
fwd graph                                      # tasks with readiness, relations, scopes
fwd ui                                         # the inbox, tasks, graph and scopes in a browser (see below)
fwd daemon stop
```

The web UI is built separately and embedded into `fwd`: `scripts/build-web.sh && cargo build --workspace` (needs [trunk](https://trunkrs.dev) and `rustup target add wasm32-unknown-unknown`), then `fwd ui`. The header's picker, or `?as=<a name or an id>` in the URL, acts as an agent the way `--as` does. Design: `docs/design/web-ui.md`.

Agent sessions: `integrations/claude-code/` is the Claude Code plugin, which gives each agent a user of its own and brings the skills, and its README says how to install it and where the inbox hooks go; the scope is found from the checkout (its bound scope, else the one naming its branch). The VS Code extension is in `extensions/vscode` (`npm install && npm run build`, then F5).

- [docs/using-fwd.md](docs/using-fwd.md): setting up a project and working in it, as a person or as an agent; start here.
- [docs/CONTEXT.md](docs/CONTEXT.md): what it is for, the pieces, foundations.
- [docs/DECISIONS.md](docs/DECISIONS.md): what is established, proposed, and open.
- [docs/design/](docs/design/): the design proposal (data model, daemon, CLI, anchoring, agents and inbox, extension, roadmap).
- [docs/research/](docs/research/): landscape research and the synthesis in `findings.md`.
- [integrations/claude-code/](integrations/claude-code/) and [integrations/codex/](integrations/codex/): the Claude Code and Codex plugins, with the skills for each role.
- [extensions/vscode/](extensions/vscode/): the VS Code extension: comment threads and highlights on the code, the web UI's page in a panel, and an inbox view.

## License

This repository is fwdcentaur's open core: everything a person needs to run fwd on their own machine.

Copyright (c) 2026 Synesthetic Systems LLC.

- **The fwd code** is licensed under the GNU Affero General Public License, version 3 only ([LICENSE](LICENSE)). That covers the daemon and CLI (`crates/fwd`, `crates/fwd-core`), the web page (`crates/fwd-web`), the VS Code extension and its host node (`extensions/vscode`, `crates/fwd-host`), and everything else here outside `integrations/`. Whoever distributes a modified fwd, or offers one to users over a network, publishes its source under the same license.
- **The agent integrations** in [integrations/](integrations/), the Claude Code and Codex plugins and their skills, are licensed under either the MIT license ([integrations/LICENSE-MIT](integrations/LICENSE-MIT)) or the Apache License 2.0 ([integrations/LICENSE-APACHE](integrations/LICENSE-APACHE)), at your option. They are copied into agent setups of every kind, so they carry no copyleft.

The hosted service is proprietary and lives in a separate private repository: backup and availability, connections to other apps, and the public MCP server.
