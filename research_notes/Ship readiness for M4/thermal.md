# Thermal response for M4

A scout's read on 2026-11-10, web only, nothing built or run. "ours:" marks our own inference. "ours (memory):" marks facts recalled from before this search and not re-checked against a source today: verify them before relying on them.

## 1. Android ADPF: thermal

- The Java Thermal API arrived in Android 11 (API 30) and the NDK version in Android 12 (API 31). `getThermalHeadroomThresholds()` came in Android 15. Source: https://developer.android.com/games/optimize/adpf/thermal
- `getThermalHeadroom(forecastSeconds)` forecasts headroom, where 1.0 means the SEVERE throttling threshold. Google's guidance:
  - above 1.0, the status is likely SEVERE or worse, so cut load right away;
  - around 0.95, the status is likely MODERATE or worse, so reduce load and keep watching;
  - an initial NaN means the device doesn't support the API.

  Source: same page.
- Google suggests registering a thermal status listener and polling headroom only once the status reaches LIGHT or above. Cooling differs between devices, so the same headroom can mean throttling at different points. Source: same page.
- ours (memory): the NDK calls are `AThermal_getThermalHeadroom` and `AThermal_registerThermalStatusListener` (both API 30-31; check which) and `AThermal_getThermalHeadroomThresholds` (API 35). Headroom is rate-limited to about one call per second; faster calls return NaN. A forecast of about 10 s is typical.
- ours: our existing `AThermal_getCurrentThermalStatus` read can stay as the coarse signal. Add a listener plus a 1 Hz headroom poll once the status is LIGHT or above, feeding the response ladder in section 5.

## 2. Android ADPF: performance hints and Game Mode

- Google says the Performance Hint API tells the system your threading model and workload so it can allocate CPU resources. Source: https://developer.android.com/games/optimize/adpf/thermal
- ours (memory), the API levels:
  - API 33: `APerformanceHint_getManager`, `createSession(tids, initialTargetNs)`, `reportActualWorkDuration` and `updateTargetWorkDuration`;
  - API 34: `setThreads`;
  - API 35: `setPreferPowerEfficiency` and `reportActualWorkDuration2` with `AWorkDuration`, which carries CPU and GPU durations separately;
  - API 36: SDK-side GPU-duration plumbing and session-creation refinements.
- ours (memory): ADPF guidance is to report a duration every frame for the main and render threads, with the target set to the frame budget (16.6 ms or 33.3 ms). ours: the main and render threads need to be at least API 33, which is within our minSdk 29 only behind a runtime check. Load the symbols with `dlsym` or guard them by API level.
- ours (memory): the Game Mode API (`GameManager.getGameMode`, API 31) reports the user's chosen mode: standard, performance, battery or custom. `GameManager.setGameState` (API 33) tells the system when the game is loading or in gameplay. It requires `appCategory="game"`. Both are Java APIs, so they need a JNI call through SDL's activity. Low priority.

## 3. iOS

- ours (memory): `ProcessInfo.thermalState` has four levels: nominal, fair, serious and critical. Changes post `NSProcessInfoThermalStateDidChangeNotification`. Low Power Mode is `isLowPowerModeEnabled` with `NSProcessInfoPowerStateDidChangeNotification`.
- Apple's archived energy guide (Mac-era, may be stale for iOS) says:
  - at serious, apps should begin corrective action: reduce frame rates and the quality of visual effects, and cut CPU, GPU and I/O use;
  - at critical, apps should cut CPU, GPU and I/O to the minimum needed to respond to the user.

  Source: https://developer.apple.com/library/mac/documentation/Performance/Conceptual/power_efficiency_guidelines_osx/RespondToThermalStateChanges.html
- ours (memory): Apple's current docs describe fair as a slightly elevated state where a background-work reduction is optional.
- Unity has a player setting that lowers the iOS frame rate at serious and critical, which is engine precedent rather than Apple guidance. Source: https://docs.unity3d.com/6000.5/Documentation/ScriptReference/PlayerSettings-adjustIOSFPSUsingThermalState.html
- ours: map nominal or fair to no action, serious to the ladder's middle rungs, and critical to the 30 fps floor plus a minimum tier. Treat Low Power Mode like serious.

