//! A scope's panel on the Scopes page, where a person makes a scope or changes one (Daniel,
//! 2026-09-28: "I need to be able to create scopes and tasks myself ... I need to be able to edit
//! metadata for scopes in the scope page"; `DECISIONS.md`, "A scope is created and edited in a side
//! panel opened from an Edit control; its name still leads to the filtered Tasks page"). It is the
//! scope's full mode (`modes.rs`): its name, intention, the repository its branches are in, its
//! branches and its status as fields to change in place, with who made it, when it started and
//! what names it (the tasks worked in it, the messages that mention it). It showed the scope's
//! owner and heartbeat in place of who made it until 2026-09-29, when scopes lost both
//! (docs/DECISIONS.md, D108, "A scope has no owner: scopes are not assigned to principals"): who
//! made a scope is a record, and anyone works in any scope.
//!
//! A field nobody has changed shows what the scope holds now, so a rename from the CLI shows at
//! once; a field a person changed shows what they typed until Save writes it or Revert forgets it.
//! Save sends only the fields that differ from the scope's ([`edit`]) through `ops::edit_scope`,
//! one transaction under the rules the CLI runs (a blank name, and branches without their
//! repository, are refused, and the refusal shows beneath the fields). The same fields, empty and
//! active, make a new scope through `ops::create_scope`, owned by whoever the page acts as; the
//! panel then shows the scope made. What the fields send is worked out without the browser and
//! tested natively.

use crate::empty::{Empty, Nothing};
use crate::fmt::CopyId;
use crate::route::{self, Route, ScopePanel};
use crate::session::{self, ctx};
use crate::{fmt, keys, lozenges, panel, store};
use ankurah::EntityId;
use fwd_core::model::{ScopeStatus, ScopeView};
use fwd_core::ops::{self, CreateScope, EditScope};
use leptos::prelude::*;
use serde_json::Value;
use wasm_bindgen_futures::spawn_local;

/// The statuses a scope can be given, in the order the select lists them. No rule limits how a
/// scope moves between them (`fwd scope status` takes any of them from any), so each is offered.
pub const STATUSES: [ScopeStatus; 6] = [
    ScopeStatus::Planned,
    ScopeStatus::Active,
    ScopeStatus::Paused,
    ScopeStatus::Blocked,
    ScopeStatus::Done,
    ScopeStatus::Abandoned,
];

/// A scope's fields as the panel shows them: text as it is typed, the branches as one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fields {
    pub title: String,
    pub intention: String,
    pub repo: String,
    pub branches: String,
    pub status: ScopeStatus,
}

impl Fields {
    /// A new scope's fields before anyone types in them: active, and in `repo`, the repository
    /// every scope of the project names when they all name the same one.
    pub fn new(repo: Option<String>) -> Fields {
        Fields {
            title: String::new(),
            intention: String::new(),
            repo: repo.unwrap_or_default(),
            branches: String::new(),
            status: ScopeStatus::Active,
        }
    }

    /// A scope's fields as it holds them.
    fn of(s: &ScopeView) -> Fields {
        let context: Value = s.context().map(Value::from).unwrap_or(Value::Null);
        Fields {
            title: s.title().unwrap_or_default(),
            intention: ops::text(s.intention()).unwrap_or_default(),
            repo: context["repo"].as_str().unwrap_or_default().to_string(),
            branches: ops::scope_branches(&context).join(", "),
            status: s.status().unwrap_or(ScopeStatus::Active),
        }
    }
}

/// The branches typed into the field, which takes commas, spaces or new lines between them: each
/// once, in the order typed.
pub fn branches(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for b in text.split(|c: char| c == ',' || c.is_whitespace()).filter(|b| !b.is_empty()) {
        if !out.iter().any(|o| o == b) {
            out.push(b.to_string());
        }
    }
    out
}

