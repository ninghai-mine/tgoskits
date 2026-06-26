#!/bin/bash
# Generate kernel fingerprint for dynamic HPA discovery.
# Extracts the first 64 bytes of _text from vmlinux.
set -euo pipefail
VMLINUX="${1:-/tmp/linux-6.12.94/vmlinux}"
OUTPUT="${2:-os/arceos/examples/ax-monitor-guest/kernel-fingerprint.bin}"
aarch64-linux-gnu-objcopy -O binary -j .text "$VMLINUX" /tmp/vmlinux-text.bin
head -c 64 /tmp/vmlinux-text.bin > "$OUTPUT"
rm -f /tmp/vmlinux-text.bin
echo "Fingerprint: 64 bytes -> $OUTPUT"
xxd "$OUTPUT" | head -4
