//! How what a session holds becomes the person's view: which list each record stands in, what its
//! row says and what can be done from it, what is folded, the caps and the document's other states.
//! Reading it from a node is tested in crates/fwd-core/tests/session_view.rs, and the command in
//! crates/fwd/tests/session_view/.

use super::*;
use crate::model::AgentRole;
use crate::working_set::{Asking, RecordKind, Section};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use uuid::Uuid;

const PAGE: &str = "http://127.0.0.1:4242/";

fn id(n: u8) -> EntityId {
    EntityId::from_bytes([n; 32])
}

fn project(n: u8) -> ProjectRecord {
    let name = ["alpha", "beta", "gamma"][usize::from(n)].to_string();
    ProjectRecord { record: id(100 + n), uuid: Uuid::from_bytes([n + 1; 16]), name }
}

/// The person `tester`, whose principal in alpha is `id(1)` and in beta `id(2)`, with none in
/// gamma.
fn person(answered: &[EntityId]) -> Person {
    Person {
        handle: Some("tester".into()),
        principals: BTreeMap::from([(project(0).record, id(1)), (project(1).record, id(2))]),
        shared: Vec::new(),
        said: None,
        answered: answered.iter().copied().collect(),
    }
}

fn ask(n: u8, asking: Asking, in_project: u8) -> Record {
    Record {
        id: id(n),
        kind: RecordKind::Ask(asking),
        title: format!("Question {n}?"),
        text: Some(format!("Question {n}? The whole of it.")),
        created: Some(1_794_600_000_000 + i64::from(n) * 1000),
        project: project(in_project),
        route: Some(Route::question(id(n))),
    }
}

fn by(to: Addressee) -> Asking {
    Asking::By { to }
}

fn from(n: u8) -> Asking {
    Asking::Of { from: Party { id: id(n), handle: format!("handle-of-{n}") } }
}

fn to(n: u8) -> Addressee {
    Addressee::Principal(Party { id: id(n), handle: format!("handle-of-{n}") })
}

fn task(n: u8, status: TaskStatus, what: &str, in_project: u8) -> Record {
    Record {
        id: id(n),
        kind: RecordKind::Task { status: Some(status) },
        title: format!("Task {n}"),
        text: Some(what.into()),
        created: Some(0),
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
    sections.push(Section { total: unread, ..section(SectionKind::Unread, Vec::new()) });
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

/// What the view is drawn from, with the names of the person's principals and of `id(20)` and
/// `id(30)` known.
fn held(holdings: Vec<Holding>, person: Person) -> Held {
    let names = HashMap::from([
        (id(1), "tester".to_string()),
        (id(2), "tester".to_string()),
        (id(20), "lead-sam".to_string()),
        (id(30), "impl-sam".to_string()),
    ]);
    Held {
        session: "s1".into(),
        holdings,
        person,
        projects: vec![project(0), project(1), project(2)],
        names,
        cited: Cited::default(),
    }
}

/// The JSON of `document`, as the command prints it.
fn printed(document: &Document) -> Value {
    serde_json::to_value(document).expect("a document serializes")
}

/// A question the principal asked the person waits on them, by the person's principal in any
/// project or by their name, unless they answered it in its thread; one it asked anyone else, or no
/// one in particular, waits on others; what it holds is the questions asked of it, then its tasks.
/// Each row carries its own text, its tags and its path, other parties named by their names; only
/// what waits on the person can be answered, as the person's principal in the question's project,
/// by its id, in the person's words on standard input.
#[test]
fn each_record_stands_in_the_list_of_whom_it_waits_on() {
    let asked = vec![
        ask(10, by(to(1)), 0),
        ask(11, by(Addressee::Name("@tester".into())), 0),
        ask(12, by(to(1)), 1),
        ask(13, by(to(30)), 0),
        ask(14, by(Addressee::NoOne), 0),
        ask(15, by(to(1)), 0),
    ];
    let holds = vec![
        section(SectionKind::AskedOfYou, vec![ask(16, from(1), 0)]),
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
            let titles = s["records"].as_array().unwrap().iter().map(|r| r["title"].as_str().unwrap()).collect();
            (s["section"].as_str().unwrap(), titles)
        })
        .collect();
    assert_eq!(
        lists,
        [
            (
                "waits_on_you",
                vec!["Question 10? The whole of it.", "Question 11? The whole of it.", "Question 12? The whole of it."]
            ),
            ("holds", vec!["Question 16? The whole of it.", "Task 17", "Task 18"]),
            ("waits_on_others", vec!["Question 13? The whole of it.", "Question 14? The whole of it."]),
        ],
        "the question the person answered is in no list: {json}"
    );
    let waits = &group["sections"][0];
    assert_eq!(waits["records"][0]["kind"], "ask");
    assert_eq!(
        waits["records"][0]["tags"],
        json!([{"text": "asked", "tone": "attention", "at": "2026-11-13T20:00:10Z"}])
    );
    assert_eq!(waits["records"][0]["lines"], json!([]), "the title holds the whole text");
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
            "args": ["--project", project(1).uuid.to_string(), "--as", id(2).to_base64(), "--format", "pane", "reply", id(12).to_base64(), "-"],
            "input": {"prompt": "Your answer", "multiline": true},
        }]),
        "the answer runs in the question's project, as the person's principal there"
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

