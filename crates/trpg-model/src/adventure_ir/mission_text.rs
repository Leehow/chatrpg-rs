//! Adventure IR — shared text utilities for mission-template induction (P2-1).
//!
//! The Vault source is a two-column PDF flattened to text: every line is
//! `<column-a>   <wide gutter>   <column-b>[   <column-c>]`. A mission's scored
//! blocks live in EITHER narrative column, and the CHAOS ladder is itself a
//! three-column micro-table (`cost | ability | wrapped detail`). The earlier
//! per-line "cut at the first run of 3 spaces" heuristic silently dropped every
//! right-column block (and mis-cut leading-indented lines to empty), so ~half of
//! the missions lost their scored mechanics. A page-level river decolumnize is
//! worse — it splits the chaos `cost | ability` row at the wide blank band.
//!
//! [`columns_of`] solves both: split a single line into its gutter-separated
//! segments and let each parser consume the segments it understands — a section
//! header matches ANY segment (so a right-column `CHAOS EFFECTS` is seen), an
//! objective bullet is matched in whichever segment holds it, and a chaos rung
//! reads `cost | ability | detail` as consecutive segments. No global reorder,
//! no row-splitting, no column ever silently dropped.

/// Split a flattened line into its column segments (runs of `>=3` spaces are the
/// inter-column gutters). Each segment is trimmed; empty segments are dropped.
/// Single-column lines return one segment.
pub(super) fn columns_of(line: &str) -> Vec<&str> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut seg_start = 0usize;
    let mut run = 0usize;
    for i in 0..bytes.len() {
        if bytes[i] == b' ' {
            run += 1;
        } else {
            if run >= 3 {
                let seg = line[seg_start..i - run].trim();
                if !seg.is_empty() {
                    out.push(seg);
                }
                seg_start = i;
            }
            run = 0;
        }
    }
    let seg = line[seg_start..].trim();
    if !seg.is_empty() {
        out.push(seg);
    }
    out
}

/// The marker among `markers` that equals (case-insensitively) one of the line's
/// column segments, if any. Lets a header in EITHER column be recognized.
pub(super) fn line_marker<'a>(line: &str, markers: &[&'a str]) -> Option<&'a str> {
    let cols = columns_of(line);
    markers
        .iter()
        .find(|m| cols.iter().any(|c| c.eq_ignore_ascii_case(m)))
        .copied()
}

/// Lowercase a heading slice into a stable id fragment (ascii alnum → '_').
pub(super) fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut prev_us = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.extend(c.to_lowercase());
            prev_us = false;
        } else if !prev_us && !out.is_empty() {
            out.push('_');
            prev_us = true;
        }
    }
    out.trim_matches('_').to_string()
}

/// The canonical Vault template phase markers (ALL-CAPS section headers).
pub(super) const PHASE_MARKERS: &[&str] = &[
    "ANOMALY PROFILE",
    "PRE-INVESTIGATION",
    "MISSION BRIEFING",
    "CHAOS EFFECTS",
    "INVESTIGATION",
    "ENCOUNTER",
    "AFTERMATH",
];

/// Return the slice of `text` belonging to a section whose header is one of
/// `heads` (matched against ANY column segment, so a right-column header is
/// seen), ending at the next line bearing a marker in `ends` (or end of text).
/// `None` if no header is found. The slice keeps raw lines; per-line column
/// segmentation is the caller's job ([`columns_of`]).
pub(super) fn section_slice<'a>(text: &'a str, heads: &[&str], ends: &[&str]) -> Option<&'a str> {
    let lines: Vec<(usize, &str)> = text
        .lines()
        .scan(0usize, |off, l| {
            let start = *off;
            *off += l.len() + 1;
            Some((start, l))
        })
        .collect();
    let mut begin: Option<usize> = None;
    let mut start_idx = 0usize;
    for (i, (off, l)) in lines.iter().enumerate() {
        if line_marker(l, heads).is_some() {
            begin = Some(*off + l.len() + 1);
            start_idx = i + 1;
            break;
        }
    }
    let begin = begin?;
    let mut end = text.len();
    for (off, l) in lines.iter().skip(start_idx) {
        if line_marker(l, ends).is_some() {
            end = *off;
            break;
        }
    }
    Some(text[begin..end.max(begin)].trim())
}

/// Split page-anchored module markdown into [`MissionPage`]s by `# Page N`
/// headers, dropping the HTML source-anchor comment lines. Shared by the parser
/// wiring and the live IR test so both slice pages identically.
pub fn pages_from_markdown(md: &str) -> Vec<super::MissionPage> {
    let mut pages = Vec::new();
    let mut cur_page: Option<u32> = None;
    let mut buf = String::new();
    let flush = |pages: &mut Vec<super::MissionPage>, page: Option<u32>, buf: &str| {
        if let Some(p) = page {
            pages.push(super::MissionPage::new(p, buf.to_string()));
        }
    };
    for line in md.lines() {
        if let Some(rest) = line.strip_prefix("# Page ") {
            if let Ok(n) = rest.trim().parse::<u32>() {
                flush(&mut pages, cur_page, &buf);
                cur_page = Some(n);
                buf.clear();
                continue;
            }
        }
        if line.starts_with("<!-- source_id=") {
            continue;
        }
        buf.push_str(line);
        buf.push('\n');
    }
    flush(&mut pages, cur_page, &buf);
    pages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_of_splits_at_wide_gutters_only() {
        // single-space inside "2 Chaos" stays one segment; the wide gutters split
        // the chaos row into cost | ability | detail.
        assert_eq!(
            columns_of("      2 Chaos       Manifest        the puddle reflects"),
            vec!["2 Chaos", "Manifest", "the puddle reflects"]
        );
        // a single-column line yields one segment.
        assert_eq!(
            columns_of("Optional Objectives"),
            vec!["Optional Objectives"]
        );
    }

    #[test]
    fn columns_of_sees_right_column_objective() {
        // left-column prose + right-column bullet (the case the old left-only cut
        // dropped) → both segments preserved.
        let line = "  palpable delight as she waves         f +3 Commendations for the most waves";
        let cols = columns_of(line);
        assert_eq!(cols[0], "palpable delight as she waves");
        assert_eq!(cols[1], "f +3 Commendations for the most waves");
    }

    #[test]
    fn line_marker_matches_either_column() {
        assert_eq!(
            line_marker("CHAOS EFFECTS", PHASE_MARKERS),
            Some("CHAOS EFFECTS")
        );
        // header in the RIGHT column is still recognized.
        assert_eq!(
            line_marker(
                "some left prose here           CHAOS EFFECTS",
                PHASE_MARKERS
            ),
            Some("CHAOS EFFECTS")
        );
        assert_eq!(line_marker("just narrative prose", PHASE_MARKERS), None);
    }

    #[test]
    fn section_slice_bounds_at_next_marker() {
        let t = "CHAOS EFFECTS\n2 Chaos Refresh\nINVESTIGATION\nclues";
        let s = section_slice(t, &["CHAOS EFFECTS"], PHASE_MARKERS).unwrap();
        assert_eq!(s, "2 Chaos Refresh");
    }
}
