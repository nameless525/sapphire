# What Sapphire derived from FSR 1

Files written for Sapphire after the method, none copied:

- `crates/sapphire_render/shaders/post_upscale.wgsl` (M5 L14): the phone tiers' upscale (`fs_upscale_edge`, after EASU) and sharpen (`fs_upscale_sharpen`, after RCAS). It differs from the method in that the sharpen's result is held within its five pixels' own range, so it cannot overshoot an edge; RCAS's noise detection is left out; both read their input by texel, held inside the frame; and the sharpen's strength fades to nothing as the render scale nears 1 (`sapphire_render::post`'s `upscale` module).
- `crates/sapphire_render/shaders/post.wgsl`'s `sharpened` (M1 P2): the player's sharpening in the fused pass, after RCAS, on exposed light compressed by its luma, unclamped to its neighbours.
