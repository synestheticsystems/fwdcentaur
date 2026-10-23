//! The fwd plugin's hook for Claude Code (`integrations/claude-code/hooks`): which Bash commands it
//! stamps with the ids of the session and the agent that run them, and the stamp. It exists so that
//! an agent's fwd commands act as that agent's own principal, with no `--as`
//! (docs/design/agents-and-inbox.md, "An agent's identity"; docs/DECISIONS.md, D112 and D118).
//!
//! Claude Code applies a PreToolUse hook's change to a command only when the hook also approves the
//! command (`permissionDecision: "allow"`), which skips the permission prompt and, in auto mode, the
//! classifier; only the deny and ask rules of the person's settings still apply. So the hook stamps,
//! and approves, only a command whose approval gives away nothing but fwd ([`stampable`]): fwd
//! commands an agent runs as itself, `cd`, and filters that read fwd's output on their standard
//! input and print to theirs, joined by `|`, `&&`, `||`, `;` or a newline, with nothing the shell
//! would expand or run besides. Every other command gets no answer from the hook and goes through
//! the permission flow as it would without the plugin, unstamped. So does a call that asks to run
//! outside Claude Code's sandbox, whatever its command ([`LEAVES_THE_SANDBOX`]): the hook approves
//! a command for what it runs, and that approval never extends to a call the agent asked to elevate.
//!
//! The inbox hooks, which Claude Code runs itself, are not Bash commands and carry no stamp, but
//! Claude Code hands them the same JSON on standard input, naming the session and, inside a
//! subagent, the agent whose hook fired: a command given `--format hook` reads it and acts for that
//! agent's principal ([`hook_subject`]), or, on a session's own thread that has none yet, for the
//! one FWD_AS names, as the hooks did before, so that no subagent reads or marks its lead's inbox.

use anyhow::{anyhow, Result};
use fwd_core::subject;
use serde_json::{json, Value};

/// fwd's commands the hook never approves: they act for the machine or its person, not for an
/// agent, so they need no agent's stamp, and approving them without a prompt would give away more
/// than an agent's own work. Maintenance (`admin`), the daemon (`daemon`), joining or creating a
/// project and recording the person (`init`), the page, which can put the daemon on the LAN
/// (`ui`), and the machine's index of projects (`project`). They run unstamped, under the normal
/// permission flow.
const NOT_FOR_AN_AGENT: &[&str] = &["admin", "daemon", "init", "ui", "project"];

/// fwd's global options that take the next word as their value, before a command's words.
const GLOBAL_WITH_VALUE: &[&str] = &["--project", "--format", "--as", "--scope"];

/// The Bash tool's argument by which an agent asks Claude Code to run the command outside its
/// sandbox (code.claude.com/docs/en/sandboxing, "The unsandboxed retry escape hatch"). The hook
/// leaves such a call alone, whatever its command, so that it goes through the permission flow and
/// the person sees the request: the hook approves a command for what it runs, and that approval
/// must not extend to a call the agent asked to elevate, as the lead ruled on 2026-09-30 (task
/// W-ysfMYD). Unstamped, the call's fwd writes are refused in a session whose agents are bound,
/// and the refusal (`subject::unstamped`) says to run fwd without this argument.
const LEAVES_THE_SANDBOX: &str = "dangerouslyDisableSandbox";

