//! The tests of what a card says and which tasks are drawn, which build their tasks and edges with
//! the Graph page's test helpers.
use super::*;
use crate::graph::tests::{id, part_of, waits};

#[test]
fn wallboard_priority_has_its_own_lozenge_without_replacing_actual_metadata() {
    let (markup, rows) =
        wallboard_footer(&["Example scope".into()], "open · teammate", Some(1), 440.0, &Availability::default());
    assert_eq!(rows, 1);
    assert!(markup.contains("wallboard-priority p1"));
    assert!(markup.contains(">P1</text>"));
    assert!(markup.contains("open · teammate"));
    assert!(markup.contains("Example scope"));
}

#[test]
fn wallboard_cards_grow_with_footer_rows_and_a_parent_line_and_scale_with_text() {
    let mut presentation = Presentation { node_width: 360, ..Default::default() };
    let names: Vec<String> =
        ["Design & planning", "Release preparation", "A third longer scope"].map(Into::into).into();
    let (_, rows) = wallboard_footer(&names, "open · teammate", Some(1), 360.0, &Availability::default());
    assert_eq!(rows, 3, "the chips and metadata wrap onto three footer rows");
    assert_eq!(wallboard_card_size(&presentation, 0, false), (360.0, 149.0), "an empty footer keeps the base card");
    assert_eq!(wallboard_card_size(&presentation, 1, false), (360.0, 149.0));
    assert_eq!(wallboard_card_size(&presentation, rows, false), (360.0, 149.0 + 2.0 * 24.0));
    assert_eq!(wallboard_card_size(&presentation, rows, true), (360.0, 149.0 + 2.0 * 24.0 + 20.0));
    presentation.text_scale_percent = 125;
    assert_eq!(wallboard_card_size(&presentation, rows, true), (450.0, (149.0 + 2.0 * 24.0 + 20.0) * 1.25));
}

#[test]
fn wallboard_scope_chips_wrap_onto_rows_and_are_escaped() {
    let (chips, rows) = wallboard_footer(
        &["Design & planning".into(), "<scope>".into(), "A third longer scope".into()],
        "",
        None,
        240.0,
        &Availability::default(),
    );
    assert!(rows >= 2);
    assert!(!chips.contains("<scope>"));
    assert!(chips.contains("&lt;scope&gt;"));
    assert!(chips.contains("Design &amp; planning"));
}

