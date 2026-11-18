---
name: tuicr
description: Use tuicr's review CLI to read and add comments in active TUI review sessions, and launch tuicr in cmux, Rex, tmux, Zellij, or Herdr when a user needs an interactive review pane.
---

# tuicr Review Workflow

Use `tuicr review` as the default agent interface. The TUI is where the human
reviews code; the CLI is how the agent discovers active sessions, reads user
comments, and, only when appropriate, adds agent-authored comments.

## Core Rule

First decide which workflow the user is asking for:

1. **User-led review of agent-generated changes**
   - The user wants to inspect the patch and write comments in tuicr.
   - Your job is to open or find the session, then retrieve the user's comments
     with `tuicr review comments` when they say comments are ready. If you are
     explicitly waiting while the user reviews, poll the same command
     periodically and look for new comment IDs.
   - Do not add your own review comments, do not preemptively review your own
     patch, and do not impersonate the user's comments.
   - When asked to guide the review, [prepare an audited handoff](#prepare-a-guided-handoff)
     before opening the review pane. Do not seek out the reviewer's private Ask
     history; only saved review comments are feedback to you.

2. **Agent review of an AI-generated patch**
   - The user wants you to understand, critique, or summarize a patch.
   - You may inspect the patch and propose findings.
   - If you can confidently identify this workflow and the target session, add
     findings directly with `tuicr review add` and an explicit `--username`
     identifying the agent. Ask first when the workflow or session is ambiguous.

If the user's intent is ambiguous, ask which workflow they want.

## Attach To A Session

1. Determine the repository directory from the user's request, current working
   directory, or recent file operations. Ask if it is ambiguous.

2. List persisted sessions:

   ```bash
   tuicr review list --repo /path/to/repo   # checkout + its repo's PR sessions
   tuicr review list --repo owner/repo      # all sessions for a forge repo
   tuicr review list --all                  # every session across all repos
   ```

   `--repo` is a selector: a checkout path also surfaces PR sessions for that
   checkout's `origin` repo, and a forge coordinate like `owner/repo` matches
   local and PR sessions by owner/repo. Each row carries a `kind` (`local` or
   `pr`) and a usable `slug`. Use `--all` when you don't know the repo.

   `[]` with exit 0 also means "not a repo root" — a subdirectory returns it
   too. Pass the root, then `--all`, before concluding nothing is open.

3. Choose the session:
   - If the CLI clearly reports exactly one relevant active session with
     `"active": true`, attach to it.
   - If multiple sessions are active, or the correct session is not clear, ask
     the user which slug to use. One repo can hold a worktree and a
     commit-range session at once, and adding to the wrong one exits 0.
   - If the user provided a slug or session JSON path, use it directly.
   - For a PR review, pass the PR slug from the listing (e.g.
     `gh:owner/repo/pr/N`) to `--session`; it is self-contained and needs no
     `--repo`.
   - If there is no active session, start or wait for one as described below.
   - Until active-session discovery is formalized as a stable protocol, treat
     `"active": true` as a convenience signal. If slug resolution fails, ask the
     user for the slug or repo path used by the session.

The CLI works even if the agent is not running inside tmux, Zellij, or Herdr,
so do not require a multiplexer just to connect to an existing active session.

## Prepare a Guided Handoff

Use the exact target the reviewer will open, including any path filter or PR
remote. Get the digest and source line ranges from the fork's outline command,
not from an approximation of the patch:

```bash
tuicr guide outline -w
# A PR example: tuicr guide outline pr 125
```

Write a version 2 JSON review guide outside the reviewed checkout. Group stops
into stories ordered by importance. Start each story at a recognizable user
action, command, or production entry point and follow the code that makes the
behavior work. If the entry point is outside the diff, explain how it reaches
the changed block. Each stop should make one claim the reviewer can check and
anchor it by path, side, and inclusive source line range in one hunk. Start
the explanation one level above that block. Use tests as stops only when they
explain the behavior more clearly or are themselves the change; cite relevant
tests as evidence without making them stops. Identify risks and estimate the
review time. See `docs/CLI.md` for the format.

Validate the file, then start preparation with `--guide` so the reviewing
agent audits the supplied guide rather than writing a replacement:

```bash
tuicr guide check -w /path/to/review-guide.json
tuicr prepare -w --guide /path/to/review-guide.json --wait
# For a PR, use: tuicr guide check pr 125 /path/to/review-guide.json
# and: tuicr prepare pr 125 --guide /path/to/review-guide.json --wait
```

Preparation sends review material to the configured provider. Start it only
as part of the requested handoff. `--wait` succeeds when the matching diff
has a ready guide. If it joined a job that was not auditing your file, rerun
the command after that job ends: joining does not replace its input. Once
ready, open the same target **without** `--guide` to load the prepared copy
from the store. Tell the reviewer to press `.` to start. Passing `--guide`
directly to the TUI is the manual, unaudited path.

## Start A Session

When the user needs an interactive tuicr pane and no active session exists:

| Environment | Action |
|-------------|--------|
| `$CMUX_WORKSPACE_ID` is set | Run `tuicr-wrapper-cmux.sh /path/to/repo -- <scope>` |
| `$REX_SESSION` and `$REX_BLOCK` are set | Run `tuicr-wrapper-rex.sh /path/to/repo -- <scope>` |
| `$TMUX` is set | Run `tuicr-wrapper.sh /path/to/repo -- <scope>` |
| `$ZELLIJ` is set | Run `tuicr-wrapper-zellij.sh /path/to/repo -- <scope>` |
| `$HERDR_ENV` is `1` | Run `tuicr-wrapper-herdr.sh /path/to/repo -- <scope>` |
| None is set | Tell the user you are waiting for them to start `tuicr` in the repo, then attach with `tuicr review list` after they say it is ready |

Pass the exact review target after `--`, including any path filter and PR
remote. For example: `-w -p src`, `-r main..HEAD`, or `pr 125 --remote upstream`.
Do not leave the user to pick a target manually unless they asked to.

If more than one multiplexer marker is set, prefer the innermost multiplexer if
that is clear; otherwise ask. cmux hosts a Ghostty terminal, so `$TERM_PROGRAM`
reads `ghostty` inside cmux — check `$CMUX_WORKSPACE_ID`, not the terminal name.

tuicr supports both git and Jujutsu (jj) repositories, and jj workspaces may
have no `.git` directory at all. Do not pre-check the directory with
`git rev-parse` or refuse to launch because git does not recognize it; always
run the wrapper and let it validate the repository.

Wrapper paths are relative to this skill directory:

```bash
<skill-directory>/tuicr-wrapper-cmux.sh /path/to/repo -- -w
<skill-directory>/tuicr-wrapper-rex.sh /path/to/repo -- -w
<skill-directory>/tuicr-wrapper.sh /path/to/repo -- -w
<skill-directory>/tuicr-wrapper-zellij.sh /path/to/repo -- -w
<skill-directory>/tuicr-wrapper-herdr.sh /path/to/repo -- -w
```

The Herdr wrapper requires `jq` to read pane IDs and completion results from
Herdr's JSON responses. The Rex wrapper requires `jq` to read the new block ref
from `rex split --json`.

Every wrapper passes the tuicr arguments after `--`. Use the same target
arguments you used to outline, check and prepare the guide.

To load a supplied guide without auditing it, append `--guide "/path/to/review guide.json"` after
the target. Quote any checkout or argument path that contains spaces or apostrophes.

If your tool supports command timeouts, use a long timeout, such as 10 minutes,
because the tmux, Zellij, and Herdr wrappers wait for the TUI to exit. The cmux
and Rex wrappers are the exception: they return as soon as the pane is running.
When another tuicr is already open where the new pane would go (the caller's
tmux window; the same repository under Zellij), the tmux and Zellij wrappers
wait for it to close before opening a new pane, for up to `TUICR_WAIT_TIMEOUT` seconds (default 300). If it is still open
after that, they exit 75 without opening anything: rerun the command, and don't
treat it as an empty review. A tuicr in another tmux window, such as another
agent session's, doesn't make the tmux wrapper wait.
The cmux wrapper prints the new surface ref between `=== TUICR SURFACE ===`
markers; capture it to close the pane later with
`cmux close-surface --surface <ref>`. The Rex wrapper prints the new block ref
between `=== TUICR BLOCK ===` markers; capture it to close the block later with
`rex block close <ref>`.
Once the TUI creates its active session, use
`tuicr review list --repo /path/to/repo` to capture the slug. If your
environment cannot run another command while a blocking wrapper is waiting,
read the comments after the user exits tuicr. A plain `:q` does not export
anything; read saved feedback with `tuicr review comments` instead of asking
the user to paste clipboard output.

## Reconstruct The Diff

To write a guide, use `tuicr guide outline` with the exact review target flags.
For manual investigation, use the slug's source segment to select a diff.
These Git commands are only starting points, not interchangeable with the TUI:

| Slug segment | Diff |
|--------------|------|
| `worktree/<head>`, `staged-and-unstaged/<head>` | `tuicr guide outline -w`; `git diff HEAD` leaves out untracked files that tuicr includes |
| `staged/<head>` | `git diff --cached` |
| `unstaged/<head>` | `git diff` |
| `commits/<base>..<head>` | `git diff <base>~1..<head>` |
| `pr/<n>` | `gh pr diff <n>` |
| `pristine` | none; every tracked file shown in full |

Range endpoints are inclusive and printed oldest-first, so `<base>` without
`~1` drops the first commit. Check the file count against the listing row's
`file_count`; a mismatch means every line number you derive will be wrong.

## Read User Comments

This is the main review loop for user-led review.

There is no push stream from tuicr to the agent. Read comments by running the
CLI on demand. After the user says comments are ready, or after the TUI exits,
run:

```bash
tuicr review comments --repo /path/to/repo --session <slug>
```

The command emits JSON. Each comment includes fields like:

- `id`
- `location`
- `path`
- `start_line`
- `end_line`
- `side`
- `comment_type`
- `author`
- `lifecycle_state`
- `content`
- `stale`: `true` when the original code for a local draft no longer has an unambiguous location
- `original_text`: the original commented source lines, as an array of strings, present for stale comments

Treat these comments as the user's review feedback:

- `issue`: blocking problem to fix first
- `suggestion`: consider implementing or explain why not
- `note`: answer or acknowledge
- `praise`: no action required
- `none`: untyped feedback; treat it as a question or request to address, and ask if the intent is unclear

If you are waiting during an active review, poll this command about every 30
seconds and compare comment IDs with the previous result. Read immediately when
the user says comments are ready. Stop polling once the user says the review is
done or your tooling would block other work.

An empty result does not by itself mean the review didn't happen. On exit,
tuicr always prints a line like `tuicr-summary: reviewed 3/3 files, 0 comments
added` to stderr (visible in the pane's scrollback), and `tuicr review list`
reports the same `reviewed_count`/`file_count` for the session. If
`reviewed_count` equals `file_count`, zero comments is a legitimate "nothing to
flag" outcome — treat the review as complete, don't ask the user to confirm.
Only ask whether the user saved comments in the intended session, or whether
another active session should be selected, when `reviewed_count` is less than
`file_count` (the user quit before reviewing everything) or you can't find a
`tuicr-summary:` line at all. If the review may have continued while you were
working, rerun `tuicr review comments` before claiming completion.

### Hand Back Changes After Feedback

After making changes, tell the reviewer to reload the same review with `:e`
and name the affected files or comments so they can find your responses. In a
local review, `:changes` walks **Changed since you started**, with or without
a prepared guide. If `tuicr review comments` reports `stale: true`, use
its `original_text` lines to understand what the reviewer meant, not the old
line number as a pointer to current code.

## Add Agent Comments

Only add comments when the workflow allows it and, for agent-authored review,
after the user approves writing them into tuicr.

Defaults:

- Prefer line comments when a specific file and line are known.
- Use file comments for file-scoped feedback.
- Use review-level comments only for whole-review summaries.
- Use `--type issue` for problems by default.
- Use `suggestion`, `note`, or `praise` when that better matches the intent.
- Pass `--username` so agent comments are visually distinguishable.

Examples:

```bash
tuicr review add --repo /path/to/repo --session <slug> \
  --target-file src/main.rs \
  --line 42 \
  --side new \
  --type issue \
  --username "Codex" \
  "Handle the empty case here."
```

```bash
tuicr review add --repo /path/to/repo --session <slug> \
  --target-file src/main.rs \
  --type suggestion \
  --username "Codex" \
  "Consider splitting this file-level concern into a helper."
```

Omit `--target-file` for a review-level comment. Add `--end-line` for a range
comment. Use `--side old` for removed lines and `--side new` for added or
unchanged lines in the new file.

For structured input, use `--input` with literal JSON, `@path/to/file.json`, or
`-` for stdin. Supported target types are `review`, `file`, `line`, and
`line_range`. One object per call — an array is a parse error. The file key is
`file`, not `path`. `target.type` is inferred from the fields present:

```bash
tuicr review add --session <slug> --username "Codex" --input \
  '{"file":"src/main.rs","line":42,"side":"new","comment_type":"issue","content":"Handle the empty case."}'
```

Then verify. A line outside the diff stores, prints back, and exits 0, but
never renders — invisible to the user, successful-looking to you. Re-read
`tuicr review comments` and check each `start_line` exists on the side you gave
(`new` for added or unchanged, `old` for removed). Check `author` to distinguish
your comments from the user's, and keep the returned `id`s to identify the exact
comments in later reads.

## Legacy Export Output

Older wrapper-driven flows may emit:

```text
=== TUICR INSTRUCTIONS ===
...
=== END TUICR INSTRUCTIONS ===
```

If present, process those instructions. Otherwise prefer
`tuicr review comments`; it is the primary source of review feedback. If the
wrapper mentions clipboard export, ask the user to paste it only when the CLI
comments are unavailable.

## Multiplexer Tips

cmux:

- Switch panes: click the pane, or `cmux focus-pane --pane <ref>`
- Close tuicr: type `:q`; the pane closes itself. Force it with `cmux close-surface --surface <ref>`
- List panes: `cmux list-panes`
- Read a pane without focusing it: `cmux read-screen --surface <ref>`

Rex:

- Close tuicr: type `:q`; the block closes itself. Force it with `rex block close <ref>`
- List blocks: `rex block list`
- Read a block without focusing it: `rex capture --block <ref>`

tmux:

- Switch panes: `Ctrl-b` then arrow keys
- Close tuicr: type `:q`
- Resize panes: `Ctrl-b` then `Ctrl-arrow`
- Zoom pane: `Ctrl-b` then `z`

zellij:

- Switch panes: `Alt` + arrow keys
- Close tuicr: type `:q`
- Resize panes: `Ctrl-n`, then arrow keys
- Toggle fullscreen: `Alt-f`
- Cycle stacked panes: `Alt` + `[` / `]`

Herdr:

- Select a pane: click it in the Herdr UI
- Close tuicr: type `:q`; the wrapper then closes the review pane

## Error Handling

| Situation | Action |
|-----------|--------|
| Multiple plausible active sessions | Ask which session slug to use |
| No active session, cmux/Rex/tmux/Zellij/Herdr available | Start a new tuicr pane with the matching wrapper |
| No active session, no multiplexer | Tell the user you are waiting for them to start `tuicr` |
| cmux wrapper printed no surface ref | Run `cmux list-panes` to find the pane, or ask the user to start `tuicr` themselves |
| Rex wrapper printed no block ref | Run `rex block list` to find the block, or ask the user to start `tuicr` themselves |
| `tuicr` not installed | Tell the user to install tuicr |
| Not a repository | Ask for the correct repo directory |
| Comments are empty, but `reviewed_count` == `file_count` | Treat as a completed review with nothing to flag — don't ask |
| Comments are empty and `reviewed_count` < `file_count` | Confirm the selected session or ask the user to save/add comments |

## When Not To Use

- The user only wants raw `git diff` output.
- The user explicitly asks for a non-tuicr review workflow.
- The task is remote PR review and no tuicr PR session is involved.