/// What the hook prints for the PreToolUse input `input`, the JSON Claude Code hands it on standard
/// input: the JSON that approves the Bash command with `FWD_CLAUDE_SESSION` and `FWD_CLAUDE_AGENT`
/// exported before it, when [`stampable`] accepts the command, the call does not ask to run outside
/// the sandbox ([`LEAVES_THE_SANDBOX`]) and the ids are ones a subject holds (`subject::valid_id`);
/// otherwise none, and the hook prints nothing. `agent_id`, which Claude Code gives only inside a
/// subagent, is `main` without one.
///
/// Claude Code runs the call with `updatedInput` as its whole input, not as a change to it
/// (code.claude.com/docs/en/hooks, "PreToolUse decision control"), so `updatedInput` is the
/// call's input as it came with its command alone replaced: `run_in_background`, `timeout`,
/// `description` and any other argument, named here or not, go back as given. Without them, a
/// `fwd watch` the agent ran in the background would run in the foreground and hold the agent's
/// turn until the default timeout moved it to the background.
pub fn stamp(input: &str) -> Option<String> {
    let v: Value = serde_json::from_str(input).ok()?;
    if v.get("hook_event_name").and_then(Value::as_str).is_some_and(|event| event != "PreToolUse") {
        return None;
    }
    if v.get("tool_name").and_then(Value::as_str) != Some("Bash") {
        return None;
    }
    let tool_input = v.get("tool_input")?.as_object()?;
    let command = tool_input.get("command")?.as_str()?;
    // Claude Code sends a boolean, when it sends one: any value but false or null asks to leave.
    if !matches!(tool_input.get(LEAVES_THE_SANDBOX), None | Some(Value::Null | Value::Bool(false))) {
        return None;
    }
    let session = v.get("session_id")?.as_str()?;
    let agent = agent_id(&v)?;
    if !subject::valid_id(session) || !subject::valid_id(agent) || !stampable(command) {
        return None;
    }
    let stamped = format!("export {}='{session}' {}='{agent}'; {command}", subject::SESSION_VAR, subject::AGENT_VAR);
    let mut updated = tool_input.clone();
    updated.insert("command".to_string(), Value::String(stamped));
    let out = json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "updatedInput": Value::Object(updated),
        }
    });
    Some(out.to_string())
}

/// The agent whose hook runs a command given `--format hook`, from `input`, the JSON Claude Code
/// hands every hook on standard input, as it hands the stamp its own: the subject
/// `claude-code:<session_id>/<agent_id>`, whose principal the command acts for, the agent id being
/// `main` without one, as in the stamp. None when `input` is no hook's JSON, no JSON object with a
/// `session_id`, as when a person or a script runs the command, which then reads `--as` and
/// `FWD_AS`; refused for ids a subject cannot hold (`subject::claude_code`).
pub fn hook_subject(input: &str) -> Option<Result<String>> {
    let v: Value = serde_json::from_str(input).ok()?;
    let session = v.get("session_id")?.as_str()?;
    Some(match agent_id(&v) {
        Some(agent) => subject::claude_code(session, agent),
        None => Err(anyhow!("the hook's `agent_id` is not a string, as the id Claude Code gives a subagent is")),
    })
}

/// The id of the agent a hook's input `v` names: its `agent_id`, which Claude Code gives only
/// inside a subagent, and [`subject::MAIN`], the session's own thread, without one. None for an
/// `agent_id` that is not a string.
fn agent_id(v: &Value) -> Option<&str> {
    match v.get("agent_id") {
        None | Some(Value::Null) => Some(subject::MAIN),
        Some(id) => id.as_str(),
    }
}

/// Whether the hook stamps and approves `command`: each command in it is `fwd` (the bare word, not a
/// path to a binary) with a command other than those [`NOT_FOR_AN_AGENT`] lists, `cd`, or a filter
/// that reads only its standard input ([`reads_only_input`]), at least one of them `fwd`; they are
/// joined by `|`, `&&`, `||`, `;` or a newline; and nothing in it asks the shell for more: no `$`
/// or backtick outside single quotes, no globbing, grouping, background or history, no variable
/// set before a program, and no redirection but `2>&1` and to or from `/dev/null`.
pub fn stampable(command: &str) -> bool {
    let Some(tokens) = tokens(command) else { return false };
    // Each command's words, and what joins it to the next.
    let mut commands: Vec<Vec<String>> = vec![Vec::new()];
    let mut joins: Vec<Join> = Vec::new();
    for token in tokens {
        match token {
            Token::Word(word) => commands.last_mut().expect("one at least").push(word),
            Token::Redirect => {}
            Token::Join(join) => {
                joins.push(join);
                commands.push(Vec::new());
            }
        }
    }
    let mut any_fwd = false;
    for (i, words) in commands.iter().enumerate() {
        let Some((program, args)) = words.split_first() else {
            // An empty command is a blank line, or what a trailing `;` leaves; beside a pipe, `&&`
            // or `||` the shell refuses the line, and so does the hook.
            let before = i.checked_sub(1).map(|j| joins[j]);
            let after = joins.get(i).copied();
            if [before, after].into_iter().flatten().any(|join| join != Join::Then) {
                return false;
            }
            continue;
        };
        let accepted = match program.as_str() {
            "fwd" => {
                any_fwd = true;
                fwd_command(args).is_none_or(|word| !NOT_FOR_AN_AGENT.contains(&word))
            }
            "cd" => true,
            filter => reads_only_input(filter, args),
        };
        if !accepted {
            return false;
        }
    }
    any_fwd
}

