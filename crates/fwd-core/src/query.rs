//! Small helpers for building Ankurah selections with typed `EntityId` literals. A string
//! literal in a `Ref` predicate misses live updates (Ankurah issue #259); a typed literal does
//! not.

use ankurah::ankql::ast::{Expr, Parsed, Selection};
use ankurah::ankql::parser::parse_selection;
use ankurah::{EntityId, Value};
use anyhow::Result;

/// `predicate` with each `?` bound, in order, to a typed EntityId literal.
pub fn with_ids(predicate: &str, ids: &[EntityId]) -> Result<Selection<Parsed>> {
    let mut selection = parse_selection(predicate)?;
    selection.predicate = selection.predicate.populate(ids.iter().map(|id| Expr::Literal(Value::EntityId(*id))))?;
    Ok(selection)
}

pub fn with_id(predicate: &str, id: EntityId) -> Result<Selection<Parsed>> {
    with_ids(predicate, &[id])
}

/// `predicate` with its one `?` bound to a string literal, whatever characters it holds (a file
/// path with `/`, say); splicing text into a predicate is what `literal` guards against.
pub fn with_string(predicate: &str, s: &str) -> Result<Selection<Parsed>> {
    let mut selection = parse_selection(predicate)?;
    selection.predicate = selection.predicate.populate(std::iter::once(Expr::Literal(Value::String(s.to_string()))))?;
    Ok(selection)
}

/// A string literal safe to splice into a predicate: only the characters handles, statuses and
/// base64 ids use are allowed, so no quoting is needed.
pub fn literal(s: &str) -> Result<String> {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':')) {
        Ok(s.to_string())
    } else {
        anyhow::bail!("`{s}` may only contain letters, digits, `_`, `-`, `.` and `:`")
    }
}