/// A question asked of the person, now in a project where they have no principal of their own,
/// still waits on them but has no Answer, a line saying why; one where their name is shared says
/// that instead, and the document says the name is shared there.
#[test]
fn a_question_where_the_person_has_no_principal_of_their_own_has_no_answer_and_says_why() {
    let sections = vec![section(SectionKind::YouAsked, vec![ask(10, by(to(1)), 2), ask(11, by(to(1)), 1)])];
    let mut shared = person(&[]);
    shared.principals.remove(&project(1).record);
    shared.shared.push(project(1));
    let json = printed(&held(vec![holding(20, "lead-sam", "main", sections, 0)], shared).document(PAGE));
    let rows = &json["principals"][0]["sections"][0]["records"];
    assert_eq!(rows[0]["actions"], json!([]));
    assert_eq!(
        rows[0]["lines"],
        json!(["You have no principal in gamma, where this question now is, so it cannot be answered here."])
    );
    assert_eq!(rows[1]["actions"], json!([]));
    assert_eq!(
        rows[1]["lines"],
        json!([
            "More than one person shares your name in beta, where this question now is, so it cannot be answered here."
        ])
    );
    assert_eq!(
        json["lines"],
        json!(["More than one person is named tester in beta: what they are asked there is not counted as waiting on you, and none of it can be answered here."])
    );
}

/// The session's own thread comes first and stands open; a subagent's group is folded unless
/// something in it waits on the person, and says what it is doing, the first task it has in
/// progress; each group names the principal, its role and its project, and how many unread
/// notifications it has; a list with nothing in it is not in the document.
#[test]
fn a_subagents_group_is_folded_unless_something_in_it_waits_on_the_person() {
    let doing = section(
        SectionKind::Assigned,
        vec![
            task(17, TaskStatus::Open, "", 0),
            task(18, TaskStatus::InProgress, "", 0),
            task(19, TaskStatus::InProgress, "", 0),
        ],
    );
    let quiet = holding(21, "impl-quiet", "a1", vec![doing], 3);
    let asking = holding(22, "impl-asking", "a2", vec![section(SectionKind::YouAsked, vec![ask(10, by(to(1)), 0)])], 0);
    let main = holding(
        20,
        "lead-sam",
        "main",
        vec![section(SectionKind::Assigned, vec![task(23, TaskStatus::InProgress, "", 0)])],
        0,
    );
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
            (&json!("lead-sam"), &json!(false), 1),
            (&json!("impl-quiet"), &json!(true), 1),
            (&json!("impl-asking"), &json!(false), 1)
        ]
    );
    assert_eq!(
        json["principals"][1]["tags"],
        json!([
            {"text": "lead", "tone": null, "at": null},
            {"text": "alpha", "tone": null, "at": null},
            {"text": "Task 18", "tone": null, "at": null},
            {"text": "3 unread", "tone": null, "at": null},
        ])
    );
    assert_eq!(json["principals"][0]["tags"].as_array().map(Vec::len), Some(2), "the own thread says no task");
    assert_eq!(json["principals"][1]["id"], id(21).to_base64());
    assert_eq!(json["page"], PAGE);
    assert_eq!(json["lines"], json!([]));
}

