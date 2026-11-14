//! How a working set becomes the person's view: which list each record stands in, what its row
//! says and what can be done from it, what is folded, and the document's other states. The read
//! itself, against a daemon, is tested from the CLI (crates/fwd/tests/session_view.rs).

use super::*;
use crate::model::AgentRole;
use crate::working_set::Section;
use serde_json::{json, Value};
use uuid::Uuid;

const PAGE: &str = "http://127.0.0.1:4242/";

fn id(n: u8) -> EntityId {
    EntityId::from_bytes([n; 32])
}

fn project(n: u8) -> ProjectRecord {
    let name = ["alpha", "beta"][usize::from(n)].to_string();
    ProjectRecord { record: id(100 + n), uuid: Uuid::from_bytes([n + 1; 16]), name }
}

/// The person `tester`, whose principal in alpha is `id(1)` and in beta `id(2)`.
fn person(answered: &[EntityId]) -> Person {
    Person {
        handle: "tester".into(),
        principals: HashSet::from([id(1), id(2)]),
        answered: answered.iter().copied().collect(),
    }
}

fn ask(n: u8, from: &str, to: Addressee, in_project: u8) -> Record {
    Record {
        id: id(n),
        kind: RecordKind::Ask { from: from.into(), to },
        title: format!("Question {n}?"),
        text: format!("Question {n}? The whole of it."),
        created: 1_794_600_000_000 + i64::from(n) * 1000,
        project: project(in_project),
        route: Some(Route::question(id(n))),
    }
}

fn task(n: u8, status: TaskStatus, what: &str, in_project: u8) -> Record {
    Record {
        id: id(n),
        kind: RecordKind::Task { status },
        title: format!("Task {n}"),
        text: what.into(),
        created: 0,
        project: project(in_project),
        route: Some(Route::task(id(n))),
    }
}

fn section(kind: SectionKind, records: Vec<Record>) -> Section {
    Section { kind, total: records.len(), records, listed_elsewhere: 0, unreadable: Vec::new() }
}

/// A holding of the principal `id(n)`, named `handle`, in alpha, bound to the session's agent
/// `agent`, with `unread` unread notifications.
fn holding(n: u8, handle: &str, agent: &str, sections: Vec<Section>, unread: usize) -> Holding {
    let mut sections = sections;
    sections.push(Section { kind: SectionKind::Unread, total: unread, ..section(SectionKind::Unread, Vec::new()) });
    Holding {
        project: project(0),
        principal: id(n),
        handle: handle.into(),
        kind: PrincipalKind::Agent,
        role: Some(AgentRole::Lead),
        subject: Some(format!("claude-code:s1/{agent}")),
        sections,
    }
}

fn held(holdings: Vec<Holding>, person: Person) -> Held {
    Held { holdings, person, projects: vec![project(0), project(1)] }
}

fn to(n: u8, handle: &str) -> Addressee {
    Addressee::Principal { id: id(n), handle: handle.into() }
}

/// The JSON of `document`, as the command prints it.
fn printed(document: &Document) -> Value {
    serde_json::to_value(document).expect("a document serializes")
}

