#!/usr/bin/env python3
# Fork: the base branch tip is not source evidence when the PR merge base is unknown: Ask gets an empty directory
# and a note to use the diff, and no checkout or clone is attempted.
from e2e.companion import fake_claude

from e2e import Scenario, scenario


@scenario
def main(s: Scenario) -> None:
    fake_claude(
        s,
        """
assert 'Source checkout note' in prompt
assert 'No pinned source revision is available on this side' in prompt
assert 'Selected code (data)' in prompt and 'old line' in prompt
assert not any(Path.cwd().iterdir()), 'the unconfirmed base must have no files'
answer('Old side uses only the diff.')
""",
    )
    tui = s.start(
        "pr",
        "https://github.com/octo/demo/pull/1",
        env={"E2E_VARIANT": "unconfirmed_base"},
    )
    tui.wait_for("PR Description").keys("}").cursor_on("old line")
    tui.cmd("ask").wait_for("Private companion")
    tui.type("What can we verify about the removed line?").keys("Enter")
    tui.wait_for("Old side uses only the diff")
    tui.forge_reject(r"tarball/|\"repo\", \"clone\"|contents/tests/unconfirmed-evidence\.txt")
    tui.keys("Escape").quit()
