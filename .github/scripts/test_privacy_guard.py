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
AT = "@"
USERS = "/Us" + "ers/"
HOMES = "/ho" + "me/"
LOWER_USERS = "/us" + "ers/"
ENCODED_USERS = "-Us" + "ers-"
ENCODED_HOME = "-ho" + "me-"
CONTACT = "licensing" + AT + "author-mail.test"


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

    def test_dependabot_sign_off_is_public_but_a_person_signing_off_is_not(self):
        self.assertFalse(self.rules(b"Signed-off-by: dependabot[bot] <support@github.com>"))
        self.assertTrue(self.rules(b"Signed-off-by: A Person <" + PRIVATE.encode() + b">"))

    def test_private_filename_rules(self):
        for path in (b".env", b"src/.env.production", b".aws/credentials", b"signing/release.p12",
                     b"CLAUDE.md", b".claude/tools/run.sh", b".claude/settings.local.json"):
            self.assertTrue(pg.forbidden_path(path), path)
        for path in (b".env.example", b".env.sample", b"crates/sapphire_tod/src/key.rs"):
            self.assertFalse(pg.forbidden_path(path), path)

    def assert_all(self, rule, values, metadata=False):
        for value in values:
            with self.subTest(value=value):
                found = {r for r, _ in pg.content_findings(value.encode(), b"", metadata)}
                self.assertIn(rule, found)

    def assert_none(self, values, metadata=False):
        for value in values:
            with self.subTest(value=value):
                self.assertEqual(pg.content_findings(value.encode(), b"", metadata), [])

    # DV-3: Claude Code names its directories after a working directory, slashes as dashes.

    def test_encoded_home_paths_are_home_paths(self):
        leaks = ["/private/tmp/claude-501/" + ENCODED_USERS + "jane-work-engine/abc/x.md",
                 "~/.claude/projects/" + ENCODED_HOME + "jane-work-engine/memory",
                 "~/.claude/projects/" + ENCODED_USERS + "jane/memory",
                 ENCODED_USERS + "jane-work-engine",
                 "see " + ENCODED_HOME + "jane-work, then",
                 "C-" + ENCODED_USERS + "jane-work-engine",
                 "-private-tmp-claude-501-" + ENCODED_USERS + "jane-work-engine",
                 "-mnt-c" + ENCODED_USERS + "jane-work-engine",
                 "agent" + ENCODED_USERS + "jane-work-engine"]
        self.assert_all("personal-home-path", leaks)
        self.assert_all("personal-home-path", leaks, metadata=True)
        for value in leaks[:2]:
            self.assertIn("personal-home-path", self.rules(value.encode("utf-16")))
            self.assertIn("personal-home-path", self.rules(b"\xff\0" + value.encode("utf-16-le")))

    def test_ordinary_dashed_names_are_not_encoded_homes(self):
        lower_users = "-us" + "ers-"
        self.assert_none(["my" + ENCODED_HOME + "page-link", "docs/plan.md#how" + lower_users + "see-it",
                          "a" + ENCODED_HOME + "b-c", "-" + ENCODED_HOME + "dir-name",
                          "--foo" + ENCODED_HOME + "dir", "-" + lower_users + "file-path"])

    def test_generic_names_are_exempt_in_the_encoded_form_too(self):
        self.assert_none([ENCODED_HOME + "runner-work-sapphire", ENCODED_USERS + "runner-work-sapphire",
                          ENCODED_HOME + "user-project", ENCODED_USERS + "you-work"])
        self.assert_all("personal-home-path", [ENCODED_HOME + "userreal-work", ENCODED_USERS + "your-work"])

    # DV-5: ordinary content, each with the nearest real leak beside it.

    def test_git_remotes_are_ordinary_but_other_addresses_on_their_hosts_are_not(self):
        self.assert_none(["git" + AT + "github.com:nameless525/sapphire.git",
                          "ssh://git" + AT + "github.com/nameless525/sapphire.git"])
        self.assert_all("non-public-email", ["person" + AT + "github.com:nameless525/sapphire.git",
                                             "git.person" + AT + "github.com:nameless525/sapphire.git",
                                             "git" + AT + "personal.test",
                                             "git" + AT + "personal.test: see this",
                                             "git" + AT + "gmail.com:person/repo.git",
                                             "ssh://git" + AT + "mail.gmail.com/person/repo.git",
                                             "ssh://person" + AT + "github.com/nameless525/sapphire.git"])

    def test_scaled_asset_names_are_ordinary_but_address_shaped_names_are_not(self):
        names = ["AppIcon" + AT + "2x.png", "AppIcon-60" + AT + "3x.png", "hero" + AT + "2x.jpg"]
        self.assert_none(names)
        self.assert_none(names, metadata=True)
        self.assert_all("non-public-email", ["person" + AT + "2x.test", "person" + AT + "2xmail.com",
                                             "person" + AT + "2x.png.test", "person" + AT + "2x.com",
                                             PRIVATE + ".png"], metadata=True)

    def test_package_archives_are_ordinary_but_address_shaped_names_are_not(self):
        self.assert_none(["foo" + AT + "latest.tgz", "foo" + AT + "1.2.3.tgz"])
        self.assert_all("non-public-email", ["person" + AT + "latest.dev", PRIVATE + ".tgz",
                                             "person" + AT + "latest.tgz.personal.test"])

    def test_subdomains_of_example_domains_are_fixtures_but_look_alikes_are_not(self):
        self.assert_none(["a" + AT + "mail.example.com", "a" + AT + "example.org", "a" + AT + "x.y.example.net"])
        self.assert_all("non-public-email", ["a" + AT + "example.com.personal.test", "a" + AT + "notexample.com",
                                             "a" + AT + "my-example.org", "a" + AT + "example.community"])

    def test_site_paths_are_ordinary(self):
        self.assert_none(["GET /api" + LOWER_USERS + "42/profile", "api" + LOWER_USERS + "42/profile",
                          "https://example.org/api/v2" + LOWER_USERS + "42",
                          "https://github.com" + LOWER_USERS + "nameless525/projects/1",
                          "https://example.com" + HOMES + "index.html",
                          "https://example.com:8443" + HOMES + "index.html"])

    def test_paths_near_site_paths_are_still_homes(self):
        self.assert_all("personal-home-path", [
            USERS + "real-person/work", LOWER_USERS + "real-person/work", HOMES + "real-person/work",
            "/mnt/c" + USERS + "real-person/work", "/mnt/c" + LOWER_USERS + "real-person/work",
            "/c" + USERS + "real-person/work", "/cygdrive/c" + USERS + "real-person/work",
            "/System/Volumes/Data" + USERS + "real-person/work", "/var" + HOMES + "real-person/work",
            "backup" + USERS + "real-person/work", "backup" + HOMES + "real-person/work",
            "file://" + USERS + "real-person/work", "file://" + HOMES + "real-person/work",
            "file://localhost" + HOMES + "real-person/work", "http://localhost:8000" + HOMES + "real-person/work",
            "http://127.0.0.1" + HOMES + "real-person/work", "http://[::1]:8000" + HOMES + "real-person/work",
            "http://builder" + HOMES + "real-person/work", "http://builder.local" + HOMES + "real-person/work",
            "http://host.docker.internal" + HOMES + "real-person/work",
            "https://example.com" + USERS + "real-person/work",
            "https://example.com/docs" + HOMES + "real-person/work",
            "example.com" + HOMES + "real-person/work", "ssh://example.com" + HOMES + "real-person/work",
            "builder:" + HOMES + "real-person/repo.git", HOMES + "real-person/api" + LOWER_USERS + "42",
            "/myapi" + LOWER_USERS + "real-person", "/api2" + LOWER_USERS + "real-person",
            "/api" + USERS + "real-person"])

    def test_placeholder_homes_are_ordinary_but_names_near_them_are_not(self):
        self.assert_none([HOMES + "user/project", USERS + "you/work"])
        self.assert_all("personal-home-path", [HOMES + "userreal/x", HOMES + "user-real/x", USERS + "your/x",
                                               USERS + "you.real/x", HOMES + "users/x"])

    # DV-5's reviewed exemption: the address rule alone, at exact paths.

    NOTICES = (b"content/third_party/pack/LICENSE", b"content/third_party/pack/LICENSE.md",
               b"content/third_party/pack/License.txt", b"content/third_party/pack/LICENSE-MIT",
               b"content/third_party/pack/SOURCE.md", b"content/third_party/pack/sub/dir/LICENSE")

    def test_third_party_notices_may_hold_a_contact_address(self):
        for path in self.NOTICES:
            with self.subTest(path=path):
                self.assertEqual(pg.content_findings(CONTACT.encode(), path), [])
                self.assertEqual(pg.content_findings(("person" + AT + "gmail.com").encode(), path), [])

    def test_address_next_to_a_third_party_notice_is_still_rejected(self):
        for path in (b"content/third_party/pack/MODIFIED.md", b"content/third_party/pack/notes.txt",
                     b"content/third_party/pack/model.gltf", b"content/third_party/README.md",
                     b"content/third_party/LICENSE", b"content/third_party/SOURCE.md",
                     b"LICENSE", b"SOURCE.md", b"docs/LICENSE", b"content/LICENSE",
                     b"vendor/content/third_party/pack/LICENSE", b"content/third_party_more/pack/LICENSE",
                     b"Content/third_party/pack/LICENSE", b"content/third_party/pack/SOURCE.md.bak",
                     b"content/third_party/pack/source.md", b"content/third_party/pack/MY_LICENSE",
                     b"content/third_party/../secrets/LICENSE", b"content/third_party/pack/LICENSE/notes.txt"):
            with self.subTest(path=path):
                self.assertIn("non-public-email", self.rules(CONTACT.encode(), path))

    def test_every_other_rule_still_applies_to_third_party_notices(self):
        cases = [(HOME, "personal-home-path"), ("~/.claude/projects/" + ENCODED_USERS + "jane-work", "personal-home-path"),
                 (TOKEN, "access-token"), ("-----BEGIN " + "PGP PRIVATE KEY BLOCK-----", "private-key"),
                 ("https://" + "user:password" + AT + "host.test", "credential-url")]
        for path in self.NOTICES:
            for value, rule in cases:
                with self.subTest(path=path, rule=rule):
                    self.assertIn(rule, self.rules(CONTACT.encode() + b"\n" + value.encode(), path))
        self.assertTrue(pg.forbidden_path(b"content/third_party/pack/LICENSE.pem"))

    # DV-6: smaller gaps.

    def test_every_armored_private_key_is_caught(self):
        self.assert_all("private-key", ["-----BEGIN " + "PGP PRIVATE KEY BLOCK-----", "-----BEGIN " + "PRIVATE KEY-----",
                                        "-----BEGIN " + "RSA PRIVATE KEY-----", "-----BEGIN " + "ENCRYPTED PRIVATE KEY-----",
                                        "-----BEGIN " + "ANY PRIVATE KEY-----", "---- BEGIN " + "SSH2 ENCRYPTED PRIVATE KEY ----"])
        self.assert_none(["-----BEGIN " + "PGP PUBLIC KEY BLOCK-----", "-----BEGIN " + "PUBLIC KEY-----",
                          "-----BEGIN " + "CERTIFICATE-----", "-----BEGIN " + "PGP SIGNATURE-----"])

    def test_text_that_merely_fails_utf8_is_text(self):
        company = "jane" + AT + "corp-mail.dev\n"
        self.assertEqual(pg.content_findings(("caf\xe9 " + company).encode("latin-1")), [("non-public-email", 1)])
        self.assertEqual(pg.content_findings(("one\n" + company).encode("utf-16-le")), [("non-public-email", 2)])
        self.assertEqual(pg.content_findings(("one\n" + company).encode("utf-16-be")), [("non-public-email", 2)])

    def test_bytes_that_merely_decode_as_utf16_stay_raw(self):
        # Even length, a few NULs, no control characters when read as UTF-16: nonsense that hid this address.
        data = b"\0\x01" + ("jane" + AT + "gmail.comABC").encode() + b"\0"
        self.assertEqual(len(data) % 2, 0)
        self.assertEqual(pg.utf16_as_utf8(data), data)
        self.assertIn("non-public-email", self.rules(data))
        # Half ASCII units, half anything else: still text, so any address counts.
        mixed = ("漢字 " + "jane" + AT + "corp-mail.dev").encode("utf-16-le")
        self.assertIn("non-public-email", self.rules(mixed))

    def test_binary_payloads_still_check_personal_providers_only(self):
        self.assertFalse(self.rules(b"\x89PNG\0\x01a" + AT.encode() + b"b.cd\0\x02"))
        self.assertFalse(self.rules(b"\x01\0\x02\0a\0" + AT.encode() + b"\0b\0.\0c\0d\0"))

    def test_personal_address_in_binary_by_domain_suffix(self):
        for tail in ("gmail.comABC", "gmail.com\0", "mail.protonmail.com", "a.b.icloud.com.", "PROTON.ME"):
            with self.subTest(tail=tail):
                value = ("jane" + AT + tail).encode()
                self.assertIn("non-public-email", self.rules(b"\x00\x01" + value + b"\x00"))
        wide = ("jane" + AT + "mail.protonmail.com").encode("utf-16-le")
        self.assertIn("non-public-email", self.rules(b"\xff\0\x01\0\0\0" + wide))
        for tail in ("notgmail.com", "gmail.example.org", "proton.test"):
            with self.subTest(tail=tail):
                self.assertFalse(self.rules(b"\x00\x01" + ("jane" + AT + tail).encode() + b"\x00"))

    def test_private_key_and_signing_file_names(self):
        for path in (b"_netrc", b".git-credentials", b"secret.asc", b"foo.gpg", b"AuthKey_ABC.p8", b"id_ecdsa",
                     b"id_dsa", b"android/local.properties", b"android/key.properties"):
            self.assertTrue(pg.forbidden_path(path), path)
        for path in (b"notes.p8.txt", b"src/netrc.rs", b"local_properties.rs", b"gradle.properties", b"foo.gpg.md"):
            self.assertFalse(pg.forbidden_path(path), path)

    def test_github_public_profile_alias_is_identity_scoped(self):
        guard = pg.Guard()
        guard.identity("merge author", b"nameless <342860605+nameless525@users.noreply.github.com> 123 +0000")
        self.assertFalse(guard.findings)
        guard.identity("unrelated author", ("nameless <" + PUBLIC + "> 123 +0000").encode())
        self.assertTrue(guard.findings)


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

    # DV-4: every header and every tag object.

    def raw_object(self, kind, body, literally=False):
        flags = ["--literally"] if literally else []
        return self.git("hash-object", "-t", kind, "-w", "--stdin", *flags, input=body)

    def raw_commit(self, extra=b"", author=None, committer=None, message=b"crafted\n", literally=False):
        ident = ("Developer <" + PUBLIC + "> 1700000000 +0000").encode()
        body = b"tree " + self.git("rev-parse", "HEAD^{tree}").encode() + b"\nparent " + self.base.encode()
        body += b"\nauthor " + (author or ident) + b"\ncommitter " + (committer or ident) + b"\n" + extra + b"\n" + message
        return self.raw_object("commit", body, literally)

    def mergetag(self, tagger):
        return (b"mergetag object " + self.base.encode() + b"\n type commit\n tag v1\n tagger " + tagger
                + b" 1700000000 +0000\n \n release\n -----BEGIN PGP SIGNATURE-----\n abc\n -----END PGP SIGNATURE-----\n")

    def test_clean_crafted_commit_passes(self):
        self.assertEqual(self.check("--head", self.raw_commit(), "--base", self.base)[0], 0)

    def test_private_tagger_in_a_mergetag_header_is_rejected(self):
        commit = self.raw_commit(self.mergetag(b"Jane Doe <" + PRIVATE.encode() + b">"))
        code, output = self.check("--head", commit, "--base", self.base)
        self.assertEqual(code, 1)
        self.assertIn("mergetag", output)
        self.assertNotIn(PRIVATE, output)
        self.assertNotIn("Jane", output)

    def test_real_name_on_a_noreply_tagger_in_a_mergetag_header_is_rejected(self):
        commit = self.raw_commit(self.mergetag(b"Jane Doe <" + PUBLIC.encode() + b">"))
        code, output = self.check("--head", commit, "--base", self.base)
        self.assertEqual(code, 1)
        self.assertIn("identity-name-must-use-public-handle", output)
        clean = self.raw_commit(self.mergetag(b"developer <" + PUBLIC.encode() + b">"))
        self.assertEqual(self.check("--head", clean, "--base", self.base)[0], 0)

    def test_private_address_in_any_other_header_is_rejected(self):
        commit = self.raw_commit(b"x-note " + PRIVATE.encode() + b"\n")
        code, output = self.check("--head", commit, "--base", self.base)
        self.assertEqual(code, 1)
        self.assertIn("non-public-email", output)
        self.assertNotIn(PRIVATE, output)
        continued = self.raw_commit(b"x-note safe\n and " + HOME.encode() + b"\n")
        code, output = self.check("--head", continued, "--base", self.base)
        self.assertEqual(code, 1)
        self.assertIn("personal-home-path", output)

    def signed(self, armor_lines):
        return self.raw_commit(b"gpgsig -----BEGIN PGP SIGNATURE-----\n" + armor_lines + b" -----END PGP SIGNATURE-----\n")

    def test_armored_signature_body_is_skipped_and_nothing_else_in_it(self):
        # Base64's alphabet holds slashes; a chance path inside one must not fail a signed commit.
        body = b" \n " + HOMES.encode() + b"abcdefghij0123456789+=\n =AbCd\n"
        self.assertEqual(self.check("--head", self.signed(body), "--base", self.base)[0], 0)
        ssh = self.raw_commit(b"gpgsig-sha256 -----BEGIN SSH SIGNATURE-----\n " + USERS.encode() + b"abc=\n -----END SSH SIGNATURE-----\n")
        self.assertEqual(self.check("--head", ssh, "--base", self.base)[0], 0)
        for armor_header in (b" Comment: " + PRIVATE.encode() + b"\n \n abc=\n", b" Comment: " + HOME.encode() + b"\n \n abc=\n",
                             b" -----BEGIN PGP " + b"PRIVATE KEY BLOCK-----\n"):
            with self.subTest(armor_header=armor_header):
                self.assertEqual(self.check("--head", self.signed(armor_header), "--base", self.base)[0], 1)

    def test_every_author_line_is_an_identity(self):
        second = b"author Jane Doe <" + PUBLIC.encode() + b"> 1700000000 +0000\n"
        commit = self.raw_commit(second, literally=True)
        code, output = self.check("--head", commit, "--base", self.base)
        self.assertEqual(code, 1)
        self.assertIn("identity-name-must-use-public-handle", output)
        self.assertNotIn("Jane", output)

    def test_missing_identity_is_rejected(self):
        tree = self.git("rev-parse", "HEAD^{tree}").encode()
        ident = b"Developer <" + PUBLIC.encode() + b"> 1700000000 +0000\n"
        commit = self.raw_object("commit", b"tree " + tree + b"\ncommitter " + ident + b"\nmsg\n", literally=True)
        code, output = self.check("--head", commit)
        self.assertEqual(code, 1)
        self.assertIn("non-noreply-identity", output)

    def identity_result(self, name, address):
        ident = name + b" <" + address + b"> 1700000000 +0000"
        commit = self.raw_commit(author=ident, committer=ident)
        return self.check("--head", commit, "--base", self.base)

    def test_bot_addresses_take_only_their_exact_names(self):
        for name, address in ((b"GitHub", b"noreply@github.com"), (b"Claude", b"noreply@anthropic.com"),
                              (b"dependabot[bot]", b"support@github.com")):
            with self.subTest(name=name):
                self.assertEqual(self.identity_result(name, address)[0], 0)
        for name, address in ((b"Jane Doe", b"noreply@github.com"), (b"Jane Doe", b"noreply@anthropic.com"),
                              (b"Jane Doe", b"support@github.com"), (b"github", b"noreply@github.com"),
                              (b"GitHub Jane", b"noreply@github.com"), (b"Claude", b"noreply@github.com")):
            with self.subTest(name=name, address=address):
                code, output = self.identity_result(name, address)
                self.assertEqual(code, 1)
                self.assertIn("identity-name-must-use-public-handle", output)
                self.assertNotIn("Jane", output)

    def private_tag(self, name="test"):
        with patch.dict(os.environ, {"GIT_COMMITTER_EMAIL": PRIVATE}):
            self.git("tag", "-a", name, "-m", "safe")
        return self.git("rev-parse", name)

    def test_head_naming_an_annotated_tag_reads_the_tag(self):
        tag = self.private_tag()
        for head in ("test", "refs/tags/test", tag):
            with self.subTest(head=head):
                code, output = self.check("--head", head)
                self.assertEqual(code, 1)
                self.assertIn("tag", output)
                self.assertNotIn(PRIVATE, output)
                self.assertEqual(self.check("--head", head, "--base", self.base)[0], 1)

    def test_head_reads_a_tag_of_a_tag(self):
        inner = self.private_tag("inner")
        self.git("tag", "-a", "outer", "-m", "safe", inner)
        self.git("tag", "-d", "inner")
        self.assertEqual(self.check("--head", "outer")[0], 1)

    def test_head_reads_a_tags_message_and_whole_header(self):
        self.git("tag", "-a", "message", "-m", "contact " + PRIVATE)
        self.assertEqual(self.check("--head", "message")[0], 1)
        ident = ("developer <" + PUBLIC + "> 1700000000 +0000\n").encode()
        body = b"object " + self.base.encode() + b"\ntype commit\ntag v1\ntagger " + ident
        self.assertEqual(self.check("--head", self.raw_object("tag", body + b"\nmsg\n"))[0], 0)
        noted = self.raw_object("tag", body + b"x-note " + PRIVATE.encode() + b"\n\nmsg\n", literally=True)
        self.assertEqual(self.check("--head", noted)[0], 1)

    def test_clean_tags_pass_and_are_counted(self):
        self.git("tag", "-a", "clean", "-m", "Release")
        self.git("tag", "light")
        code, output = self.check("--head", "clean")
        self.assertEqual(code, 0)
        self.assertIn("1 tags", output)
        self.assertEqual(self.check("--head", "light")[0], 0)

    def test_head_naming_a_tag_with_a_private_name_is_rejected(self):
        self.git("tag", PRIVATE)
        code, output = self.check("--head", PRIVATE)
        self.assertEqual(code, 1)
        self.assertNotIn(PRIVATE, output)

    def test_ci_fetch_restores_an_annotated_tag_that_checkout_rewrote(self):
        self.private_tag()
        remote = Path(self.temp.name) / "remote.git"
        self.git("init", "--bare", "-q", str(remote))
        self.git("remote", "add", "origin", str(remote))
        self.git("push", "-q", "origin", "refs/tags/test")
        self.git("tag", "-f", "test", "HEAD")
        self.assertEqual(self.check("--head", "refs/tags/test")[0], 0)
        self.git("fetch", "--no-tags", "--force", "origin", "+refs/tags/test:refs/tags/test")
        self.assertEqual(self.check("--head", "refs/tags/test")[0], 1)

    def test_tag_of_a_blob_fails_closed(self):
        self.git("tag", "-a", "blob", "-m", "safe", self.git("rev-parse", "HEAD:safe.txt"))
        self.assertEqual(self.check("--head", "blob")[0], 2)

    # DV-5's exemption is bound to the path through the whole scan.

    def test_third_party_notice_passes_at_its_own_path(self):
        self.write("content/third_party/pack/LICENSE", CONTACT)
        self.assertEqual(self.check("--staged")[0], 0)
        self.commit()
        self.assertEqual(self.check("--head", "HEAD")[0], 0)

    def test_same_notice_blob_at_another_path_is_rejected_either_way(self):
        for notes in ("a-notes.txt", "z-notes.txt"):
            with self.subTest(notes=notes):
                self.write("content/third_party/pack/LICENSE", CONTACT)
                self.write(notes, CONTACT)
                code, output = self.check("--staged")
                self.assertEqual(code, 1)
                self.assertNotIn(CONTACT, output)
                self.git("rm", "-qf", "--", notes)

    def test_notice_blob_copied_elsewhere_and_removed_again_is_rejected(self):
        # The final tree holds only the notice, so its clean scan mustn't excuse the copy's commit.
        self.write("content/third_party/pack/SOURCE.md", CONTACT)
        self.commit()
        self.write("docs/credits.md", CONTACT)
        self.commit()
        self.git("rm", "-q", "docs/credits.md")
        self.commit()
        self.assertEqual(self.check("--head", "HEAD")[0], 1)

    def test_private_address_in_a_third_party_file_name_is_rejected(self):
        self.write("content/third_party/pack/LICENSE-" + PRIVATE, "safe")
        code, output = self.check("--staged")
        self.assertEqual(code, 1)
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


class WorkflowTests(unittest.TestCase):
    def test_ci_checks_pushed_tags_and_the_tag_itself(self):
        workflow = (ROOT / ".github/workflows/privacy.yml").read_text()
        self.assertIn("tags: ['**']", workflow)
        self.assertIn('--head "$GITHUB_REF"', workflow)
        # Checkout rewrites an annotated tag to its commit, so the real tag must be fetched back.
        self.assertIn('git fetch --no-tags --force origin "+$GITHUB_REF:$GITHUB_REF"', workflow)


if __name__ == "__main__":
    unittest.main()
