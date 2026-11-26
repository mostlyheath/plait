#!/bin/bash
# Install a published build, not a moving source checkout. Run as your normal user.
set -euo pipefail

repository='mostlyheath/plait'
api="https://api.github.com/repos/$repository"
releases="https://github.com/$repository/releases/download"
host_base='/Library/Application Support/Plait/Host'

usage() {
    printf '%s\n' \
        'Usage: install.sh [--host] [--version TAG] [--component app|all|client|host]' \
        '                  [--allow-unsigned] [--download DIRECTORY]' \
        '' \
        'Without --version, use the newest published release, including previews.' \
        'The default app install needs no build tools and supports in-app updates.' \
        'all, client, and host select the older system-wide Installer packages.' \
        'Preview packages are unsigned. --allow-unsigned explicitly accepts that limit.' \
        'The script checks SHA-256, but this is not publisher signature verification.' \
        'App installation refuses active runs. Manual host packages stage their update.'
}

fail() { printf 'Plait: %s\n' "$*" >&2; exit 1; }
fetch() {
    /usr/bin/curl --fail --silent --show-error --location --proto '=https' \
        --proto-redir '=https' --connect-timeout 15 --max-time 300 --retry 2 \
        "$1" --output "$2"
}
field() {
    if [[ ${linux_host:-false} == true ]]; then
        python3 -c 'import json, sys
value = json.load(open(sys.argv[2]))
for part in sys.argv[1].split("."):
    value = value[int(part)] if isinstance(value, list) else value[part]
print(json.dumps(value) if isinstance(value, (dict, list)) else value)' "$1" "$2"
    else
        /usr/bin/plutil -extract "$1" raw -o - "$2"
    fi
}

