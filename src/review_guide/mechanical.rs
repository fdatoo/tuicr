//! Shape-based supporting groups and risks. These labels say what changed,
//! not whether the change is safe. Unrecognized changes stay unclaimed.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use crate::model::{DiffFile, DiffHunk, DiffLine, FileStatus, LineOrigin};

use super::resolve::{ResolvedStop, hunk_ref, line_number, resolve_stop, source_text, symbol};
use super::{
    Audit, Basis, HunkRef, ReviewGuide, Risk, RiskKind, Side, Stop, Story, SupportingGroup,
};

const UNCOVERED: &str = "not covered by the guide";
const RISK_STORY: &str = "Risk items";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MechanicalRisk {
    pub path: String,
    pub side: Side,
    pub start_line: u32,
    pub end_line: u32,
    pub kind: RiskKind,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mechanical {
    pub risks: Vec<MechanicalRisk>,
    pub groups: Vec<SupportingGroup>,
}

enum Shape {
    Docs,
    NewTests,
    PureMove,
    Generated,
    Binary,
}

impl Shape {
    const ALL: [(Self, &'static str, &'static str); 5] = [
        (
            Self::Docs,
            "docs",
            "Documentation files identified by path and markup format.",
        ),
        (
            Self::NewTests,
            "new tests",
            "New files at conventional test paths.",
        ),
        (
            Self::PureMove,
            "pure moves",
            "Renames with no text changes in the reviewed diff.",
        ),
        (
            Self::Generated,
            "generated files",
            "Recognized lockfiles or files with a known generator header and output path.",
        ),
        (
            Self::Binary,
            "binary files",
            "Files reported as binary by the diff backend.",
        ),
    ];
}

pub fn classify(files: &[DiffFile]) -> Mechanical {
    let mut risks = Vec::new();
    let mut grouped: [Vec<_>; Shape::ALL.len()] = Default::default();
    for file in files.iter().filter(|file| !file.is_commit_message) {
        let config = paths(file).any(is_config_path);
        let test = paths(file).any(is_test_path);
        if let Some(shape) = supporting_shape(file, config, test) {
            grouped[shape as usize].extend(changed_hunks(file));
            continue;
        }
        let path = file.display_path().to_string_lossy();
        let code = paths(file).any(is_code_path);
        for hunk in &file.hunks {
            classify_hunk(&mut risks, &path, hunk, test, config, code);
        }
    }
    let groups = Shape::ALL
        .into_iter()
        .zip(grouped)
        .filter(|(_, hunks)| !hunks.is_empty())
        .map(|((_, label, note), hunks)| SupportingGroup {
            label: label.into(),
            basis: Basis::Mechanical,
            note: note.into(),
            hunks,
        })
        .collect();
    Mechanical { risks, groups }
}

fn supporting_shape(file: &DiffFile, config: bool, test: bool) -> Option<Shape> {
    if config || file.is_too_large {
        None
    } else if file.is_binary {
        Some(Shape::Binary)
    } else if file.status == FileStatus::Renamed
        && file
            .old_path
            .as_ref()
            .zip(file.new_path.as_ref())
            .is_some_and(|(old, new)| old != new)
        && file.hunks.is_empty()
    {
        Some(Shape::PureMove)
    } else if paths(file).all(is_doc_path) {
        Some(Shape::Docs)
    } else if test && file.status == FileStatus::Added {
        Some(Shape::NewTests)
    } else if paths(file).all(is_lockfile) || is_generated_protobuf(file) {
        Some(Shape::Generated)
    } else {
        None
    }
}

fn classify_hunk(
    risks: &mut Vec<MechanicalRisk>,
    path: &str,
    hunk: &DiffHunk,
    test: bool,
    config: bool,
    code: bool,
) {
    let deletion_only = !hunk
        .lines
        .iter()
        .any(|line| line.origin == LineOrigin::Addition);
    for change in hunk
        .lines
        .split(|line| line.origin == LineOrigin::Context)
        .filter(|run| !run.is_empty())
    {
        if test {
            push_risk(
                risks,
                path,
                change,
                Side::Old,
                RiskKind::TestChanged,
                |_| true,
            );
        }
        if config && !push_risk(risks, path, change, Side::New, RiskKind::Config, |_| true) {
            push_risk(risks, path, change, Side::Old, RiskKind::Config, |_| true);
        }
        if code && !test && !config && deletion_only {
            push_risk(
                risks,
                path,
                change,
                Side::Old,
                RiskKind::CodeRemoved,
                |_| true,
            );
        }
        if code
            && !test
            && !config
            && !push_risk(
                risks,
                path,
                change,
                Side::New,
                RiskKind::ErrorHandling,
                exception_syntax,
            )
        {
            push_risk(
                risks,
                path,
                change,
                Side::Old,
                RiskKind::ErrorHandling,
                exception_syntax,
            );
        }
    }
}

/// Return whether this change run had a matching line on the chosen side.
fn push_risk(
    risks: &mut Vec<MechanicalRisk>,
    path: &str,
    change: &[DiffLine],
    side: Side,
    kind: RiskKind,
    matches: impl Fn(&str) -> bool,
) -> bool {
    let mut numbers = change
        .iter()
        .filter_map(|line| line_number(line, side).filter(|_| matches(source_text(line))));
    let Some(start_line) = numbers.next() else {
        return false;
    };
    let end_line = numbers.next_back().unwrap_or(start_line);
    risks.push(MechanicalRisk {
        path: path.into(),
        side,
        start_line,
        end_line,
        kind,
        reason: risk_reason(kind).into(),
    });
    true
}

fn paths(file: &DiffFile) -> impl Iterator<Item = &Path> {
    file.old_path
        .as_deref()
        .into_iter()
        .chain(file.new_path.as_deref())
}

fn name(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
}

fn extension(path: &Path) -> &str {
    path.extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
}

fn is_doc_path(path: &Path) -> bool {
    let file = name(path);
    // Instruction files can drive tools, despite using a documentation format.
    !path.starts_with(".github")
        && !matches!(file, "AGENTS.md" | "CLAUDE.md" | "SKILL.md" | "reviewer.md")
        && !file.ends_with(".instructions.md")
        && (matches!(extension(path), "md" | "rst" | "adoc")
            || matches!(file, "README" | "CHANGELOG" | "README.txt"))
}

fn is_test_path(path: &Path) -> bool {
    let file = name(path);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    match extension(path) {
        "go" | "rb" => stem.ends_with("_test"),
        "py" => file.starts_with("test_") || stem.ends_with("_test"),
        // A top-level Rust integration test is a crate, not a helper submodule.
        "rs" => {
            stem.ends_with("_test")
                || stem == "tests"
                || path
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|dir| dir == "tests")
        }
        "js" | "jsx" | "ts" | "tsx" => stem.ends_with(".test") || stem.ends_with(".spec"),
        "java" => stem.ends_with("Test"),
        _ => false,
    }
}

