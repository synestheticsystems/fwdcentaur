//! The nine models' records, made anew from the values an old store holds: every field as it was,
//! with the project named and every id of a record of the same project replaced by the id that
//! record has now. Text is carried as typed; an id that names no record of the project is carried
//! as it was and reported.

use std::collections::HashMap;
use std::str::FromStr;

use ankurah::property::Json;
use ankurah::{EntityId, Ref};
use fwd_core::anchoring::anchor_key;
use fwd_core::model::*;
use fwd_core::project_graph::{ProjectGraph, ProjectTransaction};

use crate::old_store::{Record, Stored};

/// The nine models, in the order their records are made: each after the models its required
/// references name, so that only the references listed in [`patch`] can name a record not made
/// yet. Principals come before scopes, which name their maker, so a triage agent's `serves` waits
/// for its scopes; decisions come before messages, which comment on them, so a decision's
/// `closed_reason_message` waits for its message.
pub const MODELS: [&str; 9] =
    ["Principal", "Repo", "Scope", "Task", "CodeAnchor", "Decision", "Message", "Relation", "Notification"];

/// The fields of each model, but the project, as the old stores name its properties.
pub fn fields_of(model: &str) -> &'static [&'static str] {
    match model {
        "Principal" => &[
            "kind",
            "handle",
            "display_name",
            "auth_subject",
            "agent_role",
            "active",
            "member",
            "subject",
            "succeeded_by",
            "serves",
            "last_active",
        ],
        "Repo" => &["name", "remote_urls", "default_branch"],
        "Task" => &[
            "title",
            "what",
            "why",
            "done_when",
            "status",
            "assignee",
            "needs_review",
            "hold",
            "not_before",
            "priority",
            "labels",
            "context",
            "created_by",
            "created",
            "updated",
            "closed",
            "closed_reason",
        ],
        "Relation" => &[
            "kind",
            "from_id",
            "to_id",
            "status",
            "created_by",
            "created",
            "via",
            "note",
            "position",
            "closed_by",
            "closed",
            "closed_reason",
        ],
        "CodeAnchor" => &[
            "repo",
            "path",
            "anchor_key",
            "span_key",
            "basis_blob",
            "head_blob",
            "index_blob",
            "nearest_blob",
            "basis_patch",
            "blob_sha",
            "commit_sha",
            "side",
            "start_line",
            "end_line",
            "whole_file",
            "anchored_text",
            "context_before",
            "context_after",
            "pieces",
            "symbol_path",
            "created_by",
            "created",
        ],
        "Message" => &[
            "author",
            "created",
            "content",
            "priority",
            "status",
            "task",
            "anchor",
            "scope",
            "decision",
            "reply_to",
            "thread",
            "context",
            "resolved_by",
            "resolved",
            "edited",
            "edited_by",
        ],
        "Notification" => {
            &["recipient", "notification_type", "status", "priority", "created", "updated", "target", "originator"]
        }
        "Scope" => &["status", "title", "intention", "context", "started", "ended", "created_by", "created"],
        "Decision" => &[
            "number",
            "title",
            "status",
            "area",
            "scope",
            "context",
            "decision",
            "consequences",
            "author",
            "decided_by",
            "created",
            "decided",
            "closed_by",
            "closed",
            "closed_reason",
            "closed_reason_message",
        ],
        _ => &[],
    }
}

/// The collaborative-text fields, which the engine leaves out of a materialization while empty.
const TEXTS: [(&str, &str); 8] = [
    ("Task", "what"),
    ("Task", "why"),
    ("Task", "done_when"),
    ("Message", "content"),
    ("Scope", "intention"),
    ("Decision", "context"),
    ("Decision", "decision"),
    ("Decision", "consequences"),
];

/// Why a record cannot be made as it was.
#[derive(Debug, thiserror::Error)]
#[error("{model} {id}: {reason}")]
pub struct Unrepresentable {
    pub model: &'static str,
    pub id: String,
    pub reason: String,
}

