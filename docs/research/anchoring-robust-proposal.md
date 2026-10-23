# Robust anchoring: deterministic identity, index-aware bases, edit following, honest states

Proposal for T20, 2026-09-24, replacing the first cut in `../design/anchoring.md` ("Implementation notes (T8)") and `crates/fwd-core/src/anchoring.rs`. Prior art already in `code-annotations.md` is not repeated; new sources are cited inline with fetch dates, and every number attributed to "today" comes from scripts run this session against the `ankurah` and `simplikeys` histories on Daniel's machine (section 6).

The one-paragraph answer. An anchor's identity is a hash of what was anchored: the repository, the path, the git object id of the exact file content the person was looking at, and the line (later column) range. Git assigns that object id to the content whether or not it is ever committed, so a dirty, partly staged or untracked file has a basis exactly as stable as a committed one, and two people pointing at the same span of the same content get the same anchor. The daemon writes that content into the repository's object database and pins it, records how it relates to the HEAD and index blobs at the time (so a span crossing staged and unstaged hunks is decomposed into pieces, each with its own basis), and carries a small patch on the anchor so any other machine can rebuild the basis from a blob it already has. Following edits uses two mechanisms with one interface: when a version's derivation is known exactly (editor deltas streamed from VS Code, or a diff between two observed states), the daemon records it in a per-file Automerge document and the anchor is a pair of Automerge cursors that resolve at any recorded version; when it is not known (an arbitrary commit, another machine's checkout), the daemon maps lines by diff between the basis blob and the target blob and falls back to content fingerprints for moves. Every answer carries three separate facts (where, whether the text changed, whether it is there at all) and never rebinds silently.

## 1. Requirements as testable statements

Each requirement names the fixture that proves it. Line ranges are 0-based, end-exclusive as in the current code.

- **R1 Determinism.** Forming an anchor for the same lines of byte-identical file content at the same path in the same repository yields the same `anchor_key` on any machine, from the CLI or the editor, and `fwd anchor create` a second time returns the existing anchor. Fixture: two clones at one commit, both run `fwd anchor create --path src/a.rs --lines 10-12`; one entity results after sync.
- **R2 Basis without a commit.** An anchor on an untracked, modified, staged or mixed file has a basis blob id and resolves against later versions by diff, not only by text search. Fixture: modify a file in two hunks, stage one, anchor a span covering both and the unchanged lines between; `fwd anchor show` reports the basis, HEAD and index blobs and three pieces.
- **R3 Index decomposition.** After `git commit` of that index, resolution against HEAD reports `partial`: the staged piece and the unchanged lines are present at their new lines, the unstaged piece is absent; after committing the rest, `exact`. A `git stash` and `pop` in between change nothing.
- **R4 Editor edits.** With the file open in VS Code, typing above, inside and below the span gives the same live range from the daemon as from the extension's own delta walk; cutting the span and pasting it elsewhere is `moved` from the daemon where the delta walk collapses.
- **R5 Outside edits.** `sed -i`, a formatter pass, and a `git merge` that inserts then resolves conflict markers end in `shifted`, `recovered` and `shifted`; `git checkout other` reports `absent in this checkout` and `git checkout -` reports `exact` from cache.
- **R6 History rewriting.** Commit, `git commit --amend`, an interactive rebase that squashes the commit carrying the span, and a cherry-pick to another branch all leave the anchor `exact` against the new HEAD when the file content is unchanged, because blob ids survive rewriting; when the rewrite changes the content, resolution is `modified` by diff from the basis blob.
- **R7 Cross-file move.** Moving the span's function to another file in one commit resolves to `moved` with the new path when that file is renamed, copied or added in the commit range; a whole-tree search is available on demand and reported as such.
- **R8 Honesty.** Every resolution states location certainty, content change and availability separately; a text mismatch after any strategy downgrades the location claim; an unfound anchor keeps its identity, snapshot and messages and shows where it was last seen.
- **R9 Coincidence.** Two anchors that resolve to overlapping ranges with equal content are reported as coincident and can be merged by a person without deleting either.
- **R10 Cost.** Forming costs one `git hash-object` and two small diffs; resolving against a known version is a cursor lookup in milliseconds; a file's sidecar after a hundred observed versions is a few times its size; nothing in the sidecar is needed for correctness elsewhere.

## 2. The design space

Approaches are scored against the requirements above; "cost" covers storage, CPU, code complexity and what must sync between machines. Evidence: this session's experiments (section 6), LHDiff (ICSM 2013) and CodeMapper (arXiv 2511.05205, 2025) as cited in 3.6, and the library documentation cited in 3.4.

