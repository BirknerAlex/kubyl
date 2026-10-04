#!/usr/bin/env bash
# Fails when a GPUI-free crate (kubyl_base, kubyl_*_core) depends on GPUI or Kubyl's UI crates,
# directly or through another crate. Run from the repository root; CI runs it on every push.
set -euo pipefail

forbidden='^(gpui-pre|gpui-pre-platform|gpui-component|gpui-base|kubyl_ui|kubyl_core) '
status=0
for manifest in crates/kubyl_base/Cargo.toml crates/kubyl_*_core/Cargo.toml; do
  [ -f "$manifest" ] || continue
  crate=$(basename "$(dirname "$manifest")")
  found=$(cargo tree -p "$crate" -e normal --prefix none --all-features | sort -u | grep -E "$forbidden" || true)
  if [ -n "$found" ]; then
    echo "error: $crate must not depend on GPUI or the UI crates, but its tree has:"
    echo "$found" | sed 's/^/  /'
    status=1
  else
    echo "ok: $crate"
  fi
done
exit $status
