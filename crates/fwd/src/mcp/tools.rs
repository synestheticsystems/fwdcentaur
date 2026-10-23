//! The tools the daemon's MCP endpoint offers, made from the CLI's own grammar (`crate::Cli`) so
//! that the two cannot drift: each tool is a CLI command, named by its words joined with `_`
//! (`task create` is `task_create`), whose properties are the command's arguments under their
//! long names (`--done-when` is `done_when`) or, for a positional one, its name in the grammar,
//! with the command's own help as their descriptions. A call's arguments become that command's
//! words again ([`Spec::argv`]) and are parsed by the same parser the CLI uses, so a value the CLI
//! refuses is refused alike, in the CLI's words.
//!
//! What the endpoint leaves out, since it has no checkout, no terminal and no process of its own to
//! give the caller: the commands of the daemon's lifecycle, the machine's data folder and project
//! index (`init`, `daemon`, `ui`, `project` but `project_list`, `admin`, `checkouts`), the
//! commands and arguments that read a file in a checkout (`anchor`, `scope attach-dir`, `--path`,
//! `--lines`, `--against`, `--side`), the watch, which never ends by itself, the hooks' commands
//! (`session`), `capabilities`, and `principal bind`, which takes over another principal and is the
//! person's (`fwd admin bind`). No tool takes `--as`: the caller is the thread that calls.

use clap::{ArgAction, CommandFactory};
use serde_json::{json, Map, Value};

/// The CLI commands offered as tools, by their words.
pub const OFFERED: &[&str] = &[
    "principal list",
    "principal add",
    "principal role",
    "principal rename",
    "principal retire",
    "principal succeed",
    "principal scope add",
    "principal scope remove",
    "principal scope project-wide",
    "task create",
    "task list",
    "task show",
    "task next",
    "task assign",
    "task unassign",
    "task approve",
    "task reject",
    "task start",
    "task submit",
    "task accept",
    "task return",
    "task done",
    "task cancel",
    "task reopen",
    "task rejustify",
    "task amend",
    "task hold",
    "task release",
    "task explain",
    "step add",
    "step list",
    "step check",
    "step uncheck",
    "step remove",
    "link",
    "unlink",
    "relations",
    "relation approve",
    "relation reject",
    "scope create",
    "scope branches",
    "scope list",
    "scope show",
    "scope rename",
    "scope intention",
    "scope status",
    "comment",
    "ask",
    "reply",
    "forward",
    "message list",
    "message show",
    "message resolve",
    "message unresolve",
    "message edit",
    "decision create",
    "decision list",
    "decision show",
    "decision attach",
    "decision detach",
    "decision accept",
    "decision reject",
    "decision repeal",
    "decision supersede",
    "decision export",
    "cite",
    "inbox",
    "inbox pop",
    "read",
    "dismiss",
    "prime",
    "graph",
];

/// The arguments no tool takes: those that read a file in a checkout, which the daemon has none
/// of, and the hook's. Every other argument of an offered command is a property of its tool.
const LEFT_OUT: &[&str] = &["path", "lines", "against", "side", "hook_event", "heartbeat"];

/// The arguments every project tool takes besides its command's, which the CLI takes as global
/// flags: which project, and the scope the call works in.
pub const PROJECT: &str = "project";
pub const SCOPE: &str = "scope";

/// How a property is given on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Shape {
    /// `--name`, when true.
    Switch,
    /// `--name=<value>`, once.
    Flag,
    /// `--name=<value>` for each of a list.
    Flags,
    /// A positional value, in its place.
    Positional,
    /// Positional values, the last place, for a list.
    Positionals,
}

/// One property of a tool: its name, the long flag it is or its place, its shape, and the word of
/// the command line it follows: a parent command's argument goes right after the parent's word, as
/// `fwd inbox --for x pop` has it, since clap refuses it after the subcommand's.
#[derive(Debug, Clone)]
struct Property {
    name: String,
    long: Option<String>,
    index: usize,
    shape: Shape,
    after: usize,
}

/// One tool: its command's words, its name, and its properties.
#[derive(Debug, Clone)]
pub struct Spec {
    pub words: &'static str,
    pub name: String,
    properties: Vec<Property>,
    /// The tool as `tools/list` gives it: description and JSON schema.
    pub description: String,
    pub schema: Map<String, Value>,
}

