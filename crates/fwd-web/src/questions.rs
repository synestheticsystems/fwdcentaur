//! The Questions page: the questions asked of one principal, the acting principal by default
//! (Daniel, 2026-09-28: "Ideally it would default to showing only those addressed to me"), or of
//! another chosen from the selector beside its heading, to read without acting as them
//! (`whose.rs`), never everyone's at once. A question is a message that asks someone something (a
//! `to` in its context, or `asks: project`, as `fwd ask` and `fwd forward` write them), and it asks
//! a principal when its `to` names them, by id or by a handle no other principal of the project
//! has, as the routing service reads it when
//! it notifies them. A question asked of no one in particular asks a scope, the one it recorded as
//! it was posted (`asks_scope`), or none, and is the triage agent's that answers first for that
//! scope, the scope's own, else the one that serves the whole project, listed on its page as its
//! own (docs/DECISIONS.md, D122); while no triage agent answers for it, every principal's page lists
//! it, under a heading for its scope, "Asked of the scope Replication, waiting for its triage
//! agent", or "Asked of the project, waiting for triage" for a question asked of no scope, above
//! the priorities (docs/design/web-ui.md, "The Questions page"). The page lists the rest under the priority each
//! wants its answer at, now, then next, then later, each heading saying how many it holds, newest
//! first beneath it: the open ones by default, and the resolved ones too with "open only" off,
//! under the Ask box that records new ones. Each question is a card around its message row, with
//! its answers and the replies to them beneath, worked from the keyboard as the inbox is
//! (`cards.rs`), top to bottom. A question also reaches an inbox, as a notification to
//! whomever it asks. The route can name one (`#/questions/<id>`, where a lozenge naming a question
//! or an answer to one leads), and its card is then selected, shown even when "open only" would
//! hide it, it asks someone else or it is about another scope.
//!
//! The page lists the questions about the scope the header's scope filter chooses for the
//! Questions, Tasks, Decisions and Graph pages at once (`scope_filter.rs`; Daniel, 2026-09-29:
//! "Feature request: Questions, Task, Decisions, and Graph all need a unified Scope selector"): a
//! question asked of no one is about the scope it asks, and any other is about the scope it names,
//! else the scopes the task it is about is worked in, each of them (a task may be worked in several
//! since D136, "A task may be worked in several scopes"), else the scope the decision it is about
//! is attached to (`scope_about`); "no scope" keeps the questions about none, and "all scopes"
//! every question. The scope applies together with whose questions
//! show and "open only". So that no question asked of the principal is out of sight without a
//! word, a line beneath the heading says how many the scope leaves out, with a button that shows
//! all scopes.

use crate::cards::{self, Item, Marks, Notifications, Page};
use crate::empty::{Empty, Nothing};
use crate::referents::Models;
use crate::route::{self, Route};
use crate::scope_filter::ScopeChoice;
use crate::thread::{self, Model, OpenQueries, ThreadState};
use crate::whose::{self, Found, WhosePicker};
use crate::{ask, session, store};
use ankurah::EntityId;
use fwd_core::model::*;
use fwd_core::ops;
use leptos::prelude::*;
use serde_json::Value;
use std::cmp::Reverse;
use std::collections::HashMap;

/// What the selector says on hover.
const HINT: &str =
    "Whose questions this page shows: those asked of you, or of another principal, to read without acting as them";

/// The questions asked of the principal `whose` names, from the route (`whose.rs`), or of the
/// acting principal when it names none; `routed` is the question the route selects, and `scope`
/// the scope the questions are about, the page's choice (`scope_filter::follow`): none until the
/// page has picked the one it opens on.
#[component]
pub fn Questions(
    routed: Memo<Option<EntityId>>,
    whose: Memo<Option<String>>,
    scope: Memo<Option<ScopeChoice>>,
) -> impl IntoView {
    let found = whose::found(whose);
    let choose =
        Callback::new(move |whose| route::go(Route::Questions { question: None, whose, scope: scope.get_untracked() }));
    move || {
        let me = session::acting().get()?.id();
        Some(match found.get() {
            Found::Principal(whose) => {
                view! { <QuestionsFor me=me whose=whose routed=routed scope=scope choose=choose/> }.into_any()
            }
            other => whose::no_one_to_show(other, "Questions", me, HINT, choose),
        })
    }
}

