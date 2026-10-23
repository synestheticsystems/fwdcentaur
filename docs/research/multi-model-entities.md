# Multi-model entities on the pinned Ankurah line

Written 2026-09-26. The spike ran natively against Ankurah `90dff659` (the `storage-engine-contract` branch, 0.10.0, unpublished), with SQLite storage and the WebSocket connectors. Every result below comes from a passing test in a throwaway crate outside this repository. Where a statement comes from reading the source instead, it says so.

## What it is for

fwdcentaur wants Manifold's generic models (an annotation, an edge, an anchor, an actor, a titled item) to live on the same entities, under the same ids, as its product models (`Task`, `Message`, ...). One id could then be both a `Message` and an annotation. The component library could choose renderers from the set of models an entity carries, and no product model would have to move (docs/DECISIONS.md, "The component library waits for Manifold's models").

All of this works on this line today, through a thin API one level below `Transaction::create`. What is missing is listed under the verdict at the end.

Ankurah's LEXICON.md calls a model that an entity carries a *component*, and so does the error for a missing one. The code, and this note, still say `Model`. The tests used two models:

```rust
#[derive(Model)] pub struct Note { #[active_type(LWW)] pub title: String, pub body: String } // body: Yrs text
#[derive(Model)] pub struct Item { #[active_type(LWW)] pub status: String }
```

| Question | Result | API or error |
|---|---|---|
| One entity carrying both | pass, at creation or later | `Model::initialize_new_entity`, or `LocalTrxEntity::add_membership` then `trx.get::<Item>` |
| Read and edit through either | pass; edits through both make one event | `ctx.get::<NoteView>` / `ctx.get::<ItemView>`; `trx.get::<Note>` and `trx.get::<Item>` share one snapshot |
| Query one model, open the other | pass | `ctx.fetch::<NoteView>(..)` then `ctx.get::<ItemView>(id)`; `MEMBEROF('item')` in a predicate |
| Live queries | pass | gaining `Item` adds the entity to `Item` queries; `Item` queries hear edits made through `Note` |
| On disk | one entity row, one association row per model | `_ankurah_entity_model`, and one row per model table |
| `Note`-only entity asked for as `Item` | error from every path | `RetrievalError::MissingComponent { entity_id, model_id }` |
| Daemon and CLI over WebSocket | pass, both directions | `WebsocketServer` / `WebsocketClient` |

## Putting a second model on an entity

`Transaction` has no call for this. The pieces are on the transaction entity that a mutable wraps (`Mutable::entity()`, a `LocalTrxEntity`). To create an entity with both models, stage the second model before anything asks for the id:

```rust
let item_model = ctx.resolve_model_id::<Item>().await?; // registers Item if needed
let trx = ctx.begin();
let note = trx.create(&Note { title: "a".into(), body: "b".into() }).await?;
Item { status: "open".into() }.initialize_new_entity(note.entity(), item_model, note.entity().system_epoch())?;
let id = note.id(); // freezes the genesis, which now adds both models and carries both models' values
trx.commit().await?;
```

If `Item` is staged after `note.id()`, it lands in a second event of the same transaction instead. To add `Item` to an existing entity, a later transaction opens the entity as `Note` and stages `Item`, and the entity gains one update event:

```rust
let trx = ctx.begin();
let note = trx.get::<Note>(&id).await?;
note.entity().add_membership(item_model)?;
trx.get::<Item>(&id).await?.status()?.set(&"open".to_string())?; // reuses the snapshot, which now carries Item
trx.commit().await?;
```

Two edges need guarding today:
- Calling `initialize_new_entity` for a model the entity already carries inserts the model's Yrs text again: a body of "old" became "newold".
- `add_membership` accepts any `ModelId`. An unregistered random id committed, and SQLite made a table for it (`m_cusjaehc`) after waiting one second for a catalog label.

## Reading and editing through either model

`ctx.get::<NoteView>(id)` and `ctx.get::<ItemView>(id)` wrap the same resident entity. Either view's `entity().memberships()` lists both models, which is the set a renderer registry would key on. In one transaction, `trx.get::<Note>` and `trx.get::<Item>` returned two mutables over one snapshot. The test set `title` and `body` through the first and `status` through the second, and the commit wrote one event. Views fetched before the edit showed all three changes after the commit.

## Querying by one model, opening the other

`ctx.fetch::<NoteView>("title = 'x'")` returned the entity, and `ctx.get::<ItemView>(id)` opened it as `Item`. `ctx.fetch::<ItemView>("status = 'open'")` found the same id from the other side. A predicate can test membership by label:
- `"title = 'x' AND MEMBEROF('item')"` returned only the entity that carries both models.
- `"title = 'x' AND (NOT (MEMBEROF('item')))"` returned only the `Note`-only entity. Without the parentheses the predicate does not parse ("Expected atomic expression, got UnaryNot").

A predicate cannot name the other model's fields. `"title = 'x' AND status = 'open'"` on `NoteView` fails with "unknown property 'status' in model '<id>'".

## Live queries

The test ran three live queries on one durable node while one entity went through four transactions:

| Transaction | `ItemView`, `status = 'open'` | `NoteView`, `title = 'watched'` | `NoteView`, `MEMBEROF('item')` |
|---|---|---|---|
| create it as `Note` only | nothing | Add | nothing |
| add `Item`, status open | Add | Update (view now lists both models) | Add |
| edit `Note.body` only | Update | Update | Update |
| set `Item.status` to closed | Remove | Update | Update |

Once an entity matches a query, the reactor re-evaluates that query on every change to the entity, whichever model made the change. The mechanism is the entity watchers in `core/src/reactor/watcherset.rs`.

## What the durable node stores

