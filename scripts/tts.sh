#!/bin/bash
# Sofia TTS — local Piper voice synthesis
# Usage: ./scripts/tts.sh "Текст для озвучки" [output.wav]

PIPER="/Users/lokitheone/sophia/venv/bin/piper"
MODEL="/Users/lokitheone/sophia/data/models/piper/ru_RU-irina-medium.onnx"
OUTPUT="${2:-/tmp/sofia_tts.wav}"

if [ -z "$1" ]; then
    echo "Usage: $0 \"текст\" [output.wav]"
    exit 1
fi

echo "$1" | "$PIPER" --model "$MODEL" --output_file "$OUTPUT" 2>/dev/null
echo "$OUTPUT"
