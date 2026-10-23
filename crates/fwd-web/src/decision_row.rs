//! A decision as a row: its number (`D12`) and short id (a click copies the whole id), its title
//! linked to its card on the Decisions page, its status and area, the scope it governs as that
//! scope's lozenge or "project-wide" (docs/DECISIONS.md, D93), who ruled on it and when, the
//! lozenges of what it is joined to (what cites it, what it cites and where it was decided, what
//! supersedes it: `lozenges.rs`), and one line of the ruling. On a card's own row that line opens
//! onto the whole record, where why it closed is the note of the comment its closing move wrote,
//! its references drawn as lozenges ([`why_it_closed`]), and beneath its line the row offers, as
//! the page's buttons, the moves its status allows (accept or reject a proposal; supersede or
//! repeal a ruling in force), each asking for the reason it keeps as a comment on the decision.
//! Beneath a decision hang its comments.

use crate::fmt::CopyId;
use crate::lozenges::{self, Target};
use crate::referents::{Child, Listing, Mode, Models};
use crate::route::{self, Route};
use crate::session::{self, ctx};
use crate::thread::{self, Actions, Editor, Link, Model, Query, RowAt, Sending, ThreadState};
use crate::{empty, fmt, keys, modes, references, store};
use ankurah::EntityId;
use fwd_core::decision_log;
use fwd_core::model::{DecisionStatus, DecisionView, Priority};
use fwd_core::ops::{self, DecisionComment, MessageContext};
use fwd_core::rules::DecisionMove;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

/// The links a decision follows to the messages beneath it: its comments.
pub const FOLLOWS: &[Link] = &[Link::DecisionComments];

/// The live query behind a decision's comments.
pub fn open(parent: EntityId, link: Link) -> Option<Query> {
    (link == Link::DecisionComments).then(|| thread::messages_where("decision = ?", parent))
}

/// The messages beneath the decision `parent` through `link`: its comments, each one that answers
/// no other message (a reply belongs under what it answers).
pub fn listing(parent: EntityId, link: Link, query: Option<&Query>) -> Listing<EntityId, Model, Link> {
    match link {
        Link::DecisionComments => thread::from_messages(query, |m| {
            let on_decision = m.decision().ok().flatten().map(EntityId::from) == Some(parent);
            let answers_another = m.reply_to().ok().flatten().is_some();
            (on_decision && !answers_another).then(|| Child {
                id: m.id(),
                models: Models::one(Model::Message),
                link,
                label: String::new(),
                created: m.created().unwrap_or(0),
            })
        }),
        other => Listing::Failed(format!("a decision does not follow {other:?}")),
    }
}

/// `D12` and the decision's title, to name it where it shows again.
pub fn name(id: EntityId) -> Option<String> {
    let d = store::decision(id)?;
    Some(format!("D{} {}", d.number().unwrap_or(0), d.title().unwrap_or_default()))
}

/// The lozenge mode: `D12` and its title, a fallen ruling in the warning colour.
pub fn lozenge(id: EntityId) -> AnyView {
    lozenges::single(Target::Decision(id))
}

/// The list item mode: `D12`, its title (leading to its panel) and its status, as the Decisions
/// page's line begins.
pub fn list_item(id: EntityId) -> AnyView {
    (move || {
        let Some(d) = store::decision(id) else {
            let absent = format!("{}: {}", fmt::short(id), empty::absent(Some(Model::Decision)));
            return view! { <span class="list-item muted">{absent}</span> }.into_any();
        };
        view! {
            <span class="list-item decision-item">
                <span class="decision-number">{format!("D{}", d.number().unwrap_or(0))}</span>
                " "
                <a href=link_to(id)>{d.title().unwrap_or_default()}</a>
                " "
                {d.status().ok().map(|s| view! { <span class=format!("pill decision-{s}")>{s.to_string()}</span> })}
            </span>
        }
        .into_any()
    })
    .into_any()
}

/// The embedded mode: the decision's row, its comments beneath it.
pub fn embedded(state: ThreadState, at: RowAt) -> AnyView {
    view! { <DecisionRow state=state at=at/> }.into_any()
}

/// So that the references in a closed decision's note read as the lozenges of what they name, not
/// as the ids or handles typed (task b6bAAF57, "Show a decision's closing note as its comment, with
/// names drawn, not the raw text"), what its record says under "Why it closed": the note the move
/// that closed a decision in `status` gave in its comment, found among `comments`, the decision's
/// comments (`ops::closing_note`), which keeps each reference as the move resolved it; else, for a
/// decision closed before its moves wrote comments, `closed_reason`, the note as it was typed.
/// None when neither says anything, as for a decision still open.
pub fn why_it_closed(
    status: Option<DecisionStatus>,
    closed_reason: Option<String>,
    comments: &[DecisionComment],
) -> Option<String> {
    let from_comment = status.and_then(|status| ops::closing_note(status, comments)).map(str::to_string);
    from_comment.or(closed_reason).filter(|note| !note.trim().is_empty())
}