/// The first word of a fwd invocation's arguments that is its command (`task` in `fwd --format json
/// task show X`), past the global options and their values; none for `fwd` alone or `fwd --help`.
fn fwd_command(args: &[String]) -> Option<&str> {
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if GLOBAL_WITH_VALUE.contains(&word.as_str()) {
            words.next();
        } else if !word.starts_with('-') {
            return Some(word);
        }
    }
    None
}

/// A filter the hook lets beside fwd: its short options that take no value, those that take one
/// (the rest of the word, or the next word), its long options that take no value and those that
/// take one (`--name=value`, or the next word), and how many words it takes that are no option (a
/// pattern for `grep`, a program for `jq`). A word it does not know, and any further word, is a file
/// to read or write, so the filter is refused: it reads only its standard input.
struct Filter {
    name: &'static str,
    flags: &'static str,
    with_value: &'static str,
    long_flags: &'static [&'static str],
    long_with_value: &'static [&'static str],
    /// Whether it takes one word that is no option: `grep`'s pattern (none when `-e` gives it) and
    /// `jq`'s program.
    one_operand: bool,
}

const FILTERS: &[Filter] = &[
    Filter {
        name: "head",
        flags: "qv",
        with_value: "nc",
        long_flags: &["quiet", "silent", "verbose"],
        long_with_value: &["lines", "bytes"],
        one_operand: false,
    },
    Filter {
        name: "tail",
        flags: "qvr",
        with_value: "nc",
        long_flags: &["quiet", "silent", "verbose"],
        long_with_value: &["lines", "bytes"],
        one_operand: false,
    },
    Filter {
        name: "wc",
        flags: "lwcmL",
        with_value: "",
        long_flags: &["lines", "words", "bytes", "chars", "max-line-length"],
        long_with_value: &[],
        one_operand: false,
    },
    Filter {
        name: "cut",
        flags: "snz",
        with_value: "bcdf",
        long_flags: &["only-delimited", "complement", "zero-terminated"],
        long_with_value: &["bytes", "characters", "delimiter", "fields", "output-delimiter"],
        one_operand: false,
    },
    // No -o or --output, which write a file, no -T, which writes temporary ones, and no
    // --compress-program, which runs one.
    Filter {
        name: "sort",
        flags: "bdfgiMhnRrVsuz",
        with_value: "kt",
        long_flags: &[
            "ignore-leading-blanks",
            "dictionary-order",
            "ignore-case",
            "general-numeric-sort",
            "human-numeric-sort",
            "month-sort",
            "numeric-sort",
            "random-sort",
            "reverse",
            "version-sort",
            "stable",
            "unique",
            "zero-terminated",
        ],
        long_with_value: &["key", "field-separator"],
        one_operand: false,
    },
    // No second operand, which uniq writes to.
    Filter {
        name: "uniq",
        flags: "cdDiuz",
        with_value: "fsw",
        long_flags: &["count", "repeated", "unique", "ignore-case", "zero-terminated", "all-repeated", "group"],
        long_with_value: &["skip-fields", "skip-chars", "check-chars"],
        one_operand: false,
    },
    // No -r or -R, which read a directory (the current one, given no file), and no -f, which reads
    // patterns from a file.
    Filter {
        name: "grep",
        flags: "ivcnlLoqswxEFGPhHaIUzbZ",
        with_value: "emABC",
        long_flags: &[
            "ignore-case",
            "invert-match",
            "count",
            "line-number",
            "files-with-matches",
            "files-without-match",
            "only-matching",
            "quiet",
            "silent",
            "no-messages",
            "word-regexp",
            "line-regexp",
            "extended-regexp",
            "fixed-strings",
            "basic-regexp",
            "perl-regexp",
            "no-filename",
            "with-filename",
            "text",
            "null",
            "null-data",
            "color",
            "colour",
        ],
        long_with_value: &["regexp", "max-count", "after-context", "before-context", "context", "color", "colour"],
        one_operand: true,
    },
    // No -f, --from-file, --slurpfile, --rawfile or -L, which read files, and a program that
    // imports nothing and reads no environment ([`jq_program_reads_only_input`]).
    Filter {
        name: "jq",
        flags: "rjcnseaSCMR",
        with_value: "",
        long_flags: &[
            "raw-output",
            "join-output",
            "compact-output",
            "null-input",
            "slurp",
            "exit-status",
            "ascii-output",
            "sort-keys",
            "color-output",
            "monochrome-output",
            "raw-input",
            "tab",
            "seq",
            "stream",
            "raw-output0",
        ],
        long_with_value: &["indent"],
        one_operand: true,
    },
];

