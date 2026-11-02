"""Offline, redacting Git privacy checks. No third-party packages or working-tree reads.

Hooks inspect the index and outgoing objects; CI inspects the complete proposed tree,
every new commit (including content subsequently reverted in the same push) and tags.
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


def oid(ref, kind="commit"):
    # Resolve before passing user/event-controlled revisions to traversal commands.
    return git("rev-parse", "--verify", "--end-of-options", ref + "^{" + kind + "}").strip().decode()


EMAIL = re.compile(rb"[A-Za-z0-9_.+%~-]{1,64}@[A-Za-z0-9.-]{1,190}\.[A-Za-z]{2,24}")
HOME = re.compile(rb"(?:/Users/|/home/|%2FUsers%2F|%2Fhome%2F|[A-Za-z]:[\\/]+(?:Users|Documents and Settings)[\\/]+"
                  rb"|\\\\[^\\/\s]+\\[^\\/\s]+\\(?:Users|home)\\)([A-Za-z0-9_.@\x80-\xff-]+)", re.I)
# Claude Code names its directories after a working directory, with slashes and dots as dashes.
ENCODED_HOME = re.compile(rb"-(?:Users|home)-([A-Za-z0-9_.]+)", re.I)
# Such a name opens with the root's own dash or a drive's `C--`. `my-home-page` and `--home-dir` don't.
ENCODED_OPENING = re.compile(rb"(?:\A|[^A-Za-z0-9_.-])(?:-(?!-)[A-Za-z0-9_.-]+)?\Z|[A-Za-z0-9]-\Z")
NOREPLY = re.compile(rb"(?:[0-9]+\+)?[A-Za-z0-9-]+(?:\[bot\])?@users\.noreply\.github\.com", re.I)
SECRET_PATTERNS = (
    ("private-key", re.compile(rb"-{4,5} ?BEGIN (?:[A-Z0-9]+ ){0,4}PRIVATE KEY(?: BLOCK)? ?-{4,5}")),
    ("access-token", re.compile(rb"\b(?:gh[pousr]_[A-Za-z0-9]{30,255}|github_pat_[A-Za-z0-9_]{40,255}|"
                                 rb"(?:AKIA|ASIA)[A-Z0-9]{16}|AIza[A-Za-z0-9_-]{35}|"
                                 rb"sk-(?:proj-)?[A-Za-z0-9_-]{32,255}|xox[baprs]-[A-Za-z0-9-]{15,255})\b")),
    ("credential-url", re.compile(rb"\b[a-z][a-z0-9+.-]{1,20}://[^\s/:@\"']{1,80}:[^\s/@\"']{1,100}@", re.I)),
)
PERSONAL_DOMAINS = {b"gmail.com", b"icloud.com", b"outlook.com", b"hotmail.com", b"yahoo.com",
                    b"protonmail.com", b"proton.me", b"me.com", b"live.com"}
# In binary, any address on these providers or their subdomains, whatever bytes follow:
# string tables glue the next string straight onto the address.
PERSONAL_EMAIL = re.compile(rb"[A-Za-z0-9_.+%~-]@(?:[A-Za-z0-9-]{1,63}\.){0,8}(?:"
                            + b"|".join(re.escape(domain) for domain in sorted(PERSONAL_DOMAINS)) + rb")", re.I)
EXAMPLE_DOMAINS = {b"example.com", b"example.org", b"example.net"}
# Address-shaped file names: only these exact shapes, so a file named after a real address still fails.
SCALED_ASSET = re.compile(rb"[1-9][0-9]?x\.(?:png|jpe?g|gif|webp|svg|pdf)")
PACKAGE_ARCHIVE = re.compile(rb"(?:latest|next|[0-9]+(?:\.[0-9]+)*)\.tgz")
# Public service addresses and the one name each may carry on a commit or tag.
# support@github.com is what Dependabot signs its commits off with.
BOT_NAMES = {b"noreply@github.com": b"GitHub", b"noreply@anthropic.com": b"Claude",
             b"support@github.com": b"dependabot[bot]"}
# Verified public GitHub profile alias used on GitHub's synthetic PR merge commits.
# Scope this to the exact public identity; do not allow arbitrary real-name headers.
PUBLIC_NAME_ALIASES = {b"342860605+nameless525@users.noreply.github.com": {b"nameless"}}
# Runner and OS account names, and documentation placeholders. Exact names only.
GENERIC_HOMES = {b"...", b"runner", b"runneradmin", b"public", b"shared", b"default", b"user", b"you"}
# A site's own /home/ and /users/ pages. Local-looking hosts (no dot, an address, these
# suffixes) stay out: a dev server's URL can carry a real home path.
PUBLIC_URL_ROOT = re.compile(rb"(?:\A|[^A-Za-z0-9+.-])https?://(?:[A-Za-z0-9-]{1,63}\.)+([A-Za-z]{2,24})"
                             rb"(?::[0-9]{1,5})?\Z", re.I)
LOCAL_SUFFIXES = {b"local", b"localhost", b"internal", b"lan", b"home", b"corp", b"test"}
API_ROOT = re.compile(rb"(?:\A|[^A-Za-z0-9_.-])api(?:/v[0-9]{1,3})?\Z")
SIGNATURE_FIELDS = {b"gpgsig", b"gpgsig-sha256"}
ARMORED_BODY = re.compile(rb"[A-Za-z0-9+/=]*")
CONTROL_CHARS = re.compile("[\0-\x08\x0e-\x1f\x7f]")


def public_email(address):
    return bool(NOREPLY.fullmatch(address)) or address.lower() in BOT_NAMES


def under(domain, domains):
    return any(domain == parent or domain.endswith(b"." + parent) for parent in domains)


def example_email(address):
    domain = address.lower().rsplit(b"@", 1)[-1]
    return domain.endswith(b".invalid") or under(domain, EXAMPLE_DOMAINS)


def git_remote(buf, match):
    """`git@host:path` or `ssh://git@host`, except on a personal provider's domain."""
    local, _, domain = match.group().lower().rpartition(b"@")
    return local == b"git" and not under(domain, PERSONAL_DOMAINS) and bool(
        re.match(rb":[A-Za-z0-9_.~/-]", buf[match.end():match.end() + 2])
        or buf[max(0, match.start() - 6):match.start()] == b"ssh://")


