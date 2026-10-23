# Business-model research: open-core dev tools, agent-usage pricing, sync hosting, licensing

Research note, 2026-09-24. Compiled by a research agent from ~60 primary fetches (pricing pages, LICENSE files, founder posts, HN threads) plus search. Dates in parentheses are post dates, or "fetched Sept 24, 2026" for live pricing pages. Items the agent could not confirm from a primary page are marked **unverified**. The agent's search budget ran out near the end, so a handful of items rest on secondary sources.

## 1. Open-core / self-host-free comparison

| Product | License | Free line | Paid line | Price points |
|---|---|---|---|---|
| **Plane** | Community AGPL-3.0; Commercial edition closed (12 free seats self-hosted) | Cloud ≤12 users; Community = free-tier parity | Pro: 500 AI credits/seat, wiki, time tracking, integrations, SAML (self-host only). Business: 1,000 credits, workflows, SAML on cloud. Enterprise: LDAP, API audit logs, approvals | $6 / $13 per seat/mo annual ($8/$15 monthly) |
| **Huly** | EPL-2.0 | Self-host free, unlimited users | Per-workspace by storage + A/V traffic, not seats | $0 / $19.99 / $99.99 / $399.99 per mo on the pricing page, but the README now says the hosted service was "discontinued because its hosting is no longer funded" (no date) |
| **GitLab** | CE MIT, EE proprietary | Free (5 users/top group, 400 compute min) | Premium: approvals, 10k min. Ultimate: security/compliance, custom audit. Duo agents via GitLab Credits | Premium $29/user/mo annual; Ultimate custom; credits $1 each, 12–24 included per user |
| **Gitea** | MIT | Everything | Enterprise: SAML SSO, audit log, org-wide branch protection, dependency scanning, IP allowlist, K8s runners, SLA | $9.50/user/mo (1-yr commit) or $19 monthly |
| **Sourcegraph** | Apache → enterprise license (June 2023) → private repo (Aug 22, 2024; Slack: open source "added a lot of extra work and risk") | Amp Hobby free, BYOK, "no Amp token fees" | Enterprise: code search + pooled AI credits, single tenant | Enterprise "starting at $16K" annual minimum; Amp $20/mo individual |
| **PostHog** | MIT core + proprietary `ee/` | Large monthly allowances (1M events, 5K replays…); "97% of companies use PostHog for free" | Usage overages + platform packages: Boost (SSO enforcement, unlimited projects), Scale (SAML, priority support, 2-month activity logs), Enterprise (RBAC, 60-month logs) | Boost $250/mo, Scale $750/mo, Enterprise custom |
| **Cal.com** | AGPL-3.0 + commercial `/ee` | 1 user | Teams: sub-teams, branding removal. Organizations: SAML, SCIM, roles, SOC2/HIPAA, EU hosting. Enterprise: dedicated DB, SLA | $12 / $28 per user/mo; custom |
| **Supabase** | Apache-2.0 | 2 projects, 500MB, pauses after 7 idle days | Pro: no pausing, backups. Team: SOC2/ISO, 14-day backups, read-only roles. Enterprise: SLA, PrivateLink | $25/mo (+$10 compute credit), $599/mo, custom |
| **Appsmith** | Apache-2.0 | ≤5 users (cloud) | Business: custom roles, audit logs, workflows, de-branding. Enterprise: SAML/OIDC, SCIM, air-gap | $15/user/mo (≤99 users); $2,500/mo from 100 users |
| **n8n** | Sustainable Use License (fair-code, not OSI) + enterprise license | Self-host: all core; may not resell or run as a hosted service for others | Cloud by executions; Business (self-host only): SSO/SAML/LDAP, git, environments; Enterprise: log streaming, external secrets | €20 / €50 / €667 per mo; custom (205k GitHub stars) |
| **Coder** | AGPL-3.0 + LICENSE.enterprise | Unlimited workspaces/members, one org, **max 5 concurrent Coder Agents**, 1 integration | Premium: audit log, multi-org RBAC, OIDC group sync, proxies, SLA. AI Premium: unlimited concurrent agents, headless API, network-isolated agent workspaces | Premium per-user annual, unpublished; AI Premium custom |
| **Continue.dev** | Apache-2.0 | Extension, BYOK | Hosted Hub: model credits, shared agents, SSO | Was Team $20/seat incl. $10 credits (**unverified**; pricing page now 404). Acquired by Cursor ~June 2026; Hub shut, repo read-only |
| **OpenHands** | MIT | Local free; cloud Individual free, 10 conversations/day, LLM "at cost, with no markup", BYOK | Enterprise: SAML/SSO, self-host in VPC, unlimited concurrency | Custom only (a paid Growth plan reportedly dropped in 2026, **unverified**) |
| **Vibe Kanban** | Apache-2.0 (28.2k stars) | 1 user, all core; self-hosting team features free | Pro: hosted cloud collaboration (launched Feb 3, 2026), 99.5% SLA. Enterprise: SSO/SAML | $30/user/mo. Bloop shut down April 10, 2026: "thousands" of daily users, "the vast majority are free users" |
| **Conductor** | Closed Mac app | Parallel local agents, BYO Claude/Codex subscription or keys | Pro: cloud workspaces, multiplayer (≤5), API. Teams: org-wide, admin. Enterprise: SAML/SCIM, DPA. Usage-based cloud compute "planned" | $0 / $50/mo / $60/user/mo / custom; $22M Series A (Mar 30, 2026), 10x users since January |
| **Entire** | CLI MIT | Checkpoints CLI | Nothing priced; distributed Git network in preview (Jul 8, 2026), Git backend to be open-sourced | $60M seed at $300M valuation (Feb 10, 2026, Thomas Dohmke; Felicis lead) |
| **Graphite** | Closed | Hobby: personal repos, limited AI review | Starter: org repos, insights. Team: unlimited AI reviews, merge queue, automations. Enterprise: SAML, SIEM audit, ACLs | $20 / $40 per user/mo annual ($25/$50 monthly) |
| **CodeRabbit** | Closed | Free forever on public repos | Essentials: agentic reviews. Team: triage, custom checks. Advanced: security review. Enterprise: self-host, SSO, audit, RBAC | $24 / $48 / $72 per dev/mo annual; bills only devs who open PRs |
| **Linear** (benchmark) | Closed; in-house sync engine (Artman, Jun 29, 2023) | Unlimited members, 250 issues, 2 teams, 10MB uploads; agent platform on every tier | Basic: 5 teams, unlimited issues. Business: SAML, guests, Triage/Code Intelligence. Enterprise: audit log, SCIM | $10 / $16 per user/mo annual; **agents are workspace members and do not consume seats** |
| **Beads** | MIT (27.4k stars, Go, Dolt-backed) | Everything | Nothing: no hosted tier, no sponsorship, no company; repos now under the community org "gastownhall". A "Gas Town by Kilo" managed cloud was reported by The New Stack (2026), **unverified**; Kilo was itself acquired by Anaconda (Sept 2026, per kilo.ai) | — |

