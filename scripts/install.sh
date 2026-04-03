#!/bin/sh
# Clido installer — https://github.com/clido-ai/clido-cli
#
# Usage:
#   curl -fsSL https://clido.ai/install.sh | sh
#
# Options (via env vars):
#   CLIDO_VERSION   Install a specific version (e.g. "v0.1.0"). Default: latest.
#   CLIDO_DIR       Install directory. Default: ~/.local/bin
#
# Supports: macOS (arm64, x86_64), Linux (x86_64, aarch64)

set -e

REPO="clido-ai/clido-cli"
BINARY="clido"
DEFAULT_DIR="${HOME}/.local/bin"

# ── Helpers ───────────────────────────────────────────────────────────────────

bold=""
reset=""
green=""
red=""
cyan=""
if [ -t 1 ] && command -v tput >/dev/null 2>&1; then
    bold=$(tput bold 2>/dev/null || true)
    reset=$(tput sgr0 2>/dev/null || true)
    green=$(tput setaf 2 2>/dev/null || true)
    red=$(tput setaf 1 2>/dev/null || true)
    cyan=$(tput setaf 6 2>/dev/null || true)
fi

info()  { printf '%s\n' "${cyan}${bold}info${reset}  $*"; }
ok()    { printf '%s\n' "${green}${bold}  ok${reset}  $*"; }
err()   { printf '%s\n' "${red}${bold}error${reset} $*" >&2; }
die()   { err "$@"; exit 1; }

need() {
    command -v "$1" >/dev/null 2>&1 || die "Required tool not found: $1"
}

# ── Detect platform ──────────────────────────────────────────────────────────

detect_platform() {
    local os arch

    os=$(uname -s)
    arch=$(uname -m)

    case "$os" in
        Linux)  os="linux" ;;
        Darwin) os="macos" ;;
        *)      die "Unsupported OS: $os (only Linux and macOS are supported)" ;;
    esac

    case "$arch" in
        x86_64|amd64)  arch="x86_64" ;;
        aarch64|arm64) arch="aarch64" ;;
        *)             die "Unsupported architecture: $arch (only x86_64 and aarch64 are supported)" ;;
    esac

    PLATFORM="${os}"
    ARCH="${arch}"
    ARTIFACT="${BINARY}-${os}-${arch}"
}

# ── Resolve version ──────────────────────────────────────────────────────────

resolve_version() {
    if [ -n "${CLIDO_VERSION:-}" ]; then
        VERSION="$CLIDO_VERSION"
        # Ensure it starts with "v"
        case "$VERSION" in
            v*) ;;
            *)  VERSION="v${VERSION}" ;;
        esac
        return
    fi

    info "Fetching latest release..."

    local url="https://api.github.com/repos/${REPO}/releases/latest"
    local response

    if command -v curl >/dev/null 2>&1; then
        response=$(curl -fsSL "$url" 2>/dev/null) || die "Failed to fetch latest release from GitHub. Check your internet connection."
    elif command -v wget >/dev/null 2>&1; then
        response=$(wget -qO- "$url" 2>/dev/null) || die "Failed to fetch latest release from GitHub. Check your internet connection."
    else
        die "Either curl or wget is required."
    fi

    # Parse tag_name from JSON without jq (works with grep + sed)
    VERSION=$(printf '%s' "$response" | grep '"tag_name"' | head -1 | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')

    if [ -z "$VERSION" ]; then
        die "Could not determine latest version. Set CLIDO_VERSION manually or check https://github.com/${REPO}/releases"
    fi
}

# ── Download ──────────────────────────────────────────────────────────────────

download() {
    local url="https://github.com/${REPO}/releases/download/${VERSION}/${ARTIFACT}"
    local dest="$1"

    info "Downloading ${BINARY} ${VERSION} for ${PLATFORM}/${ARCH}..."
    info "  ${url}"

    if command -v curl >/dev/null 2>&1; then
        curl -fSL --progress-bar "$url" -o "$dest" || die "Download failed. The release may not have a binary for your platform (${ARTIFACT}).\nCheck available assets at: https://github.com/${REPO}/releases/tag/${VERSION}"
    elif command -v wget >/dev/null 2>&1; then
        wget -q --show-progress "$url" -O "$dest" || die "Download failed. The release may not have a binary for your platform (${ARTIFACT}).\nCheck available assets at: https://github.com/${REPO}/releases/tag/${VERSION}"
    fi
}

# ── Install ───────────────────────────────────────────────────────────────────

install() {
    local install_dir="${CLIDO_DIR:-$DEFAULT_DIR}"
    local tmp

    tmp=$(mktemp) || die "Failed to create temp file"
    trap 'rm -f "$tmp"' EXIT

    download "$tmp"
    chmod +x "$tmp"

    # Verify it's a real binary (not an HTML error page)
    if ! file "$tmp" 2>/dev/null | grep -qiE 'executable|ELF|Mach-O'; then
        die "Downloaded file is not a valid binary. The release may not include ${ARTIFACT}."
    fi

    # Create install directory if needed
    mkdir -p "$install_dir" || die "Failed to create ${install_dir}. Try: CLIDO_DIR=/usr/local/bin sudo -E sh install.sh"

    # Move binary into place
    mv "$tmp" "${install_dir}/${BINARY}" || die "Failed to install to ${install_dir}/${BINARY}. You may need sudo."
    trap - EXIT  # disarm cleanup since mv succeeded

    ok "Installed ${BINARY} ${VERSION} to ${install_dir}/${BINARY}"

    # Check if directory is in PATH
    case ":${PATH}:" in
        *":${install_dir}:"*) ;;
        *)
            printf '\n'
            info "${install_dir} is not in your PATH. Add it:"
            printf '\n'

            local shell_name
            shell_name=$(basename "${SHELL:-/bin/sh}")
            case "$shell_name" in
                zsh)  printf '  echo '\''export PATH="%s:$PATH"'\'' >> ~/.zshrc && source ~/.zshrc\n' "$install_dir" ;;
                bash) printf '  echo '\''export PATH="%s:$PATH"'\'' >> ~/.bashrc && source ~/.bashrc\n' "$install_dir" ;;
                fish) printf '  fish_add_path %s\n' "$install_dir" ;;
                *)    printf '  export PATH="%s:$PATH"\n' "$install_dir" ;;
            esac
            printf '\n'
            ;;
    esac

    # Quick smoke test
    if command -v "${BINARY}" >/dev/null 2>&1; then
        local installed_version
        installed_version=$("${BINARY}" --version 2>/dev/null || echo "unknown")
        ok "Ready! ${installed_version}"
    else
        ok "Binary installed. Run '${BINARY} --help' to get started."
    fi

    printf '\n'
    info "Get started:"
    printf '  %s setup      # configure your first profile\n' "${BINARY}"
    printf '  %s            # start interactive session\n' "${BINARY}"
    printf '  %s --help     # see all options\n' "${BINARY}"
    printf '\n'
}

# ── Main ──────────────────────────────────────────────────────────────────────

main() {
    printf '\n'
    printf '  %s%sclido installer%s\n' "$bold" "$cyan" "$reset"
    printf '\n'

    detect_platform
    resolve_version
    install
}

main
