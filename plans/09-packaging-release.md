# Phase 09: Packaging, release, auto-update, hardening

**Status:** in progress (manual build pipeline landed 2026-09-24; distribution jobs and
auto-update landed and verified against a real release 2026-09-27 — v0.3.1, macOS/Linux/silo
all confirmed working, winget PR awaiting moderator review;
hardening and docs site still open)
**Depends on:** 00 for CI (already running), then feature phases for the release
**Owns:** `script/bundle-*`, `.github/workflows/release.yml`, bundling metadata in `crates/kubyl`
**Mockups:** none (logo/icons in `assets/logo`)

## Goal

Signed, installable, auto-updating builds for macOS, Windows and Linux, plus the polish needed for
a public 1.0: accessibility, performance budgets, crash reporting (opt-in) and docs.

## Tasks

### Packaging
- [x] **macOS**: universal (arm64 + x86_64) `.app` with `kubyl.icns`, hardened runtime, Developer ID signing, notarization and stapling, `.dmg`. No custom DMG background yet
- [x] **macOS keychain**: Developer ID provisioning profile embedded (`MACOS_PROVISIONING_PROFILE`), `keychain-access-groups` in `packaging/macos/entitlements.plist`, so Kubyl uses the data protection keychain without prompts
- [x] **macOS Homebrew cask**: `Casks/kubyl.rb` in `/Users/alexander/Projects/homebrew-tap` (one universal `.dmg`, single url/sha256 — no per-arch blocks needed, unlike silo's formula). Auto-bumped by homebrew-tap's scheduled `bump-formulae` workflow: `bump-kubyl.sh` runs `brew bump-cask-pr` when a newer GitHub release exists
- [x] **Windows**: unsigned `.zip` (both archs) + `publish-winget` CI job submits/updates the `microsoft/winget-pkgs` manifest via `wingetcreate`. Avoids Authenticode signing by using winget's own unsigned-zip+portable installer type instead
- [x] Windows arm64 build (`aarch64-pc-windows-msvc`, cross-compiled)
- [x] **Linux**: `.deb`, `.rpm`, amd64 Arch `.pkg.tar.zst`. `.desktop` file (`packaging/linux/kubyl.desktop`) plus hicolor icons. Wayland and X11
- [x] **Linux silo**: `publish-silo` CI job pushes `.deb`/`.rpm`/`.pkg.tar.zst` to silo repo `kubyl` channel `stable` via the reusable `BirknerAlex/silo/.github/actions/publish` action (GitHub Actions, not GitLab). `package.kubyl.dev` CNAME alias still open (needs `kubyl.dev`, itself unregistered — see Docs and site)
- [x] **Linux AppImage**: `script/build-appimage.sh` (`linuxdeploy` + GTK plugin, both Linux archs, wired into `build-linux`). Not verified on a real Linux desktop yet — see handoff log
- [x] **Linux Flatpak (silo)**: `script/build-flatpak.sh` builds a `.flatpak` bundle per arch from the release binary (`packaging/linux/flatpak/`, GNOME 48 runtime for GTK 3 + WebKitGTK), `build-linux` uploads it and `publish-silo` pushes it to silo repo `kubyl` channel `flatpak` (silo >= 0.15.0). Not on Flathub (its policy rejects AI-assisted apps, see handoff log). Not yet run in CI or on a real desktop
- [ ] Per-platform "open with / register URL handler" `kubyl://` for deep links (open a context/namespace/resource)
- [ ] CLI shim `kubyl` (optional), e.g. `kubyl --context prod -n payments pods`

### Release and auto-update
- [x] Manual release workflow (`.github/workflows/release.yml`, `workflow_dispatch` only — merges to `main` never trigger it): build matrix, macOS sign+notarize, upload to GitHub Releases, checksums (`SHA256SUMS`)
- [x] **Distribution**: post-build jobs publish packages: GitHub Releases (all platforms), `publish-winget` (winget manifest PR), `publish-silo` (silo CLI to `kubyl` repo). macOS Homebrew auto-bumping lives entirely in homebrew-tap's own scheduled workflow (no push from kubyl CI)
- [ ] SBOM (`cargo cyclonedx`), provenance attestation
- [x] **Auto-update**: new crate `kubyl_selfupdate`. Signed update manifest (`updates-stable.json`/`updates-preview.json`, ed25519 via `minisign`, public key hardcoded in `verify.rs`), `build-update-manifest` CI job signs and publishes it as a release asset. Background check (6 h poll) → download → sha256 verify → `self_replace` the running executable → status bar item "restart to update to vX.Y.Z" (click relaunches, never automatic). `self_update.channel`/`self_update.auto_check` in `settings.json`. Homebrew, winget, apt/dnf/pacman, Flatpak and snap installs are detected (`installed::detect`) and skip self-update entirely, deferring to their own tool
- [x] Changelog generation from conventional commits (`git-cliff`, `cliff.toml`), also drives semantic version bumps (`cargo set-version`) — see `docs/RELEASING.md`
- [ ] Third-party notices: `cargo-about` generates `THIRD_PARTY_LICENSES.html` in CI, bundled into every package (and the About dialog), together with the IBM Plex OFL and Lucide ISC licenses

### Hardening
- [ ] Accessibility: keyboard reachability audit, focus order, screen reader labels. Follow GPUI/AccessKit progress (Zed is adding AccessKit) and wire roles as soon as GPUI exposes them
- [ ] Performance budgets in CI: cold start < 400 ms to first frame, 5k-row table at 60 fps, 10 clusters connected < 300 MB RSS, log view 5k lines/s. Add benchmarks with `criterion` plus a scripted UI smoke test
- [ ] Security: threat model doc (kubeconfig exec plugins run arbitrary commands; never auto-add kubeconfigs from untrusted sources without showing the commands), `cargo-deny` advisories, fuzz the kubeconfig and YAML parsers (`cargo-fuzz`)
- [ ] Crash reporting: opt-in, minidumps via `sentry` or similar, with PII/secret scrubbing. No telemetry by default
- [ ] Settings UI for all user settings (not only JSON)
- [ ] Light theme finished. Custom theme loading (Zed theme JSON compatibility would be a nice extra)

### Docs and site
- [x] `kubyl.dev` registered and live: landing page with the lockup from `assets/logo`, feature breakdown, roadmap, screenshots from the real app, `/download` and `/sponsor` pages, footer `/imprint` and `/privacy`
- [ ] Deeper docs on the site: install guide, kubeconfig/auth guide, OIDC setup, keybindings reference, k9s migration guide — today the site is a single-page overview plus `/download`/`/sponsor`, nothing yet for these
- [x] README has badges (CI, release, license, sponsors) and a Contributing section, `LICENSE-MIT`/`LICENSE-APACHE` referenced in the License section
- [ ] No code of conduct yet (no `CODE_OF_CONDUCT.md`, no section for it)

## Acceptance criteria

- Clean-machine install works from the DMG, Homebrew cask, winget, silo packages and AppImage. The app launches, updates itself (manual installs only — package-manager installs defer to their own tool) and signature checks pass.
- Performance budgets are enforced in CI.
- Docs cover every feature from phases 01–09.

## Handoff log

- **2026-09-24**: Landed the manual build/release pipeline (`.github/workflows/release.yml`,
  `workflow_dispatch` only — never fires on push/PR). `version` job uses `git-cliff` +
  `cargo-edit` to compute the next SemVer version from Conventional Commits (or a forced
  patch/minor/major), writes `CHANGELOG.md`, commits `chore(release): vX.Y.Z`, tags, pushes to
  `main`; `dry_run` input previews without doing any of that. Build jobs: macOS universal
  (`cargo-bundle` + `lipo`, Developer ID codesign with `packaging/macos/entitlements.plist`,
  `notarytool` submit + staple, `.dmg`); Windows x86_64/aarch64 (`.zip`, **unsigned** — no
  Authenticode cert yet); Linux x86_64 (`ubuntu-latest`) and aarch64 (native
  `ubuntu-24.04-arm` runner — avoided cross-compiling GPUI's Wayland/X11/Vulkan C deps) each
  producing a raw `.tar.gz`, `.deb` (`cargo-deb`, metadata in `crates/kubyl/Cargo.toml`) and
  `.rpm` (`cargo-generate-rpm`, same file); amd64-only Arch `.pkg.tar.zst` via
  `packaging/arch/PKGBUILD` run through `archlinux:latest` + `makepkg` (Arch is x86_64-only
  upstream, so no arm64 variant). `release` job publishes via `softprops/action-gh-release`
  with a `SHA256SUMS` file and the git-cliff-generated release notes as the body.
  Cert/secret setup for macOS notarization is documented step-by-step in `docs/RELEASING.md`
  (Developer ID Application `.p12` + App Store Connect API key `.p8`) — not yet configured as
  repo secrets, so a real (non-dry-run) release will fail at the macOS signing step until that's
  done. Not implemented: Windows/EV signing, AppImage/MSI/winget/Homebrew, auto-update,
  SBOM/provenance, third-party notices — tracked as open checkboxes above.
- **2026-09-26**: Keychain prompts on every launch/update: the login keychain trusts apps by code
  signature, dev builds (ad-hoc, new cdhash per build) and releases shared the same items, and
  each write reset an item's partition list to the writer, so releases asked again. Release
  builds now embed a Developer ID provisioning profile and claim `keychain-access-groups`
  (`<team ID>.io.github.birkneralex.Kubyl`); `kubyl_kube::auth::store` uses the data protection
  keychain when `Contents/embedded.provisionprofile` exists, else the login keychain. No
  migration of old items (sign in again once). The release job fails before signing without
  the `MACOS_PROVISIONING_PROFILE` secret or when the profile doesn't grant the entitlements
  (setup: `docs/RELEASING.md`, step 4). Not verified on a signed build yet: the first release
  with the profile is the test.
- **2026-09-27**: Finalized distribution strategy. **macOS**: Homebrew formula in homebrew-tap with auto-bumping (similar to silo). Create `kubyl.rb` formula and `bump-kubyl.sh` script to check releases every 3 hours and open PRs. **Windows**: unsigned `.zip` published to GitHub Releases + winget manifest PR to `microsoft/winget-pkgs`. **Linux packages**: `.deb`, `.rpm`, `.pkg.tar.zst` to silo repo `kubyl` channel `stable` via silo CLI (actions helper from `/Users/alexander/Projects/silo`). Implementation: update `.github/workflows/release.yml` for winget/silo; Homebrew auto-update via homebrew-tap. Setup: add `SILO_KUBYL_TOKEN` secret for Linux package publishing.
- **2026-09-27 (later)**: Implemented distribution jobs and app self-update, on `phase/09-packaging-release`.
  - Corrected the earlier plan: kubyl's macOS package is a Homebrew **cask** (`Casks/kubyl.rb`), not a
    formula — formulae are for CLI tools (silo), casks install `.app` bundles. Since Kubyl already
    ships one universal `.dmg`, the cask needs a single url/sha256 pair; `bump-kubyl.sh` just calls
    `brew bump-cask-pr --no-fork --version=X`, no by-hand sed like silo's formula needs. Verified
    `brew style`/`brew audit --online` pass against the real v0.2.5 release (temporarily copied into
    the already-installed local tap clone to validate, then removed — the tap repo itself is
    untouched pending a push). Branch/PR creation itself is unverified (no newer release exists yet
    to test against; `brew bump-cask-pr` correctly refused a same-version dry run).
  - `.github/workflows/release.yml`: added `build-update-manifest` (signs `updates-stable.json` with
    `minisign`, needs secret `UPDATE_SIGNING_KEY`), `publish-winget` (`wingetcreate update
    BirknerAlex.Kubyl --submit`, needs secret `WINGET_TOKEN`; only works after the package's first,
    one-time-manual `wingetcreate new` submission — documented in `docs/RELEASING.md`), and
    `publish-silo` (`BirknerAlex/silo/.github/actions/publish@v0.14.0`, needs secret
    `SILO_KUBYL_TOKEN`). `release` now depends on `build-update-manifest` so the signed manifest
    ships as a release asset alongside the platform artifacts.
  - Generated the real ed25519 (minisign) release signing keypair locally (`brew install minisign`).
    Public key hardcoded in `crates/kubyl_selfupdate/src/verify.rs`; private key given to the user to
    store as repo secret `UPDATE_SIGNING_KEY` (never committed). End-to-end proof it all fits
    together: signed a real manifest with the real private key, committed the manifest + `.minisig`
    as `crates/kubyl_selfupdate/tests/fixtures/`, and `tests/e2e_signature.rs` verifies them against
    the embedded public key — not just the throwaway key `verify.rs`'s own unit tests use.
  - New crate `kubyl_selfupdate` (app self-update; unrelated to `kubyl_updates`, which is Kubernetes
    *cluster* updates — settings key deliberately `self_update`, not `updates`, to avoid confusion).
    `manifest.rs` (signed JSON shape, `platform_key()` per target), `verify.rs` (minisign, via the
    `minisign-verify` crate — verify-only, no signing capability in the binary), `installed.rs`
    (detects Homebrew/Caskroom, winget's `WinGet\Packages`, `/usr`/`/opt` deb-rpm-pacman installs,
    Flatpak's `FLATPAK_ID`, snap's `SNAP` — all skip self-update), `download.rs` (fetches from
    `github.com/.../releases/latest/download/...`, sha256-checks the artifact against the signed
    manifest), `apply.rs` (extracts the new binary — `tar`+`flate2` on Linux, `zip` on Windows,
    `hdiutil attach` for the `.dmg` on macOS since it's a disk image, not a plain archive — and
    `self-replace`s the running executable; known gap: doesn't refresh the macOS bundle's
    `Info.plist`/icon/signature, only the binary), `service.rs` (GPUI global, 6 h poll, state machine
    Idle→Checking→Available→Downloading→ReadyToRestart, never auto-restarts), `ui.rs` (status bar
    item, "restart to update to vX.Y.Z" click triggers `SelfUpdate::restart`), `settings.rs`
    (`self_update.channel`/`.auto_check`). Wired into `crates/kubyl/src/main.rs` (append-only line).
    New workspace deps: `minisign-verify`, `self-replace`, `semver`, `zip` (Windows-only target dep).
  - Full validation: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`
    (clean), `cargo test --workspace` (all green, `kubyl_selfupdate` alone: 10 unit + 2 integration),
    `cargo deny check` (advisories/bans/licenses/sources all ok).
  - Not done: Windows Authenticode signing (unchanged — winget accepts unsigned zips, so this was
    deliberately not blocking), SBOM/provenance, third-party notices, AppImage, `package.kubyl.dev` (silo alias — `kubyl.dev`
    itself is registered now, see below), and the whole Hardening section. Nothing in this session
    has run against a real release yet — `build-update-manifest`/`publish-winget`/`publish-silo`
    are new and their first real invocation is the test, same caveat as the macOS provisioning
    profile in the 2026-09-26 entry above.
- **2026-09-27 (later still)**: `kubyl.dev` is registered and live (landing page, `/download`,
  `/sponsor`, footer `/imprint` and `/privacy`; feature breakdown, roadmap and screenshots from the
  real app). No deeper docs yet — no install guide, kubeconfig/auth guide, OIDC setup page,
  keybindings reference or k9s migration guide; those still need their own pages. `package.kubyl.dev`
  as a silo alias is unblocked now that the domain exists, still not done.
- **2026-09-27 (PR review pass)**: Fixed a Windows CI failure and 16 CodeRabbit findings on
  PR #16 (`phase/09-packaging-release`).
  - **Windows CI**: `tests/e2e_signature.rs`'s fixture files had no `.gitattributes` entry, so
    Windows' checkout applied its default CRLF normalization and broke the byte-exact minisign
    signature. Added `.gitattributes` marking `crates/kubyl_selfupdate/tests/fixtures/* -text`.
  - **macOS self-update, corrected**: the original `apply.rs` only swapped
    `Kubyl.app/Contents/MacOS/kubyl`, leaving the bundle's `_CodeSignature/CodeResources` sealed
    over the *old* binary's hash — the hardened runtime would have refused to launch the updated
    app. Rewrote it to stage the *whole* `Kubyl.app` from the mounted `.dmg` (via `ditto`, which
    preserves resource forks/xattrs a plain recursive copy could drop), verify it with
    `codesign --verify --deep --strict` before ever touching the installed copy, then swap the
    bundle directories (a rename, which Unix allows even while the old bundle's executable is
    the one currently running this code) with a restore-on-failure fallback. Added local tests
    for the new `app_bundle_root` path logic (finally something verifiable on this dev machine).
  - `download.rs`: added a 30 s connect / 15 min total timeout to the reqwest client — without
    one, a stalled request left the service stuck in `Checking`/`Downloading` forever (both
    states skip re-checking).
  - Fixed a settings-key doc typo (`updates_app` → `self_update`, the real
    `SelfUpdateSettings::KEY`) in `lib.rs` and this file.
    anything in this repo).
- **2026-09-27 (merge + winget submission)**: PR #16 merged to `main` (`gh pr merge --admin` —
  all six checks green: rustfmt, both cargo-deny jobs, macOS/Ubuntu/Windows, CodeRabbit).
  Submitted the winget PR:
  [microsoft/winget-pkgs#442272](https://github.com/microsoft/winget-pkgs/pull/442272)
  (`BirknerAlex.Kubyl` v0.2.5). No Windows machine or Wine needed in the end — the three
  manifest files (version, installer, `en-US` locale) were hand-written against a real current
  reference package (Deno's, schema `1.12.0`) rather than generated by `wingetcreate new`'s
  interactive wizard, then pushed via the GitHub Contents API directly to a branch on a fork (no
  full clone of the very large winget-pkgs repo needed either). `docs/RELEASING.md`'s "one-time
  setup: winget" section updated to point at the submitted PR instead of the manual-Windows-step
  instructions. Automated validation (Pull Request/Manifest/URL/Policy Validation, Installers
  Scan, Installation Validation — the last two actually run and install the .zip) was still
  pending as of this entry; that bot feedback, not this handoff, is the real test of whether the
  manifest is accepted.
- **2026-09-27 (AppImage)**: Added `script/build-appimage.sh`, wired into `build-linux`'s existing
  matrix (reuses the binary that job already built, both archs). Researched two real references
  before writing anything: Zed's own `script/bundle-linux` turned out not to build an AppImage at
  all (hand-curated library tarball instead, excluding glibc-family libs to avoid host conflicts —
  no `linuxdeploy`/`appimagetool` anywhere), so no shortcut there. `tauri-bundler`'s
  `bundle/linux/appimage/linuxdeploy.rs` did have exactly what was needed: kubyl_webview uses
  `wry`, the same webview crate Tauri uses, and both hit the identical webkit2gtk problem —
  neither `linuxdeploy` nor its GTK plugin's dependency scan finds `WebKitWebProcess`,
  `WebKitNetworkProcess` or the injected-bundle `.so` (nothing directly links them), so they have
  to be copied into the AppDir by hand at the same relative path they'd have on a real system.
  Mirrored tauri-bundler's exact tool provenance rather than "latest": `linuxdeploy` pinned to the
  commit (`07333c6`) and `AppRun` binary from `tauri-apps/binary-releases` that ships in
  production Tauri apps today, not `linuxdeploy`'s own "continuous" build (which tauri-bundler
  specifically avoids — it has shipped broken AppImage output before). The GTK plugin script
  itself has no downloadable release asset upstream (confirmed via the GitHub API), so it's
  fetched fresh from `linuxdeploy-plugin-gtk`'s `master` branch at build time, matching how
  tauri-bundler vendors the same file. Also replicated a `dd` byte-patch that zeroes the
  AppImage type-2 magic bytes in the downloaded `linuxdeploy` tool itself (not kubyl's output) —
  otherwise a desktop's AppImage integration daemon tries to "integrate" `linuxdeploy` while it's
  only used transiently as a build tool. Validated what's checkable without a real Linux GUI
  session: `bash -n` and `shellcheck` clean, YAML syntax valid. **Not verified**: nothing in this
  step has run in CI or launched on a real Linux desktop yet (no FUSE/X11 session available
  here) — the first CI run building it, and someone actually launching the resulting
  `.AppImage`, are both still open. If it fails, the webkit2gtk subprocess-helper paths (uses
  `webkit2gtk-4.1`; Ubuntu's actual layout should match since that's the same package name
  `release.yml` already installs) are the most likely culprit.
- **2026-09-27 (first real v0.3.x release)**: Triggered the release pipeline for real — the
  first time everything landed in this phase actually ran end to end. Two bugs surfaced, both
  fixed and re-verified on a second real run:
  - `build-update-manifest` failed: `taiki-e/install-action` has no prebuilt-binary mapping for
    `minisign` and silently fell through to `cargo-binstall`, which fails outright ("no binaries
    specified nor inferred") since the `minisign` crate isn't binstall-compatible. This isn't
    something a `dry_run` would have caught. Fixed by installing the real CLI via `apt` instead
    (Ubuntu ships it, confirmed via packages.ubuntu.com). Because `build-update-manifest` gates
    `release`, this also meant **v0.3.0 built everything successfully but was never published**
    — the tag and changelog commit exist, but no GitHub Release. Left as-is (harmless); v0.3.1
    carries the fix and is the first real published release of this phase's work.
    treatment now (`needs: [version, release]`), and the standalone workflow file is deleted.
  - What did work on the first try: macOS notarization (real Developer ID sign + notarize +
    staple, not just "the secrets exist" — this is the first actual verification since the
    2026-09-26 provisioning-profile work), both Linux `.AppImage`s built successfully, and
    `publish-silo` published real `.deb`/`.rpm`/`.pkg.tar.zst` for both v0.3.0 and v0.3.1 to the
    live silo server (independently confirmed via `silo list --repo kubyl --channel stable`, not
    just the green checkmark). `publish-winget` failed as expected — the winget PR
    ([#442272](https://github.com/microsoft/winget-pkgs/pull/442272)) hasn't been merged yet, so
    `wingetcreate update` has no existing package to update against.
  - winget PR #442272: all nine automated checks finished. Eight passed; `08. Installation
    Validation` came back `NEUTRAL` with label `Validation-Executable-Error`, which Microsoft's
    own docs define as "the test was unable to locate the primary application" — expected for a
    `zip`+`portable` installer type, which doesn't register in Add/Remove Programs the way a
    traditional installer does. Per that same doc's instruction for exactly this case, left an
    explanatory comment on the PR. `10. Validation Completed: SUCCESS` overall; now in the
    `New-Package` queue for a human moderator (see the "no human involved?" discussion — the
    checks are the first gate, not the whole review).
- **2026-10-08**: Flatpak is back, published to silo instead of Flathub (branch `chore/flatpak-silo`). The old Flathub manifest used the freedesktop runtime, which has no GTK 3 or WebKitGTK, so web views could not have worked; the new one uses `org.gnome.Platform//48`. Also fixed: the desktop file's `Icon=kubyl` did not match the app-id-named icons. Untested: written without flatpak available locally, the first release run is the test (check the runtime version exists on Flathub, that `silo publish` accepts the bundle, and that WebKit starts inside the sandbox). Exec-plugin kubeconfigs (`aws`, `gcloud`) cannot run inside the sandbox.
