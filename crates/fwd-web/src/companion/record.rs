//! One record of the companion view: its title, which opens it, with its tags at the right of the
//! title's last line, smaller; opened, its lines, an intention's attribution at the right, smaller
//! and quieter (Daniel, D227: "the Name and the date of any citations should be to the right side,
//! smaller, subtler. Attribution is secondary to the context, not primary"), a question's Answer
//! where the document offers one (`answer.rs`), and a link to its page in the app, in a new tab,
//! where it has one. A title cut from the text the first line holds gives way to that line when
//! the record opens, so that nothing is said twice (Daniel: "we need to be extremely efficient
//! with layout, ergonomics, and non-repetition"), and no tag says again what the heading above it
//! says ([`shown_tags`]). A record with no line and no action does not open, and its title leads
//! to its page. No id shows: the record is reached through its page.

use super::answer::Answer;
use super::markdown;
use super::section::SectionAt;
use super::Minute;
use crate::fmt;
use ankurah::EntityId;
use fwd_core::model::TaskStatus;
use fwd_core::session_view::{status_words, Byline, Document, ListKind, Row, RowKind, Tag, Tone};
use leptos::prelude::*;
use wasm_bindgen::JsCast;

/// What a link to a record's page says: the word the app's lozenge preview has for that link. An
/// arrow after it says the page opens in a new tab.
const OPEN: &str = "open";