/// A session no principal is bound to says so in one line; with the daemon out of reach, the last
/// principals stay, with no page and a line saying so; a read that failed says why in plain words
/// with no id, and keeps them too; what keeps the reader from answering is said on the document.
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

    let error = anyhow::Error::new(PropertyError::Missing).context(format!("reading {}", id(9).to_base64()));
    let failed = shown.unreadable(&error);
    assert_eq!((failed.page.as_deref(), &failed.principals), (Some(PAGE), &shown.principals));
    assert_eq!(
        failed.lines,
        ["fwd could not read what this session holds: a field of a record it reads could not be read. This is what it last read; it tries again."]
    );
    let other = shown.unreadable(&anyhow!("no such table {}", id(9).to_base64()));
    assert_eq!(
        other.lines,
        ["fwd could not read what this session holds: the daemon did not give what it asked for. This is what it last read; it tries again."]
    );

    let mut nobody = person(&[]);
    nobody.handle = None;
    nobody.principals.clear();
    nobody.said = Some(
        "`--as` names an agent, and the view is a person's, so nothing here waits on you and nothing can be answered."
            .into(),
    );
    let sections = vec![section(SectionKind::YouAsked, vec![ask(10, by(to(1)), 0)])];
    let json = printed(&held(vec![holding(20, "lead-sam", "main", sections, 0)], nobody).document(PAGE));
    assert_eq!(
        json["lines"],
        json!(["`--as` names an agent, and the view is a person's, so nothing here waits on you and nothing can be answered."])
    );
    assert_eq!(json["principals"][0]["sections"][0]["section"], "waits_on_others");
}

/// The document has the contract's keys and no others, at every level, and no string holds a
/// control or direction character but a line's own newlines, whatever the records' text and names
/// hold; a principal the view cannot find is named without its id; a text that could not be read
/// says so.
#[test]
fn the_document_has_the_contracts_keys_and_no_control_characters() {
    let mut tricky = ask(10, by(to(1)), 0);
    tricky.text = Some("First\r\nsecond\tline\u{0} and more words than a title\u{202E}".into());
    let mut unread = task(17, TaskStatus::Open, "", 0);
    unread.text = None;
    let sections = vec![
        section(SectionKind::YouAsked, vec![tricky]),
        section(SectionKind::AskedOfYou, vec![ask(11, from(99), 0)]),
        section(SectionKind::Assigned, vec![unread]),
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
        assert!(!s.chars().any(|c| (c.is_control() || c == '\u{202E}') && !allowed(c)), "{key}: {s:?}");
    }
    assert_eq!(row["lines"], json!(["First\nsecond    line and more words than a title"]));
    assert_eq!(
        (&row["title"], &group["title"]),
        (&json!("First second line and more words than a…"), &json!("lead-sam"))
    );
    let holds = &group["sections"][1]["records"];
    assert_eq!(holds[0]["tags"][0]["text"], "from someone fwd cannot find");
    assert_eq!(holds[1]["lines"], json!(["Its text could not be read."]));
}

/// Titles, tags and lines are capped, so that a principal with an enormous name and a task with an
/// enormous title still give a document within the budget.
#[test]
fn every_string_is_capped_and_the_whole_document_stays_within_the_budget() {
    let enormous = "x".repeat(100_001);
    let mut long = task(17, TaskStatus::InProgress, "", 0);
    long.title = "word ".repeat(1000);
    let subagent = holding(21, &enormous, "a1", vec![section(SectionKind::Assigned, vec![long])], 0);
    let document = held(vec![subagent], person(&[])).document(PAGE);
    assert!(serde_json::to_string(&document).unwrap().len() <= fit::BUDGET);
    let group = &document.principals[0];
    assert_eq!(group.title.chars().count(), TITLE_LIMIT);
    assert!(group.tags.iter().all(|tag| tag.text.chars().count() <= TAG_LIMIT));
    assert!(group.sections[0].records[0].title.chars().count() <= TITLE_LIMIT);
}

/// What the view listens to: the projects, every project's principals of the session, each
/// principal's working set, and the thread of each question that asks the person, answered or not.
#[test]
fn the_live_reads_cover_what_the_document_is_made_from() {
    let asked = vec![ask(10, by(to(1)), 0), ask(11, by(to(30)), 0)];
    let sections = vec![section(SectionKind::YouAsked, asked), section(SectionKind::Assigned, Vec::new())];
    let view = held(vec![holding(20, "lead-sam", "main", sections, 0)], person(&[id(10)]));
    let session = |n| LiveRead::SessionPrincipals { project: project(n), session: "s1".into() };
    let expected = BTreeSet::from([
        LiveRead::Projects,
        session(0),
        session(1),
        session(2),
        LiveRead::QuestionsTo(id(20)),
        LiveRead::QuestionsBy(id(20)),
        LiveRead::QuestionsByName { project: project(0), handle: "lead-sam".into(), of_no_one: false },
        LiveRead::TasksAssignedTo(id(20)),
        LiveRead::Unread { project: project(0), principal: id(20) },
        LiveRead::AnswersTo(id(10)),
    ]);
    assert_eq!(view.live_reads(), expected);
}

