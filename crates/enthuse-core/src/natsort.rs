//! Natural ("human") ordering of names: `Episode 9` sorts before
//! `Episode 10`, and letter case is ignored.

use std::cmp::Ordering;

/// Compares two names in natural order.
///
/// Runs of ASCII digits compare by numeric value (of any length, without
/// overflow), everything else compares case-insensitively. Names that are
/// equal under those rules fall back to a plain comparison so the order is
/// total and stable.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut x = Chunks::new(a);
    let mut y = Chunks::new(b);
    loop {
        match (x.next(), y.next()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(cx), Some(cy)) => {
                let ord = match (cx, cy) {
                    (Chunk::Digits(dx), Chunk::Digits(dy)) => cmp_digits(dx, dy),
                    (Chunk::Digits(_), Chunk::Text(_)) => Ordering::Less,
                    (Chunk::Text(_), Chunk::Digits(_)) => Ordering::Greater,
                    (Chunk::Text(tx), Chunk::Text(ty)) => cmp_text(tx, ty),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

fn cmp_digits(a: &str, b: &str) -> Ordering {
    let a_trim = a.trim_start_matches('0');
    let b_trim = b.trim_start_matches('0');
    a_trim
        .len()
        .cmp(&b_trim.len())
        .then_with(|| a_trim.cmp(b_trim))
        // "007" after "7", so equal values still have a defined order.
        .then_with(|| a.len().cmp(&b.len()))
}

fn cmp_text(a: &str, b: &str) -> Ordering {
    let lower = |s: &str| s.chars().flat_map(char::to_lowercase).collect::<Vec<_>>();
    lower(a).cmp(&lower(b))
}

#[derive(Debug, Clone, Copy)]
enum Chunk<'a> {
    Digits(&'a str),
    Text(&'a str),
}

struct Chunks<'a> {
    rest: &'a str,
}

impl<'a> Chunks<'a> {
    fn new(s: &'a str) -> Self {
        Self { rest: s }
    }
}

impl<'a> Iterator for Chunks<'a> {
    type Item = Chunk<'a>;

    fn next(&mut self) -> Option<Chunk<'a>> {
        let first = self.rest.chars().next()?;
        let digits = first.is_ascii_digit();
        let end = self
            .rest
            .find(|c: char| c.is_ascii_digit() != digits)
            .unwrap_or(self.rest.len());
        let (chunk, rest) = self.rest.split_at(end);
        self.rest = rest;
        Some(if digits {
            Chunk::Digits(chunk)
        } else {
            Chunk::Text(chunk)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(names: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        v.sort_by(|a, b| natural_cmp(a, b));
        v
    }

    #[test]
    fn numbers_compare_by_value() {
        assert_eq!(
            sorted(&[
                "Episode 10.mkv",
                "Episode 9.mkv",
                "Episode 1.mkv",
                "Episode 2.mkv"
            ]),
            [
                "Episode 1.mkv",
                "Episode 2.mkv",
                "Episode 9.mkv",
                "Episode 10.mkv"
            ]
        );
    }

    #[test]
    fn season_episode_codes() {
        assert_eq!(
            sorted(&[
                "Show S01E10.mkv",
                "Show S02E01.mkv",
                "Show S01E02.mkv",
                "Show S01E01.mkv"
            ]),
            [
                "Show S01E01.mkv",
                "Show S01E02.mkv",
                "Show S01E10.mkv",
                "Show S02E01.mkv"
            ]
        );
    }

    #[test]
    fn case_is_ignored() {
        assert_eq!(
            sorted(&["b.mkv", "A.mkv", "c.mkv"]),
            ["A.mkv", "b.mkv", "c.mkv"]
        );
    }

    #[test]
    fn leading_zeros_and_huge_numbers() {
        assert_eq!(
            sorted(&["file 010", "file 9", "file 0011"]),
            ["file 9", "file 010", "file 0011"]
        );
        assert_eq!(
            natural_cmp("x 99999999999999999999999", "x 100000000000000000000000"),
            Ordering::Less
        );
    }

    #[test]
    fn total_order_for_equivalent_names() {
        assert_ne!(natural_cmp("a.mkv", "A.mkv"), Ordering::Equal);
        assert_ne!(natural_cmp("7", "007"), Ordering::Equal);
        assert_eq!(natural_cmp("same", "same"), Ordering::Equal);
    }

    #[test]
    fn prefix_sorts_first() {
        assert_eq!(sorted(&["abc 1", "abc"]), ["abc", "abc 1"]);
    }

    #[test]
    fn non_ascii_text() {
        assert_eq!(
            sorted(&["Ödland 2", "ödland 10", "Ödland 1"]),
            ["Ödland 1", "Ödland 2", "ödland 10"]
        );
    }
}
