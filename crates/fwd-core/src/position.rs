//! The order of a task's steps. Each `step_of` relation carries a key (`Relation::position`) that
//! sorts as text, so a step can be put after the last, or between two others, without renumbering
//! anything (docs/DECISIONS.md, "The checked state is the step edge's status; position is a
//! fractional key now and a predecessor pointer later", 2026-09-28).
//!
//! A key is a string of base-62 digits (`0-9`, `A-Z`, `a-z`, which sort in that order as bytes)
//! read as a fraction: the first digit counts sixty-seconds, the next the sixty-second part of
//! those, and so on. A key never ends in the smallest digit, `0`, so there is always a key below
//! it; and none is empty. Appending after the last key adds a digit only once in 61 appends. Many
//! inserts at one place halve the room there each time, so keys grow there by one digit for about
//! every six inserts, which no key that knows only its two neighbours can beat for every order of
//! inserts.

use anyhow::{bail, Result};

const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// The key of a first step, in the middle so that steps can go before it as well as after.
pub const FIRST: &str = "V";

fn value(d: u8) -> usize {
    DIGITS.iter().position(|x| *x == d).expect("checked to be a base-62 digit")
}

fn digit(v: usize) -> u8 {
    DIGITS[v]
}

/// Whether `key` is one this module makes: base-62 digits, not empty, not ending in `0`.
pub fn is_key(key: &str) -> bool {
    !key.is_empty() && key.bytes().all(|b| DIGITS.contains(&b)) && !key.ends_with('0')
}

fn check(key: &str) -> Result<()> {
    if !is_key(key) {
        bail!("`{key}` is not a position: a position is base-62 digits that do not end in 0");
    }
    Ok(())
}

/// A key strictly between `before` and `after`: after the last when `after` is none, before the
/// first when `before` is none, and [`FIRST`] when both are. Refused when `before` is not below
/// `after`, or when either is not a key.
pub fn between(before: Option<&str>, after: Option<&str>) -> Result<String> {
    for key in [before, after].into_iter().flatten() {
        check(key)?;
    }
    let key = match (before, after) {
        (None, None) => FIRST.as_bytes().to_vec(),
        (Some(lo), None) => above(lo.as_bytes()),
        (None, Some(hi)) => below(hi.as_bytes()),
        (Some(lo), Some(hi)) => {
            if lo >= hi {
                bail!("no position lies between `{lo}` and `{hi}`: the first must sort before the second");
            }
            midpoint(lo.as_bytes(), Some(hi.as_bytes()))
        }
    };
    Ok(String::from_utf8(key).expect("base-62 digits are ASCII"))
}

/// The key after `last`, or [`FIRST`] when there is none.
pub fn after(last: Option<&str>) -> Result<String> {
    between(last, None)
}

/// The shortest key above `lo`, one digit up where that fits, so that appends stay short.
fn above(lo: &[u8]) -> Vec<u8> {
    match lo.first() {
        // Reached only after a run of `z`: the least digit that may end a key.
        None => vec![b'1'],
        Some(&d) if d != b'z' => vec![digit(value(d) + 1)],
        Some(_) => [&b"z"[..], &above(&lo[1..])].concat(),
    }
}

/// The shortest key below `hi`, one digit down where that fits, so that prepends stay short.
fn below(hi: &[u8]) -> Vec<u8> {
    let d = hi[0];
    if d == b'0' {
        // A key never ends in `0`, so a key starting with it goes on: stay below its rest.
        return [&b"0"[..], &below(&hi[1..])].concat();
    }
    if hi.len() > 1 {
        // The first digit alone sorts before the key it starts.
        return vec![d];
    }
    if d == b'1' {
        return b"0z".to_vec();
    }
    vec![digit(value(d) - 1)]
}

