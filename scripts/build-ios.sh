#!/bin/bash
# Build Rust agent_core for iOS (device + simulator).
# Produces an XCFramework at swift/AgentApp/agent_core.xcframework
#
# Usage: bash scripts/build-ios.sh
#
# Prerequisites: rustup target add aarch64-apple-ios aarch64-apple-ios-sim

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CRATES_DIR="$ROOT_DIR/crates"
OUTPUT_DIR="$ROOT_DIR/swift/AgentApp"

echo "Building agent_core for iOS..."

# Build for device (aarch64-apple-ios)
echo "  [1/3] Building for iOS device (aarch64-apple-ios)..."
cd "$CRATES_DIR"
cargo build -p lib --release --no-default-features \
    --target aarch64-apple-ios 2>&1 | tail -3

# Build for simulator (aarch64-apple-ios-sim)
echo "  [2/3] Building for iOS simulator (aarch64-apple-ios-sim)..."
cargo build -p lib --release --no-default-features \
    --target aarch64-apple-ios-sim 2>&1 | tail -3

# Create XCFramework
echo "  [3/3] Creating XCFramework..."

DEVICE_LIB="$CRATES_DIR/target/aarch64-apple-ios/release/libagent_core.a"
SIM_LIB="$CRATES_DIR/target/aarch64-apple-ios-sim/release/libagent_core.a"

# Use the UniFFI-generated header
HEADER_DIR="$ROOT_DIR/vendor/uniffi-swift"
HEADER="$HEADER_DIR/agent_coreFFI.h"
MODULEMAP="$HEADER_DIR/agent_core.modulemap"

if [ ! -f "$HEADER" ]; then
    echo "Error: $HEADER not found. Run: bash scripts/gen_uniffi.sh"
    exit 1
fi

# Prepare header directories for each platform
DEVICE_HEADERS="$CRATES_DIR/target/ios-headers-device"
SIM_HEADERS="$CRATES_DIR/target/ios-headers-sim"
rm -rf "$DEVICE_HEADERS" "$SIM_HEADERS"
mkdir -p "$DEVICE_HEADERS" "$SIM_HEADERS"
cp "$HEADER" "$DEVICE_HEADERS/"
cp "$MODULEMAP" "$DEVICE_HEADERS/"
cp "$HEADER" "$SIM_HEADERS/"
cp "$MODULEMAP" "$SIM_HEADERS/"

XCFW_PATH="$OUTPUT_DIR/agent_core.xcframework"
rm -rf "$XCFW_PATH"

xcodebuild -create-xcframework \
    -library "$DEVICE_LIB" -headers "$DEVICE_HEADERS" \
    -library "$SIM_LIB" -headers "$SIM_HEADERS" \
    -output "$XCFW_PATH" 2>&1 | tail -3

echo ""
echo "Done! XCFramework: $XCFW_PATH"
echo ""
echo "Next steps:"
echo "  1. Open swift/AgentApp/AgentApp.xcodeproj in Xcode"
echo "  2. Drag agent_core.xcframework into the project (if not already added)"
echo "  3. Build and run"
