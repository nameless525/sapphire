# Apple ship readiness for M4: iOS and macOS

A scout's read on 2026-11-10. Web only, nothing built or run. Items marked "unverified" come from the scout's prior knowledge (to mid-2026) and were not re-checked against a page this run. Treat them as leads.

## 1. Privacy manifests (`PrivacyInfo.xcprivacy`)

- Top-level keys: `NSPrivacyTracking` (bool), `NSPrivacyTrackingDomains` (array), `NSPrivacyCollectedDataTypes` (array), `NSPrivacyAccessedAPITypes` (array of dicts with `NSPrivacyAccessedAPIType` + `NSPrivacyAccessedAPITypeReasons`). Source: https://developer.apple.com/documentation/bundleresources/privacy-manifest-files (unverified this run).
- Required-reason categories and the codes that fit a game reading its own files and timing frames (source: https://developer.apple.com/documentation/bundleresources/describing-use-of-required-reason-api, unverified this run):
  - File timestamp `NSPrivacyAccessedAPICategoryFileTimestamp`: `C617.1` covers timestamps of files inside the app container, app group or CloudKit container. ours: this covers hot reload and crash-report cleanup.
  - System boot time `NSPrivacyAccessedAPICategorySystemBootTime`: `35F9.1` covers measuring elapsed time between in-app events. ours: `mach_absolute_time` falls in this category, so it must be declared.
  - Disk space `NSPrivacyAccessedAPICategoryDiskSpace`: `E174.1` covers checking for enough space before writing files. ours: the patcher needs this; also check whether SDL calls it.
  - User defaults `NSPrivacyAccessedAPICategoryUserDefaults`: `CA92.1` covers data that only the app itself reads. ours: needed if SDL touches `NSUserDefaults`.
  - Active keyboards: no reason fits a game. ours: make sure nothing calls it.
- Third-party SDKs: Apple's list of "commonly used SDKs" (for example Firebase and Alamofire) must ship their own signed manifest. Source: https://developer.apple.com/support/third-party-SDK-requirements/ (unverified this run). ours: SDL and the Rust crates are not on that list as far as known, and with static linking there is no separate bundle. Declare everything in the app's own manifest.
- Enforcement: since 2024-05-01, App Store Connect rejects uploads that use a required-reason API without a declared reason (ITMS-91053). Source: https://developer.apple.com/news/?id=pvszzano (unverified this run). ours: the manifest belongs in the `.app` root on iOS and in `Contents/Resources` on macOS, but it is only enforced for App Store builds.
- Local check: Xcode's Organizer can produce a privacy report from an archive. Unverified whether this works on a bundle that Xcode did not build. ours: `plutil -lint` checks the file's syntax but not its content.

## 2. Upload rules (App Store Connect / TestFlight)

- From 2026-04-28, uploads must be built with Xcode 26 or later and the iOS 26 SDK. Sources: https://developer.apple.com/news/upcoming-requirements/, https://expo.dev/blog/app-store-connect-minimum-sdk-26. ours: Xcode 26.2 is installed locally, so this is met, provided the build tool links against the Xcode 26 SDK and records `DTSDKName`, `DTXcode` and `DTPlatformVersion` in Info.plist. Xcode adds those keys itself, and validation checks them (unverified). A hand-made plist must add them too.
- One forum thread reports that a build with the iOS 26.2 SDK uploaded after the deadline with only a warning: https://forums.unrealengine.com/t/ios-26-sdk-app-store-submission/2717478. This is anecdotal.
- App icon: uploads need a 1024x1024 icon compiled into `Assets.car` by `actool`, plus `CFBundleIcons` in the plist (unverified). Xcode 26 adds Icon Composer `.icon` files, which `actool` also compiles. Source: https://developer.apple.com/icon-composer/ (unverified). ours: `xcrun actool` runs offline without an Xcode project, so the existing build tool can call it.
- Launch screen: an iOS app needs either `UILaunchScreen` (an empty dict is allowed) or a storyboard (unverified). ours: what exists today is probably enough.
- Export compliance: set `ITSAppUsesNonExemptEncryption` to `false` when the only encryption is the OS's HTTPS/TLS. That skips the export-compliance question on every build. Source: https://developer.apple.com/documentation/security/complying-with-encryption-export-regulations (unverified this run). ours: this fits the planned HTTPS patcher. If the content packs have their own encryption, check again.
- `CFBundleVersion`: must be one to three period-separated integers and must increase for each upload under the same `CFBundleShortVersionString` (unverified).
- arm64-only is fine. Set `UIRequiredDeviceCapabilities` to include `arm64` and `metal` (unverified).
- Tools:
  - `xcodebuild -exportArchive` needs an `.xcarchive` and distribution signing (an account, certificate and profile).
  - `xcrun altool --validate-app` / `--upload-app` need an App Store Connect API key or an Apple ID. ours: altool is being phased out in favour of the newer upload paths, so check before relying on it (unverified).
  - `notarytool` needs credentials and Apple's servers.
- ours: these checks run offline: `codesign --verify --strict`, `plutil -lint`, `actool` output, a privacy-manifest lint by script, and running in the Simulator. Full "Validate App" checks need the account.

## 3. TestFlight internal testing

- Internal testers (up to 100 members of the App Store Connect team) get builds without Beta App Review. External testers (up to 10,000) need Beta App Review of the first build of a version. Source: https://developer.apple.com/testflight/ (unverified this run).
- ours: an internal build still needs everything in §2 (distribution signing, the export compliance answer), and the app record, agreements and testers must be set up in App Store Connect first.

## 4. Size limits (spec Q11)

- Max app size: 4 GB uncompressed for the main binary bundle (unverified; https://developer.apple.com/help/app-store-connect/reference/maximum-build-file-sizes). Cellular downloads: since iOS 13 the user decides; 200 MB is the point where the OS asks (unverified).
- On-Demand Resources are deprecated as of iOS 27, and Apple recommends moving to Background Assets. Source: https://developer.apple.com/forums/thread/820986 (secondary; check the ODR docs).
- Apple-hosted background asset packs (WWDC25 "Discover Apple-Hosted Background Assets", https://developer.apple.com/videos/play/wwdc2025/325): Apple hosts the packs and delivers them through the BackgroundAssets framework, and they can download at install or later.
  - Limit is 200 GB total per app record, counting each pack ID's largest eligible version: https://developer.apple.com/help/app-store-connect/reference/apple-hosted-asset-pack-size-limits.
  - The same file in two packs counts twice: https://developer.apple.com/forums/thread/827100. Whether the limit is per team or per app is unclear (same thread).
  - Apple-hosted packs need iOS 26 or later (forum claim, https://developer.apple.com/forums/thread/805124). ours: that conflicts with the engine's min iOS 17 unless older OS versions fall back to the patcher.
- ours: Apple-hosted packs need their own app extension, which an Xcode-less build would have to produce. Self-hosting through the patcher plus a background `URLSession` avoids that.

## 5. Review guidelines on downloaded content

- Guideline 2.5.2: apps may not "download, install, or execute code which introduces or changes features or functionality". There is a stated exception for educational apps that show code. Source: https://developer.apple.com/app-store/review/guidelines/#2.5.2.
- Guideline 4.7 lets apps offer HTML5 mini apps and games, plus software not embedded in the binary. Its stated conditions are that the code must not change the app's primary purpose, must stay within the guidelines and must not create a store (paraphrased, unverified wording).
- Ambiguous: whether Luau bytecode in a downloaded pack counts as "code". ours: interpreted script that changes gameplay is the grey zone. Many shipped games download Lua scripts, and review has tolerated it when the scripts don't change the app's purpose and don't get around review or IAP. No JIT is needed, so no entitlement question arises. The safest stance: ship all script in the binary's bundle at review time, use downloaded packs for content and balance data, and never unlock features that review didn't see.

## 6. iOS thermal and memory

- `ProcessInfo.thermalState` has four levels: `nominal`, `fair`, `serious` and `critical`. Source: https://developer.apple.com/documentation/foundation/processinfo/thermalstate (unverified this run).
  - Apple's suggested response (paraphrased): at `fair`, start cutting background work; at `serious`, reduce CPU, GPU and I/O and lower the frame rate or quality; at `critical`, cut to the minimum.
  - The change notification is `ProcessInfo.thermalStateDidChangeNotification`.
- `os_proc_available_memory()` (iOS 13 and later) returns the bytes left before the jetsam limit. Source: https://developer.apple.com/documentation/os/3191911-os_proc_available_memory (unverified). ours: it pairs with `task_info` `phys_footprint`.
- The entitlement `com.apple.developer.kernel.increased-memory-limit` raises the limit on some devices (unverified).
- Game Mode on iOS 18 and later starts on its own for a full-screen game. It lowers background activity and Bluetooth latency. Declaring `LSApplicationCategoryType` = `public.app-category.games` is what qualifies an app; on iOS 26 a Games app category also exists (unverified). ours: add the key.
- Low Power Mode: read `ProcessInfo.isLowPowerModeEnabled` and listen for `NSProcessInfoPowerStateDidChange`. ours: treat it like `fair` and cap at 30 fps.

## 7. macOS outside the Mac App Store

- Notarization requires the hardened runtime (`codesign --options runtime`) with a Developer ID Application certificate and a secure timestamp. Source: https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution (unverified this run).
- Entitlements: wgpu on Metal needs no JIT entitlement (unverified, though Metal shader compilation runs in the OS's own processes).
  - ours: none of `allow-jit`, `allow-unsigned-executable-memory` or `disable-library-validation` should be needed with static SDL and no plug-in dylibs.
  - Add `com.apple.security.cs.disable-library-validation` only if Steam overlay injection or the Steamworks dylib requires it; Steam's guidance varies, so check when integrating.
- `xcrun notarytool submit --wait` needs an App Store Connect API key, or an Apple ID with an app-specific password and team ID. Then run `xcrun stapler staple` on the `.app`, `.dmg` or `.pkg`. Zip a bare `.app` before submitting.
- Local checks:
  - `codesign --verify --strict --deep --verbose=2` runs offline.
  - `codesign -d --entitlements -` runs offline.
  - `spctl --assess --type execute -vv` only passes for a Developer ID signature, and needs notarization online or a stapled ticket. With an ad-hoc signature it is expected to say rejected.
  - `stapler validate` checks a stapled ticket.

## 8. Accessibility Nutrition Labels

- These are per-device declarations on the App Store product page, shown on iOS 26 and the other 26 releases. Source: https://developer.apple.com/help/app-store-connect/manage-app-accessibility/overview-of-accessibility-nutrition-labels.
- There are nine features: VoiceOver, Voice Control, Larger Text, Dark Interface, Differentiate Without Color Alone, Sufficient Contrast, Reduced Motion, Captions and Audio Descriptions. Sources: https://support.apple.com/123073, https://sixcolors.com/post/2025/05/apple-teases-accessibility-nutrition-labels-as-part-of-wide-ranging-feature-preview/.
- They are voluntary for now. Apple says they will become required to submit apps "over time", with no date given (help page above).
- ours: a game can honestly claim Reduced Motion, Differentiate Without Color Alone and Captions if it meets Apple's criteria for common tasks. VoiceOver is unlikely.

## What the run can do with no account

- Write `PrivacyInfo.xcprivacy` with the codes in §1 and lint it (`plutil -lint`, plus a script that checks the keys).
- Add `ITSAppUsesNonExemptEncryption=false`, `LSApplicationCategoryType`, `UIRequiredDeviceCapabilities` and the `DT*` build keys to the plist template.
- Compile an icon with `xcrun actool` into `Assets.car`.
- Ad-hoc sign, run `codesign --verify --strict --deep`, and run in the Simulator.
- Hook up the thermal, Low Power Mode and `os_proc_available_memory` signals and test them in the Simulator.
- macOS: sign with the hardened runtime ad-hoc and check that the app still runs (catches any missing entitlement).

## What only the account holder can do

- Get Developer ID and Apple Distribution certificates and provisioning profiles.
- Create the App Store Connect app record, accept the agreements and fill in the privacy and accessibility labels.
- Upload, validate and add TestFlight testers (internal and external Beta App Review).
- Notarize with `notarytool` and run `spctl` against the result.
- Upload Apple-hosted asset packs.
- Sign for a device and test on one.
