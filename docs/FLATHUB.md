# Flathub Submission & Publishing

Kubyl is published to Flathub, the community app store for Linux Flatpak applications.

## Automatic Updates

Every time a release is tagged in this repo, `.github/workflows/publish-flathub.yml`:
1. Extracts the version from the git tag
2. Fetches the sha256 checksum from the GitHub release
3. Updates `io.github.birkneralex.Kubyl.yml` with the new version and checksum
4. Commits the updated manifest back to `main`

Flathub's CI automatically detects the new manifest and rebuilds the app.

## One-Time Submission to Flathub

The manifest is already created at `io.github.birkneralex.Kubyl.yml` and ready for submission. Follow these steps:

### 1. Fork flathub/flathub

```bash
# Visit https://github.com/flathub/flathub and fork it
# Then clone your fork locally
git clone https://github.com/YOUR_USERNAME/flathub
cd flathub
git checkout -b add/io.github.birkneralex.Kubyl
```

### 2. Add the manifest

```bash
# Copy the manifest from kubyl repo to flathub
cp /path/to/kubyl/io.github.birkneralex.Kubyl.yml \
   flathub/io.github.birkneralex.Kubyl/io.github.birkneralex.Kubyl.yml

# Create CHANGELOG.md for initial submission
cat > flathub/io.github.birkneralex.Kubyl/CHANGELOG.md << 'EOF'
# Initial Flathub submission
- Initial release of Kubyl on Flathub
EOF

git add io.github.birkneralex.Kubyl/
git commit -m "Add io.github.birkneralex.Kubyl"
git push origin add/io.github.birkneralex.Kubyl
```

### 3. Submit PR to flathub/flathub

- Visit your fork on GitHub and open a PR to `flathub/flathub:master`
- Flathub maintainers will review, check for compliance (permissions, dependencies, security)
- They may ask to adjust permissions in `finish-args` (network, device, filesystem access)
- Once approved, they merge and set up CI

### 4. After Approval

Once merged:
- Flathub CI is configured to watch this repo (`github.com/BirknerAlex/kubyl`)
- Every time you tag a release, Flathub automatically:
  1. Detects the new tag
  2. Reads the updated manifest from your repo
  3. Builds the new version
  4. Publishes to Flathub

- Users can install with: `flatpak install flathub io.github.birkneralex.Kubyl`
- Users can update with: `flatpak update`

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

- [Flathub Contributing Guide](https://docs.flathub.org/submission/)
- [Flatpak Manifest Format](https://docs.flatpak.org/en/latest/manifests.html)
- [Flathub CI Setup](https://docs.flathub.org/en/latest/maintenance/application-updates.html)
