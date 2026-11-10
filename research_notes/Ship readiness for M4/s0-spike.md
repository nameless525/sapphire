# S0: what each store's tools accept here

M4 S0's spike, run on 2026-11-10 on an Apple M5 (macOS 27, build 26A434) with Xcode 26.2 (17C52), the iOS 26.2 SDK (23C53), NDK 29.0.14206865, build-tools 36.0.0, AGP 9.4.1, Gradle 9.8.2 and Rust 1.99.0, at base 0755aafb. No account, no signing identity, no team, nothing uploaded, nothing installed on a device, and nothing downloaded: Gradle ran `--offline`, and every tool used was already on this Mac. Other agents were building at the same time (loads of 3 to 18), so times are rough.

The scripts are kept out of the repo: S1–S4 write their own steps into `xtask package`, and the commands below are what they need. `$S` is a scratch directory; paths in `target/` are this checkout's.

## Summary for S1–S4

| Step | Result | Where it stops | For |
|---|---|---|---|
| Hand-made `.xcarchive`, `xcodebuild -exportArchive` | Xcode takes the archive and runs eight of its distribution steps | Signing: `No signing certificate "iOS Distribution" found` (manual) or `No profiles for 'dev.sapphire.game' were found` (automatic) | S2: no Xcode project needed so far. §3 stands |
| Icon with `actool` | `Assets.car`, two loose PNGs and the plist keys, from one 1024 px PNG, no project | Nothing | S1, S2, S4 |
| Required-reason APIs | The game links `stat`, `fstat`, `fstatat` and `mach_absolute_time`, and no Objective-C one | Nothing | S2: two categories, C617.1 and 35F9.1 |
| Android AAB, lint, bundletool | `bundleRelease --offline` builds a 38 MB AAB from the cache; `validate`, `build-apks --local-testing`, an install and a capture on the emulator all pass | Release lint: 52 errors, all in SDL's synced Java | S3: a `lint.xml` scoped to SDL's glue. §3 stands |
| macOS, ad hoc with the hardened runtime | Signs, verifies `--strict --deep`, and the title and a new game run headless with no entitlement | `spctl` rejects it, as an ad hoc signature must | S4 |
| Windows manifest and icon | `embed-resource` with a hand-written `.rc`: a manifest, an icon and a version, read back out of a linked PE | The game itself can't build for Windows here (`blake3`'s C needs MSVC) | S4 |

## 1. The iOS archive

### What was built

`cargo xtask bundle --platform ios --app game_app --release`, through `slot --all --load 8`: 365 s from cold, the mobile cook included. It gives `target/ios/SapphireGame.app`, signed ad hoc: a 42.8 MB executable (thin LTO, `debug = "line-tables-only"`, unstripped) and 54 MB of content, 67 MB in all.

The dSYM came from `xcrun dsymutil target/aarch64-apple-ios/release/game_app -o $S/game_app.dSYM` (171 MB). Its UUID matches the bundled binary's (`xcrun dwarfdump --uuid`). It isn't whole: the `release` profile's `split-debuginfo` is `unpacked` on Apple targets, and LTO extracts the C objects in rlibs to temporary directories that are gone by the time `dsymutil` runs. It warned about 207 objects: 133 of SDL's, 64 of Luau's, 8 of zstd's, 1 of mimalloc's and 1 of blake3's. The Rust code's line tables are there. The `shipping` profile's `split-debuginfo = "packed"` has rustc run `dsymutil` itself at link time, while those objects still exist. **S2 and C1 must check that the shipping dSYM carries SDL's and Luau's lines.**

### The archive

The layout, by hand, with no Xcode project:

```
SapphireGame.xcarchive/
  Info.plist
  Products/Applications/SapphireGame.app
  dSYMs/SapphireGame.app.dSYM
```

The archive's `Info.plist`:

- `ApplicationProperties`: `ApplicationPath` (`Applications/SapphireGame.app`), `Architectures` (`arm64`), `CFBundleIdentifier`, `CFBundleShortVersionString` and `CFBundleVersion`;
- `ArchiveVersion` 2, `CreationDate`, `Name` and `SchemeName`.

`plutil -lint` passes on both plists, and `codesign --verify --strict --deep` on the archived app.

