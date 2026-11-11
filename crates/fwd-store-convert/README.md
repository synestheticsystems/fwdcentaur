# fwd-store-convert

Converts a fwd data folder whose project stores were written by Ankurah **f8e85d2** (the
`feat/storage-node-sqlite-wasm` branch, fwd's pin until 2026-11-11) into a new data folder that
Ankurah **5d8a000b** (main, fwd's pin since) reads. A fwd built at the new pin cannot open the old
stores: three stored shapes changed without a version that tells them apart.

| What | At f8e85d2 | At 5d8a000b |
| --- | --- | --- |
| A clock (an entity's head, an event's parent) | a list of event ids | a list of (generation, event id) pairs |
| An update event's id | hashes the parent clock as a list of ids | hashes it with the generations, so every update event's id changes |
| An LWW value's entry | the event that wrote it | the writer's generation and id |

A genesis has no parent, so its id, which is its entity's id, does not change. Event bodies,
collaborative-text (yrs) state, memberships and every file other than the stores are carried byte
for byte. Attestations kept their shape, but only empty ones are accepted (an attestation signs
bytes this hop rewrites, and fwd's daemon attests nothing). Everything the converter knows about the
two formats is in `src/f8e85d2_to_5d8a000b.rs`; when Ankurah's stored format next moves, the next
hop starts from a copy of that module.

## Use

Stop the daemon that serves the folder first, then:

```
fwd-store-convert --from <data folder> --to <new data folder>
```

The source is only read (SQLite opens it as an unchanging file, so nothing is written beside it).
The destination must not exist; the converter writes `<new data folder>.partial-<pid>` beside it and
renames it only once every store is converted and judged. It removes that folder on any failure it
survives, a panic included; a converter that is killed can leave it behind, and no fwd reads it, so
remove it by hand. It prints, per project, the entities, the events and how many event ids changed,
or why it refused the store; any refusal leaves no destination.

It refuses:

- a folder with a write-ahead log, shared-memory file or journal beside a database: a daemon is
  serving it, or the last process to open that database did not close it. This is a look at the
  folder when the converter starts, not a lock: stop the daemon and keep it stopped (`cutover.py`
  does) until the conversion is over;
- a store that records a protocol version other than 1, or holds a table its engine did not make
  or register;
- an event stored under an id its content does not hash to, an event whose parent is not stored,
  a head or LWW value naming an event the entity does not have, state for a backend other than
  `lww` and `yrs`, a membership that does not decode exactly, and any attestation.

## How a store is converted

Each project's store is read row by row, each entity's events are rewritten from its genesis,
parents before children (generation, parent clock, id), and its head and LWW entries are rewritten
to name the new ids. The result is written into a fresh file through Ankurah 5d8a000b's own SQLite
engine, so it lays out its tables and materializes every state itself, under the table and column
names the old engine recorded, and with every row where it sat in its table, since fwd lists what a
query finds in the order the store yields it.

Then two judges look at the new file, and either refuses the store:

- Ankurah 5d8a000b reads it back. Every event row decodes into an event whose id, as that revision
  computes it, is the row's own key, and which is the event that was converted, and every converted
  event is stored. Every state reads back through a fresh engine as it was converted, decodes, and
  names stored events in its head and as the writers of its LWW values.
- An inventory, read with plain SQLite from the old file and the new one and resting on nothing the
  conversion computed: the same entity ids, the same number of events, and in every materialization
  table the same rows, in the same order, with equal values in every column the new table has (a
  column only the old table has must be empty in every old row: the engine adds a column when a
  value first needs it). The order is reproduced when each entity belongs to one model, as fwd's
  do; a store whose order could not be, such as one with an entity in two models, is refused.

## Switching a machine over

`cutover.py` beside this file stops the daemon, backs up, converts, and exchanges the data folder
and the installed `fwd`, holding the daemon's locks so that no daemon of either build starts in
between; `cutover.py back` goes back. This hop is `--hop ankurah-main`; the script also carries the
one-store hop (crates/fwd-store-merge/README.md). Its `--help` lists the steps. A failure, SIGINT, SIGTERM or
SIGHUP before the new daemon answers undoes what was switched. After it has answered, clients may
have written to it, so a failure is reported and the new pair stays; going back is a decision.

A SIGKILL runs no undo. The locks go with the process, and a command may then start the daemon of
whichever build is installed on whichever folder is in place; a daemon of either build refuses the
other build's folder, failing every project and writing nothing. Here `<data>` is the data folder,
`<fwd>` the installed binary, and `<new fwd>` the staged one. What a kill during `forward` leaves:

| Killed during | Left behind | To finish or undo |
| --- | --- | --- |
| steps 1 to 5 (stop, backup, conversion) | the old pair, unchanged; maybe a stopped backup's `.partial` folder in the backups folder, and `<data>.5d8a000b.partial-<pid>` or a finished `<data>.5d8a000b` | `rm -rf "<data>.5d8a000b"*`, then run `forward` again |
| step 6 (the exchange) | the converted folder at `<data>`, the old one at `<data>.5d8a000b` or `<data>.f8e85d2`, the old `<fwd>` | finish: `[ -d "<data>.5d8a000b" ] && mv "<data>.5d8a000b" "<data>.f8e85d2"`; `cp "<new fwd>" "<fwd>.incoming" && ln "<fwd>" "<fwd>.f8e85d2" && mv "<fwd>.incoming" "<fwd>"`; `fwd daemon stop` |
| step 7 (the install) | as above, plus maybe `<fwd>.incoming` and `<fwd>.f8e85d2` (a link to the old binary) | if `<fwd>` is still the old build: `rm -f "<fwd>.incoming"`, and the finish above without its `ln` when `<fwd>.f8e85d2` exists; then `fwd daemon stop` |
| steps 8 to 10 | the new pair in place; the daemon the script started keeps running | nothing: check with `fwd daemon status` |
| the undo, failed or killed | the undo puts the old binary back first, then the old folder: a step 6 or step 7 state, which the script's last line names by the paths that exist | finish as in that row; `back` then goes back, if wanted |

What a kill during `back` leaves. Before its exchange nothing is to be deleted: `back` makes no
backup and no folder, so both folders and both binaries are the ones to keep.

| Killed during | Left behind | To finish or undo |
| --- | --- | --- |
| steps 1 to 3 (stop) | the new pair, unchanged, with `<data>.f8e85d2` and `<fwd>.f8e85d2` beside it | keep everything; run any fwd command, which starts the installed build's daemon again, then run `back` again |
| step 6 (the exchange) | the old folder at `<data>`, the converted one at `<data>.f8e85d2` or `<data>.5d8a000b`, the new `<fwd>`, `<fwd>.f8e85d2` | finish: `[ -d "<data>.f8e85d2" ] && mv "<data>.f8e85d2" "<data>.5d8a000b"`; `ln "<fwd>" "<fwd>.5d8a000b" && mv "<fwd>.f8e85d2" "<fwd>"`; `fwd daemon stop` |
| step 7 (the install) | as above, plus maybe `<fwd>.5d8a000b` (a link to the new binary) | if `<fwd>` is still the new build: the finish above without its `ln` when `<fwd>.5d8a000b` exists; then `fwd daemon stop` |
| steps 8 to 10 | the old pair in place; the daemon the script started keeps running | nothing: check with `fwd daemon status` |
| the undo, failed or killed | a step 6 or step 7 state of `back`, named by the script's last line | finish as in that row |
