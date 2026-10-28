"""Offline, redacting Git privacy checks. No third-party packages or working-tree reads.

Hooks inspect the index and outgoing objects; CI inspects the complete proposed tree
and every new commit (including content subsequently reverted in the same push).
"""

import argparse
from pathlib import Path
import re
import subprocess
import sys


class GuardError(Exception):
    """A check could not complete. Do not include raw Git output in this exception."""


def git(*args, input=None):
    result = subprocess.run(["git", *args], input=input, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE)
    if result.returncode:
        raise GuardError("Git inspection failed; check refs/index locally (details redacted)")
    return result.stdout


def oid(ref):
    # Resolve before passing user/event-controlled revisions to traversal commands.
    return git("rev-parse", "--verify", "--end-of-options", ref + "^{commit}").strip().decode()


EMAIL = re.compile(rb"[A-Za-z0-9_.+%~-]{1,64}@[A-Za-z0-9.-]{1,190}\.[A-Za-z]{2,24}")
HOME = re.compile(rb"(?:/Users/|/home/|[A-Za-z]:[\\/]+Users[\\/]+)([A-Za-z0-9_.@\x80-\xff-]+)", re.I)
NOREPLY = re.compile(rb"(?:[0-9]+\+)?[A-Za-z0-9-]+(?:\[bot\])?@users\.noreply\.github\.com", re.I)
SECRET_PATTERNS = (
    ("private-key", re.compile(rb"-----BEGIN (?:RSA |EC |OPENSSH |DSA |ENCRYPTED )?PRIVATE KEY-----")),
    ("access-token", re.compile(rb"\b(?:gh[pousr]_[A-Za-z0-9]{30,255}|github_pat_[A-Za-z0-9_]{40,255}|"
                                 rb"(?:AKIA|ASIA)[A-Z0-9]{16}|AIza[A-Za-z0-9_-]{35}|"
                                 rb"sk-(?:proj-)?[A-Za-z0-9_-]{32,255}|xox[baprs]-[A-Za-z0-9-]{15,255})\b")),
    ("credential-url", re.compile(rb"\b[a-z][a-z0-9+.-]{1,20}://[^\s/:@\"']{1,80}:[^\s/@\"']{1,100}@", re.I)),
)
PERSONAL_DOMAINS = {b"gmail.com", b"icloud.com", b"outlook.com", b"hotmail.com", b"yahoo.com",
                    b"protonmail.com", b"proton.me", b"me.com", b"live.com"}
EXAMPLE_DOMAINS = {b"example.com", b"example.org", b"example.net"}
BOT_EMAILS = {b"noreply@github.com", b"noreply@anthropic.com"}
# Verified public GitHub profile alias used on GitHub's synthetic PR merge commits.
# Scope this to the exact public identity; do not allow arbitrary real-name headers.
PUBLIC_NAME_ALIASES = {b"342860605+nameless525@users.noreply.github.com": {b"nameless"}}


def public_email(address):
    return bool(NOREPLY.fullmatch(address)) or address.lower() in BOT_EMAILS


def example_email(address):
    domain = address.lower().rsplit(b"@", 1)[-1]
    return domain.endswith(b".invalid") or domain in EXAMPLE_DOMAINS


def forbidden_path(path):
    parts = path.lower().replace(b"\\", b"/").split(b"/")
    name = parts[-1]
    return (
        any(p in {b".ssh", b".aws", b".gnupg"} for p in parts)
        or name in {b".ds_store", b".netrc", b"id_rsa", b"id_ed25519", b"credentials"}
        or name == b".env" or (name.startswith(b".env.") and name not in {b".env.example", b".env.sample"})
        or name.endswith((b".pem", b".key", b".p12", b".pfx", b".jks", b".keystore", b".mobileprovision"))
        or path.lower() == b"claude.md"
        or path.lower().startswith((b".claude/tools/", b".claude/hooks/", b".claude/worktrees/"))
        or path.lower() == b".claude/settings.local.json"
    )


