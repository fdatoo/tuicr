# Command Line Reference

Every flag and subcommand `tuicr` accepts, plus the environment variables it
reads and the output it produces for scripts and agents. `tuicr --help` is the
same list in condensed form.

Related references: [CONFIG.md](CONFIG.md) for the config file and themes,
[KEYBINDINGS.md](KEYBINDINGS.md) for keys and `:` commands inside the TUI, and
[REVIEW_CLI.md](REVIEW_CLI.md) for the `tuicr review` subcommands in full.

## Commands

| Form | What it does |
|---|---|
| `tuicr` | Open the TUI on the review target selector |
| `tuicr tui` | Same TUI, explicit subcommand |
| `tuicr pr <target>` (`tuicr mr`) | Review a pull request / merge request |
| `tuicr tui pr <target>` (`tuicr tui mr`) | Same, under the explicit TUI subcommand |
| `tuicr prepare <target>` | Write and audit a review guide, or audit a supplied guide |
| `tuicr guide outline <target>` | Print the diff digest and a source-lined outline for guide authors |
| `tuicr guide check <target> PATH` | Validate a version 2 guide against the exact diff |
| `tuicr review list` | List persisted review sessions |
| `tuicr review add` | Add a local draft comment to a session |
| `tuicr review comments` (`tuicr review get`) | Print a session's comments |
| `tuicr review delete` | Delete one stored comment by ID |
| `tuicr review clearc` | Clear comments but keep review marks |
| `tuicr review clear` | Clear comments and review marks |
| `tuicr update [VERSION]` | Update the installed binary |

`mr` is an alias of `pr`, and `get` an alias of `comments`; they behave
identically. `<target>` accepts a bare `<number>`, `<owner/repo#N>`, or a PR
URL.

`tuicr update` takes an optional SemVer version to install a specific release,
including an older known-good one. It only accepts a version number — there is
no `latest` keyword; omit the argument to get the newest release.

There is no `tuicr help` subcommand. Use `-h` / `--help`, which also works per
command (`tuicr review add --help`).

## Flags

These apply to the TUI and can be given on the bare command, on `tui`, or on
`pr` / `mr`. When the same option appears at more than one level, string values
take the last one given and boolean flags stay on once set, so
`tuicr --stdout tui pr 125 --theme nord-dark` is valid. Conflicts between
options (`--file` / `--all-files` against `-r`, `-w`, `-p`) are checked on the
merged set, so they also fail when split across levels. `pr` rejects the
local-review flags `-r`, `-w`, `-p`, `--file` and `-A`.

| Flag | Short | Value | Description |
|---|---|---|---|
| `--revisions` | `-r` | `REVSET` | Commit range / revset to review. Syntax depends on the VCS backend (`main..HEAD` for git, a jj revset for jj) |
| `--theme` | | `THEME` | Color theme. Bundled names resolve first, then `<config>/themes/<name>.toml`. An invalid value exits with code `2` |
| `--appearance` | | `light`, `dark`, `system` | Appearance mode, used when no explicit theme is set |
| `--path` | `-p` | `PATH` | Filter the diff to a file or directory |
| `--working-tree` | `-w` | | Include uncommitted changes and skip the target selector |
| `--file` | | `PATH` | Open a file or directory for annotation with no VCS required |
| `--line` | | `N` | Start with the cursor on line `N` (new side). Requires `--file` or `--path`; not with `pr` |
| `--guide` | | `PATH` | Load a supplied review guide without auditing it or moving the cursor. Press `.` or run `:guide` to start. Conflicts with `--line`, `--file` and `--all-files` |
| `--all-files` | `-A` | | Review every tracked file in the current repo |
| `--stdout` | | | Print the export to stdout instead of copying to the clipboard |
| `--no-update-check` | | | Skip the startup update check (same as `no_update_check` in the config) |
| `--repo-url` | | `URL` | Override the forge repo for PR operations |
| `--remote` | | `NAME` | Use a named VCS remote's fetch/pull URL for PR operations; conflicts with `--repo-url` |
| `--version` | `-V` | | Print the version and exit. Top-level only (`tuicr --version`), not under `tui` or `pr` |
| `--help` | `-h` | | Print help and exit |

