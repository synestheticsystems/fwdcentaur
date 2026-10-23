//! A highlight (a code anchor: lines of a file, or a whole file, at a basis) in the modes the
//! page draws it in besides its lozenge (`lozenges.rs`) and its page (`anchors.rs`): its list item
//! (its name as every surface gives it, fwd-core's `anchor_name`: where it is, `path:lines`, in
//! monospace, the basis as a pill, then the first line of the highlighted text cut at about fifty
//! characters, as code; Daniel, 2026-09-29, "I think we should reverse the order of code
//! references. I suggest filename:line-line `code` or something like that. to make it more
//! readable", where his sketch of 2026-09-28 had put the text first, "[x] `highlight content first
//! ~50 nonbreaking chars` @ foo.rs version lines xx-xx"), and its row, the embedded mode: the
//! highlighted text, its lozenges, and beneath it its threads (the messages whose `anchor` is this
//! highlight and that answer no other message), with a box that opens a new thread on it. A
//! whole-file highlight has no lines: it shows as its file.

use crate::lozenges::{self, Target};
use crate::referents::{Child, Listing, Models};
use crate::session::ctx;
use crate::thread::{self, Actions, Editor, Link, Model, Query, RowAt, Sending, ThreadState};
use crate::{empty, fmt, store};
use ankurah::EntityId;
use fwd_core::anchor_name::{self, AnchorName, Part};
use fwd_core::file_mention::Version;
use fwd_core::model::{CodeAnchorView, MessageView, Priority, RelationKind};
use fwd_core::ops::{self, MessageContext};
use fwd_core::rules;
use leptos::prelude::*;

/// What the version's pill says of itself.
pub const VERSION_TITLE: &str = "the version the lines were chosen in";

/// The links a highlight follows to the messages beneath it: the first message of each thread.
pub const FOLLOWS: &[Link] = &[Link::AnchorComments];

/// The live query behind a highlight's threads.
pub fn open(parent: EntityId, link: Link) -> Option<Query> {
    (link == Link::AnchorComments).then(|| thread::messages_where("anchor = ?", parent))
}

/// The messages beneath the highlight `parent` through `link`: each that is on it and answers no
/// other message (a reply belongs under what it answers).
pub fn listing(parent: EntityId, link: Link, query: Option<&Query>) -> Listing<EntityId, Model, Link> {
    match link {
        Link::AnchorComments => thread::from_messages(query, |m| {
            let on_anchor = m.anchor().ok().flatten().map(EntityId::from) == Some(parent);
            let answers_another = m.reply_to().ok().flatten().is_some();
            (on_anchor && !answers_another).then(|| Child {
                id: m.id(),
                models: Models::one(Model::Message),
                link,
                label: String::new(),
                created: m.created().unwrap_or(0),
            })
        }),
        other => Listing::Failed(format!("a highlight does not follow {other:?}")),
    }
}

/// What the page shows of a highlight, read once from its anchor.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    /// Its name: where it is, the version its lines were chosen in, its first line.
    pub name: AnchorName,
    /// Its lines, 1-based and inclusive; none for a whole file.
    pub lines: Option<(i64, i64)>,
    /// The highlighted text.
    pub text: String,
    /// git's id for the content it was made on, whole: for a whole file, what prints the file.
    pub basis: Option<String>,
}

impl Shown {
    pub fn of(a: &CodeAnchorView) -> Shown {
        Shown {
            name: AnchorName::of(a),
            lines: anchor_name::lines(ops::lines_of(a).ok().flatten()),
            text: ops::text(a.anchored_text()).unwrap_or_default(),
            basis: a.basis_blob().ok().flatten(),
        }
    }

    /// Its name without the first line, `src/ws.rs:4-8 2418c54`, where its lines show beneath.
    pub fn heading(&self) -> String {
        AnchorName { code: None, ..self.name.clone() }.text()
    }

    /// What shows in place of a whole file's lines, which the page does not hold: that the page
    /// does not show a file, and the command that prints this one, `git show` and its blob, which
    /// fwd pins in the repository under `refs/fwd/basis/` (the lead's ruling on task nVE7KTeSqiX0,
    /// 2026-09-30; task XftyHmxT would have the daemon read it for the page).
    pub fn whole_file_note(&self) -> String {
        match &self.basis {
            Some(blob) => format!(
                "The whole file: the page does not show a file's content; `git show {blob}` prints it as this highlight holds it, in a checkout of its repository."
            ),
            None => "The whole file: the page does not show a file's content.".into(),
        }
    }
}