def ordinary_address(buf, match):
    address = match.group()
    domain = address.lower().rsplit(b"@", 1)[-1]
    return (public_email(address) or example_email(address) or git_remote(buf, match)
            or bool(SCALED_ASSET.fullmatch(domain) or PACKAGE_ARCHIVE.fullmatch(domain)))


def site_path(buf, match):
    """A lowercase /home/ or /users/ right after a public URL's host or an API root."""
    if not match.group().startswith((b"/home/", b"/users/")):
        return False
    before = buf[max(0, match.start() - 280):match.start()]
    host = PUBLIC_URL_ROOT.search(before)
    return bool(API_ROOT.search(before)) or bool(host and host.group(1).lower() not in LOCAL_SUFFIXES)


def personal_home(buf, match, path):
    user = match.group(1).lower()
    fixture = path == b"crates/sapphire_platform/src/dirs.rs" and user == b"ana"
    return not fixture and user not in GENERIC_HOMES and not site_path(buf, match)


def encoded_home(buf, match):
    return match.group(1).lower() not in GENERIC_HOMES and bool(
        match.group().startswith(b"-Users-")
        or ENCODED_OPENING.search(buf[max(0, match.start() - 256):match.start()]))


def third_party_notice(path):
    """A third-party source's licence or SOURCE.md, in its own folder; see docs/privacy.md."""
    parts = path.split(b"/")
    return (len(parts) >= 4 and parts[:2] == [b"content", b"third_party"]
            and not {b"", b".", b".."} & set(parts)
            and (parts[-1] == b"SOURCE.md" or parts[-1].upper().startswith(b"LICENSE")))


