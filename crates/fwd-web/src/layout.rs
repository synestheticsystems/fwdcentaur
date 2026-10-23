//! Where the Graph page puts each task and each edge (`graph.rs` draws them): a layered drawing in
//! which a task sits to the right of everything it waits for, so work flows left to right, with as
//! few edges crossing as a short search finds and edges running straight where they can. It is a
//! module of its own, free of the page, so that it is tested natively. Daniel, 2026-09-28, looking
//! at the page filtered to one scope: "Some crossings are unavoidable, but this is noisier than it
//! could be - I think we could find a better algorithm. maybe a sort by priority/status vs auto
//! (default. sort to minimize crossings)?"
//!
//! What happens, in order:
//! - Each task gets a column: the length of the longest chain of tasks it waits for, so the first
//!   column holds the tasks that wait for nothing. Should proposed relations close a loop (the
//!   rules refuse a loop of accepted ones), one edge of the loop runs against the flow instead.
//! - An edge that spans more than one column gets a waypoint in each column it passes: a place in
//!   that column, like a task's, that the edge runs through, so the edge passes between the tasks
//!   there instead of across them, and counts when crossings are counted.
//! - The automatic order lays out on its own each group of tasks that edges join to one another
//!   but to nothing else, and stacks the groups, the one holding the most urgent task first; tasks
//!   with no edge at all go in a stack of their own below them. Within a group, each column is
//!   sorted by the median place of its nodes' neighbours in the column before it, then in the
//!   column after it, alternately, and after each pass neighbours in a column swap while that
//!   removes crossings. Crossings are counted exactly between each pair of neighbouring columns,
//!   and the order with the fewest seen is kept. Where two tasks could trade places without
//!   changing that count, the more urgent one goes higher (by priority, then title, then id), so a
//!   graph is drawn the same way every time, whatever order its tasks arrive in.
//! - The priority and status orders sort each column whole, every group and the tasks with no edge
//!   together, and nothing moves a task out of that order: only the waypoints move, each to where
//!   its edge crosses fewest others.
//! - Heights: in the automatic order each column in turn moves as close as its order and spacing
//!   allow to where its neighbours pull it, each task toward the median height of its neighbours in
//!   the column before (and, on the way back, in the column after), so edges run straight where
//!   they can; a waypoint pulls hardest, so a long edge runs level. In the priority and status
//!   orders each column is packed from the top, so its order reads straight down the page.
//! - Bands: so that a parent task's subtasks are drawn together (`graph.rs`; D134), a task may be
//!   drawn in a band, a strip across the drawing that bands nested in it lie within. With bands,
//!   every task is laid out in one group instead, each kept within the strip of its band, and the
//!   strips are stacked down the drawing in the order that disturbs the layout without bands least
//!   (`in_bands`, `Tiers`); the columns, waypoints, search and heights work as above within them.
//! - Lines the page draws over the layout, a selected task's implied dependencies (`graph.rs`),
//!   take no place in it, so that selecting a task moves nothing: each is threaded through the
//!   columns it crosses, level through each at a height where it keeps clear of the boxes and lines
//!   there, as near as it can to the straight line between its tasks.
//! - Ends: so that the lines meeting a task can be told apart there, each meets the task's side at
//!   a point of its own, the points spread down the side in the order of where each line goes next,
//!   which keeps lines that meet one task from crossing each other there (task bL58YOSa, "Draw only
//!   the dependencies no chain implies, give each line its own end, and light a selected task's
//!   chain": the busiest task of a scope of 71 had 32 lines leave it at one point).

use std::cmp::Ordering;
use std::collections::BTreeMap;

/// A task's box and the room around it, in the drawing's pixels: boxes `W` wide and `H` tall,
/// columns `GX` apart, boxes in a column at least `GY` apart, and `PAD` around the drawing.
pub const W: f64 = 200.0;
pub const H: f64 = 56.0;
pub const GX: f64 = 80.0;
pub const GY: f64 = 18.0;
pub const PAD: f64 = 24.0;

/// The extra room above a group stacked below another, and above the stack of tasks with no edge,
/// so that each reads as a group of its own.
const GROUP_GAP: f64 = 2.0 * GY;

/// How far a band's edge lies outside what it holds, the tasks drawn in it and the bands nested in
/// it, so that a band nested in another is drawn inside it, and what the page draws above a task's
/// top edge, its count of messages and the parents whose band does not hold it (`graph.rs`), lies
/// inside the band too.
pub const BAND_PAD: f64 = GY;

/// The room above a band that opens with a band nested in it rather than its own tasks, for its
/// label (`graph.rs`), which would otherwise lie on the nested band's.
pub const LABEL_ROOM: f64 = 40.0;

/// How many passes the search for the automatic order makes, keeping the best it sees, and how
/// many times the heights are pulled toward the neighbours each way.
const ORDER_PASSES: usize = 24;
const HEIGHT_PASSES: usize = 8;

/// How many times the bands that are siblings move toward the bands their edges lead to.
const BAND_SWEEPS: usize = 8;

/// How much harder a waypoint pulls toward its neighbour than a task with one neighbour does, so a
/// long edge runs level through the columns it crosses.
const WAYPOINT_PULL: f64 = 64.0;

/// How far down a task's side the ends of the lines meeting it spread, 8 px short of each corner,
/// and the most room between two of them: a few lines meet a side `END_GAP` apart about its middle,
/// and more share the whole span.
const END_SPAN: f64 = H - 16.0;
const END_GAP: f64 = 8.0;

/// How far a threaded line keeps from the boxes and lines of a column it crosses: half the least
/// gap between two of them, so that it can pass between any two.
const CLEARANCE: f64 = GY / 2.0;

/// How far into the gap after a column a threaded line between two tasks of that column runs
/// before it turns back; less than `PAD`, so that it stays in the drawing beside the last column.
const LOOP_OUT: f64 = GX / 4.0;

/// How each column of the drawing is ordered, top to bottom.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Order {
    /// So that edges cross as little as the search finds, and run straight where they can.
    #[default]
    Auto,
    /// By priority, P0 first and tasks with none last, then by title.
    Priority,
    /// By status in working order (in progress, in review, open, proposed, then the closed ones),
    /// then by priority, then by title.
    Status,
}

impl Order {
    pub const ALL: [Order; 3] = [Order::Auto, Order::Priority, Order::Status];

    /// Its name, as the page's select shows it and this browser remembers it.
    pub fn name(self) -> &'static str {
        match self {
            Order::Auto => "auto",
            Order::Priority => "priority",
            Order::Status => "status",
        }
    }

    /// The order `name` names, or none for anything else.
    pub fn named(name: &str) -> Option<Order> {
        Order::ALL.into_iter().find(|order| order.name() == name)
    }
}

/// A task as the layout weighs it.
#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub title: String,
    /// Its id, written out: the last word between tasks that tie on everything else.
    pub id: String,
    pub priority: Option<i64>,
    /// Where its status comes in working order (`fmt::status_rank`): in progress first.
    pub status: u8,
    /// The band it is drawn in, an index into the bands the layout is given; none for a task in no
    /// band.
    pub band: Option<usize>,
}

/// A band: a strip across the drawing that holds the tasks drawn in it and the bands nested in it,
/// so that a parent task's subtasks are drawn together (`graph.rs`).
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    /// The band it is nested in, an index into the bands given, or none.
    pub within: Option<usize>,
    /// Its id, written out: the last word between two bands that tie on everything else.
    pub id: String,
}

/// Where the drawing puts things, in its own pixels.
#[derive(Debug, PartialEq)]
pub struct Drawing {
    /// Each task's box, by its top-left corner, in the order the tasks were given.
    pub boxes: Vec<(f64, f64)>,
    /// Each edge's route, in the order the edges were given: from a side of the box of the task
    /// waited for to a side of the box of the task that waits, through the place of its waypoint in
    /// each column between, where it enters on the left and leaves on the right at the same height.
    /// Each edge meets each of its tasks at a point of its own on that side (`END_SPAN`), unless
    /// it joins the same two tasks as another edge, which it is then drawn over. The page draws a
    /// route as one curve: level from a point to the next at the same height, and from a point to
    /// the next at another height a curve that leaves the first level and reaches the second level,
    /// bending halfway across. An edge that names no task given, or joins a task to itself, has no
    /// route.
    pub routes: Vec<Vec<(f64, f64)>>,
    /// Each threaded line's route, in the order the lines were given, drawn as an edge's is: from
    /// the side of the box of its first task that faces its other task, level through each column
    /// between at a height clear of everything there, to the side of the other task's box that
    /// faces the first; between two tasks of one column, out into the gap after it and back. It too
    /// meets each task at a point of its own.
    pub threaded: Vec<Vec<(f64, f64)>>,
    /// Each band's top and bottom, in the order the bands were given: around its tasks and the
    /// bands nested in it, `BAND_PAD` clear of them. None for a band that holds no task, nor any
    /// band that does.
    pub bands: Vec<Option<(f64, f64)>>,
    pub width: f64,
    pub height: f64,
}

/// A layout made (`Layout::new`), all of it that lines threaded through it cannot change, so that
/// the page makes it once for what it draws and draws it again with each selected task's threaded
/// lines (`Layout::draw`), which costs little: the search for an order and the heights cost the
/// most, and at several hundred tasks in bands more than a page can spend on every click.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// Per task given, its number: the layout numbers the tasks by urgency and works on the
    /// numbers, so nothing it does depends on the order the tasks arrive in.
    number: Vec<usize>,
    /// Per edge given, the pair of numbered tasks it joins; none for one that names no task given
    /// or joins a task to itself.
    given: Vec<Option<(usize, usize)>>,
    /// Per numbered task, its column.
    column: Vec<usize>,
    placed: Placed,
}

impl Layout {
    /// Lays out `tasks` joined by `edges`, each `(first, then)`, indexes into `tasks`: the task
    /// `then` waits for the task `first`, so it sits to the right of it. `order` says how each
    /// column is ordered. A task in one of `bands` is drawn within that band's strip of the
    /// drawing, which lies within the bands that band is nested in (`in_bands`).
    pub fn new(tasks: &[Task], bands: &[Band], edges: &[(usize, usize)], order: Order) -> Layout {
        let n = tasks.len();
        let keys: Vec<_> = tasks.iter().map(urgency).collect();
        let mut by_urgency: Vec<usize> = (0..n).collect();
        by_urgency.sort_by(|&a, &b| keys[a].cmp(&keys[b]));
        let mut number = vec![0; n];
        for (k, &t) in by_urgency.iter().enumerate() {
            number[t] = k;
        }
        let given = numbered(&number, edges);
        let pairs = without_loops(n, given.iter().flatten().copied().collect());
        let column = columns_of(n, &pairs);
        // Each task's rank in the chosen order: it sorts the columns of the priority and status
        // orders and settles every tie.
        let rank: Vec<usize> = match order {
            Order::Auto | Order::Priority => (0..n).collect(),
            Order::Status => {
                let mut by_status: Vec<usize> = (0..n).collect();
                by_status.sort_by_key(|&k| (tasks[by_urgency[k]].status, k));
                let mut rank = vec![0; n];
                for (r, &k) in by_status.iter().enumerate() {
                    rank[k] = r;
                }
                rank
            }
        };
        // The band each numbered task is drawn in, an index into `bands`.
        let mut band_of: Vec<Option<usize>> = vec![None; n];
        for (t, task) in tasks.iter().enumerate() {
            band_of[number[t]] = task.band.filter(|&band| band < bands.len());
        }
        let laid = LaidOut { n, pairs: &pairs, column: &column, rank: &rank, order };
        let placed = if band_of.iter().any(Option::is_some) {
            laid.in_bands(&band_of, bands)
        } else {
            Placed { bands: vec![None; bands.len()], ..laid.in_groups() }
        };
        Layout { number, given, column, placed }
    }