/// Every record of one project by its old id: its model and, once made, its new id.
#[derive(Default)]
pub struct Ids {
    by_old: HashMap<String, (&'static str, Option<EntityId>)>,
}

impl Ids {
    pub fn add(&mut self, model: &'static str, old: &str) {
        self.by_old.insert(old.to_owned(), (model, None));
    }
    pub fn made(&mut self, old: &str, new: EntityId) {
        if let Some(slot) = self.by_old.get_mut(old) {
            slot.1 = Some(new);
        }
    }
    pub fn new_of(&self, old: &str) -> Option<EntityId> {
        self.by_old.get(old).and_then(|(_, new)| *new)
    }
    pub fn knows(&self, old: &str) -> bool {
        self.by_old.contains_key(old)
    }
}

/// An id-shaped string (an entity id's base64 text) that names no record of the project.
#[derive(Debug, Clone)]
pub struct Dangling {
    pub model: &'static str,
    pub field: &'static str,
}

/// Reads one record's fields, re-pointing its ids, noting the references to records not made yet,
/// which [`patch`] sets once they are, and the ids that name nothing.
pub struct Fields<'a> {
    model: &'static str,
    record: &'a Record,
    ids: &'a Ids,
    placeholder: EntityId,
    pub waiting: Vec<&'static str>,
    pub dangling: Vec<Dangling>,
}

impl<'a> Fields<'a> {
    pub fn new(model: &'static str, record: &'a Record, ids: &'a Ids, placeholder: EntityId) -> Self {
        Fields { model, record, ids, placeholder, waiting: Vec::new(), dangling: Vec::new() }
    }

    fn refuse(&self, reason: String) -> Unrepresentable {
        Unrepresentable { model: self.model, id: self.record.id.clone(), reason }
    }

    fn opt_text(&self, field: &'static str) -> Result<Option<String>, Unrepresentable> {
        match self.record.get(field) {
            Stored::Null if TEXTS.contains(&(self.model, field)) => Ok(Some(String::new())),
            Stored::Null => Ok(None),
            Stored::Text(text) => Ok(Some(text.clone())),
            other => Err(self.refuse(format!("{field} holds {other:?}, not text"))),
        }
    }
    fn text(&self, field: &'static str) -> Result<String, Unrepresentable> {
        self.opt_text(field)?.ok_or_else(|| self.refuse(format!("{field}, which every record has, is empty")))
    }
    fn opt_int(&self, field: &'static str) -> Result<Option<i64>, Unrepresentable> {
        match self.record.get(field) {
            Stored::Null => Ok(None),
            Stored::Integer(n) => Ok(Some(*n)),
            other => Err(self.refuse(format!("{field} holds {other:?}, not an integer"))),
        }
    }
    fn int(&self, field: &'static str) -> Result<i64, Unrepresentable> {
        self.opt_int(field)?.ok_or_else(|| self.refuse(format!("{field}, which every record has, is empty")))
    }
    fn opt_bool(&self, field: &'static str) -> Result<Option<bool>, Unrepresentable> {
        match self.opt_int(field)? {
            None => Ok(None),
            Some(0) => Ok(Some(false)),
            Some(1) => Ok(Some(true)),
            Some(n) => Err(self.refuse(format!("{field} holds {n}, not a boolean"))),
        }
    }
    fn boolean(&self, field: &'static str) -> Result<bool, Unrepresentable> {
        self.opt_bool(field)?.ok_or_else(|| self.refuse(format!("{field}, which every record has, is empty")))
    }
    fn opt_word<E: FromStr>(&self, field: &'static str) -> Result<Option<E>, Unrepresentable> {
        match self.opt_text(field)? {
            None => Ok(None),
            Some(word) => E::from_str(&word)
                .map(Some)
                .map_err(|_| self.refuse(format!("{field} holds {word:?}, which this build does not know"))),
        }
    }
    fn word<E: FromStr>(&self, field: &'static str) -> Result<E, Unrepresentable> {
        self.opt_word(field)?.ok_or_else(|| self.refuse(format!("{field}, which every record has, is empty")))
    }

    /// The id a reference holds now: the new id of the record it named, or, for a record not made
    /// yet, none (the field waits); or the old id unchanged when it names nothing.
    fn opt_reference(&mut self, field: &'static str) -> Result<Option<Option<EntityId>>, Unrepresentable> {
        let Some(old) = self.opt_text(field)? else { return Ok(None) };
        if let Some(new) = self.ids.new_of(&old) {
            return Ok(Some(Some(new)));
        }
        if self.ids.knows(&old) {
            self.waiting.push(field);
            return Ok(Some(None));
        }
        self.dangling.push(Dangling { model: self.model, field });
        EntityId::from_base64(&old)
            .map(|id| Some(Some(id)))
            .map_err(|e| self.refuse(format!("{field} holds {old:?}: {e}")))
    }
    fn opt_ref<T>(&mut self, field: &'static str) -> Result<Option<Ref<T>>, Unrepresentable> {
        Ok(self.opt_reference(field)?.flatten().map(Ref::new))
    }
    fn id(&mut self, field: &'static str) -> Result<EntityId, Unrepresentable> {
        match self.opt_reference(field)? {
            Some(id) => Ok(id.unwrap_or(self.placeholder)),
            None => Err(self.refuse(format!("{field}, which every record has, is empty"))),
        }
    }
    fn reference<T>(&mut self, field: &'static str) -> Result<Ref<T>, Unrepresentable> {
        self.id(field).map(Ref::new)
    }

