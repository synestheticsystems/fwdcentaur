//! The form on the Tasks page that writes a task with every field `fwd task create` takes (Daniel,
//! 2026-09-26: "can we have a task request function? or maybe a direct create task function?
//! Ideally I'd be able to give the jist and have the triager actually write the task details, or
//! specify every field myself."). Giving the gist is the Ask box's: a question asked of no one in
//! particular, as it asks by default, reaches the triage agent of the scope it asks, else the one
//! that serves the whole project, which writes the task. This
//! form is for specifying every field: the title (verb first, as the project writes them), why,
//! what and done-when, the priority and the labels (its box suggests `bug` and `feature` first:
//! `labels.rs`), whether it is open or only proposed and whether its work needs review; and, each
//! optional, the task it is part of, the tasks it waits on, and the task or message it came from or
//! that motivates it. A picker takes each of those from its list, or from the start of an id (six
//! characters or more) typed in its box, by the rule a command line runs (`ops::name_to_id`).
//!
//! "Create task" (or Ctrl+Enter in any field) writes the task through `ops::create_task`, as
//! `fwd task create` does, as whoever the page acts as and worked in the scope the header's
//! selector chose, where the command's task is worked in the scope the command works in; the panel
//! then shows the task made, and the list, which reads the page's standing query, shows it as soon
//! as its filters keep it. The form keeps what was written while the panel shows a task and back,
//! until the task is made or Clear empties it. What the form sends and what it refuses (no title; a
//! name that stands for no entity its picker takes) are worked out without the browser and tested
//! natively.

use crate::referents::Mode;
use crate::route::{self, Route};
use crate::scope_filter::ScopeChoice;
use crate::session::{self, ctx};
use crate::{fmt, keys, labels, modes, panel, references, store};
use ankurah::EntityId;
use fwd_core::ops::{self, CreateTask, Kind};
use fwd_core::rules;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

/// One of the form's pickers: what it is called, the kinds of entity it takes, and whether it
/// takes several.
#[derive(Clone, Copy, Debug)]
pub struct Slot {
    pub name: &'static str,
    pub kinds: &'static [Kind],
    pub several: bool,
}

/// What a task can come from or be motivated by, as `fwd task create` takes them: a task or a
/// message, what `discovered_from` and `motivated_by` join a task to (`ops::joins`). The command
/// refuses any other kind there, a decision too, before it writes anything, so neither the
/// came-from nor the motivated-by picker offers one (task 5bJv0xvq).
const SOURCES: &[Kind] = &[Kind::Task, Kind::Message];

/// The task it is part of: `--under`, a `subtask_of` relation.
pub const PART_OF: Slot = Slot { name: "part of", kinds: &[Kind::Task], several: false };
/// The tasks it cannot start before: `--after`, `depends_on` relations.
pub const WAITS_ON: Slot = Slot { name: "waits on", kinds: &[Kind::Task], several: true };
/// The work it surfaced from: `--from`, a `discovered_from` relation.
pub const CAME_FROM: Slot = Slot { name: "came from", kinds: SOURCES, several: false };
/// Why it exists, in graph form: `--motivated-by`, a `motivated_by` relation.
pub const MOTIVATED_BY: Slot = Slot { name: "motivated by", kinds: SOURCES, several: false };

/// What a picker holds: the entities chosen, and whatever is typed in its box and not chosen yet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Picked {
    pub chosen: Vec<EntityId>,
    pub typed: String,
}

/// The form's fields as they stand when "Create task" is pressed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filled {
    pub title: String,
    pub why: String,
    pub what: String,
    pub done_when: String,
    pub priority: Option<i64>,
    /// As typed, commas between them.
    pub labels: String,
    pub propose: bool,
    pub needs_review: bool,
    pub part_of: Picked,
    pub waits_on: Picked,
    pub came_from: Picked,
    pub motivated_by: Picked,
}

