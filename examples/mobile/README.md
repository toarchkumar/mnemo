# MNemo sample apps

Two small apps, one per platform, built against the packages in this repo
(`mnemo/bindings/swift` and `mnemo/bindings/kotlin`, not a release):

| | iOS (`ios/`) | Android (`android/`) |
|---|---|---|
| UI | SwiftUI, XcodeGen project | Jetpack Compose, Gradle |
| Notes demo | notes, a keyed fact with history, recall with and without keywords, a thread | the same |
| Embedder | Apple's built-in `NLEmbedding` sentence embedding (no model in the app) | a **demo** embedder that hashes words (see below) |
| Benchmark | the `device_budget` cells at 1K and 10K, JSON with a Share button | the same |
| Release check | the background test (`0xdead10cc`), the file's protection class | a self-test of the R8-minified release build (run by CI) |

**Why the Android demo embedder:** MediaPipe's Text Embedder would cost more
than the 10 MB the sample may add. The current `tasks-text` (1.2.2) ships a
16.6 MB arm64 native library, and the older 7.7 MB one (0.10.20) isn't
aligned for 16 KB pages. An app plugs in its own model the same way the
demo does, by implementing `MnemoEmbedder`. The Benchmark doesn't use an
embedder at all: it measures synthetic 384-dim vectors.

This page is the run sheet for the on-device checks: install, run the
Benchmark, send the JSON back, and run the iOS background test.

## iPhone

### One-time setup on the Mac

1. Finish Xcode's setup. Each `sudo` command asks for your password:

   ```bash
   sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
   ```

   ```bash
   sudo xcodebuild -license accept
   ```

   ```bash
   sudo xcodebuild -runFirstLaunch
   ```

   ```bash
   xcodebuild -downloadPlatform iOS
   ```

2. Install XcodeGen and the Rust iOS targets:

   ```bash
   brew install xcodegen
   ```

   ```bash
   rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios aarch64-apple-darwin
   ```

3. Build the XCFramework (again after any engine change):

   ```bash
   mnemo/bindings/swift/scripts/build-xcframework.sh
   ```

### Signing: a free Apple ID works

Sign in at **Xcode > Settings > Accounts** with your Apple ID. A free account
appears as a *Personal Team*.