    /// A JSON value with every string that is the id of a record of the project replaced by its
    /// new id; one naming a record not made yet stays until [`patch`] replaces it.
    fn repointed(&mut self, field: &'static str, value: &serde_json::Value) -> serde_json::Value {
        let mut waits = false;
        let mut dangles = false;
        let value = repoint(value, self.ids, &mut waits, &mut dangles);
        if waits {
            self.waiting.push(field);
        }
        if dangles {
            self.dangling.push(Dangling { model: self.model, field });
        }
        value
    }
    fn json(&mut self, field: &'static str) -> Result<Json, Unrepresentable> {
        match self.record.get(field).clone() {
            Stored::Json(value) => Ok(Json::new(self.repointed(field, &value))),
            Stored::Null => Err(self.refuse(format!("{field}, which every record has, is empty"))),
            other => Err(self.refuse(format!("{field} holds {other:?}, not JSON"))),
        }
    }
    /// A JSON value kept as text (`Principal::serves`).
    fn opt_json_text(&mut self, field: &'static str) -> Result<Option<String>, Unrepresentable> {
        let Some(text) = self.opt_text(field)? else { return Ok(None) };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else { return Ok(Some(text)) };
        let repointed = self.repointed(field, &value);
        // Unchanged text stays byte for byte; changed text is written as serde_json writes it,
        // which is how fwd writes it.
        Ok(Some(if repointed == value { text } else { repointed.to_string() }))
    }
}