/// Whether `program` is a filter the hook lets beside fwd ([`FILTERS`]) and `args` give it only
/// options it knows, and no file: it reads its standard input alone, so that approving the command
/// reads nothing fwd did not print. `--arg` and `--argjson` give `jq` a name and a value.
fn reads_only_input(program: &str, args: &[String]) -> bool {
    let Some(filter) = FILTERS.iter().find(|f| f.name == program) else { return false };
    let mut operands: Vec<&str> = Vec::new();
    let mut given_pattern = false;
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if !operands.is_empty() || word == "-" || !word.starts_with('-') {
            operands.push(word);
            continue;
        }
        if word == "--" {
            operands.extend(words.by_ref().map(String::as_str));
            break;
        }
        if let Some(long) = word.strip_prefix("--") {
            let (name, value) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (long, None),
            };
            if filter.name == "jq" && value.is_none() && matches!(name, "arg" | "argjson") {
                // A name and a value, neither a file.
                if words.next().is_none() || words.next().is_none() {
                    return false;
                }
                continue;
            }
            if filter.long_with_value.contains(&name) {
                given_pattern |= name == "regexp";
                if value.is_none() && !filter.long_flags.contains(&name) && words.next().is_none() {
                    return false;
                }
            } else if !(filter.long_flags.contains(&name) && value.is_none()) {
                return false;
            }
            continue;
        }
        // A cluster of short options (`-rn`), where one that takes a value takes the rest of the
        // word or, at its end, the next word; `-40` is head's and tail's count of lines.
        let cluster = &word[1..];
        if matches!(filter.name, "head" | "tail") && cluster.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        for (at, letter) in cluster.char_indices() {
            if filter.with_value.contains(letter) {
                given_pattern |= letter == 'e';
                if at + letter.len_utf8() == cluster.len() && words.next().is_none() {
                    return false;
                }
                break;
            }
            if !filter.flags.contains(letter) {
                return false;
            }
        }
    }
    let wanted = usize::from(filter.one_operand && !given_pattern);
    if operands.len() != wanted {
        return false;
    }
    filter.name != "jq" || operands.first().is_some_and(|program| jq_program_reads_only_input(program))
}

/// Whether a jq program reads nothing but its input: no module or data file (`import`,
/// `include`), no environment (`env`, `$ENV`), which holds the session's tokens, and no input file's
/// name (`input_filename`). Judged by the words alone, so a field named `environment` is refused
/// too, and the command runs unstamped.
fn jq_program_reads_only_input(program: &str) -> bool {
    !["import", "include", "env", "ENV", "input_filename", "$__prog"].iter().any(|word| program.contains(word))
}

/// What joins two commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Join {
    /// `|`.
    Pipe,
    /// `&&`.
    And,
    /// `||`.
    Or,
    /// `;` or a newline.
    Then,
}

/// A piece of a command line as the shell splits it, of the pieces [`tokens`] lets through.
#[derive(Debug, PartialEq, Eq)]
enum Token {
    /// A word, with its quotes and escapes taken off.
    Word(String),
    Join(Join),
    /// `2>&1`, or a redirection to or from `/dev/null`.
    Redirect,
}

/// Whether the shell reads `c` as a blank between words: a space or a tab, and no other character
/// (a no-break space is part of a word to it, as it is here).
fn blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// The characters that end a word outside quotes.
fn ends_word(c: char) -> bool {
    blank(c) || "\n|&;<>()".contains(c)
}

/// The characters that ask the shell for more than a word outside quotes: expansion (`$`, a
/// backtick), globbing (`*`, `?`, `[`, `]`), grouping and subshells (`(`, `)`, `{`, `}`), history
/// (`!`, `^`). Any of them outside quotes makes a command one the hook leaves alone.
fn asks_for_more(c: char) -> bool {
    "$`*?[](){}!^".contains(c)
}

