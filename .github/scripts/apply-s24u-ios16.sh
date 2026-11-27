#!/usr/bin/env bash
set -euo pipefail

target_branch=codex/s24u-ios16
if [[ "${GITHUB_REF_NAME:-}" != "$target_branch" && "${GITHUB_BASE_REF:-}" != "$target_branch" ]]; then
  echo "S24 iOS 16.5 app overlay: skipped for ${GITHUB_REF_NAME:-local}"
  exit 0
fi

expected_base=2324b00fbb260c559279ea455bd3f1b0eb691f3d
actual_base=$(git -C app rev-parse HEAD)
if [[ "$actual_base" != "$expected_base" ]]; then
  echo "::error::S24 iOS 16.5 overlay expects app base $expected_base, got $actual_base"
  exit 1
fi

patch="$GITHUB_WORKSPACE/.github/patches/s24u-ios16-app.patch"
git -C app apply --check "$patch"
git -C app apply "$patch"
echo "Applied the 17-file S24 iOS 16.5 app change set to the pinned upstream app base."
