//! The decision log as markdown: what `fwd decision export` prints, in the style `docs/DECISIONS.md`
//! was written in by hand before decisions were records (an entry per ruling, its words quoted,
//! grouped by whether it is in force). The rulings that govern the whole project come first, then
//! each scope's under the scope's name, so that whoever works in one scope reads the project's
//! rulings and that scope's without the rest (docs/DECISIONS.md, D93). A ruling that was rejected,
//! superseded or repealed says why in the note of the comment its move wrote, each reference by
//! name, as `--format text` prints a message (task b6bAAF57, "Show a decision's closing note as
//! its comment, with names drawn, not the raw text"). Once the records hold every ruling, that
//! file is this rendering, so its first line says so. Days are shown in the reader's own
//! calendar: this machine's time zone, or the browser's in the page.

use crate::model::*;
use crate::ops;
use crate::rules::DecisionMove;
use ankurah::{Context, EntityId};
use anyhow::{bail, Result};
use std::collections::HashMap;

const DAY_MS: i64 = 86_400_000;

/// The calendar day of a time in milliseconds since the epoch, as `YYYY-MM-DD`, in this
/// machine's time zone (the browser's, in the page): a ruling made on a Pacific evening is dated
/// that evening, not the next UTC day.
pub fn date(ms: i64) -> String {
    date_at(ms, local_offset_ms(ms))
}

/// The calendar day of `ms` in a time zone `offset_ms` east of UTC.
pub fn date_at(ms: i64, offset_ms: i64) -> String {
    let (y, m, d) = civil_from_days((ms + offset_ms).div_euclid(DAY_MS));
    format!("{y:04}-{m:02}-{d:02}")
}

/// How far east of UTC this machine's clock is at `ms`, daylight saving included.
#[cfg(all(feature = "native", unix))]
fn local_offset_ms(ms: i64) -> i64 {
    let secs = ms.div_euclid(1000) as libc::time_t;
    // SAFETY: `localtime_r` writes only into the `tm` it is given, which is zeroed plain data.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let found = unsafe { !libc::localtime_r(&secs, &mut tm).is_null() };
    if found {
        // `c_long` is 64 bits wide on the targets fwd builds for; the cast keeps the rest honest.
        #[allow(clippy::unnecessary_cast)]
        let seconds_east = tm.tm_gmtoff as i64;
        seconds_east * 1000
    } else {
        0
    }
}

/// How far east of UTC the browser's clock is at `ms`.
#[cfg(target_arch = "wasm32")]
fn local_offset_ms(ms: i64) -> i64 {
    -(js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64)).get_timezone_offset() as i64) * 60_000
}

/// Elsewhere (the page's native tests), UTC.
#[cfg(not(any(all(feature = "native", unix), target_arch = "wasm32")))]
fn local_offset_ms(_ms: i64) -> i64 {
    0
}

/// A `YYYY-MM-DD` day, as noon UTC that day: noon falls on the same calendar day in every time
/// zone from UTC-11 to UTC+11, so the day stays the day it was given wherever it is shown.
pub fn parse_date(s: &str) -> Result<i64> {
    let parts: Vec<&str> = s.trim().split('-').collect();
    let parsed = match parts.as_slice() {
        [y, m, d] if y.len() == 4 && m.len() == 2 && d.len() == 2 => {
            (y.parse::<i64>().ok(), m.parse::<u32>().ok(), d.parse::<u32>().ok())
        }
        _ => (None, None, None),
    };
    let (Some(y), Some(m), Some(d)) = parsed else {
        bail!("`{s}` is not a day: write it as YYYY-MM-DD");
    };
    let days = days_from_civil(y, m, d);
    if !(1..=12).contains(&m) || civil_from_days(days) != (y, m, d) {
        bail!("`{s}` is not a day of the calendar");
    }
    Ok(days * DAY_MS + DAY_MS / 2)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day number since 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// The scope a ruling governs, as the log names it and groups its rulings under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeName {
    pub id: EntityId,
    /// Its name, or the start of its id when the scope cannot be read.
    pub name: String,
}