/// The CLI's own `--help` for the offered command a tool stands for, named by the tool's name
/// (`task_create`) or the command's words (`task create`), for the `help` tool: everything the
/// catalog's short descriptions leave out, from the same source. Refused for a command no tool
/// offers.
pub fn long_help(named: &str) -> Result<String, String> {
    let named = named.trim().trim_start_matches("fwd ").trim();
    let Some(words) = OFFERED.iter().find(|w| **w == named || tool_name(w) == named) else {
        return Err(format!(
            "no tool is the command `{named}`: give a tool's name, such as task_create, or its CLI words, such as `task create`"
        ));
    };
    let mut cmd = crate::Cli::command().bin_name("fwd");
    cmd.build();
    let mut sub = &mut cmd;
    for word in words.split(' ') {
        sub = sub.find_subcommand_mut(word).expect("an offered command");
    }
    Ok(sub.render_long_help().to_string())
}

/// The tool named `words` (`task create`) is named `task_create`.
pub fn tool_name(words: &str) -> String {
    words.replace([' ', '-'], "_")
}

/// Every offered command's tool, made from the CLI's grammar.
pub fn specs() -> Vec<Spec> {
    let root = crate::Cli::command();
    OFFERED.iter().map(|words| spec(&root, words)).collect()
}

fn spec(root: &clap::Command, words: &'static str) -> Spec {
    // The command at each word, the tool's own last: each takes its own arguments.
    let mut cmds = Vec::new();
    let mut cmd = root;
    for word in words.split(' ') {
        cmd = cmd.find_subcommand(word).unwrap_or_else(|| panic!("`fwd {words}` is a command"));
        cmds.push(cmd);
    }
    let last = cmds.len() - 1;
    let mut properties = Vec::new();
    let mut schema_properties = Map::new();
    let mut required = Vec::new();
    let args = cmds.iter().enumerate().flat_map(|(after, c)| c.get_arguments().map(move |a| (after, a)));
    for (after, arg) in args {
        let id = arg.get_id().as_str();
        if arg.is_global_set() || arg.is_hide_set() || LEFT_OUT.contains(&id) {
            continue;
        }
        // A parent's positional would have to come before the subcommand's word; no offered
        // command's parent has one.
        assert!(after == last || arg.get_long().is_some(), "`fwd {words}`: a parent's positional `{id}`");
        let (name, long) = match arg.get_long() {
            Some(long) => (long.replace('-', "_"), Some(long.to_string())),
            None => (id.to_string(), None),
        };
        let many =
            matches!(arg.get_action(), ArgAction::Append) || arg.get_num_args().is_some_and(|n| n.max_values() > 1);
        let shape = match (long.is_some(), arg.get_action(), many) {
            (true, ArgAction::SetTrue, _) => Shape::Switch,
            (true, _, false) => Shape::Flag,
            (true, _, true) => Shape::Flags,
            (false, _, false) => Shape::Positional,
            (false, _, true) => Shape::Positionals,
        };
        // The argument's short help, its doc comment's first paragraph; the rest of its help is in
        // the `help` tool's answer, the CLI's own `--help` (see `CATALOG_BUDGET`).
        let description =
            short(arg.get_help().or(arg.get_long_help()).map(|h| h.to_string()).unwrap_or_default(), PROPERTY_CHARS);
        let defaults: Vec<String> = arg.get_default_values().iter().map(|d| d.to_string_lossy().into_owned()).collect();
        // A value enum's words come from its parser: the argument lists none until the grammar is
        // built.
        let possible: Vec<String> = arg
            .get_value_parser()
            .possible_values()
            .map(|values| values.filter(|v| !v.is_hide_set()).map(|v| v.get_name().to_string()).collect())
            .unwrap_or_default();
        let item = if !possible.is_empty() {
            json!({"type": "string", "enum": possible})
        } else if defaults.len() == 1 && defaults[0].parse::<i64>().is_ok() {
            json!({"type": "integer"})
        } else {
            json!({"type": "string"})
        };
        let mut property = match shape {
            Shape::Switch => json!({"type": "boolean"}),
            Shape::Flags | Shape::Positionals => json!({"type": "array", "items": item}),
            Shape::Flag | Shape::Positional => item,
        };
        if !description.is_empty() {
            property["description"] = json!(description);
        }
        // A default the CLI gives a value it is not given (`priority`: `now` for `ask`, `next` for
        // `comment`), not a switch's `false`.
        if let ([default], false) = (defaults.as_slice(), shape == Shape::Switch) {
            property["default"] = match default.parse::<i64>() {
                Ok(n) => json!(n),
                Err(_) => json!(default),
            };
        }
        if arg.is_required_set() {
            required.push(Value::String(name.clone()));
        }
        schema_properties.insert(name.clone(), property);
        // Positionals by the order the grammar declares them, which is the order clap reads them
        // in: an unbuilt command numbers none.
        let index = if long.is_some() { 0 } else { properties.iter().filter(|p: &&Property| p.long.is_none()).count() };
        properties.push(Property { name, long, index, shape, after });
    }
    // Said once, in `project_list`'s description, rather than in each of the 72 tools that take
    // them.
    schema_properties.insert(PROJECT.into(), json!({"type": "string"}));
    schema_properties.insert(SCOPE.into(), json!({"type": "string"}));
    required.insert(0, Value::String(PROJECT.into()));
    // The command's short help, its doc comment's first paragraph; a command with none, whose
    // words say what it does (`task start` has a long help alone), takes its long help's first
    // sentence.
    let description =
        short(cmd.get_about().or(cmd.get_long_about()).map(|a| a.to_string()).unwrap_or_default(), TOOL_CHARS);
    let schema = json!({
        "type": "object",
        "properties": schema_properties,
        "required": required,
    });
    Spec {
        words,
        name: tool_name(words),
        properties,
        description,
        schema: schema.as_object().cloned().expect("an object"),
    }
}

