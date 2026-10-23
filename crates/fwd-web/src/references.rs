//! What a message's text names, drawn as the lozenges of what it names (docs/DECISIONS.md,
//! "Inline references are shadowed by `mention` edges", 2026-09-28: each reference draws as "the
//! lozenge render component for the (first) member model of that entity"). The posting path
//! stores every reference as the entity's whole id in double brackets (`[[<id>]]`), whatever the
//! writer typed (`@handle`, an id's first characters, `D12`), and writes a `mention` edge beside
//! it, which lets the thing named list the message under "mentioned in" (`lozenges.rs`). So the
//! page looks each id up in its standing queries and draws the lozenge of the first model it
//! finds; an id it does not hold draws as a lozenge with its short id. Text written before that
//! kept `@handle` as typed: a bare `@handle` draws as the person's lozenge when a principal has
//! that handle, and stays text otherwise; so does anything in double brackets that is not an id.
//!
//! A file a message names at a version (`fwd_core::file_mention`) is stored as its whole-file
//! highlight's id with the version after an at-sign (`[[<id>@<version>]]`), and draws as a lozenge
//! naming the highlight's path and the version the text names, since the highlight serves every
//! version that holds the same bytes.
//!
//! What counts as a reference is `fwd_core::references::find`'s answer, the one the posting path
//! reads by, so code (a code span, a fenced block) stays text here too. What each reference draws
//! as is worked out as data first ([`resolve`]) and tested natively.

use crate::lozenges::{FileVersion, Target};
use crate::referents::Mode;
use crate::{anchor_row, fmt, lozenges, modes, person, store, thread};
use ankurah::EntityId;
use fwd_core::file_mention::Version;
use fwd_core::references::{find, Written};
use leptos::prelude::*;

/// What a stretch of a message's text draws as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Drawn<I> {
    Text(String),
    /// The lozenge of the entity with this id.
    Entity(I),
    /// A whole file a mention names: the whole-file highlight with this id, at the version the
    /// text names.
    File(I, Version),
}

/// Adds `text` to what is drawn, joined to the text before it.
fn push_text<I>(drawn: &mut Vec<Drawn<I>>, text: &str) {
    if text.is_empty() {
        return;
    }
    match drawn.last_mut() {
        Some(Drawn::Text(before)) => before.push_str(text),
        _ => drawn.push(Drawn::Text(text.to_string())),
    }
}

/// What `text` draws as: each reference in it whose entity `id` (for what stands in double
/// brackets) or `handle` (for an `@handle`) finds, as that entity's lozenge, and a file mention
/// (an id, an at-sign and a version a stored mention writes, `fwd_core::references::Named`) as the
/// file at its version; every other stretch, a reference nothing was found for among them, as the
/// text it was, joined to the text around it.
pub fn resolve<I>(text: &str, id: impl Fn(&str) -> Option<I>, handle: impl Fn(&str) -> Option<I>) -> Vec<Drawn<I>> {
    let mut drawn = Vec::new();
    let mut from = 0;
    for found in find(text) {
        push_text(&mut drawn, &text[from..found.range.start]);
        let entity = match &found.written {
            Written::Bracketed(inner) => match inner.split_once('@') {
                Some((at, version)) => id(at).zip(Version::parse(version)).map(|(file, v)| Drawn::File(file, v)),
                None => id(inner).map(Drawn::Entity),
            },
            Written::Handle(name) => handle(name).map(Drawn::Entity),
        };
        match entity {
            Some(entity) => drawn.push(entity),
            None => push_text(&mut drawn, &text[found.range.clone()]),
        }
        from = found.range.end;
    }
    push_text(&mut drawn, &text[from..]);
    drawn
}

/// What `text` draws as, from the page's standing queries: any id that reads as one is its
/// entity's, held or not, and a handle is the principal's who has it, while no other principal of
/// the project has it too (`fwd_core::naming::by_name`).
fn resolved(text: &str) -> Vec<Drawn<EntityId>> {
    let handle = |name: &str| match fwd_core::naming::by_name(&store::principals(), name, None).as_slice() {
        [one] => Some(one.id()),
        _ => None,
    };
    resolve(text, |inner| inner.parse::<EntityId>().ok(), handle)
}

