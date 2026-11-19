use std::fmt::Write as _;

use crate::model::{DiffFile, LineOrigin};

const PATCH_BUDGET: usize = 96 * 1024;

pub(super) struct Evidence {
    pub(super) text: String,
    pub(super) limits: Vec<&'static str>,
}

impl Evidence {
    pub(super) fn collect(files: &[DiffFile]) -> Self {
        let mut evidence = Self {
            text: String::new(),
            limits: Vec::new(),
        };
        evidence.patch(files);
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
}
