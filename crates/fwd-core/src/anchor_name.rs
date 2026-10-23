//! How a highlight (a code anchor: lines of a file, or a whole file, at a version) is named in
//! text, built here once so that the page, the VS Code extension (through fwd-host) and the CLI
//! name it alike. A name reads in the order a reader scans it: first where the highlight is,
//! `path:lines` (`src/ws.rs:4-8`, `src/ws.rs:4`, or the path alone for a whole file), since a
//! reader looks for the file first; then the version of the file its lines were chosen in, where
//! one is shown; then the first line of its code in a code span, where one is shown, since code is
//! the hardest part to read (Daniel, 2026-09-29: "I think we should reverse the order of code
//! references. I suggest filename:line-line `code` or something like that. to make it more
//! readable"; task ZYjo8Yyr). Until then a step's line began with its code, then "@", the path,
//! the version and the lines. A lozenge and a reference in a message's text show the place alone;
//! a step's line and a highlight's list item show all three. A file a message names at a version
//! (`file_mention.rs`) is named by its path and the version the mention names ([`AnchorName::file_at`]).

use crate::file_mention::Version;
use crate::model::CodeAnchorView;

/// About how many characters of a highlight's first line its name shows.
pub const FIRST_LINE: usize = 50;

/// What names a highlight, part by part in the order a name reads ([`AnchorName::parts`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnchorName {
    /// Where the highlight is: `path:5-7`, `path:5`, or the path alone for a whole file
    /// ([`place`]).
    pub place: String,
    /// The version of the file its lines were chosen in ([`version`]).
    pub version: String,
    /// Its first line that is not blank ([`first_line`]); none for a whole file, which highlights
    /// no lines, and for lines that are all blank.
    pub code: Option<String>,
}

/// One part of a name, for a surface that draws each part in a style of its own (the page draws
/// the place in monospace, the version as a pill and the code as code).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part<'a> {
    Place(&'a str),
    Version(&'a str),
    Code(&'a str),
}

impl AnchorName {
    /// The name of a highlight on `lines` of `path` (1-based and inclusive, as [`lines`] reads
    /// them; none for the whole file), chosen in the version `blob` (git's id for the file's
    /// content; none when the highlight recorded none), whose highlighted text is `text`.
    pub fn new(path: &str, lines: Option<(i64, i64)>, blob: Option<&str>, text: &str) -> AnchorName {
        let code = lines.map(|_| first_line(text, FIRST_LINE)).filter(|line| !line.is_empty());
        AnchorName { place: place(path, lines), version: version(blob), code }
    }

    /// The name of the anchor `a`, from what it stores: its version is its basis blob, or on an
    /// anchor of the first cut the HEAD blob it was mapped onto. A field that cannot be read names
    /// nothing, as an empty path, no version or no text, so a surface still draws what it has.
    pub fn of(a: &CodeAnchorView) -> AnchorName {
        let version = a.basis_blob().ok().flatten().or_else(|| a.blob_sha().ok().flatten());
        let text = crate::ops::text(a.anchored_text()).unwrap_or_default();
        let stored = crate::ops::lines_of(a).ok().flatten();
        AnchorName::new(&a.path().unwrap_or_default(), lines(stored), version.as_deref(), &text)
    }

    /// The name of the whole file at `path` as a mention names it at `version`: its path, then the
    /// version in a reader's words ([`file_version`]), `docs/cli.md e13b8e2`. The version is the
    /// mention's, not the highlight's basis, since one whole-file highlight serves every version
    /// that holds the same bytes, and the mention keeps the one its writer named.
    pub fn file_at(path: &str, version: &Version) -> AnchorName {
        AnchorName { place: path.to_string(), version: file_version(version), code: None }
    }

    /// The parts in the order the name reads: the place, the version, then the code when there is
    /// some.
    pub fn parts(&self) -> Vec<Part<'_>> {
        let mut parts = vec![Part::Place(&self.place), Part::Version(&self.version)];
        parts.extend(self.code.as_deref().map(Part::Code));
        parts
    }

    /// The name as plain text, a space between its parts and its code in a code span:
    /// ``src/ws.rs:4-8 2418c54 `fn reconnect() {` ``, or `src/net.rs 2418c54` for a whole file.
    pub fn text(&self) -> String {
        let written: Vec<String> = self
            .parts()
            .into_iter()
            .map(|part| match part {
                Part::Place(plain) | Part::Version(plain) => plain.to_string(),
                Part::Code(code) => code_span(code),
            })
            .collect();
        written.join(" ")
    }
}

/// The lines of a highlight as a person reads them, 1-based and inclusive, from the lines its
/// anchor stores (`ops::lines_of`: 0-based, the end exclusive, and none on a whole file); none for
/// an empty range too, which names no line (docs/DECISIONS.md, D43, "A whole-file reference is an
/// anchor with no line range").
pub fn lines(stored: Option<(i64, i64)>) -> Option<(i64, i64)> {
    stored.filter(|(start, end)| end > start).map(|(start, end)| (start + 1, end))
}

/// Where a highlight is: `path:5-7`, `path:5` for one line, or the path alone for a whole file.
pub fn place(path: &str, lines: Option<(i64, i64)>) -> String {
    match lines {
        Some((first, last)) if first == last => format!("{path}:{first}"),
        Some((first, last)) => format!("{path}:{first}-{last}"),
        None => path.to_string(),
    }
}

/// The version a highlight's lines were chosen in, as a name shows it: the first seven characters
/// of `blob`, git's id for the file's content they were chosen in, or "working tree" for an anchor
/// of the first cut made on a file with changes, which recorded no version.
pub fn version(blob: Option<&str>) -> String {
    match blob.filter(|blob| !blob.is_empty()) {
        Some(blob) => blob.chars().take(7).collect(),
        None => "working tree".into(),
    }
}

/// The version of a file a mention names, as its name shows it: the commit's first seven
/// characters, "index" for the copy the index held, or "on disk" for the file as it stood in the
/// working tree, unstaged changes included.
pub fn file_version(version: &Version) -> String {
    match version {
        Version::Worktree => "on disk".into(),
        Version::Index => "index".into(),
        Version::Commit(commit) => commit.chars().take(7).collect(),
    }
}

/// What the highlighted text begins with: its first line that is not blank, without the
/// indentation, cut at `max` characters with an ellipsis after them.
pub fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("");
    let mut cut: String = line.chars().take(max).collect();
    if line.chars().count() > max {
        cut.push('…');
    }
    cut
}

