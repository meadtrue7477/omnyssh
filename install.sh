#!/bin/sh
# OmnySSH installation script
# Usage: curl -fsSL https://raw.githubusercontent.com/timhartmann7/omnyssh/main/install.sh | sh

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# GitHub repository
REPO="timhartmann7/omnyssh"
BINARY_NAME="omny"

# Print colored messages
print_info() {
    printf "${BLUE}[INFO]${NC} %s\n" "$1"
}

print_success() {
    printf "${GREEN}[SUCCESS]${NC} %s\n" "$1"
}

print_error() {
    printf "${RED}[ERROR]${NC} %s\n" "$1"
}

print_warning() {
    printf "${YELLOW}[WARNING]${NC} %s\n" "$1"
}

# Detect OS and architecture
detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Linux*)
            PLATFORM="unknown-linux-gnu"
            INSTALL_DIR="/usr/local/bin"
            ;;
        Darwin*)
            PLATFORM="apple-darwin"
            INSTALL_DIR="/usr/local/bin"
            ;;
        MINGW*|MSYS*|CYGWIN*)
            PLATFORM="pc-windows-msvc"
            INSTALL_DIR="$HOME/bin"
            EXT=".exe"
            print_warning "Windows detected. Manual PATH configuration may be required."
            ;;
        *)
            print_error "Unsupported OS: $OS"
            exit 1
            ;;
    esac

    case "$ARCH" in
        x86_64|amd64)
            ARCH="x86_64"
            ;;
        aarch64|arm64)
            ARCH="aarch64"
            ;;
        *)
            print_error "Unsupported architecture: $ARCH"
            exit 1
            ;;
    esac

    TARGET="${ARCH}-${PLATFORM}"
    print_info "Detected platform: $TARGET"
}

# Get latest release version
get_latest_release() {
    print_info "Fetching latest release version..."

    # Try using curl first, then wget
    if command -v curl >/dev/null 2>&1; then
        VERSION=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | \
                  grep '"tag_name":' | \
                  sed -E 's/.*"([^"]+)".*/\1/')
    elif command -v wget >/dev/null 2>&1; then
        VERSION=$(wget -qO- "https://api.github.com/repos/$REPO/releases/latest" | \
                  grep '"tag_name":' | \
                  sed -E 's/.*"([^"]+)".*/\1/')
    else
        print_error "Neither curl nor wget found. Please install one of them."
        exit 1
    fi

    if [ -z "$VERSION" ]; then
        print_error "Failed to fetch latest release version"
        exit 1
    fi

    print_info "Latest version: $VERSION"
}

# Download and extract binary
download_and_install() {
    ARCHIVE_NAME="${BINARY_NAME}-${TARGET}"

    if [ "$PLATFORM" = "pc-windows-msvc" ]; then
        ARCHIVE_EXT="zip"
    else
        ARCHIVE_EXT="tar.gz"
    fi

    DOWNLOAD_URL="https://github.com/$REPO/releases/download/$VERSION/${ARCHIVE_NAME}.${ARCHIVE_EXT}"

    print_info "Downloading from: $DOWNLOAD_URL"

    TMP_DIR="$(mktemp -d)"
    trap 'rm -rf "$TMP_DIR"' EXIT

    ARCHIVE_FILE="$TMP_DIR/${ARCHIVE_NAME}.${ARCHIVE_EXT}"

    # Download the archive
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$DOWNLOAD_URL" -o "$ARCHIVE_FILE"
    else
        wget -qO "$ARCHIVE_FILE" "$DOWNLOAD_URL"
    fi

    if [ ! -f "$ARCHIVE_FILE" ]; then
        print_error "Failed to download archive"
        exit 1
    fi

    print_info "Extracting archive..."

    # Extract based on archive type
    if [ "$ARCHIVE_EXT" = "zip" ]; then
        unzip -q "$ARCHIVE_FILE" -d "$TMP_DIR"
    else
        tar -xzf "$ARCHIVE_FILE" -C "$TMP_DIR"
    fi

    # Find the binary
    BINARY_PATH="$(find "$TMP_DIR" -name "${BINARY_NAME}${EXT}" -type f | head -n 1)"

    if [ ! -f "$BINARY_PATH" ]; then
        print_error "Binary not found in archive"
        exit 1
    fi

    # Create install directory if it doesn't exist
    if [ ! -d "$INSTALL_DIR" ]; then
        print_info "Creating install directory: $INSTALL_DIR"
        mkdir -p "$INSTALL_DIR"
    fi

    # Install the binary
    print_info "Installing to $INSTALL_DIR..."

    if [ -w "$INSTALL_DIR" ]; then
        mv "$BINARY_PATH" "$INSTALL_DIR/$BINARY_NAME${EXT}"
        chmod +x "$INSTALL_DIR/$BINARY_NAME${EXT}"
    else
        # Need sudo for system directories
        print_warning "Installing to system directory requires sudo privileges"
        sudo mv "$BINARY_PATH" "$INSTALL_DIR/$BINARY_NAME${EXT}"
        sudo chmod +x "$INSTALL_DIR/$BINARY_NAME${EXT}"
    fi

    print_success "Binary installed to: $INSTALL_DIR/$BINARY_NAME${EXT}"
}