What the table teaches. The paid line is almost always the same four things: hosting, identity (SSO/SAML/SCIM), governance (RBAC, audit, retention), and support/SLA. Tracker seats cluster at $6–16; agent/review tooling at $20–60. 2026 added a fifth paid line, agent capacity: Coder gates concurrent agents, Plane bundles AI credits per seat, GitLab sells credits. Hosted collaboration alone did not save Vibe Kanban at $30/user, and Huly's storage-priced cloud died; free-heavy user bases do not convert on hosting by itself. The permissively licensed hosted businesses on this list were acquired or wound down (Continue, Instant, Jamsocket), while AGPL-plus-`ee` (Plane, Cal.com, Coder) and fair-code (n8n) remain independent.

## 2. Charging for agent usage

| Vendor | Unit | Price points (dated) |
|---|---|---|
| Devin | ACU (~15 min of VM + inference + network), then dollar quotas | Apr 2025: Core $20 + $2.25/ACU, Team $500 incl. 250 ACUs. Apr 14, 2026 relaunch: Free / Pro $20 / Max $200 / Teams $80 min + $40 per seat / Enterprise (still ACU-billed). Ask Devin Deep Mode, Devin Review, DeepWiki premium moved from free to usage-billed |
| Cursor | $ of model usage | Jun 16, 2025: Pro went from 500 requests to a $20 usage pool; "unlimited" applied only to Auto. Jul 4, 2025 apology and refunds (TechCrunch Jul 7). Now Individual $20, Teams $40/user (SSO, Bugbot, cloud agents), on-demand billed in arrears |
| GitHub Copilot | Premium requests → AI Credits | Coding agent: 1 session = 1 premium request + Actions minutes. Announced Apr 27, 2026, effective Jun 1, 2026: $0.01 credits billed per token; Pro $10 incl. $10, Pro+ $39 incl. $39, Business $19 incl. $19, Enterprise $39 incl. $39. GitHub: "the current premium request model is no longer sustainable." Dev reaction headline: "You will get less, but pay the same price" (Visual Studio Magazine, Apr 27, 2026) |
| OpenAI Codex | ChatGPT plan + per-token credits | Bundled: Go $8, Plus $20, Pro $100, Business $20/user annual; credits priced per model per million tokens, 5-hour windows, fast mode doubles cost. Per-message metering replaced by credits Apr 2, 2026 (**unverified** date) |
| Factory | Rolling 5h/7d/30d windows | Pro $20, Plus $100 (~5x), Max $200 (~10x); Extra Usage prepaid from $10; free BYOK allowance |
| Google Jules | Tasks/day | Free 15/day (3 concurrent), Pro 100/day (15), Ultra 300/day (60), via Google AI Pro/Ultra. Dollar prices **unverified** |
| Claude Code (benchmark) | Session + weekly caps | Pro $20, Max $100/$200. Weekly limits announced Jul 28, 2025; May 6, 2026 doubled 5-hour limits; Sept 14, 2026 net ~17% weekly cut vs the summer promo. A June 2026 class action over Max multipliers is **unverified** |
| Linear | Provider tokens at cost + sandbox time | Coding Sessions (Aug 20, 2026): tokens "at the provider's published rates, with no markup" + $0.25 per 20-minute sandbox block; Loops (Jul 20, 2026, Business+) $0.07–0.20 per run; prepaid workspace balance, $10 minimum top-up, admin spend caps |
| Conductor | None on inference | App free; Pro $50 is for cloud workspaces + multiplayer; Anthropic deferred billing restrictions for third-party Claude apps (Conductor blog, Jun 15, 2026) |
| Vibe Kanban | None on inference | Founder on Show HN (Jul 11, 2025): wanted "a long term free offering that doesn't cost us compute or tokens" and paid resource-intensive features; never got there |

