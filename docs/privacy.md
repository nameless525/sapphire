# Keeping private data out of Sapphire

Sapphire is public. Use your public GitHub handle and GitHub-provided noreply email
for both commit author and committer. Keep personal paths, contact details,
credentials, signing material and local agent configuration out of tracked files,
commit messages, PR descriptions, screenshots and uploaded logs.

## Before committing or pushing

The offline guard needs Git and Python 3.9 or newer, with no downloaded dependencies.
From the repository root:

```sh
python3 .github/scripts/privacy_guard.py --staged
python3 .github/scripts/privacy_guard.py --head HEAD --base origin/main
```

The first command reads the **index**, so an unstaged cleanup cannot conceal a
private staged version. It scans staged additions/modifications, avoiding unchanged
assets on each commit, and checks the effective author and committer identity.
The second checks the full proposed tree plus identities, messages and changed
blobs in **every new commit**, including content subsequently removed or reverted.
Omit `--base` to scan all reachable history. Shallow clones fail closed; fetch the
full history before checking. Untracked and ignored files are not read.

To enable the included pre-commit, commit-msg and pre-push hooks in a standalone
clone:

```sh
python3 .github/scripts/install_privacy_hooks.py
```

The installer changes only that clone's local `core.hooksPath`. It refuses to
replace existing local hooks, an existing custom hooksPath, or to configure a
repository with linked worktrees: those settings can affect other active work.
Use an independent clone when another agent is working. Integrate with any
existing hooks deliberately; do not overwrite another tool's hooks. No installation
or Git identity change runs automatically on checkout, build or test.

The pre-push hook checks all updates supplied by Git, including new branches,
annotated tag identities/messages, and intermediate outgoing commits. When the
remote's advertised old object is unavailable locally, it scans full local
ancestry instead of trusting stale remote-tracking refs. This can take longer on
a first push. Deletion-only updates need no content scan.

Findings print a rule, Git object/commit ID and text line where available. They
never print a matching value, filename or source snippet. Inspect that object
locally with Git; do not paste its contents into public CI, PRs or issues.

## What is checked

- Author and committer use GitHub noreply addresses and public handles matching
  those addresses; GitHub and Anthropic's noreply bot addresses are accepted.
- Text files, commit messages and filenames reject non-public emails, except
  explicit example domains and `.invalid` fixtures. Binary email scanning is
  limited to common personal-email providers to avoid accidental byte matches.
- Personal Unix/macOS/Windows home paths, recognizable private keys, common
  GitHub/AWS/Google/OpenAI/Slack tokens and credential-bearing URLs are rejected.
  Raw binary and UTF-16 strings are also inspected; this is not an archive decoder.
- Known private files are rejected even if force-added: environment files (apart
  from example/sample files), credential directories, private agent configuration
  and signing/key files. `.gitignore` additionally prevents ordinary accidental adds.
- Existing cache-directory tests have an exception for their exact fixture user
  only in their exact source path. GitHub-hosted runner and generic OS home names
  are accepted. A duplicate blob at another path does not inherit the exception.
- Submodules fail closed because the guard cannot inspect their contents here.

Use synthetic example values in tests. If a legitimate public attribution or
fixture needs a new exception, review a narrow, path-specific rule with a
regression test. Do not add an actual private address to an allowlist.

## GitHub safeguards

Enable **Keep my email addresses private** and **Block command line pushes that
expose my email** in GitHub's [email settings](https://github.com/settings/emails).
GitHub's latter check covers the most recent commit's author email; it does not
replace checking every outgoing author, committer and message locally. Keep the
repository's secret scanning and secret push protection enabled too.

The independent **Privacy guard** CI job runs without Rust compilation. On PRs it
checks both the actual head and GitHub's generated merge commit. On main pushes it
checks the published range. Manual runs scan the entire reachable history. The
job has read-only repository permissions and no persisted checkout credential.
Its tests require Python 3.11+, as do the existing CI helper tests.

CI runs **after upload**, so it cannot prevent the initial disclosure. Hooks are
opt-in and can be bypassed, and branch protection only applies if separately
configured by the repository owner. GitHub's native push protection catches
supported secrets before publication; it does not detect arbitrary personal data.

## Limits and removal

These checks reduce accidental leaks; they cannot identify every real name,
telephone number or address, OCR screenshots, recursively decode all compressed
assets, or recognize every credential format. They do not audit GitHub PR bodies,
issue comments, Actions artifacts or account settings. Review screenshots and logs
before attaching them. Local analysis packages may contain machine-specific paths.

If something escapes, stop propagating it and handle the evidence privately.
Rotate an exposed credential. Removing a file or reverting a commit does not erase
its history. A rewritten branch can still leave GitHub PR refs and cached commit
views accessible. Follow [GitHub's removal guidance](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/removing-sensitive-data-from-a-repository)
and coordinate any rewrite with everyone using the repository. Never rewrite an
active agent's history or delete its checkout as part of an automatic cleanup.

Validation:

```sh
python3 -m unittest discover -s .github/scripts -p 'test_privacy*.py' -v
```

Tests create disposable repositories and local bare remotes. They never contact
GitHub or use real personal information or credentials.
