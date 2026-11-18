# Spike A2: an FBX's curves in deterministic Rust

What M6 A2 had to know before an FBX could feed a move's tables: whether ufbx's raw keys can be reached through the `ufbx` 0.11.5 binding, and whether evaluating them in the engine's own arithmetic reproduces ufbx's evaluation within the kill line (0.25 mm on any channel, or the turn that moves a bone's end that far). Read from ufbx 0.23's C source as cargo holds it and measured by `import::fbx::curves::tests::every_channel_evaluates_within_the_kill_line_of_ufbx`, on 2026-11-17 on an Apple M5 (aarch64), a debug test build, with other agents building. Nothing here is timed.

## Are the raw keys reachable?

Yes. `AnimCurve` gives each key's time, value, interpolation and tangents (`Keyframe::left`, `right`, `dx` and `dy` in `f32`), and each end's extrapolation with its repeat count; `AnimLayer` its weight, blend mode, accumulation and animated properties by element and name; `AnimValue` its curve node's defaults; `Node` everything the load's conversion folded into it (`adjust_*`, `use_rotation_space`, scale helpers). ufbx's `Real` is `f64`.

## Where ufbx's evaluation stops being the same bits

- `ufbx_euler_to_quat` calls the platform's `sin` and `cos`; slerps and Euler round trips in layer blending call `acos`, `asin`, `atan2` and `pow`. These differ between Apple's libm, glibc and the MSVC runtime in the last bit.
- ufbx is C built by `cc` with the compiler's defaults, and clang contracts `a * b + c` into a fused multiply-add on aarch64, where x86_64's baseline has none. Every quaternion product and Bézier step is such an expression.
- What the binding hands over is safe: key times are one division, linear, constant and user tangents one product each, and the conversion's quaternions come from axis permutations, whose arithmetic is exact. **Auto and TCB tangents are not quite:** ufbx derives them as it parses, in a few multiply-adds rounded to `f32`, so a fused one can move a tangent by one `f32` bit when the `f64` result sits within a rounding of an `f32` boundary. Nothing in the binding gives the file's own tangent data back (that would need ufbx's retained DOM).

## The measurement

The evaluation in `import::fbx::curves` against `ufbx::evaluate_transform` and `ufbx::evaluate_blend_weight`, at 30 and 60 ticks a second over each stack's range and half a second either side of it, for every node, and each node's rest against ufbx's `local_transform`. A translation is measured in meters through its parent's scale at rest; a rotation and a scale at the end of a 2 m bone, longer than any here. Fixtures:

- `animated_clips.fbx` (Blender 5.2, `tools/blender/fbx_fixtures.py`): 2,538 keys, all linear, two stacks, seven nodes;
- hand-written ASCII files: one node with constant, constant-from-next, linear and cubic keys (auto, auto clamped, user, broken, TCB) and every extrapolation with and without a repeat count; a Z-up centimeter file with pivots, offsets, pre- and post-rotations and each of the seven rotation orders, and a node whose rotation space is off; two stacks of layers (additive and blended, composing and by component, an animated weight, an override); a blend channel with a cubic `DeformPercent`.

| What | Worst difference | Ticks where the `f32` differs |
|---|---|---|
| `animated_clips.fbx`, every channel and rest | 1.6e-12 mm | 2 of 182 on the stride (one on the hips' translation, one on the arm's) |
| every interpolation, tangent and extrapolation | 1.0e-11 mm | none of 542 |
| pivots, offsets, rotations, every rotation order | 2.9e-12 mm | none |
| layers of every mode | 5.4e-12 mm | none |
| a blend channel's weight | 1.7e-13 mm | none |
| scale helpers (`InheritType` 0 and 2 under an animated non-uniform scale, a compensated uniform one) | 5.3e-12 mm | none |
| every node's world rest, geometry matrix and skin cluster's mesh-to-bone matrix (the fixtures, the pavilion, a geometric transform, scale helpers) | 8.3e-13 mm | 70 of 220 differ from ufbx's in a last `f64` bit |

The kill line holds by about ten orders of magnitude. The two ticks are the point of the package: ufbx's C on this Mac and plain IEEE arithmetic disagree in the last bit of an `f64` often enough for a clip's `f32` to land on another value now and then, which is why an FBX couldn't bake before.

Building the hand-written files found one trap: from FBX 7.2 on an ASCII file writes `KeyAttrDataFloat` as the floats' bits, as integers. Written as decimals, ufbx reads them as bits and gets NaN tangents.

## What was built from it

`import::fbx::curves`, evaluating clips and rests, and the mesh importer's world rests and geometric transforms; the refusal of FBX in a move's bake is gone. Spec §5.13's "As built (M6 A2)" has what it does and can't.

## Unverified

- **Other architectures:** this ran on aarch64 only. CI's macOS, Linux and Windows jobs hold the pinned table, clip and cooked set from the Blender fixture, and the hand-written files' clips, which carry the auto and TCB tangents; that's where a parse-time difference would show.
- **Files from other tools:** no Maya, MotionBuilder or 3ds Max export was read (no downloads in agent work). Their layers, scale helpers and connections are written here from ufbx's code and ASCII by hand, not from real files.
- **Connected properties** (constraints, expressions) aren't evaluated; a clip that has one is refused by name.