/// A row's lines never repeat its title: a short question, whose title is its whole text, opens
/// into its actions alone, even when its text breaks lines the title joins; a question whose title
/// is a cut of its text keeps the whole text as its line.
#[test]
fn a_title_that_holds_the_whole_text_is_not_said_again_beneath_it() {
    let mut short = ask(10, by(to(1)), 0);
    short.text = Some("May I cap the backoff\nat two seconds?".into());
    let mut long = ask(11, by(to(1)), 0);
    long.text = Some("Should the primer name the page? It costs every session about forty characters.".into());
    let sections = vec![section(SectionKind::YouAsked, vec![short, long])];
    let json = printed(&held(vec![holding(20, "lead-sam", "main", sections, 0)], person(&[])).document(PAGE));
    let rows = &json["principals"][0]["sections"][0]["records"];
    assert_eq!(rows[0]["title"], "May I cap the backoff at two seconds?");
    assert_eq!(rows[0]["lines"], json!([]));
    assert_eq!(rows[0]["actions"][0]["label"], "Answer");
    assert_eq!(rows[1]["title"], "Should the primer name the page? It costs…");
    assert_eq!(
        rows[1]["lines"],
        json!(["Should the primer name the page? It costs every session about forty characters."])
    );
}

/// Records that could not be read are said in the list they would stand in, by count, with no id:
/// the questions asked of the principal and its tasks in what it holds, and the questions it asked,
/// whose addressee is not known, in what waits on others, which then stands with no rows.
#[test]
fn records_that_could_not_be_read_are_said_in_their_list() {
    let mut asked_of = section(SectionKind::AskedOfYou, Vec::new());
    asked_of.unreadable = vec![id(40), id(41)];
    let mut assigned = section(SectionKind::Assigned, vec![task(17, TaskStatus::Open, "", 0)]);
    assigned.unreadable = vec![id(42)];
    let mut asked = section(SectionKind::YouAsked, Vec::new());
    asked.unreadable = vec![id(43)];
    let view = held(vec![holding(20, "lead-sam", "main", vec![asked, asked_of, assigned], 0)], person(&[]));
    let json = printed(&view.document(PAGE));
    let lists: Vec<(&Value, &Value)> =
        json["principals"][0]["sections"].as_array().unwrap().iter().map(|s| (&s["section"], &s["lines"])).collect();
    assert_eq!(
        lists,
        [
            (
                &json!("holds"),
                &json!([
                    "fwd could not read 2 of the questions asked of it.",
                    "fwd could not read 1 of the tasks assigned to it."
                ])
            ),
            (
                &json!("waits_on_others"),
                &json!(["fwd could not read 1 of the questions it asked, so whom they ask is not known."])
            ),
        ]
    );
}

/// What waits on the person, cut to keep the document short, gives the person's own Questions page
/// in the principal's project when every question of it is there, and no page when one is in
/// another project, which that page does not list.
#[test]
fn what_waits_on_the_person_cut_short_gives_their_questions_page_only_where_it_lists_them_all() {
    let many = |last_project: u8| -> Vec<Record> {
        (0..80u8)
            .map(|n| {
                let mut question = ask(n, by(to(1)), if n == 79 { last_project } else { 0 });
                question.text = Some("word ".repeat(400));
                question
            })
            .collect()
    };
    let document = |records| {
        held(vec![holding(20, "lead-sam", "main", vec![section(SectionKind::YouAsked, records)], 0)], person(&[]))
            .document(PAGE)
    };
    let here = document(many(0));
    let waits = &here.principals[0].sections[0];
    assert!(waits.shown < waits.of, "{} of {}", waits.shown, waits.of);
    assert_eq!(waits.more, Some(format!("?p={}#/questions?for={}", project(0).uuid, id(1).to_base64())));
    let elsewhere = document(many(1));
    let waits = &elsewhere.principals[0].sections[0];
    assert_eq!(waits.more, None);
    assert_eq!(waits.lines, [format!("{} of 80 shown; the rest are left out to keep this view short.", waits.shown)]);
}
