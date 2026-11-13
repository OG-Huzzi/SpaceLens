# Release runbook

Maintenance document. Last updated for **0.1.0** (2026-11-13).

`docs/PACKAGING.md` records what the packaging path does and what an unsigned
artifact means in detail. This file is the step-by-step procedure a maintainer
follows to bump, build, verify and publish a release. Where the two disagree,
`docs/PACKAGING.md` wins on platform behaviour and this file wins on procedure.

**Current state:** `0.1.0` is versioned but not distributable. The installers are
unsigned and unnotarised, there is no auto-update, and an installed build has
not been launched yet. Do not describe a `0.1.0` artifact as a public release.

## 1. Where the version lives

The app version exists in exactly three source files. The two lockfiles follow
automatically and must not be hand-edited.

| File | Field | Value at 0.1.0 | Written by |
| --- | --- | --- | --- |
| `package.json` | top-level `version` | `0.1.0` | `npm version --no-git-tag-version`, or by hand |
| `package-lock.json` | top-level `version` and `packages[""].version` | `0.1.0` | npm, from `package.json` |
| `src-tauri/tauri.conf.json` | top-level `version` | `0.1.0` | by hand; **this is what the bundler uses** |
| `src-tauri/Cargo.toml` | `[package] version` | `0.1.0` | by hand |
| `src-tauri/Cargo.lock` | `[[package]] name = "coresight"` → `version` | `0.1.0` | cargo, on the next `cargo check`/`cargo build` |

`src-tauri/Cargo.lock` is a separate lockfile because `src-tauri` is its own
Cargo workspace root (the empty `[workspace]` table in `src-tauri/Cargo.toml`).

The engine crates under `crates/` stay at `0.0.0`. They are unpublished internal
libraries; their version is not the application version and is not shown to a
user.

### Why the version is not centralised

There is no single source of truth, deliberately:

- The root workspace has 19 member crates. A `[workspace.package] version` +
  `version.workspace = true` would require editing every member's
  `Cargo.toml`. That is a wide, risky change for an internal-library version
  nobody consumes, and it would still not cover the next point.
- `src-tauri` is a **separate workspace root**. It cannot inherit
  `[workspace.package]` from the repository root at all, so the shell would keep
  its own version regardless.
- `package.json` and `tauri.conf.json` are consumed by npm and the Tauri
  bundler respectively; neither reads the other's version.

The practical consequence: **three files must be bumped together**. The
`tauri.conf.json` value is authoritative for artifact names and the app's
reported version; `src-tauri/Cargo.toml` must match it so the binary metadata
and the bundle agree. If a future change adds a version-bump script, it must
update all three in one commit.

## 2. Bumping the version

From the repository root:

```powershell
# 1. Update CHANGELOG.md first: move the [Unreleased] entries into a new dated
#    section and update the compare links.

# 2. Edit "version" in package.json by hand, then sync the lockfile from it.
#    (`npm version 0.1.1 --no-git-tag-version` also works, but it refuses a
#    dirty tree and the changelog edit has already made the tree dirty.)
npm install --package-lock-only

# 3. src-tauri/tauri.conf.json -> "version"
#    src-tauri/Cargo.toml     -> [package] version
#    Edit both by hand; they must equal the npm version.

# 4. Regenerate src-tauri/Cargo.lock with the new package version
cargo check --manifest-path src-tauri/Cargo.toml

# 5. Prove nothing else broke
npm ci
npm run build
npm audit --audit-level=high
cargo check --workspace
```

Do not run `npm version` without `--no-git-tag-version`: it would create a git
tag and a version commit. The tag must point at the commit whose artifacts were
actually verified (section 3), not at the bump commit.

## 3. Cutting a release

There is no release workflow and no publishing automation. CI produces
artifacts; a maintainer verifies and publishes them by hand.

### 3.1 Preconditions

- `CI` is green on the exact commit being released (`cargo fmt`, clippy, tests,
  the shell type-check and the frontend build).
- `Bundle (unsigned)` is green on that commit, or you have run it manually for
  that commit.
- `CHANGELOG.md` has a dated section for the version.
- The three version files agree (section 1).

### 3.2 Run the bundle workflow

`Bundle (unsigned)` runs on every push to `main`, or manually:

**Actions → Bundle (unsigned) → Run workflow → branch `main`.**

The workflow has two matrix entries, both fail-closed (no `continue-on-error`):

| Runner | Bundles | Artifact |
| --- | --- | --- |
| `windows-latest` | NSIS `.exe`, MSI `.msi` | `coresight-windows-x64-unsigned` |
| `macos-latest` | `.app`, `.dmg`, universal | `coresight-macos-universal-unsigned` |