/// The questions asked of `whose`, on a page acting as `me`, about `scope`.
#[component]
fn QuestionsFor(
    me: EntityId,
    whose: EntityId,
    routed: Memo<Option<EntityId>>,
    scope: Memo<Option<ScopeChoice>>,
    choose: Callback<Option<String>>,
) -> impl IntoView {
    let open_only = RwSignal::new(true);
    let selected = RwSignal::new(None::<Item>);
    let asked = Memo::new(move |_| Recipient::of(whose));
    // Every question, each with the scopes it is about.
    let questions = Memo::new(move |_| {
        let governs: HashMap<EntityId, EntityId> =
            store::decisions().iter().filter_map(|d| Some((d.id(), ops::governs(d).ok().flatten()?))).collect();
        store::messages().iter().filter_map(|m| Question::of(m, scope_about(m, &governs))).collect::<Vec<_>>()
    });
    // The question selected, which the page lists whatever the filters.
    let chosen = move || match selected.get() {
        Some(Item::Question(id)) => Some(id),
        _ => None,
    };
    // With "open only", the selected card stays until the selection moves on, so resolving it
    // does not pull it away. None while the page picks the scope it opens on.
    let groups = Memo::new(move |_| {
        let Some(choice) = scope.get() else { return Vec::new() };
        let kept = Kept { whom: asked.get(), front: FrontLines::now(), open_only: open_only.get(), choice };
        questions.with(|questions| kept.listed(questions, chosen()))
    });
    // How many questions asked of `whose` the scope leaves out.
    let left_out = Memo::new(move |_| {
        let Some(choice) = scope.get() else { return 0 };
        let kept = Kept { whom: asked.get(), front: FrontLines::now(), open_only: open_only.get(), choice };
        questions.with(|questions| kept.left_out(questions, chosen()))
    });
    // The cards in the page's order, which the keyboard walks: each group's, top to bottom.
    let items =
        Memo::new(move |_| groups.get().into_iter().flat_map(|(_, ids)| ids).map(Item::Question).collect::<Vec<_>>());
    let page = Page {
        notifications: Notifications::none(),
        items,
        selected,
        thread: ThreadState::new(me),
        whose,
        marks: RwSignal::new(Marks::default()),
        sorted: None,
    };
    // The questions come from the page's standing message query, so the first selection waits
    // for the daemon's answer to it, as the inbox's does, and for the scope the page lists by.
    cards::keep_one_selected(page, Signal::derive(move || store::standing_loaded() && scope.with(Option::is_some)));
    cards::follow_selection(page);
    cards::listen_for_keys(page);
    // The question the route names is the one selected, each time the route names one.
    Effect::new(move |_| {
        if let Some(question) = routed.get() {
            page.select(Item::Question(question));
        }
    });
    // Another scope lists other questions, so the selection starts over on them, as it does on
    // another principal's, rather than keep a card that scope leaves out (a selected card stays
    // listed whatever the filters).
    Effect::new(move |before: Option<Option<ScopeChoice>>| {
        let now = scope.get();
        if before.flatten().is_some_and(|before| Some(before) != now) {
            selected.set(None);
        }
        now
    });
    // Another principal's questions, shown to read, say so beneath the heading.
    let acts = page.acts();
    let reading = move || (!acts).then(|| whose::reading_questions(&store::handle_of(whose), &store::handle_of(me)));
    // The questions the scope leaves out, said beneath the heading, with a way to all of them.
    let outside = move || {
        let sentence = left_out_words(left_out.get(), open_only.get(), &store::handle_of(whose), scope.get()?)?;
        let every_scope = move |_| route::go(route::route().get_untracked().with_scope(ScopeChoice::All));
        Some(view! {
            <p class="left-out-note">
                {sentence} " "
                <button class="btn subtle tiny" on:click=every_scope>"Show all scopes"</button>
            </p>
        })
    };
    let nothing = Signal::derive(move || match scope.get() {
        Some(_) => Nothing::Questions {
            handle: store::handle_of(whose),
            open_only: open_only.get(),
            filtered_out: left_out.get() > 0,
        },
        None => Nothing::Yet,
    });
    let headings = move || groups.with(|groups| groups.iter().map(|(heading, _)| *heading).collect::<Vec<_>>());
    view! {
        <section class="page questions">
            <ask::AskBox me=me scope=scope/>
            <header class="page-head">
                <h2>"Questions"</h2>
                <WhosePicker me=me shown=Some(whose) hint=HINT choose=choose/>
                <label class="toggle">
                    <input type="checkbox" prop:checked=move || open_only.get()
                        on:change=move |ev| open_only.set(event_target_checked(&ev))/>
                    " open only"
                </label>
                <span class="muted">"newest first under each priority"</span>
                <OpenQueries/>
            </header>
            {move || reading().map(|sentence| view! { <p class="reading-note">{sentence}</p> })}
            {outside}
            {move || items.with(Vec::is_empty).then(|| view! { <Empty nothing=nothing/> })}
            <For each=headings key=|heading| *heading children=move |heading| {
                view! { <HeadingGroup heading=heading groups=groups page=page/> }
            }/>
        </section>
    }
}

