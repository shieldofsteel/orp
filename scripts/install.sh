#!/usr/bin/env sh
# ORP Universal Installer
# Usage: curl -fsSL https://orp.dev/install | sh
#
# Supports:
#   OS:   Linux, macOS
#   Arch: x86_64, aarch64/arm64
#
# Options (env vars):
#   ORP_VERSION   — specific version to install (default: latest)
#   ORP_INSTALL_DIR — install directory (default: /usr/local/bin)
#   ORP_NO_MODIFY_PATH — set to 1 to skip PATH update

set -eu

# ── Configuration ─────────────────────────────────────────────────────────────
REPO="shieldofsteel/orp"
INSTALL_DIR="${ORP_INSTALL_DIR:-/usr/local/bin}"
BINARY_NAME="orp"
TMP_DIR=""

# ── Colours (disabled if not a TTY) ───────────────────────────────────────────
if [ -t 1 ]; then
  RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
  BOLD='\033[1m'; RESET='\033[0m'
else
  RED=''; GREEN=''; YELLOW=''; BOLD=''; RESET=''
fi

info()    { printf "${GREEN}[orp]${RESET} %s\n" "$1"; }
warn()    { printf "${YELLOW}[orp] WARN:${RESET} %s\n" "$1"; }
error()   { printf "${RED}[orp] ERROR:${RESET} %s\n" "$1" >&2; exit 1; }
success() { printf "${GREEN}${BOLD}[orp] ✓${RESET} %s\n" "$1"; }

# ── Cleanup on exit ───────────────────────────────────────────────────────────
cleanup() {
  if [ -n "$TMP_DIR" ] && [ -d "$TMP_DIR" ]; then
    rm -rf "$TMP_DIR"
  fi
}
trap cleanup EXIT INT TERM

# ── Require commands ──────────────────────────────────────────────────────────
need() {
  command -v "$1" >/dev/null 2>&1 || error "Required tool not found: $1. Please install it and retry."
}

need curl
need tar

# ── Detect OS ─────────────────────────────────────────────────────────────────
detect_os() {
  OS="$(uname -s)"
  case "$OS" in
    Linux*)  OS_NAME="linux" ;;
    Darwin*) OS_NAME="macos" ;;
    *)       error "Unsupported OS: $OS. ORP currently supports Linux and macOS." ;;
  esac
  info "Detected OS: $OS_NAME"
}

# ── Detect architecture ───────────────────────────────────────────────────────
detect_arch() {
  ARCH="$(uname -m)"
  case "$ARCH" in
    x86_64 | amd64)          ARCH_NAME="x86_64" ;;
    aarch64 | arm64 | armv8*) ARCH_NAME="aarch64" ;;
    armv7*)
      warn "armv7 (32-bit ARM) detected. Trying aarch64 binary — may not work on Raspberry Pi Zero."
      warn "For armv7, build from source: https://github.com/$REPO#building-from-source"
      ARCH_NAME="aarch64"
      ;;
    *)
      error "Unsupported architecture: $ARCH. Supported: x86_64, aarch64/arm64."
      ;;
  esac
  info "Detected arch: $ARCH_NAME"
}

# ── Fetch latest version ──────────────────────────────────────────────────────
fetch_version() {
  if [ -n "${ORP_VERSION:-}" ]; then
    VERSION="$ORP_VERSION"
    info "Using specified version: $VERSION"
  else
    info "Fetching latest version..."
    VERSION="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
      | grep '"tag_name"' \
      | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"
    [ -n "$VERSION" ] || error "Could not determine latest version. Check your network or set ORP_VERSION."
    info "Latest version: $VERSION"
  fi
}

# ── Build download URL ────────────────────────────────────────────────────────
build_url() {
  ARTIFACT="orp-${OS_NAME}-${ARCH_NAME}"
  TARBALL="${ARTIFACT}.tar.gz"
  BASE_URL="https://github.com/$REPO/releases/download/$VERSION"
  DOWNLOAD_URL="$BASE_URL/$TARBALL"
  CHECKSUM_URL="$BASE_URL/checksums.sha256"
}

# ── Download ──────────────────────────────────────────────────────────────────
download() {
  TMP_DIR="$(mktemp -d)"
  info "Downloading $TARBALL..."
  curl -fsSL --progress-bar "$DOWNLOAD_URL" -o "$TMP_DIR/$TARBALL" \
    || error "Download failed. URL: $DOWNLOAD_URL"

  # Download checksums
  info "Downloading checksums..."
  curl -fsSL "$CHECKSUM_URL" -o "$TMP_DIR/checksums.sha256" \
    || warn "Could not download checksums file — skipping verification."
}

