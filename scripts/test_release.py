#!/usr/bin/env python3
"""Run with python3 -m unittest discover -s scripts -p test_release.py -v."""

from contextlib import redirect_stdout
import importlib.util
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location("release", Path(__file__).with_name("release.py"))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.deny = self.root / "private.txt"
        self.deny.write_text("Example Private Name\nZ9Y8X7W6V5\n")
        self.binary = self.root / "kr"

    def check_bytes(self, data):
        self.binary.write_bytes(data)
        release.privacy_check(self.binary, self.deny)

    def test_local_path_is_blocked_without_printing_value(self):
        with self.assertRaisesRegex(ValueError, "local user path") as error:
            self.check_bytes(b"prefix /Users/private-person/project/file.rs suffix\n")
        self.assertNotIn("private-person", str(error.exception))

    def test_private_terms_and_email_are_blocked(self):
        for data in [b"example private name", b"Z9Y8X7W6V5", b"private@example.com"]:
            with self.subTest(data=data), self.assertRaises(ValueError):
                self.check_bytes(data)

    def test_unicode_terms_are_blocked_even_if_strings_omits_them(self):
        self.deny.write_text("\u6d4b\u8bd5\u59d3\u540d\n")
        with self.assertRaisesRegex(ValueError, "private blocked term"):
            self.check_bytes("prefix \u6d4b\u8bd5\u59d3\u540d suffix".encode())

    def test_contextual_team_id_is_blocked_but_random_identifier_is_allowed(self):
        self.check_bytes(b"ABCDEFGHIJKLMNOP Q1W2E3R4T5\n")
        with self.assertRaisesRegex(ValueError, "team ID"):
            self.check_bytes(b"TeamIdentifier=Q1W2E3R4T5\n")

    def test_missing_and_empty_private_list_are_rejected(self):
        self.binary.write_bytes(b"clean binary")
        with self.assertRaises(ValueError):
            release.privacy_check(self.binary, None)
        self.deny.write_text("# comment only\n")
        with self.assertRaises(ValueError):
            release.privacy_check(self.binary, self.deny)

    def test_build_home_is_read_at_runtime_and_blocked(self):
        with patch.object(release.Path, "home", return_value=Path("/Volumes/BuildHome")):
            with self.assertRaisesRegex(ValueError, "build HOME"):
                self.check_bytes(b"source /Volumes/BuildHome/project/file.rs\n")

    def test_build_username_is_read_from_id_and_blocked(self):
        original_output = release.output

        def output(*args, **kwargs):
            if args == ("id", "-un"):
                return "synthetic-build-user\n"
            return original_output(*args, **kwargs)

        with patch.object(release, "output", side_effect=output):
            with self.assertRaisesRegex(ValueError, "build username"):
                self.check_bytes(b"synthetic-build-user\n")

    def fake_binary(self, init=False, config=False):
        commands = ["skill", "setup", "doctor", "check", "decide"]
        if init:
            commands.append("init")
        self.binary.write_text(
            "#!/bin/sh\n"
            'case "$1" in\n'
            '  --version) echo "kr 0.1.0";;\n'
            "  --help) printf '%s\\n' " +
            " ".join("'" + f"  {c}  Command" + "'" for c in commands) + ";;\n" +
            ("  skill) exit 0;;\n" if config else
             "  skill) echo 'no reference `config`; references: decisions, json, locks' >&2; exit 2;;\n") +
            "  *) exit 2;;\n"
            "esac\n")
        self.binary.chmod(0o755)

    def test_readme_rejects_unknown_command(self):
        self.fake_binary()
        with self.assertRaisesRegex(ValueError, "kr typo"):
            release.command_check(self.binary, "`kr typo`")

    def test_pending_47_is_explicit_and_blocks_publish(self):
        self.fake_binary()
        readme = (release.ROOT / "packaging/homebrew-tap/README.md").read_text()
        pending = release.command_check(self.binary, readme)
        self.assertEqual(pending, ["kr init: pending #47",
                                   "kr skill --reference config: pending #47"])
        with self.assertRaisesRegex(ValueError, "Publish refused.*pending #47"):
            release.require_publish_ready(pending)

    def test_available_47_commands_remove_pending(self):
        self.fake_binary(init=True, config=True)
        self.assertEqual(release.command_check(
            self.binary, "`kr init` and `kr skill --reference config`"), [])
        release.require_publish_ready([])

    def test_config_runtime_failure_is_not_classified_as_pending(self):
        self.fake_binary()
        self.binary.write_text(self.binary.read_text().replace("exit 2;;", "exit 1;;"))
        with self.assertRaisesRegex(ValueError, "binary/runtime error"):
            release.command_check(self.binary, "`kr skill --reference config`")

    def test_verify_only_cli_allows_pending(self):
        self.fake_binary()
        env = {**os.environ, "KR46_DENY_FILE": str(self.deny)}
        result = subprocess.run(
            [str(release.ROOT / "scripts/release.sh"), "0.1.0", "--verify-only", str(self.binary)],
            text=True, capture_output=True, env=env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("kr init: pending #47", result.stdout)

    def test_dry_run_prepares_and_prints_without_public_operations(self):
        self.fake_binary()
        dist = self.root / "dist"

        def fake_build(ref, version, dest):
            import shutil
            shutil.copy2(self.binary, dest / "kr")
            return "a" * 40

        log = io.StringIO()
        with patch.dict(os.environ, {"KR46_DENY_FILE": str(self.deny)}), \
                patch.object(release, "build", side_effect=fake_build), \
                patch.object(release, "run", wraps=release.run) as calls, redirect_stdout(log):
            release.main(["0.1.0", "--ref", "HEAD", "--dry-run", "--output", str(dist)])
        self.assertIn("gh release create v0.1.0", log.getvalue())
        self.assertIn("pending #47", log.getvalue())
        self.assertFalse(any(call.args[0] in {"gh", "git"} for call in calls.call_args_list))
        self.assertEqual(sorted(str(p.relative_to(dist / "tap"))
                                for p in (dist / "tap").rglob("*") if p.is_file()),
                         ["Formula/kr.rb", "README.md"])

    def test_public_tap_commit_uses_the_noreply_identity(self):
        commands = release.publish_commands(self.root, "0.1.0", self.root / "tap")
        commit = next(c for c in commands if "commit" in c)
        self.assertIn(f"user.name={release.TAP_AUTHOR_NAME}", commit)
        self.assertIn(f"user.email={release.TAP_AUTHOR_EMAIL}", commit)
        self.assertTrue(release.TAP_AUTHOR_EMAIL.endswith("@users.noreply.github.com"))

    def test_publish_pending_never_reaches_public_operations(self):
        self.fake_binary()

        def fake_build(ref, version, dest):
            import shutil
            shutil.copy2(self.binary, dest / "kr")
            return "a" * 40

        original_output = release.output

        def fake_output(*args, **kwargs):
            if args[0] == "git":
                return "a" * 40
            return original_output(*args, **kwargs)

        with patch.dict(os.environ, {"KR46_DENY_FILE": str(self.deny)}), \
                patch.object(release, "build", side_effect=fake_build), \
                patch.object(release, "output", side_effect=fake_output), \
                patch.object(release, "publish_commands") as commands, redirect_stdout(io.StringIO()):
            with self.assertRaisesRegex(ValueError, "Publish refused.*pending #47"):
                release.main(["0.1.0", "--publish", "--output", str(self.root / "dist")])
        commands.assert_not_called()

    def test_archive_has_only_binary_and_no_personal_owner_metadata(self):
        self.binary.write_bytes(b"clean binary")
        archive = self.root / "kr.tar.gz"
        release.archive_binary(self.binary, archive)
        with tarfile.open(archive) as tar:
            self.assertEqual(tar.getnames(), ["kr"])
            info = tar.getmember("kr")
            self.assertEqual((info.uid, info.gid, info.uname, info.gname), (0, 0, "", ""))
            self.assertEqual(info.mode, 0o755)

    def test_formula_requires_all_placeholders_resolved(self):
        template = (release.ROOT / "packaging/homebrew-tap/Formula/kr.rb").read_text()
        formula = release.render_formula(template, "1.2.3", "a" * 64, "file:///tmp/kr.tar.gz")
        self.assertIn('version "1.2.3"', formula)
        self.assertNotIn("@SHA256@", formula)
        with self.assertRaises(ValueError):
            release.render_formula("@UNKNOWN@", "1.2.3", "a" * 64, "file:///tmp/kr.tar.gz")


if __name__ == "__main__":
    unittest.main()