/// A question, as the page lists it.
#[derive(Clone, Debug, PartialEq)]
struct Question {
    id: EntityId,
    asks: Asks,
    /// Not resolved.
    open: bool,
    /// How soon it wants an answer; none when its priority cannot be read.
    priority: Option<Priority>,
    created: i64,
    /// The scopes it is about: for a question asked of no one, the scope it asks; for any other,
    /// the scopes of its work (`scope_about`); none when it is about no scope's work.
    about: Vec<EntityId>,
}

/// Whom a question asks.
#[derive(Clone, Debug, PartialEq)]
enum Asks {
    /// One principal, as its context's `to` holds it: a principal's id, or a handle.
    Principal(String),
    /// No one in particular (`asks: project` in its context), of the scope it asks, or of none: the
    /// triage agent that answers first for that scope ([`FrontLines`]), or, while none does,
    /// whoever reads the page until one does.
    NoOne(Option<EntityId>),
}

impl Question {
    /// The question the message `m` is, when it asks someone, `about` being the scopes of its work
    /// (`scope_about`). A question asked of no one asks the scope it recorded as it was posted
    /// (`ops::ASKS_SCOPE_KEY`), else, from before it recorded one (2026-09-30), the first scope of
    /// its work, and is about the scope it asks alone.
    fn of(m: &MessageView, about: Vec<EntityId>) -> Option<Question> {
        let context: Value = m.context().map(Into::into).unwrap_or(Value::Null);
        let (asks, about) = if context[ops::ASKS_KEY].as_str() == Some(ops::ASKS_PROJECT) {
            let recorded = context[ops::ASKS_SCOPE_KEY].as_str().and_then(|s| s.parse::<EntityId>().ok());
            let asked = recorded.or_else(|| about.first().copied());
            (Asks::NoOne(asked), asked.into_iter().collect())
        } else {
            (Asks::Principal(context["to"].as_str()?.to_string()), about)
        };
        Some(Question {
            id: m.id(),
            asks,
            open: m.status().ok() == Some(MessageStatus::Active),
            priority: m.priority().ok(),
            created: m.created().unwrap_or(0),
            about,
        })
    }
}

/// The scopes the message `m` is about, so that the page lists a question under the scopes of the
/// work it asks about: the scope it names (a question about the scope itself), else the scopes the
/// task it is about is worked in, each of them, read through the page's index of relations by
/// their ends (`store::scopes_of_task`), else the scope the decision it is about is attached to
/// (`governs` holding each attached decision's scope). None when it is about none of these: about
/// nothing, about a highlight alone, about a task worked in no scope or a ruling for the whole
/// project. A reply, a forward among them, carries what the message it answers is about
/// (`ops::post_message`), so a question forwarded stays where it was.
fn scope_about(m: &MessageView, governs: &HashMap<EntityId, EntityId>) -> Vec<EntityId> {
    let named = m.scope().ok().flatten().map(EntityId::from);
    let task = m.task().ok().flatten().map(EntityId::from);
    let decision = m.decision().ok().flatten().map(EntityId::from);
    about(named, task.map(store::scopes_of_task).unwrap_or_default(), decision.and_then(|d| governs.get(&d).copied()))
}

/// The scopes a message is about, from the scope it names, the scopes its task is worked in and the
/// scope its decision is attached to, the first of these that finds any ([`scope_about`]).
fn about(named: Option<EntityId>, task_in: Vec<EntityId>, decision_on: Option<EntityId>) -> Vec<EntityId> {
    match (named, task_in.is_empty()) {
        (Some(named), _) => vec![named],
        (None, false) => task_in,
        (None, true) => decision_on.into_iter().collect(),
    }
}

/// A heading the page lists questions under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Heading {
    /// The questions asked of no one of a scope, or of no scope, that no triage agent answers for,
    /// one heading per scope above the priorities, so that whoever reads the page sees which
    /// scope's questions wait for an agent to take it.
    Waiting(Option<EntityId>),
    /// The priority the questions under it want their answer at; none when it cannot be read.
    Priority(Option<Priority>),
}

impl Heading {
    fn label(self) -> String {
        match self {
            Heading::Waiting(Some(scope)) => {
                format!("Asked of the scope {}, waiting for its triage agent", store::scope_label(scope))
            }
            Heading::Waiting(None) => "Asked of the project, waiting for triage".to_string(),
            Heading::Priority(priority) => priority.map_or_else(|| "no priority".to_string(), |p| p.to_string()),
        }
    }