/// The "Why it closed" section of the record of the decision `id`: [`why_it_closed`], its comments
/// being the page's messages on it, drawn as a message's text is (`references::content`); nothing
/// while there is nothing to say. A memo holds the note, so the section redraws when the note
/// changes, not whenever any message of the project does.
pub fn closing_section(id: EntityId) -> impl IntoView {
    let note = Memo::new(move |_| {
        let d = store::decision(id)?;
        let comments: Vec<DecisionComment> = store::messages()
            .iter()
            .filter(|m| m.decision().ok().flatten().map(EntityId::from) == Some(id))
            .filter_map(|m| DecisionComment::of(m).ok())
            .collect();
        why_it_closed(d.status().ok(), d.closed_reason().ok().flatten(), &comments)
    });
    move || {
        note.get().map(|note| {
            view! { <section class="prose"><h4>"Why it closed"</h4><p>{references::content(&note)}</p></section> }
        })
    }
}

/// Who ruled on a decision and when, or who proposed it: "decided by dnorman, 2026-09-24".
pub fn ruled(d: &DecisionView) -> String {
    let author = d.author().ok().map(EntityId::from);
    let decider = d.decided_by().ok().flatten().map(EntityId::from);
    let name = |p: Option<EntityId>| p.map(store::handle_of).unwrap_or_default();
    let day = d.decided().ok().flatten().or(d.created().ok()).map(decision_log::date).unwrap_or_default();
    match (d.status().ok(), decider) {
        (Some(DecisionStatus::Proposed), _) | (_, None) => format!("proposed by {}, {day}", name(author)),
        (Some(DecisionStatus::Rejected), _) => format!("rejected by {}, {day}", name(decider)),
        _ if author != decider => format!("decided by {}, {day}, recorded by {}", name(decider), name(author)),
        _ => format!("decided by {}, {day}", name(decider)),
    }
}

/// A move a decision's own row offers, and whether its button is the page's primary one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offer {
    pub mv: DecisionMove,
    pub primary: bool,
}

impl Offer {
    /// The classes of the move's button: the page's own buttons, as a task's panel draws Approve
    /// and Reject, so a move reads as one. Drawn as the row's small grey words (the "comment"
    /// beside them), the moves read as part of the row's line on a decision's panel, and Daniel
    /// found nothing there to accept a proposal with (2026-09-28).
    pub fn class(self) -> &'static str {
        if self.primary {
            "btn primary"
        } else {
            "btn"
        }
    }
}

/// The moves the row of a decision in `status` offers, in the order its buttons show: Accept (the
/// primary one) and Reject on a proposal, Supersede and Repeal on a ruling in force, and none on a
/// decision that has closed or whose status cannot be read.
pub fn offers(status: Option<DecisionStatus>) -> Vec<Offer> {
    [DecisionMove::Accept, DecisionMove::Reject, DecisionMove::Supersede, DecisionMove::Repeal]
        .into_iter()
        .filter(|mv| Some(mv.moves_from()) == status)
        .map(|mv| Offer { mv, primary: mv == DecisionMove::Accept })
        .collect()
}

