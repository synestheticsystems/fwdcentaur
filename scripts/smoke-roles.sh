#!/usr/bin/env bash
# Walk the fixed roles, an implementer, the project's leads and its triage agents (one per scope,
# and one for the whole project), through the CLI against a scratch project, with who belongs to it,
# and check each expected effect. Usage: scripts/smoke-roles.sh [path-to-fwd]   (default: target/debug/fwd)
# With a build from before the fixed roles at hand (FWD_SMOKE_BEFORE names one, or
# target/fwd-before holds it: its section says how to make it), it also carries a graph that
# build wrote over to them; without one it says it skips that.
# FWD_SMOKE_LAN=1 also moves the scratch daemon onto the LAN and back (it listens on every
# interface for a moment) and checks what a phone on the same network may and may not do.
set -euo pipefail
# The Claude Code session running this, if one is, names none of the agents below: they are given
# their own session and agent ids ("an agent of its own", below).
unset FWD_CLAUDE_SESSION FWD_CLAUDE_AGENT CLAUDE_CODE_SESSION_ID
F=${1:-$PWD/target/debug/fwd}
REPO=$(cd "$(dirname "$0")/.." && pwd)   # this checkout of fwdcentaur, which builds the fwd from before the fixed roles
T=$(mktemp -d)
export FWD_RUNTIME_DIR=$T/run FWD_DATA_DIR=$T/data FWD_DEV_RUNNER_DIR=$T/dev-runner
mkdir -p "$T/repo" && cd "$T/repo"
git init -q -b main && git config user.email t@e.com && git config user.name t && git remote add origin git@example.com:t/roles.git
printf 'a\nb\nc\nfn reconnect() {\n    loop {\n        try_once();\n    }\n}\nx\ny\n' > ws.rs && mkdir -p src && printf 'one\ntwo\nthree\n' > src/net.rs && git add ws.rs src/net.rs && git commit -q -m init

j() { python3 -c "import sys,json; d=json.load(sys.stdin); print($1)"; }
pass=0; fail=0
check() { if [ "$2" = "$3" ]; then pass=$((pass+1)); echo "  ok   $1"; else fail=$((fail+1)); echo "  FAIL $1: expected [$3] got [$2]"; fi; }
inbox_types() { "$F" --as "$1" inbox --unread --format json 2>/dev/null | j "','.join(sorted(n['type'] for n in d))"; }
unread_ids() { "$F" --as "$1" inbox --unread --format json | j "' '.join(n['id'] for n in d)"; }
# The status code a WebSocket upgrade request for a path gets from the daemon (101: upgraded), sent
# to 127.0.0.1 or to the address given second, with that address as the Host, and with the origin
# given third, as a page's browser sends it, when there is one.
upgrade() { python3 - "${2:-127.0.0.1}" "$PORT" "$1" "${3:-}" <<'PY'
import socket, sys
address, port, path, origin = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4]
origin_line = f"Origin: {origin}\r\n" if origin else ""
s = socket.create_connection((address, port), timeout=5)
s.sendall(f"GET {path} HTTP/1.1\r\nHost: {address}:{port}\r\n{origin_line}Connection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n".encode())
print(s.recv(1024).decode(errors="replace").split()[1])
s.close()
PY
}
wait_for() { for _ in $(seq 1 50); do if [ "$(inbox_types "$1")" = "$2" ]; then return 0; fi; sleep 0.1; done; return 1; }

echo "== setup =="
"$F" init --name roles --handle dnorman --display-name Daniel >/dev/null
STATUS=$("$F" daemon status)
PORT=$(echo "$STATUS" | j "d['port']")
PROJECT=$(echo "$STATUS" | j "d['project']")   # this checkout's project
U="http://127.0.0.1:$PORT"
check "the port file holds the port, pid, instance, start and bind only" "$(j "','.join(sorted(d))" < "$FWD_RUNTIME_DIR/daemon.ws")" "bind,instance_id,pid,port,started"
check "only its owner can read the port file" "$(python3 -c "import os,sys; print(oct(os.stat(sys.argv[1]).st_mode & 0o777))" "$FWD_RUNTIME_DIR/daemon.ws")" "0o600"
SUTRA=$(python3 -c "import hashlib,sys; print(hashlib.sha256(sys.argv[1].encode()).hexdigest()[:16])" "$FWD_DATA_DIR")   # sutra's id for this data directory
check "sutra's registry holds the daemon's card and its one row, daemon, none per project" "$(ls "$FWD_DEV_RUNNER_DIR" | tr '\n' ' ')" "$SUTRA $SUTRA.daemon.status "
check "the card names the data directory, the daemon's pid, when it started and its port, nothing else" "$(sed -E 's/^STARTED=[0-9]+$/STARTED=<seconds>/' "$FWD_DEV_RUNNER_DIR/$SUTRA" | tr '\n' ' ')" "DIR=$FWD_DATA_DIR PID=$(j "d['pid']" < "$FWD_RUNTIME_DIR/daemon.ws") STARTED=<seconds> DAEMON_PORT=$PORT "
check "the daemon's row is ready, with its one project" "$(cat "$FWD_DEV_RUNNER_DIR/$SUTRA.daemon.status")" "ready: 1 project"
check "the daemon serves the built web UI" "$(curl -s "$U/" | grep -q 'fwd-web_bg.wasm' && echo served)" "served"
check "the wasm is served with its type" "$(curl -fsS -o /dev/null -w '%{http_code} %{content_type}' "$U/fwd-web_bg.wasm")" "200 application/wasm"
check "the script is served with its type" "$(curl -fsS -o /dev/null -w '%{http_code} %{content_type}' "$U/fwd-web.js")" "200 text/javascript"
check "the project's mark, the page's favicon, is served with its type" "$(curl -fsS -o /dev/null -w '%{http_code} %{content_type}' "$U/mark.png")" "200 image/png"
check "no other site may frame the page" "$(curl -sI "$U/" | tr -d '\r' | grep -ciE "^(x-frame-options: DENY|content-security-policy: frame-ancestors 'none')$")" "2"
check "a page from another origin is refused" "$(curl -s -o /dev/null -w '%{http_code}' -H 'Origin: https://evil.example' "$U/health")" "403"
check "a rebinding page's host is refused" "$(curl -s -o /dev/null -w '%{http_code}' -H "Host: evil.example:$PORT" "$U/health")" "403"
check "health names no project" "$(curl -s "$U/health" | j "','.join(sorted(d))")" "bind,build,instance_id,ok,pid,started"
check "stop from a page of another origin is refused" "$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'Origin: https://evil.example' "$U/stop")" "403"
check "the daemon still runs after that" "$(curl -s -o /dev/null -w '%{http_code}' "$U/health")" "200"
check "there is no socket at /ws" "$(curl -s -o /dev/null -w '%{http_code}' "$U/ws")" "404"
check "there is no socket under /k/" "$(curl -s -o /dev/null -w '%{http_code}' "$U/k/anything/ws")" "404"
check "the project's socket upgrades" "$(upgrade "/p/$PROJECT/ws")" "101"
check "a project the daemon does not host has no socket" "$(upgrade "/p/00000000-0000-0000-0000-000000000000/ws")" "404"
check "fwd ui prints the url with the project" "$("$F" ui --no-open)" "$U/?p=$PROJECT"
check "the daemon lists its projects" "$(curl -s "$U/projects" | j "','.join(p['name'] for p in d)")" "roles"
check "the project is listed on this machine" "$("$F" project list | j "','.join(p['name'] for p in d)")" "roles"
check "the daemon saw this checkout on its branch" "$("$F" checkouts --format json | j "len([c for c in d if c['branch']=='main'])")" "1"

