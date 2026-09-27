# Flathub Submission & Publishing

Kubyl is published to Flathub, the community app store for Linux Flatpak applications.

## Known risk: prebuilt binary, not a source build

Flathub's policy is that apps build from source; a prebuilt-binary source (what
`io.github.birkneralex.Kubyl.yml` uses today, matching the "use the release artifact" choice for
Kubyl's other package managers) needs an exception granted case by case at review time. There's
no guarantee this manifest is accepted as-is — reviewers may ask for a from-source build instead
(compiling GPUI and its Wayland/X11/Vulkan C deps inside the Flatpak sandbox), which is
substantially more work than anything below. Know this going in; don't be surprised if the first
submission comes back with that ask.

## Keeping this repo's copy current

Every time a release is tagged, `.github/workflows/publish-flathub.yml` runs
`script/update-flatpak-manifest.sh`, which regenerates the whole `sources:` block (both Linux
archs' archive URL/sha256, and the desktop file/icon URLs/sha256 pinned to the new tag) and adds
a `<release>` entry to `io.github.birkneralex.Kubyl.metainfo.xml`, then commits both back to
`main`.

**This keeps this repo's copy current — it does not publish anything to Flathub by itself.**
Flathub only rebuilds on changes to *its own* `flathub/io.github.birkneralex.Kubyl` repo (created
after the initial submission is accepted), not on anything that happens here. Until there's
automation that also pushes to that repo, publishing an update after the initial submission is a
manual step: open a PR against `flathub/io.github.birkneralex.Kubyl` with the refreshed manifest
this workflow just produced.

## One-Time Submission to Flathub

### 1. Clone Flathub's `new-pr` branch

Flathub's submission flow branches off `new-pr`, not the default branch:

```bash
# Visit https://github.com/flathub/flathub and fork it
git clone https://github.com/YOUR_USERNAME/flathub
cd flathub
git checkout new-pr
git checkout -b add-io.github.birkneralex.Kubyl new-pr
```

### 2. Add the manifest and Metainfo

```bash
mkdir io.github.birkneralex.Kubyl
cp /path/to/kubyl/io.github.birkneralex.Kubyl.yml io.github.birkneralex.Kubyl/
cp /path/to/kubyl/io.github.birkneralex.Kubyl.metainfo.xml io.github.birkneralex.Kubyl/

git add io.github.birkneralex.Kubyl/
git commit -m "Add io.github.birkneralex.Kubyl"
git push origin add-io.github.birkneralex.Kubyl
```

### 3. Submit PR to flathub/flathub

- Visit your fork on GitHub and open a PR **against `flathub/flathub`'s `new-pr` branch** (not
  `master`)
- Flathub maintainers will review: permissions, dependencies, security, and — per the risk noted
  above — whether the prebuilt-binary source is acceptable for this app
- They may ask to adjust permissions in `finish-args` (network, device, filesystem access)
- Once approved, Flathub creates `flathub/io.github.birkneralex.Kubyl`, which is what its build
  bot actually watches from then on

### 4. After Approval

- Users can install with: `flatpak install flathub io.github.birkneralex.Kubyl`
- Users can update with: `flatpak update`
- Publishing a new version means pushing the refreshed manifest (see above) to
  `flathub/io.github.birkneralex.Kubyl`, typically via a PR there

## Testing Locally (Before Submission)

To test the Flatpak build before submitting to Flathub:

```bash
# Install Flatpak SDK
flatpak install flathub org.freedesktop.Platform/x86_64/24.08
flatpak install flathub org.freedesktop.Sdk/x86_64/24.08

# Build
flatpak-builder build-dir io.github.birkneralex.Kubyl.yml --user --install --force-clean

# Test the built app
flatpak run io.github.birkneralex.Kubyl
```

## Permissions Explained

The manifest requests:
- `--share=network` — needed for Kubernetes API communication
- `--device=all` — device access (cluster operations)
- `--filesystem=home` — access to kubeconfig and credentials in `~/.kube`
- `--socket=wayland` / `--socket=x11` — display server access
- `--socket=pulseaudio` — audio notifications (if used)
- `--share=ipc` — shared memory for X11

These are standard for a desktop app that needs to access network APIs and home directory config.

## References

- [Flathub Requirements](https://docs.flathub.org/docs/for-app-authors/requirements) (source
  builds, Metainfo, architecture support)
- [Flathub Submission Guide](https://docs.flathub.org/docs/for-app-authors/submission) (the
  `new-pr` branch flow)
- [Flathub App Maintenance](https://docs.flathub.org/docs/for-app-authors/maintenance) (how
  updates actually get published post-acceptance)
- [Flatpak Manifest Format](https://docs.flatpak.org/en/latest/manifests.html)
