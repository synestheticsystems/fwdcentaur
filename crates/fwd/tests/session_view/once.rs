//! The view printed once, and the actions it names, run as the pane runs them.

use super::*;

/// The session's own thread names itself `lead-sam` in alpha and in beta, a subagent names itself
/// `impl-sam` in alpha, and another subagent names itself and retires there. In alpha, lead-sam
/// asks the person something, asks impl-sam something and asks no one in particular something,
/// works a task the person gave it, and is asked something by the person; impl-sam asks the person
/// something; in beta, lead-sam holds a task. The view lists the own thread's groups first, by
/// project, then the subagent at work's, each with what waits on the person, what it holds and what
/// waits on others, the unread notifications counted on the group, and the subagent's group open
/// since something in it waits on the person.
#[test]
fn the_view_lists_each_principal_of_the_session_with_what_waits_on_the_person_first() -> Result<()> {
    let s = Scratch::new()?;
    s.json(&s.alpha, Some("main"), &["principal", "add", "lead-sam", "--role", "lead"])?;
    s.json(&s.beta, Some("main"), &["principal", "add", "lead-sam", "--role", "lead"])?;
    s.json(&s.alpha, Some("a1"), &["principal", "add", "impl-sam", "--role", "implementer"])?;
    s.json(&s.alpha, Some("a2"), &["principal", "add", "impl-done", "--role", "implementer"])?;
    s.json(&s.alpha, Some("a2"), &["principal", "retire", "impl-done"])?;
    let what = "When the daemon restarts, a command in flight fails.\n\nRetry it once the daemon answers.";
    let task = s.made(&s.alpha, None, &["task", "create", "Retry on disconnect", "--what", what])?;
    s.json(&s.alpha, None, &["task", "assign", &task, "lead-sam"])?;
    s.json(&s.alpha, Some("main"), &["task", "start", &task])?;
    let held_in_beta = s.made(&s.beta, None, &["task", "create", "Cap the backoff"])?;
    s.json(&s.beta, None, &["task", "assign", &held_in_beta, "lead-sam"])?;
    let asked = "Should the primer name the page?\n\nIt costs every session about forty characters.";
    s.made(&s.alpha, Some("main"), &["ask", "--to", "tester", asked])?;
    s.made(&s.alpha, Some("main"), &["ask", "--to", "impl-sam", "Does the retry cover the page too?"])?;
    s.made(&s.alpha, Some("main"), &["ask", "Who reviews the retry?"])?;
    s.made(&s.alpha, Some("a1"), &["ask", "--to", "tester", "May I cap the backoff at two seconds?"])?;
    s.made(&s.alpha, None, &["ask", "--to", "lead-sam", "How long a wait?"])?;
    // The assignment and the person's question.
    s.told(&s.alpha, "main", 2)?;

    let view = s.view()?;
    assert_eq!(view["title"], "fwd: what this session holds");
    assert!(view["page"].as_str().is_some_and(|page| page.starts_with("http://127.0.0.1:")), "{view}");
    assert_eq!(view["lines"], json!([]));
    let groups = view["principals"].as_array().ok_or_else(|| anyhow!("no principals in {view}"))?;
    let named: Vec<(&Value, &Value, &Value)> =
        groups.iter().map(|g| (&g["title"], &g["tags"][1]["text"], &g["folded"])).collect();
    assert_eq!(
        named,
        [
            (&json!("lead-sam"), &json!("alpha"), &json!(false)),
            (&json!("lead-sam"), &json!("beta"), &json!(false)),
            (&json!("impl-sam"), &json!("alpha"), &json!(false)),
        ],
        "{view}"
    );
    let lead = &groups[0];
    assert_eq!(
        lead["tags"],
        json!([{"text": "lead", "tone": null, "at": null}, {"text": "alpha", "tone": null, "at": null}, {"text": "2 unread", "tone": null, "at": null}])
    );
    let sections: Vec<&str> =
        lead["sections"].as_array().into_iter().flatten().filter_map(|s| s["section"].as_str()).collect();
    assert_eq!(sections, ["waits_on_you", "holds", "waits_on_others"]);
    assert_eq!(titles(lead, "waits_on_you"), ["Should the primer name the page? It costs…"]);
    assert_eq!(titles(lead, "holds"), ["How long a wait?", "Retry on disconnect"]);
    assert_eq!(
        titles(lead, "waits_on_others"),
        ["Who reviews the retry?", "Does the retry cover the page too?"],
        "newest first"
    );
    let question = &list(lead, "waits_on_you").expect("waits on you")["records"][0];
    assert_eq!(question["lines"], json!([asked]));
    assert_eq!(question["tags"][0]["text"], "asked");
    assert_eq!(question["tags"][0]["tone"], "attention");
    let id = question["id"].as_str().unwrap_or_default();
    assert_eq!(question["path"], format!("?p={}#/questions/{id}", s.project_id(&s.alpha)?));
    let held = &list(lead, "holds").expect("holds")["records"];
    assert_eq!(
        (&held[0]["tags"][0]["text"], &held[1]["tags"][0]["text"]),
        (&json!("from tester"), &json!("in progress"))
    );
    assert_eq!(held[1]["lines"], json!([what]));
    let others = &list(lead, "waits_on_others").expect("waits on others");
    assert_eq!(others["folded"], true);
    let to: Vec<&Value> = others["records"].as_array().into_iter().flatten().map(|r| &r["tags"][0]["text"]).collect();
    assert_eq!(to, [&json!("to no one in particular"), &json!("to impl-sam")]);
    assert_eq!(titles(&groups[1], "holds"), ["Cap the backoff"]);
    assert_eq!(titles(&groups[2], "waits_on_you"), ["May I cap the backoff at two seconds?"]);
    let short = &list(&groups[2], "waits_on_you").expect("waits on you")["records"][0];
    assert_eq!(short["lines"], json!([]), "a title that holds the whole text is not said again");

    // `--as` naming the person by their principal's id reads as their name does; naming an agent,
    // it says so and nothing waits on anyone.
    let tester = s.person_id(&s.alpha)?;
    assert_eq!(s.view_with(&["--as", &tester])?["principals"], view["principals"]);
    let agent = s.view_with(&["--as", "impl-sam"])?;
    assert_eq!(
        agent["lines"],
        json!(["impl-sam names an agent, and the view is a person's, so nothing here waits on you and nothing can be answered."])
    );
    assert_eq!(list(&agent["principals"][0], "waits_on_you"), None);
    Ok(())
}