| Approach | R1 identity | R2/R3 dirty and index bases | R4/R5 edits (editor, outside, rewrites) | R6/R7 moves and honesty | Cost |
|---|---|---|---|---|---|
| **A. Current (T8):** HEAD blob or `worktree`, random ULID, Myers cascade | No: random id | No: dirty spans have no diff basis; the index is invisible | Saves only; no delta feed; worktree anchors never gain a basis | In-file moves by O(n·m) scan; no cross-file; states exist | Low; already built |
| **B. Content-addressed basis:** `git hash-object -w --path` of what was seen, pinned ref, HEAD/index/worktree triple, pieces, patch to the nearest blob | Yes: key = f(repo, path, blob, range) | Yes: every state is a blob; pieces carry the index split; blob ids survive rewrites (verified) | Outside edits and rewrites by blob-to-blob diff; editor edits only at save | Needs C for moves; three-fact record | One `hash-object` and two diffs per anchor; a patch on the entity; pins are refs |
| **C. Content fingerprints only:** normalized line and context hashes (LHDiff: simhash content 0.6 + context 0.4, top-15 candidates, Levenshtein refinement; CodeMapper: diff candidates + movement + exact text search, ranked by Levenshtein with 15 context lines) | Partial: collides on duplicated code (CodeMapper's second-largest failure class, 75 cases) | Basis-free, but cannot say which lines were staged | Any version, no history needed | Best evidence for moves: LHDiff 82.8/85.5% vs `git blame -M` 45.3/53.7% on heavy-edit benchmarks; needs a threshold to stay honest | O(lines) CPU per resolution; nothing to store or sync |
| **D. AST/symbol paths:** tree-sitter node paths, CodeTracker-style matching | As a secondary key | Same as C | Reparse per version (tree-sitter `Tree::edit`) | Strong for method moves (CodeTracker: 99.97% precision on Java methods) but language-specific; Reiss (ICSE 2008, as LHDiff reports it) found textual content+context beat AST methods for lines | A grammar per language; a later addition |
| **E. Per-file Automerge DAG of observed states** with two cursors per anchor (this proposal's tracker) | Not by itself (op ids are not content); paired with B | Every observed blob is a version; cursors resolve at any recorded heads | Exact for editor deltas, diff-ingested otherwise; time travel with an honest error before creation (verified) | Deletion shows as crossed cursors; moves need C | 2–5× the latest file size per ~100 versions (measured); local; rebuildable |
| **F. Full git-history mirror in a CRDT** (Eg-walker's A1/A2 traces were built the same way) | Same as E | Same as E | Same as E for commits | No accuracy gain: one location disagreement vs direct diff over 3,494 samples (measured) | ~76 ms per version with `update_text`; grows with history; pointless to sync |
| **G. Yrs text property in Ankurah** holding each annotated file | No | In principle | `StickyIndex` resolves only against the live doc; the `yrs::Doc` is `pub(crate)` in Ankurah's backend; every keystroke syncs to every peer | Same limits as E | Highest sync cost; needs Ankurah API changes |
| **H. Editor-only tracking** (extension delta walk; VS Code decorations; Zed-style anchors, which resolve only forward) | No | No | Exact while open, lost on close; VS Code never reports shifted ranges back | Cut and paste collapses | Lowest, not persistent |
| **I. Patch-theory line identity** (Pijul: a vertex is identified by its introducing change; deletion relabels an edge) | From the change, not the content: two machines reaching one text by different changes disagree | Not available on a git host | n/a | Moves are delete plus insert (inferred) | Not adoptable |

Verdict: B supplies identity and bases, E exact tracking wherever a derivation is known, C moves, reformatting and the last resort, D an optional secondary key later; F is not default because the measurements show no accuracy gain over B's direct diff, G fails on API and sync grounds, H stays the extension's visit-local convenience, I does not apply.

## 3. The proposal

### 3.1 Identity

```
anchor_key = "fwd-anchor:1:" + hex(sha256(
    repo_entity_id ‖ 0x00 ‖ path ‖ 0x00 ‖ basis_blob ‖ 0x00 ‖
    start_line ‖ "-" ‖ end_line ‖ 0x00 ‖ cols_or_empty))
span_key   = "fwd-span:1:" + hex(sha256(normalize(anchored_text)))
```

`basis_blob` is git's object id of the anchored content, computed as `git hash-object -w --stdin --path=<path>` fed with the text the person saw (the editor buffer, the worktree file, or a blob). `--path` applies the repository's clean filters, which is what makes the id equal to the one `git add` will record later; this session's experiment shows the same CRLF file hashing to `0aa0f34a` without filters and `b77b4eb1` with `core.autocrlf=true` (git-hash-object manual, https://git-scm.com/docs/git-hash-object, fetched 2026-09-24: "the path is used to determine which Git filters should be applied"). `-w` writes the object; the daemon then pins it with `git update-ref refs/fwd/basis/<blob> <blob>` (a ref may point at a blob; verified today, `git fsck --unreachable` reports nothing). The version prefix lets the key format change later; SHA-256 repositories (`extensions.objectFormat=sha256`) yield 64-hex ids and the same scheme.

`span_key` is the content-only fingerprint used to recognise coincident anchors (R9): `normalize` strips trailing whitespace on each line and collapses runs of whitespace to one space; it does not touch identifiers or literals, because a "same span" claim should survive reformatting but not a rename.

The Ankurah entity keeps its ULID `EntityId`; `anchor_key` is an LWW string property and `create` becomes get-or-create (fetch `anchor_key = '<key>'`, create on miss). `EntityId::from_bytes([u8; 16])` exists in ankurah-proto 0.9.3 (`src/id.rs`), but `Transaction::create` calls `EntityId::new()` itself (ankurah-core 0.9.3 `src/transaction.rs` and `node.rs`), so a content-derived entity id needs an Ankurah change (open question 1); until then a race can create two entities with one key, which section 3.7 makes harmless.

### 3.2 The basis and its pieces

At form time the daemon records three object ids for the path: `head_blob` (`git rev-parse HEAD:<path>`, absent when untracked or unborn), `index_blob` (`git rev-parse :<path>`; `git ls-files -s` prints the same id at stage 0; an intent-to-add entry shows the empty blob `e69de29b`), and `basis_blob`. Today's experiment (a 24-line file, two hunks 18 lines apart, `git apply --cached` of the first) produced three distinct ids, with `git diff --cached` showing only the staged hunk and `git diff` only the unstaged one: the state the current code cannot see.

The span is then decomposed into **pieces**: a Myers line map from the basis text to the HEAD text and another to the index text (both via `similar`, the crate already in the workspace) classify each span line as present-in-HEAD, present-in-index, or worktree-only; maximal runs with the same classification become pieces `{basis_lines, head_lines: Option<range>, index_lines: Option<range>}`. Pieces are stored on the anchor as JSON and allow `partial` answers (R3) without guessing: a piece with `index_lines` has a basis in the index blob, and the commit made from that index carries the same blob id (verified today: `git log --find-object=<index_blob>` lists the commit).

The anchor also carries `nearest_blob` (whichever of `head_blob`/`index_blob` has the smaller diff to the basis, or none) and `basis_patch`, a unified diff from `nearest_blob` to the basis, empty when the basis is already that blob, the whole file (capped, section 5) when the file is untracked. Any machine with the repository can rebuild the basis text (`git cat-file -p <nearest_blob>`, apply the patch, `git hash-object --stdin --path`) and verify it against `basis_blob` before trusting it. This replaces "rebase worktree anchors onto commits": identity is already content-addressed, so when a later commit contains the blob the daemon only records `known_in += commit` (found with `git log --find-object=<basis_blob>` on HEAD changes; today's run shows the record surviving `git commit --amend` because the blob id does not change).

`git gc` prunes unreachable loose objects after `gc.pruneExpire` (two weeks by default) but keeps "anything else in the refs/* namespace" (https://git-scm.com/docs/git-gc, fetched 2026-09-24), so the pin suffices locally; a fresh clone lacks it, hence `basis_patch` on the entity. Pins can later be pushed the way git-notes are (open question 6).

### 3.3 Where each piece lives

| Piece | Lives in | Why |
|---|---|---|
| `CodeAnchor` entity: `anchor_key`, `span_key`, `repo`, `path`, `basis_blob`, `head_blob`, `index_blob`, `nearest_blob`, `basis_patch`, `start_line`, `end_line`, `cols`, `side`, `anchored_text`, `context_before/after`, `pieces` (Json), `known_in` (Json, advisory), `created_by`, `created` | Ankurah, synced | Everything another machine needs to resolve by diff or fingerprint. Immutable except `known_in`. The `basis: Blob|Worktree` enum and the `resolution` cache leave the entity. |
| Basis and observed content | The repository's object database, pinned under `refs/fwd/basis/` | Content addressing for free; `git diff <blob> <blob>` and `--find-object` work directly; shareable through git. |
| `observations(repo, path, blob, kind: head|index|worktree|buffer|stash, checkout, at)` | Sidecar SQLite in the daemon's workspace data directory (next to the existing local-only table) | Local, high-churn, rebuildable. |
| `file_docs(repo, path, doc BLOB, updated)` and `version_heads(repo, path, blob, heads)` and `anchor_cursors(anchor_key, path, start BLOB, end BLOB)` | Sidecar SQLite; Automerge document bytes | The per-file version DAG (3.4). Rebuildable from git plus pins except for editor deltas since the last save, which only affect live positions. |
| `resolutions(anchor_key, target_blob, record, at)` | Sidecar SQLite | Advisory cache keyed by target blob, so a branch switch back hits the same key; out of the entity so a resolution is no longer an Ankurah write synced to peers (open question 2). |
| Live ranges in open editors | Extension memory plus the daemon's cursors | Visit-local, per the extension design. |

### 3.4 The per-file version DAG (Automerge)

Automerge 0.11 (Rust; docs.rs shows 0.12.0 on 2026-09-16) is chosen for three verified properties. First, cursors resolve at any recorded version: `ReadDoc::get_cursor_moving(obj, position, at: Option<&[ChangeHash]>, MoveCursor)` and `get_cursor_position(obj, &cursor, at: Option<&[ChangeHash]>)` (https://docs.rs/automerge/latest/automerge/trait.ReadDoc.html, fetched 2026-09-24), with `text_at(obj, heads)` to read the text there. Yrs 0.24's `StickyIndex::get_offset(txn)` resolves only against the live document (https://docs.rs/yrs/latest/yrs/struct.StickyIndex.html, fetched 2026-09-24; `Snapshot` and `skip_gc` exist, sticky-index-at-snapshot does not), and Ankurah's `YrsBackend` keeps its `yrs::Doc` `pub(crate)` behind a `YrsString` API of `insert`/`delete`/`value` (ankurah-core 0.9.3 `property/backend/yrs.rs`), so a Yrs text property would hide the CRDT and sync every keystroke of every annotated file to every peer. Second, deletion is signalled rather than papered over: `MoveCursor::After`/`Before` move a cursor whose character was deleted to the next or previous character that was visible when the cursor was made and still is (`src/cursor.rs`), so a span's two cursors cross when its text is deleted (today's runs: `1422..1259`), and resolving at heads that predate the text returns `Err(cursor ... is invalid)` rather than a position. Third, changes are content-addressed: a change hash is the SHA-256 of the change chunk (https://automerge.org/automerge-binary-format-spec/, fetched 2026-09-24), and `AutoCommit::set_actor(ActorId)` plus `commit_with(CommitOptions::default().with_time(0))` remove both sources of nondeterminism, so two machines ingesting the same versions the same way produce byte-identical documents (today: seven files, two builds each, identical `save()` bytes and heads).

**Document shape.** One `AutoCommit` per (repo, path lineage) with a single text object `t`. Every observed version of the file is a change whose parents are the heads of the version it was derived from; `version_heads` maps a blob id to the change hash that produced that content. Reads never use the merged "current" state; every read and every cursor lookup passes `at: Some(&heads)`.

**Ingesting a version.** A new blob is ingested as a sibling of its parent version, not on top of whatever was ingested last:

```
ingest(path, blob, text, kind, parent: Option<blob>):
    if version_heads[blob] exists: return version_heads[blob]
    doc = file_docs.load_or_create(path)               # AutoCommit::load / new + put_object(ROOT, "t", ObjType::Text)
    parent ??= argmin_{p in {last_ingested, head_blob, index_blob} ∩ known} line_diff_size(text_of(p), text)
    parent_heads = version_heads[parent] or [root_change]
    doc.isolate(&parent_heads)                          # subsequent changes depend on parent_heads (autocommit.rs 401-411; undocumented, test in M4; fallback: fork_at + merge)
    doc.set_actor(if kind in {head, index, stash}: ActorId::from(sha256("fwd-edge:1" ‖ parent_heads ‖ blob)[..16]) else ActorId::random())
    for hunk in similar::TextDiff::from_lines(text_of(parent), text).ops():   # line-level hunks, then char offsets
        doc.splice_text(t, char_offset(hunk.old_start), hunk.old_len_chars, &new_text_of(hunk))
    hash = doc.commit_with(CommitOptions::default().with_time(0).with_message("blob:" + blob))
    doc.integrate()
    version_heads[blob] = [hash]; observations.insert(path, blob, kind, now)
    file_docs.append(path, doc.save_incremental())
```

Applying line hunks as `splice_text` is deliberate: `update_text` runs a character-level Myers diff internally (`src/text_diff.rs`), which cost about 76 ms per version on a 45 KB file today; a line diff with per-hunk splices is far cheaper and keeps the ops aligned with how git and the resolver think.

**Editor sessions.** When the extension opens a document for an anchored path it sends the buffer's text hash; the daemon ingests the buffer as a `buffer` version if unknown, then applies each `onDidChangeTextDocument` batch as splices on that session's head chain (a random actor, wall-clock time allowed), one Automerge change per batch; the session chain is an isolation at the session's heads, and an interleaved ingest of another version isolates, commits, integrates and re-isolates there. The extension periodically (every N batches and on save) sends the SHA-256 of its full text; on mismatch the daemon asks for the full text and re-syncs by diff. On save, the saved content's blob id is computed and recorded as `version_heads[blob] = session heads`, which is what lets a later `git add`/`commit` of that content be recognised without re-resolution. Cost is negligible: 10,000 single-character inserts, one change each, saved to 2,249 bytes today (columnar run-length encoding; sequential typing, the best case).

**Git-derived versions.** Only versions the daemon actually observes (HEAD blob at form time and on HEAD changes, index blobs on index changes, saved files, stash trees) are ingested, lazily and only for paths that have anchors. Mirroring a file's whole commit history is not required: today's comparison of a direct Myers map (basis to latest) against the stepwise cursor walk over 3,494 sampled lines from five real histories found one disagreement about where a surviving line is (duplicated text), and a few percent where one method considered a line deleted and the other still found its text. The mirror is kept as an optional `fwd anchor history --build` for time-travel queries, not as a dependency.

### 3.5 Resolution

The result is a record with three independent facts and a derived label:

```
Resolution {
  target: { blob, kind: commit(sha)|index|worktree|buffer(version)|stash, path },
  location: Exact | Shifted | Moved | Recovered{score} | Unknown,
  content:  Same | Changed{similarity, changed_lines} | Gone,
  availability: Present | Partial{pieces: [{basis_lines, target_lines?}]} | Absent{BeforeCreation|Deleted|FileMissing|NotInCheckout},
  range: Option<{start_line, end_line, cols}>,
  method: SameBlob | Cursor | Diff | Fingerprint | CrossFile,
  hint: Option<{last_seen_line, ambiguity: u32}>, at }
label = exact | shifted | modified | moved | recovered | partial | orphaned   # for UIs and hooks; orphaned = Unknown + Absent
```

```
resolve(anchor A, target T, checkout C):
    if T.blob == A.basis_blob: return {Exact, Same, Present, A.range, SameBlob}
    if cached(A.key, T.blob) and not stale: return it
    r = None
    if version_heads has both A.basis_blob and T.blob in file_docs[A.path]:      # exact derivation known
        s = get_cursor_position(t, cursors[A].start, Some(heads(T)))            # MoveCursor::After on the first char
        e = get_cursor_position(t, cursors[A].end,   Some(heads(T)))            # MoveCursor::Before on the last char (incl. newline)
        if both Ok and s < e:
            got = text_at(t, heads(T))[s..e]
            r = classify(got vs A.anchored_text, lines(s..e) vs A.range)        # Same+Exact, Same+Shifted, or Changed+Shifted
        elif Err(invalid): r = {Unknown, Gone, Absent(BeforeCreation)}
        else: hint = s; r = None                                                # crossed: deleted here, search below
    if r is None:
        basis_text = cat_file(A.basis_blob) or rebuild(A.nearest_blob, A.basis_patch)   # verify hash
        if basis_text:
            m = line_map(basis_text, T.text, algorithm = Patience if len > 4000 lines else Myers)
            kept = [m[i] for i in A.range if m[i] is Some]
            if kept covers A.range contiguously: r = {Shifted, Same, Present}
            elif kept nonempty: r = {Shifted, Changed{|kept|/len}, Partial{pieces by run}}   # candidate; a search may beat it
    if r is None or r.availability is Partial with coverage < 0.5:
        f = fingerprint_search(A, T.text, hint)                                   # 3.6
        if f.whole:  r = {Moved, Same, Present, f.range, Fingerprint}
        elif f.score >= 0.6: r = {Recovered{f.score}, Changed{f.score}, Present, f.range, Fingerprint}
        elif T.kind is commit and A.known_in nonempty:
            for p in git diff --name-only -M -C <A.known_in[0]>..<T.commit>:     # renamed, copied, added files
                f = fingerprint_search(A, text_at(T.commit, p), None); if f.whole or f.score >= 0.7: r = {Moved{path p}, ..., CrossFile}; break
    if r is None: r = {Unknown, Gone, Absent(Deleted), None, hint}
    if r.range and text(r.range) != A.anchored_text and r.content is Same: r.content = Changed    # the Hypothesis rule
    cache(A.key, T.blob, r); return r
```

`classify`: equal text at the same lines is `Exact`, elsewhere `Shifted`; different text is `Changed` with `similar`'s `ratio()` as similarity and the per-line diff as `changed_lines`. Cursor resolution and diff mapping are two routes to the same facts; the cursor route is preferred when both versions are in the document because its derivation is exact (editor deltas) or already paid for (ingest), at 1 to 30 ms on a 45 KB file with a hundred versions today.

### 3.6 Fingerprint search (moves and reformatting)

The current window scan (`TextDiff::from_chars` over every window) is replaced by the pipeline LHDiff and CodeMapper validated, adapted from single lines to spans:

1. **Whole-block match.** Every target line is hashed after `normalize`; lines with fewer than four non-space characters are wildcards (git's move detection ignores blocks under 20 alphanumeric characters for the same reason: `--color-moved=blocks`, https://git-scm.com/docs/git-diff, fetched 2026-09-24). A hash-to-positions map yields every candidate start for the span's first non-wildcard line; each candidate extends while hashes match, wildcards skipping. A candidate matching every non-wildcard span line in order is a whole match: `Moved`, or `Shifted` when the diff route reached the same lines. This is CodeMapper's movement detection plus its exact text search, at line granularity.
2. **Per-line content and context scoring.** For lines not covered, each span line gets two 64-bit simhashes, of its normalized content and of its context (four lines above and below, blanks skipped); target lines are indexed the same way once. The 15 target lines with the highest `0.6·content + 0.4·context` similarity (normalized Hamming distance) are refined with normalized Levenshtein on content and cosine on context at the same weights and accepted above 0.6 (LHDiff's tool default; its main paper says 0.45; E4 settles the value). Reiss's ICSE 2008 experiment found this content-plus-context form better than AST-based tracking (as LHDiff reports it), and LHDiff scores 82.8% and 85.5% correct on the Eclipse and NetBeans benchmarks where `git blame -M` scores 45.3% and 53.7% (ICSM 2013, https://www.cs.usask.ca/faculty/croy/papers/2013/AsaduzzamanICSM2013LineDraft.pdf, fetched 2026-09-24).
3. **Span assembly.** Accepted lines are grouped into order-preserving runs within a window of twice the span's length; the run covering the most span lines is scored by the token-level `similar::TextDiff::from_words(...).ratio()` over normalized tokens (whitespace and comments removed, the representation Roy, Cordy and Koschke's clone taxonomy identifies as robust to formatting changes, https://research.cs.queensu.ca/home/cordy/Papers/RCK_SCP_Clones.pdf, fetched 2026-09-24), plus a bonus when `context_before`/`context_after` match and a penalty by distance from `hint`. A score at or above 0.6 is `Recovered{score}`; ties within 0.02 set `ambiguity: n` (coincidental identical text is CodeMapper's second-largest failure class, 75 of its cases) and take the candidate nearest the hint, never labelled above `recovered`.

Cross-file search runs only for commit targets, over the files `git diff --name-only -M -C <known_in>..<target>` reports as renamed, copied or added, each searched with the three steps; CodeMapper attributes 46 failures to renames its `git log` did not detect, which is why copies and added files are candidates, not only `--follow` lineage.

### 3.7 Coincident anchors

Coincidence is detected, recorded and left to people: after any resolution of A in target T, an anchor B on the same path whose cached resolution in T overlaps A's range by at least 80% of the shorter range and whose `span_key` (or resolved text) equals A's gets a `Relation{kind: Coincides, from: A, to: B, status: Accepted, via: {"resolver": T.blob}}`; the panel and `fwd anchor show` group coincident anchors and their messages. `fwd anchor merge B --into A` (a person, or the supervisor by its rights) creates `Relation{kind: Supersedes, from: A, to: B}` and re-points nothing; readers follow `Supersedes` to display messages together. Two entities created for one `anchor_key` by a race are the exact-coincidence case and merge the same way, so the missing uniqueness constraint costs a relation, never data.

## 4. One machinery for CLI, extension and daemon

The daemon owns the sidecar, the resolver and the git observer; the CLI and the extension are its clients. `fwd anchor create --path P --lines a-b [--cols x:y] [--text-from-stdin]` forms an anchor from the worktree file or from stdin (the extension pipes the buffer, so unsaved text gets the buffer's blob as basis and the child-process shortcut keeps working). `fwd anchor show ID --against HEAD|index|worktree|<rev>|<blob>` and `fwd anchor find ID [--all-files]` return the record of 3.5; `fwd task next` embeds it per anchor against the caller's checkout.

The extension streams `workspace.onDidChangeTextDocument` events to the daemon's loopback listener (`POST /session/<id>/changes` with `{path, version, changes: [{rangeOffset, rangeLength, text}], textHash?}`). VS Code's contract, read today in `src/vscode-dts/vscode.d.ts` and `pieceTreeTextBuffer.ts` (`_doApplyEdits` sorts operations bottom to top and reports each with its pre-edit `range`, `rangeOffset`, `rangeLength`), is that applying `contentChanges` in array order to a mirror of the previous text reproduces the document; `MirrorTextModel.onEvents` does exactly that, the array has been frozen since microsoft/vscode#88310, and `document.version` strictly increases including undo and redo (`reason` is `Undo` or `Redo`). The event also fires with empty `contentChanges` on dirty-state changes, which the extension ignores. Offsets are UTF-16 code units (`offsetAt`, `positionAt`), so the extension converts them on its own mirror to Unicode scalar counts; the daemon verifies with the text hash and re-syncs by diff on mismatch. The extension keeps its delta-walked ranges for immediate glyph updates (VS Code tracks decoration ranges internally but exposes no getter: issues #48364 and #54147, the latter open since 2018) and replaces them with the daemon's cursor positions after each batch and on `onDidSaveTextDocument`. Any LSP-capable editor can feed the same endpoint from `textDocument/didChange` (`TextDocumentSyncKind.Incremental`), whose events are likewise sequential in array order. On diff-editor sides, the built-in git extension's `git:` URIs carry `{path, ref}` in their query (`extensions/git/src/uri.ts`: `''` is the index, `'HEAD'`, `'~'` the previous version, `'~1'..'~3'` merge stages, or a commit hash) and for commit refs `Repository.getObjectDetails(treeish, path)` returns the blob id, so an anchor made on a `git:` document gets its basis without hashing (the daemon uses `git rev-parse :<path>` for the index side).

The git observer watches `.git/HEAD`, `.git/index`, `.git/refs/`, `.git/packed-refs`, `.git/logs/HEAD` and each linked worktree's `HEAD` and `index` (`git worktree list --porcelain`), debounced at 200 ms, and does nothing while `.git/rebase-merge`, `.git/rebase-apply`, `MERGE_HEAD` or `CHERRY_PICK_HEAD` exist except mark resolutions against HEAD as `in-progress`. On an index change it runs `git ls-files -s -- <anchored paths>`, records `index` observations, and ingests new blobs only for paths with open sessions or pending resolutions. On a HEAD change (commit, checkout, rebase end, cherry-pick, reset) it resolves every anchor in touched paths against the new HEAD and the worktree, updates `known_in` for bases that appear in the new commit, and notifies clients (the extension rebuilds threads, `fwd watch` streams). A `refs/stash` change is observed only: the stash commit's second parent holds the index tree and the commit itself the worktree tree (https://git-scm.com/docs/git-stash, fetched 2026-09-24; verified today), so a popped stash returns known blob ids and resolutions come from cache. A worktree file change on an anchored path with no open session is hashed and ingested as a `worktree` version; with an open session the session is the source of truth and the watcher only logs.

## 5. Failure modes and their honest outputs

| Situation | Output |
|---|---|
| Basis blob unavailable: pruned, unpinned (fresh clone) with `nearest_blob` also gone, or an untracked file over the 256 KB `basis_patch` cap | `method: Fingerprint` with a `basis: unavailable` flag; location `Recovered` at best, never `Exact`, until the content reaches git. |
| Span deleted in the editor (cursors cross) and not found | `Unknown, Gone, Absent(Deleted)` with `hint.last_seen_line`; the thread becomes file-level with the snapshot and a Reposition command, as the extension design says. |
| Cut and paste inside the file | Cursors cross, whole-block fingerprint finds it: `Moved, Same, Present`. |
| Formatter pass | Diff mapping partial or empty, token similarity high: `Recovered{score}, Changed`. Agents are told the text changed. |
| The span's text occurs twice | `ambiguity: 2`, nearest-to-hint chosen, label never above `recovered`. |
| Checkout of a branch without the change | `Absent(NotInCheckout)` with the last commit it was known in; switching back hits the cache by blob id. |
| Editor delta stream desynchronised (missed event, extension restart) | Text-hash mismatch → full text requested → re-synced by diff; cursors survive because the text is the same; a diagnostic is logged. |
| Binary or very large file (> 2 MB) | No Automerge document; diff and fingerprint routes only; binary content is `SameBlob` or `Absent`. |
| Two entities for one `anchor_key` (race) | Exact coincidence: `Coincides` relation, merged by a person; nothing deleted. |

## 6. Incremental plan and experiments

**M1 Content-addressed basis (small).** In `anchoring.rs`: `form` computes `basis_blob` with `git hash-object -w --stdin --path` and pins it; records `head_blob`, `index_blob`, `nearest_blob`, `basis_patch`, `pieces`, `anchor_key`, `span_key`; `ops::create_anchor` becomes get-or-create by key; `AnchorBasis` is removed; `resolve_against` accepts `index` and `<blob>` targets and rebuilds the basis from the patch when the blob is missing. Fixtures: R1 (two clones), R2 (three blobs and pieces), the reconstruction round trip, an untracked file.
**M2 Three-fact resolution and the index (small).** The `Resolution` record of 3.5 with the derived label; `partial` from pieces; the CLI's `--against index`; the extension shows the label; the resolution cache moves to the sidecar SQLite. Fixtures: R3, R6 (amend, squash, cherry-pick with unchanged content).

**Status of M1 and M2, 2026-09-28: built** (Daniel, 2026-09-28, "Anchor robustness: the first two milestones now"), with the fixtures above as tests in `crates/fwd-core` (`src/anchoring.rs`, `tests/anchors.rs`) and the design as built in `../design/anchoring.md`. Where it departs from the text above: the basis is the index blob whenever the working-tree file matches the index, and `hash-object --path` otherwise (E1 below says why); `known_in` is not recorded (it belongs to the git observer of M6); the extension's showing of the label is its own task, and the CLI keeps the first cut's `state`, `start_line`, `end_line` and `path` beside the three facts meanwhile, `state` reporting `partial` as `modified`. Added by Daniel's rulings of the same day: whole-file anchors (an empty range in the key, availability alone), a `change` field on answers against the working tree (`unstaged`, `staged`, `both`, `none`, `untracked`), and renames followed when locating the file (from the target's history, a staged rename, or an untracked file holding a known blob), ahead of M3's cross-file search. Anchors made before M1 keep resolving as they did and gain a key and basis when a checkout can supply one.
**M3 Fingerprint index (small).** Section 3.6 replacing the window scan; cross-file candidates from `git diff --name-only -M -C`; `fwd anchor find`. Fixtures: R5's formatter case, cut-and-paste within a file, R7 with a rename and with a fresh file.
**M4 Sidecar version DAG.** Automerge documents, `ingest`, `version_heads`, cursors at form time, the cursor route in `resolve`, `isolate`/`integrate` under test with `fork_at` + `merge` as fallback. Fixtures: every M1–M3 fixture again through the cursor route with identical answers, plus `BeforeCreation`.
**M5 Editor sessions.** Delta streaming, hash validation, live ranges from cursors, save recognition, the watcher's "session wins" rule. Fixtures: R4, R5 with the file open, extension restart mid-session.
**M6 Git observer and team.** The observer of section 4 (none exists today), `known_in`, coincidence relations, `fwd anchor merge`, optional `refs/fwd/basis` push and fetch.

**Experiments, in the order their answers change the design.** E1 (before M1): the same content hashed on macOS and Linux clones with `core.autocrlf` and a `text=auto` attribute gives one id (the filter model is the risk). *Result, 2026-09-28 (git 2.54.0 on macOS; no Linux machine was at hand, and git's conversion is the same code there for the same settings):* one id. CRLF content (`one\r\ntwo\r\nthree\r\n`) hashed with `hash-object --stdin --path` gives `4cb29ea3`, its LF form's id, in a clone with `core.autocrlf=true` and in one with `* text=auto`, at a committed path and at a new one; without `--path` it keeps its CRLF id, `e1587ff9`. The risk was real in one case: when the index already holds the file with CRLF (committed before the setting or the attribute), `git add` keeps CRLF under either one while `hash-object --path`, which does not read the index, converts (`f384549c` against `git add`'s `7fbf4d84` for the same four lines). M1 therefore takes the index blob as the basis whenever the working-tree file matches the index (`git diff --quiet`, which does read it), and `hash-object --path` otherwise; a dirty file in that state gets the converted id, and resolves by diff (lines compared without their line ends) rather than as the same blob. The test `e1_line_endings_give_one_id` in `crates/fwd-core/src/anchoring.rs` repeats this on every run. E2 (M2): the three-hunk fixture through commit, stash, pop and `rebase --root`. E3 (done today): direct versus stepwise mapping on real history; repeat on a heavily refactored repository before history mirroring could ever become default. E4 (M3): formatter commits (rustfmt, prettier) from the ankurah and simplikeys histories; for every span of 3–30 lines before the commit, measure recovery by line diff, normalized line hashes and token similarity, and set the 0.6 threshold and the wildcard rule from the ROC rather than CodeStream's constant. E5 (M3): ankurah commits where a function moved between files (`git log -C -C --stat`), measuring cross-file recovery and candidate-set cost. E6 (M5): replay recorded VS Code `contentChanges` sequences (undo, multi-cursor, format-on-save) into the daemon; the text hash must match after every batch and IPC cost per keystroke stay under a millisecond. E7 (M4): sidecar size after a week of dogfooding against today's baseline (2.2–5.4× the latest file size for 27–114 versions; 40 ms load for the largest). E8 (M4): `isolate`/`integrate` versus `fork_at`/`merge` give identical heads and text for sibling ingests.

## 7. Open questions

1. **Content-derived `EntityId`.** Ankurah could add `Transaction::create_with_id(EntityId, &M)`; how two independent creations with one id merge (two root events in one entity's DAG) is untested. Lean: ship `anchor_key` with get-or-create and the `Coincides` fallback now; file the Ankurah feature and adopt it once two-root merging has a test, because idempotent creation removes a class of duplicates.
2. **Resolution cache location.** Sidecar SQLite versus the entity's `resolution` Json. Lean: sidecar; a resolution is per-checkout knowledge that churns on every commit, and syncing it produces LWW fights between machines. `known_in` stays on the entity because it is small and useful to teammates.
3. **Should the git history mirror ever be default?** Today's evidence says direct diffing is as accurate on committed history and the mirror costs ~76 ms per version with `update_text` (less with hunk splices). Lean: never by default; build on demand for `fwd anchor history` and for files with many anchors.
4. **Columns.** Store Unicode scalar offsets within the line, converting at the VS Code (UTF-16) and CLI (bytes) boundaries. Lean: yes; `cols` stays an empty key field until columns ship (Q11's default stands).
5. **Repository key in the identity.** The `Repo` entity id (workspace-scoped) versus the root commit id (recognisable across workspaces and forks). Lean: the entity id now; a root-commit index later if cross-workspace recognition is wanted.
6. **Sharing pins.** Push `refs/fwd/basis/*` to the origin as git-notes and git-bug do, so teammates get basis blobs without patches. Lean: opt-in per repository at M6; `basis_patch` covers the common case with no remote configuration. Daniel (2026-09-25) adds the requirement this must serve: an anchor or a scope may name a local branch that is unstaged, uncommitted, unpushed, or since deleted with its worktree, and the graph must handle that gracefully, by backing the referenced content up (the pinned basis plus `basis_patch` are that backup for anchored text; branch-level backup would be pushed refs or bundles) or by another grace such as the three-fact `absent` answer, never by losing the reference or erroring.
7. **Automerge versus Loro.** Both offer historical cursor resolution and diff-based text update; Automerge's deterministic hashing and API are the ones exercised today. Lean: Automerge, revisited only if E7 shows size or load-time trouble.
8. **Should the extension hold its own Automerge document** (`@automerge/automerge`) instead of streaming deltas? Lean: no; one resolver, one store, a thin extension; revisit if E6 shows IPC latency in the editor.

## Sources

Local: the design and research docs named above, `crates/fwd-core/src/{anchoring,model,ops}.rs`, `extensions/vscode/src/extension.ts`; cargo registry sources ankurah-core 0.9.3 (`src/property/backend/yrs.rs`, `src/property/value/yrs.rs`, `src/transaction.rs`, `src/node.rs`), ankurah-proto 0.9.3 (`src/id.rs`), automerge 0.11.0 (`src/cursor.rs`, `src/read.rs`, `src/autocommit.rs`, `src/transaction/transactable.rs`, `src/text_diff.rs`), yrs 0.24.0 (`src/moving.rs`, `src/doc.rs`, `src/state_vector.rs`). Experiments (2026-09-24): `scratchpad/gitexp2` (git, macOS) and `scratchpad/amexp` (Rust; automerge 0.11.0, similar 2.7, sha2 0.10) over the `~/ss/code/ankurah` and `~/sk/simplikeys` histories.

Web, all fetched 2026-09-24:
- git manual: https://git-scm.com/docs/git-hash-object, https://git-scm.com/docs/git-ls-files, https://git-scm.com/docs/git-add, https://git-scm.com/docs/git-stash, https://git-scm.com/docs/git-diff, https://git-scm.com/docs/git-log, https://git-scm.com/docs/git-gc, https://git-scm.com/docs/hash-function-transition.
- Automerge: https://docs.rs/automerge/latest/automerge/trait.ReadDoc.html (0.12.0, 2026-09-16), https://docs.rs/automerge/latest/automerge/enum.MoveCursor.html, https://docs.rs/automerge/latest/automerge/transaction/trait.Transactable.html, https://docs.rs/automerge/latest/automerge/struct.Automerge.html, https://docs.rs/automerge/latest/automerge/transaction/struct.CommitOptions.html, binary format spec https://automerge.org/automerge-binary-format-spec/ (draft, 2025-08-27), JS `getCursor` https://automerge.org/automerge/api-docs/js/functions/getCursor.html (3.5.0).
- Yjs/Yrs: https://docs.yjs.dev/api/relative-positions, https://docs.rs/yrs/latest/yrs/struct.StickyIndex.html (0.28.0).
- Loro (open question 7): https://docs.rs/loro/latest/loro/struct.LoroDoc.html (1.16.2; `LoroText::get_cursor`, `LoroDoc::checkout`).
- Zed anchors: https://zed.dev/blog/crdts (2022-12-01), https://raw.githubusercontent.com/zed-industries/zed/main/crates/text/src/anchor.rs.
- Peritext: https://www.inkandswitch.com/peritext/ (2021; CSCW 2022, DOI 10.1145/3555644). Eg-walker: arXiv 2409.14252 https://arxiv.org/abs/2409.14252 (EuroSys 2025).
- Line tracking: LHDiff https://www.cs.usask.ca/faculty/croy/papers/2013/AsaduzzamanICSM2013LineDraft.pdf and https://www.cs.usask.ca/~croy/papers/2013/LHDiffParvezToolICSM2013.pdf (ICSM 2013); Reiss, ICSE 2008, DOI 10.1145/1368088.1368091 (closed access; content as reported by LHDiff); CodeMapper https://arxiv.org/abs/2511.05205 (2025-11-07), code https://github.com/sola-st/CodeMapper; CodeTracker https://users.encs.concordia.ca/~nikolaos/publications/FSE_2022.pdf (ESEC/FSE 2022).
- Fingerprinting: simhash near-duplicate detection, Manku, Jain, Das Sarma, WWW 2007 https://static.googleusercontent.com/media/research.google.com/en//pubs/archive/33026.pdf; Roy, Cordy, Koschke, SCP 2009 https://research.cs.queensu.ca/home/cordy/Papers/RCK_SCP_Clones.pdf; Hypothesis fuzzy anchoring https://web.hypothes.is/blog/fuzzy-anchoring/ (2013-04-22).
- Diff libraries: https://docs.rs/similar/latest/similar/ (3.2.0; the workspace pins 2.7), https://docs.rs/imara-diff/latest/imara_diff/ (0.2.0, histogram by default, the faster choice if M3 needs it).
- VS Code: `src/vscode-dts/vscode.d.ts`, `src/vs/editor/common/model/pieceTreeTextBuffer/pieceTreeTextBuffer.ts`, `src/vs/editor/common/model/mirrorTextModel.ts`, `extensions/git/src/api/git.d.ts`, `extensions/git/src/uri.ts` (all https://github.com/microsoft/vscode, main); issues https://github.com/microsoft/vscode/issues/11487, /88310, /48364, /54147; LSP 3.17 `textDocument/didChange` https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/; gitrevisions https://git-scm.com/docs/gitrevisions.
- Version-control models: https://pijul.org/manual/theory.html, https://docs.jj-vcs.dev/latest/working-copy/, https://sapling-scm.com/docs/dev/internals/linelog/. tree-sitter: https://docs.rs/tree-sitter/latest/tree_sitter/ (0.27.0).
