# Google Play readiness for M4

A scout's read on 2026-11-10, web only, nothing built or run. "ours:" marks our own inference. "ours (memory):" marks facts recalled from before this search and not re-checked against a source today: verify them before relying on them.

## 1. Target API level

- New apps and app updates on the phone track must target Android 16 (API 36) by 2026-08-31. Existing apps must target API 35 or higher to stay available to new users on newer devices. Source: https://developer.android.com/google/play/requirements/target-sdk and https://support.google.com/googleplay/android-developer/answer/11926878
- Third-party sources say an extension to 2026-11-01 could be requested in Play Console. That date has passed. Source: https://www.appsonair.com/blogs/android-16-api-level-36-target-sdk-requirement-what-you-need-to-do-before-august-31
- The yearly pattern is API 34 in 2024, API 35 in 2025 and API 36 in 2026. Source: https://dev.to/mr_manushukla/android-target-api-36-is-due-aug-31-2026-api-37-is-next-the-play-store-migration-checklist-2f9i
- ours: expect API 37 (Android 17) to be required for new apps and updates around 2027-08-31. Google hadn't announced it in what we found. Our targetSdk 36 is compliant today. Plan the bump to 37 for mid-2027 and track it in the M4 handoff.

## 2. 16 KB page size

- Apps that target Android 15 or higher and ship native code had to support 16 KB pages from 2025-11-01. Approved extensions ran to about 2026-05-30 or 05-31; the two sources disagree on the day. Sources: https://forums.unrealengine.com/t/app-must-support-16-kb-memory-page-sizes/2655548 and https://knowledge.broadcom.com/external/article/411558/guidance-on-google-plays-16-kb-page-size.html
- Two things need checking. The ELF LOAD segments must be 16 KB aligned (`-Wl,-z,max-page-size=16384`). Uncompressed `.so` files must also sit 16 KB-aligned inside the zip. Source: https://github.com/expo/fyi/blob/main/android-16kb-page-sizes.md and https://median.co/blog/how-to-prepare-android-apps-google-play-16-kb-page-size-requirement
- ours (memory): NDK r28 and later link with 16 KB alignment by default, so r29 is fine for C/C++ objects. Our cdylib is linked by rustc through the NDK clang. Check its `p_align` with `llvm-readelf -l` or `readelf -lW`; don't assume it. Prebuilt static SDL3 objects are linked into the same `.so`, so they're covered.
- ours (memory): AGP 8.5.1 and later zip-align uncompressed `.so` files to 16 KB in both APKs and AABs. AGP 9.4 qualifies.
- ours (memory): local checks need no account:
  - `zipalign -c -P 16 -v 4 app.apk` from build-tools 35 and later;
  - Google's `check_elf_alignment.sh` from the 16 KB guide;
  - Android Studio's APK Analyzer, which flags misaligned libraries;
  - `bundletool` runs the same check when it builds APKs from an AAB.
- ours: run these as a CI gate on the release artifact.

## 3. AAB and Play Asset Delivery

- ours (memory): new apps on Play must ship as an AAB, a rule in force since 2021-08. The compressed download of the base plus install-time modules is capped at 200 MB. Asset packs don't count toward that cap. Source to check: https://developer.android.com/guide/app-bundle
- Per Google's PAD page:
  - each fast-follow or on-demand pack may be up to 512 MB of download;
  - all install-time packs together may be up to 1 GB;
  - all asset packs in a bundle may total up to 2 GB, across at most 50 packs;
  - the Play Partner Program for Games allows higher limits.

  Source: https://developer.android.com/guide/playcore/asset-delivery. Epic's table differs on whether 1 GB applies per pack or in total: https://dev.epicgames.com/documentation/en-us/unreal-engine/using-google-play-asset-delivery-in-unreal-engine
- The delivery modes, per the same page:
  - install-time packs arrive with the app and are read through `AAssetManager`, just like APK assets today;
  - fast-follow packs download right after install, with no app code needed to trigger them;
  - on-demand packs are fetched by the app through the Play Asset Delivery API (Java or native).
- ours (memory): Play allows a game to download extra content from its own CDN. Large live-service games commonly do this; Play's Device and Network Abuse policy forbids downloading executable code, not data. ours: cooked packs are data, so a CDN fits. The cost is our own integrity and resume logic, plus disclosing the downloads on the Data safety form.
- ours: the M4 path is install-time asset packs for the cooked content. This keeps the engine's `AAssetManager` read path, and the native `.so` stays in a base module well under 200 MB.