echo "== the fixed roles, and who belongs to the project =="
check "fwd init makes no agent principal, and the person who ran it belongs" "$("$F" principal list --format json | j "','.join(p['handle'] + ' ' + (p['agent_role'] or p['kind']) for p in d)")" "dnorman human"
check "--as never makes a principal: a name the project lacks is refused with the command that makes it" "$("$F" --as impl-alice whoami 2>&1 | grep -c 'no principal of this project is named `impl-alice`: an agent names itself and selects its role with `fwd principal add impl-alice --role lead|triage|implementer|reviewer`')" "1"
check "and it made none" "$("$F" principal list --all --format json | j "len(d)")" "1"
check "an agent names itself and selects its role, from a session that acts as that name already" "$("$F" --as impl-alice principal add impl-alice --role implementer --format text)" "impl-alice (implementer)"
check "a new agent without a role is refused" "$("$F" principal add impl-x 2>&1 | grep -c 'an agent selects its role as it names itself: `fwd principal add impl-x --role lead|triage|implementer|reviewer`')" "1"
check "an agent that selects lead where no agent holds triage is told how to create one, on Sonnet" "$("$F" principal add claude --role lead 2>&1 >/dev/null | grep -c 'A triage agent is a Sonnet agent, which annotates and relays rather than judging the work (docs/DECISIONS.md, D129). From Claude Code, spawn it as a background Sonnet subagent, briefed with the fwd-triage skill')" "1"
check "so is the lead at its inbox" "$("$F" --as claude inbox --format json 2>&1 >/dev/null | grep -c 'no agent serves the whole project, so no triage agent answers')" "1"
check "and in the hook's context at its session's start" "$("$F" --as claude prime --format hook | grep -c 'no agent serves the whole project, so no triage agent answers')" "1"
Q0=$("$F" --as impl-alice ask "who takes the TLS task?" --format json 2>"$T/asked.err")
check "a question asked of no one, here where the checkout finds no scope, is marked as asked of the project, with no addressee" "$(echo "$Q0" | j "d['record']['context']")" "{'asks': 'project'}"
check "and while no triage agent answers for it it waits, which the asker is told with the guidance" "$(grep -c 'this question waits, asked of the project, until an agent serves it or the whole project, and reaches that agent then. This command works in no scope (no --scope, and no checkout that finds one), and no agent serves the whole project' "$T/asked.err")" "1"
# A triage agent serves a scope, or by a deliberate endorsement the whole project; with neither, the
# add is refused, naming the two ways out (docs/DECISIONS.md, D122).
check "an agent that would take triage and serve nothing is refused, naming --scope and --project-wide" "$("$F" --as triage-bot principal add triage-bot --role triage 2>&1 | grep -c "an agent that selects triage serves a scope, and \`@triage-bot\` would serve none, since the command names none and its checkout finds none, or several: name one, \`--scope <id>\`, .*; or endorse it to serve the whole project, \`--project-wide\`, which suits only a small project with small scopes")" "1"
TRIAGE=$("$F" --as triage-bot principal add triage-bot --role triage --project-wide --format json | j "d['record']['id']")   # the triage agent names itself, and this small project endorses it for the whole project
check "its JSON says it serves the whole project, and no scope" "$("$F" principal list --format json | j "[str(p['serves']['project_wide']) + ' ' + str(p['serves']['scopes']) for p in d if p['handle'] == 'triage-bot'][0]")" "True []"
wait_for triage-bot "question_asked" || true
check "the question that waited reaches the agent that takes triage, once" "$(inbox_types triage-bot)" "question_asked"
check "it is that question, for triage" "$("$F" --as triage-bot inbox --unread --format json | j "d[0]['message']['id'] + ' ' + d[0]['originator']['why']")" "$(echo "$Q0" | j "d['record']['id']") triage"
"$F" --as triage-bot read $(unread_ids triage-bot) >/dev/null   # read, it leaves the inbox as the checks below expect
check "a second agent for the whole project is refused, naming the one that serves it and how to free it" "$("$F" principal add triage-2 --role triage --project-wide 2>&1 | grep -c "@triage-bot serves the whole project already, and a project has one triage agent that serves it whole: withdraw its endorsement first (\`fwd principal scope project-wide --withdraw --for triage-bot\`), or retire it (\`fwd principal retire triage-bot\`)")" "1"
check "a second lead is another lead, a project having one per session, where fwd cannot tell two unstamped agents' sessions apart" "$("$F" principal role impl-alice lead --format text)" "impl-alice (lead)"
"$F" principal role impl-alice implementer >/dev/null   # impl-alice implements again, as the checks below expect
check "a person takes no role" "$("$F" principal role dnorman triage 2>&1 | grep -c 'is a person, and a role is an agent')" "1"
check "the lead's inbox says nothing more once an agent serves the whole project" "$("$F" --as claude inbox --format json 2>&1 >/dev/null | grep -c 'no triage agent answers')" "0"
check "principal list says what each is, the lead first among the agents, and what the triage agent serves" "$("$F" principal list --format text | tr '\n' ',')" "dnorman (person),claude (lead),triage-bot (triage, serving the whole project),impl-alice (implementer),"

echo "== a person muses on a task; the triage agent hears =="
T60=$("$F" task create "Connector" --why "the daemon needs a client" --priority 3 --needs-review --format json 2>"$T/priority.err" | j "d['record']['id']")
check "a task's number is still taken as its priority, in this build" "$("$F" task show "$T60" --format json | j "d['priority']")" "3"
check "saying that the next build takes only the words" "$(grep -c 'took `--priority 3` as the task.s number 3: a priority is now, next or later, which a task stores as 1, 2 and 3, and the next build refuses a number' "$T/priority.err")" "1"
check "a wrong priority is refused, naming the values taken" "$("$F" task create "Nowhen" --priority soon 2>&1 | grep -c "invalid value 'soon' for '--priority <PRIORITY>': a priority is now, next or later (a task stores them as 1, 2 and 3), or, in this build only, a task's number from 0 to 4")" "1"
check "by ask too, which takes the same values" "$("$F" ask "when?" --priority 7 2>&1 | grep -c "invalid value '7' for '--priority <PRIORITY>': a priority is now, next or later")" "1"
M1=$("$F" comment "$T60" "what if we retried on disconnect?" --format json | j "d['record']['id']")
wait_for triage-bot "task_commented" || true
check "the triage agent is notified of the task comment" "$(inbox_types triage-bot)" "task_commented"
check "author not notified" "$(inbox_types dnorman)" ""
ASKED=$("$F" ask --to @dnorman "does a question asked of myself reach my inbox?" --priority 4 --format json 2>"$T/ask-priority.err")
check "a question given a task's number keeps the word nearest it" "$(echo "$ASKED" | j "d['record']['priority']")" "later"
check "and says so, and that the next build takes only the words" "$(grep -c 'took `--priority 4` as later, the word nearest the task.s number 4: a priority is now, next or later, and the next build refuses a number' "$T/ask-priority.err")" "1"
wait_for dnorman "question_asked" || true
check "a question asked of oneself is in one's own inbox" "$(inbox_types dnorman)" "question_asked"
"$F" dismiss $("$F" inbox --unread --format json | j "' '.join(n['id'] for n in d)") >/dev/null   # dismissing it leaves his inbox as the checks below expect

echo "== the triage agent's pass =="
POP=$("$F" --as triage-bot inbox pop --format json)
N1=$(echo "$POP" | j "d[0]['id']")
check "pop marks read" "$(inbox_types triage-bot)" ""
T80=$("$F" --as triage-bot task create "Retry on disconnect" --why "from Daniel's comment" --from "$M1" --propose --priority now --needs-review --format json | j "d['record']['id']")
check "task created from a message is proposed" "$("$F" task show "$T80" --format json | j "d['status']")" "proposed"
check "a task's priority is a word, as a question's is, which it stores as its number: now as 1" "$("$F" task show "$T80" --format json | j "d['priority']")" "1"
wait_for dnorman "task_created" || true
check "the author is told the task exists (not a reply)" "$(inbox_types dnorman)" "task_created"
check "task_created points at the task" "$("$F" inbox --unread --format json | j "d[0]['target']['id']")" "$T80"
"$F" --as triage-bot link "$M1" concerns "$T80" >/dev/null
"$F" --as triage-bot task amend "$T80" --done-when-append "reconnect within 5s with backoff" >/dev/null
check "amend appended to done_when" "$("$F" task show "$T80" --format json | j "d['done_when']")" "reconnect within 5s with backoff"
"$F" --as triage-bot reply "$N1" "linked to the new task" >/dev/null
check "the triage agent's reply reaches the author as replied" "$("$F" inbox --unread --format json | j "','.join(sorted(n['type'] for n in d))")" "replied,task_created"
"$F" read $("$F" inbox --unread --format json | j "' '.join(n['id'] for n in d)") >/dev/null

echo "== the person approves; the implementer takes it =="
check "approve" "$("$F" task approve "$T80" --format json | j "d['status']")" "open"
S1=$("$F" --as impl-alice scope create "Retry work" --intention "make reconnects survive NAT drops" --format json | j "d['record']['id']")
check "who belongs, in the order the pickers list them" "$("$F" principal list --format json | j "','.join(p['handle'] for p in d)")" "dnorman,claude,triage-bot,impl-alice"
check "the scope names the checkout's branch by default" "$("$F" scope show "$S1" --format json | j "','.join(d['branches'])")" "main"
check "a session finds its scope by the checkout's branch" "$("$F" --as impl-alice whoami --format json | j "d['scope']")" "$S1"
check "so does anyone's, whoever made the scope" "$("$F" --as triage-bot whoami --format json | j "d['scope']")" "$S1"
check "a scope's JSON names who made it, and no owner or heartbeat" "$("$F" scope show "$S1" --format json | j "','.join(k for k in ('owner', 'last_heartbeat', 'heartbeat_ttl_s', 'created_by') if k in d)")" "created_by"
check "fwd scope heartbeat is gone" "$("$F" scope heartbeat "$S1" 2>&1 | grep -c "unrecognized subcommand 'heartbeat'")" "1"
"$F" --as impl-alice scope branches "$S1" --add feat/retry >/dev/null
check "a scope may span branches" "$("$F" scope show "$S1" --format json | j "','.join(d['branches'])")" "main,feat/retry"
# Each write stamps the principal it acts as with the time, which keeps an agent at work off the
# pickers' dormant list (docs/DECISIONS.md, D123); marking notifications stamps no one (below).
last_active() { "$F" principal list --format json | j "[p['last_active'] for p in d if p['handle'] == '$1'][0]"; }
check "an agent at work is not dormant, and its JSON says when it last wrote" "$("$F" principal list --format json | j "[str(p['dormant']) + ' ' + str(p['last_active'] is not None) for p in d if p['handle'] == 'impl-alice'][0]")" "False True"
STAMPED=$(last_active impl-alice)
"$F" --as impl-alice scope intention "$S1" --set "make reconnects survive NAT drops and daemon restarts" >/dev/null
check "its next write stamps it again, later" "$(python3 -c "print($(last_active impl-alice) > $STAMPED)")" "True"
S2=$("$F" scope create "Docs work" --intention "document the retry loop" --format json | j "d['record']['id']")   # the person's scope, naming main too
check "two active scopes naming the branch give a command no scope" "$("$F" --as impl-alice whoami --format json | j "d['scope']")" "None"
REFUSED=$("$F" --as impl-alice task start "$T80" 2>&1 || true)
check "task start there is refused, naming both scopes and how to say which" "$(echo "$REFUSED" | grep -c "^Error: several active scopes name this checkout's branch main: .*; say which you work in: run the command again with \`--scope <id>\`, or bind this checkout to one with \`fwd scope attach-dir <id>\`$") $(echo "$REFUSED" | grep -oE "\"Retry work\" \(${S1:0:8}\)|\"Docs work\" \(${S2:0:8}\)" | wc -l | tr -d ' ')" "1 2"
check "and starts nothing" "$("$F" task show "$T80" --format json | j "d['status']")" "open"
check "so is task next --claim" "$("$F" --as impl-alice task next --claim 2>&1 | grep -c "^Error: several active scopes name this checkout's branch main")" "1"
check "every other command runs, with no scope: next without --claim offers the task" "$("$F" --as impl-alice task next --format json | j "d['id']")" "$T80"
"$F" comment "$S2" "is the retry loop documented anywhere yet?" >/dev/null
wait_for triage-bot "task_commented" || true
check "a comment on a scope reaches the triage agent, whoever made the scope, saying how to answer it in its thread" "$("$F" --as triage-bot inbox --unread --format json | j "d[0]['reason'].replace(d[0]['id'][:8], '<its id>')")" "dnorman commented on the scope Docs work; reply with fwd reply <its id>"
STAMPED=$(last_active triage-bot)
"$F" --as triage-bot read $(unread_ids triage-bot) >/dev/null
check "marking one's notifications read stamps no one" "$(last_active triage-bot)" "$STAMPED"
"$F" --as impl-alice scope attach-dir "$S1" >/dev/null
check "attach-dir binds this checkout to the scope" "$("$F" checkouts --format json | j "[c['scope'] for c in d if c['branch']=='main'][0]")" "$S1"
check "a binding wins over the branch, for anyone" "$("$F" --as triage-bot whoami --format json | j "d['scope']")" "$S1"
"$F" --as impl-alice scope attach-dir "$S2" >/dev/null
check "anyone binds a checkout to any scope, the person's too" "$("$F" --as impl-alice whoami --format json | j "d['scope']")" "$S2"
"$F" --as impl-alice scope attach-dir "$S1" --clear >/dev/null
check "attach-dir --clear unbinds" "$("$F" checkouts --format json | j "[c['scope'] for c in d if c['branch']=='main'][0]")" "None"
"$F" scope status "$S2" paused >/dev/null   # one active scope names main again
check "a session still finds its scope by branch" "$("$F" --as impl-alice whoami --format json | j "d['scope']")" "$S1"
NEXT=$("$F" --as impl-alice --scope "$S1" task next --claim --format json)
check "next --claim picks the higher-priority approved task" "$(echo "$NEXT" | j "d['id']")" "$T80"
check "the claim works the task in the scope" "$("$F" task explain "$T80" --format json | j "d['worked_in']")" "$S1"
check "and makes the claimer its assignee" "$("$F" task show "$T80" --format json | j "d['assignee_handle']")" "impl-alice"
check "the claimer is not notified of their own claim" "$(inbox_types impl-alice)" ""
"$F" --as triage-bot link "$T80" anchored_at "$("$F" anchor create --path ws.rs --lines 4-8 --format json | j "d['record']['id']")" >/dev/null

echo "== the person comments on code the implementer is working on =="
"$F" comment --path ws.rs --lines 5-7 "back off, don't hammer" >/dev/null
check "an anchor path may live in a directory" "$("$F" anchor create --path src/net.rs --lines 1-2 --format json | j "d['record']['path']")" "src/net.rs"
check "anchors in a nested path are listed" "$("$F" anchor list --path src/net.rs --format json | j "len(d)")" "1"
check "an anchor path may not leave the checkout" "$("$F" anchor create --path ../outside.rs --lines 1-1 2>&1 | grep -c 'leaves the checkout')" "1"
check "an absolute path inside the checkout is stored relative" "$("$F" anchor create --path "$PWD/src/net.rs" --lines 2-3 --format json | j "d['record']['path']")" "src/net.rs"
check "anchors are listed by an absolute path too" "$("$F" anchor list --path "$PWD/src/net.rs" --format json | j "len(d)")" "2"
printf 'x\n' > "$T/outside.rs"
check "an absolute path outside the checkout is refused, saying so" "$("$F" anchor create --path "$T/outside.rs" --lines 1-1 2>&1 | grep -c 'is outside this checkout')" "1"
wait_for impl-alice "code_commented" || true
check "implementer hears the code comment (task anchored in this file)" "$(inbox_types impl-alice)" "code_commented"
check "as the assignee of the task near the lines" "$("$F" --as impl-alice inbox --unread --format json | j "d[0]['reason'].replace(d[0]['id'][:8], '<its id>')")" "dnorman commented on ws.rs:5-7, near Retry on disconnect, which is assigned to you; reply with fwd reply <its id>"
wait_for triage-bot "code_commented" || true
check "the triage agent hears it too" "$(inbox_types triage-bot)" "code_commented"
HOOK=$("$F" --as impl-alice --scope "$S1" inbox pop --format hook --hook-event PostToolBatch)
check "hook output names the event" "$(echo "$HOOK" | j "d['hookSpecificOutput']['hookEventName']")" "PostToolBatch"
check "hook context carries the comment" "$(echo "$HOOK" | j "'back off' in d['hookSpecificOutput']['additionalContext']")" "True"
check "stop-check passes after pop" "$("$F" --as impl-alice session stop-check --format hook)" "{}"
check "a hook written before scopes lost their heartbeat, passing --heartbeat, is refused, saying to take it out" "$("$F" --as impl-alice inbox pop --format hook --hook-event PostToolBatch --heartbeat 2>&1 >/dev/null | grep -c 'take `--heartbeat` out of the hook that passes it')" "1"
NA=$("$F" --as impl-alice inbox --format json | j "[n for n in d if n['type']=='code_commented'][0]['id']")
"$F" --as impl-alice reply "$NA" "added exponential backoff and a test" >/dev/null
wait_for dnorman "replied" || true
check "the person hears the implementer's reply" "$(inbox_types dnorman)" "replied"
"$F" read $("$F" inbox --unread --format json | j "' '.join(n['id'] for n in d)") >/dev/null

echo "== a question asked of the project travels implementer -> triage agent -> person -> implementer =="
"$F" --as triage-bot inbox pop --format json >/dev/null   # the triage agent's pass drains what it has heard so far
Q=$("$F" --as impl-alice ask --about "$T80" "should this also cover TLS handshakes?" --format json | j "d['record']['id']")
wait_for triage-bot "question_asked" || true
check "the triage agent gets the question asked of the project" "$(inbox_types triage-bot)" "question_asked"
NQ=$("$F" --as triage-bot inbox --unread --format json | j "d[0]['id']")
check "forward --to scope: is refused, a scope having no owner" "$("$F" --as triage-bot forward "$NQ" --to "scope:$S1" --note "yours" 2>&1 | grep -c '^Error: --to scope:<id> is refused: a scope has no owner to forward to')" "1"
FWD=$("$F" --as triage-bot forward "$NQ" --to @dnorman --note "this is a product call" --format json)
check "forward marks the triage agent's notification read" "$(inbox_types triage-bot)" ""
check "and names it as settled" "$(echo "$FWD" | j "d['settled']")" "$NQ"
check "the forward opens with the note and quotes the question" "$(echo "$FWD" | j "d['record']['content'].startswith('this is a product call') and '> should this also cover TLS' in d['record']['content']")" "True"
check "the forward stays in the question's thread" "$(echo "$FWD" | j "d['record']['thread']")" "$Q"
wait_for dnorman "question_asked" || true
check "the forward reaches the person as a question" "$(inbox_types dnorman)" "question_asked"
check "the forward says why it is the person's" "$("$F" inbox --unread --format json | j "d[0]['reason'].replace(d[0]['id'][:8], '<its id>')")" "triage-bot forwarded you impl-alice's question on Retry on disconnect; reply with fwd reply <its id>"
# A reply addressed to someone else is not for the thread's author, who sees it in the thread.
check "the implementer is not notified of a forward addressed to the person" "$(inbox_types impl-alice)" ""
NF=$("$F" inbox --unread --format json | j "d[0]['id']")
RE=$("$F" reply "$NF" "no, TLS is a separate task" --format json)
check "a reply through the notification marks it read, naming it as settled" "$(echo "$RE" | j "d['settled']") $("$F" inbox --format json | j "[n['status'] for n in d if n['id'] == '$NF'][0]")" "$NF read"
check "a notification is its recipient's alone to settle: another principal's is refused, naming whose it is" "$("$F" --as impl-alice dismiss "$NF" 2>&1 | grep -c "notification ${NF:0:8} is @dnorman's, not @impl-alice's, and a principal reads, dismisses, replies through or forwards only its own notifications")" "1"
wait_for impl-alice "replied" || true
check "the answer reaches the implementer" "$("$F" --as impl-alice inbox --unread --format json | j "','.join(n['message']['content'] for n in d)")" "no, TLS is a separate task"
"$F" --as impl-alice read $("$F" --as impl-alice inbox --unread --format json | j "' '.join(n['id'] for n in d)") >/dev/null

echo "== a person asks the project about lines of code =="
for who in dnorman impl-alice triage-bot; do "$F" --as "$who" read $(unread_ids "$who") >/dev/null; done   # earlier sections left some unread
QL=$("$F" ask --path ws.rs --lines 9-10 "why do x and y come after the loop?" --format json)
check "a question on lines asked of no one is asked of the project" "$(echo "$QL" | j "d['record']['context']['asks']")" "project"
check "its lines are a highlight whose thread it starts" "$("$F" anchor show "$(echo "$QL" | j "d['record']['anchor']")" --format json | j "'{}-{} {}'.format(d['start_line'] + 1, d['end_line'], [m['content'] for m in d['messages']])")" "9-10 ['why do x and y come after the loop?']"
wait_for triage-bot "question_asked" || true
check "the triage agent is asked it, once" "$(inbox_types triage-bot)" "question_asked"
check "the implementer whose task is anchored near the lines is not told of a question asked of someone else" "$(inbox_types impl-alice)" ""
for who in impl-alice triage-bot; do "$F" --as "$who" read $(unread_ids "$who") >/dev/null; done

echo "== a comment on the index's side of a diff anchors the lines the index has =="
printf 'one\nTWO\nthree\n' > src/net.rs && git add src/net.rs && printf 'zero\none\nTWO\nthree\n' > src/net.rs   # a change staged, and a line above it not
IL=$("$F" comment --path src/net.rs --lines 2-2 --against index "why upper case?" --format json | j "d['record']['anchor']")
check "the anchor records the diff's side, and the index's line" "$("$F" anchor show "$IL" --format json | j "'{} {}'.format(d['side'], d['anchored_text'])")" "base TWO"
check "its basis is the index's blob" "$("$F" anchor show "$IL" --format json | j "d['basis_blob']")" "$(git rev-parse :src/net.rs)"
check "it is exact in the index" "$("$F" anchor show "$IL" --against index --format json | j "'{} {}'.format(d['resolved']['label'], d['resolved']['start_line'] + 1)")" "exact 2"
check "and a line down in the working tree" "$("$F" anchor show "$IL" --format json | j "'{} {}'.format(d['resolved']['label'], d['resolved']['start_line'] + 1)")" "shifted 3"
git checkout -q HEAD -- src/net.rs   # as committed again, for what follows
wait_for triage-bot "code_commented" || true
"$F" --as triage-bot read $(unread_ids triage-bot) >/dev/null

echo "== submit and review =="
"$F" --as triage-bot read $("$F" --as triage-bot inbox --unread --format json | j "' '.join(n['id'] for n in d)") >/dev/null   # the person's answer to its forward, heard already
check "submit" "$("$F" --as impl-alice task submit "$T80" --evidence "commit abc, tests green" --format json | j "d['status']")" "in_review"
wait_for triage-bot "review_requested" || true
check "review requested reaches the triage agent first" "$(inbox_types triage-bot)" "review_requested"
check "the person who approved the task is not asked while the triage agent can be" "$(inbox_types dnorman)" ""
check "the review says why it reached the triage agent, which judges no evidence" "$("$F" --as triage-bot inbox --unread --format json | j "d[0]['reason']")" "impl-alice submitted Retry on disconnect for review; you triage the project"
check "self-acceptance refused" "$("$F" --as impl-alice task accept "$T80" 2>&1 | grep -c 'own submission')" "1"
check "accept by the person" "$("$F" task accept "$T80" --format json | j "d['status']")" "done"
"$F" read $("$F" inbox --unread --format json | j "' '.join(n['id'] for n in d)") >/dev/null

echo "== watch sees a live event =="
"$F" --as impl-alice watch --for impl-alice --timeout 5 > "$T/watch.jsonl" 2>/dev/null & WATCH=$!; sleep 1
"$F" comment "$T80" "nice" >/dev/null   # a comment on a done task still notifies its assignee
for _ in $(seq 1 50); do grep -q '"event":"notification"' "$T/watch.jsonl" 2>/dev/null && break; sleep 0.1; done
check "watch streamed the live notification to the implementer" "$(grep -c '"event":"notification"' "$T/watch.jsonl" || true)" "1"
check "the streamed event carries the comment" "$(grep '"event":"notification"' "$T/watch.jsonl" | head -1 | j "d['message']['content']")" "nice"
"$F" --as impl-alice session stop-check --format hook | j "d.get('decision','')" | sed 's/^/  stop-check for alice now: /'

echo "== prime =="
"$F" --as impl-alice --scope "$S1" prime --format hook | head -3 | sed 's/^/  /'

wait "$WATCH" 2>/dev/null || true   # the watch reaches its timeout before the daemon goes away

echo "== watch --count 1 ends at the first notification, new or already waiting =="
events() { python3 -c "import sys,json; print(','.join(json.loads(l)['event'] for l in sys.stdin))"; }   # a watch's event names, in order
"$F" principal add impl-bob --role implementer >/dev/null   # a new agent, with nothing waiting
"$F" --as impl-bob watch --count 1 --format json > "$T/watch-count.jsonl" 2>/dev/null & WAITING=$!; sleep 1   # as an agent waits: in the background, with no timeout
"$F" --as triage-bot ask --to @impl-bob "can you take the TLS task?" >/dev/null
for _ in $(seq 1 100); do kill -0 "$WAITING" 2>/dev/null || break; sleep 0.1; done
kill "$WAITING" 2>/dev/null || true   # a watch still waiting ten seconds after the question missed it, and fails the next check
EXITED=0; wait "$WAITING" || EXITED=$?
check "with nothing waiting, watch --count 1 exits with status 0" "$EXITED" "0"
check "a new question ends it: the notification, then done" "$(events < "$T/watch-count.jsonl")" "notification,done"
check "the question, still unread, ends the next watch --count 1 at once: a snapshot, then done" "$("$F" --as impl-bob watch --count 1 --timeout 20 --format json 2>/dev/null | events)" "snapshot,done"   # the timeout only bounds a watch that does not end at once, so that it fails the check rather than hangs the script

echo "== short ids =="
check "a task is found by the first 8 characters of its id" "$("$F" task show "${T80:0:8}" --format json | j "d['id']")" "$T80"
check "fewer than 6 characters are refused" "$("$F" task show "${T80:0:5}" 2>&1 | grep -c 'first 6 characters or more')" "1"

echo "== labels: a task's kind, given when it is filed and changed afterwards =="
TL=$("$F" task create "Fix the hover preview" --label Bug --format json | j "d['record']['id']")
check "a label is stored in lower case" "$("$F" task show "$TL" --format json | j "','.join(d['labels'])")" "bug"
check "amend adds labels, commas between them" "$("$F" task amend "$TL" --label feature,ui --format json | j "','.join(d['labels'])")" "bug,feature,ui"
check "amend takes one off, however it is written" "$("$F" task amend "$TL" --unlabel UI --format json | j "','.join(d['labels'])")" "bug,feature"
check "task list --label keeps the tasks with the label" "$("$F" task list --label BUG --format json | j "','.join(t['id'] for t in d)")" "$TL"
check "task show prints the labels for people" "$("$F" task show "$TL" --format text | grep -c '· labels: bug, feature$')" "1"
check "a blank label is refused" "$("$F" task amend "$TL" --label ' ' 2>&1 | grep -c 'a label cannot be blank')" "1"

echo "== a review: assignment, steps and inline references =="
for who in dnorman impl-alice triage-bot; do "$F" --as "$who" read $(unread_ids "$who") >/dev/null; done   # earlier sections left some unread
RV=$("$F" task create "Review: the retry loop" --why "read it before it ships" --format json | j "d['record']['id']")
check "a new task has no assignee" "$("$F" task show "$RV" --format json | j "d['assignee']")" "None"
"$F" task assign "$RV" @impl-alice >/dev/null
wait_for impl-alice "task_assigned" || true
check "assigning it tells the assignee" "$(inbox_types impl-alice)" "task_assigned"
check "saying who assigned it" "$("$F" --as impl-alice inbox --unread --format json | j "d[0]['reason']")" "Review: the retry loop was assigned to you by dnorman"
"$F" --as impl-alice read $(unread_ids impl-alice) >/dev/null
check "--mine lists it for its assignee" "$("$F" --as impl-alice task list --mine --format json | j "','.join(t['title'] for t in d)")" "Review: the retry loop"
check "and for no one else" "$("$F" task list --mine --format json | j "len(d)")" "0"
check "no one else starts it" "$("$F" task start "$RV" 2>&1 | grep -c 'assigned to @impl-alice')" "1"
STEP=$("$F" --as impl-alice step add "$RV" --path ws.rs --lines 4-8 "the loop that retries; see [[${T80:0:8}]], and @dnorman for the cap" --format json)
check "step add makes a highlight the first step" "$(echo "$STEP" | j "'{} {} {}:{}'.format(d['record']['n'], d['record']['kind'], d['record']['path'], d['record']['lines'])")" "1 anchor ws.rs:4-8"
check "its first comment stores what it names as ids" "$(echo "$STEP" | j "d['message']['content'] == 'the loop that retries; see [[$T80]], and [[{}]] for the cap'.format(d['message']['references'][1]['id'])")" "True"
check "and lists them by kind and name" "$(echo "$STEP" | j "','.join(r['kind'] + ':' + r['name'] for r in d['message']['references'])")" "task:Retry on disconnect,principal:@dnorman"
check "message show names each reference" "$("$F" message show "$(echo "$STEP" | j "d['message']['id']")" --format text | tail -1)" "the loop that retries; see [Retry on disconnect], and @dnorman for the cap"
wait_for dnorman "mentioned,step_added" || true
check "the person it names hears of the comment once, as a mention, and of the step on his review apart" "$(inbox_types dnorman)" "mentioned,step_added"
"$F" read $(unread_ids dnorman) >/dev/null
"$F" --as impl-alice step add "$RV" "$T80" >/dev/null
check "a task is a step too, after the highlight" "$("$F" step list "$RV" --format json | j "','.join(s['kind'] for s in d)")" "anchor,task"
check "step check checks it, saying who" "$("$F" step check "$RV" 1 --format json | j "'{} {}'.format(d['checked'], d['checked_by_handle'])")" "True dnorman"
FIRST=$(echo "$STEP" | j "d['message']['id']")   # the highlight's first thread
check "checking the highlight's step resolves its first thread" "$("$F" message show "$FIRST" --format json | j "d['status']")" "resolved"
check "task show prints the checklist with its progress" "$("$F" task show "$RV" --format text | grep -c '^steps: 1 of 2 checked$')" "1"
check "step uncheck unchecks it" "$("$F" step uncheck "$RV" 1 --format json | j "d['checked']")" "False"
check "and sets the thread back to active" "$("$F" message show "$FIRST" --format json | j "d['status']")" "active"
check "resolving the step's first thread through the CLI checks the step, saying so" "$("$F" message resolve "$FIRST" --format json | j "'{} {}'.format(d['status'], [(s['task'], s['checked']) for s in d['checked_steps']] == [('$RV', True)])")" "resolved True"
check "step list shows the step checked and its thread resolved" "$("$F" step list "$RV" --format json | j "'{} {}'.format(d[0]['checked'], d[0]['thread']['status'])")" "True resolved"
check "unresolving it unchecks the step" "$("$F" message unresolve "$FIRST" --format json | j "'{} {}'.format(d['status'], [s['checked'] for s in d['unchecked_steps']])")" "active [False]"
check "and the progress follows" "$("$F" task show "$RV" --format json | j "'{checked} of {total}'.format(**d['progress'])")" "0 of 2"
LATER=$("$F" comment "$(echo "$STEP" | j "d['record']['id']")" "why no jitter?" --format json | j "d['record']['id']")
wait_for impl-alice "code_commented" || true
check "a comment on the highlight reaches the review's assignee" "$(inbox_types impl-alice)" "code_commented"
check "saying whose step it is" "$("$F" --as impl-alice inbox --unread --format json | j "d[0]['reason'].replace(d[0]['id'][:8], '<its id>')")" "dnorman commented on ws.rs:4-8, a step of your task Review: the retry loop; reply with fwd reply <its id>"
check "resolving that later thread leaves the step as it was" "$("$F" message resolve "$LATER" --format json | j "len(d['checked_steps'])") $("$F" step list "$RV" --format json | j "d[0]['checked']")" "0 False"
check "its assignee starts it in her scope" "$("$F" --as impl-alice task start "$RV" --format json | j "d['status'] + ' ' + d['worked_in']")" "in_progress $S1"
for who in dnorman impl-alice triage-bot; do "$F" --as "$who" read $(unread_ids "$who") >/dev/null; done
ANCHOR=$(echo "$STEP" | j "d['record']['id']")
check "only the review's creator and assignee take a step off it" "$("$F" --as triage-bot step remove "$RV" 2 --reason "moved" 2>&1 | grep -c 'take a step off its checklist; ask @dnorman or @impl-alice$')" "1"
check "step remove takes a step off, named by the id of what it is, and prints the checklist left" "$("$F" --as impl-alice step remove "$RV" "${T80:0:8}" --reason "moved to its own review" --format json | j "'{} {} {}'.format(d['removed']['kind'], d['removed']['reason'], ','.join(s['kind'] for s in d['steps']))")" "task moved to its own review anchor"
check "a step not on the review's checklist is refused" "$("$F" step remove "$RV" "${T80:0:8}" --reason "again" 2>&1 | grep -c "names no step on this task's checklist")" "1"
check "--retire resolves the highlight's open thread as its step comes off" "$("$F" step remove "$RV" 1 --reason "replaced by smaller reviews" --retire --format json | j "'{} {}'.format([m['id'] for m in d['resolved']] == ['$FIRST'], len(d['steps']))")" "True 0"
check "the highlight still lists its threads, resolved" "$("$F" anchor show "$ANCHOR" --format json | j "','.join(m['status'] for m in d['messages'] if not m['thread'])")" "resolved,resolved"
check "the step's relation keeps the reason" "$("$F" relations "$ANCHOR" --kind step_of --format json | j "','.join(r['status'] + ': ' + (r['closed_reason'] or '') for r in d if r['to'] == '$RV')")" "retracted: replaced by smaller reviews"
sleep 1
check "taking steps off tells no one" "$(inbox_types dnorman)$(inbox_types impl-alice)$(inbox_types triage-bot)" ""
"$F" task unassign "$RV" >/dev/null
check "unassign leaves it with no one" "$("$F" task show "$RV" --format json | j "d['assignee']")" "None"

echo "== a burst from one author on one task is one notification =="
for who in dnorman impl-alice triage-bot; do "$F" --as "$who" read $(unread_ids "$who") >/dev/null; done   # earlier sections left some unread
reasons() { "$F" --as "$1" inbox --unread --format json 2>/dev/null | j "' | '.join(n['reason'] for n in d)"; }
until_said() { for _ in $(seq 1 50); do if [ "$(reasons "$1")" = "$2" ]; then return 0; fi; sleep 0.1; done; return 1; }
RB=$("$F" --as impl-alice task create "Review: the backoff" --format json | j "d['record']['id']")
"$F" --as impl-alice task assign "$RB" @dnorman >/dev/null
wait_for dnorman "task_assigned" || true
"$F" read $(unread_ids dnorman) >/dev/null
for n in 1 2 3; do "$F" --as impl-alice step add "$RB" --path ws.rs --lines "$n-$n" "line $n: why here?" >/dev/null; done
BURST="impl-alice added 3 steps and commented 3 times on your task Review: the backoff"
until_said dnorman "$BURST" || true
check "three steps with their first comments reach the assignee as one notification, which says so" "$(reasons dnorman)" "$BURST"
check "fwd inbox --for prints it as one item" "$("$F" --as triage-bot inbox --for dnorman --unread --format text | grep -c "^$BURST (digest, ")" "1"
check "which lists each step beneath it with its first comment" "$("$F" --as triage-bot inbox --for dnorman --unread --format text | tail -n +2 | tr '\n' '|')" "    [ws.rs:1] line 1: why here?|    [ws.rs:2] line 2: why here?|    [ws.rs:3] line 3: why here?|"
watched() { python3 -c "import sys,json; print(' | '.join(e['event'] + (': ' + e['notification']['reason'] if 'notification' in e else '') for e in map(json.loads, sys.stdin)))"; }   # a watch's events, each with its notification's reason
check "fwd watch prints it as one item" "$("$F" --as triage-bot watch --for dnorman --count 1 --timeout 20 --format json 2>/dev/null | watched)" "snapshot: $BURST | done"
"$F" read $(unread_ids dnorman) >/dev/null
"$F" --as impl-alice step add "$RB" --path ws.rs --lines 9-9 "and here" >/dev/null
until_said dnorman "impl-alice added a step and commented on your task Review: the backoff" || true
check "once it is read, the next step and its comment make a new one" "$(reasons dnorman)" "impl-alice added a step and commented on your task Review: the backoff"
"$F" read $(unread_ids dnorman) >/dev/null
WHOLE=$("$F" --as impl-alice step add "$RB" --path src/net.rs "read the whole file after the lines" --format json)
check "step add --path without --lines makes the whole file a step" "$(echo "$WHOLE" | j "'{} {} {} {}'.format(d['record']['n'], d['record']['path'], d['record']['whole_file'], d['record']['lines'])")" "5 src/net.rs True None"
until_said dnorman "impl-alice added a step and commented on your task Review: the backoff" || true
check "made before its first comment, the step lets that comment reach the task's assignee with it" "$(reasons dnorman)" "impl-alice added a step and commented on your task Review: the backoff"
check "which lists the whole file with its first comment" "$("$F" --as triage-bot inbox --for dnorman --unread --format text | tail -n +2)" "    [src/net.rs] read the whole file after the lines"

echo "== decisions: recorded, cited, superseded, repealed, exported =="
for who in dnorman impl-alice triage-bot; do "$F" --as "$who" read $(unread_ids "$who") >/dev/null; done   # earlier sections left some unread
DA=$("$F" decision create "Retries back off exponentially" --area architecture --decision "back off exponentially, capped at 30s" --format json)
NA=$(echo "$DA" | j "d['record']['number']")
check "a person's own ruling is accepted and decided by them" "$(echo "$DA" | j "d['record']['status']") $("$F" decision show "D$NA" --format json | j "d['decided_by_handle']")" "accepted dnorman"
TC=$("$F" --as impl-alice task create "Cap the retry delay" --format json | j "d['record']['id']")
"$F" --as impl-alice cite "$TC" "D$NA" >/dev/null
check "a task cites the ruling by its number" "$("$F" decision show "D$NA" --format json | j "','.join(c['id'] for c in d['cited_by'])")" "$TC"
check "a task is worked in a scope, never in a decision, which link refuses, naming the kinds" "$("$F" --as impl-alice link "$TC" worked_in "D$NA" 2>&1 | grep -c 'refused: `worked_in` joins a task to a scope, not a task to a decision')" "1"
check "task create refuses a relation its flag would make to what the relation does not join, as link does, and makes no task" "$("$F" --as impl-alice task create "Motivated by a ruling" --motivated-by "D$NA" 2>&1 | grep -c 'refused: `motivated_by` joins a task to a task or a message, not a task to a decision') $("$F" task list --format json | j "len([t for t in d if t['title'] == 'Motivated by a ruling'])")" "1 0"
check "and so it does for a whole id of another kind, which the CLI passes through" "$("$F" --as impl-alice task create "After a ruling" --after "$(echo "$DA" | j "d['record']['id']")" 2>&1 | grep -c 'refused: `depends_on` joins a task to a task, not a task to a decision')" "1"
check "list prints D<number>, status, area and title" "$("$F" decision list --format text | grep -cE "^D$NA +accepted +architecture +Retries back off exponentially$")" "1"
check "list filters by area, whatever its case" "$("$F" decision list --area Architecture --format json | j "len(d)")" "1"
NB=$("$F" decision create "Retries back off with full jitter" --area architecture --decision "full jitter" --format json | j "d['record']['number']")
"$F" decision supersede "D$NA" "D$NB" --note "jitter avoids stampedes, as [[${TC:0:8}]] found" >/dev/null
check "supersede marks the old ruling and names the new one" "$("$F" decision show "D$NA" --format json | j "d['status'] + ' ' + d['superseded_by'][0]['name']")" "superseded D$NB Retries back off with full jitter"
check "the reason is kept as a comment on it, the task it names by its whole id" "$("$F" decision show "D$NA" --format json | j "d['comments'][0]['content']")" "Superseded by D$NB: jitter avoids stampedes, as [[$TC]] found"
wait_for impl-alice "decision_superseded" || true
check "the citing task's author hears of the supersession" "$(inbox_types impl-alice)" "decision_superseded"
"$F" --as impl-alice read $(unread_ids impl-alice) >/dev/null
"$F" --as impl-alice cite "$TC" "D$NB" >/dev/null
"$F" decision repeal "D$NB" --note "the transport retries by itself now" >/dev/null
wait_for impl-alice "decision_repealed" || true
check "repeal notifies the citing task's author" "$(inbox_types impl-alice)" "decision_repealed"
check "with the reason" "$("$F" --as impl-alice inbox --unread --format json | j "d[0]['message']['content']")" "Repealed: the transport retries by itself now"
check "whoever repealed it is not notified" "$(inbox_types dnorman)" ""
"$F" --as impl-alice read $(unread_ids impl-alice) >/dev/null
NT=$("$F" --as impl-alice decision create "Use tokio in the client" --area dependency --propose --format json | j "d['record']['number']")
wait_for dnorman "decision_proposed" || true
check "an agent's proposal reaches the person" "$(inbox_types dnorman)" "decision_proposed"
wait_for triage-bot "decision_proposed" || true
check "and the triage agent" "$(inbox_types triage-bot)" "decision_proposed"
for who in dnorman triage-bot; do "$F" --as "$who" read $(unread_ids "$who") >/dev/null; done
check "a proposal is not accepted on an agent's word" "$("$F" --as triage-bot decision accept "D$NT" --decided-by @impl-alice --note "fine" 2>&1 | grep -c 'is an agent, not a person')" "1"
DT=$("$F" --as triage-bot decision accept "D$NT" --decided-by dnorman --note "Daniel: tokio is fine in the client" --format json)
check "accepted on the person's word, it names them as its decider, and the note is the triage agent's" "$(echo "$DT" | j "d['status'] + ' ' + d['decided_by_handle'] + ' ' + d['comments'][0]['author_handle']")" "accepted dnorman triage-bot"
EXPORT=$("$F" decision export)
check "export says it is rendered" "$(echo "$EXPORT" | head -1 | grep -c 'Rendered by `fwd decision export`')" "1"
check "export prints the entry" "$(echo "$EXPORT" | grep -cF -e "- **D$NB: Retries back off with full jitter** (architecture, ")" "1"
check "export names who ruled on a proposal and who filed it" "$(echo "$EXPORT" | grep -cE "^- \*\*D$NT: Use tokio in the client\*\* \(dependency, [0-9-]+, decided by dnorman, recorded by impl-alice\)")" "1"
check "export gives why a ruling closed with the names its note named" "$(echo "$EXPORT" | grep -cE "^  - Superseded by D$NB on [0-9-]+ \(dnorman\): jitter avoids stampedes, as \[Cap the retry delay\] found$")" "1"
check "and so does show's text" "$("$F" decision show "D$NA" --format text | grep -cxF "why closed: jitter avoids stampedes, as [Cap the retry delay] found")" "1"

echo "== an author edits a message where it stands =="
for who in dnorman impl-alice impl-bob triage-bot; do "$F" --as "$who" read $(unread_ids "$who") >/dev/null; done   # earlier sections left some unread
QE=$("$F" --as triage-bot ask --to @dnorman "does @impl-alice take the TLS task?" --format json | j "d['record']['id']")
wait_for impl-alice "mentioned" || true
for who in dnorman impl-alice; do "$F" --as "$who" read $(unread_ids "$who") >/dev/null; done   # the question and its mention, heard
check "only its author edits a message, or an agent that succeeded its author" "$("$F" message edit "$QE" "mine now" 2>&1 | grep -c "only its author edits a message, or an agent that succeeded its author, and this one is @triage-bot's; reply to it instead, or ask @triage-bot to edit it")" "1"
EDITED=$("$F" --as triage-bot message edit "$QE" "does @impl-bob take the TLS task, as [[D$NA]] asked?" --format json)
check "an edit keeps the message's id, records when and by whom, and resolves the new text's names" "$(echo "$EDITED" | j "d['id'] == '$QE' and d['edited'] >= d['created'] and d['edited_by'] == d['author'] and [r['name'] for r in d['references']] == ['@impl-bob', 'D$NA (Retries back off exponentially)']")" "True"
wait_for impl-bob "mentioned" || true
check "the one the edit names newly hears of it; the one it no longer names, and the one asked, do not" "$(inbox_types impl-bob)/$(inbox_types impl-alice)/$(inbox_types dnorman)" "mentioned//"
echo "== decisions: attached to the scope whose work they govern, else project-wide =="
wait_for dnorman "decision_commented" || true   # the triage agent's acceptance, told to its decider and its filer
wait_for impl-alice "decision_commented" || true
for who in dnorman impl-alice triage-bot; do "$F" --as "$who" read $(unread_ids "$who") >/dev/null; done
check "a decision governs the whole project unless it is attached" "$("$F" decision show "D$NT" --format json | j "d['scope']")" "None"
check "attach takes the scope's title, whatever its case" "$("$F" --as impl-alice decision attach "D$NT" "retry work" --format json | j "d['scope'] + ' ' + d['scope_title']")" "$S1 Retry work"
NS=$("$F" decision create "Reconnects log each attempt" --area process --attach "${S1:0:8}" --format json | j "d['record']['number']")
check "create --attach attaches the new ruling by the start of the scope's id" "$("$F" decision show "D$NS" --format json | j "d['scope']")" "$S1"
check "list --attached lists that scope's rulings" "$("$F" decision list --attached "$S1" --format json | j "','.join(x['ref'] for x in d)")" "D$NT,D$NS"
check "list --project-wide lists the rulings attached to none" "$("$F" decision list --project-wide --format json | j "','.join(x['ref'] for x in d)")" "D$NA,D$NB"
check "both list every ruling that applies to the scope's work" "$("$F" decision list --attached "Retry work" --project-wide --format json | j "','.join(x['ref'] for x in d)")" "D$NA,D$NB,D$NT,D$NS"
check "list names the scope a ruling governs" "$("$F" decision list --format text | grep -cE "^D$NT +accepted +dependency +Use tokio in the client \(governs Retry work\)$")" "1"
check "show says what a ruling governs" "$("$F" decision show "D$NA" --format text | grep -c ' · governs the whole project · ')" "1"
check "an unknown scope is refused, saying so" "$("$F" decision attach "D$NS" "Retry wrok" 2>&1 | grep -c 'no scope has the title `Retry wrok` or an id starting with it')" "1"
NS_ID=$("$F" decision show "D$NS" --format json | j "d['id']")
check "so is a whole id that names no scope, which list would otherwise answer with nothing" "$("$F" decision list --attached "$NS_ID" 2>&1 | grep -c "no scope of this project has the id $NS_ID")" "1"
check "export lists a scope's rulings under its name, after the project-wide ones" "$("$F" decision export | python3 -c "import sys; t = sys.stdin.read(); print(0 <= t.find('\n## Project-wide\n') < t.find('\n## Retry work\n') < t.find('- **D$NS: Reconnects log each attempt** (process, governs Retry work, '))")" "True"
"$F" decision detach "D$NS" >/dev/null
check "detach makes a ruling project-wide again" "$("$F" decision show "D$NS" --format json | j "d['scope']")" "None"
# Attaching and detaching tell nobody. Routing handles changes in the order they arrive, so once
# the comment that follows has reached its decider, anything they caused would be there too.
"$F" --as impl-alice comment "D$NS" "does this cover the first attempt?" >/dev/null
wait_for dnorman "decision_commented" || true
check "attaching and detaching told nobody; the comment after them reached its decider" "$(inbox_types dnorman)/$(inbox_types impl-alice)/$(inbox_types triage-bot)" "decision_commented//"
"$F" read $(unread_ids dnorman) >/dev/null

echo "== retiring a principal, and adding one back =="
"$F" --as impl-carol principal add impl-carol --role reviewer >/dev/null
T_CAROL=$("$F" --as impl-carol task create "Carol's first task" --format json | j "d['record']['id']")
check "an agent belongs from its addition" "$("$F" principal list --format json | j "'impl-carol' in [p['handle'] for p in d]")" "True"
check "retire leaves it out of who belongs, and says so with --all" "$("$F" principal retire impl-carol --format text) / $("$F" principal list --format json | j "'impl-carol' in [p['handle'] for p in d]")" "impl-carol (reviewer, retired) / False"
check "a retired agent's write is refused, naming the command that brings it back" "$("$F" --as impl-carol comment "$T_CAROL" "still here" 2>&1 | grep -c '`@impl-carol` is retired, and a retired principal writes nothing, since no picker offers it: to write again, bring it back first with `fwd --as impl-carol principal add impl-carol --role reviewer`, run as it')" "1"
check "and it stays retired, its reads running as before" "$("$F" principal list --format json | j "'impl-carol' in [p['handle'] for p in d]") $("$F" --as impl-carol task show "$T_CAROL" --format json | j "d['title']")" "False Carol's first task"
check "no one else brings it back: its name is taken, whose it is said" "$("$F" principal add impl-carol 2>&1 | grep -c 'the name `impl-carol` is taken: .* (reviewer, retired, in .*) has it in this project, and only that principal, acting as itself')" "1"
check "principal add, acting as it, brings it back, with the role it held" "$("$F" --as impl-carol principal add impl-carol --format text)" "impl-carol (reviewer)"
check "and back, it writes again" "$("$F" --as impl-carol comment "$T_CAROL" "back again" --format json | j "d['record']['content']")" "back again"
check "an agent selects another role" "$("$F" principal role impl-carol implementer --format text)" "impl-carol (implementer)"
check "and triage, serving the scope its checkout finds, which no other agent serves" "$("$F" principal role impl-carol triage --format text)" "impl-carol (triage, serving Retry work)"
check "a second triage agent for that scope is refused, naming the one that serves it" "$("$F" principal add triage-x --role triage 2>&1 | grep -c "the scope \"Retry work\" (.*) has a triage agent already, @impl-carol, and a scope has one")" "1"
check "another role frees the scope, which the agent that serves the whole project answers for again" "$("$F" principal role impl-carol implementer 2>&1 >/dev/null | grep -c "@triage-bot, which serves the whole project, answers first for the scope \"Retry work\"")" "1"
check "an agent added before its first write belongs" "$("$F" principal add impl-dave --role implementer --format json | j "d['record']['belongs']")" "True"
check "retiring the agent that serves the whole project says none does now, and what waits" "$("$F" principal retire triage-bot 2>&1 >/dev/null | grep -c 'no agent serves the whole project now: the questions asked of no scope, or of a scope no triage agent serves, wait on the Questions page')" "1"
check "and principal add, acting as it, brings it back, holding triage and serving what it served" "$("$F" --as triage-bot principal add triage-bot --format text)" "triage-bot (triage, serving the whole project)"

echo "== the supervisor role's old word, read this build only =="
"$F" --as triage-bot read $(unread_ids triage-bot) >/dev/null   # impl-carol's comment reached it
OLD=$("$F" --as impl-alice ask --to supervisor "is the old word still taken?" --format json 2>"$T/old-word.err" | j "d['record']['context']['asks']")
check "--to supervisor asks no one in particular" "$OLD" "project"
check "and says so on standard error, with what to write" "$(grep -c "took .--to supervisor. as a question asked of no one in particular, which the triage agent of the scope it concerns answers: .* so leave --to off to ask no one in particular" "$T/old-word.err")" "1"
wait_for triage-bot "question_asked" || true
check "which reaches the triage agent" "$(inbox_types triage-bot)" "question_asked"
check "--as supervisor is a name like any other, which no principal here has" "$("$F" --as supervisor whoami 2>&1 | grep -c 'no principal of this project is named `supervisor`')" "1"
"$F" --as triage-bot read $(unread_ids triage-bot) >/dev/null

echo "== triage per scope =="
# A triage agent serves the scopes it takes, one agent to a scope, and the one endorsed for the
# whole project takes what no scope's own agent does (docs/DECISIONS.md, D122): each scope's
# questions reach its own agent; a question asked of a scope no agent answers for waits, and
# reaches the agent that takes the scope; succession moves the scopes; and an agent left serving
# nothing is told so on each of its commands. The new scopes name branches of their own, so this
# checkout finds the scope it found before.
for who in dnorman impl-alice triage-bot; do "$F" --as "$who" read $(unread_ids "$who") >/dev/null; done
SR=$("$F" --as impl-alice scope create "Replication" --branch feat/replication --format json | j "d['record']['id']")
SS=$("$F" --as impl-alice scope create "Storage" --branch feat/storage --format json | j "d['record']['id']")
check "a triage agent takes the scope --scope names" "$("$F" --as triage-repl --scope "$SR" principal add triage-repl --role triage --format text)" "triage-repl (triage, serving Replication)"
check "a second agent for that scope is refused, naming the one that serves it and how to free it" "$("$F" --scope "$SR" principal add triage-r2 --role triage 2>&1 | grep -c "the scope \"Replication\" (${SR:0:8}) has a triage agent already, @triage-repl, and a scope has one: free the scope first (\`fwd principal scope remove ${SR:0:8} --for triage-repl\`), or retire @triage-repl (\`fwd principal retire triage-repl\`), for \`@triage-r2\` to serve it")" "1"
check "another scope takes another agent" "$("$F" --as triage-store --scope "$SS" principal add triage-store --role triage --format text)" "triage-store (triage, serving Storage)"
"$F" --as impl-alice ask --scope "$SR" "does the relay replicate?" >/dev/null
"$F" --as impl-alice ask --scope "$SS" "is SQLite enough?" >/dev/null
wait_for triage-repl "question_asked" || true
wait_for triage-store "question_asked" || true
check "a question asked of a scope reaches that scope's triage agent alone" "$(inbox_types triage-repl)/$(inbox_types triage-store)/$(inbox_types triage-bot)" "question_asked/question_asked/"
check "which says which scope it asks" "$("$F" --as triage-repl inbox --unread --format json | j "d[0]['reason'].replace(d[0]['id'][:8], '<its id>')")" "impl-alice asked the scope Replication “does the relay replicate?”; reply with fwd reply <its id>"
"$F" --as impl-alice ask --scope "$S1" "who cuts the release?" >/dev/null
wait_for triage-bot "question_asked" || true
check "one asked of a scope no agent serves reaches the agent that serves the whole project" "$(inbox_types triage-bot)" "question_asked"
"$F" --as triage-bot read $(unread_ids triage-bot) >/dev/null
"$F" principal scope project-wide --withdraw --for triage-bot 2>"$T/withdrawn.err" >/dev/null
check "withdrawing the endorsement says no agent serves the whole project now" "$(grep -c '^fwd: no agent serves the whole project now' "$T/withdrawn.err")" "1"
SD=$("$F" --as impl-alice scope create "Docs" --branch feat/docs --format json | j "d['record']['id']")
"$F" --as impl-alice ask --scope "$SD" "who writes the primer?" --format json 2>"$T/docs-asked.err" >/dev/null
check "a question asked of a scope no triage agent answers for waits, which the asker is told" "$(grep -c "this question waits, asked of the scope \"Docs\" (${SD:0:8}), until an agent serves it or the whole project" "$T/docs-asked.err")" "1"
check "a lead working in that scope is told how to make its triage agent, naming the scope" "$("$F" --as claude --scope "$SD" inbox --format json 2>&1 >/dev/null | grep -c "The scope \"Docs\" (${SD:0:8}) has no triage agent, and no agent serves the whole project.*--role triage --scope ${SD:0:8}")" "1"
"$F" principal scope add "$SD" --for triage-store >/dev/null   # triage-store takes Docs too
for _ in $(seq 1 50); do [ "$(inbox_types triage-store)" = "question_asked,question_asked" ] && break; sleep 0.1; done
check "the agent that takes the scope hears of its question that waited, once" "$("$F" --as triage-store inbox --unread --format json | j "','.join(sorted(n['message']['content'] for n in d))")" "is SQLite enough?,who writes the primer?"
"$F" principal scope project-wide --for triage-bot >/dev/null   # this small project endorses it again
check "succession moves the scopes: a relaunched agent serves its predecessor's, and its unread mail moves" "$("$F" --as triage-repl-2 principal add triage-repl-2 --succeeds triage-repl --format text)" "triage-repl-2 (triage, serving Replication) succeeds @triage-repl; 1 unread notification(s) moved"
"$F" --as impl-alice ask --scope "$SR" "and the notifications?" >/dev/null
for _ in $(seq 1 50); do [ "$(inbox_types triage-repl-2)" = "question_asked,question_asked" ] && break; sleep 0.1; done
check "what is asked of the scope reaches the successor" "$("$F" --as triage-repl-2 inbox --unread --format json | j "','.join(sorted(n['message']['content'] for n in d))")" "and the notifications?,does the relay replicate?"
"$F" principal scope remove "$SS" --for triage-store 2>"$T/freed.err" >/dev/null
check "freeing a scope says who answers for it now" "$(grep -c "^fwd: @triage-bot, which serves the whole project, answers first for the scope \"Storage\" (${SS:0:8}) now$" "$T/freed.err")" "1"
"$F" principal scope remove "$SD" --for triage-store 2>"$T/unbound.err" >/dev/null
check "left serving nothing, the agent is named with the two commands its lead runs" "$(grep -c '^fwd: @triage-store holds triage and serves no scope, so nothing reaches it for its role' "$T/unbound.err")" "1"
check "and the agent is told on each of its commands, on standard error" "$("$F" --as triage-store task list --format json 2>&1 >/dev/null | grep -c '^fwd: @triage-store holds triage and serves no scope, so nothing reaches it for its role: bind it to the scope it triages, `fwd principal scope add <scope id> --for triage-store`, or, in a small project with small scopes only, endorse it for the whole project, `fwd principal scope project-wide --for triage-store`.$')" "1"
check "and in the hook's context at its session's start" "$("$F" --as triage-store prime --format hook | grep -c '^@triage-store holds triage and serves no scope')" "1"
check "principal list says what each triage agent serves" "$("$F" principal list --format text | grep '(triage' | sort | tr '\n' ',')" "triage-bot (triage, serving the whole project),triage-repl-2 (triage, serving Replication),triage-store (triage, serving nothing),"
for who in triage-repl-2 triage-store; do "$F" principal retire "$who" >/dev/null 2>&1; done   # their work here is done

echo "== a graph from before the fixed roles, carried over =="
# The build before the fixed roles writes the graph, as it wrote the graphs people hold: main at
# 4347c8f, where --as made the principal it named, a supervisor when the handle started with
# supervisor and an implementer otherwise. The section runs only where such a build is at hand,
# FWD_SMOKE_BEFORE naming one or target/fwd-before holding it, since the native test
# crates/fwd-core/tests/principals.rs covers the carry-over and building that commit on every run
# costs more than a path taken once per graph is worth; CI runs without it. To make
# target/fwd-before, from this checkout's root (on macOS, `cp -Rc target/debug
# target/fwd-before/debug` first reuses its compiled dependencies, about a minute in all):
#   mkdir -p target/fwd-before/src && git archive 4347c8f Cargo.toml Cargo.lock rust-toolchain.toml .cargo crates/fwd crates/fwd-core | tar -x -C target/fwd-before/src
#   (cd target/fwd-before/src && cargo build -p fwd --target-dir ..)
OLDF=${FWD_SMOKE_BEFORE:-$REPO/target/fwd-before/debug/fwd}
if [ -z "${FWD_SMOKE_BEFORE:-}" ] && [ ! -x "$OLDF" ]; then
    echo "   skipped: no fwd from before the fixed roles at hand (FWD_SMOKE_BEFORE unset, no target/fwd-before/debug/fwd); crates/fwd-core/tests/principals.rs covers the carry-over"