/// A question the principal asked the person waits on them, by the person's principal in either
/// project or by their name, unless they answered it in its thread; one it asked anyone else, or no
/// one in particular, waits on others; what it holds is the questions asked of it, then its tasks.
/// Each row carries its own text, its tags and its path, and only what waits on the person can be
/// answered, by a reply in the person's words on standard input.
#[test]
fn each_record_stands_in_the_list_of_whom_it_waits_on() {
    let asked = vec![
        ask(10, "lead-sam", to(1, "tester"), 0),
        ask(11, "lead-sam", Addressee::Name("@tester".into()), 0),
        ask(12, "lead-sam", to(2, "tester"), 1),
        ask(13, "lead-sam", to(30, "impl-sam"), 0),
        ask(14, "lead-sam", Addressee::NoOne, 0),
        ask(15, "lead-sam", to(1, "tester"), 0),
    ];
    let holds = vec![
        section(SectionKind::AskedOfYou, vec![ask(16, "tester", to(20, "lead-sam"), 0)]),
        section(
            SectionKind::Assigned,
            vec![
                task(17, TaskStatus::InProgress, "Retry once the daemon answers.", 0),
                task(18, TaskStatus::Open, "", 1),
            ],
        ),
    ];
    let sections = [vec![section(SectionKind::YouAsked, asked)], holds].concat();
    let view = held(vec![holding(20, "lead-sam", "main", sections, 0)], person(&[id(15)]));
    let json = printed(&view.document(PAGE));
    let group = &json["principals"][0];
    let lists: Vec<(&str, Vec<&str>)> = group["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            let ids = s["records"].as_array().unwrap().iter().map(|r| r["title"].as_str().unwrap()).collect();
            (s["section"].as_str().unwrap(), ids)
        })
        .collect();
    assert_eq!(
        lists,
        [
            ("waits_on_you", vec!["Question 10?", "Question 11?", "Question 12?"]),
            ("holds", vec!["Question 16?", "Task 17", "Task 18"]),
            ("waits_on_others", vec!["Question 13?", "Question 14?"]),
        ],
        "the question the person answered is in no list: {json}"
    );
    let waits = &group["sections"][0];
    assert_eq!(waits["records"][0]["kind"], "ask");
    assert_eq!(
        waits["records"][0]["tags"],
        json!([{"text": "asked", "tone": "attention", "at": "2026-11-13T20:00:10Z"}])
    );
    assert_eq!(waits["records"][0]["lines"], json!(["Question 10? The whole of it."]));
    assert_eq!(waits["records"][0]["path"], format!("?p={}#/questions/{}", project(0).uuid, id(10).to_base64()));
    assert_eq!(
        waits["records"][2]["tags"][1],
        json!({"text": "in beta", "tone": null, "at": null}),
        "a record of another project names it"
    );
    assert_eq!(
        waits["records"][2]["actions"],
        json!([{
            "label": "Answer",
            "args": ["--project", project(1).uuid.to_string(), "--as", "tester", "--format", "pane", "reply", id(12).to_base64(), "--text-from-stdin"],
            "input": {"prompt": "Your answer", "multiline": true},
        }]),
        "the answer runs in the question's project"
    );
    let holds = &group["sections"][1]["records"];
    assert_eq!(holds[0]["tags"][0]["text"], "from tester");
    assert_eq!(holds[1]["tags"], json!([{"text": "in progress", "tone": null, "at": null}]));
    assert_eq!(holds[1]["lines"], json!(["Retry once the daemon answers."]));
    assert_eq!(holds[2]["lines"], json!([]), "a task with no what has no line");
    assert_eq!(holds[2]["tags"][1]["text"], "in beta");
    let others = &group["sections"][2];
    let tags: Vec<&Value> = others["records"].as_array().unwrap().iter().map(|r| &r["tags"][0]["text"]).collect();
    assert_eq!(tags, [&json!("to impl-sam"), &json!("to no one in particular")]);
    for list in group["sections"].as_array().unwrap() {
        let answerable = list["section"] == "waits_on_you";
        for row in list["records"].as_array().unwrap() {
            assert_eq!(row["actions"].as_array().unwrap().is_empty(), !answerable, "{row}");
        }
    }
    let folded: Vec<(&Value, &Value)> =
        group["sections"].as_array().unwrap().iter().map(|s| (&s["section"], &s["folded"])).collect();
    assert_eq!(
        folded,
        [
            (&json!("waits_on_you"), &json!(false)),
            (&json!("holds"), &json!(false)),
            (&json!("waits_on_others"), &json!(true))
        ]
    );
}

