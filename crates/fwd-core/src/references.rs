//! Inline references in a message's text: a writer names a person as `@handle`, and anything by
//! its id, the first six characters or more of it, or `D<number>` for a decision, after an
//! at-sign or in double brackets (`@vwugaN8h`, `[[vwugaN8h]]`, `[[D12]]`; an at-sign tries the
//! handle first), and a whole file at a version by its path in double brackets
//! (`[[docs/cli.md@e13b8e2]]`, `file_mention.rs`). When the message is posted,
//! `ops::post_message` resolves each one, stores it as `[[<full id>]]` whatever the writer typed,
//! so every reader finds the thing without a lookup, a file as its whole-file highlight's id with
//! the version it names after an at-sign (`[[<full id>@<version>]]`, [`Named`]), and writes a
//! `mention` relation from the message to the thing for each, which is what routing and
//! "mentioned in" read (docs/DECISIONS.md, "Inline references are shadowed by `mention` edges",
//! 2026-09-28).
//!
//! Code is text, not a reference: nothing inside a code span (between backticks) or a fenced
//! code block is read, so `@Override` in backticks stays as written. This module only reads and
//! writes text; resolving a reference needs the graph and lives in `ops`.

use crate::file_mention::Version;
use ankurah::EntityId;
use std::ops::Range;

/// A reference as the writer typed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Written {
    /// `@handle`: a person or an agent by handle, else anything by its id or the start of one.
    Handle(String),
    /// `[[...]]`: anything, by id, by the start of its id, or a decision by `D<number>`; the text
    /// between the brackets, trimmed.
    Bracketed(String),
}

impl Written {
    /// The reference as it reads in the text.
    pub fn as_typed(&self) -> String {
        match self {
            Written::Handle(h) => format!("@{h}"),
            Written::Bracketed(inner) => format!("[[{inner}]]"),
        }
    }
}

/// One reference found in a text: where it is (a byte range) and what was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub range: Range<usize>,
    pub written: Written,
}

/// Characters a handle is made of; a trailing `.` ends the sentence rather than the handle.
fn handle_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')
}

/// Whether an `@` after `before` starts a mention: at the start of the text, or after a space or
/// an opening mark, never inside a word (`daniel@example.com` names no one).
fn opens_mention(before: Option<char>) -> bool {
    match before {
        None => true,
        Some(c) => {
            c.is_whitespace() || matches!(c, '(' | '[' | '{' | ',' | ';' | ':' | '!' | '?' | '"' | '\'' | '*' | '>')
        }
    }
}

/// The byte ranges of `text` that are code: fenced blocks (a line opening with three or more
/// backticks or tildes, to a line closing with as many of the same) and code spans (a run of
/// backticks to the next run of as many; a run with no partner is plain text).
pub fn code_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut fence: Option<(char, usize, usize)> = None; // (mark, length, start)
    let mut offset = 0;
    let mut prose: Vec<Range<usize>> = Vec::new(); // the lines outside fences, for code spans
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let mark = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'));
        let run = mark.map(|m| trimmed.chars().take_while(|c| *c == m).count()).unwrap_or(0);
        match fence {
            Some((m, len, start)) if mark == Some(m) && run >= len && trimmed[run..].trim().is_empty() => {
                ranges.push(start..offset + line.len());
                fence = None;
            }
            Some(_) => {}
            None if run >= 3 => fence = Some((mark.expect("a run has a mark"), run, offset)),
            None => prose.push(offset..offset + line.len()),
        }
        offset += line.len();
    }
    if let Some((_, _, start)) = fence {
        ranges.push(start..text.len());
    }
    // Code spans are read within a line: one that runs across lines is rare in a comment, and
    // its backticks then read as plain text.
    for span in prose {
        let bytes = text[span.clone()].as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != b'`' {
                i += 1;
                continue;
            }
            let open = bytes[i..].iter().take_while(|b| **b == b'`').count();
            let mut j = i + open;
            let mut close = None;
            while j < bytes.len() {
                if bytes[j] == b'`' {
                    let run = bytes[j..].iter().take_while(|b| **b == b'`').count();
                    if run == open {
                        close = Some(j + run);
                        break;
                    }
                    j += run;
                } else {
                    j += 1;
                }
            }
            match close {
                Some(end) => {
                    ranges.push(span.start + i..span.start + end);
                    i = end;
                }
                None => i += open,
            }
        }
    }
    ranges.sort_by_key(|r| r.start);
    ranges
}