def content_findings(data, path=b"", metadata=False):
    """Return rule/line only. Never return matching values or source snippets."""
    if data.startswith((b"\xff\xfe", b"\xfe\xff")):
        try:
            data = data.decode("utf-16").encode("utf-8")
        except UnicodeError:
            pass
    try:
        data.decode("utf-8")
        text = b"\0" not in data
    except UnicodeError:
        text = False
    # Raw binary paths/keys can be UTF-16 without a BOM (e.g. debug strings).
    variants = [data]
    if not text and b"\0" in data:
        variants.append(data.replace(b"\0", b""))
    found = set()
    for buf in variants:
        for rule, pattern in SECRET_PATTERNS:
            for match in pattern.finditer(buf):
                found.add((rule, buf.count(b"\n", 0, match.start()) + 1 if text else 0))
        for match in HOME.finditer(buf):
            user = match.group(1).lower()
            fixture = path == b"crates/sapphire_platform/src/dirs.rs" and user == b"ana"
            if not fixture and user not in {b"...", b"runner", b"runneradmin", b"public", b"shared", b"default"}:
                found.add(("personal-home-path", buf.count(b"\n", 0, match.start()) + 1 if text else 0))
        # Binary payloads produce random email-shaped sequences; restrict these to
        # common personal-email providers. Text/commit messages check all domains.
        for match in EMAIL.finditer(buf):
            address = match.group()
            if not (text or metadata or address.lower().rsplit(b"@", 1)[-1] in PERSONAL_DOMAINS):
                continue
            if not public_email(address) and not example_email(address):
                found.add(("non-public-email", buf.count(b"\n", 0, match.start()) + 1 if text else 0))
    return sorted(found)


