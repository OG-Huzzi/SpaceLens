# Installer verification — 0.0.0 unsigned artifacts

Date: 2026-11-13. This is a record of what was **actually observed**, not what is
configured. It is the first time a CoreSight installer has been produced or
opened.

## Provenance

| Field | Value |
|---|---|
| Run | `43387812474`, workflow **Bundle (unsigned)** |
| Event / branch | `push` / `main` |
| Head commit | `96b9b267a0c3035ad9b5a8e99756adf10a2dd6eb` |
| Conclusion | `success` |
| Windows artifact | `coresight-windows-x64-unsigned`, 7,042,404 bytes |
| macOS artifact | `coresight-macos-arm64-unsigned`, 6,646,420 bytes |

Both downloads matched GitHub's published SHA-256 digests exactly.

## Windows: it runs

Contents of the Windows artifact:

| File | Bytes |
|---|---:|
| `CoreSight_0.0.0_x64-setup.exe` | 2,987,909 (NSIS 3, LZMA solid) |
| `CoreSight_0.0.0_x64_en-US.msi` | 4,284,416 (MSI + app.cab, MSZip) |
| `UNSIGNED-BUILD.txt` | 649 |

The app binary (`coresight.exe`, 11,333,120 bytes) was extracted from **both**
the NSIS installer and the MSI (administrative extract) and the two are the same
binary apart from Tauri's bundle-type marker.

Binary checks:

- `MZ` at offset 0, `PE\0\0` at `e_lfanew` (0xF8).
- COFF machine `0x8664` — `IMAGE_FILE_MACHINE_AMD64`.
- Optional header magic `0x020B` (PE32+), subsystem 2 (`WINDOWS_GUI`).
- Version resource: `CoreSight` / `coresight` / `FileVersion 0.0.0` / `ProductVersion 0.0.0`.
- **Authenticode: `NotSigned`** for the app exe, the NSIS setup and the MSI.

Launch test (extracted to a scratch folder; never installed):

| Observation | Result |
|---|---|
| Started | PID 12604 |
| After 10 s | `HasExited = False` |
| Responding | `True` |
| Main window title | **`CoreSight`** |
| Main window handle | 1181468 |
| Working set | 25,481,216 bytes |
| Child process | `msedgewebview2.exe` (PID 5760) with `--webview-exe-name=coresight.exe` |

WebView2 local storage under the scratch profile contained
`_http://tauri.localhost` and `coresight.settings.v1`, which proves the React
frontend actually booted, not merely that the process started.

A second launch also ran and exited within 8 s of `CloseMainWindow()` — a clean
shutdown.

Side effects: one empty `%LOCALAPPDATA%\com.coresight.desktop` directory was
created and removed. No registry entries, no `Program Files` writes, no Startup
entries, no leftover processes.

### What this means

The Windows artifact is **a real, launchable x86-64 Tauri 2 desktop
application** — not a package that merely builds. It is unsigned, version
`0.0.0`, and depends on the system WebView2 runtime (157.0.4341.46 was present
on the verification machine).

## macOS: inspected, not executed

Contents: `CoreSight-unsigned.zip` (3,292,312), `CoreSight_0.0.0_aarch64.dmg`
(3,390,667), `UNSIGNED-BUILD.txt` (615).

`.app` structure present: `Contents/MacOS/coresight` (11,056,048 bytes),
`Info.plist` (1,056), `Resources/icon.icns` (96,937),
`CFBundleIdentifier com.coresight.desktop`.

Mach-O header: magic `0xFEEDFACF` (64-bit), `cputype 0x0100000C` (arm64),
`filetype 2` (`MH_EXECUTE`). An ad-hoc `LC_CODE_SIGNATURE` is present
(`CS_ADHOC`, 85,856 bytes, no Team ID) with no `_CodeSignature` directory.

**It was not executed** — a Mach-O binary cannot run on Windows. The macOS
artifact is structurally sound but unproven at runtime.

## What this verification does NOT establish

1. **Nothing about signing.** All Windows artifacts are `NotSigned`; the macOS
   binary is ad-hoc signed only. Both will be blocked or warned against.
2. **Nothing about a real scan.** The app opened to its own UI; no scan was
   driven through the running binary.
3. **Nothing about installation.** The installers were extracted, never run
   through their setup flow, so uninstall behaviour is unverified.
4. **Nothing about the macOS runtime.**

## Reproducing

The artifacts are on run `43387812474` and expire per GitHub's retention policy.
`docs/RELEASE.md` has the maintainer procedure; `docs/PACKAGING.md` has the
signing prerequisites, none of which exist in this repository.