### `xcodebuild -exportArchive`

The command, for each `ExportOptions.plist` (`method`, `destination` = `export`, `signingStyle`, no `teamID`, no identity), never with `-allowProvisioningUpdates`:

```
xcodebuild -exportArchive -archivePath $S/SapphireGame.xcarchive -exportPath $S/export \
  -exportOptionsPlist $S/ExportOptions.plist
```

Every run exits 70. What it said, in order of how far it got:

1. **The app as `xtask bundle` makes it.** With `method` `release-testing`: `exportOptionsPlist error for key "method" expected one {app-store-connect, developer-id, debugging, mac-application, validation}`. Xcode offers macOS's methods, so it doesn't know the archive is for iOS. With `app-store-connect` or `debugging`, in either signing style: `No Team Found in Archive`.
2. **With `DTPlatformName` = `iphoneos` in the app's Info.plist,** Xcode takes it as an iOS archive and `release-testing` is accepted. That key alone does it: `CFBundleSupportedPlatforms` and `DTSDKName` on their own don't. Every method then stops at `No Team Found in Archive`. The team comes from the signature, and an ad hoc one has none.
3. **With a placeholder team, `teamID` = `0000000000` in the export options** (nobody's team; a `Team` key in the archive's `ApplicationProperties` isn't read, and alone still gives `No Team Found in Archive`), Xcode's distribution log shows eight steps run: analyse the archive, team, upload account ("Failed to find an account with App Store Connect access", logged and passed), fetch and create the app record, analyse the version, options and signing method. It stops at the ninth, signing assets:
   - `app-store-connect`, manual: `No signing certificate "iOS Distribution" found`;
   - `app-store-connect`, automatic: `No profiles for 'dev.sapphire.game' were found`;
   - `debugging`, manual: `No signing certificate "iOS Development" found`.
4. **`validation`** wants `destination` = `upload`, which talks to Apple, so it wasn't run.

So Xcode accepts a hand-made archive up to the signing it can't do. What it checks after signing (re-signing the bundle, making the `.ipa`, processing symbols, App Store Connect's checks on upload) is still unseen. The logs go to `$TMPDIR/<Name>_<date>.xcdistributionlogs/IDEDistribution.standard.log`.

**For S2:**

- Write `DTPlatformName` and the other `DT*` keys (`DTPlatformVersion`, `DTSDKName`, `DTSDKBuild`, `DTPlatformBuild`, `DTXcode`, `DTXcodeBuild`, `DTCompiler`, `BuildMachineOSBuild`), with `CFBundleSupportedPlatforms` = `iPhoneOS`. Read their values from `xcodebuild -version` and `xcodebuild -version -sdk iphoneos`.
- The store variant also took `ITSAppUsesNonExemptEncryption` = false, `LSApplicationCategoryType` = `public.app-category.games`, the icon and `PrivacyInfo.xcprivacy`. With all of it, re-signed ad hoc, the archive went exactly as far, and passed `plutil -lint` and `codesign --verify --strict --deep`.
- The maintainer's export is the first time distribution signing runs. If it fails after the signing step, §3's fallback (a generated Xcode project) is what's left.

## 2. The icon

From one 1024×1024 PNG with no alpha (App Store icons must not have one):

```
Assets.xcassets/Contents.json
Assets.xcassets/AppIcon.appiconset/Contents.json   # one image: idiom universal, platform ios, size 1024x1024
xcrun actool Assets.xcassets --compile $S/out --platform iphoneos --minimum-deployment-target 17.0 \
  --app-icon AppIcon --output-partial-info-plist $S/out/partial.plist \
  --target-device iphone --target-device ipad --output-format human-readable-text --notices --warnings --errors
```

`actool` 26.2 (24506) exits 0 with no project, in 56 s cold (it starts a simulator service; later runs take seconds). It writes `Assets.car` (285 KB), `AppIcon60x60@2x.png` and `AppIcon76x76@2x~ipad.png`, all three for the `.app`'s root. `xcrun assetutil --info` shows the 1024 px icon for phone and pad. The partial plist, merged into the app's Info.plist with `PlistBuddy -c "Merge partial.plist"`, adds `CFBundleIcons` and `CFBundleIcons~ipad`, each with `CFBundlePrimaryIcon` holding `CFBundleIconFiles` (`AppIcon60x60`, and `AppIcon76x76` on iPad) and `CFBundleIconName` = `AppIcon`.