/// `path:lines`, to name a highlight where it shows again and in its lozenge.
pub fn name(id: EntityId) -> Option<String> {
    Some(AnchorName::of(&store::anchor(id)?).place)
}

/// The path of the whole-file highlight `id` and the version a mention names it at, `docs/cli.md
/// e13b8e2`, to name a file a message names (`fwd_core::file_mention`) where it shows again and in
/// its lozenge; none while the page does not hold the highlight.
pub fn file_name(id: EntityId, version: &Version) -> Option<String> {
    Some(AnchorName::file_at(&store::anchor(id)?.path().ok()?, version).text())
}

/// The page's messages on the highlight `id`.
fn messages_on(id: EntityId) -> Vec<MessageView> {
    store::messages().into_iter().filter(|m| m.anchor().ok().flatten().map(EntityId::from) == Some(id)).collect()
}

/// The first message of the first thread on the highlight `id` (`rules::first_thread`), from the
/// page's messages: the thread that moves with the highlight's steps (docs/DECISIONS.md, D99,
/// "Checking a step resolves its highlight's first thread, and resolving that thread checks the
/// step; each control says it does both"). None while no one has started a thread on it.
pub fn first_thread(id: EntityId) -> Option<MessageView> {
    let on = messages_on(id);
    let read: Vec<rules::ThreadMessage> = on.iter().filter_map(|m| ops::thread_message(m).ok()).collect();
    let first = rules::first_thread(&read)?;
    on.into_iter().find(|m| m.id() == first)
}

/// What retiring the highlight `id` moves as its step `leaving` comes off a checklist
/// (`rules::retire_highlight`), worked out from the page's messages and relations as
/// `ops::remove_step` works it out from the graph: the threads it resolves, and the highlight's
/// steps on other checklists its first thread checks. So the choice to retire it says what it does
/// before anyone makes it (task 9dvGW0EI, "Take a step off a checklist and retire its highlight in
/// one command: fwd step remove").
pub fn retiring(id: EntityId, leaving: EntityId) -> rules::Retired {
    let read: Vec<rules::ThreadMessage> = messages_on(id).iter().filter_map(|m| ops::thread_message(m).ok()).collect();
    let steps: Vec<rules::StepEdge> = store::relations()
        .iter()
        .filter(|r| r.from_id().ok() == Some(id) && r.kind().ok() == Some(RelationKind::StepOf))
        .filter_map(|r| ops::step_edge(r).ok())
        .collect();
    rules::retire_highlight(&read, &steps, leaving).unwrap_or_default()
}

/// The lozenge mode: code as `path:lines` in monospace.
pub fn lozenge(id: EntityId) -> AnyView {
    lozenges::single(Target::Anchor(id))
}

/// The list item mode: the highlight's name, `` ws.rs:4-8 1a2b3c4 `fn reconnect() {` ``, its
/// parts in the order fwd-core's `anchor_name` gives them, with a space between: the place in
/// monospace, the version as a pill, the first line as code. A whole-file highlight shows as its
/// file and version.
pub fn list_item(id: EntityId) -> AnyView {
    (move || {
        let Some(a) = store::anchor(id) else {
            let absent = format!("{}: {}", fmt::short(id), empty::absent(Some(Model::Anchor)));
            return view! { <span class="list-item muted">{absent}</span> }.into_any();
        };
        let name = AnchorName::of(&a);
        let mut drawn = Vec::new();
        for part in name.parts() {
            if !drawn.is_empty() {
                drawn.push(" ".into_any());
            }
            drawn.push(match part {
                Part::Place(place) => view! { <span class="mono anchor-path">{place.to_string()}</span> }.into_any(),
                Part::Version(version) => {
                    view! { <span class="pill basis" title=VERSION_TITLE>{version.to_string()}</span> }.into_any()
                }
                Part::Code(code) => view! { <code class="anchor-first">{code.to_string()}</code> }.into_any(),
            });
        }
        view! { <span class="list-item anchor-item">{drawn}</span> }.into_any()
    })
    .into_any()
}

/// The highlighted lines, each with its number, as the embedded mode and the page show them.
pub fn text_view(shown: &Shown) -> AnyView {
    let Some((first, _)) = shown.lines else {
        return view! { <p class="muted anchor-text-none">{shown.whole_file_note()}</p> }.into_any();
    };
    let lines = shown
        .text
        .lines()
        .enumerate()
        .map(|(n, line)| {
            let number = first + n as i64;
            view! { <span class="line"><span class="ln" aria-hidden="true">{number}</span>{line.to_string()}</span> }
        })
        .collect::<Vec<_>>();
    view! { <pre class="anchor-text"><code>{lines}</code></pre> }.into_any()
}

