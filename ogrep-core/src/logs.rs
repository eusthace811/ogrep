//! Log templates: group log lines that differ only in their variable parts.
//!
//! A million log lines are usually a few dozen messages with different ids,
//! numbers and durations in them. This groups lines the way Drain (He et al.,
//! 2017) does: tokens that look variable (numbers, hex, UUIDs, IP addresses,
//! paths with digits) are masked up front; lines with the same number of tokens
//! and the same first token are compared position by position, and a line joins
//! the first template it matches in at least [`SIMILARITY`] of its positions,
//! turning the positions that differ into `<*>`.
//!
//! A leading timestamp (ISO 8601, with or without `T`, zone or fraction) is
//! read, not templated, so each template knows when it was first and last seen.
//! A template whose first sighting is recent is the classic sign that something
//! new started happening.

use std::collections::HashMap;

/// Share of token positions that must agree for a line to join a template.
pub const SIMILARITY: f64 = 0.5;
const WILDCARD: &str = "<*>";

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Template {
    pub tokens: Vec<String>,
    pub count: usize,
    pub first_seen: Option<i64>,
    pub last_seen: Option<i64>,
    /// Where the first line of this template was seen: `(file, line)`.
    pub example: (String, usize),
    pub example_text: String,
}

impl Template {
    pub fn text(&self) -> String {
        self.tokens.join(" ")
    }

    /// The longest run of constant tokens, the part most likely to be written
    /// literally in the code that logs it.
    pub fn constant_part(&self) -> String {
        let mut best: &[String] = &[];
        let mut start = 0;
        for i in 0..=self.tokens.len() {
            if i == self.tokens.len()
                || self.tokens[i] == WILDCARD
                || self.tokens[i].contains(WILDCARD)
            {
                let run = &self.tokens[start..i];
                if run.iter().map(|t| t.len() + 1).sum::<usize>()
                    > best.iter().map(|t| t.len() + 1).sum::<usize>()
                {
                    best = run;
                }
                start = i + 1;
            }
        }
        best.join(" ")
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
    groups: HashMap<(usize, String), Vec<usize>>,
    pub templates: Vec<Template>,
}

impl Drain {
    pub fn add(&mut self, file: &str, line_number: usize, line: &str) {
        let (time, rest) = leading_timestamp(line);
        let tokens: Vec<String> = rest
            .split_whitespace()
            .map(|t| {
                if is_variable(t) {
                    WILDCARD.to_string()
                } else {
                    t.to_string()
                }
            })
            .collect();
        if tokens.is_empty() {
            return;
        }
        let key = (tokens.len(), tokens[0].clone());
        let slots = self.groups.entry(key).or_default();
        for &i in slots.iter() {
            let t = &mut self.templates[i];
            let same = t.tokens.iter().zip(&tokens).filter(|(a, b)| a == b).count();
            if same as f64 / tokens.len() as f64 >= SIMILARITY {
                for (a, b) in t.tokens.iter_mut().zip(&tokens) {
                    if a != b {
                        *a = WILDCARD.to_string();
                    }
                }
                t.count += 1;
                if let Some(time) = time {
                    t.first_seen = Some(t.first_seen.map_or(time, |f| f.min(time)));
                    t.last_seen = Some(t.last_seen.map_or(time, |l| l.max(time)));
                }
                return;
            }
        }
        slots.push(self.templates.len());
        self.templates.push(Template {
            tokens,
            count: 1,
            first_seen: time,
            last_seen: time,
            example: (file.to_string(), line_number),
            example_text: line.to_string(),
        });
    }

    /// Templates, most frequent first; ties by first sighting, then text.
    pub fn sorted(mut self) -> Vec<Template> {
        self.templates.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then(a.first_seen.cmp(&b.first_seen))
                .then_with(|| a.text().cmp(&b.text()))
        });
        self.templates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
