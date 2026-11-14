//! One project's records in the node a daemon keeps for every project it hosts (docs/DECISIONS.md,
//! D135). Each record names its project (`model::Project`), and the project groups it with the
//! project's other records: "a project is just a grouping, and a mandatory filter term for listing
//! on any given screen, but it shouldn't cause relationships not to be rendered" (D176, Daniel,
//! 2026-11-12). This module holds the rule for which reads carry the project, so that no operation
//! decides it for itself:
//!
//! - Every list and every live query is one project's: [`ProjectGraph::fetch`], `all`, `query` and
//!   `query_all` add the project to what they select, so a screen or a list shows one project's
//!   records. A record read by its id ([`ProjectGraph::get`]) is the project's, and a transaction
//!   ([`ProjectTransaction`]) makes and changes only the project's records.
//! - What connects records is read wherever in the node it is. The relations that touch a record
//!   are read by that record's id ([`ProjectGraph::relations_from`], `relations_to`, `relation`),
//!   and so are the messages about it ([`ProjectGraph::messages_about`]), whichever project made
//!   them, so a relation made in another project shows on the record it points at, and a command
//!   that changes a relation runs in the project that made it, as any command that acts on a
//!   record of another project does. An id a record holds (a relation's
//!   end, a message's author, a task's assignee or scope, a cited decision) is followed wherever
//!   its record is ([`ProjectGraph::follow`]), and the record comes with its project, which a
//!   reader names beside it.
//! - An id someone types names a record of any project (D181): the resolver reads the former ids
//!   that start with it ([`ProjectGraph::former_ids_starting`]) and, when no record of the
//!   project has an id that starts with it, every project's ids
//!   ([`ProjectGraph::ids_in_every_project`]), and a command that acts on a record of another
//!   project runs in that project, as if it had been run there.
//! - The routing service alone hears every project's records, through [`NodeGraph`], whose
//!   standing queries name no project (D177), and it routes each change through the graph of the
//!   record's own project ([`ProjectGraph::graph_of`]), so that each rule is read where its subject
//!   is and each notification is made in its recipient's project.
//! - What an agent holds is read wherever it now is, since a task moved to another project keeps
//!   its assignee, and its messages their authors and addressees (`task_move`): three reads, each
//!   given one principal's id and building its own selection here, find the tasks assigned to it
//!   ([`ProjectGraph::unfinished_tasks_assigned_to`]), the open questions it asked
//!   ([`ProjectGraph::open_questions_by`]) and those asked of it by its id
//!   ([`ProjectGraph::open_questions_to`]) in every project, each with its project, which the
//!   reader names beside it (`working_set`). No read takes a caller's selection across projects.
//!
//! - A page shows one project's lists from live queries that carry the project, and names what
//!   their records hold the ids of through live reads that carry none ([`ProjectGraph::follow_live`],
//!   [`ProjectGraph::relations_made_elsewhere_live`], [`ProjectGraph::query_messages_about`],
//!   [`ProjectGraph::query_projects`]): each holds what a reader names by id, so a record of another
//!   project is shown with its project and its conversation, and no list reads through them.
//!
//! - A record built on the Commons' models names no project: the `Placement` component on its
//!   entity does (`model::Placement`), and the graph reads it under the same rule (`placed`): a
//!   list reads its components with its `Placement` in one selection that carries the project, a
//!   record read by its id is the project's or followed into its own, and a record's readings are
//!   read by its id wherever they were placed ([`ProjectGraph::readings_of`]).
//!
//! - A record changes project in one write alone, [`ProjectTransaction::take_in`], which a task's
//!   move to another project makes (`task_move`); every other write of a project's graph makes and
//!   changes that project's records alone.
//!
//! The Ankurah context the graph wraps never leaves this module, so an operation that left the
//! project out of a list would not compile. A record's project is read each time and never
//! remembered, so a record moved to another project leaves this graph's lists and live queries and
//! joins the other's.

use crate::model::*;
use crate::ops::{ASKS_KEY, ASKS_PROJECT};
use crate::query::{with_id, with_string};
use ankurah::ankql::ast::{ComparisonOperator, Expr, Parsed, PathExpr, Predicate, Selection};
use ankurah::error::{MutationError, RetrievalError};
use ankurah::model::MutableBorrow;
use ankurah::property::PropertyError;
use ankurah::transaction::Transaction;
use ankurah::{Context, EntityId, LiveQuery, MatchArgs, Model, Ref, Value, View};
use ankurah_commons::intention_graph::{IntentionStatus, IntentionView};
use std::collections::BTreeSet;
use uuid::Uuid;

mod placed;

/// A model whose records each name their project: the nine models of a project's work, and
/// [`FormerId`].
pub trait InProject: Model {
    /// The project the record names.
    fn project_id(&self) -> EntityId;
}

/// The view of a record that names its project, which is what a [`ProjectGraph`] reads.
pub trait InProjectView: View {
    /// The project the record names.
    fn project_id(&self) -> Result<EntityId, PropertyError>;