/// `command` split as the shell splits it, when it holds only words, the joins and the
/// redirections the hook allows; none when it holds anything else (see [`asks_for_more`]), a
/// redirection but `2>&1` and to or from `/dev/null`, a background `&`, a here-document, `;;`, or
/// a quote left open. Inside single quotes every character is itself; inside double quotes `$` and
/// a backtick still ask the shell for more, and a backslash escapes `"`, `\`, `$`, a backtick and a
/// newline. A `#` that begins a word begins a comment, which runs to the end of its line and is
/// skipped as the shell skips it, quotes and all: read as words, a quote in a comment could hide
/// the lines after it, which the shell runs.
fn tokens(command: &str) -> Option<Vec<Token>> {
    let mut out = Vec::new();
    let mut chars = command.chars().peekable();
    // The word being read, whether one is being read (`''` is a word too), and whether every
    // character of it so far was a digit outside quotes, which makes it a redirection's descriptor
    // when a `<` or a `>` follows at once (`2>`).
    let mut word = String::new();
    let mut in_word = false;
    let mut plain_digits = true;
    while let Some(c) = chars.next() {
        match c {
            '#' if !in_word => {
                while chars.peek().is_some_and(|c| *c != '\n') {
                    chars.next();
                }
            }
            '\'' => {
                in_word = true;
                plain_digits = false;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        inside => word.push(inside),
                    }
                }
            }
            '"' => {
                in_word = true;
                plain_digits = false;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            escaped @ ('"' | '\\' | '$' | '`') => word.push(escaped),
                            '\n' => {}
                            other => {
                                word.push('\\');
                                word.push(other);
                            }
                        },
                        '$' | '`' => return None,
                        inside => word.push(inside),
                    }
                }
            }
            '\\' => match chars.next()? {
                // A line continued: the shell takes both away.
                '\n' => {}
                escaped => {
                    in_word = true;
                    plain_digits = false;
                    word.push(escaped);
                }
            },
            '<' | '>' => {
                let descriptor = if in_word && plain_digits && !word.is_empty() {
                    in_word = false;
                    Some(std::mem::take(&mut word))
                } else {
                    end_word(&mut out, &mut word, &mut in_word);
                    None
                };
                plain_digits = true;
                redirection(c, descriptor.as_deref(), &mut chars)?;
                out.push(Token::Redirect);
            }
            '&' if chars.peek() == Some(&'>') => {
                chars.next();
                end_word(&mut out, &mut word, &mut in_word);
                plain_digits = true;
                if target(&mut chars)? != "/dev/null" {
                    return None;
                }
                out.push(Token::Redirect);
            }
            '|' | '&' | ';' | '\n' => {
                end_word(&mut out, &mut word, &mut in_word);
                plain_digits = true;
                let join = match (c, chars.peek()) {
                    ('|', Some('|')) => Join::Or,
                    ('|', Some('&')) => return None,
                    ('|', _) => Join::Pipe,
                    ('&', Some('&')) => Join::And,
                    ('&', _) => return None,
                    (';', Some(';')) => return None,
                    _ => Join::Then,
                };
                if matches!(join, Join::Or | Join::And) {
                    chars.next();
                }
                out.push(Token::Join(join));
            }
            c if blank(c) => {
                end_word(&mut out, &mut word, &mut in_word);
                plain_digits = true;
            }
            c if asks_for_more(c) => return None,
            c => {
                in_word = true;
                plain_digits &= c.is_ascii_digit();
                word.push(c);
            }
        }
    }
    end_word(&mut out, &mut word, &mut in_word);
    Some(out)
}

/// Ends the word being read, if one is.
fn end_word(out: &mut Vec<Token>, word: &mut String, in_word: &mut bool) {
    if std::mem::take(in_word) {
        out.push(Token::Word(std::mem::take(word)));
    }
}

