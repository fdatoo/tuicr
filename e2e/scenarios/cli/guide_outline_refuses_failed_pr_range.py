#!/usr/bin/env python3
# Covers the fork's review-guide rebuild: a failed requested PR range never exports the cumulative diff.

from e2e.core import TOOL_PATH

from e2e import E2E_DIR, Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    repo = repos.pr_repo(s.dir / "repo")
    config = s.path_of(s.name) / "config/tuicr/config.toml"
    repos.write(config, 'initial_commit_selection = "oldest"\n')
    original = E2E_DIR / "bin/gh"
    fake = repos.write(
        s.dir / "bin/gh",
        "#!/usr/bin/env python3\n"
        "import os, sys\n"
        "if any('vnd.github.diff' in arg for arg in sys.argv) and any('/compare/' in arg for arg in sys.argv):\n"
        "    print('Requested range does not exist', file=sys.stderr)\n"
        "    sys.exit(1)\n"
        f"os.execv({str(original)!r}, [{str(original)!r}, *sys.argv[1:]])\n",
    )
    fake.chmod(0o755)
    result = s.cli_result(
        "guide",
        "outline",
        "pr",
        "1",
        cwd=repo,
        path=f"{fake.parent}:{E2E_DIR}/bin:{TOOL_PATH}",
        env={"E2E_VARIANT": "threads_and_commits"},
    )
    assert result.rc != 0, result
    assert "Requested range does not exist" in result.out, result
    assert "diff: sha256:" not in result.out, result
