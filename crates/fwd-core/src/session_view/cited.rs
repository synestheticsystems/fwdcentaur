//! What the records' texts cite, named for the person: a message or a task stores each reference
//! as the id of what it names (`[[<id>]]`, `references`), and the view shows the person names, never
//! ids. Each cited record is followed wherever it is, and shown by what names it, as the inbox's
//! sentences name it (`reason`): a task by its title, a decision by its number and title, a scope by
//! its title, a highlight by its file, a message by its first words, a person or an agent by `@` and
//! its name, an intention by its title; in a Markdown block, as a link to its page where the page has
//! one.

use crate::anchor_name::AnchorName;
use crate::project_graph::{Followed, ProjectGraph, Referent, Whose};
use crate::references::{self, Named};
use crate::route::{page_path, Route};
use ankurah::EntityId;
use anyhow::Result;
use std::collections::HashMap;
use uuid::Uuid;

/// What a reference that names nothing fwd holds is shown as.
const NOT_FOUND: &str = "something fwd cannot find";

/// The records the texts of one read cite, followed.
#[derive(Debug, Clone, Default)]
pub(super) struct Cited {
    found: HashMap<EntityId, Option<Found>>,
}

/// A cited record fwd holds: what names it, and the project it is in, by its UUID.
#[derive(Debug, Clone)]
struct Found {
    record: EntityId,
    project: Uuid,
    referent: Referent,
}

impl Cited {
    /// Follows every record `texts` cite, through `graph`, whose project's UUID is `here`.
    pub(super) async fn read<'t>(
        graph: &ProjectGraph,
        here: Uuid,
        texts: impl IntoIterator<Item = &'t str>,
    ) -> Result<Cited> {
        let mut found = HashMap::new();
        for text in texts {
            for (_, named) in references::stored_named(text) {
                if found.contains_key(&named.id) {
                    continue;
                }
                let followed = match graph.follow_any(named.id).await? {
                    Followed::Held { record, project, found: referent } => {
                        let project = match project {
                            Whose::This => here,
                            Whose::Other(there) => there.uuid,
                        };
                        Some(Found { record, project, referent })
                    }
                    Followed::NotHeld => None,
                };
                found.insert(named.id, followed);
            }
        }
        Ok(Cited { found })
    }

    /// `text` with each reference shown by what names it.
    pub(super) fn plain(&self, text: &str) -> String {
        references::render_named(text, |named| Some(self.label(named)))
    }

    /// `text` with each reference shown by what names it, as a Markdown link to its page, after
    /// `page`, where the page has one.
    pub(super) fn markdown(&self, text: &str, page: &str) -> String {
        references::render_named(text, |named| {
            let label = escaped(&self.label(named));
            Some(match self.place(named.id) {
                Some(path) => format!("[{label}]({page}{path})"),
                None => label,
            })
        })
    }

    fn label(&self, named: &Named) -> String {
        let Some(Some(found)) = self.found.get(&named.id) else { return NOT_FOUND.into() };
        match &found.referent {
            Referent::Task { title, .. } => title.clone(),
            Referent::Decision { number, title, .. } => format!("D{number} {title}"),
            Referent::Scope { title, .. } => title.clone(),
            Referent::Message { words, .. } if words.is_empty() => "a message".into(),
            Referent::Message { words, .. } => format!("“{words}”"),
            Referent::Anchor { path, lines } => match (&named.version, lines) {
                (Some(version), _) => AnchorName::file_at(path, version).text(),
                (None, Some((start, end))) => format!("{path}:{}-{end}", start + 1),
                (None, None) => path.clone(),
            },
            Referent::Principal { handle } => format!("@{handle}"),
            Referent::Relation { kind } => format!("a {} relation", kind.to_string().replace('_', " ")),
            Referent::Intention { title, .. } => title.clone(),
        }
    }

    /// Where the page shows the cited record, after its address; none for what has no page.
    fn place(&self, id: EntityId) -> Option<String> {
        let found = self.found.get(&id)?.as_ref()?;
        let route = match found.referent {
            Referent::Task { .. } => Route::task(found.record),
            Referent::Decision { .. } => Route::decision(found.record),
            Referent::Scope { .. } => Route::scope_tasks(found.record),
            Referent::Anchor { .. } => Route::Anchor(found.record),
            Referent::Message { .. }
            | Referent::Principal { .. }
            | Referent::Relation { .. }
            | Referent::Intention { .. } => return None,
        };
        Some(page_path(found.project, &route))
    }
}

/// `label` with what Markdown reads as markup escaped, so that it shows as the words it is.
fn escaped(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for c in label.chars() {
        if matches!(c, '\\' | '[' | ']' | '(' | ')' | '<' | '>' | '*' | '_' | '`' | '!' | '#') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}
