#!/usr/bin/env python3
# Fork review guide: Ask reads the selected PR commit's archive, not an unrelated checkout or unsafe paths.
import io
import tarfile

from e2e.companion import fake_claude

from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    checkout = repos.empty_repo(s.dir / "unrelated")
    repos.write(checkout / "tests/pinned-context.txt", "WRONG_CHECKOUT_SECRET checkout test token\n")
    repos.git(checkout, "add", ".")
    repos.git(checkout, "commit", "-qm", "unrelated work")
    repos.git(checkout, "remote", "add", "origin", "https://github.com/unrelated/work.git")

    archive = s.dir / "pinned.tar.gz"
    with tarfile.open(archive, "w:gz") as tar:
        for path, data in [
            ("tests/pinned-context.txt", b"Unchanged test source from the pinned forge revision.\n"),
            ("tests/.env", b"SYNTHETIC_EXCLUDED_TEST_TOKEN\n"),
        ]:
            item = tarfile.TarInfo(f"octo-demo-bbbbbbb/{path}")
            item.size = len(data)
            tar.addfile(item, io.BytesIO(data))
        link = tarfile.TarInfo("octo-demo-bbbbbbb/tests/pinned-link.txt")
        link.type = tarfile.SYMTYPE
        link.linkname = str(checkout / "tests/pinned-context.txt")
        tar.addfile(link)

    fake_claude(
        s,
        """
activity('Read')
assert 'WRONG_CHECKOUT_SECRET' not in prompt
assert Path('tests/pinned-context.txt').read_text().startswith('Unchanged test source')
assert not Path('tests/pinned-link.txt').is_symlink() and not Path('tests/pinned-link.txt').exists()
assert not Path('tests/.env').exists()
answer('Confirmed pinned test source and excluded unsafe paths.')
""",
    )
    tui = s.start(
        "pr",
        "https://github.com/octo/demo/pull/1",
        cwd=checkout,
        env={
            "E2E_VARIANT": "threads_and_commits",
            "E2E_GITHUB_ASK_ARCHIVE": str(archive),
            "E2E_GITHUB_ASK_ARCHIVE_SHA": "b" * 40,
        },
    )
    tui.wait_for("PR Description").keys(";k", "j", "Enter")
    tui.wait_for(r"Commit Message \(bbbbbbb\)").wait_for("line from commit B")
    tui.keys(";j").cursor_on(r"▌.*line from commit B")
    tui.cmd("ask").wait_for("Private companion")
    tui.type("What unchanged test covers this?").keys("Enter").wait_for("Confirmed pinned test source")
    tui.forge_expect(r"tarball/b{40}")
    tui.forge_reject(r"git/(trees|blobs)|contents/tests/")
    tui.keys("Escape").quit()