class Guard:
    def __init__(self):
        self.findings = set()
        self.scanned = set()
        self.commits = set()

    def add_content(self, label, data, path=b"", metadata=False):
        for rule, line in content_findings(data, path, metadata):
            self.findings.add((label, rule, line))

    def identity(self, label, value):
        match = re.search(rb"<([^<>]+)> [0-9]+ [+-][0-9]{4}$", value.strip())
        if not match or not public_email(match.group(1)):
            self.findings.add((label, "non-noreply-identity", 0))
        elif NOREPLY.fullmatch(match.group(1)):
            handle = match.group(1).split(b"@", 1)[0].split(b"+", 1)[-1]
            names = {handle.lower()} | PUBLIC_NAME_ALIASES.get(match.group(1).lower(), set())
            if value[:match.start()].strip().lower() not in names:
                self.findings.add((label, "identity-name-must-use-public-handle", 0))
        self.add_content(label, value, metadata=True)

    def blobs(self, entries):
        # Index/tree filenames are scanned too, but never echoed into public logs.
        pending = {}
        for mode, object_id, path in entries:
            label = "object " + object_id[:12]
            if forbidden_path(path):
                self.findings.add((label, "private-file", 0))
            self.add_content(label + " filename", path, metadata=True)
            if mode == b"160000":
                self.findings.add((label, "unscanned-submodule", 0))
                continue
            # A blob used at multiple paths must not inherit a fixture exemption.
            key = (object_id, path == b"crates/sapphire_platform/src/dirs.rs")
            if key not in self.scanned:
                pending.setdefault(object_id, []).append((path, key))
        if not pending:
            return
        proc = subprocess.Popen(["git", "cat-file", "--batch"], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        try:
            for object_id, uses in pending.items():
                proc.stdin.write(object_id.encode() + b"\n")
                proc.stdin.flush()
                header = proc.stdout.readline().split()
                if len(header) != 3 or header[1] != b"blob":
                    raise GuardError("Missing Git blob; privacy check is incomplete")
                length = int(header[2])
                data = proc.stdout.read(length)
                if len(data) != length or proc.stdout.read(1) != b"\n":
                    raise GuardError("Incomplete Git object read")
                for path, key in uses:
                    self.add_content("object " + object_id[:12], data, path)
                    self.scanned.add(key)
        finally:
            proc.stdin.close()
            proc.stdout.close()
            if proc.poll() is None:
                proc.terminate()
            proc.wait()

    def staged(self):
        if git("ls-files", "--unmerged", "-z"):
            raise GuardError("Unmerged index; resolve conflicts before privacy validation")
        # Only additions/modifications can introduce new private data. Avoid reading
        # unchanged textures and golden images on every inner-loop commit.
        changed = git("diff", "--cached", "--raw", "--no-abbrev", "--no-renames", "-z", "--")
        self.blobs(diff_entries(changed))
        for key in ("GIT_AUTHOR_IDENT", "GIT_COMMITTER_IDENT"):
            self.identity("pending " + key.lower(), git("var", key))

    def history(self, head, base=None):
        if git("rev-parse", "--is-shallow-repository").strip() != b"false":
            raise GuardError("Full history is required for privacy validation")
        head = oid(head)
        exclusions = [] if not base or set(base) == {"0"} else ["^" + oid(base)]
        commits = git("rev-list", head, *exclusions, "--").decode().splitlines()
        # Scan the entire final tree even when only metadata changed.
        self.blobs(tree_entries(head))
        for commit in commits:
            if commit in self.commits:
                continue
            self.commits.add(commit)
            raw = git("cat-file", "commit", commit)
            header, _, message = raw.partition(b"\n\n")
            label = "commit " + commit[:12]
            for field in (b"author ", b"committer "):
                value = next((line[len(field):] for line in header.splitlines() if line.startswith(field)), b"")
                self.identity(label + " " + field.decode().strip(), value)
            self.add_content(label + " message", message, metadata=True)
            # diff against every parent catches merge-resolution content and
            # transient secrets removed before the range's final tree.
            changed = git("diff-tree", "--root", "-m", "-r", "--no-renames", "--no-commit-id",
                          "--raw", "--no-abbrev", "-z", commit, "--")
            self.blobs(diff_entries(changed))

    def pre_push(self, stream):
        # Deliberately do not trust remote-tracking refs. The remote's advertised
        # old OID bounds each update; absent objects cause a safe full traversal.
        for line in stream:
            fields = line.split()
            if len(fields) != 4:
                raise GuardError("Invalid pre-push input")
            local_ref, local, remote_ref, remote = fields
            if not re.fullmatch(r"[0-9a-f]{40,64}", local) or not re.fullmatch(r"[0-9a-f]{40,64}", remote):
                raise GuardError("Invalid pre-push object IDs")
            if set(local) == {"0"}:
                continue
            self.add_content("outgoing ref name", (local_ref + "\n" + remote_ref).encode(), metadata=True)
            # Annotated tags have their own tagger/message, independent of commits.
            target = local
            while git("cat-file", "-t", target).strip() == b"tag":
                raw = git("cat-file", "tag", target)
                header, _, message = raw.partition(b"\n\n")
                tagger = next((x[7:] for x in header.splitlines() if x.startswith(b"tagger ")), b"")
                self.identity("tag " + target[:12], tagger)
                self.add_content("tag " + target[:12] + " message", message, metadata=True)
                target = header.splitlines()[0].split()[1].decode()
            try:
                base = oid(remote) if set(remote) != {"0"} else None
            except GuardError:
                base = None
            self.history(target, base)

    def finish(self):
        if self.findings:
            print("Privacy check failed (values and filenames redacted):", file=sys.stderr)
            for label, rule, line in sorted(self.findings):
                suffix = " line " + str(line) if line else ""
                print(f"  {label}{suffix}: {rule}", file=sys.stderr)
            print("Inspect the named object/commit locally; do not paste its contents into public logs."
                  " See docs/privacy.md.", file=sys.stderr)
            return 1
        print(f"Privacy check passed: {len(self.scanned)} blob contexts, {len(self.commits)} commits.")
        return 0


def tree_entries(revision):
    for record in git("ls-tree", "-r", "-z", revision, "--").split(b"\0"):
        if record:
            meta, path = record.split(b"\t", 1)
            mode, _, object_id = meta.split()
            yield mode, object_id.decode(), path


def diff_entries(changed):
    records = changed.split(b"\0")
    if len(records) % 2 != 1 or records[-1] != b"":
        raise GuardError("Unexpected Git diff records")
    for i in range(0, len(records) - 1, 2):
        fields = records[i].split()
        if len(fields) != 5 or fields[4] not in (b"A", b"M", b"T", b"D"):
            raise GuardError("Unexpected Git tree diff")
        if fields[4] != b"D":
            yield fields[1], fields[3].decode(), records[i + 1]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--staged", action="store_true")
    mode.add_argument("--commit-msg", type=Path)
    mode.add_argument("--pre-push", action="store_true")
    mode.add_argument("--head", default=None)
    parser.add_argument("--base")
    args = parser.parse_args(argv)
    if args.base and not args.head:
        parser.error("--base requires --head")
    try:
        guard = Guard()
        if args.staged:
            guard.staged()
        elif args.commit_msg:
            guard.add_content("pending commit message", args.commit_msg.read_bytes(), metadata=True)
        elif args.pre_push:
            guard.pre_push(sys.stdin)
        else:
            guard.history(args.head or "HEAD", args.base)
        return guard.finish()
    except (GuardError, OSError, ValueError):
        print("Privacy check could not complete; refusing to proceed. Check Git refs, index and Python locally."
              " Error details withheld to avoid exposing paths or data.", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
