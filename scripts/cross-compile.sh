#!/usr/bin/env bash
# ORP Cross-Compilation Helper
# Builds ORP for aarch64 (Raspberry Pi, ARM servers) from macOS or Linux x86_64.
#
# Usage:
#   ./scripts/cross-compile.sh                    # aarch64 Linux (Raspberry Pi)
#   ./scripts/cross-compile.sh --target x86_64    # Linux x86_64 (force)
#   ./scripts/cross-compile.sh --target aarch64   # Linux aarch64 (default)
#   ./scripts/cross-compile.sh --list             # show all available targets
#   ./scripts/cross-compile.sh --all              # build all targets
#
# Requirements:
#   macOS: Docker (for cross-rs) OR Homebrew cross-compilation toolchain
#   Linux: Docker or native cross-compilation toolchain

set -euo pipefail

# ── Colours ───────────────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
CYAN='\033[0;36m'; BOLD='\033[1m'; RESET='\033[0m'

info()    { printf "${CYAN}[cross]${RESET} %s\n" "$1"; }
success() { printf "${GREEN}${BOLD}[cross] ✓${RESET} %s\n" "$1"; }
warn()    { printf "${YELLOW}[cross] WARN:${RESET} %s\n" "$1"; }
error()   { printf "${RED}[cross] ERROR:${RESET} %s\n" "$1" >&2; exit 1; }
section() { printf "\n${BOLD}━━━ %s ━━━${RESET}\n\n" "$1"; }

# ── Supported targets ─────────────────────────────────────────────────────────
declare -A TARGET_NAMES=(
  ["aarch64-unknown-linux-gnu"]="Linux aarch64 (Raspberry Pi 3/4/5, ARM64 servers)"
  ["x86_64-unknown-linux-gnu"]="Linux x86_64 (Intel/AMD servers, most cloud VMs)"
  ["x86_64-apple-darwin"]="macOS x86_64 (Intel Mac)"
  ["aarch64-apple-darwin"]="macOS aarch64 (Apple Silicon M1/M2/M3)"
)

# ── Defaults ──────────────────────────────────────────────────────────────────
DEFAULT_TARGET="aarch64-unknown-linux-gnu"
BUILD_TARGET="$DEFAULT_TARGET"
BUILD_ALL=false
LIST_TARGETS=false
OUTPUT_DIR="$(pwd)/dist"
PACKAGE_NAME="orp-core"

# ── Argument parsing ──────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)
      shift
      case "$1" in
        aarch64 | arm64 | rpi) BUILD_TARGET="aarch64-unknown-linux-gnu" ;;
        x86_64 | amd64)        BUILD_TARGET="x86_64-unknown-linux-gnu" ;;
        macos-x86)             BUILD_TARGET="x86_64-apple-darwin" ;;
        macos-arm64)           BUILD_TARGET="aarch64-apple-darwin" ;;
        *)                     BUILD_TARGET="$1" ;;  # pass through full triple
      esac
      ;;
    --all)    BUILD_ALL=true ;;
    --list)   LIST_TARGETS=true ;;
    --output) shift; OUTPUT_DIR="$1" ;;
    --package) shift; PACKAGE_NAME="$1" ;;
    -h | --help)
      cat <<EOF
${BOLD}ORP Cross-Compilation Helper${RESET}

Usage:
  $0 [OPTIONS]

Options:
  --target <target>   Target triple or alias (default: aarch64)
                      Aliases: aarch64|arm64|rpi, x86_64|amd64, macos-x86, macos-arm64
  --all               Build all supported targets
  --list              List supported targets
  --output <dir>      Output directory (default: ./dist)
  --package <name>    Cargo package to build (default: orp-core)
  -h, --help          Show this help

Examples:
  $0                              # Raspberry Pi build
  $0 --target aarch64             # Explicit Raspberry Pi
  $0 --target x86_64              # Linux x86_64
  $0 --all                        # All targets

EOF
      exit 0
      ;;
    *)
      warn "Unknown argument: $1 (use --help)"
      ;;
  esac
  shift
done