/// What the pickers can name, from the page's standing queries: each entity with its kind, and
/// each decision's number.
#[derive(Clone, Debug, Default)]
pub struct Known {
    pub entities: Vec<(EntityId, Kind)>,
    pub numbers: Vec<(EntityId, i64)>,
}

/// The entity a name typed into a picker stands for, by the rule a command line runs
/// (`ops::name_to_id`: the whole id, the start of one, six characters or more, or `D<n>` for a
/// decision), among what the page holds of the picker's kinds. The command line takes any whole
/// id as it is; the picker takes only one of the kinds it offers.
pub fn resolve(text: &str, slot: Slot, known: &Known) -> Result<EntityId, String> {
    let text = text.trim();
    let numbered: Vec<EntityId> = match rules::decision_number(text) {
        Some(n) => known.numbers.iter().filter(|(_, number)| *number == n).map(|(id, _)| *id).collect(),
        None => Vec::new(),
    };
    let id = ops::name_to_id(text, slot.kinds, &numbered, &known.entities).map_err(|e| e.to_string())?;
    if known.entities.iter().any(|(held, kind)| *held == id && slot.kinds.contains(kind)) {
        Ok(id)
    } else {
        let kinds = slot.kinds.iter().map(|k| k.name()).collect::<Vec<_>>().join(" or ");
        Err(format!("`{text}` is not the id of a {kinds} this page holds"))
    }
}

/// What a picker names, each once: what was chosen from its list, then what its typed name stands
/// for, which in a picker of one takes the place of the choice. A refusal names the picker.
pub fn picked(p: &Picked, slot: Slot, known: &Known) -> Result<Vec<EntityId>, String> {
    let mut ids = p.chosen.clone();
    if !p.typed.trim().is_empty() {
        let id = resolve(&p.typed, slot, known).map_err(|why| format!("{}: {why}", slot.name))?;
        if !slot.several {
            ids.clear();
        }
        ids.push(id);
    }
    let mut once: Vec<EntityId> = Vec::new();
    for id in ids {
        if !once.contains(&id) {
            once.push(id);
        }
    }
    Ok(once)
}

/// The task the form describes, or why it cannot be made: it needs a title, and each name typed
/// into a picker must stand for an entity the picker takes; nothing is written until both hold.
/// The prose goes as typed, and the title without the spaces around it. The task is worked in the
/// scope the header's selector chose, `scope`, as `fwd task create` makes a task worked in the
/// scope the command works in, and in none while it chooses all scopes or no scope.
pub fn spec(filled: &Filled, known: &Known, scope: Option<ScopeChoice>) -> Result<CreateTask, String> {
    let title = filled.title.trim();
    if title.is_empty() {
        return Err("a task needs a title: what to do, verb first".into());
    }
    let one = |p: &Picked, slot: Slot| picked(p, slot, known).map(|ids| ids.last().copied());
    Ok(CreateTask {
        title: title.to_string(),
        what: filled.what.clone(),
        why: filled.why.clone(),
        done_when: filled.done_when.clone(),
        propose: filled.propose,
        needs_review: filled.needs_review,
        priority: filled.priority,
        labels: labels::typed(&filled.labels),
        after: picked(&filled.waits_on, WAITS_ON, known)?,
        under: one(&filled.part_of, PART_OF)?,
        from: one(&filled.came_from, CAME_FROM)?,
        motivated_by: one(&filled.motivated_by, MOTIVATED_BY)?,
        scope: scope.and_then(ScopeChoice::scope),
    })
}

/// What the page's standing queries hold that a picker can name.
fn known() -> Known {
    let decisions = store::decisions();
    let mut entities: Vec<(EntityId, Kind)> = store::tasks().iter().map(|t| (t.id(), Kind::Task)).collect();
    entities.extend(store::messages().iter().map(|m| (m.id(), Kind::Message)));
    entities.extend(decisions.iter().map(|d| (d.id(), Kind::Decision)));
    let numbers = decisions.iter().filter_map(|d| Some((d.id(), d.number().ok()?))).collect();
    Known { entities, numbers }
}

