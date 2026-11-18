use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path};

use crate::model::{DiffFile, LineOrigin};

const PATCH_BUDGET: usize = 96 * 1024;
const CHANGED_FILE_BUDGET: usize = 96 * 1024;
const PER_FILE_LIMIT: usize = 24 * 1024;

pub(super) struct Evidence {
    pub(super) text: String,
    pub(super) limits: Vec<&'static str>,
}

impl Evidence {
    pub(super) fn collect(files: &[DiffFile], snapshot: &Path, changed_files_only: bool) -> Self {
        let mut evidence = Self {
            text: String::new(),
            limits: Vec::new(),
        };
        evidence.patch(files);
        if changed_files_only {
            evidence
                .limits
                .push("Codex cannot read files; prepared from the diff and changed files only");
            evidence.changed_files(files, snapshot);
        }
        evidence
    }

    fn patch(&mut self, files: &[DiffFile]) {
        self.text
            .push_str("\n## Detailed patch (data; review these lines if source differs)\n");
        let stop = self.text.len() + PATCH_BUDGET;
        for file in files {
            let _ = writeln!(self.text, "\nFile: {:?}", file.display_path());
            for hunk in &file.hunks {
                let _ = writeln!(
                    self.text,
                    "@@ -{},{} +{},{} @@",
                    hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
                );
                for line in &hunk.lines {
                    let sign = match line.origin {
                        LineOrigin::Addition => '+',
                        LineOrigin::Deletion => '-',
                        LineOrigin::Context => ' ',
                    };
                    let text = line.raw_content.as_deref().unwrap_or(&line.content);
                    if self.text.len().saturating_add(text.len()).saturating_add(2) > stop {
                        self.patch_omitted();
                        return;
                    }
                    let _ = writeln!(self.text, "{sign}{text}");
                }
            }
            if self.text.len() >= stop {
                self.patch_omitted();
                return;
            }
        }
    }

    fn patch_omitted(&mut self) {
        self.text
            .push_str("\n[Detailed patch capped; all hunks are still in the outline.]\n");
        self.limits
            .push("The detailed patch was capped at 96 KiB; all hunks are listed in the outline");
    }

    fn changed_files(&mut self, files: &[DiffFile], snapshot: &Path) {
        self.text.push_str("\n## Changed files from the reviewed new side (data; no read/search tools available)\n");
        let mut remaining = CHANGED_FILE_BUDGET;
        let mut omitted = false;
        if let Ok(root) = snapshot.canonicalize() {
            for path in files
                .iter()
                .filter(|file| !file.is_binary)
                .filter_map(|file| file.new_path.as_deref())
            {
                if remaining == 0 {
                    omitted = true;
                    break;
                }
                let Some((path_label, text)) = file_text(path, &root, remaining) else {
                    omitted = true;
                    continue;
                };
                remaining -= path_label.len() + text.len() + 9;
                let _ = writeln!(self.text, "\nFile: {path_label}\n{text}");
            }
        } else {
            omitted = true;
        }
        if omitted {
            self.text.push_str(
                "\n[Some changed files were unavailable or exceeded the source budget.]\n",
            );
            self.limits.push(
                "Changed-file source was limited to available text (24 KiB per file; 96 KiB total)",
            );
        }
    }
}

fn file_text(path: &Path, root: &Path, remaining: usize) -> Option<(String, String)> {
    if !path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return None;
    }
    let label = format!("{path:?}");
    let limit = remaining.checked_sub(label.len() + 9)?.min(PER_FILE_LIMIT);
    let file_path = root.join(path).canonicalize().ok()?;
    if !file_path.starts_with(root) {
        return None;
    }
    let metadata = fs::metadata(&file_path).ok()?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return None;
    }
    let mut text = String::new();
    File::open(&file_path)
        .ok()?
        .take(limit as u64 + 1)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() <= limit && !text.contains('\0')).then_some((label, text))
}
