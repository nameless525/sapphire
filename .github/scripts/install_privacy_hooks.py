"""Opt in this standalone clone; never replace existing hooks or affect other worktrees."""

from pathlib import Path
import subprocess
import sys


def install():
    def git(*args):
        return subprocess.check_output(["git", *args], stderr=subprocess.DEVNULL).decode().strip()

    root = Path(git("rev-parse", "--show-toplevel"))
    # core.hooksPath is shared by worktrees unless worktreeConfig is configured.
    # Do not enable that extension or change its semantics in an active repository.
    if sum(line.startswith("worktree ") for line in git("worktree", "list", "--porcelain").splitlines()) != 1:
        raise RuntimeError("Refusing a repository with linked worktrees; install in an independent clone")
    configured = subprocess.run(["git", "config", "--get", "core.hooksPath"],
                                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    if configured.returncode not in (0, 1):
        raise RuntimeError("Unable to read hook configuration")
    if configured.returncode == 0:
        if configured.stdout.strip() == b".githooks":
            print("Privacy hook path already configured; no settings changed.")
            return
        raise RuntimeError("An existing hooksPath is configured; integrate manually without replacing it")
    git_dir = Path(git("rev-parse", "--absolute-git-dir"))
    if any(p.is_file() and not p.name.endswith(".sample") for p in (git_dir / "hooks").glob("*")):
        raise RuntimeError("Existing local hooks found; integrate manually without replacing them")
    for name in ("pre-commit", "commit-msg", "pre-push"):
        path = root / ".githooks" / name
        if not path.is_file():
            raise RuntimeError("Missing privacy hook file")
        path.chmod(path.stat().st_mode | 0o111)
    subprocess.run(["git", "config", "--local", "core.hooksPath", ".githooks"], check=True)
    print("Privacy hooks enabled in this standalone clone only; identity settings unchanged.")


if __name__ == "__main__":
    try:
        install()
    except (RuntimeError, OSError, subprocess.CalledProcessError) as error:
        # RuntimeError messages above are static and contain no local paths.
        print(str(error) if isinstance(error, RuntimeError) else "Hook installation failed; settings may need local inspection.", file=sys.stderr)
        sys.exit(1)
