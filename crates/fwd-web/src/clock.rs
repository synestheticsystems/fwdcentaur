//! Keeps what the page says about not-before dates true without a reload. Whether a date holds a
//! task back changes because time passes, or because someone corrects the machine's clock, not
//! because anything in the graph changes, so no live query says so.
//!
//! Whatever judges a date asks [`present`], which reads the wall clock each time, so that a task
//! arriving after a quiet spell is judged by the time it really is. Asking also subscribes the
//! asker to a tick, which holds no time of its own. A check once a minute, and whenever the page
//! comes back into view, fires the tick only when a task the page holds would now be judged
//! otherwise than it was, whichever way the clock moved, so that a task comes and goes on time and
//! nothing redraws on a minute that changed nothing.

use crate::store;
use fwd_core::model::{TaskStatus, TaskView};
use fwd_core::rules;
use leptos::prelude::*;
use std::sync::atomic::{AtomicI64, Ordering::Relaxed};
use std::sync::OnceLock;
use std::time::Duration;

/// How often an open page looks for a task it would now judge otherwise.
const CHECK_EVERY: Duration = Duration::from_secs(60);

/// What the readers of the present share with the checks. The page has one; a test has its own.
struct Clock {
    /// What a reader of the present subscribes to, and a check fires.
    tick: ArcTrigger,
    /// The earliest and the latest instants given to readers since the last check, that check's own
    /// included. Every judgment still standing says what one made somewhere between the two would,
    /// however the clock was corrected in the meantime.
    earliest: AtomicI64,
    latest: AtomicI64,
}

impl Clock {
    fn new(now: i64) -> Clock {
        Clock { tick: ArcTrigger::new(), earliest: AtomicI64::new(now), latest: AtomicI64::new(now) }
    }

    /// Gives a reader the present, `now` by the wall clock, and subscribes it to the tick.
    fn read(&self, now: i64) -> i64 {
        self.tick.track();
        self.earliest.fetch_min(now, Relaxed);
        self.latest.fetch_max(now, Relaxed);
        now
    }

    /// Fires the tick when a judgment still standing may no longer hold at `now` for any of `tasks`,
    /// each a status and a not-before date. After this, every judgment still standing says what one
    /// made at `now` would, or its reader is about to run again.
    fn check(&self, tasks: impl IntoIterator<Item = (TaskStatus, Option<i64>)>, now: i64) {
        let judged = [self.earliest.swap(now, Relaxed), self.latest.swap(now, Relaxed)];
        if a_judgment_changed(tasks, judged, now) {
            self.tick.notify();
        }
    }
}

/// Whether any of `tasks`, each a status and a not-before date, is judged otherwise at `now` than
/// at either of `judged`. The two speak for every instant between them, because time running on
/// changes a task's judgment at most once (`rules::not_yet_due`).
fn a_judgment_changed(tasks: impl IntoIterator<Item = (TaskStatus, Option<i64>)>, judged: [i64; 2], now: i64) -> bool {
    tasks.into_iter().any(|(status, date)| {
        let not_yet_due = |at| rules::not_yet_due(status, date, at);
        judged.into_iter().any(|at| not_yet_due(at) != not_yet_due(now))
    })
}

fn page_clock() -> &'static Clock {
    static CLOCK: OnceLock<Clock> = OnceLock::new();
    CLOCK.get_or_init(|| Clock::new(fwd_core::now_ms()))
}

/// The present, in milliseconds since the epoch, as the wall clock says at this moment. Asked inside
/// a closure, it makes the closure a dependent of the clock, so the closure asks again when a check
/// finds a task it would judge otherwise.
pub fn present() -> i64 {
    page_clock().read(fwd_core::now_ms())
}

/// The date `task` comes due, while it is not yet due; none for a task due now. A task with no date
/// has nothing to judge, so asking about it reads no clock: when it gets a date, whoever asked runs
/// again through the field it read.
pub fn comes_due(task: &TaskView) -> Option<i64> {
    let date = task.not_before().ok().flatten()?;
    rules::not_yet_due(task.status().ok()?, Some(date), present()).then_some(date)
}