    /// The drawing of the layout, with each line's ends spread on the tasks it meets. `threaded`
    /// are lines of the same form as the edges that take no place in the layout: they are routed
    /// through the drawing the edges make, whose boxes and waypoints stay where they are; only the
    /// ends of the edges that meet a task on the side a threaded line meets it move along that
    /// side.
    pub fn draw(&self, threaded: &[(usize, usize)]) -> Drawing {
        let Placed { centre, routes, taken, columns, bottom, bands } = &self.placed;
        let mut taken = taken.clone();
        let mut lines: Vec<Line> =
            routes.iter().map(|(pair, route)| Line { pair: *pair, threaded: false, route: route.clone() }).collect();
        // Each threaded line once, in the order of its numbers, so that which of two takes a gap
        // they both want does not depend on the order they were given in.
        let wanted = numbered(&self.number, threaded);
        let mut threads: Vec<(usize, usize)> = wanted.iter().flatten().copied().collect();
        threads.sort_unstable();
        threads.dedup();
        for (a, b) in threads {
            let route = thread(a, b, &self.column, centre, &mut taken);
            lines.push(Line { pair: (a, b), threaded: true, route });
        }
        spread_ends(&mut lines, centre);
        let mut laid = BTreeMap::new();
        let mut through = BTreeMap::new();
        for line in lines {
            if line.threaded {
                through.insert(line.pair, line.route);
            } else {
                laid.insert(line.pair, line.route);
            }
        }
        let boxes = self.number.iter().map(|&k| (x(self.column[k]), centre[k] - H / 2.0)).collect();
        let routes = self
            .given
            .iter()
            .map(|pair| {
                let Some((a, b)) = *pair else { return Vec::new() };
                match laid.get(&(a, b)) {
                    Some(route) => route.clone(),
                    // An edge that runs against the flow, to close a loop, goes the other way.
                    None => laid.get(&(b, a)).map(|route| route.iter().rev().copied().collect()).unwrap_or_default(),
                }
            })
            .collect();
        let threaded =
            wanted.iter().map(|pair| pair.and_then(|pair| through.get(&pair).cloned()).unwrap_or_default()).collect();
        Drawing {
            boxes,
            routes,
            threaded,
            bands: bands.clone(),
            width: PAD * 2.0 + *columns as f64 * (W + GX) - GX,
            height: (bottom + PAD).max(PAD * 2.0 + H),
        }
    }
}

/// `lines`, each `(first, then)`, indexes into the tasks `number` numbers, as pairs of numbered
/// tasks; none for a line that names no task, or joins a task to itself.
fn numbered(number: &[usize], lines: &[(usize, usize)]) -> Vec<Option<(usize, usize)>> {
    let n = number.len();
    lines
        .iter()
        .map(|&(first, then)| (first < n && then < n && first != then).then(|| (number[first], number[then])))
        .collect()
}

/// What the layout works from once it has numbered the tasks: the pairs of numbered tasks the
/// edges join, each task's column and its rank in the chosen order, and that order.
struct LaidOut<'a> {
    n: usize,
    pairs: &'a [(usize, usize)],
    column: &'a [usize],
    rank: &'a [usize],
    order: Order,
}

/// Where the layout put the numbered tasks and the edges between them, before the threaded lines
/// and the ends: each task's centre, each pair's route, per column the spans its boxes and the
/// lines through its waypoints take (top to bottom, which a threaded line keeps clear of), how
/// many columns there are, how far down the drawing reaches, and each band's top and bottom.
#[derive(Clone, Debug, PartialEq)]
struct Placed {
    centre: Vec<f64>,
    routes: BTreeMap<(usize, usize), Vec<(f64, f64)>>,
    taken: Vec<Vec<(f64, f64)>>,
    columns: usize,
    bottom: f64,
    bands: Vec<Option<(f64, f64)>>,
}

impl LaidOut<'_> {
    /// How many columns the drawing has.
    fn columns(&self) -> usize {
        self.column.iter().max().map_or(1, |c| c + 1)
    }

    /// The layout with no bands: in the automatic order each group of tasks that edges join to one
    /// another but to nothing else laid out on its own, the groups stacked, the one holding the most
    /// urgent task first, and the tasks with no edge in a stack of their own below them; in the
    /// priority and status orders every task in one group.
    fn in_groups(&self) -> Placed {
        let (groups, edgeless) = match self.order {
            Order::Auto => groups_of(self.n, self.pairs),
            Order::Priority | Order::Status => (vec![(0..self.n).collect()], Vec::new()),
        };
        let mut placed = Placed {
            centre: vec![0.0; self.n],
            routes: BTreeMap::new(),
            taken: vec![Vec::new(); self.columns()],
            columns: 1,
            bottom: PAD,
            bands: Vec::new(),
        };
        let mut top = PAD;
        for members in groups.iter().filter(|members| !members.is_empty()) {
            let group = Group::new(members, self.pairs, self.column, self.rank, |_| 0, |_| 0);
            // One strip with no bounds, which the group is packed into from 0 and pulled about in
            // freely: its heights are its own, set below the groups before it afterwards.
            let strip = [(f64::NEG_INFINITY, f64::INFINITY)];
            let (columns, heights) = match self.order {
                Order::Auto => {
                    let columns = group.order_auto(group.columns_by_tie(true));
                    let heights = group.pulled(&columns, &strip);
                    (columns, heights)
                }
                Order::Priority | Order::Status => {
                    let columns = group.order_fixed();
                    let heights = group.packed(&columns, &strip);
                    (columns, heights)
                }
            };
            let highest = (0..group.len()).map(|v| heights[v] - group.half(v)).fold(f64::INFINITY, f64::min);
            let lowest = (0..group.len()).map(|v| heights[v] + group.half(v)).fold(f64::NEG_INFINITY, f64::max);
            let at: Vec<f64> = heights.iter().map(|height| top + height - highest).collect();
            group.place(&at, &mut placed);
            placed.columns = placed.columns.max(columns.len());
            placed.bottom = top + lowest - highest;
            top = placed.bottom + GROUP_GAP;
        }
        // The tasks with no edge, in the first column, which is theirs, below everything else.
        for (i, &t) in edgeless.iter().enumerate() {
            placed.centre[t] = top + H / 2.0 + i as f64 * (H + GY);
            placed.bottom = placed.centre[t] + H / 2.0;
            placed.taken[0].push((placed.centre[t] - H / 2.0, placed.bottom));
        }
        placed
    }

    /// The layout with bands (`Tiers`): every task in one group, each kept within the strip of its
    /// tier, the strips stacked down the drawing. The tiers, and in the automatic order where each
    /// column's search starts, follow the layout the tasks have with no bands, so that the bands
    /// disturb it as little as they can. Each strip is as tall as its tier needs in its busiest
    /// column, and in the automatic order each task is then pulled toward its neighbours within its
    /// strip, as `pulled` pulls it without bands.
    ///
    /// Two choices weigh differently from one graph to another, so the layout makes both ways and
    /// keeps the drawing whose edges cross least, the first on a tie: whether the bands that are
    /// siblings keep the order the tasks have with no bands or move toward the bands their edges
    /// lead to (`Tree::sweep`), and whether each waypoint of a long edge keeps to the tier of the
    /// task its edge leads to, so that the edge runs level beside it, or of the task it leaves.
    fn in_bands(&self, band_of: &[Option<usize>], bands: &[Band]) -> Placed {
        let free = self.in_groups();
        let everything: Vec<usize> = (0..self.n).collect();
        // One way: the tiers after `sweeps` sweeps, each waypoint in the tier of the task its edge
        // leads to (`toward_then`) or leaves; with how many times its edges cross.
        let way = |sweeps: usize, toward_then: bool| {
            let tiers = Tiers::new(band_of, bands, self.pairs, &free.centre, sweeps);
            let waypoint = |(a, b): (usize, usize)| tiers.of[if toward_then { b } else { a }];
            let group = Group::new(&everything, self.pairs, self.column, self.rank, |t| tiers.of[t], waypoint);
            let columns = match self.order {
                Order::Auto => {
                    // Where each node sits with no bands: a task's centre, a waypoint's line.
                    let mut start = vec![0.0; group.len()];
                    for (v, task) in group.task.iter().enumerate() {
                        if let Some(t) = task {
                            start[v] = free.centre[*t];
                        }
                    }
                    for (pair, chain) in &group.chains {
                        let route = &free.routes[pair];
                        for (k, &waypoint) in chain[1..chain.len() - 1].iter().enumerate() {
                            start[waypoint] = route[2 * k + 1].1;
                        }
                    }
                    group.order_auto(group.columns_from(&start))
                }
                Order::Priority | Order::Status => group.order_fixed(),
            };
            (group.crossings(&columns), tiers, group, columns)
        };
        let mut best = way(0, true);
        for (sweeps, toward_then) in [(0, false), (BAND_SWEEPS, true), (BAND_SWEEPS, false)] {
            let other = way(sweeps, toward_then);
            if other.0 < best.0 {
                best = other;
            }
        }
        let (_, tiers, group, columns) = best;
        let (strips, extents, bottom) = tiers.stack(&group.needs(&columns, tiers.count()), bands.len());
        let heights = match self.order {
            Order::Auto => group.pulled(&columns, &strips),
            Order::Priority | Order::Status => group.packed(&columns, &strips),
        };
        let mut placed = Placed {
            centre: vec![0.0; self.n],
            routes: BTreeMap::new(),
            taken: vec![Vec::new(); self.columns()],
            columns: self.columns(),
            bottom,
            bands: extents,
        };
        group.place(&heights, &mut placed);
        placed
    }
}

/// A stretch down the drawing: its top and its bottom.
type Span = (f64, f64);

/// The bands as the layout stacks them, each a run of tiers: a tier is the strip of the drawing
/// that holds the tasks drawn in one band and in none nested in it, or the tasks in no band. A
/// band's own tasks come first in it, then the bands nested in it, each in turn; the tasks in no
/// band take their place among the outermost bands. Bands that are siblings go in the order of
/// the mean height their tasks, nested ones too, have in the layout with no bands (`weight`), then
/// by id, so that the bands stack as the tasks would without them; then, as many times as
/// `Tiers::new` is asked, each moves toward the bands its edges lead to (`Tree::sweep`).
struct Tiers {
    /// Per numbered task, its tier.
    of: Vec<usize>,
    /// Per tier, the bands it is in, outermost first.
    within: Vec<Vec<usize>>,
}

impl Tiers {
    fn new(
        band_of: &[Option<usize>],
        bands: &[Band],
        pairs: &[(usize, usize)],
        weight: &[f64],
        sweeps: usize,
    ) -> Tiers {
        let tree = Tree::new(band_of, bands, pairs, weight, sweeps);
        let mut tiers = Tiers { of: vec![0; band_of.len()], within: Vec::new() };
        tree.visit(tree.root(), &mut Vec::new(), &mut tiers);
        tiers
    }

    fn count(&self) -> usize {
        self.within.len()
    }

    /// Stacks the tiers down the drawing from `PAD`, each `needs` tall: `GY` between two strips,
    /// `BAND_PAD` more for each band edge between them, and `LABEL_ROOM` more above each band that
    /// opens with a band nested in it, so that its label clears the nested band's. Returns each
    /// tier's strip, top and bottom; each of the `bands` bands' top and bottom; and how far down
    /// the drawing reaches.
    fn stack(&self, needs: &[f64], bands: usize) -> (Vec<Span>, Vec<Option<Span>>, f64) {
        let mut strips = Vec::with_capacity(self.count());
        let mut extents: Vec<Option<Span>> = vec![None; bands];
        let mut y = PAD;
        let mut open: &[usize] = &[];
        for (tier, path) in self.within.iter().enumerate() {
            let common = open.iter().zip(path).take_while(|(a, b)| a == b).count();
            for &b in open[common..].iter().rev() {
                y += BAND_PAD;
                extents[b] = extents[b].map(|(top, _)| (top, y));
            }
            if tier > 0 {
                y += GY;
            }
            for (k, &b) in path[common..].iter().enumerate() {
                if k > 0 {
                    y += LABEL_ROOM;
                }
                extents[b] = Some((y, y));
                y += BAND_PAD;
            }
            strips.push((y, y + needs[tier]));
            y += needs[tier];
            open = path;
        }
        for &b in open.iter().rev() {
            y += BAND_PAD;
            extents[b] = extents[b].map(|(top, _)| (top, y));
        }
        (strips, extents, y)
    }
}