/// The embedded mode: the highlight's row, its threads beneath it.
pub fn embedded(state: ThreadState, at: RowAt) -> AnyView {
    view! { <AnchorRow state=state at=at/> }.into_any()
}

#[component]
fn AnchorRow(state: ThreadState, at: RowAt) -> impl IntoView {
    let id = at.id;
    // Changes only when the anchor arrives or goes; it never changes its lines or its text.
    let anchor = Memo::new(move |_| store::anchor(id).map(|a| Shown::of(&a)));
    let head = move || {
        anchor.with(|shown| match shown {
            Some(shown) => shown.heading(),
            None => format!("{}: {}", fmt::short(id), empty::absent(Some(Model::Anchor))),
        })
    };
    let text = move || anchor.with(|shown| shown.as_ref().map(text_view));
    let edit = state.edit(&at);
    let strip = at.strip();
    let line = view! {
        <div class="anchor-row">
            <a class="mono anchor-caption" href=crate::route::Route::Anchor(id).hash()
                on:click=move |ev| {
                    // In the VS Code webview the editor shows the lines instead.
                    if crate::session::reveal(id, None) {
                        ev.prevent_default();
                    }
                }>
                {head}
            </a>
            " "
            {strip}
            {text}
            <span class="row-actions">
                <button class="btn subtle tiny" on:click=move |_| edit.run(())>"comment"</button>
            </span>
        </div>
    }
    .into_any();
    thread::frame(state, at, line, Actions { editor: Some(COMMENT), activate: Some(open_anchor) })
}

/// A highlight's comment box: what is written in it opens a new thread on the highlight, a message
/// about the highlight alone (its `anchor` set, no task), so it sits in no task's thread
/// (docs/DECISIONS.md, D41, "A step is an edge from any entity to a task, and the highlight is the
/// step"). Routing tells of it as of any comment on code: the triage agent of the scope of the tasks
/// it is a step of or is near, else the one that serves the whole project, and the assignees of the
/// tasks anchored near it hear of it, and, when the highlight is a step, so do each such task's
/// creator and assignee (docs/design/agents-and-inbox.md, rules 1 and 14). On a
/// step's highlight it joins, for each of them, the one notification that collects what its author
/// does on that task (rule 10; docs/DECISIONS.md, D95, "A burst of comments and additions from one
/// author on one task reaches a person as one notification").
const COMMENT: Editor = Editor { placeholder: "comment on this highlight", send: send_comment };

fn send_comment(me: EntityId, anchor: EntityId, text: String) -> Sending {
    Box::pin(async move {
        let mc = MessageContext { anchor_id: Some(anchor), ..Default::default() };
        ops::post_message(&ctx(), me, &text, Priority::Next, mc).await.map_err(|e| e.to_string())
    })
}

fn open_anchor(id: EntityId) {
    lozenges::follow(Target::Anchor(id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_highlights_heading_is_its_name_without_its_first_line() {
        let shown = Shown {
            name: AnchorName::new("src/ws.rs", Some((4, 8)), Some("1a2b3c4d5e6f"), "fn reconnect() {"),
            lines: Some((4, 8)),
            text: "fn reconnect() {".into(),
            basis: Some("1a2b3c4d5e6f".into()),
        };
        assert_eq!(shown.name.text(), "src/ws.rs:4-8 1a2b3c4 `fn reconnect() {`");
        assert_eq!(shown.heading(), "src/ws.rs:4-8 1a2b3c4");
    }

    #[test]
    fn a_whole_file_says_the_page_does_not_show_it_and_what_prints_it() {
        let blob = "2418c540aa55b7e1d5d8c7a90f7e0ad3b6f0c1d2";
        let whole = Shown {
            name: AnchorName::new("docs/using-fwd.md", None, Some(blob), ""),
            lines: None,
            text: String::new(),
            basis: Some(blob.into()),
        };
        assert_eq!(whole.heading(), "docs/using-fwd.md 2418c54");
        assert_eq!(
            whole.whole_file_note(),
            format!("The whole file: the page does not show a file's content; `git show {blob}` prints it as this highlight holds it, in a checkout of its repository.")
        );
        let unknown = Shown { basis: None, ..whole };
        assert_eq!(unknown.whole_file_note(), "The whole file: the page does not show a file's content.");
    }
}