The test made three entities: one with `Note` only, one with both models from creation, and one with `Note` first and `Item` a transaction later. In the SQLite file:
- `_ankurah_entity` holds one row per entity, and `_ankurah_event` holds each entity's events once. Neither table has a model column.
- `_ankurah_entity_model` holds one row per entity per model, keyed by the bincode `ModelId`: one row for the first entity and two for each of the others.
- The `note` and `item` tables each hold a row for every entity that carries that model. Each row projects every property in the entity's state, not just its own model's:

```
note: id=added_later, body='l', title='added later', status='open'
item: id=added_later, body='l', status='open', title='added later'
```

A model added later therefore looks the same on disk as one given at creation, apart from the extra event. specs/storage/architecture.md section 3 (the catalog resolver) specifies this projection. It has two consequences for fwdcentaur:
- An annotation on a message copies the message's `content` into the annotation's table.
- When two models on one entity each declare `title`, each table gives the plain column name to one of them and a suffixed name (`title_lddg`) to the other. In the second model's own table, `title` held `Note`'s title. Queries stay correct because they resolve by property id, but the file misleads anyone who reads it directly.

## A `Note`-only entity asked for as `Item`

`ctx.get`, `ctx.get_cached`, `ItemView::from_entity`, `trx.get::<Item>` and `trx.edit::<Item>` all return `Err(RetrievalError::MissingComponent { entity_id, model_id })`, which displays as "Entity … does not have component …". Nothing panics or falls back to a default, and `ctx.fetch::<ItemView>("true")` does not list the entity.

## The daemon and the CLI over WebSocket

`WebsocketServer::run` served a durable node on a SQLite file, and an ephemeral node on in-memory SQLite connected to it through `WebsocketClient`. These are the shapes `crates/fwd/src/serve.rs` and `crates/fwd-core/src/daemon.rs` build. The client resolved `Item` to the durable node's id. Then:
- An entity the client created with both models reached the durable node with both memberships and both projected rows.
- The client added `Item` to a `Note`-only entity that the durable node had created. The durable node stored the second event and both memberships.
- An entity the durable node created with both models reached the client, through `fetch::<ItemView>` and `get::<NoteView>`, as one resident entity carrying both.
- A client live query on `Item` gained an entity when the durable node added `Item` to it, then heard an edit the durable node made through `Note`.

## Verdict

fwdcentaur can rely on everything in the results table today. Ankurah's own `tests/tests/multi_model_membership.rs` uses `add_membership` the same way, so this is intended behaviour.

What is missing, most useful first, with the Ankurah change that would close each gap:

1. **A typed call that adds a model to an entity.** Callers today resolve the `ModelId`, reach through `Mutable::entity()`, and must not re-initialise a model the entity already carries. The fix is a `Transaction` method beside `create` and `get` in `core/src/transaction.rs`. It would take the model value and the transaction entity, not an id, so a pending creation's genesis stays open. It would then register the model (`SchemaResolver::ensure_registered`), refuse a model already carried, call `Model::initialize_new_entity`, run the write check, and return the typed mutable. The doc comment on `initialize_new_entity` in `core/src/model.rs` should cover existing entities too.
2. **Refusing memberships in unknown models.** `LocalTrxEntity::add_membership` (`core/src/entity/trx/local.rs`) takes any id, and `check_models` in `core/src/policy/context.rs` refuses only reserved system models. The local commit path (`core/src/transaction/commit.rs`, where `check_genesis_membership` runs) could refuse a model the local catalog does not know.
3. **A field shared between a generic model and a product model.** Same-named fields of two models are separate properties by default. `#[property(id = "...")]` can make them share one: a model whose `title` was bound to `Note.title`'s property id read, wrote and queried `Note`'s title. But each system mints that id for itself, and fwdcentaur has one system per project, so the id cannot go in source. The fix is a binding by model label and field name, resolved at registration: in the explicit-id branch of `register` (`core/src/schema/catalog/register.rs`) and the `#[property(...)]` parser (`derive/src/model/schema.rs`). Manifold's titled item could then read `Task.title` instead of keeping a copy.
4. **Removing a model from an entity.** There is no API for this. `Membership` in `proto/src/data.rs` has only `Add`. SQLite's `commit` in `storage/sqlite/src/engine/transaction.rs` refuses a state that drops a membership. `member_view` in `core/src/resultset.rs` assumes memberships are never removed. specs/storage/architecture.md section 1.3 (entity-model association) leaves removal undefined. So this is a design decision, not a patch. fwdcentaur can do without it by giving generic models a status: a retracted annotation stays an annotation.
5. **Predicates over another model's fields.** `resolve_selection` in `core/src/schema/catalog/resolver.rs` resolves names per model, even though every SQLite model table and the reactor already hold every property. Until that changes, query the model whose fields you filter on, add `MEMBEROF('<other label>')`, and open the other view per id.
6. **`NOT` before a bare term.** `ankql/src/parser.rs` accepts `NOT` only as the first token of an expression and only before a parenthesised expression.
7. **Stale documents.** Two passages predate `add_membership` and need updating. The associations section of Ankurah's docs/internals/storage-engines.md says "add-to-existing is deferred". The required-tests list in specs/storage/architecture.md section 6 says the core path for adding a model to an existing entity "remains #412".

One point on fwdcentaur's own side, read from source only. A model's catalog identity is its label, which defaults to the lowercased struct name (`collection_str` in `derive/src/model/description.rs`). Manifold's models and fwdcentaur's therefore share one namespace per project, and prefixed `#[model(label = "...")]` labels keep them apart.

Not established: the browser's IndexedDB engine and wasm connector, the extension host's storage, policy agents other than `PermissiveAgent`, and two nodes adding different models to one entity at the same moment.