## 4. Testing tracks and accounts

- A new personal developer account (created after 2023-11-13) must run a closed test with at least 12 opted-in testers for 14 consecutive days before Production unlocks. Organization accounts are exempt. The count dropped from 20 to 12 in late 2024. Sources: https://support.google.com/googleplay/android-developer/answer/14151465 and https://www.choicely.com/blog/google-play-12-tester-rule. One guide says the rule applies per app (unverified): https://www.testerscommunity.com/blog/google-play-closed-testing-requirements-2026
- ours (memory): the internal testing track allows up to 100 testers and is available right away, even on new personal accounts. Builds skip full review and usually appear within minutes.
- ours (memory): uploading an AAB requires Play App Signing. Google holds the app signing key, and we sign uploads with an upload key that we generate locally. The keystore and its password must stay out of the repo and out of agents' hands.
- ours (memory): Android developer verification is rolling out for sideloaded apps in 2026-2027. It doesn't change Play uploads, which already verify the developer. Source to check: https://ontest.app/blog/android-developer-verification-google-play-closed-testing-2026

## 5. Policy declarations in Play Console

- ours (memory): before release, Play Console's App content page requires:
  - a privacy policy URL;
  - the ads declaration;
  - app access (login instructions for reviewers);
  - target audience and content;
  - the content rating, an IARC questionnaire that issues ESRB, PEGI and other ratings;
  - the Data safety form;
  - government, financial and health declarations where they apply.
- ours: we can draft these in the repo as plain text ahead of time: answers for the IARC questionnaire (fantasy violence, no gambling, no user-generated content), Data safety answers (what crash reports and telemetry collect and whether they're shared), the privacy policy text, and the target audience (13+ or 18+; under 13 brings in the Families policy). The account holder enters them; agents never sign in.
- ours: if crash upload or analytics exist, the Data safety form must list them. Third-party SDKs count too. An engine with no SDKs keeps the form short.

## 6. Local validation with no account

- ours (memory), tools already installed:
  - `bundletool validate --bundle=app.aab`;
  - `bundletool build-apks --bundle app.aab --output app.apks --local-testing`, then `install-apks` onto the emulator, which also exercises asset-pack layout;
  - `bundletool get-size total`, to compare against the 200 MB cap.
- ours (memory): `apksigner verify --print-certs -v` (v2, v3 and v4 schemes), `zipalign -c -P 16 4`, and `aapt2 dump badging` (package, versionCode, sdkVersion, targetSdkVersion, native-code, uses-feature for Vulkan).
- ours (memory): Gradle `lint` includes `ExpiredTargetSdkVersion` and `OldTargetApi`, which warn as Play's deadline nears, plus manifest checks. Run it with `abortOnError` on a release variant.
- ours: Play's pre-launch report runs devices in Google's lab and needs an account, so it isn't local. The emulator plus bundletool's `--local-testing` mode is the nearest stand-in.

## 7. Android 15 and 16 behaviour that hits an SDL landscape game

- Edge-to-edge (ours, memory): from targetSdk 35, windows are edge-to-edge by default. At targetSdk 36 the `windowOptOutEdgeToEdgeEnforcement` opt-out is removed. ours: SDL already runs immersive full-screen. Test that HUD safe areas handle the cutout insets SDL reports, on a punch-hole emulator skin.
- Predictive back (ours, memory): at targetSdk 36, back gestures go to `OnBackInvokedCallback` by default, and `onBackPressed` and `KEYCODE_BACK` stop being called unless the app opts out with `android:enableOnBackInvokedCallback="false"`. ours: check whether SDL3's `SDLActivity` handles this. If it doesn't, the back gesture may quit the game instead of reaching our pause menu. Test on the emulator.
- Large screens: on displays whose shorter side is 600 dp or more, Android 16 ignores the orientation, resizability and aspect-ratio locks of apps that target 36. A per-activity opt-out exists at 36 and goes away at 37. Sources: https://developer.android.com/about/versions/16/behavior-changes-16 and https://android-developers.googleblog.com/2025/01/orientation-and-resizability-changes-in-android-16.html
- Games declared with `android:appCategory="game"` are exempt. Sources: https://developer.android.com/develop/adaptive-apps/guides/app-orientation-aspect-ratio-resizability and https://docs.unity3d.com/Manual/android-large-screen-and-foldable-support.html. ours: set `appCategory="game"` in our manifest. That's cheap insurance, and Game Mode also keys on it. We didn't confirm that the exemption survives at API 37.