For macOS, an `AppIcon.appiconset` of ten `mac` images (16 to 512 at 1× and 2×, resized with `sips -z`) through `actool --platform macosx --minimum-deployment-target 14.0` gives `Assets.car`, `AppIcon.icns` and the keys `CFBundleIconFile` and `CFBundleIconName`, both `AppIcon`. `iconutil -c icns AppIcon.iconset` makes an `.icns` from the same files without `actool`.

**For S1:** `actool` is the route on both Apple platforms, run by xtask from a generated catalog. The generator draws one 1024 px opaque PNG, and xtask resizes it for macOS. It needs Xcode, so it's a macOS-host step, as the bundles are.

## 3. Required-reason APIs

Apple's list, read 2026-11-10 from its documentation's data for "Describing use of required reason API" and `NSPrivacyAccessedAPIType` (developer.apple.com):

- file timestamp: `stat`, `fstat`, `fstatat`, `lstat`, `getattrlist`, `fgetattrlist`, `getattrlistbulk`, `getattrlistat`, and Foundation's creation and modification dates;
- system boot time: `mach_absolute_time`, `systemUptime`;
- disk space: `statfs`, `statvfs`, `fstatfs`, `fstatvfs`, `getattrlist` and its kin, and Foundation's capacity keys;
- active keyboards: `activeInputModes`;
- user defaults: `NSUserDefaults`.

Since 2024-05-01 App Store Connect refuses an upload that uses one without declaring a reason (same page). The page says an SDK must declare its own use and can't rely on the app's manifest, but says nothing about statically linked code. With SDL, Luau and the crates linked into one executable, there's no other bundle to hold a manifest, so the app's own declares everything.

The scan, on the release device binary: `nm -u` for the C names, and for the Objective-C ones `nm -u` (`_OBJC_CLASS_$_…`) plus `strings` (selectors and constants). Then each input rlib, with Xcode's `nm` for machine code and LLVM 23's `llvm-nm` for Rust's bitcode objects, to say who references what:

| API | Category | Referenced by |
|---|---|---|
| `stat` | file timestamp | std (`fs::metadata` and the like, which our platform, save, crash and pipeline-cache code call), SDL (`SDL_sysfsops.c`, `SDL_steam_virtual_gamepad.c`) |
| `fstat` | file timestamp | std, SDL (`SDL_iostream.c`) |
| `fstatat` | file timestamp | std |
| `mach_absolute_time` | system boot time | SDL (`SDL_systimer.c`), Luau (`mlua_sys`), `wgpu_hal` (Metal), `cpal` (Core Audio) |

None of the disk-space, keyboard or user-defaults APIs is linked, and no Objective-C one is: the `objc2` crates name many in their bindings, but none survives into the binary. `lstat` is referenced by std and dead-stripped. Our own crates call no listed function directly; Rust's `Instant` uses `clock_gettime`, which Apple doesn't list.

So the manifest needs `NSPrivacyAccessedAPICategoryFileTimestamp` with `C617.1` (files inside the app's container) and `NSPrivacyAccessedAPICategorySystemBootTime` with `35F9.1` (elapsed time between events in the app). The patcher will add `statvfs` or similar for disk space, which needs `E174.1`.

**For S2:** the test runs on the shipping binary, not the release one, since fat LTO strips differently. Scanning the binary's imports is enough to fail on an undeclared category; the per-rlib attribution is for a person reading the failure.

## 4. The Android bundle

### Gradle, offline

`cargo xtask bundle --platform android --app game_app --release` generates the project's inputs (the `.so`, SDL's Java, the activity and the mobile cook) and runs `assembleRelease`. xtask doesn't pass `--offline` to Gradle, so for this run the checkout's `gradlew` was edited to add it, and put back after. Through `slot --all --load 8`, 168 s with the library build, Gradle's part 2 s warm. Then, with the same `-Psapphire.*` properties xtask passes (`genDir`, `buildDir`, `ndkPath`, `ndkVersion`, `applicationId`, `activity`, `displayName`, `versionName`, `versionCode`):

