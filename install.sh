#!/bin/sh
set -eu

REPOSITORY="antiburn/antiburn"
GITHUB_URL="https://github.com/${REPOSITORY}"
TMP_DIR=""
MOUNT_POINT=""
MACOS_STAGING_ROOT=""
MACOS_BACKUP=""
MACOS_DESTINATION=""
APPIMAGE_STAGED=""
APPIMAGE_BACKUP=""
APPIMAGE_DESTINATION=""
APPIMAGE_NEW_INSTALLED="0"
APPIMAGE_SWAP_COMPLETE="0"

info() {
  printf '%s\n' "antiburn: $*"
}

fail() {
  printf '%s\n' "antiburn: error: $*" >&2
  exit 1
}

cleanup() {
  if [ -n "$MACOS_BACKUP" ] && [ -e "$MACOS_BACKUP" ] && [ ! -e "$MACOS_DESTINATION" ]; then
    as_root mv "$MACOS_BACKUP" "$MACOS_DESTINATION" >/dev/null 2>&1 || true
  fi
  if [ -n "$MACOS_STAGING_ROOT" ]; then
    as_root rm -rf "$MACOS_STAGING_ROOT" >/dev/null 2>&1 || true
  fi
  if [ -n "$APPIMAGE_STAGED" ]; then
    rm -f "$APPIMAGE_STAGED"
  fi
  if [ "$APPIMAGE_SWAP_COMPLETE" != "1" ]; then
    if [ "$APPIMAGE_NEW_INSTALLED" = "1" ]; then
      rm -f "$APPIMAGE_DESTINATION"
    fi
    if [ -n "$APPIMAGE_BACKUP" ] && [ -e "$APPIMAGE_BACKUP" ]; then
      mv "$APPIMAGE_BACKUP" "$APPIMAGE_DESTINATION" >/dev/null 2>&1 || true
      APPIMAGE_BACKUP=""
    fi
  fi
  if [ -n "$APPIMAGE_BACKUP" ]; then
    rm -f "$APPIMAGE_BACKUP"
  fi
  if [ -n "$MOUNT_POINT" ]; then
    hdiutil detach "$MOUNT_POINT" -quiet >/dev/null 2>&1 || true
  fi
  if [ -n "$TMP_DIR" ]; then
    rm -rf "$TMP_DIR"
  fi
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "$1 is required but was not found in PATH."
}

as_root() {
  if [ "$(id -u)" -eq 0 ]; then
    "$@"
  else
    require_command sudo
    sudo "$@"
  fi
}

download() {
  url="$1"
  output="$2"
  curl --fail --silent --show-error --location \
    --proto '=https' --proto-redir '=https' \
    --retry 3 --retry-delay 1 \
    --output "$output" "$url"
}

validate_version() {
  case "$1" in
    '' | *[!0-9A-Za-z.-]*) fail "Invalid version: $1" ;;
  esac
}

resolve_release() {
  requested_version="$1"
  if [ -n "$requested_version" ]; then
    validate_version "$requested_version"
    VERSION="$requested_version"
    TAG="antiburn-v${VERSION}"
    return
  fi

  info "Resolving the latest release"
  effective_url=$(curl --fail --silent --show-error --location \
    --proto '=https' --proto-redir '=https' \
    --retry 3 --retry-delay 1 \
    --output /dev/null --write-out '%{url_effective}' \
    "${GITHUB_URL}/releases/latest") || fail "Could not resolve the latest release."
  TAG=${effective_url##*/}
  case "$TAG" in
    antiburn-v*) VERSION=${TAG#antiburn-v} ;;
    *) fail "GitHub returned an invalid release tag: $TAG" ;;
  esac
  validate_version "$VERSION"
  [ "$TAG" = "antiburn-v${VERSION}" ] || fail "GitHub returned an invalid release tag: $TAG"
}