/// What Save writes: each field whose text differs from the scope's, and nothing for a field left
/// alone, which keeps whatever the scope holds by the time the edit lands. Spaces around the name
/// and the repository change nothing, nor does how the branches are spaced.
pub fn edit(stored: &Fields, shown: &Fields) -> EditScope {
    let title = shown.title.trim();
    let repo = shown.repo.trim();
    EditScope {
        title: (title != stored.title.trim()).then(|| title.to_string()),
        intention: (shown.intention != stored.intention).then(|| shown.intention.clone()),
        intention_append: None,
        repo: (repo != stored.repo.trim()).then(|| Some(repo.to_string()).filter(|r| !r.is_empty())),
        branches: Some(branches(&shown.branches)).filter(|b| *b != branches(&stored.branches)),
        status: (shown.status != stored.status).then_some(shown.status),
    }
}

/// The scope the fields make.
pub fn create(shown: &Fields) -> CreateScope {
    CreateScope {
        title: shown.title.trim().to_string(),
        intention: shown.intention.clone(),
        repo: Some(shown.repo.trim().to_string()).filter(|r| !r.is_empty()),
        branches: branches(&shown.branches),
        status: shown.status,
    }
}

/// The repositories named in these scope contexts, each once, sorted: what the repository field
/// offers.
pub fn repositories(contexts: impl IntoIterator<Item = Value>) -> Vec<String> {
    let mut out: Vec<String> =
        contexts.into_iter().filter_map(|c| c["repo"].as_str().map(str::to_string)).filter(|r| !r.is_empty()).collect();
    out.sort();
    out.dedup();
    out
}

/// What a person has changed in the panel's fields: each none while it shows what the scope holds.
/// `Copy` handles, so the Scopes page can keep a new scope's changes while its panel shows another
/// scope, and the closures in the view share them.
#[derive(Clone, Copy)]
pub struct Draft {
    title: RwSignal<Option<String>>,
    intention: RwSignal<Option<String>>,
    repo: RwSignal<Option<String>>,
    branches: RwSignal<Option<String>>,
    status: RwSignal<Option<ScopeStatus>>,
}

impl Draft {
    /// No field changed yet.
    pub fn untouched() -> Draft {
        Draft {
            title: RwSignal::new(None),
            intention: RwSignal::new(None),
            repo: RwSignal::new(None),
            branches: RwSignal::new(None),
            status: RwSignal::new(None),
        }
    }

    /// The fields as shown: each change over what `stored` holds.
    fn over(self, stored: &Fields) -> Fields {
        let text = |edit: RwSignal<Option<String>>, stored: &String| edit.get().unwrap_or_else(|| stored.clone());
        Fields {
            title: text(self.title, &stored.title),
            intention: text(self.intention, &stored.intention),
            repo: text(self.repo, &stored.repo),
            branches: text(self.branches, &stored.branches),
            status: self.status.get().unwrap_or(stored.status),
        }
    }

    /// Forgets every change, so each field shows what the scope holds again.
    fn clear(self) {
        self.title.try_set(None);
        self.intention.try_set(None);
        self.repo.try_set(None);
        self.branches.try_set(None);
        self.status.try_set(None);
    }
}

/// The full mode: the scope's panel, its fields to change in place.
pub fn full(id: EntityId) -> AnyView {
    view! { <ScopeForm scope=Some(id) draft=Draft::untouched()/> }.into_any()
}

/// The panel for a new scope, keeping what was typed in `draft`.
pub fn new_scope(draft: Draft) -> AnyView {
    view! { <ScopeForm scope=None draft=draft/> }.into_any()
}

/// The repository every scope of the project names, when they all name the same one.
fn sole_repository() -> Option<String> {
    let mut named = repositories(store::scopes().iter().filter_map(|s| s.context().ok().map(Value::from)));
    (named.len() == 1).then(|| named.remove(0))
}