    /// Where the heading stands: the waiting questions first, those asked of no scope, then each
    /// scope's by its id, so every client lists them alike; then now, next, later, then none.
    fn order(self) -> (bool, Option<EntityId>, bool, Option<Priority>) {
        match self {
            Heading::Waiting(scope) => (false, scope, false, None),
            Heading::Priority(priority) => (true, None, priority.is_none(), priority),
        }
    }
}

/// Who answers first for the questions asked of no one, as the page holds the principals: the
/// triage agent of each scope that has one, and the one that serves the whole project, chosen as
/// the routing service chooses them (`fwd_core::ops::front_line`; docs/DECISIONS.md, D122).
#[derive(Clone, Debug, Default, PartialEq)]
struct FrontLines {
    /// Each scope a triage agent serves, with that agent.
    scopes: HashMap<EntityId, EntityId>,
    /// The agent that serves the whole project.
    project_wide: Option<EntityId>,
}

impl FrontLines {
    /// Who answers first for what, from the page's standing query of principals.
    fn now() -> FrontLines {
        let principals = store::principals();
        let mut scopes = HashMap::new();
        for p in principals.iter().filter(|p| ops::holds(p, AgentRole::Triage)) {
            for scope in ops::Serving::of(p).scopes {
                if let Some(f) = ops::front_line(&principals, Some(scope)) {
                    scopes.insert(scope, f.agent);
                }
            }
        }
        FrontLines { scopes, project_wide: ops::front_line(&principals, None).map(|f| f.agent) }
    }

    /// The agent that answers first for a question asked of `scope`, or of no scope: the scope's
    /// own, else the one that serves the whole project; none while neither exists.
    fn of(&self, scope: Option<EntityId>) -> Option<EntityId> {
        scope.and_then(|s| self.scopes.get(&s).copied()).or(self.project_wide)
    }
}

/// A principal, as the page weighs whether a question asks them.
#[derive(Clone, Debug, PartialEq)]
struct Recipient {
    id: EntityId,
    /// Their handle, which a question's `to` may hold in place of their id, while no other
    /// principal of the project has it (a handle two or more share names none of them); none while
    /// the page has not received them.
    handle: Option<String>,
}

impl Recipient {
    /// The principal `id`, as the page's standing query of principals holds them.
    fn of(id: EntityId) -> Recipient {
        Recipient { id, handle: store::unique_handle(id) }
    }

    /// Whether a question whose `to` holds `to` asks them, read as the routing service reads it when
    /// it notifies (`fwd_core::routing::plan_for_message`): a `to` names one principal, by id or else
    /// by a handle no other principal of the project has. (A question from before 2026-09-29 to the
    /// supervisor role holds the word `supervisor`, which `fwd admin migrate` makes a question asked
    /// of the project; until then it asks the principal whose handle it is, as routing reads it.)
    fn is_asked(&self, to: &str) -> bool {
        match to.parse::<EntityId>() {
            Ok(id) => id == self.id,
            Err(_) => self.handle.as_deref() == Some(to),
        }
    }
}

/// Which questions the page lists: those asked of `whom`, where `front` says which triage agent
/// answers first for each scope's questions asked of no one, the open ones only with `open_only`,
/// about the scope `choice` keeps.
struct Kept {
    whom: Recipient,
    front: FrontLines,
    open_only: bool,
    choice: ScopeChoice,
}

impl Kept {
    /// Whether `q` asks `whom`: its `to` names them, or it is asked of no one and they answer first
    /// for the scope it asks, or no triage agent does, so that it waits on every principal's page.
    fn asks_whom(&self, q: &Question) -> bool {
        match &q.asks {
            Asks::Principal(to) => self.whom.is_asked(to),
            Asks::NoOne(scope) => self.front.of(*scope).is_none_or(|agent| agent == self.whom.id),
        }
    }

    /// Whether `q` is a question to `whom` that "open only" lists, whatever it is about.
    fn to_whom(&self, q: &Question) -> bool {
        self.asks_whom(q) && (q.open || !self.open_only)
    }

