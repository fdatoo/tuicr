//! Translate source line ranges to positions in the rendered diff.

use crate::model::{DiffFile, DiffHunk, DiffLine};

use super::{HunkRef, Side, Stop};

/// Inclusive indices into `files[file].hunks[hunk].lines`. The rows between
/// the endpoints may include lines visible only on the opposite side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedStop {
    pub file: usize,
    pub hunk: usize,
    pub first_line: usize,
    pub last_line: usize,
    pub side: Side,
}

pub fn resolve_stop(stop: &Stop, files: &[DiffFile]) -> Result<ResolvedStop, String> {
    let location = || {
        format!(
            "{} ({}, lines {}-{})",
            stop.path,
            match stop.side {
                Side::Old => "old side",
                Side::New => "new side",
            },
            stop.start_line,
            stop.end_line
        )
    };
    if stop.start_line == 0 || stop.end_line < stop.start_line {
        return Err(format!("{}: invalid line range", location()));
    }
    let (file, entry) = find_file(&stop.path, files)
        .ok_or_else(|| format!("{}: path is not in the diff", location()))?;
    for (hunk, chunk) in entry.hunks.iter().enumerate() {
        if let Some((first_line, last_line)) = locate_range(chunk, stop) {
            return Ok(ResolvedStop {
                file,
                hunk,
                first_line,
                last_line,
                side: stop.side,
            });
        }
    }
    Err(format!("{}: {}", location(), range_error(entry, stop)))
}

pub fn block_text(files: &[DiffFile], at: &ResolvedStop) -> Vec<String> {
    files[at.file].hunks[at.hunk].lines[at.first_line..=at.last_line]
        .iter()
        .filter(|line| line_number(line, at.side).is_some())
        .map(|line| source_text(line).to_owned())
        .collect()
}

pub fn relocate(block: &[String], stop: &Stop, files: &[DiffFile]) -> Option<ResolvedStop> {
    let first = block.first()?;
    let (file, entry) = find_file(&stop.path, files)?;
    let mut best = None;
    let mut best_distance = u32::MAX;
    let mut tied = false;
    for (hunk, chunk) in entry.hunks.iter().enumerate() {
        let mut lines = side_lines(chunk, stop.side);
        while let Some((first_line, start, text)) = lines.next() {
            let distance = stop.start_line.abs_diff(start);
            if distance > best_distance || text != first.as_str() {
                continue;
            }
            let Some(last_line) = block[1..]
                .iter()
                .try_fold(
                    (first_line, start, lines.clone()),
                    |(_, previous, mut tail), expected| {
                        let (index, number, text) = tail.next()?;
                        (previous.checked_add(1) == Some(number) && text == expected.as_str())
                            .then_some((index, number, tail))
                    },
                )
                .map(|(last, _, _)| last)
            else {
                continue;
            };
            if distance == best_distance && best.is_some() {
                tied = true;
            } else {
                best_distance = distance;
                tied = false;
                best = Some(ResolvedStop {
                    file,
                    hunk,
                    first_line,
                    last_line,
                    side: stop.side,
                });
            }
        }
    }
    if tied { None } else { best }
}

pub fn hunk_ref(file: &DiffFile, hunk: &DiffHunk) -> HunkRef {
    HunkRef {
        path: file.display_path().to_string_lossy().into_owned(),
        old_start: hunk.old_start,
        new_start: hunk.new_start,
    }
}

pub(super) fn symbol(header: &str) -> Option<&str> {
    let (_, suffix) = header.strip_prefix("@@")?.split_once("@@")?;
    let symbol = suffix.trim();
    (!symbol.is_empty()).then_some(symbol)
}

pub(super) fn line_number(line: &DiffLine, side: Side) -> Option<u32> {
    match side {
        Side::Old => line.old_lineno,
        Side::New => line.new_lineno,
    }
}

pub(super) fn source_text(line: &DiffLine) -> &str {
    line.raw_content.as_deref().unwrap_or(&line.content)
}

fn find_file<'a>(path: &str, files: &'a [DiffFile]) -> Option<(usize, &'a DiffFile)> {
    files
        .iter()
        .enumerate()
        .find(|(_, file)| file.display_path().to_string_lossy() == path)
}

fn side_lines(
    hunk: &DiffHunk,
    side: Side,
) -> impl DoubleEndedIterator<Item = (usize, u32, &str)> + Clone {
    hunk.lines
        .iter()
        .enumerate()
        .filter_map(move |(index, line)| {
            line_number(line, side).map(|number| (index, number, source_text(line)))
        })
}

fn locate_range(hunk: &DiffHunk, stop: &Stop) -> Option<(usize, usize)> {
    let mut lines = side_lines(hunk, stop.side);
    let (first, _, _) = lines.find(|&(_, number, _)| number == stop.start_line)?;
    let mut last = first;
    for previous in stop.start_line..stop.end_line {
        let (index, number, _) = lines.next()?;
        if number != previous + 1 {
            return None;
        }
        last = index;
    }
    Some((first, last))
}

