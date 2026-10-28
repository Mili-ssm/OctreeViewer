import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
import check_docs_links as c  # noqa: E402


class LinkCheckerTests(unittest.TestCase):
    def check(self, files, allow=None):
        with tempfile.TemporaryDirectory() as root:
            for name, text in files.items():
                p = os.path.join(root, name)
                os.makedirs(os.path.dirname(p), exist_ok=True)
                with open(p, "w", encoding="utf-8") as fh:
                    fh.write(text)
            allow_path = None
            if allow is not None:
                allow_path = os.path.join(root, "allow.txt")
                with open(allow_path, "w", encoding="utf-8") as fh:
                    fh.write(allow)
            mds = [n for n in files if n.endswith(".md")]
            return c.run(root, mds, allow_path)

    def test_untracked_target_is_broken_when_tracked_set_given(self):
        with tempfile.TemporaryDirectory() as root:
            os.makedirs(os.path.join(root, "artifacts"))
            open(os.path.join(root, "artifacts", "out.png"), "w").close()
            with open(os.path.join(root, "a.md"), "w", encoding="utf-8") as fh:
                fh.write("[x](artifacts/out.png)\n")
            tracked = c.tracked_paths(root, ["a.md"])
            self.assertEqual(c.run(root, ["a.md"], None, None), [])
            self.assertEqual(
                c.run(root, ["a.md"], None, tracked),
                ["a.md:1: artifacts/out.png"],
            )

    def test_accented_heading_anchor_resolves(self):
        errs = self.check({
            "a.md": "[x](b.md#evidencia-del-séptimo-incremento)\n",
            "b.md": "# Evidencia del séptimo incremento\n",
        })
        self.assertEqual(errs, [])

    def test_unaccented_anchor_is_broken(self):
        errs = self.check({
            "a.md": "[x](b.md#evidencia-del-septimo-incremento)\n",
            "b.md": "# Evidencia del séptimo incremento\n",
        })
        self.assertEqual(errs, ["a.md:1: b.md#evidencia-del-septimo-incremento"])

    def test_emphasis_underscore_dropped(self):
        errs = self.check({"a.md": "[x](#emph-x)\n## _emph_ x\n"})
        self.assertEqual(errs, [])

    def test_in_word_underscore_kept(self):
        errs = self.check({"a.md": "[x](#max_index)\n## `max_index`\n"})
        self.assertEqual(errs, [])

    def test_in_word_underscore_kept_next_to_non_ascii_letters(self):
        self.assertEqual(c.slugify("é_x"), "é_x")
        self.assertEqual(c.slugify("_énfasis_ x"), "énfasis-x")
        errs = self.check({"a.md": "[x](#é_x)\n## é_x\n"})
        self.assertEqual(errs, [])

    def test_trailing_hash_handling(self):
        errs = self.check({"a.md": "[x](#c) [y](#d)\n# C#\n# D #\n"})
        self.assertEqual(errs, [])

    def test_percent_encoded_anchor(self):
        errs = self.check({"a.md": "[x](#s%C3%A9ptimo)\n# Séptimo\n"})
        self.assertEqual(errs, [])

    def test_missing_file(self):
        errs = self.check({"a.md": "[x](nope.md)\n"})
        self.assertEqual(errs, ["a.md:1: nope.md"])

    def test_missing_anchor(self):
        errs = self.check({"a.md": "[x](#gone)\n# Here\n"})
        self.assertEqual(errs, ["a.md:1: #gone"])

    def test_duplicate_heading_suffix(self):
        errs = self.check({
            "a.md": "# Dup\n## Dup\n[a](#dup) [b](#dup-1) [c](#dup-2)\n",
        })
        self.assertEqual(errs, ["a.md:3: #dup-2"])

    def test_link_in_code_fence_ignored(self):
        errs = self.check({"a.md": "```\n[x](nope.md)\n```\n"})
        self.assertEqual(errs, [])

    def test_external_links_ignored(self):
        errs = self.check({
            "a.md": "[x](https://e.com/a) [y](mailto:a@b.c) [z](http://e.com)\n"
        })
        self.assertEqual(errs, [])

    def test_allowlisted_break_passes_and_stale_entry_fails(self):
        files = {"a.md": "[x](nope.md)\n"}
        self.assertEqual(self.check(files, "a.md|nope.md\n"), [])
        errs = self.check({"a.md": "ok\n"}, "a.md|nope.md\n")
        self.assertEqual(errs, ["stale allowlist entry: a.md|nope.md"])


if __name__ == "__main__":
    unittest.main()
