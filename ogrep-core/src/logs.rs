//! Log templates: group log lines that differ only in their variable parts.
//!
//! A million log lines are usually a few dozen messages with different ids,
//! numbers and durations in them. This groups lines the way Drain (He et al.,
//! 2017) does: tokens that look variable (numbers, hex, UUIDs, IP addresses,
//! paths with digits) are masked up front; lines with the same number of tokens
//! and the same first token are compared position by position. A line joins
//! the template whose constant tokens it matches in the most positions (at
//! least [`SIMILARITY`] of them), preferring the more general one on a tie,
//! and the positions that differ become `<*>`. As in Drain3, masked tokens
//! are equal to each other, while a position a template generalised counts
//! for neither side.
//!
//! A leading timestamp (ISO 8601, with or without `T`, zone or fraction) is
//! read, not templated, so each template knows when it was first and last seen.
//! A template whose first sighting is recent is the classic sign that something
//! new started happening.

use std::collections::HashMap;

/// Share of token positions that must agree for a line to join a template.
pub const SIMILARITY: f64 = 0.5;
/// How masked and generalised positions are shown.
const WILDCARD: &str = "<*>";

/// One position of a template.
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Word(String),
    /// A variable token (a number, an id), masked up front. Masked tokens are
    /// equal to each other, like Drain3's `<NUM>`/`<IP>` masks.
    Masked,
    /// A position the template generalised by merging lines that differ
    /// there. It counts for neither side of a comparison, as Drain's
    /// parameter does; a line never has one.
    Generalised,
}