fn in_code(code: &[Range<usize>], at: usize) -> bool {
    code.iter().any(|r| r.contains(&at))
}

/// A mention being typed, for a mention search to offer the principals it may name: its bytes in
/// the text, from the `@` to the end of the name, the caret included, and what is typed between
/// the `@` and the caret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Typing {
    pub range: Range<usize>,
    pub typed: String,
}

/// The mention the caret, at byte `caret` of `text`, is in: an `@` that opens a mention as [`find`]
/// reads one (at the start, or after a space or an opening mark, never inside a word or in code),
/// then the characters a handle is made of, up to the caret and on past it. None anywhere else.
pub fn mention_at(text: &str, caret: usize) -> Option<Typing> {
    if !text.is_char_boundary(caret) {
        return None;
    }
    let typed: usize = text[..caret].chars().rev().take_while(|c| handle_char(*c)).map(char::len_utf8).sum();
    let at = (caret - typed).checked_sub(1)?;
    if text.as_bytes()[at] != b'@' || !opens_mention(text[..at].chars().next_back()) || in_code(&code_ranges(text), at)
    {
        return None;
    }
    let rest: usize = text[caret..].chars().take_while(|c| handle_char(*c)).map(char::len_utf8).sum();
    Some(Typing { range: at..caret + rest, typed: text[at + 1..caret].to_string() })
}

/// The references in `text` as the writer typed them, in order, outside code. A `@` followed by
/// no handle, and `[[` with no `]]` on the same line, are plain text.
pub fn find(text: &str) -> Vec<Found> {
    let code = code_ranges(text);
    let mut found = Vec::new();
    let mut previous: Option<char> = None;
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let before = previous;
        previous = Some(c);
        if in_code(&code, at) {
            continue;
        }
        if c == '@' && opens_mention(before) {
            let rest = &text[at + 1..];
            let handle: String = rest.chars().take_while(|c| handle_char(*c)).collect();
            let handle = handle.trim_end_matches('.');
            if !handle.is_empty() {
                let end = at + 1 + handle.len();
                found.push(Found { range: at..end, written: Written::Handle(handle.to_string()) });
                while chars.peek().is_some_and(|(i, _)| *i < end) {
                    previous = chars.next().map(|(_, c)| c);
                }
            }
        } else if c == '[' && text[at..].starts_with("[[") {
            let inner_start = at + 2;
            let line_end = text[inner_start..].find('\n').map(|n| inner_start + n).unwrap_or(text.len());
            if let Some(close) = text[inner_start..line_end].find("]]") {
                let end = inner_start + close + 2;
                let inner = text[inner_start..inner_start + close].trim().to_string();
                found.push(Found { range: at..end, written: Written::Bracketed(inner) });
                while chars.peek().is_some_and(|(i, _)| *i < end) {
                    previous = chars.next().map(|(_, c)| c);
                }
            }
        }
    }
    found
}

/// What a reference names once it is resolved, as a stored text writes it: the id of the thing,
/// and for a file mention the version of the file it names (`file_mention.rs`), so that the
/// version the writer named goes with the mention while the whole-file highlight, keyed by its
/// content, serves every version that holds the same bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    pub id: EntityId,
    /// The version of the file a file mention names; none for any other reference.
    pub version: Option<Version>,
}

impl Named {
    /// What names the entity `id`, with no version.
    pub fn entity(id: EntityId) -> Named {
        Named { id, version: None }
    }

    /// The inner text of a stored reference, read back: a full id, and for a file mention an
    /// at-sign and the version (`worktree`, `index` or a commit's id); none for anything else,
    /// such as what a writer typed before it was resolved.
    pub fn parse(inner: &str) -> Option<Named> {
        match inner.split_once('@') {
            None => inner.parse::<EntityId>().ok().map(Named::entity),
            Some((id, version)) => Some(Named { id: id.parse().ok()?, version: Some(Version::parse(version)?) }),
        }
    }