/// Answer replies to the question in the person's words, exactly as the pane writes them on
/// standard input, a leading hyphen and several lines included, run from any directory with no
/// session in its environment, and prints nothing; the question then leaves what waits on the
/// person. A press that fails says why in one line on standard error and exits non-zero, and
/// prints nothing on standard output.
#[test]
fn answer_replies_in_the_persons_words_and_a_failed_press_says_why_in_one_line() -> Result<()> {
    let s = Scratch::new()?;
    s.json(&s.alpha, Some("main"), &["principal", "add", "lead-sam", "--role", "lead"])?;
    let question = s.made(&s.alpha, Some("main"), &["ask", "--to", "tester", "Should the primer name the page?"])?;
    let view = s.view()?;
    let row = &list(&view["principals"][0], "waits_on_you").expect("waits on you")["records"][0];
    let answer = &row["actions"][0];
    assert_eq!(answer["label"], "Answer");
    assert_eq!(answer["input"], json!({"prompt": "Your answer", "multiline": true}));
    let args: Vec<String> = serde_json::from_value(answer["args"].clone())?;
    assert_eq!(
        args,
        [
            "--project",
            &s.project_id(&s.alpha)?,
            "--as",
            &s.person_id(&s.alpha)?,
            "--format",
            "pane",
            "reply",
            &question,
            "-"
        ]
    );

    let words = "-- no: the pane links there.\n\nLeave the primer as it is.\n";
    let pressed = s.press(&args, words)?;
    assert_eq!(
        (pressed.status.code(), pressed.stdout.as_slice(), pressed.stderr.as_slice()),
        (Some(0), &b""[..], &b""[..]),
        "{}",
        String::from_utf8_lossy(&pressed.stderr)
    );
    let thread = s.json(&s.alpha, None, &["message", "list"])?;
    let replies: Vec<&Value> =
        thread.as_array().into_iter().flatten().filter(|m| m["reply_to"] == json!(question)).collect();
    assert_eq!(replies.len(), 1, "{thread}");
    assert_eq!(replies[0]["content"], words);
    assert_eq!(list(&s.view()?["principals"][0], "waits_on_you"), None, "answered, it waits on no one");

    let mut gone = args.clone();
    gone[7] = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into();
    let failed = s.press(&gone, "too late")?;
    let said = String::from_utf8(failed.stderr)?;
    assert!(!failed.status.success() && failed.stdout.is_empty(), "{said}");
    assert_eq!(said.lines().count(), 1, "{said}");
    assert!(said.ends_with('\n') && !said.starts_with("Error"), "{said}");
    Ok(())
}

