//! What in a record's own Markdown could act or mislead, shown as the text it is: an image, which a
//! renderer would fetch; raw HTML; and a link whose address is not `http:` or `https:` (a `file:`
//! link, which the pane's renderer opens, or a `javascript:` one), whether inline, an autolink or a
//! reference definition. Code spans and fenced blocks are left as they are, since nothing in them
//! renders. This is the cheap part of a safe subset, by escaping: the schemes a drawn link may have
//! are still the drawer's to enforce (docs/design/cli.md, "The person's view of a session").

use crate::references::code_ranges;
use std::collections::BTreeSet;
use std::ops::Range;

/// `text` with a backslash before each character that would make an image, raw HTML or a link to an
/// address other than the web.
pub(super) fn neutral(text: &str) -> String {
    let code = code_ranges(text);
    let escape = to_escape(text, &code);
    let mut out = String::with_capacity(text.len() + escape.len());
    for (at, c) in text.char_indices() {
        if escape.contains(&at) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The byte positions of `text`, outside `code`, of the characters [`neutral`] escapes.
fn to_escape(text: &str, code: &[Range<usize>]) -> BTreeSet<usize> {
    let bytes = text.as_bytes();
    let mut escape = BTreeSet::new();
    let mut at = 0;
    while at < text.len() {
        if let Some(range) = code.iter().find(|r| r.contains(&at)) {
            at = range.end;
            continue;
        }
        match bytes[at] {
            // An escaped character stays as its writer escaped it.
            b'\\' => at += 1 + text[at + 1..].chars().next().map_or(0, char::len_utf8),
            b'!' if bytes.get(at + 1) == Some(&b'[') => {
                escape.extend([at, at + 1]);
                at += 2;
            }
            b'<' => {
                escape.insert(at);
                at += 1;
            }
            b'[' => {
                if let Some(close) = closing(text, at, code) {
                    let after = &text[close + 1..];
                    let address = match after.as_bytes().first() {
                        Some(b'(') => Some(inline_address(&after[1..])),
                        Some(b':') if opens_line(text, at) => Some(defined_address(&after[1..])),
                        _ => None,
                    };
                    if address.is_some_and(|address| !on_the_web(address)) {
                        escape.extend([at, close]);
                    }
                }
                at += 1;
            }
            _ => at += text[at..].chars().next().map_or(1, char::len_utf8),
        }
    }
    escape
}

/// Where the `]` that closes the `[` at `open` is, brackets nested and escapes skipped, within the
/// paragraph and outside code; none for a bracket nothing closes.
fn closing(text: &str, open: usize, code: &[Range<usize>]) -> Option<usize> {
    let mut depth = 0usize;
    let mut chars = text[open..].char_indices().map(|(i, c)| (open + i, c));
    while let Some((at, c)) = chars.next() {
        if code.iter().any(|r| r.contains(&at)) || text[at..].starts_with("\n\n") {
            return None;
        }
        match c {
            '\\' => {
                chars.next();
            }
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
    }
    None
}

/// The address of an inline link, from just after its `(`: within angle brackets, or up to the
/// first whitespace or the `)` that balances it.
fn inline_address(after: &str) -> &str {
    let after = after.trim_start_matches([' ', '\t']);
    if let Some(inside) = after.strip_prefix('<') {
        return inside.split('>').next().unwrap_or("");
    }
    let mut depth = 0usize;
    for (at, c) in after.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => return &after[..at],
            ')' => depth -= 1,
            c if c.is_whitespace() => return &after[..at],
            _ => {}
        }
    }
    after
}

/// The address of a link reference definition, from just after its `:`.
fn defined_address(after: &str) -> &str {
    let address = after.split_whitespace().next().unwrap_or("");
    address.strip_prefix('<').map_or(address, |inside| inside.split('>').next().unwrap_or(""))
}

/// Whether the `[` at `at` opens its line, as a reference definition's does, after at most three
/// spaces.
fn opens_line(text: &str, at: usize) -> bool {
    let start = text[..at].rfind('\n').map_or(0, |newline| newline + 1);
    let indent = &text[start..at];
    indent.len() <= 3 && indent.bytes().all(|b| b == b' ')
}

/// Whether `address` is on the web, its scheme `http` or `https`.
fn on_the_web(address: &str) -> bool {
    address
        .split_once(':')
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A link to the web stays a link; one to a file, to script, to a relative address, an
    /// autolink, an image and raw HTML are shown as the text they are; code is left as it is.
    #[test]
    fn what_could_act_or_mislead_is_shown_as_its_text_and_web_links_stay() {
        let cases = [
            ("[the page](https://example.com/a)", "[the page](https://example.com/a)"),
            ("[Answer](http://127.0.0.1:1/?p=x#/tasks/y)", "[Answer](http://127.0.0.1:1/?p=x#/tasks/y)"),
            ("[open](file:///etc/hosts)", r"\[open\](file:///etc/hosts)"),
            ("[run](javascript:alert(1))", r"\[run\](javascript:alert(1))"),
            ("[here](notes.md)", r"\[here\](notes.md)"),
            ("<file:///etc/hosts> and <b>bold</b>", r"\<file:///etc/hosts> and \<b>bold\</b>"),
            ("![cat](https://example.com/cat.png)", r"\!\[cat](https://example.com/cat.png)"),
            ("[ref]: file:///etc/hosts\n\nSee [ref].", "\\[ref\\]: file:///etc/hosts\n\nSee [ref]."),
            ("[ref]: https://example.com\n\nSee [ref].", "[ref]: https://example.com\n\nSee [ref]."),
            (
                "Run `[x](file:///a)` or\n```\n<b>![i](file:///b)</b>\n```",
                "Run `[x](file:///a)` or\n```\n<b>![i](file:///b)</b>\n```",
            ),
            (r"Already \[escaped](file:///a)", r"Already \[escaped](file:///a)"),
            ("a [plain] bracket and a [[stored]] one", "a [plain] bracket and a [[stored]] one"),
        ];
        for (written, shown) in cases {
            assert_eq!(neutral(written), shown, "{written}");
        }
    }
}