fn is_config_path(path: &Path) -> bool {
    path.starts_with(".github/workflows")
        || path.starts_with(".github/actions")
        || path == Path::new(".circleci/config.yml")
        || matches!(
            name(path),
            ".gitlab-ci.yml"
                | "Jenkinsfile"
                | "CODEOWNERS"
                | ".gitmodules"
                | "Makefile"
                | "CMakeLists.txt"
                | "Dockerfile"
                | "Cargo.toml"
                | "package.json"
                | "go.mod"
                | "pyproject.toml"
                | "requirements.txt"
                | "Gemfile"
                | "pom.xml"
                | "build.gradle"
                | "build.gradle.kts"
                | "settings.gradle"
                | "settings.gradle.kts"
                | "rbac.yaml"
                | "rbac.yml"
                | "permissions.yaml"
                | "permissions.yml"
        )
}

fn is_lockfile(path: &Path) -> bool {
    matches!(
        name(path),
        "Cargo.lock"
            | "go.sum"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "Gemfile.lock"
            | "poetry.lock"
            | "uv.lock"
            | "composer.lock"
    )
}

fn is_generated_protobuf(file: &DiffFile) -> bool {
    // A suffix alone can hide hand-written code. Require a known protoc marker
    // near the top on each existing side when the header is visible in the diff.
    paths(file).all(|path| name(path).ends_with(".pb.go"))
        && (file.status == FileStatus::Added || has_protobuf_header(file, Side::Old))
        && (file.status == FileStatus::Deleted || has_protobuf_header(file, Side::New))
}

