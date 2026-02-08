#!/bin/bash
# Download Whisper GGML model for whisper-rs
# Compatible models from: https://huggingface.co/ggerganov/whisper.cpp

set -e

MODEL=${1:-base}
MODELS_DIR="models"

echo "📥 Downloading Whisper $MODEL model..."

# Create models directory if it doesn't exist
mkdir -p "$MODELS_DIR"

# Download from Hugging Face
case $MODEL in
  tiny)
    URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin"
    ;;
  base)
    URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin"
    ;;
  small)
    URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"
    ;;
  medium)
    URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin"
    ;;
  large-v3)
    URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin"
    ;;
  *)
    echo "❌ Unknown model: $MODEL"
    echo "Available models: tiny, base, small, medium, large-v3"
    exit 1
    ;;
esac

OUTPUT="$MODELS_DIR/ggml-$MODEL.bin"

if [ -f "$OUTPUT" ]; then
  echo "✅ Model already exists: $OUTPUT"
  exit 0
fi

echo "Downloading from: $URL"
curl -L -o "$OUTPUT" "$URL"

echo "✅ Downloaded to: $OUTPUT"
echo ""
echo "Update your config to use:"
echo "  modelPath: \"$OUTPUT\""
