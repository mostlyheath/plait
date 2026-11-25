import unittest

import release_plan as plan


class NextPreview(unittest.TestCase):
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

    def test_an_unknown_previous_commit_is_tested(self):
        code, result = self.run_plan("merged", "0" * 40)
        self.assertEqual(code, 0)
        self.assertEqual(result, {"test": "true", "release": "false"})

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
            "scripts/release_reuse.py", "scripts/install-nextest.sh", "scripts/test_release_reuse.py",
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