fn has_protobuf_header(file: &DiffFile, side: Side) -> bool {
    file.hunks.first().is_some_and(|hunk| {
        hunk.lines.iter().any(|line| {
            line_number(line, side).is_some_and(|number| number <= 8)
                && matches!(
                    source_text(line),
                    "// Code generated by protoc-gen-go. DO NOT EDIT."
                        | "// Code generated by protoc-gen-go-grpc. DO NOT EDIT."
                )
        })
    })
}

fn is_code_path(path: &Path) -> bool {
    // Unknown inputs aren't assumed to be source (or documentation).
    matches!(
        extension(path),
        "rs" | "go"
            | "py"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "java"
            | "kt"
            | "rb"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "cs"
            | "swift"
            | "sh"
            | "php"
    )
}

fn exception_syntax(text: &str) -> bool {
    // Match explicit, line-leading exception syntax shared by several languages,
    // not generic words like `error` in identifiers, logs, strings or comments.
    let text = text.trim();
    let text = text.strip_prefix('}').map_or(text, str::trim_start);
    text.starts_with("catch (")
        || text.starts_with("catch(")
        || text == "except:"
        || (text.starts_with("except ") && text.ends_with(':'))
        || text.starts_with("throw ")
        || text.starts_with("raise ")
}

fn risk_reason(kind: RiskKind) -> &'static str {
    match kind {
        RiskKind::TestChanged => "Existing test lines were edited or removed.",
        RiskKind::CodeRemoved => "This hunk deletes code without adding replacement lines.",
        RiskKind::Config => {
            "This change touches build, CI, dependency or permission configuration."
        }
        RiskKind::ErrorHandling => {
            "A changed line contains explicit exception handling or propagation syntax."
        }
        RiskKind::Unexplained => "The description does not account for this change.",
        RiskKind::Rule => "This change was flagged by a review rule.",
        RiskKind::Other => "This change was flagged for manual review.",
    }
}

fn risk_claim(kind: RiskKind) -> &'static str {
    match kind {
        RiskKind::TestChanged => {
            "Check that these test changes preserve the coverage that is still required."
        }
        RiskKind::CodeRemoved => "Check that the removed code is no longer needed by its callers.",
        RiskKind::Config => "Check that this configuration change has only the intended effects.",
        RiskKind::ErrorHandling => {
            "Check that failures still reach the right caller or recovery path."
        }
        _ => "Check that this change is intentional.",
    }
}

/// Complete a guide without trusting model-supplied mechanical labels. Stops
/// own hunks first; only local shape checks can make a group dismissible.
pub fn complete(guide: &mut ReviewGuide, files: &[DiffFile], mechanical: &Mechanical) {
    add_risk_stops(guide, files, &mechanical.risks);
    complete_supporting(guide, files, mechanical);
}

pub(crate) fn complete_supporting(
    guide: &mut ReviewGuide,
    files: &[DiffFile],
    mechanical: &Mechanical,
) {
    let previous = std::mem::take(&mut guide.supporting);
    let mut available: BTreeSet<_> = uncovered_hunks(guide, files).into_iter().collect();
    for group in &mechanical.groups {
        let hunks: Vec<_> = group
            .hunks
            .iter()
            .filter_map(|hunk| available.take(hunk))
            .collect();
        if !hunks.is_empty() {
            guide.supporting.push(SupportingGroup {
                label: group.label.clone(),
                basis: Basis::Mechanical,
                note: group.note.clone(),
                hunks,
            });
        }
    }
    for mut group in previous {
        if group.basis == Basis::Claimed && group.label != UNCOVERED {
            group.hunks.retain(|hunk| available.remove(hunk));
            if !group.hunks.is_empty() {
                guide.supporting.push(group);
            }
        }
    }
    if !available.is_empty() {
        guide.supporting.push(SupportingGroup {
            label: UNCOVERED.into(),
            basis: Basis::Claimed,
            note: "These changes have no stop or mechanically verified supporting group.".into(),
            hunks: available.into_iter().collect(),
        });
    }
}

