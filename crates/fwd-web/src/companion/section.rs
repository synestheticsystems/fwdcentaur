//! One section of the companion view, the session's own or a principal's: its heading, what is said
//! of it, and its records, each looked up by its id ([`Record`]). The session's Active Intentions
//! are one plain list, and its other sections lists whose records stand apart, as the document's
//! prominence says.

use super::markdown;
use super::record::Record;
use ankurah::EntityId;
use fwd_core::session_view::{Document, List, ListKind, Prominence, Row};
use leptos::prelude::*;

/// Where a section stands in the document: among the session's own, or in the group of the
/// principal whose id it holds, by its kind, of which each holds one at most.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SectionAt {
    group: Option<EntityId>,
    pub kind: ListKind,
}

impl SectionAt {
    /// One of the session's own sections.
    pub fn root(kind: ListKind) -> SectionAt {
        SectionAt { group: None, kind }
    }

    /// A section of the principal `group`'s.
    pub fn of(group: EntityId, kind: ListKind) -> SectionAt {
        SectionAt { group: Some(group), kind }
    }

    /// The section in `document`, where it stands now.
    pub fn find(self, document: &Document) -> Option<&List> {
        let sections = match self.group {
            None => &document.sections,
            Some(id) => &document.principals.iter().find(|g| g.id == id)?.sections,
        };
        sections.iter().find(|s| s.section == self.kind)
    }

    /// The record `id` of the section in `document`.
    pub fn row(self, document: &Document, id: EntityId) -> Option<&Row> {
        self.find(document)?.records.iter().find(|r| r.id == id)
    }
}

/// The section `at` of the document `doc` shows, with `first`, the question opened ready to answer
/// at the page's first draw.
#[component]
pub fn Section(doc: Signal<Option<Document>>, at: SectionAt, first: Signal<Option<EntityId>>) -> impl IntoView {
    let head = Memo::new(move |_| {
        doc.with(|d| {
            d.as_ref().and_then(|d| at.find(d)).map(|s| (s.title.clone(), s.prominence == Prominence::Primary))
        })
    });
    // What is said of it, and where the page lists all it holds, where the document cut it short.
    let said = Memo::new(move |_| {
        doc.with(|d| {
            let d = d.as_ref()?;
            let section = at.find(d)?;
            let more = section.more.as_ref().zip(d.page.as_ref()).map(|(more, page)| format!("{page}{more}"));
            Some((section.lines.clone(), more))
        })
    });
    let ids = Memo::new(move |_| {
        doc.with(|d| {
            d.as_ref()
                .and_then(|d| at.find(d))
                .map(|s| s.records.iter().map(|r| r.id).collect::<Vec<_>>())
                .unwrap_or_default()
        })
    });
    let primary = move || head.with(|h| h.as_ref().is_some_and(|(_, primary)| *primary));
    view! {
        <section class="companion-section" class:primary=primary>
            <h2>{move || head.get().map(|(title, _)| title)}</h2>
            {move || {
                said.get().filter(|(lines, _)| !lines.is_empty()).map(|(lines, more)| match more {
                    // A section cut short says so in its own words, which lead to the page's list.
                    Some(more) => view! {
                        <a class="section-lines" href=more target="_blank" rel="noopener noreferrer">{blocks(&lines)}</a>
                    }
                    .into_any(),
                    None => view! { <div class="section-lines">{blocks(&lines)}</div> }.into_any(),
                })
            }}
            <ul class="records">
                <For each=move || ids.get() key=|id| *id children=move |id| {
                    view! { <Record doc=doc at=at id=id opened=first.get_untracked() == Some(id)/> }
                }/>
            </ul>
        </section>
    }
}

/// `lines`, the document's words, under `class`, and nothing at all where there are none.
#[component]
pub fn Lines(class: &'static str, lines: Signal<Vec<String>>) -> impl IntoView {
    move || lines.with(|lines| (!lines.is_empty()).then(|| view! { <div class=class>{blocks(lines)}</div> }))
}

/// Each of `lines`, a Markdown block, as the page draws it.
pub fn blocks(lines: &[String]) -> Vec<impl IntoView> {
    lines.iter().map(|line| view! { <div class="md" inner_html=markdown::html(line)></div> }).collect()
}
