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
`.zip` (unsigned — see below); Linux `x86_64`/`aarch64` `.tar.gz`, `.deb`, `.rpm`, `.AppImage`;
Arch `.pkg.tar.zst` (amd64 only — Arch Linux is x86_64-only upstream). Plus `SHA256SUMS` and the
signed self-update manifest (`updates-stable.json` + `.minisig`).

The AppImage is built by `script/build-appimage.sh` (`linuxdeploy` + its GTK plugin, pinned to
the commit tauri-bundler validates in production — its own "continuous" build has shipped broken
AppImage output before). webkit2gtk's subprocess helpers (`WebKitWebProcess`,
`WebKitNetworkProcess`) are hand-copied into the AppDir, the same fix tauri-bundler applies for
apps using `wry` (kubyl_webview's underlying webview crate) — neither `linuxdeploy` nor its GTK
plugin's dependency scan finds them on their own, since nothing directly links them. Not
verified on a real Linux desktop yet; the first release is the test, same caveat as everything
else new in this pipeline.

After the GitHub Release publishes, three more jobs distribute it further:

- `build-update-manifest` runs *before* `release` (its output ships as release assets):
  signs `updates-stable.json` with `minisign` so `kubyl_selfupdate` can verify it.
- `publish-winget` submits a manifest update to `microsoft/winget-pkgs` via `wingetcreate`.
- `publish-silo` pushes the `.deb`/`.rpm`/`.pkg.tar.zst` to the `kubyl` repo (channel `stable`)
  on the silo instance at `silo.tyrola.dev`, via the reusable action in
  `BirknerAlex/silo/.github/actions/publish`.

The macOS Homebrew cask (`birkneralex/homebrew-tap`) isn't pushed from this repo's CI at all —
that tap's own scheduled workflow polls `kubyl`'s GitHub releases and opens its own PR (see
"Homebrew cask" below).

## Known gaps (not covered yet)

- **Windows binaries are unsigned.** No Authenticode certificate is configured, so Windows will
  show a SmartScreen warning. Get an EV code-signing cert or set up Azure Trusted Signing, then
  add a signing step to the `build-windows` job.
- No AppImage or MSI/MSIX yet — see `plans/09-packaging-release.md` for the full packaging
  backlog. Flatpak (Flathub) is set up — see `docs/FLATHUB.md`.
- Self-update (`kubyl_selfupdate`) only replaces the binary in a manual `.dmg`/`.zip`/`.tar.gz`
  install. Homebrew, winget, apt/dnf/pacman, Flatpak and snap installs defer to their own tool
  (`kubyl_selfupdate::installed::detect`) and never self-update.

## One-time setup: self-update signing key

`build-update-manifest` signs the update manifest with `minisign` (ed25519). The public half is
hardcoded in `crates/kubyl_selfupdate/src/verify.rs` — only the private half needs to stay
secret.

1. Install `minisign` (`brew install minisign` or your distro's package).
2. Generate an unencrypted key pair (unencrypted so CI can sign non-interactively — nothing
   downstream trusts this key without the manifest first passing the app's own checks, so a
   password on top buys little and would need its own secret anyway):
   ```sh
   minisign -G -W -s minisign.key -p minisign.pub -c "kubyl release signing key"
   ```
3. Add the **secret key file's full contents** (both lines) as repo secret
   `UPDATE_SIGNING_KEY`:
   ```sh
   gh secret set UPDATE_SIGNING_KEY < minisign.key
   ```
4. Delete `minisign.key` from disk once it's in the secret store. If the public key in
   `minisign.pub` differs from the one hardcoded in `verify.rs`, update that constant to match
   (every future manifest is signed with the key from step 2, so the two must agree).

## One-time setup: winget

1. Get a GitHub personal access token with `public_repo` scope (submits PRs to
   `microsoft/winget-pkgs` on your behalf) and add it as repo secret `WINGET_TOKEN`:
   ```sh
   gh secret set WINGET_TOKEN
   ```
2. `publish-winget` only *updates* an existing manifest — the first submission needs the
   package to already exist. **Done**: `BirknerAlex.Kubyl` v0.2.5 was submitted as
   [microsoft/winget-pkgs#442272](https://github.com/microsoft/winget-pkgs/pull/442272) — three
   manifest files (version, installer, `en-US` locale) written by hand against the current
   schema (`1.12.0`) and a real reference package (Deno's, itself zip+portable), rather than
   through `wingetcreate new`'s interactive wizard (this needs a Windows machine or Wine;
   hand-written YAML doesn't). `InstallerType: zip` + `NestedInstallerType: portable` with
   `kubyl.exe` as the relative path is the shape `publish-winget`'s `wingetcreate update` expects
   to find and preserve on every later release.
3. Once that PR is merged, `publish-winget` keeps it current automatically.

## One-time setup: silo

1. Create a repo `kubyl` on the silo instance (`silo.tyrola.dev`) with channel `stable` (or let
   the first publish create it, if your silo version supports that).
2. From the silo CLI, mint a token scoped to the `kubyl` repo and add it as a secret:
   ```sh
   silo login  # if not already
   gh secret set SILO_KUBYL_TOKEN
   ```

## Homebrew cask

Unlike the above, there's nothing to configure in *this* repo — `BirknerAlex/homebrew-tap`
tracks `Casks/kubyl.rb` and its own `bump-formulae` workflow (scheduled every 3 hours) checks
`kubyl`'s latest GitHub release and opens a PR with `brew bump-cask-pr` when it's newer. Merge
that PR to publish. Kubyl's own `.dmg` being one universal binary (not per-arch) means the cask
needs no by-hand bump script, unlike `silo`'s formula.

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
a 10-character alphanumeric string). `codesign` finds the only Developer ID Application identity
in the temporary keychain without it, but `packaging/macos/entitlements.plist` spells it out in
the application identifier and keychain access group (`<team ID>.io.github.birkneralex.Kubyl`).

### 4. Developer ID provisioning profile (keychain without prompts)

Kubyl keeps tokens in the data protection keychain, where items belong to its keychain access
group and macOS never asks for permission (the login keychain asks per item whenever the app's
code signature changes). The `keychain-access-groups` entitlement that this needs is restricted:
macOS kills Kubyl at launch unless a provisioning profile embedded in the app grants it.

1. [Identifiers](https://developer.apple.com/account/resources/identifiers/list) → **+** →
   **App IDs** → **App**. Description `Kubyl`, **Explicit** Bundle ID
   `io.github.birkneralex.Kubyl`. No capabilities needed: the app's own keychain access group
   is always granted.
2. [Profiles](https://developer.apple.com/account/resources/profiles/list) → **+** →
   Distribution → **Developer ID**. Pick the `Kubyl` App ID and the Developer ID Application
   certificate from step 1 (the one in `MACOS_CERTIFICATE_P12`), name it `Kubyl Developer ID`,
   generate and download the `.provisionprofile`.
3. Add it as repo secret `MACOS_PROVISIONING_PROFILE`:
   ```sh
   base64 -i Kubyl_Developer_ID.provisionprofile | gh secret set MACOS_PROVISIONING_PROFILE
   ```

The workflow embeds it as `Kubyl.app/Contents/embedded.provisionprofile`, signs with the
certificate the profile names, and fails before signing when the secret is missing, the profile
has expired, doesn't name the certificate in `MACOS_CERTIFICATE_P12`, or doesn't grant what
`entitlements.plist` claims. macOS checks the profile at every launch: a new or revoked
certificate needs a new profile (Developer ID profiles themselves are valid for about 18 years).

Builds without the profile (`cargo run`, local builds) can't use the data protection keychain
and fall back to the login keychain, which asks after every rebuild; use
`KUBYL_CREDENTIAL_STORE=file` or `memory` there.

### Adding secrets to GitHub

**Settings → Secrets and variables → Actions → New repository secret** for each of:
`MACOS_CERTIFICATE_P12`, `MACOS_CERTIFICATE_PASSWORD`, `MACOS_PROVISIONING_PROFILE`,
`APPLE_API_KEY_P8`, `APPLE_API_KEY_ID`, `APPLE_API_ISSUER`, plus `UPDATE_SIGNING_KEY`,
`WINGET_TOKEN` and `SILO_KUBYL_TOKEN` (below) for the distribution jobs.

### If notarization or launch fails

- `xcrun notarytool submit ... --wait` prints a status; if `Invalid`, fetch the detailed log with
  `xcrun notarytool log <submission-id> --key ... --key-id ... --issuer ...`.
- If the app crashes on launch on a clean machine, check Console.app for a hardened-runtime
  entitlement violation and add the missing key to `packaging/macos/entitlements.plist`.
- If macOS kills Kubyl right at launch ("Killed: 9", AMFI in Console.app), the embedded profile
  doesn't match the signing certificate or the entitlements: regenerate it for the certificate
  in `MACOS_CERTIFICATE_P12` (step 4). `codesign -d --entitlements - Kubyl.app` shows what the
  app claims, `security cms -D -i Kubyl.app/Contents/embedded.provisionprofile` what the profile
  grants.