pub fn uncovered_hunks(guide: &ReviewGuide, files: &[DiffFile]) -> Vec<HunkRef> {
    let stopped = stopped_hunks(guide, files);
    let supported: HashSet<_> = guide
        .supporting
        .iter()
        .flat_map(|group| group.hunks.iter())
        .map(|hunk| (hunk.path.as_str(), hunk.old_start, hunk.new_start))
        .collect();
    let mut uncovered = Vec::new();
    for (file_idx, file) in files.iter().enumerate() {
        let path = file.display_path().to_string_lossy();
        if file.hunks.is_empty() && !supported.contains(&(path.as_ref(), 0, 0)) {
            uncovered.push(file_ref(file));
        }
        for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
            if !stopped.contains(&(file_idx, hunk_idx))
                && !supported.contains(&(path.as_ref(), hunk.old_start, hunk.new_start))
                && is_changed(hunk)
            {
                uncovered.push(hunk_ref(file, hunk));
            }
        }
    }
    uncovered
}

/// Unique changed text hunks reached by the guide, versus the total in the diff.
pub(crate) fn stop_coverage(guide: &ReviewGuide, files: &[DiffFile]) -> (usize, usize) {
    let stopped = stopped_hunks(guide, files);
    let (mut covered, mut total) = (0, 0);
    for (file, entry) in files.iter().enumerate() {
        for (hunk, chunk) in entry.hunks.iter().enumerate() {
            if is_changed(chunk) {
                total += 1;
                covered += usize::from(stopped.contains(&(file, hunk)));
            }
        }
    }
    (covered, total)
}

fn stopped_hunks(guide: &ReviewGuide, files: &[DiffFile]) -> HashSet<(usize, usize)> {
    guide
        .stops()
        .filter_map(|(_, stop)| resolve_stop(stop, files).ok())
        .map(|at| (at.file, at.hunk))
        .collect()
}

fn changed_hunks(file: &DiffFile) -> impl Iterator<Item = HunkRef> {
    file.hunks
        .iter()
        .filter(|hunk| is_changed(hunk))
        .map(|hunk| hunk_ref(file, hunk))
        .chain(file.hunks.is_empty().then(|| file_ref(file)))
}

fn is_changed(hunk: &DiffHunk) -> bool {
    hunk.lines
        .iter()
        .any(|line| line.origin != LineOrigin::Context)
}

fn file_ref(file: &DiffFile) -> HunkRef {
    // A no-hunk file (for example binary or metadata-only) is a file-level
    // supporting target. A changed text hunk must have lines on one side.
    HunkRef {
        path: file.display_path().to_string_lossy().into_owned(),
        old_start: 0,
        new_start: 0,
    }
}

fn add_risk_stops(guide: &mut ReviewGuide, files: &[DiffFile], risks: &[MechanicalRisk]) {
    let mut ids: HashSet<_> = guide.stops().map(|(_, stop)| stop.id.clone()).collect();
    let mut stops_by_hunk: HashMap<_, Vec<_>> = HashMap::new();
    for stop in guide.stories.iter_mut().flat_map(|story| &mut story.stops) {
        if let Ok(at) = resolve_stop(stop, files) {
            stops_by_hunk
                .entry((at.file, at.hunk))
                .or_default()
                .push((at, stop));
        }
    }
    let mut generated: Vec<(ResolvedStop, Stop)> = Vec::new();
    for risk in risks {
        let mut stop = Stop {
            id: String::new(),
            claim: risk_claim(risk.kind).into(),
            explanation: risk.reason.clone(),
            path: risk.path.clone(),
            side: risk.side,
            start_line: risk.start_line,
            end_line: risk.end_line,
            symbol: None,
            test: None,
            untested: true,
            risk: Some(as_risk(risk)),
            audit: Audit::Unchecked,
            followups: Vec::new(),
        };
        let Ok(at) = resolve_stop(&stop, files) else {
            continue;
        };
        let hunk = &files[at.file].hunks[at.hunk];
        if let Some((_, existing)) = stops_by_hunk
            .get_mut(&(at.file, at.hunk))
            .and_then(|stops| {
                stops
                    .iter_mut()
                    .find(|(existing, _)| covers(*existing, at, hunk))
            })
        {
            existing.risk.get_or_insert_with(|| as_risk(risk));
            continue;
        }
        if generated
            .iter()
            .any(|(existing, _)| covers(*existing, at, hunk))
        {
            continue;
        }
        let base = format!(
            "risk:{:?}:{:?}:{}-{}:{}",
            risk.kind, risk.side, risk.start_line, risk.end_line, risk.path
        );
        stop.id.clone_from(&base);
        let mut suffix = 2;
        while !ids.insert(stop.id.clone()) {
            stop.id = format!("{base}#{suffix}");
            suffix += 1;
        }
        stop.symbol = symbol(&hunk.header)
            .filter(|text| !is_import_header(text))
            .map(str::to_owned);
        generated.push((at, stop));
    }
    if !generated.is_empty() {
        let generated = generated.into_iter().map(|(_, stop)| stop);
        if let Some(last) = guide
            .stories
            .last_mut()
            .filter(|story| story.title == RISK_STORY)
        {
            last.stops.extend(generated);
        } else {
            guide.stories.push(Story {
                title: RISK_STORY.into(),
                summary: "Changes flagged by tuicr that were not covered by the guide's stops."
                    .into(),
                stops: generated.collect(),
            });
        }
    }
}

