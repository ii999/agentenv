#!/usr/bin/env bash
# Installs a matching agentenv executable bundle from GitHub Releases on macOS
# or Linux, together with the agentenv agent skill.
#
# Usage:
#   ./install.sh [--version <tag>] [--dir <install-dir>] [--claude-skills] [--no-skill]
#
# Options:
#   --version <tag>   Release tag to install, e.g. v0.2.0. Defaults to the
#                     latest release. AGENTENV_VERSION works the same way.
#   --dir <path>      Binary install directory. Defaults to ~/.local/bin.
#                     AGENTENV_INSTALL_DIR works the same way.
#   --claude-skills   Also install the agent skill to ~/.claude/skills for
#                     Claude Code, in addition to the ~/.agents/skills default.
#   --no-skill        Install the executable bundle only.
#
# Downloads use plain HTTPS from GitHub Releases.

set -euo pipefail

readonly repo="ii999/agentenv"

fail() {
    echo "install.sh: $*" >&2
    exit 1
}

usage() {
    sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'
}

version="${AGENTENV_VERSION:-}"
install_dir="${AGENTENV_INSTALL_DIR:-$HOME/.local/bin}"
install_skill=true
claude_skills=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --version) [[ $# -ge 2 ]] || fail "--version needs a value"; version="$2"; shift 2 ;;
        --dir) [[ $# -ge 2 ]] || fail "--dir needs a value"; install_dir="$2"; shift 2 ;;
        --claude-skills) claude_skills=true; shift ;;
        --no-skill) install_skill=false; shift ;;
        -h|--help) usage; exit 0 ;;
        *) fail "unknown option '$1'; run with --help for usage" ;;
    esac
done

os="$(uname -s)"
arch="$(uname -m)"
case "$os/$arch" in
    Darwin/arm64) target="aarch64-apple-darwin" ;;
    Darwin/x86_64) target="x86_64-apple-darwin" ;;
    Linux/x86_64) target="x86_64-unknown-linux-gnu" ;;
    Linux/aarch64) target="aarch64-unknown-linux-gnu" ;;
    *) fail "no prebuilt binary for $os/$arch; build from source with 'cargo build --release'" ;;
esac
readonly target

# The Linux executables are built against glibc 2.28. Refuse here with a clear
# reason instead of leaving the loader to fail after the download.
readonly glibc_floor="2.28"
if [[ "$os" == Linux ]]; then
    if ldd --version 2>&1 | grep -qi musl; then
        fail "the Linux release binaries need glibc $glibc_floor or newer and this system uses musl; build from source with 'cargo build --release'"
    fi
    glibc="$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{print $2}')" || glibc=""
    if [[ -n "$glibc" && "$(printf '%s\n%s\n' "$glibc_floor" "$glibc" | sort -V | head -n 1)" != "$glibc_floor" ]]; then
        fail "the Linux release binaries need glibc $glibc_floor or newer and this system has glibc $glibc; build from source with 'cargo build --release'"
    fi
fi

if [[ -z "$version" ]]; then
    version="$(curl -fsSL "https://api.github.com/repos/$repo/releases/latest" \
        | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')" || true
    [[ -n "$version" ]] || fail "cannot determine the latest release; pass --version <tag>"
fi
readonly version
readonly asset="agentenv-${version}-${target}.tar.gz"

workdir="$(mktemp -d)"
readonly workdir
trap 'rm -rf "$workdir"' EXIT

download() {
    local name="$1"
    curl -fSL --output "$workdir/$name" \
        "https://github.com/$repo/releases/download/$version/$name" \
        || fail "cannot download $name from release $version"
}

echo "Downloading agentenv $version for $target..."
download "$asset"
download "SHA256SUMS"

if command -v sha256sum >/dev/null 2>&1; then
    checksum=(sha256sum --check)
else
    checksum=(shasum -a 256 --check)
fi
(cd "$workdir" && grep -F "  $asset" SHA256SUMS | "${checksum[@]}" -) \
    || fail "checksum verification failed for $asset"