    /// Names `project` as the project of the record `record` changes: what
    /// [`ProjectTransaction::take_in`] writes.
    fn name_project(record: &<Self::Model as Model>::Mutable, project: &Ref<Project>) -> Result<(), PropertyError>;
}

macro_rules! in_project {
    ($($model:ty => $view:ty),* $(,)?) => {$(
        impl InProject for $model {
            fn project_id(&self) -> EntityId {
                *self.project
            }
        }
        impl InProjectView for $view {
            fn project_id(&self) -> Result<EntityId, PropertyError> {
                Ok(*self.project()?)
            }

            fn name_project(
                record: &<$model as Model>::Mutable,
                project: &Ref<Project>,
            ) -> Result<(), PropertyError> {
                record.project()?.set(project)
            }
        }
    )*};
}

in_project!(
    Principal => PrincipalView,
    Repo => RepoView,
    Task => TaskView,
    Relation => RelationView,
    CodeAnchor => CodeAnchorView,
    Message => MessageView,
    Notification => NotificationView,
    Scope => ScopeView,
    Decision => DecisionView,
    FormerId => FormerIdView,
    Placement => PlacementView,
);

/// The scope as `fwd admin migrate` reads and clears what it held before D108, which no one makes.
impl InProjectView for before_d108::ScopeView {
    fn project_id(&self) -> Result<EntityId, PropertyError> {
        Ok(*self.project()?)
    }

    fn name_project(record: &before_d108::ScopeMut, project: &Ref<Project>) -> Result<(), PropertyError> {
        record.project()?.set(project)
    }
}

