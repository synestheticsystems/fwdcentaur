#!/usr/bin/env python3
"""Import the hand-written decision log (docs/DECISIONS.md) into a project's decision records
through the `fwd` CLI, once, so the rulings can be cited by number and the file can become what
`fwd decision export` renders.

Each entry becomes a decision, numbered in file order:
- under "Established (Daniel)", `- **Title** (date, qualifier): text` is an accepted ruling decided by
  the person the project knows as its person (whoever ran `fwd init`), on that date, with the text
  (his quoted words included) as the ruling and the qualifier as its context;
- under "Proposed", each entry is a proposal filed by `claude`: `- **Title** (…): text`, or
  `- Label: text`, or a bare `- text`, whose first clause becomes the title. An entry whose text
  begins with "superseded" is no longer open: it is filed and then rejected by `claude` in the
  same run, with the file's words as the reason, and it cites the ruling it names in quotes when
  exactly one established entry's title is, or begins with, those words;
- under "Open", each paragraph (`**Title.** question… Lean: …`) is a proposal filed by `claude`
  whose context is the question and whose ruling is the lean, when it states one.
The area is guessed from a small keyword table only when exactly one area's words appear in the
title, and left empty otherwise. Entries whose title a decision already has are skipped, so a
second run creates nothing. The proposals' `decision_proposed` notifications are marked read for
the supervisor (the triager knows them from the file) and, for the entries rejected here, for the
person too, since those ask nothing of anyone; the person's others stay unread, since the open
entries are the person's to rule on and their inbox is where everything open for them waits
(Daniel, 2026-09-26). Nothing is deleted and the file is not changed.

Run it in a checkout of the target project, after `fwd init` there:

    FWD=./target/debug/fwd python3 scripts/import-decisions.py [docs/DECISIONS.md] [--dry-run]

`--dry-run` prints what would be created, and creates nothing.
"""
import json, os, re, subprocess, sys, time, pathlib

FWD = os.environ.get("FWD", "fwd")
WORK = pathlib.Path.cwd()
FILER = "claude"

# An area only when exactly one area's words appear in the title, as whole words.
AREAS = {
    "architecture": ["architecture", "daemon", "node", "extension", "model", "models", "multi-model", "transport",
                     "identifiers", "identified", "token", "wasm", "sqlite", "leptos"],
    "product": ["vision", "github issues", "single-user", "business", "license"],
    "process": ["supervisor", "dogfooding", "rights", "creates tasks"],
    "ux": ["page", "button", "buttons", "dropdown", "outline", "selector", "keyboard"],
    "naming": ["name", "names", "naming", "workspace"],
    "dependency": ["dependency", "pinned by hash", "tracks the", "crates.io"],
}


def area_of(title):
    found = [area for area, words in AREAS.items()
             if any(re.search(rf"(?<![\w-]){re.escape(w)}(?![\w-])", title, re.I) for w in words)]
    return found[0] if len(found) == 1 else None


def fwd(*args, as_=None):
    cmd = [FWD, "--format", "json", "--project", str(WORK)] + (["--as", as_] if as_ else []) + list(args)
    out = subprocess.run(cmd, cwd=WORK, capture_output=True, text=True)
    if out.returncode != 0:
        raise SystemExit(f"{' '.join(cmd[:8])} ...\n{out.stderr}")
    return json.loads(out.stdout) if out.stdout.strip() else None


def sections(text):
    """The file's `## ` sections, by heading, each as its lines."""
    found, heading = {}, None
    for line in text.splitlines():
        if line.startswith("## "):
            heading = line[3:].strip()
            found[heading] = []
        elif heading is not None:
            found[heading].append(line)
    return found


def items(lines):
    """The section's entries: bullets (`- `) and paragraphs, each with its continuation lines."""
    out, current = [], None
    for line in lines:
        if not line.strip():
            current = None
            continue
        if line.startswith("- ") or current is None:
            current = [line]
            out.append(current)
        else:
            current.append(line)
    return [" ".join(part.strip() for part in item) for item in out]


BOLD = re.compile(r"^(?:- )?\*\*(?P<title>.+?)\*\*\s*(?:\((?P<paren>[^)]*)\))?\s*:?\s*(?P<text>.*)$")
DAY = re.compile(r"\d{4}-\d{2}-\d{2}")


def established(line):
    m = BOLD.match(line)
    if not m or not line.startswith("- "):
        return None
    paren = (m.group("paren") or "").strip()
    day = DAY.search(paren)
    qualifier = paren[day.end():].lstrip(" ,;").strip() if day else paren
    title, text = m.group("title").strip(), m.group("text").strip()
    # An entry written as one sentence running on past its date ("**Title** (date), not …") keeps
    # its title at the front of the ruling, so the ruling reads as written.
    if text[:1] in ",.;":
        text = title + text
    return {"title": title, "decided": day.group(0) if day else None,
            "context": qualifier, "decision": text, "propose": False}


def first_clause(text):
    """A bare proposal's title: its text up to the first parenthesis, comma or semicolon."""
    cut = min([i for i in (text.find(" ("), text.find(", "), text.find("; ")) if i > 0] or [len(text)])
    return text[:cut].strip().rstrip(".")


def proposed(line):
    if not line.startswith("- "):
        return None
    m = BOLD.match(line)
    if m:
        entry = {"title": m.group("title").strip(), "context": (m.group("paren") or "").strip(),
                 "decision": m.group("text").strip(), "propose": True}
    else:
        body = line[2:].strip()
        label, sep, rest = body.partition(": ")
        if sep and len(label) <= 60 and "(" not in label and "`" not in label:
            entry = {"title": label.strip(), "context": "", "decision": rest.strip(), "propose": True}
        else:
            entry = {"title": first_clause(body), "context": "", "decision": body, "propose": True}
    # Superseded already in the file: filed, then rejected with the file's words as the reason.
    entry["superseded"] = entry["decision"].startswith(("Superseded", "superseded"))
    return entry


