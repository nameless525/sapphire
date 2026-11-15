# What a patch against a 1 GiB release costs (M5 M7)

M7's spike, run on 2026-11-15. Unless a line says otherwise: an Apple M5 (24 GB, internal SSD), macOS 27.0, the `tools` profile for the pipeline, the patcher and the game (`game_app --features dev`), a loopback host (`cargo xtask patch serve` on 127.0.0.1), and the machine shared with other agents' builds. The load average (1 minute) is given with the runs: it was 12 to 20 throughout. Each figure is one run once warm (the game's Metal pipelines built and the release's pages cached), and two runs of the same thing differed by up to 3× in install time (§3), so a quiet window's repeat is owed (§9). "ours:" marks our own inference.

**Verdict: the download scales, the install doesn't.** What a patch fetches is what changed plus the changed packs' tables, as M4 designed it. Everything after the download scales with the size of every pack the patch touches. The install writes each touched pack again whole, from kept and fetched blocks, and hashes it twice. The player then holds the shipped pack and the patched copy. On the benchmark's one zone pack of 1.04 GiB, a 5.6 MB patch (a texture and a document) held the boot screen 5.4 s and cost 1.13 GB of disk for as long as that app build stays installed. **Before real content ships, the packer must cap a pack's size:** parts of 64 to 128 MiB, a texture kept with its levels. Then a patch's install time, its free-space check, the disk it holds and the tables it fetches scale with the parts it touches, not with the zone.

| Patch | Fetched | The player waits | The player holds besides the app |
|---|---|---|---|
| Small (one texture, one document) | 5.6 MB in 11 requests, 0.49% of the release | the boot screen 5.4 to 5.6 s: the download 0.1 s, the install 5.0 to 5.2 s | 1.13 GB, the whole district pack again; 1.14 GB at the peak |
| Medium (10% of each pack) | 119.6 MB in 374 requests, 10.3% | the boot screen 6.7 to 20.0 s: the download 1.8 to 3.5 s, the install 4.4 to 15.9 s | 1.17 GB; 1.30 GB at the peak |
| Large (60%, assets growing, shrinking, appearing and going) | 763.9 MB in 1,375 requests, 65.3% | the boot screen 23.9 s: the download 8.3 to 10.7 s, the install 10.8 to 13.5 s, clearing the staging 1.7 to 2.1 s | 1.18 GB; 2.00 GB at the peak |

## 1. The release and the three patches

**Release 1** is the desktop cook of `643d5ea66` (`cargo xtask content --cook desktop`: 12 packs and 5 files), with B4's generated budget district (`cargo xtask bench --cook` from `bench/`) in place of the cook's own small district. The district pack is 1,118,033,253 bytes: 2,666 assets in 44,842 blocks of 64 KiB, and a table of 2,441,552 bytes. It holds:

- 1,920 texture levels: 2,550 MiB raw, 1,037 MiB stored (zstd at 19 gets them to 0.41);
- 480 textures' tails: 10.2 MiB raw, 7.1 MiB stored;
- 104 meshes: 89.5 MiB raw, 18.9 MiB stored;
- 160 materials, a zone and an encounter.

1,376 of its blocks are stored raw. The whole release is 1,158,099,820 bytes. It is signed with the test key and published as a host lays it out (`sapphire_pipeline release`). The "shipped" app is a copy of it with the signed manifest.

**Successors** come from a throwaway tool (`m7_successor`, §9). It edits release 1's packs instead of cooking again, because a new cook of the generated district would change every block. It changes the chosen assets' bytes and writes each pack again through `sapphire_pipeline::pack::write_pack`. Then the pipeline's own `release --patch-from` signs release 2 and counts what an install of release 1 fetches. How an asset changes depends on its pack:

- **The district:** a changed asset has one byte flipped in each 64 KiB of its raw bytes and is compressed again at zstd 19. Every one of its blocks changes, as a re-authored texture's would.
- **The title packs** (shared, fonts, ui, loc, audio) and, in the large patch, the zones the boot loads: an asset is only compressed again at level 3.

A first large run flipped bytes in the boot's zones as well. That broke a probe grid's type tag and dropped an asset the lantern ward needs, so the boot refused the release. Those zones are now only compressed again. In the three patches, by the pipeline's own count:

