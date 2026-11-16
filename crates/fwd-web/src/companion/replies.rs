//! The replies the person sent from the companion view that the daemon has not confirmed, kept by
//! the question each answers from the moment it is sent until the daemon confirms it or the person
//! dismisses it, whatever becomes of the question's row. A reply the connection's drop left
//! unconfirmed may have gone to the page's own store alone, and Ankurah's fetch returns what that
//! store matches as well as what the daemon holds, so the page's next read takes the reply for the
//! person's answer and drops the question's row, with its box. The reply then shows under the
//! page's title, with its words and why the daemon has not confirmed it, and the link to the
//! question's page, where a page opened afresh shows the thread as the daemon holds it, so that the
//! person looks, then sends it again or dismisses it.

use super::answer::send;
use super::record::{answer_of, PageLink};
use super::Connected;
use ankurah::EntityId;
use fwd_core::session_view::{Document, Row};
use leptos::prelude::*;
use std::collections::BTreeMap;

/// The replies sent from this page that the daemon has not confirmed, by the question each answers.
#[derive(Debug, Default)]
pub struct Replies(BTreeMap<EntityId, Reply>);

/// A reply sent from this page that the daemon has not confirmed.
#[derive(Debug)]
pub struct Reply {
    /// The title of the question it answers, and the link to the question's page, as the
    /// question's row showed them when the reply was sent, which the reply shows by once the row is
    /// gone.
    pub title: String,
    pub link: Option<String>,
    pub words: String,
    /// None while it is being sent; then the error it ended with, which says why the daemon has not
    /// confirmed it.
    pub ended: Option<anyhow::Error>,
}

impl Replies {
    /// `words` are sent as the reply to the question `question`, titled `title`, whose page is at
    /// `link`: whether they are, which they are unless a reply to it is being sent already. They
    /// take the place of a reply to it that ended unconfirmed.
    pub fn start(&mut self, question: EntityId, title: String, link: Option<String>, words: String) -> bool {
        if self.0.get(&question).is_some_and(Reply::sending) {
            return false;
        }
        self.0.insert(question, Reply { title, link, words, ended: None });
        true
    }

    /// The reply to `question` ended with `outcome`: one the daemon confirmed is done with, and
    /// any other is kept, with why, until the person dismisses it.
    pub fn end(&mut self, question: EntityId, outcome: anyhow::Result<()>) {
        match outcome {
            Ok(()) => {
                self.0.remove(&question);
            }
            Err(why) => {
                if let Some(reply) = self.0.get_mut(&question) {
                    reply.ended = Some(why);
                }
            }
        }
    }

    /// The person is done with the reply to `question`.
    pub fn dismiss(&mut self, question: EntityId) {
        self.0.remove(&question);
    }

    /// The reply to `question`, while one is kept.
    pub fn get(&self, question: EntityId) -> Option<&Reply> {
        self.0.get(&question)
    }

    /// The questions of the replies kept here that `document` draws no Answer for, so that the page
    /// shows those replies apart from its rows.
    pub fn apart(&self, document: Option<&Document>) -> Vec<EntityId> {
        let drawn =
            |question: EntityId| document.is_some_and(|d| rows(d).any(|r| r.id == question && answer_of(r).is_some()));
        self.0.keys().copied().filter(|question| !drawn(*question)).collect()
    }
}

impl Reply {
    /// Whether it is being sent.
    pub fn sending(&self) -> bool {
        self.ended.is_none()
    }
}

/// Every row of `document`, the session's own sections' and then each principal's.
fn rows(document: &Document) -> impl Iterator<Item = &Row> {
    let sections = document.sections.iter().chain(document.principals.iter().flat_map(|g| &g.sections));
    sections.flat_map(|s| &s.records)
}

/// The replies kept here whose questions the page no longer draws, under its title, so that the
/// person sees each, with its words, and decides what becomes of it.
#[component]
pub fn RepliesApart(doc: Signal<Option<Document>>) -> impl IntoView {
    let replies = expect_context::<RwSignal<Replies>>();
    let apart = Memo::new(move |_| doc.with(|d| replies.with(|r| r.apart(d.as_ref()))));
    move || {
        (!apart.with(Vec::is_empty)).then(|| {
            view! {
                <section class="replies-apart" role="status">
                    <For each=move || apart.get() key=|question| *question children=move |question| {
                        view! { <ReplyApart question=question/> }
                    }/>
                </section>
            }
        })
    }
}