## 4. Dynamic resolution

- Unreal adjusts the primary screen percentage from the previous frames' GPU time. The defaults are a minimum of 50%, a maximum of 100% and a 33.3 ms frame budget. `HistorySize` sets the history window, which trades noise against lag. `ChangePercentageThreshold` prevents thrashing between nearly equal resolutions, so it acts as the hysteresis. `CPUBoundScreenPercentage` sets the target when the game is CPU-bound. Source: https://dev.epicgames.com/documentation/en-us/unreal-engine/dynamic-resolution-in-unreal-engine
- ours (memory): Unreal and other engines upscale temporally (TSR, FSR, DLSS) after dynamic resolution, so history reprojection hides scale changes. Without a temporal upscaler, a scale step shows as visible shimmer, and the floor has to stay higher.
- ours (memory): Unity's Adaptive Performance has an Android provider built on ADPF and a Samsung GameSDK provider. Its Indexer scalers step resolution, LOD, shadows and frame rate according to thermal trend and bottleneck. Source to check: https://docs.unity3d.com/Packages/com.unity.adaptiveperformance@latest
- ours: our controller (5% steps, 0.75 floor) matches common practice on phones. Add hysteresis, with a faster step down than up and an up-step only after N stable frames. Keep 0.75 unless an upscaler arrives, since anime line work and outlines degrade badly at low scale. Outline width must scale with resolution.
- The reference games: we found nothing from Digital Foundry or Android Authority on Genshin Impact, Honkai: Star Rail, Wuthering Waves or ZZZ's thermal logic. Smaller outlets report:
  - Wuthering Waves held a steady ~30 fps on flagship phones in one test: https://www.mysmartprice.com/personal-tech/mobiles/mobiles-features/oneplus-15-vs-iqoo-15-do-dedicated-gaming-chipsets-actually-improve-performance/
  - Wuthering Waves 3.4 throttled and dropped performance on phones: https://www.notebookcheck.net/Wuthering-Waves-version-3-4-places-heavy-strain-on-mobile-players.1319209.0.html
- ours: those games expose user-facing frame-rate caps (30, 45, 60, sometimes 120) and quality presets. Nothing reputable documents an automatic thermal ladder in them. Treat that as unknown.

## 5. The response ladder

- Google's thermal page orders the options as smaller cores, lower frame rate, then lower fidelity. In Unity that means Adaptive Performance quality settings; in Unreal, scalability settings. Source: https://developer.android.com/games/optimize/adpf/thermal
- ours (memory): Arm's and Google's game-performance talks favour a stable, capped frame rate over a high, varying one, because a sustained 30 fps beats 60 fps falling to 40. Frame pacing through Swappy or Choreographer at a fixed interval matters more than the peak.
- ours: a proposed ladder for Sapphire, each rung entered on the trigger and left only after about 10 s below the threshold minus a margin:
  1. LIGHT, headroom 0.8 or iOS fair: let dynamic resolution step down toward 0.85, with nothing else.
  2. MODERATE, headroom 0.95 or iOS serious: drop optional passes (bloom resolution, SSAO or contact shadows, extra outline passes, particle density), with the resolution floor at 0.75.
  3. SEVERE, headroom 1.0 or more: cap the frame rate at 30 through `ANativeWindow_setFrameRate` and pacing, and on iOS through `preferredFramesPerSecond`. Lower `updateTargetWorkDuration` to match, and call `setPreferPowerEfficiency`.
  4. CRITICAL or above: the Mobile-Low tier and 30 fps. Show a "device is hot" notice; never quit.
- ours: never change the frame rate up and down repeatedly. Raise the cap back to 60 only after a long cool period (about 60 s) and only if the user's setting asks for 60. Log every rung change to telemetry so the exit run can show the ladder working on the emulator, which reports a fake thermal state, and on CI.