    /// The questions of `questions` that ask `whom`, open ones only with `open_only`, about the
    /// scope `choice` keeps, grouped by the priority each wants its answer at (now, next, later,
    /// then any whose priority cannot be read); the questions asked of no one of a scope no triage
    /// agent answers for are listed for whomever the page shows, under a heading of their scope's
    /// own above the priorities. Each group is newest first (then by id, so every client lists them
    /// alike). The `selected` question is listed too, open or not, whomever it asks and whatever it
    /// is about, so a card stays while it is selected and the question a route names shows.
    fn listed(&self, questions: &[Question], selected: Option<EntityId>) -> Vec<(Heading, Vec<EntityId>)> {
        let heading = |q: &Question| match q.asks {
            Asks::NoOne(scope) if self.front.of(scope).is_none() => Heading::Waiting(scope),
            _ => Heading::Priority(q.priority),
        };
        let mut kept: Vec<(Heading, &Question)> = questions
            .iter()
            .filter(|q| selected == Some(q.id) || (self.to_whom(q) && self.choice.keeps(&q.about)))
            .map(|q| (heading(q), q))
            .collect();
        kept.sort_by_key(|(heading, q)| (heading.order(), Reverse(q.created), q.id));
        let mut groups: Vec<(Heading, Vec<EntityId>)> = Vec::new();
        for (heading, q) in kept {
            match groups.last_mut() {
                Some((last, ids)) if *last == heading => ids.push(q.id),
                _ => groups.push((heading, vec![q.id])),
            }
        }
        groups
    }

    /// How many questions of `questions` that ask `whom`, open ones only with `open_only`, the
    /// scope leaves out of [`Kept::listed`]: those about another scope's work, or about none. The
    /// `selected` question is listed whatever it is about, so it is never among them.
    fn left_out(&self, questions: &[Question], selected: Option<EntityId>) -> usize {
        questions.iter().filter(|q| selected != Some(q.id) && self.to_whom(q) && !self.choice.keeps(&q.about)).count()
    }
}

/// What the line beneath the heading says of the `count` questions to `handle` the scope `choice`
/// leaves out, open ones with `open_only`; none while it leaves none out.
fn left_out_words(count: usize, open_only: bool, handle: &str, choice: ScopeChoice) -> Option<String> {
    let whose = match choice {
        ScopeChoice::All => return None,
        _ if count == 0 => return None,
        ScopeChoice::Scope(_) => "not about this scope",
        ScopeChoice::NoScope => "about work in a scope",
    };
    let open = if open_only { "open " } else { "" };
    let (noun, verb) = if count == 1 { ("question", "is") } else { ("questions", "are") };
    Some(format!("{count} {open}{noun} to {handle} {verb} {whose}."))
}

/// One heading's questions, under the heading, which says how many questions it holds.
#[component]
fn HeadingGroup(heading: Heading, groups: Memo<Vec<(Heading, Vec<EntityId>)>>, page: Page) -> impl IntoView {
    let ids = Memo::new(move |_| {
        groups.with(|groups| groups.iter().find(|(h, _)| *h == heading).map(|(_, ids)| ids.clone()).unwrap_or_default())
    });
    let label = heading.label();
    view! {
        <section class="priority-group">
            <h3 class="priority-head">{label} " " <span class="count">{move || ids.with(Vec::len)}</span></h3>
            <For each=move || ids.get() key=|id| *id children=move |id| view! { <QuestionCard id=id page=page/> }/>
        </section>
    }
}

