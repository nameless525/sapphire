#!/usr/bin/env python3
"""Set up Rust on disposable GitHub-hosted runners; never mutate a local toolchain.

The explicit override prevents later Cargo calls from installing every target in
rust-toolchain.toml again. rust-cache v2.9.3 hashes *all* installed toolchains, so
unused runner-image toolchains must also be removed before it calculates its key.
Python 3.11+ is preinstalled on the hosted images selected by ci.yml.
"""

import os
from pathlib import Path
import re
import subprocess
import tomllib


EXTRA_TARGETS = {
    "host": (),
    "ios": ("aarch64-apple-ios", "aarch64-apple-ios-sim"),
    "android": ("aarch64-linux-android",),
}


def setup(root: Path, env: dict[str, str]) -> None:
    # Check before reading the manifest or invoking rustup. This helper deliberately
    # uninstalls other toolchains; a developer or persistent self-hosted runner must
    # never take that path. The composite action supplies runner.environment.
    if (
        env.get("GITHUB_ACTIONS") != "true"
        or env.get("SAPPHIRE_RUNNER_ENVIRONMENT") != "github-hosted"
        or not env.get("GITHUB_ENV")
    ):
        raise RuntimeError("Rust setup is restricted to disposable GitHub-hosted CI runners")

    with (root / "rust-toolchain.toml").open("rb") as source:
        manifest = tomllib.load(source)["toolchain"]
    channel = manifest["channel"]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", channel):
        raise ValueError("CI requires an exact Rust version in rust-toolchain.toml")
    group = env.get("SAPPHIRE_RUST_GROUP", "host")
    if group not in EXTRA_TARGETS:
        raise ValueError(f"Unknown Rust target group: {group}")
    targets = EXTRA_TARGETS[group]
    if not set(targets).issubset(manifest.get("targets", [])):
        raise ValueError(f"The {group} targets must be declared in rust-toolchain.toml")
    expected_os = {"ios": "macOS", "android": "Linux"}.get(group)
    if expected_os and env.get("RUNNER_OS") != expected_os:
        raise ValueError(f"The {group} target group requires a {expected_os} runner")
    lint = env.get("SAPPHIRE_RUST_LINT", "false")
    if lint not in ("true", "false"):
        raise ValueError("SAPPHIRE_RUST_LINT must be true or false")

    rust_env = dict(env, RUSTUP_TOOLCHAIN=channel)
    command = ["rustup", "toolchain", "install", channel, "--profile", "minimal"]
    if lint == "true":
        for component in manifest.get("components", []):
            command.extend(["--component", component])
    for target in targets:
        command.extend(["--target", target])
    subprocess.run(command, env=rust_env, check=True)

    # Derive the canonical toolchain name from the compiler, not OS/CPU aliases.
    # If it cannot be identified exactly, fail before uninstalling anything.
    version = subprocess.check_output(
        ["rustup", "run", channel, "rustc", "-vV"], env=rust_env, text=True
    )
    hosts = re.findall(r"^host: (\S+)$", version, flags=re.MULTILINE)
    if len(hosts) != 1:
        raise RuntimeError("Cannot identify the pinned compiler's host")
    pinned = f"{channel}-{hosts[0]}"
    installed = subprocess.check_output(
        ["rustup", "toolchain", "list", "--quiet"], env=rust_env, text=True
    )
    toolchains = [line.split()[0] for line in installed.splitlines() if line.strip()]
    if pinned not in toolchains:
        raise RuntimeError(f"Pinned toolchain {pinned} is absent; refusing to remove toolchains")
    unused = [toolchain for toolchain in toolchains if toolchain != pinned]
    if unused:
        subprocess.run(
            ["rustup", "toolchain", "uninstall", *unused], env=rust_env, check=True
        )

    with Path(env["GITHUB_ENV"]).open("a", encoding="utf-8") as output:
        output.write(f"RUSTUP_TOOLCHAIN={channel}\n")
    print(f"Rust {channel}: host {hosts[0]}, additional targets {list(targets)}, lint {lint}")


if __name__ == "__main__":
    setup(Path.cwd(), dict(os.environ))