/// One decision as the log shows it, its people named by handle and its links by number.
#[derive(Debug, Clone, Default)]
pub struct Entry {
    pub number: i64,
    pub title: String,
    pub status: Option<DecisionStatus>,
    pub area: Option<String>,
    /// The scope it governs; none for a ruling that governs the whole project.
    pub scope: Option<ScopeName>,
    pub context: String,
    pub decision: String,
    pub consequences: String,
    /// Who filed it.
    pub author: String,
    /// Who ruled on it.
    pub decided_by: Option<String>,
    /// When it was ruled on, else when it was filed.
    pub date: i64,
    pub closed_by: Option<String>,
    pub closed: Option<i64>,
    /// Why it was rejected, superseded or repealed, each reference by name: the note its closing
    /// move gave in its comment on it (`ops::closing_note`), else, for a decision closed before its
    /// moves wrote comments, its `closed_reason`.
    pub closing_note: Option<String>,
    /// The numbers of the decisions it supersedes, and of those that superseded it.
    pub supersedes: Vec<i64>,
    pub superseded_by: Vec<i64>,
    /// The numbers of the decisions it cites.
    pub cites: Vec<i64>,
    /// The tasks and messages it was decided in the context of, each named in a few words.
    pub decided_in: Vec<String>,
}

/// The first line of every export: the file is a rendering, and where its content lives.
pub const RENDERED: &str = "<!-- Rendered by `fwd decision export` from the project's decision records; an edit made here is lost at the next export. Record a ruling with `fwd decision create`, and move one with `fwd decision accept|reject|supersede|repeal`. -->";

/// The whole log: the rulings that govern the whole project, then those attached to each scope,
/// under the scope's name, the scopes by name. In each group, rulings in force, then proposals,
/// then what was rejected, superseded or repealed, each by number, and a status with no ruling
/// left out.
pub fn render(entries: &[Entry]) -> String {
    let mut out = format!("{RENDERED}\n\n# Decisions\n\n");
    out.push_str("Every ruling the project has recorded, cited by its number (`D12`). A ruling attached to a scope governs that scope's work, and one attached to none governs the whole project: the project-wide rulings come first, then each scope's under its name. Accepted rulings are in force; proposed ones wait for a ruling; rejected, superseded and repealed ones stay, so that what cited them can be found.\n");
    let project_wide: Vec<&Entry> = entries.iter().filter(|e| e.scope.is_none()).collect();
    out.push_str("\n## Project-wide\n");
    out.push_str(&group(&project_wide));
    let mut scopes: Vec<&ScopeName> = entries.iter().filter_map(|e| e.scope.as_ref()).collect();
    scopes.sort_by_key(|s| (s.name.to_lowercase(), s.id));
    scopes.dedup_by_key(|s| s.id);
    for scope in scopes {
        let governed: Vec<&Entry> =
            entries.iter().filter(|e| e.scope.as_ref().map(|s| s.id) == Some(scope.id)).collect();
        out.push_str(&format!("\n## {}\n", scope.name.trim()));
        out.push_str(&group(&governed));
    }
    out
}

/// One group's rulings under a heading for each status they have, by number: in force, proposed,
/// then rejected, superseded or repealed, and last any of a status this build does not know.
/// "None." when the group has no ruling at all.
fn group(entries: &[&Entry]) -> String {
    use DecisionStatus::*;
    let sections: [(&str, &[DecisionStatus]); 3] = [
        ("Accepted", &[Accepted]),
        ("Proposed", &[Proposed]),
        ("Rejected, superseded and repealed", &[Rejected, Superseded, Repealed]),
    ];
    let mut out = String::new();
    for (heading, statuses) in sections {
        let mut listed: Vec<&Entry> =
            entries.iter().copied().filter(|e| e.status.is_some_and(|s| statuses.contains(&s))).collect();
        if listed.is_empty() {
            continue;
        }
        listed.sort_by_key(|e| e.number);
        out.push_str(&format!("\n### {heading}\n\n"));
        for e in listed {
            out.push_str(&entry(e));
        }
    }
    let mut unknown: Vec<&Entry> = entries.iter().copied().filter(|e| e.status.is_none()).collect();
    if !unknown.is_empty() {
        unknown.sort_by_key(|e| e.number);
        out.push_str("\n### Of a status this build does not know\n\n");
        for e in unknown {
            out.push_str(&entry(e));
        }
    }
    if out.is_empty() {
        out.push_str("\nNone.\n");
    }
    out
}