| Patch | What changed | Bytes to fetch | Of the release | Tables in it | Packs touched |
|---|---|---|---|---|---|
| Small | one district texture with most of its levels (5 assets: its tail and 4 levels, 90 blocks), and `documents/ui/menus.ron` | 5,622,210 | 0.49% | 2,441,808 (43% of the patch) | 1 |
| Medium | 10% of each pack's stored bytes, assets picked at random (the district: 255 assets, 4,426 blocks) | 119,578,354 | 10.32% | 2,686,880 | 9 |
| Large | 60% of each pack's stored bytes (the district: 1,552 assets, 29,143 blocks; 194 grow by half, 194 shrink by half, 164 appear, 149 go) | 763,662,570 | 65.33% | 2,694,064 | 7 |

Making them took 5.3 s, 39.3 s and 154 s (the large one compresses about 730 MB at zstd 19 on four threads).

## 2. How it was run

One script (`rehearse.py`, §9) runs each patch under one gate slot, in order:

1. **The patcher alone** (`m7_phases`, an example over `sapphire_patch`'s public API), once to warm up and once measured. It opens an `Installation` over the shipped copy, reads `installed()`, then runs a `Download` from the host, timed by its progress (checking, reading the changed tables and planning, fetching). Then it runs `install` and reads back and hashes every installed pack. It calls `confirm_launch`, and checks again as a second launch would. Disk is the allocated blocks under the patch root, sampled every 100 ms. Memory is the process's peak resident size.
2. **The game:** `game_app --capture` over a shipped copy of release 1, with `SAPPHIRE_PATCH_HOST` set to the host. It checks, downloads and installs in its boot (`game_flow::update`, M4 P4), then launches a second time. The marks are the game's own log lines, timed from the process's start. `update_ms`, the boot's worst frame and its worst interval come from its `booted` line.
3. **A kill at half the download:** the game is killed once the host has sent half the patch's bytes, then launched again.
4. **A kill during the install:** the game is killed partway through its install (half the install time step 2 took; 0.35 of it for the large patch), then launched again.

The host's log gives what each launch fetched, request by request.

## 3. What the player waits for

The patcher alone (step 1) and the game's boot (step 2), from the same runs. The "second run" columns are a repeat after §7's fix, with no kills, at loads 13.5 to 15.7:

| | Small | Medium | Large |
|---|---|---|---|
| Load | 13.4 | 15.2 | 17.7 |
| Download, the patcher alone | 40 ms (checking 13, tables and plan 8, fetching 20) | 1,559 ms (24, 87, 1,449) | 6,649 ms (12, 138, 6,500) |
| Requests: all, blocks | 12, 7 | 375, 354 | 1,376, 1,359 |
| Install, the patcher alone | 3,481 ms (second run 3,398) | 13,871 ms (4,940) | 9,771 ms (13,372) |
| Reading back every installed pack | 664 ms (1,212) | 1,129 ms (1,284) | 1,311 ms (891) |
| Staged | 5.8 MB in 94 files | 131 MB in 4,952 files | 832 MB in 30,392 files |
| Patcher's peak memory (the whole process) | 28.7 MB | 29.5 MB | 37.1 MB |
| **The game** (load) | 12.7 | 14.7 | 14.9 |
| The boot screen is up at | 0.33 s | 0.71 s | 0.58 s |
| Download, then install, in the boot | 0.04 s, then 5.0 s (second run 0.08, 5.2) | 3.45 s, then 15.9 s (1.83, 4.4) | 10.7 s, then 10.8 s (8.3, 13.5) |
| `update_ms`: the boot held on the update | 5,449 (5,649) | 19,987 (6,723) | 23,863 (23,878) |
| Worst boot frame, worst interval, intervals over 33.3 ms | 39.7, 39.7 ms, 1 (100.5, 100.5, 6) | 44.1, 44.1 ms, 3 (38.4, 38.4, 1) | 82.2, 82.2 ms, 1 (57.1, 57.1, 2) |
| The next launch: `update_ms`, fetched | 126 ms, the manifest (1,101 bytes) | 122 ms, the manifest | 120 ms, the manifest |

What the table shows:

- **The download is what changed.** On loopback it costs the patcher about 8 ms a MB. Fetching all of a changed pack's table costs 2.4 MB even for the small patch.
- **The install is the touched packs' size.** The small patch's 90 changed blocks cost an install of 3.4 to 7.6 s across seven runs: 44,752 blocks are copied from the shipped pack and the 1.12 GB pack is hashed twice. One install varied 3× between runs here: medium took 13.9 s in one run and 4.9 s in another. ours: the machine's load, which a quiet repeat would settle. Even the fastest, 3.4 s for 5.6 MB, is 30 times the download.
- **The game doesn't stall while it waits.** The install runs on its own thread under the boot screen and its bar. The worst boot frames (38 to 100 ms) are the stand being built after the remake, as on a boot with no patch; one large run's worst was in the update step. ours: frames over 33 ms at this load aren't a finding without a quiet repeat.
- **Clearing the staging is a cost of its own:** 1.7 to 2.1 s for the large patch's 30,392 files after the release was installed (the install logs "release installed" before it clears, and the game continues once it's done), and 0.2 to 0.4 s for the medium patch's.

