//! `fwd admin migrate`'s move of intentions written on the Commons' models of 84ea084 onto the
//! Commons' kept mint (`fwd_core::intentions::convert`), on a store the build before the mint wrote,
//! on a node never seeded, then opened as fwd opens one (`fwd_core::durable`): each intention keeps
//! its id, its old components, its title, its words as its description, its status as `Closed`, its
//! speaker on a quotation, and its assignments, all on the mint's definitions, whichever side of the
//! mint's the node's own definition of `records.attribution` sorts; a second run changes nothing;
//! and an intention that would lose part of what it holds stops the run before anything is written.

use ankurah::core::schema::ModelStructDescriptor;
use ankurah::model::Mutable;
use ankurah::property::{Json, Ref};
use ankurah::proto::{Attested, Event};
use ankurah::{policy::DEFAULT_CONTEXT, EntityId, Model, ModelId, Node, PermissiveAgent, View};
use ankurah_commons::akev::{Creation, ModelDefinitions};
use ankurah_commons::cognition::{Intention, IntentionView};
use ankurah_commons::records::{Attribution, ClosedView, CreatedView, EditedView};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::durable;
use fwd_core::intentions::{self, before_d229, Outcome, Parts, Quoted};
use fwd_core::model::*;
use fwd_core::ops;
use fwd_core::project_graph::{Followed, ProjectGraph};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

/// When the intentions here were written down, and when the one reworded was reworded.
const WRITTEN: i64 = 1_700_000_000_000;
const REWORDED: i64 = WRITTEN + 3_600_000;
const FULFILLED: i64 = WRITTEN + 7_200_000;

/// What the build before the kept mint declared by its label alone, beside `before_d229`.
mod old {
    use ankurah::{Model, Ref};
    use ankurah_commons::identity::Principal;
    use serde::{Deserialize, Serialize};

    /// Whose words a record holds, declared by its label alone, so that a node never seeded mints a
    /// definition of its own for it.
    #[derive(Model, Debug, Clone, Serialize, Deserialize)]
    #[model(label = "records.attribution")]
    #[model(no_ffi)]
    pub struct Attribution {
        pub speaker: Ref<Principal>,
    }
}

/// Which side of the kept mint's definition of `records.attribution` the node's own sorts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Below,
    Above,
}

/// The id the kept mint gives the model `descriptor` declares.
fn minted(descriptor: &ModelStructDescriptor) -> EntityId {
    EntityId::from_base64(descriptor.explicit_id.expect("a model of the Commons declares its minted id")).unwrap()
}

/// The node's own definition of `records.attribution`, as the build before the kept mint minted it
/// when it first bound `old::Attribution`, made as the Commons make their mint, with a nonce that
/// puts its id `side` of the mint's: its id, and its creation events for a store to import before
/// anything binds. An id is the hash of its creation event, so a node minting its own puts it below
/// the mint's only about one time in eighteen.
fn own_attribution(side: Side) -> Result<(EntityId, Vec<Attested<Event>>)> {
    // `CoHr…` below the mint's `DfGh…`, and `GFp8…` above it.
    let nonce = match side {
        Side::Below => [3; 32],
        Side::Above => [0; 32],
    };
    let definitions =
        ModelDefinitions::declared(old::Attribution::descriptor(), |_| Creation { at: WRITTEN as u64, nonce });
    let events = definitions.events()?;
    let own = events[0].entity_id;
    assert_eq!(
        own < minted(Attribution::descriptor()),
        side == Side::Below,
        "{} sorts {side:?} the mint's",
        own.to_base64()
    );
    Ok((own, events.into_iter().map(Attested::from).collect()))
}

/// A store the build before the kept mint wrote, with alpha's person Ines and its agent impl-tide.
struct Store {
    _dir: TempDir,
    path: PathBuf,
    project: uuid::Uuid,
    ines: EntityId,
    tide: EntityId,
}