/// Continuation lines of a list item, indented to stay inside it.
fn indented(text: &str, pad: &str) -> String {
    text.trim().lines().collect::<Vec<_>>().join(&format!("\n{pad}"))
}

fn numbers(ns: &[i64]) -> String {
    ns.iter().map(|n| format!("D{n}")).collect::<Vec<_>>().join(", ")
}

/// `- **D12: Title** (area, governs a scope, day, decided by someone): the ruling`, then what
/// explains it; a ruling that governs the whole project names no scope.
fn entry(e: &Entry) -> String {
    let status = e.status;
    let mut about: Vec<String> = e.area.iter().cloned().collect();
    if let Some(scope) = &e.scope {
        about.push(format!("governs {}", scope.name.trim()));
    }
    about.push(date(e.date));
    match (status, &e.decided_by) {
        (Some(DecisionStatus::Proposed), _) | (_, None) => about.push(format!("proposed by {}", e.author)),
        (Some(DecisionStatus::Rejected), Some(who)) => about.push(format!("rejected by {who}")),
        (_, Some(who)) => {
            about.push(format!("decided by {who}"));
            if *who != e.author {
                about.push(format!("recorded by {}", e.author));
            }
        }
    }
    let mut out = format!("- **D{}: {}** ({})", e.number, e.title.trim(), about.join(", "));
    if !e.decision.trim().is_empty() {
        out.push_str(&format!(": {}", indented(&e.decision, "  ")));
    }
    out.push('\n');
    let mut notes: Vec<String> = Vec::new();
    if !e.context.trim().is_empty() {
        notes.push(format!("Context: {}", indented(&e.context, "    ")));
    }
    if !e.consequences.trim().is_empty() {
        notes.push(format!("Consequences: {}", indented(&e.consequences, "    ")));
    }
    if !e.supersedes.is_empty() {
        notes.push(format!("Supersedes {}.", numbers(&e.supersedes)));
    }
    if !e.cites.is_empty() {
        notes.push(format!("Cites {}.", numbers(&e.cites)));
    }
    if !e.decided_in.is_empty() {
        notes.push(format!("Decided in {}.", e.decided_in.join("; ")));
    }
    let why = e.closing_note.as_deref().map(|r| format!(": {}", indented(r, "    "))).unwrap_or_default();
    let on = e.closed.map(|t| format!(" on {}", date(t))).unwrap_or_default();
    match status {
        Some(DecisionStatus::Superseded) => {
            let by =
                if e.superseded_by.is_empty() { String::new() } else { format!(" by {}", numbers(&e.superseded_by)) };
            let who = e.closed_by.as_deref().map(|w| format!(" ({w})")).unwrap_or_default();
            notes.push(format!("Superseded{by}{on}{who}{why}"));
        }
        Some(DecisionStatus::Repealed) => {
            let who = e.closed_by.as_deref().map(|w| format!(" by {w}")).unwrap_or_default();
            notes.push(format!("Repealed{on}{who}{why}"));
        }
        Some(DecisionStatus::Rejected) if !why.is_empty() => notes.push(format!("Rejected{why}")),
        _ => {}
    }
    for note in notes {
        out.push_str(&format!("  - {note}\n"));
    }
    out
}