/// The panel: the scope's fields (a new scope's when `scope` is none), what it is, and what names
/// it.
#[component]
fn ScopeForm(scope: Option<EntityId>, draft: Draft) -> impl IntoView {
    let error = RwSignal::new(None::<String>);
    let saved = RwSignal::new(false);
    let busy = RwSignal::new(false);
    // The fields as the scope holds them (none while it has not arrived), or a new scope's.
    let stored = Memo::new(move |_| match scope {
        Some(id) => store::scope(id).map(|s| Fields::of(&s)),
        None => Some(Fields::new(sole_repository())),
    });
    let shown = Memo::new(move |_| stored.get().map(|stored| draft.over(&stored)));
    let changed = move || match (stored.get(), shown.get()) {
        (Some(stored), Some(shown)) if scope.is_some() => !edit(&stored, &shown).is_empty(),
        (Some(stored), Some(shown)) => stored != shown,
        _ => false,
    };
    let save = move || {
        let (Some(stored), Some(shown)) = (stored.get_untracked(), shown.get_untracked()) else { return };
        // A new scope is made by whoever the page acts as, which it records and nothing more.
        let maker = session::me();
        if busy.get_untracked() || (scope.is_none() && maker.is_none()) {
            return;
        }
        busy.set(true);
        saved.set(false);
        spawn_local(async move {
            let done = match (scope, maker) {
                (Some(id), _) => {
                    let edit = edit(&stored, &shown);
                    if edit.is_empty() {
                        Ok(None)
                    } else {
                        let edited = ops::edit_scope(&ctx(), id, edit).await;
                        if let (Ok(()), Some(me)) = (&edited, maker) {
                            session::wrote(me);
                        }
                        edited.map(|()| None)
                    }
                }
                (None, Some(maker)) => {
                    let made = ops::create_scope(&ctx(), maker, create(&shown)).await;
                    if made.is_ok() {
                        session::wrote(maker);
                    }
                    made.map(Some)
                }
                (None, None) => Ok(None),
            };
            busy.try_set(false);
            match done {
                Ok(made) => {
                    error.try_set(None);
                    draft.clear();
                    match made {
                        // The panel shows the scope made, as the list does.
                        Some(id) => route::go(Route::Scopes(Some(ScopePanel::Edit(id)))),
                        None => {
                            saved.try_set(true);
                            let forget = move || {
                                saved.try_set(false);
                            };
                            set_timeout(forget, std::time::Duration::from_millis(1500));
                        }
                    }
                }
                Err(e) => {
                    error.try_set(Some(e.to_string()));
                }
            }
        });
    };
    // Enter in a one-line field saves, as it sends in the page's other boxes; the intention takes
    // new lines.
    let on_key = move |ev: web_sys::KeyboardEvent| {
        if keys::is_send(&ev) {
            ev.prevent_default();
            save();
        }
    };
    let text_field = move |edit: RwSignal<Option<String>>, field: fn(&Fields) -> String| {
        let value = move || shown.get().map(|f| field(&f)).unwrap_or_default();
        (value, move |ev: leptos::ev::Event| edit.set(Some(event_target_value(&ev))))
    };
    let (title, on_title) = text_field(draft.title, |f| f.title.clone());
    let (intention, on_intention) = text_field(draft.intention, |f| f.intention.clone());
    let (repo, on_repo) = text_field(draft.repo, |f| f.repo.clone());
    let (branch_text, on_branches) = text_field(draft.branches, |f| f.branches.clone());
    // The select shows the status the fields hold: set once its options are drawn, since an
    // option's `selected` attribute stops moving a select once a person has chosen in it.
    let status_select = NodeRef::<leptos::html::Select>::new();
    Effect::new(move |_| {
        let status = shown.get().map(|f| f.status.to_string()).unwrap_or_default();
        if let Some(select) = status_select.get() {
            select.set_value(&status);
        }
    });
    let on_status = move |ev: leptos::ev::Event| {
        if let Some(status) = STATUSES.into_iter().find(|s| s.to_string() == event_target_value(&ev)) {
            draft.status.set(Some(status));
        }
    };
    let repositories_offered = move || {
        repositories(store::scopes().iter().filter_map(|s| s.context().ok().map(Value::from)))
            .into_iter()
            .map(|r| view! { <option value=r.clone()></option> })
            .collect::<Vec<_>>()
    };
    let list_id = format!("scope-repositories-{}", scope.map(|s| s.to_base64()).unwrap_or_else(|| "new".into()));
    let pane = NodeRef::<leptos::html::Div>::new();
    panel::show_beneath_list(pane);
    view! {
        <div class="detail scope-panel" node_ref=pane>
            <button class="btn subtle panel-close" title="Close the panel"
                on:click=move |_| route::go(Route::Scopes(None))>"Close"</button>
            {move || head(scope)}
            {move || (scope.is_some() && stored.get().is_none()).then(|| view! { <Empty nothing=Nothing::Scope/> })}
            <div class="form" hidden=move || stored.get().is_none()>
                <label class="form-field">
                    <span class="field-name">"Name"</span>
                    <input type="text" placeholder="what the work is, for people to read"
                        prop:value=title on:input=on_title on:keydown=on_key/>
                </label>
                <label class="form-field">
                    <span class="field-name">"Intention"</span>
                    <textarea placeholder="what the work in it is trying to achieve"
                        prop:value=intention on:input=on_intention></textarea>
                </label>
                <label class="form-field">
                    <span class="field-name">"Repository"</span>
                    <input type="text" list=list_id.clone() placeholder="github.com/org/repo: where its branches are"
                        prop:value=repo on:input=on_repo on:keydown=on_key/>
                    <datalist id=list_id>{repositories_offered}</datalist>
                </label>
                <label class="form-field">
                    <span class="field-name">"Branches"</span>
                    <input type="text" placeholder="main, feat/inbox: commas or spaces between them"
                        prop:value=branch_text on:input=on_branches on:keydown=on_key/>
                </label>
                <label class="form-field">
                    <span class="field-name">"Status"</span>
                    <select node_ref=status_select on:change=on_status>
                        {STATUSES.into_iter().map(|s| view! { <option value=s.to_string()>{s.to_string()}</option> }).collect::<Vec<_>>()}
                    </select>
                </label>
                <div class="form-actions">
                    <button class="btn primary" disabled=move || busy.get() on:click=move |_| save()>
                        {if scope.is_some() { "Save" } else { "Create scope" }}
                    </button>
                    <button class="btn subtle" disabled=move || !changed() on:click=move |_| {
                        draft.clear();
                        error.set(None);
                    }>{if scope.is_some() { "Revert" } else { "Clear" }}</button>
                    {move || (scope.is_some() && changed()).then(|| view! { <span class="muted">"unsaved changes"</span> })}
                    {move || saved.get().then(|| view! { <span class="saved">"saved"</span> })}
                    {move || error.get().map(|e| view! { <span class="error">{e}</span> })}
                </div>
            </div>
            {scope.map(|id| lozenges::section(id, "References", &[]))}
        </div>
    }
}

