# Device runs

Measurements from real phones, made with the sample apps' **Benchmark**
tab. Each file is one run on one device, in the `device_budget_mobile/1`
schema below. Anyone with an iPhone or an Android phone can add one: the
more devices, the better the phone budgets in `perf_v0.5.0.json` get.

## Submit a run

1. **Build and install a sample app,** as a Release build: the iOS sample
   (`examples/mobile/ios/`) or the Android sample (`examples/mobile/android/`).
   [`examples/mobile/README.md`](../../../examples/mobile/README.md) has the
   steps for each.
2. **Get the phone ready:** unplugged, charged above 50%, Low Power Mode
   (Battery Saver) off, other apps closed, not warm. A run that isn't ideal
   is still useful: the JSON records each of these, and the validator warns
   about them.
3. **Run it:** open the **Benchmark** tab and tap **Run the benchmark**.
   Keep the app in the foreground. The first run builds the corpora, which
   takes a minute or two.
4. **Share it:** tap **Share results**, and AirDrop or save the JSON. On
   Android you can also pull it with `adb pull` (see the mobile README).
5. **Name it after the device,** in lowercase with underscores: the iPhone's
   model identifier (`iPhone16,1` becomes `iphone16_1.json`) or the Android
   model (`pixel8.json`). Add a suffix for a second run on the same model
   (`pixel8_android16.json`).
6. **Check it:**

   ```bash
   python3 test/scripts/device_runs/validate.py test/scripts/device_runs/iphone16_1.json
   ```

7. **Open a pull request** that adds the file to this folder. CI runs the
   validator on every file here. If you'd rather not open a PR, attach the
   JSON to an issue and say which device it's from.

Don't edit the numbers by hand. If a run went wrong (the app was sent to
the background, the phone got hot), run it again.

## The schema: `device_budget_mobile/1`

A JSON object. The validator checks the fields marked **required**; the
others are recorded where the platform has them.

| Field | Type | |
|---|---|---|
| `schema` | string | **required**, `"device_budget_mobile/1"` |
| `timestamp` | string | **required**, ISO 8601, when the run ended |
| `elapsed_secs` | number | **required**, the whole run |
| `device` | object | **required**, see below |
| `build` | object | **required**, see below |
| `corpus` | object | **required**, see below |
| `results` | object | **required**, one cell per corpus size, keyed by the size (`"1000"`, `"10000"`) |

**`device`:**
- **Required:**
  - `platform` (`"ios"` or `"android"`);
  - `model_identifier` (`iPhone16,1`; on Android `Build.MODEL`);
  - `os`, `os_version`;
  - `ram_bytes`, `cpu_count`;
  - `page_size` (16384 on iPhones; 4096 or 16384 on Android).
- **Recorded where the platform has them:**
  - `soc`, `os_build`;
  - `simulator` (iOS) or `emulator` (Android);
  - `battery_level`, `battery_state`, `thermal_state`, `low_power_mode`;
  - `free_storage_bytes`;
  - on Android, `manufacturer`, `device`, `abi`, `sdk_int` and
    `os_fingerprint`.

**`build`:**
- **Required:** `mnemo_version` (`MAJOR.MINOR.PATCH`), `configuration`,
  `release_build` (bool) and `commit`.
- **Recorded:** `dirty`, and `xcode` on iOS.

**`corpus`:**
- **Required:** `dimensions` (384) and `text_bytes` (300).
- **Recorded:** `generator`, `key`, `index` and `not_measured`, which
  describe how the corpus was made and what the run doesn't measure.

**Each `results` cell** holds one object per phase. Times are in
milliseconds and memory is in KB. All of these are required non-negative
numbers:

| Phase | Fields |
|---|---|
| `file` | `memories` (equals the cell's size), `file_bytes`, `bytes_per_memory`, `format_version` |
| `handle` | `open_ms`, `first_recall_ms`, `warm_recall_median_ms`, `warm_recall_p95_ms`, `handle_rss_kb` |
| `cycles` | `cycles`, `open_median_ms`, `open_p95_ms`, `first_recall_median_ms`, `first_recall_p95_ms` |
| `writes` | `single_write_median_ms`, `single_write_p95_ms`, `batch_size`, `batch_write_median_ms`, `batch_write_p95_ms` |
| `keyopen` | `open_with_key_median_ms`, `open_with_key_p95_ms` |
| `background` | `cycles`, `close_median_ms`, `close_p95_ms`, `reopen_median_ms`, `reopen_p95_ms` |

The cells also record other numbers, which the validator doesn't check:
- `rss_*` and `pss_*` snapshots, and `rss_metric`;
- the compacted size, under `file`;
- the corpus `build` time;
- an empty `layers` object.

A change to these fields is a new schema version (`device_budget_mobile/2`):
the generators are `examples/mobile/ios/MnemoSample/Benchmark.swift` and
`examples/mobile/android/app/src/main/kotlin/io/github/toarchkumar/mnemo/sample/Benchmark.kt`.
Change them together, and update `validate.py` and this file.
