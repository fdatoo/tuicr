#!/usr/bin/env python3
# Fork review guide: when the current checkout is a clone of the PR's repository, Ask runs in a private checkout of the
# selected PR commit built from its objects: no `gh` clone, and the working tree's own edits are never what Ask reads.
from e2e.companion import fake_claude

from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    clone = repos.pr_repo(s.dir / "clone")
    context = repos.write(clone / "tests/pinned-context.txt", "Unchanged test source from the pinned forge revision.\n")
    repos.git(clone, "add", ".")
    repos.git(clone, "commit", "-qm", "pinned")
    pinned = repos.git(clone, "rev-parse", "HEAD")
    repos.write(context, "WORKTREE_ONLY_MARKER\n")

    fake_claude(
        s,
        f"""
import subprocess
activity('Read')
assert Path.cwd().resolve() != Path({str(clone)!r}).resolve()
assert Path('tests/pinned-context.txt').read_text().startswith('Unchanged test source')
git = lambda *args: subprocess.run(['git', *args], capture_output=True, text=True, check=True).stdout.strip()
assert git('rev-parse', 'HEAD') == {pinned!r}
assert git('log', '--format=%s').splitlines() == ['pinned', 'base']
answer('Confirmed the private checkout of the pinned commit.')
""",
    )
    tui = s.start(
        "pr",
        "https://github.com/octo/demo/pull/1",
        cwd=clone,
        env={"E2E_VARIANT": "threads_and_commits", "E2E_SHA_MAP": f"{'b' * 40}={pinned}"},
    )
    tui.wait_for("PR Description").keys(";k", "j", "Enter")
    tui.wait_for(rf"Commit Message \({pinned[:7]}\)").wait_for("line from commit B")
    tui.keys(";j").cursor_on(r"▌.*line from commit B")
    tui.cmd("ask").wait_for("Private companion")
    tui.type("What unchanged test covers this?").keys("Enter").wait_for("Confirmed the private checkout")
    tui.forge_reject(r'"repo", "clone"')
    tui.keys("Escape").quit()