expected_checksum() {
  checksum_file="$1"
  asset_name="$2"
  matches=$(awk -v name="$asset_name" '
    $2 == name || $2 == "*" name {
      if ($1 ~ /^[0-9A-Fa-f]{64}$/) print tolower($1)
    }
  ' "$checksum_file")
  count=$(printf '%s\n' "$matches" | awk 'NF { count++ } END { print count + 0 }')
  [ "$count" -eq 1 ] || fail "SHA256SUMS must contain exactly one valid entry for ${asset_name}."
  printf '%s\n' "$matches"
}

actual_checksum() {
  file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{ print tolower($1) }'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | awk '{ print tolower($1) }'
  else
    fail "sha256sum or shasum is required to verify the download."
  fi
}

verify_checksum() {
  file="$1"
  checksum_file="$2"
  asset_name=${file##*/}
  expected=$(expected_checksum "$checksum_file" "$asset_name")
  actual=$(actual_checksum "$file")
  [ "$actual" = "$expected" ] || fail "Checksum verification failed for ${asset_name}."
  info "Verified SHA-256 for ${asset_name}"
}

verify_attestation_if_requested() {
  file="$1"
  if [ "${ANTIBURN_VERIFY_ATTESTATION:-0}" != "1" ]; then
    return
  fi
  require_command gh
  gh release verify-asset "$TAG" "$file" --repo "$REPOSITORY" >/dev/null \
    || fail "GitHub could not verify the release attestation for ${file##*/}."
  info "Verified the GitHub release attestation"
}

verify_macos_app() {
  app="$1"
  [ -d "$app" ] || fail "The DMG does not contain antiburn.app."
  codesign --verify --deep --strict "$app" >/dev/null 2>&1 \
    || fail "The antiburn application signature is invalid."
  spctl --assess --type execute "$app" >/dev/null 2>&1 \
    || fail "Gatekeeper did not accept antiburn.app."
  bundle_id=$(defaults read "$app/Contents/Info" CFBundleIdentifier 2>/dev/null || true)
  [ "$bundle_id" = "ai.antiburn.desktop" ] \
    || fail "The application has an unexpected bundle identifier: ${bundle_id:-missing}."
}

install_macos() {
  arch="$1"
  require_command hdiutil
  require_command codesign
  require_command spctl
  require_command defaults
  require_command ditto
  require_command id
  require_command sw_vers

  macos_major=$(sw_vers -productVersion | awk -F. '{ print $1 }')
  case "$macos_major" in
    '' | *[!0-9]*) fail "Could not determine the macOS version." ;;
  esac
  [ "$macos_major" -ge 13 ] || fail "antiburn requires macOS 13 or later."

  case "$arch" in
    arm64) arch_label="aarch64" ;;
    x86_64) arch_label="x64" ;;
    *) fail "Unsupported macOS architecture: $arch" ;;
  esac

  asset_name="antiburn_${VERSION}_${arch_label}.dmg"
  asset_path="${TMP_DIR}/${asset_name}"
  download_release_asset "$asset_name" "$asset_path"
  hdiutil verify "$asset_path" >/dev/null || fail "The DMG failed its internal verification."

  MOUNT_POINT="${TMP_DIR}/mount"
  mkdir "$MOUNT_POINT"
  hdiutil attach "$asset_path" -readonly -nobrowse -mountpoint "$MOUNT_POINT" >/dev/null \
    || fail "Could not mount the DMG."
  source_app="${MOUNT_POINT}/antiburn.app"
  verify_macos_app "$source_app"

  MACOS_DESTINATION="/Applications/antiburn.app"
  MACOS_STAGING_ROOT=$(as_root mktemp -d "/Applications/.antiburn-install.XXXXXX") \
    || fail "Could not create a staging directory in /Applications."
  as_root chmod 755 "$MACOS_STAGING_ROOT"
  staged="${MACOS_STAGING_ROOT}/antiburn.app"
  MACOS_BACKUP="${MACOS_STAGING_ROOT}/previous.app"
  info "Installing to ${MACOS_DESTINATION}"
  as_root ditto "$source_app" "$staged"
  verify_macos_app "$staged"
  if [ -e "$MACOS_DESTINATION" ]; then
    as_root mv "$MACOS_DESTINATION" "$MACOS_BACKUP"
  fi
  if ! as_root mv "$staged" "$MACOS_DESTINATION"; then
    if [ -e "$MACOS_BACKUP" ]; then
      as_root mv "$MACOS_BACKUP" "$MACOS_DESTINATION" || true
    fi
    fail "Could not replace ${MACOS_DESTINATION}."
  fi
  as_root rm -rf "$MACOS_BACKUP"
  MACOS_BACKUP=""
  as_root rm -rf "$MACOS_STAGING_ROOT"
  MACOS_STAGING_ROOT=""
  info "Installed antiburn ${VERSION} to ${MACOS_DESTINATION}"
}

install_deb() {
  require_command id
  asset_name="antiburn_${VERSION}_amd64.deb"
  asset_path="${TMP_DIR}/${asset_name}"
  download_release_asset "$asset_name" "$asset_path"

  package_name=$(dpkg-deb -f "$asset_path" Package)
  package_arch=$(dpkg-deb -f "$asset_path" Architecture)
  package_version=$(dpkg-deb -f "$asset_path" Version)
  [ "$package_name" = "antiburn" ] || fail "The Debian package has an unexpected name: $package_name."
  [ "$package_arch" = "amd64" ] || fail "The Debian package has an unexpected architecture: $package_arch."
  [ "$package_version" = "$VERSION" ] || fail "The Debian package has an unexpected version: $package_version."

  info "Installing the Debian package"
  # Debian compares a hyphenated prerelease as newer than the stable version.
  # The selected local package is already pinned to the requested release and verified.
  as_root apt-get install --yes --allow-downgrades "$asset_path"
  info "Installed antiburn ${VERSION} with APT"
}

