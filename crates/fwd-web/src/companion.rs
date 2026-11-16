//! A Claude Code session's companion view, `#/sessions/<session id>`: one session's page, opened
//! beside its chat in the desktop's browser pane (380 to 720 px wide) or in a browser, so that the
//! person stays aligned with what the session works toward and answers it from there (Daniel,
//! 2026-11-15, D226: "Lets call this the session companion view. Presumably it should be built into
//! the main fwd app, but minus all the general navigation, and highly focused for this one
//! session."). It shows what `fwd session view` gives the pane beside the chat, made by the same
//! reads (`fwd_core::session_view`) and kept current by the same live queries (`follow.rs`), under
//! the same title, with none of the app's navigation: the session's Active Intentions first, then,
//! after a bit of vertical whitespace, what each principal of the session holds (Daniel,
//! 2026-11-13: "the header should say fwd Current Session or something like that / and the section
//! heading for the first section should say Active Intentions, then the list of intentions. / then
//! a bit of vertical whitespace, then the rest of the stuff actively associated with this session in
//! whatever form"). A principal's name, role and project head its sections only when the session has
//! more than one. Each part of the page takes the id of what it draws and looks it up in the
//! document as the document changes, so that a record is drawn again only when it changes itself,
//! and stays open or closed as the person left it.

mod answer;
mod follow;
mod markdown;
mod record;
mod section;

use crate::app::NewerBuild;
use crate::{empty, session};
use ankurah::signals::Get as AnkurahGet;
use ankurah::EntityId;
use follow::Follow;
use fwd_core::session_view::{Document, Group, ListKind, OUT_OF_REACH, TITLE};
use leptos::prelude::*;
use section::{Lines, Section, SectionAt};
use send_wrapper::SendWrapper;
use std::time::Duration;

/// Whether the page's connection to the daemon is up, for the parts that write.
#[derive(Clone, Copy)]
pub struct Connected(pub Signal<bool>);

/// What makes the ages on the page read the clock again, once a minute ([`keep_ages`]).
#[derive(Clone)]
pub struct Minute(pub ArcTrigger);

/// The companion view of the Claude Code session `session`.
#[component]
pub fn Companion(session: String) -> impl IntoView {
    let follow = Follow::start(session);
    let doc: Signal<Option<Document>> = follow.shown().into();
    let follow = SendWrapper::new(follow);
    // While the connection to the daemon is lost the page keeps what it shows, says so, and lets
    // nothing be sent; drafts stay where they are written.
    let connected = Memo::new(move |_| session::client().connection_state().get().to_string() == "Connected");
    provide_context(Connected(connected.into()));
    Effect::new({
        let follow = follow.clone();
        move |was: Option<bool>| {
            let now = connected.get();
            match (was, now) {
                (None | Some(true), false) => follow.lost(),
                (Some(false), true) => follow.back(),
                _ => {}
            }
            now
        }
    });
    on_cleanup(move || follow.stop());
    keep_ages();
    let title = Memo::new(move |_| doc.with(|d| d.as_ref().map_or_else(|| TITLE.to_string(), |d| d.title.clone())));
    // The tab says what the page shows, and the app's name again once the page shows the app.
    Effect::new(move |_| set_tab_title(&title.get()));
    on_cleanup(|| set_tab_title("fwd"));
    let lines = Memo::new(move |_| doc.with(|d| d.as_ref().map(|d| d.lines.clone()).unwrap_or_default()));
    let sections = Memo::new(move |_| doc.with(|d| d.as_ref().map(sections_of).unwrap_or_default()));
    let principals = Memo::new(move |_| doc.with(|d| d.as_ref().map(principals_of).unwrap_or_default()));
    let several = Signal::derive(move || principals.with(|p| p.len() > 1));
    // The question opened at the page's first draw, ready to answer: decided once, by the first
    // document read.
    let first = Memo::new(move |decided: Option<&Option<Option<EntityId>>>| match decided {
        Some(Some(first)) => Some(*first),
        _ => doc.with(|d| d.as_ref().map(first_waiting)),
    });
    let first = Signal::derive(move || first.get().flatten());
    view! {
        <div class="companion">
            <NewerBuild/>
            <header class="companion-header">{move || view! { <Title title=title.get()/> }}</header>
            {move || (!connected.get()).then(|| view! { <p class="connection-note" role="status">{OUT_OF_REACH}</p> })}
            <main class="companion-content">
                {move || doc.with(Option::is_none).then(|| view! { <p class="muted">{empty::LOADING}</p> })}
                <Lines class="document-lines" lines=lines.into()/>
                <For each=move || sections.get() key=|kind| *kind children=move |kind| {
                    view! { <Section doc=doc at=SectionAt::root(kind) first=first/> }
                }/>
                <For each=move || principals.get() key=|id| *id children=move |id| {
                    view! { <Principal doc=doc id=id several=several first=first/> }
                }/>
            </main>
        </div>
    }
}

/// The view's title, its first word in the app's mark where that is the app's name, as the app's
/// header shows it.
#[component]
fn Title(title: String) -> impl IntoView {
    match title.strip_prefix("fwd ") {
        Some(rest) => view! {
            <h1>
                <span class="brand"><span class="mark">"fwd"</span></span>
                <span>{rest.to_string()}</span>
            </h1>
        }
        .into_any(),
        None => view! { <h1>{title}</h1> }.into_any(),
    }
}

