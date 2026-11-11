//! One project's records in the node a daemon keeps for every project it hosts (docs/DECISIONS.md,
//! D135). Each record names its project (`model::Project`), and that field is all that keeps one
//! project's records apart from another's in the shared store: no one working in one project may
//! see another project's records by accident (D38). So fwd's operations read and write only through
//! a [`ProjectGraph`], which is bound to one project: it adds the project to every fetch and every
//! live query, finds a record by its id only when the record is that project's, and refuses to make
//! or edit a record of another project. The Ankurah context it wraps never leaves this module, so
//! an operation that left the project out would not compile.

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
    before_d108::Scope => before_d108::ScopeView,
);

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

/// One project's records in the daemon's node: every read and write of fwd's operations.
#[derive(Clone)]
pub struct ProjectGraph {
    ctx: Context,
    project: EntityId,
    /// `project = <project>`, which every selection is joined to.
    in_project: Predicate<Parsed>,
}

impl ProjectGraph {
    fn bound(ctx: Context, project: EntityId) -> ProjectGraph {
        let in_project = equals("project", Value::EntityId(project));
        ProjectGraph { ctx, project, in_project }
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

    /// `args` narrowed to the project's records.
    fn scoped(&self, args: MatchArgs<Parsed>) -> MatchArgs<Parsed> {
        let MatchArgs { selection, cache_policy } = args;
        let predicate = Predicate::And(Box::new(self.in_project.clone()), Box::new(selection.predicate));
        MatchArgs { selection: Selection { predicate, ..selection }, cache_policy }
    }

    /// Every record of the project's in `V`'s model.
    fn everything(&self) -> Selection<Parsed> {
        Selection { predicate: self.in_project.clone(), order_by: None, limit: None }
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

    /// The project's record `id`, read as `V`. A record of another project is not found, as an id
    /// that names nothing is not.
    pub async fn get<V: InProjectView>(&self, id: EntityId) -> Result<V, RetrievalError> {
        let view: V = self.ctx.get(id).await?;
        match view.project_id() {
            Ok(project) if project == self.project => Ok(view),
            _ => Err(RetrievalError::EntityNotFound(id)),
        }
    }

    /// A transaction that makes and edits the project's records alone.
    pub fn begin(&self) -> ProjectTransaction {
        ProjectTransaction { trx: self.ctx.begin(), project: self.project }
    }
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

    /// The record `view` shows, to change in this transaction. A record of another project is not
    /// found, as in [`ProjectGraph::get`].
    pub fn edit<'rec, 'trx: 'rec, V: InProjectView>(
        &'trx self,
        view: &V,
    ) -> Result<MutableBorrow<'rec, <V::Model as Model>::Mutable>, RetrievalError> {
        match view.project_id() {
            Ok(project) if project == self.project => self.trx.edit::<V::Model>(view.entity()),
            _ => Err(RetrievalError::EntityNotFound(view.id())),
        }
    }

    pub async fn commit(self) -> Result<(), ProjectGraphError> {
        Ok(self.trx.commit().await?)
    }
}