`--repo-url` accepts GitHub, GitLab, Gitea, Bitbucket, Azure DevOps, and Gerrit repos in HTTPS,
SCP-style SSH, or `ssh://` form — for example
`https://github.com/owner/repo`, `git@gitlab.com:owner/repo`, or
`https://dev.azure.com/org/project/_git/repo`. It is what to reach for when the
checkout's `origin` remote does not point at the forge repo you want to review
against.

If the repository is already configured as a remote, use its name instead:

```bash
tuicr pr 382 --remote staging-upstream
```

`--remote` discovers a Git repository from the current directory, including
subdirectories and colocated Jujutsu workspaces. Remote lookup does not run `jj`.
Non-colocated Jujutsu and Mercurial workspaces can use `--repo-url` instead.

- Git uses `git remote get-url <name>` and honors `url.<base>.insteadOf` rewrites.

The fetch/pull URL is used, not a push URL. A missing remote or unrecognized
forge URL is an error, not a fallback to another remote. The URL must identify
a repository on one of the supported forges.

Both `--remote` and `--repo-url` select the repository directly, without a fork-parent
lookup. They cannot be combined. When `--remote` is supplied, its lookup must
succeed before a full PR URL or `owner/repo#N` target takes precedence. Omit
`--remote` when using an explicit target outside a Git checkout.

### Scope selection

`--file` and `--all-files` are each exclusive of the other and of
`--revisions`, `--working-tree`, and `--path`. For a diff review, you can
combine the last three to select a range, include uncommitted changes,
and filter by path.

`--path` on its own implies `--working-tree`. Pair it with `-r` to filter a
commit range instead:

```bash
tuicr -p src/cli.rs                 # uncommitted changes to one file
tuicr -r main..HEAD -p src/cli.rs   # that file's changes across a range
```

### Starting at a line

`--line N` puts the cursor on source line `N` of the first file in the review,
on the new side, and centers it. It needs `--file` or `--path`, which name the
file the line belongs to, and cannot be combined with `tuicr pr`:

```bash
tuicr --file README.md --line 42      # annotate a file, starting at line 42
tuicr -p src/cli.rs --line 120        # uncommitted changes, starting at line 120
```

The jump works like `:N`. A line inside collapsed context expands to reach it,
and a line outside the diff lands on the nearest line with a status message.
Editors and viewers can use it to hand off the line the user is looking at.

`--revisions` accepts leading hyphens, so revsets like `-r -3` reach the VCS
rather than being read as another flag.

TUI flags belong to the TUI; headless `prepare` and `guide` take their
target flags after the subcommand. `review` and `update` take no TUI flags:

```bash
tuicr --stdout review list
# error: TUI options cannot be used with `tuicr review`; run `tuicr review --help` for command options
```

## Preparing a review

These commands do not open the TUI. Choose the same explicit target as the
review: `-w` (`--working-tree`), `-r REVSET` (`--revisions`), `-p PATH`
(`--path`), or `pr TARGET` (`mr TARGET`). Local options can be combined:
`-p` alone implies working-tree changes, `-r … -p …` filters a commit range,
and `-r … -w` includes working-tree changes alongside the range. PR targets
also accept `--remote NAME` or `--repo-url URL`. `-w` includes untracked files;
`git diff HEAD` is not the same diff.

| `prepare` option | Effect |
|---|---|
| `--guide PATH` | Audit a supplied guide instead of writing one. Requests a fresh audit if no job is running |
| `--wait` | Wait for a guide for the requested diff, starting preparation or joining an existing job for the review |
| `--print-prompt` | Print the effective Write, Audit and Ask prompts, including reviewer rules. No target or AI client is needed; no job is started |

```bash
tuicr prepare -w
tuicr prepare pr 125 --guide /path/to/review-guide.json --wait
tuicr prepare --print-prompt
```