fn covers(stop: ResolvedStop, risk: ResolvedStop, hunk: &DiffHunk) -> bool {
    if (stop.file, stop.hunk) != (risk.file, risk.hunk) {
        return false;
    }
    if stop.first_line <= risk.last_line && risk.first_line <= stop.last_line {
        return true;
    }
    if stop.side == risk.side {
        return false;
    }
    // Opposite sides of a replacement are adjacent in patch rows even
    // though the source line numbers can differ. Context separates runs.
    let (first, last) = if stop.last_line < risk.first_line {
        (stop.last_line, risk.first_line)
    } else {
        (risk.last_line, stop.first_line)
    };
    hunk.lines[first..=last]
        .iter()
        .all(|line| line.origin != LineOrigin::Context)
}

fn is_import_header(text: &str) -> bool {
    [
        "use ", "pub use ", "import ", "from ", "#include", "package ", "using ", "require ",
        "require(",
    ]
    .iter()
    .any(|prefix| text.starts_with(prefix))
}

fn as_risk(risk: &MechanicalRisk) -> Risk {
    Risk {
        kind: risk.kind,
        reason: risk.reason.clone(),
    }
}

#[cfg(test)]
mod tests {
    use crate::model::FileStatus;
    use crate::review_guide::GUIDE_VERSION;
    use crate::review_guide::resolve::fixtures::{file, stop};

    use super::*;

    const REPLACED: &str = "@@ -10 +10 @@\n-before\n+after\n";

    fn rename(path: &str, patch: &str) -> DiffFile {
        let mut result = file(path, FileStatus::Renamed, patch);
        result.old_path = Some("old.rs".into());
        result
    }

    #[test]
    fn a_rename_with_edits_is_not_a_pure_move() {
        let mechanical = classify(&[rename("moved.rs", ""), rename("rewritten.rs", REPLACED)]);
        assert_eq!(mechanical.groups.len(), 1);
        assert_eq!(mechanical.groups[0].label, "pure moves");
        assert_eq!(
            mechanical.groups[0].hunks,
            [HunkRef {
                path: "moved.rs".into(),
                old_start: 0,
                new_start: 0,
            }]
        );
    }

    #[test]
    fn new_tests_are_supporting_but_edits_and_deletions_of_existing_tests_are_risks() {
        let mechanical = classify(&[
            file(
                "tests/new.rs",
                FileStatus::Added,
                "@@ -0,0 +1 @@\n+assert!(ready);\n",
            ),
            file("tests/existing.rs", FileStatus::Modified, REPLACED),
            file(
                "parser_test.go",
                FileStatus::Deleted,
                "@@ -1 +0,0 @@\n-old test\n",
            ),
            file(
                "tests/more.rs",
                FileStatus::Modified,
                "@@ -1,0 +2 @@\n+another case\n",
            ),
        ]);
        assert_eq!(mechanical.groups.len(), 1);
        assert_eq!(mechanical.groups[0].label, "new tests");
        assert_eq!(mechanical.groups[0].hunks[0].path, "tests/new.rs");
        assert_eq!(
            mechanical
                .risks
                .iter()
                .map(|risk| (risk.path.as_str(), risk.side, risk.kind))
                .collect::<Vec<_>>(),
            [
                ("tests/existing.rs", Side::Old, RiskKind::TestChanged),
                ("parser_test.go", Side::Old, RiskKind::TestChanged),
            ]
        );
    }