    /// The reference as a stored text writes it: `[[<full id>]]`, or `[[<full id>@<version>]]`.
    pub fn stored(&self) -> String {
        match &self.version {
            None => format!("[[{}]]", self.id.to_base64()),
            Some(version) => format!("[[{}@{version}]]", self.id.to_base64()),
        }
    }
}

/// `text` with each of `found` replaced by the stored form of what it names, the one at the same
/// place in `named` ([`Named::stored`]): the form a posted message stores.
pub fn rewrite(text: &str, found: &[Found], named: &[Named]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    for (f, n) in found.iter().zip(named) {
        out.push_str(&text[from..f.range.start]);
        out.push_str(&n.stored());
        from = f.range.end;
    }
    out.push_str(&text[from..]);
    out
}

/// The references of a stored text: each `[[<full id>]]` and `[[<full id>@<version>]]` outside
/// code, in order, with where it is and what it names.
pub fn stored_named(text: &str) -> Vec<(Range<usize>, Named)> {
    find(text)
        .into_iter()
        .filter_map(|f| match f.written {
            Written::Bracketed(inner) => Named::parse(&inner).map(|named| (f.range, named)),
            Written::Handle(_) => None,
        })
        .collect()
}

/// The references of a stored text, as [`stored_named`] reads them, by the id of what each names.
pub fn stored(text: &str) -> Vec<(Range<usize>, EntityId)> {
    stored_named(text).into_iter().map(|(range, named)| (range, named.id)).collect()
}

/// A stored text for reading: each reference replaced by what `name` gives for the id it names
/// (for the CLI's text output, a person as `@handle` and anything else as its name in brackets),
/// and left as it is where `name` gives nothing.
pub fn render(text: &str, name: impl Fn(EntityId) -> Option<String>) -> String {
    render_named(text, |named| name(named.id))
}

/// A stored text for reading, as [`render`] reads it, with what `name` gives for each reference
/// read whole, so that a file mention is named with the version it names.
pub fn render_named(text: &str, mut name: impl FnMut(&Named) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    for (range, named) in stored_named(text) {
        if let Some(shown) = name(&named) {
            out.push_str(&text[from..range.start]);
            out.push_str(&shown);
            from = range.end;
        }
    }
    out.push_str(&text[from..]);
    out
}

/// A stored text as its writer would type it again, for a box that edits it: each `[[<full id>]]`
/// becomes what `as_typed` gives for it (a person's `Written::Handle`, a decision's
/// `Written::Bracketed("D12")`), or the first eight characters of its id in double brackets where
/// it gives nothing, so the box shows names a person can read, and the edit resolves each back to
/// what it named. A handle is written only where it would be read back as that handle: after a
/// space or an opening mark, and before anything that cannot continue a handle; elsewhere the
/// reference keeps the bracketed form. A file mention keeps the version it names after the first
/// eight characters of its highlight's id (`[[vwugaN8h@index]]`), which an edit reads back with
/// no checkout, so that the page's edit box, and an edit made after the file changed, keep the
/// file as it was named.
pub fn editable(text: &str, as_typed: impl Fn(EntityId) -> Option<Written>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    for (range, Named { id, version }) in stored_named(text) {
        out.push_str(&text[from..range.start]);
        let start: String = id.to_base64().chars().take(8).collect();
        if let Some(version) = version {
            out.push_str(&format!("[[{start}@{version}]]"));
            from = range.end;
            continue;
        }
        let short = || format!("[[{start}]]");
        let written = match as_typed(id) {
            Some(Written::Handle(handle)) => {
                let reads_back = !handle.is_empty()
                    && handle.chars().all(handle_char)
                    && !handle.ends_with('.')
                    && opens_mention(out.chars().last())
                    && ends_handle(&text[range.end..]);
                if reads_back {
                    format!("@{handle}")
                } else {
                    short()
                }
            }
            Some(Written::Bracketed(inner)) if !inner.is_empty() && !inner.contains("]]") => format!("[[{inner}]]"),
            _ => short(),
        };
        out.push_str(&written);
        from = range.end;
    }
    out.push_str(&text[from..]);
    out
}