## 4. Kills

| | Small | Medium | Large |
|---|---|---|---|
| **Killed at half the download:** sent before the kill | 3.51 MB | 60.2 MB | 382.8 MB |
| Staged when killed | 2.96 MB, 18 files | 65.1 MB, 2,304 files | 415.2 MB, 14,830 files |
| The next launch fetched | 2.71 MB | 59.7 MB | 381.9 MB |
| Fetched over the patch, both launches | +0.60 MB (10.7%) | +0.28 MB (0.24%) | +0.86 MB (0.11%) |
| The next launch's `update_ms` | 6,535 | 10,167 | 23,911 |
| **Killed during the install:** when | 4.9 s in, about 4.5 s into the install | 9.76 s in, 0.17 s after the switch | 12.74 s in, about 1.6 s into the install |
| Written when killed | 903 MB of the new pack | the whole release; 2,991 staged files not yet cleared | 1.03 GB of the new packs |
| The next launch, before the boot screen | the install again, 4.9 s, before any content is read; the boot screen at 5.9 s | nothing: release 2 is current; the boot screen at 0.81 s | the install again, 8.2 s, then clearing the staging, 2.5 s; the boot screen at 11.5 s |

**A kill in the download costs little.** Each staged block is kept, and the extra is what the host sent that the killed process never staged. On the small patch that extra is mostly the district's table in flight.

**A kill in the install costs the next launch its first frame.** A release staged by an earlier launch is installed in the app's `init` (`Patches::open`), synchronously, before the boot screen. The player sees no picture for the install's whole length: a black window here, a frozen launch screen on a phone. The medium patch's kill landed after the switch, because that run's install took 7.8 s against the first run's 15.9 s. So it shows the other side: a kill after the manifest's rename leaves a launch that is simply up to date.

## 5. Phone storage

ours, from the figures above and M4's layout (`docs/engine-spec.md` §6.4):

