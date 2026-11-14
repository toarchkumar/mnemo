# Releasing MNemo

One tag, `vX.Y.Z`, releases everything (the mobile packages on the rule
below):

- **the engine:** the CLI binaries and the Python wheels on the GitHub
  Release, the `mnemo-engine` crate on crates.io, and `mnemo-engine` on
  PyPI;
- **the Swift package:** the XCFramework zip on the GitHub Release, and the
  [`toarchkumar/mnemo-swift`](https://github.com/toarchkumar/mnemo-swift)
  mirror, tagged `X.Y.Z`;
- **the Android library:** `io.github.toarchkumar:mnemo-android` on Maven
  Central, and the AAR on the GitHub Release.

`.github/workflows/release.yml` does all of it, in this order (REL-2):

1. **`version check`:** every manifest agrees, with the tag on a tag run.
2. **Every build, in parallel:** the CLI binaries, the wheels, the sdist,
   and (with mobile on) the Swift and Android packages.
3. **`approve the release (vX.Y.Z, mobile true|false)`: the one human
   approval** (the `release` environment). It appears only once every
   build is green, so a failed build publishes nothing anywhere. Its name
   shows the version and the mobile mode the run read: check both before
   approving. Approve it there; nothing else asks. A dry run skips this
   job, and GitHub doesn't evaluate a skipped job's name, so a dry run
   shows it raw (`approve the release (v${{ needs.mobile-prepare...`).
4. **The GitHub Release and the engine publishes:** the Release and its
   assets, crates.io and PyPI.
5. **`engine published (gate)`:** the mobile packages publish only after
   every engine publish succeeded (a registry switched off with
   `ENABLE_CRATES_PUBLISH` or `ENABLE_PYPI_PUBLISH` counts as done). A
   mobile failure doesn't undo crates.io or PyPI: fix the cause and re-run
   the failed jobs.
6. **The mobile publishes:** `mnemo-swift` and Maven Central, then the
   published-package check.

**The mobile packages publish on a rule, not on every release** (user
decision, 2026-11-11). The Swift and Android packages are released only
when:

1. the mobile code changed (the bindings, the stores, the Keystore or
   Keychain code);
2. an engine fix matters on phones (security, data loss, corruption); or
3. a minor version is released: every minor version publishes mobile at
   least once, so mobile is never more than one minor version behind.

Otherwise a release publishes only crates.io, PyPI and the CLI binaries.
Every manifest stays on one version number, so the mobile version numbers
skip; each package carries the engine version it was built from.

**The release commit sets the mode** (RB-004.4, from v0.13.2): the file
`.github/release-mobile` says `true` if the rule above says this release
publishes mobile, and `false` if it doesn't. The tag run reads it from the
tagged commit, so the mode is reviewed with the release, and no re-run can
turn it on (a re-run does read the off switch below again, so it can turn
mobile off). `version check` refuses anything but `true` or `false`,
prints the mode and where it came from, and writes what the commit says to
the run summary. With mobile off, a tag skips the
Swift and Android build, publish and verify jobs, and the GitHub Release
says "Mobile packages not published in this release", with the newest
published versions. The version check still runs, and still requires one
version in every manifest. A dry run picks with its own `mobile` input
instead (Actions > Release > Run workflow), so you can rehearse either kind
of release; when the input differs from the file, `version check` warns,
since a tag of that commit will follow the file.

**`ENABLE_MOBILE_PUBLISH` is only an off switch** (Settings > Secrets and
variables > Actions > Variables). Set to `false` (in any case), it turns
mobile off for every tag, whatever the commit says (for example while a
mobile credential is broken). Otherwise leave it `true` or unset: there's
nothing to flip before or after a tag. Any other value (`off`, `0`, a
stray space) fails `version check`, on a dry run too, so nothing builds
until it's fixed.

Every job has a 45-minute limit, and every step a limit of 10 to 20
minutes, so a hang fails the step instead of running for hours.

**Build and publish are separate jobs.** The jobs that compile (the CLI,
the wheels, the sdist, the XCFramework, the Android libraries) have a
read-only token and no secrets, and hand their output to small jobs that
attach or publish it:
- `attach engine assets` puts the CLI binaries, wheels and sdist on the
  Release;
- `publish to crates.io` and `publish to PyPI` upload;
- `mobile (Swift publish)` attaches the zip and pushes the mirror;
- `mobile (Swift verify published)` resolves the published `mnemo-swift`
  version from GitHub and builds against it. It's a job of its own, with no
  secrets, so after a slow runner it can be re-run alone without pushing
  again;
- `mobile (Android publish)` signs, uploads to Maven Central and attaches
  the AAR.

The publish jobs run in the environments `crates-io`, `pypi`, `mnemo-swift`
and `maven-central`, and each secret is visible only to the step that uses
it. Every publish also waits for `approve the release`. Every action is
pinned to a commit (Dependabot keeps them current).

## Before the tag

1. **Versions and the mobile mode.** Bump every one of these to `X.Y.Z`.
   The workflow refuses a tag that disagrees with them. Set
   `.github/release-mobile` to `true` or `false` by the rule above.
   - the engine and Python manifests and their lockfiles;
   - Node's `Cargo.toml`;
   - `mnemo-uniffi`;
   - `VERSION_NAME` in `mnemo/bindings/kotlin/gradle.properties`;
   - the samples (`MARKETING_VERSION` in `examples/mobile/ios/project.yml`,
     and the `mnemo-android` version in the Android sample).
2. **The changelog.** Cut `[Unreleased]` into `[X.Y.Z] - <date>`.
3. **Install lines.** The Swift and Kotlin package READMEs
   (`mnemo/bindings/*/README.md`) name the new version. On a release that
   leaves mobile out, they keep the newest published version, since the
   new one won't exist on `mnemo-swift` or Maven Central (the samples build
   the libraries from this repository, so they still take the new version).
