import tempfile
import unittest
from pathlib import Path

import changelog


def tree(notes, existing=None):
    root = Path(tempfile.mkdtemp())
    (root / "changes").mkdir()
    (root / "changes" / "README.md").write_text("How to write a note.\n")
    for name, body in notes.items():
        (root / "changes" / name).write_text(body)
    if existing is not None:
        (root / "CHANGELOG.md").write_text(existing)
    return root


class Collect(unittest.TestCase):
    def test_notes_become_a_section_by_category_and_are_removed(self):
        root = tree({
            "b.md": "Fixed\nThreads keep their name.\n",
            "a.md": "Added\nCreate Linear issues\nfrom Plait.\n",
        })
        self.assertEqual(changelog.collect("v0.1.0-preview.82", root), 2)
        text = (root / "CHANGELOG.md").read_text()
        self.assertIn("## v0.1.0-preview.82\n\n### Added\n\n- Create Linear issues from Plait.\n\n### Fixed\n\n- Threads keep their name.", text)
        self.assertTrue(text.startswith("# Changelog"))
        self.assertEqual(sorted(p.name for p in (root / "changes").iterdir()), ["README.md"])

    def test_a_new_section_goes_above_older_ones(self):
        root = tree({"a.md": "Changed\nNew.\n"}, "# Changelog\n\nIntro.\n\n## v0.1.0-preview.81\n\n### Fixed\n\n- Old.\n")
        changelog.collect("v0.1.0-preview.82", root)
        text = (root / "CHANGELOG.md").read_text()
        self.assertLess(text.index("preview.82"), text.index("preview.81"))
        self.assertIn("- Old.", text)

    def test_bad_notes_and_tags_are_refused_without_changes(self):
        for notes in ({"a.md": "Improved\nSomething.\n"}, {"a.md": "Fixed\n"}, {}):
            with self.subTest(notes=notes), self.assertRaises(changelog.Invalid):
                changelog.collect("v0.1.0-preview.82", tree(notes))
        with self.assertRaises(changelog.Invalid):
            changelog.collect("preview-82", tree({"a.md": "Fixed\nX.\n"}))

    def test_a_tag_is_collected_once(self):
        root = tree({"a.md": "Fixed\nX.\n"}, "# Changelog\n\n## v0.1.0-preview.82\n\n- Y.\n")
        with self.assertRaises(changelog.Invalid):
            changelog.collect("v0.1.0-preview.82", root)
        self.assertTrue((root / "changes" / "a.md").exists())


class Section(unittest.TestCase):
    def test_returns_only_that_release(self):
        root = tree({}, "# Changelog\n\n## v0.1.0-preview.82\n\n### Added\n\n- New.\n\n## v0.1.0-preview.81\n\n- Old.\n")
        self.assertEqual(changelog.section("v0.1.0-preview.82", root), "### Added\n\n- New.")

    def test_a_missing_section_fails(self):
        with self.assertRaises(changelog.Invalid):
            changelog.section("v0.1.0-preview.83", tree({}, "# Changelog\n"))
        with self.assertRaises(changelog.Invalid):
            changelog.section("v0.1.0-preview.83", tree({}))


if __name__ == "__main__":
    unittest.main()