/// `fwd reply` given `-` takes its text from standard input whole, even when it holds nothing;
/// `--format pane` says nothing of a reply that ran in another project than the one named, and is
/// for the commands the view's actions run alone.
#[test]
fn a_reply_takes_its_text_from_standard_input_whole_and_pane_is_for_actions_alone() -> Result<()> {
    let s = Scratch::new()?;
    let question = s.made(&s.alpha, None, &["ask", "--to", "tester", "Is an empty answer kept?"])?;
    let args = ["--as", "tester", "--format", "json", "--project", "alpha", "reply", &question, "-"];
    let empty = s.press(&args.map(str::to_string), "")?;
    assert!(empty.status.success(), "{}", String::from_utf8_lossy(&empty.stderr));
    let made: Value = serde_json::from_slice(&empty.stdout)?;
    assert_eq!(made["record"]["content"], "");

    // A question of another project than the one the press names, as a question moved since the
    // view was printed is: the reply runs there, and with `--format pane` says nothing of it.
    let elsewhere = s.made(&s.beta, None, &["ask", "--to", "tester", "Was it moved?"])?;
    let args = ["--project", "alpha", "--as", "tester", "--format", "pane", "reply", &elsewhere, "-"];
    let moved = s.press(&args.map(str::to_string), "It was.")?;
    assert_eq!(
        (moved.status.code(), moved.stdout.as_slice(), moved.stderr.as_slice()),
        (Some(0), &b""[..], &b""[..]),
        "{}",
        String::from_utf8_lossy(&moved.stderr)
    );

    // Refused before anything runs, a hook's command and a deadline's included, in one line.
    let refused = [&["task", "list"][..], &["session", "stamp"], &["prime", "--timeout", "5"]];
    for command in refused {
        let mut run = s.command(&s.alpha, None, &[&["--format", "pane"][..], command].concat());
        let out = run.stdin(Stdio::null()).output()?;
        let said = String::from_utf8(out.stderr)?;
        assert_eq!(
            (out.status.code(), out.stdout.as_slice(), said.as_str()),
            (
                Some(1),
                &b""[..],
                "--format pane is for the commands a session view's actions run, `reply` alone; leave it off\n"
            ),
            "{command:?}"
        );
    }
    // A failure's one line holds no control character, whatever its error quotes.
    let args = ["--as", "tester", "--format", "pane", "--project", "alpha", "reply", "no\u{1b}[31mthing", "-"];
    let failed = s.press(&args.map(str::to_string), "words")?;
    let said = String::from_utf8(failed.stderr)?;
    assert!(!failed.status.success() && said.lines().count() == 1, "{said}");
    assert!(!said.trim_end().chars().any(char::is_control), "{said:?}");
    Ok(())
}

/// A session no principal is bound to says so in its one line, and the view is JSON alone: another
/// `--format` is refused, as is an id that is no Claude Code session's, and an identity file that is
/// not there or cannot be read.
#[test]
fn a_session_no_principal_is_bound_to_says_so() -> Result<()> {
    let s = Scratch::new()?;
    let view = s.view()?;
    assert_eq!(view["principals"], json!([]));
    assert_eq!(
        view["lines"],
        json!(["No fwd principal is bound to this session yet. Its agent names itself with `fwd principal add <name> --role <role>`."])
    );
    let text = s.command(s.dir.path(), None, &["--format", "text", "session", "view", SESSION]).output()?;
    assert!(!text.status.success() && String::from_utf8_lossy(&text.stderr).contains("prints JSON alone"));
    let wrong = s.command(s.dir.path(), None, &["session", "view", "not a session"]).output()?;
    assert!(
        !wrong.status.success() && String::from_utf8_lossy(&wrong.stderr).contains("is not a Claude Code session id")
    );

    // Without --as, the person is the one the identity file names: a file that is not there, or
    // that cannot be read, is an error naming it, never the login name in its place.
    let identity = s.dir.path().join("data").join("identity.toml");
    let kept = std::fs::read(&identity)?;
    std::fs::remove_file(&identity)?;
    let missing = s.command(s.dir.path(), None, &["session", "view", SESSION]).env("USER", "someone-else").output()?;
    let said = String::from_utf8_lossy(&missing.stderr);
    assert!(
        !missing.status.success() && said.contains(&identity.display().to_string()) && said.contains("does not exist"),
        "{said}"
    );
    std::fs::write(&identity, "handle = [\"not a name\"]")?;
    let garbled = s.command(s.dir.path(), None, &["session", "view", SESSION]).output()?;
    let said = String::from_utf8_lossy(&garbled.stderr);
    assert!(!garbled.status.success() && said.contains(&identity.display().to_string()), "{said}");
    std::fs::write(&identity, kept)?;
    assert_eq!(s.view()?["principals"], json!([]));
    Ok(())
}
