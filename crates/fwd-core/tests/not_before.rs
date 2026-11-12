//! A graph written before `Task::not_before` existed opens with no migration: on an in-process durable
//! node, a task written without the property reads as a task with no date.

use ankurah::property::{Json, Ref};
use ankurah::{policy::DEFAULT_CONTEXT, Context, Node, PermissiveAgent};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::model::{PrincipalKind, TaskStatus, TaskView};
use fwd_core::ops;
use fwd_core::project_graph::ProjectGraph;
use fwd_core::rules::{NotReadyReason, Readiness};
use serde_json::{json, Value};
use std::sync::Arc;

/// A project alone in a store in memory: the node's context, through which the test writes a task
/// as an older build wrote it, and the project's graph.
async fn older_project() -> Result<(Context, ProjectGraph)> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    let ctx = node.context_async(DEFAULT_CONTEXT).await?;
    let graph = ProjectGraph::ensure(ctx.clone(), uuid::Uuid::new_v4(), "test").await?;
    Ok((ctx, graph))
}

/// `Task` as it stood before the field, with the project it names, under the task collection's own
/// name, so that the test can write what a graph from then holds.
mod before_the_field {
    use ankurah::property::{Json, Ref};
    use ankurah::Model;
    use fwd_core::model::{Principal, Project, TaskStatus};
    use serde::{Deserialize, Serialize};

    #[derive(Model, Debug, Clone, Serialize, Deserialize)]
    pub struct Task {
        pub project: Ref<Project>,
        #[active_type(LWW)]
        pub title: String,
        #[active_type(YrsString)]
        pub what: String,
        #[active_type(YrsString)]
        pub why: String,
        #[active_type(YrsString)]
        pub done_when: String,
        #[active_type(LWW)]
        pub status: TaskStatus,
        pub assignee: Option<Ref<Principal>>,
        pub needs_review: bool,
        #[active_type(LWW)]
        pub hold: Option<String>,
        #[active_type(LWW)]
        pub priority: Option<i64>,
        pub labels: Json,
        pub context: Json,
        pub created_by: Ref<Principal>,
        pub created: i64,
        pub updated: i64,
        #[active_type(LWW)]
        pub closed: Option<i64>,
        #[active_type(LWW)]
        pub closed_reason: Option<String>,
    }
}

/// Such a task has no date, is ready as it was, and takes a date like any other.
#[tokio::test]
async fn a_task_written_before_the_field_existed_reads_as_one_with_no_date() -> Result<()> {
    let (ctx, graph) = older_project().await?;
    let tester = ops::ensure_principal(&graph, "tester", "Tester", PrincipalKind::Human, None).await?;
    let now = fwd_core::now_ms();
    let trx = ctx.begin();
    let old = trx
        .create(&before_the_field::Task {
            project: graph.project(),
            title: "Written before the field".into(),
            what: String::new(),
            why: String::new(),
            done_when: String::new(),
            status: TaskStatus::Open,
            assignee: None,
            needs_review: false,
            hold: None,
            priority: None,
            labels: Json::new(json!([])),
            context: Json::new(json!({})),
            created_by: Ref::new(tester),
            created: now,
            updated: now,
            closed: None,
            closed_reason: None,
        })
        .await?
        .id();
    trx.commit().await?;

    let task: TaskView = graph.get(old).await?;
    assert_eq!(task.not_before()?, None);
    assert_eq!(ops::json_of_task(&task, None)?.get("not_before"), Some(&Value::Null));
    assert!(ops::readiness(&graph, &task).await?.is_ready());
    let ready = ops::ready_tasks(&graph, None).await?;
    assert_eq!(ready.iter().map(|t| t.id()).collect::<Vec<_>>(), vec![old], "it is on the frontier as it was");

    let ahead = now + 3_600_000;
    ops::amend(&graph, old, ops::Amend { not_before: Some(Some(ahead)), ..Default::default() }).await?;
    assert_eq!(ops::readiness(&graph, &task).await?, Readiness::NotReady(vec![NotReadyReason::NotYetDue(ahead)]));
    assert!(ops::ready_tasks(&graph, None).await?.is_empty(), "it leaves the frontier until then");
    Ok(())
}
