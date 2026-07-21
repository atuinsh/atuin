"""Tests for prerelease_docs (prune logic for versioned docs).

Run: uv run python -m unittest scripts.test_prerelease_docs -v
(from the docs/ directory)
"""

import unittest

from prerelease_docs import base_version, is_prerelease, prereleases_to_delete


class TestBaseVersion(unittest.TestCase):
    def test_full_stable(self):
        self.assertEqual(base_version("18.18.0"), (18, 18, 0))

    def test_prerelease_suffix_stripped(self):
        self.assertEqual(base_version("18.18.0-beta.1"), (18, 18, 0))

    def test_minor_only_padded(self):
        # mike stores stable releases under their major.minor id (e.g. "18.17").
        self.assertEqual(base_version("18.17"), (18, 17, 0))

    def test_leading_v_tolerated(self):
        self.assertEqual(base_version("v18.18.0-rc.2"), (18, 18, 0))

    def test_non_version_raises(self):
        with self.assertRaises(ValueError):
            base_version("main")


class TestIsPrerelease(unittest.TestCase):
    def test_prerelease_true(self):
        self.assertTrue(is_prerelease("18.18.0-beta.1"))

    def test_stable_minor_false(self):
        self.assertFalse(is_prerelease("18.17"))

    def test_stable_full_false(self):
        self.assertFalse(is_prerelease("18.18.0"))

    def test_main_false(self):
        self.assertFalse(is_prerelease("main"))


class TestPrereleasesToDelete(unittest.TestCase):
    def versions(self, *ids):
        return [{"version": v, "title": v, "aliases": []} for v in ids]

    def test_stable_release_prunes_matching_beta(self):
        # 18.18.0 lands -> the 18.18.0-beta.1 preview is superseded.
        vs = self.versions("main", "18.17", "18.18.0-beta.1")
        self.assertEqual(
            prereleases_to_delete(vs, boundary="18.18.0"),
            ["18.18.0-beta.1"],
        )

    def test_later_stable_prunes_older_beta(self):
        # 18.19.0 lands while an 18.18 beta is still around (18.18 stable skipped).
        vs = self.versions("main", "18.17", "18.18.0-beta.1")
        self.assertEqual(
            prereleases_to_delete(vs, boundary="18.19.0"),
            ["18.18.0-beta.1"],
        )

    def test_older_patch_keeps_future_beta(self):
        # A patch to an older line must NOT delete a beta for a newer line.
        vs = self.versions("main", "18.18.0-beta.1")
        self.assertEqual(
            prereleases_to_delete(vs, boundary="18.17.5"),
            [],
        )

    def test_new_beta_supersedes_same_base_beta(self):
        # Deploying beta.2 (excluded) prunes beta.1 of the same base.
        vs = self.versions("18.17", "18.18.0-beta.1", "18.18.0-beta.2")
        self.assertEqual(
            prereleases_to_delete(vs, boundary="18.18.0-beta.2", exclude="18.18.0-beta.2"),
            ["18.18.0-beta.1"],
        )

    def test_new_beta_supersedes_older_line_beta(self):
        # Deploying an 18.19 beta prunes a stale 18.18 beta (older base).
        vs = self.versions("18.17", "18.18.0-beta.3", "18.19.0-beta.1")
        self.assertEqual(
            prereleases_to_delete(vs, boundary="18.19.0-beta.1", exclude="18.19.0-beta.1"),
            ["18.18.0-beta.3"],
        )

    def test_exclude_is_never_returned(self):
        vs = self.versions("18.18.0-beta.2")
        self.assertEqual(
            prereleases_to_delete(vs, boundary="18.18.0-beta.2", exclude="18.18.0-beta.2"),
            [],
        )

    def test_stable_and_main_never_deleted(self):
        # Only prereleases are candidates; stable minors and 'main' are ignored.
        vs = self.versions("main", "18.17", "18.16")
        self.assertEqual(prereleases_to_delete(vs, boundary="18.17.0"), [])

    def test_unparseable_versions_ignored(self):
        # A stray non-semver id must not crash the prune.
        vs = self.versions("main", "weird-thing", "18.18.0-beta.1")
        self.assertEqual(
            prereleases_to_delete(vs, boundary="18.18.0"),
            ["18.18.0-beta.1"],
        )

    def test_empty_versions(self):
        self.assertEqual(prereleases_to_delete([], boundary="18.18.0"), [])


if __name__ == "__main__":
    unittest.main()