/// Starts the checks, until the component it is called in goes (in `App`, for as long as the page
/// lives). They read the store, so the store must already be open.
pub fn keep_time() {
    let check_in_view = || {
        if web_sys::window().and_then(|w| w.document()).is_some_and(|d| !d.hidden()) {
            check();
        }
    };
    if let Ok(every) = set_interval_with_handle(check_in_view, CHECK_EVERY) {
        on_cleanup(move || every.clear());
    }
    let back_in_view = window_event_listener_untyped("visibilitychange", move |_| check_in_view());
    on_cleanup(move || back_in_view.remove());
}

/// Holds the tasks the page has up against the wall clock.
fn check() {
    let tasks = store::tasks();
    let dated = tasks.iter().filter_map(|t| Some((t.status().ok()?, t.not_before().ok()?)));
    page_clock().check(dated, fwd_core::now_ms());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    const MINUTE: i64 = 60_000;
    const DATE: i64 = 1_000 * MINUTE;

    /// A page holding one open task dated `DATE`, with a reader of whether it is not yet due, as the
    /// Graph's drawing is, and a wall clock the test sets.
    struct Page {
        clock: Arc<Clock>,
        wall: Arc<AtomicI64>,
        /// Something else the reader reads, whose change runs it again between two checks.
        elsewhere: ArcTrigger,
        not_yet_due: ArcMemo<bool>,
        runs: Arc<AtomicUsize>,
    }

    impl Page {
        /// A page opened with the wall clock at `now`, which has not judged the task yet.
        fn opened_at(now: i64) -> Page {
            let (clock, wall) = (Arc::new(Clock::new(now)), Arc::new(AtomicI64::new(now)));
            let (elsewhere, runs) = (ArcTrigger::new(), Arc::new(AtomicUsize::new(0)));
            let not_yet_due = ArcMemo::new({
                let (clock, wall, elsewhere, runs) = (clock.clone(), wall.clone(), elsewhere.clone(), runs.clone());
                move |_| {
                    runs.fetch_add(1, Relaxed);
                    elsewhere.track();
                    rules::not_yet_due(TaskStatus::Open, Some(DATE), clock.read(wall.load(Relaxed)))
                }
            });
            Page { clock, wall, elsewhere, not_yet_due, runs }
        }

        /// What the reader says now, and how many times it has run to say it.
        fn says(&self) -> (bool, usize) {
            (self.not_yet_due.get(), self.runs.load(Relaxed))
        }

        /// The wall clock comes to read `now`, and a check runs.
        fn check_at(&self, now: i64) {
            self.wall.store(now, Relaxed);
            self.clock.check([(TaskStatus::Open, Some(DATE))], now);
        }

        /// The wall clock comes to read `now`, and something else the reader reads changes, so
        /// that it judges the task again with no check.
        fn reader_runs_at(&self, now: i64) {
            self.wall.store(now, Relaxed);
            self.elsewhere.notify();
        }
    }

    /// The page's own clock, with no check in all this time: the page last judged a date at
    /// `quiet_since`, a task dated after that arrives once its date has passed, and judging it asks
    /// the present afresh. A present that stood still until a date crossed would hold the task back.
    #[test]
    fn a_task_dated_during_a_quiet_spell_is_judged_by_the_time_it_really_is() {
        let quiet_since = present();
        std::thread::sleep(Duration::from_millis(5));
        let dated_during_the_quiet = quiet_since + 1;
        assert!(!rules::not_yet_due(TaskStatus::Open, Some(dated_during_the_quiet), present()));
    }

    /// Time passes on an open page, and a check finds the date has arrived.
    #[test]
    fn a_task_comes_due_when_a_check_finds_its_date_has_arrived() {
        let page = Page::opened_at(DATE - MINUTE);
        assert_eq!(page.says(), (true, 1));
        page.check_at(DATE - 1);
        assert_eq!(page.says(), (true, 1), "a moment before its date");
        page.check_at(DATE);
        assert_eq!(page.says(), (false, 2), "exactly at its date");
    }

    /// An always-running page's clock runs ahead, so that the task shows, and is then corrected.
    #[test]
    fn a_task_goes_back_to_not_yet_due_when_the_clock_is_corrected_to_before_its_date() {
        let page = Page::opened_at(DATE + MINUTE);
        assert_eq!(page.says(), (false, 1));
        page.check_at(DATE);
        assert_eq!(page.says(), (false, 1), "corrected, but not to before its date");
        page.check_at(DATE - 1);
        assert_eq!(page.says(), (true, 2));
    }

    /// Minutes go by on both sides of the date, some of them after corrections of the clock that
    /// stay on that side, and nothing the page shows has changed in any of them.
    #[test]
    fn a_minute_that_changes_no_judgment_runs_no_reader_again() {
        let page = Page::opened_at(DATE - 10 * MINUTE);
        assert_eq!(page.says(), (true, 1));
        for now in [DATE - 9 * MINUTE, DATE - 8 * MINUTE, DATE - 60 * MINUTE, DATE - 59 * MINUTE] {
            page.check_at(now);
            assert_eq!(page.says(), (true, 1), "still before its date, at {now}");
        }
        page.check_at(DATE + MINUTE);
        assert_eq!(page.says(), (false, 2), "the crossing runs it once");
        for now in [DATE + 2 * MINUTE, DATE + 3 * MINUTE, DATE, DATE + MINUTE] {
            page.check_at(now);
            assert_eq!(page.says(), (false, 2), "still after its date, at {now}");
        }
    }

    /// Between two checks the reader judges the task because something else it reads changed, while
    /// the clock is off, and the clock is corrected before the next check. The last check and the
    /// next one agree about the task, and only the reader's judgment differs from them.
    #[test]
    fn a_judgment_made_between_checks_is_made_again_when_the_clock_is_then_corrected() {
        let ran_ahead = Page::opened_at(DATE - 2 * MINUTE);
        assert_eq!(ran_ahead.says(), (true, 1));
        ran_ahead.reader_runs_at(DATE + MINUTE);
        assert_eq!(ran_ahead.says(), (false, 2));
        ran_ahead.check_at(DATE - MINUTE);
        assert_eq!(ran_ahead.says(), (true, 3), "the task goes back to not yet due");

        let ran_behind = Page::opened_at(DATE + 2 * MINUTE);
        assert_eq!(ran_behind.says(), (false, 1));
        ran_behind.reader_runs_at(DATE - MINUTE);
        assert_eq!(ran_behind.says(), (true, 2));
        ran_behind.check_at(DATE + MINUTE);
        assert_eq!(ran_behind.says(), (false, 3), "the task comes back");
    }

    #[test]
    fn a_judgment_changes_when_a_date_lies_between_the_instants_compared_whichever_way_the_clock_moved() {
        const EARLIER: i64 = 1_000;
        const LATER: i64 = 2_000;
        let open = |date| (TaskStatus::Open, Some(date));
        for (judged, now) in [(EARLIER, LATER), (LATER, EARLIER)] {
            let changed =
                |tasks: &[(TaskStatus, Option<i64>)]| a_judgment_changed(tasks.iter().copied(), [judged, judged], now);
            assert!(changed(&[open(EARLIER + 1)]), "a date between the two, from {judged} to {now}");
            assert!(changed(&[open(LATER)]), "a date exactly at the later, from {judged} to {now}");
            assert!(changed(&[open(LATER + 1), (TaskStatus::Proposed, Some(LATER))]), "one of several");
            assert!(!changed(&[open(LATER + 1)]), "a date still to come at both, from {judged} to {now}");
            assert!(!changed(&[open(EARLIER)]), "a date passed at both, from {judged} to {now}");
            assert!(!changed(&[(TaskStatus::Open, None)]), "no date");
            assert!(!changed(&[(TaskStatus::InProgress, Some(LATER))]), "a task no date holds back");
            assert!(!changed(&[]), "no tasks");
        }
        let date = [open(1_500)];
        assert!(a_judgment_changed(date, [EARLIER, LATER], 1_400), "only the later of those judged differs");
        assert!(a_judgment_changed(date, [EARLIER, LATER], 1_600), "only the earlier of those judged differs");
        assert!(!a_judgment_changed(date, [EARLIER, 1_400], 1_450), "both are before the date");
    }
}
