# Keybindings

Full reference. Press `?` inside tuicr for an in-app version of this list.

`<leader>` defaults to `;`. Override it with `leader = ","` in `~/.config/tuicr/config.toml`.

## Navigation

| Key | Action |
|-----|--------|
| `j` / `↓` | Scroll down |
| `k` / `↑` | Scroll up |
| `h` / `←` | Scroll left |
| `l` / `→` | Scroll right |
| `Ctrl-d` / `Ctrl-u` | Half page down / up |
| `Ctrl-f` / `Ctrl-b` | Full page down / up |
| `g` / `G` | Go to first / last file |
| `{N}G` | Go to source line N in current file |
| `{N}{motion}` | Vim-style count prefix — repeats `j` / `k` / `h` / `l` / `{` / `}` / `[` / `]` `N` times |
| `{` / `}` | Jump to previous / next file |
| `<leader>p` | Open the file picker (see below) |
| `a` (or `<leader>a`) | Ask privately about the current diff line, visual selection or comment |
| `[` / `]` | Jump to previous / next hunk |
| `m` / `M` | Jump to next / previous comment |
| `/` | Search within diff (case-insensitive regex); matches on diff content are highlighted and the status bar shows the `[current/total]` position (headers, comments, and PR info are searchable but not highlighted) |
| `n` / `N` | Next / previous search match (wraps around) |
| `Esc` | Clear search-match highlighting; the pattern is kept so `n` / `N` still work |
| `Enter` | Expand or collapse hidden context between hunks |
| `zt` | Scroll cursor to top of screen |
| `zz` | Center cursor on screen |
| `zb` | Scroll cursor to bottom of screen |
| `Ctrl-l` | Clear and redraw the whole screen |

`Ctrl-l` works in every mode, including while typing a comment, a `:` command or
a search. Use it when the terminal leaves stale text on screen, for example after
a font size change in a window that wasn't visible. tuicr also redraws in full
whenever the terminal reports a resize.

## File picker

`<leader>p` opens a popup over the diff that lists the files in the review, so you
can jump to one without showing the file list. It lists only the files on screen:
the file tree's `i` / `e` filters and hidden reviewed files (`:set reviewed!`)
apply. The diff's current file starts selected.

| Key | Action |
|-----|--------|
| `j` / `↓` | Select next file |
| `k` / `↑` | Select previous file |
| `/` | Filter paths by case-insensitive substring; the list narrows as you type. `↑` / `↓` move the selection, `Enter` jumps to it and closes, `Esc` drops the filter and keeps the popup open |
| `Enter` | Jump to the selected file, the same as `Enter` in the file tree (single-file view switches to it) |
| `Esc` | Close without moving |

## Review guide

In the normal diff footer, `:prepare` appears when no guide is installed
and preparation isn't running. When a guide is ready, a line above the diff
shows **Review ready**, the stop count and estimate, and `. start`. It stays
there without moving the cursor or taking focus. Press `.` or run `:guide`
to enter. After `:guide off`, **Review paused** offers `. resume`;
`:guide off` again hides that line. Narrow diff panes show the action first.

When `:prepare` installs a replacement in the open review, it pauses the
guide without moving the cursor. `. resume` starts at the first available
unchecked stop. Guide verdicts carry over only when the stop ID and its
anchored block text both match.
If the diff has changed since preparation, the map shows **Earlier version · :prepare**
(**Stale · :prepare** in a narrow pane) until an up-to-date guide is installed.

Stories group stops by behavior. The stop card above the diff states the
claim, names the block and enclosing symbol when known, and shows the
explanation and any risks. The gutter marks the block your verdict covers.
Its diff colors stay unchanged while surrounding rows dim. Moving off the
stop restores normal contrast; `'` returns to the block.
The map gives the current stop an accent bar and up to two lines; the other
stops stay compact so you can scan the story.
Finished stories collapse to their verdict count. Select the heading and
press `Enter` to revisit the story.
Clipped text ends in `…`. Run `:guide detail` to read the full claim,
location, risk or test warnings, and explanation in a private view.
Scroll with `j` / `k` and press `Esc` to return to the diff.

