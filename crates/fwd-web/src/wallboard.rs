//! The wallboard: a dedicated display of one configured project's graph, with no routes, actions or
//! editors (docs/design/wallboard.md).
// main.rs lets what only the app uses go unused in the wallboard's build; the wallboard's own code may not.
#![warn(dead_code)]

use crate::{graph, session, store};
use ankurah::signals::Get as AnkurahGet;
use ankurah::{EntityId, LiveQuery};
use fwd_core::model::{RelationView, TaskStatus, TaskView};
use leptos::prelude::*;
use std::collections::{HashMap, HashSet};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};

mod settings;
pub use settings::{Presentation, Settings, TaskSelection};

/// What the wallboard reads beside its tasks and their relations: the scopes, the assignees' handles
/// and the counts of messages its cards show. It shows no anchors or decisions, so it opens none.
const READS: [store::Collection; 3] =
    [store::Collection::Scopes, store::Collection::Principals, store::Collection::Messages];

/// Watches only prerequisites of visible tasks, never the completed-task backlog.
/// Keeping a query until its last dependent leaves also follows a completed task being reopened.
#[derive(Default)]
pub struct Dependencies {
    queries: HashMap<EntityId, Option<LiveQuery<TaskView>>>,
}

impl Dependencies {
    pub fn statuses(&mut self, visible: &[TaskView], relations: &[RelationView]) -> HashMap<EntityId, TaskStatus> {
        let ids: HashSet<_> = visible.iter().map(|task| task.id()).collect();
        let edges: Vec<_> = relations
            .iter()
            .filter_map(|r| {
                Some(fwd_core::rules::Edge {
                    id: r.id(),
                    from: r.from_id().ok()?,
                    to: r.to_id().ok()?,
                    kind: r.kind().ok()?,
                    status: r.status().ok()?,
                })
            })
            .collect();
        let wanted = prerequisite_ids(&ids, &edges);
        self.queries.retain(|id, _| wanted.contains(id));
        for id in wanted {
            self.queries.entry(id).or_insert_with(|| {
                session::ctx()
                    .query::<TaskView>(format!("id = '{}'", id.to_base64()).as_str())
                    .map_err(|error| tracing::warn!("Prerequisite query refused: {error}"))
                    .ok()
            });
        }
        let mut statuses: HashMap<_, _> = self
            .queries
            .values()
            .flatten()
            .flat_map(|query| query.get())
            .filter_map(|task| Some((task.id(), task.status().ok()?)))
            .collect();
        // Both sources hold the same entities; visible tasks need not wait for a second query.
        statuses.extend(visible.iter().filter_map(|task| Some((task.id(), task.status().ok()?))));
        statuses
    }
}

fn prerequisite_ids(visible: &HashSet<EntityId>, edges: &[fwd_core::rules::Edge]) -> HashSet<EntityId> {
    use fwd_core::model::{RelationKind, RelationStatus};
    edges
        .iter()
        .filter(|edge| {
            visible.contains(&edge.from)
                && edge.kind == RelationKind::DependsOn
                && edge.status == RelationStatus::Accepted
        })
        .map(|edge| edge.to)
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Availability {
    pub held: bool,
    pub blocked: bool,
    pub dependency_issue: bool,
    pub unknown: bool,
}

impl Availability {
    pub fn of(id: EntityId, hold: Option<String>, dependencies: &[(EntityId, Option<TaskStatus>)]) -> Self {
        use fwd_core::rules::{self, NotReadyReason, Readiness, TaskSnapshot};
        let mut availability = Self::default();
        // Workflow status is displayed separately, and a task's own date decides whether it is drawn
        // at all. Here only holds and prerequisites matter, so no date is given to judge, and the
        // present that would judge one goes unread.
        let card = TaskSnapshot { id, status: TaskStatus::Open, hold, not_before: None };
        match rules::readiness(&card, dependencies, 0) {
            Readiness::Ready => {}
            Readiness::Unknown(_) => availability.unknown = true,
            Readiness::NotReady(reasons) => {
                for reason in reasons {
                    match reason {
                        NotReadyReason::Held(_) => availability.held = true,
                        NotReadyReason::BlockedBy(_) => availability.blocked = true,
                        NotReadyReason::DependencyClosed(_, _) => availability.dependency_issue = true,
                        NotReadyReason::Status(_) | NotReadyReason::NotYetDue(_) => {}
                    }
                }
            }
        }
        availability
    }
    pub fn quiet(&self) -> bool {
        self.held || self.blocked
    }
    pub fn labels(&self) -> Vec<&'static str> {
        [
            (self.held, "On hold"),
            (self.blocked, "Blocked"),
            (self.dependency_issue, "Check dependency"),
            (self.unknown, "Dependency unknown"),
        ]
        .into_iter()
        .filter_map(|(show, label)| show.then_some(label))
        .collect()
    }
}

/// Loads the project and settings that the wallboard's own server generates from its
/// wallboard.json. Loading them never reaches the daemon, so a cold start gets this far while no
/// daemon answers.
async fn load_configuration() -> Result<(session::Project, Settings), String> {
    let window = web_sys::window().ok_or("No browser window")?;
    let response = JsFuture::from(window.fetch_with_str("/wallboard-config.json"))
        .await
        .map_err(|_| "Waiting for local wallboard configuration")?
        .dyn_into::<web_sys::Response>()
        .map_err(|_| "Invalid wallboard response")?;
    if !response.ok() {
        return Err("Waiting for local wallboard configuration".into());
    }
    let text = JsFuture::from(response.text().map_err(|_| "Unreadable wallboard configuration")?)
        .await
        .ok()
        .and_then(|text| text.as_string())
        .ok_or("Unreadable wallboard configuration")?;
    let configuration: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Invalid wallboard configuration: {e}"))?;
    let id = configuration["project_id"].as_str().filter(|id| !id.is_empty()).ok_or("Missing project_id")?;
    Ok((
        session::Project {
            id: id.into(),
            name: configuration["project_name"].as_str().unwrap_or("Graph").into(),
            error: None,
        },
        Settings::parse(&configuration)?,
    ))
}