Preparation requires Unix, even with `--wait`. Without `--wait`, the command
returns when it starts a worker or finds an existing job and prints the log
path. A detached job outlives the TUI. When no job is running, a saved guide
is reused only if the diff and reviewer rules still match and `--guide`
didn't request a fresh audit. Joining a running job does not apply a newly
supplied file to it. There is no overall time limit; five minutes without
progress is treated as failure. In the TUI, use `:prepare` or the PR list's
`p` to retry.

Files live under `<data dir>/guides/<review key>/`: `guide.json` (the prepared
guide), `state.json` (job status, target and error if any), `progress.json`
(verdicts and spot-checks), and `prepare.log` (worker log). The directory key
identifies the PR including its forge host, or the local checkout and scope;
the guide's `diff` identifies the specific diff. The platform data directory
is `$XDG_DATA_HOME/tuicr` on Linux (default `~/.local/share/tuicr`),
`~/Library/Application Support/tuicr` on macOS and `%APPDATA%\tuicr\data` on
Windows. Preparation logs and guides are private local files; do not share them
indiscriminately.
Local reviews also keep a private, size-capped text baseline in `baseline/`.

### Authoring a review guide

```bash
tuicr guide outline -w
tuicr guide check -w /path/to/review-guide.json
tuicr guide outline pr 125
tuicr guide check pr 125 /path/to/review-guide.json
```

`outline` prints a digest and an inventory of every file and hunk, with source
line ranges and short excerpts where available. `check` validates the author's
JSON structure, matching digest, and resolvable stop anchors. Neither calls
an AI provider. Passing `check` does not verify the claims, audit the guide,
or require complete supporting coverage.

Group stops into stories ordered by importance. Start at a user action, command
or production entry point and follow it to the code that changes the behavior.
If the entry point is unchanged, start at the changed implementation and explain
how it is reached. Use a test as a stop only when it explains behavior better,
or when tests are themselves the change. At each stop, give the reviewer a
claim to verify and explain one level above the highlighted block before
returning to it. `estimate_minutes` is optional for authors; if supplied,
give an honest estimate for checking the stops, not the whole diff.

Save the JSON outside the reviewed checkout. The following is a complete
single-stop example; replace the digest with the one from your outline and
use the paths and line numbers from that diff.

```json
{
  "version": 2,
  "diff": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "summary": "Retry a payment without charging twice.",
  "estimate_minutes": 3,
  "stories": [{
    "title": "Retry a timed-out payment",
    "summary": "Follow the Retry action into the payment store.",
    "stops": [{
      "id": "retry-keeps-payment-id",
      "claim": "Retry uses the payment ID from the original request.",
      "explanation": "The checkout handler receives Retry after a timeout. This block sends the original ID to the payment store so it can find an existing receipt.",
      "path": "src/checkout.rs",
      "side": "new",
      "start_line": 42,
      "end_line": 43,
      "symbol": "Checkout::retry_payment",
      "test": {"path": "tests/checkout.rs", "name": "retry_uses_original_id"},
      "followups": [{
        "question": "Where is the receipt looked up?",
        "answer": "The payment store looks it up by ID before charging."
      }]
    }]
  }],
  "supporting": []
}
```

