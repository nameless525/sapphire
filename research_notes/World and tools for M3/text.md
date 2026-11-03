# UI text and localization for M3

A scout's read for [the M3 plan](../../docs/m3-plan.md), 2026-11-03: read-only, nothing built or run, on `main` at 1335f14d and crate sources from crates.io. Its claims are an agent's reading, and a package proves them before leaning on them.

**Spec corrections**
- `klippa` on crates.io (0.2.6) is an unrelated geometry clipper (R). The font subsetter is `skera`.
- `subsetter` is PDF-only: it strips `cmap` and its README says output is unusable elsewhere (R). Don't use it.
- `FluentResource` can't be cooked: its only constructor is `try_new(String)` (R). "Binary tables" (spec §5.25) therefore needs our own runtime.
- There is no `icu` 2.4 need; take the component crates. The `icu` meta crate's version wasn't checked (my index lookup used the wrong path).

**1. Taffy and parley**
- `taffy` 0.15.1, MIT, MSRV 1.71. Deps are all optional: slotmap, smallvec, serde, cssparser (R). Turn off defaults and take `std`, `taffy_tree`, `flexbox`, `grid`.
- `parley` 0.12.0, Apache-2.0 OR MIT, MSRV 1.88. It pulls `fontique` 0.12, `parley_engine` 0.12 and `parley_data` (R).
- `parley_engine` requires `harfrust` ^0.12, `skrifa` ^0.44, `icu_segmenter`, `icu_normalizer`, `icu_properties` ^2.3 with compiled data, and `parley_emoji` (R, index and Cargo.toml).
- Those are not the newest (`skrifa` 0.49, `harfrust` 0.15). `skrifa` 0.44 and `harfrust` 0.12 are already in Cargo.lock; `skrifa` comes through `epaint`, dev-only (R), and `harfrust` I assume comes the same way (I). `harfrust` is MIT, `skrifa` is MIT OR Apache-2.0, ICU is Unicode-3.0, all admitted (R).
- Embedded fonts only: yes (R for code, I for devices).
  - `Collection::new(CollectionOptions { shared: false, system_fonts: false })` plus `register_fonts(Blob<u8>)`, and `set_fallbacks` per script (`fontique/src/collection/mod.rs:37-52, 182, 210`).
  - Building parley with `default-features = false, features = ["std"]` removes the `system` feature, and with it objc2/CoreText, roxmltree on Android, fontconfig and windows (`fontique` and `parley` Cargo.toml; `backend/mod.rs:6-19`). `memmap2` stays under `std`.
  - Not run on a device yet.
- Rasterizing: parley rasterizes nothing. It yields `positioned_glyphs()`, plus `Run::font()`, `normalized_coords` and `synthesis` (`layout/line.rs:337`, `run.rs:86,108`). Three ways to rasterize:
  - `swash` 0.2.10 (Apache-2.0 OR MIT): pins `skrifa` `>=0.31.1, <=0.44`, so it shares parley's version, and rasterizes with `zeno` (R). Best first choice (I).
  - `skrifa` `OutlinePen` into `zeno` 0.3.3: small code of our own, gives control (I).
  - `glifo` 0.4 (`skrifa` 0.44): brings `vello_common` and `peniko`; too heavy (I).
- Cost: no published numbers found. My guess is tens of µs for a short HUD string and low hundreds of µs for a paragraph (I, unmeasured). An old HarfRust README says up to 3x slower than HarfBuzz, possibly stale (web snippet only).
- Caching:
  - Shaper plan, data and instance LRUs hold 16 entries each (`shape/shaper.rs:88-102`) (R).
  - `build_into` reuses a `Layout` (`builder.rs:93`), and `break_all_lines` and `align` are separate calls, so a width change needn't reshape (`layout.rs:210,226`) (R; that no reshape happens is I).
  - Our own cache: (text hash, style, max width) to `Layout` or to a flat glyph list (I). HUD digits should bypass parley with a preshaped digit strip, keeping the current no-allocation HUD.