Pattern. Every vendor that bundled inference into a flat seat repriced within 12 months (Cursor, Copilot, Codex, Anthropic, Devin), and each repricing produced the same backlash: users experience "same price, less work." The stable 2026 positions are (a) BYOK or at-cost pass-through with no markup (Linear, OpenHands, Amp, Kilo, Conductor), and (b) charging for things whose cost the vendor controls: sandbox minutes, concurrency, seats, governance. Coder's "5 concurrent agents free, unlimited paid" is the cleanest gate for a coordination product.

**Hosted supervising-agent analog.** Linear Agent (public beta Mar 24, 2026; agents triage incoming issues, research root causes, draft project updates, dispatch to coding agents) is almost exactly fwdcentaur's "supervising agent," and Linear prices it as Loops at $0.07–0.20 per run plus at-cost tokens, on Business ($16/seat) and above. CodeRabbit sells triage as a tier step ($24 → $48/dev). Devin Review went from free to usage-billed after a 2-week trial. So the market price for a grooming agent is: cents per run at cost-plus-nothing, gated to a paid tier, or a +$20–25/dev tier step.

## 3. Sync-backend businesses

| Product | License | Metering | Price points (fetched Sept 24, 2026 unless noted) |
|---|---|---|---|
| Linear sync engine | Proprietary, in-house | Not sold | — |
| Rocicorp Zero | Apache-2.0 (rocicorp/mono, 3.4k stars); 1.0 on Jun 8, 2026 | vCPU + storage | Cloud Zero: Hobby $30/mo (10GB, 3 shared vCPU), Professional $300/mo (100GB, 7 dedicated vCPU, SLA), BYOC $1,000+/mo + AWS; +$0.20/GB, +$50/vCPU |
| ElectricSQL (now electric.ax, "Agents on sync") | Apache-2.0 server | Writes + retention; reads, fan-out, connections free | Pricing live Apr 2, 2026: $1 per 1M writes, $0.10/GB-month; PAYG waived under $5/mo; Pro $249/mo prepaid, Scale $1,999/mo 6-month commit; Postgres Sync +$2/1M writes |
| PowerSync | FSL (Fair Source adopter Aug 6, 2024) | GB synced + peak connections | Free 2GB/50 clients; Pro $49/mo (30GB, 1,000 clients); Team $599/mo (roles, SLA); $1/GB synced, $1/GB hosted, $30 per 1,000 peak clients. Sept 12, 2025: dropped per-sync-operation billing because "spikes weren't proportional to costs" |
| Jazz | MIT (196 stars; 2.0 alpha) | MAU tiers, then storage + egress | Starter free (100 MAU), Indie $4/mo (10k MAU), Pro from $19/mo: $0.02/GB storage, $1/GB sync egress |
| Liveblocks | Proprietary hosted | Credits: $0.002/collab minute, $0.01/comment, $1 per 1M storage updates, $0.15/GB | Free; Pro $30/mo with $30 credits; Team from $600/mo with $600 credits |
| Convex | FSL-1.1-Apache-2.0 backend (12.6k stars) | Per developer + usage | Free/Starter PAYG; Professional $25/dev/mo (25M calls, 50GB); Business $2,500/mo minimum |
| Y-Sweet / Jamsocket | MIT | — | Jamsocket acquired by Modal Jul 10, 2025 |
| InstantDB | Open source (10.5k stars) | — | Team joined OpenAI (2026); signups closed; cloud shuts Aug 31, 2027; "most of our users began to use Instant through agents" |
| Automerge / Ink & Switch | MIT | Not a business: sponsors, support contracts, grants | — |