#[component]
pub fn DecisionRow(state: ThreadState, at: RowAt) -> impl IntoView {
    let id = at.id;
    let card = at.is_card_row();
    // Changes only when the decision arrives or goes; each field below redraws on its own.
    let decision = Memo::new(move |_| store::decision(id));
    let number = move || decision.get().and_then(|d| d.number().ok()).map(|n| format!("D{n}")).unwrap_or_default();
    let title =
        move || decision.get().and_then(|d| d.title().ok()).unwrap_or_else(|| empty::absent(Some(Model::Decision)));
    let status = move || decision.get().and_then(|d| d.status().ok());
    let area = move || decision.get().and_then(|d| d.area().ok().flatten());
    // The scope it governs, unless the panel the row heads shows it above with its picker.
    let governs = (!at.joined_drawn_above()).then_some(move || match decision.get() {
        None => ().into_any(),
        Some(d) => match ops::governs(&d).ok().flatten() {
            Some(scope) => modes::draw(Mode::Embedded, Mode::Lozenge, scope, &Models::one(Model::Scope)),
            None => view! { <span class="governs muted">"project-wide"</span> }.into_any(),
        },
    });
    let words = move || decision.get().map(|d| ops::text(d.decision()).unwrap_or_default()).unwrap_or_default();
    let prose = move |field: fn(&DecisionView) -> String| decision.get().map(|d| field(&d)).unwrap_or_default();
    let (edit, report, me) = (state.edit(&at), state.report(at.appearance), state.me);
    // A move made from a card's own row deals with what the card is about, as a reply does.
    let dealt_with = at.card_sent().filter(|_| card);
    let strip = at.strip();

    // A move waiting for its reason (and, for a supersession, for the ruling that replaces it).
    // Both belong to one opening of the form: opening, cancelling or applying a move starts the
    // next from nothing, so the form never keeps a choice it no longer shows.
    let pending = RwSignal::new(None::<DecisionMove>);
    let note = RwSignal::new(String::new());
    let successor = RwSignal::new(None::<EntityId>);
    let choose = move |mv: Option<DecisionMove>| {
        pending.try_set(mv);
        note.try_set(String::new());
        successor.try_set(None);
    };
    let apply = move || {
        let Some(mv) = pending.get_untracked() else { return };
        let why = note.get_untracked();
        if why.trim().is_empty() {
            report.run("say why: the reason is kept as a comment on the decision".into());
            return;
        }
        let new = successor.get_untracked();
        spawn_local(async move {
            let done = match (mv, new) {
                (DecisionMove::Supersede, Some(new)) => ops::supersede_decision(&ctx(), me, id, new, &why).await,
                (DecisionMove::Supersede, None) => Err(anyhow::anyhow!("choose the ruling that replaces it")),
                _ => ops::move_decision(&ctx(), me, id, mv, &why).await.map(|_| ()),
            };
            match done {
                Ok(()) => {
                    session::wrote(me);
                    choose(None);
                    if let Some(dealt_with) = dealt_with {
                        dealt_with.try_run(());
                    }
                }
                Err(e) => {
                    report.try_run(e.to_string());
                }
            }
        });
    };
    let moves = move || {
        let offered = offers(status());
        (!offered.is_empty()).then(|| {
            let buttons = offered
                .into_iter()
                .map(|o| view! { <button class=o.class() on:click=move |_| choose(Some(o.mv))>{move_label(o.mv)}</button> })
                .collect::<Vec<_>>();
            view! { <div class="actions decision-moves">{buttons}</div> }
        })
    };
    let form = move || {
        let mv = pending.get()?;
        let others = move || {
            store::decisions()
                .into_iter()
                .filter(|d| d.id() != id && d.status().ok() == Some(DecisionStatus::Accepted))
                .map(|d| {
                    let label = format!("D{} {}", d.number().unwrap_or(0), d.title().unwrap_or_default());
                    let chosen = successor.get_untracked() == Some(d.id());
                    view! { <option value=d.id().to_base64() selected=chosen>{label}</option> }
                })
                .collect::<Vec<_>>()
        };
        // The select shows the successor the move will use, and nothing else.
        let pick = (mv == DecisionMove::Supersede).then(|| {
            view! {
                <select prop:value=move || successor.get().map(|d| d.to_base64()).unwrap_or_default()
                    on:change=move |ev| successor.set(event_target_value(&ev).parse().ok())>
                    <option value="" selected=move || successor.get().is_none()>"replaced by…"</option>
                    {others}
                </select>
            }
        });
        Some(view! {
            <div class="note-box">
                {pick}
                <input type="text" placeholder=format!("why it is {} (kept as a comment)", mv.participle())
                    prop:value=move || note.get()
                    on:input=move |ev| note.set(event_target_value(&ev))
                    on:keydown=move |ev| {
                        if keys::is_send(&ev) {
                            ev.prevent_default();
                            apply();
                        } else if ev.key() == "Escape" {
                            choose(None);
                        }
                    }/>
                <button class="btn primary" on:click=move |_| apply()>{move_label(mv)}</button>
                <button class="btn subtle" on:click=move |_| choose(None)>"Cancel"</button>
            </div>
        })
    };
    let full = move || {
        let parts = [
            ("Context", prose(|d| ops::text(d.context()).unwrap_or_default())),
            ("Ruling", prose(|d| ops::text(d.decision()).unwrap_or_default())),
            ("Consequences", prose(|d| ops::text(d.consequences()).unwrap_or_default())),
        ];
        parts
            .into_iter()
            .filter(|(_, text)| !text.trim().is_empty())
            .map(|(label, text)| view! { <section class="prose"><h4>{label}</h4><p>{text}</p></section> })
            .collect::<Vec<_>>()
    };
    let line = view! {
        <div class="decision-ref">
            <span class="decision-number">{number}</span>
            " "
            <CopyId id=id/>
            " "
            <a href=link_to(id)>{title}</a>
            " "
            {move || status().map(|s| view! { <span class=format!("pill decision-{s}")>{s.to_string()}</span> })}
            {move || area().map(|a| view! { <span class="pill area">{a}</span> })}
            {governs}
            <span class="meta">{move || decision.get().map(|d| ruled(&d))}</span>
            " "
            {strip}
            {(!card).then(|| view! { <p class="content" title=words>{move || fmt::truncate(&words(), 160)}</p> })}
            // A card lists one line of the ruling too, and opens onto the whole record.
            {card.then(|| {
                view! {
                    <details class="decision-full">
                        <summary class="content">{move || fmt::truncate(&words(), 160)}</summary>
                        {full}
                        {closing_section(id)}
                    </details>
                }
            })}
            <span class="row-actions">
                <button class="btn subtle tiny" on:click=move |_| edit.run(())>"comment"</button>
            </span>
            {card.then_some(moves)}
            {card.then_some(form)}
        </div>
    }
    .into_any();
    thread::frame(state, at, line, Actions { editor: Some(COMMENT), activate: Some(open_decision) })
}