install_linux_host() {
    local version=$1 arch build_id base name expected actual archive releases_dir current running status migration=false count
    [[ $(uname -s) == Linux ]] || fail 'The host installer requires Linux.'
    case $(uname -m) in
        x86_64) arch=x86_64 ;;
        aarch64|arm64) arch=arm64 ;;
        *) fail 'The host installer supports only Linux x86_64 and arm64.' ;;
    esac
    command -v python3 >/dev/null || fail 'Python 3 is required to read release metadata.'
    if command -v curl >/dev/null; then
        fetch_linux() { curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --connect-timeout 15 --max-time 300 --retry 2 "$1" --output "$2"; }
    elif command -v wget >/dev/null; then
        fetch_linux() { wget -q --https-only -O "$2" "$1"; }
    else
        fail 'Install curl or wget to download the host.'
    fi
    if command -v sha256sum >/dev/null; then
        checksum() { sha256sum "$1"; }
    elif command -v shasum >/dev/null; then
        checksum() { shasum -a 256 "$1"; }
    else
        fail 'Install sha256sum or shasum to verify the host.'
    fi
    work=$(mktemp -d "${TMPDIR:-/tmp}/plait-install.XXXXXXXX")
    trap 'rm -rf -- "$work"' EXIT
    if [[ -z $version ]]; then
        fetch_linux "$api/releases?per_page=1" "$work/releases.json" || fail 'Could not find the newest release.'
        version=$(field '0.tag_name' "$work/releases.json") || fail 'No published release was found.'
    fi
    [[ $version =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-preview\.[0-9]+)?$ ]] || fail 'Release tag is not a supported Plait version.'
    base="$releases/$version"
    # This platform's host manifest names the archive, its SHA-256, and its build.
    fetch_linux "$base/update-host-linux-$arch.json" "$work/host.json" || fail "This release has no Linux $arch host."
    [[ $(field schema_version "$work/host.json") == 2 && $(field component "$work/host.json") == host ]] || fail 'Unsupported host manifest.'
    [[ $(field tag "$work/host.json") == "$version" ]] || fail 'The host manifest does not match the requested version.'
    build_id=$(field build_id "$work/host.json")
    [[ $build_id =~ ^[0-9]+\.[0-9]+\.[0-9]+-[0-9a-f]{20}$ ]] || fail 'Invalid host build identity.'
    name=$(field "artifacts.linux-$arch.name" "$work/host.json") || fail 'The host manifest names no archive.'
    [[ $name =~ ^Plait-Host-[0-9A-Za-z.-]+-linux-$arch\.tar\.gz$ ]] || fail 'Invalid host archive name.'
    expected=$(field "artifacts.linux-$arch.sha256" "$work/host.json")
    [[ $expected =~ ^[0-9a-f]{64}$ ]] || fail 'Invalid host checksum.'
    archive="$work/$name"
    printf 'Downloading Plait %s (Linux host)…\n' "$version"
    fetch_linux "$base/$name" "$archive" || fail 'Could not download the host archive.'
    actual=$(checksum "$archive")
    [[ ${actual%% *} == "$expected" ]] || fail 'Host checksum does not match. Nothing was installed.'
    releases_dir="$HOME/.local/share/plait-host/releases"
    if [[ -L "$HOME/.local/bin/plait-host" ]]; then
        current=$(readlink -m -- "$HOME/.local/bin/plait-host")
        case "$current" in
            "$releases_dir/$build_id/bin/plait-host") ;;
            "$releases_dir"/*) fail 'A Plait host is already installed. Run plait-host update to update it.' ;;
            *) migration=true ;;
        esac
    fi
    # The service may have been installed directly from a checkout, without a link.
    if systemctl --user is-active --quiet plait-host.service; then
        running=$(systemctl --user show plait-host.service -p ExecStart --value) || fail 'Could not inspect the running host service.'
        running=$(printf '%s' "$running" | python3 -c 'import re, sys
m = re.search(r"(?:^|[ {])path=(.*?)\s*;", sys.stdin.read())
if not m or not m.group(1).startswith("/"): sys.exit(1)
print(m.group(1))') || fail 'Could not inspect the running host service.'
        running=$(readlink -m -- "$running")
        case "$running" in
            "$releases_dir/$build_id/bin/plait-host") ;;
            "$releases_dir"/*) fail 'A Plait host is already installed. Run plait-host update to update it.' ;;
            *) migration=true ;;
        esac
    else
        status=$?
        [[ $status == 3 || $status == 4 ]] || fail 'Could not check the running host service.'
    fi
    mkdir -p -- "$releases_dir"
    stage=$(mktemp -d "$releases_dir/.install.XXXXXXXX")
    trap 'rm -rf -- "$work" "$stage"' EXIT
    if [[ ! -e "$releases_dir/$build_id" ]]; then
        tar -xzf "$archive" -C "$stage" || fail 'Could not unpack the host archive.'
        [[ -f "$stage/$build_id/bin/plait-host" && -x "$stage/$build_id/bin/plait-host" ]] || fail 'The archive does not contain the expected host.'
        mv -T -- "$stage/$build_id" "$releases_dir/$build_id" || fail 'Could not install the host release.'
    fi
    if "$migration"; then
        count=$("$releases_dir/$build_id/bin/plait-host" active-runs) || fail 'Could not check active runs; the source host was not replaced.'
        [[ $count =~ ^[0-9]+$ ]] || fail 'Could not check active runs; the source host was not replaced.'
        [[ $count =~ ^0+$ ]] || fail "The host has $count active runs. Finish them before replacing the source build."
    fi
    mkdir -p -- "$HOME/.local/bin"
    ln -sfn -- "$releases_dir/$build_id/bin/plait-host" "$HOME/.local/bin/plait-host"
    "$releases_dir/$build_id/bin/plait-host" install-service
    if "$migration"; then
        if [[ -f "$HOME/.config/systemd/user/plait-host-tailnet.service" ]]; then
            "$releases_dir/$build_id/bin/plait-host" install-service --tailnet
        fi
        systemctl --user restart plait-host.service || fail 'Could not restart plait-host.service.'
        if [[ -f "$HOME/.config/systemd/user/plait-host-tailnet.service" ]]; then
            systemctl --user restart plait-host-tailnet.service || fail 'Could not restart plait-host-tailnet.service.'
        fi
        printf 'Replaced the source-built host; data in ~/.local/share/plait was kept.\n'
    fi
    if [[ $(loginctl show-user "${USER:-}" -p Linger 2>/dev/null || true) != Linger=yes ]]; then
        printf 'The host stops at logout unless an administrator runs loginctl enable-linger $USER.\n'
    fi
    case ":${PATH:-}:" in
        *:"$HOME/.local/bin":*) ;;
        *) printf '%s is not on PATH.\n' "$HOME/.local/bin" ;;
    esac
    printf 'The script checks SHA-256, but this is not publisher signature verification.\n'
}

main() {
    local version='' component='app' unsigned=false destination='' linux_host=false
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --version|--component|--download)
                [[ $# -ge 2 && -n $2 ]] || fail "Missing value for $1."
                case "$1" in
                    --version) version=$2 ;;
                    --component) component=$2 ;;
                    --download) destination=$2 ;;
                esac
                shift 2 ;;
            --host) linux_host=true; shift ;;
            --allow-unsigned) unsigned=true; shift ;;
            --help|-h) usage; return ;;
            *) usage >&2; fail "Unknown option: $1" ;;
        esac
    done
    if "$linux_host"; then
        [[ $component == app && -z $destination && $unsigned == false ]] || fail '--host cannot be combined with app installation options.'
        install_linux_host "$version"
        return
    fi
    [[ $component == app || $component == all || $component == client || $component == host ]] || fail 'Unknown component.'
    [[ $(/usr/bin/uname -s) == Darwin && $(/usr/bin/uname -m) == arm64 ]] || fail 'This installer requires an Apple Silicon Mac.'
    local os
    os=$(/usr/bin/sw_vers -productVersion)
    [[ ${os%%.*} -ge 15 ]] || fail 'macOS 15 or later is required.'
    [[ $EUID -ne 0 ]] || fail 'Run as your normal user, not with sudo. Installer requests administrator access only at the installation step.'

    work=$(/usr/bin/mktemp -d "${TMPDIR:-/tmp}/plait-install.XXXXXXXX")
    trap '/bin/rm -rf -- "$work"' EXIT
    if [[ -z $version ]]; then
        fetch "$api/releases?per_page=1" "$work/releases.json"
        version=$(field '0.tag_name' "$work/releases.json") || fail 'No published release was found. Choose --version TAG or use the releases page.'
    fi
    [[ $version =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-preview\.[0-9]+)?$ ]] || fail 'Release tag is not a supported Plait version.'
    local base="$releases/$version"
    fetch "$base/release.json" "$work/release.json"
    [[ $(field schema_version "$work/release.json") == 1 ]] || fail 'Unsupported release metadata.'
    [[ $(field tag "$work/release.json") == "$version" ]] || fail 'Release metadata does not match the requested version.'
    local signing build_id
    signing=$(field signing "$work/release.json")
    # Signing is not yet configured. Never treat an unknown policy as trusted.
    [[ $signing == unsigned-preview ]] || fail 'Unsupported signing policy; use the release instructions.'
    if ! "$unsigned" && [[ -z $destination ]]; then
        fail 'This is an unsigned preview. Read the release notes, then pass --allow-unsigned to install, or --download DIRECTORY to inspect it first. Gatekeeper settings are not changed.'
    fi
    build_id=$(field host_build_id "$work/release.json")
    [[ $build_id =~ ^[0-9]+\.[0-9]+\.[0-9]+-[0-9a-f]{20}$ ]] || fail 'Invalid host build identity.'
    if [[ -z $destination && -e "$host_base/releases/$build_id" ]]; then
        if [[ $component == host ]]; then
            printf 'This host build is already installed. No process was restarted.\n'
            return
        elif [[ $component == all ]]; then
            printf 'Host build already installed; updating only the client.\n'
            component=client
        fi
    fi
    local name expected actual package
    name=$(field "downloads.$component.name" "$work/release.json") || fail 'This release has no requested installer. Use a newer release or choose an explicit legacy component.'
    expected=$(field "downloads.$component.sha256" "$work/release.json")
    if [[ $component == app ]]; then
        [[ $name =~ ^Plait-[0-9a-fA-Z.-]+-macos-arm64\.zip$ ]] || fail 'Invalid app archive filename.'
    else
        [[ $name =~ ^Plait-(Installer|Client|Host)-[0-9a-fA-Z.-]+-macos-arm64\.pkg$ ]] || fail 'Invalid package filename.'
    fi
    [[ $expected =~ ^[0-9a-f]{64}$ ]] || fail 'Invalid package checksum.'
    package="$work/$name"
    printf 'Downloading Plait %s (%s)…\n' "$version" "$component"
    fetch "$base/$name" "$package"
    actual=$(/usr/bin/shasum -a 256 "$package")
    [[ ${actual%% *} == "$expected" ]] || fail 'Package checksum does not match. Nothing was installed.'
    if [[ -n $destination ]]; then
        /bin/mkdir -p -- "$destination"
        [[ ! -e "$destination/$name" && ! -L "$destination/$name" ]] || fail 'The download destination already contains this filename.'
        /bin/cp -n "$package" "$destination/$name"
        printf 'Verified download: %s/%s\nUnsigned preview; see https://github.com/%s/releases/tag/%s\n' "$destination" "$name" "$repository" "$version"
        return
    fi
    if [[ $component == app ]]; then
        /usr/bin/ditto -x -k "$package" "$work/unpacked"
        local bundle="$work/unpacked/Plait.app"
        /usr/bin/codesign --verify --deep --strict "$bundle" || fail 'The app failed its signature check.'
        [[ $(field CFBundleIdentifier "$bundle/Contents/Info.plist") == dev.plait.desktop ]] || fail 'The archive is not Plait.'
        [[ $(field build_id "$bundle/Contents/Resources/host.json") == "$build_id" ]] || fail 'The bundled host does not match the release.'
        /bin/bash "$bundle/Contents/Resources/install-user.sh" --app "$bundle"
        printf '\nInstalled Plait %s. Future updates: Plait → Check for Updates…\n' "$version"
        return
    fi
    printf '\nUnsigned preview. No macOS security setting will be changed.\nQuit Plait before installing the client. Active host work will not be stopped.\n\n'
    /usr/bin/sudo /usr/sbin/installer -pkg "$package" -target /
    printf '\nInstalled Plait %s. Open Plait from Applications.\n' "$version"
    if [[ $component != client ]]; then
        printf 'A host update is staged if an older host is active. Finish work, then log out and back in to activate it.\n'
    fi
    printf 'Setup and update guide: https://github.com/%s/blob/%s/docs/GUIDE.md\n' "$repository" "$version"
}

main "$@"