/// The reply to `question`, apart from the rows: the question's title, that the reply is being
/// sent or why the daemon has not confirmed it, its words, and the person's two choices, to send it
/// again, once they have looked at the question's thread, or to dismiss it.
#[component]
fn ReplyApart(question: EntityId) -> impl IntoView {
    let replies = expect_context::<RwSignal<Replies>>();
    let Connected(connected) = expect_context::<Connected>();
    // Nothing once the reply is gone, confirmed or dismissed.
    let shown = Memo::new(move |_| {
        replies.with(|r| {
            let reply = r.get(question)?;
            let why = reply.ended.as_ref().map(|why| format!("{why:#}"));
            Some((reply.title.clone(), reply.link.clone(), reply.words.clone(), why))
        })
    });
    move || {
        shown.get().map(|(title, link, words, why)| {
            let sending = why.is_none();
            let again = {
                let (title, link, words) = (title.clone(), link.clone(), words.clone());
                move |_| send(replies, question, title.clone(), link.clone(), words.clone(), || {})
            };
            view! {
                <article class="reply-apart">
                    <p>"Your answer to " <q>{title}</q></p>
                    {match why {
                        Some(why) => view! { <p class="error">{why}</p> }.into_any(),
                        None => view! { <p class="muted">"being sent…"</p> }.into_any(),
                    }}
                    <p class="reply-apart-words">{words}</p>
                    <div class="record-footer reply-apart-footer">
                        <span class="reply-apart-actions">
                            <button type="button" class="btn primary" disabled=move || sending || !connected.get() on:click=again>
                                "Send again"
                            </button>
                            <button type="button" class="btn subtle" disabled=sending
                                on:click=move |_| replies.update(|r| r.dismiss(question))>
                                "Dismiss"
                            </button>
                        </span>
                        <PageLink link=Signal::stored(link)/>
                    </div>
                </article>
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use fwd_core::session_view::{Action, Group, Input, List, ListKind, Prominence, RowKind, TITLE};

    fn id(n: u8) -> EntityId {
        EntityId::from_bytes([n; 32])
    }

    /// A document whose one principal waits on the person for the questions `asking`, each with the
    /// document's Answer.
    fn waiting_on(asking: &[u8]) -> Document {
        let answer = Action {
            label: "Answer".into(),
            args: Vec::new(),
            input: Some(Input { prompt: "Your answer".into(), multiline: true }),
        };
        let row = |n: &u8| Row {
            id: id(*n),
            kind: RowKind::Ask,
            title: String::new(),
            title_is_cut: false,
            tags: Vec::new(),
            lines: Vec::new(),
            attribution: None,
            path: None,
            actions: vec![answer.clone()],
        };
        let waits = List {
            section: ListKind::WaitsOnYou,
            prominence: Prominence::Secondary,
            title: String::new(),
            lines: Vec::new(),
            shown: asking.len(),
            of: asking.len(),
            more: None,
            records: asking.iter().map(row).collect(),
        };
        let principal =
            Group { id: id(9), title: String::new(), tags: Vec::new(), lines: Vec::new(), sections: vec![waits] };
        Document {
            title: TITLE.into(),
            page: None,
            companion: None,
            lines: Vec::new(),
            sections: Vec::new(),
            principals: vec![principal],
        }
    }

    const QUESTION: &str = "Which shelf takes the atlas?";

    /// A reply the daemon has not confirmed is kept, with its words and why, after its question
    /// leaves the document, as the page's next read makes it leave when it takes the page's own copy
    /// of the reply for the person's answer, and shows apart from the rows from then on, until the
    /// person dismisses it; while its question's row is drawn, the row shows it instead. A reply
    /// being sent is not sent twice, and one the daemon confirmed is done with at once.
    #[test]
    fn a_reply_the_daemon_has_not_confirmed_outlives_its_questions_row_until_the_person_dismisses_it() {
        let (question, other) = (id(1), id(2));
        let mut replies = Replies::default();
        assert!(replies.start(question, QUESTION.into(), None, "The bottom one.".into()));
        assert!(!replies.start(question, QUESTION.into(), None, "The bottom one.".into()), "sent twice");
        assert_eq!(replies.apart(Some(&waiting_on(&[1, 2]))), [], "shown apart while its row is drawn");
        replies.end(question, Err(anyhow!("it may or may not have reached the daemon")));
        let answered = waiting_on(&[2]);
        assert_eq!(replies.apart(Some(&answered)), [question], "gone with its question's row");
        let kept = replies.get(question).expect("the reply is kept");
        assert_eq!(kept.words, "The bottom one.");
        assert_eq!(
            kept.ended.as_ref().map(ToString::to_string).as_deref(),
            Some("it may or may not have reached the daemon")
        );
        replies.dismiss(question);
        assert!(replies.get(question).is_none() && replies.apart(Some(&answered)).is_empty(), "kept once dismissed");

        assert!(replies.start(other, QUESTION.into(), None, "Yes.".into()));
        replies.end(other, Ok(()));
        assert!(replies.get(other).is_none(), "a reply the daemon confirmed is kept");
    }
}