install_appimage() {
  [ -n "${HOME:-}" ] || fail "HOME is required for an AppImage installation."
  asset_name="antiburn_${VERSION}_amd64.AppImage"
  asset_path="${TMP_DIR}/${asset_name}"
  download_release_asset "$asset_name" "$asset_path"

  applications_dir="${HOME}/Applications"
  bin_dir="${HOME}/.local/bin"
  destination="${applications_dir}/antiburn.AppImage"
  APPIMAGE_DESTINATION="$destination"
  APPIMAGE_STAGED="${applications_dir}/.antiburn.AppImage.$$"
  APPIMAGE_BACKUP="${applications_dir}/.antiburn-backup.$$"
  mkdir -p "$applications_dir" "$bin_dir"
  chmod 755 "$asset_path"
  mv "$asset_path" "$APPIMAGE_STAGED"
  if [ -e "$destination" ]; then
    mv "$destination" "$APPIMAGE_BACKUP"
  else
    APPIMAGE_BACKUP=""
  fi
  APPIMAGE_NEW_INSTALLED="1"
  mv -f "$APPIMAGE_STAGED" "$destination"
  APPIMAGE_STAGED=""
  APPIMAGE_SWAP_COMPLETE="1"
  if [ -n "$APPIMAGE_BACKUP" ]; then
    rm -f "$APPIMAGE_BACKUP"
    APPIMAGE_BACKUP=""
  fi
  link_path="${bin_dir}/antiburn"
  if [ -L "$link_path" ]; then
    require_command readlink
    if [ "$(readlink "$link_path")" != "$destination" ]; then
      info "Not replacing the existing link at ${link_path}."
    fi
  elif [ -e "$link_path" ]; then
    info "Not replacing the existing path at ${link_path}."
  elif ! ln -s "$destination" "$link_path"; then
    info "Could not create the optional command link at ${link_path}."
  fi
  info "Installed antiburn ${VERSION} to ${destination}"
  case ":${PATH:-}:" in
    *":${bin_dir}:"*) ;;
    *) info "Add ${bin_dir} to PATH to run antiburn from a terminal." ;;
  esac
}

install_linux() {
  arch="$1"
  case "$arch" in
    x86_64 | amd64) ;;
    *) fail "Unsupported Linux architecture: $arch" ;;
  esac

  if command -v apt-get >/dev/null 2>&1 && command -v dpkg-deb >/dev/null 2>&1; then
    install_deb
  else
    install_appimage
  fi
}

download_release_asset() {
  asset_name="$1"
  asset_path="$2"
  release_url="${GITHUB_URL}/releases/download/${TAG}"
  checksum_path="${TMP_DIR}/SHA256SUMS"
  if [ ! -f "$checksum_path" ]; then
    download "${release_url}/SHA256SUMS" "$checksum_path" \
      || fail "Could not download SHA256SUMS for ${TAG}."
  fi
  info "Downloading ${asset_name}"
  download "${release_url}/${asset_name}" "$asset_path" \
    || fail "Could not download ${asset_name}."
  verify_checksum "$asset_path" "$checksum_path"
  verify_attestation_if_requested "$asset_path"
}

parse_args() {
  VERSION_REQUESTED="${ANTIBURN_VERSION:-}"
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --version)
        [ "$#" -ge 2 ] || fail "--version requires a value."
        VERSION_REQUESTED="$2"
        shift 2
        ;;
      --help)
        printf '%s\n' "Usage: install.sh [--version VERSION]"
        exit 0
        ;;
      *) fail "Unknown argument: $1" ;;
    esac
  done
}

install_antiburn() {
  parse_args "$@"
  require_command curl
  require_command awk
  require_command mktemp
  require_command uname
  TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/antiburn-install.XXXXXX") \
    || fail "Could not create a temporary directory."
  trap cleanup EXIT
  trap 'exit 129' HUP
  trap 'exit 130' INT
  trap 'exit 143' TERM

  resolve_release "$VERSION_REQUESTED"
  os=$(uname -s)
  arch=$(uname -m)
  info "Installing antiburn ${VERSION} for ${os}/${arch}"
  case "$os" in
    Darwin) install_macos "$arch" ;;
    Linux) install_linux "$arch" ;;
    *) fail "Unsupported operating system: $os" ;;
  esac
}

install_antiburn "$@"