4. **CI is green** on the release commit.
5. **The dry run.** It publishes nothing and never waits for an approval,
   so run it on the release branch, with `mobile` set as
   `.github/release-mobile` says (the run summary prints what the commit
   says, and `version check` warns if the input differs):

   ```bash
   gh workflow run release.yml --ref <release branch> -f dry_run=true -f mobile=true
   ```

   It checks:
   - **the engine artifacts (REL-2):** builds the five CLI binaries, the
     five wheels and the sdist exactly as a tag does; the attach job
     downloads them with the tag's pattern and lists the 11 files; the
     crates.io job runs `cargo publish --dry-run`; the PyPI job counts 5
     wheels and 1 sdist. Only the upload steps and `action-gh-release`
     are left out;
   - **the Swift package:** builds the XCFramework zip and its checksum,
     then runs an SPM consumer on the iOS simulator against the
     `mnemo-swift` tree, using that zip;
   - **the `mnemo-swift` token:** pushes and deletes a `dry-run-X.Y.Z`
     branch (or runs `git push --dry-run` while the repository is empty);
   - **the published-package check:** resolves the newest version
     `mnemo-swift` already has from GitHub and builds it for the
     simulator, the same two steps a tag runs on the new version
     (`scripts/check-published-package.sh`);
   - **the Android library:** runs a signed `publishToMavenLocal` and a
     Gradle consumer from `mavenLocal()`;
   - **Maven Central:** uploads a deployment that the Central Portal
     validates, then drops (`scripts/central-dry-run.sh`). Central
     refuses a version it already has, so a dry run on a version that's
     already published (between releases, to test a workflow change)
     skips this step with a notice.
6. **The coordination files.** Commit `PLAN.md` and `reviewer_notes.md` as
   `chore(plan): sync coordination files` (see AGENTS.md).

## The tag

```bash
git tag -a vX.Y.Z -m "MNemo X.Y.Z"
```

```bash
git push origin vX.Y.Z
```

## After the tag

Check all four registries:

- **crates.io:** `cargo search mnemo-engine` shows `X.Y.Z`.
- **PyPI:** `pip index versions mnemo-engine` lists `X.Y.Z`.
- **SPM:** `mnemo-swift` has the tag `X.Y.Z`, and the workflow's
  `mobile (Swift verify published)` job succeeded for it.