fn move_label(mv: DecisionMove) -> &'static str {
    match mv {
        DecisionMove::Accept => "Accept",
        DecisionMove::Reject => "Reject",
        DecisionMove::Supersede => "Supersede",
        DecisionMove::Repeal => "Repeal",
    }
}

/// A decision's comment box: what is written in it becomes a comment on the decision.
const COMMENT: Editor = Editor { placeholder: "comment on this decision", send: send_comment };

fn send_comment(me: EntityId, decision: EntityId, text: String) -> Sending {
    Box::pin(async move {
        let mc = MessageContext { decision_id: Some(decision), ..Default::default() };
        ops::post_message(&ctx(), me, &text, Priority::Next, mc).await.map_err(|e| e.to_string())
    })
}

fn open_decision(id: EntityId) {
    route::go(Route::decision(id));
}

/// Where the decision's title leads: its panel on the Decisions page, its list keeping the page's
/// scope choice (`scope_filter::follow`).
fn link_to(id: EntityId) -> String {
    Route::decision(id).hash()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proposal_offers_accept_and_reject_as_buttons_and_a_ruling_in_force_supersede_and_repeal() {
        use DecisionMove::*;
        use DecisionStatus::*;
        let buttons = |status| offers(Some(status)).into_iter().map(|o| (o.mv, o.class())).collect::<Vec<_>>();
        assert_eq!(buttons(Proposed), vec![(Accept, "btn primary"), (Reject, "btn")]);
        assert_eq!(buttons(Accepted), vec![(Supersede, "btn"), (Repeal, "btn")]);
        for closed in [Rejected, Superseded, Repealed] {
            assert!(buttons(closed).is_empty(), "a {closed} decision offers no move");
        }
        assert!(offers(None).is_empty(), "nor does one whose status cannot be read");
    }

    /// A comment on a decision written at `created`, whose `context.move` names the move that wrote
    /// it, if a move did.
    fn comment(created: i64, mv: Option<&str>, text: &str) -> DecisionComment {
        let context = mv.map_or_else(|| serde_json::json!({}), |mv| serde_json::json!({ ops::MOVE_KEY: mv }));
        DecisionComment { created, context, text: text.into() }
    }

    #[test]
    fn why_it_closed_is_the_note_of_the_closing_moves_comment_and_the_note_as_typed_only_without_one() {
        use DecisionStatus::*;
        let task = EntityId::from_bytes([7; 32]).to_base64();
        // The note as typed names a task by the start of its id; the comment the supersession wrote
        // keeps the reference as the move resolved it, the task's whole id, which draws as its
        // lozenge.
        let typed = || Some("jitter avoids stampedes, as [[BwcHBwcH]] found".to_string());
        let superseded = [
            comment(1, Some("accepted"), "Accepted: measured it"),
            comment(2, Some("superseded"), &format!("Superseded by D4: jitter avoids stampedes, as [[{task}]] found")),
            comment(3, None, "a remark after the move"),
        ];
        assert_eq!(
            why_it_closed(Some(Superseded), typed(), &superseded),
            Some(format!("jitter avoids stampedes, as [[{task}]] found")),
            "the move's comment, without the words the move opened it with"
        );
        // A decision closed before its moves wrote comments has only the note as typed.
        assert_eq!(why_it_closed(Some(Superseded), typed(), &superseded[..1]), typed(), "no supersession's comment");
        assert_eq!(why_it_closed(Some(Repealed), typed(), &[]), typed(), "no comment at all");
        assert_eq!(why_it_closed(None, typed(), &superseded), typed(), "a status that cannot be read");
        // An open decision has not closed, whatever its acceptance's comment says, and a closed one
        // with nothing to say shows nothing.
        assert_eq!(why_it_closed(Some(Accepted), None, &superseded[..1]), None);
        assert_eq!(why_it_closed(Some(Rejected), Some("  ".into()), &[]), None);
    }
}
