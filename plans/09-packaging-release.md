# Phase 09: Packaging, release, auto-update, hardening

**Status:** in progress (manual build pipeline landed 2026-09-24; distribution jobs and
auto-update landed 2026-09-27; hardening and docs site still open)
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
- [ ] Linux AppImage: standalone executable container
- [ ] **Linux Flatpak (Flathub)**: manifest `io.github.birkneralex.Kubyl.yml` in repo, auto-update via `.github/workflows/publish-flathub.yml` on release (updates sha256, commits manifest). Infra done; submission to flathub/flathub still open (see `docs/FLATHUB.md`)
- [ ] Per-platform "open with / register URL handler" `kubyl://` for deep links (open a context/namespace/resource)
- [ ] CLI shim `kubyl` (optional), e.g. `kubyl --context prod -n payments pods`

### Release and auto-update
- [x] Manual release workflow (`.github/workflows/release.yml`, `workflow_dispatch` only — merges to `main` never trigger it): build matrix, macOS sign+notarize, upload to GitHub Releases, checksums (`SHA256SUMS`)
- [x] **Distribution**: post-build jobs publish packages: GitHub Releases (all platforms), `publish-winget` (winget manifest PR), `publish-silo` (silo CLI to `kubyl` repo). macOS Homebrew auto-bumping lives entirely in homebrew-tap's own scheduled workflow (no push from kubyl CI)
- [ ] SBOM (`cargo cyclonedx`), provenance attestation
- [x] **Auto-update**: new crate `kubyl_selfupdate`. Signed update manifest (`updates-stable.json`/`updates-preview.json`, ed25519 via `minisign`, public key hardcoded in `verify.rs`), `build-update-manifest` CI job signs and publishes it as a release asset. Background check (6 h poll) → download → sha256 verify → `self_replace` the running executable → status bar item "restart to update to vX.Y.Z" (click relaunches, never automatic). `updates_app.channel`/`updates_app.auto_check` in `settings.json`. Homebrew, winget, apt/dnf/pacman, Flatpak and snap installs are detected (`installed::detect`) and skip self-update entirely, deferring to their own tool
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
- [ ] README with badges, contributing guide, code of conduct, `LICENSE-MIT`/`LICENSE-APACHE` referenced

## Acceptance criteria

- Clean-machine install works from the DMG, Homebrew cask, winget, silo packages, AppImage and Flatpak. The app launches, updates itself (manual installs only — package-manager installs defer to their own tool) and signature checks pass.
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
  done. Not implemented: Windows/EV signing, AppImage/Flatpak/MSI/winget/Homebrew, auto-update,
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
- **2026-09-27**: Finalized distribution strategy and Flathub setup. **macOS**: Homebrew formula in homebrew-tap with auto-bumping (similar to silo). Create `kubyl.rb` formula and `bump-kubyl.sh` script to check releases every 3 hours and open PRs. **Windows**: unsigned `.zip` published to GitHub Releases + winget manifest PR to `microsoft/winget-pkgs`. **Linux packages**: `.deb`, `.rpm`, `.pkg.tar.zst` to silo repo `kubyl` channel `stable` via silo CLI (actions helper from `/Users/alexander/Projects/silo`). **Flathub**: Created manifest `io.github.birkneralex.Kubyl.yml` (pre-built binary from releases, references desktop file and icons from repo). Created `.github/workflows/publish-flathub.yml` that triggers on release, runs `script/update-flatpak-manifest.sh` to fetch sha256 and commit updated manifest back to main. Flathub CI automatically rebuilds on tag. **Submission**: Fork `flathub/flathub`, add manifest, submit PR (see `docs/FLATHUB.md`). After approval, Flathub CI watches this repo. Implementation: update `.github/workflows/release.yml` for winget/silo; Homebrew auto-update via homebrew-tap; manifest auto-update via publish-flathub. Setup: add `SILO_KUBYL_TOKEN` secret for Linux package publishing.
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
    (`updates_app.channel`/`.auto_check`). Wired into `crates/kubyl/src/main.rs` (append-only line).
    New workspace deps: `minisign-verify`, `self-replace`, `semver`, `zip` (Windows-only target dep).
  - Full validation: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`
    (clean), `cargo test --workspace` (all green, `kubyl_selfupdate` alone: 10 unit + 2 integration),
    `cargo deny check` (advisories/bans/licenses/sources all ok).
  - Not done: Windows Authenticode signing (unchanged — winget accepts unsigned zips, so this was
    deliberately not blocking), SBOM/provenance, third-party notices, AppImage, Flatpak's actual
    Flathub submission (infra ready, PR not filed), `package.kubyl.dev` (silo alias — `kubyl.dev`
    itself is registered now, see below), and the whole Hardening section. Nothing in this session
    has run against a real release yet — `build-update-manifest`/`publish-winget`/`publish-silo`
    are new and their first real invocation is the test, same caveat as the macOS provisioning
    profile in the 2026-09-26 entry above.
- **2026-09-27 (later still)**: `kubyl.dev` is registered and live (landing page, `/download`,
  `/sponsor`, footer `/imprint` and `/privacy`; feature breakdown, roadmap and screenshots from the
  real app). No deeper docs yet — no install guide, kubeconfig/auth guide, OIDC setup page,
  keybindings reference or k9s migration guide; those still need their own pages. `package.kubyl.dev`
  as a silo alias is unblocked now that the domain exists, still not done.