/// The record `id` of the section `at` of the document `doc` shows, open from the start when
/// `opened`.
#[component]
pub fn Record(doc: Signal<Option<Document>>, at: SectionAt, id: EntityId, opened: bool) -> impl IntoView {
    let row = Memo::new(move |_| doc.with(|d| d.as_ref().and_then(|d| at.row(d, id)).cloned()));
    let page = Memo::new(move |_| doc.with(|d| d.as_ref().and_then(|d| d.page.clone())));
    let opens =
        Memo::new(move |_| row.with(|r| r.as_ref().is_some_and(|r| !r.lines.is_empty() || !r.actions.is_empty())));
    let title = Memo::new(move |_| row.with(|r| r.as_ref().map(|r| r.title.clone()).unwrap_or_default()));
    let tags = Memo::new(move |_| row.with(|r| r.as_ref().map(|r| shown_tags(at.kind, r)).unwrap_or_default()));
    let link = Memo::new(move |_| {
        let path = row.with(|r| r.as_ref().and_then(|r| r.path.clone()))?;
        page.get().map(|page| format!("{page}{path}"))
    });
    move || {
        if !opens.get() {
            // Nothing to open: its title leads to its page, where it has one.
            let title = move || match link.get() {
                Some(href) => view! {
                    <a class="record-title" href=href target="_blank" rel="noopener noreferrer">{title}</a>
                }
                .into_any(),
                None => view! { <span class="record-title">{title}</span> }.into_any(),
            };
            return view! {
                <li class="record">
                    <div class="record-head">
                        // The caret's room, so that the title stands where the others' do.
                        <span aria-hidden="true"></span>
                        {title}
                        <Tags tags=tags.into()/>
                    </div>
                </li>
            }
            .into_any();
        }
        let cut = Memo::new(move |_| row.with(|r| r.as_ref().is_some_and(|r| r.title_is_cut)));
        // The first line takes the title's place once the record opens, where the title is cut from it.
        let full = Memo::new(move |_| {
            row.with(|r| r.as_ref().filter(|r| r.title_is_cut).and_then(|r| r.lines.first()).map(|l| markdown::html(l)))
        });
        let lines = Memo::new(move |_| {
            row.with(|r| {
                r.as_ref()
                    .map(|r| {
                        r.lines.iter().skip(usize::from(r.title_is_cut)).map(|l| markdown::html(l)).collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
        });
        let byline = Memo::new(move |_| row.with(|r| r.as_ref().and_then(|r| r.attribution.clone())));
        // The document's Answer, on a question it offers to answer: the action that asks for words.
        let answer = Memo::new(move |_| {
            row.with(|r| {
                let asking = r.as_ref().filter(|r| r.kind == RowKind::Ask)?;
                asking.actions.iter().find_map(|a| Some((a.label.clone(), a.input.clone()?)))
            })
        });
        view! {
            <li class="record">
                <details open=opened>
                    <summary class="record-head" on:click=keep_selection>
                        <span class="chevron" aria-hidden="true"></span>
                        <span class="record-title">
                            <span class="closed-title" class:cut=cut>{title}</span>
                            {move || full.get().map(|html| view! { <span class="open-title md" inner_html=html></span> })}
                        </span>
                        <Tags tags=tags.into()/>
                    </summary>
                    <div class="record-body">
                        <div class="record-words" class:attributed=move || byline.with(Option::is_some)>
                            <div class="record-lines">
                                {move || lines.get().into_iter().map(|html| view! { <div class="md" inner_html=html></div> }).collect::<Vec<_>>()}
                            </div>
                            {move || byline.get().map(|byline| view! { <Attribution byline=byline/> })}
                        </div>
                        {move || match answer.get() {
                            Some((label, input)) => view! {
                                <Answer question=id label=label input=input writing=opened link=link.into()/>
                            }
                            .into_any(),
                            None => view! { <div class="record-footer"><PageLink link=link.into()/></div> }.into_any(),
                        }}
                    </div>
                </details>
            </li>
        }
        .into_any()
    }
}

/// A row's tags, smaller, at the right of its title's last line: each one's words and, for one that
/// dates something, how long ago, by this page's clock, the time itself on hover.
#[component]
fn Tags(tags: Signal<Vec<Shown>>) -> impl IntoView {
    view! {
        <span class="row-tags">
            {move || {
                tags.get()
                    .into_iter()
                    .enumerate()
                    .map(|(n, tag)| {
                        let at = tag.at.as_deref().map(instant_ms);
                        view! {
                            {(n > 0).then_some(" · ")}
                            {tag.text.clone()}
                            {(tag.text.is_some() && at.is_some()).then_some(" · ")}
                            {at.flatten().map(|ms| view! { <Age ms=ms/> })}
                        }
                    })
                    .collect::<Vec<_>>()
            }}
        </span>
    }
}

/// How long ago `ms` was, kept current by the page's minute, with the time itself on hover.
#[component]
fn Age(ms: i64) -> impl IntoView {
    let Minute(minute) = expect_context::<Minute>();
    view! {
        <time title=fmt::clock(ms)>
            {move || {
                minute.track();
                fmt::ago(ms)
            }}
        </time>
    }
}

/// Whose words an intention holds and when they were last written down, at the right, smaller.
#[component]
fn Attribution(byline: Byline) -> impl IntoView {
    view! {
        <p class="attribution">
            <span>{byline.by}</span>
            <span class="attribution-at">{byline.at}</span>
        </p>
    }
}

/// The link to a record's page, in a new tab, so that the view stays beside the chat.
#[component]
pub(super) fn PageLink(link: Signal<Option<String>>) -> impl IntoView {
    move || {
        link.get().map(|href| {
            view! {
                <a class="page-link" href=href target="_blank" rel="noopener noreferrer">
                    {OPEN}
                    <span aria-hidden="true">" ↗"</span>
                </a>
            }
        })
    }
}

/// A selection made in a record's title, to copy its words, neither opens nor closes the record.
fn keep_selection(ev: web_sys::MouseEvent) {
    if ev.detail() == 0 {
        // Enter or Space, which open and close it.
        return;
    }
    let Some(selection) = web_sys::window().and_then(|w| w.get_selection().ok().flatten()) else { return };
    let Some(summary) = ev.current_target().and_then(|t| t.dyn_into::<web_sys::Node>().ok()) else { return };
    if !selection.is_collapsed()
        && selection.contains_node_with_allow_partial_containment(&summary, true).unwrap_or(false)
    {
        ev.prevent_default();
    }
}

/// The instant a tag dates, in milliseconds since the epoch, as the document writes it in UTC
/// (`2026-11-13T21:52:49Z`); none for one the browser cannot read.
fn instant_ms(at: &str) -> Option<i64> {
    let ms = js_sys::Date::parse(at);
    ms.is_finite().then_some(ms as i64)
}

/// What the page shows of a tag: its words, unless the heading says them, and when it dates.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Shown {
    text: Option<String>,
    at: Option<String>,
}

/// The tags `row` shows in the section `kind`, less what the section's heading says already: under
/// Waiting On You, that a question waits on the person, whose time stays; under Working On, a task's
/// usual state there, in progress.
fn shown_tags(kind: ListKind, row: &Row) -> Vec<Shown> {
    let usual = (kind == ListKind::Holds && row.kind == RowKind::Task).then(|| status_words(TaskStatus::InProgress));
    row.tags
        .iter()
        .filter(|tag| usual.as_ref() != Some(&tag.text) || tag.at.is_some())
        .map(|tag: &Tag| {
            let waits_on_you = kind == ListKind::WaitsOnYou && tag.tone == Some(Tone::Attention);
            Shown { text: (!waits_on_you).then(|| tag.text.clone()), at: tag.at.clone() }
        })
        .filter(|shown| shown.text.is_some() || shown.at.is_some())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(text: &str, tone: Option<Tone>, at: Option<&str>) -> Tag {
        Tag { text: text.into(), tone, at: at.map(str::to_string) }
    }

    fn row(kind: RowKind, tags: Vec<Tag>) -> Row {
        Row {
            id: EntityId::from_bytes([1; 32]),
            kind,
            title: "A title".into(),
            title_is_cut: false,
            tags,
            lines: Vec::new(),
            attribution: None,
            path: None,
            actions: Vec::new(),
        }
    }

    fn shown(text: Option<&str>, at: Option<&str>) -> Shown {
        Shown { text: text.map(str::to_string), at: at.map(str::to_string) }
    }

    const AT: &str = "2026-11-13T21:52:49Z";

    /// A question waiting on the person keeps its time and loses "asked", which its heading says; a
    /// task in progress under Working On says nothing of its state, and one in another state says it;
    /// every other tag stands as it is.
    #[test]
    fn no_tag_says_again_what_its_heading_says() {
        let asked = row(RowKind::Ask, vec![tag("asked", Some(Tone::Attention), Some(AT)), tag("in beta", None, None)]);
        assert_eq!(shown_tags(ListKind::WaitsOnYou, &asked), [shown(None, Some(AT)), shown(Some("in beta"), None)]);
        let task = |state| row(RowKind::Task, vec![tag(state, None, None), tag("in beta", None, None)]);
        assert_eq!(shown_tags(ListKind::Holds, &task("in progress")), [shown(Some("in beta"), None)]);
        assert_eq!(
            shown_tags(ListKind::Holds, &task("in review")),
            [shown(Some("in review"), None), shown(Some("in beta"), None)]
        );
        let from = row(RowKind::Ask, vec![tag("from lead-sam", None, Some(AT))]);
        assert_eq!(shown_tags(ListKind::Holds, &from), [shown(Some("from lead-sam"), Some(AT))]);
        let to = row(RowKind::Ask, vec![tag("to no one in particular", None, Some(AT))]);
        assert_eq!(shown_tags(ListKind::WaitsOnOthers, &to), [shown(Some("to no one in particular"), Some(AT))]);
    }
}
