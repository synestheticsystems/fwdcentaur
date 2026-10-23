//! A task's labels: words that say what kind of work a task is, so that people and agents can find
//! the tasks of one kind and filter a list by it. A task's kind is a label, not a task type
//! (docs/DECISIONS.md, D97, "A task's kind, such as bug or feature, is a label, not a task type"),
//! and nothing in fwd's rules reads a label. `bug` and `feature` are the labels the project uses
//! by convention, which the page suggests first; any other word will do.
//!
//! A label is stored in one form, so that the same word typed twice is one label: trimmed, each
//! run of whitespace inside it made one space, and in lower case, so `Bug`, ` bug ` and `BUG` are
//! all the label `bug`. Case is dropped rather than kept as typed because a label is there to
//! filter by, where `Bug` and `bug` must find the same tasks, and one stored form lets the page
//! list each label in use once. A label is never blank and never holds a comma, since a comma
//! separates labels wherever several are typed at once (a box on the page, `--label bug,ui`), and
//! a task carries each label once. A label stored before these rules compares in the same form, so
//! taking off `web` takes off a `Web` written earlier; it stays as it was written until then.

use thiserror::Error;

/// The labels the project uses by convention, a task's kind, in the order the page suggests them.
pub const CONVENTIONAL: [&str; 2] = ["bug", "feature"];

/// Why typed text cannot be a label, or why a change to a task's labels is refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LabelError {
    #[error("a label cannot be blank")]
    Blank,
    #[error("a label cannot hold a comma, which separates labels: give `{0}` as separate labels")]
    Comma(String),
    #[error("`{0}` is both added and taken off; give it once")]
    AddedAndTakenOff(String),
}

/// The form a label is stored and compared in: trimmed, each run of whitespace one space, in lower
/// case. It refuses nothing, so that a label stored before the rules compares in it too.
fn stored_form(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// The label `typed` stands for, in the form it is stored in, or why it cannot be one: it is
/// blank, or it holds a comma.
pub fn label(typed: &str) -> Result<String, LabelError> {
    let label = stored_form(typed);
    if label.is_empty() {
        return Err(LabelError::Blank);
    }
    if label.contains(',') {
        return Err(LabelError::Comma(typed.trim().to_string()));
    }
    Ok(label)
}

/// Whether `a` and `b` are the same label, whatever case and spacing each was written with.
pub fn same(a: &str, b: &str) -> bool {
    stored_form(a) == stored_form(b)
}

/// Whether `labels` holds `label`.
pub fn has(labels: &[String], label: &str) -> bool {
    labels.iter().any(|held| same(held, label))
}

/// The labels a task is filed with, from those typed: each in its stored form and each once, in
/// the order given; refused when one is blank or holds a comma.
pub fn list(typed: &[String]) -> Result<Vec<String>, LabelError> {
    let mut labels: Vec<String> = Vec::new();
    for t in typed {
        let label = label(t)?;
        if !has(&labels, &label) {
            labels.push(label);
        }
    }
    Ok(labels)
}

/// A task's labels once `add` is added and `remove` taken off: the labels it had, in their order
/// and as they were written, less each one the same as a label taken off, then each label added
/// that it did not have, in its stored form. A label both added and taken off is refused, and so
/// is a blank one or one holding a comma, before anything changes.
pub fn relabel(current: &[String], add: &[String], remove: &[String]) -> Result<Vec<String>, LabelError> {
    let (add, remove) = (list(add)?, list(remove)?);
    if let Some(both) = add.iter().find(|label| has(&remove, label)) {
        return Err(LabelError::AddedAndTakenOff(both.clone()));
    }
    let mut labels: Vec<String> = current.iter().filter(|held| !has(&remove, held)).cloned().collect();
    for label in add {
        if !has(&labels, &label) {
            labels.push(label);
        }
    }
    Ok(labels)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(labels: &[&str]) -> Vec<String> {
        labels.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn a_label_is_stored_trimmed_with_single_spaces_and_in_lower_case() {
        assert_eq!(label("  Bug "), Ok("bug".into()));
        assert_eq!(label("BUG"), Ok("bug".into()));
        assert_eq!(label("Page \t  Forms"), Ok("page forms".into()));
        assert_eq!(label("feature"), Ok("feature".into()));
    }

    #[test]
    fn a_label_is_never_blank_and_never_holds_a_comma() {
        assert_eq!(label(""), Err(LabelError::Blank));
        assert_eq!(label(" \n "), Err(LabelError::Blank));
        assert_eq!(label(" bug, ui "), Err(LabelError::Comma("bug, ui".into())));
        assert_eq!(
            LabelError::Comma("bug, ui".into()).to_string(),
            "a label cannot hold a comma, which separates labels: give `bug, ui` as separate labels"
        );
    }

    #[test]
    fn labels_compare_in_their_stored_form_whatever_was_written() {
        assert!(same("Web", "web") && same(" page  forms", "Page Forms"));
        assert!(!same("bug", "bugs"));
        assert!(has(&strings(&["Web", "bug"]), "WEB"));
        assert!(!has(&strings(&["web"]), "bug"));
    }

    #[test]
    fn a_task_is_filed_with_each_label_once_in_the_order_given() {
        assert_eq!(list(&strings(&["Bug", "feature", " bug", "UI"])), Ok(strings(&["bug", "feature", "ui"])));
        assert_eq!(list(&[]), Ok(vec![]));
        assert_eq!(list(&strings(&["bug", " "])), Err(LabelError::Blank));
    }

    #[test]
    fn relabelling_adds_what_is_missing_and_takes_off_what_is_there() {
        let current = strings(&["bug", "Web"]);
        assert_eq!(relabel(&current, &strings(&["Feature", "BUG"]), &[]), Ok(strings(&["bug", "Web", "feature"])));
        assert_eq!(
            relabel(&current, &[], &strings(&["web"])),
            Ok(strings(&["bug"])),
            "a label written before the rules"
        );
        assert_eq!(
            relabel(&current, &strings(&["web"]), &[]),
            Ok(current.clone()),
            "a label it has is not added again"
        );
        assert_eq!(relabel(&current, &[], &strings(&["ui"])), Ok(current.clone()), "a label it lacks is not taken off");
        assert_eq!(
            relabel(&current, &strings(&["ui"]), &strings(&["bug"])),
            Ok(strings(&["Web", "ui"])),
            "both at once"
        );
    }

    #[test]
    fn a_label_both_added_and_taken_off_or_unfit_changes_nothing() {
        let current = strings(&["bug"]);
        assert_eq!(
            relabel(&current, &strings(&["UI"]), &strings(&["ui"])),
            Err(LabelError::AddedAndTakenOff("ui".into()))
        );
        assert_eq!(relabel(&current, &strings(&[""]), &[]), Err(LabelError::Blank));
        assert_eq!(relabel(&current, &[], &strings(&["a,b"])), Err(LabelError::Comma("a,b".into())));
    }
}