Read-through for Ankurah. Standalone sync hosting is a crowded, low-margin market with two exits and one wind-down in 14 months; nobody here charges per connection anymore. The metering that survives is writes/data volume (Electric, PowerSync) or a flat instance fee (Zero). Hosting the Ankurah server is viable only as the substrate of a hosted fwdcentaur workspace priced by workspace or seat, not as a sync product sold by the MB.

## 4. License strategy

Recent discourse, dated. Sentry created FSL (Nov 20, 2023): source-available, "competing use" excluded, converts to Apache/MIT after two years; Fair Source (fair.io, 2024) now lists 13 adopters including PowerSync, GitButler, Keygen, Tuist (Jul 2025), Liquibase (Sept 30, 2025). Sentry's "Fair Source in the AI age" (Mar 17, 2026) argues FSL's contract-law lever also blocks LLM-assisted clones. On the other side: Elastic returned to AGPL (Sept 2024), Redis returned to AGPLv3 (May 1, 2025) after SSPL "hurt our relationship with the Redis community" and Valkey forked; HashiCorp's BSL produced OpenTofu and IBM's $6.4B acquisition (Feb 2025) without a reversal. Sourcegraph closed entirely (Aug 2024), with Slack arguing open source suits "infrastructure products or client tools," not end-user server apps. n8n shows fair-code can carry a large business but you forfeit the words "open source." AGPL+`ee` is the modal choice among independent self-host-free tools launched 2022–2025 (Plane, Cal.com, Coder, Lago).

Recommendation matrix for "free self-host + hosted business + protection against cloud capture":