tar -xzf "$workdir/$asset" -C "$workdir"
readonly extracted="$workdir/agentenv-${version}-${target}"
readonly release_version="${version#v}"
readonly binaries=(agentenv agentenv-sudo-helper agentenv-ssh-askpass)

for binary in "${binaries[@]}"; do
    [[ -f "$extracted/$binary" ]] \
        || fail "$asset is incomplete: missing $binary; install a complete release bundle"
done
[[ "$("$extracted"/agentenv --version)" == "agentenv $release_version" ]] \
    || fail "$asset contains a mismatched agentenv executable"
[[ "$("$extracted"/agentenv-sudo-helper --identity)" == "agentenv-sudo-helper 1 $release_version" ]] \
    || fail "$asset contains a mismatched sudo helper"
[[ "$("$extracted"/agentenv-ssh-askpass --identity)" == "agentenv-ssh-askpass 1 $release_version" ]] \
    || fail "$asset contains a mismatched SSH askpass helper"

mkdir -p "$install_dir"

rollback_bundle() {
    local binary
    local failed=false
    for binary in "${swapped[@]}"; do
        rm -f "$install_dir/$binary" || failed=true
        if [[ -e "$install_dir/.$binary.agentenv-old" ]]; then
            mv "$install_dir/.$binary.agentenv-old" "$install_dir/$binary" || failed=true
        fi
    done
    for binary in "${binaries[@]}"; do
        rm -f "$install_dir/.$binary.agentenv-new" || failed=true
    done
    [[ "$failed" == false ]]
}

swapped=()
for binary in "${binaries[@]}"; do
    rm -f "$install_dir/.$binary.agentenv-new" "$install_dir/.$binary.agentenv-old"
    install -m 755 "$extracted/$binary" "$install_dir/.$binary.agentenv-new" \
        || { rollback_bundle || fail "cannot stage or restore the executable bundle in $install_dir; repair the complete bundle"; fail "cannot stage the executable bundle in $install_dir"; }
done
for binary in agentenv-sudo-helper agentenv-ssh-askpass agentenv; do
    if [[ -e "$install_dir/$binary" ]]; then
        mv "$install_dir/$binary" "$install_dir/.$binary.agentenv-old" \
            || { rollback_bundle || fail "cannot retire or restore $install_dir/$binary; repair the complete bundle"; fail "cannot retire $install_dir/$binary"; }
    fi
    swapped+=("$binary")
    mv "$install_dir/.$binary.agentenv-new" "$install_dir/$binary" \
        || { rollback_bundle || fail "cannot install or restore $install_dir/$binary; repair the complete bundle"; fail "cannot install $install_dir/$binary; the previous bundle was restored"; }
done
for binary in "${binaries[@]}"; do
    rm -f "$install_dir/.$binary.agentenv-old"
done
echo "Installed $("$install_dir/agentenv" --version) to $install_dir/agentenv"

# Replaces one skill directory under a skills root with the packaged copy.
install_skill_to() {
    local root="$1"
    local destination="$root/agentenv"
    if [[ -e "$destination" && ! -f "$destination/SKILL.md" ]]; then
        fail "$destination exists but is not an agentenv skill directory; move it aside and rerun"
    fi
    mkdir -p "$root"
    rm -rf "$destination"
    cp -R "$extracted/skills/agentenv" "$destination"
    echo "Installed the agentenv agent skill to $destination"
}

if [[ "$install_skill" == true ]]; then
    if [[ -f "$extracted/skills/agentenv/SKILL.md" ]]; then
        install_skill_to "$HOME/.agents/skills"
        if [[ "$claude_skills" == true ]]; then
            install_skill_to "$HOME/.claude/skills"
        fi
    else
        echo "install.sh: release $version ships no agent skill; skipping the skill install" >&2
    fi
fi

case ":$PATH:" in
    *":$install_dir:"*) ;;
    *) echo "Add $install_dir to PATH to run 'agentenv' from any directory." ;;
esac
echo "Later releases install with 'agentenv update'."