```
cd platform/android
JAVA_HOME=<JDK 17> ANDROID_HOME=<SDK> ./gradlew --offline bundleRelease -Psapphire.…
JAVA_HOME=<JDK 17> ANDROID_HOME=<SDK> ./gradlew --offline lintRelease -Psapphire.…
```

- **`bundleRelease` passes offline,** from Gradle's cache alone: `app-release.aab`, 38.3 MB, signed with the debug key (the `release` build type's `signingConfig` today). Nothing was missing from the cache.
- **`lintRelease` runs offline too, and fails on errors:** 52 errors and 43 warnings, all but a few in SDL's Java glue, which xtask syncs from `sdl3-src` and never commits.
  - Errors: `MissingPermission` ×51 (`HIDDeviceBLESteamController.java` 40, `HIDDeviceManager.java` 11: Bluetooth LE calls behind SDL's own runtime checks) and `UnspecifiedRegisterReceiverFlag` ×1 (`HIDDeviceManager.java`'s pre-API-33 branch; the API 33 branch passes `RECEIVER_EXPORTED`, so it's a false positive at minSdk 29).
  - Warnings in SDL's Java: `ObsoleteSdkInt` ×31, `StaticFieldLeak` ×5.
  - Ours: `MissingApplicationIcon` (S1's icon), `DiscouragedApi` (`screenOrientation`, which Android 16 ignores on large screens, though `appCategory="game"` exempts us), `UnusedAttribute` (`enableOnBackInvokedCallback` below API 33, harmless), `OldTargetApi` and `GradleDependency` (compileSdk 37 exists), `ChromeOsAbiSupport` (no x86_64).

**For S3:** lint failing on error needs a `lint.xml` in `platform/android/app` that ignores SDL's two error rules under `**/org/libsdl/**` only, each with its reason, rather than a baseline, which would also hide our own new errors. Our manifest's findings get fixed or ignored one by one.

### bundletool and aapt2

Neither is on `PATH`. `aapt2`, `zipalign` and `apksigner` are in the SDK's `build-tools/36.0.0`. bundletool 1.18.3 is in Gradle's cache as AGP's dependency (`com.android.tools.build:bundletool`), not as the self-contained jar Google publishes. It runs with its dependencies from the same cache on the classpath: `aapt2-proto`, Guava with `failureaccess`, `protobuf-java` and `-util`, Gson, Dagger, `javax.inject`, jose4j, slf4j-api, auto-value and error-prone annotations, and `kotlin-stdlib` (its shaded ddmlib needs it for `install-apks`):

```
java -cp <those jars> com.android.tools.build.bundletool.BundleToolMain validate --bundle=app-release.aab
java -cp … BundleToolMain build-apks --bundle=app-release.aab --output=game.apks --local-testing --aapt2=<build-tools>/aapt2
java -cp … BundleToolMain get-size total --apks=game.apks
java -cp … BundleToolMain install-apks --apks=game.apks --device-id=emulator-<port> --adb=<SDK>/platform-tools/adb
```

- `validate` exits 0 and lists the base module: the content packs under `assets/content/`, four dex files, `lib/arm64-v8a/libgame_app.so` and the backup rules.
- `build-apks --local-testing` exits 0, signs with the debug keystore in the home directory's `.android/` (it says so), and makes two variants (base and arm64 config splits each). `get-size total`: 38,253,307 to 38,253,319 bytes to download, far under Play's 200 MB for the base.
- On every APK, `apksigner verify` passes (the Android Debug certificate), and `zipalign -c -P 16 -v 4` says `16384 lib/arm64-v8a/libgame_app.so (OK)`.
- The library's four `LOAD` segments align to `0x4000` (`llvm-readelf -lW`), so 16 KB pages hold. `.cargo/config.toml` also passes `-z max-page-size=16384`.
- `aapt2 dump badging`: `dev.sapphire.game`, versionCode 1, versionName 0.1.0, target SDK 36, label Sapphire, launchable `dev.sapphire.game.GameActivity`, Vulkan 1.1 required, landscape, `native-code: 'arm64-v8a'` on the config split.
- The AAB's library is AGP's stripped copy, 31.1 MB of the unstripped 226 MB, and the bundle carries no native symbols: `debugSymbolLevel` isn't set yet (§3 gives it to S3).

### On the emulator

Inside `slot --device`: a private `sapphire_api36` emulator (`-no-window -gpu host -read-only -no-snapshot-save -port 5598`), then `install-apks`, which pushed the API 32+ variant and its `local_testing` directory. `dumpsys package` shows splits `base` and `config.arm64_v8a`, versionCode 1, targetSdk 36 and `DEBUGGABLE` (the `release` build type is debuggable today, so `run-as` works). `am start -W -n dev.sapphire.game/.GameActivity --es sapphire.args <base64 of ["--capture","title.png","--size","640x360","--ticks","120"]>` returned in 159 ms, the app logged `main returned status=0`, and `run-as dev.sapphire.game cat cache/title.png` gave the same title frame as on macOS. The emulator was shut down with `adb emu kill`.

**For S1:** `xtask`'s Gradle call should pass `--offline` itself, so no agent's build can fetch an artifact.

**For S3:** the whole local chain works today with no download. Store builds differ from this one in what S3 changes: not debuggable (so a capture can't be fetched with `run-as`), signed with the upload key, `debugSymbolLevel`, and lint failing on error. Fetching bundletool's published all-in-one jar would remove the hand-made classpath, but the cache's copy is the one AGP itself uses. S3 should build its classpath the same way, or run bundletool through a Gradle task that resolves it offline.

## 5. A macOS bundle

`cargo xtask bundle --platform macos --app game_app --release` (182 s, desktop cook included), then on a copy:

```
codesign --force --sign - --options runtime --timestamp=none SapphireGame.app
codesign -dv SapphireGame.app            # flags=0x10002(adhoc,runtime), Runtime Version=26.2.0
codesign -d --entitlements - SapphireGame.app   # none
codesign --verify --strict --deep --verbose=2 SapphireGame.app   # valid on disk, satisfies its Designated Requirement
spctl --assess --type execute -vv SapphireGame.app   # rejected, as expected for ad hoc
SapphireGame.app/Contents/MacOS/game_app --capture title.png --size 640x360 --ticks 120
SapphireGame.app/Contents/MacOS/game_app --new-game --capture fight.png --size 640x360 --ticks 600
```

The copy also carried actool's `Assets.car` and `AppIcon.icns` in `Contents/Resources` and the two icon keys. Both runs exit 0 with a 640×360 capture (the title over the Lantern Ward, and 600 ticks of a new game, in 2.7 s), with nothing logged about signing or entitlements. The hardened runtime needs no entitlement: no JIT (Luau interprets), no unsigned executable memory, no library validation exemption (SDL is static, and nothing is loaded at run time).

**For S4:** sign with `--options runtime` and no entitlements file. Notarization needs a Developer ID identity and a secure timestamp, which are the maintainer's. What this doesn't show: the Steam overlay, which injects a dylib. If it needs `com.apple.security.cs.disable-library-validation`, that shows only under Steam.

## 6. Windows: manifest and icon

**What SDL does already:** SDL 3.4.18 declares per-monitor v2 awareness itself when its video subsystem starts (`WIN_InitDPIAwareness` in `SDL_windowsvideo.c`, unless the `SDL_WINDOWS_DPI_AWARENESS` hint says otherwise). Microsoft recommends the manifest over the API and requires the mode to be set before any window exists (learn.microsoft.com, "Setting the default DPI awareness for a process", read 2026-11-10). The manifest makes it true from process start, the crash monitor's re-executed process included, and carries what the API can't: `supportedOS` for Windows 10 and 11, the UTF-8 code page, long paths and `asInvoker`.

**The options:**

- `embed-resource` 3.0.12 (MIT): compiles a hand-written `.rc` in a build script and links it into the binaries. On a Windows MSVC host it finds `rc.exe` in the Windows SDK; cross-compiling for MSVC it uses `llvm-rc`. `manifest_required()` turns a missing compiler into a build error rather than an executable without its manifest.
- `winresource` and `tauri-winres` (forks of the unmaintained `winres`): they write the `.rc` from Cargo's metadata. Less to write, and less control over the manifest.
- No crate: link.exe's `/MANIFEST:EMBED /MANIFESTINPUT:` through `cargo:rustc-link-arg`. That covers the manifest only: an icon still needs a `.res`.

**Pick `embed-resource`,** in `game_app`'s build script, which returns early unless `CARGO_CFG_TARGET_OS` is `windows` (a build script's environment, not a `#[cfg(target_os)]`, so the layer rule holds). One `.rc` holds `1 24 "app.manifest"`, `1 ICON "app.ico"` and a `VERSIONINFO` whose numbers come in as macros from the build script: the workspace version and S1's build number (`cargo:rerun-if-env-changed`).

**What ran:** a scratch crate (`no_std`, so it links with no Windows SDK) with that build script, `.rc`, manifest (`dpiAware` `true/pm` and `dpiAwareness` `PerMonitorV2`, `supportedOS`, `activeCodePage` UTF-8, `longPathAware`, `asInvoker`) and a four-size ICO:

- `cargo check --target x86_64-pc-windows-msvc` with `RC_x86_64_pc_windows_msvc` pointing at `llvm-rc` passes. Without it the build script fails with `NotAttempted("llvm-rc")`, as `manifest_required()` should.
- `cargo build --release` with `rust-lld` as the linker links a PE32+ GUI executable. `llvm-readobj --coff-resources` lists `ICON` 1–4, `GROUP_ICON` 1, `VERSIONINFO` 1 and `MANIFEST` 1. The manifest text is in it, and `VS_FIXEDFILEINFO` reads 0.4.0.1234 for a version of 0.4.0 and `SAPPHIRE_BUILD_NUMBER=1234`.

**Where this Mac stops:** `cargo check -p game_app --target x86_64-pc-windows-msvc --locked` fails at `blake3`'s build script, which needs an MSVC C compiler for its assembly (`The C compiler "cc" does not support /arch:AVX512`), before SDL's CMake is reached. So the game with its resources builds only on CI's Windows runner, where `rc.exe` is found on its own. **S4** proves it there: the resources read back with `llvm-readobj` or PowerShell, and that link.exe's own manifest doesn't collide with the embedded one (rustc doesn't ask for one today).

**For S4's `embed-resource`:** cargo couldn't fetch the crate here (`--offline`, and the crate file isn't cached), so the spike built a copy of its source from cargo's registry with its Windows-host dependencies (`vswhom`, `winreg`) removed, which a Mac doesn't build anyway. S4 adds it through `[workspace.dependencies]` as usual.

## Steps only the maintainer's accounts can take

Written for the maintainer, with what this spike made as the input. `<TEAM>`, the identities and profiles are theirs.

1. **iOS export and upload.** In Xcode, Settings, Accounts, with the Apple Developer Program membership, an Apple Distribution certificate and an App Store provisioning profile for the bundle ID:
   ```
   xcodebuild -exportArchive -archivePath SapphireGame.xcarchive -exportPath export \
     -exportOptionsPlist ExportOptions.plist -allowProvisioningUpdates
   ```
   with `method` `app-store-connect`, `teamID` `<TEAM>`, `signingStyle` `automatic`, and `destination` `upload` to send it to App Store Connect, or `export` to make an `.ipa` to upload with Transporter. Or open the archive in Xcode's Organizer and choose Distribute App.
2. **macOS notarization** (for Steam): `codesign --force --sign "Developer ID Application: <name> (<TEAM>)" --options runtime --timestamp SapphireGame.app`, then `ditto -c -k --keepParent SapphireGame.app SapphireGame.zip`, `xcrun notarytool submit SapphireGame.zip --keychain-profile <profile> --wait`, and `xcrun stapler staple SapphireGame.app`.
3. **Play upload.** Generate an upload key (`keytool -genkeypair -v -keystore upload.jks -alias upload -keyalg RSA -keysize 4096 -validity 10000`), keep it offline, sign the AAB with it at package time through the environment, and upload it to the internal testing track in Play Console with Play App Signing on.
4. **Steam depots:** `steamcmd +login <account> +run_app_build app_build_<appid>.vdf +quit`, with the scripts S4 writes.
