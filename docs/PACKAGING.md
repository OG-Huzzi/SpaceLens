# Packaging

Status: **nothing in this repository is distributable.** There is no signed
build, no notarised build, no installer that a normal user can launch without
a security bypass, and no release channel. This document records what the
packaging path actually does today, what an unsigned artifact means on each
platform, and exactly what is still missing.

Maintenance document â€” update it when any fact below changes. It is not a
marketing page.

## 1. What can be built today

The desktop shell (`src-tauri/`) is its own Cargo workspace root (empty
`[workspace]` table). `tauri::generate_context!()` embeds `../dist` at compile
time, so the frontend must exist before the shell compiles;
`build.beforeBuildCommand` (`npm run build`) handles that automatically.

| Platform | Bundle targets | Expected files (Tauri naming) |
| --- | --- | --- |
| Windows x64 | `nsis`, `msi` | `CoreSight_0.1.0_x64-setup.exe`, `CoreSight_0.1.0_x64_en-US.msi` |
| macOS universal | `app`, `dmg` | `CoreSight.app`, `CoreSight_0.1.0_universal.dmg` |

`bundle.targets` in `src-tauri/tauri.conf.json` lists all four
(`["nsis", "msi", "app", "dmg"]`). The bundler filters that list against the
target platform's native package types, so a Windows run never attempts a DMG
and a macOS run never attempts an MSI. (`tauri-bundler`,
`Settings::package_types`.) `bundle.active` is `true`.

Configuration values that now matter:

- **Identifier** â€” `com.coresight.desktop`. Do not change it after the first
  public release: macOS keys TCC (Full Disk Access), Keychain and
  LaunchServices state to the bundle id, and Windows installer upgrades key to
  a product code derived from it. It previously ended in `.app`, which the
  Tauri bundler warns about because it collides with the macOS application
  bundle extension; it was changed before anything shipped, which is the only
  cheap moment to change it.
- **Version** â€” `0.0.0`, and duplicated in three independent places:
  `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`. The
  bundle takes the version from `tauri.conf.json`. Bump all three together;
  `0.0.0` is deliberate, because nothing here is a release.
- **CSP** â€” `default-src 'self'; img-src 'self' asset: https://asset.localhost`
  in `app.security.csp`. Packaging must not weaken it to make an asset load.
- **Category** â€” `Utility` (macOS `LSApplicationCategoryType`).
- **Icons** â€” `src-tauri/icons/` already contained a complete, valid set
  (32/128/256/512 PNG, a 16â€“256 multi-size `.ico`, a valid `.icns`). Nothing
  was generated for this work. They are a simple geometric mark, not reviewed
  as final brand artwork.

## 2. Local commands

Prerequisites:

- Node 22+ and npm.
- Rust stable via rustup.
- **Windows:** "Desktop development with C++" (MSVC v143) plus the Windows
  10/11 SDK. WebView2 runtime (preinstalled on Windows 11). The Tauri CLI
  downloads NSIS and WiX on the first bundle, so the first run needs network
  access.
- **macOS:** Xcode Command Line Tools (`xcode-select --install`). Universal
  builds additionally need both Rust targets:
  `rustup target add aarch64-apple-darwin x86_64-apple-darwin`.

Commands, from the repository root:

```powershell
npm ci                      # reproducible install from package-lock.json
npm run build               # tsc --noEmit && vite build -> dist/
npm run tauri:dev           # dev shell against http://localhost:1420
npm run tauri:build         # release build + every bundle target for this OS
```

Targeted builds (the `--bundles` values are validated per platform, so
`msi,nsis` is only accepted on Windows and `app,dmg` only on macOS):

```powershell
npm run tauri:build -- --bundles nsis,msi                  # Windows
npm run tauri:build -- --bundles app,dmg                   # macOS
npm run tauri:build -- --bundles dmg --target universal-apple-darwin
npm run tauri:build -- --no-bundle                         # compile only, no installer
```

Outputs land in `src-tauri/target/release/bundle/{nsis,msi,macos,dmg}/`.

`npm run tauri:build` runs `build.beforeBuildCommand` itself, so the frontend
is rebuilt as part of it.

## 3. CI

`.github/workflows/bundle.yml` â€” workflow name **`Bundle (unsigned)`**.

- Triggers: `push` to `main` and `workflow_dispatch`. **Not** on pull
  requests: bundling is slow and a PR does not need an installer to be
  reviewed.
- Matrix: `windows-latest` and `macos-latest`, `fail-fast: false`, so one
  platform's failure does not cancel the other.
