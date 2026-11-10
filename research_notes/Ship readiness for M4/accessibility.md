# Accessibility for an action game: what the guidelines ask

A scout's read on 2026-11-10, web only, nothing built or run. "ours:" marks inference; everything else is a source's claim with its link. Facts recalled rather than read today say "may be stale".

## 1. Guideline sets

- Game Accessibility Guidelines sorts items into basic / intermediate / advanced across motor, cognitive, vision, hearing, speech, general. https://gameaccessibilityguidelines.com/full-list/
- Basic items that fit us (may be stale on tiering): remappable controls; no essential info by colour alone; subtitles for speech, clear and resizable; separate volume controls; avoid flickering images and repetitive patterns; adjustable difficulty; no unnecessary simultaneous button presses. Same list.
- Intermediate/advanced: hold-vs-toggle, adjustable timing/QTE windows or alternatives, camera-shake and motion options, mono audio, captions for significant sounds, adjustable UI size. Same list.
- Xbox Accessibility Guidelines (XAGs): numbered guidelines, e.g. 101 text display, 102 contrast, 103 additional channels for visual and audio cues, 104 subtitles and captions, 107 input, 117 visual distractions and motion, 118 photosensitivity, 112 UI navigation, 114 difficulty and assists (numbers may be stale). https://learn.microsoft.com/en-us/gaming/accessibility/guidelines
- Apple Accessibility Nutrition Labels (App Store, 2025): VoiceOver, Voice Control, Larger Text, Dark Interface, Differentiate Without Color Alone, Sufficient Contrast, Reduced Motion, Captions, Audio Descriptions; claim one only if all common tasks work with it. https://developer.apple.com/help/app-store-connect/manage-app-accessibility/overview-of-accessibility-nutrition-labels
- ours: labels are declarations, not requirements; Larger Text, Differentiate Without Color, Reduced Motion and Captions are the ones a game like ours could honestly claim.
- ours, items for a dodge-and-counter game: full remap (have), hold/toggle for guard and sprint, a wider parry/dodge window or slow-motion assist option, auto-target strength, a story/assist difficulty, flash and shake intensity sliders, separate music/SFX/voice sliders, mono audio, subtitles for in-combat barks too.

## 2. Photosensitive epilepsy

- WCAG 2.3.1: nothing flashes more than three times in any one second, or the flash is below the general and red flash thresholds. https://www.w3.org/WAI/WCAG22/Understanding/three-flashes-or-below-threshold
- General flash threshold: a pair of opposing changes in relative luminance of 10% or more of max, where the darker state is below 0.80, over a combined area larger than ~25% of a 10-degree visual field (WCAG uses 341x256 px at 1024x768 as the proxy). Red flash: a transition to/from saturated red. Same page.
- ITU-R BT.1702 (broadcast): same three-per-second limit, with luminance change of 20 cd/m2 or more where the darker image is below 160 cd/m2, over more than 25% of screen; also limits regular patterns. https://www.itu.int/rec/R-REC-BT.1702 (details from memory, may be stale)
- Ofcom Broadcasting Code guidance (UK) is the basis for the Harding FPA test used for UK TV. https://www.ofcom.org.uk/tv-radio-and-on-demand/broadcast-codes/broadcast-code (may be stale)
- Japan: NHK/NAB guidelines after the 1997 Pokemon incident restrict rapid flashes (no more than 3 per second) and vivid red flashing. https://en.wikipedia.org/wiki/Denn%C5%8D_Senshi_Porygon (secondary source)
- Stores: no store found that requires a flash test; Harding offers a games certification but it's voluntary. XAG 118 recommends testing. (ours: couldn't confirm any current store rule.)
- ours, what we can compute per frame: mean relative luminance per screen tile, delta vs previous frame; count opposing transitions in a sliding 1 s window; flag when area with delta >= 0.1 exceeds 25% of screen and count > 3; flag saturated-red transitions separately (R/(R+G+B) >= 0.8 per WCAG's definition). Impact frames (full-screen inversion) are exactly what trips this; a "reduce flashes" option should cap their luminance swing and rate.

## 3. Subtitles

- BBC subtitle guidelines: up to ~37 characters per line, two lines max, roughly 160-180 words per minute, speaker identified by colour or label, sound captions in brackets. https://www.bbc.co.uk/accessibility/forproducts/guides/subtitles/
- XAG 104: subtitles on by default or offered at first run, size options, background with adjustable opacity, speaker names, direction cues for off-screen speakers. https://learn.microsoft.com/en-us/gaming/accessibility/xbox-accessibility-guidelines/104
- Industry (Ian Hamilton / GDC talks and gameaccessibilityguidelines): 46 px minimum at 1080p is a common quoted floor; ~38 characters per line. https://gameaccessibilityguidelines.com/provide-subtitles-for-all-important-speech/ (figure may be stale)
- Phone: no authoritative px figure found; ours: scale from the 46 px@1080p floor by physical size, and honour the OS text-size setting.
- ours: extend speaker-named subtitles from cutscenes to combat barks, add a background box with an opacity slider (default ~75%), and caption key sounds (enemy windup) for the parry cue.

## 4. Colour-vision deficiency

- GAG and XAG 103: never convey essential info by colour alone; add shape, icon, pattern or sound. https://gameaccessibilityguidelines.com/ensure-no-essential-information-is-conveyed-by-a-colour-alone/
- Simulation for testing: Machado, Oliveira & Fernandes (2009) give matrices for protan/deutan/tritan at graded severity; Brettel, Vienot & Mollon (1997) and Vienot et al. (1999) for dichromats. https://www.inf.ufrgs.br/~oliveira/pubs_files/CVD_Simulation/CVD_Simulation.html
- Full-screen recolour ("daltonize") filters are discouraged because they shift every colour including art direction and still leave pairs confusable; designers recommend fixing specific cues instead (e.g. Game Accessibility Guidelines and accessibility talks). https://gameaccessibilityguidelines.com/ensure-no-essential-information-is-conveyed-by-a-colour-alone/ (ours: reasoning paraphrased)
- ours: run Machado matrices as a debug post pass over goldens for enemy-telegraph and element colours; give telegraphs a shape/flash pattern, and offer a cue-palette choice (e.g. blue/orange) rather than a global filter.