/// A message's text with each reference drawn as the lozenge of what it names. Each lozenge
/// looks its entity up in its own closure, so the text around it does not redraw when that
/// entity arrives or changes.
pub fn content(text: &str) -> AnyView {
    resolved(text)
        .into_iter()
        .map(|drawn| match drawn {
            Drawn::Text(text) => text.into_any(),
            Drawn::Entity(id) => {
                (move || modes::draw(Mode::Embedded, Mode::Lozenge, id, &modes::models_of(id))).into_any()
            }
            Drawn::File(id, version) => lozenges::single(Target::File(id, FileVersion::of(&version))),
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// A message's stored text as its writer would type it again, for the box that edits it: a person
/// by `@handle` while no other principal of the project has the handle, a decision by `[[D12]]`,
/// anything else, a person whose handle is shared included, by the start of its id in double
/// brackets (`fwd_core::references::editable`), each looked up in the page's standing queries, so
/// the box shows names a person can read and the edit resolves them to what they named: a handle
/// two principals share would be refused when the edit is saved.
pub fn editable(text: &str) -> String {
    fwd_core::references::editable(text, |id| {
        if store::principal(id).is_some() {
            return store::unique_handle(id).map(Written::Handle);
        }
        store::decision(id).and_then(|d| d.number().ok()).map(|n| Written::Bracketed(format!("D{n}")))
    })
}

/// `text` with each reference named in words, where a lozenge cannot go (a lozenge's own name, a
/// preview, a list item, an inbox card's line), as the CLI's text output names them: a person by
/// `@handle`, anything else in brackets, a task by its title and the rest as it names itself where
/// it shows again ("[D12 Assignment is a field]").
pub fn plain(text: &str) -> String {
    resolved(text)
        .into_iter()
        .map(|drawn| match drawn {
            Drawn::Text(text) => text,
            Drawn::Entity(id) => {
                let models = modes::models_of(id);
                match Target::by_models(id, &models) {
                    Target::Principal(id) => {
                        format!("@{}", person::name(id).unwrap_or_else(|| thread::name(id, &models)))
                    }
                    Target::Task(id) => format!(
                        "[{}]",
                        store::task(id).and_then(|t| t.title().ok()).unwrap_or_else(|| thread::name(id, &models))
                    ),
                    _ => format!("[{}]", thread::name(id, &models)),
                }
            }
            Drawn::File(id, version) => {
                format!("[{}]", anchor_row::file_name(id, &version).unwrap_or_else(|| fmt::short(id)))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> EntityId {
        EntityId::from_bytes([n; 32])
    }

    /// The page's lookups, with one principal, alice, whose id is `id(50)`.
    fn draw(text: &str) -> Vec<Drawn<EntityId>> {
        resolve(text, |inner| inner.parse::<EntityId>().ok(), |name| (name == "alice").then(|| id(50)))
    }

    #[test]
    fn references_draw_as_what_they_name_and_the_rest_stays_text() {
        let (task, other) = (id(1), id(9));
        let text = format!("ask @alice about [[{}]] and [[{}]], not @nobody.", task.to_base64(), other.to_base64());
        assert_eq!(
            draw(&text),
            vec![
                Drawn::Text("ask ".into()),
                Drawn::Entity(id(50)),
                Drawn::Text(" about ".into()),
                Drawn::Entity(task),
                Drawn::Text(" and ".into()),
                Drawn::Entity(other),
                Drawn::Text(", not @nobody.".into()),
            ],
            "a handle nobody has stays as written, joined to the text around it"
        );
    }

    #[test]
    fn a_stored_reference_is_its_entitys_lozenge_whether_the_page_holds_it_or_not() {
        let unheld = id(7);
        assert_eq!(draw(&format!("[[{}]]", unheld.to_base64())), vec![Drawn::Entity(unheld)]);
    }

    #[test]
    fn what_is_not_an_id_or_a_handle_stays_text() {
        for text in ["[[D12]] and [[notanid]]", "mail t@example.com", "[[two words]]", "[[unclosed", "a lone @ here"] {
            assert_eq!(draw(text), vec![Drawn::Text(text.into())], "{text}");
        }
        assert_eq!(draw(""), vec![]);
    }

    #[test]
    fn a_file_mention_draws_as_the_file_at_the_version_its_text_names() {
        let file = id(3);
        let commit = "e13b8e216257b10dc4f3df4908ab34c26ed5497b";
        let text = format!("The primer is [[{f}@{commit}]], or [[{f}@worktree]] on disk", f = file.to_base64());
        assert_eq!(
            draw(&text),
            vec![
                Drawn::Text("The primer is ".into()),
                Drawn::File(file, Version::Commit(commit.into())),
                Drawn::Text(", or ".into()),
                Drawn::File(file, Version::Worktree),
                Drawn::Text(" on disk".into()),
            ]
        );
        // A version no stored mention writes, and a path the posting path never stores, stay text.
        for text in [format!("[[{}@HEAD]]", file.to_base64()), "[[docs/cli.md@index]]".into()] {
            assert_eq!(draw(&text), vec![Drawn::Text(text.clone())], "{text}");
        }
        // Its version, held by value for its lozenge, comes back whole.
        for version in [Version::Worktree, Version::Index, Version::Commit(commit.into())] {
            assert_eq!(FileVersion::of(&version).version(), version);
        }
    }

    #[test]
    fn code_stays_text_as_the_posting_path_reads_it() {
        let text = format!("run `@alice [[{}]]` then\n```\n@alice\n```\n", id(1).to_base64());
        assert_eq!(draw(&text), vec![Drawn::Text(text.clone())]);
    }
}