| Component | License | Why |
|---|---|---|
| Graph server / sync service (the thing a hyperscaler would host) | **AGPL-3.0**, with a proprietary `ee/` directory for SSO/SCIM/audit/policy | Keeps the OSI "open source" claim that drives adoption; forces any hoster to publish modifications; matches the surviving independents. Caveat: AGPL does not stop unmodified hosting, so the real moat is the hosted product's pace plus `ee/`. Choose FSL instead only if enforceable anti-compete terms matter more than the open-source label |
| CLI, agent SDKs, protocol/schema, VS Code extension | **MIT or Apache-2.0** | These get installed into every agent harness and CI; AGPL there causes compliance refusals (Open Core Ventures: "AGPL is a non-starter for most companies" as a dependency) |
| Ankurah | Keep **MIT/Apache-2.0** (already dual-licensed) | Infrastructure library; permissive is where infra wins adoption |
| Hosted control plane (multi-tenant, billing, org management), supervising-agent policies | **Proprietary** | Every comparable keeps this closed, including AGPL shops |

Avoid BSL/SSPL outright (reputational history) and avoid inventing a bespoke license (n8n spent years litigating "fair-code").

## 5. Go-to-market signals

- **Beads** (Oct 2025): 1,000 stars in six days from Yegge's Medium/X audience, no Show HN. HN threads: 19 points (Oct 2025), 111 points/68 comments (Nov 2025). Gas Town's "Welcome to Gas Town" hit 354 points/224 comments (Jan 2026), dominated by skepticism and the cost question, then the $GAS meme-coin episode (Jan 17–23, 2026) showed the reputation-capture risk of viral agent tools. Praise centered on the dependency graph and "somewhere to jot down notes"; complaints on merge conflicts and bugginess. Wants: semantic search/RAG, jujutsu support.
- **Vibe Kanban**: Show HN Jul 11, 2025, 195 points/132 comments; criticisms were default-on telemetry, agents that "clobber each other," and "noise and slop." 28k stars and thousands of daily users still produced no viable business; the cloud tier arrived only nine weeks before shutdown.
- **Conductor**: Show HN Jul 2025, YC S24, $22M Series A Mar 30, 2026 with named enterprise users; it charges for cloud workspaces and collaboration, never for tokens.
- **Entire**: $60M seed on a founder brand; MIT CLI first, revenue deferred to hosted Git.
- **Demand for "task graph for agents"**: Linear's CEO essay "Issue Tracking is Dead" (Mar 24, 2026; secondary via MindStudio May 4, 2026) argues human ticket-grooming dies while structured work state matters more; five weeks later OpenAI's Symphony (Apr 27–28, 2026, Apache-2.0) used a Linear board as the agent control plane and reported 500% more landed PRs on some teams. Linear shipped agents as workspace members (Aug 13, 2026), GitHub shipped Agent HQ/Mission Control (Oct 28, 2025), and a Claude Code issue filed Sept 3, 2026 (#91888, "Critical - Blocking my work") asks for exactly fwdcentaur's spec: a task dependency graph with per-task status and owning agent, file-touch tracking to prevent collisions, and fix-round history that survives context compaction. amux (Mixpeek, MIT) and the GRADE paper model the same depends_on/reviewer edge types. Instant's founders noted most new users arrived "through agents."

## What fwdcentaur could charge for, ranked by plausibility

1. **Team/governance tier on the hosted graph**: SSO/SAML, SCIM, RBAC, audit log, per-agent policies (what an agent may claim, close, or annotate), retention. The universal paid line; $10–16/seat matches Linear/Plane, $9.50 matches Gitea.
2. **Hosted workspace (Ankurah-backed sync + inboxes) as the default for teams**: flat per-workspace or per-seat, not per-MB; Conductor Pro ($50) and Plane Pro ($6/seat) are the brackets. Risk: Vibe Kanban's $30/user hosted collab failed against a free-heavy base, so pair it with item 1.
3. **Hosted supervising agent**: cents per grooming run plus tokens at cost (Linear Loops $0.07–0.20/run), gated to the paid tier; or a +$20/dev tier step (CodeRabbit Triage). This is the compute-consuming feature the Vibe Kanban founder explicitly planned to charge for.
4. **Agent concurrency**: N concurrent agents/inboxes free, unlimited paid (Coder's exact gate).
5. **Cloud runners/sandboxes** for agents that act on graph tasks: $0.25 per 20 minutes is Linear's price; heavy infra, so later.
6. **Premium integrations** (Linear/Jira/GitHub two-way sync, Slack): Plane gates these to Pro.
7. **History and retention** (long-lived graph history, session checkpoints à la Entire): PostHog's 7-day/2-month/60-month ladder.
8. **Support/SLA**: enterprise only.
9. **Per-agent seats**: implausible; Linear set the norm that agents are free members.
10. **Selling Ankurah hosting as a standalone sync platform**: implausible on this evidence; two exits, one wind-down, reads priced at zero.

## Sources (fetched Sept 24, 2026 unless dated)

Pricing/license primaries: plane.so/pricing; developers.plane.so editions; huly.io/pricing and Huly README; about.gitlab.com/pricing; about.gitea.com/pricing; sourcegraph.com/pricing; devclass.com 2024-08-21 (Sourcegraph private); ampcode.com/pricing; posthog.com/pricing and platform-addons; cal.com/pricing; supabase.com/pricing; appsmith.com/pricing; n8n.io/pricing and docs.n8n.io/sustainable-use-license; coder.com/pricing and github.com/coder/coder; continue.dev; HN item 48548758 (Continue acquired by Cursor); openhands.dev/pricing; vibekanban.com/pricing, /blog/shutdown (2026-04-10), BloopAI/vibe-kanban discussion 2510 (2026-02-03); conductor.build/pricing and /blog/series-a (2026-03-30); entire.io news (2026-02-10, 2026-07-08); graphite.com/pricing; coderabbit.ai/pricing; linear.app/pricing, /docs/ai-credits, changelog 2026-08-20 (coding environments), blog "scaling the linear sync engine" (2023-06-29); github.com/steveyegge/beads; github.com/gastownhall; kilo.ai.

Agent pricing: devin.ai/pricing; cognition.com blog 2026-04-14; VentureBeat Devin 2.0 (Apr 2025); cursor.com/blog/june-2025-pricing (2025-07-04); TechCrunch 2025-07-07; cursor.com/pricing; github.blog usage-based billing (2026-04-27); docs.github.com premium requests; visualstudiomagazine.com 2026-04-27; learn.chatgpt.com/docs/pricing; docs.factory.ai/pricing; jules.google/docs/usage-limits; VentureBeat 2025-07-28 (Claude limits); github.blog Agent HQ (2025-10-28); helpnetsecurity.com 2026-04-28 (Symphony).

Sync backends: zero.rocicorp.dev; github.com/rocicorp/mono; InfoQ Zero 1.0 (Jun 2026); electric.ax/pricing and blog 2026-04-02; powersync.com/pricing and blog 2025-09-12; classic.jazz.tools/pricing; github.com/garden-co/jazz; liveblocks.io/pricing; modal.com/blog/jamsocket-is-joining-modal (2025-07-10); convex.dev/pricing and LICENSE; instantdb.com/essays/instant_team_joins_openai.

Licensing: fsl.software; TechCrunch 2023-11-20 (FSL); fair.io/companies; blog.sentry.io fair-source-software-in-the-ai-age (2026-03-17); redis.io/blog/agplv3 (2025-05-01); flowverify.co open-source-relicensing-2026; opencoreventures.com AGPL non-starter; getlago.com AGPLv3 post; cal.com/blog AGPLv3 post.

GTM: HN items 45566864 (Beads, Oct 2025), 46075616 (Nov 2025), 46458936 (Gas Town, Jan 2026), 47250133 (Wasteland, Mar 2026), 44533004 (Vibe Kanban Show HN, 2025-07-11), 44594584 (Conductor Show HN, Jul 2025); seangoedecke.com/gas-and-ralph (2026-01-17); mindstudio.ai blog on "Issue Tracking is Dead" (2026-05-04); github.com/anthropics/claude-code/issues/91888 (2026-09-03); amux.io/guides/ai-agent-graph.
