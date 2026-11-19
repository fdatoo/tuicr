#!/usr/bin/env python3
# Fork review guide: with no matching local clone, Ask runs in a checkout of the selected PR commit cloned through
# `gh repo clone` (full history, no copied size-capped snapshot), never in the unrelated checkout or at a later commit.
from e2e.companion import fake_claude

from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    checkout = repos.empty_repo(s.dir / "unrelated")
    repos.write(checkout / "tests/pinned-context.txt", "WRONG_CHECKOUT_SECRET checkout test token\n")
    repos.git(checkout, "add", ".")
    repos.git(checkout, "commit", "-qm", "unrelated work")
    repos.git(checkout, "remote", "add", "origin", "https://github.com/unrelated/work.git")

    # What the fake `gh repo clone` copies: the PR's commit B, one earlier commit, and a later one that must not appear.
    upstream = repos.empty_repo(s.dir / "upstream")
    context = repos.write(upstream / "tests/pinned-context.txt", "Earlier test source.\n")
    repos.git(upstream, "add", ".")
    repos.git(upstream, "commit", "-qm", "earlier")
    repos.write(context, "Unchanged test source from the pinned forge revision.\n")
    repos.git(upstream, "commit", "-qam", "pinned")
    pinned = repos.git(upstream, "rev-parse", "HEAD")
    repos.write(context, "LATER_COMMIT_MARKER\n")
    repos.git(upstream, "commit", "-qam", "later")

    fake_claude(
        s,
        f"""
import subprocess
activity('Read')
assert 'WRONG_CHECKOUT_SECRET' not in prompt
assert Path('tests/pinned-context.txt').read_text().startswith('Unchanged test source')
git = lambda *args: subprocess.run(['git', *args], capture_output=True, text=True, check=True).stdout.strip()
assert git('rev-parse', 'HEAD') == {pinned!r}
assert git('log', '--format=%s').splitlines() == ['pinned', 'earlier']
answer('Confirmed pinned test source and history.')
""",
    )
    tui = s.start(
        "pr",
        "https://github.com/octo/demo/pull/1",
        cwd=checkout,
        env={
            "E2E_VARIANT": "threads_and_commits",
            "E2E_SHA_MAP": f"{'b' * 40}={pinned}",
            "E2E_GITHUB_CLONE_SOURCE": str(upstream),
        },
    )
    tui.wait_for("PR Description").keys(";k", "j", "Enter")
    tui.wait_for(rf"Commit Message \({pinned[:7]}\)").wait_for("line from commit B")
    tui.keys(";j").cursor_on(r"▌.*line from commit B")
    tui.cmd("ask").wait_for("Private companion")
    tui.type("What unchanged test covers this?").keys("Enter").wait_for("Confirmed pinned test source")
    tui.forge_expect(r'"repo", "clone", "octo/demo"')
    tui.forge_reject(r"tarball/|git/(trees|blobs)|contents/tests/")
    tui.keys("Escape").quit()