/// A question as a card: selected and worked from the keyboard like a notification's, around the
/// question's own message row, which brings reply, resolve and the rows beneath it.
#[component]
fn QuestionCard(id: EntityId, page: Page) -> impl IntoView {
    let item = Item::Question(id);
    let selected = Memo::new(move |_| page.selected.get() == Some(item));
    let hooks = page.hooks(item, None);
    view! {
        <article class="notif question" id=item.element_id() tabindex="-1" class:selected=move || selected.get()>
            {thread::card_row(page.thread, id, id, Models::one(Model::Message), hooks)}
        </article>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fwd_core::model::Priority::{Later, Next, Now};

    fn id(n: u8) -> EntityId {
        EntityId::from_bytes([n; 32])
    }

    fn question(n: u8, to: &str, open: bool, priority: Option<Priority>, created: i64) -> Question {
        Question { id: id(n), asks: Asks::Principal(to.to_string()), open, priority, created, about: Vec::new() }
    }

    /// A question asked of the project, of no one in particular.
    fn of_the_project(n: u8, open: bool, priority: Option<Priority>, created: i64) -> Question {
        Question { id: id(n), asks: Asks::NoOne(None), open, priority, created, about: Vec::new() }
    }

    fn person(n: u8, handle: &str) -> Recipient {
        Recipient { id: id(n), handle: Some(handle.to_string()) }
    }

    /// What the page lists of `questions` for `whom` with the scope on all scopes, as it listed
    /// every question before the scope filter, where `triage` serves the whole project and no scope
    /// has a triage agent of its own.
    fn listed(
        questions: &[Question],
        whom: &Recipient,
        triage: Option<EntityId>,
        open_only: bool,
        selected: Option<EntityId>,
    ) -> Vec<(Heading, Vec<EntityId>)> {
        let front = FrontLines { project_wide: triage, ..Default::default() };
        Kept { whom: whom.clone(), front, open_only, choice: ScopeChoice::All }.listed(questions, selected)
    }

    fn by_priority(groups: Vec<(Option<Priority>, Vec<EntityId>)>) -> Vec<(Heading, Vec<EntityId>)> {
        groups.into_iter().map(|(priority, ids)| (Heading::Priority(priority), ids)).collect()
    }

    /// Questions to dnorman (100), claude (101) and triage-bot (102), the project's triage agent.
    fn asked() -> Vec<Question> {
        let (dnorman, claude, triage) = (id(100).to_base64(), id(101).to_base64(), id(102).to_base64());
        vec![
            question(1, &dnorman, true, Some(Later), 10),
            question(2, &dnorman, true, Some(Now), 20),
            question(3, &claude, true, Some(Now), 30),
            question(4, "dnorman", true, Some(Later), 40),
            question(5, &dnorman, false, Some(Now), 50),
            question(6, &triage, true, Some(Now), 60),
            question(7, &dnorman, true, Some(Next), 5),
            question(8, &dnorman, true, None, 70),
            question(9, &claude, false, Some(Later), 80),
        ]
    }

    #[test]
    fn a_question_asks_the_one_principal_its_to_names_by_id_or_handle() {
        let dnorman = person(100, "dnorman");
        assert!(dnorman.is_asked(&id(100).to_base64()));
        assert!(dnorman.is_asked("dnorman"), "by handle, which the routing service takes too");
        assert!(!dnorman.is_asked(&id(101).to_base64()));
        assert!(!dnorman.is_asked("claude"));
        let triage = person(102, "triage-bot");
        assert!(triage.is_asked(&id(102).to_base64()), "the triage agent is asked by its id, as anyone is");
        assert!(!dnorman.is_asked(&id(102).to_base64()), "a question to the triage agent asks no one else");
        // A principal the page has not received yet is still found by id.
        let unseen = Recipient { id: id(103), handle: None };
        assert!(unseen.is_asked(&id(103).to_base64()) && !unseen.is_asked("dnorman"));
    }

    #[test]
    fn the_page_lists_the_open_questions_asked_of_its_principal_alone_by_priority_newest_first() {
        let dnorman = person(100, "dnorman");
        let triage = Some(id(102));
        // Neither claude's (3, 9), nor the triage agent's (6), nor the resolved one (5).
        assert_eq!(
            listed(&asked(), &dnorman, triage, true, None),
            by_priority(vec![
                (Some(Now), vec![id(2)]),
                (Some(Next), vec![id(7)]),
                (Some(Later), vec![id(4), id(1)]),
                (None, vec![id(8)])
            ])
        );
        // With "open only" off, the resolved ones too, each in its place.
        assert_eq!(
            listed(&asked(), &dnorman, triage, false, None),
            by_priority(vec![
                (Some(Now), vec![id(5), id(2)]),
                (Some(Next), vec![id(7)]),
                (Some(Later), vec![id(4), id(1)]),
                (None, vec![id(8)])
            ])
        );
        // Another principal's list holds theirs alone: claude's, and the triage agent's.
        assert_eq!(
            listed(&asked(), &person(101, "claude"), triage, false, None),
            by_priority(vec![(Some(Now), vec![id(3)]), (Some(Later), vec![id(9)])])
        );
        assert_eq!(
            listed(&asked(), &person(102, "triage-bot"), triage, true, None),
            by_priority(vec![(Some(Now), vec![id(6)])])
        );
    }

    #[test]
    fn a_question_asked_of_the_project_is_the_triage_agents_own_and_waits_on_every_page_while_no_agent_holds_triage() {
        let mut questions = asked();
        questions.extend([of_the_project(20, true, Some(Now), 90), of_the_project(21, true, Some(Later), 95)]);
        questions.push(of_the_project(22, false, Some(Now), 99));
        // An agent holds triage: the questions asked of the project are its own, under their
        // priorities, and no one else's.
        let triage = Some(id(102));
        assert_eq!(
            listed(&questions, &person(102, "triage-bot"), triage, true, None),
            by_priority(vec![(Some(Now), vec![id(20), id(6)]), (Some(Later), vec![id(21)])])
        );
        assert_eq!(
            listed(&questions, &person(101, "claude"), triage, true, None),
            listed(&asked(), &person(101, "claude"), triage, true, None)
        );
        // No agent holds triage: every principal's page lists them, newest first, under a heading
        // of their own above the priorities, the resolved one too with "open only" off.
        for whom in [person(100, "dnorman"), person(101, "claude"), person(102, "triage-bot")] {
            let listing = listed(&questions, &whom, None, true, None);
            assert_eq!(listing[0], (Heading::Waiting(None), vec![id(21), id(20)]), "on {:?}'s page", whom.handle);
            assert_eq!(listing[1..], listed(&asked(), &whom, None, true, None)[..]);
            assert_eq!(
                listed(&questions, &whom, None, false, None)[0],
                (Heading::Waiting(None), vec![id(22), id(21), id(20)])
            );
        }
    }

    /// Triage per scope (docs/DECISIONS.md, D122): a question asked of no one of a scope is the
    /// triage agent's of that scope, else the one's that serves the whole project; a question
    /// asked of a scope no agent answers for waits on every page, under its scope's own heading,
    /// beside the heading of those asked of no scope; and each such question is about the scope it
    /// asks, for the scope filter.
    #[test]
    fn a_question_asked_of_a_scope_is_its_triage_agents_own_and_waits_under_its_scope_while_none_is() {
        let (a, b) = (id(50), id(60));
        let (triage_a, whole) = (person(102, "triage-a"), person(103, "triage-all"));
        let of_scope = |n: u8, scope: Option<EntityId>, created: i64| Question {
            asks: Asks::NoOne(scope),
            about: scope.into_iter().collect(),
            ..of_the_project(n, true, Some(Now), created)
        };
        let questions = vec![of_scope(1, Some(a), 10), of_scope(2, Some(b), 20), of_scope(3, None, 30)];
        let lists = |front: &FrontLines, whom: &Recipient| {
            Kept { whom: whom.clone(), front: front.clone(), open_only: true, choice: ScopeChoice::All }
                .listed(&questions, None)
        };
        let now = |ids: Vec<u8>| by_priority(vec![(Some(Now), ids.into_iter().map(id).collect())]);
        // A's agent has A's question; the agent that serves the whole project has B's and the one
        // asked of no scope; Daniel has none of them.
        let both = FrontLines { scopes: HashMap::from([(a, triage_a.id)]), project_wide: Some(whole.id) };
        assert_eq!(lists(&both, &triage_a), now(vec![1]));
        assert_eq!(lists(&both, &whole), now(vec![3, 2]));
        assert_eq!(lists(&both, &person(100, "dnorman")), vec![]);
        // With no agent serving the whole project, B's question and the one asked of no scope wait
        // on every page, each under its own heading, the one asked of no scope first.
        let scoped = FrontLines { scopes: HashMap::from([(a, triage_a.id)]), project_wide: None };
        let waiting = vec![(Heading::Waiting(None), vec![id(3)]), (Heading::Waiting(Some(b)), vec![id(2)])];
        assert_eq!(lists(&scoped, &person(100, "dnorman")), waiting);
        let mut theirs = waiting.clone();
        theirs.extend(now(vec![1]));
        assert_eq!(lists(&scoped, &triage_a), theirs);
        // The scope filter keeps the question under the scope it asks.
        let kept = Kept { whom: person(100, "dnorman"), front: scoped, open_only: true, choice: ScopeChoice::Scope(b) };
        assert_eq!(kept.listed(&questions, None), vec![(Heading::Waiting(Some(b)), vec![id(2)])]);
    }

    #[test]
    fn the_selected_question_stays_listed_whomever_it_asks_and_once_resolved() {
        let dnorman = person(100, "dnorman");
        let triage = Some(id(102));
        // The route named claude's question (3); dnorman's resolved one (5) was resolved while
        // selected. Each shows in its place until the selection moves on.
        assert_eq!(
            listed(&asked(), &dnorman, triage, true, Some(id(3)))[0],
            (Heading::Priority(Some(Now)), vec![id(3), id(2)])
        );
        assert_eq!(
            listed(&asked(), &dnorman, triage, true, Some(id(5)))[0],
            (Heading::Priority(Some(Now)), vec![id(5), id(2)])
        );
        assert_eq!(
            listed(&asked(), &dnorman, triage, true, Some(id(42))),
            listed(&asked(), &dnorman, triage, true, None)
        );
    }

    #[test]
    fn questions_asked_at_the_same_time_list_alike_everywhere() {
        let dnorman = person(100, "dnorman");
        let to = id(100).to_base64();
        let same = [question(12, &to, true, Some(Now), 7), question(11, &to, true, Some(Now), 7)];
        assert_eq!(listed(&same, &dnorman, None, true, None), by_priority(vec![(Some(Now), vec![id(11), id(12)])]));
    }

    #[test]
    fn a_question_is_about_the_scope_it_names_else_its_tasks_else_its_decisions() {
        let (named, decision_on) = (Some(id(50)), Some(id(70)));
        let task_in = || vec![id(60), id(61)];
        assert_eq!(about(named, task_in(), decision_on), vec![id(50)]);
        // A task worked in two scopes: its question is about both (docs/DECISIONS.md, D136).
        assert_eq!(about(None, task_in(), decision_on), task_in());
        assert_eq!(about(None, Vec::new(), decision_on), vec![id(70)]);
        // About nothing, a highlight alone, a task in no scope or a ruling for the whole project.
        assert_eq!(about(None, Vec::new(), None), Vec::<EntityId>::new());
    }

    #[test]
    fn the_scope_keeps_the_questions_about_its_work_and_the_page_says_how_many_it_leaves_out() {
        let (a, b, c) = (id(50), id(60), id(70));
        let to = id(100).to_base64();
        let about = |mut q: Question, scopes: Vec<EntityId>| {
            q.about = scopes;
            q
        };
        let questions = vec![
            about(question(1, &to, true, Some(Now), 10), vec![a]),
            about(question(2, &to, true, Some(Now), 20), vec![b]),
            about(question(3, &to, true, Some(Now), 30), vec![]),
            about(question(4, &to, false, Some(Now), 40), vec![b]),
            // Claude's, about A: never dnorman's, whatever the scope.
            about(question(5, &id(101).to_base64(), true, Some(Now), 50), vec![a]),
        ];
        let kept =
            |choice, open_only| Kept { whom: person(100, "dnorman"), front: FrontLines::default(), open_only, choice };
        let now = |ids: Vec<u8>| by_priority(vec![(Some(Now), ids.into_iter().map(id).collect())]);
        // A scope keeps its own questions; the others, open ones only with "open only", are counted.
        assert_eq!(kept(ScopeChoice::Scope(a), true).listed(&questions, None), now(vec![1]));
        assert_eq!(
            kept(ScopeChoice::Scope(a), true).left_out(&questions, None),
            2,
            "B's open one and the one about none"
        );
        assert_eq!(kept(ScopeChoice::Scope(a), false).left_out(&questions, None), 3, "and B's resolved one too");
        assert_eq!(kept(ScopeChoice::Scope(b), false).listed(&questions, None), now(vec![4, 2]));
        // "no scope" keeps the questions about no scope's work; "all scopes" leaves none out.
        assert_eq!(kept(ScopeChoice::NoScope, true).listed(&questions, None), now(vec![3]));
        assert_eq!(kept(ScopeChoice::NoScope, true).left_out(&questions, None), 2);
        assert_eq!(kept(ScopeChoice::All, true).listed(&questions, None), now(vec![3, 2, 1]));
        assert_eq!(kept(ScopeChoice::All, false).left_out(&questions, None), 0);
        // The question the route names shows whatever it is about, and is not counted as left out.
        assert_eq!(kept(ScopeChoice::Scope(a), true).listed(&questions, Some(id(2))), now(vec![2, 1]));
        assert_eq!(kept(ScopeChoice::Scope(a), true).left_out(&questions, Some(id(2))), 1);
        // A question about a task worked in two scopes is kept under each, and left out of a third.
        let both = vec![about(question(6, &to, true, Some(Now), 60), vec![a, b])];
        assert_eq!(kept(ScopeChoice::Scope(a), true).listed(&both, None), now(vec![6]));
        assert_eq!(kept(ScopeChoice::Scope(b), true).listed(&both, None), now(vec![6]));
        assert_eq!(kept(ScopeChoice::Scope(c), true).left_out(&both, None), 1);
        assert_eq!(kept(ScopeChoice::NoScope, true).left_out(&both, None), 1);
    }

    #[test]
    fn the_line_beneath_the_heading_says_how_many_questions_the_scope_leaves_out() {
        let a = ScopeChoice::Scope(id(50));
        assert_eq!(
            left_out_words(3, true, "dnorman", a).as_deref(),
            Some("3 open questions to dnorman are not about this scope.")
        );
        assert_eq!(
            left_out_words(1, false, "dnorman", a).as_deref(),
            Some("1 question to dnorman is not about this scope.")
        );
        assert_eq!(
            left_out_words(2, true, "claude", ScopeChoice::NoScope).as_deref(),
            Some("2 open questions to claude are about work in a scope.")
        );
        // Nothing to say when the scope leaves none out, or keeps every question.
        assert_eq!(left_out_words(0, true, "dnorman", a), None);
        assert_eq!(left_out_words(4, true, "dnorman", ScopeChoice::All), None);
    }
}
