//! What the inbox's keyboard handling needs to know about a key press and about the element that
//! has the focus.

use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlElement, HtmlInputElement, HtmlTextAreaElement, KeyboardEvent};

/// Enter without Shift, outside an input method's composition (whose Enter confirms the
/// composed text): it sends the box it is pressed in, and Shift+Enter starts a new line.
pub fn is_send(ev: &KeyboardEvent) -> bool {
    // 229 is the key code a browser reports for a key press that belongs to a composition.
    ev.key() == "Enter" && !ev.shift_key() && !ev.is_composing() && ev.key_code() != 229
}

/// No modifier held. A plain arrow moves between cards; with a modifier it keeps its usual
/// meaning (extending a text selection, jumping to the start of the text).
pub fn is_plain(ev: &KeyboardEvent) -> bool {
    !(ev.shift_key() || ev.alt_key() || ev.ctrl_key() || ev.meta_key())
}

/// Whether the caret is on the first line of a text box, and whether it is on the last. Lines
/// are the ones the text breaks into (a long line the box wraps counts as one line), and an
/// empty box is both. A selection that spans text is on neither, so the arrows first collapse
/// it as usual.
pub fn caret_lines(textarea: &HtmlTextAreaElement) -> (bool, bool) {
    // The selection offsets count UTF-16 code units, so the text is counted in them too.
    let text: Vec<u16> = textarea.value().encode_utf16().collect();
    let start = textarea.selection_start().ok().flatten().unwrap_or(0) as usize;
    let end = textarea.selection_end().ok().flatten().unwrap_or(0) as usize;
    caret_edges(&text, start, end)
}

/// `caret_lines` for `text` in UTF-16 code units with the selection running from `start` to `end`.
fn caret_edges(text: &[u16], start: usize, end: usize) -> (bool, bool) {
    if start != end {
        return (false, false);
    }
    let newline = u16::from(b'\n');
    let at = start.min(text.len());
    (!text[..at].contains(&newline), !text[at..].contains(&newline))
}

/// A control that takes typing or uses the arrow keys itself: a text box, a text input, a select,
/// or editable content. The page's own keys leave it alone.
pub fn is_text_field(element: &Element) -> bool {
    match element.tag_name().as_str() {
        "TEXTAREA" | "SELECT" => true,
        "INPUT" => element.dyn_ref::<HtmlInputElement>().is_none_or(|input| {
            !matches!(input.type_().as_str(), "checkbox" | "radio" | "button" | "submit" | "reset")
        }),
        _ => element.dyn_ref::<HtmlElement>().is_some_and(HtmlElement::is_content_editable),
    }
}

#[cfg(test)]
mod tests {
    use super::caret_edges;

    fn utf16(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn the_caret_is_on_the_first_or_last_line_by_line_breaks() {
        let text = utf16("ab\ncd");
        assert_eq!(caret_edges(&text, 1, 1), (true, false));
        assert_eq!(caret_edges(&text, 3, 3), (false, true));
        assert_eq!(caret_edges(&utf16(""), 0, 0), (true, true), "an empty box is both");
        assert_eq!(caret_edges(&utf16("one line"), 3, 3), (true, true));
        assert_eq!(caret_edges(&text, 0, 4), (false, false), "a selection collapses first");
    }

    #[test]
    fn offsets_count_utf16_code_units() {
        // The emoji takes two code units, so offset 2 is just after it, still on the first line.
        let text = utf16("\u{1F600}\nx");
        assert_eq!(caret_edges(&text, 2, 2), (true, false));
        assert_eq!(caret_edges(&text, 3, 3), (false, true));
    }
}
