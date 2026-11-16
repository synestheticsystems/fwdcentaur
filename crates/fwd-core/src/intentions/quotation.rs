//! Whose words an intention's description holds. An intention belongs to no one (D229), so it
//! carries no `Attribution`: the words as someone gave them are a quotation, a record of its own
//! beside the intention (`Quotation`, with `Created` for who recorded them and `Attribution` for
//! whose they are when someone else recorded them), placed in the intention's project and joined to
//! it by a `relates_to` relation from the quotation, which becomes `supports` once fwd's relations
//! move to the Commons' edges. Each wording given makes a quotation, so an intention reworded keeps
//! the quotations of its words before, and its latest says whose words it holds now.

use super::{at, principal_ref};
use crate::model::{PlacementView, RelationKind, RelationStatus};
use crate::ops;
use crate::project_graph::{Followed, ProjectGraph, ProjectTransaction};
use crate::rules;
use ankurah::model::Mutable;
use ankurah::property::PropertyError;
use ankurah::EntityId;
use ankurah_commons::records::{Attribution, AttributionView, Created, CreatedView};
use ankurah_commons::text::{Quotation, QuotationView};
use anyhow::Result;
use serde_json::{json, Value};

/// The components every quotation carries, which a read takes it by.
pub type Quoted = (QuotationView, CreatedView);

/// Records in `trx` that `words`, an intention's description as given, are `speaker`'s, written
/// down by `by` at `now` ([`quote`]). When they were said is known only where `by` gives its own
/// words.
pub(super) async fn quote_given(
    trx: &ProjectTransaction,
    intention: EntityId,
    words: String,
    speaker: EntityId,
    by: EntityId,
    now: i64,
) -> Result<EntityId> {
    let said = (speaker == by).then(|| at(now));
    let quotation = Quotation { text: words, said, source: None };
    let created = Created { created: at(now), by: principal_ref(by) };
    quote(trx, intention, &quotation, &created, Some(speaker), by, json!({"actor_id": by.to_base64()})).await
}

/// Places `quotation` in `trx`'s project, recorded as `created` says, with an `Attribution` naming
/// `speaker` when the words are not those of whoever recorded them (`None` for a speaker no one
/// knows), and joins it to the intention `intention` by an accepted `relates_to` relation that
/// `by` makes, `via` saying what caused it. The quotation's id.
pub(super) async fn quote(
    trx: &ProjectTransaction,
    intention: EntityId,
    quotation: &Quotation,
    created: &Created,
    speaker: Option<EntityId>,
    by: EntityId,
    via: Value,
) -> Result<EntityId> {
    let made = trx.create_placed(quotation).await?;
    let record = Mutable::entity(&*made);
    trx.add(record, created).await?;
    if speaker != Some(*created.by) {
        trx.add(record, &Attribution { speaker: speaker.map(principal_ref) }).await?;
    }
    let quoted = record.id();
    let ends = (quoted, intention);
    let relation =
        ops::new_relation(trx.project(), by, RelationKind::RelatesTo, ends, RelationStatus::Accepted, via, None, None);
    trx.create(&relation).await?;
    Ok(quoted)
}

/// Whose words a quotation holds: the speaker its `Attribution` names when it carries one, else
/// whoever recorded them (`created`); none when its `Attribution` names no one, the words being
/// someone else's, speaker unknown.
pub fn speaker(
    created: &CreatedView,
    attribution: Option<&AttributionView>,
) -> Result<Option<EntityId>, PropertyError> {
    Ok(match attribution {
        Some(a) => a.speaker()?.map(|speaker| *speaker),
        None => Some(*created.by()?),
    })
}

/// The records joined to the intention `intention` by its accepted `relates_to` relations,
/// wherever they are: its quotations, and any other record related to it. A reader that follows
/// whose words the intention holds listens to each of them, and to the relations.
pub async fn joined_to(graph: &ProjectGraph, intention: EntityId) -> Result<Vec<EntityId>> {
    let mut joined = Vec::new();
    for r in graph.relations_to(intention, Some(RelationKind::RelatesTo)).await? {
        if rules::holds(RelationKind::RelatesTo, r.status()?) {
            joined.push(r.from_id()?);
        }
    }
    Ok(joined)
}

/// The quotation that says whose words an intention holds, of the records `joined` to it
/// ([`joined_to`]): the one recorded last, by when it was recorded and then by id. None when no
/// quotation supports it: a quotation is optional (D229).
pub async fn quotation_of(graph: &ProjectGraph, joined: &[EntityId]) -> Result<Option<(PlacementView, Quoted)>> {
    let mut latest: Option<((i64, EntityId), (PlacementView, Quoted))> = None;
    for id in joined {
        // Another record related to the intention is not held as a quotation.
        let Followed::Held { found, .. } = graph.follow_placed::<Quoted>(*id).await? else { continue };
        let recorded = (found.1 .1.created()?.unix_millis(), found.0.id());
        if latest.as_ref().is_none_or(|(last, _)| recorded > *last) {
            latest = Some((recorded, found));
        }
    }
    Ok(latest.map(|(_, quoted)| quoted))
}

/// Whose words an intention's description holds, as the latest of the quotations among the records
/// `joined` to it says ([`quotation_of`]): the speaker, none when no one knows who, with the
/// quotation's `Created`, which says who recorded the words and when. None when no quotation
/// supports it.
pub async fn whose_words_among(
    graph: &ProjectGraph,
    joined: &[EntityId],
) -> Result<Option<(Option<EntityId>, CreatedView)>> {
    let Some((placement, (_, created))) = quotation_of(graph, joined).await? else { return Ok(None) };
    let attribution = graph.component_of::<AttributionView>(&placement).await?;
    Ok(Some((speaker(&created, attribution.as_ref())?, created)))
}

/// Whose words the intention `intention`'s description holds, as the latest of its quotations says
/// ([`joined_to`], [`whose_words_among`]).
pub async fn whose_words(graph: &ProjectGraph, intention: EntityId) -> Result<Option<(Option<EntityId>, CreatedView)>> {
    whose_words_among(graph, &joined_to(graph, intention).await?).await
}
