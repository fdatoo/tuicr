//! Compact, source-lined inventory shared by preparation and guide authors.

use std::fmt::Write as _;

use crate::model::{DiffFile, LineOrigin};

use super::resolve::{source_text, symbol};

const BYTE_BUDGET: usize = 256 * 1024;
const SYMBOL_BYTES: usize = 120;
const SAMPLE_LINES: usize = 3;
const SAMPLE_LINE_BYTES: usize = 160;

pub fn outline(files: &[DiffFile], digest: &str) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "diff: {digest}");
    let sample_bytes = sample_budget(files, output.len());
    for file in files {
        let (added, deleted) = file.stat();
        let status = file.status.as_char();
        let annotation = match (file.is_binary, file.is_too_large) {
            (true, _) => " (binary)",
            (_, true) => " (too large to show)",
            _ => "",
        };
        let _ = writeln!(
            output,
            "{status} {:?} +{added} -{deleted}{annotation}",
            file.display_path()
        );
        for hunk in &file.hunks {
            let _ = write!(
                output,
                "  @@ -{},{} +{},{} @@",
                hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
            );
            if let Some(symbol) = symbol(&hunk.header) {
                output.push(' ');
                push_clipped(&mut output, symbol, SYMBOL_BYTES);
            }
            output.push('\n');
            if sample_bytes == 0 {
                continue;
            }
            let changed = hunk.lines.iter().filter_map(|line| {
                let sign = match line.origin {
                    LineOrigin::Addition => '+',
                    LineOrigin::Deletion => '-',
                    LineOrigin::Context => return None,
                };
                Some((sign, source_text(line)))
            });
            for (sign, text) in changed.take(SAMPLE_LINES) {
                output.push_str("    ");
                output.push(sign);
                push_clipped(&mut output, text, sample_bytes);
                output.push('\n');
            }
        }
    }
    output
}

fn sample_budget(files: &[DiffFile], digest_line_bytes: usize) -> usize {
    let mut hunk_count = 0;
    let mut mandatory_bytes = digest_line_bytes;
    for file in files {
        hunk_count += file.hunks.len();
        // Debug-quoted paths may grow when escaping control characters. In
        // addition to the path, reserve room for counts, status and annotation.
        mandatory_bytes += file.display_path().as_os_str().len() * 6 + 72;
        for hunk in &file.hunks {
            // Four u32 values use up to 40 digits. Reserve the separators too.
            mandatory_bytes +=
                54 + symbol(&hunk.header).map_or(0, |text| 1 + text.len().min(SYMBOL_BYTES));
        }
    }
    // Only samples compete for the remaining budget. Mandatory metadata may
    // itself exceed the budget for huge diffs; never drop a file or hunk row.
    let bytes = (BYTE_BUDGET.saturating_sub(mandatory_bytes) / hunk_count.max(1) / SAMPLE_LINES)
        .saturating_sub(6) // indentation, sign and newline
        .min(SAMPLE_LINE_BYTES);
    if bytes <= '…'.len_utf8() { 0 } else { bytes }
}

fn push_clipped(output: &mut String, text: &str, bytes: usize) {
    if text.len() <= bytes {
        output.push_str(text);
        return;
    }
    let mut end = bytes - '…'.len_utf8();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    output.push_str(&text[..end]);
    output.push('…');
}

#[cfg(test)]
mod tests {
    use crate::model::FileStatus;
    use crate::review_guide::resolve::fixtures::file;

    use super::*;

    #[test]
    fn outline_has_digest_status_counts_ranges_symbols_and_changed_lines() {
        let files = [file(
            "src/renamed.rs",
            FileStatus::Renamed,
            "@@ -4,2 +7,2 @@ fn parse()\n context is omitted\n-before\n+after\n",
        )];
        assert_eq!(
            outline(&files, "sha256:test"),
            concat!(
                "diff: sha256:test\n",
                "R \"src/renamed.rs\" +1 -1\n",
                "  @@ -4,2 +7,2 @@ fn parse()\n",
                "    -before\n",
                "    +after\n",
            )
        );
    }

    #[test]
    fn outline_limits_large_samples_without_dropping_any_file_or_hunk() {
        let make_patch = |file_index: u32| {
            let sample = "界".repeat(250);
            (0..3).map(|hunk_index| {
                let start = file_index * 30 + hunk_index * 10 + 1;
                format!("@@ -{start},2 +{start},2 @@ f_{file_index}_{hunk_index}\n-old {sample}\n-old {sample}\n+new {sample}\n+new {sample}\n")
            }).collect::<String>()
        };
        let files: Vec<_> = (0..300)
            .map(|index| {
                file(
                    &format!("src/file-{index}.rs"),
                    FileStatus::Modified,
                    &make_patch(index),
                )
            })
            .collect();
        let output = outline(&files, "fixture-digest");
        assert!(output.len() <= BYTE_BUDGET);
        assert_eq!(
            output.lines().filter(|line| line.starts_with("M ")).count(),
            300
        );
        assert_eq!(
            output
                .lines()
                .filter(|line| line.starts_with("  @@ "))
                .count(),
            900
        );
        assert!(output.contains("  @@ -8991,2 +8991,2 @@ f_299_2\n"));
        assert!(output.contains('…'));
        assert!(!output.contains(&"界".repeat(SAMPLE_LINE_BYTES / '界'.len_utf8())));
    }
}
