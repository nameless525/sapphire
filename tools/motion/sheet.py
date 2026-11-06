"""Writes the key-pose documents, cooks them, and draws each named move as a contact sheet under `target/motion/`.

usage: sheet.py <move>[:<tick>,<tick>,...] ... [--views <yaw>,<yaw>] [--size <w>x<h>] [--zoom <factor>]
                [--travel] [--no-write]

It leaves root motion out unless `--travel` says, so a cut stays in the frame. Run it from anywhere.
"""
import argparse
import json
import os
import subprocess
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from rig import REPO

OUT = os.path.join(REPO, 'target/motion')


def checked(command, **kwargs):
    """Keep successful build/capture noise quiet, but retain subprocess diagnostics on failure."""
    try:
        return subprocess.run(command, check=True, capture_output=True, text=True, **kwargs)
    except subprocess.CalledProcessError as error:
        sys.stderr.write(error.stdout or '')
        sys.stderr.write(error.stderr or '')
        raise SystemExit(error.returncode) from error


def main(args=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('specs', nargs='+', metavar='<move>[:<ticks>]')
    parser.add_argument('--views', default='205,90')
    parser.add_argument('--size', default='160x200')
    parser.add_argument('--zoom')
    parser.add_argument('--travel', action='store_true')
    parser.add_argument('--no-write', action='store_false', dest='writing')
    args = parser.parse_args(args)
    if args.writing:
        subprocess.run([sys.executable, os.path.join(REPO, 'tools/motion/moves.py'), '--write'], check=True)
    checked(['cargo', 'xtask', 'content'], cwd=REPO)
    build = checked(['cargo', 'build', '-p', 'sapphire_sandbox', '--locked', '--message-format=json'], cwd=REPO)
    artifacts = [json.loads(line) for line in build.stdout.splitlines() if line.strip()]
    binaries = [artifact['executable'] for artifact in artifacts
                if artifact.get('reason') == 'compiler-artifact' and artifact.get('executable')
                and artifact['target']['name'] == 'sapphire_sandbox' and 'bin' in artifact['target']['kind']]
    if len(binaries) != 1:
        sys.exit('cargo did not report exactly one sapphire_sandbox executable')
    sandbox = binaries[0]
    os.makedirs(OUT, exist_ok=True)
    env = dict(os.environ, SAPPHIRE_CONTENT_DIR=os.path.join(REPO, 'target/content/desktop/'))
    for spec in args.specs:
        name, _, ticks = spec.partition(':')
        path = os.path.join(OUT, name + '.png')
        command = [sandbox, '--contact-sheet', name, path, '--sheet-views', args.views, '--size', args.size, '--blade', 'on',
                   '--springs', 'off']
        if ticks:
            command += ['--sheet-ticks', ticks]
        else:
            command += ['--sheet-cells', '12']
        if args.zoom:
            command += ['--zoom', args.zoom]
        if not args.travel:
            command.append('--clip-in-place')
        checked(command, cwd=REPO, env=env)
        print(path)


if __name__ == '__main__':
    main()
