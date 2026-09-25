# Phase 09: Packaging, release, auto-update, hardening

**Status:** in progress (manual release pipeline landed 2026-09-24; auto-update, hardening, docs
site still open)
**Depends on:** 00 for CI (already running), then feature phases for the release
**Owns:** `script/bundle-*`, `.github/workflows/release.yml`, bundling metadata in `crates/kubyl`
**Mockups:** none (logo/icons in `assets/logo`)

## Goal

Signed, installable, auto-updating builds for macOS, Windows and Linux, plus the polish needed for
a public 1.0: accessibility, performance budgets, crash reporting (opt-in) and docs.

## Tasks

### Packaging
- [x] **macOS**: universal (arm64 + x86_64) `.app` with `kubyl.icns`, hardened runtime, Developer ID signing, notarization and stapling, `.dmg`. No custom DMG background yet
- [ ] macOS Homebrew cask
- [ ] **Windows**: MSI (WiX) or MSIX with `kubyl.ico`, Authenticode signing (Azure Trusted Signing or an EV cert), winget manifest. `.zip` for both archs ships unsigned today (`docs/RELEASING.md` tracks this gap)
- [x] Windows arm64 build (`aarch64-pc-windows-msvc`, cross-compiled)
- [x] **Linux**: `.deb`, `.rpm`, amd64 Arch `.pkg.tar.zst`. `.desktop` file (`packaging/linux/kubyl.desktop`) plus hicolor icons. Wayland and X11
- [ ] Linux AppImage and Flatpak (Flathub)
- [ ] Per-platform "open with / register URL handler" `kubyl://` for deep links (open a context/namespace/resource)
- [ ] CLI shim `kubyl` (optional), e.g. `kubyl --context prod -n payments pods`

### Release and auto-update
- [x] Manual release workflow (`.github/workflows/release.yml`, `workflow_dispatch` only — merges to `main` never trigger it): build matrix, macOS sign+notarize, upload to GitHub Releases, checksums (`SHA256SUMS`)
- [ ] SBOM (`cargo cyclonedx`), provenance attestation
- [ ] Auto-update: signed update manifest (ed25519), stable and preview channels, background download, "Restart to update" in the title bar (Zed-style). Linux package-manager installs skip self-update
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
- [ ] `kubyl.dev` (domain was unregistered on 2026-09-24, register it): landing page with the lockup from `assets/logo`, docs (install, kubeconfig/auth guide, OIDC setup, keybindings, k9s migration guide), screenshots from the real app
- [ ] README with badges, contributing guide, code of conduct, `LICENSE-MIT`/`LICENSE-APACHE` referenced

## Acceptance criteria

- Clean-machine install works from the DMG, MSI, AppImage and Flatpak. The app launches, updates itself (non-Linux-PM builds) and signature checks pass.
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