impl Store {
    /// A new store, as the build before the kept mint wrote one, and alpha's graph on its node,
    /// which was never seeded and holds a definition of `records.attribution` of its own whose id
    /// sorts `side` of the mint's ([`own_attribution`]).
    async fn before_the_mint(side: Side) -> Result<(Store, ProjectGraph)> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("fwd.sqlite");
        let node = Node::new_durable(Arc::new(SqliteStorageEngine::open(&path).await?), PermissiveAgent::new());
        node.system.create().await?;
        let ctx = node.context_async(DEFAULT_CONTEXT).await?;
        let (own, definitions) = own_attribution(side)?;
        node.import_events(&definitions).await?;
        assert_eq!(
            ctx.resolve_model_id::<old::Attribution>().await?,
            ModelId::EntityId(own),
            "bound to it, not minted"
        );
        let project = uuid::Uuid::new_v4();
        let alpha = ProjectGraph::ensure(ctx, project, "alpha").await?;
        let ines = ops::ensure_principal(&alpha, "ines", "Ines", PrincipalKind::Human, None).await?;
        let tide = ops::ensure_principal(&alpha, "impl-tide", "impl-tide", PrincipalKind::Agent, None).await?;
        Ok((Store { _dir: dir, path, project, ines, tide }, alpha))
    }

    /// alpha's graph on the store opened as fwd opens one, seeded with the kept mint.
    async fn opened(&self) -> Result<ProjectGraph> {
        let node = durable::open(SqliteStorageEngine::open(&self.path).await?).await?;
        Ok(ProjectGraph::open(node.context_async(DEFAULT_CONTEXT).await?, self.project).await?)
    }
}

/// What an intention held on the Commons' models of 84ea084: its record, written down by `by`; its
/// speaker, when someone else's words; its last rewording, by whom and when; and how it closed.
struct Old {
    intention: before_d229::Intention,
    by: EntityId,
    speaker: Option<EntityId>,
    reworded: Option<(EntityId, i64)>,
    closure: Option<before_d229::Closure>,
}

fn old(title: &str, status: &str, by: EntityId) -> Old {
    let words = format!("{title}, in the speaker's words");
    let intention = before_d229::Intention { title: title.into(), words, status: status.into() };
    Old { intention, by, speaker: None, reworded: None, closure: None }
}

/// Writes `old` into `alpha` as the build before the kept mint wrote an intention, with an
/// `assigned_to` relation to `to`; its id.
async fn written(alpha: &ProjectGraph, to: EntityId, old: Old) -> Result<EntityId> {
    let trx = alpha.begin();
    let made = trx.create_placed(&old.intention).await?;
    let record = Mutable::entity(&*made);
    trx.add(record, &before_d229::Creation { created_by: Ref::new(old.by), created: WRITTEN }).await?;
    if let Some(speaker) = old.speaker {
        trx.add(record, &old::Attribution { speaker: Ref::new(speaker) }).await?;
    }
    if let Some((by, at)) = old.reworded {
        trx.add(record, &before_d229::LastEdit { edited: at, edited_by: Some(Ref::new(by)) }).await?;
    }
    if let Some(closure) = &old.closure {
        trx.add(record, closure).await?;
    }
    let id = made.id();
    trx.create(&Relation {
        project: alpha.project(),
        kind: RelationKind::AssignedTo,
        from_id: id,
        to_id: to,
        status: RelationStatus::Accepted,
        created_by: Ref::new(to),
        created: WRITTEN,
        via: Json::new(serde_json::json!({})),
        note: None,
        position: None,
        closed_by: None,
        closed: None,
        closed_reason: None,
    })
    .await?;
    trx.commit().await?;
    Ok(id)
}

/// The quotations that say whose words the intention `id` holds.
async fn quotations(alpha: &ProjectGraph, id: EntityId) -> Result<Vec<(PlacementView, Quoted)>> {
    let mut found = Vec::new();
    for r in alpha.relations_to(id, Some(RelationKind::RelatesTo)).await? {
        found.push(alpha.get_placed(r.from_id()?).await?);
    }
    Ok(found)
}

