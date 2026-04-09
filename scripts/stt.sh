#!/bin/bash
# Sofia STT — local Whisper speech recognition
# Usage: ./scripts/stt.sh audio_file.wav [model]
# Models: tiny, base, small, medium, large, turbo
# Default: base (fast, decent quality for Russian)

INPUT="$1"
MODEL="${2:-base}"

if [ -z "$INPUT" ]; then
    echo "Usage: $0 audio_file [model]"
    echo "Models: tiny, base, small, medium, large, turbo"
    exit 1
fi

if [ ! -f "$INPUT" ]; then
    echo "Error: file not found: $INPUT"
    exit 1
fi

whisper "$INPUT" --language ru --model "$MODEL" --output_format txt --output_dir /tmp/ 2>/dev/null
# Output the transcription
BASENAME=$(basename "$INPUT" | sed 's/\.[^.]*$//')
cat "/tmp/${BASENAME}.txt" 2>/dev/null