def forbidden_path(path):
    parts = path.lower().replace(b"\\", b"/").split(b"/")
    name = parts[-1]
    return (
        any(p in {b".ssh", b".aws", b".gnupg"} for p in parts)
        or name in {b".ds_store", b".netrc", b"_netrc", b".git-credentials", b"id_rsa", b"id_ed25519",
                    b"id_ecdsa", b"id_dsa", b"credentials", b"local.properties", b"key.properties", b".npmrc",
                    b".pypirc", b"google-services.json", b"googleservice-info.plist"}
        or name == b".env" or (name.startswith(b".env.") and name not in {b".env.example", b".env.sample"})
        or name.endswith((b".pem", b".key", b".p8", b".p12", b".pfx", b".asc", b".gpg", b".jks", b".keystore",
                          b".mobileprovision", b".provisionprofile", b".cer"))
        or path.lower() == b"claude.md"
        or path.lower().startswith((b".claude/tools/", b".claude/hooks/", b".claude/worktrees/"))
        or path.lower() == b".claude/settings.local.json"
    )


def utf16_as_utf8(data):
    """UTF-16 text as UTF-8, BOM or not; anything else unchanged."""
    if data.startswith((b"\xff\xfe", b"\xfe\xff")):
        try:
            return data.decode("utf-16").encode("utf-8")
        except UnicodeError:
            return data
    # Without a BOM, only text that's at least half ASCII shows itself: zero high bytes on one
    # side and no control characters. Most other bytes decode too, into nonsense that hides
    # what's in them, so they stay raw.
    if b"\0" in data and not len(data) % 2:
        little, big = data[1::2].count(0), data[0::2].count(0)
        if max(little, big) * 4 >= len(data):
            try:
                decoded = data.decode("utf-16-le" if little >= big else "utf-16-be")
            except UnicodeError:
                return data
            if not CONTROL_CHARS.search(decoded):
                return decoded.encode("utf-8")
    return data


def content_findings(data, path=b"", metadata=False):
    """Return rule/line only. Never return matching values or source snippets."""
    data = utf16_as_utf8(data)
    # One byte that isn't UTF-8 doesn't make a file binary (Latin-1 text, say); a NUL does.
    text = b"\0" not in data
    # Raw binary paths/keys can be UTF-16 without a BOM (e.g. debug strings).
    variants = [data]
    if not text:
        variants.append(data.replace(b"\0", b""))
    notice = third_party_notice(path)
    found = set()

    def add(rule, buf, match):
        found.add((rule, buf.count(b"\n", 0, match.start()) + 1 if text else 0))

    for buf in variants:
        for rule, pattern in SECRET_PATTERNS:
            for match in pattern.finditer(buf):
                add(rule, buf, match)
        for match in HOME.finditer(buf):
            if personal_home(buf, match, path):
                add("personal-home-path", buf, match)
        for match in ENCODED_HOME.finditer(buf):
            if encoded_home(buf, match):
                add("personal-home-path", buf, match)
        if notice:
            continue
        if text or metadata:
            for match in EMAIL.finditer(buf):
                if not ordinary_address(buf, match):
                    add("non-public-email", buf, match)
        else:
            # Binary payloads produce random email-shaped sequences; restrict these to
            # common personal-email providers. Text/commit messages check all domains.
            for match in PERSONAL_EMAIL.finditer(buf):
                add("non-public-email", buf, match)
    return sorted(found)


def header_fields(header):
    """A commit or tag header as (name, lines); continuation lines start with a space."""
    fields = []
    for line in header.split(b"\n"):
        if line.startswith(b" ") and fields:
            fields[-1][1].append(line[1:])
        else:
            name, _, value = line.partition(b" ")
            fields.append((name, [value]))
    return fields