# ── List targets ──────────────────────────────────────────────────────────────
if $LIST_TARGETS; then
  printf "\n${BOLD}Supported targets:${RESET}\n\n"
  for triple in "${!TARGET_NAMES[@]}"; do
    printf "  ${CYAN}%-40s${RESET} %s\n" "$triple" "${TARGET_NAMES[$triple]}"
  done
  printf "\nAliases: aarch64|arm64|rpi → aarch64-unknown-linux-gnu\n"
  printf "         x86_64|amd64      → x86_64-unknown-linux-gnu\n"
  printf "         macos-x86         → x86_64-apple-darwin\n"
  printf "         macos-arm64       → aarch64-apple-darwin\n\n"
  exit 0
fi

# ── Detect host OS + arch ─────────────────────────────────────────────────────
HOST_OS="$(uname -s)"
HOST_ARCH="$(uname -m)"
info "Host: $HOST_OS $HOST_ARCH"

# ── Check prerequisites ───────────────────────────────────────────────────────
check_rust() {
  command -v cargo >/dev/null 2>&1 || {
    error "Rust/Cargo not found. Install via: https://rustup.rs"
  }
  RUST_VERSION="$(rustc --version)"
  info "Rust: $RUST_VERSION"
}

check_or_install_cross() {
  if ! command -v cross >/dev/null 2>&1; then
    info "cross-rs not found. Installing..."
    cargo install cross --git https://github.com/cross-rs/cross --locked \
      || error "Failed to install cross-rs. Try: cargo install cross"
  fi
  CROSS_VERSION="$(cross --version 2>&1 | head -1)"
  info "cross: $CROSS_VERSION"
}

check_docker() {
  if ! command -v docker >/dev/null 2>&1; then
    warn "Docker not found. cross-rs requires Docker for Linux cross-compilation."
    warn "Install Docker: https://docs.docker.com/get-docker/"
    return 1
  fi
  if ! docker info >/dev/null 2>&1; then
    warn "Docker daemon not running. Start Docker and retry."
    return 1
  fi
  return 0
}

check_protoc() {
  if ! command -v protoc >/dev/null 2>&1; then
    warn "protoc not found — required for orp-proto crate."
    if [[ "$HOST_OS" == "Darwin" ]]; then
      info "Installing protoc via Homebrew..."
      brew install protobuf || warn "Could not install protoc. Build may fail."
    else
      info "Install protoc: sudo apt-get install protobuf-compiler"
    fi
  else
    info "protoc: $(protoc --version)"
  fi
}

# ── Determine build method ────────────────────────────────────────────────────
# native:  cargo build (host toolchain supports target)
# cross:   cross build (via Docker + cross-rs)
decide_method() {
  local target="$1"

  # macOS targets — always native on macOS host
  if [[ "$target" == *"apple"* ]]; then
    if [[ "$HOST_OS" != "Darwin" ]]; then
      error "Cannot build macOS targets on $HOST_OS. Requires macOS host."
    fi
    echo "native"
    return
  fi

  # Same-arch Linux on Linux host → native
  if [[ "$HOST_OS" == "Linux" ]]; then
    if [[ "$target" == "x86_64-unknown-linux-gnu" && "$HOST_ARCH" == "x86_64" ]]; then
      echo "native"
      return
    fi
    if [[ "$target" == "aarch64-unknown-linux-gnu" && "$HOST_ARCH" == "aarch64" ]]; then
      echo "native"
      return
    fi
  fi

  # Cross-arch Linux → use cross-rs
  echo "cross"
}

# ── Install Rust target ───────────────────────────────────────────────────────
install_target() {
  local target="$1"
  info "Ensuring Rust target $target is installed..."
  rustup target add "$target" 2>/dev/null || true
}