| Object | Fields |
|---|---|
| Guide | Required: `version` (`2`), `diff` (digest from `outline`), `stories` (at least one stop overall). Optional: `summary`, `supporting` (array), `estimate_minutes` (integer), `provenance` (omit when authoring) |
| Story | `title`, ordered `stops`; optional `summary` |
| Stop | Required: unique, stable `id`, `claim`, diff `path`, inclusive `start_line` and `end_line`. `side` is `old` or `new` (default); use source line numbers in one hunk, not patch offsets, and choose a block with a changed line on that side. Optional: `explanation`, `symbol`, `test`, `untested` (boolean), `risk`, `followups`, `audit` (omit when authoring) |
| Test reference | `path`; optional `name` and `line`. Cite the test that would fail if the claim were false; use `untested: true` if there is none |
| Risk | `kind`: `unexplained`, `test_changed`, `code_removed`, `config`, `error_handling`, `rule` or `other`; `reason`: why the reviewer should check it |
| Follow-up | `question`, `answer`; shown in Ask without contacting a provider |
| Supporting group | `label`, `basis` (`claimed` for authors; tuicr computes `mechanical`), `hunks`, optional `note`. Each hunk reference has `path`, `old_start`, `new_start` from the outline. tuicr accounts for remaining hunks during preparation or loading |
| Audit metadata | `audit` is an object with `result` (`unchecked`, `holds` or `doubtful`); `doubtful` also has `note` |
| Guide provenance | tuicr records `author` (`reviewer` or `supplied`), `audited`, `provider`, `model`, `rules_digest`, `prepared_at` and `limits` during preparation |

Preparation audits the claims, explanations, and follow-ups, records test
evidence when available, and uses up to two supplemental audit passes for
stops that are still unchecked. Generated guides keep the first 12 stops
without a risk or doubtful audit, plus all risky and doubtful stops.
Overflow hunks not already covered by a retained stop go into supporting
groups named `More on <story>`. Supplied guides are exempt from the budget;
preparation can still drop stops whose anchors it cannot repair.

Prepared guides with more than 16 stops warn that the review may take more
than one sitting. A missing or zero estimate is filled with one minute per
stop, plus an extra minute for each stop needing an explicit verdict. For a
guide over 16 stops, this calculation is also the minimum estimate. Preparation
records audit provenance and assigns all changed hunks to stops or
supporting groups. None of this is required by `guide check`.

The prepared summary includes `Stops cover N of M changed hunks`. This counts
distinct changed text hunks reached by stops, not hunks verified by a reviewer.

For an audited handoff, run `tuicr prepare -w --guide PATH --wait`, then open
`tuicr -w` without `--guide`, so tuicr picks up the prepared copy. If the
command joined a different job, rerun it after that job ends to audit your
file. To use the file without an audit, pass `--guide PATH` directly to the TUI.

## Environment variables

No flag reads an environment variable as a fallback — anything below either
configures something with no flag equivalent, or is read by a tool tuicr shells
out to.

| Variable | Effect |
|---|---|
| `EDITOR` | Editor used by `e` and `:edit`. The config `editor` setting wins; `vi` is the last resort |
| `XDG_CONFIG_HOME`, `HOME`, `APPDATA` | Locate the config directory. See [CONFIG.md](CONFIG.md) |
| `TUICR_PROFILE` | Write a startup/render profile log. See below |
| `TUICR_PROFILE_FILE` | Path for that log, overriding the default location |
| `TUICR_COMPANION` | Choose the provider for Ask and preparation: `claude` (default) or `codex`. Overrides `companion` in the config and uses that CLI's authentication |
| `TUICR_COMPANION_MODEL` | Override `companion_model` for Ask. Claude defaults to `haiku`; Codex uses its default model with low reasoning effort |
| `TUICR_REVIEWER_MODEL` | Override `reviewer_model` for guide preparation. Claude defaults to `sonnet`; Codex uses its default model with medium reasoning effort |
| `TUICR_PREPARE_ON_OPEN` | Override `prepare_on_open` for PRs: `0` or `false` disables, `1` or `true` enables |
| `AZURE_DEVOPS_SERVER_URL` | Hostname of a self-hosted Azure DevOps Server / TFS so its remotes are recognized as Azure DevOps. Requires a PAT. See [AZURE.md](AZURE.md) |
| `AZURE_DEVOPS_EXT_PAT`, `AZURE_DEVOPS_PAT` | Azure DevOps PAT. Checked in that order; without one tuicr falls back to `az rest`. See [AZURE.md](AZURE.md) |
| `GERRIT_URL` | Gerrit server root; also identifies Gerrit remotes on otherwise neutral hostnames. See [GERRIT.md](GERRIT.md) |
| `GERRIT_USERNAME`, `GERRIT_PASSWORD` | Gerrit REST API credentials. `GERRIT_PASSWORD` must be an HTTP password, not the account password |
| `GITLAB_SUBFOLDER` | Subfolder a self-hosted GitLab is installed under (`gitlab` for `https://example.com/gitlab/`). Read by `glab` and by tuicr, and overrides `glab`'s per-host `subfolder` setting. See [GITLAB.md](GITLAB.md) |
| `TUICR_GLAB_DEBUG` | Log `glab` invocations for debugging. See [GITLAB.md](GITLAB.md) |
| `TMUX`, `ZELLIJ`, `SSH_TTY` | Detected to choose a clipboard path — OSC 52 over SSH, `tmux load-buffer` inside tmux |
| `REX_SESSION` | Prefer OSC 52 clipboard writes to reach the Rex client's clipboard |
| `TERM_PROGRAM` | When set to `rex`, prefer OSC 52 clipboard writes |
| `XDG_SESSION_TYPE` | Picks `wl-copy` on Wayland or `xclip` on X11 |