impl Token {
    fn as_str(&self) -> &str {
        match self {
            Token::Word(w) => w,
            Token::Masked | Token::Generalised => WILDCARD,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    pub tokens: Vec<Token>,
    pub count: usize,
    pub first_seen: Option<i64>,
    pub last_seen: Option<i64>,
    /// Where the first line of this template was seen: `(file, line)`.
    pub example: (String, usize),
}

impl Template {
    pub fn text(&self) -> String {
        let shown: Vec<&str> = self.tokens.iter().map(Token::as_str).collect();
        shown.join(" ")
    }

    /// The longest run of constant tokens, the part most likely to be written
    /// literally in the code that logs it.
    pub fn constant_part(&self) -> String {
        let len = |run: &[Token]| run.iter().map(|t| t.as_str().len() + 1).sum::<usize>();
        let mut best: &[Token] = &[];
        for run in self.tokens.split(|t| !matches!(t, Token::Word(_))) {
            if len(run) > len(best) {
                best = run;
            }
        }
        let words: Vec<&str> = best.iter().map(Token::as_str).collect();
        words.join(" ")
    }
}

/// Seconds since the epoch for a leading ISO 8601 timestamp, and the rest of
/// the line after it.
pub fn leading_timestamp(line: &str) -> (Option<i64>, &str) {
    let s = line.trim_start_matches(['[', ' ']);
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| -> Option<i64> {
        let part = s.get(r)?;
        part.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || !(b[10] == b'T' || b[10] == b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return (None, line);
    }
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(se)) = (
        digits(0..4),
        digits(5..7),
        digits(8..10),
        digits(11..13),
        digits(14..16),
        digits(17..19),
    ) else {
        return (None, line);
    };
    let mut end = 19;
    if b.get(end) == Some(&b'.') || b.get(end) == Some(&b',') {
        end += 1;
        while b.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
    }
    let mut offset = 0i64;
    match b.get(end) {
        Some(b'Z') => end += 1,
        Some(&c) if (c == b'+' || c == b'-') && b.len() >= end + 6 && b[end + 3] == b':' => {
            if let (Some(oh), Some(om)) = (digits(end + 1..end + 3), digits(end + 4..end + 6)) {
                offset = (oh * 3600 + om * 60) * if c == b'+' { 1 } else { -1 };
                end += 6;
            }
        }
        _ => {}
    }
    // Days from civil (Howard Hinnant).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + se - offset;
    let rest = s[end..].trim_start_matches([']', ' ']);
    (Some(secs), rest)
}

fn is_variable(token: &str) -> bool {
    let t = token.trim_matches(|c: char| ",;:()[]{}\"'".contains(c));
    if t.is_empty() {
        return false;
    }
    let digits = t.bytes().filter(u8::is_ascii_digit).count();
    // Numbers, ids with digits, hex, durations (12ms), IPs, UUIDs.
    digits > 0 && (digits * 2 >= t.len() || t.contains('-') && t.len() >= 16 || t.starts_with("0x"))
        || (t.len() >= 16 && t.bytes().all(|c| c.is_ascii_hexdigit()))
}

/// Groups lines into templates.
#[derive(Default)]
pub struct Drain {
    /// Templates by length, then by first token as shown.
    groups: HashMap<usize, HashMap<String, Vec<usize>>>,
    pub templates: Vec<Template>,
}

/// A line's token, borrowed from the line until it starts a template.
#[derive(Clone, Copy)]
enum LineToken<'a> {
    Word(&'a str),
    Masked,
}

impl LineToken<'_> {
    fn as_str(&self) -> &str {
        match self {
            LineToken::Word(w) => w,
            LineToken::Masked => WILDCARD,
        }
    }

    /// Whether a template's token matches it: a generalised position
    /// matches nothing, as a line never has one.
    fn matches(self, t: &Token) -> bool {
        match (t, self) {
            (Token::Word(a), LineToken::Word(b)) => a == b,
            (Token::Masked, LineToken::Masked) => true,
            _ => false,
        }
    }

    fn to_owned(self) -> Token {
        match self {
            LineToken::Word(w) => Token::Word(w.to_string()),
            LineToken::Masked => Token::Masked,
        }
    }
}

impl Drain {
    pub fn add(&mut self, file: &str, line_number: usize, line: &str) {
        let (time, rest) = leading_timestamp(line);
        let tokens: Vec<LineToken> = rest
            .split_whitespace()
            .map(|t| {
                // A literal `<*>` prints like a masked value, so it is one.
                if is_variable(t) || t == WILDCARD {
                    LineToken::Masked
                } else {
                    LineToken::Word(t)
                }
            })
            .collect();
        let Some(first) = tokens.first().map(LineToken::as_str) else {
            return;
        };
        let by_first = self.groups.entry(tokens.len()).or_default();
        if !by_first.contains_key(first) {
            by_first.insert(first.to_string(), Vec::new());
        }
        let slots = by_first.get_mut(first).expect("just inserted");
        // As in Drain3: similarity counts equal tokens (a line has no
        // generalised position, so those count for neither side), and the
        // most similar template wins, then the more general one.
        let best = slots
            .iter()
            .filter_map(|&i| {
                let t = &self.templates[i];
                let same = t
                    .tokens
                    .iter()
                    .zip(&tokens)
                    .filter(|(a, b)| b.matches(a))
                    .count();
                (same as f64 / tokens.len() as f64 >= SIMILARITY).then(|| {
                    let wild = t
                        .tokens
                        .iter()
                        .filter(|a| **a == Token::Generalised)
                        .count();
                    (same, wild, i)
                })
            })
            .max_by_key(|&(same, wild, i)| (same, wild, std::cmp::Reverse(i)));
        if let Some((_, _, i)) = best {
            let t = &mut self.templates[i];
            for (a, b) in t.tokens.iter_mut().zip(&tokens) {
                if !b.matches(a) {
                    *a = Token::Generalised;
                }
            }
            t.count += 1;
            if let Some(time) = time {
                t.first_seen = Some(t.first_seen.map_or(time, |f| f.min(time)));
                t.last_seen = Some(t.last_seen.map_or(time, |l| l.max(time)));
            }
            return;
        }
        slots.push(self.templates.len());
        self.templates.push(Template {
            tokens: tokens.into_iter().map(LineToken::to_owned).collect(),
            count: 1,
            first_seen: time,
            last_seen: time,
            example: (file.to_string(), line_number),
        });
    }