# ── Build ─────────────────────────────────────────────────────────────────────
build_target() {
  local target="$1"
  local method
  method="$(decide_method "$target")"

  section "Building $target (${TARGET_NAMES[$target]:-custom target})"
  info "Method: $method"

  install_target "$target"

  mkdir -p "$OUTPUT_DIR/$target"

  local start_time
  start_time="$(date +%s)"

  if [[ "$method" == "native" ]]; then
    check_protoc
    CARGO_TERM_COLOR=always cargo build \
      --release \
      --target "$target" \
      -p "$PACKAGE_NAME" \
      2>&1

  else
    # cross-rs method
    if ! check_docker; then
      error "Docker is required for cross-compilation to $target. Start Docker and retry."
    fi
    check_or_install_cross

    CARGO_TERM_COLOR=always cross build \
      --release \
      --target "$target" \
      -p "$PACKAGE_NAME" \
      2>&1
  fi

  local end_time elapsed
  end_time="$(date +%s)"
  elapsed=$((end_time - start_time))

  # Locate binary
  local binary="target/$target/release/orp"
  if [[ ! -f "$binary" ]]; then
    error "Binary not found at $binary after build."
  fi

  # Strip (Linux only, matching strip available)
  if [[ "$target" == *"linux"* ]]; then
    if [[ "$target" == "x86_64-unknown-linux-gnu" ]] && command -v strip >/dev/null 2>&1; then
      strip "$binary"
      info "Stripped x86_64 binary"
    elif [[ "$target" == "aarch64-unknown-linux-gnu" ]] && command -v aarch64-linux-gnu-strip >/dev/null 2>&1; then
      aarch64-linux-gnu-strip "$binary"
      info "Stripped aarch64 binary"
    else
      warn "Strip tool not available for $target — binary unstripped"
    fi
  fi

  # Get size
  local size_bytes size_mb
  if [[ "$HOST_OS" == "Darwin" ]]; then
    size_bytes="$(stat -f%z "$binary")"
  else
    size_bytes="$(stat -c%s "$binary")"
  fi
  size_mb=$((size_bytes / 1024 / 1024))

  # Copy to output
  local out_name
  case "$target" in
    x86_64-unknown-linux-gnu)  out_name="orp-linux-x86_64" ;;
    aarch64-unknown-linux-gnu) out_name="orp-linux-aarch64" ;;
    x86_64-apple-darwin)       out_name="orp-macos-x86_64" ;;
    aarch64-apple-darwin)      out_name="orp-macos-aarch64" ;;
    *)                         out_name="orp-$target" ;;
  esac

  mkdir -p "$OUTPUT_DIR/$out_name"
  cp "$binary" "$OUTPUT_DIR/$out_name/orp"
  [[ -f LICENSE ]]   && cp LICENSE   "$OUTPUT_DIR/$out_name/"
  [[ -f README.md ]] && cp README.md "$OUTPUT_DIR/$out_name/"

  local tarball="$OUTPUT_DIR/$out_name.tar.gz"
  tar czf "$tarball" -C "$OUTPUT_DIR" "$out_name"

  # Checksum
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$tarball" | tee -a "$OUTPUT_DIR/checksums.sha256"
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$tarball" | tee -a "$OUTPUT_DIR/checksums.sha256"
  fi

  success "Built $out_name (${size_mb}MB) in ${elapsed}s → $tarball"
}

# ── Main ──────────────────────────────────────────────────────────────────────
main() {
  printf "\n${BOLD}ORP Cross-Compilation Helper${RESET}\n\n"

  check_rust

  # Clear old checksums
  mkdir -p "$OUTPUT_DIR"
  rm -f "$OUTPUT_DIR/checksums.sha256"

  if $BUILD_ALL; then
    info "Building all targets..."
    for target in "${!TARGET_NAMES[@]}"; do
      build_target "$target" || warn "Failed: $target (continuing)"
    done
  else
    build_target "$BUILD_TARGET"
  fi

  # Summary
  section "Summary"
  printf "Output directory: ${CYAN}$OUTPUT_DIR${RESET}\n\n"
  if [[ -f "$OUTPUT_DIR/checksums.sha256" ]]; then
    printf "${BOLD}Checksums:${RESET}\n"
    cat "$OUTPUT_DIR/checksums.sha256"
    printf "\n"
  fi
  ls -lh "$OUTPUT_DIR"/*.tar.gz 2>/dev/null || true

  printf "\n${BOLD}Deploy to Raspberry Pi:${RESET}\n"
  printf "  scp ${OUTPUT_DIR}/orp-linux-aarch64.tar.gz pi@raspberrypi:~\n"
  printf "  ssh pi@raspberrypi 'tar xzf orp-linux-aarch64.tar.gz && sudo mv orp-linux-aarch64/orp /usr/local/bin/orp && orp start'\n\n"
}

main "$@"