/// `code` as a Markdown code span that reads it as written: fenced by one backtick more than the
/// longest run of backticks inside it, and set off by a space from a fence it would otherwise
/// run into.
fn code_span(code: &str) -> String {
    let longest = code.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    let pad = if code.starts_with('`') || code.ends_with('`') { " " } else { "" };
    format!("{fence}{pad}{code}{pad}{fence}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOB: &str = "2418c540aa55b7e1";
    const TEXT: &str = "\n    fn reconnect() {\n        loop {\n";

    #[test]
    fn a_name_reads_the_place_then_the_version_then_the_first_line_of_code() {
        // Lines 4 to 8 as an anchor stores them: 0-based start, exclusive end.
        let name = AnchorName::new("src/ws.rs", lines(Some((3, 8))), Some(BLOB), TEXT);
        assert_eq!(
            name.parts(),
            [Part::Place("src/ws.rs:4-8"), Part::Version("2418c54"), Part::Code("fn reconnect() {")]
        );
        assert_eq!(name.text(), "src/ws.rs:4-8 2418c54 `fn reconnect() {`");
        let one = AnchorName::new("src/ws.rs", lines(Some((3, 4))), Some(BLOB), "let x = 1;");
        assert_eq!(one.text(), "src/ws.rs:4 2418c54 `let x = 1;`");
    }

    #[test]
    fn a_whole_file_is_named_by_its_path_and_version_alone() {
        assert_eq!(lines(None), None);
        assert_eq!(lines(Some((0, 0))), None, "an empty range names no line");
        let whole = AnchorName::new("src/net.rs", None, Some(BLOB), "");
        assert_eq!(whole.parts(), [Part::Place("src/net.rs"), Part::Version("2418c54")]);
        assert_eq!(whole.text(), "src/net.rs 2418c54");
        let blank = AnchorName::new("src/ws.rs", lines(Some((3, 5))), Some(BLOB), "  \n\t\n");
        assert_eq!(blank.text(), "src/ws.rs:4-5 2418c54", "lines all blank show no code");
    }

    #[test]
    fn a_file_a_mention_names_is_named_by_its_path_and_the_version_the_mention_names() {
        let commit = Version::Commit("e13b8e216257b10dc4f3df4908ab34c26ed5497b".into());
        let at_commit = AnchorName::file_at("docs/using-fwd.md", &commit);
        assert_eq!(at_commit.parts(), [Part::Place("docs/using-fwd.md"), Part::Version("e13b8e2")]);
        assert_eq!(at_commit.text(), "docs/using-fwd.md e13b8e2");
        assert_eq!(AnchorName::file_at("docs/cli.md", &Version::Index).text(), "docs/cli.md index");
        assert_eq!(AnchorName::file_at("docs/cli.md", &Version::Worktree).text(), "docs/cli.md on disk");
    }

    #[test]
    fn a_highlight_that_recorded_no_version_was_chosen_in_the_working_tree() {
        assert_eq!(version(Some(BLOB)), "2418c54");
        assert_eq!(version(None), "working tree");
        assert_eq!(version(Some("")), "working tree");
        let name = AnchorName::new("src/ws.rs", lines(Some((3, 8))), None, TEXT);
        assert_eq!(name.text(), "src/ws.rs:4-8 working tree `fn reconnect() {`");
    }

    #[test]
    fn the_first_line_is_cut_at_about_fifty_characters() {
        let long = format!("    let {} = 1;", "x".repeat(80));
        let cut = first_line(&long, FIRST_LINE);
        assert_eq!(cut.chars().count(), 51, "fifty characters and an ellipsis");
        assert!(cut.starts_with("let xxx") && cut.ends_with('…'));
        assert_eq!(first_line("   \n\t\n", FIRST_LINE), "", "nothing but blank lines");
    }

    #[test]
    fn code_with_backticks_keeps_them_inside_its_code_span() {
        let markdown = AnchorName::new("docs/cli.md", Some((3, 3)), Some(BLOB), "- `fwd step list` prints ``[x]``");
        assert_eq!(markdown.text(), "docs/cli.md:3 2418c54 ``` - `fwd step list` prints ``[x]`` ```");
        let edge = AnchorName::new("a.js", Some((1, 1)), Some(BLOB), "`${x}`");
        assert_eq!(edge.text(), "a.js:1 2418c54 `` `${x}` ``");
    }
}
