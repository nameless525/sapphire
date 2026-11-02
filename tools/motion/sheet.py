"""Writes the moves, cooks them, and draws each named one as a contact sheet under `target/motion/`.

usage: sheet.py <move>[:<tick>,<tick>,...] ... [--views <yaw>,<yaw>] [--size <w>x<h>] [--zoom <factor>]
                [--travel] [--no-write]

It leaves root motion out unless `--travel` says, so a cut stays in the frame. Run it from anywhere.
"""
import os
import subprocess
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from rig import REPO

SANDBOX = os.path.join(REPO, 'target/debug/sapphire_sandbox')
OUT = os.path.join(REPO, 'target/motion')


def main():
    args = sys.argv[1:]
    views, size, zoom, travel, writing = '205,90', '160x200', None, False, True
    specs = []
    while args:
        arg = args.pop(0)
        if arg == '--views':
            views = args.pop(0)
        elif arg == '--size':
            size = args.pop(0)
        elif arg == '--zoom':
            zoom = args.pop(0)
        elif arg == '--travel':
            travel = True
        elif arg == '--no-write':
            writing = False
        else:
            specs.append(arg)
    if not specs:
        sys.exit(__doc__)
    if writing:
        subprocess.run([sys.executable, os.path.join(REPO, 'tools/motion/moves.py'), '--write'], check=True,
                       stdout=subprocess.DEVNULL)
    subprocess.run(['cargo', 'xtask', 'content'], cwd=REPO, check=True, capture_output=True)
    subprocess.run(['cargo', 'build', '-p', 'sapphire_sandbox', '--locked'], cwd=REPO, check=True, capture_output=True)
    os.makedirs(OUT, exist_ok=True)
    env = dict(os.environ, SAPPHIRE_CONTENT_DIR=os.path.join(REPO, 'target/content/desktop/'))
    for spec in specs:
        name, _, ticks = spec.partition(':')
        path = os.path.join(OUT, name + '.png')
        command = [SANDBOX, '--contact-sheet', name, path, '--sheet-views', views, '--size', size, '--blade', 'on',
                   '--springs', 'off']
        if ticks:
            command += ['--sheet-ticks', ticks]
        else:
            command += ['--sheet-cells', '12']
        if zoom:
            command += ['--zoom', zoom]
        if not travel:
            command.append('--clip-in-place')
        subprocess.run(command, cwd=REPO, env=env, check=True, capture_output=True)
        print(path)


if __name__ == '__main__':
    main()