The macOS job installs both Rust targets (`aarch64-apple-darwin` and
`x86_64-apple-darwin`), builds with
`--target universal-apple-darwin`, and then runs `lipo -archs` on
`CoreSight.app/Contents/MacOS/CoreSight`. The job fails if either `arm64` or
`x86_64` is missing, so an accidental thin build cannot be uploaded as
"universal".

### 3.3 What CI produces

With version `0.1.0`, Tauri's bundler names files from
`productName_version_arch`:

| Platform | Files inside the artifact |
| --- | --- |
| Windows | `CoreSight_0.1.0_x64-setup.exe` (NSIS), `CoreSight_0.1.0_x64_en-US.msi` (WiX), `UNSIGNED-BUILD.txt` |
| macOS universal | `CoreSight_0.1.0_universal.dmg`, `CoreSight-unsigned.zip` (the zipped `.app`, for inspection), `UNSIGNED-BUILD.txt` |

The DMG name uses `universal` because the Tauri bundler maps
`universal-apple-darwin` to the `universal` architecture label. A host-only
build would instead produce `CoreSight_0.1.0_aarch64.dmg` (Apple silicon) or
`CoreSight_0.1.0_x64.dmg` (Intel).

Artifacts are retained for **14 days** (`retention-days: 14`). Download them
before that expires; there is no other copy.

### 3.4 Tag and publish

Only after verification (section 5):

```powershell
git tag -a v0.1.0 -m "CoreSight 0.1.0"
git push origin v0.1.0
```

The tag does **not** trigger a build — `bundle.yml` triggers on `main` pushes
and `workflow_dispatch`, not tags. The tag is a marker for the verified commit.

Create a GitHub Release manually and attach the verified files plus
`SHA256SUMS.txt`. Mark it **pre-release** and state plainly in the release notes
that the build is unsigned, unnotarised, has no auto-update, and is intended for
testers only. Do not publish it as a normal end-user download.

## 4. Signing and notarisation prerequisites — none exist yet

Neither credential exists in this repository. `bundle.yml` has no signing step
and every artifact is named `-unsigned`. The following is what would have to be
supplied; see `docs/PACKAGING.md` §5 for the full discussion.

### Apple (macOS)

- A paid Apple Developer Program membership.
- A **Developer ID Application** certificate with private key, exported as a
  `.p12`.
- Notarisation credentials for `notarytool`: either an App Store Connect API key
  (`.p8`, key id, issuer id) or an Apple ID with an app-specific password and
  the team id.
- Hardened runtime enabled, plus entitlements for the filesystem access the app
  genuinely needs (Full Disk Access interaction is not designed or tested yet).
- Signing/notarisation must run on macOS.
- Tauri reads: `APPLE_CERTIFICATE` (base64 `.p12`), `APPLE_CERTIFICATE_PASSWORD`,
  `APPLE_SIGNING_IDENTITY`, and either `APPLE_ID` / `APPLE_PASSWORD` /
  `APPLE_TEAM_ID` or `APPLE_API_KEY` / `APPLE_API_KEY_PATH` /
  `APPLE_API_ISSUER`. `bundle.macOS.signingIdentity` can replace
  `APPLE_SIGNING_IDENTITY`.
- The workflow must add a verification step that fails closed:
  `codesign --verify --deep --strict`, `spctl -a -vv`, and
  `xcrun stapler validate` must all succeed. A signing step that does not prove
  the signature is worse than no signing step.

### Windows

- An OV or EV code-signing certificate from a publicly trusted CA. Since 2023
  the CA/Browser Forum requires the private key in FIPS 140-2 Level 2 hardware,
  so CI generally needs a cloud signing service (Azure Trusted Signing, DigiCert
  KeyLocker, SSL.com eSigner) or a self-hosted runner with the token attached.
- Tauri supports this through `bundle.windows.signCommand`;
  `certificateThumbprint` assumes the certificate is already in the runner's
  local store.
- Verification must run afterwards: `signtool verify /pa /v <installer>` must
  succeed, and the workflow must fail closed if it does not.

## 5. Verifying an artifact before publishing

Do all of this on a machine that did not build the artifact.

### 5.1 Hash

Record and compare the SHA-256 of every file that will be published. CI does
not currently produce a checksum manifest, so generate one locally and attach
it to the release.

