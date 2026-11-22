#!/bin/bash
# Install a published build, not a moving source checkout. Run as your normal user.
set -euo pipefail

repository='heath0xFF/plait'
api="https://api.github.com/repos/$repository"
releases="https://github.com/$repository/releases/download"
host_base='/Library/Application Support/Plait/Host'

usage() {
    printf '%s\n' \
        'Usage: install.sh [--version TAG] [--component app|all|client|host]' \
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
field() { /usr/bin/plutil -extract "$1" raw -o - "$2"; }

main() {
    local version='' component='app' unsigned=false destination=''
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
            --allow-unsigned) unsigned=true; shift ;;
            --help|-h) usage; return ;;
            *) usage >&2; fail "Unknown option: $1" ;;
        esac
    done
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