/// What the principal `id` of the session holds: its sections, headed by its name, role and
/// project only when the session has `several` principals, and what is said of it; nothing at all
/// for the session's one principal while it holds nothing.
#[component]
fn Principal(
    doc: Signal<Option<Document>>,
    id: EntityId,
    several: Signal<bool>,
    first: Signal<Option<EntityId>>,
) -> impl IntoView {
    let head = Memo::new(move |_| {
        doc.with(|d| {
            let g = group(d, id)?;
            Some((g.title.clone(), g.tags.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" · ")))
        })
    });
    let lines = Memo::new(move |_| doc.with(|d| group(d, id).map(|g| g.lines.clone()).unwrap_or_default()));
    let kinds = Memo::new(move |_| {
        doc.with(|d| group(d, id).map(|g| g.sections.iter().map(|s| s.section).collect::<Vec<_>>()).unwrap_or_default())
    });
    let shows = Memo::new(move |_| several.get() || !lines.with(Vec::is_empty) || !kinds.with(Vec::is_empty));
    move || {
        shows.get().then(|| {
            view! {
                <section class="principal">
                    {move || {
                        several.get().then(|| head.get()).flatten().map(|(name, tags)| {
                            view! {
                                <header class="principal-header">
                                    <h2>{name}</h2>
                                    <span class="principal-tags">{tags}</span>
                                </header>
                            }
                        })
                    }}
                    <Lines class="principal-lines" lines=lines.into()/>
                    <For each=move || kinds.get() key=|kind| *kind children=move |kind| {
                        view! { <Section doc=doc at=SectionAt::of(id, kind) first=first/> }
                    }/>
                </section>
            }
        })
    }
}

/// The kinds of the session's own sections, in the document's order.
fn sections_of(document: &Document) -> Vec<ListKind> {
    document.sections.iter().map(|s| s.section).collect()
}

/// The ids of the session's principals, in the document's order.
fn principals_of(document: &Document) -> Vec<EntityId> {
    document.principals.iter().map(|g| g.id).collect()
}

/// The principal `id`'s group in `document`, borrowed, so that each part of the page clones only
/// what it shows of it.
fn group(document: &Option<Document>, id: EntityId) -> Option<&Group> {
    document.as_ref()?.principals.iter().find(|g| g.id == id)
}

/// The question that waits on the person first, which the page opens ready to answer at its first
/// draw: the first record of the first section of what waits on them.
fn first_waiting(document: &Document) -> Option<EntityId> {
    let sections = document.principals.iter().flat_map(|g| &g.sections);
    sections.filter(|s| s.section == ListKind::WaitsOnYou).find_map(|s| s.records.first()).map(|r| r.id)
}

/// Keeps the ages the page shows current, reading the clock again once a minute, for as long as the
/// page: how long ago something happened is no record's to say.
fn keep_ages() {
    let minute = ArcTrigger::new();
    provide_context(Minute(minute.clone()));
    if let Ok(every) = set_interval_with_handle(move || minute.notify(), Duration::from_secs(60)) {
        on_cleanup(move || every.clear());
    }
}

/// Puts `title` on the browser's tab.
fn set_tab_title(title: &str) {
    if let Some(document) = web_sys::window().and_then(|w| w.document()) {
        document.set_title(title);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fwd_core::session_view::{Group, List, Prominence, Row, RowKind};

    fn id(n: u8) -> EntityId {
        EntityId::from_bytes([n; 32])
    }

    fn section(kind: ListKind, rows: &[u8]) -> List {
        List {
            section: kind,
            prominence: Prominence::Secondary,
            title: String::new(),
            lines: Vec::new(),
            shown: rows.len(),
            of: rows.len(),
            more: None,
            records: rows
                .iter()
                .map(|n| Row {
                    id: id(*n),
                    kind: RowKind::Ask,
                    title: String::new(),
                    title_is_cut: false,
                    tags: Vec::new(),
                    lines: Vec::new(),
                    attribution: None,
                    path: None,
                    actions: Vec::new(),
                })
                .collect(),
        }
    }

    fn principal(n: u8, sections: Vec<List>) -> Group {
        Group { id: id(n), title: String::new(), tags: Vec::new(), lines: Vec::new(), sections }
    }

    /// What waits on the person first is the first question of the first principal that has any,
    /// whatever else stands before it; with none waiting, nothing opens.
    #[test]
    fn the_question_opened_at_the_first_draw_is_the_first_that_waits_on_the_person() {
        let quiet = principal(20, vec![section(ListKind::Holds, &[1])]);
        let asking =
            principal(21, vec![section(ListKind::WaitsOnYou, &[2, 3]), section(ListKind::WaitsOnOthers, &[4])]);
        let document = |principals| Document {
            title: TITLE.into(),
            page: None,
            companion: None,
            lines: Vec::new(),
            sections: Vec::new(),
            principals,
        };
        assert_eq!(first_waiting(&document(vec![quiet.clone(), asking])), Some(id(2)));
        assert_eq!(first_waiting(&document(vec![quiet])), None);
    }
}
