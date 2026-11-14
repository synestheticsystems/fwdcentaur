//! Keeping the view's document well under 100,000 characters, as the pane that reads it line by
//! line expects: past a budget, rows are left out from the least important end, and each list that
//! lost rows says so, with the page's own list of them where there is one.

use super::{Group, ListKind};

/// The most bytes the principals of a document take, rows and all; the rest of a document is a
/// few hundred. Bytes are never fewer than characters, so it bounds those too.
pub(super) const BUDGET: usize = 60_000;

/// Leaves rows out of `groups` until they take [`BUDGET`] bytes or fewer: from the end of the least
/// important list with any left, waiting on others before holds before waiting on you, the last
/// group's before the first's. Each list that lost rows says how many it shows of how many, and
/// gives `more(group, list)`, the page's own list of them, where there is one.
pub(super) fn within_budget(groups: &mut [Group], more: impl Fn(usize, ListKind) -> Option<String>) {
    let mut total = json_len(&groups);
    if total <= BUDGET {
        return;
    }
    let order: Vec<(usize, usize)> = [ListKind::WaitsOnOthers, ListKind::Holds, ListKind::WaitsOnYou]
        .into_iter()
        .flat_map(|kind| {
            groups
                .iter()
                .enumerate()
                .rev()
                .filter_map(move |(g, group)| group.sections.iter().position(|s| s.section == kind).map(|s| (g, s)))
                .collect::<Vec<_>>()
        })
        .collect();
    for (g, s) in order {
        let list = &mut groups[g].sections[s];
        let more = more(g, list.section);
        while total > BUDGET {
            let Some(row) = list.records.pop() else { break };
            // The row and the comma before it.
            total -= json_len(&row) + usize::from(!list.records.is_empty());
            // A list's first cut makes it say so, which takes no more than its line with every
            // record shown, and the comma before it, and its `more` in place of null.
            if list.records.len() + 1 == list.of {
                total += json_len(&cut_line(list.of, list.of, &more)) + 1;
                total += more.as_ref().map_or(0, |more| json_len(more) - json_len(&()));
            }
        }
        list.shown = list.records.len();
        if list.shown < list.of {
            list.lines.push(cut_line(list.shown, list.of, &more));
            list.more = more;
        }
    }
}

/// What a list that shows `shown` of its `of` rows says, given the page's list of them, `more`.
fn cut_line(shown: usize, of: usize, more: &Option<String>) -> String {
    let rest = match more {
        Some(_) => "the page lists them all",
        None => "the rest are left out to keep this view short",
    };
    format!("{shown} of {of} shown; {rest}.")
}

/// How many bytes `value` takes as the document writes it.
fn json_len(value: &impl serde::Serialize) -> usize {
    serde_json::to_string(value).expect("the view's document always serializes: its keys are strings").len()
}

#[cfg(test)]
mod tests {
    use super::super::{List, Row, RowKind};
    use super::*;

    fn row(n: usize, words: usize) -> Row {
        Row {
            id: format!("r{n}"),
            kind: RowKind::Task,
            title: format!("Task {n}"),
            tags: Vec::new(),
            lines: vec!["word ".repeat(words)],
            path: None,
            actions: Vec::new(),
        }
    }

    fn group(lists: &[(ListKind, usize)]) -> Group {
        Group {
            id: "g".into(),
            title: "impl-sam".into(),
            tags: Vec::new(),
            folded: false,
            lines: Vec::new(),
            sections: lists
                .iter()
                .map(|&(kind, rows)| List {
                    section: kind,
                    title: String::new(),
                    folded: false,
                    lines: Vec::new(),
                    shown: rows,
                    of: rows,
                    more: None,
                    records: (0..rows).map(|n| row(n, 250)).collect(),
                })
                .collect(),
        }
    }

    /// Past the budget the least important rows go first, the last group's before the first's, and
    /// only as many as the budget needs; each list that lost rows says so, with the page's list of
    /// them where there is one, and one that lost none says nothing.
    #[test]
    fn past_the_budget_the_least_important_rows_go_first_and_each_cut_list_says_so() {
        let lists = [(ListKind::WaitsOnYou, 20), (ListKind::Holds, 20), (ListKind::WaitsOnOthers, 20)];
        let mut groups = vec![group(&lists), group(&lists)];
        let more = |g: usize, kind: ListKind| (kind == ListKind::WaitsOnYou).then(|| format!("#/questions?g={g}"));
        within_budget(&mut groups, more);
        assert!(serde_json::to_string(&groups).unwrap().len() <= BUDGET);
        let shown = |g: usize| groups[g].sections.iter().map(|s| s.shown).collect::<Vec<_>>();
        // Every row of waiting on others and of the second group's holds goes before any of the
        // first group's holds, and nothing waiting on the person goes while anything else is left.
        assert_eq!(shown(1)[2], 0);
        assert_eq!(shown(0)[2], 0);
        assert_eq!(shown(1)[1], 0);
        assert!(shown(0)[1] > 0 && shown(0)[1] < 20, "{:?}", shown(0));
        assert_eq!((shown(0)[0], shown(1)[0]), (20, 20));
        let holds = &groups[0].sections[1];
        assert_eq!(
            holds.lines,
            [format!("{} of 20 shown; the rest are left out to keep this view short.", holds.shown)]
        );
        assert_eq!(holds.more, None);
        assert!(groups[0].sections[0].lines.is_empty() && groups[0].sections[0].more.is_none());

        // Waiting on you, cut, gives the page's list of it.
        let mut many = vec![group(&[(ListKind::WaitsOnYou, 80)])];
        within_budget(&mut many, more);
        let waits = &many[0].sections[0];
        assert!(waits.shown < 80);
        assert_eq!(waits.more.as_deref(), Some("#/questions?g=0"));
        assert_eq!(waits.lines, [format!("{} of 80 shown; the page lists them all.", waits.shown)]);
    }

    /// Within the budget nothing is left out, and nothing is said.
    #[test]
    fn within_the_budget_nothing_is_left_out() {
        let mut groups = vec![group(&[(ListKind::Holds, 3)])];
        let before = groups.clone();
        within_budget(&mut groups, |_, _| Some("#/x".into()));
        assert_eq!(groups, before);
    }
}
