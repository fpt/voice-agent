#!/bin/bash
set -e

# Start llama.cpp server with gpt-oss model

MODEL_PATH="models/gpt-oss-20b-mxfp4.gguf"

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

echo -e "${GREEN}🦙 Starting llama.cpp server...${NC}"
echo ""

# Check if model exists
if [ ! -f "$MODEL_PATH" ]; then
    echo -e "${RED}❌ Model not found: $MODEL_PATH${NC}"
    echo ""
    echo "Please download the model first:"
    echo "  huggingface-cli download ggml-org/gpt-oss-20b-GGUF --include '*mxfp4.gguf' --local-dir models/"
    echo ""
    exit 1
fi

echo "Model: $MODEL_PATH"
echo "Port: 8080"
echo ""

# Start server with optimal settings for gpt-oss
# -c: context window size (8192 is good for most questions)
# -fa: FlashAttention for faster inference
# --jinja: Enable Jinja template support (required for Harmony)
# -ngl: Number of GPU layers (99 = all layers)
exec llama-server \
    -m "$MODEL_PATH" \
    -c 8192 \
    -fa \
    --jinja \
    --host 127.0.0.1 \
    --port 8080 \
    -ngl 99 \
    --log-disable