    /// Templates, most frequent first; ties by first sighting, then text.
    pub fn sorted(mut self) -> Vec<Template> {
        self.templates
            .sort_by_cached_key(|t| (std::cmp::Reverse(t.count), t.first_seen, t.text()));
        self.templates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_lines_with_masked_values_share_a_template() {
        let mut drain = Drain::default();
        for (i, line) in ["Connected 10.0.0.1 5432", "Connected 10.0.0.2 5433"]
            .iter()
            .enumerate()
        {
            drain.add("app.log", i + 1, line);
        }
        let templates: Vec<(String, usize)> = drain
            .sorted()
            .into_iter()
            .map(|t| (t.text(), t.count))
            .collect();
        assert_eq!(templates, [("Connected <*> <*>".to_string(), 2)]);
    }

    #[test]
    fn lines_shown_alike_share_a_template() {
        // A literal `<*>` word prints like a masked value, so it is one.
        let mut drain = Drain::default();
        drain.add("app.log", 1, "12 connected ok");
        drain.add("app.log", 2, "<*> connected ok");
        // These agree with it in two of three positions, so they join it.
        drain.add("app.log", 3, "<*> 7 ok");
        drain.add("app.log", 4, "9 <*> ok");
        let templates: Vec<(String, usize)> = drain
            .sorted()
            .into_iter()
            .map(|t| (t.text(), t.count))
            .collect();
        assert_eq!(templates, [("<*> <*> ok".to_string(), 4)]);
    }

    #[test]
    fn the_most_similar_template_wins() {
        let mut drain = Drain::default();
        // Two templates share a first token; a line exactly like the second
        // joins it, though the first also clears the threshold.
        for (i, line) in [
            "retry job alpha now",
            "retry job beta now",
            "retry task gamma later",
            "retry task gamma later",
        ]
        .iter()
        .enumerate()
        {
            drain.add("app.log", i + 1, line);
        }
        let counts: Vec<(String, usize)> = drain
            .sorted()
            .into_iter()
            .map(|t| (t.text(), t.count))
            .collect();
        assert!(
            counts.contains(&("retry task gamma later".to_string(), 2)),
            "{counts:?}"
        );
    }

    #[test]
    fn timestamps_parse_with_zones_and_fractions() {
        assert_eq!(
            leading_timestamp("2024-05-01T12:00:00Z hello").0,
            Some(1_714_564_800)
        );
        assert_eq!(
            leading_timestamp("2024-05-01 12:00:00.123 hi").0,
            Some(1_714_564_800)
        );
        assert_eq!(
            leading_timestamp("[2024-05-01T14:00:00+02:00] x").0,
            Some(1_714_564_800)
        );
        assert_eq!(leading_timestamp("2024-05-01T12:00:00Z hello").1, "hello");
        assert_eq!(leading_timestamp("no time here").0, None);
    }

    #[test]
    fn similar_lines_become_one_template() {
        let mut d = Drain::default();
        d.add(
            "a.log",
            1,
            "2024-05-01T12:00:00Z connection to db-1 failed after 30ms user=42",
        );
        d.add(
            "a.log",
            2,
            "2024-05-01T12:00:05Z connection to db-2 failed after 12ms user=7",
        );
        d.add("a.log", 3, "2024-05-01T12:01:00Z cache warmed in 5s");
        d.add(
            "a.log",
            4,
            "2024-05-01T12:02:00Z connection to db-9 failed after 1ms user=3",
        );
        let t = d.sorted();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].count, 3);
        assert_eq!(t[0].text(), "connection to <*> failed after <*> <*>");
        assert_eq!(t[0].first_seen, Some(1_714_564_800));
        assert_eq!(t[0].last_seen, Some(1_714_564_920));
        assert_eq!(t[0].constant_part(), "connection to");
        assert_eq!(t[1].text(), "cache warmed in <*>");
    }
}