#[wasm_bindgen::prelude::wasm_bindgen(module = "/src/wallboard/wallboard-theme.mjs")]
extern "C" {
    #[wasm_bindgen(js_name = configureTheme)]
    fn configure_theme(settings: &str);
}

#[wasm_bindgen::prelude::wasm_bindgen(module = "/src/wallboard/wallboard-diagnostics.mjs")]
extern "C" {
    #[wasm_bindgen(js_name = markBoot)]
    fn mark_boot(stage: &str);
}

#[component]
pub fn Wallboard() -> impl IntoView {
    let started = RwSignal::new(false);
    // Set once, when the store opens: the task selection its query binds for the page's lifetime.
    let opened = RwSignal::new(None::<TaskSelection>);
    let error = RwSignal::new(None::<String>);
    let presentation = RwSignal::new(Presentation::default());
    spawn_local(async move {
        mark_boot("mounted");
        let (project, settings) = loop {
            match load_configuration().await {
                Ok(configuration) => break configuration,
                Err(why) => {
                    error.set(Some(why));
                    session::sleep_ms(3_000).await;
                }
            }
        };
        mark_boot("configuration");
        configure_theme(&settings.json().to_string());
        presentation.set(settings.presentation);
        error.set(None);
        if let Err(why) = session::wallboard_node(project).await {
            error.set(Some(why));
            return;
        }
        mark_boot("node");
        started.set(true);
        let predicate = settings.selection.predicate();
        if let Err(why) = session::wallboard_context().await.and_then(|_| store::open_with(&predicate, &READS)) {
            error.set(Some(why));
            return;
        }
        mark_boot("context");
        opened.set(Some(settings.selection));
    });
    let ready = move || opened.with(Option::is_some);
    let connected = move || started.get() && session::client().connection_state().get().to_string() == "Connected";
    view! {
        <main class="wallboard" data-theme="dark-aubergine" data-ready=move || ready().to_string()>
            {move || opened.get().map(|selection| view! {
                <graph::WallboardGraph presentation=presentation selection=selection/>
            })}
            {move || (!ready()).then(|| view! {
                <p class="wallboard-wait">{move || error.get().unwrap_or_else(|| "Waiting for graph…".into())}</p>
            })}
            <span class="wallboard-connection" class:offline=move || !connected()
                role="status" aria-live="polite" data-connected=move || connected().to_string()>
                {move || if connected() { "Live" } else if ready() { "Offline · retrying" } else { "Connecting…" }}
            </span>
        </main>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prerequisite_watch_set_is_only_accepted_dependencies_of_visible_cards() {
        use fwd_core::model::{RelationKind, RelationStatus};
        use fwd_core::rules::Edge;
        let id = |n| EntityId::from_bytes([n; 32]);
        let dependency = Edge {
            id: id(10),
            from: id(1),
            to: id(2),
            kind: RelationKind::DependsOn,
            status: RelationStatus::Accepted,
        };
        let edges = [
            dependency.clone(),
            Edge { from: id(3), to: id(4), ..dependency.clone() },
            Edge { to: id(5), status: RelationStatus::Proposed, ..dependency.clone() },
            Edge { to: id(6), status: RelationStatus::Retracted, ..dependency.clone() },
            Edge { to: id(7), kind: RelationKind::SubtaskOf, ..dependency },
        ];
        // Its prerequisite need not be drawn itself: a task not yet due is not, and still blocks.
        assert_eq!(prerequisite_ids(&[id(1)].into(), &edges), [id(2)].into());
        assert_eq!(prerequisite_ids(&[id(3)].into(), &edges), [id(4)].into());
        assert!(prerequisite_ids(&Default::default(), &edges).is_empty());
    }

    #[test]
    fn availability_distinguishes_holds_blockers_closed_and_missing_prerequisites() {
        let id = EntityId::from_bytes([1; 32]);
        let prerequisite = EntityId::from_bytes([2; 32]);
        assert_eq!(Availability::of(id, None, &[]), Availability::default());
        assert_eq!(Availability::of(id, None, &[(prerequisite, Some(TaskStatus::Done))]), Availability::default());
        for status in [TaskStatus::Proposed, TaskStatus::Open, TaskStatus::InProgress, TaskStatus::InReview] {
            assert_eq!(Availability::of(id, None, &[(prerequisite, Some(status))]).labels(), ["Blocked"]);
        }
        let both =
            Availability::of(id, Some("Waiting for a decision".into()), &[(prerequisite, Some(TaskStatus::Open))]);
        assert_eq!(both.labels(), ["On hold", "Blocked"]);
        assert!(both.quiet());
        for status in [TaskStatus::Cancelled, TaskStatus::Rejected] {
            let issue = Availability::of(id, None, &[(prerequisite, Some(status))]);
            assert_eq!(issue.labels(), ["Check dependency"]);
            assert!(!issue.quiet());
        }
        let unknown = Availability::of(id, None, &[(prerequisite, None)]);
        assert_eq!(unknown.labels(), ["Dependency unknown"]);
        assert!(!unknown.quiet());
    }
}
