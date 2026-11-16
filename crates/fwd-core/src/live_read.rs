//! The live queries that tell a reader that what it read may have changed, so that a view that
//! follows what a session holds (`fwd session view --follow`) reads it again when one of them
//! changes, and never on a timer. Each [`LiveRead`] is the live form of one read the view is made
//! from, keyed by what it selects by, so that the set the last read needs is compared with the set
//! open and only the difference is opened or dropped. A change is only the signal to read again:
//! the read says what changed.

use crate::model::*;
use crate::project_graph::{ProjectGraph, ProjectRecord};
use crate::query::with_id;
use crate::subject;
use crate::working_set::questions_by_name;
use ankurah::core::livequery::EntityLiveQuery;
use ankurah::signals::{Listener, ListenerGuard, Signal};
use ankurah::{EntityId, LiveQuery};
use ankurah_commons::cognition::IntentionView;
use ankurah_commons::text::QuotationView;
use anyhow::Result;

/// The live form of one read of what a session holds.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LiveRead {
    /// Every project of the node, which a project made joins.
    Projects,
    /// The principals of a project bound to an agent of a Claude Code session: one that names
    /// itself, retires, is renamed or selects another role ([`subject::of_session_selection`]).
    SessionPrincipals { project: ProjectRecord, session: String },
    /// The tasks assigned to a principal that are not finished, wherever they are.
    TasksAssignedTo(EntityId),
    /// The open questions a principal asked, wherever they are.
    QuestionsBy(EntityId),
    /// The open questions asked of a principal by its id, wherever they are.
    QuestionsTo(EntityId),
    /// The open questions of a project that ask a principal by its name, and, for a triage agent
    /// (`of_no_one`), those asked of no one in particular ([`questions_by_name`]).
    QuestionsByName { project: ProjectRecord, handle: String, of_no_one: bool },
    /// A principal's unread notifications, in its project.
    Unread { project: ProjectRecord, principal: EntityId },
    /// The answers in a question's thread, wherever they were written.
    AnswersTo(EntityId),
    /// The intentions assigned to a principal, of any project: an assignment made or taken off.
    IntentionsAssignedTo(EntityId),
    /// An intention assigned to a principal, wherever it is: its title, its description, or its
    /// ending, which a component of its own records, since a change to any component of the entity
    /// changes the query's result.
    AssignedIntention(EntityId),
    /// The `relates_to` relations to an assigned intention, which join its quotations to it: one
    /// made or retracted.
    QuotationsOf(EntityId),
    /// A record joined to an assigned intention, a quotation of its words, wherever it is: the
    /// words, who wrote them down and when, or whose they are, which components of its own record,
    /// since a change to any component of the entity changes the query's result.
    Quotation(EntityId),
}

/// A live read open on the daemon, which calls the listener it was opened with at each change of
/// its results, its first results included, until it is dropped.
pub struct Listening {
    // Declared first, so dropped first: the guard holds the query's broadcast.
    _listener: ListenerGuard,
    query: EntityLiveQuery,
}

impl Listening {
    /// Why the query can no longer be relied on to tell of a change, when it cannot: Ankurah gave up
    /// on it, as it does on a daemon that came back without it.
    pub fn failure(&self) -> Option<String> {
        self.query.error().value().map(|error| error.to_string())
    }
}

impl LiveRead {
    /// Opens the live query through `graph`, any project's graph of the node, calling `listener`
    /// at each change.
    pub fn listen(&self, graph: &ProjectGraph, listener: Listener) -> Result<Listening> {
        Ok(match self {
            LiveRead::Projects => listening(graph.query_projects()?, listener),
            LiveRead::SessionPrincipals { project, session } => {
                let bound = subject::of_session_selection(session)?;
                listening(graph.graph_of_project(project).query::<PrincipalView>(bound)?, listener)
            }
            LiveRead::TasksAssignedTo(principal) => {
                listening(graph.query_unfinished_tasks_assigned_to(*principal)?, listener)
            }
            LiveRead::QuestionsBy(principal) => listening(graph.query_open_questions_by(*principal)?, listener),
            LiveRead::QuestionsTo(principal) => listening(graph.query_open_questions_to(*principal)?, listener),
            LiveRead::QuestionsByName { project, handle, of_no_one } => {
                let there = graph.graph_of_project(project);
                listening(there.query::<MessageView>(questions_by_name(handle, *of_no_one)?)?, listener)
            }
            LiveRead::Unread { project, principal } => {
                let unread = with_id("recipient = ? AND status = 'unread'", *principal)?;
                listening(graph.graph_of_project(project).query::<NotificationView>(unread)?, listener)
            }
            LiveRead::AnswersTo(question) => listening(graph.query_answers_to(*question)?, listener),
            LiveRead::IntentionsAssignedTo(principal) => {
                listening(graph.query_relations_to(*principal, Some(RelationKind::AssignedTo))?, listener)
            }
            LiveRead::AssignedIntention(intention) => {
                listening(graph.query_placed::<IntentionView>(*intention)?, listener)
            }
            LiveRead::QuotationsOf(intention) => {
                listening(graph.query_relations_to(*intention, Some(RelationKind::RelatesTo))?, listener)
            }
            LiveRead::Quotation(quotation) => listening(graph.query_placed::<QuotationView>(*quotation)?, listener),
        })
    }
}

fn listening<V: ankurah::model::View + Clone + Send + Sync + 'static>(
    query: LiveQuery<V>,
    listener: Listener,
) -> Listening {
    Listening { _listener: query.listen(listener), query: query.into() }
}