/// Its new components, read as a list reads it, beside those of 84ea084, which stay.
async fn moved(alpha: &ProjectGraph, id: EntityId) -> Result<(PlacementView, Parts)> {
    let record: (PlacementView, Parts) = alpha.get_placed(id).await?;
    assert!(alpha.component_of::<before_d229::IntentionView>(&record.0).await?.is_some(), "the old ones stay");
    let on_the_mint = ModelId::EntityId(minted(Intention::descriptor()));
    assert!(record.0.entity().has_membership(&on_the_mint), "the mint's definition");
    Ok(record)
}

/// Three intentions: one written down for Ines, one she reworded through impl-tide, and one in her
/// own words that she fulfilled. Each comes through with what it held, on the mint's definitions, its
/// speaker on a quotation recorded by whoever last wrote its words down, when they did, and still
/// assigned.
async fn each_intention_moves_whole(side: Side) -> Result<()> {
    let (store, before) = Store::before_the_mint(side).await?;
    let (ines, tide) = (store.ines, store.tide);
    let for_ines = written(&before, tide, Old { speaker: Some(ines), ..old("Readers keep their place", "open", tide) });
    let for_ines = for_ines.await?;
    let reworded = Old {
        speaker: Some(ines),
        reworded: Some((tide, REWORDED)),
        ..old("Builds stay under a minute", "open", tide)
    };
    let reworded = written(&before, tide, reworded).await?;
    let closure = before_d229::Closure {
        closed: Some(FULFILLED),
        closed_by: Some(Ref::new(ines)),
        reason: Some("shipped".into()),
        reason_message: None,
    };
    let fulfilled = written(&before, tide, Old { closure: Some(closure), ..old("Name the page", "fulfilled", ines) });
    let fulfilled = fulfilled.await?;
    drop(before);
    let alpha = store.opened().await?;
    assert_eq!(alpha.models_labelled("records.attribution").await?.len(), 2, "the node's own beside the mint's");

    let converted = intentions::convert(&alpha, ines).await?;
    let mut added: Vec<(EntityId, Vec<&str>)> = converted.iter().map(|c| (c.intention, c.added.clone())).collect();
    added.sort();
    let mut expected = vec![
        (for_ines, vec!["cognition.intention", "records.created"]),
        (reworded, vec!["cognition.intention", "records.created", "records.edited"]),
        (fulfilled, vec!["cognition.intention", "records.created", "records.closed"]),
    ];
    expected.sort();
    assert_eq!(added, expected);

    let (placement, (intention, created)) = moved(&alpha, for_ines).await?;
    assert_eq!(
        (intention.title()?, intention.description()?),
        ("Readers keep their place".into(), "Readers keep their place, in the speaker's words".into())
    );
    assert_eq!((*created.by()?, created.created()?.unix_millis()), (tide, WRITTEN));
    assert_eq!(intentions::outcome(alpha.component_of::<ClosedView>(&placement).await?.as_ref())?, None);
    let [(quoted, (quotation, recorded))] = &quotations(&alpha, for_ines).await?[..] else { panic!("one quotation") };
    assert_eq!(quotation.text()?, "Readers keep their place, in the speaker's words");
    assert_eq!((*recorded.by()?, recorded.created()?.unix_millis(), quotation.said()?), (tide, WRITTEN, None));
    assert!(quoted.entity().has_membership(&ModelId::EntityId(minted(Attribution::descriptor()))));
    let speaker = intentions::whose_words(&alpha, for_ines).await?.and_then(|(speaker, _)| speaker);
    assert_eq!(speaker, Some(ines), "her words, as the node's own attribution said");

    let (placement, _) = moved(&alpha, reworded).await?;
    let edited = alpha.component_of::<EditedView>(&placement).await?.expect("reworded");
    assert_eq!((edited.edited()?.unix_millis(), edited.by()?.map(|by| *by)), (REWORDED, Some(tide)));
    let (speaker, recorded) = intentions::whose_words(&alpha, reworded).await?.expect("quoted");
    let recorded = (*recorded.by()?, recorded.created()?.unix_millis());
    assert_eq!((speaker, recorded), (Some(ines), (tide, REWORDED)), "as last written down");

    let (placement, _) = moved(&alpha, fulfilled).await?;
    let closed = alpha.component_of::<ClosedView>(&placement).await?.expect("fulfilled");
    assert_eq!(intentions::outcome(Some(&closed))?, Some(Outcome::Fulfilled));
    let closed = (closed.closed()?.map(|at| at.unix_millis()), closed.by()?.map(|by| *by), closed.reason()?);
    assert_eq!(closed, (Some(FULFILLED), Some(ines), Some("shipped".into())));
    let [(_, (quotation, recorded))] = &quotations(&alpha, fulfilled).await?[..] else { panic!("one quotation") };
    assert_eq!((*recorded.by()?, quotation.said()?), (ines, Some(recorded.created()?)), "her own, said then");

    let assigned: Vec<EntityId> = intentions::assigned_to(&alpha, tide)
        .await?
        .into_iter()
        .filter_map(|(id, followed)| matches!(followed, Followed::Held { .. }).then_some(id))
        .collect();
    assert_eq!(assigned.len(), 3, "still assigned: {assigned:?}");

    assert_eq!(intentions::convert(&alpha, ines).await?, vec![], "a second run");
    assert_eq!(quotations(&alpha, for_ines).await?.len(), 1, "and no second quotation");
    Ok(())
}