- CJK line breaks: `complex-scripts` is off by default and selects dictionary segmenters (`analysis.rs:110-116, 153-159`). Without it, `new_for_non_complex_scripts` is used for words (R). Whether UAX-14 alone gives acceptable Japanese breaks and kinsoku is unconfirmed; `break_overrides.rs` exists but I didn't read it.

**2. Atlas and MSDF**
- Allocator: `etagere` 0.3.0, MIT OR Apache-2.0, has allocate and deallocate; deps are `euclid` plus optional ones (R).
- Ours (I): size buckets, glyph key, LRU eviction per frame, `queue.write_texture` dirty rects, multiple pages, and the renderer's texture binding. `UiAtlas` today is one immutable `Arc` with R8 texels (`sapphire_look/src/ui.rs:20,113`), so the list and pass must grow a page id and uploads through `FramePacket`. `cosmic-text` and `glyphon` overlap and fix their own shaper; rejected (I, sources unread).
- MSDF: `fdsm` 0.8.0, MIT, pure Rust, last updated 2025-10; requires `image` and `nalgebra` (R). `fdsm-ttf-parser` 0.2 only adapts ttf-parser. `msdfgen` and `msdf` are C++ bindings pinned to old ttf-parser, so rejected (R for deps). Ours: a `skrifa` pen to `fdsm::Shape`, atlas packing, and the median shader. Run it in the cooker only, so `image` and `nalgebra` stay out of the runtime (I).
- MSDF only for fixed display strings. `sdfer` 0.2.1 and `sdf_glyph_renderer` 1.1 exist, unread.

**3. Fonts** (sizes from the google/fonts and notofonts/noto-cjk GitHub APIs, R; all OFL files read at line 1 only)
| Font | Size | Note |
|---|---|---|
| Noto Sans JP [wght] | 9.59 MB | static Regular OTF 4.53 MB; RFN "Source" only |
| Noto Sans SC [wght] | 17.77 MB | RFN "Source" only |
| Noto Sans [wdth,wght] | 2.05 MB | no RFN |
| M PLUS 1 [wght] | 4.20 MB | no RFN, Latin plus JP, most game-like body (taste, I) |
| Zen Kaku Gothic New | ~2.3 MB per weight | no RFN |
| Dela Gothic One, RocknRoll One | 2.5, 2.7 MB | JP display, no RFN |
| Oxanium, Saira, Exo 2 | 44, 484, 304 KB | Latin display, no RFN |
- Sources: github.com/google/fonts `ofl/<name>/` and github.com/notofonts/noto-cjk `Sans/`. M PLUS Rounded 1c is under another licence path (its `ofl/` listing has fonts but no OFL.txt fetched), so check before use.
- Subsetting: `skera` 0.9.0, googlefonts/fontations, MIT OR Apache-2.0, updated 2026-10-21; deps `read-fonts` 0.46, `skrifa` 0.49, `write-fonts` 0.55, and takes text input and CFF (R). That's a second fontations line, so keep it in `sapphire_pipeline`/tools only; `multiple-versions` only warns (R). Its handling of GSUB/GPOS closure and variable instancing isn't confirmed; verify. Alternative: `hb-subset` 0.3 (C++ HarfBuzz), but adds a C++ build. `fontcull` is a fork-based wrapper, avoid.
- Commit the whole font. `content/third_party/README.md` requires byte-identical sources and never commits cooked output, so the subset is a cook product (R). 4-10 MB against a 251 MB pack is fine (R).
- Runtime: cook per locale. Keep strings' closure plus kana, punctuation and full ASCII, plus a policy for player-typed names, which a string-only subset can't cover: reject or ship Joyo kanji (I). A ~3,000-glyph CJK subset should be under ~1 MB by proportion (I), and instancing to 2-3 weights lowers it further. The binary must carry the OFL text and credit (known licence term, I).
- `deny.toml` doesn't scan `content/`; OFL matters only if a crate embeds a font (`epaint_default_fonts` exception) (R). So don't ship fonts in crates.