else
    before() { FWD_DATA_DIR=$T/before FWD_RUNTIME_DIR=$T/run-before FWD_DEV_RUNNER_DIR=$T/dev-runner-before "$@"; }   # a fwd with a daemon of its own on the graph from before
    mkdir -p "$T/before-repo" && cd "$T/before-repo"
    git init -q -b main && git config user.email t@e.com && git config user.name t && git remote add origin git@example.com:t/before.git && git commit -q --allow-empty -m init
    before "$OLDF" init --name before --handle dnorman --display-name Daniel >/dev/null
    TB=$(before "$OLDF" --as supervisor task create "Groom the backlog" --propose --format json | j "d['id']")   # the first supervisor did work here
    QB=$(before "$OLDF" ask --to supervisor "who cuts the release?" --format json | j "d['id']")   # a question to the role
    TC=$(before "$OLDF" --as claude task create "Cut the release" --format json | j "d['id']")   # an implementer, as every other handle was
    before "$OLDF" --as supervisor-2 comment "$TC" "looked at it" >/dev/null   # a second supervisor, the last to write
    before "$OLDF" --as chatty comment "$TC" "hello" >/dev/null   # an agent that only wrote a message
    SB=$(before "$OLDF" --as claude scope create "Release work" --format json | j "d['id']")   # a scope with an owner and a heartbeat
    before "$OLDF" daemon stop >/dev/null
    check "before the migration, an agent whose old role this build does not read is refused a write, with the command that carries the graph over" "$(before "$F" --as supervisor comment "$TC" "still here" 2>&1 | grep -c "^Error: \`@supervisor\` holds no role, and an agent selects one before it writes (this project's graph is from before the fixed roles of 2026-09-29: \`fwd admin migrate\`, after \`fwd admin backup\`, carries it over): \`fwd principal role supervisor lead|triage|implementer|reviewer\`$")" "1"
    MIGRATED=$(before "$F" admin migrate --format text)
    check "the migration gives triage to the supervisor that wrote most recently, serving no scope until its lead binds or endorses it" "$(echo "$MIGRATED" | grep -c '^triage: @supervisor-2, of the principals that held the supervisor role the one that wrote most recently, serving no scope until its lead binds it (fwd principal scope add <scope id> --for supervisor-2) or endorses it for the whole project (fwd principal scope project-wide --for supervisor-2)$')" "1"
    check "and names the other for a person to retire" "$(echo "$MIGRATED" | grep -c '^supervisor role cleared, holding no role now, for a person to retire (fwd principal retire <handle>): @supervisor$')" "1"
    check "and the agent that only wrote a message, for a person to retire or add" "$(echo "$MIGRATED" | grep -c ': @chatty (1 message)$')" "1"
    check "the question to the role is asked of the project now" "$(echo "$MIGRATED" | grep -c '^questions to the supervisor role, asked of the project now: 1$') $(before "$F" message show "$QB" --format json | j "d['context']")" "1 {'asks': 'project'}"
    # The agents of a graph from before 2026-09-30 have recorded no write, so they read as dormant
    # until their next (docs/DECISIONS.md, D123).
    check "it translates the words only: the implementer stays one, the other supervisor holds no role" "$(before "$F" principal list --all --format text | tr '\n' ',')" "dnorman (person),supervisor-2 (triage, serving nothing, dormant),chatty (implementer, dormant, does not belong here),claude (implementer, dormant),supervisor (agent, dormant),"
    check "an agent with no role is refused a write, with the command that selects one" "$(before "$F" --as supervisor comment "$TC" "still here" 2>&1 | grep -c '^Error: `@supervisor` holds no role, and an agent selects one before it writes: `fwd principal role supervisor lead|triage|implementer|reviewer`$')" "1"
    check "and its reads still work" "$(before "$F" --as supervisor task show "$TB" --format json | j "d['title']") $(before "$F" --as supervisor inbox --format json >/dev/null && echo read)" "Groom the backlog read"
    before "$F" --as claude ask "who cuts the release now?" >/dev/null
    check "a dormant agent that writes is active again, listed above the dormant ones" "$(before "$F" principal list --all --format text | tr '\n' ',')" "dnorman (person),claude (implementer),supervisor-2 (triage, serving nothing, dormant),chatty (implementer, dormant, does not belong here),supervisor (agent, dormant),"
    release() { before "$F" --as supervisor-2 inbox --unread --format json 2>/dev/null | j "','.join(n['type'] for n in d if (n.get('message') or {}).get('content') == 'who cuts the release now?')"; }
    sleep 1
    check "the triage agent the migration made serves nothing, so the next question asked of the project waits" "$(release)" ""
    check "and each of its commands says why, naming the two commands its lead runs" "$(before "$F" --as supervisor-2 inbox --format json 2>&1 >/dev/null | grep -c '^fwd: @supervisor-2 holds triage and serves no scope, so nothing reaches it for its role: bind it to the scope it triages, `fwd principal scope add <scope id> --for supervisor-2`, or, in a small project with small scopes only, endorse it for the whole project, `fwd principal scope project-wide --for supervisor-2`.$')" "1"
    before "$F" principal scope project-wide --for supervisor-2 >/dev/null   # its lead endorses it, the project being small
    for _ in $(seq 1 50); do [ "$(release)" = "question_asked" ] && break; sleep 0.1; done
    check "endorsed for the whole project, it hears the question that waited" "$(release)" "question_asked"
    check "and its commands say nothing more of it" "$(before "$F" --as supervisor-2 inbox --format json 2>&1 >/dev/null | grep -c 'serves no scope')" "0"
    check "the migration clears the scope's owner and heartbeat" "$(echo "$MIGRATED" | grep -c "^scopes whose owner and heartbeat were cleared, a scope having neither since 2026-09-29: Release work (${SB:0:8})$")" "1"
    CLAUDE_B=$(before "$F" principal list --all --format json | j "[p['id'] for p in d if p['handle'] == 'claude'][0]")
    check "and nothing else about it: its name, its status and who made it" "$(before "$F" scope show "$SB" --format json | j "d['title'] + ' ' + d['status'] + ' ' + str(d['created_by'] == '$CLAUDE_B')")" "Release work active True"
    check "a second run changes nothing" "$(before "$F" admin migrate --format text | grep -c -E ': (none|0)$|^triage: given to no one ')" "8"
    before "$F" daemon stop >/dev/null
    cd "$T/repo"
fi

echo "== names =="
"$F" --as impl-alice scope rename "$S1" "Reconnects that survive NAT drops" >/dev/null
check "scope rename changes the name scope show prints" "$("$F" scope show "$S1" --format json | j "d['title']")" "Reconnects that survive NAT drops"
INSTANCE=$(curl -s "$U/health" | j "d['instance_id']")
"$F" project rename "roles renamed" >/dev/null
check "project rename changes the name the running daemon lists (same instance)" "$(curl -s "$U/projects" | j "','.join(p['name'] for p in d)") $(curl -s "$U/health" | j "d['instance_id']")" "roles renamed $INSTANCE"
DAVE=$("$F" principal list --format json | j "[p['id'] for p in d if p['handle'] == 'impl-dave'][0]")
check "an agent renames itself, keeping its id and its role, and is told to give --as and FWD_AS the new name" "$("$F" --as impl-dave principal rename implementer-dave --format json 2>"$T/renamed.err" | j "d['handle'] + ' ' + d['agent_role'] + ' ' + str(d['id'] == '$DAVE')") $(grep -c 'fwd: @impl-dave is @implementer-dave now, with the same id' "$T/renamed.err")" "implementer-dave implementer True 1"
check "the name it left names no one, and the new one names it" "$("$F" --as impl-dave whoami 2>&1 | grep -c 'no principal of this project is named `impl-dave`') $("$F" --as implementer-dave whoami --format json | j "d['principal']['id'] == '$DAVE'")" "1 True"
check "a name another principal has is refused, saying whose it is" "$("$F" --as implementer-dave principal rename impl-alice 2>&1 | grep -c 'the name `impl-alice` is taken: .* (implementer, in .*) has it in this project; choose a name no other principal of this project has')" "1"
check "a person is not renamed" "$("$F" principal rename daniel 2>&1 | grep -c '`@dnorman` is a person, whose name is the handle `fwd init` recorded on this machine')" "1"

echo "== the VS Code webview's origin =="
# The webview runs the page from the extension's own files, so it reaches the daemon from its own
# origin, and its browser shows it an answer only when the answer names that origin. Any
# extension's webview can run script, so the daemon admits only the origins the extension
# registers (POST /webview-origins, from this machine and with no Origin, like the CLI's calls),
# and no page reaches the control routes, whatever its origin.
from_page() { curl -s -o /dev/null -w '%{http_code}' -H "Origin: $1" "${@:2}"; }   # the status a request from a page of origin $1 gets
named() { curl -s -o /dev/null -w '%{http_code} %header{access-control-allow-origin}' -H "Origin: $1" "$2"; }   # its status and the origin its answer names
register() { curl -s -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/json' -d "{\"origin\":\"$1\"}" "${@:2}" "$U/webview-origins"; }
OURS=vscode-webview://fwd-smoke-panel
THEIRS=vscode-webview://another-extension
check "an unregistered webview is refused health, projects, the checkouts and the socket" "$(from_page $THEIRS "$U/health") $(from_page $THEIRS "$U/projects") $(from_page $THEIRS "$U/p/$PROJECT/checkouts") $(upgrade "/p/$PROJECT/ws" 127.0.0.1 $THEIRS)" "403 403 403 403"
check "its page's stop, a plain-text POST that needs no preflight, is refused" "$(from_page $THEIRS -X POST -H 'Content-Type: text/plain' "$U/stop")" "403"
check "the extension registers its panel's origin from this machine" "$(register $OURS)" "200"
check "the registered webview reads health and projects, each answer naming it" "$(named $OURS "$U/health") $(named $OURS "$U/projects")" "200 $OURS 200 $OURS"
check "the registered webview's socket upgrades" "$(upgrade "/p/$PROJECT/ws" 127.0.0.1 $OURS)" "101"
check "another webview is still refused" "$(from_page $THEIRS "$U/health")" "403"
check "the registered webview's page may not stop the daemon or open a project" "$(from_page $OURS -X POST -H 'Content-Type: text/plain' "$U/stop") $(from_page $OURS -X POST -H 'Content-Type: application/json' -d "{\"id\":\"$PROJECT\"}" "$U/projects")" "403 403"
check "nor may the daemon's own page" "$(from_page "$U" -X POST -H 'Content-Type: text/plain' "$U/stop")" "403"
check "a page may not register an origin" "$(register $THEIRS -H "Origin: $OURS")" "403"
check "only a webview's origin is registered" "$(register https://evil.example)" "400"
check "the daemon still runs, the same instance" "$(curl -s "$U/health" | j "d['instance_id']")" "$INSTANCE"

echo "== names that repeat =="
# A name is a label two principals may share: here a person joins under the lead agent's name, as
# nothing forbids. A typed name is read within the project, and one two principals share is
# refused, listing each by its id, never guessed (docs/design/agents-and-inbox.md, "A principal's name").
LEAD=$("$F" principal list --format json | j "[p['id'] for p in d if p['handle'] == 'claude'][0]")
"$F" init --handle claude --display-name "Claude the person" >/dev/null
PERSON=$("$F" whoami --format json | j "d['principal']['id']")
check "a person may take a name an agent has, and the CLI acts as the person with its handle" "$("$F" whoami --format json | j "d['principal']['kind'] + ' ' + d['principal']['handle'] + ' ' + str(d['principal']['id'] != '$LEAD')")" "human claude True"
"$F" --as claude whoami >/dev/null 2>"$T/shared.err" || true
check "--as refuses a name two principals of the project share" "$(grep -c '2 principals of this project are named `claude`' "$T/shared.err")" "1"
check "listing the agent by its id and role, and the person with the scopes its work is in" "$(grep -c -e "${LEAD:0:8}: lead, in .*" "$T/shared.err") $(grep -c -e "${PERSON:0:8}: person, in no scope" "$T/shared.err")" "1 1"
check "the first eight characters of the id name one of them, in --as" "$("$F" --as "${LEAD:0:8}" whoami --format json | j "d['principal']['agent_role']")" "lead"
check "and in FWD_AS" "$(FWD_AS="${LEAD:0:8}" "$F" whoami --format json | j "d['principal']['id']")" "$LEAD"
check "principal list gives the id and the scopes beside the shared name" "$("$F" principal list --format text | grep -cx "claude (${PERSON:0:8}: person, in no scope)")" "1"
check "a mention of the shared name is refused, listing them" "$("$F" comment "$T80" "over to @claude" 2>&1 | grep -c 'refused: 2 principals of this project are named `claude`')" "1"
check "and @ with the id's start names one, stored by its id" "$("$F" comment "$T80" "over to @${LEAD:0:8}" --format json | j "d['record']['content'] + ' ' + d['record']['references'][0]['id']")" "over to [[$LEAD]] $LEAD"
check "a second agent naming itself with a taken name is refused, told whose it is and to choose another" "$("$F" --as impl-alice-2 principal add impl-alice --role implementer 2>&1 | grep -c 'the name `impl-alice` is taken: .* (implementer, in .*) has it in this project.*choose a name no other principal of this project has')" "1"
check "and no second impl-alice is made" "$("$F" principal list --all --format json | j "len([p for p in d if p['handle'] == 'impl-alice'])")" "1"
NP=$("$F" --as impl-alice decision create "Name agents by role and scope" --area naming --propose --format json | j "d['record']['number']")
check "a ruling is a person's, so --decided-by passes over the agent that shares the person's name" "$("$F" --as triage-bot decision accept "D$NP" --decided-by claude --note "fine" --format json | j "d['decided_by']")" "$PERSON"
"$F" init --handle dnorman --display-name Daniel >/dev/null   # the machine's person again
check "a principal command takes the id: the person retires" "$("$F" principal retire "${PERSON:0:8}" --format text)" "claude (person, retired)"
check "and still shares its name, which stays refused" "$("$F" --as claude whoami 2>&1 | grep -c '2 principals of this project are named `claude`')" "1"

echo "== an agent of its own: named, acting, succeeded =="
# Each agent a user of its own (docs/design/agents-and-inbox.md, "An agent's identity"; D112, D114,
# D118): the fwd plugin's hook, run here as Claude Code runs it, stamps each command made only of fwd
# with the ids of the session and of the agent that run it, and the command, run in a shell that
# has the session's id as Claude Code gives it, acts as the principal bound to that agent.
FWDDIR=$(dirname "$F")
# The command the hook gives Claude Code in place of $1, for the subagent $2 of the session smoke-s1
# (the session's own thread without one); nothing when the hook leaves the command alone.
stamp() {
    python3 -c 'import json, sys
v = {"session_id": "smoke-s1", "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": sys.argv[1]}}
if sys.argv[2]: v["agent_id"] = sys.argv[2]
print(json.dumps(v))' "$1" "${2:-}" | PATH="$FWDDIR:$PATH" "$REPO/integrations/claude-code/hooks/stamp"
}
# Runs $1 as the subagent $2 of the session smoke-s1 does (the session's own thread without one):
# stamped by the hook, then in a shell with the session's id.
as_agent() {
    local line
    line=$(stamp "$1" "${2:-}" | j "d['hookSpecificOutput']['updatedInput']['command']")
    CLAUDE_CODE_SESSION_ID=smoke-s1 PATH="$FWDDIR:$PATH" sh -c "$line"
}
check "the hook leaves a command with another program in it alone" "$(stamp 'cargo test && fwd task list' a1)" ""
check "and stamps one made only of fwd and a filter of its output, with the session and the agent" "$(stamp 'fwd task list | head -3' a1 | j "d['hookSpecificOutput']['updatedInput']['command']")" "export FWD_CLAUDE_SESSION='smoke-s1' FWD_CLAUDE_AGENT='a1'; fwd task list | head -3"
check "a subagent that has not named itself acts as no principal, and says which agent it is" "$(as_agent 'fwd whoami --format json' a1 | j "str(d['principal']) + ' ' + d['subject']")" "None claude-code:smoke-s1/a1"
check "its first write is refused, with how to name itself" "$(as_agent "fwd task create 'Stamped work'" a1 2>&1 | grep -c 'which has not named itself in this project.*fwd principal add <name> --role lead|triage|implementer|reviewer')" "1"
STAMPED=$(as_agent 'fwd principal add impl-stamped --role implementer --format json' a1 | j "d['record']['id']")
check "naming itself binds it to the agent" "$("$F" principal list --format json | j "[p['subject'] for p in d if p['id'] == '$STAMPED'][0]")" "claude-code:smoke-s1/a1"
check "from then on its commands act as it, with no --as" "$(as_agent 'fwd whoami --format json' a1 | j "d['principal']['handle']")" "impl-stamped"
check "FWD_AS, which a settings file gives every agent of the session, is not read" "$(FWD_AS=dnorman as_agent "fwd task create 'Stamped work' --format json" a1 | j "d['record']['created_by']")" "$STAMPED"
check "a typed --as naming another principal is refused" "$(as_agent "fwd --as dnorman task create 'Not mine'" a1 2>&1 | grep -c '`--as dnorman` asks to act as another principal.*an agent acts only as itself')" "1"
check "the session's own thread takes over the lead from before the plugin, by saying it twice" "$(as_agent "fwd --as ${LEAD:0:8} principal bind ${LEAD:0:8} --format json" | j "d['subject']")" "claude-code:smoke-s1/main"
check "and acts as it from then on" "$(as_agent 'fwd whoami --format json' | j "d['principal']['id']")" "$LEAD"
check "a session has one lead, its own thread: a subagent of it that selects lead is refused, naming the session's lead" "$(as_agent 'fwd principal add lead-sub --role lead' a5 2>&1 | grep -c 'the Claude Code session smoke-s1 has a lead already, @claude, and a session has one lead, its own thread: an agent a lead dispatches selects implementer, reviewer or triage instead')" "1"
check "while the own thread of another session selects lead beside it, as the stamp names that session" "$(FWD_CLAUDE_SESSION=smoke-s3 FWD_CLAUDE_AGENT=main CLAUDE_CODE_SESSION_ID=smoke-s3 "$F" principal add lead-s3 --role lead --format text)" "lead-s3 (lead)"
check "a write the hook did not stamp, in the session, is refused with FWD_AS alone, which every agent of the session inherits, and the refusal names both ways out" "$(CLAUDE_CODE_SESSION_ID=smoke-s1 FWD_AS=impl-alice "$F" task create 'Unstamped' 2>&1 | grep -c "but it carries no agent's stamp.*run fwd on its own, which the fwd plugin stamps.*or type \`--as <name>\` on the command")" "1"
check "so is one given --format hook without a hook's JSON, which names the agent a hook runs for" "$(CLAUDE_CODE_SESSION_ID=smoke-s1 FWD_AS=impl-alice "$F" inbox pop --format hook </dev/null 2>&1 | grep -c "but it carries no agent's stamp")" "1"
check "but a --as typed on the command line, which no agent inherits, writes there as the principal it names" "$(CLAUDE_CODE_SESSION_ID=smoke-s1 "$F" --as impl-alice task create 'Typed in the session' --format json | j "d['record']['created_by']")" "$("$F" --as impl-alice whoami --format json | j "d['principal']['id']")"
check "with neither --as nor FWD_AS such a command acts as no one, not as the person: its read of the caller's own inbox is refused the same way" "$(CLAUDE_CODE_SESSION_ID=smoke-s1 "$F" inbox --unread 2>&1 | grep -c "but it carries no agent's stamp.*run fwd on its own, which the fwd plugin stamps.*or type \`--as <name>\` on the command")" "1"
check "while a read about the graph alone runs, and whoami names no principal, saying why on standard error" "$(CLAUDE_CODE_SESSION_ID=smoke-s1 "$F" whoami --format json 2>"$T/no-one.err" | j "d['principal']") $(grep -c '^fwd: this command runs in the Claude Code session smoke-s1, whose agents act as themselves' "$T/no-one.err") $(CLAUDE_CODE_SESSION_ID=smoke-s1 "$F" task list --format json | j "len(d) > 0")" "None 1 True"
"$F" ask --to impl-stamped "is the stamp on?" >/dev/null
for _ in $(seq 1 50); do [ "$(as_agent 'fwd inbox --unread --format json' a1 | j "len(d)")" = "1" ] && break; sleep 0.1; done
check "a question asked of the agent reaches its inbox" "$(as_agent 'fwd inbox --unread --format json' a1 | j "' '.join(n['type'] for n in d)")" "question_asked"
# The agent is relaunched: a new subagent names itself and succeeds it.
SUCCESSOR=$(as_agent 'fwd principal add impl-stamped-2 --role reviewer --format json' a2 | j "d['record']['id']")
SUCCEEDED=$(as_agent 'fwd principal succeed impl-stamped --format json' a2)
check "the successor takes the old one's role and its unread notification" "$(echo "$SUCCEEDED" | j "d['agent_role'] + ' ' + d['succeeded']['handle'] + ' ' + str(d['moved'])")" "implementer impl-stamped 1"
check "the old one is retired, with its successor recorded" "$("$F" principal list --all --format text | grep -cx 'impl-stamped (agent, retired, succeeded by @impl-stamped-2)')" "1"
"$F" ask --to impl-stamped "still there?" >/dev/null
for _ in $(seq 1 50); do [ "$(as_agent 'fwd inbox --unread --format json' a2 | j "len(d)")" = "2" ] && break; sleep 0.1; done
check "what is asked of the old one reaches the successor, saying whom it was for" "$(as_agent 'fwd inbox --unread --format json' a2 | j "' | '.join(sorted(n['reason'].replace(n['id'][:8], '<its id>') for n in d))")" "dnorman asked you “is the stamp on?” (for @impl-stamped, whom you succeeded); reply with fwd reply <its id> | dnorman asked you “still there?” (for @impl-stamped, whom you succeeded); reply with fwd reply <its id>"
check "and nothing more waits in the old one's inbox" "$(as_agent 'fwd inbox --unread --format json' a1 2>&1 | j "len(d)")" "0"
check "a principal a successor took over is not taken over by binding it" "$(as_agent "fwd --as impl-stamped principal bind impl-stamped" a3 2>&1 | grep -c 'was succeeded by @impl-stamped-2')" "1"
check "a relaunched triage agent names itself as its predecessor's successor in one step, holding triage and serving what it served, which the one-per-project rule counts on it" "$(as_agent 'fwd principal add triage-2 --succeeds triage-bot --format json' a4 | j "d['record']['handle'] + ' ' + d['record']['agent_role'] + ' ' + str(d['record']['serves']['project_wide']) + ' ' + d['succeeded']['handle']")" "triage-2 triage True triage-bot"
# The relaunched triage agent corrects the question its predecessor asked where it stands (task
# sVCUXcEg): an agent that succeeded a message's author edits the message, which stays the
# author's and says who edited it; another agent is refused, and told to ask the successor.
check "an agent that did not succeed a message's author is refused its edit, and told to ask the agent that did" "$(as_agent "fwd message edit $QE 'mine now'" a2 2>&1 | grep -c "and this one is @triage-bot's, whom @triage-2 succeeded; reply to it instead, or ask @triage-2 to edit it")" "1"
TRIAGE2=$(as_agent 'fwd whoami --format json' a4 | j "d['principal']['id']")
CORRECTED=$(as_agent "fwd message edit $QE 'does @impl-bob take the TLS task, as [[D$NA]] asked, and by when?' --format json" a4)
check "the agent that succeeded its author edits the message, which stays the author's and records who edited it" "$(echo "$CORRECTED" | j "d['id'] == '$QE' and d['author'] != d['edited_by'] and d['edited_by'] == '$TRIAGE2' and d['content'].endswith('asked, and by when?')")" "True"
check "and message show says who edited it" "$("$F" message show "$QE" --format text | head -1 | grep -c "@triage-bot \[active, edited .* by @triage-2\]$")" "1"
# The inbox hooks, which Claude Code runs itself, carry no stamp: each is handed its event's JSON on
# standard input, naming the session and, inside a subagent, the agent whose hook fired, and acts for
# the principal bound to that agent, not for FWD_AS, which a settings file's env block gives every
# agent of the session; only a session's own thread with no principal bound falls back to FWD_AS.
# Runs fwd with the arguments after the first two as the hook of the event $1 fired inside the
# subagent $2 of the session $HOOK_SESSION, smoke-s1 unless set (the session's own thread when $2 is
# empty), FWD_AS naming the person.
hook() {
    local event=$1 agent=$2 session=${HOOK_SESSION:-smoke-s1}
    shift 2
    python3 -c 'import json, sys
v = {"session_id": sys.argv[3], "hook_event_name": sys.argv[1]}
if sys.argv[2]: v.update(agent_id=sys.argv[2], agent_type="general-purpose")
print(json.dumps(v))' "$event" "$agent" "$session" | CLAUDE_CODE_SESSION_ID=$session FWD_AS=dnorman "$F" "$@"
}
as_agent "fwd ask --to dnorman 'whose inbox does a hook read?'" a2 >/dev/null
for _ in $(seq 1 50); do [ "$("$F" inbox --unread --format json | j "len(d) > 0")" = "True" ] && break; sleep 0.1; done
check "a hook inside a subagent that has not named itself acts for no one, not for FWD_AS: it prints nothing and marks nothing read" "$(hook PostToolBatch a9 inbox pop --format hook --hook-event PostToolBatch && echo ran) $("$F" inbox --unread --format json | j "len(d) > 0")" "ran True"
check "a hook inside a subagent that named itself pops that agent's notifications" "$(hook PostToolBatch a2 inbox pop --format hook --hook-event PostToolBatch | j "d['hookSpecificOutput']['additionalContext']" | grep -c 'as impl-stamped-2:')" "1"
check "and marks them read" "$(as_agent 'fwd inbox --unread --format json' a2 | j "len(d)")" "0"
check "the session's own thread's hook acts for the lead bound to it" "$(hook SessionStart '' prime --format hook | head -1 | grep -c '^fwdcentaur context for claude:')" "1"
check "the own thread of a session no principal is bound to falls back to FWD_AS, as the hooks did before, so an unbound lead keeps its notifications" "$(HOOK_SESSION=smoke-s2 hook SessionStart '' prime --format hook | head -1 | grep -c '^fwdcentaur context for dnorman:')" "1"
check "while its subagent's hook acts for no one" "$(HOOK_SESSION=smoke-s2 hook PostToolBatch b1 inbox pop --format hook --hook-event PostToolBatch && echo ran)" "ran"

echo "== a command that makes something answers with it under record =="
# cli.md, "Global conventions": each prints the record it made under `record`, as `show` or `list`
# prints it, and for this build the record's fields beside it too, as it printed them before.
made() { j "str(d['record']['$1']) + ' ' + str(all(d[k] == v for k, v in d['record'].items()))"; }
MT=$("$F" task create "Made alike" --format json)
check "task create answers with the task, its fields beside it for this build" "$(echo "$MT" | made title)" "Made alike True"
check "and record.created is the time it was made" "$(echo "$MT" | j "type(d['record']['created']).__name__")" "int"
MTID=$(echo "$MT" | j "d['record']['id']")
MC=$("$F" comment "$MTID" "made alike" --format json)
check "comment answers with the message" "$(echo "$MC" | made content)" "made alike True"
MCID=$(echo "$MC" | j "d['record']['id']")
check "reply answers with the message" "$("$F" reply "$MCID" "and a reply" --format json | made reply_to)" "$MCID True"
check "ask answers with the message" "$("$F" ask --about "$MTID" "made alike?" --format json 2>/dev/null | made content)" "made alike? True"
check "forward answers with the message" "$("$F" forward "$MCID" --to @dnorman --note "look" --format json | made reply_to)" "$MCID True"
check "anchor create answers with the anchor" "$("$F" anchor create --path ws.rs --lines 9-9 --format json | made path)" "ws.rs True"
check "step add answers with the step" "$("$F" step add "$MTID" "$T60" --format json | made kind)" "task True"
MD=$("$F" decision create "Records are read alike" --area process --decision "one field" --format json)
check "decision create answers with the decision" "$(echo "$MD" | made title)" "Records are read alike True"
check "link answers with the relation" "$("$F" link "$MTID" relates_to "$T60" --format json | made kind)" "relates_to True"
check "cite answers with the relation" "$("$F" cite "$MTID" "D$(echo "$MD" | j "d['record']['number']")" --format json | made kind)" "cites True"
check "principal add answers with the principal" "$("$F" --as impl-made principal add impl-made --role implementer --format json | made handle)" "impl-made True"
check "scope create answers with the scope" "$("$F" scope create "Made work" --format json | made title)" "Made work True"

echo "== a file named in a message =="
# A whole file at a version, named by its path in double brackets (task nVE7KTeSqiX0): read in this
# checkout, stored as its whole-file highlight's id and the version named, and named by both.
printf 'one\ntwo\nthree\nfour\n' > src/net.rs   # a change not staged
HEADSHA=$(git rev-parse HEAD)
FM=$("$F" comment "$MTID" "the file [[src/net.rs]], as committed [[src/net.rs@HEAD]]" --format json)
check "a file named on disk and at a commit is stored as its highlight's id and the version named" "$(echo "$FM" | j "' '.join(r['kind'] + ':' + r['version'] for r in d['record']['references'])")" "anchor:worktree anchor:$HEADSHA"
check "and named by its path and that version" "$(echo "$FM" | j "' | '.join(r['name'] for r in d['record']['references'])")" "src/net.rs on disk | src/net.rs ${HEADSHA:0:7}"
check "which the message's text shows in words" "$("$F" message show "$(echo "$FM" | j "d['record']['id']")" --format text | tail -n 1)" "the file [src/net.rs on disk], as committed [src/net.rs ${HEADSHA:0:7}]"
check "a file not there is refused, naming it" "$("$F" comment "$MTID" "see [[src/nope.rs]]" 2>&1 | grep -c 'src/nope.rs is not on disk in this checkout')" "1"
git checkout -q -- src/net.rs

echo "== backup and restore =="
BACKUP=$("$F" admin backup --to "$T/backups" --format json)   # while the daemon runs
BK=$(echo "$BACKUP" | j "d['backup']")
check "the backup lists the project" "$(echo "$BACKUP" | j "','.join(p['name'] for p in d['projects'])")" "roles renamed"
check "the project's database is in it, copied through SQLite" "$(echo "$BACKUP" | j "[f['database'] for f in d['files'] if f['path'].endswith('/project.sqlite')]")" "[True]"
check "no log, lock file or write-ahead log is" "$(echo "$BACKUP" | j "[f['path'] for f in d['files'] if f['path'].endswith(('.log', '.lock', '-wal', '-shm'))]")" "[]"
check "the backup restores into a new folder" "$("$F" admin restore "$BK" --into "$T/restored" --format json | j "d['into'].endswith('/restored')")" "True"
restored() { FWD_DATA_DIR=$T/restored FWD_RUNTIME_DIR=$T/run-restored FWD_DEV_RUNNER_DIR=$T/dev-runner-restored "$F" "$@"; }   # fwd with a daemon of its own on the restored folder
check "a daemon of its own opens it with the tasks the live graph has" "$(restored task list --format json | j "len(d)")" "$("$F" task list --format json | j "len(d)")"
restored daemon stop >/dev/null
check "a restore into a folder that is not empty is refused" "$("$F" admin restore "$BK" --into "$T/restored" 2>&1 | grep -c 'is not empty')" "1"
check "a restore into the data folder the daemon serves is refused, saying how to stop it" "$("$F" admin restore "$BK" 2>&1 | grep -c 'is serving the data folder .*fwd daemon stop')" "1"

if [ "${FWD_SMOKE_LAN:-}" = "1" ]; then
    echo "== a phone on the LAN =="
    START=$("$F" daemon start --lan 2>"$T/start.err" || true)
    check "daemon start --lan restarts the loopback daemon and says so on stderr" "$(grep -c '^Restarted the daemon' "$T/start.err")" "1"
    check "its stdout is the port file alone" "$(echo "$START" | j "d['bind']")" "lan"
    LANURL=$("$F" ui --lan --no-open 2>/dev/null)
    LANHOST=$(echo "$LANURL" | sed -E 's#^http://([^/]+)/.*#\1#')
    LANIP=${LANHOST%:*}
    check "fwd ui --lan names a LAN address and the project" "$(echo "$LANURL" | sed -E 's#^http://[0-9.]+:[0-9]+/#http://LAN/#')" "http://LAN/?p=$PROJECT"
    check "health answers on the LAN address" "$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "http://$LANHOST/health")" "200"
    check "the project list answers on the LAN address" "$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "http://$LANHOST/projects")" "200"
    check "the page is served on the LAN address" "$(curl -s --max-time 5 "http://$LANHOST/" | grep -q 'fwd-web_bg.wasm' && echo served)" "served"
    check "the project's socket upgrades on the LAN address" "$(upgrade "/p/$PROJECT/ws" "$LANIP")" "101"
    check "stop from another address is refused" "$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 -X POST "http://$LANHOST/stop")" "403"
    check "opening a project from another address is refused" "$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 -X POST -H 'Content-Type: application/json' -d "{\"id\":\"$PROJECT\"}" "http://$LANHOST/projects")" "403"
    check "the checkout registry is refused to another address" "$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "http://$LANHOST/p/$PROJECT/checkouts")" "403"
    check "the daemon still runs, on the LAN" "$("$F" daemon status | j "d['bind']")" "lan"
    check "daemon start --loopback restarts it and says so" "$("$F" daemon start --loopback 2>&1 | grep -c '^Restarted the daemon')" "1"
    check "the daemon is back on loopback" "$("$F" daemon status | j "d['bind']")" "loopback"
    check "the LAN address no longer answers" "$(curl -s -o /dev/null -w '%{http_code}' --max-time 3 "http://$LANHOST/health" || true)" "000"
fi
"$F" daemon stop >/dev/null
check "sutra's files go when the daemon stops" "$(ls -A "$FWD_DEV_RUNNER_DIR" | wc -l | tr -d ' ')" "0"
echo "== $pass passed, $fail failed =="
[ "$fail" -eq 0 ]