/// Why a project's graph could not be opened, or refused a write.
#[derive(Debug, thiserror::Error)]
pub enum ProjectGraphError {
    /// The store holds no record of the project: the daemon makes it as it hosts the project,
    /// before it answers anything about it, so a client finds it there.
    #[error("the daemon's store holds no project {0}")]
    NotHosted(Uuid),
    /// The store holds several records of one project, which only a fault could have made: which
    /// of them the project's records name cannot be guessed.
    #[error("the daemon's store holds {} records of project {uuid}: {}", .records.len(), ids(.records))]
    Duplicated { uuid: Uuid, records: Vec<EntityId> },
    /// A record naming one project was to be made in another project's graph.
    #[error("a record of project {named} was to be made in the graph of project {bound}")]
    OtherProject { named: EntityId, bound: EntityId },
    /// A record of another project, such as a relation another project made that points at one of
    /// this project's records, was to be changed in this project's graph, which changes only its
    /// own project's records, so that what a command writes is attributed to its caller in the
    /// project it runs in: a command that acts on the record runs in the record's project. Only a
    /// task's move to another project takes a record of another project in
    /// ([`ProjectTransaction::take_in`]).
    #[error(
        "{} is a record of another project ({}), which only a command run in that project changes",
        .record.to_base64(),
        .project.to_base64()
    )]
    OfAnotherProject { record: EntityId, project: EntityId },
    /// A record read by its id through the graph of the project named `here`
    /// ([`ProjectGraph::get`]) is a record of the project named `project`: a command that acts on
    /// a record runs in the record's project, so a command whose record moved there while it ran
    /// ends with this, and runs there when it is run again.
    #[error("{} is a record of project `{project}`, not of `{here}`, whose graph read it by its id", .record.to_base64())]
    InAnotherProject { record: EntityId, project: String, here: String },
    /// A component was to be added to a record that carries it already
    /// ([`ProjectTransaction::add`]).
    #[error("the record carries `{label}` already")]
    Carried { label: &'static str },
    #[error(transparent)]
    Retrieval(Box<RetrievalError>),
    #[error(transparent)]
    Mutation(Box<MutationError>),
    #[error(transparent)]
    Property(#[from] PropertyError),
}

impl From<RetrievalError> for ProjectGraphError {
    fn from(e: RetrievalError) -> Self {
        ProjectGraphError::Retrieval(Box::new(e))
    }
}

impl From<MutationError> for ProjectGraphError {
    fn from(e: MutationError) -> Self {
        ProjectGraphError::Mutation(Box::new(e))
    }
}

fn ids(records: &[EntityId]) -> String {
    records.iter().map(EntityId::to_base64).collect::<Vec<_>>().join(", ")
}

/// `<field> <operator> <value>`, with the value a typed literal: a string literal compared with a
/// `Ref` field misses live updates (Ankurah issue #259).
fn comparing(field: &str, operator: ComparisonOperator, value: Value) -> Predicate<Parsed> {
    Predicate::Comparison {
        left: Box::new(Expr::Path(PathExpr { component: None, steps: vec![field.to_string()] })),
        operator,
        right: Box::new(Expr::Literal(value)),
    }
}

/// `<field> = <value>` ([`comparing`]).
fn equals(field: &str, value: Value) -> Predicate<Parsed> {
    comparing(field, ComparisonOperator::Equal, value)
}

/// The values of the text `field` that start with `prefix`, an ASCII text: from `prefix` up to,
/// not including, the text whose last character is the one after `prefix`'s. The store compares
/// text by its bytes (SQLite's binary collation), as Ankurah does where it reads a predicate
/// itself, and in that order the texts that start with `prefix` are exactly those of that range.
fn starting(field: &str, prefix: &str) -> Predicate<Parsed> {
    let Some((&last, rest)) = prefix.as_bytes().split_last() else { return Predicate::True };
    let rest = std::str::from_utf8(rest).expect("an ASCII text's bytes before its last are ASCII");
    let after = format!("{rest}{}", char::from(last + 1));
    let from = comparing(field, ComparisonOperator::GreaterThanOrEqual, Value::String(prefix.to_string()));
    let before = comparing(field, ComparisonOperator::LessThan, Value::String(after));
    Predicate::And(Box::new(from), Box::new(before))
}

/// Every record a selection with this predicate is made from.
fn selecting(predicate: Predicate<Parsed>) -> Selection<Parsed> {
    Selection { predicate, order_by: None, limit: None }
}

/// The relations whose `end` (`from_id` or `to_id`) is `id`, of `kind` when one is given.
fn touching(end: &str, id: EntityId, kind: Option<RelationKind>) -> Selection<Parsed> {
    let at = equals(end, Value::EntityId(id));
    selecting(match kind {
        Some(kind) => Predicate::And(Box::new(at), Box::new(equals("kind", Value::String(kind.to_string())))),
        None => at,
    })
}

/// None for a record the node does not hold, or holds as a record of another model.
fn absent<T>(read: Result<T, RetrievalError>) -> Result<Option<T>, RetrievalError> {
    match read {
        Ok(found) => Ok(Some(found)),
        Err(RetrievalError::EntityNotFound(_) | RetrievalError::MissingComponent { .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// One project's records in the daemon's node: every read and write of fwd's operations.
#[derive(Clone)]
pub struct ProjectGraph {
    ctx: Context,
    project: EntityId,
}

impl ProjectGraph {
    fn bound(ctx: Context, project: EntityId) -> ProjectGraph {
        ProjectGraph { ctx, project }
    }

    /// The records of the project `uuid` names, of which there is one once the daemon hosts it.
    async fn records_of(ctx: &Context, uuid: Uuid) -> Result<Vec<ProjectView>, RetrievalError> {
        ctx.fetch(equals("uuid", Value::String(uuid.to_string()))).await
    }

    /// The graph of the project `uuid` names, for a client of the daemon that hosts it.
    pub async fn open(ctx: Context, uuid: Uuid) -> Result<ProjectGraph, ProjectGraphError> {
        let found = Self::records_of(&ctx, uuid).await?;
        match found.as_slice() {
            [one] => Ok(Self::bound(ctx, one.id())),
            [] => Err(ProjectGraphError::NotHosted(uuid)),
            several => Err(ProjectGraphError::Duplicated { uuid, records: several.iter().map(View::id).collect() }),
        }
    }

    /// The graph of the project `uuid` names, for the daemon as it hosts the project: its record
    /// made when the store has none, and given `name`, the name the index has for it now.
    pub async fn ensure(ctx: Context, uuid: Uuid, name: &str) -> Result<ProjectGraph, ProjectGraphError> {
        let found = Self::records_of(&ctx, uuid).await?;
        let project = match found.as_slice() {
            [] => {
                let trx = ctx.begin();
                let made = trx.create(&Project { uuid: uuid.to_string(), name: name.to_string() }).await?.id();
                trx.commit().await?;
                made
            }
            [one] => {
                if one.name()? != name {
                    let trx = ctx.begin();
                    one.edit(&trx)?.name()?.set(&name.to_string())?;
                    trx.commit().await?;
                }
                one.id()
            }
            several => {
                return Err(ProjectGraphError::Duplicated { uuid, records: several.iter().map(View::id).collect() })
            }
        };
        Ok(Self::bound(ctx, project))
    }

    /// The project's record, for the `project` field of a record being made.
    pub fn project(&self) -> Ref<Project> {
        Ref::new(self.project)
    }

    /// Gives the project's record `name`, as `fwd project rename` renames the project in this
    /// machine's index while the daemon runs; a daemon that starts later takes the name from the
    /// index as it hosts the project ([`ProjectGraph::ensure`]).
    pub async fn rename(&self, name: &str) -> Result<(), ProjectGraphError> {
        let record: ProjectView = self.ctx.get(self.project).await?;
        let trx = self.ctx.begin();
        record.edit(&trx)?.name()?.set(&name.to_string())?;
        Ok(trx.commit().await?)
    }

    /// `args` narrowed to the project's records.
    fn scoped(&self, args: MatchArgs<Parsed>) -> MatchArgs<Parsed> {
        let MatchArgs { selection, cache_policy } = args;
        let in_project = equals("project", Value::EntityId(self.project));
        let predicate = Predicate::And(Box::new(in_project), Box::new(selection.predicate));
        MatchArgs { selection: Selection { predicate, ..selection }, cache_policy }
    }

    /// Every record of the project's in `V`'s model.
    fn everything(&self) -> Selection<Parsed> {
        selecting(equals("project", Value::EntityId(self.project)))
    }

    /// The project's records of `V`'s model that `args` selects.
    pub async fn fetch<V: InProjectView>(
        &self,
        args: impl TryInto<MatchArgs<Parsed>, Error = impl Into<RetrievalError>>,
    ) -> Result<Vec<V>, RetrievalError> {
        let args = args.try_into().map_err(Into::into)?;
        self.ctx.fetch(self.scoped(args)).await
    }

    /// Every record of the project's in `V`'s model.
    pub async fn all<V: InProjectView>(&self) -> Result<Vec<V>, RetrievalError> {
        self.ctx.fetch(self.everything()).await
    }

    /// A live query on the project's records of `V`'s model that `args` selects.
    pub fn query<V: InProjectView>(
        &self,
        args: impl TryInto<MatchArgs<Parsed>, Error = impl Into<RetrievalError>>,
    ) -> Result<LiveQuery<V>, RetrievalError> {
        let args = args.try_into().map_err(Into::into)?;
        self.ctx.query(self.scoped(args))
    }

    /// A live query on every record of the project's in `V`'s model.
    pub fn query_all<V: InProjectView>(&self) -> Result<LiveQuery<V>, RetrievalError> {
        self.ctx.query(self.everything())
    }

    /// The project's record `id`, read as `V`. A record of another project is refused, naming
    /// that project ([`ProjectGraphError::InAnotherProject`]): a command that acts on a record of
    /// another project runs in that project's graph, and an id a record holds is followed
    /// ([`ProjectGraph::follow`]). A record that names a project the node holds no record of is
    /// not found here, as an id that names nothing is not.
    pub async fn get<V: InProjectView>(&self, id: EntityId) -> Result<V, ProjectGraphError> {
        let view: V = self.ctx.get(id).await?;
        let project = view.project_id()?;
        if project == self.project {
            return Ok(view);
        }
        match self.whose(project).await? {
            Some(Whose::Other(there)) => {
                let here: ProjectView = self.ctx.get(self.project).await?;
                Err(ProjectGraphError::InAnotherProject { record: id, project: there.name, here: here.name()? })
            }
            Some(Whose::This) | None => Err(RetrievalError::EntityNotFound(id).into()),
        }
    }

    /// The relations from the record `from`, of `kind` when one is given, whichever project made
    /// them: a record's relations show on it wherever they were made (D176).
    pub async fn relations_from(
        &self,
        from: EntityId,
        kind: Option<RelationKind>,
    ) -> Result<Vec<RelationView>, RetrievalError> {
        self.ctx.fetch(touching("from_id", from, kind)).await
    }

    /// The relations to the record `to`, of `kind` when one is given, whichever project made them:
    /// a relation another project made that points at one of this project's records shows on that
    /// record (D176).
    pub async fn relations_to(
        &self,
        to: EntityId,
        kind: Option<RelationKind>,
    ) -> Result<Vec<RelationView>, RetrievalError> {
        self.ctx.fetch(touching("to_id", to, kind)).await
    }

    /// The messages whose `field`, one of the fields that say what a message is about (`task`,
    /// `anchor`, `scope`, `decision`) or which thread it is in (`thread`, `reply_to`), holds the
    /// record `about`, whichever project wrote them: a message about a record is a connection to it,
    /// as a relation is, so it shows on the record wherever it was written (D176).
    pub async fn messages_about(&self, field: &str, about: EntityId) -> Result<Vec<MessageView>, RetrievalError> {
        self.ctx.fetch(selecting(equals(field, Value::EntityId(about)))).await
    }

    /// The relation `id`, whichever project made it, as a record's relations hold it
    /// ([`ProjectGraph::relations_from`]): a command that changes it runs in the project that made
    /// it.
    pub async fn relation(&self, id: EntityId) -> Result<RelationView, RetrievalError> {
        self.ctx.get(id).await
    }

    /// The graph of the project `record` is in: this project's graph for one of its records, else
    /// that project's. A rule about a record reached by following an id, such as who triages the
    /// scope a task is worked in, is read among that record's project's principals, and what tells
    /// a principal of another project is written in that project, where its inbox lists it.
    pub fn graph_of<V: InProjectView>(&self, record: &V) -> Result<ProjectGraph, PropertyError> {
        Ok(Self::bound(self.ctx.clone(), record.project_id()?))
    }

    /// Whether `record` is one of this project's, which this graph's transactions change.
    pub fn owns<V: InProjectView>(&self, record: &V) -> bool {
        record.project_id().is_ok_and(|project| project == self.project)
    }

    /// The project whose record is `project`, as a reader names it; none when the store has no
    /// such record.
    async fn whose(&self, project: EntityId) -> Result<Option<Whose>, ProjectGraphError> {
        if project == self.project {
            return Ok(Some(Whose::This));
        }
        let Some(p) = absent(self.ctx.get::<ProjectView>(project).await)? else { return Ok(None) };
        Ok(Some(Whose::Other(ProjectRecord::of(&p)?)))
    }

    /// Every project of the node, by its record, UUID and name: where a task may be moved
    /// (`task_move`). A project is no project's record, so this lists no project's records.
    pub async fn projects(&self) -> Result<Vec<ProjectRecord>, ProjectGraphError> {
        let every: Vec<ProjectView> = self.ctx.fetch(selecting(Predicate::True)).await?;
        Ok(every.iter().map(ProjectRecord::of).collect::<Result<_, _>>()?)
    }

    /// The graph of the project `project` is the record of, which a record moved there joins.
    pub fn graph_of_project(&self, project: &ProjectRecord) -> ProjectGraph {
        Self::bound(self.ctx.clone(), project.record)
    }

    /// The record of this graph's project, as [`ProjectGraph::projects`] lists it.
    pub fn record(&self) -> EntityId {
        self.project
    }

    /// The record that `id`, an id a record had before it was renumbered, names now, in whichever
    /// project keeps that former id (`former_ids`); none when no project keeps it, or when the
    /// projects that keep it name different records, which would be a guess.
    async fn renumbered(&self, id: EntityId) -> Result<Option<EntityId>, ProjectGraphError> {
        let kept: Vec<FormerIdView> =
            self.ctx.fetch(selecting(equals("former", Value::String(id.to_base64())))).await?;
        let mut named = kept.iter().map(|f| f.record());
        let Some(first) = named.next().transpose()? else { return Ok(None) };
        for other in named {
            if other? != first {
                return Ok(None);
            }
        }
        Ok(Some(first))
    }

    /// The record `id` that a record holds, read as `V` wherever in the node it is, with its
    /// project, which a reader names beside it: an id a record holds is followed, never declined
    /// for being another project's (D176). An id the record had before it was renumbered, which a
    /// relation or a text written before then holds, names the record it has now. Nothing of the
    /// record is copied into the record that holds the id. A record the node does not hold, as
    /// `V`, is [`Followed::NotHeld`], which a reader shows as a record that cannot be followed
    /// here, never as an error or a guess.
    pub async fn follow<V: InProjectView>(&self, id: EntityId) -> Result<Followed<V>, ProjectGraphError> {
        let found = match absent(self.ctx.get::<V>(id).await)? {
            Some(view) => Some(view),
            None => match self.renumbered(id).await? {
                Some(record) => absent(self.ctx.get::<V>(record).await)?,
                None => None,
            },
        };
        let Some(view) = found else { return Ok(Followed::NotHeld) };
        Ok(match self.whose(view.project_id()?).await? {
            Some(project) => Followed::Held { record: view.id(), project, found: view },
            None => Followed::NotHeld,
        })
    }

    /// The record `id` that a record holds, of whichever kind it is, as [`ProjectGraph::follow`]
    /// reads it: what a reader needs to name it and to judge a dependency on it.
    pub async fn follow_any(&self, id: EntityId) -> Result<Followed<Referent>, ProjectGraphError> {
        let found = match self.referent(id).await? {
            Some(found) => Some(found),
            None => match self.renumbered(id).await? {
                Some(record) => self.referent(record).await?,
                None => None,
            },
        };
        let Some((record, project, referent)) = found else { return Ok(Followed::NotHeld) };
        Ok(match self.whose(project).await? {
            Some(project) => Followed::Held { record, project, found: referent },
            None => Followed::NotHeld,
        })
    }

    /// The record `id` wherever in the node it is, its project and what [`Referent`] keeps of it;
    /// none when the node holds no record of a kind a reference names.
    async fn referent(&self, id: EntityId) -> Result<Option<(EntityId, EntityId, Referent)>, ProjectGraphError> {
        let at = |project: Result<EntityId, PropertyError>, referent| Ok(Some((id, project?, referent)));
        if let Some(t) = absent(self.ctx.get::<TaskView>(id).await)? {
            return at(t.project_id(), Referent::Task { title: t.title()?, status: t.status()? });
        }
        if let Some(d) = absent(self.ctx.get::<DecisionView>(id).await)? {
            let referent = Referent::Decision { number: d.number()?, title: d.title()?, status: d.status()? };
            return at(d.project_id(), referent);
        }
        if let Some(s) = absent(self.ctx.get::<ScopeView>(id).await)? {
            return at(s.project_id(), Referent::Scope { title: s.title()?, status: s.status()? });
        }
        if let Some(m) = absent(self.ctx.get::<MessageView>(id).await)? {
            let content = match m.content() {
                Err(PropertyError::Missing) => String::new(),
                read => read?,
            };
            let words = crate::reason::first_words(&content, crate::reason::NAME_WORDS);
            return at(m.project_id(), Referent::Message { words, status: m.status()? });
        }
        if let Some(a) = absent(self.ctx.get::<CodeAnchorView>(id).await)? {
            let lines = if a.whole_file()?.unwrap_or(false) { None } else { Some((a.start_line()?, a.end_line()?)) };
            return at(a.project_id(), Referent::Anchor { path: a.path()?, lines });
        }
        if let Some(p) = absent(self.ctx.get::<PrincipalView>(id).await)? {
            return at(p.project_id(), Referent::Principal { handle: p.handle()? });
        }
        if let Some(r) = absent(self.ctx.get::<RelationView>(id).await)? {
            return at(r.project_id(), Referent::Relation { kind: r.kind()? });
        }
        if let Some(i) = absent(self.ctx.get::<IntentionView>(id).await)? {
            let placement: PlacementView = self.ctx.get(id).await?;
            return at(placement.project_id(), Referent::Intention { title: i.title()?, status: i.status()? });
        }
        Ok(None)
    }

    /// The id of every record of `V`'s model in the node, with the record of the project it is in:
    /// what an id someone types may name, since a typed id names a record of any project (D181).
    /// Only the command line's resolver reads it; no screen or list shows it.
    pub async fn ids_in_every_project<V: InProjectView>(&self) -> Result<Vec<(EntityId, EntityId)>, ProjectGraphError> {
        let every: Vec<V> = self.ctx.fetch(selecting(Predicate::True)).await?;
        Ok(every.iter().map(|v| Ok((v.id(), v.project_id()?))).collect::<Result<_, PropertyError>>()?)
    }

    /// The former ids of each of `records`, of whichever project keeps them: what a record moved
    /// to another project takes with it, so that what cited it by an id it had still finds it
    /// there (`task_move`).
    pub async fn former_ids_of(&self, records: &[EntityId]) -> Result<Vec<FormerIdView>, RetrievalError> {
        let mut each = records.iter().map(|id| equals("record", Value::EntityId(*id)));
        let Some(first) = each.next() else { return Ok(Vec::new()) };
        let any = each.fold(first, |any, one| Predicate::Or(Box::new(any), Box::new(one)));
        self.ctx.fetch(selecting(any)).await
    }

    /// The former ids, of every project, that start with `prefix`, the start of an id someone
    /// typed: the former ids it may name, read as one range of their text ([`starting`]) rather
    /// than every former id the node keeps, which are tens of thousands.
    pub async fn former_ids_starting(&self, prefix: &str) -> Result<Vec<FormerIdView>, RetrievalError> {
        // An id's text is ASCII, so no former id starts with any other character.
        if !prefix.is_ascii() {
            return Ok(Vec::new());
        }
        self.ctx.fetch(selecting(starting("former", prefix))).await
    }

    /// The tasks assigned to `principal` that are not done, cancelled or rejected, of every
    /// project: a task moved to another project keeps its assignee.
    pub async fn unfinished_tasks_assigned_to(&self, principal: EntityId) -> anyhow::Result<Vec<TaskView>> {
        let held = "assignee = ? AND status != 'done' AND status != 'cancelled' AND status != 'rejected'";
        Ok(self.ctx.fetch(with_id(held, principal)?).await?)
    }

    /// The open messages `principal` wrote that ask someone, or no one in particular, of every
    /// project: a message moved with its task keeps its author.
    pub async fn open_questions_by(&self, principal: EntityId) -> anyhow::Result<Vec<MessageView>> {
        let asking = format!(
            "author = ? AND status = '{}' AND (context.to IS NOT NULL OR context.{ASKS_KEY} = '{ASKS_PROJECT}')",
            MessageStatus::Active
        );
        Ok(self.ctx.fetch(with_id(&asking, principal)?).await?)
    }

    /// The open messages that ask `principal` by its id, of every project: a message moved with its
    /// task keeps its addressee.
    pub async fn open_questions_to(&self, principal: EntityId) -> anyhow::Result<Vec<MessageView>> {
        let asked = format!("status = '{}' AND context.to = ?", MessageStatus::Active);
        Ok(self.ctx.fetch(with_string(&asked, &principal.to_base64())?).await?)
    }

    /// The project of each of `projects`, project records as [`ProjectGraph::ids_in_every_project`]
    /// gives them, as a reader names it; one the store has no record of is left out.
    pub async fn projects_named(&self, projects: &[EntityId]) -> Result<Vec<(EntityId, Whose)>, ProjectGraphError> {
        let mut out = Vec::with_capacity(projects.len());
        for project in projects {
            if let Some(whose) = self.whose(*project).await? {
                out.push((*project, whose));
            }
        }
        Ok(out)
    }

    /// A transaction that makes and edits the project's records alone.
    pub fn begin(&self) -> ProjectTransaction {
        ProjectTransaction { trx: self.ctx.begin(), ctx: self.ctx.clone(), project: self.project }
    }
}

/// Every project's records in the daemon's node, as the one service that answers to a change in
/// any project reads them: routing, which tells whoever a change concerns (D177). It hears every
/// project's records through standing queries that name no project, so a record whose project
/// changes stays in them, and it routes each change by the rules of the record's own project,
/// through that project's graph ([`NodeGraph::graph_of`]). Nothing that lists or shows records
/// reads through it: a list is one project's ([`ProjectGraph`]).
#[derive(Clone)]
pub struct NodeGraph {
    ctx: Context,
}

impl NodeGraph {
    /// The records of every project in the node `ctx` is a context on.
    pub fn new(ctx: Context) -> NodeGraph {
        NodeGraph { ctx }
    }

    /// Every record of `V`'s model in the node, of whichever project.
    pub async fn all<V: InProjectView>(&self) -> Result<Vec<V>, RetrievalError> {
        self.ctx.fetch(selecting(Predicate::True)).await
    }

    /// A live query on every record of `V`'s model in the node, of whichever project: a record
    /// whose project changes is an update in it, never a record that leaves and another that
    /// arrives.
    pub fn query_all<V: InProjectView>(&self) -> Result<LiveQuery<V>, RetrievalError> {
        self.ctx.query(selecting(Predicate::True))
    }

    /// The graph of the project `record` is in, by whose rules a change to it is routed.
    pub fn graph_of<V: InProjectView>(&self, record: &V) -> Result<ProjectGraph, PropertyError> {
        Ok(ProjectGraph::bound(self.ctx.clone(), record.project_id()?))
    }

    /// The graph of every project the node holds a record of.
    pub async fn projects(&self) -> Result<Vec<ProjectGraph>, RetrievalError> {
        let every: Vec<ProjectView> = self.ctx.fetch(selecting(Predicate::True)).await?;
        Ok(every.iter().map(|p| ProjectGraph::bound(self.ctx.clone(), p.id())).collect())
    }
}

/// The live reads of a page that shows one project: what its records name, wherever it is. A page
/// lists one project's records ([`ProjectGraph::query_all`]) and names what they hold the ids of
/// (the far end of a relation, a message's author, a task's assignee), which may be a record of
/// another project; it names that record with its project's name, as the command line does
/// ([`ProjectGraph::follow`]), and keeps it current as it changes there.
impl ProjectGraph {
    /// A live query on the records of `V`'s model whose ids are `ids`, of whichever project: the
    /// records a page names by their ids and does not list, which [`LiveFollow::follow_ids`] changes
    /// as the page names others. An id the node holds no record of in `V`'s model is not among its
    /// results. Nothing that lists records reads through it.
    pub fn follow_live<V: InProjectView>(&self, ids: &BTreeSet<EntityId>) -> Result<LiveFollow<V>, RetrievalError> {
        let reads = Reads::Records;
        Ok(LiveFollow { query: self.ctx.query(selecting(reads.predicate(ids)))?, reads })
    }

    /// A live query on the relations that other projects made from or to any of `ids`: what a page
    /// reads beside its project's own relations, so that a relation another project made, pointing
    /// at one of this project's records, shows on that record, and a record of another project
    /// shows with its own relations. It reads by the records' ids, since the store has no join
    /// that would find them by the records' project. [`LiveFollow::follow_ids`] changes the records
    /// it reads them for.
    pub fn relations_made_elsewhere_live(
        &self,
        ids: &BTreeSet<EntityId>,
    ) -> Result<LiveFollow<RelationView>, RetrievalError> {
        let reads = Reads::RelationsMadeElsewhere { here: self.project };
        Ok(LiveFollow { query: self.ctx.query(selecting(reads.predicate(ids)))?, reads })
    }

    /// A live query on the messages whose `field` holds the record `about`, whichever project wrote
    /// them: the conversation a page shows beneath a record, read live as
    /// [`ProjectGraph::messages_about`] reads it, so that a record of another project shows with its
    /// own conversation.
    pub fn query_messages_about(&self, field: &str, about: EntityId) -> Result<LiveQuery<MessageView>, RetrievalError> {
        self.ctx.query(selecting(equals(field, Value::EntityId(about))))
    }

    /// A live query on every project of the node: the names by which a page names a record of
    /// another project beside it. A project is no project's record, so this lists no project's
    /// records.
    pub fn query_projects(&self) -> Result<LiveQuery<ProjectView>, RetrievalError> {
        self.ctx.query(selecting(Predicate::True))
    }
}

/// What a [`LiveFollow`] reads for the ids it is given.
#[derive(Clone, Copy)]
enum Reads {
    /// The records with those ids.
    Records,
    /// The relations from or to those ids that a project other than `here` made.
    RelationsMadeElsewhere { here: EntityId },
}

impl Reads {
    fn predicate(self, ids: &BTreeSet<EntityId>) -> Predicate<Parsed> {
        // `IN ()` is no predicate the query language reads: no id selects nothing.
        if ids.is_empty() {
            return Predicate::False;
        }
        let among = |field: &str| Predicate::Comparison {
            left: Box::new(Expr::Path(PathExpr { component: None, steps: vec![field.to_string()] })),
            operator: ComparisonOperator::In,
            right: Box::new(Expr::ExprList(ids.iter().map(|id| Expr::Literal(Value::EntityId(*id))).collect())),
        };
        match self {
            Reads::Records => among("id"),
            Reads::RelationsMadeElsewhere { here } => {
                let elsewhere = comparing("project", ComparisonOperator::NotEqual, Value::EntityId(here));
                let touching = Predicate::Or(Box::new(among("from_id")), Box::new(among("to_id")));
                Predicate::And(Box::new(elsewhere), Box::new(touching))
            }
        }
    }
}

/// A live query a page reads what its records name through ([`ProjectGraph::follow_live`],
/// [`ProjectGraph::relations_made_elsewhere_live`]), over ids it changes as the page names others.
#[derive(Clone)]
pub struct LiveFollow<V: InProjectView> {
    query: LiveQuery<V>,
    reads: Reads,
}

impl<V: InProjectView> LiveFollow<V> {
    /// The live query, to read its results and wait for the daemon's answer.
    pub fn live(&self) -> &LiveQuery<V> {
        &self.query
    }

    /// Reads for `ids` from now on, in place of the ids it read for: what no longer matches leaves
    /// the results and what now matches arrives, each a change of its own.
    pub fn follow_ids(&self, ids: &BTreeSet<EntityId>) -> Result<(), RetrievalError> {
        self.query.update_selection(selecting(self.reads.predicate(ids)))
    }
}

/// A record that another record holds the id of, as [`ProjectGraph::follow`] found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Followed<T> {
    /// The node holds it: its id now (another than the id followed where that was one it had
    /// before it was renumbered), the project it is a record of, and the record.
    Held { record: EntityId, project: Whose, found: T },
    /// The node holds no such record: a reader shows it as a record that cannot be followed here.
    NotHeld,
}

/// The project a record is in, as a reader of another project's graph names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Whose {
    /// The project of the graph that read it.
    This,
    /// Another project, which a reader names beside the record.
    Other(ProjectRecord),
}

/// A project of the node: its record, the UUID the index and the daemon's routes name it by, and
/// its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRecord {
    pub record: EntityId,
    pub uuid: Uuid,
    pub name: String,
}

impl ProjectRecord {
    /// The project `view` shows.
    pub fn of(view: &ProjectView) -> Result<ProjectRecord, PropertyError> {
        let text = view.uuid()?;
        let uuid =
            text.parse().map_err(|_| PropertyError::InvalidValue { value: text, ty: "a project's UUID".into() })?;
        Ok(ProjectRecord { record: view.id(), uuid, name: view.name()? })
    }
}

/// What a reader needs of a record another record refers to, by its kind: what names it, and
/// what a dependency on it is judged by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Referent {
    Task {
        title: String,
        status: TaskStatus,
    },
    /// A decision, whose number is its own project's (`D12` there), so a reader of another
    /// project names that project beside it.
    Decision {
        number: i64,
        title: String,
        status: DecisionStatus,
    },
    Scope {
        title: String,
        status: ScopeStatus,
    },
    /// A message, by its first words.
    Message {
        words: String,
        status: MessageStatus,
    },
    /// A highlight, by its path and its lines (0-based, end exclusive), none for a whole file.
    Anchor {
        path: String,
        lines: Option<(i64, i64)>,
    },
    Principal {
        handle: String,
    },
    Relation {
        kind: RelationKind,
    },
    Intention {
        title: String,
        status: IntentionStatus,
    },
}

/// A transaction in one project's graph: it makes only records that name the project, and edits
/// only the project's records.
pub struct ProjectTransaction {
    trx: Transaction,
    /// The context the transaction began on, which registers the components a placed record is
    /// given ([`ProjectTransaction::add`]).
    ctx: Context,
    project: EntityId,
}

impl ProjectTransaction {
    /// The project's record, for the `project` field of a record being made.
    pub fn project(&self) -> Ref<Project> {
        Ref::new(self.project)
    }

    /// Makes `model`, which must name the transaction's project.
    pub async fn create<'rec, 'trx: 'rec, M: InProject>(
        &'trx self,
        model: &M,
    ) -> Result<MutableBorrow<'rec, M::Mutable>, ProjectGraphError> {
        let named = model.project_id();
        if named != self.project {
            return Err(ProjectGraphError::OtherProject { named, bound: self.project });
        }
        Ok(self.trx.create(model).await?)
    }

    /// Takes the record `view` shows into the transaction's project, from whichever project it is
    /// in, and gives it to change further in this transaction: the one write that changes a
    /// record's project, which a task's move to another project makes for the task and what
    /// belongs to it (`task_move`), so that what one project grouped another groups, every id that
    /// names the record naming it still. A record of the project already is taken as it is. Every
    /// other change to a record of another project is refused ([`ProjectTransaction::edit`]).
    #[cfg(feature = "native")]
    pub(crate) fn take_in<'rec, 'trx: 'rec, V: InProjectView>(
        &'trx self,
        view: &V,
    ) -> Result<MutableBorrow<'rec, <V::Model as Model>::Mutable>, ProjectGraphError> {
        let taken = self.trx.edit::<V::Model>(view.entity())?;
        V::name_project(&taken, &Ref::new(self.project))?;
        Ok(taken)
    }

    /// The record `view` shows, to change in this transaction. A record of another project is
    /// refused ([`ProjectGraphError::OfAnotherProject`]).
    pub fn edit<'rec, 'trx: 'rec, V: InProjectView>(
        &'trx self,
        view: &V,
    ) -> Result<MutableBorrow<'rec, <V::Model as Model>::Mutable>, ProjectGraphError> {
        match view.project_id()? {
            project if project == self.project => Ok(self.trx.edit::<V::Model>(view.entity())?),
            project => Err(ProjectGraphError::OfAnotherProject { record: view.id(), project }),
        }
    }

    pub async fn commit(self) -> Result<(), ProjectGraphError> {
        Ok(self.trx.commit().await?)
    }
}
