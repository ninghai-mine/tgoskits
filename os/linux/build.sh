#!/bin/bash
# Build Linux kernel Image for AxVisor target guest (aarch64).
#
# Prerequisites:
#   1. Download kernel source to /tmp/linux-6.12.94:
#      git clone --depth 1 --branch v6.12.94 \
#        https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git \
#        /tmp/linux-6.12.94
#   2. Install cross-compiler:
#      apt install gcc-aarch64-linux-gnu
#
# Usage:
#   ./build.sh [output_dir]

set -euo pipefail

LINUX_SRC="${LINUX_SRC:-/tmp/linux-6.12.94}"
CONFIG_FILE="$(cd "$(dirname "$0")" && pwd)/config"
PATCH_FILE="$(cd "$(dirname "$0")" && pwd)/panic_hvc.patch"
OUTPUT_DIR="${1:-$(cd "$(dirname "$0")" && pwd)/../../target}"

cd "$LINUX_SRC"

echo "[1/4] Apply kernel config..."
cp "$CONFIG_FILE" .config

echo "[2/4] Apply HVC panic patch..."
patch -p1 < "$PATCH_FILE"

echo "[3/4] Build kernel Image..."
make ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu- -j$(nproc) Image

echo "[4/4] Copy Image to output dir..."
mkdir -p "$OUTPUT_DIR"
cp arch/arm64/boot/Image "$OUTPUT_DIR/linux-Image-6.12.94"

echo "Done: $OUTPUT_DIR/linux-Image-6.12.94"