# ── Verify checksum ───────────────────────────────────────────────────────────
verify_checksum() {
  if [ ! -f "$TMP_DIR/checksums.sha256" ]; then
    warn "Skipping checksum verification (checksums.sha256 not available)."
    return
  fi

  info "Verifying checksum..."

  # Extract expected checksum for this artifact
  EXPECTED="$(grep "$TARBALL" "$TMP_DIR/checksums.sha256" | awk '{print $1}')"

  if [ -z "$EXPECTED" ]; then
    warn "No checksum found for $TARBALL in checksums.sha256 — skipping."
    return
  fi

  # Compute actual checksum
  if command -v sha256sum >/dev/null 2>&1; then
    ACTUAL="$(sha256sum "$TMP_DIR/$TARBALL" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    ACTUAL="$(shasum -a 256 "$TMP_DIR/$TARBALL" | awk '{print $1}')"
  else
    warn "Neither sha256sum nor shasum found — skipping checksum verification."
    return
  fi

  if [ "$ACTUAL" = "$EXPECTED" ]; then
    success "Checksum verified: $ACTUAL"
  else
    error "Checksum mismatch!
  Expected: $EXPECTED
  Actual:   $ACTUAL
  Download may be corrupted or tampered. Aborting."
  fi
}

# ── Extract ───────────────────────────────────────────────────────────────────
extract() {
  info "Extracting..."
  tar xzf "$TMP_DIR/$TARBALL" -C "$TMP_DIR" \
    || error "Extraction failed."

  BINARY_PATH="$(find "$TMP_DIR" -name "$BINARY_NAME" -type f | head -1)"
  [ -n "$BINARY_PATH" ] || error "Binary '$BINARY_NAME' not found in archive."
  chmod +x "$BINARY_PATH"
}

# ── Install ───────────────────────────────────────────────────────────────────
install_binary() {
  info "Installing to $INSTALL_DIR/$BINARY_NAME..."

  # Try direct install; fall back to sudo
  if [ -w "$INSTALL_DIR" ]; then
    cp "$BINARY_PATH" "$INSTALL_DIR/$BINARY_NAME"
  elif command -v sudo >/dev/null 2>&1; then
    info "Requesting sudo to install to $INSTALL_DIR..."
    sudo cp "$BINARY_PATH" "$INSTALL_DIR/$BINARY_NAME"
    sudo chmod +x "$INSTALL_DIR/$BINARY_NAME"
  else
    # Last resort: install to ~/.local/bin
    LOCAL_BIN="$HOME/.local/bin"
    mkdir -p "$LOCAL_BIN"
    cp "$BINARY_PATH" "$LOCAL_BIN/$BINARY_NAME"
    INSTALL_DIR="$LOCAL_BIN"
    warn "Installed to $LOCAL_BIN (no write access to $INSTALL_DIR and sudo unavailable)"
  fi

  success "Installed $BINARY_NAME $VERSION → $INSTALL_DIR/$BINARY_NAME"
}

# ── PATH check ────────────────────────────────────────────────────────────────
check_path() {
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) : ;;  # already in PATH
    *)
      warn "$INSTALL_DIR is not in your PATH."
      if [ "${ORP_NO_MODIFY_PATH:-0}" != "1" ]; then
        SHELL_PROFILE=""
        case "$SHELL" in
          */zsh)  SHELL_PROFILE="$HOME/.zshrc" ;;
          */bash) SHELL_PROFILE="${HOME}/.bashrc" ;;
          *)      SHELL_PROFILE="$HOME/.profile" ;;
        esac
        LINE="export PATH=\"\$PATH:$INSTALL_DIR\""
        if [ -n "$SHELL_PROFILE" ] && ! grep -qF "$LINE" "$SHELL_PROFILE" 2>/dev/null; then
          printf '\n# ORP\n%s\n' "$LINE" >> "$SHELL_PROFILE"
          warn "Added to $SHELL_PROFILE. Run: source $SHELL_PROFILE"
        else
          warn "Add manually: $LINE"
        fi
      fi
      ;;
  esac
}

# ── Quickstart ────────────────────────────────────────────────────────────────
print_quickstart() {
  printf "\n"
  printf "${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}\n"
  printf "${BOLD}  ORP ${VERSION} installed successfully!${RESET}\n"
  printf "${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}\n"
  printf "\n"
  printf "${BOLD}Quick Start:${RESET}\n"
  printf "  Start a node:     ${GREEN}orp start${RESET}\n"
  printf "  Check status:     ${GREEN}orp status${RESET}\n"
  printf "  View help:        ${GREEN}orp --help${RESET}\n"
  printf "\n"
  printf "${BOLD}Edge / Headless mode:${RESET}\n"
  printf "  ${GREEN}orp start --headless${RESET}\n"
  printf "\n"
  printf "${BOLD}Docker:${RESET}\n"
  printf "  ${GREEN}docker run -p 9090:9090 ghcr.io/shieldofsteel/orp:latest${RESET}\n"
  printf "\n"
  printf "${BOLD}Docs:${RESET}  https://orp.dev/docs\n"
  printf "${BOLD}Repo:${RESET}  https://github.com/$REPO\n"
  printf "\n"
}

# ── Main ──────────────────────────────────────────────────────────────────────
main() {
  printf "\n${BOLD}ORP Installer${RESET}\n\n"

  detect_os
  detect_arch
  fetch_version
  build_url
  download
  verify_checksum
  extract
  install_binary
  check_path
  print_quickstart
}

main