- Steps: checkout -> Node 22 (npm cache) -> Rust stable -> cache the
  `src-tauri` target directory -> `npm ci` -> `npm run tauri:build --
  --bundles <per-OS>` -> collect installers -> upload artifact.
- Artifact names (both explicitly marked unsigned):
  - `coresight-windows-x64-unsigned` â€” NSIS `.exe`, MSI `.msi`, `UNSIGNED-BUILD.txt`.
  - `coresight-macos-arm64-unsigned` â€” `.dmg`, `CoreSight-unsigned.zip` (the
    zipped `.app`, for inspection only; `upload-artifact` does not preserve the
    executable bits of a bundle directory), `UNSIGNED-BUILD.txt`.
- The job fails if the bundler produced no installer
  (`if-no-files-found: error` plus an explicit count check). It is **not**
  `continue-on-error`: a packaging gate that cannot fail is not a gate.

Why it is a separate workflow from `ci.yml`: the merge gate (`ci.yml`) is
`cargo fmt`/`clippy`/`test` plus the frontend build. Bundling needs OS
toolchains that unit tests do not (MSVC + Windows SDK, Xcode command line
tools) and downloads NSIS/WiX at build time. Keeping it separate means a
packaging failure cannot turn the required gate red, and â€” equally important â€”
cannot hide a test failure behind a bundler failure. A red `Bundle (unsigned)`
run is still a release blocker; it is just not a merge blocker.

The macOS runner is `macos-latest`, which is Apple silicon. The bundle job now
builds `--target universal-apple-darwin` (both Rust targets installed first)
and a `lipo -archs` step fails the job unless the app contains **both** `arm64`
and `x86_64`, so an Intel-capable artifact cannot be claimed without proof.
**That job had not run when this note was written** — treat the universal build
as unverified until a green `Bundle (unsigned)` run exists.

## 4. What an unsigned build means per platform

### macOS

The app is built without a Developer ID signature, without the hardened
runtime entitlement set and without notarisation, and the DMG is not
notarised either. On a machine that did not build it:

- The download carries the `com.apple.quarantine` attribute, and Gatekeeper
  refuses the first launch: *"CoreSight" cannot be opened because the developer
  cannot be verified* â€” or, when the signature is missing/invalid, *"CoreSight"
  is damaged and can't be opened. You should move it to the Trash.*
- A developer or tester can bypass this per app: right-click -> **Open**, or
  System Settings -> Privacy & Security -> **Open Anyway**, or
  `xattr -dr com.apple.quarantine /Applications/CoreSight.app`.
- Those are security bypasses, not installation instructions. Do not hand them
  to end users as if the build were distributable.
- Managed/MDM Macs and Mac App Store distribution reject unsigned apps
  outright.

### Windows

The installers are not Authenticode-signed:

- SmartScreen shows *"Windows protected your PC â€” Microsoft Defender
  SmartScreen prevented an unrecognized app from starting"*, and the UAC
  prompt shows **Unknown publisher**.
- The user can continue via **More info -> Run anyway**. Because downloads
  carry the Mark-of-the-Web, this appears for every user on every download.
- AppLocker/WDAC policies and some EDR products block unsigned installers
  outright.
- SmartScreen reputation is tied to a signing certificate; an unsigned binary
  can never accrue any.

## 5. What signing and notarisation require

### Apple (macOS)

- A paid Apple Developer Program membership.
- A **Developer ID Application** certificate with its private key, exported as
  a `.p12`.
- Notarisation through `notarytool` (Xcode 13+), authenticated with either an
  App Store Connect API key (`.p8` + key id + issuer id) or an Apple ID with an
  app-specific password plus the team id.
- Hardened runtime enabled, plus any entitlements the app genuinely needs.
- Signing and notarisation must run on macOS; a macOS app cannot be signed
  from Windows.
