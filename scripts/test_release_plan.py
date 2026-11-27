import json
import os
import subprocess
import sys
import textwrap
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

import release_plan as plan


class NextPreview(unittest.TestCase):
    def test_package_version_allows_toml_spacing(self):
        with TemporaryDirectory() as directory:
            path = Path(directory) / "Cargo.toml"
            (path.parent / "src").mkdir()
            (path.parent / "src/lib.rs").touch()
            path.write_text("[workspace]\n[package]\nname = 'fixture'\nversion  = '0.1.0'\n")
            self.assertEqual(plan.cargo_version(path), "0.1.0")

    def test_release_prep_runs_without_tomllib(self):
        command = ("import sys; sys.modules['tomllib'] = None; "
                   "import release_plan; release_plan.git = lambda *args: ''; "
                   "sys.exit(release_plan.main(['main']))")
        result = subprocess.check_output(
            [sys.executable, "-c", command], cwd=plan.ROOT / "scripts", text=True)
        self.assertRegex(result, r"(?m)^tag=v[0-9.]+-preview\.1$")

    def test_final_requires_latest_successful_main_push_at_dispatch_sha(self):
        workflow = (plan.ROOT / ".github/workflows/macos.yml").read_text()
        step = workflow.split("      - name: Require the latest main check", 1)[1].split("\n      #", 1)[0]
        script = textwrap.dedent(step.split("        run: |\n", 1)[1])
        for sha, conclusion, allowed in [
            ("current", "success", True), ("old", "success", False),
            ("current", "failure", False), ("current", None, False),
        ]:
            with self.subTest(sha=sha, conclusion=conclusion):
                feed = json.dumps({"workflow_runs": [{"head_sha": sha, "conclusion": conclusion}]})
                result = subprocess.run(
                    ["bash", "-eo", "pipefail", "-c", 'gh() { echo "$FEED"; }\n' + script],
                    env=dict(os.environ, GITHUB_SHA="current", FEED=feed), capture_output=True)
                self.assertEqual(result.returncode == 0, allowed)

    def test_publish_marks_only_previews_as_prereleases(self):
        workflow = (plan.ROOT / ".github/workflows/macos.yml").read_text()
        script = textwrap.dedent(workflow.split("          # The tag is created with the release", 1)[1].split("\n", 1)[1])
        # Exercise the actual publish arguments without accessing GitHub.
        mocks = 'git() { [[ $1 == fetch ]]; }\ngh() { echo "$@"; }\n'
        for tag, preview in [("v0.1.0", False), ("v0.1.0-preview.1", True)]:
            with self.subTest(tag=tag):
                output = subprocess.check_output(
                    ["bash", "-e", "-c", mocks + script], text=True,
                    env=dict(os.environ, RELEASE_TAG=tag, GITHUB_SHA="checked"))
                create = output.splitlines()[0].split()
                self.assertEqual(create[:5], ["release", "create", tag, "--target", "checked"])
                self.assertEqual("--prerelease" in create, preview)

    def test_workflow_propagates_planner_failure(self):
        workflow = (plan.ROOT / ".github/workflows/macos.yml").read_text()
        step = workflow.split("      - id: decide\n", 1)[1].split("\n      #", 1)[0]
        script = textwrap.dedent(step.split("        run: |\n", 1)[1])
        with TemporaryDirectory() as directory:
            env = dict(os.environ, GITHUB_EVENT_NAME="workflow_dispatch", RELEASE="true",
                       GITHUB_REF="refs/heads/main", GITHUB_OUTPUT=str(Path(directory) / "outputs"))
            result = subprocess.run(["bash", "-e", "-c", "python3() { return 17; }\n" + script], env=env)
            self.assertEqual(result.returncode, 17)

    def test_counts_past_every_tag_including_ones_that_never_published(self):
        tags = ["v0.1.0-preview.9", "v0.1.0-preview.21", "v0.1.0-preview.23", "v0.1.0-preview.10"]
        self.assertEqual(plan.last_tag("0.1.0", tags), "v0.1.0-preview.23")
        self.assertEqual(plan.next_tag("0.1.0", tags), "v0.1.0-preview.24")

    def test_a_new_version_starts_at_one(self):
        tags = ["v0.1.0-preview.23", "v0.1.0", "notes", "v0.1.0-preview.x"]
        self.assertIsNone(plan.last_tag("0.2.0", tags))
        self.assertEqual(plan.next_tag("0.2.0", tags), "v0.2.0-preview.1")
        self.assertEqual(plan.next_tag("0.1.0", tags), "v0.1.0-preview.24")

    def test_the_version_is_matched_whole(self):
        self.assertEqual(plan.next_tag("0.1.0", ["v0.1.00-preview.7", "v0x1x0-preview.8"]), "v0.1.0-preview.1")