Set `TUICR_PROFILE` to `1`, `true`, `on`, or `yes` to log to
`tuicr-profile.<timestamp>.log` in the system temp directory. Any other
non-empty value is treated as the log path itself; `0`, `false`, `off`, `no`,
and an empty value disable profiling. `TUICR_PROFILE_FILE` overrides the path
either way, but only when `TUICR_PROFILE` is enabled.

To compare wheel latency between Unix release binaries, run
`python3 scripts/bench_scroll.py /path/to/stock/tuicr /path/to/fork/tuicr`.
It drives a real PTY with SGR wheel events on isolated synthetic diffs and measures time
through the synchronized repaint, including an ordered redraw after a burst and reversal.
It does not measure physical display latency or OS-generated trackpad inertia.

## Output for scripts and agents

Two markers go to **stderr**, so they survive on the user's scrollback and stay
out of piped output:

```
tuicr-session: agavra/tuicr@main/worktree
tuicr-summary: reviewed 3/3 files, 2 comments added
```

`tuicr-session:` is printed once at startup, before the alternate screen is
entered, and names the session slug. Pass it straight to
`tuicr review comments --session <slug>` to read comments while the TUI is
still open, or after it exits. A PR session on a forge's default host is
`gh:owner/repo/pr/N`; on any other host (and always for Gerrit) the slug ends
in `@<host>`, e.g. `gh:owner/repo/pr/N@ghe.example.com`. `tuicr-summary:` is printed on exit, always —
including with zero comments, so a watcher can tell "reviewed everything, found
nothing" apart from "quit without looking".

With `--stdout`, press `y` or run `:clip` (`:export`) to write the Markdown
to stdout and exit. `ZZ`, `:wq`, and `:x` also export after saving if the
session has comments. Plain `:q` never exports. The TUI itself is rendered
to `/dev/tty` and the session markers go to stderr, leaving stdout for the
export.

`tuicr review` subcommands print pretty JSON to stdout and nothing else.
There is no `--json` flag because JSON is the only format. See
[REVIEW_CLI.md](REVIEW_CLI.md) for the schemas.

`tuicr update` prints a single line describing what it did — updated, already
up to date, or completed through an external package manager.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success, including `--help` and `--version`. Detached `prepare` can report launch rather than completion; `prepare --wait` succeeds only when a matching guide is ready |
| `1` | Startup failure (not a repository, forge auth, missing PR, bad `--file` path), failed `review` or `update`, invalid guide or unavailable diff for `guide check`/`outline`, or a preparation failure. With `--wait`, a failed, cancelled, stalled, or completed mismatched job returns a reason and a log path |
| `2` | Usage error (unknown flag, conflicting flags, missing target, invalid `--appearance` value) or a theme that could not be resolved |

## Not provided

tuicr ships no man page and no shell completions. `tuicr --help` and this page
are the reference.
