//! A task's labels on the page, so that a person sees what kind of work each task is (a bug, a
//! feature) and finds the tasks of one kind (docs/DECISIONS.md, D97, "A task's kind, such as bug
//! or feature, is a label, not a task type"). They show as small pills after a task's title
//! wherever the task is drawn: its line in the Tasks list, its row, its list item and its
//! lozenge's preview. The Tasks page filters by one ([`LabelFilter`]), the task's panel adds and
//! takes them off ([`TaskLabels`]), and the form for a new task takes them. Every box that takes
//! labels takes several with commas between them, as `fwd task create --label bug,ui` does, and
//! suggests `bug` and `feature` first, then the other labels in use. `fwd_core::labels` says what
//! a label is. What the page offers, and in what order, is worked out as data first and tested
//! natively.

use crate::session::{self, ctx};
use crate::{fmt, keys, store};
use ankurah::EntityId;
use fwd_core::labels::{self as rule, CONVENTIONAL};
use fwd_core::model::TaskView;
use fwd_core::ops;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

/// A task's labels, as stored; none while they cannot be read.
pub fn of(task: &TaskView) -> Vec<String> {
    ops::labels_of(task).unwrap_or_default()
}

/// The labels typed in one box, commas between them: each in its stored form and once, in the
/// order typed. The blank a stray comma leaves is passed over.
pub fn typed(text: &str) -> Vec<String> {
    let mut labels: Vec<String> = Vec::new();
    for label in text.split(',').filter_map(|piece| rule::label(piece).ok()) {
        if !rule::has(&labels, &label) {
            labels.push(label);
        }
    }
    labels
}

/// Where a label sorts in what the page offers: `bug`, then `feature`, then the rest alphabetical.
fn rank(label: &str) -> (usize, &str) {
    (CONVENTIONAL.iter().position(|c| *c == label).unwrap_or(CONVENTIONAL.len()), label)
}

/// The labels among `labels`, each once in its stored form, `bug` and `feature` first and the
/// rest alphabetical. One that could not be written now (blank, or holding a comma, stored before
/// labels had rules) is left out.
pub fn in_order(labels: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for label in labels.into_iter().filter_map(|l| rule::label(&l).ok()) {
        if !found.contains(&label) {
            found.push(label);
        }
    }
    found.sort_by(|a, b| rank(a).cmp(&rank(b)));
    found
}

/// Every label a task of the project carries, in the order the page offers them.
pub fn in_use() -> Vec<String> {
    in_order(store::tasks().iter().flat_map(of))
}

/// What a box that takes labels suggests while it holds `text`: `bug` and `feature`, then the other
/// labels `in_use`, less those the task `has` and those the box holds already. Each is written
/// after whatever the box holds before the label being typed, the space after its last comma
/// included, so that the browser matches it against what is typed and choosing it keeps the labels
/// typed before it.
pub fn suggestions(text: &str, in_use: &[String], has: &[String]) -> Vec<String> {
    let piece = text.rfind(',').map_or(0, |comma| comma + 1);
    let start = text.len() - text[piece..].trim_start().len();
    let (before, _) = text.split_at(start);
    let typed_before = typed(before);
    in_order(CONVENTIONAL.iter().map(|c| c.to_string()).chain(in_use.iter().cloned()))
        .into_iter()
        .filter(|label| !rule::has(has, label) && !rule::has(&typed_before, label))
        .map(|label| format!("{before}{label}"))
        .collect()
}

/// Whether the Tasks page's label filter keeps a task with `labels`: every task while no label is
/// chosen, else the tasks with the label chosen, however they wrote it.
pub fn keeps(chosen: Option<&str>, labels: &[String]) -> bool {
    chosen.is_none_or(|label| rule::has(labels, label))
}

/// The labels as small pills, a space before each, for after a task's title.
pub fn pills(labels: Vec<String>) -> impl IntoView {
    labels.into_iter().map(|label| view! { " " <span class="pill label">{label}</span> }).collect::<Vec<_>>()
}

/// A `<datalist>` of `suggested`, for the box whose `list` names `id`.
pub fn datalist(id: String, suggested: impl Fn() -> Vec<String> + Send + Sync + 'static) -> impl IntoView {
    view! {
        <datalist id=id>
            {move || suggested().into_iter().map(|s| view! { <option value=s></option> }).collect::<Vec<_>>()}
        </datalist>
    }
}

/// The Tasks page's label filter: "all labels" first, then each label in use, `bug` and `feature`
/// first. `chosen` is the label chosen, none for all.
#[component]
pub fn LabelFilter(chosen: RwSignal<Option<String>>) -> impl IntoView {
    let choices = Memo::new(move |_| in_use());
    let options = move || {
        let mut labels = choices.get();
        // A label no task has any more stays chosen, and shows so, until another is chosen.
        if let Some(current) = chosen.get().filter(|c| !labels.contains(c)) {
            labels.push(current);
        }
        labels
            .into_iter()
            .map(|label| {
                let value = label.clone();
                view! { <option value=value>{label}</option> }
            })
            .collect::<Vec<_>>()
    };
    // The select shows the label chosen, set once its options are drawn again, since a select
    // whose options are replaced shows its first one.
    let select = NodeRef::<leptos::html::Select>::new();
    Effect::new(move |_| {
        let value = chosen.get().unwrap_or_default();
        choices.track();
        if let Some(select) = select.get() {
            select.set_value(&value);
        }
    });
    view! {
        <label class="field label-filter" title="only the tasks with this label">
            "label "
            <select node_ref=select
                on:change=move |ev| chosen.set(Some(event_target_value(&ev)).filter(|l| !l.is_empty()))>
                <option value="">"all labels"</option>
                {options}
            </select>
        </label>
    }
}

