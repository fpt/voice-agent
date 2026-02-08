#!/bin/bash
set -e

# Generate UniFFI bindings for Swift

echo "🦀 Building Rust library..."
cd crates
cargo build --release

echo "🔧 Generating UniFFI Swift bindings..."
cd lib

LIBRARY_PATH="../target/release/libagent_core.dylib"
OUT_DIR="../../vendor/uniffi-swift"

mkdir -p $OUT_DIR

# Generate Swift sources
cargo run --bin uniffi-bindgen-swift -- --swift-sources $LIBRARY_PATH $OUT_DIR

# Generate headers
cargo run --bin uniffi-bindgen-swift -- --headers $LIBRARY_PATH $OUT_DIR

# Generate modulemap
cargo run --bin uniffi-bindgen-swift -- --modulemap $LIBRARY_PATH $OUT_DIR

echo "✅ UniFFI bindings generated!"
echo ""
echo "Generated files in vendor/uniffi-swift/:"
ls -lh ../../vendor/uniffi-swift/

echo ""
echo "📝 Next steps:"
echo "  1. Review generated files in vendor/uniffi-swift/"
echo "  2. Copy agent_core.swift to swift/Sources/AgentBridge/"
echo "  3. Update Package.swift to link the dylib"
echo "  4. Remove the mock implementation from AgentFFI.swift"