class Guard:
    def __init__(self):
        self.findings = set()
        self.scanned = set()
        self.commits = set()
        self.tags = set()

    def add_content(self, label, data, path=b"", metadata=False):
        for rule, line in content_findings(data, path, metadata):
            self.findings.add((label, rule, line))

    def identity(self, label, value):
        value = value.strip()
        match = re.search(rb"<([^<>]+)> [0-9]+ [+-][0-9]{4}$", value)
        if not match or not public_email(match.group(1)):
            self.findings.add((label, "non-noreply-identity", 0))
        else:
            address, name = match.group(1), value[:match.start()].strip()
            if NOREPLY.fullmatch(address):
                handle = address.split(b"@", 1)[0].split(b"+", 1)[-1]
                allowed = name.lower() in {handle.lower()} | PUBLIC_NAME_ALIASES.get(address.lower(), set())
            else:
                allowed = name == BOT_NAMES[address.lower()]
            if not allowed:
                self.findings.add((label, "identity-name-must-use-public-handle", 0))
        self.add_content(label, value, metadata=True)

    def header(self, label, header, identities):
        """Check a header's identities by rule and everything else as content. Only an
        armored signature's base64 goes unread, so its armor headers are checked too."""
        missing = set(identities)
        rest = []
        for name, lines in header_fields(header):
            if name in (b"author", b"committer", b"tagger"):
                missing.discard(name)
                self.identity(label + " " + name.decode(), b"\n".join(lines))
            elif name == b"mergetag":
                self.tag_text(label + " mergetag", b"\n".join(lines))
            elif name in SIGNATURE_FIELDS:
                rest += [line for line in lines if not ARMORED_BODY.fullmatch(line)]
            else:
                rest += [name, *lines]
        for name in sorted(missing):
            self.identity(label + " " + name.decode(), b"")
        self.add_content(label + " header", b"\n".join(rest), metadata=True)

    def tag_text(self, label, raw):
        header, _, message = raw.partition(b"\n\n")
        self.header(label, header, {b"tagger"})
        self.add_content(label + " message", message, metadata=True)

    def read_tags(self, revision):
        """Tags have their own tagger and message. Reads each one `revision` names, down to its target."""
        target = oid(revision, "object")
        while git("cat-file", "-t", target).strip() == b"tag":
            self.tags.add(target)
            raw = git("cat-file", "tag", target)
            self.tag_text("tag " + target[:12], raw)
            found = re.match(rb"object ([0-9a-f]{40,64})\n", raw)
            if not found:
                raise GuardError("Unexpected Git tag object")
            target = found.group(1).decode()
        return target

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
            # A blob used at multiple paths must not inherit a path's exemption.
            key = (object_id, path == b"crates/sapphire_platform/src/dirs.rs", third_party_notice(path))
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
        head = oid(self.read_tags(head))
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
            self.header(label, header, {b"author", b"committer"})
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
            try:
                base = oid(remote) if set(remote) != {"0"} else None
            except GuardError:
                base = None
            self.history(local, base)

    def finish(self):
        if self.findings:
            print("Privacy check failed (values and filenames redacted):", file=sys.stderr)
            for label, rule, line in sorted(self.findings):
                suffix = " line " + str(line) if line else ""
                print(f"  {label}{suffix}: {rule}", file=sys.stderr)
            print("Inspect the named object/commit locally; do not paste its contents into public logs."
                  " See docs/privacy.md.", file=sys.stderr)
            return 1
        print(f"Privacy check passed: {len(self.scanned)} blob contexts, {len(self.commits)} commits,"
              f" {len(self.tags)} tags.")
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
            # CI passes a pushed tag's ref here, and a tag's name is published with it.
            head = args.head or "HEAD"
            guard.add_content("head ref name", head.encode(), metadata=True)
            guard.history(head, args.base)
        return guard.finish()
    except (GuardError, OSError, ValueError):
        print("Privacy check could not complete; refusing to proceed. Check Git refs, index and Python locally."
              " Error details withheld to avoid exposing paths or data.", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