- **App Groups work with a free account.** The background test needs
  them, and Apple's
  [capabilities table](https://developer.apple.com/help/account/reference/supported-capabilities-ios)
  ticks App Groups for "Apple Developer" (free) accounts as well as the
  paid programs.
- **The group must be unique to your team.** It's derived from the bundle
  id (`group.<bundle id>`), so choose a bundle id of your own.
- **A free account's limits:** the app stops launching after 7 days
  (build and install it again), and at most 3 such apps fit on a device.

Then generate the project:

```bash
cd examples/mobile/ios && cp Local.xcconfig.example Local.xcconfig
```

1. Edit `Local.xcconfig`: set `DEVELOPMENT_TEAM` to your team id and
   `MNEMO_BUNDLE_ID` to a bundle id of your own (for example
   `com.yourname.mnemo-sample`). If you don't know the team id, pick the
   team once in the target's **Signing & Capabilities** tab and copy the
   value Xcode shows.
2. Generate and open the project:

   ```bash
   xcodegen generate && open MnemoSample.xcodeproj
   ```

3. Select the **MnemoSample** target, then **Signing & Capabilities**:
   - check that your team is selected;
   - check that **App Groups** lists `group.<your bundle id>` with a tick;
   - if Xcode shows a red error, press its **Try Again** or **+** so it
     registers the group.

`Local.xcconfig` is gitignored, and `xcodegen generate` never overwrites
it.

### Install on the iPhone

1. **Developer Mode:** connect the iPhone by USB and pick it as the run
   destination in Xcode. On the iPhone, go to **Settings > Privacy &
   Security > Developer Mode**, turn it on, and let it restart.
2. **A Release build** (the Benchmark needs it):
   1. Open **Product > Scheme > Edit Scheme... > Run**.
   2. Set **Build Configuration** to **Release**.
   3. Press **Run** (Cmd-R).
3. **The first time with a free account,** iOS refuses to open the app.
   Trust your certificate under **Settings > General > VPN & Device
   Management**, then run it again.

### Benchmark

1. **Get the phone ready:**
   - unplug it and charge it above 50%;
   - turn Low Power Mode off;
   - close other apps;
   - let it cool if it's warm.

   The JSON records each of these, so a run that isn't ideal is still
   usable. Just say what was different.
2. **Run it:** open the **Benchmark** tab and tap **Run the benchmark**.
   - Keep the app in the foreground; the screen stays on.
   - The first run builds the two corpora (a minute or two). Later runs
     reuse them.
3. **Send the JSON:**
   - when it finishes, tap **Share the JSON** (AirDrop it to the Mac, or
     save it to Files);
   - or find it in the **Files** app under **On My iPhone > MNemo**;
   - name it after the device, for example `iphone16_1.json`, and put it
     in `test/scripts/device_runs/` (or just send it, and the Worker files
     it);
   - the results go into `perf_v0.5.0.json` as `device_budget_mobile`.

### Background test (`0xdead10cc`)

This check gates v0.12. iOS kills an app that's suspended while holding a
lock on a file in a shared (App Group) container. The crash log shows the
exception code `0xdead10cc`. `MnemoStore` flushes and closes its file when
the app enters the background, and this test checks that it does so on
real hardware.

1. Open the **Background** tab and check the first line:
   - it should say **App Group container**;
   - if it says *Application Support*, the App Group isn't enabled for
     your team (see Signing above). The test still runs, but it's the
     weaker form.
2. **File protection should read
   `NSFileProtectionCompleteUntilFirstUserAuthentication`** for the file
   and its directory. That's the second release check.
3. **Run the test:**
   1. Tap **Start the write loop** and let it write for 10 seconds or
      so.
   2. Go to the Home Screen and lock the phone.
   3. Wait at least 30 seconds; a minute or two is better.
   4. Unlock and go back to the app.

   **Pass:** the app resumes where it was, without relaunching. Its log
   shows "back in the foreground after N s" and then "stored N, wrote N"
   with no "WRITES MISSING".

   **Fail:** the app relaunches, with an empty log and its counters at 0.
4. **Check the device logs either way.** A kill can go unnoticed if you
   reopen the app later.
   - **On the Mac:** connect the iPhone, open **Console.app**, select the
     iPhone in the sidebar, and choose **Crash Reports**. (Or use
     **Xcode > Window > Devices and Simulators > Open Recent Logs**.)
   - **On the iPhone:** **Settings > Privacy & Security > Analytics &
     Improvements > Analytics Data**. If the list is empty, turn on
     **Share iPhone Analytics** on the screen before it, then run the
     test again.
   - **Look for an entry for `MnemoSample`** from the time of the test.
     Open it and search for `dead10cc`. "No entry", or entries without
     `0xdead10cc`, is a pass.
5. **Report:**
   - pass or fail;
   - the protection class shown;
   - whether the App Group was used;
   - any `MnemoSample` crash entry, as a file or a screenshot.

## Android phone

### Build the APK

Either take it from CI, or build it on the Mac.

- **From CI:** open the latest green run of the **CI** workflow, download
  the `mnemo-android` artifact, and use
  `examples/mobile/android/app/build/outputs/apk/release/app-release.apk`.
  It's signed with a debug key, so it installs directly.
- **On the Mac:**
  - you need a JDK 17, the Android SDK with the NDK, Rust's Android
    targets and `cargo-ndk` (see `mnemo/bindings/kotlin/README.md`);
  - accepting the Android SDK licence is yours to do;
  - then run:

  ```bash
  mnemo/bindings/kotlin/scripts/build-native.sh
  ```

  ```bash
  gradle -p examples/mobile/android assembleRelease
  ```

  Android Studio works as well: open `examples/mobile/android`.

### Install

1. **USB debugging:** on the phone, tap **Settings > About phone > Build
   number** seven times. Then turn on **Settings > System > Developer
   options > USB debugging**.
2. **Connect the phone** and accept the prompt on it.
3. **Install the APK:**

   ```bash
   adb install -r app-release.apk
   ```

### Benchmark

1. **Get the phone ready,** as on iOS: unplugged, charged, Battery Saver
   off, cool.
2. **Run it:** open the **Benchmark** tab and tap **Run the benchmark**.
3. **Send the JSON:**
   - tap **Share the JSON**, or pull it over USB:

     ```bash
     adb pull /sdcard/Android/data/io.github.toarchkumar.mnemo.sample/files/device_budget/ .
     ```

   - name it after the device (for example `pixel8.json`), and put it in
     `test/scripts/device_runs/`;
   - the JSON records the page size (4 KB or 16 KB).

### The R8 check

The release APK is minified with R8, with no rules of its own. The
library's `consumer-rules.pro` has to keep what JNA and the bindings
need.

- **In CI:** the nightly `android-emulator` workflow installs the APK and
  runs its self-test.
- **By hand:**

  ```bash
  adb shell am start -n io.github.toarchkumar.mnemo.sample/.MainActivity --ez selftest true
  ```

  ```bash
  adb logcat -d -s MnemoSelfTest:I
  ```

  The second command should print `MNEMO_SELFTEST ok ...`.

## The JSON

The keys follow `mnemo/examples/device_budget.rs`. There's one block per
size (`"1000"`, `"10000"`), with these phases:

- `build`;
- `file`: bytes per memory, as built and compacted;
- `handle`: one handle's memory, the first recall, and 1,000 warm recalls
  (median and p95);
- `cycles`: 20 fresh handles;
- `keyopen`: raw-key opens;
- `writes`: durable single writes, and batches of 100;
- `background`: the background close and the reopen after it.

Alongside them, `device` and `build` hold the fields from design note Q7.

**Differences from the desktop bench** are listed in the JSON under
`corpus`:

- every open uses a raw key, so there's no KDF;
- the mobile API builds no IVF+PQ index, so recall reads the vector
  column;
- `layers`, the secure-KDF open and the fsync counts aren't measured.

**Memory is measured per platform:**

- iOS: `phys_footprint`;
- Android: `VmRSS`, plus total PSS in `handle_pss_kb`.