```powershell
# Windows
Get-FileHash -Algorithm SHA256 .\CoreSight_0.1.0_x64-setup.exe
Get-FileHash -Algorithm SHA256 .\CoreSight_0.1.0_x64_en-US.msi
```

```bash
# macOS
shasum -a 256 CoreSight_0.1.0_universal.dmg CoreSight-unsigned.zip
```

Store the result in `SHA256SUMS.txt`. If a hash does not match the artifact you
inspected, stop: do not publish.

### 5.2 macOS: signature, notarisation and architecture

```bash
# The DMG itself is intact
hdiutil verify CoreSight_0.1.0_universal.dmg

# Mount it and check the app inside
hdiutil attach CoreSight_0.1.0_universal.dmg
codesign --verify --deep --strict --verbose=2 /Volumes/CoreSight/CoreSight.app
codesign -dv --verbose=4 /Volumes/CoreSight/CoreSight.app
spctl -a -vv /Volumes/CoreSight/CoreSight.app
xcrun stapler validate /Volumes/CoreSight/CoreSight.app

# Both slices must be present
lipo -archs /Volumes/CoreSight/CoreSight.app/Contents/MacOS/CoreSight
# expected: x86_64 arm64   (order may vary)
file /Volumes/CoreSight/CoreSight.app/Contents/MacOS/CoreSight
# expected: Mach-O universal binary with 2 architectures
```

Expected **today**, for an unsigned build:

- `codesign` → `code object is not signed at all` (non-zero exit).
- `spctl` → `rejected` (non-zero exit).
- `xcrun stapler validate` → no ticket (non-zero exit).
- `lipo` → `x86_64 arm64`.

A correctly signed and notarised build would instead show a
`Developer ID Application: …` authority, `source=Notarized Developer ID` from
`spctl`, and `The validate action worked!` from `stapler`. Do not claim those
until they appear in the output.

### 5.3 Windows: Authenticode

```powershell
Get-AuthenticodeSignature .\CoreSight_0.1.0_x64-setup.exe |
  Format-List Status, StatusMessage, SignerCertificate

signtool verify /pa /v .\CoreSight_0.1.0_x64-setup.exe
```

Expected today: `Status : NotSigned`, and `signtool` reports
`No signature found`. A signed build must show `Status : Valid` and
`Successfully verified`.

### 5.4 Installer sanity

- The file is non-zero and roughly the size the job log reported.
- `UNSIGNED-BUILD.txt` is inside the artifact and names the commit and run id.
- The Windows installer launches on a clean VM; the macOS DMG mounts and the
  app drags to `/Applications`.
- Then complete the launch checklist in section 8. A hash and a signature do not
  prove the app runs.

## 6. What "unsigned" means to a user

`docs/PACKAGING.md` §4 has the detailed platform behaviour; this is the summary
that belongs in a release note.

### macOS

The app has no Developer ID signature and no notarisation ticket. On a machine
that did not build it, Gatekeeper refuses the first launch: *"CoreSight" cannot
be opened because the developer cannot be verified*, or *"CoreSight" is damaged
and can't be opened*. The tester can bypass it per app with right-click →
**Open**, System Settings → Privacy & Security → **Open Anyway**, or
`xattr -dr com.apple.quarantine /Applications/CoreSight.app`. Those are security
bypasses, not installation instructions. Managed/MDM Macs reject the app
outright.

### Windows

The installers are not Authenticode-signed. SmartScreen shows *"Windows
protected your PC"* and UAC shows **Unknown publisher**; the tester continues
through **More info → Run anyway**. Because downloads carry the Mark-of-the-Web,
this happens on every download. AppLocker/WDAC policies and some EDR products
block the installer outright. SmartScreen reputation is tied to a signing
certificate, so an unsigned binary can never earn any.

Neither platform's artifact is a normal-user install until section 4 is done.

## 7. Auto-update — deliberately not configured

There is **no** `plugins.updater` section in `src-tauri/tauri.conf.json`, no
`tauri-plugin-updater` dependency, no update signing key and no update endpoint.
The app cannot check for or install updates.

A half-configured updater is worse than none: Tauri's updater requires a public
key and at least one endpoint, the plugin must be registered in
`src-tauri/src/main.rs`, the capability must permit it, and update bundles must
be signed with a private key whose public half is in the config. Configuring the
JSON alone would produce an updater that silently fails — or, worse, an app that
believes it can update and cannot.

To add updates later, all of the following are required:

1. Add the Rust `tauri-plugin-updater` crate and the
   `@tauri-apps/plugin-updater` npm package, and register the plugin in the
   desktop shell (`src-tauri/src/main.rs` — a source change outside release
   engineering).
