# Releasing Kubyl

`.github/workflows/release.yml` is manual-only (`workflow_dispatch`). Merges to `main` never
trigger it — regular CI (`.github/workflows/ci.yml`) still runs on every push and PR.

## Cutting a release

1. Go to **Actions → Release → Run workflow** on `main`.
2. Pick `bump`:
   - `auto` (default) — reads [Conventional Commits](https://www.conventionalcommits.org/) since
     the last tag and picks the level itself (`feat` → minor, `fix` → patch, `!`/`BREAKING CHANGE`
     → major, via [git-cliff](https://git-cliff.org)). Fails loudly if nothing since the last tag
     warrants a bump.
   - `patch` / `minor` / `major` — force a bump regardless of commit messages.
3. Tick `dry_run` to preview the computed version and changelog (printed to the job summary)
   without building, tagging, or publishing anything. Use this to sanity-check before a real cut.
4. On a real run, the `version` job bumps `Cargo.toml`, writes `CHANGELOG.md`, commits
   `chore(release): vX.Y.Z`, tags it, and pushes both to `main` — then the build jobs check out
   that tag and the release job publishes it.

Artifacts published: macOS universal (arm64+x86_64) notarized `.dmg`; Windows `x86_64`/`aarch64`
`.zip` (unsigned — see below); Linux `x86_64`/`aarch64` `.tar.gz`, `.deb`, `.rpm`; Arch
`.pkg.tar.zst` (amd64 only — Arch Linux is x86_64-only upstream). Plus `SHA256SUMS`.

## Known gaps (not covered yet)

- **Windows binaries are unsigned.** No Authenticode certificate is configured, so Windows will
  show a SmartScreen warning. Get an EV code-signing cert or set up Azure Trusted Signing, then
  add a signing step to the `build-windows` job.
- No AppImage, Flatpak, MSI/MSIX, winget manifest, or Homebrew cask yet — see
  `plans/09-packaging-release.md` for the full packaging backlog.
- No auto-update mechanism; users update by re-downloading.

## One-time setup: pushing the version-bump commit to a protected `main`

The `version` job commits `chore(release): vX.Y.Z` and pushes it (plus the tag) straight to
`main`. If `main` has a ruleset requiring PRs/status checks, the default `GITHUB_TOKEN`
(`github-actions[bot]`) can't push directly unless it's a configured bypass actor — and a
personal (non-org) repo's rulesets can't list "GitHub Actions" as an Integration bypass actor at
all. The workaround: add a repo secret `RELEASE_PUSH_TOKEN` containing a token for a user who
*is* listed as a bypass actor on the ruleset (Settings → Rules → your ruleset → Bypass list), e.g.
`gh auth token | gh secret set RELEASE_PUSH_TOKEN`. The `version` job's checkout step uses this
token instead of `GITHUB_TOKEN` so the push succeeds.

## One-time setup: Apple Developer certificates (for macOS signing + notarization)

You said you already have an Apple Developer Program membership — good, that's the only paid
prerequisite. Everything below is done once.

### 1. Developer ID Application certificate

This signs the `.app` so Gatekeeper trusts it.

1. Open **Keychain Access → Certificate Assistant → Request a Certificate From a Certificate
   Authority** and save a Certificate Signing Request (CSR) to disk (email/CA fields can be
   anything; select "Saved to disk").
2. Go to [developer.apple.com/account/resources/certificates](https://developer.apple.com/account/resources/certificates/list),
   click **+**, choose **Developer ID Application**, upload the CSR, and download the resulting
   `.cer`.
3. Double-click the downloaded `.cer` to import it into Keychain Access (login keychain). It
   pairs with the private key generated alongside the CSR.
4. In Keychain Access, find the new "Developer ID Application: ..." certificate, expand it to
   reveal the private key, select **both** the cert and key, right-click → **Export 2 items...**,
   save as `cert.p12` with a password you choose.
5. Base64-encode it for GitHub Actions:
   ```sh
   base64 -i cert.p12 | pbcopy
   ```
   Add as repo secret `MACOS_CERTIFICATE_P12`. Add the password you chose as
   `MACOS_CERTIFICATE_PASSWORD`.

### 2. App Store Connect API key (for `notarytool`)

Used instead of an Apple ID + app-specific password so CI doesn't hit 2FA or expiring passwords.

1. Go to [appstoreconnect.apple.com/access/integrations/api](https://appstoreconnect.apple.com/access/integrations/api),
   click **+** under "Team Keys", give it a name, and set the role to **Developer** (enough for
   notarization).
2. Download the `.p8` file **immediately** — Apple only lets you download it once.
3. Note the **Key ID** and **Issuer ID** shown on that page.
4. Add repo secrets:
   - `APPLE_API_KEY_P8` — the full contents of the `.p8` file
   - `APPLE_API_KEY_ID` — the Key ID
   - `APPLE_API_ISSUER` — the Issuer ID

### 3. Team ID

Shown on your [membership page](https://developer.apple.com/account/#/membership) (top right,
a 10-character alphanumeric string). Not currently used directly by the workflow (there's only
one Developer ID Application identity in the temporary keychain, so `codesign` finds it without
needing the Team ID spelled out) — keep it noted for when Windows/other tooling needs it.

### Adding secrets to GitHub

**Settings → Secrets and variables → Actions → New repository secret** for each of:
`MACOS_CERTIFICATE_P12`, `MACOS_CERTIFICATE_PASSWORD`, `APPLE_API_KEY_P8`, `APPLE_API_KEY_ID`,
`APPLE_API_ISSUER`.

### If notarization or launch fails

- `xcrun notarytool submit ... --wait` prints a status; if `Invalid`, fetch the detailed log with
  `xcrun notarytool log <submission-id> --key ... --key-id ... --issuer ...`.
- If the app crashes on launch on a clean machine, check Console.app for a hardened-runtime
  entitlement violation and add the missing key to `packaging/macos/entitlements.plist`.
