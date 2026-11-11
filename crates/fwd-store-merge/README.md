# fwd-store-merge

Moves every project's store of a fwd data folder into the one store the daemon keeps for all of
them (docs/DECISIONS.md, D174). The input is a data folder in the per-project layout, with Ankurah
5d8a000b's stored format: one `projects/<project>/project.sqlite` per project. The output is a new
data folder in the one-store layout: one `store.sqlite` holding every project's records, each
record naming its project, renumbered, and findable by the id it had.

```
fwd-store-merge --from <data folder> --to <new data folder>
```

Stop the daemon that serves the folder first (`cutover.py` does, below). The source is only read
(SQLite opens each store as an unchanging file); the destination must not exist. The tool writes
`<new data folder>.partial-<pid>` beside it, readable by its owner alone, and renames it, with the
source folder's permissions, only once the new store is judged; it removes that folder on any
failure it survives, and a tool that is killed can leave it behind, which no fwd reads. It prints,
per project, how many records of each model it moved, how many fields it set in a second pass, the
ids that name nothing (carried as they were), and the records `fwd admin migrate` would still change
(carried as they are, since the current models represent them).

## What it carries

- Each record's current values, read with plain SQLite from the old store's materialization
  tables, each table found through the engine's model map and each column through its column map
  and the store's catalog. Ankurah's event history is not carried: it stays in the old folder, which
  the per-project build still reads.
- Every record is made again through a node of this build on a fresh store, with fwd-core's models
  in plain Ankurah transactions: one `Project` per project, made as the daemon makes it
  (`ProjectGraph::ensure`), then every record with its project named and every value as it was. No
  daemon runs, nothing is routed or notified, and no field is set to now; decision numbers and every
  time stay as they were.
- Ids: every reference, and every string in a JSON value (and in `Principal::serves`) that is the
  whole id of a record of the same project, names that record's new id. An id that names nothing is
  carried as it was and reported. Text people typed is carried as typed: the ids it cites still find
  their records through the former ids. A `CodeAnchor`'s key, which hashes its repository's id, is
  made again with fwd-core's `anchor_key` from the new one, once the stored key is checked to be the
  one the old id gives.
- One `FormerId` per old id, naming the record it names now, so that every place that takes an id
  takes the old one, whole or as a prefix.
- Order: each project's records of a model are made in the order they were made in their old store,
  one per transaction, and their materialized rows are then put back in the order the old tables
  held them, so lists come back in the order they did.
- Every other file of the folder byte for byte, but each project's `anchors.sqlite`, the anchors'
  cache, which rebuilds, and each project's `checkouts.json`, whose scope bindings are rewritten to
  the scopes' new ids.

A record is made only once every record its required references name is: the models go Principal,
Repo, Scope, Task, CodeAnchor, Decision, Message, Relation, Notification. A reference to a record
made later (a principal's successor or the scopes it serves, a task's parent in its context, a
decision's closing message, a message's thread and what it replies to, a relation's end on another
relation) is set in a second pass.

## What it refuses

Anything the current models cannot represent: a table of a model other than the nine, a column with
values that names no field of its model, a required field that is empty, an enum word this build
does not know, an anchor whose stored key is not the one fwd-core makes; a folder with a write-ahead
log, shared-memory file or journal beside a database (a daemon serves it); a folder that holds
`store.sqlite` already; and a store of a project the index does not list. Any refusal leaves no
destination.

## The judge

Before the destination may exist, the new store is judged against the old ones with plain SQLite,
the old ids mapped through the new store's own `FormerId` records and its references found through
its own catalog, not through the tool's own mapping: for every project and model, the same number
of records; each old id with exactly one former id, naming a record of the same model and project;
every value as it was, references and JSON ids mapped; every id naming a record of the same
project, or carried unchanged from an old value that named nothing; and the records made, and
their rows held, in the order they were. JSON is compared parsed, whatever its spacing; a `Json`
field that does not hold JSON is refused, and text in `Principal::serves` that is not JSON, which
the move carries as it was, must be the same to the byte.

## Switching a machine over

`crates/fwd-store-convert/cutover.py --hop one-store` stops the daemon, backs up, runs this tool and
exchanges the data folder and the installed `fwd` under the daemon's locks; `back --hop one-store`
goes back. The old folder rests as `<data>.per-project`, the old binary as `<fwd>.per-project`, and
after going back the merged folder rests as `<data>.one-store`. What a SIGKILL can leave and how to
finish or undo it by hand is in `crates/fwd-store-convert/README.md`, with `f8e85d2` read as
`per-project` and `5d8a000b` as `one-store`.