/// How many messages a picker's list offers, the newest; the start of any message's id names it.
const MESSAGES_OFFERED: usize = 200;

/// The groups of a picker's list, each a heading and its entities' ids and names: tasks by title,
/// those still to do first; the newest messages.
fn offered(slot: Slot) -> Vec<(&'static str, Vec<(EntityId, String)>)> {
    let mut groups: Vec<(&'static str, Vec<(EntityId, String)>)> = Vec::new();
    if slot.kinds.contains(&Kind::Task) {
        let mut tasks: Vec<(bool, String, EntityId)> = store::tasks()
            .iter()
            .map(|t| {
                let title = t.title().unwrap_or_default();
                (t.status().is_ok_and(fmt::is_closed), title, t.id())
            })
            .collect();
        tasks.sort_by_cached_key(|(closed, title, id)| (*closed, title.to_lowercase(), *id));
        let name = |(_, title, id): &(bool, String, EntityId)| (*id, format!("{} {title}", fmt::short(*id)));
        let (closed, open): (Vec<_>, Vec<_>) = tasks.iter().partition(|t| t.0);
        groups.push(("tasks to do", open.iter().map(|t| name(t)).collect()));
        groups.push(("closed tasks", closed.iter().map(|t| name(t)).collect()));
    }
    if slot.kinds.contains(&Kind::Message) {
        let mut messages = store::messages();
        messages.sort_by_key(|m| std::cmp::Reverse(m.created().unwrap_or(0)));
        let named = messages
            .iter()
            .take(MESSAGES_OFFERED)
            .map(|m| {
                let author = m.author().ok().map(|a| store::handle_of(EntityId::from(a))).unwrap_or_default();
                let text = references::plain(&ops::text(m.content()).unwrap_or_default());
                (m.id(), format!("{author} “{}”", fmt::truncate(&text, 60)))
            })
            .collect();
        groups.push(("messages, newest first", named));
    }
    groups.retain(|(_, members)| !members.is_empty());
    groups
}

/// One picker's state: what is chosen, what is typed in its box, and why the last name typed there
/// was refused.
#[derive(Clone, Copy)]
struct Pick {
    chosen: RwSignal<Vec<EntityId>>,
    typed: RwSignal<String>,
    refused: RwSignal<Option<String>>,
}

impl Pick {
    fn empty() -> Pick {
        Pick { chosen: RwSignal::new(Vec::new()), typed: RwSignal::new(String::new()), refused: RwSignal::new(None) }
    }

    fn read(self) -> Picked {
        Picked { chosen: self.chosen.get_untracked(), typed: self.typed.get_untracked() }
    }

    fn clear(self) {
        self.chosen.try_set(Vec::new());
        self.typed.try_set(String::new());
        self.refused.try_set(None);
    }

    /// Takes `id` as chosen; in a picker of one, in place of what was.
    fn choose(self, slot: Slot, id: EntityId) {
        self.chosen.update(|chosen| {
            if !slot.several {
                chosen.clear();
            }
            if !chosen.contains(&id) {
                chosen.push(id);
            }
        });
        self.refused.set(None);
    }

    /// Chooses what the name typed in the box stands for and empties the box, or says why it
    /// stands for nothing and leaves it there to mend.
    fn take_typed(self, slot: Slot) {
        let typed = self.typed.get_untracked();
        if typed.trim().is_empty() {
            return;
        }
        match resolve(&typed, slot, &known()) {
            Ok(id) => {
                self.choose(slot, id);
                self.typed.set(String::new());
            }
            Err(why) => self.refused.set(Some(why)),
        }
    }
}

/// What was written for a new task: `Copy` handles, which the Tasks page keeps while its panel
/// shows a task, and which the form's closures share.
#[derive(Clone, Copy)]
pub struct Draft {
    title: RwSignal<String>,
    why: RwSignal<String>,
    what: RwSignal<String>,
    done_when: RwSignal<String>,
    priority: RwSignal<Option<i64>>,
    labels: RwSignal<String>,
    propose: RwSignal<bool>,
    needs_review: RwSignal<bool>,
    part_of: Pick,
    waits_on: Pick,
    came_from: Pick,
    motivated_by: Pick,
}

impl Draft {
    /// Nothing written yet: an open task, needing no review.
    pub fn empty() -> Draft {
        Draft {
            title: RwSignal::new(String::new()),
            why: RwSignal::new(String::new()),
            what: RwSignal::new(String::new()),
            done_when: RwSignal::new(String::new()),
            priority: RwSignal::new(None),
            labels: RwSignal::new(String::new()),
            propose: RwSignal::new(false),
            needs_review: RwSignal::new(false),
            part_of: Pick::empty(),
            waits_on: Pick::empty(),
            came_from: Pick::empty(),
            motivated_by: Pick::empty(),
        }
    }

    fn read(self) -> Filled {
        Filled {
            title: self.title.get_untracked(),
            why: self.why.get_untracked(),
            what: self.what.get_untracked(),
            done_when: self.done_when.get_untracked(),
            priority: self.priority.get_untracked(),
            labels: self.labels.get_untracked(),
            propose: self.propose.get_untracked(),
            needs_review: self.needs_review.get_untracked(),
            part_of: self.part_of.read(),
            waits_on: self.waits_on.read(),
            came_from: self.came_from.read(),
            motivated_by: self.motivated_by.read(),
        }
    }

    fn clear(self) {
        for text in [self.title, self.why, self.what, self.done_when, self.labels] {
            text.try_set(String::new());
        }
        self.priority.try_set(None);
        self.propose.try_set(false);
        self.needs_review.try_set(false);
        for pick in [self.part_of, self.waits_on, self.came_from, self.motivated_by] {
            pick.clear();
        }
    }
}

/// The panel's form. `scope` is the scope the Tasks page's list keeps, the header's selector's
/// choice, which the task made is worked in when it is one scope ([`spec`]) and which the page keeps
/// once the task is made.
#[component]
pub fn NewTask(draft: Draft, scope: Memo<Option<ScopeChoice>>) -> impl IntoView {
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let create = move || {
        let Some(me) = session::me() else { return };
        if busy.get_untracked() {
            return;
        }
        let scope = scope.get_untracked();
        let spec = match spec(&draft.read(), &known(), scope) {
            Ok(spec) => spec,
            Err(why) => {
                error.set(Some(why));
                return;
            }
        };
        busy.set(true);
        spawn_local(async move {
            let made = ops::create_task(&ctx(), me, spec).await;
            busy.try_set(false);
            match made {
                Ok(id) => {
                    session::wrote(me);
                    error.try_set(None);
                    draft.clear();
                    route::go(Route::tasks(Some(id), scope));
                }
                Err(e) => {
                    error.try_set(Some(e.to_string()));
                }
            }
        });
    };
    // Ctrl+Enter (⌘+Enter on a Mac) creates the task from any field, since Enter alone starts a
    // new line in the prose and chooses the typed name in a picker.
    let on_key = move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Enter" && (ev.ctrl_key() || ev.meta_key()) {
            ev.prevent_default();
            create();
        }
    };
    let text = move |signal: RwSignal<String>| {
        (move || signal.get(), move |ev: leptos::ev::Event| signal.set(event_target_value(&ev)))
    };
    let (title, on_title) = text(draft.title);
    let (why, on_why) = text(draft.why);
    let (what, on_what) = text(draft.what);
    let (done_when, on_done_when) = text(draft.done_when);
    let (typed_labels, on_labels) = text(draft.labels);
    // What the Labels field suggests: bug and feature first, then the other labels in use, each
    // after the labels typed before it.
    let suggested = move || labels::suggestions(&draft.labels.get(), &labels::in_use(), &[]);
    // The priority select shows the draft's, set once its options are drawn, since an option's
    // `selected` attribute stops moving a select once a person has chosen in it.
    let priority = NodeRef::<leptos::html::Select>::new();
    Effect::new(move |_| {
        let value = draft.priority.get().map(|p| p.to_string()).unwrap_or_default();
        if let Some(select) = priority.get() {
            select.set_value(&value);
        }
    });
    let who = move || session::acting().get().and_then(|p| p.handle().ok()).unwrap_or_default();
    let pane = NodeRef::<leptos::html::Div>::new();
    panel::show_beneath_list(pane);
    view! {
        <div class="detail new-task" node_ref=pane on:keydown=on_key>
            <button class="btn subtle panel-close" title="Close the form; what is written stays until it is cleared"
                on:click=move |_| route::go(Route::tasks(None, scope.get_untracked()))>"Close"</button>
            <header class="detail-head">
                <h3>"New task"</h3>
                <div class="muted">{move || format!("created by {}, who the page acts as", who())}</div>
            </header>
            <div class="form">
                <label class="form-field">
                    <span class="field-name">"Title"</span>
                    <input type="text" placeholder="verb first: what to do, as in Cap the retry delay"
                        prop:value=title on:input=on_title/>
                </label>
                <label class="form-field">
                    <span class="field-name">"Why"</span>
                    <textarea placeholder="why it should exist" prop:value=why on:input=on_why></textarea>
                </label>
                <label class="form-field">
                    <span class="field-name">"What"</span>
                    <textarea placeholder="what to do, in full" prop:value=what on:input=on_what></textarea>
                </label>
                <label class="form-field">
                    <span class="field-name">"Done when"</span>
                    <textarea placeholder="how anyone will know it is done" prop:value=done_when
                        on:input=on_done_when></textarea>
                </label>
                <div class="form-row">
                    <label class="form-field">
                        <span class="field-name">"Priority"</span>
                        <select node_ref=priority
                            on:change=move |ev| draft.priority.set(event_target_value(&ev).parse().ok())>
                            <option value="">"none"</option>
                            <option value="0">"P0, urgent"</option>
                            <option value="1">"P1"</option>
                            <option value="2">"P2"</option>
                            <option value="3">"P3"</option>
                            <option value="4">"P4"</option>
                        </select>
                    </label>
                    <fieldset class="form-field choice">
                        <legend class="field-name">"Status"</legend>
                        <label title="sanctioned: anyone may start it">
                            <input type="radio" name="new-task-status" prop:checked=move || !draft.propose.get()
                                on:change=move |_| draft.propose.set(false)/>
                            " open"
                        </label>
                        <label title="I think this should exist; tell me if not">
                            <input type="radio" name="new-task-status" prop:checked=move || draft.propose.get()
                                on:change=move |_| draft.propose.set(true)/>
                            " proposed"
                        </label>
                    </fieldset>
                    <fieldset class="form-field choice">
                        <legend class="field-name">"Review"</legend>
                        <label title="it is done only once someone other than whoever submits it accepts it">
                            <input type="checkbox" prop:checked=move || draft.needs_review.get()
                                on:change=move |ev| draft.needs_review.set(event_target_checked(&ev))/>
                            " needs review"
                        </label>
                    </fieldset>
                </div>
                <label class="form-field">
                    <span class="field-name">"Labels"</span>
                    <input type="text" list="new-task-labels" placeholder="bug, feature: commas between them"
                        prop:value=typed_labels on:input=on_labels/>
                    {labels::datalist("new-task-labels".into(), suggested)}
                </label>
                <Picker of=PART_OF pick=draft.part_of hint="the task this is a part of" choose="choose a task…"/>
                <Picker of=WAITS_ON pick=draft.waits_on hint="tasks that must be done before this can start"
                    choose="add a task…"/>
                <Picker of=CAME_FROM pick=draft.came_from hint="the task or message whose work surfaced it"
                    choose="choose one…"/>
                <Picker of=MOTIVATED_BY pick=draft.motivated_by hint="the task or message it exists for"
                    choose="choose one…"/>
                <div class="form-actions">
                    <button class="btn primary" disabled=move || busy.get() on:click=move |_| create()>"Create task"</button>
                    <button class="btn subtle" on:click=move |_| {
                        draft.clear();
                        error.set(None);
                    }>"Clear"</button>
                    <span class="muted">"or Ctrl+Enter"</span>
                    {move || error.get().map(|e| view! { <span class="error">{e}</span> })}
                </div>
            </div>
        </div>
    }
}

