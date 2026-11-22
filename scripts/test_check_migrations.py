"""Runs scripts/check_migrations.py against temp trees. See docs/plan/0.1.9/00-guard.md."""

import io
import pathlib
import tempfile
import unittest
from contextlib import redirect_stdout

import check_migrations

ROOT = pathlib.Path(__file__).resolve().parents[1]


def run(root: pathlib.Path) -> tuple[int, str]:
    buffer = io.StringIO()
    with redirect_stdout(buffer):
        code = check_migrations.main(["--root", str(root)])
    return code, buffer.getvalue()


class CheckMigrationTests(unittest.TestCase):
    def write(self, root: pathlib.Path, name: str, sql: str, allowlist: str = "") -> None:
        migrations = root / "migrations"
        migrations.mkdir(parents=True)
        (migrations / name).write_text(sql, encoding="utf-8")
        (migrations / "destructive-allowlist.txt").write_text(allowlist, encoding="utf-8")

    def test_current_tree_passes(self):
        code, out = run(ROOT)
        self.assertEqual(code, 0, out)
        self.assertIn("migrations: 6 files, 3 allowlisted", out)

    def test_drop_table_fails(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            self.write(root, "0099_drop.sql", "DROP TABLE x;\n")
            code, out = run(root)
            self.assertEqual(code, 1)
            self.assertIn("migrations/0099_drop.sql", out)

    def test_alter_drop_column_fails(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            self.write(root, "0099_alter.sql", "ALTER TABLE memories DROP COLUMN reason;\n")
            code, out = run(root)
            self.assertEqual(code, 1)
            self.assertIn("migrations/0099_alter.sql", out)

    def test_delete_and_truncate_fail(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            self.write(root, "0099_delete.sql", "DELETE FROM memories;\n")
            code, out = run(root)
            self.assertEqual(code, 1, out)
            self.assertIn("migrations/0099_delete.sql", out)
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            self.write(root, "0099_truncate.sql", "TRUNCATE observations;\n")
            code, out = run(root)
            self.assertEqual(code, 1, out)
            self.assertIn("migrations/0099_truncate.sql", out)

    def test_comments_and_fk_rules_pass(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            sql = (
                "-- DROP TABLE x\n"
                "/* TRUNCATE observations; */\n"
                "CREATE TABLE t (\n"
                "  id text REFERENCES memories ON DELETE CASCADE,\n"
                "  other text REFERENCES memories ON DELETE SET NULL\n"
                ");\n"
            )
            self.write(root, "0099_safe.sql", sql)
            code, out = run(root)
            self.assertEqual(code, 0, out)

    def test_additive_passes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            self.write(
                root,
                "0099_add.sql",
                "ALTER TABLE memories ADD COLUMN rank text NOT NULL DEFAULT 'normal';\n",
            )
            code, out = run(root)
            self.assertEqual(code, 0, out)

    def test_stale_allowlist_fails(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            self.write(
                root,
                "0099_clean.sql",
                "CREATE TABLE t (id text);\n",
                "0099_clean.sql|DROP TABLE missing|no such statement\n",
            )
            code, out = run(root)
            self.assertEqual(code, 1)
            self.assertIn("stale allowlist entry", out)


if __name__ == "__main__":
    unittest.main()
