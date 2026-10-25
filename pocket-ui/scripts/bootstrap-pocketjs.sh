#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEST="$ROOT/vendor/pocketjs"
REV=1c248fb18f976e91a313f11e0dbb35273c9e48dd
REPO=https://github.com/pocket-nexus/pocketjs.git
PATCH="$ROOT/pocket-ui/vendor-patches/pocketjs-$REV.patch"
LOCAL_PATCH="$ROOT/vendor/.pocketjs-bootstrap.patch"

fail() { printf 'bootstrap-pocketjs: %s\n' "$*" >&2; exit 1; }
verify_source() {
  local dir=$1 actual
  actual=$(git -C "$dir" rev-parse HEAD 2>/dev/null) || fail "not a Git checkout: $dir"
  [[ "$actual" == "$REV" ]] || fail "expected upstream commit $REV, got $actual"
}
cleanup() {
  rm -f "$DEST/.bootstrap-actual.patch" "$DEST/.bootstrap-expected.patch" "$DEST/.bootstrap-expected-local.patch" "$LOCAL_PATCH"
}
trap cleanup EXIT

if [[ -d "$DEST/.git" ]]; then
  verify_source "$DEST"
  git -C "$DEST" diff --cached --quiet || fail "refusing to overwrite staged modifications"
  awk '/^diff --git / { if ($3 == "a/PROVENANCE.md") exit; emit=1 } emit { print }' "$PATCH" > "$LOCAL_PATCH"
  git -C "$DEST" apply --reverse --check "$LOCAL_PATCH" || fail "existing checkout does not match parent patch"
  git -C "$DEST" diff --binary --full-index > "$DEST/.bootstrap-actual.patch"
  cp "$LOCAL_PATCH" "$DEST/.bootstrap-expected-local.patch"
  cmp -s "$DEST/.bootstrap-actual.patch" "$DEST/.bootstrap-expected-local.patch" || fail "tracked patch differs from parent inventory"
else
  [[ ! -e "$DEST" ]] || fail "destination exists but is not a Git checkout: $DEST"
  git init -q "$DEST"
  git -C "$DEST" remote add origin "$REPO"
  git -C "$DEST" fetch --quiet --depth 1 origin "$REV"
  git -C "$DEST" checkout --quiet --detach FETCH_HEAD
  verify_source "$DEST"
  git -C "$DEST" apply --check "$PATCH" || fail "parent patch does not apply to pinned source"
  git -C "$DEST" apply "$PATCH"
fi

git -C "$DEST" diff --check
git -C "$DEST" diff --binary --full-index > "$DEST/.bootstrap-actual.patch"
awk '/^diff --git / { emit=1 } emit { print }' "$LOCAL_PATCH" > "$DEST/.bootstrap-expected-local.patch"
if ! cmp -s "$DEST/.bootstrap-actual.patch" "$DEST/.bootstrap-expected-local.patch"; then
  git -C "$DEST" apply --reverse --check "$LOCAL_PATCH" || fail "patched checkout does not match parent inventory"
  git -C "$DEST" apply --check "$LOCAL_PATCH" || fail "patch does not apply cleanly to pinned source"
  git -C "$DEST" apply "$LOCAL_PATCH"
  git -C "$DEST" diff --binary --full-index > "$DEST/.bootstrap-actual.patch"
  cmp -s "$DEST/.bootstrap-actual.patch" "$DEST/.bootstrap-expected-local.patch" || fail "tracked patch differs from approved inventory"
fi
git -C "$DEST" diff --check || fail "patched checkout contains whitespace errors"
awk '/^diff --git / { emit=($3 == "a/PROVENANCE.md") } emit && /^\+/ && !/^\+\+\+/ { sub(/^\+/, ""); print }' "$PATCH" > "$DEST/.bootstrap-provenance.expected"
cmp -s "$DEST/.bootstrap-provenance.expected" "$DEST/PROVENANCE.md" || fail "provenance differs from parent inventory"
rm -f "$DEST/.bootstrap-provenance.expected"
test -f "$DEST/PROVENANCE.md" || fail "parent patch did not create provenance record"
[[ -z "$(git -C "$DEST" ls-files --others --exclude-standard | grep -Ev "^(PROVENANCE\.md|\.bootstrap-.*\.patch)$" || true)" ]] || fail "refusing unknown untracked files"
printf 'PocketJS %s prepared and patch inventory verified.\n' "$REV"