| Diff key | Action |
|---|---|
| `.` | Start or resume; at a stop, record Checks out and advance. If you've moved away, return without recording a verdict |
| `K` | Explicitly record Checks out and advance. Use this instead of `.` to pass a risky, untested or doubtful stop |
| `x` | Open a range comment prefilled from the claim. Saving records Wrong and advances; cancelling leaves the stop undecided |
| `p` | Record Parked and advance; parked is not verified |
| `,` | Go to the previous stop without changing its verdict |
| `'` | Return to the current stop after a detour |
| `t` | Switch between the map and file tree while inside the guide |
| `a` / `<leader>a` | Open Ask; at the stop, use its prepared explanation and follow-ups |

The configured leader and existing bindings take precedence. Verdicts are saved
as they're made; re-entering a guide resumes at the next available stop without
a verdict. Checking out a stop also marks its hunk reviewed only if the block
covers every changed line of that hunk. Stops hidden by file filters are skipped.

### Map sidebar

The map takes the place of the file tree when you enter the guide. It shows
your place, verdicts, risk items, the estimate for checking the stops, and
active review time. The map header, `:coverage`, and the coverage note
report **covers N of M hunks**: how many distinct changed text hunks have
a stop, not how many hunks you verified. When the walk ends, the stop card
reports parked stops and unreviewed supporting hunks. Focus the map with
the normal [panel focus](#panel-focus) keys.

| Map key | Action |
|---|---|
| `j` / `k`, `↓` / `↑` | Select a stop or supporting group |
| `g` / `G` | Select the first / last entry |
| `Enter` | Jump to the selected stop, or show a selected supporting group's note |
| `D` | Dismiss the selected mechanical supporting group. Supporting groups open once every story stop has a verdict; claimed groups cannot be dismissed in bulk |
| `t` | Return to the file tree; `t` there brings back the map |

### Spot-check and coverage

`:spot` walks a repeatable sample of up to five supporting hunks, preferring
groups whose classification is only claimed. `.` or `K` checks out a hunk,
`x` opens a range comment (saving records Wrong), `p` parks it, and `Esc`
stops the walk. Run `:spot` again to resume for the same diff. If a sampled
hunk is filtered out, reveal its file and resume.

`:coverage` opens the read-only record of story and stop verdicts, risk items,
supporting changes and spot-checks, review marks, and active time. Scroll with
`j` / `k`, `↑` / `↓`, or `PgDn` / `PgUp`; `Esc`, `q`, or `?` closes it.
With a guide, Markdown review exports also include a factual coverage note.

### Reviewing changes after feedback

In a local review, reload with `:e` after the author makes changes, then use
`:changes` with or without a prepared guide. It walks **Changed since you
started**, the first story in the map, using the same stop keys. Each stop
requires an explicit `K`, `x`, or `p`, and can quote nearby feedback. The
story compares reviewable text with a private, size-capped baseline captured
at the start of the review, not with HEAD. It does not call a provider.
If the story detects a change outside the current diff, it lists the file
and line (when known) as **no longer in diff**. These entries are
informational: they do not jump to code or need a verdict.

Unchanged stops in an older guide remain navigable; the map tells you
`:prepare` updates it for the new diff. Draft comments move with matching
code when its location is unambiguous. Otherwise the comment stays at its
last location and says **Code changed since this comment**. With diff watch
enabled, the Ask panel stays open during reload; a thread whose anchor
changed is labeled as being about earlier code.

## Private Ask

Press `a` on a diff line, visual range or comment, including a comment
selected in the comment navigator. `<leader>a` and `:ask` also open Ask.
The panel is docked below the diff so you can keep reading the code. At a
guide stop it shows the prepared explanation and highlights the first
prepared follow-up, if there is one. With an empty input, `Enter` answers
the selected follow-up locally. A typed question takes priority. Answers
to new questions stream and are saved in full. After completion, at most
three sentences are shown initially. Prepared follow-ups and replies to
an explicit request for more aren't folded.

| Ask key | Action |
|---|---|
| `Enter` | Send a typed question, or show the selected prepared follow-up with no provider call |
| `↑` / `↓`, `PgUp` / `PgDn` | Scroll the answer or select an entry in history. With prepared follow-ups shown, `↑` / `↓` selects one |
| `Ctrl-o` | Reveal the rest of the latest folded answer locally; if none is hidden, ask the provider for more detail |
| `Ctrl-x` | Cancel the active request and restore its submitted text to that thread's input, keeping any newer text |
| `Ctrl-↑` / `Ctrl-↓` | Grow / shrink the panel |
| Mouse wheel | Scroll the pane under the pointer, including the diff or Ask panel |
| `Shift-↑` / `Shift-↓` | Scroll the diff while Ask has focus |
| `Shift-←` / `Shift-→` | Pan the diff while Ask has focus, when line wrap is off |
| `Tab` / `Shift-Tab` | Switch between the thread and private history |
| `Enter` in history | Open the selected thread and jump to its code if the anchor is still available |
| `Ctrl-g` | Confirm the concern you've typed (or your latest eligible question) as context for drafting |
| `Ctrl-d` | Draft feedback using your typed instruction or a concern from the thread |
| `Ctrl-e` | Open the draft in the normal comment editor. Edit it and save with `Ctrl-s` to make it a review comment |
| `Esc` | Close Ask and return to your review |

About 45 seconds without provider activity ends the request with an error,
without an automatic retry. Only an empty or unusable answer (such as raw
tool-protocol output) is retried once. Failed or cancelled answers aren't
saved.

Answers render Markdown and highlight language-tagged code. The provider runs
in a real checkout of the reviewed revision, with its full git history, and may
only read: it can search and read files (including those outside the diff), run
read-only git or Mercurial commands, and use the MCP servers from your
own Claude or Codex settings. For the working tree, that checkout is your
repository itself. For a past commit or a PR it is a private checkout under
tuicr's cache, built from your clone's objects (a matching clone of the PR's
repository supplies the commit, fetching it if needed) or, for GitHub PRs
without one, from a private blobless clone made with `gh`. Unused checkouts are
removed after a week. If the revision is unavailable (including an unconfirmed
old side), Ask is told so and uses the diff, never an unrelated checkout.

Ask history is stored under `companion/` in tuicr's platform data directory,
separate from `reviews/`. To erase it locally, close tuicr and delete
`companion/`. Private means not included in review comments, Markdown exports,
or forge submissions. Questions, code context, and relevant thread history
can still be sent to the configured AI provider to answer or draft feedback.

## Diff search

Diff `/` search uses the Rust `regex` syntax, case-insensitively by default,
like the file-tree include/exclude filters. For example, `foo|bar` matches either
word, `^fn\s+` finds function declarations, and `foo\.bar` matches a literal dot.
Plain-text searches keep their existing Unicode case folding. The same compiled
pattern drives navigation and highlighting in both diff layouts; side-by-side
columns are matched independently. Zero-width matches (such as `^`) are navigable
but have no characters to highlight. An invalid expression reports an error
without moving the cursor or replacing the previous valid search; `n` / `N` can
still repeat that search. File-tree and help `/` searches remain text searches.

## Help

Press `?` to open help.

| Key | Action |
|-----|--------|
| `/` | Search within help (case-insensitive) |
| `n` / `N` | Next / previous help search match |
| `j` / `k` | Scroll down / up |
| `q` / `?` / `Esc` | Close help |

## File tree

| Key | Action |
|-----|--------|
| `Space` | Toggle expand directory |
| `Enter` | Expand directory / jump to file in diff |
| `o` | Expand all directories |
| `O` | Collapse all directories |
| `i` | Filter to files matching a regex (include) |
| `e` | Filter out files matching a regex (exclude) |
| `I` | Clear the include filter |
| `E` | Clear the exclude filter |
| `/` | Search file paths (substring) |
| `n` / `N` | Next / previous file-path match |

These keys are only active while the file tree is focused — in the diff, `i` still
edits the comment at the cursor and `/` still searches the diff.

### Filtering

`i` and `e` take case-insensitive regexes matched against each file's **full
relative path** (`^src/`, `\.rs$`, `test|spec`). Both can be active at once:
include runs first, then exclude removes from what's left.

A filter is not just a tree view — hidden files also disappear from the diff
pane, from `{`/`}` file navigation, `[`/`]` hunk navigation, and from the file
and `+/-` counts in the header. The tree title reports how much is hidden
(`Files · 2/12 · 12 of 58`), and the active patterns show in its bottom border.

Enter applies, `Esc` cancels, `Ctrl-u` clears the line. Reopening a prompt
pre-fills the pattern already applied, so submitting an emptied buffer is the
same as `I` / `E`. An invalid regex reports the error and leaves the prompt open
so it can be fixed. Comments on hidden files are not deleted and still export —
filters are a view, not an edit.

Filters are session-local: they are not written to the review session and reset
when tuicr restarts, but they survive a `:e` reload.

### Hiding reviewed files

`:set noreviewed` / `:set reviewed` / `:set reviewed!` (or the bare `:reviewed`)
hide or show files already marked reviewed with `r`. There is deliberately no
single-key binding: `H` is a vim motion, and this is a command-only feature. Like
`i` / `e`, hidden files leave the tree, the diff pane, `{`/`}` and `[`/`]`
navigation, `/` search, and the `+/-` counts in the header — so the header reports
the diff still left to review.

Two things stay deliberately unaffected. The tree title keeps counting reviewed
files in its `reviewed/total` fraction, since scoping it to the visible rows would
collapse it to `0/n` exactly when progress matters most; the bottom border carries
a `reviewed hidden` cue instead. And a file whose hunks are individually marked
with `R` is not hidden — only the file-level `r` flag counts.

While hiding, `r` becomes a burn-down loop: marking the file you are reading moves
you to the next unreviewed file, wrapping at the end. A hidden file cannot be
un-reviewed, because `r` can no longer reach it — `:set reviewed` brings it back.
Start a session with them hidden via `show_reviewed = false` in `config.toml`.

### Search

`/` moves only the tree selection to the next matching path, expanding collapsed
parents as needed, and leaves the diff viewport where it was — press `Enter` to
jump the diff there. `n` / `N` step matches and wrap around. Search only ever
considers files that pass the active filters.

## Panel focus

| Key | Action |
|-----|--------|
| `Tab` / `Shift-Tab` | Cycle focus forward / backward between file list, comment navigator, diff, and commit selector |
| `<leader>h` | Move focus one panel left (side-by-side: new side → old side → file list; mirrored when `file_list_position = "right"`) |
| `<leader>l` | Move focus one panel right (side-by-side: file list → diff → old side → new side; mirrored when `file_list_position = "right"`) |
| `<leader>k` | Move focus up (comments to files, or diff/files to commit selector when visible) |
| `<leader>j` | Move focus down (files to comments when visible, otherwise diff) |
| `<leader>e` | Toggle file list visibility |
| `<leader>s` | Toggle commit selector visibility (also `:set commits!`) |
| `<leader>n` | Toggle relative line numbers (also `:set relativenumber!`) |
| `Enter` | Select file (when file list is focused) |

## Comment navigator

Shown below the file tree when local comments or visible remote PR threads exist.

| Key | Action |
|-----|--------|
| `j` / `k` | Move selection |
| `h` / `l` | Scroll rows left / right |
| `Enter` | Jump to selected comment |

## Review actions

| Key | Action |
|-----|--------|
| `r` | Toggle file reviewed (on the PR description: mark it reviewed and collapse it to one row) |
| `R` | Toggle hunk reviewed |
| `c` | Add line comment (or file comment if not on a diff line) |
| `C` | Add file comment |
| `<leader>c` | Add review comment |
| `v` / `V` | Enter visual mode for range comments |
| `dd` | Delete comment at cursor |
| `i` | Edit comment at cursor (vim: text cursor at start) |
| `A` | Edit comment at cursor with text cursor at end (vim mode only) |
| `e` | Open focused file in `$EDITOR` |
| `y` | Copy review to clipboard |
| `Y` | Copy the comment at cursor to clipboard |

In PR review, `r` with the cursor anywhere on the PR description marks the
description reviewed and collapses it to its `═══ ✓ PR #N title` row; `r` on that
row expands it again. The mark is saved with the session like a file's reviewed
mark (`:w`), carries over when the PR gets new commits, and `:clear` resets it.

`e` opens the file at the cursor's line. Terminal editors (`vim`, `nvim`, `nano`, …)
take over the screen and tuicr reloads the diff once they exit. Windowed editors
(`code`, `cursor`, `zed`, `subl`, …) open in their own window while tuicr stays on
screen; reload with `:e` after editing. Adding `--wait` to the editor command opts a
windowed editor back into the blocking behaviour. Set the `editor` config key to
override `$EDITOR`.

In PR review `e` opens the revision under review, not whatever the checkout
happens to hold. When the local file *is* that revision you get the real file and
your edits land in it; otherwise — wrong branch checked out, file added or deleted
by the PR, no checkout at all — tuicr writes a read-only copy of the PR's content
to a private per-user cache directory and opens that, so the text and the line the cursor sits on
match the diff. The status bar names which one you got, e.g.
`Opened src/main.rs @ 1a2b3c4 (read-only PR copy)`. Binary files are only opened
from the checkout, never copied.

## Visual mode

| Key | Action |
|-----|--------|
| `j` / `k` | Extend selection down / up |
| `c` / `Enter` | Create comment for selected range |
| `s` | Start an editable suggestion prefilled with the selected code lines |
| `Esc` / `v` / `V` | Cancel selection |

Range comments must stay within one file: if the selection spans files, `c` / `Enter` warns "Invalid selection" and cancels visual mode without creating a comment.

Suggestions replace whole lines on the new side of a single diff hunk. Selections
across gaps (including deleted-only rows) or expanded context are rejected.
Unchanged context between added code blocks is included in the suggestion.
GitLab suggestions can include at most 100 lines above the final selected line.

## Comment mode

| Key | Action |
|-----|--------|
| `Tab` / `Shift-Tab` | Cycle comment type forward / backward (per `comment_types` order) |
| `Enter` / `Ctrl-Enter` / `Ctrl-s` | Save comment |
| `Shift-Enter` / `Ctrl-j` | Insert newline |
| `←` / `→` | Move cursor left / right |
| `↑` / `↓` | Move cursor up / down one displayed row (default editor) |
| `Ctrl-w` / `Alt-Backspace` / `Cmd-Backspace` | Delete word |
| `Ctrl-u` | Clear line |
| `Esc` / `Ctrl-c` | Cancel |

In the default editor, Up/Down move through wrapped rows and explicit newlines.
The cursor keeps its column where possible and restores it after crossing a
shorter row. Horizontal movement or editing sets the column for subsequent
vertical movement. Up/Down stop at the first or last row.

With `comment_vim = true` the box uses [`edtui`](https://github.com/preiter93/edtui)
modal editing (Normal/Insert/Visual: `hjkl`, `w`/`b`/`e`, `dd`/`D`/`ciw`/`x`,
`u`/`Ctrl-r`, visual `v`+`y`/`d`/`p`). From Normal mode `:w` (or `Enter` twice)
saves and `:q` (or `Esc`/`q` twice) cancels — the first press arms the action
and the header shows a confirm hint. `Alt-Enter` (Option+Enter) accepts and
`Alt-Esc` discards directly (no double-press) — Alt is the one modified
`Enter`/`Esc` that reaches the app across terminals, including browser/web
terminals like zellij web. `Tab` cycles the comment type in Normal
mode and inserts `comment_tab_width` spaces (default 4) in Insert mode; `Ctrl-s`
also saves.

## Commands

In command mode,
`Tab` and `Shift-Tab` complete or cycle command names.

| Command | Action |
|---------|--------|
| `:{N}` | Jump to new-side line N in current file |
| `:o{N}` | Jump to old-side line N in current file (matches deletions) |
| `:w` (`:write`) | Save session |
| `:e` (`:reload`) | Reload diff files |
| `:edit` | Open focused file in `$EDITOR` |
| `:clip` (`:export`) | Copy review to clipboard |
| `:file <path>` | Write the same review to `<path>`, replacing any file there (`~/` means home) |
| `:copy-url` | Copy the open PR URL to clipboard (PR mode) |
| `:summary` | Show all pending local-draft comments; `j`/`k` select, `dd` deletes, and `Enter` jumps |
| `:ask` / `:ask history` | Ask privately at the cursor / view the current review's private history |
| `:prepare` | Start background preparation for the open diff (Unix only) |
| `:prepare cancel` | Cancel preparation for the open review |
| `:guide` | Enter the guide or return to the current stop |
| `:guide off` | Pause the guide without clearing verdicts or review marks |
| `:guide detail` | Read the current stop's full claim, location, warnings, and explanation; `j` / `k` scroll, `Esc` returns |
| `:changes` | Walk changes from the original baseline of a local review, with or without a prepared guide |
| `:map` | Switch the sidebar between map and file tree while inside the guide |
| `:spot` | Start or resume a spot-check of supporting changes |
| `:coverage` | View the coverage record |
| `:diff` | Toggle diff view (unified / side-by-side) |
| `:theme` | Open the live-preview theme picker; `/` filters, `Enter` applies for this session, and `Esc` reverts |
| `:theme <name>` | Apply `<name>` for this session without opening the picker |
| `:focus` (`:f`) | Toggle single-file view |
| `:stage` | Stage reviewed files (unstaged diffs only) |
| `:vim` (`:set vim!`) / `:novim` (`:set novim`) / `:set vim` | Toggle / disable / enable vim modal editing in the comment box (overrides `comment_vim`) |
| `:commits` (`:targets`) | Select commits to review |
| `:prs` | Open the review target selector on Pull Requests |
| `:sessions` (`:reviews`) | Resume a saved review for this checkout |
| `:submit` | Open the PR submit picker. With a guide, show coverage and [guided choices](#guided-submit) |
| `:submit comment` | Submit a Comment review |
| `:submit approve` | Submit an Approve review |
| `:submit request-changes` | Submit a Request-changes review |
| `:submit draft` | Submit a Draft (pending) review |
| `:comments unresolved` | Show unresolved remote comments (PR mode, default) |
| `:comments all` | Show all remote comments including resolved and outdated |
| `:comments hide` | Hide remote comments in PR mode |
| `:set wrap` | Enable line wrap in diff view |
| `:set wrap!` (`:wrap`) | Toggle line wrap in diff view |
| `:set relativenumber` / `:set norelativenumber` | Enable / disable relative rendered-row numbers |
| `:set relativenumber!` | Toggle relative rendered-row numbers |
| `:set commits` | Show inline commit selector |
| `:set nocommits` | Hide inline commit selector |
| `:set commits!` | Toggle inline commit selector |
| `:set reviewed` | Show files already marked reviewed |
| `:set noreviewed` | Hide files already marked reviewed |
| `:set reviewed!` / `:reviewed` | Toggle files already marked reviewed |
| `:clear` | Clear comments and reset reviewed marks |
| `:clearc` | Clear comments without clearing reviewed marks |
| `:help` (`:h`) | Open the help screen |
| `:messages` | Open full details for the current error |
| `:version` | Show tuicr version |
| `:update` | Check for updates |
| `:q` (`:quit`) | Quit (warns on unsaved comments; discards review-only state unless the saved session holds comments, e.g. added by `tuicr review add`) |
| `:q!` (`:quit!`) | Force quit |
| `:x` / `:wq` | Save and quit (prompts to copy if comments exist) |
| `ZZ` | Save and quit (stays open and reports the error if the save fails, like `:wq`) |
| `ZQ` | Quit without saving |
| `?` | Toggle help |

Pressing bare `q` no longer quits by default; it prints a reminder to use `:q` instead. Set
`q_quits = true` to restore `q` as a quit key in review modes.

The summary replaces the diff while leaving the file sidebar visible when it is open. The first
pending comment is selected when the summary opens. Use `j`/`k` to select the next
or previous comment; the view scrolls automatically to keep the selection visible. `Enter` returns
to the continuous diff and jumps to the selected comment, leaving single-file view if necessary,
while `Esc` returns without jumping. Reviewed files and hunks are revealed for the jump without
losing their reviewed state.
Press `dd` to delete the selected local draft, including comments whose original file or line
is absent from the current diff. Other comments are kept. Return with `Esc` and use `:w` to
save the deletion, following the existing diff-view deletion workflow.

The theme picker lists every bundled theme plus any local `*.toml` themes in the theme directory.
`j`/`k` move the selection and immediately repaint the UI with that theme (live preview) without
saving anything. `/` opens a filter prompt — type to narrow the list by substring, `Enter` applies
the filter and returns to navigation, `Esc` discards the filter edit without changing the list.
`Enter` on the picker itself keeps the currently previewed theme for the rest of the session. Like
other `:` toggles, the choice is not written to `config.toml`; set `theme = "<name>"` there to keep
it. `Esc` on the picker reverts the preview to whatever theme was active before it opened.

### Guided submit

With a review guide installed, `:submit` shows verified stops and stories,
unverified risks, and supporting-change coverage. Use `j` / `k` or `↓` / `↑`
to choose, `Enter` to continue, and `Esc` to cancel.

| Choice | What happens |
|---|---|
| **Approve what I verified** | Preview a normal forge approval with a note describing the review's scope. The forge vote itself is not scoped |
| **Approve** | Approve without a coverage note |
| **Request changes** / **Comment** | Preview the review with a coverage note |
| **Ask the author to split** | Open an unsent review comment suggesting a split along the stories |
| **Save and come back** | Save the review and return to the diff without submitting |

In the preview, press `e` to edit a coverage note. Clear its text and save
the edit to omit it. `y` confirms and `n` or `Esc` cancels. If an approval has
unverified risk items, the confirmation requires `y` (or `Y`), not `Enter`.
Once confirmed, the note and saved comments are submitted through the usual
forge review flow. The note is based on recorded coverage, not Ask history.

### Forge support

Not every forge supports every event:

| Event | GitHub | GitLab | Gitea | Bitbucket | Azure DevOps | Gerrit |
|---|---|---|---|---|---|---|
| `comment` | yes | yes | yes | yes | yes | yes |
| `approve` | yes | yes | yes | yes | yes (vote +10) | yes (vote +2) |
| `request-changes` | yes | yes | yes | yes | yes (vote -10) | yes (vote -1) |
| `draft` | yes | yes | yes | yes | yes (plain comment) | yes |

Bitbucket `draft` posts pending comments that the author publishes from Bitbucket's own UI.
Azure DevOps has no pending-review primitive, so `draft` posts as a plain comment with no vote.
Gitea requires a review summary for `request-changes` and `draft`, and for `comment` when there
are no inline comments; inline comments alone do not satisfy it. GitLab `draft` creates draft
notes that the author publishes from GitLab's own "Submit review" UI. Gerrit `draft` stores draft
comments that the author publishes from Gerrit's Reply UI.

GitLab, Bitbucket and Azure DevOps post a submit as several requests. If one fails midway, the
comments that already posted are locked and not posted again; the rest stay local drafts. Retry
with the same command. `:submit request-changes` may run with nothing left but the vote (a bare
retry); GitHub rejects an empty request-changes review, so it needs a comment there.

## Commit selection / review target selector

| Key | Action |
|-----|--------|
| `Tab` / `Shift-Tab` | Switch between Local, Pull Requests, and Sessions tabs |
| `j` / `k` | Move selection |
| `Space` | Toggle local commit selection |
| `Enter` | Confirm local commit range, open PR, load more PRs, or resume a saved review |
| `/` | Filter currently loaded PR rows locally |
| `r` | In Pull Requests tab, cycle all open PRs / PRs requesting your review (the `pr_search` query on GitHub when set) / (GitHub) PRs requesting you by name |
| `p` / `P` | In Pull Requests, prepare the selected PR in the background / cancel its preparation |
| `a` | In Sessions tab, show or hide saved reviews holding no progress |
| `dd` | In Sessions tab, delete the saved review under the cursor (asks to confirm) |
| `Esc` | Return to the diff |
| `:q` | Quit |

In the Pull Requests tab, `r` steps through the list scopes the forge supports and
refetches the first page: all open PRs, then PRs requesting your review, then back. On
GitHub a third scope, `requested (me)`, sits between "requested" and "all": it lists only
PRs that request your review by name, leaving out requests made to a team you belong to.

The Sessions tab lists saved reviews for the current checkout, as well as
preparing and ready guides. Saved-session rows show the target, comment count,
reviewed files, and age. Sessions with no comments and no reviewed files are
usually omitted; press `a` to show them. `Enter` on a preparation row opens
its target. Selecting a PR re-fetches it from the forge, just like opening from
Pull Requests. Browsing the list does not start preparation.

Sessions labels a prepared PR **out of date** if the loaded PR list reports
a changed head or the open review has a different diff. If tuicr doesn't
have a current head or diff to compare, it shows `ready (prepared for SHA)`
with the short SHA, or `ready (head unknown)` if the stored preparation
doesn't identify a head. The row does not refetch the forge to check.

Opening a different local target (a commit selection on the Local tab, or a saved review
on the Sessions tab) first saves the review on screen, so its unsaved marks and comments
are kept. If that save fails, the switch is aborted and the current review stays open
with its unsaved changes. The commit pane of the replaced review is closed, and focus
returns to the diff if it was in that pane.

Press `dd` to delete the saved review under the cursor. tuicr asks to confirm first, and
refuses to delete a review that is open in another tuicr, since that process would write
it straight back. Combine `a` with `dd` to clear out empty sessions left behind by a
crash, or by `tuicr review delete`/`clear` emptying a review another tuicr had open.

## Inline commit selector

Shown at the top of the diff when reviewing multiple commits. Focus it with `<leader>k` or `Tab`.
When opening a GitHub PR or GitLab MR you have reviewed before, tuicr may preselect only commits
newer than your latest submitted review; commits already covered by that review are marked with
`✓`. Use `Space` to expand or adjust the range, or `Enter` to jump to a single commit. Bitbucket does not record which
commit an approval covered, so no commits are preselected there.

When exactly one commit is selected, its message is shown above its diff as a
`Commit Message (<sha>)` entry, in local and pull request reviews alike. A review of a single
commit, including a single-commit pull request, shows the message without the selector. Comments
on a commit message in a pull request are submitted in the review summary rather than inline.
When a pull request opens with a narrowed range, the message appears once the selected range has
loaded. When the PR's diff base or its known commit list changes, a reload resets the selection to
all commits.

Reviewed marks belong to the whole review, not to the selected commits: pressing `r` on a file
while narrowed to one commit marks the entire cumulative file, and the mark stays when you
widen or change the selection.

| Key | Action |
|-----|--------|
| `j` / `k` | Navigate commits |
| `Space` | Toggle commit selection (updates diff) |
| `Enter` | Select only the commit under the cursor, unselecting the others (updates diff) |
| `(` / `)` | Cycle through individual commits |
| `Esc` | Return focus to diff |

## Confirm dialogs

| Key | Action |
|-----|--------|
| `y` / `Enter` | Yes |
| `n` / `Esc` | No |

## Mouse

Mouse support is on by default. Disable with `mouse = false` in config.

| Action | Effect |
|--------|--------|
| Wheel up / down | Scroll the panel under the cursor (file list, comment navigator, diff, commit list, or help popup) without moving the cursor line |
| Click on a file | Jump to that file (lazygit-style) |
| Click on a directory | Expand or collapse it |
| Click on a diff line | Position the cursor on that line |
| Click on a commit | Toggle selection (or expand the row to load more) |
| Drag in diff | Highlight a range; press `y` to copy the selected source lines |

For full native terminal selection across the UI, hold your terminal's bypass modifier while dragging (usually **Shift** or **Option/Alt**, depending on the terminal).