/// The bands as a tree whose root holds the tasks in no band and the outermost bands, for `Tiers`
/// to walk.
struct Tree<'a> {
    bands: &'a [Band],
    /// Per numbered task, its band.
    band_of: &'a [Option<usize>],
    /// Per band, the band it is nested in, with any loop cut.
    within: Vec<Option<usize>>,
    /// The pairs of numbered tasks the edges join.
    pairs: &'a [(usize, usize)],
    /// How many times `sweep` moves the bands that are siblings.
    sweeps: usize,
    /// Per band, and last for the root, the numbered tasks drawn in it and in none nested in it.
    own: Vec<Vec<usize>>,
    /// Per band, and last for the root, the bands nested in it.
    nested: Vec<Vec<usize>>,
    /// Per band, and last for the root, how many tasks it holds, nested bands' included, and the
    /// sum of their weights.
    held: Vec<(usize, f64)>,
    /// The mean weight of the root's own tasks.
    loose: f64,
}

impl<'a> Tree<'a> {
    fn new(
        band_of: &'a [Option<usize>],
        bands: &'a [Band],
        pairs: &'a [(usize, usize)],
        weight: &[f64],
        sweeps: usize,
    ) -> Tree<'a> {
        let count = bands.len();
        // Each band's enclosing band; a band that a loop of `within` would nest in itself sits in
        // none, so that every band is reached from the root.
        let mut within: Vec<Option<usize>> = bands.iter().map(|band| band.within.filter(|&w| w < count)).collect();
        for b in 0..count {
            let (mut at, mut steps) = (within[b], 0);
            while let Some(w) = at {
                if w == b {
                    within[b] = None;
                    break;
                }
                steps += 1;
                if steps > count {
                    break;
                }
                at = within[w];
            }
        }
        let mut tree = Tree {
            bands,
            band_of,
            within: within.clone(),
            pairs,
            sweeps,
            own: vec![Vec::new(); count + 1],
            nested: vec![Vec::new(); count + 1],
            held: vec![(0, 0.0); count + 1],
            loose: 0.0,
        };
        for (t, band) in band_of.iter().enumerate() {
            tree.own[band.unwrap_or(count)].push(t);
        }
        for (b, w) in within.iter().enumerate() {
            tree.nested[w.unwrap_or(count)].push(b);
        }
        tree.weigh(count, weight);
        let loose = &tree.own[count];
        tree.loose = loose.iter().map(|&t| weight[t]).sum::<f64>() / loose.len().max(1) as f64;
        tree
    }

    fn root(&self) -> usize {
        self.bands.len()
    }

    /// Counts and weighs the tasks `b` holds, after those of each band nested in it.
    fn weigh(&mut self, b: usize, weight: &[f64]) {
        let mut held = (self.own[b].len(), self.own[b].iter().map(|&t| weight[t]).sum::<f64>());
        for c in self.nested[b].clone() {
            self.weigh(c, weight);
            held = (held.0 + self.held[c].0, held.1 + self.held[c].1);
        }
        self.held[b] = held;
    }

    /// The mean weight of the tasks `b` holds.
    fn mean(&self, b: usize) -> f64 {
        self.held[b].1 / self.held[b].0 as f64
    }

    /// Adds the tiers of `b`, within the bands `path`, to `tiers`: its own tasks' tier and, after
    /// it, those of the bands nested in it that hold any task, by their mean weight, then by id,
    /// then swept (`sweep`). The root's own tasks go instead where their mean weight puts them
    /// among the outermost bands, and are swept with them.
    fn visit(&self, b: usize, path: &mut Vec<usize>, tiers: &mut Tiers) {
        let mut children: Vec<usize> = self.nested[b].iter().copied().filter(|&c| self.held[c].0 > 0).collect();
        children.sort_by(|&c, &d| {
            self.mean(c).total_cmp(&self.mean(d)).then_with(|| self.bands[c].id.cmp(&self.bands[d].id))
        });
        let own_at = if b == self.root() {
            children.iter().position(|&c| self.mean(c) > self.loose).unwrap_or(children.len())
        } else {
            0
        };
        let mut items: Vec<Option<usize>> = children.iter().map(|&c| Some(c)).collect();
        if !self.own[b].is_empty() {
            items.insert(own_at, None);
        }
        let items = self.sweep(b, items);
        for item in items {
            match item {
                None => {
                    for &t in &self.own[b] {
                        tiers.of[t] = tiers.within.len();
                    }
                    tiers.within.push(path.clone());
                }
                Some(c) => {
                    path.push(c);
                    self.visit(c, path, tiers);
                    path.pop();
                }
            }
        }
    }

    /// Which of `b`'s parts holds task `t`: its own tasks (`Some(None)`) or a band nested in it;
    /// none when `b` does not hold it.
    fn holder(&self, t: usize, b: usize) -> Option<Option<usize>> {
        let mut at = self.band_of[t];
        loop {
            match at {
                None => return (b == self.root()).then_some(None),
                Some(x) if x == b => return Some(None),
                Some(x) => {
                    let outer = self.within[x];
                    if outer.unwrap_or(self.root()) == b {
                        return Some(Some(x));
                    }
                    at = outer;
                }
            }
        }
    }

    /// `items`, the parts of `b` (its own tasks, `None`, and the bands nested in it) in their first
    /// order, moved `sweeps` times toward the mean place of the parts their tasks' edges lead to,
    /// each halfway from where it is, so that the edges between bands stay short; a band's own
    /// tasks stay first in it.
    fn sweep(&self, b: usize, mut items: Vec<Option<usize>>) -> Vec<Option<usize>> {
        let holder: Vec<Option<Option<usize>>> = (0..self.band_of.len()).map(|t| self.holder(t, b)).collect();
        for _ in 0..self.sweeps {
            let place = |item: Option<usize>| items.iter().position(|&i| i == item).unwrap_or(0) as f64;
            let mut keyed: Vec<(f64, usize, Option<usize>)> = items
                .iter()
                .enumerate()
                .map(|(k, &item)| {
                    let mut toward = Vec::new();
                    for &(u, v) in self.pairs {
                        for (mine, theirs) in [(u, v), (v, u)] {
                            match (holder[mine], holder[theirs]) {
                                (Some(here), Some(there)) if here == item && there != item => toward.push(place(there)),
                                _ => {}
                            }
                        }
                    }
                    let key = if toward.is_empty() {
                        k as f64
                    } else {
                        (k as f64 + toward.iter().sum::<f64>() / toward.len() as f64) / 2.0
                    };
                    (key, k, item)
                })
                .collect();
            keyed.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
            items = keyed.into_iter().map(|(_, _, item)| item).collect();
            if b != self.root() {
                if let Some(own) = items.iter().position(Option::is_none) {
                    items.remove(own);
                    items.insert(0, None);
                }
            }
        }
        items
    }
}

/// Where a column's left side lies.
fn x(column: usize) -> f64 {
    PAD + column as f64 * (W + GX)
}

/// A line the layout routes: the pair of numbered tasks it joins, `(first, then)` as it runs, and
/// its route; `threaded` when it takes no place in the layout.
struct Line {
    pair: (usize, usize),
    threaded: bool,
    route: Vec<(f64, f64)>,
}

/// The route of a threaded line from the numbered task `a` to `b`: from the side of `a` facing `b`,
/// through each column between, to the side of `b` facing `a`. Through each column it runs level,
/// at the height nearest the straight line between the two tasks' middles where it keeps clear of
/// what `taken` holds there (`free_height`), which it then takes, so that another threaded line
/// keeps clear of it too. Between two tasks of one column it runs out into the gap after it and
/// back.
fn thread(a: usize, b: usize, column: &[usize], centre: &[f64], taken: &mut [Vec<(f64, f64)>]) -> Vec<(f64, f64)> {
    let (from, to) = (column[a], column[b]);
    match from.cmp(&to) {
        Ordering::Greater => {
            let mut route = thread(b, a, column, centre, taken);
            route.reverse();
            route
        }
        Ordering::Equal => {
            let (side, out) = (x(from) + W, x(from) + W + LOOP_OUT);
            vec![(side, centre[a]), (out, centre[a]), (out, centre[b]), (side, centre[b])]
        }
        Ordering::Less => {
            let mut route = vec![(x(from) + W, centre[a])];
            for (c, spans) in taken.iter_mut().enumerate().take(to).skip(from + 1) {
                let aim = centre[a] + (centre[b] - centre[a]) * (c - from) as f64 / (to - from) as f64;
                let y = free_height(spans, aim);
                spans.push((y, y));
                route.push((x(c), y));
                route.push((x(c) + W, y));
            }
            route.push((x(to), centre[b]));
            route
        }
    }
}

/// The height nearest `aim` at which a line through a column keeps `CLEARANCE` from every span in
/// `taken` (the column's boxes and the lines through it): above them all, below them all, or in a
/// gap between two at least `GY` wide, the least gap the layout leaves, which a line already
/// threaded through it closes. Between two equally near, the higher.
fn free_height(taken: &[(f64, f64)], aim: f64) -> f64 {
    let mut spans = taken.to_vec();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let Some(&(first, _)) = spans.first() else { return aim };
    // Where a line may run: above everything, in each wide enough gap, below everything.
    let mut room = vec![(f64::NEG_INFINITY, first - CLEARANCE)];
    let mut above = f64::NEG_INFINITY;
    for &(low, high) in &spans {
        if above > f64::NEG_INFINITY && low - above >= GY - 1e-6 {
            room.push((above + CLEARANCE, low - CLEARANCE));
        }
        above = above.max(high);
    }
    room.push((above + CLEARANCE, f64::INFINITY));
    let mut best = (f64::INFINITY, aim);
    for (low, high) in room {
        // A gap exactly `GY` wide leaves a single height, which rounding may turn over.
        let y = aim.clamp(low.min(high), high.max(low));
        if (y - aim).abs() < best.0 {
            best = ((y - aim).abs(), y);
        }
    }
    best.1
}

/// Gives every line its own end on each task it meets. The ends meeting one side of a task are
/// spread down it about its middle, `END_GAP` apart or closer so that they span `END_SPAN` at
/// most, in the order of the height each line goes to next, then of the height of its other end:
/// so two lines that meet one task do not cross where they leave it. Every end is read where the
/// layout put it before any moves.
fn spread_ends(lines: &mut [Line], centre: &[f64]) {
    // Per task and side (true for its right side), each end meeting it: the height its line goes
    // to next and the height of its other end, what settles a tie between two lines, and which end
    // of which line it is (true for the start).
    type End = (f64, f64, (bool, (usize, usize)), usize, bool);
    let mut sides: BTreeMap<(usize, bool), Vec<End>> = BTreeMap::new();
    for (i, line) in lines.iter().enumerate() {
        let (route, last) = (&line.route, line.route.len().saturating_sub(1));
        if last == 0 {
            continue;
        }
        let tie = (line.threaded, line.pair);
        let start = (route[1].1, route[last].1, tie, i, true);
        sides.entry((line.pair.0, route[1].0 > route[0].0)).or_default().push(start);
        let end = (route[last - 1].1, route[0].1, tie, i, false);
        sides.entry((line.pair.1, route[last - 1].0 > route[last].0)).or_default().push(end);
    }
    for ((task, _), mut ends) in sides {
        ends.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
        let middle = (ends.len() - 1) as f64 / 2.0;
        let gap = if middle > 0.0 { END_GAP.min(END_SPAN / (2.0 * middle)) } else { 0.0 };
        for (k, &(_, _, _, i, start)) in ends.iter().enumerate() {
            let route = &mut lines[i].route;
            let at = if start { 0 } else { route.len() - 1 };
            route[at].1 = centre[task] + (k as f64 - middle) * gap;
        }
    }
}

/// What makes a task more urgent than another: its priority (P0 first, none after every
/// priority), then its title, regardless of case first, then its id.
fn urgency(task: &Task) -> (bool, i64, String, &str, &str) {
    (task.priority.is_none(), task.priority.unwrap_or(0), task.title.to_lowercase(), &task.title, &task.id)
}