/// A picker: what is chosen, each as its lozenge with a button that takes it out, then a list to
/// choose from and a box for the start of an id, which Enter, or leaving the box, chooses.
#[component]
fn Picker(of: Slot, pick: Pick, hint: &'static str, choose: &'static str) -> impl IntoView {
    // Named `of` where it is given, since `view!` keeps `slot` for Leptos's slots.
    let slot = of;
    let list = NodeRef::<leptos::html::Select>::new();
    let on_list = move |ev: leptos::ev::Event| {
        if let Ok(id) = event_target_value(&ev).parse::<EntityId>() {
            pick.choose(slot, id);
        }
        // Back to its first line, ready for another choice.
        if let Some(list) = list.get_untracked() {
            list.set_value("");
        }
    };
    let chosen = move || {
        pick.chosen
            .get()
            .into_iter()
            .map(|id| {
                let models = modes::models_of(id);
                let name = crate::thread::name(id, &models);
                view! {
                    <span class="picked">
                        {modes::draw(Mode::Full, Mode::Lozenge, id, &models)}
                        <button class="btn subtle tiny" title="Take it out" aria-label=format!("take {name} out")
                            on:click=move |_| pick.chosen.update(|c| c.retain(|x| *x != id))>"×"</button>
                    </span>
                }
            })
            .collect::<Vec<_>>()
    };
    let options = Memo::new(move |_| offered(slot));
    let groups = move || {
        options
            .get()
            .into_iter()
            .map(|(heading, members)| {
                let lines = members
                    .into_iter()
                    .map(|(id, name)| view! { <option value=id.to_base64()>{name}</option> })
                    .collect::<Vec<_>>();
                view! { <optgroup label=heading>{lines}</optgroup> }
            })
            .collect::<Vec<_>>()
    };
    let on_key = move |ev: web_sys::KeyboardEvent| {
        if keys::is_send(&ev) && !ev.ctrl_key() && !ev.meta_key() {
            ev.prevent_default();
            pick.take_typed(slot);
        }
    };
    let typed_label = format!("{}: the start of its id", slot.name);
    view! {
        <div class="form-field picker">
            <span class="field-name">{slot.name}</span>
            <span class="hint">{hint}</span>
            <div class="picked-list">{chosen}</div>
            <div class="pick-row">
                <select node_ref=list on:change=on_list aria-label=format!("{}: choose from the list", slot.name)>
                    <option value="">{choose}</option>
                    {groups}
                </select>
                <input type="text" class="mono" aria-label=typed_label placeholder="or its id's start"
                    prop:value=move || pick.typed.get()
                    on:input=move |ev| {
                        pick.typed.set(event_target_value(&ev));
                        pick.refused.set(None);
                    }
                    on:keydown=on_key
                    on:change=move |_| pick.take_typed(slot)/>
            </div>
            {move || pick.refused.get().map(|why| view! { <span class="error">{why}</span> })}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> EntityId {
        EntityId::from_bytes([n; 32])
    }

    /// Three tasks, a message and a decision, D7; the first two tasks' ids start alike.
    fn known() -> (Known, [EntityId; 5]) {
        let mut a = [7u8; 32];
        a[31] = 1;
        let mut b = [7u8; 32];
        b[31] = 2;
        let (a, b) = (EntityId::from_bytes(a), EntityId::from_bytes(b));
        let (c, message, decision) = (id(3), id(4), id(5));
        let known = Known {
            entities: vec![
                (a, Kind::Task),
                (b, Kind::Task),
                (c, Kind::Task),
                (message, Kind::Message),
                (decision, Kind::Decision),
            ],
            numbers: vec![(decision, 7)],
        };
        (known, [a, b, c, message, decision])
    }

    fn start(id: EntityId) -> String {
        id.to_base64().chars().take(8).collect()
    }

    #[test]
    fn a_task_needs_a_title_and_takes_it_without_the_spaces_around_it() {
        let (known, _) = known();
        for title in ["", "   \n"] {
            let filled = Filled { title: title.into(), why: "a reason".into(), ..Filled::default() };
            assert_eq!(
                spec(&filled, &known, None),
                Err("a task needs a title: what to do, verb first".into()),
                "{title:?}"
            );
        }
        let made = spec(&Filled { title: "  Cap the retry delay ".into(), ..Filled::default() }, &known, None).unwrap();
        assert_eq!(made, CreateTask { title: "Cap the retry delay".into(), ..CreateTask::default() });
    }

    #[test]
    fn every_field_goes_to_the_task_as_fwd_task_create_takes_it() {
        let (known, [a, b, c, message, _]) = known();
        let filled = Filled {
            title: "Cap the retry delay".into(),
            why: "A reconnect should wait longer each time.".into(),
            what: "Double the wait, up to 30 seconds.\n".into(),
            done_when: "The tests show the cap.".into(),
            priority: Some(1),
            labels: "web, retry".into(),
            propose: true,
            needs_review: true,
            part_of: Picked { chosen: vec![c], typed: String::new() },
            waits_on: Picked { chosen: vec![a, a], typed: start(c) },
            came_from: Picked { chosen: vec![], typed: start(message) },
            motivated_by: Picked { chosen: vec![b], typed: String::new() },
        };
        assert_eq!(
            spec(&filled, &known, None),
            Ok(CreateTask {
                title: "Cap the retry delay".into(),
                what: "Double the wait, up to 30 seconds.\n".into(),
                why: "A reconnect should wait longer each time.".into(),
                done_when: "The tests show the cap.".into(),
                propose: true,
                needs_review: true,
                priority: Some(1),
                labels: vec!["web".into(), "retry".into()],
                after: vec![a, c],
                under: Some(c),
                from: Some(message),
                motivated_by: Some(b),
                scope: None,
            })
        );
    }

    /// The task is worked in the scope the header's selector chose, as `fwd task create` makes its
    /// task worked in the scope the command works in (task cToJvD22), and in none while the
    /// selector chooses all scopes or no scope, or before the page has chosen.
    #[test]
    fn the_task_is_worked_in_the_scope_the_header_chose_and_in_none_under_all_or_no_scope() {
        let (known, _) = known();
        let filled = Filled { title: "Cap the retry delay".into(), ..Filled::default() };
        let worked_in = |choice: Option<ScopeChoice>| spec(&filled, &known, choice).map(|t| t.scope);
        let scope = id(6);
        assert_eq!(worked_in(Some(ScopeChoice::Scope(scope))), Ok(Some(scope)));
        for choice in [Some(ScopeChoice::All), Some(ScopeChoice::NoScope), None] {
            assert_eq!(worked_in(choice), Ok(None), "{choice:?}");
        }
    }

    #[test]
    fn a_bad_id_refuses_the_whole_form_and_says_which_picker_and_why() {
        let (known, [a, _, c, message, _]) = known();
        let with = |waits_on: &str| Filled {
            title: "Cap the retry delay".into(),
            waits_on: Picked { chosen: vec![a], typed: waits_on.into() },
            ..Filled::default()
        };
        let refused = |typed: &str| spec(&with(typed), &known, None).unwrap_err();
        assert!(refused("abc").starts_with("waits on: `abc` is not an entity id"), "{}", refused("abc"));
        assert!(refused("zzzzzzz").contains("no task has an id starting with `zzzzzzz`"), "{}", refused("zzzzzzz"));
        let shared: String = a.to_base64().chars().take(6).collect();
        assert!(refused(&shared).contains("starts the ids of 2 entities"), "{}", refused(&shared));
        assert!(refused("D7").contains("names a decision, and this takes a task"), "{}", refused("D7"));
        assert!(refused("D8").contains("names a decision"), "a decision number names a decision first");
        // The whole id of a message is no task, though the command line would take it as it is.
        let whole = message.to_base64();
        assert!(refused(&whole).contains("is not the id of a task this page holds"), "{}", refused(&whole));
        // A name that fits the picker goes through, the chosen ones first.
        assert_eq!(spec(&with(&start(c)), &known, None).map(|t| t.after), Ok(vec![a, c]));
    }

    #[test]
    fn in_a_picker_of_one_the_typed_name_takes_the_place_of_the_choice() {
        let (known, [a, _, c, message, _]) = known();
        let one = Picked { chosen: vec![a], typed: start(c) };
        assert_eq!(picked(&one, PART_OF, &known), Ok(vec![c]));
        assert_eq!(picked(&Picked { chosen: vec![c], typed: " ".into() }, PART_OF, &known), Ok(vec![c]));
        assert_eq!(picked(&Picked { chosen: vec![a], typed: start(message) }, CAME_FROM, &known), Ok(vec![message]));
        assert_eq!(picked(&Picked::default(), CAME_FROM, &known), Ok(vec![]));
    }

    /// A picker takes exactly the kinds of target `fwd task create` accepts for the picker's
    /// relation from the new task (the kinds `ops::ends_refused` lets through), so that nothing it
    /// offers ends in the command's refusal. Neither the came-from nor the motivated-by picker
    /// takes a decision, then (task 5bJv0xvq): a decision's number, the start of its id or its
    /// whole id typed into either is refused, and the refusal names the picker.
    #[test]
    fn a_picker_takes_what_its_relation_joins_a_task_to_so_none_takes_a_decision() {
        use fwd_core::model::RelationKind;
        let every = [
            Kind::Task,
            Kind::Message,
            Kind::Decision,
            Kind::Scope,
            Kind::Anchor,
            Kind::Relation,
            Kind::Notification,
            Kind::Principal,
        ];
        let (new, target) = (id(1), id(2));
        for (slot, relation) in [
            (PART_OF, RelationKind::SubtaskOf),
            (WAITS_ON, RelationKind::DependsOn),
            (CAME_FROM, RelationKind::DiscoveredFrom),
            (MOTIVATED_BY, RelationKind::MotivatedBy),
        ] {
            let accepted: Vec<Kind> = every
                .into_iter()
                .filter(|kind| ops::ends_refused(relation, (new, Some(Kind::Task)), (target, Some(*kind))).is_none())
                .collect();
            assert_eq!(slot.kinds, accepted, "{}", slot.name);
            assert!(!slot.kinds.contains(&Kind::Decision), "{}", slot.name);
        }
        let (known, [.., decision]) = known();
        for slot in [CAME_FROM, MOTIVATED_BY] {
            for (typed, why) in [
                ("D7".to_string(), "names a decision"),
                (start(decision), "no task, message has an id starting with"),
                (decision.to_base64(), "is not the id of a task or message this page holds"),
            ] {
                let refused = picked(&Picked { chosen: vec![], typed: typed.clone() }, slot, &known).unwrap_err();
                assert!(
                    refused.starts_with(&format!("{}: ", slot.name)) && refused.contains(why),
                    "{typed}: {refused}"
                );
            }
        }
    }
}
