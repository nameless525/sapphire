# Modifications

- `chinese_four_corner_pavilion.fbx` is `四角亭.fbx` renamed to ASCII, byte for byte the archived file (see the SHA-256 in [SOURCE.md](SOURCE.md)).
- The archive's UV-layout images are not committed.
- `chinese_four_corner_pavilion.fbx.meta` is Sapphire's import sidecar (asset ID and import settings), not part of the model. It says `cook: false`: X1 converts the FBX to a glTF piece of its own, so the FBX itself is never cooked or packed (M1 A10, spec §5.27).

## Derived files

None committed yet. M1 X1 (part 2) converts the FBX to glTF for the cooker and re-materials it in Sapphire's environment family, in the Jinzhou palette of the look target. It lists the derived files here, with how they were made.