# Check if installation was successful
verify_installation() {
    print_info "Verifying installation..."

    # Check if binary is in PATH
    if command -v "$BINARY_NAME" >/dev/null 2>&1; then
        INSTALLED_VERSION=$($BINARY_NAME --version | head -n 1)
        print_success "Installation successful! $INSTALLED_VERSION"
        print_info "Run '$BINARY_NAME' to get started"
    else
        print_warning "$BINARY_NAME installed but not found in PATH"
        print_info "Add $INSTALL_DIR to your PATH or run: $INSTALL_DIR/$BINARY_NAME"

        # Suggest PATH configuration
        SHELL_NAME="$(basename "$SHELL")"
        case "$SHELL_NAME" in
            bash)
                CONFIG_FILE="$HOME/.bashrc"
                ;;
            zsh)
                CONFIG_FILE="$HOME/.zshrc"
                ;;
            fish)
                CONFIG_FILE="$HOME/.config/fish/config.fish"
                ;;
            *)
                CONFIG_FILE="$HOME/.profile"
                ;;
        esac

        print_info "To add to PATH, run:"
        print_info "  echo 'export PATH=\"$INSTALL_DIR:\$PATH\"' >> $CONFIG_FILE"
        print_info "  source $CONFIG_FILE"
    fi
}

# Install man page
install_man_page() {
    # Skip man page installation on Windows
    if [ "$PLATFORM" = "pc-windows-msvc" ]; then
        return
    fi

    print_info "Installing man page..."

    MAN_URL="https://raw.githubusercontent.com/$REPO/main/doc/omny.1"
    MAN_DIR="/usr/local/share/man/man1"

    # Try to install man page if we can
    if [ -d "$MAN_DIR" ] || mkdir -p "$MAN_DIR" 2>/dev/null; then
        if command -v curl >/dev/null 2>&1; then
            if curl -fsSL "$MAN_URL" -o "$MAN_DIR/omny.1" 2>/dev/null; then
                print_success "Man page installed. Run 'man omny' for documentation"
                return
            fi
        fi
    fi

    # If we get here, man page installation failed (not critical)
    print_info "Man page installation skipped (optional)"
}

# Main installation flow
main() {
    echo ""
    echo "╔═══════════════════════════════════════╗"
    echo "║                                       ║"
    echo "║   OmnySSH Installation Script         ║"
    echo "║   TUI SSH Dashboard & Server Manager  ║"
    echo "║                                       ║"
    echo "╚═══════════════════════════════════════╝"
    echo ""

    detect_platform
    get_latest_release
    download_and_install
    install_man_page
    verify_installation

    echo ""
    print_success "Installation complete!"
    echo ""
    print_info "Next steps:"
    print_info "  1. Run 'omny' to start the application"
    print_info "  2. Configure your servers in ~/.config/omnyssh/"
    print_info "  3. Check 'man omny' for documentation (Linux/macOS)"
    print_info "  4. Visit https://github.com/$REPO for more info"
    echo ""
}

main