2. Generate a signing keypair with `npm run tauri signer generate -- -w
   ~/.tauri/coresight-updater.key`. The **public** key goes in
   `plugins.updater.pubkey`; the private key becomes a CI secret
   (`TAURI_SIGNING_PRIVATE_KEY` plus `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`).
3. Set `plugins.updater.endpoints` to one or more HTTPS URLs serving a Tauri
   `latest.json` manifest, and set `bundle.createUpdaterArtifacts: true` so the
   build emits `.sig` files.
4. Add an update-signing step to `bundle.yml` and verify the `.sig` files exist
   and match the artifacts; the workflow must fail closed otherwise.
5. Decide who hosts `latest.json`, how a bad release is withdrawn, and how
   downgrade/rollback works. None of that exists.

Until those exist, every release note must say "no auto-update".

## 8. Pre-release checklist

Copy this into the release issue and tick every box. **Publishing is blocked if
any box is unticked** — in particular the launch checks, which no amount of CI
can replace.

### Version and source

- [ ] `package.json`, `src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml`
      all carry the same version.
- [ ] `package-lock.json` and `src-tauri/Cargo.lock` were regenerated, not
      hand-edited.
- [ ] `CHANGELOG.md` has a dated section for the version and the links are
      updated.
- [ ] `CI` is green on the exact release commit.
- [ ] `Bundle (unsigned)` is green on the exact release commit.
- [ ] `ci.yml` is unchanged by the release work (byte-identical to `main`).

### Artifacts

- [ ] Windows artifact contains the NSIS `.exe`, the MSI and
      `UNSIGNED-BUILD.txt`; none is zero bytes.
- [ ] macOS artifact contains the `.dmg`, `CoreSight-unsigned.zip` and
      `UNSIGNED-BUILD.txt`; none is zero bytes.
- [ ] `lipo -archs` on the macOS executable reports **both** `arm64` and
      `x86_64`.
- [ ] SHA-256 hashes recorded in `SHA256SUMS.txt`.
- [ ] `UNSIGNED-BUILD.txt` names the correct commit and run id.
- [ ] Signature checks in section 5 produce the expected "unsigned" output (or
      a genuine valid signature, if signing has been added).

### Launch the installed app — Windows

- [ ] Install the NSIS `.exe` (or the MSI) on a clean Windows VM.
- [ ] Launch **CoreSight** from the Start menu; the window opens and stays open.
- [ ] Home, Storage, Cleanup, History and Drives all render; Settings opens.
- [ ] Switch language between English and German and confirm the UI copy
      changes.
- [ ] Run a scan on a small folder; a partial scan announces itself.
- [ ] In Cleanup, preview a plan. Confirm there is no execute affordance before
      a preview, and that executing is refused with `PLAN_REJECTED_UNSAFE`
      under the read-only policy.
- [ ] Close the app and uninstall it; confirm the entry is removed from
      Apps & features.

### Launch the installed app — macOS

- [ ] Mount the DMG on an **Apple silicon** Mac, drag CoreSight to
      `/Applications`, and launch it (using the documented Gatekeeper bypass
      for a tester machine only).
- [ ] Repeat on an **Intel** Mac with the same universal DMG. This is the only
      proof the x86_64 slice actually runs.
- [ ] The window opens; the five screens and Settings render.
- [ ] Switch language between English and German.
- [ ] Run a scan; note whether the Full Disk Access prompt appears and record
      the result (entitlements are not designed yet).
- [ ] Quit, eject the DMG, and remove the app from `/Applications`.

### Publication

- [ ] Release notes state **unsigned**, **not notarised**, **no auto-update**,
      and (if true at the time) **first launch requires a Gatekeeper bypass**.
- [ ] Release is marked pre-release.
- [ ] `SHA256SUMS.txt` and `UNSIGNED-BUILD.txt` are attached.
- [ ] Tag `vX.Y.Z` points at the verified commit.
- [ ] Nobody has described the artifact as a normal-user download.

## 9. Known gaps in this runbook

- The macOS universal build is new in 0.1.0 and has not yet completed a hosted
  runner run; until it does, treat the `lipo` step as the required proof.
- `bundle.yml` does not publish a checksum manifest or a GitHub Release; both
  are manual steps above.
- Installer policy (per-user vs per-machine, EULA, uninstall behaviour) is
  undecided and untested.
- macOS entitlements, hardened runtime and the Full Disk Access first-run path
  have never been exercised.
- Icons are valid but not reviewed as final brand artwork.