/// Whether a handle ends where `rest` begins: `rest` starts with nothing a handle continues with,
/// full stops aside when nothing a handle is made of follows them, since a handle's trailing full
/// stops end the sentence rather than the handle.
fn ends_handle(rest: &str) -> bool {
    rest.trim_start_matches('.').chars().next().is_none_or(|c| !handle_char(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(text: &str) -> Vec<String> {
        find(text).iter().map(|f| f.written.as_typed()).collect()
    }

    #[test]
    fn the_mention_being_typed_is_the_at_sign_and_name_around_the_caret() {
        // The caret after `@impl`: the whole name is the mention's, what is before the caret typed.
        let text = "ask @impl-alice now";
        let at = |caret: &str| text.find(caret).unwrap() + caret.len();
        assert_eq!(mention_at(text, at("@impl")), Some(Typing { range: 4..15, typed: "impl".into() }));
        assert_eq!(mention_at(text, at("@")), Some(Typing { range: 4..15, typed: String::new() }));
        assert_eq!(mention_at(text, at("@impl-alice")), Some(Typing { range: 4..15, typed: "impl-alice".into() }));
        assert_eq!(mention_at(text, at("ask")), None, "no at-sign before the caret");
        assert_eq!(mention_at(text, text.len()), None, "past the name, after a space");
        assert_eq!(mention_at("@", 1), Some(Typing { range: 0..1, typed: String::new() }), "at the start of the text");
        assert_eq!(mention_at("mail daniel@exam", 16), None, "an at-sign inside a word opens no mention");
        assert_eq!(mention_at("see `@over`", 9), None, "nor one in code");
        assert_eq!(mention_at("(@dn", 4), Some(Typing { range: 1..4, typed: "dn".into() }), "after an opening mark");
        assert_eq!(
            mention_at("é @d", 5),
            Some(Typing { range: 3..5, typed: "d".into() }),
            "bytes, whatever came before"
        );
    }

    #[test]
    fn a_stored_text_is_written_again_as_a_writer_types_it() {
        let (person, decision, task) = (EntityId::random(), EntityId::random(), EntityId::random());
        let as_typed = |id: EntityId| {
            if id == person {
                Some(Written::Handle("impl-alice".into()))
            } else if id == decision {
                Some(Written::Bracketed("D12".into()))
            } else {
                None
            }
        };
        let [p, d, t] = [person, decision, task].map(|id| id.to_base64());
        let short: String = t.chars().take(8).collect();
        let text = format!("[[{p}]], does [[{t}]] follow [[{d}]]? Ask [[{p}]]. `[[{t}]]` stays");
        let edited = editable(&text, as_typed);
        assert_eq!(edited, format!("@impl-alice, does [[{short}]] follow [[D12]]? Ask @impl-alice. `[[{t}]]` stays"));
        assert_eq!(written(&edited), vec!["@impl-alice", &format!("[[{short}]]"), "[[D12]]", "@impl-alice"]);
        // Where a handle would run into the text around it, the reference stays bracketed.
        let short_person: String = p.chars().take(8).collect();
        for (stored_text, expected) in [
            (format!("x[[{p}]]"), format!("x[[{short_person}]]")),
            (format!("[[{p}]]-ish"), format!("[[{short_person}]]-ish")),
            (format!("[[{p}]].x"), format!("[[{short_person}]].x")),
            (format!("[[{p}]][[{p}]]"), format!("@impl-alice[[{short_person}]]")),
        ] {
            assert_eq!(editable(&stored_text, as_typed), expected, "{stored_text}");
        }
    }

    #[test]
    fn handles_and_brackets_are_found_in_order() {
        assert_eq!(
            written("hey @dnorman, and @impl-alice: see [[D12]] and [[ vwugaN8h ]]. (@triager)"),
            vec!["@dnorman", "@impl-alice", "[[D12]]", "[[vwugaN8h]]", "@triager"]
        );
        assert_eq!(written("@dnorman."), vec!["@dnorman"], "a full stop ends the sentence, not the handle");
    }

    #[test]
    fn an_at_inside_a_word_or_with_no_handle_names_no_one() {
        assert!(written("email me at daniel@example.com").is_empty());
        assert!(written("meet @ noon, or @!").is_empty());
        assert!(written("[[ unclosed\n]]").is_empty(), "brackets close on the line they open");
    }

    #[test]
    fn code_is_text() {
        assert!(written("use `@Override` and `[[D12]]` here").is_empty());
        assert!(written("```\n@dnorman [[D12]]\n```\n").is_empty());
        assert_eq!(written("```rust\nlet a = b@c;\n```\nthen @dnorman"), vec!["@dnorman"]);
        assert_eq!(written("a lone ` backtick, @dnorman"), vec!["@dnorman"], "a run with no partner is text");
        assert_eq!(written("``a ` b`` then [[D1]]"), vec!["[[D1]]"]);
    }

    #[test]
    fn a_stored_text_names_full_ids_and_renders_them() {
        let (task, person) = (EntityId::random(), EntityId::random());
        let text = "look at [[x]] with @dnorman";
        let found = find(text);
        let stored_text = rewrite(text, &found, &[Named::entity(task), Named::entity(person)]);
        assert_eq!(stored_text, format!("look at [[{}]] with [[{}]]", task.to_base64(), person.to_base64()));
        assert_eq!(stored(&stored_text).iter().map(|(_, id)| *id).collect::<Vec<_>>(), vec![task, person]);
        let shown = render(&stored_text, |id| {
            if id == task {
                Some("[Retry on disconnect]".into())
            } else if id == person {
                Some("@dnorman".into())
            } else {
                None
            }
        });
        assert_eq!(shown, "look at [Retry on disconnect] with @dnorman");
        let unknown = format!("see [[{}]]", EntityId::random().to_base64());
        assert_eq!(render(&unknown, |_| None), unknown, "what cannot be named is left as it is");
    }

    #[test]
    fn a_file_mention_is_stored_as_its_highlights_id_and_the_version_it_names() {
        let (file, task) = (EntityId::random(), EntityId::random());
        let commit = "e13b8e216257b10dc4f3df4908ab34c26ed5497b";
        let text = "the primer [[docs/using-fwd.md@HEAD]], on disk [[docs/using-fwd.md]], for [[x]]";
        let named = [
            Named { id: file, version: Some(Version::Commit(commit.into())) },
            Named { id: file, version: Some(Version::Worktree) },
            Named::entity(task),
        ];
        let stored_text = rewrite(text, &find(text), &named);
        let (f, t) = (file.to_base64(), task.to_base64());
        assert_eq!(stored_text, format!("the primer [[{f}@{commit}]], on disk [[{f}@worktree]], for [[{t}]]"));
        assert_eq!(stored_named(&stored_text).into_iter().map(|(_, n)| n).collect::<Vec<_>>(), named);
        assert_eq!(stored(&stored_text).iter().map(|(_, id)| *id).collect::<Vec<_>>(), vec![file, file, task]);

        // Read whole, a file mention is named with its version; read by its id, as the highlight.
        let shown = render_named(&stored_text, |n| match &n.version {
            Some(version) => Some(format!("[docs/using-fwd.md {version}]")),
            None => Some("[a task]".into()),
        });
        assert_eq!(
            shown,
            format!("the primer [docs/using-fwd.md {commit}], on disk [docs/using-fwd.md worktree], for [a task]")
        );
        assert_eq!(
            render(&stored_text, |id| (id == file).then(|| "[docs/using-fwd.md]".to_string())),
            format!("the primer [docs/using-fwd.md], on disk [docs/using-fwd.md], for [[{t}]]")
        );

        // An edit box writes it back with its version, which reads again with no checkout.
        let start: String = f.chars().take(8).collect();
        let edited = editable(&stored_text, |_| None);
        let short_task: String = t.chars().take(8).collect();
        assert_eq!(
            edited,
            format!("the primer [[{start}@{commit}]], on disk [[{start}@worktree]], for [[{short_task}]]")
        );

        // Only a full id and a version a stored mention writes read as stored.
        for not_stored in [format!("[[{start}@index]]"), format!("[[{f}@HEAD]]"), "[[docs/cli.md@index]]".into()] {
            assert!(stored_named(&not_stored).is_empty(), "{not_stored}");
        }
        assert_eq!(Named::parse(&format!("{f}@index")), Some(Named { id: file, version: Some(Version::Index) }));
    }
}