    #[test]
    fn flags_explicit_error_syntax_not_logs_or_comments() {
        let mechanical = classify(&[file(
            "fetch.py",
            FileStatus::Modified,
            "@@ -1,0 +1,4 @@\n+# raise ValueError()\n+logger.error('failed')\n+except IOError:\n+    raise ValueError()\n",
        )]);
        assert_eq!(mechanical.risks.len(), 1);
        let risk = &mechanical.risks[0];
        assert_eq!(
            (risk.kind, risk.side, risk.start_line, risk.end_line),
            (RiskKind::ErrorHandling, Side::New, 3, 4)
        );
    }

    #[test]
    fn completion_gives_every_change_one_owner_and_is_idempotent() {
        let mut image = file("diagram.png", FileStatus::Modified, "");
        image.is_binary = true;
        let files = [
            file("src/logic.rs", FileStatus::Modified, REPLACED),
            file("README.md", FileStatus::Modified, REPLACED),
            file(
                "Cargo.toml",
                FileStatus::Modified,
                "@@ -10,3 +10,3 @@\n-before\n+after\n unchanged\n-old2\n+new2\n",
            ),
            file(
                "src/dead.rs",
                FileStatus::Deleted,
                "@@ -10 +0,0 @@ use crate::old;\n-old()\n",
            ),
            file("src/unexplained.rs", FileStatus::Modified, REPLACED),
            file("src/wiring.rs", FileStatus::Modified, REPLACED),
            image,
        ];
        let config_stop = Stop {
            id: "config".into(),
            ..stop("Cargo.toml", Side::New, 10, 10)
        };
        let mut guide = ReviewGuide {
            version: GUIDE_VERSION,
            diff: "digest".into(),
            summary: String::new(),
            stories: vec![Story {
                title: "Behavior".into(),
                summary: String::new(),
                stops: vec![stop("src/logic.rs", Side::New, 10, 10), config_stop],
            }],
            supporting: vec![
                SupportingGroup {
                    label: "forged docs label".into(),
                    basis: Basis::Mechanical,
                    note: String::new(),
                    hunks: vec![hunk_ref(&files[4], &files[4].hunks[0])],
                },
                SupportingGroup {
                    label: "wiring".into(),
                    basis: Basis::Claimed,
                    note: String::new(),
                    hunks: [0, 1, 3, 5]
                        .map(|i| hunk_ref(&files[i], &files[i].hunks[0]))
                        .into(),
                },
            ],
            estimate_minutes: None,
            provenance: Default::default(),
        };
        let mechanical = classify(&files);
        let mut fresh = guide.clone();
        complete(&mut guide, &files, &mechanical);
        assert!(uncovered_hunks(&guide, &files).is_empty());
        assert_eq!(guide.stop_count(), 4);
        assert_eq!(
            guide.stop("config").unwrap().risk.as_ref().unwrap().kind,
            RiskKind::Config
        );
        assert_eq!(guide.stories[1].title, RISK_STORY);
        let outside_config_stop = &guide.stories[1].stops[0];
        assert_eq!(
            (
                outside_config_stop.path.as_str(),
                outside_config_stop.start_line
            ),
            ("Cargo.toml", 12)
        );
        assert_eq!(
            outside_config_stop.risk.as_ref().unwrap().kind,
            RiskKind::Config
        );
        let removal = &guide.stories[1].stops[1];
        assert_eq!(removal.path, "src/dead.rs");
        assert_eq!(removal.risk.as_ref().unwrap().kind, RiskKind::CodeRemoved);
        assert!(removal.untested);
        assert!(removal.symbol.is_none());
        assert!(removal.needs_explicit_verdict());
        assert_eq!(
            guide
                .supporting
                .iter()
                .map(|group| (
                    group.label.as_str(),
                    group.basis,
                    group.hunks[0].path.as_str(),
                    group.hunks.len()
                ))
                .collect::<Vec<_>>(),
            [
                ("docs", Basis::Mechanical, "README.md", 1),
                ("binary files", Basis::Mechanical, "diagram.png", 1),
                ("wiring", Basis::Claimed, "src/wiring.rs", 1),
                (UNCOVERED, Basis::Claimed, "src/unexplained.rs", 1),
            ]
        );
        let once = guide.clone();
        complete(&mut guide, &files, &mechanical);
        assert_eq!(guide, once);
        complete(&mut fresh, &files, &mechanical);
        assert_eq!(guide, fresh);
    }
}
