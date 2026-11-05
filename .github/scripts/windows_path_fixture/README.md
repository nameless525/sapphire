# Windows output-path fixture

Run on native Windows from the repository root with Python 3.9+, the repository
Rust toolchain, and a local drive-letter temporary directory that supports
junctions. No GPU, cooked content, administrator symlink privilege, or third-party
Python package is needed.

```powershell
cargo build --locked --package sapphire_replay --package sapphire_testkit --bins
python -B .github/scripts/windows_path_fixture/run.py --inventory
python -B .github/scripts/windows_path_fixture/run.py --execute `
  --replay target/debug/sapphire_replay.exe --testkit target/debug/sapphire_testkit.exe
```

The portable inventory lists all 38 cases. Native execution checks actual CLI
writes, overlap and dangling-junction refusals, protected inputs, and golden
output pruning with a deterministic synthetic capture. It probes each verbatim
junction/parent spelling through native IO first, then runs the actual CLI. A
proved alias must get an overlap refusal. If native IO cannot read the spelling,
the CLI must still reject the output cleanly and leave the case tree unchanged.
The receipt records unavailable alias proof separately from the CLI result.

The runner prints the disposable fixture directory and leaves the full receipt
and command logs there. CI exports only a path-redacted JSON receipt with
bounded logs. It never uploads the fixture tree or follows its junctions.
