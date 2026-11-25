import unittest

import release_reuse as reuse

REPO = "owner/plait"
SHA = "merge"
BEFORE = "parent"
TREE = "tree-a"


def request(**changes):
    value = {
        "number": 7,
        "merged_at": "2026-11-25T00:00:00Z",
        "merge_commit_sha": SHA,
        "base": {"ref": "main"},
        "head": {"sha": "head", "repo": {"full_name": REPO}},
    }
    value.update(changes)
    return value


def run(number, **changes):
    value = {
        "id": number,
        "run_number": number,
        "run_attempt": 1,
        "event": "pull_request",
        "head_sha": "head",
        "conclusion": "success",
        "path": ".github/workflows/macos.yml",
        "head_repository": {"full_name": REPO},
        "html_url": f"https://example.test/runs/{number}",
    }
    value.update(changes)
    return value


def decide(requests=None, runs=None, trees=None, parent=BEFORE, before=BEFORE):
    trees = trees if trees is not None else {1: TREE}
    return reuse.decide(
        SHA, before, parent, TREE, REPO,
        [request()] if requests is None else requests,
        lambda head: [run(1)] if runs is None else runs,
        lambda r: trees.get(r["id"]),
    )


class Reuse(unittest.TestCase):
    def test_a_passing_check_of_the_same_tree_is_reused(self):
        self.assertEqual(decide(), "https://example.test/runs/1")

    def test_a_different_tree_runs_the_check(self):
        with self.assertRaises(reuse.NoReuse):
            decide(trees={1: "tree-b"})

    def test_a_missing_record_runs_the_check(self):
        with self.assertRaises(reuse.NoReuse):
            decide(trees={})

    def test_any_passing_run_of_the_tree_counts_newest_first(self):
        checked = []
        url = reuse.decide(
            SHA, BEFORE, BEFORE, TREE, REPO, [request()],
            lambda head: [run(1), run(3), run(2)],
            lambda r: checked.append(r["id"]) or {2: TREE}.get(r["id"]),
        )
        self.assertEqual(url, "https://example.test/runs/2")
        self.assertEqual(checked, [3, 2])

    def test_failed_foreign_or_other_workflow_runs_are_ignored(self):
        for changes in (
            {"conclusion": "failure"},
            {"conclusion": None},
            {"event": "push"},
            {"head_sha": "other"},
            {"path": ".github/workflows/headless.yml"},
            {"head_repository": {"full_name": "fork/plait"}},
        ):
            with self.subTest(changes=changes), self.assertRaises(reuse.NoReuse):
                decide(runs=[run(1, **changes)])

    def test_a_push_of_several_commits_runs_the_check(self):
        with self.assertRaises(reuse.NoReuse):
            decide(parent="other")
        with self.assertRaises(reuse.NoReuse):
            decide(before="")

    def test_the_push_must_be_exactly_one_merged_request(self):
        for requests in (
            [],
            [request(), request(number=8)],
            [request(merged_at=None)],
            [request(merge_commit_sha="other")],
            [request(base={"ref": "release"})],
        ):
            with self.subTest(requests=requests), self.assertRaises(reuse.NoReuse):
                decide(requests=requests)

    def test_a_fork_is_never_reused(self):
        for head in ({"sha": "head", "repo": {"full_name": "fork/plait"}}, {"sha": "head", "repo": None}, {"repo": {"full_name": REPO}}):
            with self.subTest(head=head), self.assertRaises(reuse.NoReuse):
                decide(requests=[request(head=head)])


if __name__ == "__main__":
    unittest.main()
