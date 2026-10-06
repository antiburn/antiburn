#!/usr/bin/env bash
set -euo pipefail

target=${1:?usage: package-remote-helper.sh TARGET OUTPUT_DIRECTORY}
output=${2:?missing output directory}

# The crate manifest is the only version source. The archive name, `--version`
# and the `hello` response all come from it, so they cannot disagree. The
# release tag gate (scripts/verify-release-version.mjs) holds it equal to the
# application version.
version=$(cargo metadata --format-version 1 --no-deps \
  --manifest-path crates/antiburn-remote/Cargo.toml \
  | jq -r '.packages[] | select(.name == "antiburn-remote") | .version')
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
  echo "Invalid helper archive version" >&2
  exit 1
fi
case "$target" in
  x86_64-unknown-linux-musl|aarch64-unknown-linux-musl) ;;
  *) echo "Unsupported helper target" >&2; exit 1 ;;
esac

binary="${CARGO_TARGET_DIR:-crates/antiburn-remote/target}/$target/release/antiburn-remote"
epoch=${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct HEAD)}
if [[ ! "$epoch" =~ ^[0-9]+$ ]]; then
  echo "Invalid source timestamp" >&2
  exit 1
fi
test -x "$binary"
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
readelf --program-headers "$binary" > "$stage/program-headers"
readelf --dynamic "$binary" > "$stage/dynamic"
if grep -q 'INTERP' "$stage/program-headers" || grep -q '(NEEDED)' "$stage/dynamic"; then
  echo "The remote helper must not require a dynamic loader or shared libraries" >&2
  exit 1
fi

name="antiburn-remote-$version-$target"
mkdir -p "$stage/$name" "$output"
install -m 755 "$binary" "$stage/$name/antiburn-remote"
cp LICENSE NOTICE THIRD_PARTY_NOTICES docs/remote-sessions.md "$stage/$name/"
tar --create --gzip --sort=name \
  --mtime="@$epoch" \
  --owner=0 --group=0 --numeric-owner \
  --directory "$stage" --file "$output/$name.tar.gz" "$name"

mkdir "$stage/extracted"
tar -xzf "$output/$name.tar.gz" -C "$stage/extracted"
cmp "$binary" "$stage/extracted/$name/antiburn-remote"
test -x "$stage/extracted/$name/antiburn-remote"

# The packaged binary must report the version in its own archive name. This
# proves the build used the manifest that named the archive.
reported=$("$stage/extracted/$name/antiburn-remote" --version)
if [[ "$reported" != "antiburn-remote $version" ]]; then
  echo "The helper reports '$reported' but its archive claims version $version" >&2
  exit 1
fi
echo "$reported"