fn range_error(file: &DiffFile, stop: &Stop) -> String {
    let mut ranges: Vec<_> = file
        .hunks
        .iter()
        .filter_map(|hunk| {
            let mut lines = side_lines(hunk, stop.side);
            let (_, start, _) = lines.next()?;
            let end = lines.next_back().map_or(start, |(_, number, _)| number);
            let distance = start
                .saturating_sub(stop.end_line)
                .max(stop.start_line.saturating_sub(end));
            Some((distance, start, end))
        })
        .collect();
    if ranges.is_empty() {
        return "the diff has no hunk lines on this side".into();
    }
    let crosses = ranges
        .iter()
        .filter(|&&(distance, _, _)| distance == 0)
        .take(2)
        .count()
        == 2;
    ranges.sort_unstable();
    let nearest = ranges
        .iter()
        .take(2)
        .map(|&(_, start, end)| format!("{start}-{end}"))
        .collect::<Vec<_>>()
        .join(", ");
    let reason = if crosses {
        "range crosses hunks; a stop must be inside one hunk"
    } else {
        "range is not contained in any hunk"
    };
    format!("{reason}; nearest hunk ranges: {nearest}")
}

#[cfg(test)]
pub(super) mod fixtures {
    use crate::model::{DiffFile, FilePatch, FileStatus};
    use crate::vcs::diff_parser::parse_file_patches;

    use super::{Side, Stop};

    pub(in crate::review_guide) fn file(path: &str, status: FileStatus, patch: &str) -> DiffFile {
        let old_path = (status != FileStatus::Added).then(|| path.into());
        let new_path = (status != FileStatus::Deleted).then(|| path.into());
        parse_file_patches(vec![FilePatch::new(old_path, new_path, status, patch)])
            .unwrap()
            .remove(0)
    }

    pub(in crate::review_guide) fn stop(
        path: &str,
        side: Side,
        start_line: u32,
        end_line: u32,
    ) -> Stop {
        Stop {
            id: "main-stop".into(),
            claim: "The new value is used by callers.".into(),
            explanation: String::new(),
            path: path.into(),
            side,
            start_line,
            end_line,
            symbol: None,
            test: None,
            untested: false,
            risk: None,
            audit: Default::default(),
            followups: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::model::FileStatus;

    use super::fixtures::{file, stop};
    use super::*;

    #[test]
    fn block_text_uses_only_the_selected_side_and_preserves_tabs() {
        let files = [file(
            "src/lib.rs",
            FileStatus::Modified,
            "@@ -10,3 +14,3 @@\n before\n-old\n+\tnew\n after\n",
        )];
        let old = resolve_stop(&stop("src/lib.rs", Side::Old, 10, 12), &files).unwrap();
        let new = resolve_stop(&stop("src/lib.rs", Side::New, 14, 16), &files).unwrap();
        assert_eq!((old.first_line, old.last_line), (0, 3));
        assert_eq!((new.first_line, new.last_line), (0, 3));
        assert_eq!(block_text(&files, &old), ["before", "old", "after"]);
        assert_eq!(block_text(&files, &new), ["before", "\tnew", "after"]);
    }

    #[test]
    fn resolution_errors_explain_missing_paths_nearest_ranges_and_crossings() {
        let files = [file(
            "src/lib.rs",
            FileStatus::Modified,
            "@@ -10,2 +14,2 @@\n a\n-old\n+new\n@@ -44 +50 @@\n-c\n+d\n",
        )];
        let missing = resolve_stop(&stop("missing", Side::Old, 1, 2), &files).unwrap_err();
        assert!(missing.contains("missing (old side, lines 1-2): path is not in the diff"));
        let outside = resolve_stop(&stop("src/lib.rs", Side::New, 47, 48), &files).unwrap_err();
        assert!(outside.contains("nearest hunk ranges: 50-50, 14-15"));
        let crossing = resolve_stop(&stop("src/lib.rs", Side::New, 15, 50), &files).unwrap_err();
        assert!(crossing.contains("range crosses hunks"));
        assert!(crossing.contains("nearest hunk ranges: 14-15, 50-50"));
    }

    #[test]
    fn relocation_keeps_exact_block_and_prefers_the_nearest_match() {
        let stop = stop("src/lib.rs", Side::New, 10, 11);
        let original = [file(
            "src/lib.rs",
            FileStatus::Modified,
            "@@ -10,2 +10,2 @@\n-old\n+\tnew\n tail\n",
        )];
        let block = block_text(&original, &resolve_stop(&stop, &original).unwrap());
        let moved = [file(
            "src/lib.rs",
            FileStatus::Modified,
            "@@ -12,2 +12,3 @@\n+above\n-old\n+\tnew\n tail\n@@ -25 +26,2 @@\n+\tnew\n tail\n",
        )];
        let matched = relocate(&block, &stop, &moved).unwrap();
        assert_eq!(
            (matched.hunk, matched.first_line, matched.last_line),
            (0, 2, 3)
        );
        assert_eq!(block_text(&moved, &matched), block);
    }

    #[test]
    fn equally_near_relocations_are_ambiguous() {
        let files = [file(
            "src/lib.rs",
            FileStatus::Modified,
            "@@ -6,0 +7 @@\n+same\n@@ -11,0 +13 @@\n+same\n",
        )];
        assert_eq!(
            relocate(
                &["same".into()],
                &stop("src/lib.rs", Side::New, 10, 10),
                &files
            ),
            None
        );
    }
}