/// Reads the rest of a redirection that began with `first` (`<` or `>`), after its descriptor if it
/// had one (`2` in `2>&1`): none unless it is `2>&1` or sends a descriptor from 0 to 2 to or from
/// `/dev/null` (`>`, `>>`, `<`).
fn redirection(first: char, descriptor: Option<&str>, chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<()> {
    if descriptor.is_some_and(|d| !matches!(d, "0" | "1" | "2")) {
        return None;
    }
    match (first, chars.peek().copied()) {
        ('>', Some('&')) => {
            chars.next();
            (descriptor == Some("2") && target(chars)? == "1").then_some(())
        }
        ('>', Some('>')) => {
            chars.next();
            (target(chars)? == "/dev/null").then_some(())
        }
        // `>|` overwrites a file whatever the shell's options; `<<`, `<&`, `<>`, `<(` and `>(`
        // are here-documents, descriptors, files opened both ways and processes.
        ('>', Some('|' | '(')) | ('<', Some('<' | '&' | '>' | '(')) => None,
        _ => (target(chars)? == "/dev/null").then_some(()),
    }
}

/// The word a redirection names, after any blanks: quoted or not, as a word is read; none when
/// there is none, or it asks the shell for more.
fn target(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<String> {
    while chars.peek().copied().is_some_and(blank) {
        chars.next();
    }
    let mut word = String::new();
    while let Some(&c) = chars.peek() {
        if ends_word(c) {
            break;
        }
        chars.next();
        match c {
            '\'' => loop {
                match chars.next()? {
                    '\'' => break,
                    inside => word.push(inside),
                }
            },
            '"' => loop {
                match chars.next()? {
                    '"' => break,
                    '$' | '`' | '\\' => return None,
                    inside => word.push(inside),
                }
            },
            '\\' => word.push(chars.next()?),
            c if asks_for_more(c) => return None,
            c => word.push(c),
        }
    }
    (!word.is_empty()).then_some(word)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(command: &str, agent: Option<&str>) -> String {
        let mut v = json!({
            "session_id": "56693b92-71ca-49ec-8ab4-aee1de36a4d3",
            "transcript_path": "/tmp/t.jsonl",
            "cwd": "/tmp",
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": command, "description": "d", "timeout": 1000, "run_in_background": true},
            "tool_use_id": "toolu_1",
        });
        if let Some(agent) = agent {
            v["agent_id"] = json!(agent);
            v["agent_type"] = json!("general-purpose");
        }
        v.to_string()
    }

    #[test]
    fn a_command_of_fwd_alone_is_stamped_with_the_session_and_the_agent_and_approved() {
        let out: Value =
            serde_json::from_str(&stamp(&input("fwd task show LK12MGks", Some("a7b8a42"))).unwrap()).unwrap();
        assert_eq!(
            out,
            json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "updatedInput": {
                    "command": "export FWD_CLAUDE_SESSION='56693b92-71ca-49ec-8ab4-aee1de36a4d3' FWD_CLAUDE_AGENT='a7b8a42'; fwd task show LK12MGks",
                    "description": "d",
                    "timeout": 1000,
                    "run_in_background": true,
                },
            }})
        );
        // The session's own thread has no agent id: it is `main`.
        let out: Value = serde_json::from_str(&stamp(&input("fwd whoami", None)).unwrap()).unwrap();
        assert_eq!(
            out["hookSpecificOutput"]["updatedInput"]["command"],
            "export FWD_CLAUDE_SESSION='56693b92-71ca-49ec-8ab4-aee1de36a4d3' FWD_CLAUDE_AGENT='main'; fwd whoami"
        );
    }

    /// Claude Code runs the call with `updatedInput` as its whole input, so the stamp gives back the
    /// call's arguments as they came, one it does not name included, with the command alone
    /// changed: a `fwd watch` run in the background stays there. A command left alone gets no
    /// answer, whatever its arguments.
    #[test]
    fn a_stamped_call_keeps_its_other_arguments_as_they_came() {
        let mut v: Value = serde_json::from_str(&input("fwd --format json watch --count 1", Some("a1"))).unwrap();
        v["tool_input"]["an_argument_of_a_later_claude_code"] = json!({"kept": [1, "two"]});
        let out: Value = serde_json::from_str(&stamp(&v.to_string()).unwrap()).unwrap();
        let mut expected = v["tool_input"].clone();
        expected["command"] = json!(
            "export FWD_CLAUDE_SESSION='56693b92-71ca-49ec-8ab4-aee1de36a4d3' FWD_CLAUDE_AGENT='a1'; fwd --format json watch --count 1"
        );
        assert_eq!(out["hookSpecificOutput"]["updatedInput"], expected);
        v["tool_input"]["command"] = json!("cargo test");
        assert_eq!(stamp(&v.to_string()), None);
    }

    /// A call that asks to run outside the sandbox is left alone even when its command is fwd alone,
    /// so that the person sees the request in the permission flow; one that says it stays inside is
    /// stamped, the argument given back as it came.
    #[test]
    fn a_call_that_asks_to_leave_the_sandbox_is_left_alone() {
        let mut v: Value = serde_json::from_str(&input("fwd whoami", Some("a1"))).unwrap();
        for leaves in [json!(true), json!("true"), json!(1)] {
            v["tool_input"]["dangerouslyDisableSandbox"] = leaves.clone();
            assert_eq!(stamp(&v.to_string()), None, "{leaves}");
        }
        for stays in [json!(false), Value::Null] {
            v["tool_input"]["dangerouslyDisableSandbox"] = stays.clone();
            let out: Value = serde_json::from_str(&stamp(&v.to_string()).expect("stamped")).unwrap();
            assert_eq!(out["hookSpecificOutput"]["updatedInput"]["dangerouslyDisableSandbox"], stays);
        }
    }

    #[test]
    fn nothing_is_printed_for_another_tool_another_event_or_ids_a_subject_cannot_hold() {
        let edit = input("fwd whoami", None).replace("\"Bash\"", "\"Edit\"");
        assert_eq!(stamp(&edit), None);
        let after = input("fwd whoami", None).replace("\"PreToolUse\"", "\"PostToolUse\"");
        assert_eq!(stamp(&after), None);
        assert_eq!(stamp(&input("fwd whoami", Some("a'; rm -rf ~; '"))), None);
        assert_eq!(stamp("not json"), None);
        assert_eq!(stamp("{}"), None);
    }

    /// The inbox hooks' input: the Stop hook's in the session's own thread, a PostToolBatch hook's
    /// in a subagent (`agent_id` and `agent_type` beside the common fields).
    #[test]
    fn a_hooks_input_names_the_agent_whose_hook_fired() {
        let session = "56693b92-71ca-49ec-8ab4-aee1de36a4d3";
        let stop = json!({"session_id": session, "transcript_path": "/tmp/t.jsonl", "cwd": "/tmp", "hook_event_name": "Stop", "stop_hook_active": false});
        assert_eq!(hook_subject(&stop.to_string()).unwrap().unwrap(), format!("claude-code:{session}/main"));
        let batch = json!({"session_id": session, "hook_event_name": "PostToolBatch", "agent_id": "a7b8a42", "agent_type": "general-purpose", "tool_calls": []});
        assert_eq!(hook_subject(&batch.to_string()).unwrap().unwrap(), format!("claude-code:{session}/a7b8a42"));
        let null = json!({"session_id": session, "hook_event_name": "Stop", "agent_id": null});
        assert_eq!(hook_subject(&null.to_string()).unwrap().unwrap(), format!("claude-code:{session}/main"));
        // No hook's JSON: the command reads --as and FWD_AS.
        for input in ["", "not json", "{}", "[]", r#"{"session_id": 7}"#] {
            assert!(hook_subject(input).is_none(), "{input}");
        }
        // A hook's JSON naming ids a subject cannot hold is refused, never read as no hook's.
        for input in [
            json!({"session_id": "s 1", "hook_event_name": "Stop"}),
            json!({"session_id": session, "agent_id": "a/b"}),
            json!({"session_id": session, "agent_id": 7}),
        ] {
            assert!(hook_subject(&input.to_string()).unwrap().is_err(), "{input}");
        }
    }

    #[test]
    fn the_commands_the_hook_stamps_are_fwd_cd_and_filters_of_fwds_output() {
        for command in [
            "fwd whoami",
            "fwd --format json task show LK12MGks",
            "fwd task list | head -40",
            "fwd task list | tail -n 5",
            "fwd inbox --format json 2>&1 | jq -r '.[].id'",
            "fwd task show X | jq --arg id X '.id == $id'",
            "fwd graph | grep -c ready",
            "fwd graph | grep -e ready -i | wc -l",
            "fwd task list --format json | jq -c '.[]' | sort -u | uniq -c | cut -d, -f1",
            "cd /tmp/work && fwd task next --claim",
            "fwd task start X && fwd comment X 'started: the `foo` loop costs $5'",
            "fwd comment X \"it's done, see crates/fwd/src/main.rs\"",
            "fwd comment X 'first line\nsecond line'",
            "fwd ask --about X \"why?\" 2>/dev/null",
            "fwd whoami >/dev/null 2>&1 || fwd principal list",
            "fwd whoami; fwd inbox pop",
            "fwd whoami\nfwd inbox\n",
            "fwd whoami # a comment, with a quote: it's skipped",
            "# a comment first\nfwd whoami",
            "fwd reply N1 'it'\\''s fixed'",
            "fwd --as claude principal bind claude",
            "fwd task list --format json </dev/null",
            "fwd whoami &>/dev/null",
            "fwd",
        ] {
            assert!(stampable(command), "stamped: {command}");
        }
    }

    #[test]
    fn a_command_that_gives_away_more_than_fwd_is_left_alone() {
        for command in [
            // Another program, however harmless, or fwd by a path to its binary.
            "cargo test --workspace && fwd task submit X --evidence 'green'",
            "fwd whoami && cargo build",
            "ls; fwd whoami",
            "./target/debug/fwd whoami",
            "/usr/local/bin/fwd whoami",
            "FWD_SCOPE=x fwd task next",
            "export FWD_CLAUDE_AGENT=main; fwd whoami",
            // What the shell would expand or run besides.
            "fwd comment X \"$(cat notes.md)\"",
            "fwd comment X \"`cat notes.md`\"",
            "fwd comment X $(cat notes.md)",
            "fwd comment X \"costs $5\"",
            "fwd comment X $HOME",
            "fwd task show *",
            "fwd task show X?",
            "(fwd whoami)",
            "{ fwd whoami; }",
            "fwd whoami &",
            "fwd whoami | tee out.txt",
            "fwd whoami |& head",
            // Redirections to files, and the rest.
            "fwd task list > tasks.json",
            "fwd task list >> tasks.json",
            "fwd task list 2> errors.txt",
            "fwd task list >| tasks.json",
            "fwd comment X - <<'EOF'\ntext\nEOF",
            "fwd task list < input.txt",
            "fwd task list >&2",
            "fwd task list 3>/dev/null",
            "fwd task list <(fwd graph)",
            // fwd's commands that act for the machine or its person.
            "fwd admin migrate",
            "fwd admin restore backup",
            "fwd --no-autostart daemon stop",
            "fwd --project x daemon start --lan",
            "fwd init --handle mallory",
            "fwd ui --lan",
            "fwd project rename other",
            // Filters that would read or write files.
            "fwd whoami; head ~/.ssh/id_rsa",
            "fwd whoami | grep -r token ~",
            "fwd whoami | grep -rn token",
            "fwd whoami | grep -f patterns.txt",
            "fwd whoami | grep token secrets.txt",
            "fwd task list | sort -o out.txt",
            "fwd task list | sort --output=out.txt",
            "fwd task list | sort --compress-program=sh",
            "fwd task list | uniq - out.txt",
            "fwd task list | jq -n env",
            "fwd task list | jq '$ENV'",
            "fwd task list | jq 'import \"x\" as y; .'",
            "fwd task list | jq --rawfile k /etc/passwd '.'",
            "fwd task list | jq -f program.jq",
            "fwd task list | jq . extra.json",
            "fwd task list | tail -f log.txt",
            "fwd task list | wc -l notes.md",
            "fwd task list | cut -f1 data.tsv",
            // A comment's quote hides nothing: the shell runs the line after it.
            "fwd whoami # it's\nrm -rf ~\n# '",
            // A no-break space is no blank to the shell: this names a program `fwd\u{a0}whoami`.
            "fwd\u{a0}whoami",
            // A line the shell would refuse.
            "fwd whoami | | head",
            "fwd whoami && && fwd inbox",
            "fwd whoami ;; fwd inbox",
            "fwd comment X 'open",
            "fwd comment X \"open",
            // No fwd at all: nothing to stamp.
            "cd /tmp",
            "head -3",
        ] {
            assert!(!stampable(command), "left alone: {command}");
        }
    }

    #[test]
    fn a_stamped_command_carries_the_command_it_was_given_after_the_stamp() {
        let command = "cd /tmp/work && fwd comment X 'a note' | head -1";
        let out: Value = serde_json::from_str(&stamp(&input(command, Some("a1"))).unwrap()).unwrap();
        let stamped = out["hookSpecificOutput"]["updatedInput"]["command"].as_str().unwrap();
        assert_eq!(stamped.split_once("; ").unwrap().1, command);
    }
}
