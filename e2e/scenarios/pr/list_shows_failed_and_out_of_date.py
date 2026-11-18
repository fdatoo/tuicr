#!/usr/bin/env python3
# Review-guide rebuild: surface a failed preparation and detect a prepared PR's moved head.
from e2e.prepare import fake_claude, github_pr_response

from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    repo = repos.pr_repo(s.dir / "repo")
    fake_claude(s, response=None)
    tui = s.start(cwd=repo, cols=100, env={"TUICR_PREPARE_ON_OPEN": "1"})
    tui.wait_for("Pull Requests").keys("Tab").wait_for(r"#1 .*not prepared")
    tui.keys("p").wait_for(r"#1 .*failed · claude: provider exited", 20)
    tui.keys("Enter").wait_for("PR Description").wait_for(
        r"Review preparation failed · :prepare to retry · claude: provider exited"
    )
    tui.quit()

    fake_claude(s, response=github_pr_response(), digest_from_prompt=True)
    tui = s.start(cwd=repo, keep=True)
    tui.wait_for("Pull Requests").keys("Tab").wait_for(r"#1 .*failed ·")
    tui.keys("p").wait_for(r"#1 .*ready · \d+ stops · about 12 min", 20)
    tui.quit()

    tui = s.start(cwd=repo, keep=True, cols=100, env={"E2E_VARIANT": "moved_head"})
    tui.wait_for("Pull Requests").keys("BTab").wait_for(r"guide +#1 .*ready \(prepared for 22222222\)")
    tui.keys("BTab").wait_for(r"#1 .*out of date")
    tui.keys("Tab").wait_for(r"guide +#1 .*out of date")
    tui.quit()
