//! Wallboard packing and priority ordering over core graph geometry.
// main.rs lets what only the app uses go unused in the wallboard's build; the wallboard's own code may not.
#![warn(dead_code)]
// The layout's private items this packing builds on. A rename in layout.rs fails here, in one place.
use super::{
    columns_of, groups_of, numbered, spread_ends_with, urgency, without_loops, Drawing, Group, Layout, Line, Placed,
    Task, GX, PAD, W,
};
use std::collections::BTreeMap;

impl Layout {
    /// Pack connected work into nearby dependency columns, then fill remaining capacity with
    /// unrelated cards. Reuse the ordinary graph's crossing minimizer, waypoint placement,
    /// smooth routes and distinct ports, with wallboard card dimensions and a strict row cap.
    pub fn wallboard(tasks: &[Task], edges: &[(usize, usize)], max_rows: usize, width: f64, height: f64) -> Drawing {
        let n = tasks.len();
        let cap = max_rows.max(1);
        let mut sorted: Vec<usize> = (0..n).collect();
        sorted.sort_by(|&a, &b| urgency(&tasks[a]).cmp(&urgency(&tasks[b])));
        let mut number = vec![0; n];
        for (rank, &task) in sorted.iter().enumerate() {
            number[task] = rank;
        }
        let given = numbered(&number, edges);
        let pairs = without_loops(n, given.iter().flatten().copied().collect());
        let depth = columns_of(n, &pairs);
        let (groups, edgeless) = groups_of(n, &pairs);
        let mut column = vec![0; n];
        let mut occupied = vec![0usize];
        let mut rank = vec![0; n];
        let mut ordered = Vec::new();
        for members in &groups {
            ordered.extend(members.iter().copied());
            let mut needs = vec![0usize; members.iter().map(|&t| depth[t]).max().unwrap_or(0) + 1];
            for &task in members {
                needs[depth[task]] += 1;
            }
            if needs.iter().all(|&count| count <= cap) {
                let mut start = 0;
                while needs
                    .iter()
                    .enumerate()
                    .any(|(c, &count)| occupied.get(start + c).copied().unwrap_or(0) + count > cap)
                {
                    start += 1;
                }
                occupied.resize(occupied.len().max(start + needs.len()), 0);
                for &task in members {
                    column[task] = start + depth[task];
                    occupied[column[task]] += 1;
                }
            } else {
                // A large fan-in/out cannot fit its natural layer. Spill only this component,
                // keeping every dependent to the right of all its prerequisites.
                let mut pending = members.clone();
                pending.sort_by_key(|&task| (depth[task], task));
                for task in pending {
                    let mut c =
                        pairs.iter().filter(|(_, b)| *b == task).map(|(a, _)| column[*a] + 1).max().unwrap_or(0);
                    while occupied.get(c).copied().unwrap_or(0) >= cap {
                        c += 1;
                    }
                    occupied.resize(occupied.len().max(c + 1), 0);
                    column[task] = c;
                    occupied[c] += 1;
                }
            }
        }
        for task in edgeless {
            ordered.push(task);
            let c = occupied.iter().position(|&count| count < cap).unwrap_or(occupied.len());
            occupied.resize(occupied.len().max(c + 1), 0);
            column[task] = c;
            occupied[c] += 1;
        }
        for (position, task) in ordered.into_iter().enumerate() {
            rank[task] = position;
        }
        let mut group = Group::new(&(0..n).collect::<Vec<_>>(), &pairs, &column, &rank, |_| 0, |_| 0);
        group.task_height = height;
        group.gap = 16.0;
        let priorities: Vec<_> = sorted.iter().map(|&task| tasks[task].priority).collect();
        let columns = group.wallboard_order(group.order_auto(group.columns_by_tie(true)), &priorities);
        let needed = group.needs(&columns, 1)[0];
        let limit = cap as f64 * height + cap.saturating_sub(1) as f64 * group.gap;
        let heights = group.pulled(&columns, &[(PAD, PAD + needed.max(limit))]);
        let mut placed = Placed {
            centre: vec![0.0; n],
            routes: BTreeMap::new(),
            taken: vec![Vec::new(); occupied.len()],
            columns: occupied.len(),
            bottom: PAD,
            bands: Vec::new(),
        };
        group.place(&heights, &mut placed);
        let mut lines: Vec<_> = placed
            .routes
            .iter()
            .map(|(pair, route)| Line { pair: *pair, threaded: false, route: route.clone() })
            .collect();
        spread_ends_with(&mut lines, &placed.centre, (height - 28.0).min(112.0), 28.0);
        let routes: BTreeMap<_, _> = lines.into_iter().map(|line| (line.pair, line.route)).collect();
        // Spend horizontal room where arrows actually run. Empty columns keep a compact gap;
        // a busy gutter needs room for curves to separate rather than become a vertical bundle.
        let gaps: Vec<_> = (0..occupied.len().saturating_sub(1))
            .map(|c| if pairs.iter().any(|&(a, b)| column[a] <= c && column[b] > c) { 300.0 } else { 64.0 })
            .collect();
        let mut left = vec![PAD; occupied.len()];
        for c in 1..left.len() {
            left[c] = left[c - 1] + width + gaps[c - 1];
        }
        // Map both card interiors and gaps piecewise, including any future waypoint positions.
        let map_x = |x: f64| {
            let c = (((x - PAD) / (W + GX)).floor() as usize).min(left.len().saturating_sub(1));
            let offset = x - PAD - c as f64 * (W + GX);
            left[c] + if offset <= W { offset * width / W } else { width + (offset - W) * gaps[c] / GX }
        };
        let boxes: Vec<_> = number.iter().map(|&t| (left[column[t]], placed.centre[t] - height / 2.0)).collect();
        let routes = given
            .iter()
            .map(|pair| {
                let Some((a, b)) = *pair else { return Vec::new() };
                let route = routes.get(&(a, b)).cloned().unwrap_or_else(|| {
                    routes.get(&(b, a)).map(|route| route.iter().rev().copied().collect()).unwrap_or_default()
                });
                route.into_iter().map(|(x, y)| (map_x(x), y)).collect()
            })
            .collect();
        let bottom = boxes.iter().map(|(_, y)| y + height).fold(PAD, f64::max);
        Drawing {
            boxes,
            routes,
            threaded: Vec::new(),
            bands: Vec::new(),
            width: left.last().copied().unwrap_or(PAD) + width + PAD,
            height: bottom + PAD,
        }
    }
}

