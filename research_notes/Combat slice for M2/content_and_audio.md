# Content and audio for M2

Research for [the M2 plan](../../docs/m2-plan.md), read from public pages on 2026-11-01, with no asset downloaded. It asks which clips, enemy models and sounds a public MIT repository may hold, and whether FMOD can live in one. This is not legal advice: read each license again before adding a source.

**Bottom line.** Smallest licensed set: KayKit Character Animations (CC0) for clips, KayKit Skeletons (CC0) as the enemy, Kenney Impact Sounds and RPG Audio (CC0) for SFX, and `kira` for audio. Skip FMOD. Nothing I found licensed gives anime-grade action motion. To stay within my quoting limit I paraphrase terms, with the URL for each so you can check the wording.

**1. Animation**

| Source | Terms | Public repo | Commercial | Melee | Format and VRM fit |
|---|---|---|---|---|---|
| KayKit Character Animations (kaylousberg.itch.io/kaykit-character-animations) | CC0, no credit | Yes | Yes | 1H, 2H, unarmed, dual-wield, block; dodge, hit, death | FBX/glTF, `Rig_Medium`; needs retarget; chibi proportions |
| Mesh2Motion (github.com/scottpetrovic/mesh2motion-app) | README: art assets CC0 | Yes, with caveat below | Yes | 135 untagged human clips: sword, spear and staff combos, dodges, jabs, kicks, deaths, `Run_Anime` | GLB |
| pixiv VRMA pack (booth.pm/en/items/5512385) | Credit line required; bars distributing so it "can be rigged or extracted" | **No** | Yes, credited | No (greeting, peace sign, squat…) | .vrma, ideal fit |
| Mixamo | Adobe's FAQ gave me 403. A community thread says raw files can't be distributed. | No | Yes | Yes | FBX; Adobe login |
| Bandai Namco dataset (GitHub) | CC BY-NC 4.0 | n/a | **No** | Fighting | BVH |
| CMU MoCap (mocap.cs.cmu.edu) | Allowed in products, no direct resale "even in converted form"; no redistribution grant | Ambiguous | In products | Some, realistic | BVH/ASF; not recommended |
| 100STYLE (ianxmason.com/100style) | CC BY 4.0 | Yes, credited | Yes | **No**: locomotion and idle only, even for "Punch" | BVH, 28 bones |
| Kenney animated characters | CC0 | Yes | Yes | No (about 3 clips per pack, from a snippet) | Low value |
| Rokoko free library | 2020 press: commercial OK; redistribution unstated; needs Studio | Ambiguous | Yes | Some | Not recommended |
| LAFAN1 | CC BY-NC-ND | No | No | Fight | BVH |
| itch.io packs | maxparata is CC0 but 5 clips for voxel rigs. kevdev and MoCap Online state no license. | maxparata only | maxparata only | 1 sword attack | Not recommended |

Quaternius's Universal Animation Library 2 has the melee combos, but the project's content rules exclude it. Quaternius's license page (quaternius.com/license.html, updated 8/28/2026) bars repackaging or redistributing.

**Mesh2Motion caveat.** 87 of its 222 human clips carry a `quaternius` tag, including `Sword_Regular_*`, `Hit_*` and `Punch_*`. Its history has a 2026-10-21 commit "start adding some more quaternius animation stuff", after Quaternius's terms changed. Exclude all tagged clips. The untagged clips have no recorded authors, so they are CC0 only on the maintainers' word.

**2. Enemy**
- **KayKit Skeletons** (kaylousberg.itch.io/kaykit-skeletons): CC0, 4 free characters, rigged and built to work with the KayKit clips. It shares the clip set, but the flat 1024 gradient atlas needs anime re-materialing.
- **Mesh2Motion non-human rigs** (kaiju, spider, dragon): idle, attack, hit, death and walk clips, none tagged `quaternius`. CC0 by declaration only.
- **VRoid Studio CC0 samples:** a Gitee mirror says "the CC license for these models is CC0", and AvatarSample A to C carry conditions. pixiv's help page returned 403 and I couldn't confirm which models are CC0, so treat this as a lead only.
- **Seed-san:** it sits in `vrm-c/vrm-specification/samples/Seed-san`. The README says VPL 1.0, "model by VirtualCast, Inc.", and the file was last touched 2022-09-28. The credit and redistribution flags are inside the .vrm's metadata, and I couldn't read them without downloading, so "credit required" is unverified.

