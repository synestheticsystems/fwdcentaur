//! The person the view is for, resolved once in every project and strictly, so that what waits on
//! them and the Answer they press act as one principal the view checked, never as a name resolved
//! again later. In each project the person is the one person (a principal of kind human)
//! with their name; where two have it, the view says so and offers nothing to answer there, rather
//! than taking either.

use super::text::{plain, LINE_LIMIT};
use crate::model::{PrincipalKind, PrincipalView};
use crate::naming;
use crate::ops;
use crate::project_graph::ProjectRecord;
use crate::working_set::{Addressee, Known, Record};
use ankurah::EntityId;
use std::collections::{BTreeMap, HashSet};

/// Who the view is read for, as the command names them.
#[derive(Debug, Clone, Copy)]
pub enum Reader<'a> {
    /// The person this machine's identity names, by their name.
    Identity(&'a str),
    /// What `--as` names: a name, or a principal's id or the start of one, which names that one
    /// principal and, by its name, the same person in the other projects.
    Named(&'a str),
}

/// The person the view is for.
#[derive(Debug, Clone)]
pub struct Person {
    /// Their name, by which their principal is found in each project; none when the reader names
    /// no person.
    pub handle: Option<String>,
    /// Their principal in each project where it is theirs alone, by the project's record.
    pub(super) principals: BTreeMap<EntityId, EntityId>,
    /// The projects where more than one person has their name.
    pub(super) shared: Vec<ProjectRecord>,
    /// What the document says of the reader, when it names no person.
    pub(super) said: Option<String>,
    /// The questions asked of them that they have answered in their threads.
    pub answered: HashSet<EntityId>,
}

impl Person {
    /// The person `reader` names, among the principals `known` holds.
    pub fn resolve(known: &Known, reader: Reader<'_>) -> Person {
        let mut person = Person {
            handle: None,
            principals: BTreeMap::new(),
            shared: Vec::new(),
            said: None,
            answered: HashSet::new(),
        };
        let (handle, chosen) = match reader {
            Reader::Identity(name) => (naming::bare(name).to_string(), None),
            Reader::Named(typed) => {
                let typed = naming::bare(typed);
                let by_id: Vec<(&ProjectRecord, &PrincipalView)> = if ops::may_start_an_id(typed) {
                    known
                        .projects
                        .iter()
                        .flat_map(|(project, all)| all.iter().map(move |p| (project, p)))
                        .filter(|(_, p)| p.id().to_base64().starts_with(typed))
                        .collect()
                } else {
                    Vec::new()
                };
                match by_id.as_slice() {
                    [] => (typed.to_string(), None),
                    [(project, p)] if p.kind().ok() == Some(PrincipalKind::Human) => match p.handle() {
                        Ok(handle) => (handle, Some((project.record, p.id()))),
                        Err(_) => return person.saying("the person `--as` names has a name fwd cannot read"),
                    },
                    [_] => return person.saying("`--as` names an agent, and the view is a person's"),
                    _ => return person.saying("`--as` starts the ids of more than one principal; give more of it"),
                }
            }
        };
        for (project, all) in &known.projects {
            if let Some((record, principal)) = chosen.filter(|(record, _)| *record == project.record) {
                person.principals.insert(record, principal);
                continue;
            }
            match naming::by_name(all, &handle, Some(PrincipalKind::Human)).as_slice() {
                [] => {}
                [one] => {
                    person.principals.insert(project.record, one.id());
                }
                _ => person.shared.push(project.clone()),
            }
        }
        if person.principals.is_empty() && person.shared.is_empty() {
            let agent = known.projects.iter().any(|(_, all)| !naming::by_name(all, &handle, None).is_empty());
            let why = if agent {
                format!("{handle} names an agent, and the view is a person's")
            } else {
                format!("no person is named {handle} in any project fwd holds")
            };
            return person.saying(&why);
        }
        person.handle = Some(handle);
        person
    }

    fn saying(mut self, why: &str) -> Person {
        self.said = Some(format!("{why}, so nothing here waits on you and nothing can be answered."));
        self
    }

    /// Their principal in the project whose record is `project`, when it is theirs alone.
    pub fn principal_in(&self, project: EntityId) -> Option<EntityId> {
        self.principals.get(&project).copied()
    }

    /// Whether `principal` is one of theirs.
    pub fn is(&self, principal: EntityId) -> bool {
        self.principals.values().any(|p| *p == principal)
    }

    /// Whether `question`, which asks `to`, asks them: by one of their principals' ids, wherever
    /// the question now is, or by their name where it is and they have a principal of their own.
    pub fn asked(&self, question: &Record, to: &Addressee) -> bool {
        match to {
            Addressee::Principal(party) => self.is(party.id),
            Addressee::Name(name) => {
                self.handle.as_deref() == Some(naming::bare(name))
                    && self.principal_in(question.project.record).is_some()
            }
            Addressee::NoOne => false,
        }
    }

    /// Why `question`, which asks them, cannot be answered from the view, when it cannot: they have
    /// no principal of their own where it now is.
    pub fn cannot_answer(&self, question: &Record) -> Option<String> {
        if self.principal_in(question.project.record).is_some() {
            return None;
        }
        let project = &question.project.name;
        let why = if self.shared.iter().any(|p| p.record == question.project.record) {
            format!("More than one person shares your name in {project}, where this question now is, so it cannot be answered here.")
        } else {
            format!("You have no principal in {project}, where this question now is, so it cannot be answered here.")
        };
        Some(plain(&why, LINE_LIMIT))
    }

    /// What the document says of them: that the reader names no person, or where their name is
    /// shared.
    pub fn lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self.said.iter().cloned().collect();
        if let Some(handle) = &self.handle {
            lines.extend(self.shared.iter().map(|project| {
                format!(
                    "More than one person is named {handle} in {}: what they are asked there is not counted as waiting on you, and none of it can be answered here.",
                    project.name
                )
            }));
        }
        lines.iter().map(|line| plain(line, LINE_LIMIT)).collect()
    }
}
