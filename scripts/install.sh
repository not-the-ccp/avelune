#!/bin/sh
# Install the Avelune CLI from source.
#
#     curl -fsSL https://raw.githubusercontent.com/not-the-ccp/avelune/main/scripts/install.sh | sh
#
# The script clones the repository, builds `avelune` with the user's Rust
# toolchain, and installs the binary. Avelune has no prebuilt release
# artifacts; the build needs git and cargo (Rust 1.97.1 is pinned by the
# repository toolchain file). Ordinary media workflows additionally need
# FFmpeg (ffmpeg, ffprobe, ffplay) on PATH.
#
# Environment overrides:
#   AVELUNE_REPO      git URL to build from (default: the public GitHub repository)
#   AVELUNE_REF       branch, tag, or commit to build (default: newest v* tag, else main)
#   INSTALL_PREFIX    directory that owns bin/ (default: /usr/local if writable, else ~/.local)

set -eu

repo="${AVELUNE_REPO:-https://github.com/not-the-ccp/avelune.git}"
ref="${AVELUNE_REF:-}"
bin_name="avelune"

note() { printf 'avelune: %s\n' "$*" >&2; }
fail() { note "error: $*"; exit 1; }

command -v git >/dev/null 2>&1 || fail "git not found on PATH"
command -v cargo >/dev/null 2>&1 || fail "cargo not found on PATH; install Rust first (https://rustup.rs)"

if command -v ffprobe >/dev/null 2>&1; then
  note "FFmpeg detected; ordinary encode/decode/play use it at the media boundary"
else
  note "warning: FFmpeg not found; encode/decode/play of ordinary media need ffmpeg on PATH"
fi

if [ -z "$ref" ]; then
  tag=$(git ls-remote --refs --sort=-v:refname --tags "$repo" 'v*' 2>/dev/null \
    | awk 'NR == 1 { print $2 }' | sed 's#^refs/tags/##' || :)
  if [ -n "${tag:-}" ]; then
    ref="$tag"
  else
    ref="main"
  fi
fi
note "building $bin_name from $repo ($ref)"

work=$(mktemp -d "${TMPDIR:-/tmp}/avelune-install.XXXXXX")
trap 'rm -rf "$work"' EXIT INT TERM

if ! git clone --quiet --depth 1 --branch "$ref" "$repo" "$work/src"; then
  git clone --quiet "$repo" "$work/src"
  git -C "$work/src" checkout --quiet "$ref"
fi
( cd "$work/src" && cargo build --release --locked -p avelune-cli )

built="$work/src/target/release/$bin_name"
[ -f "$built" ] || fail "build finished but $built is missing"

if [ -n "${INSTALL_PREFIX:-}" ]; then
  prefix="$INSTALL_PREFIX"
elif [ -w /usr/local ] || [ ! -d /usr/local ]; then
  prefix=/usr/local
else
  prefix="$HOME/.local"
fi
bin_dir="$prefix/bin"
mkdir -p "$bin_dir" || fail "cannot create $bin_dir; rerun with INSTALL_PREFIX set to a writable prefix"

if ! install -m 0755 "$built" "$bin_dir/$bin_name" 2>/dev/null; then
  cp "$built" "$bin_dir/$bin_name"
  chmod 0755 "$bin_dir/$bin_name"
fi

"$bin_dir/$bin_name" --version
case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) note "note: add $bin_dir to PATH to use $bin_name directly" ;;
esac
note "installed $bin_dir/$bin_name"
note "start with: $bin_name --help"