- **Maven Central:** the version is on
  `https://central.sonatype.com/artifact/io.github.toarchkumar/mnemo-android`.
  The search index can lag the repository by up to an hour;
  `https://repo1.maven.org/maven2/io/github/toarchkumar/mnemo-android/X.Y.Z/`
  shows it first.

**If a mobile job fails after the engine jobs succeeded,** re-run only the
failed jobs:

- **Swift:** the push step (in `mobile (Swift publish)`) refuses a tag
  that `mnemo-swift` already has. If the mirror was tagged but
  `mobile (Swift verify published)` failed (a slow runner, say), re-run
  that job alone: it pushes nothing. If the published package itself is
  broken, the release is out: fix the cause forward, in the next version.
- **Maven Central** rejects a version it already has. If the deployment
  reached `PUBLISHED`, the release is out; if it failed validation, drop
  it in the Portal (Deployments) and re-run.

## Credentials

They're secrets on `toarchkumar/mnemo`: repository secrets, or secrets of
the environment that uses them, which is better (a branch's copy of the
workflow then can't read them). Renew them before they lapse, or a release
fails at its publish step.

| Secret | Environment | What | Renew by |
|---|---|---|---|
| `MAVEN_CENTRAL_USERNAME`, `MAVEN_CENTRAL_PASSWORD` | `maven-central` | the Central Portal user token `mnemo-github-actions` | **2027-11-09**, when it expires: make a new token in the Portal (View Account > Generate User Token) and update both secrets |
| `MAVEN_SIGNING_KEY`, `MAVEN_SIGNING_KEY_PASSWORD` | `maven-central` | the GPG signing key (RSA 4096, fingerprint `CFAFF7A174EBC15ED5475C4619104D9B3DA89A04`) | **before 2028-11-08**, when it expires: extend it (`gpg --edit-key CFAFF7A174EBC15ED5475C4619104D9B3DA89A04`, then `expire`) and re-send it with `gpg --keyserver keyserver.ubuntu.com --send-keys CFAFF7A174EBC15ED5475C4619104D9B3DA89A04`. The key stays the same, so the secrets don't change unless you replace it |
| `MNEMO_SWIFT_DEPLOY_KEY` | `mnemo-swift` | the private half of an SSH deploy key with write access on `toarchkumar/mnemo-swift`. When it's set, the push uses it | doesn't expire; replace it if it leaks |
| `MNEMO_SWIFT_TOKEN` | `mnemo-swift` | a GitHub token with write access to `toarchkumar/mnemo-swift`, used only while `MNEMO_SWIFT_DEPLOY_KEY` isn't set | **2027-11**, or delete it once the deploy key is in |
| `CRATES_IO_TOKEN` | `crates-io` | the crates.io API token, used only while the repository variable `CRATES_TRUSTED_PUBLISHING` isn't `true` | when crates.io says it expires, or delete it once Trusted Publishing is on |

PyPI needs no secret: it uses Trusted Publishing (the `pypi` environment).
crates.io does the same once the trusted publisher is added on crates.io
(repository `toarchkumar/mnemo`, workflow `release.yml`, environment
`crates-io`) and the variable `CRATES_TRUSTED_PUBLISHING` is `true`.

**The environments (REL-2: one approval per release).**
- `release`: the required reviewer (you), and a deployment rule allowing
  only `v*` tags. Only `approve the release` uses it, on a tag, once every
  build is green; every publish waits for it.
- `crates-io`, `pypi`, `mnemo-swift` and `maven-central`: the secrets (and,
  for crates.io and PyPI, the Trusted Publishing claim), and deployment
  rules allowing `v*` tags and the release branches the dry run runs from.
  **No required reviewer:** the `release` approval covers them, and dry
  runs don't pause.
- **The trade-off:** a dry run on a `release-*` branch can use the registry
  secrets without a click. Only your account can push those branches, and
  every real publish still waits for you. An agent never approves a
  deployment.

**The mirror.** A tag ruleset on `toarchkumar/mnemo-swift` that blocks
updating and deleting tags keeps a published version from being moved.
The deploy key still creates new tags.

The Maven Central namespace `io.github.toarchkumar` is verified once and
stays verified.