**3. Sound**
- **Kenney** (kenney.nl/support): CC0. Impact Sounds has 130 files, RPG Audio 50 (tags footstep, weapon) and Interface Sounds 100. Use these.
- **Sonniss GDC** (sonniss.com/gdc-bundle-license/): it bars distributing or supplying the effects, and bars sample packs and asset packs. A public repo is out.
- **Freesound:** licensing is per sound, and downloads need a login. CC0 sounds are usable one by one, but there's no bulk CI fetch.
- **OpenGameArt:** CC0 hit, punch and sword packs exist per search snippets, but OpenGameArt returned 403 to my fetcher, so I couldn't check them.

**4. FMOD (fmod.com/licensing, its EULA text)**
- **Tiers:**
  - Indie: budget under $600k; free if company revenue is under $200k, otherwise $2,000 per title.
  - Basic: $600k–$1.8M, $6,000.
  - Premium: over $1.8M, $18,000.
- **Credit and registration:** every tier needs an in-game credit line naming FMOD and Firelight Technologies. A splash logo is required unless waived, and only Basic and Premium can waive it. Commercial projects must be registered in your fmod.com profile.
- **Redistribution:** SDK files, headers included, may not be distributed except the runtime libraries. The EULA also says verbatim: "FMOD Engine is not distributed as part of a game engine or tool set". An open engine cannot bundle it.
- **CI:** the download needs an account. The Arch package scripts it with login credentials against `api-get-download-link`. I found no anonymous path.
- **Verdict:** not viable for a public repo and its CI.

**kira:** MIT OR Apache-2.0, 0.12.6, on cpal 0.18.1. Its README says "mainly meant for desktop platforms". cpal's README lists Android (AAudio, API 26) and iOS (CoreAudio). Kira issue #141: the maintainer says "I think so?", and users report iOS and Android working. That makes mobile community-confirmed, not documented. Outside `cargo-apk`, cpal needs the JavaVM and Activity set manually (`ndk_context::initialize_android_context`), which matters for the SDL entry point.

**What's missing.** No licensed source has anime-style action: exaggerated anticipation, dash-strikes, aerial strings or perfect-dodge. KayKit's clips are generic RPG swings. You also still need a glTF clip importer, since animation isn't imported yet, plus a retarget from the KayKit rig to VRM bones, and hit-frame events authored by hand.

**Fallback: author the signature moves.**
- **In code (proven for one pose):** `apps/sapphire_sandbox/src/lookdev.rs` `turns()` holds the character's pose as 7 `(HumanBone, Quat)` rotations, run through forward kinematics in `pose.rs`. Extending that to timed key tables (anticipation, strike, follow-through, recovery) with slerp is untested. Spring bones are cooked but not simulated, so hair and skirt stay rigid.
- **Headless Blender (infrastructure only):** `tools/blender/` already runs Blender 5.2 headless for mask bakes, FBX fixtures and the pavilion conversion. Nobody has authored armature animation or exported clips through it yet. Expect the repo's known exporter traps: nondeterministic FBX, absolute image paths.
- **Likely best:** key-pose tables as RON, diffable and deterministic, for the signature moves, with CC0 clips for locomotion only.

**Couldn't verify:**
- KayKit's exact clip names and whether `Rig_Medium` is humanoid-complete.
- Seed-san's metadata flags.
- Which VRoid samples are CC0.
- The OpenGameArt pack licenses.
- Adobe's official Mixamo text.
- Whether Mesh2Motion's untagged clips were all contributed under CC0.
