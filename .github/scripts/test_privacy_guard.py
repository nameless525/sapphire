"""Adversarial privacy fixtures stay in disposable repos and never use real user data."""

from contextlib import redirect_stderr, redirect_stdout
import io
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import install_privacy_hooks
import privacy_guard as pg


ROOT = Path(__file__).resolve().parents[2]
PUBLIC = "123+developer@users.noreply.github.com"
PRIVATE = "person" + "@" + "personal.test"
HOME = "/Us" + "ers/real-person/work/project"
TOKEN = "ghp_" + "A" * 36


class ContentTests(unittest.TestCase):
    def rules(self, data, path=b""):
        return {r for r, _ in pg.content_findings(data, path)}

    def test_paths_emails_and_keys(self):
        cases = [(HOME, "personal-home-path"),
                 ("C:" + "\\Users\\real-person\\work", "personal-home-path"),
                 ("/ho" + "me/\u00e9lodie/work", "personal-home-path"),
                 (PRIVATE, "non-public-email"), (TOKEN, "access-token"),
                 ("-----BEGIN " + "OPENSSH PRIVATE KEY-----", "private-key"),
                 ("https://" + "user:password" + "@" + "host.test", "credential-url")]
        for value, rule in cases:
            with self.subTest(rule=rule, encoding="utf8"):
                self.assertIn(rule, self.rules(value.encode()))
            with self.subTest(rule=rule, encoding="utf16"):
                self.assertIn(rule, self.rules(value.encode("utf-16")))

    def test_binary_paths_and_personal_email(self):
        self.assertIn("personal-home-path", self.rules(b"\xff\0" + HOME.encode("utf-16-le")))
        value = "person" + "@" + "protonmail.com"
        self.assertIn("non-public-email", self.rules(b"\xff\0" + value.encode()))

    def test_fixture_exception_is_path_scoped(self):
        value = ("/ho" + "me/ana/.cache").encode()
        self.assertFalse(self.rules(value, b"crates/sapphire_platform/src/dirs.rs"))
        self.assertIn("personal-home-path", self.rules(value, b"notes.txt"))

    def test_public_and_documentation_identities_are_not_private(self):
        self.assertFalse(self.rules((PUBLIC + " public@sentry.example.invalid noreply@anthropic.com").encode()))
        self.assertTrue(pg.public_email(b"49699333+dependabot[bot]@users.noreply.github.com"))
        self.assertFalse(pg.public_email(b"person@example.com"))

    def test_private_filename_rules(self):
        for path in (b".env", b"src/.env.production", b".aws/credentials", b"signing/release.p12",
                     b"CLAUDE.md", b".claude/tools/run.sh", b".claude/settings.local.json"):
            self.assertTrue(pg.forbidden_path(path), path)
        for path in (b".env.example", b".env.sample", b"crates/sapphire_tod/src/key.rs"):
            self.assertFalse(pg.forbidden_path(path), path)


class RepositoryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "repo"
        self.root.mkdir()
        previous = Path.cwd()
        os.chdir(self.root)
        self.addCleanup(os.chdir, previous)
        env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
                   GIT_TERMINAL_PROMPT="0", LC_ALL="C")
        self.enterContext(patch.dict(os.environ, env, clear=True))
        self.git("init", "-q")
        self.git("config", "user.name", "Developer")
        self.git("config", "user.email", PUBLIC)
        self.write("safe.txt", "safe\n")
        self.base = self.commit()

    def git(self, *args, **kwargs):
        return subprocess.check_output(["git", *args], stderr=subprocess.PIPE, **kwargs).decode().strip()

    def write(self, name, content):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
        self.git("add", "--", name)

    def commit(self, message="Safe change"):
        self.git("commit", "-qm", message)
        return self.git("rev-parse", "HEAD")

    def check(self, *args):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            result = pg.main(list(args))
        return result, out.getvalue() + err.getvalue()

    def install_files(self):
        shutil.copytree(ROOT / ".githooks", self.root / ".githooks")
        scripts = self.root / ".github/scripts"
        scripts.mkdir(parents=True)
        shutil.copy(ROOT / ".github/scripts/privacy_guard.py", scripts)
        install_privacy_hooks.install()

    def test_safe_history_and_index_pass(self):
        self.assertEqual(self.check("--head", "HEAD")[0], 0)
        self.assertEqual(self.check("--staged")[0], 0)

    def test_staged_scan_does_not_read_unchanged_assets(self):
        guard = pg.Guard()
        guard.staged()
        self.assertEqual(len(guard.scanned), 0)
        self.write("one.txt", "one")
        guard.staged()
        self.assertEqual(len(guard.scanned), 1)

    def test_index_not_worktree_is_checked(self):
        self.write("notes.txt", HOME)
        (self.root / "notes.txt").write_text("safe but unstaged")
        code, output = self.check("--staged")
        self.assertEqual(code, 1)
        self.assertIn("personal-home-path", output)
        self.assertNotIn(HOME, output)
        self.assertNotIn("real-person", output)

    def test_unstaged_private_file_is_not_committed_or_scanned(self):
        (self.root / "private.txt").write_text(PRIVATE)
        self.assertEqual(self.check("--staged")[0], 0)

    def test_private_identity_in_environment_is_rejected(self):
        with patch.dict(os.environ, {"GIT_AUTHOR_EMAIL": PRIVATE}):
            code, output = self.check("--staged")
        self.assertEqual(code, 1)
        self.assertNotIn(PRIVATE, output)

    def test_noreply_email_does_not_hide_a_private_author_name(self):
        with patch.dict(os.environ, {"GIT_AUTHOR_NAME": "Private Person"}):
            code, output = self.check("--staged")
        self.assertEqual(code, 1)
        self.assertIn("identity-name-must-use-public-handle", output)
        self.assertNotIn("Private Person", output)

    def test_forced_private_file_and_private_filename_are_rejected(self):
        self.write(".env", "even without a recognized secret")
        self.write(PRIVATE + ".txt", "safe")
        code, output = self.check("--staged")
        self.assertEqual(code, 1)
        self.assertIn("private-file", output)
        self.assertNotIn(PRIVATE, output)

    def test_private_commit_message_and_coauthor_are_rejected(self):
        path = self.root / "message"
        path.write_text("Safe subject\n\nCo-authored-by: Person <" + PRIVATE + ">\n")
        code, output = self.check("--commit-msg", str(path))
        self.assertEqual(code, 1)
        self.assertNotIn(PRIVATE, output)

    def test_transient_secret_removed_in_later_commit_is_still_rejected(self):
        self.write("notes.txt", TOKEN)
        self.commit()
        self.git("rm", "notes.txt")
        self.commit()
        code, output = self.check("--head", "HEAD", "--base", self.base)
        self.assertEqual(code, 1)
        self.assertIn("access-token", output)
        self.assertNotIn(TOKEN, output)

    def test_private_author_on_earlier_outgoing_commit_is_rejected(self):
        self.write("one.txt", "safe")
        with patch.dict(os.environ, {"GIT_AUTHOR_EMAIL": PRIVATE}):
            self.commit()
        self.write("two.txt", "safe")
        self.commit()
        code, output = self.check("--head", "HEAD", "--base", self.base)
        self.assertEqual(code, 1)
        self.assertIn("author", output)

    def test_private_merge_committer_is_rejected(self):
        branch = self.git("branch", "--show-current")
        self.git("checkout", "-qb", "feature")
        self.write("feature.txt", "safe")
        self.commit()
        self.git("checkout", "-q", branch)
        with patch.dict(os.environ, {"GIT_COMMITTER_EMAIL": PRIVATE}):
            self.git("merge", "--no-ff", "-qm", "Safe merge", "feature")
        code, output = self.check("--head", "HEAD", "--base", self.base)
        self.assertEqual(code, 1)
        self.assertIn("committer", output)

    def test_invalid_revisions_fail_closed_without_path_output(self):
        self.assertEqual(self.check("--head", "missing")[0], 2)
        self.assertEqual(self.check("--head=--help")[0], 2)

    def test_shallow_repository_is_rejected(self):
        (self.root / ".git/shallow").write_text(self.base + "\n")
        self.assertEqual(self.check("--head", "HEAD")[0], 2)

    def test_renames_config_does_not_change_diff_parser(self):
        self.git("config", "diff.renames", "true")
        self.git("mv", "safe.txt", "renamed.txt")
        self.commit()
        self.assertEqual(self.check("--head", "HEAD", "--base", self.base)[0], 0)

    def test_same_fixture_blob_at_non_fixture_path_is_rejected(self):
        value = "/ho" + "me/ana"
        self.write("crates/sapphire_platform/src/dirs.rs", value)
        self.write("notes.txt", value)
        self.assertEqual(self.check("--staged")[0], 1)

    def test_new_branch_and_multiple_push_refs_scan_all_outgoing_history(self):
        self.write("notes.txt", PRIVATE)
        head = self.commit()
        stream = f"refs/heads/safe {self.base} refs/heads/safe {'0' * 40}\n"
        stream += f"refs/heads/private {head} refs/heads/private {'0' * 40}\n"
        with patch.object(sys, "stdin", io.StringIO(stream)):
            self.assertEqual(self.check("--pre-push")[0], 1)

    def test_push_with_missing_remote_object_scans_full_history(self):
        self.write("notes.txt", PRIVATE)
        head = self.commit()
        stream = f"refs/heads/main {head} refs/heads/main {'a' * 40}\n"
        with patch.object(sys, "stdin", io.StringIO(stream)):
            self.assertEqual(self.check("--pre-push")[0], 1)

    def test_tag_private_tagger_and_message_are_rejected(self):
        with patch.dict(os.environ, {"GIT_COMMITTER_EMAIL": PRIVATE}):
            self.git("tag", "-a", "test", "-m", "safe")
        tag = self.git("rev-parse", "test")
        with patch.object(sys, "stdin", io.StringIO(f"refs/tags/test {tag} refs/tags/test {'0' * 40}\n")):
            code, output = self.check("--pre-push")
        self.assertEqual(code, 1)
        self.assertIn("tag", output)
        self.assertNotIn(PRIVATE, output)

    def test_deletion_only_push_passes(self):
        with patch.object(sys, "stdin", io.StringIO(f"(delete) {'0' * 40} refs/heads/old {self.base}\n")):
            self.assertEqual(self.check("--pre-push")[0], 0)

    def test_private_email_in_ref_name_is_rejected(self):
        stream = f"refs/heads/{PRIVATE} {self.base} refs/heads/{PRIVATE} {'0' * 40}\n"
        with patch.object(sys, "stdin", io.StringIO(stream)):
            code, output = self.check("--pre-push")
        self.assertEqual(code, 1)
        self.assertNotIn(PRIVATE, output)

    def test_real_hooks_allow_safe_commit_and_local_push(self):
        self.install_files()
        self.write("notes.txt", "safe")
        self.commit()
        remote = Path(self.temp.name) / "remote.git"
        self.git("init", "--bare", "-q", str(remote))
        self.git("push", "-q", str(remote), "HEAD:refs/heads/main")
        self.assertEqual(self.git("--git-dir=" + str(remote), "rev-parse", "main"), self.git("rev-parse", "HEAD"))

    def test_real_precommit_hook_blocks_staged_private_path(self):
        self.install_files()
        self.write("notes.txt", HOME)
        p = subprocess.run(["git", "commit", "-m", "safe"], capture_output=True)
        self.assertNotEqual(p.returncode, 0)
        self.assertEqual(self.git("rev-parse", "HEAD"), self.base)
        self.assertNotIn(HOME.encode(), p.stderr)

    def test_real_commit_message_hook_blocks_private_trailer(self):
        self.install_files()
        self.write("notes.txt", "safe")
        p = subprocess.run(["git", "commit", "-m", "contact " + PRIVATE], capture_output=True)
        self.assertNotEqual(p.returncode, 0)
        self.assertEqual(self.git("rev-parse", "HEAD"), self.base)

    def test_real_pre_push_hook_blocks_publish_to_disposable_remote(self):
        self.write("notes.txt", TOKEN)
        head = self.commit()
        self.install_files()
        remote = Path(self.temp.name) / "remote.git"
        self.git("init", "--bare", "-q", str(remote))
        p = subprocess.run(["git", "push", str(remote), "HEAD:refs/heads/main"], capture_output=True)
        self.assertNotEqual(p.returncode, 0)
        self.assertNotIn(TOKEN.encode(), p.stderr)
        self.assertEqual(self.git("--git-dir=" + str(remote), "for-each-ref"), "")
        self.assertEqual(self.git("rev-parse", "HEAD"), head)

    def test_installer_preserves_existing_hooks_and_shared_worktrees(self):
        hook = self.root / ".git/hooks/pre-commit"
        hook.write_text("existing hook")
        with self.assertRaisesRegex(RuntimeError, "Existing local hooks"):
            install_privacy_hooks.install()
        self.assertEqual(hook.read_text(), "existing hook")
        hook.unlink()
        self.git("worktree", "add", "-q", "--detach", str(Path(self.temp.name) / "other"))
        with self.assertRaisesRegex(RuntimeError, "linked worktrees"):
            install_privacy_hooks.install()
        self.assertNotEqual(subprocess.run(["git", "config", "--get", "core.hooksPath"], capture_output=True).returncode, 0)

    def test_installer_preserves_existing_hooks_path(self):
        self.git("config", "core.hooksPath", "custom-hooks")
        with self.assertRaisesRegex(RuntimeError, "existing hooksPath"):
            install_privacy_hooks.install()
        self.assertEqual(self.git("config", "core.hooksPath"), "custom-hooks")


if __name__ == "__main__":
    unittest.main()