/// Whether `text` has the shape of an entity id's base64 text.
pub fn id_shaped(text: &str) -> bool {
    text.len() == 43 && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn repoint(value: &serde_json::Value, ids: &Ids, waits: &mut bool, dangles: &mut bool) -> serde_json::Value {
    use serde_json::Value::*;
    match value {
        String(text) => match ids.new_of(text) {
            Some(new) => String(new.to_base64()),
            None => {
                if ids.knows(text) {
                    *waits = true;
                } else if id_shaped(text) {
                    *dangles = true;
                }
                value.clone()
            }
        },
        Array(items) => Array(items.iter().map(|item| repoint(item, ids, waits, dangles)).collect()),
        Object(map) => Object(map.iter().map(|(k, v)| (k.clone(), repoint(v, ids, waits, dangles))).collect()),
        _ => value.clone(),
    }
}

/// A record of one of the nine models, ready to make.
pub enum Made {
    Principal(Principal),
    Repo(Repo),
    Scope(Scope),
    Task(Task),
    CodeAnchor(CodeAnchor),
    Decision(Decision),
    Message(Message),
    Relation(Relation),
    Notification(Notification),
}

impl Made {
    pub async fn create(&self, trx: &ProjectTransaction) -> anyhow::Result<EntityId> {
        Ok(match self {
            Made::Principal(m) => trx.create(m).await?.id(),
            Made::Repo(m) => trx.create(m).await?.id(),
            Made::Scope(m) => trx.create(m).await?.id(),
            Made::Task(m) => trx.create(m).await?.id(),
            Made::CodeAnchor(m) => trx.create(m).await?.id(),
            Made::Decision(m) => trx.create(m).await?.id(),
            Made::Message(m) => trx.create(m).await?.id(),
            Made::Relation(m) => trx.create(m).await?.id(),
            Made::Notification(m) => trx.create(m).await?.id(),
        })
    }
}

/// The record `f` reads, as its model's struct.
pub fn build(model: &'static str, f: &mut Fields, project: Ref<Project>) -> Result<Made, Unrepresentable> {
    Ok(match model {
        "Principal" => Made::Principal(Principal {
            project,
            kind: f.word("kind")?,
            handle: f.text("handle")?,
            display_name: f.text("display_name")?,
            auth_subject: f.opt_text("auth_subject")?,
            agent_role: f.opt_word("agent_role")?,
            active: f.boolean("active")?,
            member: f.opt_bool("member")?,
            subject: f.opt_text("subject")?,
            succeeded_by: f.opt_ref("succeeded_by")?,
            serves: f.opt_json_text("serves")?,
            last_active: f.opt_int("last_active")?,
        }),
        "Repo" => Made::Repo(Repo {
            project,
            name: f.text("name")?,
            remote_urls: f.json("remote_urls")?,
            default_branch: f.text("default_branch")?,
        }),
        "Scope" => Made::Scope(Scope {
            project,
            status: f.word("status")?,
            title: f.text("title")?,
            intention: f.text("intention")?,
            context: f.json("context")?,
            started: f.opt_int("started")?,
            ended: f.opt_int("ended")?,
            created_by: f.reference("created_by")?,
            created: f.int("created")?,
        }),
        "Task" => Made::Task(Task {
            project,
            title: f.text("title")?,
            what: f.text("what")?,
            why: f.text("why")?,
            done_when: f.text("done_when")?,
            status: f.word("status")?,
            assignee: f.opt_ref("assignee")?,
            needs_review: f.boolean("needs_review")?,
            hold: f.opt_text("hold")?,
            not_before: f.opt_int("not_before")?,
            priority: f.opt_int("priority")?,
            labels: f.json("labels")?,
            context: f.json("context")?,
            created_by: f.reference("created_by")?,
            created: f.int("created")?,
            updated: f.int("updated")?,
            closed: f.opt_int("closed")?,
            closed_reason: f.opt_text("closed_reason")?,
        }),
        "CodeAnchor" => {
            let repo: Ref<Repo> = f.reference("repo")?;
            let path = f.text("path")?;
            let basis_blob = f.opt_text("basis_blob")?;
            let (start_line, end_line, whole_file) =
                (f.int("start_line")?, f.int("end_line")?, f.opt_bool("whole_file")?);
            let anchor_key = rekeyed(f, *repo, &path, basis_blob.as_deref(), (start_line, end_line, whole_file))?;
            Made::CodeAnchor(CodeAnchor {
                project,
                repo,
                path,
                anchor_key,
                span_key: f.opt_text("span_key")?,
                basis_blob,
                head_blob: f.opt_text("head_blob")?,
                index_blob: f.opt_text("index_blob")?,
                nearest_blob: f.opt_text("nearest_blob")?,
                basis_patch: f.opt_text("basis_patch")?,
                blob_sha: f.opt_text("blob_sha")?,
                commit_sha: f.opt_text("commit_sha")?,
                side: f.word("side")?,
                start_line,
                end_line,
                whole_file,
                anchored_text: f.text("anchored_text")?,
                context_before: f.text("context_before")?,
                context_after: f.text("context_after")?,
                pieces: f.json("pieces")?,
                symbol_path: f.opt_text("symbol_path")?,
                created_by: f.reference("created_by")?,
                created: f.int("created")?,
            })
        }
        "Decision" => Made::Decision(Decision {
            project,
            number: f.int("number")?,
            title: f.text("title")?,
            status: f.word("status")?,
            area: f.opt_text("area")?,
            scope: f.opt_ref("scope")?,
            context: f.text("context")?,
            decision: f.text("decision")?,
            consequences: f.text("consequences")?,
            author: f.reference("author")?,
            decided_by: f.opt_ref("decided_by")?,
            created: f.int("created")?,
            decided: f.opt_int("decided")?,
            closed_by: f.opt_ref("closed_by")?,
            closed: f.opt_int("closed")?,
            closed_reason: f.opt_text("closed_reason")?,
            closed_reason_message: f.opt_ref("closed_reason_message")?,
        }),
        "Message" => Made::Message(Message {
            project,
            author: f.reference("author")?,
            created: f.int("created")?,
            content: f.text("content")?,
            priority: f.word("priority")?,
            status: f.word("status")?,
            task: f.opt_ref("task")?,
            anchor: f.opt_ref("anchor")?,
            scope: f.opt_ref("scope")?,
            decision: f.opt_ref("decision")?,
            reply_to: f.opt_ref("reply_to")?,
            thread: f.opt_ref("thread")?,
            context: f.json("context")?,
            resolved_by: f.opt_ref("resolved_by")?,
            resolved: f.opt_int("resolved")?,
            edited: f.opt_int("edited")?,
            edited_by: f.opt_ref("edited_by")?,
        }),
        "Relation" => Made::Relation(Relation {
            project,
            kind: f.word("kind")?,
            from_id: f.id("from_id")?,
            to_id: f.id("to_id")?,
            status: f.word("status")?,
            created_by: f.reference("created_by")?,
            created: f.int("created")?,
            via: f.json("via")?,
            note: f.opt_text("note")?,
            position: f.opt_text("position")?,
            closed_by: f.opt_ref("closed_by")?,
            closed: f.opt_int("closed")?,
            closed_reason: f.opt_text("closed_reason")?,
        }),
        "Notification" => Made::Notification(Notification {
            project,
            recipient: f.reference("recipient")?,
            notification_type: f.word("notification_type")?,
            status: f.word("status")?,
            priority: f.word("priority")?,
            created: f.int("created")?,
            updated: f.opt_int("updated")?,
            target: f.json("target")?,
            originator: f.json("originator")?,
        }),
        other => {
            return Err(Unrepresentable {
                model: "?",
                id: f.record.id.clone(),
                reason: format!("{other} is not one of the nine models"),
            })
        }
    })
}

/// An anchor's key, which hashes its repository's id: made again from the repository's new id,
/// after checking that the stored key is the one fwd-core makes from the old id, so that the
/// anchor is found by the same lines of the same content as before.
fn rekeyed(
    f: &Fields,
    repo: EntityId,
    path: &str,
    basis_blob: Option<&str>,
    (start_line, end_line, whole_file): (i64, i64, Option<bool>),
) -> Result<Option<String>, Unrepresentable> {
    let Some(stored) = f.opt_text("anchor_key")? else { return Ok(None) };
    let Some(basis) = basis_blob else {
        return Err(f.refuse("it has a key and no basis to make it from".into()));
    };
    let lines = if whole_file == Some(true) { None } else { Some((start_line, end_line)) };
    let Stored::Text(old_repo) = f.record.get("repo") else {
        return Err(f.refuse("it has a key and no repository".into()));
    };
    if anchor_key(old_repo, path, basis, lines) != stored {
        return Err(f.refuse("its key is not the one fwd-core makes from its repository, path, basis and lines".into()));
    }
    Ok(Some(anchor_key(&repo.to_base64(), path, basis, lines)))
}

/// Sets a field that waited for the record it names, now that every record is made.
pub async fn patch(
    graph: &ProjectGraph,
    trx: &ProjectTransaction,
    model: &'static str,
    new: EntityId,
    record: &Record,
    ids: &Ids,
    field: &'static str,
) -> anyhow::Result<()> {
    let mut f = Fields::new(model, record, ids, new);
    match (model, field) {
        ("Principal", "succeeded_by") => {
            let value = f.opt_ref::<Principal>(field)?;
            trx.edit(&graph.get::<PrincipalView>(new).await?)?.succeeded_by()?.set(&value)?;
        }
        ("Principal", "serves") => {
            let value = f.opt_json_text(field)?;
            trx.edit(&graph.get::<PrincipalView>(new).await?)?.serves()?.set(&value)?;
        }
        ("Task", "context") => {
            let value = f.json(field)?;
            trx.edit(&graph.get::<TaskView>(new).await?)?.context()?.set(&value)?;
        }
        ("Decision", "closed_reason_message") => {
            let value = f.opt_ref::<Message>(field)?;
            trx.edit(&graph.get::<DecisionView>(new).await?)?.closed_reason_message()?.set(&value)?;
        }
        ("Message", "reply_to") => {
            let value = f.opt_ref::<Message>(field)?;
            trx.edit(&graph.get::<MessageView>(new).await?)?.reply_to()?.set(&value)?;
        }
        ("Message", "thread") => {
            let value = f.opt_ref::<Message>(field)?;
            trx.edit(&graph.get::<MessageView>(new).await?)?.thread()?.set(&value)?;
        }
        ("Message", "context") => {
            let value = f.json(field)?;
            trx.edit(&graph.get::<MessageView>(new).await?)?.context()?.set(&value)?;
        }
        ("Relation", "from_id") => {
            let value = f.id(field)?;
            trx.edit(&graph.get::<RelationView>(new).await?)?.from_id()?.set(&value)?;
        }
        ("Relation", "to_id") => {
            let value = f.id(field)?;
            trx.edit(&graph.get::<RelationView>(new).await?)?.to_id()?.set(&value)?;
        }
        ("Relation", "via") => {
            let value = f.json(field)?;
            trx.edit(&graph.get::<RelationView>(new).await?)?.via()?.set(&value)?;
        }
        _ => anyhow::bail!(
            "{model} {}'s {field} names a record made after it, which this order does not expect",
            record.id
        ),
    }
    if !f.waiting.is_empty() {
        anyhow::bail!("{model} {}'s {field} still names a record that was not made", record.id);
    }
    Ok(())
}