**4. Fluent and ICU**
- `fluent` 0.17.0, `fluent-bundle` 0.16.0 (2025-05), `fluent-syntax` 0.12.0, all Apache-2.0 OR MIT (R). `fluent-bundle` pulls `intl_pluralrules` 7, `unic-langid`, `intl-memoizer` and `self_cell`, a second plural and locale stack beside ICU (R). Its `DATETIME` is a TODO, and `NUMBER` has no locale formatting: only the hook `set_formatter` (`builtins.rs:4`, `bundle.rs:352,601`) (R).
- `icu_plurals`, `icu_decimal`, `icu_datetime` are 2.4.0, Unicode-3.0, MSRV 1.88, CLDR 49 (R). `deny.toml:39` admits Unicode-3.0 (R).
- Data, source text for all locales (R; binary is smaller, I):
  - plurals 50 KB across three markers
  - decimal 25 KB, plus 107 KB compact forms
  - datetime 18.75 MB: 9.57 MB time zones, 9.18 MB other, ~3.15 MB by a rough Gregorian and names filter
- Size: plurals plus decimal cost tens of KB with all locales, negligible (I). `icu_datetime` with compiled data could reach MBs, and it adds `icu_calendar` and `icu_time` (R).
- Minimum: `icu_plurals` and `icu_decimal` now. For dates, either run `icu4x-datagen` with `ICU4X_DATA_DIR` set (R: `build.rs:6`, `src/lib.rs:15`) for en and ja only, or `icu_provider_blob` (R, exists). Or delay `icu_datetime` unless a menu shows dates. Check real linked size in the spike; dead-stripping by marker is I.
- Cooking: `fluent-syntax` AST has serde derives (R), but `fluent-bundle` can't load it. Recommend parse at cook with `fluent-syntax`, emit postcard tables (key id, pattern ops, selectors), and run a ~300-line evaluator using `icu_plurals` and `icu_decimal`. This drops `fluent-bundle`, and bidi isolates are ours to decide. Fallback: ship stripped `.ftl` and parse at load with `fluent-bundle`.
- `fluent-pseudo` 0.3.3 exists but pulls `regex`; a small transform of our own is enough (I).

**5. Recommendation**
- Second language: Japanese. It exercises kana, Han, mixed Latin and kinsoku, it's the genre's voice-and-menu language, and needs half the font of SC (9.6 vs 17.8 MB). Keep the `Language` tag right, since Han glyphs differ per locale. Fonts: M PLUS 1 or Noto Sans JP as body (decide after viewing, plus coverage check), Oxanium or Saira plus Dela Gothic One as display.
- Package split:
  - T1 spike (below), then `sapphire_ui` text core: font registry, parley, layout cache.
  - T2 glyph atlas: pages, buckets, eviction, `UiDrawList` page id, renderer upload, goldens.
  - T3 font cook: `.meta` importer, `skera` subset from strings, `content/third_party` entries added by maintainers (agents never download).
  - T4 `sapphire_loc`: `.ftl` to tables, evaluator, ICU, pseudo-loc, per-locale fallback chains.
  - T5 taffy documents and bindings, on T1.
  - T6 MSDF display text in the cook, last, because it's droppable if atlas large sizes look good.
- Spike on iOS Simulator and Android emulator, headless, no system fonts, with a maintainer-provided font:
  - Build with `system` off and record binary size delta (parley plus ICU data).
  - Startup time for font registration, and memory with a 4 MB versus subset font.
  - Shape and layout cost per changed string (HUD label, 200-character JP paragraph), including allocations and cold plan cache.
  - Glyph raster time and 2048² R8 atlas churn with ~1,500 JP glyphs.
  - JP line breaking with and without `complex-scripts`, and small-text legibility at 1x and 3x.
  - `skera` round trip: subset, then parley still shapes with GPOS and GSUB and the same advances.

**Couldn't confirm**
- Any runtime number: costs and linked sizes are estimates, nothing was built.
- `skera`'s layout closure and variable instancing quality.
- Kanji coverage of M PLUS 1 and Zen fonts.
- Parley behaviour on real iOS/Android.
- `swash` variable-font coords on parley's output (I from memory).
- The transitive licences of `parley_emoji`, `parlance` and `euclid`; run `cargo deny` once added.
- `docs/m1-plan.md` §3 read only in part; the content rules above come from `content/third_party/README.md`.
