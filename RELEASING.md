# Releasing MNemo

One tag, `vX.Y.Z`, releases everything:

- **the engine:** the CLI binaries and the Python wheels on the GitHub
  Release, the `mnemo-engine` crate on crates.io, and `mnemo-engine` on
  PyPI;
- **the Swift package:** the XCFramework zip on the GitHub Release, and the
  [`toarchkumar/mnemo-swift`](https://github.com/toarchkumar/mnemo-swift)
  mirror, tagged `X.Y.Z`;
- **the Android library:** `io.github.toarchkumar:mnemo-android` on Maven
  Central, and the AAR on the GitHub Release.

`.github/workflows/release.yml` does all of it. The mobile jobs run only
after every engine job succeeded: the GitHub Release, the CLI binaries,
the wheels, the sdist, and both publish jobs. The one exception is a
registry switched off with its `ENABLE_CRATES_PUBLISH` or
`ENABLE_PYPI_PUBLISH` variable: its publish job is skipped, and mobile
still runs. The `mobile (version check)` job checks this and fails with
each job's result if anything else happened. A mobile failure doesn't
undo crates.io or PyPI: fix the cause and re-run the failed jobs.

Every job has a 45-minute limit, and every step a limit of 10 to 20
minutes, so a hang fails the step instead of running for hours.

## Before the tag

1. **Versions.** Bump every one of these to `X.Y.Z`. The workflow refuses a
   tag that disagrees with them.
   - the engine and Python manifests and their lockfiles;
   - Node's `Cargo.toml`;
   - `mnemo-uniffi`;
   - `VERSION_NAME` in `mnemo/bindings/kotlin/gradle.properties`;
   - the samples (`MARKETING_VERSION` in `examples/mobile/ios/project.yml`,
     and the `mnemo-android` version in the Android sample).
2. **The changelog.** Cut `[Unreleased]` into `[X.Y.Z] - <date>`.
3. **Install lines.** The Swift and Kotlin package READMEs
   (`mnemo/bindings/*/README.md`) name the new version.
4. **CI is green** on the release commit.
5. **The dry run.** It publishes nothing, so run it on the release branch:

   ```bash
   gh workflow run release.yml --ref <release branch> -f dry_run=true
   ```

   It checks:
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
     validates, then drops (`scripts/central-dry-run.sh`).
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
- **SPM:** `mnemo-swift` has the tag `X.Y.Z`, and the workflow's "Resolve
  the published package" and "Build against the published package" steps
  succeeded for it.
- **Maven Central:** the version is on
  `https://central.sonatype.com/artifact/io.github.toarchkumar/mnemo-android`.
  The search index can lag the repository by up to an hour;
  `https://repo1.maven.org/maven2/io/github/toarchkumar/mnemo-android/X.Y.Z/`
  shows it first.

**If a mobile job fails after the engine jobs succeeded,** re-run only the
failed jobs:

- **Swift:** the push step refuses a tag that `mnemo-swift` already has. If
  the mirror was tagged but something later failed, the release is out:
  fix the cause forward, in the next version.
- **Maven Central** rejects a version it already has. If the deployment
  reached `PUBLISHED`, the release is out; if it failed validation, drop
  it in the Portal (Deployments) and re-run.

## Credentials

They're repository secrets on `toarchkumar/mnemo`, and nothing else needs
them. Renew them before they lapse, or a release fails at its publish step.

| Secret | What | Renew by |
|---|---|---|
| `MAVEN_CENTRAL_USERNAME`, `MAVEN_CENTRAL_PASSWORD` | the Central Portal user token `mnemo-github-actions` | **2027-11-09**, when it expires: make a new token in the Portal (View Account > Generate User Token) and update both secrets |
| `MNEMO_SWIFT_TOKEN` | a GitHub token with write access to `toarchkumar/mnemo-swift` | **2027-11**: replace it before it expires |
| `MAVEN_SIGNING_KEY`, `MAVEN_SIGNING_KEY_PASSWORD` | the GPG signing key (RSA 4096, fingerprint `CFAFF7A174EBC15ED5475C4619104D9B3DA89A04`) | **before 2028-11-08**, when it expires: extend it (`gpg --edit-key CFAFF7A174EBC15ED5475C4619104D9B3DA89A04`, then `expire`) and re-send it with `gpg --keyserver keyserver.ubuntu.com --send-keys CFAFF7A174EBC15ED5475C4619104D9B3DA89A04`. The key stays the same, so the secrets don't change unless you replace it |
| `CRATES_IO_TOKEN` | the crates.io API token | when crates.io says it expires |

PyPI needs no secret: it uses Trusted Publishing (the `pypi` environment).

The Maven Central namespace `io.github.toarchkumar` is verified once and
stays verified.