- Tauri's bundler performs signing/notarisation when the certificate and
  credentials are supplied: `APPLE_CERTIFICATE` (base64 `.p12`),
  `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` (or
  `bundle.macOS.signingIdentity` in the config), and either `APPLE_ID` /
  `APPLE_PASSWORD` / `APPLE_TEAM_ID` or an App Store Connect API key (`.p8`
  plus key id and issuer id, via Tauri's `APPLE_API_KEY*` variables).
- Verify with `codesign --verify --deep --strict CoreSight.app`,
  `spctl -a -vv CoreSight.app` and `xcrun stapler validate CoreSight.app`.

### Windows

- An OV or EV code-signing certificate from a publicly trusted CA.
- Since 2023 the CA/Browser Forum requires the private key to live in
  FIPS 140-2 Level 2 hardware (USB token or HSM). A `.pfx` in a CI secret is
  therefore generally not obtainable for newly issued certificates, so CI
  signing needs a cloud signing service (Azure Trusted Signing, DigiCert
  KeyLocker, SSL.com eSigner) or a self-hosted runner with the token attached.
  Tauri supports this through `bundle.windows.signCommand`; the
  `certificateThumbprint` path assumes a certificate already in the local
  store.
- Verify with `signtool verify /pa /v <installer>`.
- EV certificates get SmartScreen reputation immediately; OV certificates must
  earn it over time.

### Credentials in this repository

**Neither credential exists here.** There is no Apple Developer ID
certificate, no notarytool credential, no Windows code-signing certificate and
no `signCommand`. `bundle.yml` contains no signing step, by design, and every
artifact is named `-unsigned`. Do not add a signing step that could silently
produce an unsigned artifact while a release note claims otherwise: either the
signing step proves the signature (`signtool verify`, `codesign --verify`,
`spctl`), or the build must stay labelled unsigned.

## 6. What is still missing before this is distributable

1. **Apple signing** â€” Developer ID certificate + notarisation credentials,
   stored as CI secrets, plus a signing/notarisation step and a
   `codesign`/`spctl`/`stapler` verification step in `bundle.yml`.
2. **Windows signing** â€” a code-signing identity (certificate or cloud signing
   service) and a signing step with `signtool verify` afterwards.
3. **A real version** — addressed: `0.1.0` lives in three files, and
   `CHANGELOG.md` + `docs/RELEASE.md` exist. Still no git tag and no published
   release.
4. **Universal macOS build** — configured in `bundle.yml` and guarded by
   `lipo`, but **not yet observed on a hosted runner**. Unverified until a
   green `Bundle (unsigned)` run exists.
5. **Auto-update** â€” no updater plugin, no update signing key, no update
   endpoint. Shipping without it is possible but must be a stated decision.
6. **Publishing** â€” no GitHub Release, no checksums, no download page, no
   SBOM/provenance attestation. The workflow uploads artifacts for inspection,
   nothing more.
7. **Installer policy** â€” per-user vs per-machine install, uninstall
   behaviour, and any EULA are undecided and untested.
8. **macOS entitlements and TCC** â€” the app inspects the filesystem, so it will
   hit the Full Disk Access prompt on macOS; the entitlements and the
   first-run permission path have not been designed or tested. Hardened
   runtime has never been exercised.
9. **A green CI bundling run** â€” `bundle.yml` has never run on the hosted
   runners. Until a run exists whose artifacts were downloaded and launched,
   the bundle path is unverified; treat the first run as the experiment, not
   as proof.
10. **Final brand assets** â€” the current icons are valid and complete, but are
    not reviewed as final artwork.

## 7. Local verification status of this packaging work

Recorded here so the gap between "config exists" and "installer exists" is not
papered over.

Verified on the development machine (Windows 11, 2026-11-13):

- `npm ci` â€” pass (154 packages, 0 vulnerabilities reported by npm).
- `npm run build` â€” pass (`tsc --noEmit` + `vite build`).
- `npm audit --audit-level=high` â€” pass, 0 vulnerabilities.
- `cargo check --workspace` â€” pass.
- `src-tauri/tauri.conf.json` â€” parses and validates against the Tauri 2
  config schema.
- `.github/workflows/bundle.yml` â€” parses as YAML; `ci.yml` is unchanged.

Not verified:

- `npm run tauri:build` stopped in the Rust compile step, before the bundler
  ran. At the time of writing the working tree was mid-refactor
  (`coresight-ipc`'s `StartScanRequest` had gained a `root` field that the
  shell in `src-tauri/src/main.rs` did not yet set), so the shell failed with
  `error[E0063]: missing field root in initializer of StartScanRequest`.
  That is a source-level break outside this packaging change, not a bundler
  problem â€” but it means the bundle step itself was never reached.
- Independently of that, this machine cannot produce a supported Windows
  bundle: `tauri info` reports *"Couldn't detect any Visual Studio or VS Build
  Tools instance with MSVC and SDK components"*, and its default Rust host is
  `x86_64-pc-windows-gnu` rather than the MSVC target Tauri packages for.
  Installing a toolchain was out of scope for this work.

So: **this work did not produce an installer locally, and no CI run has
produced one yet.** The first green `Bundle (unsigned)` run is the first proof
that the packaging path works. Until then, treat the file names in section 1
and the artifact names in section 3 as configured intent, not observed output.
