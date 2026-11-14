//! The two kinds of string the view's document holds, so that whatever draws it can trust them: a
//! title or a tag's text is plain text on one line, and each of `lines` one Markdown block. Neither
//! holds a control character but the newline between a block's own lines, nor a bidirectional
//! embedding, override or isolate, since a pane renders the strings as they are and those would draw
//! a text as something other than it is. Each kind has a cap, so that the document's size is bounded
//! whatever the records hold.

use super::markdown;

/// How many characters of a record's own text the view shows: past it the text is cut, saying so,
/// so that one long task or question cannot crowd out the rest of the document.
pub(super) const TEXT_LIMIT: usize = 1_500;
/// How many characters of a principal's or a record's title the view shows.
pub(super) const TITLE_LIMIT: usize = 200;
/// How many characters of a tag's text the view shows.
pub(super) const TAG_LIMIT: usize = 80;
/// How many characters of a line the view says of a list, of a row or of the whole document shows,
/// a reason it gives among them.
pub(super) const LINE_LIMIT: usize = 400;

/// Whether `c` may stand in a string of the document: no control character, and no bidirectional
/// embedding, override or isolate; a zero-width joiner, which some scripts need, is kept.
fn kept(c: char) -> bool {
    !c.is_control() && !matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// `text` on one line: its whitespace, newlines included, made single spaces, and every character
/// [`kept`] refuses left out. What `fwd --format pane` says of a failure too.
pub fn one_line(text: &str) -> String {
    let kept: String = text.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).filter(|c| kept(*c)).collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` as plain text on one line ([`one_line`]), cut to `limit` characters, the cut marked.
pub(super) fn plain(text: &str, limit: usize) -> String {
    let line = one_line(text);
    if line.chars().count() <= limit {
        return line;
    }
    let mut cut: String = line.chars().take(limit.saturating_sub(1)).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

/// `text` as one Markdown block, or none when it holds nothing but whitespace: its line breaks
/// made newlines, its tabs four spaces, every character [`kept`] refuses but the newline left out,
/// what could act or mislead shown as text ([`markdown::neutral`]), and, past [`TEXT_LIMIT`]
/// characters, cut at the last whitespace before the limit, with a code block it cut open closed
/// again and the cut said after it.
pub(super) fn block(text: &str) -> Option<String> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n").replace('\t', "    ");
    let kept: String = text.chars().filter(|c| *c == '\n' || kept(*c)).collect();
    let kept = markdown::neutral(kept.trim());
    if kept.is_empty() {
        return None;
    }
    let length = kept.chars().count();
    if length <= TEXT_LIMIT {
        return Some(kept);
    }
    let limit = kept.char_indices().nth(TEXT_LIMIT).map_or(kept.len(), |(at, _)| at);
    let cut = match kept[..limit].rfind(char::is_whitespace) {
        Some(at) if at > 0 => &kept[..at],
        _ => &kept[..limit],
    };
    let cut = cut.trim_end();
    let mut block = cut.to_string();
    // A fence left open would draw the rest of the block, and the cut's sentence, as code.
    if block.matches("```").count() % 2 == 1 {
        block.push_str("\n```");
    }
    let shown = cut.chars().count();
    block.push_str(&format!("\n\n*Cut at {shown} of {length} characters; the page shows it whole.*"));
    Some(block)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_one_line_without_control_or_direction_characters_and_capped() {
        assert_eq!(plain("Retry on\n\tdisconnect\u{1b}[31m  now\u{7}", 80), "Retry on disconnect[31m now");
        assert_eq!(plain(" \r\n ", 80), "");
        assert_eq!(plain("abc\u{202E}def\u{2066}g\u{200D}h", 80), "abcdefg\u{200D}h", "a joiner stays");
        assert_eq!(plain(&"x".repeat(300), 200).chars().count(), 200);
        assert!(plain(&"word ".repeat(100), 20).ends_with('…'));
    }

    #[test]
    fn a_block_keeps_its_lines_and_loses_every_other_control_character() {
        assert_eq!(
            block("First line\r\nsecond\tline\u{0}\rthird\u{1b}\u{202E}"),
            Some("First line\nsecond    line\nthird".into())
        );
        assert_eq!(block("  \n\t "), None, "nothing but whitespace is no block");
        let short = "x".repeat(TEXT_LIMIT);
        assert_eq!(block(&short), Some(short.clone()), "a text at the limit is not cut");
    }

    #[test]
    fn a_long_block_is_cut_at_a_word_saying_so_and_closes_a_code_block_it_cut_open() {
        let words = "word ".repeat(TEXT_LIMIT);
        let cut = block(&words).expect("a block");
        let (kept, said) = cut.split_once("\n\n").expect("the cut is said after the text");
        assert!(kept.chars().count() <= TEXT_LIMIT && kept.ends_with("word"), "{kept}");
        assert_eq!(
            said,
            format!("*Cut at {} of {} characters; the page shows it whole.*", kept.len(), words.trim().len())
        );

        let code = format!("Run this:\n```\n{}", "let x = 1;\n".repeat(TEXT_LIMIT));
        let cut = block(&code).expect("a block");
        assert_eq!(cut.matches("```").count(), 2, "the open fence is closed: {cut}");
        assert!(cut.ends_with("the page shows it whole.*"));
    }
}
