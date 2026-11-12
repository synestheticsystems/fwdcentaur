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
//!   whichever project made them, so a relation made in another project shows on the record it
//!   points at; only the project that made a relation changes it ([`ProjectGraph::ensure_own`]). An
//!   id a record holds (a relation's end, a message's author, a task's assignee or scope, a cited
//!   decision) is followed wherever its record is ([`ProjectGraph::follow`]), and the record comes
//!   with its project, which a reader names beside it.
//! - An id someone types names a record of any project (D181): the resolver reads every project's
//!   ids ([`ProjectGraph::ids_in_every_project`]), and a command whose subject is a record of
//!   another project runs in that project, as if it had been run there.
//!
//! The Ankurah context the graph wraps never leaves this module, so an operation that left the
//! project out of a list would not compile. A record's project is read each time and never
//! remembered, so a record moved to another project (an operation of its own, which nothing
//! performs yet) leaves this graph's lists and live queries and joins the other's.

use crate::model::*;
use ankurah::ankql::ast::{ComparisonOperator, Expr, Parsed, PathExpr, Predicate, Selection};
use ankurah::error::{MutationError, RetrievalError};
use ankurah::model::MutableBorrow;
use ankurah::property::PropertyError;
use ankurah::transaction::Transaction;
use ankurah::{Context, EntityId, LiveQuery, MatchArgs, Model, Ref, Value, View};
use uuid::Uuid;

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
);

/// The scope as `fwd admin migrate` reads and clears what it held before D108, which no one makes.
impl InProjectView for before_d108::ScopeView {
    fn project_id(&self) -> Result<EntityId, PropertyError> {
        Ok(*self.project()?)
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
    /// own project's records. `name` is that project's name where the refusal read it
    /// ([`ProjectGraph::ensure_own`]).
    #[error(
        "{} is a record of {}, and only a command run in that project changes it",
        .record.to_base64(),
        .name.as_ref().map_or_else(|| format!("another project ({})", .project.to_base64()), |name| format!("project `{name}`: run the command there (--project '{name}')"))
    )]
    OfAnotherProject { record: EntityId, project: EntityId, name: Option<String> },
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

/// `<field> = <value>`, with the value a typed literal: a string literal compared with a `Ref`
/// field misses live updates (Ankurah issue #259).
fn equals(field: &str, value: Value) -> Predicate<Parsed> {
    Predicate::Comparison {
        left: Box::new(Expr::Path(PathExpr { component: None, steps: vec![field.to_string()] })),
        operator: ComparisonOperator::Equal,
        right: Box::new(Expr::Literal(value)),
    }
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

    /// The project's record `id`, read as `V`. A record of another project is not found here, as
    /// an id that names nothing is not: a command whose subject is a record of another project
    /// runs in that project's graph, and an id a record holds is followed
    /// ([`ProjectGraph::follow`]).
    pub async fn get<V: InProjectView>(&self, id: EntityId) -> Result<V, RetrievalError> {
        let view: V = self.ctx.get(id).await?;
        match view.project_id() {
            Ok(project) if project == self.project => Ok(view),
            _ => Err(RetrievalError::EntityNotFound(id)),
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

    /// The relation `id`, whichever project made it, as a record's relations hold it
    /// ([`ProjectGraph::relations_from`]): a command that changes it refuses one of another project
    /// first ([`ProjectGraph::ensure_own`]).
    pub async fn relation(&self, id: EntityId) -> Result<RelationView, RetrievalError> {
        self.ctx.get(id).await
    }

    /// Whether `record` is one of this project's, which this graph's transactions change.
    pub fn owns<V: InProjectView>(&self, record: &V) -> bool {
        record.project_id().is_ok_and(|project| project == self.project)
    }

    /// Refuses, naming its project, a record of another project, which this graph's transactions
    /// do not change ([`ProjectGraphError::OfAnotherProject`]): a command that reached it through a
    /// record's relations says so before it begins to write. A record of this project passes.
    pub async fn ensure_own<V: InProjectView>(&self, record: &V) -> Result<(), ProjectGraphError> {
        let project = record.project_id()?;
        if project == self.project {
            return Ok(());
        }
        let name = match self.whose(project).await? {
            Some(Whose::Other(p)) => Some(p.name),
            Some(Whose::This) | None => None,
        };
        Err(ProjectGraphError::OfAnotherProject { record: record.id(), project, name })
    }

    /// The project whose record is `project`, as a reader names it; none when the store has no
    /// such record.
    async fn whose(&self, project: EntityId) -> Result<Option<Whose>, ProjectGraphError> {
        if project == self.project {
            return Ok(Some(Whose::This));
        }
        let Some(p) = absent(self.ctx.get::<ProjectView>(project).await)? else { return Ok(None) };
        let text = p.uuid()?;
        let uuid =
            text.parse().map_err(|_| PropertyError::InvalidValue { value: text, ty: "a project's UUID".into() })?;
        Ok(Some(Whose::Other(ProjectRecord { record: project, uuid, name: p.name()? })))
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
        Ok(None)
    }

    /// The id of every record of `V`'s model in the node, with the record of the project it is in:
    /// what an id someone types may name, since a typed id names a record of any project (D181).
    /// Only the command line's resolver reads it; no screen or list shows it.
    pub async fn ids_in_every_project<V: InProjectView>(&self) -> Result<Vec<(EntityId, EntityId)>, ProjectGraphError> {
        let every: Vec<V> = self.ctx.fetch(selecting(Predicate::True)).await?;
        Ok(every.iter().map(|v| Ok((v.id(), v.project_id()?))).collect::<Result<_, PropertyError>>()?)
    }

    /// Every project's former ids, as [`ProjectGraph::ids_in_every_project`] reads the ids: what a
    /// typed id may be the start of.
    pub async fn former_ids_in_every_project(&self) -> Result<Vec<FormerIdView>, RetrievalError> {
        self.ctx.fetch(selecting(Predicate::True)).await
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
        ProjectTransaction { trx: self.ctx.begin(), project: self.project }
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
}

/// A transaction in one project's graph: it makes only records that name the project, and edits
/// only the project's records.
pub struct ProjectTransaction {
    trx: Transaction,
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

    /// The record `view` shows, to change in this transaction. A record of another project is
    /// refused ([`ProjectGraphError::OfAnotherProject`]); [`ProjectGraph::ensure_own`] refuses it
    /// with its project's name before a transaction begins.
    pub fn edit<'rec, 'trx: 'rec, V: InProjectView>(
        &'trx self,
        view: &V,
    ) -> Result<MutableBorrow<'rec, <V::Model as Model>::Mutable>, ProjectGraphError> {
        match view.project_id()? {
            project if project == self.project => Ok(self.trx.edit::<V::Model>(view.entity())?),
            project => Err(ProjectGraphError::OfAnotherProject { record: view.id(), project, name: None }),
        }
    }

    pub async fn commit(self) -> Result<(), ProjectGraphError> {
        Ok(self.trx.commit().await?)
    }
}
