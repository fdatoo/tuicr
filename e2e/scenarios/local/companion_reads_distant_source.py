#!/usr/bin/env python3
# Fork review guide: Ask reads distant unchanged source from the exact reviewed revision without paging.
from e2e.companion import fake_claude

from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    repo = repos.empty_repo(s.dir / "repo")
    lines = [f"Fixture row {n:04d}: {'configuration context ' * 3}" for n in range(1, 1001)]
    lines[159] = "use the old route"
    lines[319] = "CONTEXT_BEYOND_12_KIB"
    lines[799] = "DEFAULT_ROUTE = cobalt route"
    source = repos.write(repo / "routes.txt", "\n".join(lines) + "\n")
    repos.git(repo, "add", ".")
    repos.git(repo, "commit", "-qm", "base")
    lines[159] = "select a fallback route"
    repos.write(source, "\n".join(lines) + "\n")
    repos.git(repo, "commit", "-qam", "select fallback")
    lines[799] = "DEFAULT_ROUTE = CHECKOUT_ONLY_ROUTE"
    repos.write(source, "\n".join(lines) + "\n")

    fake_claude(
        s,
        """
assert 'CHECKOUT_ONLY_ROUTE' not in prompt
assert 'routes.txt:160 (new)' in prompt and 'select a fallback route' in prompt
activity('Read')
source = Path('routes.txt').read_text().splitlines()
assert len(source) == 1000 and source[319] == 'CONTEXT_BEYOND_12_KIB'
assert source[159] == 'select a fallback route'
answer('The fallback selects ' + source[799].split(' = ')[1] + '.')
""",
    )

    tui = s.start("-r", "HEAD", cwd=repo)
    tui.wait_for("select a fallback route").cursor_on(r"▌.*select a fallback route")
    tui.cmd("ask").wait_for("Private companion")
    tui.type("What is the default fallback route?").keys("Enter")
    tui.wait_for("The fallback selects cobalt route")
    tui.keys("Escape").quit()