/// A task the selection leaves out brings nothing onto the frame: no card, arrow or line for an open
/// parent, which the Graph page would draw as a band, nor for a done prerequisite, which it would
/// draw as a stub. The tasks of every status, and the relations to both, go through the wallboard's
/// own selection and reading of cards, and what comes out through its packing and markup.
#[test]
fn wallboard_draws_only_the_selected_statuses_and_nothing_of_a_task_it_leaves_out() {
    let selection = crate::wallboard::Settings::default().selection;
    let statuses = [
        TaskStatus::Proposed,
        TaskStatus::Open,
        TaskStatus::InProgress,
        TaskStatus::InReview,
        TaskStatus::Done,
        TaskStatus::Cancelled,
        TaskStatus::Rejected,
    ];
    assert!(!wallboard_draws(&selection, None, None), "nor a task whose status cannot be read");
    let drawn: Vec<_> = statuses
        .into_iter()
        .enumerate()
        .filter(|&(_, status)| wallboard_draws(&selection, Some(status), None))
        .map(|(n, status)| DrawnTask {
            id: id(n as u8),
            title: format!("{status:?}"),
            what: String::new(),
            status,
            priority: None,
            footer: String::new(),
            footer_rows: 1,
            waiting: false,
            revision: 0,
        })
        .collect();
    // The task in progress is part of the open one and waits for the done one.
    let cards = wallboard_cards(drawn, &[part_of(2, 1), waits(2, 4)], &Presentation::default());
    assert_eq!(cards.cards.iter().map(|card| card.id).collect::<Vec<_>>(), [id(2)], "the task in progress alone");
    assert_eq!(cards.cards[0].parent, "", "no line names the parent left out");
    assert!(cards.arrows.is_empty(), "no arrow comes from the prerequisite left out");
    let drawing = super::super::Placing::of(&cards).pack();
    assert_eq!(drawing.boxes.len(), 1, "the packing places the one card kept");
    assert!(drawing.routes.is_empty() && drawing.bands.is_empty(), "and no arrow or band");
    let frame = super::super::markup::frame(&cards, &drawing, &HashMap::new());
    assert_eq!(frame.matches("wallboard-node").count(), 1);
    assert!(!frame.contains(r#"class="edge"#), "the frame draws no arrow");
    assert!(!frame.contains("wallboard-parent"), "nor a parent line");
    for left_out in [id(1), id(4)] {
        assert!(!frame.contains(&left_out.to_base64()), "nothing on the frame names a task left out");
    }
}

#[test]
fn wallboard_relation_facts_state_parents_and_both_directions_of_dependency_in_one_order() {
    use WallboardOtherEnd::{Dependent, Parent, Prerequisite};
    let edges = [
        part_of(2, 4),
        waits(2, 6),
        waits(5, 2),
        Edge { proposed: true, ..waits(2, 7) },
        Edge { proposed: true, ..waits(8, 2) },
        Edge { proposed: true, ..part_of(2, 5) },
        part_of(2, 1),
        // Neither end is this card; and a card having a subtask is not one of its facts.
        waits(3, 9),
        part_of(3, 2),
    ];
    let expected = [
        (Parent, id(1), false),
        (Parent, id(4), false),
        (Parent, id(5), true),
        (Prerequisite, id(6), false),
        (Prerequisite, id(7), true),
        (Dependent, id(5), false),
        (Dependent, id(8), true),
    ];
    assert_eq!(wallboard_relation_facts(id(2), edges.iter().copied()), expected);
    assert_eq!(
        wallboard_relation_facts(id(2), edges.iter().rev().copied()),
        expected,
        "whatever order they are read in"
    );
    assert!(
        wallboard_relation_facts(id(1), [part_of(2, 1)].into_iter()).is_empty(),
        "being a parent is not having one"
    );
}

#[test]
fn wallboard_relation_facts_tell_the_changes_that_are_real_from_those_that_are_not() {
    use WallboardOtherEnd::{Dependent, Prerequisite};
    let facts = |edges: &[Edge]| wallboard_relation_facts(id(2), edges.iter().copied());
    let standing = [waits(2, 1)];
    assert_eq!(facts(&standing), [(Prerequisite, id(1), false)]);
    assert_ne!(
        facts(&standing),
        facts(&[Edge { proposed: true, ..waits(2, 1) }]),
        "accepting a proposed one changes them"
    );
    assert_ne!(facts(&standing), facts(&[waits(2, 3)]), "so does waiting for another task");
    assert_ne!(facts(&standing), facts(&[waits(1, 2)]), "and the same two tasks the other way round");
    assert_eq!(facts(&[waits(1, 2)]), [(Dependent, id(1), false)]);
    assert!(facts(&[]).is_empty(), "a retracted one has dropped out at Edge::of");
    assert_eq!(facts(&[waits(2, 1), waits(2, 1)]), facts(&standing), "linked twice, it is still one fact");
}

#[test]
fn wallboard_draws_a_selected_task_only_once_it_is_due() {
    const NOW: i64 = 1_000;
    let selection = TaskSelection {
        statuses: vec![TaskStatus::Proposed, TaskStatus::Open, TaskStatus::InProgress, TaskStatus::InReview],
    };
    // `comes_due` here is what `clock::comes_due` answers at `NOW`, from the status and date it reads.
    let draws = |status: TaskStatus, date: Option<i64>| {
        let comes_due = date.filter(|_| fwd_core::rules::not_yet_due(status, date, NOW));
        wallboard_draws(&selection, Some(status), comes_due)
    };
    assert!(!draws(TaskStatus::Open, Some(NOW + 1)), "open, its date still to come");
    assert!(!draws(TaskStatus::Proposed, Some(NOW + 1)), "proposed, its date still to come");
    assert!(draws(TaskStatus::Open, Some(NOW)), "exactly at its date");
    assert!(draws(TaskStatus::Open, Some(NOW - 1)), "its date gone by");
    assert!(draws(TaskStatus::Open, None), "no date at all");
    // A date holds back only work nobody has picked up.
    assert!(draws(TaskStatus::InProgress, Some(NOW + 1)));
    assert!(draws(TaskStatus::InReview, Some(NOW + 1)));
    // Being due does not bring back a status the selection leaves out.
    assert!(!wallboard_draws(&selection, Some(TaskStatus::Done), None));
    assert!(!wallboard_draws(&selection, None, None));
}

/// A task waiting for one the drawing leaves out still says it waits, through what `Dependencies`
/// watches and `Availability` reads; only its arrow goes, having no card to point at.
#[test]
fn wallboard_task_waiting_for_one_not_yet_due_reads_as_blocked_with_no_arrow() {
    let (waiting, not_yet_due) = (id(1), id(2));
    assert!(!wallboard_relation(&HashSet::from([waiting]), &waits(1, 2)));
    assert!(wallboard_relation(&HashSet::from([waiting, not_yet_due]), &waits(1, 2)), "drawn once both its tasks are");
    for status in [TaskStatus::Open, TaskStatus::Proposed] {
        let availability = Availability::of(waiting, None, &[(not_yet_due, Some(status))]);
        assert_eq!(availability.labels(), ["Blocked"]);
        assert!(availability.quiet(), "faded with every other blocked card");
    }
}