#[tokio::test]
async fn each_intention_moves_whole_where_the_nodes_own_attribution_definition_sorts_below_the_mints() -> Result<()> {
    each_intention_moves_whole(Side::Below).await
}

#[tokio::test]
async fn each_intention_moves_whole_where_the_nodes_own_attribution_definition_sorts_above_the_mints() -> Result<()> {
    each_intention_moves_whole(Side::Above).await
}

/// A closure that names the message giving its reason has no place in `Closed`, whether its
/// intention ended or is open with nothing else of a closing recorded, so the run stops naming each
/// such intention, and the other, which would move whole, does not move either.
#[tokio::test]
async fn an_intention_that_would_lose_part_of_what_it_holds_stops_the_run_before_anything_is_written() -> Result<()> {
    let (store, before) = Store::before_the_mint(Side::Below).await?;
    let whole = written(&before, store.tide, old("Readers keep their place", "open", store.ines)).await?;
    let message = Some(EntityId::from_bytes([7; 32]));
    let closure = before_d229::Closure {
        closed: Some(FULFILLED),
        closed_by: Some(Ref::new(store.ines)),
        reason: Some("see the thread".into()),
        reason_message: message,
    };
    let withdrawn = Old { closure: Some(closure), ..old("Name the page", "withdrawn", store.ines) };
    let withdrawn = written(&before, store.tide, withdrawn).await?;
    let only_the_message =
        before_d229::Closure { closed: None, closed_by: None, reason: None, reason_message: message };
    let open = Old { closure: Some(only_the_message), ..old("Builds stay under a minute", "open", store.ines) };
    let open = written(&before, store.tide, open).await?;
    drop(before);
    let alpha = store.opened().await?;

    let refused = format!("{:#}", intentions::convert(&alpha, store.ines).await.expect_err("refused"));
    let why = |id: EntityId| refused.split("; ").find(|each| each.contains(&id.to_base64()));
    for losing in [withdrawn, open] {
        assert!(why(losing).is_some_and(|why| why.contains("message giving the reason")), "{refused}");
    }
    assert_eq!(why(whole), None, "{refused}");
    let placement: PlacementView = alpha.get(whole).await?;
    assert!(alpha.component_of::<IntentionView>(&placement).await?.is_none(), "nothing written");
    assert!(alpha.component_of::<CreatedView>(&placement).await?.is_none());
    assert!(quotations(&alpha, whole).await?.is_empty());
    Ok(())
}