/// The session's own thread comes first and stands open; a subagent's group is folded unless
/// something in it waits on the person; each group names the principal, its role and its project,
/// and how many unread notifications it has; a list with nothing in it is not in the document.
#[test]
fn a_subagents_group_is_folded_unless_something_in_it_waits_on_the_person() {
    let quiet = holding(21, "impl-quiet", "a1", vec![section(SectionKind::YouAsked, Vec::new())], 3);
    let asking = holding(
        22,
        "impl-asking",
        "a2",
        vec![section(SectionKind::YouAsked, vec![ask(10, "impl-asking", to(1, "tester"), 0)])],
        0,
    );
    let main = holding(20, "lead-sam", "main", Vec::new(), 0);
    let json = printed(&held(vec![main, quiet, asking], person(&[])).document(PAGE));
    let groups: Vec<(&Value, &Value, usize)> = json["principals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| (&g["title"], &g["folded"], g["sections"].as_array().unwrap().len()))
        .collect();
    assert_eq!(
        groups,
        [
            (&json!("lead-sam"), &json!(false), 0),
            (&json!("impl-quiet"), &json!(true), 0),
            (&json!("impl-asking"), &json!(false), 1)
        ]
    );
    assert_eq!(
        json["principals"][1]["tags"],
        json!([
            {"text": "lead", "tone": null, "at": null},
            {"text": "alpha", "tone": null, "at": null},
            {"text": "3 unread", "tone": null, "at": null},
        ])
    );
    assert_eq!(json["principals"][1]["id"], id(21).to_base64());
    assert_eq!(json["page"], PAGE);
    assert_eq!(json["lines"], json!([]));
}

/// A session no principal is bound to says so in one line; with the daemon out of reach, the last
/// principals stay, with no page and a line saying so; a read that failed says why, and keeps them
/// too.
#[test]
fn the_documents_other_states_say_what_they_are_and_keep_the_last_principals() {
    let unbound = held(Vec::new(), person(&[])).document(PAGE);
    assert_eq!(
        unbound.lines,
        ["No fwd principal is bound to this session yet. Its agent names itself with `fwd principal add <name> --role <role>`."]
    );
    assert!(unbound.principals.is_empty());

    let shown = held(vec![holding(20, "lead-sam", "main", Vec::new(), 1)], person(&[])).document(PAGE);
    let lost = shown.out_of_reach();
    assert_eq!((lost.page.as_deref(), &lost.principals), (None, &shown.principals));
    assert_eq!(
        lost.lines,
        ["fwd's daemon is out of reach; this is what it last said, and it will show again when the daemon answers."]
    );
    assert_eq!(Document::never_read().lines, lost.lines);
    assert!(Document::never_read().principals.is_empty());

    let failed = shown.unreadable("no such\ntable");
    assert_eq!((failed.page.as_deref(), &failed.principals), (Some(PAGE), &shown.principals));
    assert_eq!(
        failed.lines,
        ["fwd could not read what this session holds (no such table); this is what it last read, and it will show again when a read succeeds."]
    );
}

/// The document has the contract's keys and no others, at every level, and no string holds a
/// control character but a line's own newlines, whatever the records' text and names hold.
#[test]
fn the_document_has_the_contracts_keys_and_no_control_characters() {
    let mut tricky = ask(10, "lead\u{1b}-sam", to(1, "tester"), 0);
    tricky.title = "Retry\u{7} now?".into();
    tricky.text = "First\r\nsecond\tline\u{0}".into();
    let sections = vec![
        section(SectionKind::YouAsked, vec![tricky]),
        section(SectionKind::AskedOfYou, vec![ask(11, "te\u{1b}ster", to(20, "lead-sam"), 0)]),
    ];
    let json = printed(&held(vec![holding(20, "lead\u{7}-sam", "main", sections, 0)], person(&[])).document(PAGE));
    let keys = |v: &Value| -> Vec<String> {
        let mut keys: Vec<String> = v.as_object().expect("an object").keys().cloned().collect();
        keys.sort();
        keys
    };
    let expect = |names: &[&str]| -> Vec<String> {
        let mut names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        names.sort();
        names
    };
    assert_eq!(keys(&json), expect(&["title", "page", "lines", "principals"]));
    let group = &json["principals"][0];
    assert_eq!(keys(group), expect(&["id", "title", "tags", "folded", "lines", "sections"]));
    let list = &group["sections"][0];
    assert_eq!(keys(list), expect(&["section", "title", "folded", "lines", "shown", "of", "more", "records"]));
    let row = &list["records"][0];
    assert_eq!(keys(row), expect(&["id", "kind", "title", "tags", "lines", "path", "actions"]));
    assert_eq!(keys(&row["tags"][0]), expect(&["text", "tone", "at"]));
    assert_eq!(keys(&row["actions"][0]), expect(&["label", "args", "input"]));
    assert_eq!(keys(&row["actions"][0]["input"]), expect(&["prompt", "multiline"]));

    fn strings<'a>(v: &'a Value, key: &str, out: &mut Vec<(String, &'a str)>) {
        match v {
            Value::String(s) => out.push((key.to_string(), s)),
            Value::Array(items) => items.iter().for_each(|i| strings(i, key, out)),
            Value::Object(map) => map.iter().for_each(|(k, i)| strings(i, k, out)),
            _ => {}
        }
    }
    let mut all = Vec::new();
    strings(&json, "", &mut all);
    for (key, s) in all {
        let allowed = |c: char| key == "lines" && c == '\n';
        assert!(!s.chars().any(|c| c.is_control() && !allowed(c)), "{key}: {s:?}");
    }
    assert_eq!(row["lines"], json!(["First\nsecond    line"]));
    assert_eq!((&row["title"], &group["title"]), (&json!("Retry now?"), &json!("lead-sam")));
    assert_eq!(group["sections"][1]["records"][0]["tags"][0]["text"], "from tester");
}

/// What the view listens to: the projects, every project's principals, each principal's working
/// set, and the thread of each question that asks the person, answered or not.
#[test]
fn the_live_reads_cover_what_the_document_is_made_from() {
    let asked = vec![ask(10, "lead-sam", to(1, "tester"), 0), ask(11, "lead-sam", to(30, "impl-sam"), 0)];
    let sections = vec![section(SectionKind::YouAsked, asked), section(SectionKind::Assigned, Vec::new())];
    let view = held(vec![holding(20, "lead-sam", "main", sections, 0)], person(&[id(10)]));
    let reads = view.live_reads();
    let expected = BTreeSet::from([
        LiveRead::Projects,
        LiveRead::Principals(project(0)),
        LiveRead::Principals(project(1)),
        LiveRead::QuestionsTo(id(20)),
        LiveRead::QuestionsBy(id(20)),
        LiveRead::QuestionsByName { project: project(0), handle: "lead-sam".into(), of_no_one: false },
        LiveRead::TasksAssignedTo(id(20)),
        LiveRead::Unread { project: project(0), principal: id(20) },
        LiveRead::AnswersTo(id(10)),
    ]);
    assert_eq!(reads, expected);
}