impl Group {
    /// The wallboard's few tall cards make long fan-outs much more prominent. Try bounded
    /// insertions beyond the ordinary adjacent swaps. Priority order within each column comes
    /// first, then fewer crossings and shorter spans. Waypoints can move freely; cards of equal
    /// priority can trade places to untangle edges. Several deterministic starts escape a local
    /// minimum, with a bounded evaluation budget. Normal graph ordering stays unchanged.
    fn wallboard_order(&self, mut initial: Vec<Vec<usize>>, priorities: &[Option<i64>]) -> Vec<Vec<usize>> {
        let priority = |task: usize| (priorities[task].is_none(), priorities[task].unwrap_or(0));
        // Start with no priority inversions, so even an exhausted search budget cannot strand a
        // promoted card below lower-priority work. Leave waypoint slots in place initially.
        for column in &mut initial {
            let slots: Vec<_> = column.iter().enumerate().filter_map(|(i, &v)| self.task[v].map(|_| i)).collect();
            let mut cards: Vec<_> = slots.iter().map(|&i| column[i]).collect();
            cards.sort_by_key(|&v| priority(self.task[v].unwrap()));
            for (slot, card) in slots.into_iter().zip(cards) {
                column[slot] = card;
            }
        }
        let score = |columns: &[Vec<usize>]| {
            let inversions: usize = columns
                .iter()
                .map(|column| {
                    let cards: Vec<_> = column.iter().filter_map(|&v| self.task[v]).collect();
                    cards
                        .iter()
                        .enumerate()
                        .map(|(i, &a)| cards[i + 1..].iter().filter(|&&b| priority(a) > priority(b)).count())
                        .sum::<usize>()
                })
                .sum();
            let place = self.places(columns);
            let spans: Vec<_> = self
                .chains
                .iter()
                .flat_map(|(_, chain)| chain.windows(2))
                .map(|pair| place[pair[0]].abs_diff(place[pair[1]]))
                .collect();
            (inversions, self.crossings(columns), spans.iter().sum::<usize>(), spans.into_iter().max().unwrap_or(0))
        };
        let mut best = initial.clone();
        let mut best_score = score(&best);
        let mut budget = 8_000usize;
        for seed in 0..6 {
            let mut columns = initial.clone();
            for (c, column) in columns.iter_mut().enumerate() {
                let slots: Vec<_> = column
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| !self.before[**v].is_empty() || !self.after[**v].is_empty())
                    .map(|(i, _)| i)
                    .collect();
                let mut active: Vec<_> = slots.iter().map(|&i| column[i]).collect();
                match seed {
                    1 => active.reverse(),
                    2 if c % 2 == 0 => active.reverse(),
                    3 if c % 2 == 1 => active.reverse(),
                    4 | 5 if !active.is_empty() => {
                        let shift = (seed + c) % active.len();
                        active.rotate_left(shift);
                    }
                    _ => {}
                }
                for (i, v) in slots.into_iter().zip(active) {
                    column[i] = v;
                }
            }
            let mut current = score(&columns);
            loop {
                let mut next = current;
                let mut move_to = None;
                'search: for c in 0..columns.len() {
                    for from in 0..columns[c].len() {
                        for to in 0..columns[c].len() {
                            if from == to {
                                continue;
                            }
                            if budget == 0 {
                                break 'search;
                            }
                            budget -= 1;
                            let node = columns[c].remove(from);
                            columns[c].insert(to, node);
                            let candidate = score(&columns);
                            if candidate < next {
                                next = candidate;
                                move_to = Some((c, from, to, false));
                            }
                            let node = columns[c].remove(to);
                            columns[c].insert(from, node);
                            if to > from && budget > 0 {
                                budget -= 1;
                                columns[c].swap(from, to);
                                let candidate = score(&columns);
                                if candidate < next {
                                    next = candidate;
                                    move_to = Some((c, from, to, true));
                                }
                                columns[c].swap(from, to);
                            }
                        }
                    }
                }
                let Some((c, from, to, swap)) = move_to else {
                    break;
                };
                if swap {
                    columns[c].swap(from, to);
                } else {
                    let node = columns[c].remove(from);
                    columns[c].insert(to, node);
                }
                current = next;
            }
            if current < best_score {
                best = columns;
                best_score = current;
            }
            if budget == 0 {
                break;
            }
        }
        best
    }
}

#[cfg(test)]
#[path = "wallboard_layout_tests.rs"]
mod tests;