/// The panel's heading: the scope's name, short id and status, who made it and when it started,
/// and a link to the tasks worked in it; for a new scope, who will have made it.
fn head(scope: Option<EntityId>) -> AnyView {
    let Some(id) = scope else {
        let maker = session::acting().get().and_then(|p| p.handle().ok()).unwrap_or_default();
        return view! {
            <header class="detail-head">
                <h3>"New scope"</h3>
                <div class="muted">{format!("made by {maker}, whom the page acts as")}</div>
            </header>
        }
        .into_any();
    };
    let Some(s) = store::scope(id) else { return ().into_any() };
    let maker = s.created_by().ok().map(|p| store::handle_of(EntityId::from(p))).unwrap_or_default();
    let started =
        s.started().ok().flatten().map_or_else(|| "not started".to_string(), |t| format!("started {}", fmt::ago(t)));
    view! {
        <header class="detail-head">
            <h3>{s.title().unwrap_or_default()}</h3>
            <div class="pills">
                <CopyId id=id/>
                {s.status().ok().map(|st| view! { <span class=format!("pill scope-{st}")>{st.to_string()}</span> })}
            </div>
            <div class="muted">{format!("made by {maker}, {started}")}</div>
            <a href=Route::scope_tasks(id).hash()>"its tasks"</a>
        </header>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn stored() -> Fields {
        Fields {
            title: "Cloud hosting".into(),
            intention: "Run the daemon for a team".into(),
            repo: "github.com/o/r".into(),
            branches: "main, feat/hosting".into(),
            status: ScopeStatus::Planned,
        }
    }

    #[test]
    fn the_branches_field_takes_commas_spaces_and_new_lines_and_names_each_branch_once() {
        assert_eq!(branches("main, feat/x\nfeat/y  main,,"), vec!["main", "feat/x", "feat/y"]);
        assert!(branches(" , \n").is_empty());
    }

    #[test]
    fn save_writes_only_what_a_person_changed() {
        let stored = stored();
        assert!(edit(&stored, &stored).is_empty(), "nothing changed, nothing to write");
        let respaced =
            Fields { title: " Cloud hosting ".into(), branches: "main feat/hosting".into(), ..stored.clone() };
        assert!(edit(&stored, &respaced).is_empty(), "spaces around the name and between branches change nothing");
        let renamed = Fields { title: "Hosting ".into(), status: ScopeStatus::Active, ..stored.clone() };
        assert_eq!(
            edit(&stored, &renamed),
            EditScope { title: Some("Hosting".into()), status: Some(ScopeStatus::Active), ..EditScope::default() },
            "the fields left alone are not written, so they keep what the scope holds when the edit lands"
        );
        let moved = Fields { repo: " ".into(), branches: String::new(), ..stored.clone() };
        assert_eq!(
            edit(&stored, &moved),
            EditScope { repo: Some(None), branches: Some(vec![]), ..EditScope::default() },
            "a blank repository names none, and emptied branches are none"
        );
        let said = Fields { intention: "Host a team's graph\n".into(), ..stored.clone() };
        assert_eq!(edit(&stored, &said).intention.as_deref(), Some("Host a team's graph\n"), "prose as typed");
    }

    #[test]
    fn a_new_scope_is_made_from_its_fields_as_typed() {
        let typed = Fields {
            title: " Reviews ".into(),
            intention: "Highlights and their threads".into(),
            repo: "  ".into(),
            branches: String::new(),
            status: ScopeStatus::Planned,
        };
        assert_eq!(
            create(&typed),
            CreateScope {
                title: "Reviews".into(),
                intention: "Highlights and their threads".into(),
                repo: None,
                branches: vec![],
                status: ScopeStatus::Planned,
            }
        );
        let placed = Fields { repo: "github.com/o/r".into(), branches: "main,feat/x".into(), ..typed };
        assert_eq!(create(&placed).repo.as_deref(), Some("github.com/o/r"));
        assert_eq!(create(&placed).branches, vec!["main", "feat/x"]);
    }

    #[test]
    fn a_new_scope_starts_active_in_the_repository_the_project_names() {
        let contexts = vec![
            json!({"repo": "github.com/o/r", "branches": ["main"]}),
            json!({"repo": null, "branches": []}),
            json!({}),
            json!({"repo": "github.com/o/r"}),
        ];
        assert_eq!(repositories(contexts.clone()), vec!["github.com/o/r"]);
        let fresh = Fields::new(Some("github.com/o/r".into()));
        assert_eq!(
            (fresh.status, fresh.repo.as_str(), fresh.title.as_str()),
            (ScopeStatus::Active, "github.com/o/r", "")
        );
        let two = [contexts, vec![json!({"repo": "github.com/o/s"})]].concat();
        assert_eq!(repositories(two), vec!["github.com/o/r", "github.com/o/s"], "each once, sorted");
    }

    #[test]
    fn the_status_select_reads_back_each_status_it_offers() {
        for status in STATUSES {
            assert_eq!(STATUSES.iter().filter(|s| **s == status).count(), 1, "{status} is offered once");
            assert_eq!(status.to_string().parse::<ScopeStatus>().ok(), Some(status), "{status} reads back");
        }
    }
}