class PullRequestTests(unittest.TestCase):
    def test_prose_alone_needs_no_tests(self):
        self.assertFalse(plan.needs_tests(["README.md", "docs/HOST.md", "docs/images/tabs.png", "LICENSE"]))

    def test_anything_else_does(self):
        self.assertTrue(plan.needs_tests(["README.md", "runtime/test/safety.test.ts"]))
        self.assertTrue(plan.needs_tests([".github/workflows/macos.yml"]))
        self.assertTrue(plan.needs_tests(["crates/plait/src/ui/tabs.rs"]))

    def test_an_unknown_or_empty_change_is_tested(self):
        self.assertTrue(plan.needs_tests(None))
        self.assertTrue(plan.needs_tests([]))


class Merged(unittest.TestCase):
    def run_plan(self, *argv):
        import contextlib
        import io
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = plan.main(list(argv))
        return code, dict(line.split("=", 1) for line in out.getvalue().splitlines())

    def test_a_merge_is_tested_and_never_released(self):
        # HEAD against itself is no change, which is tested like an unknown one.
        code, result = self.run_plan("merged", "HEAD")
        self.assertEqual(code, 0)
        self.assertEqual(result, {"test": "true", "release": "false"})

    def test_prose_push_that_replaces_pending_code_check_tests_main(self):
        with patch.object(plan, "changed", return_value=["CHANGELOG.md"]):
            self.assertEqual(self.run_plan("merged", "before")[1]["test"], "true")

    def test_an_unknown_previous_commit_is_tested(self):
        code, result = self.run_plan("merged", "0" * 40)
        self.assertEqual(code, 0)
        self.assertEqual(result, {"test": "true", "release": "false"})

    def test_final_promotes_unchanged_preview_and_refuses_existing_tag(self):
        with patch.object(plan, "cargo_version", return_value="0.1.0"), \
             patch.object(plan, "git", return_value="v0.1.0-preview.10"), \
             patch.object(plan, "changed", return_value=[]):
            self.assertEqual(self.run_plan("main")[1]["release"], "false")
            code, result = self.run_plan("main", "--final")
            self.assertEqual((code, result["release"], result["tag"]), (0, "true", "v0.1.0"))
            with patch.object(plan, "git", return_value="v0.1.0"):
                self.assertNotEqual(self.run_plan("main", "--final")[0], 0)

    def test_a_missing_argument_is_refused(self):
        import contextlib
        import io
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(plan.main(["merged"]), 2)


class Ships(unittest.TestCase):
    def test_prose_tests_and_ci_do_not_make_a_release(self):
        self.assertFalse(plan.ships([
            "docs/VERIFICATION.md", "AGENTS.md", ".github/workflows/macos.yml",
            "runtime/test/checkout-queue.test.ts", "scripts/acceptance/run.py",
            "scripts/test_release_plan.py", "scripts/release_plan.py", "scripts/check.sh",
            "scripts/release_reuse.py", "scripts/test_release_reuse.py",
            "scripts/check-installed-changes.sh", "scripts/installed-layout-check.mjs",
            "scripts/installed-fixture.mjs",
        ]))

    def test_code_and_what_packaging_reads_do(self):
        for path in [
            "crates/plait/src/ui/tabs.rs", "crates/plait-client/src/session.rs", "runtime/src/runtime.ts",
            "runtime/package-lock.json", "assets/themes/nord.yaml", "Cargo.lock",
            "scripts/build-macos.sh", "scripts/update_manifest.py", "scripts/install.sh",
        ]:
            self.assertTrue(plan.ships(["docs/GUIDE.md", path]), path)

    def test_no_change_since_the_last_preview_makes_none(self):
        self.assertFalse(plan.ships([]))

    def test_an_unknown_change_ships(self):
        self.assertTrue(plan.ships(None))


if __name__ == "__main__":
    unittest.main()