impl Spec {
    /// The command line a call with `args` stands for: `fwd --format json`, the command's words,
    /// each followed by its own flags as `--name=<value>`, so that no value is read as a flag, then
    /// `--` and the positional values in their places, so that none of them is read as a flag
    /// either. The project and the scope are left out, since the caller passes them apart. Refused
    /// for an argument the tool does not take, naming those it does; for a value of a shape the
    /// property cannot take (an object, a list where one value goes); for a positional value given
    /// without the one before it, which the CLI would read in the earlier one's place
    /// (`comment`'s `text` without `first`); and for a number as `priority`, which the CLI takes in
    /// this build only, with a warning on standard error that a call would never see.
    pub fn argv(&self, args: &Map<String, Value>) -> Result<Vec<String>, String> {
        let mut argv: Vec<String> = vec!["fwd".into(), "--format".into(), "json".into()];
        let mut flags: Vec<Vec<String>> = vec![Vec::new(); self.words.split(' ').count()];
        for key in args.keys() {
            if key != PROJECT && key != SCOPE && !self.properties.iter().any(|p| &p.name == key) {
                let mut taken: Vec<&str> = self.properties.iter().map(|p| p.name.as_str()).collect();
                taken.extend([PROJECT, SCOPE]);
                return Err(format!("`{}` takes no argument `{key}`; it takes {}", self.name, taken.join(", ")));
            }
        }
        let mut positionals: Vec<(usize, Vec<String>)> = Vec::new();
        for p in &self.properties {
            let Some(value) = args.get(&p.name).filter(|v| !v.is_null()) else { continue };
            let values = match (&p.shape, value) {
                (Shape::Switch, Value::Bool(on)) => {
                    if *on {
                        flags[p.after].push(format!("--{}", p.long.as_deref().unwrap_or_default()));
                    }
                    continue;
                }
                (Shape::Switch, _) => return Err(format!("`{}` is true or false", p.name)),
                (Shape::Flags | Shape::Positionals, Value::Array(items)) => {
                    items.iter().map(|v| scalar(&p.name, v)).collect::<Result<Vec<_>, _>>()?
                }
                (Shape::Flags | Shape::Positionals, v) => vec![scalar(&p.name, v)?],
                (_, v) => vec![scalar(&p.name, v)?],
            };
            if p.name == "priority" && values.iter().any(|v| v.parse::<i64>().is_ok()) {
                return Err("`priority` is now, next or later".into());
            }
            match &p.long {
                Some(long) => flags[p.after].extend(values.into_iter().map(|v| format!("--{long}={v}"))),
                None => positionals.push((p.index, values)),
            }
        }
        for (word, flags) in self.words.split(' ').zip(flags) {
            argv.push(word.to_string());
            argv.extend(flags);
        }
        if !positionals.is_empty() {
            positionals.sort_by_key(|(index, _)| *index);
            // The CLI reads positional values in order, so one given without those before it
            // would be read as another.
            let places: Vec<&Property> = self.properties.iter().filter(|p| p.long.is_none()).collect();
            let given = positionals.len();
            if let Some(skipped) = places.iter().take(given).find(|p| !positionals.iter().any(|(i, _)| *i == p.index)) {
                return Err(format!("`{}` takes `{}` before the later values it is given", self.name, skipped.name));
            }
            argv.push("--".into());
            argv.extend(positionals.into_iter().flat_map(|(_, values)| values));
        }
        Ok(argv)
    }
}