def named_ruling(text, titles):
    """The established entry a superseded entry names in quotes (`see Established, "…"`): the one
    title among `titles` that is, or begins with, the quoted words; None when none or several do."""
    for quoted in re.findall(r'"([^"]+)"|“([^”]+)”', text):
        name = (quoted[0] or quoted[1]).strip().rstrip(".").lower()
        found = [t for t in titles
                 if t.lower() == name or (t.lower().startswith(name) and not t[len(name)].isalnum())]
        if len(found) == 1:
            return found[0]
    return None


def open_question(line):
    m = BOLD.match(line)
    if not m:
        return None
    title = m.group("title").strip().rstrip(".")
    paren, rest = (m.group("paren") or "").strip(), m.group("text").strip()
    if line.startswith("- "):
        # A bullet among the paragraphs: its parenthesis says where it came from.
        rest = f"{paren}: {rest}" if paren else rest
    elif paren:
        rest = f"({paren}) {rest}".strip()
    lean = re.search(r"\bLean\b", rest)
    question, answer = (rest[:lean.start()].strip(), rest[lean.start():].strip()) if lean else (rest, "")
    return {"title": title, "context": question or f"Open question: {title}.", "decision": answer, "propose": True}


def entries(text):
    by_heading = sections(text)
    out = []
    for heading, parse in (("Established (Daniel)", established), ("Proposed", proposed), ("Open", open_question)):
        lines = next((v for k, v in by_heading.items() if k.startswith(heading)), None)
        if lines is None:
            raise SystemExit(f"no section starting `## {heading}` in the file")
        for item in items(lines):
            entry = parse(item)
            if entry and entry["title"]:
                entry["area"] = area_of(entry["title"])
                out.append(entry)
    rulings = [e["title"] for e in out if not e["propose"]]
    for e in out:
        if e.get("superseded"):
            e["cites"] = named_ruling(e["decision"], rulings)
    return out


def mark_read(who, targets):
    """Marks read `who`'s unread `decision_proposed` notifications about the decisions `targets`,
    once as many as there are targets have arrived (or after 6 s, those that have); answers how
    many it marked."""
    ours = []
    for _ in range(30):
        try:
            unread = fwd("inbox", "--unread", "--for", who) or []
        except SystemExit:
            return 0  # no principal with that handle in this project
        ours = [n["id"] for n in unread if n["type"] == "decision_proposed" and n["target"]["id"] in targets]
        if len(ours) >= len(targets):
            break
        time.sleep(0.2)
    if ours:
        fwd("read", *ours)
    return len(ours)


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    dry = "--dry-run" in sys.argv[1:]
    source = pathlib.Path(args[0]) if args else pathlib.Path(__file__).resolve().parents[1] / "docs/DECISIONS.md"
    wanted = entries(source.read_text())
    if dry:
        number = {e["title"]: n for n, e in enumerate(wanted, 1)}
        for n, e in enumerate(wanted, 1):
            status = "proposed" if e["propose"] else f"accepted {e.get('decided') or ''}".strip()
            note = ""
            if e.get("superseded"):
                status = "rejected"
                cites = f"; cites {number[e['cites']]} {e['cites']}" if e.get("cites") else "; names no ruling to cite"
                note = f"  (superseded in the file: filed, then rejected with its words{cites})"
            print(f"{n:>3} {status:<19} {e['area'] or '-':<12} {e['title']}{note}")
        print(f"{len(wanted)} entries; nothing created (--dry-run)")
        return
    me = fwd("whoami")["principal"]["handle"]
    have = {d["title"].strip(): d["id"] for d in fwd("decision", "list") or []}
    created, skipped, proposals, closed = [], 0, [], []
    for e in wanted:
        if e["title"] in have:
            skipped += 1
            continue
        cmd = ["decision", "create", e["title"], "--context", e["context"], "--decision", e["decision"]]
        if e["area"]:
            cmd += ["--area", e["area"]]
        if e["propose"]:
            cmd.append("--propose")
        else:
            cmd += ["--decided-by", f"@{me}"]
            if e.get("decided"):
                cmd += ["--decided", e["decided"]]
        d = fwd(*cmd, as_=FILER)
        have[e["title"]] = d["id"]
        created.append(d)
        if e.get("superseded"):
            # No longer open: rejected at once, the file's words kept as the reason, and citing
            # the ruling that superseded it when the file names one this project has.
            d = fwd("decision", "reject", d["id"], "--note", e["decision"], as_=FILER)
            if e.get("cites") in have:
                fwd("cite", d["id"], have[e["cites"]], as_=FILER)
            closed.append(d["id"])
        elif e["propose"]:
            proposals.append(d["id"])
        print(f"D{d['number']:<4} {d['status']:<9} {d['area'] or '-':<12} {d['title']}")
    # The routing service notifies the person and the supervisors of each proposal. The supervisor's
    # are marked read once they have arrived (the triager knows the file). The person's stay unread,
    # since the open entries are theirs to rule on, except those about the entries rejected here,
    # which ask nothing of anyone.
    marked = 0
    if proposals or closed:
        marked += mark_read("supervisor", set(proposals) | set(closed))
    if closed:
        marked += mark_read(me, set(closed))
    print(f"created {len(created)} ({len(closed)} rejected as superseded), skipped {skipped} already recorded, "
          f"marked {marked} proposal notification(s) read: the supervisor's, and the person's about the rejected ones; "
          f"the person's others stay unread")


if __name__ == "__main__":
    main()