/// So that whoever reads why a ruling closed, in the exported log or in `fwd decision show`'s text,
/// reads the names its note named, not the ids typed (task b6bAAF57, "Show a decision's closing
/// note as its comment, with names drawn, not the raw text"): why the decision `d` closed, the note
/// its closing move gave in its comment on it (`ops::closing_note`), else, for a decision closed
/// before its moves wrote comments, its `closed_reason`, each reference by name as `--format text`
/// prints a message (`ops::render_references`: a person as `@handle`, anything else as its name in
/// brackets). None for a decision still open.
pub async fn why_closed(ctx: &Context, d: &DecisionView) -> Result<Option<String>> {
    let status = d.status().ok();
    // An open decision has no comment of a closing move to look for.
    let comments = match status.and_then(DecisionMove::closing) {
        Some(_) => ops::messages_about(ctx, "decision", d.id())
            .await?
            .iter()
            .filter_map(|m| ops::DecisionComment::of(m).ok())
            .collect(),
        None => Vec::new(),
    };
    let note =
        status.and_then(|s| ops::closing_note(s, &comments)).map(str::to_string).or(d.closed_reason().ok().flatten());
    Ok(match note {
        Some(note) => Some(ops::render_references(ctx, &note).await),
        None => None,
    })
}

/// Every decision of the project as the log shows it.
pub async fn entries(ctx: &Context) -> Result<Vec<Entry>> {
    let decisions = ops::decisions(ctx).await?;
    let number: HashMap<EntityId, i64> = decisions.iter().filter_map(|d| Some((d.id(), d.number().ok()?))).collect();
    let principals: Vec<PrincipalView> = ctx.fetch("kind = 'human' OR kind = 'agent'").await?;
    let handle = |id: EntityId| {
        principals
            .iter()
            .find(|p| p.id() == id)
            .and_then(|p| p.handle().ok())
            .unwrap_or_else(|| id.to_base64()[..8].to_string())
    };
    let scopes: Vec<ScopeView> = ctx.fetch("created > 0").await?;
    let scope_name = |id: EntityId| ScopeName {
        id,
        name: scopes
            .iter()
            .find(|s| s.id() == id)
            .and_then(|s| s.title().ok())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| id.to_base64()[..8].to_string()),
    };
    let cites: Vec<RelationView> = ctx.fetch("kind = 'cites'").await?;
    let supersedes: Vec<RelationView> = ctx.fetch("kind = 'supersedes'").await?;
    let standing = |rels: &[RelationView]| -> Vec<(EntityId, EntityId)> {
        rels.iter().filter(|r| ops::standing(r)).filter_map(|r| Some((r.from_id().ok()?, r.to_id().ok()?))).collect()
    };
    let (cites, supersedes) = (standing(&cites), standing(&supersedes));
    let mut out = Vec::new();
    for d in &decisions {
        let id = d.id();
        let mut decided_in = Vec::new();
        for (_, to) in cites.iter().filter(|(from, to)| *from == id && !number.contains_key(to)) {
            if let Ok(t) = ctx.get::<TaskView>(*to).await {
                decided_in.push(format!("task “{}”", t.title().unwrap_or_default()));
            } else if let Ok(m) = ctx.get::<MessageView>(*to).await {
                let content = ops::text(m.content()).unwrap_or_default();
                let words: String = content.split_whitespace().collect::<Vec<_>>().join(" ");
                let short: String = words.chars().take(80).collect();
                let more = if words.chars().count() > 80 { "…" } else { "" };
                decided_in.push(format!(
                    "the message “{short}{more}” by {}",
                    m.author().map(|a| handle(a.into())).unwrap_or_default()
                ));
            }
        }
        let numbered = |pairs: Vec<EntityId>| -> Vec<i64> {
            let mut ns: Vec<i64> = pairs.iter().filter_map(|x| number.get(x).copied()).collect();
            ns.sort();
            ns
        };
        let closing_note = why_closed(ctx, d).await?;
        out.push(Entry {
            number: d.number().unwrap_or(0),
            title: d.title().unwrap_or_default(),
            status: d.status().ok(),
            area: d.area().ok().flatten(),
            scope: ops::governs(d).ok().flatten().map(scope_name),
            context: ops::text(d.context()).unwrap_or_default(),
            decision: ops::text(d.decision()).unwrap_or_default(),
            consequences: ops::text(d.consequences()).unwrap_or_default(),
            author: d.author().map(|a| handle(a.into())).unwrap_or_default(),
            decided_by: d.decided_by().ok().flatten().map(|p| handle(p.into())),
            date: d.decided().ok().flatten().unwrap_or(d.created().unwrap_or(0)),
            closed_by: d.closed_by().ok().flatten().map(|p| handle(p.into())),
            closed: d.closed().ok().flatten(),
            closing_note,
            supersedes: numbered(supersedes.iter().filter(|(f, _)| *f == id).map(|(_, t)| *t).collect()),
            superseded_by: numbered(supersedes.iter().filter(|(_, t)| *t == id).map(|(f, _)| *f).collect()),
            cites: numbered(cites.iter().filter(|(f, _)| *f == id).map(|(_, t)| *t).collect()),
            decided_in,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_round_trip_and_land_on_their_own_date() {
        const HOUR: i64 = 3_600_000;
        assert_eq!(date_at(0, 0), "1970-01-01");
        assert_eq!(date_at(-1, 0), "1969-12-31");
        // 01:00 UTC on the 27th is still the evening of the 26th in California (UTC-7).
        let late = parse_date("2026-09-27").unwrap() - 11 * HOUR;
        assert_eq!((date_at(late, 0), date_at(late, -7 * HOUR)), ("2026-09-27".into(), "2026-09-26".into()));
        for day in ["2026-09-24", "2000-02-29", "1999-12-31", "2024-03-01", "1970-01-01"] {
            let ms = parse_date(day).unwrap();
            // A given day is stored at noon UTC, which is that day from UTC-11 to UTC+11.
            for offset in [-11 * HOUR, -7 * HOUR, 0, 9 * HOUR, 11 * HOUR] {
                assert_eq!(date_at(ms, offset), day, "{day} at {offset}");
            }
            assert_eq!(date(ms), day, "{day} on this machine");
        }
        for not_a_day in ["2026-02-30", "2026-13-01", "2026-9-24", "24-09-2026", "yesterday", "2026-00-10"] {
            assert!(parse_date(not_a_day).is_err(), "{not_a_day}");
        }
    }

    fn ruling(number: i64, status: DecisionStatus) -> Entry {
        Entry {
            number,
            title: format!("Ruling {number}"),
            status: Some(status),
            author: "claude".into(),
            decided_by: (status != DecisionStatus::Proposed).then(|| "dnorman".into()),
            date: parse_date("2026-09-24").unwrap(),
            decision: "the words".into(),
            ..Entry::default()
        }
    }

    #[test]
    fn the_log_says_it_is_rendered_and_groups_rulings_by_whether_they_are_in_force() {
        use DecisionStatus::*;
        let mut in_force = ruling(2, Accepted);
        in_force.area = Some("architecture".into());
        in_force.context = "answer to Q1".into();
        in_force.supersedes = vec![1];
        in_force.cites = vec![3];
        in_force.decided_in = vec!["task “Retry on disconnect”".into()];
        let mut old = ruling(1, Superseded);
        old.superseded_by = vec![2];
        old.closed_by = Some("dnorman".into());
        old.closed = Some(parse_date("2026-09-25").unwrap());
        old.closing_note = Some("jitter avoids stampedes".into());
        let mut gone = ruling(4, Repealed);
        gone.closed_by = Some("dnorman".into());
        gone.closing_note = Some("no longer true".into());
        let mut open = ruling(3, Proposed);
        open.decision = String::new();
        open.context = "Should the retry cap be configurable?".into();
        let log = render(&[old, gone, open, in_force]);
        assert!(log.starts_with(RENDERED), "{log}");
        let project_wide = log.find("\n## Project-wide\n").unwrap();
        let accepted = log.find("\n### Accepted\n").unwrap();
        let proposed = log.find("\n### Proposed\n").unwrap();
        let closed = log.find("\n### Rejected, superseded and repealed\n").unwrap();
        assert!(project_wide < accepted && accepted < proposed && proposed < closed, "{log}");
        let d2 = log
            .find("- **D2: Ruling 2** (architecture, 2026-09-24, decided by dnorman, recorded by claude): the words\n")
            .unwrap();
        assert!(accepted < d2 && d2 < proposed, "{log}");
        assert!(log.contains("  - Context: answer to Q1\n  - Supersedes D1.\n  - Cites D3.\n  - Decided in task “Retry on disconnect”.\n"), "{log}");
        let d3 = log.find("- **D3: Ruling 3** (2026-09-24, proposed by claude)\n  - Context: Should the retry cap be configurable?\n").unwrap();
        assert!(proposed < d3 && d3 < closed, "{log}");
        let d1 = log.find("- **D1: Ruling 1**").unwrap();
        let d4 = log.find("- **D4: Ruling 4**").unwrap();
        assert!(closed < d1 && d1 < d4, "by number within a section: {log}");
        assert!(log.contains("  - Superseded by D2 on 2026-09-25 (dnorman): jitter avoids stampedes\n"), "{log}");
        assert!(log.contains("  - Repealed by dnorman: no longer true\n"), "{log}");
    }

    fn scope(n: u8, name: &str) -> ScopeName {
        ScopeName { id: EntityId::from_bytes([n; 32]), name: name.into() }
    }

    #[test]
    fn the_projects_rulings_come_first_then_each_scopes_under_its_name_keeping_their_numbers() {
        use DecisionStatus::*;
        let (hosting, core) = (scope(1, "Cloud Hosting"), scope(2, "Open Core"));
        let attached = |number, status, to: &ScopeName| Entry { scope: Some(to.clone()), ..ruling(number, status) };
        let mut billing = attached(3, Accepted, &hosting);
        billing.area = Some("product".into());
        let log = render(&[
            attached(5, Proposed, &core),
            billing,
            ruling(4, Accepted),
            attached(1, Accepted, &core),
            ruling(2, Repealed),
            attached(6, Accepted, &hosting),
        ]);
        let at = |needle: &str| log.find(needle).unwrap_or_else(|| panic!("{needle:?} is not in {log}"));
        // The project's rulings, then the scopes by name, each under its own heading.
        let (project_wide, hosted, cored) =
            (at("\n## Project-wide\n"), at("\n## Cloud Hosting\n"), at("\n## Open Core\n"));
        assert!(project_wide < hosted && hosted < cored, "{log}");
        // A ruling keeps its number wherever it is listed, and within a group the numbers run in order.
        let (d2, d4) = (at("- **D2: Ruling 2**"), at("- **D4: Ruling 4**"));
        assert!(project_wide < d4 && d4 < d2 && d2 < hosted, "in force first, then what fell: {log}");
        let (d3, d6) = (at("- **D3: Ruling 3**"), at("- **D6: Ruling 6**"));
        assert!(hosted < d3 && d3 < d6 && d6 < cored, "{log}");
        let (d1, d5) = (at("- **D1: Ruling 1**"), at("- **D5: Ruling 5**"));
        assert!(cored < d1 && d1 < d5, "{log}");
        // Each entry names the scope it governs after its area; a project-wide one names none.
        assert!(log.contains("- **D3: Ruling 3** (product, governs Cloud Hosting, 2026-09-24, decided by dnorman, recorded by claude): the words\n"), "{log}");
        assert!(
            log.contains("- **D4: Ruling 4** (2026-09-24, decided by dnorman, recorded by claude): the words\n"),
            "{log}"
        );
        // A group lists the statuses its rulings have and no others.
        let hosting_part = &log[hosted..cored];
        assert!(hosting_part.contains("### Accepted") && !hosting_part.contains("### Proposed"), "{log}");
        assert!(log[cored..].contains("### Proposed"), "{log}");
    }

    #[test]
    fn a_log_whose_rulings_all_govern_scopes_says_the_project_has_none_of_its_own() {
        let attached = Entry { scope: Some(scope(1, "Cloud Hosting")), ..ruling(1, DecisionStatus::Accepted) };
        let log = render(&[attached]);
        assert!(log.contains("\n## Project-wide\n\nNone.\n\n## Cloud Hosting\n"), "{log}");
        assert!(render(&[]).ends_with("\n## Project-wide\n\nNone.\n"), "a project with no rulings at all");
    }

    #[test]
    fn a_ruling_over_several_lines_stays_inside_its_entry() {
        let mut long = ruling(5, DecisionStatus::Accepted);
        long.decision = "first line\nsecond line".into();
        assert!(render(&[long]).contains(": first line\n  second line\n"));
    }
}