/// The most a tool's description takes of its command's help, and a property's of its argument's,
/// in characters: whole sentences up to it, and always the first, or the first's leading clauses, so that the catalog fits the
/// catalog compact (the size guard in mcp.rs's tests) while each tool and property still says what it is.
const TOOL_CHARS: usize = 240;
const PROPERTY_CHARS: usize = 180;

/// The CLI's help as a tool's or a property's description: the leading whole sentences of its first
/// paragraph, on one line, up to `max` characters but never fewer than one sentence. clap keeps a
/// doc comment's first paragraph as the short help and all of it for `--help`, which the `help` tool
/// gives in full; text set only as a long help (`link`'s `kind`) is cut alike.
fn short(help: String, max: usize) -> String {
    let first = help.trim().split("\n\n").next().unwrap_or_default();
    let line = first.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    for sentence in line.split_inclusive(". ") {
        if !out.is_empty() && out.len() + sentence.len() > max {
            break;
        }
        out.push_str(sentence);
    }
    // A first sentence longer than `max` keeps its leading clauses: up to the last `; `, `, ` or
    // `: ` within `max` that leaves at least a third of it and is outside any parenthesis or code
    // span, so that no cut leaves a fragment ("With triage") or an open bracket.
    if out.len() > max {
        let within = &out[..out.floor_char_boundary(max)];
        let outside = |at: usize| {
            let before = &within[..at];
            before.matches('(').count() == before.matches(')').count() && before.matches('`').count().is_multiple_of(2)
        };
        let cut = ["; ", ", ", ": "]
            .iter()
            .flat_map(|mark| within.match_indices(mark).map(|(at, _)| at))
            .filter(|at| *at >= max / 3 && outside(*at))
            .max();
        if let Some(cut) = cut {
            out.truncate(cut);
        }
    }
    let out = out.trim_end();
    out.strip_suffix('.').unwrap_or(out).to_string()
}

