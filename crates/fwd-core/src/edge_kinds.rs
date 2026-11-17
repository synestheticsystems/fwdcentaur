//! fwd's relation words on the kinds of edge the Commons mints (`ankurah_commons::graph`), in one
//! place: the kind an edge is written as for each word, and the word an edge of a minted kind reads
//! as. A kind is a record every node holds the same, imported with the kept mint
//! (`crate::durable`), so fwd names it by the id it was minted with and never looks it up by name.

use crate::model::RelationKind;
use ankurah::property::Ref;
use ankurah::EntityId;
use ankurah_commons::graph::EdgeKind;
use strum::IntoEnumIterator;

impl RelationKind {
    /// The minted kind an edge of this word is written as: the kind of the same name. The Commons
    /// mints none for `motivated_by`, so its edges are written as `discovered_from`, which keeps
    /// where the task came from.
    pub fn edge_kind(self) -> Ref<EdgeKind> {
        Ref::new(
            EntityId::from_base64(self.minted()).expect("every minted id here decodes, as this module's tests show"),
        )
    }

    /// The word an edge of the minted kind `kind` reads as: `discovered_from` for one written for
    /// `motivated_by`, and none for a kind fwd has no word for, such as `serves`.
    pub fn of_edge_kind(kind: EntityId) -> Option<RelationKind> {
        let kind = kind.to_base64();
        RelationKind::iter().find(|word| *word != RelationKind::MotivatedBy && word.minted() == kind)
    }

    /// The id of the kind's record, as the Commons' kept mint states it (`graph/edge_kind.akev`).
    fn minted(self) -> &'static str {
        match self {
            RelationKind::DependsOn => "ojLY_tMxG-XheeqJcGK1uLJWXF8xFSj5yB0QAsQOKrE",
            RelationKind::SubtaskOf => "9Cg3XfJI5BoUGhITE-p9lUmsQzZENchWxvyDdryWHK4",
            RelationKind::MotivatedBy | RelationKind::DiscoveredFrom => "2g2bJsqzY0dAakt7PjVJIJV8-SHCGQU2UAqZlDJsSpU",
            RelationKind::RelatesTo => "AAM6muCKBpgKoEvHX9ZKSaorAy1tsCXwRcwYmPJXLMk",
            RelationKind::Supersedes => "htF5z9cM-MQvvSr5nCWRnEZphnBJQHWAW7zB3cN46e0",
            RelationKind::Duplicates => "vf0JMZ9adSehvrGs4IPacjSrGjS9_sNHHLsjjUIMiiA",
            RelationKind::AnchoredAt => "MsvhH49dAXxkNpUlyzW7YTck5xLpwO57ZBcgIS07TAY",
            RelationKind::AssignedTo => "t9ADjEj7Ry3aYfv-JlBD0k0RPyw_Cn7gPJQotxEGGPU",
            RelationKind::Concerns => "6E9NIZjDMxGCRVJV7F09BV01cR5O7ynQ9a39Seop5_w",
            RelationKind::Cites => "dl64nCiSj1AjPWFLgHMH6uDHtf8WYqrlDCFUsLJnOWw",
            RelationKind::WorkedIn => "Y6v74d6BVSnFWPZNSXedeeA47ES_a6h8rosrJ-ZvJ5Q",
            RelationKind::StepOf => "pfK-1IsFqYxYI_ZKDDAmCpewRWevY-0Amrh7hrkX9mQ",
            RelationKind::Mention => "TfjrIGux_mWdDDvFYVPDq-lqzg-7Lw65wT6AoYqa_h0",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ankurah::policy::DEFAULT_CONTEXT;
    use ankurah_commons::graph::{EdgeKindView, EDGE_KINDS};
    use ankurah_storage_sqlite::SqliteStorageEngine;

    /// On a node seeded with the kept mint, each word's kind is the record of a kind the Commons
    /// lists, named as the word but for `motivated_by`, and an edge of each kind fwd writes for a
    /// word of its own reads as that word again.
    #[tokio::test]
    async fn each_word_is_written_as_a_minted_kind_and_reads_back_as_itself() -> anyhow::Result<()> {
        let node = crate::durable::open(SqliteStorageEngine::open_in_memory().await?).await?;
        let ctx = node.context_async(DEFAULT_CONTEXT).await?;
        for word in RelationKind::iter() {
            let kind: EdgeKindView = ctx.get(*word.edge_kind()).await?;
            let name = kind.name()?;
            assert!(EDGE_KINDS.iter().any(|(listed, _)| *listed == name), "{word} is written as {name}");
            match word {
                RelationKind::MotivatedBy => assert_eq!(name, "discovered_from"),
                _ => {
                    assert_eq!(name, word.to_string());
                    assert_eq!(RelationKind::of_edge_kind(*word.edge_kind()), Some(word));
                }
            }
        }
        let [serves]: [EdgeKindView; 1] = ctx.fetch("name = 'serves'").await?.try_into().expect("minted once");
        assert_eq!(RelationKind::of_edge_kind(serves.id()), None, "a kind fwd has no word for");
        Ok(())
    }
}