/// A task's labels on its panel, beside the words "labels": each as a pill with a button that
/// takes it off, then a box that adds one, or several with commas between them, when Enter is
/// pressed or the box is left (as a picker's box on the form for a new task chooses), suggesting
/// `bug` and `feature` first, then the other labels in use, less those the task has. The writes
/// are `ops::label_task` and `ops::unlabel_task`, which change a task's labels by the rule
/// `fwd task amend --label` and `--unlabel` follow, and a refusal shows beside the box.
#[component]
pub fn TaskLabels(task: EntityId) -> impl IntoView {
    let text = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    // Nothing to say about a task that has not arrived yet.
    let loaded = Memo::new(move |_| store::task(task).is_some());
    let labels = Memo::new(move |_| store::task(task).map(|t| of(&t)).unwrap_or_default());
    let add = move || {
        let wanted = typed(&text.get_untracked());
        text.set(String::new());
        if wanted.is_empty() {
            return;
        }
        // Whoever the page acts as labels the task, though `ops::label_task` takes no principal.
        let me = session::me();
        spawn_local(async move {
            for label in wanted {
                if let Err(e) = ops::label_task(&ctx(), task, &label).await {
                    error.try_set(Some(e.to_string()));
                    return;
                }
                me.into_iter().for_each(session::wrote);
            }
            error.try_set(None);
        });
    };
    let take_off = move |label: String| {
        let me = session::me();
        spawn_local(async move {
            match ops::unlabel_task(&ctx(), task, &label).await {
                Ok(()) => {
                    me.into_iter().for_each(session::wrote);
                    error.try_set(None)
                }
                Err(e) => error.try_set(Some(e.to_string())),
            };
        });
    };
    let shown = move || {
        labels
            .get()
            .into_iter()
            .map(|label| {
                let off = label.clone();
                let name = format!("take the label {label} off");
                view! {
                    <span class="pill label">
                        {label}
                        <button class="unlabel" title="Take it off" aria-label=name on:click=move |_| take_off(off.clone())>
                            "×"
                        </button>
                    </span>
                }
            })
            .collect::<Vec<_>>()
    };
    let list = format!("labels-for-{}", fmt::short(task));
    let suggested = move || suggestions(&text.get(), &in_use(), &labels.get());
    view! {
        <div class="task-labels" hidden=move || !loaded.get()>
            <span class="field-label">"labels"</span>
            {shown}
            <input type="text" list=list.clone() aria-label="add a label" placeholder="add a label: bug, feature…"
                prop:value=move || text.get()
                on:input=move |ev| text.set(event_target_value(&ev))
                on:keydown=move |ev| {
                    if keys::is_send(&ev) {
                        ev.prevent_default();
                        add();
                    }
                }
                on:change=move |_| add()/>
            {datalist(list, suggested)}
            {move || error.get().map(|e| view! { <span class="error">{e}</span> })}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(labels: &[&str]) -> Vec<String> {
        labels.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn a_box_takes_labels_with_commas_between_them_each_once_in_the_stored_form() {
        assert_eq!(typed(" web, CLI ,, web,Page Forms"), strings(&["web", "cli", "page forms"]));
        assert_eq!(typed("Bug"), strings(&["bug"]));
        assert!(typed(" , ").is_empty());
    }

    #[test]
    fn bug_and_feature_come_first_then_the_rest_alphabetical_each_once() {
        let labels = strings(&["web", "Feature", "api", "bug", "WEB", " ", "a,b"]);
        assert_eq!(in_order(labels), strings(&["bug", "feature", "api", "web"]));
        assert_eq!(in_order(strings(&["ui", "feature"])), strings(&["feature", "ui"]));
    }

    #[test]
    fn a_box_suggests_bug_and_feature_first_less_what_the_task_has() {
        let in_use = strings(&["ui", "feature", "api"]);
        assert_eq!(suggestions("", &in_use, &[]), strings(&["bug", "feature", "api", "ui"]));
        assert_eq!(suggestions("fe", &in_use, &[]), strings(&["bug", "feature", "api", "ui"]), "the browser matches");
        assert_eq!(suggestions("", &in_use, &strings(&["Bug", "api"])), strings(&["feature", "ui"]));
        assert_eq!(suggestions("", &[], &[]), strings(&["bug", "feature"]), "bug and feature, whatever is in use");
    }

    #[test]
    fn a_suggestion_keeps_the_labels_typed_before_it() {
        let in_use = strings(&["ui"]);
        assert_eq!(suggestions("bug, fe", &in_use, &[]), strings(&["bug, feature", "bug, ui"]));
        assert_eq!(suggestions("ui,bu", &in_use, &[]), strings(&["ui,bug", "ui,feature"]));
        assert_eq!(suggestions("Bug, ui,  ", &in_use, &[]), strings(&["Bug, ui,  feature"]));
    }

    #[test]
    fn the_filter_keeps_every_task_or_those_with_the_label_chosen() {
        let labels = strings(&["Bug", "ui"]);
        assert!(keeps(None, &labels) && keeps(None, &[]));
        assert!(keeps(Some("bug"), &labels) && keeps(Some("ui"), &labels));
        assert!(!keeps(Some("feature"), &labels) && !keeps(Some("bug"), &[]));
    }
}