/// The shortest key strictly between `lo` (the empty key reads as zero) and `hi` (none reads as
/// one), as near the middle as that length allows. `lo` sorts before `hi`, and neither ends in `0`.
fn midpoint(lo: &[u8], hi: Option<&[u8]>) -> Vec<u8> {
    if let Some(hi) = hi {
        // The digits the two share (`lo` read with zeros after its end) stay as they are.
        let shared = hi.iter().enumerate().take_while(|(i, d)| lo.get(*i).copied().unwrap_or(b'0') == **d).count();
        if shared > 0 {
            let rest = midpoint(&lo[shared.min(lo.len())..], Some(&hi[shared..]));
            return [&hi[..shared], &rest[..]].concat();
        }
    }
    let low = lo.first().map(|d| value(*d)).unwrap_or(0);
    let high = hi.map(|h| value(h[0])).unwrap_or(DIGITS.len());
    if high - low > 1 {
        return vec![digit((low + high) / 2)];
    }
    // The first digits are neighbours: the first digit of a longer `hi` alone sorts between the
    // two; otherwise keep `lo`'s first digit and go above the rest of it.
    match hi {
        Some(hi) if hi.len() > 1 => vec![hi[0]],
        _ => [&[digit(low)][..], &midpoint(lo.get(1..).unwrap_or(&[]), None)].concat(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(before: Option<&str>, after: Option<&str>) -> String {
        let k = between(before, after).unwrap();
        assert!(is_key(&k), "{k} is a key");
        if let Some(b) = before {
            assert!(b < k.as_str(), "{b} < {k}");
        }
        if let Some(a) = after {
            assert!(k.as_str() < a, "{k} < {a}");
        }
        k
    }

    #[test]
    fn a_key_lies_strictly_between_its_neighbours() {
        assert_eq!(key(None, None), "V");
        assert_eq!(key(Some("V"), None), "W");
        assert_eq!(key(None, Some("V")), "U");
        assert_eq!(key(Some("z"), None), "z1");
        assert_eq!(key(None, Some("1")), "0z");
        assert_eq!(key(Some("A"), Some("C")), "B");
        for (lo, hi) in [("V", "W"), ("V", "V1"), ("0z", "1"), ("1", "11"), ("az", "b"), ("zzz", "zzz1"), ("01", "02")]
        {
            key(Some(lo), Some(hi));
        }
    }

    #[test]
    fn a_neighbour_that_is_not_below_the_other_or_not_a_key_is_refused() {
        assert!(between(Some("W"), Some("V")).is_err());
        assert!(between(Some("V"), Some("V")).is_err());
        for bad in ["", "V0", "a b", "é"] {
            assert!(between(Some(bad), None).is_err(), "{bad:?}");
            assert!(between(None, Some(bad)).is_err(), "{bad:?}");
        }
    }

    /// A thousand inserts at one place, from either side: every key sorts where it was put, and
    /// keys grow by at most one digit for every five inserts (halving the room costs a digit for
    /// about every six).
    #[test]
    fn a_thousand_inserts_between_the_same_two_neighbours_stay_ordered_and_short() {
        let (left, right) = ("V", "W");
        // Each new step goes right after `left`, before the one put there last.
        let mut keys = vec![left.to_string(), right.to_string()];
        for n in 1..=1000 {
            let k = key(Some(left), Some(&keys[1]));
            assert!(k.len() <= 2 + n / 5, "insert {n} made a key of {} digits", k.len());
            keys.insert(1, k);
        }
        assert!(keys.windows(2).all(|w| w[0] < w[1]), "the keys sort in the order they were put");
        // Each new step goes right before `right`, after the one put there last.
        let mut keys = vec![left.to_string(), right.to_string()];
        for n in 1..=1000 {
            let last = keys.len() - 2;
            let k = key(Some(&keys[last]), Some(right));
            assert!(k.len() <= 2 + n / 5, "insert {n} made a key of {} digits", k.len());
            keys.insert(last + 1, k);
        }
        assert!(keys.windows(2).all(|w| w[0] < w[1]), "the keys sort in the order they were put");
    }

    #[test]
    fn a_thousand_appends_and_prepends_stay_ordered_and_a_few_digits_long() {
        let mut appended = vec![after(None).unwrap()];
        let mut prepended = vec![after(None).unwrap()];
        for _ in 0..1000 {
            appended.push(key(appended.last().map(String::as_str), None));
            prepended.insert(0, key(None, prepended.first().map(String::as_str)));
        }
        for keys in [&appended, &prepended] {
            assert!(keys.windows(2).all(|w| w[0] < w[1]));
            assert!(keys.iter().all(|k| k.len() <= 20), "one more digit per 61 appends");
        }
    }
}