- **A player holds a changed pack twice, whatever the patch's size.** The app's shipped content can't change (an iOS app bundle, an APK's assets). So the patched pack is a second copy in the player's directory, and it stays until an app update ships the new release in the bundle. On the benchmark that is 1.12 to 1.18 GB, for patches of 5.6 MB to 764 MB.
- **The free space an install asks for is every touched pack whole,** plus the 64 MiB reserve: 1,118,044,133 bytes for the small patch, 1,155,304,372 for the medium one and 1,165,957,542 for the large one (the install's `needed=`).
- **The peak adds the staging:** 2.0 GB for the large patch, since the staged blocks stay until the new packs are whole.
- **A second patch before the next confirmed launch** holds three copies for a moment: the shipped pack, the first patch's and the second's, about 3.4 GB on the benchmark. The confirmed launch then removes the first patch's copy. This is reasoned from `confirm_launch`, not run.

At the reference games' scale (installs of 20 to 40 GB, in zones and characters), a patch that touches one texture in each zone would ask for the whole content again in free space, on the phones that have least of it. Those games download their content after the app is installed, so their content lives in the player's directory and a patched pack can replace it. Sapphire's M4 ships the cook inside the app. ours: capped packs bound the cost either way of shipping. Moving content out of the bundle is a decision for the milestone that ships real content.

## 6. What doesn't scale

In order of what it costs the player:

1. **A changed block rewrites its whole pack.** The install assembles every block of each changed pack from kept and fetched blocks, hashing each as it's written, and flushes the pack. Then `verify_pack` reads it back whole and hashes every block again. Install time and the free-space check scale with the touched packs' size, not the patch's: 3.4 to 7.6 s and 1.12 GB for a 5.6 MB patch. **Proposal (the one thing):** a size cap in the packer that splits a zone's assets into parts of 64 to 128 MiB, a texture kept with its levels. ours: mounting and the manifest already handle any number of packs, so the parts would ship as ordinary packs. The install, the check, the disk held and the tables then shrink to the parts a patch touches. If the cap comes late, a smaller step is possible: the second hash pass could check only the fetched blocks, since kept blocks were hashed as they were copied. It cost 0.7 to 1.3 s here. It does guard against a torn write that the first pass can't see, so this is a trade, not a fix.
2. **The disk held doubles for good** (§5). Capped packs bound it to the touched parts. Content outside the bundle removes it.
3. **A changed pack's whole table is fetched.** The district's table is 2.4 MB, 43% of the small patch: every asset's entry and every block's hash, though the patch changed 90 of 44,842 blocks. A capped pack's table is only its part's. ours: a table diff would be a new format, out of scope here.
4. **An install that a killed launch left runs before the first frame** (§4): 5.9 s with no picture after the small patch's kill, and 11.5 s after the large one's. ours: iOS ends an app that takes too long to launch (on the order of 20 s), and a phone's storage is slower than this Mac's. So a large pack's install there may not finish before that limit, and each launch would start it again. **Proposal:** install a staged release from the boot's `Step::Update`, under the boot screen and its bar, as a fresh download's install already is, with the launch reading the current release until it's done. Not done here: it changes the boot's order, and that is the boot's owner's call.
5. **Staging uses one file per block.** The large patch staged 30,392 files. Each is created, written and renamed, read again by the install, then removed: 1.7 to 2.5 s for the removals alone. A staged file per pack, written at block offsets, would remove most of that. ours: a phone's file system is likely slower per file than APFS on this Mac.
6. **A shipped pack was opened once for every block** (fixed, §7).

## 7. What was fixed

**An install opened a shipped pack once for every block it kept.** `Shipped::read_at(path, offset, len)` opened the file every time. On the small patch that is 44,752 opens of the same 1 GiB file through `ContentFs` (on Android, the APK's asset manager). In a profile of one small install (`sample`, before the fix), `open` was 684 of the install's 1,204 samples. Patched packs were already opened once. Now `Shipped::open` returns a `ShippedFile` that reads at offsets, and the install keeps one reader per pack, shipped or patched, for its whole length. `tests/install.rs` `an_install_opens_each_shipped_pack_once_for_its_blocks` counts the opens and reads through a `Shipped` that records them. **Its effect on time isn't proven here.** The small install took 3,481 ms before and 3,398 ms after, which is inside the spread of five runs before the fix (3.5 to 7.6 s at loads 12 to 18). ours: the saving is larger where opening an asset costs more, as on Android.

## 8. What isn't verified

- **Phones.** Nothing here ran on a device, the Simulator or the emulator. Install times, the free-space check and the launch limit on a phone's storage are reasoned (§5, §6).
- **A cold page cache.** Release 1's packs had been read just before each run. A player's first patch reads the shipped pack from storage.
- **A quiet machine.** Every run shared the machine with other builds, at loads of 12 to 20, and the same install varied 3× between runs.
- **A real host.** The host is loopback: no latency, no bandwidth limit, no CDN. The download times are the patcher's own cost. On a network, add the bytes over the player's bandwidth and a round trip per request (11, 374 and 1,375 of them).
- **A second patch over a first,** and the disk it holds (§5), are reasoned from the code.
- **A power cut.** M4's fault injection covers kills and failed writes, not lost flushes.

## 9. How to repeat it

The harness isn't on main. It is the two examples (`m7_successor` for `sapphire_pipeline`, `m7_phases` for `sapphire_patch`) and three scripts (`build.sh`, `prepare.py`, `rehearse.py`), handed to the orchestrator with this report. With the benchmark district cooked, in a checkout with the examples under `tools/sapphire_pipeline/examples/` and `crates/sapphire_patch/examples/`, run each line under one gate slot:

```
bash build.sh                                        # the two examples, game_app --features dev (tools profile), xtask
python3 -I prepare.py <checkout> <zone_district.spack>  # release 1, the three successors, a host each: about 4 min, under 6 GiB on APFS (clones)
python3 -I rehearse.py <checkout> small medium large    # about 6 min; --skip-kills, --kill-at <share of the install>
```

Every generated file is under the checkout's `target/m7`, and goes when it does.