/// One value of a property as the command line gives it: a string as it is, a number or a
/// boolean as it reads.
fn scalar(name: &str, v: &Value) -> Result<String, String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        _ => Err(format!("`{name}` takes a string, a number or, for a list, an array of them")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(words: &str) -> Spec {
        specs().into_iter().find(|s| s.words == words).expect("offered")
    }

    fn args(v: Value) -> Map<String, Value> {
        v.as_object().cloned().expect("an object")
    }

    #[test]
    fn every_offered_command_is_a_tool_that_writes_or_reads_as_the_cli_says() {
        for words in OFFERED {
            assert!(
                crate::JOINING.iter().any(|(line, _)| line == words),
                "`{words}` is a command the CLI lists as writing or not"
            );
        }
        let names: Vec<String> = specs().into_iter().map(|s| s.name).collect();
        assert!(names.contains(&"principal_scope_project_wide".to_string()));
        assert_eq!(names.len(), OFFERED.len());
    }

    #[test]
    fn no_tool_takes_an_actor_or_a_file() {
        for spec in specs() {
            let properties = spec.schema["properties"].as_object().unwrap();
            for left_out in ["as", "act_as", "format", "no_autostart", "path", "lines", "against", "side", "hook_event"]
            {
                assert!(!properties.contains_key(left_out), "{} takes {left_out}", spec.name);
            }
            assert!(properties.contains_key(PROJECT) && properties.contains_key(SCOPE), "{}", spec.name);
        }
    }

    #[test]
    fn a_description_is_the_helps_leading_sentences_or_clauses_never_a_fragment() {
        assert_eq!(short("One. Two three.\n\nMore.".into(), 100), "One. Two three");
        assert_eq!(short("One sentence here. Another that is long enough to go over.".into(), 30), "One sentence here");
        // A long first sentence keeps its clauses up to the last boundary outside brackets.
        let long = "With triage: serve the whole project rather than a scope, a deliberate endorsement (that suits, only a small project) and more words";
        assert_eq!(short(long.into(), 90), "With triage: serve the whole project rather than a scope");
        for spec in specs() {
            assert!(spec.description.len() <= TOOL_CHARS + 60, "{}: {}", spec.name, spec.description);
            assert_eq!(spec.description.matches('(').count(), spec.description.matches(')').count(), "{}", spec.name);
        }
    }

    #[test]
    fn a_tools_properties_are_its_commands_arguments_and_say_which_are_required() {
        let create = tool("task create");
        let props = create.schema["properties"].as_object().unwrap();
        assert_eq!(props["title"]["type"], "string");
        assert_eq!(props["after"]["type"], "array");
        assert_eq!(props["propose"]["type"], "boolean");
        // The argument's short help, and a default as the schema's own `default`.
        assert_eq!(props["after"]["description"], "depends_on targets");
        assert_eq!(props["what"]["default"], "");
        assert_eq!(tool("ask").schema["properties"]["priority"]["default"], "now");
        assert_eq!(tool("comment").schema["properties"]["priority"]["default"], "next");
        assert_eq!(create.schema["required"], json!(["project", "title"]));
        let list = tool("task list");
        let props = list.schema["properties"].as_object().unwrap();
        // The model's enums parse from their words, as the CLI takes them, which name no list.
        assert_eq!(props["status"]["type"], "string");
        assert_eq!(props["limit"]["type"], "integer");
        assert_eq!(props["limit"]["default"], 50);
        assert!(tool("inbox").schema["properties"].as_object().unwrap().contains_key("for"));
        // A parent's arguments are its subcommand's tool's too, after the parent's word.
        let pop = tool("inbox pop");
        assert!(pop.schema["properties"].as_object().unwrap().contains_key("for"));
        let argv = pop.argv(&args(json!({"project": "p", "for": "impl-b", "unread": true}))).unwrap();
        assert_eq!(&argv[3..], ["inbox", "--unread", "--for=impl-b", "pop"]);
        <crate::Cli as clap::Parser>::try_parse_from(&argv).expect("the CLI parses it");
    }

    #[test]
    fn a_calls_arguments_are_the_command_line_with_no_value_read_as_a_flag() {
        let argv = tool("task create")
            .argv(&args(json!({
                "project": "p", "title": "--as dnorman", "why": "--format text", "after": ["a1", "-b2"],
                "propose": true, "needs_review": false, "priority": "now", "label": ["bug,feature"],
            })))
            .unwrap();
        assert_eq!(
            argv,
            [
                "fwd",
                "--format",
                "json",
                "task",
                "create",
                "--why=--format text",
                "--after=a1",
                "--after=-b2",
                "--label=bug,feature",
                "--priority=now",
                "--propose",
                "--",
                "--as dnorman"
            ]
        );
        let parsed = <crate::Cli as clap::Parser>::try_parse_from(&argv).expect("the CLI parses it");
        let crate::Command::Task { command: crate::TaskCommand::Create(a) } = parsed.command else { panic!() };
        assert_eq!((a.title.as_str(), a.why.as_str()), ("--as dnorman", "--format text"));
        assert_eq!(a.after, ["a1", "-b2"]);
        assert_eq!(a.label, ["bug", "feature"]);
        let comment = tool("comment").argv(&args(json!({"project": "p", "first": "8z_sQwsR", "text": "-x"}))).unwrap();
        assert_eq!(&comment[3..], ["comment", "--", "8z_sQwsR", "-x"]);
        let read = tool("read").argv(&args(json!({"project": "p", "ids": ["-a1", "b2"]}))).unwrap();
        assert_eq!(&read[3..], ["read", "--", "-a1", "b2"]);
    }

    #[test]
    fn an_argument_the_tool_does_not_take_is_refused_naming_those_it_does() {
        let refused = tool("task start").argv(&args(json!({"project": "p", "id": "x", "as": "dnorman"}))).unwrap_err();
        assert!(refused.contains("takes no argument `as`") && refused.contains("id, project, scope"), "{refused}");
        let refused = tool("task create").argv(&args(json!({"project": "p", "title": {"x": 1}}))).unwrap_err();
        assert!(refused.contains("`title` takes a string"), "{refused}");
        let refused = tool("comment").argv(&args(json!({"project": "p", "text": "hello"}))).unwrap_err();
        assert!(refused.contains("takes `first` before"), "{refused}");
        let refused =
            tool("task create").argv(&args(json!({"project": "p", "title": "t", "priority": 1}))).unwrap_err();
        assert!(refused.contains("now, next or later"), "{refused}");
    }
}
