# AMD FidelityFX Super Resolution 1.0

A method, not a file: the renderer's edge-adaptive upscale and sharpen follow the two passes of FSR 1, EASU (edge-adaptive spatial upsampling) and RCAS (robust contrast-adaptive sharpening). No file of AMD's is in this folder or anywhere in the repository; nothing was downloaded. The WGSL that follows the method was written for Sapphire from its published description, and keeps the method's notice.

| | |
|---|---|
| **Author** | Advanced Micro Devices, Inc. (AMD FidelityFX) |
| **Source** | The FidelityFX Super Resolution 1.0 method, as AMD published it under the MIT license in 2021 |
| **Downloaded** | Never: written from the published description, M5 L14, 2026-11-14 |
| **File** | None |
| **SHA-256** | None: no file was taken |
| **License** | MIT, see [LICENSE](LICENSE), whose notice is also at the head of `crates/sapphire_render/shaders/post_upscale.wgsl` |
| **Credit** | Edge-adaptive upscale and sharpen after AMD FidelityFX Super Resolution 1.0. |

A build that ships the renderer ships this notice with it (the MIT license's one condition): the shader carries it in its source, which a build compiles in.