/// The edges between numbered tasks, each pair once, turned where they close a loop so that none
/// is left: a search from each task in turn, most urgent first, along the edges to the tasks that
/// wait for it, turns any edge back to a task on the way it came. Each comes back `(left, right)`,
/// sorted.
fn without_loops(n: usize, mut pairs: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    pairs.sort_unstable();
    pairs.dedup();
    let mut next: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (e, &(a, _)) in pairs.iter().enumerate() {
        next[a].push(e);
    }
    // Where each task stands in the search: not reached, on the way, or done.
    let mut state = vec![0u8; n];
    let mut turned = vec![false; pairs.len()];
    for start in 0..n {
        if state[start] != 0 {
            continue;
        }
        state[start] = 1;
        let mut path = vec![(start, 0)];
        while let Some((a, i)) = path.last_mut() {
            let Some(&e) = next[*a].get(*i) else {
                state[*a] = 2;
                path.pop();
                continue;
            };
            *i += 1;
            let b = pairs[e].1;
            match state[b] {
                0 => {
                    state[b] = 1;
                    path.push((b, 0));
                }
                1 => turned[e] = true,
                _ => {}
            }
        }
    }
    let mut out: Vec<(usize, usize)> =
        pairs.iter().zip(&turned).map(|(&(a, b), &turn)| if turn { (b, a) } else { (a, b) }).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Each numbered task's column: the length of the longest chain of edges that leads to it.
fn columns_of(n: usize, pairs: &[(usize, usize)]) -> Vec<usize> {
    let mut waiting = vec![0usize; n];
    let mut next: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(a, b) in pairs {
        waiting[b] += 1;
        next[a].push(b);
    }
    let mut column = vec![0; n];
    let mut ready: Vec<usize> = (0..n).filter(|&t| waiting[t] == 0).collect();
    while let Some(a) = ready.pop() {
        for &b in &next[a] {
            column[b] = column[b].max(column[a] + 1);
            waiting[b] -= 1;
            if waiting[b] == 0 {
                ready.push(b);
            }
        }
    }
    column
}

/// The groups of numbered tasks that edges join to one another but to nothing else, the group
/// holding the most urgent task first, and the tasks with no edge, most urgent first.
fn groups_of(n: usize, pairs: &[(usize, usize)]) -> (Vec<Vec<usize>>, Vec<usize>) {
    // Each task's way to the most urgent task of its group, found by following `leader`.
    fn lead(leader: &mut [usize], t: usize) -> usize {
        let mut root = t;
        while leader[root] != root {
            root = leader[root];
        }
        let mut t = t;
        while leader[t] != root {
            let up = leader[t];
            leader[t] = root;
            t = up;
        }
        root
    }
    let mut leader: Vec<usize> = (0..n).collect();
    let mut joined = vec![false; n];
    for &(a, b) in pairs {
        let (ra, rb) = (lead(&mut leader, a), lead(&mut leader, b));
        leader[ra.max(rb)] = ra.min(rb);
        joined[a] = true;
        joined[b] = true;
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut edgeless = Vec::new();
    for (t, joined) in joined.into_iter().enumerate() {
        if joined {
            let root = lead(&mut leader, t);
            groups.entry(root).or_default().push(t);
        } else {
            edgeless.push(t);
        }
    }
    (groups.into_values().collect(), edgeless)
}

/// A group of tasks laid out on its own, with the waypoints of its long edges. Its nodes are its
/// tasks, then its waypoints.
struct Group {
    /// Per node: the number of the task it is, or none for a waypoint.
    task: Vec<Option<usize>>,
    /// Per node: its column.
    column: Vec<usize>,
    /// Per node: its neighbours in the column before its own, and in the column after.
    before: Vec<Vec<usize>>,
    after: Vec<Vec<usize>>,
    /// Per node: where it comes when nothing else decides, lowest first: a task by its rank in the
    /// chosen order, every waypoint after every task.
    tie: Vec<usize>,
    /// Per node: its tier (`Tiers`), the strip of the drawing it is kept in, a waypoint in the tier
    /// of the task its edge leads to. Every node of a column comes after every node of an earlier
    /// tier there. Without bands every node is in tier 0.
    tier: Vec<usize>,
    /// Per edge: its pair of numbered tasks, and the nodes it runs through from left to right.
    chains: Vec<((usize, usize), Vec<usize>)>,
}

impl Group {
    /// The group of the numbered tasks `members`, with a waypoint in each column an edge between
    /// two of them crosses: each task in the tier `tier_of` gives it, each waypoint in the tier
    /// `waypoint_tier` gives its edge's pair.
    fn new(
        members: &[usize],
        pairs: &[(usize, usize)],
        column: &[usize],
        rank: &[usize],
        tier_of: impl Fn(usize) -> usize,
        waypoint_tier: impl Fn((usize, usize)) -> usize,
    ) -> Group {
        let mut group = Group {
            task: Vec::new(),
            column: Vec::new(),
            before: Vec::new(),
            after: Vec::new(),
            tie: Vec::new(),
            tier: Vec::new(),
            chains: Vec::new(),
        };
        let mut node = vec![None; column.len()];
        for &t in members {
            node[t] = Some(group.add(Some(t), column[t], rank[t], tier_of(t)));
        }
        for &(a, b) in pairs {
            let (Some(from), Some(to)) = (node[a], node[b]) else { continue };
            let tie = rank.len() + group.chains.len();
            let mut chain = vec![from];
            for c in column[a] + 1..column[b] {
                chain.push(group.add(None, c, tie, waypoint_tier((a, b))));
            }
            chain.push(to);
            for link in chain.windows(2) {
                group.after[link[0]].push(link[1]);
                group.before[link[1]].push(link[0]);
            }
            group.chains.push(((a, b), chain));
        }
        group
    }

    fn add(&mut self, task: Option<usize>, column: usize, tie: usize, tier: usize) -> usize {
        self.task.push(task);
        self.column.push(column);
        self.before.push(Vec::new());
        self.after.push(Vec::new());
        self.tie.push(tie);
        self.tier.push(tier);
        self.task.len() - 1
    }

    /// Puts the group's nodes at `heights` in `placed`: each task's centre, the span each node
    /// takes in its column, and each edge's route.
    fn place(&self, heights: &[f64], placed: &mut Placed) {
        for (v, task) in self.task.iter().enumerate() {
            if let Some(t) = task {
                placed.centre[*t] = heights[v];
            }
            placed.taken[self.column[v]].push((heights[v] - self.half(v), heights[v] + self.half(v)));
        }
        for (pair, chain) in &self.chains {
            let (start, end) = (chain[0], chain[chain.len() - 1]);
            let mut route = vec![(x(self.column[start]) + W, heights[start])];
            for &waypoint in &chain[1..chain.len() - 1] {
                let left = x(self.column[waypoint]);
                route.push((left, heights[waypoint]));
                route.push((left + W, heights[waypoint]));
            }
            route.push((x(self.column[end]), heights[end]));
            placed.routes.insert(*pair, route);
        }
    }

    fn len(&self) -> usize {
        self.task.len()
    }

    /// Half a node's height: a task's box, or nothing for a waypoint, which is only a line.
    fn half(&self, v: usize) -> f64 {
        if self.task[v].is_some() {
            H / 2.0
        } else {
            0.0
        }
    }

    /// How far apart the centres of two neighbours in a column must be.
    fn spacing(&self, v: usize, w: usize) -> f64 {
        self.half(v) + self.half(w) + GY
    }

    /// The group's columns, each sorted by tier, then by `tie`: its tasks, and with `waypoints` its
    /// waypoints after them in each tier.
    fn columns_by_tie(&self, waypoints: bool) -> Vec<Vec<usize>> {
        let count = self.column.iter().max().map_or(0, |c| c + 1);
        let mut columns = vec![Vec::new(); count];
        for v in (0..self.len()).filter(|&v| waypoints || self.task[v].is_some()) {
            columns[self.column[v]].push(v);
        }
        for column in &mut columns {
            column.sort_by_key(|&v| (self.tier[v], self.tie[v]));
        }
        columns
    }

    /// The group's columns, each sorted by tier, then by `start`, a height per node, then by `tie`.
    fn columns_from(&self, start: &[f64]) -> Vec<Vec<usize>> {
        let mut columns = self.columns_by_tie(true);
        for column in &mut columns {
            column.sort_by(|&v, &w| {
                self.tier[v].cmp(&self.tier[w]).then(start[v].total_cmp(&start[w])).then(self.tie[v].cmp(&self.tie[w]))
            });
        }
        columns
    }

    /// Per tier, the least height its strip can have: the room its nodes need in the column where
    /// they need the most, each `spacing` from the next and a box's half beyond the first and the
    /// last.
    fn needs(&self, columns: &[Vec<usize>], tiers: usize) -> Vec<f64> {
        let mut needs = vec![0.0f64; tiers];
        for column in columns {
            for run in column.chunk_by(|&v, &w| self.tier[v] == self.tier[w]) {
                let between: f64 = run.windows(2).map(|pair| self.spacing(pair[0], pair[1])).sum();
                let need = self.half(run[0]) + between + self.half(run[run.len() - 1]);
                needs[self.tier[run[0]]] = needs[self.tier[run[0]]].max(need);
            }
        }
        needs
    }

    /// Each node's place in its column, counting from the top; a node in no column has none.
    fn places(&self, columns: &[Vec<usize>]) -> Vec<usize> {
        let mut place = vec![usize::MAX; self.len()];
        for column in columns {
            for (i, &v) in column.iter().enumerate() {
                place[v] = i;
            }
        }
        place
    }

    /// How many times edges cross, counted exactly: two edges between the same two neighbouring
    /// columns cross when the order of their ends in one column is the reverse of their order in
    /// the other.
    fn crossings(&self, columns: &[Vec<usize>]) -> usize {
        let place = self.places(columns);
        columns
            .windows(2)
            .map(|pair| {
                let mut ends: Vec<(usize, usize)> = pair[0]
                    .iter()
                    .flat_map(|&v| self.after[v].iter().map(move |&w| (v, w)))
                    .map(|(v, w)| (place[v], place[w]))
                    .collect();
                ends.sort_unstable();
                // Each edge crosses those counted before it whose end in the next column lies
                // below its own: a running count of the ends seen at each place there answers that.
                let size = pair[1].len();
                let mut seen = vec![0usize; size + 1];
                let mut crossings = 0;
                for (counted, &(_, end)) in ends.iter().enumerate() {
                    let mut at_or_above = 0;
                    let mut i = end + 1;
                    while i > 0 {
                        at_or_above += seen[i];
                        i &= i - 1;
                    }
                    crossings += counted - at_or_above;
                    let mut i = end + 1;
                    while i <= size {
                        seen[i] += 1;
                        // The lowest set bit of `i`: how many leaves this node of the tree counts.
                        i += i - (i & (i - 1));
                    }
                }
                crossings
            })
            .sum()
    }

    /// How many times the edges of `v` cross those of `w` when `v` sits just above `w` in a column.
    fn crossings_of_pair(&self, v: usize, w: usize, place: &[usize]) -> usize {
        let count = |upper: &[usize], lower: &[usize]| {
            upper.iter().map(|&a| lower.iter().filter(|&&b| place[a] > place[b]).count()).sum::<usize>()
        };
        count(&self.before[v], &self.before[w]) + count(&self.after[v], &self.after[w])
    }

    /// The automatic order: from the columns given, alternate passes sorting the columns by their
    /// neighbours' medians, each followed by swaps that remove crossings (in two passes of every
    /// four, swaps that keep the count too), keeping the order with the fewest crossings seen; then
    /// the ties settled. Nothing moves a node out of its tier.
    fn order_auto(&self, mut columns: Vec<Vec<usize>>) -> Vec<Vec<usize>> {
        let mut best = columns.clone();
        let mut fewest = self.crossings(&columns);
        for pass in 0..ORDER_PASSES {
            if fewest == 0 {
                break;
            }
            self.sort_by_medians(&mut columns, pass % 2 == 0);
            self.swap_while_fewer(&mut columns, pass % 4 < 2);
            let crossings = self.crossings(&columns);
            if crossings < fewest {
                fewest = crossings;
                best = columns.clone();
            }
        }
        self.settle_ties(&mut best);
        best
    }

    /// Sorts each tier of each column by the median place of its nodes' neighbours in the column
    /// before it, columns taken left to right (`rightward`), or in the column after it, right to
    /// left. A node with no neighbour there keeps its place; ties go by `tie`.
    fn sort_by_medians(&self, columns: &mut [Vec<usize>], rightward: bool) {
        let mut place = self.places(columns);
        let count = columns.len();
        let order: Vec<usize> =
            if rightward { (1..count).collect() } else { (0..count.saturating_sub(1)).rev().collect() };
        for c in order {
            let mut slots = Vec::new();
            let mut keyed = Vec::new();
            for (slot, &v) in columns[c].iter().enumerate() {
                let neighbours = if rightward { &self.before[v] } else { &self.after[v] };
                if neighbours.is_empty() {
                    continue;
                }
                let mut places: Vec<f64> = neighbours.iter().map(|&u| place[u] as f64).collect();
                places.sort_by(f64::total_cmp);
                slots.push(slot);
                keyed.push((self.tier[v], median(&places), self.tie[v], v));
            }
            // Sorting by tier first gives each tier the slots its nodes held.
            keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
            for (slot, (_, _, _, v)) in slots.into_iter().zip(keyed) {
                columns[c][slot] = v;
            }
            for (i, &v) in columns[c].iter().enumerate() {
                place[v] = i;
            }
        }
    }

    /// Swaps neighbours of one tier in each column while a swap removes crossings. A swap changes
    /// only the crossings between the two nodes' own edges, so each such swap lowers the count, and
    /// they stop. With `when_equal`, two neighbours whose edges cross also swap when that leaves the
    /// count as it was, which can open the way to an order the other swaps cannot reach; a pass of
    /// such swaps alone is not reason for another.
    fn swap_while_fewer(&self, columns: &mut [Vec<usize>], when_equal: bool) {
        let mut place = self.places(columns);
        loop {
            let mut fewer = false;
            for column in columns.iter_mut() {
                for i in 1..column.len() {
                    let (v, w) = (column[i - 1], column[i]);
                    if self.tier[v] != self.tier[w] {
                        continue;
                    }
                    let (now, swapped) = (self.crossings_of_pair(v, w, &place), self.crossings_of_pair(w, v, &place));
                    if swapped < now || (when_equal && now > 0 && swapped == now) {
                        column.swap(i - 1, i);
                        place[v] = i;
                        place[w] = i - 1;
                        fewer |= swapped < now;
                    }
                }
            }
            if !fewer {
                break;
            }
        }
    }

    /// Where two neighbouring tasks of one tier in a column could trade places without changing how
    /// many times edges cross, the one that comes first by `tie` goes above.
    fn settle_ties(&self, columns: &mut [Vec<usize>]) {
        let mut place = self.places(columns);
        for column in columns.iter_mut() {
            loop {
                let mut swapped = false;
                for i in 1..column.len() {
                    let (v, w) = (column[i - 1], column[i]);
                    if self.task[v].is_some()
                        && self.task[w].is_some()
                        && self.tier[v] == self.tier[w]
                        && self.tie[w] < self.tie[v]
                        && self.crossings_of_pair(w, v, &place) == self.crossings_of_pair(v, w, &place)
                    {
                        column.swap(i - 1, i);
                        place[v] = i;
                        place[w] = i - 1;
                        swapped = true;
                    }
                }
                if !swapped {
                    break;
                }
            }
        }
    }

    /// The priority and status orders: each column's tasks by tier, then by `tie`, and each
    /// waypoint put, column by column from the left, where its edge crosses fewest edges to the
    /// column before among the nodes of its tier; then each waypoint put again, weighing its edges
    /// to both neighbouring columns, while that removes crossings.
    fn order_fixed(&self) -> Vec<Vec<usize>> {
        let mut columns = self.columns_by_tie(false);
        let mut waypoints: Vec<usize> = (0..self.len()).filter(|&v| self.task[v].is_none()).collect();
        waypoints.sort_by_key(|&v| (self.column[v], self.tie[v]));
        for &w in &waypoints {
            let place = self.places(&columns);
            let c = self.column[w];
            let gap = self.best_gap(&columns[c], w, &place, false);
            columns[c].insert(gap, w);
        }
        for _ in 0..ORDER_PASSES {
            let crossings = self.crossings(&columns);
            for &w in &waypoints {
                let c = self.column[w];
                columns[c].retain(|&v| v != w);
                let place = self.places(&columns);
                let gap = self.best_gap(&columns[c], w, &place, true);
                columns[c].insert(gap, w);
            }
            if self.crossings(&columns) >= crossings {
                break;
            }
        }
        columns
    }

    /// Where in `column`, which `w` is not in, the waypoint `w` crosses fewest edges: the edges to
    /// the column before, and with `both` the edges to the column after too. Between equally good
    /// places, the one nearest its neighbours' places, then the higher. Only places among the nodes
    /// of its tier count.
    fn best_gap(&self, column: &[usize], w: usize, place: &[usize], both: bool) -> usize {
        let m = column.len();
        let first = column.iter().take_while(|&&v| self.tier[v] < self.tier[w]).count();
        let last = column.iter().take_while(|&&v| self.tier[v] <= self.tier[w]).count();
        // How many crossings each place (0 above the first node, `m` below the last) costs, kept as
        // the change from one place to the next.
        let mut change = vec![0i64; m + 2];
        let mut neighbours = Vec::new();
        let sides = [(&self.before[w], true), (&self.after[w], false)];
        for (mine, before) in sides.into_iter().filter(|(_, before)| *before || both) {
            for &p in mine {
                neighbours.push(place[p]);
                for (i, &v) in column.iter().enumerate() {
                    let theirs = if before { &self.before[v] } else { &self.after[v] };
                    for &u in theirs {
                        match place[u].cmp(&place[p]) {
                            // Their other end lies above ours: they cross when `v` lies below `w`.
                            Ordering::Less => {
                                change[0] += 1;
                                change[i + 1] -= 1;
                            }
                            // It lies below ours: they cross when `v` lies above `w`.
                            Ordering::Greater => {
                                change[i + 1] += 1;
                                change[m + 1] -= 1;
                            }
                            Ordering::Equal => {}
                        }
                    }
                }
            }
        }
        let aim =
            if neighbours.is_empty() { 0.0 } else { neighbours.iter().sum::<usize>() as f64 / neighbours.len() as f64 };
        let mut cost = 0;
        let mut best = (i64::MAX, f64::INFINITY, 0);
        for (gap, step) in change.iter().take(m + 1).enumerate() {
            cost += step;
            let distance = (gap as f64 - aim).abs();
            if (first..=last).contains(&gap) && (cost < best.0 || (cost == best.0 && distance < best.1)) {
                best = (cost, distance, gap);
            }
        }
        best.2
    }

    /// Each node's height (its centre) with each tier of each column packed from the top of its
    /// strip in `strips` (from 0 for a strip unbounded above).
    fn packed(&self, columns: &[Vec<usize>], strips: &[(f64, f64)]) -> Vec<f64> {
        let mut height = vec![0.0; self.len()];
        for column in columns {
            for run in column.chunk_by(|&v, &w| self.tier[v] == self.tier[w]) {
                let top = strips[self.tier[run[0]]].0;
                let mut at = if top.is_finite() { top } else { 0.0 };
                for (i, &v) in run.iter().enumerate() {
                    at = if i == 0 { at + self.half(v) } else { at + self.spacing(run[i - 1], v) };
                    height[v] = at;
                }
            }
        }
        height
    }

    /// Each node's height with the columns pulled toward their neighbours: left to right toward
    /// the column before, then right to left toward the column after, a few times over, and a last
    /// time left to right, so that a task lines up with what it waits for. Each tier stays within
    /// its strip in `strips`.
    fn pulled(&self, columns: &[Vec<usize>], strips: &[(f64, f64)]) -> Vec<f64> {
        let mut height = self.packed(columns, strips);
        for _ in 0..HEIGHT_PASSES {
            for column in columns.iter().skip(1) {
                self.pull(column, &mut height, true, strips);
            }
            for column in columns.iter().rev().skip(1) {
                self.pull(column, &mut height, false, strips);
            }
        }
        for column in columns.iter().skip(1) {
            self.pull(column, &mut height, true, strips);
        }
        height
    }

    /// Moves each tier of one column's nodes, keeping their order and spacing, as near as they can
    /// go to where their neighbours in the column before (`leftward`) or after pull them, within
    /// the tier's strip in `strips`: each toward the median height of those neighbours, a node with
    /// none staying where it is. Nodes that want the same room share the difference, a node pulling
    /// harder the more neighbours it has there, and a waypoint hardest.
    fn pull(&self, column: &[usize], height: &mut [f64], leftward: bool, strips: &[(f64, f64)]) {
        for run in column.chunk_by(|&v, &w| self.tier[v] == self.tier[w]) {
            // The least distance from the run's first node to each node.
            let mut offset = Vec::with_capacity(run.len());
            let mut at = 0.0;
            for (i, &v) in run.iter().enumerate() {
                if i > 0 {
                    at += self.spacing(run[i - 1], v);
                }
                offset.push(at);
            }
            let mut wants = Vec::with_capacity(run.len());
            let mut weights = Vec::with_capacity(run.len());
            for (i, &v) in run.iter().enumerate() {
                let neighbours = if leftward { &self.before[v] } else { &self.after[v] };
                let (want, weight) = if neighbours.is_empty() {
                    (height[v], 1.0)
                } else {
                    let mut heights: Vec<f64> = neighbours.iter().map(|&u| height[u]).collect();
                    heights.sort_by(f64::total_cmp);
                    let mid = heights.len() / 2;
                    let median =
                        if heights.len() % 2 == 1 { heights[mid] } else { (heights[mid - 1] + heights[mid]) / 2.0 };
                    (median, if self.task[v].is_some() { neighbours.len() as f64 } else { WAYPOINT_PULL })
                };
                wants.push(want - offset[i]);
                weights.push(weight);
            }
            // The run's first node may come no higher than the strip's top allows, and its last no
            // lower than its bottom: clamping the run's least-squares fit to those bounds gives the
            // fit that keeps to them.
            let (top, bottom) = strips[self.tier[run[0]]];
            let highest = top + self.half(run[0]);
            let lowest = (bottom - self.half(run[run.len() - 1]) - offset[run.len() - 1]).max(highest);
            for ((&v, at), offset) in run.iter().zip(nondecreasing(&wants, &weights)).zip(offset) {
                height[v] = at.clamp(highest, lowest) + offset;
            }
        }
    }
}

/// The median of `places`, sorted, as the ordering weighs it: the middle one; between the two
/// middle ones for two; and for more, between the two middle ones, nearer the one on the side where
/// the places are packed closer.
fn median(places: &[f64]) -> f64 {
    let n = places.len();
    let m = n / 2;
    if n % 2 == 1 {
        return places[m];
    }
    if n == 2 {
        return (places[0] + places[1]) / 2.0;
    }
    let left = places[m - 1] - places[0];
    let right = places[n - 1] - places[m];
    if left + right == 0.0 {
        return (places[m - 1] + places[m]) / 2.0;
    }
    (places[m - 1] * right + places[m] * left) / (left + right)
}

/// The nondecreasing sequence nearest `wants`, each weighted by `weights` (least squares): runs
/// that are out of order are pooled at their weighted mean, and pools merge until none are.
fn nondecreasing(wants: &[f64], weights: &[f64]) -> Vec<f64> {
    let mut pools: Vec<(f64, f64, usize)> = Vec::new();
    for (&want, &weight) in wants.iter().zip(weights) {
        let (mut mean, mut total, mut size) = (want, weight, 1);
        while let Some(&(last_mean, last_total, last_size)) = pools.last() {
            if last_mean <= mean {
                break;
            }
            pools.pop();
            mean = (last_mean * last_total + mean * total) / (last_total + total);
            total += last_total;
            size += last_size;
        }
        pools.push((mean, total, size));
    }
    pools.into_iter().flat_map(|(mean, _, size)| std::iter::repeat_n(mean, size)).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::fmt::status_rank;
    use fwd_core::model::TaskStatus;

    /// The layout made and drawn in one step.
    pub(crate) fn layout(
        tasks: &[Task],
        bands: &[Band],
        edges: &[(usize, usize)],
        threaded: &[(usize, usize)],
        order: Order,
    ) -> Drawing {
        Layout::new(tasks, bands, edges, order).draw(threaded)
    }

    #[test]
    fn drawing_a_layout_again_with_lines_threaded_through_it_moves_no_box_and_no_band() {
        // "b" and "c" wait for "a", "d" for both, and "a" and "b" are in a band; "a" to "d" is
        // threaded through.
        let mut tasks: Vec<Task> = ["a", "b", "c", "d"].map(open).into();
        (tasks[0].band, tasks[1].band) = (Some(0), Some(0));
        let bands = [Band { within: None, id: "p".into() }];
        let edges = [(0, 1), (0, 2), (1, 3), (2, 3)];
        let laid = Layout::new(&tasks, &bands, &edges, Order::Auto);
        let (plain, threaded) = (laid.draw(&[]), laid.draw(&[(0, 3)]));
        assert_eq!((&threaded.boxes, &threaded.bands), (&plain.boxes, &plain.bands));
        assert_eq!(threaded.threaded.len(), 1);
        assert_eq!(plain, layout(&tasks, &bands, &edges, &[], Order::Auto));
    }

    fn task(title: &str, priority: Option<i64>, status: TaskStatus) -> Task {
        Task { title: title.into(), id: format!("id-{title}"), priority, status: status_rank(status), band: None }
    }

    fn open(title: &str) -> Task {
        task(title, None, TaskStatus::Open)
    }

    /// This project's graph on 2026-09-28 (`tests/graph-fixture-2026-09-28.json`): every task not
    /// closed, and the dependency and subtask edges among them as `(first, then)`, as the page drew
    /// them until parents became bands (`graph.rs`).
    fn fixture() -> (Vec<Task>, Vec<(usize, usize)>) {
        let json: serde_json::Value =
            serde_json::from_str(include_str!("../tests/graph-fixture-2026-09-28.json")).expect("the fixture parses");
        let text = |value: &serde_json::Value| value.as_str().expect("a string").to_string();
        let tasks: Vec<Task> = json["tasks"]
            .as_array()
            .expect("tasks")
            .iter()
            .map(|t| Task {
                title: text(&t["title"]),
                id: text(&t["id"]),
                priority: t["priority"].as_i64(),
                status: status_rank(text(&t["status"]).parse().expect("a status")),
                band: None,
            })
            .collect();
        let index = |id: String| tasks.iter().position(|t| t.id == id).expect("an edge joins two tasks of the fixture");
        let edges = json["edges"]
            .as_array()
            .expect("edges")
            .iter()
            .map(|e| {
                let (from, to) = (index(text(&e["from"])), index(text(&e["to"])));
                match e["kind"].as_str() {
                    Some("depends_on") => (to, from),
                    Some("subtask_of") => (from, to),
                    kind => panic!("an edge of kind {kind:?}"),
                }
            })
            .collect();
        (tasks, edges)
    }

    /// Where a route is at `x`, as the page draws it (`Drawing::routes`): level between two points
    /// at one height, and between two at different heights the curve whose control points both lie
    /// halfway across, one at each height.
    fn height_at(route: &[(f64, f64)], x: f64) -> Option<f64> {
        route.windows(2).find_map(|piece| {
            let ((x0, y0), (x1, y1)) = (piece[0], piece[1]);
            if x < x0.min(x1) || x > x0.max(x1) || x0 == x1 {
                return None;
            }
            if y0 == y1 {
                return Some(y0);
            }
            // Across, such a curve is at x0 + (x1 - x0)(1.5t - 1.5t² + t³), which only grows.
            let share = (x - x0) / (x1 - x0);
            let (mut low, mut high) = (0.0f64, 1.0f64);
            for _ in 0..60 {
                let t = (low + high) / 2.0;
                if 1.5 * t - 1.5 * t * t + t * t * t < share {
                    low = t;
                } else {
                    high = t;
                }
            }
            let t = (low + high) / 2.0;
            Some(y0 + (y1 - y0) * (3.0 * t * t - 2.0 * t * t * t))
        })
    }

    /// Each route's height every quarter pixel across the drawing, where it has one.
    fn traced(routes: &[Vec<(f64, f64)>], width: f64) -> Vec<Vec<Option<f64>>> {
        let steps = (width * 4.0) as usize + 1;
        routes.iter().map(|route| (0..steps).map(|k| height_at(route, k as f64 / 4.0)).collect()).collect()
    }

    /// How many times the drawn edges cross: each time one passes from above another to below it,
    /// where both are drawn. Two edges that meet at a task they share touch there, which is no
    /// crossing.
    pub(crate) fn crossings_drawn(routes: &[Vec<(f64, f64)>], width: f64) -> usize {
        let traces = traced(routes, width);
        let mut crossings = 0;
        for (i, a) in traces.iter().enumerate() {
            for b in &traces[i + 1..] {
                let mut side = 0.0;
                for (ya, yb) in a.iter().zip(b) {
                    let (Some(ya), Some(yb)) = (ya, yb) else { continue };
                    let apart = ya - yb;
                    if apart.abs() < 1e-6 {
                        continue;
                    }
                    if side != 0.0 && apart.signum() != side {
                        crossings += 1;
                    }
                    side = apart.signum();
                }
            }
        }
        crossings
    }

    /// How many times an edge is drawn across the box of a task it does not end at.
    pub(crate) fn edges_across_boxes(
        boxes: &[(f64, f64)],
        routes: &[Vec<(f64, f64)>],
        edges: &[(usize, usize)],
    ) -> usize {
        edges
            .iter()
            .zip(routes)
            .map(|(&(first, then), route)| {
                let across = |&(t, &(bx, by)): &(usize, &(f64, f64))| {
                    t != first
                        && t != then
                        && (0..=(W as usize))
                            .any(|dx| height_at(route, bx + dx as f64).is_some_and(|y| y > by && y < by + H))
                };
                boxes.iter().enumerate().filter(across).count()
            })
            .sum()
    }

    /// The titles in each column of a drawing, top to bottom.
    fn columns(tasks: &[Task], drawing: &Drawing) -> Vec<Vec<String>> {
        let mut by_column: BTreeMap<i64, Vec<(f64, &str)>> = BTreeMap::new();
        for (t, &(x, y)) in tasks.iter().zip(&drawing.boxes) {
            by_column.entry(x as i64).or_default().push((y, &t.title));
        }
        by_column
            .into_values()
            .map(|mut column| {
                column.sort_by(|a, b| a.0.total_cmp(&b.0));
                column.into_iter().map(|(_, title)| title.to_string()).collect()
            })
            .collect()
    }

    /// How many crossings the page's layout before this module (`graph.rs` at 40ac460) drew on the
    /// fixture, counted by `crossings_drawn`. It also drew seven edges across the box of a task they
    /// do not end at, counted by `edges_across_boxes`.
    const CROSSINGS_BEFORE: usize = 10;

    #[test]
    fn the_fixture_is_drawn_without_the_crossings_it_had_and_with_no_edge_across_a_task() {
        let (tasks, edges) = fixture();
        let drawing = layout(&tasks, &[], &edges, &[], Order::Auto);
        let crossings = crossings_drawn(&drawing.routes, drawing.width);
        assert!(crossings < CROSSINGS_BEFORE);
        assert_eq!(crossings, 0);
        assert_eq!(edges_across_boxes(&drawing.boxes, &drawing.routes, &edges), 0);
        // Every edge is drawn, from the right side of the task waited for to the left side of the
        // task that waits, which sits to the right of it.
        for (&(first, then), route) in edges.iter().zip(&drawing.routes) {
            assert!(drawing.boxes[then].0 > drawing.boxes[first].0);
            assert_eq!(route.first().map(|p| p.0), Some(drawing.boxes[first].0 + W));
            assert_eq!(route.last().map(|p| p.0), Some(drawing.boxes[then].0));
        }
        // The orders that sort each column and leave the crossings be cross more, and still draw no
        // edge across a task.
        for (order, crossings) in [(Order::Priority, 11), (Order::Status, 14)] {
            let drawing = layout(&tasks, &[], &edges, &[], order);
            assert_eq!(crossings_drawn(&drawing.routes, drawing.width), crossings, "{order:?}");
            assert_eq!(edges_across_boxes(&drawing.boxes, &drawing.routes, &edges), 0, "{order:?}");
        }
    }

    #[test]
    fn two_columns_that_can_be_drawn_without_a_crossing_are() {
        let tasks: Vec<Task> = ["a", "b", "c", "d", "w", "x", "y", "z"].map(open).into();
        // In title order every edge crosses every other.
        let edges = [(0, 7), (1, 6), (2, 5), (3, 4)];
        let drawing = layout(&tasks, &[], &edges, &[], Order::Auto);
        assert_eq!(crossings_drawn(&drawing.routes, drawing.width), 0);
        assert_eq!(columns(&tasks, &drawing), [["a", "b", "c", "d"], ["z", "y", "x", "w"]]);
        // A task with two neighbours, and two that share one.
        let edges = [(0, 6), (0, 7), (1, 5), (2, 5), (3, 4)];
        let drawing = layout(&tasks, &[], &edges, &[], Order::Auto);
        assert_eq!(crossings_drawn(&drawing.routes, drawing.width), 0);
        assert_eq!(columns(&tasks, &drawing), [["a", "b", "c", "d"], ["y", "z", "x", "w"]]);
    }

    #[test]
    fn a_long_edge_runs_level_between_the_tasks_of_the_column_it_crosses() {
        // Two chains, "m" then "n" then "o", and "z" then "y" then "x"; "a" is waited for by "n"
        // and by "y", so it sits between "m" and "z"; and "b" waits for "n" and for "a", two
        // columns back. The edge from "a" to "b" crosses no other only between "n" and "y".
        let tasks: Vec<Task> = ["a", "b", "m", "n", "o", "x", "y", "z"].map(open).into();
        let (a, b, m, n, o, x, y, z) = (0, 1, 2, 3, 4, 5, 6, 7);
        let edges = [(m, n), (n, o), (z, y), (y, x), (a, n), (a, y), (n, b), (a, b)];
        let drawing = layout(&tasks, &[], &edges, &[], Order::Auto);
        assert_eq!(crossings_drawn(&drawing.routes, drawing.width), 0);
        assert_eq!(edges_across_boxes(&drawing.boxes, &drawing.routes, &edges), 0);
        let centre = |t: usize| drawing.boxes[t].1 + H / 2.0;
        let between = |v: f64, s: usize, t: usize| v > centre(s).min(centre(t)) && v < centre(s).max(centre(t));
        assert!(between(centre(a), m, z) && between(centre(b), o, x));
        let route = &drawing.routes[7];
        let middle = PAD + W + GX;
        assert_eq!(route.len(), 4, "{route:?}");
        let ((left, level), (right, still)) = (route[1], route[2]);
        assert_eq!((left, right, still), (middle, middle + W, level), "it runs level across the middle column");
        let (upper, lower) = if centre(n) < centre(y) { (n, y) } else { (y, n) };
        assert!(level > drawing.boxes[upper].1 + H && level < drawing.boxes[lower].1, "between n and y: {level}");
    }

    #[test]
    fn a_graph_is_drawn_the_same_way_every_time_whatever_order_its_tasks_arrive_in() {
        let (tasks, edges) = fixture();
        let last = tasks.len() - 1;
        let backwards: Vec<Task> = tasks.iter().rev().cloned().collect();
        let flipped: Vec<(usize, usize)> = edges.iter().rev().map(|&(a, b)| (last - a, last - b)).collect();
        // Lines threaded between tasks of each group, which may want the same gaps.
        let threaded = [edges[0], edges[edges.len() - 1], (edges[1].0, edges[2].1), (edges[3].1, edges[4].0)];
        let threaded_flipped: Vec<(usize, usize)> = threaded.iter().rev().map(|&(a, b)| (last - a, last - b)).collect();
        for order in Order::ALL {
            let drawing = layout(&tasks, &[], &edges, &threaded, order);
            assert_eq!(layout(&tasks, &[], &edges, &threaded, order), drawing, "{order:?}, twice");
            let other = layout(&backwards, &[], &flipped, &threaded_flipped, order);
            assert_eq!(other.boxes.into_iter().rev().collect::<Vec<_>>(), drawing.boxes, "{order:?}, backwards");
            assert_eq!(other.routes.into_iter().rev().collect::<Vec<_>>(), drawing.routes, "{order:?}, backwards");
            assert_eq!(other.threaded.into_iter().rev().collect::<Vec<_>>(), drawing.threaded, "{order:?}, backwards");
        }
    }

    #[test]
    fn tasks_that_could_trade_places_go_by_priority_then_title_then_id() {
        // Four tasks wait for "p" and for nothing else, so no order of them crosses less than another.
        let mut tasks = vec![open("p"), task("c", Some(2), TaskStatus::Open), open("a")];
        tasks.push(Task { id: "2".into(), ..task("b", Some(1), TaskStatus::Open) });
        tasks.push(Task { id: "1".into(), ..task("b", Some(1), TaskStatus::Open) });
        let edges = [(0, 1), (0, 2), (0, 3), (0, 4)];
        let drawing = layout(&tasks, &[], &edges, &[], Order::Auto);
        let from_the_top: Vec<usize> = {
            let mut waiting = vec![1, 2, 3, 4];
            waiting.sort_by(|&s, &t| drawing.boxes[s].1.total_cmp(&drawing.boxes[t].1));
            waiting
        };
        assert_eq!(from_the_top, [4, 3, 1, 2]);
    }

    #[test]
    fn the_priority_order_sorts_each_column_by_priority_then_title_and_moves_no_task() {
        use TaskStatus::Open;
        let tasks = vec![
            task("delta", Some(1), Open),
            task("alpha", Some(2), Open),
            task("bravo", None, Open),
            task("charlie", Some(0), Open),
            task("echo", Some(0), Open),
            task("golf", Some(3), Open),
            task("foxtrot", Some(3), Open),
            task("hotel", Some(0), Open),
            task("india", Some(2), Open),
        ];
        // "golf" waits for "delta", "foxtrot" for "alpha", "hotel" for "bravo", and "india" for
        // "hotel" and for "charlie", two columns back; nothing joins "echo".
        let edges = [(0, 5), (1, 6), (2, 7), (7, 8), (3, 8)];
        let drawing = layout(&tasks, &[], &edges, &[], Order::Priority);
        assert_eq!(
            columns(&tasks, &drawing),
            [vec!["charlie", "echo", "delta", "alpha", "bravo"], vec!["hotel", "foxtrot", "golf"], vec!["india"]]
        );
        // The edge from "charlie" passes above "hotel", where it crosses no other: every crossing
        // left is one the order of the tasks makes.
        let level = drawing.routes[4][1].1;
        assert!(level < drawing.boxes[7].1, "{level}");
        let others: Vec<Vec<(f64, f64)>> = drawing.routes[..4].to_vec();
        assert_eq!(crossings_drawn(&drawing.routes, drawing.width), crossings_drawn(&others, drawing.width));
    }

    #[test]
    fn the_status_order_sorts_each_column_by_working_order_then_priority_then_title() {
        use TaskStatus::*;
        let tasks = vec![
            task("a", Some(1), Open),
            task("b", Some(3), InProgress),
            task("c", Some(0), Proposed),
            task("d", Some(2), InReview),
            task("e", Some(0), Done),
            task("f", Some(0), Open),
            task("g", Some(0), Open),
            task("h", None, Open),
            task("i", Some(0), Cancelled),
            task("j", Some(0), Rejected),
            task("k", Some(1), Open),
        ];
        // "k" waits for "a" and for "c".
        let drawing = layout(&tasks, &[], &[(0, 10), (2, 10)], &[], Order::Status);
        assert_eq!(columns(&tasks, &drawing), [vec!["b", "d", "f", "g", "a", "h", "c", "e", "i", "j"], vec!["k"]]);
    }

    #[test]
    fn groups_are_stacked_most_urgent_first_and_tasks_with_no_edge_go_in_a_stack_below() {
        use TaskStatus::Open;
        let tasks = vec![
            task("x1", Some(2), Open),
            task("x2", Some(2), Open),
            task("y1", Some(0), Open),
            task("y2", Some(3), Open),
            task("y3", Some(3), Open),
            task("z1", Some(1), Open),
            task("z2", None, Open),
        ];
        // "x2" waits for "x1"; "y2" and "y3" wait for "y1"; nothing joins "z1" and "z2".
        let drawing = layout(&tasks, &[], &[(0, 1), (2, 3), (2, 4)], &[], Order::Auto);
        let (top, bottom) = (|t: usize| drawing.boxes[t].1, |t: usize| drawing.boxes[t].1 + H);
        // The group of "y1", the most urgent task, comes first, wholly above the other.
        assert!([2, 3, 4].into_iter().all(|y| [0, 1].into_iter().all(|x| bottom(y) < top(x))));
        // The stack is in the first column, below everything, most urgent first.
        assert!((0..5).all(|t| bottom(t) < top(5)) && bottom(5) < top(6));
        assert_eq!((drawing.boxes[5].0, drawing.boxes[6].0), (PAD, PAD));
    }

    #[test]
    fn a_loop_of_proposed_relations_is_drawn_with_one_edge_against_the_flow() {
        let tasks: Vec<Task> = ["a", "b", "c", "d"].map(open).into();
        // "b" waits for "a", "c" for "b", "a" for "c" (a proposal can say so), and "d" for "a".
        let edges = [(0, 1), (1, 2), (2, 0), (0, 3)];
        let drawing = layout(&tasks, &[], &edges, &[], Order::Auto);
        let against: Vec<usize> =
            (0..edges.len()).filter(|&e| drawing.boxes[edges[e].1].0 <= drawing.boxes[edges[e].0].0).collect();
        assert_eq!(against, [2]);
        // It still runs from the task waited for to the one that waits: from the left side of "c"
        // to the right side of "a".
        let route = &drawing.routes[2];
        assert_eq!(route.first().map(|p| p.0), Some(drawing.boxes[2].0));
        assert_eq!(route.last().map(|p| p.0), Some(drawing.boxes[0].0 + W));
        assert_eq!(crossings_drawn(&drawing.routes, drawing.width), 0);
    }

    #[test]
    fn an_edge_to_no_task_or_to_its_own_task_has_no_route() {
        let tasks: Vec<Task> = ["a", "b"].map(open).into();
        let drawing = layout(&tasks, &[], &[(0, 1), (0, 0), (1, 5)], &[], Order::Auto);
        assert_eq!(drawing.routes.iter().map(Vec::len).collect::<Vec<_>>(), [2, 0, 0]);
        let empty = layout(&[], &[], &[], &[], Order::Auto);
        assert_eq!((empty.width, empty.height), (W + PAD * 2.0, H + PAD * 2.0));
    }

    #[test]
    fn an_order_reads_back_by_its_name_and_nothing_else_names_one() {
        for order in Order::ALL {
            assert_eq!(Order::named(order.name()), Some(order));
        }
        assert_eq!(Order::default(), Order::Auto);
        for name in ["", "Auto", "crossings", "status "] {
            assert_eq!(Order::named(name), None, "{name:?}");
        }
    }

    /// Whether `heights` are `expected`, to within rounding.
    fn about(heights: &[f64], expected: &[f64]) -> bool {
        heights.len() == expected.len() && heights.iter().zip(expected).all(|(a, b)| (a - b).abs() < 1e-9)
    }

    #[test]
    fn each_line_meets_a_task_at_a_point_of_its_own_in_the_order_it_goes() {
        // "b", "c" and "d" wait for "a", and "e" waits for all three.
        let tasks: Vec<Task> = ["a", "b", "c", "d", "e"].map(open).into();
        let edges = [(0, 1), (0, 2), (0, 3), (1, 4), (2, 4), (3, 4)];
        let drawing = layout(&tasks, &[], &edges, &[], Order::Auto);
        let middle = |t: usize| drawing.boxes[t].1 + H / 2.0;
        let mut from_the_top = [1, 2, 3];
        from_the_top.sort_by(|&s, &t| middle(s).total_cmp(&middle(t)));
        // The three leave "a" `END_GAP` apart about the middle of its side, the highest to the
        // highest of them, and reach "e" the same way.
        let leaving: Vec<f64> = from_the_top.iter().map(|&t| drawing.routes[t - 1][0].1 - middle(0)).collect();
        assert!(about(&leaving, &[-END_GAP, 0.0, END_GAP]), "{leaving:?}");
        let reaching: Vec<f64> = from_the_top.iter().map(|&t| drawing.routes[t + 2][1].1 - middle(4)).collect();
        assert!(about(&reaching, &[-END_GAP, 0.0, END_GAP]), "{reaching:?}");
        assert_eq!(crossings_drawn(&drawing.routes, drawing.width), 0);
        // Nine lines leaving one task share the span of its side.
        let tasks: Vec<Task> = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"].map(open).into();
        let edges: Vec<(usize, usize)> = (1..10).map(|t| (0, t)).collect();
        let drawing = layout(&tasks, &[], &edges, &[], Order::Auto);
        let mut leaving: Vec<f64> =
            drawing.routes.iter().map(|route| route[0].1 - drawing.boxes[0].1 - H / 2.0).collect();
        leaving.sort_by(f64::total_cmp);
        let spread: Vec<f64> = (0..9).map(|k| (k as f64 - 4.0) * END_SPAN / 8.0).collect();
        assert!(about(&leaving, &spread), "{leaving:?}");
        assert_eq!(crossings_drawn(&drawing.routes, drawing.width), 0);
    }

    #[test]
    fn a_threaded_line_moves_nothing_and_runs_level_through_each_column_clear_of_what_is_there() {
        // "a" then "b" then "c"; "m" then "n" then "o" and "x" then "y" then "z" beside them, and
        // "z" waits for "m" too, its edge passing the middle column through a waypoint. A line is
        // threaded from "a" to "c".
        let tasks: Vec<Task> = ["a", "b", "c", "m", "n", "o", "x", "y", "z"].map(open).into();
        let edges = [(0, 1), (1, 2), (3, 4), (4, 5), (6, 7), (7, 8), (3, 8)];
        let plain = layout(&tasks, &[], &edges, &[], Order::Auto);
        let drawing = layout(&tasks, &[], &edges, &[(0, 2)], Order::Auto);
        assert_eq!(drawing.boxes, plain.boxes);
        // The edges meeting neither the side of "a" nor the side of "c" it meets are as they were.
        assert_eq!(drawing.routes[2..], plain.routes[2..]);
        let route = &drawing.threaded[0];
        assert_eq!(route.len(), 4, "{route:?}");
        assert_eq!((route[0].0, route[3].0), (drawing.boxes[0].0 + W, drawing.boxes[2].0));
        let middle = PAD + W + GX;
        let ((left, level), (right, still)) = (route[1], route[2]);
        assert_eq!((left, right, still), (middle, middle + W, level), "it runs level across the middle column");
        for &(_, y) in drawing.boxes.iter().filter(|b| b.0 == middle) {
            assert!(level <= y - CLEARANCE + 1e-9 || level >= y + H + CLEARANCE - 1e-9, "{level} beside {y}");
        }
        let waypoint = drawing.routes[6][1].1;
        assert!((level - waypoint).abs() >= CLEARANCE - 1e-9, "{level} beside the waypoint at {waypoint}");
        assert_eq!(ends_shared(&drawing, &edges, &[(0, 2)]), 0);
    }

    #[test]
    fn a_threaded_line_meets_the_sides_its_tasks_face_each_other_by() {
        // "b" and "c" wait for "a"; "d" waits for "c".
        let tasks: Vec<Task> = ["a", "b", "c", "d"].map(open).into();
        let edges = [(0, 1), (0, 2), (2, 3)];
        // From "d" back to "a", against the flow: from the left side of "d" to the right of "a".
        let drawing = layout(&tasks, &[], &edges, &[(3, 0), (1, 2)], Order::Auto);
        let back = &drawing.threaded[0];
        assert_eq!((back[0].0, back[back.len() - 1].0), (drawing.boxes[3].0, drawing.boxes[0].0 + W));
        // From "b" to "c", in one column: out of the right side of "b" into the gap after it, and
        // back into the right side of "c".
        let side = drawing.boxes[1].0 + W;
        let across: Vec<f64> = drawing.threaded[1].iter().map(|p| p.0).collect();
        assert_eq!(across, [side, side + LOOP_OUT, side + LOOP_OUT, side]);
        let (start, end) = (drawing.threaded[1][0].1, drawing.threaded[1][3].1);
        assert!((start - drawing.boxes[1].1 - H / 2.0).abs() <= END_SPAN / 2.0);
        assert!((end - drawing.boxes[2].1 - H / 2.0).abs() <= END_SPAN / 2.0);
        assert_eq!(ends_shared(&drawing, &edges, &[(3, 0), (1, 2)]), 0);
    }

    #[test]
    fn a_threaded_line_passes_between_two_things_only_through_a_gap_the_layout_left_whole() {
        // Two boxes the least gap apart, and a line well below them.
        let taken = [(0.0, H), (H + GY, 2.0 * H + GY), (2.0 * H + 2.0 * GY + 30.0, 2.0 * H + 2.0 * GY + 30.0)];
        // Aimed into the gap between the boxes, the line takes its middle, the one height there.
        assert_eq!(free_height(&taken, H + 3.0), H + CLEARANCE);
        // Aimed into a box, it takes the nearest height clear of it: here above everything.
        assert_eq!(free_height(&taken, 10.0), -CLEARANCE);
        // Aimed into a wider gap, it runs where it is aimed.
        assert_eq!(free_height(&taken, 150.0), 150.0);
        // A line threaded through the least gap closes it to another.
        let closed = [(0.0, H), (H + CLEARANCE, H + CLEARANCE), (H + GY, 2.0 * H + GY)];
        assert_eq!(free_height(&closed, H + 3.0), -CLEARANCE);
        assert_eq!(free_height(&[], 42.0), 42.0);
    }

    /// A band nested in `within`, its id `id`.
    fn band(within: Option<usize>, id: &str) -> Band {
        Band { within, id: id.into() }
    }

    /// Whether the box of task `t` lies within band `b`'s strip, clear of its edges.
    fn holds(drawing: &Drawing, b: usize, t: usize) -> bool {
        let Some((top, bottom)) = drawing.bands[b] else { return false };
        let y = drawing.boxes[t].1;
        y >= top + BAND_PAD - 1e-9 && y + H <= bottom - BAND_PAD + 1e-9
    }

    #[test]
    fn a_band_holds_its_tasks_and_the_bands_nested_in_it_and_overlaps_no_other() {
        // "p" holds "a" and "b", and "q", nested in it, holds "c"; "r" holds "d"; "e" is in none.
        // "b" waits for "a", "c" for "b", "d" for "a" and "e" for "c".
        let mut tasks: Vec<Task> = ["a", "b", "c", "d", "e"].map(open).into();
        for (t, b) in [(0, 0), (1, 0), (2, 1), (3, 2)] {
            tasks[t].band = Some(b);
        }
        let bands = [band(None, "p"), band(Some(0), "q"), band(None, "r")];
        let edges = [(0, 1), (1, 2), (0, 3), (2, 4)];
        let drawing = layout(&tasks, &bands, &edges, &[], Order::Auto);
        assert!(holds(&drawing, 0, 0) && holds(&drawing, 0, 1) && holds(&drawing, 1, 2) && holds(&drawing, 2, 3));
        assert!(holds(&drawing, 0, 2), "the band a band is nested in holds its tasks too");
        assert!((0..3).all(|b| !holds(&drawing, b, 4)), "a task in no band lies in none");
        let extent = |b: usize| drawing.bands[b].expect("a band that holds a task");
        let ((p_top, p_bottom), (q_top, q_bottom), (r_top, r_bottom)) = (extent(0), extent(1), extent(2));
        assert!(p_top < q_top && q_bottom + BAND_PAD <= p_bottom + 1e-9, "q lies within p");
        assert!(p_bottom + GY <= r_top + 1e-9 || r_bottom + GY <= p_top + 1e-9, "p and r lie apart");
        assert_eq!(edges_across_boxes(&drawing.boxes, &drawing.routes, &edges), 0);
        assert_eq!(crossings_drawn(&drawing.routes, drawing.width), 0);
    }

    #[test]
    fn a_band_that_opens_with_a_band_nested_in_it_leaves_room_for_its_label() {
        // "p" holds only "q", which holds "a" and "b".
        let mut tasks: Vec<Task> = ["a", "b"].map(open).into();
        for task in &mut tasks {
            task.band = Some(1);
        }
        let drawing = layout(&tasks, &[band(None, "p"), band(Some(0), "q")], &[(0, 1)], &[], Order::Auto);
        let (outer, inner) = (drawing.bands[0].expect("p").0, drawing.bands[1].expect("q").0);
        assert_eq!(inner - outer, BAND_PAD + LABEL_ROOM);
        // A band that holds no task, nor any band that does, has no strip.
        let drawing = layout(&tasks, &[band(None, "p"), band(Some(0), "q"), band(None, "r")], &[], &[], Order::Auto);
        assert_eq!(drawing.bands[2], None);
    }

    #[test]
    fn bands_that_would_nest_in_each_other_are_drawn_one_in_the_other() {
        // "p" says it is nested in "q", and "q" in "p", as relations arriving through sync can say.
        let mut tasks: Vec<Task> = ["a", "b"].map(open).into();
        (tasks[0].band, tasks[1].band) = (Some(0), Some(1));
        let drawing = layout(&tasks, &[band(Some(1), "p"), band(Some(0), "q")], &[(0, 1)], &[], Order::Auto);
        let ((p_top, p_bottom), (q_top, q_bottom)) = (drawing.bands[0].expect("p"), drawing.bands[1].expect("q"));
        assert!(p_top < q_top && q_bottom < p_bottom, "the loop is cut at the first band: q lies in p");
        assert!(holds(&drawing, 0, 0) && holds(&drawing, 1, 1));
    }

    #[test]
    fn any_graph_is_drawn_with_no_box_over_another_no_line_across_a_box_and_no_end_shared() {
        // Graphs made up from a fixed seed, loops and edges from a task to itself among them, with
        // lines threaded through them between tasks taken at random, and in two graphs of three
        // bands nested at random, holding tasks taken at random.
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = |below: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % below as u64) as usize
        };
        for graph in 0..30 {
            let n = 4 + next(22);
            let count = if graph % 3 == 0 { 0 } else { 1 + next(4) };
            let bands: Vec<Band> =
                (0..count).map(|b| band((b > 0 && next(2) == 0).then(|| next(b)), &format!("b{b}"))).collect();
            let tasks: Vec<Task> = (0..n)
                .map(|t| Task {
                    title: format!("t{}", next(7)),
                    id: format!("{t:03}"),
                    priority: [None, Some(0), Some(1), Some(2)][next(4)],
                    status: next(7) as u8,
                    band: (count > 0 && next(3) > 0).then(|| next(count)),
                })
                .collect();
            let edges: Vec<(usize, usize)> = (0..next(2 * n)).map(|_| (next(n), next(n))).collect();
            let threaded: Vec<(usize, usize)> = (0..next(n)).map(|_| (next(n), next(n))).collect();
            // The same graph given backwards: its tasks, bands and lines each in the other order.
            let (last, final_band) = (n - 1, count.saturating_sub(1));
            let backwards: Vec<Task> =
                tasks.iter().rev().map(|t| Task { band: t.band.map(|b| final_band - b), ..t.clone() }).collect();
            let bands_backwards: Vec<Band> =
                bands.iter().rev().map(|b| Band { within: b.within.map(|w| final_band - w), ..b.clone() }).collect();
            let flip = |lines: &[(usize, usize)]| -> Vec<(usize, usize)> {
                lines.iter().rev().map(|&(a, b)| (last - a, last - b)).collect()
            };
            // The bands each band is in, itself first.
            let enclosing = |b: usize| -> Vec<usize> {
                let mut path = vec![b];
                while let Some(w) = bands[*path.last().unwrap_or(&b)].within {
                    path.push(w);
                }
                path
            };
            for order in Order::ALL {
                let drawing = layout(&tasks, &bands, &edges, &threaded, order);
                let at = format!("graph {graph}, {order:?}");
                // A task in a band lies within it and every band it is in; bands one of which is
                // not in the other lie apart.
                for (t, task) in tasks.iter().enumerate() {
                    if let Some(b) = task.band {
                        assert!(enclosing(b).into_iter().all(|e| holds(&drawing, e, t)), "{at}: task {t}");
                    }
                }
                for (b, c) in (0..count).flat_map(|b| (b + 1..count).map(move |c| (b, c))) {
                    let (Some(p), Some(q)) = (drawing.bands[b], drawing.bands[c]) else { continue };
                    if !enclosing(b).contains(&c) && !enclosing(c).contains(&b) {
                        assert!(p.1 <= q.0 || q.1 <= p.0, "{at}: bands {b} and {c}");
                    }
                }
                let other = layout(&backwards, &bands_backwards, &flip(&edges), &flip(&threaded), order);
                assert_eq!(other.boxes.into_iter().rev().collect::<Vec<_>>(), drawing.boxes, "{at}, backwards");
                assert_eq!(other.routes.into_iter().rev().collect::<Vec<_>>(), drawing.routes, "{at}, backwards");
                assert_eq!(other.bands.into_iter().rev().collect::<Vec<_>>(), drawing.bands, "{at}, backwards");
                assert_eq!(other.threaded.into_iter().rev().collect::<Vec<_>>(), drawing.threaded, "{at}, backwards");
                for (s, a) in drawing.boxes.iter().enumerate() {
                    for b in drawing.boxes[s + 1..].iter().filter(|b| b.0 == a.0) {
                        assert!((a.1 - b.1).abs() > H + GY - 1e-6, "{at}: {a:?} and {b:?}");
                    }
                }
                assert_eq!(edges_across_boxes(&drawing.boxes, &drawing.routes, &edges), 0, "{at}");
                assert_eq!(edges_across_boxes(&drawing.boxes, &drawing.threaded, &threaded), 0, "{at}, threaded");
                // Every line runs from a side of one of its tasks' boxes to a side of the other's,
                // meeting each within the span its ends spread over.
                let lines = edges.iter().zip(&drawing.routes).chain(threaded.iter().zip(&drawing.threaded));
                for (&(first, then), route) in lines {
                    let (Some(start), Some(end)) = (route.first(), route.last()) else {
                        assert_eq!(first, then, "{at}");
                        continue;
                    };
                    let (from, to) = (drawing.boxes[first], drawing.boxes[then]);
                    let on_side = |(x, y): (f64, f64), (bx, by): (f64, f64)| {
                        [bx, bx + W].contains(&x) && (y - by - H / 2.0).abs() <= END_SPAN / 2.0 + 1e-6
                    };
                    assert!(on_side(*start, from) && on_side(*end, to), "{at}: {route:?}");
                }
                assert_eq!(ends_shared(&drawing, &edges, &threaded), 0, "{at}");
            }
        }
    }

    /// How many times two lines meet a task at the same point, leaving out two edges that join the
    /// same two tasks, which are drawn as one, and a threaded line given twice.
    pub(crate) fn ends_shared(drawing: &Drawing, edges: &[(usize, usize)], threaded: &[(usize, usize)]) -> usize {
        // Each end: which line it is (threaded or not, and its tasks), and where it is.
        type End = ((bool, (usize, usize)), (f64, f64));
        let unordered = |(a, b): (usize, usize)| (a.min(b), a.max(b));
        let ends: Vec<End> = edges
            .iter()
            .map(|&pair| (false, unordered(pair)))
            .zip(&drawing.routes)
            .chain(threaded.iter().map(|&pair| (true, pair)).zip(&drawing.threaded))
            .filter(|(_, route)| route.len() > 1)
            .flat_map(|(line, route)| [(line, route[0]), (line, route[route.len() - 1])])
            .collect();
        let mut shared = 0;
        for (i, (line, point)) in ends.iter().enumerate() {
            shared += ends[i + 1..]
                .iter()
                .filter(|(other, at)| other != line && (at.0 - point.0).abs() < 1e-6 && (at.1 - point.1).abs() < 1e-6)
                .count();
        }
        shared
    }
}
