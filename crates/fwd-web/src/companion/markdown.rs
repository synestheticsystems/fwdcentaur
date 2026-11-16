//! A Markdown block of the session view's document as the HTML the companion view puts in place.
//! The document already shows as text what could act or mislead (`fwd_core::session_view`'s
//! markdown module), and the page holds its own line all the same, since anyone who writes a record
//! writes these words: raw HTML shows as the text it is, an image as its description, and a link
//! stays a link only to an address on the web, as written or once its escapes are read, and opens
//! in a new tab, so that the view stays beside the chat. Every other character is escaped as HTML
//! escapes text, and a line break the writer typed stays a line break, as the page's paragraphs keep
//! their lines.

use pulldown_cmark::{html, Event, Options, Parser, Tag, TagEnd};

/// `block` as HTML.
pub fn html(block: &str) -> String {
    // Whether each link opened so far, and not yet closed, is kept as a link.
    let mut links: Vec<bool> = Vec::new();
    let events = Parser::new_ext(block, Options::empty()).filter_map(|event| match event {
        Event::Html(raw) | Event::InlineHtml(raw) => Some(Event::Text(raw)),
        // The image's description, which the events between these two give as text, stays.
        Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => None,
        Event::Start(Tag::Link { link_type, dest_url, title, id }) => {
            let web = on_the_web(&dest_url);
            links.push(web);
            web.then_some(Event::Start(Tag::Link { link_type, dest_url, title, id }))
        }
        Event::End(TagEnd::Link) => links.pop().unwrap_or(false).then_some(Event::End(TagEnd::Link)),
        other => Some(other),
    });
    let mut out = String::new();
    html::push_html(&mut out, events);
    // Every other `<` the text holds is escaped, so this begins a link and nothing else.
    out.replace("<a href=\"", "<a target=\"_blank\" rel=\"noopener noreferrer\" href=\"")
}

/// Whether `address` is on the web: it starts with `http://` or `https://`, in any letter case.
fn on_the_web(address: &str) -> bool {
    let scheme = address.get(..8).unwrap_or(address).to_ascii_lowercase();
    scheme.starts_with("http://") || scheme.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::html;

    /// The writer's paragraphs and line breaks stay, and code shows as code, escaped.
    #[test]
    fn paragraphs_keep_their_lines_and_code_is_escaped() {
        assert_eq!(
            html("Water the ferns:\nthe tall ones first\n\nthen `a<b`"),
            "<p>Water the ferns:\nthe tall ones first</p>\n<p>then <code>a&lt;b</code></p>\n"
        );
    }

    /// A link to the web opens in a new tab; a link anywhere else, spelled out or behind escapes,
    /// and an image, show as their words; raw HTML shows as the text it is.
    #[test]
    fn only_a_link_to_the_web_is_a_link_and_nothing_else_acts() {
        assert_eq!(
            html("See [the task](http://127.0.0.1:4242/?p=1#/tasks/x)."),
            "<p>See <a target=\"_blank\" rel=\"noopener noreferrer\" href=\"http://127.0.0.1:4242/?p=1#/tasks/x\">the task</a>.</p>\n"
        );
        assert_eq!(html("[HTTPS](HTTPS://example.com/)").matches("<a ").count(), 1, "in any letter case");
        for elsewhere in ["[run](javascript:alert(1))", "[run](&#106;avascript:alert(1))", "[read](file:///etc/passwd)"]
        {
            assert_eq!(
                html(elsewhere),
                format!("<p>{}</p>\n", &elsewhere[1..elsewhere.find(']').unwrap()]),
                "{elsewhere}"
            );
        }
        assert_eq!(html("![a cat](http://example.com/cat.png)"), "<p>a cat</p>\n");
        assert_eq!(html("<b onclick=\"x()\">bold</b>"), "<p>&lt;b onclick=\"x()\"&gt;bold&lt;/b&gt;</p>\n");
        assert_eq!(html("<div>\nblock\n</div>"), "&lt;div&gt;\nblock\n&lt;/div&gt;");
    }
}
