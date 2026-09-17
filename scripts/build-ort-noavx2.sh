#!/usr/bin/env bash
# Build ONNX Runtime from source WITHOUT AVX/AVX2/AVX-512, producing a
# libonnxruntime.so that runs on any x86_64 CPU (SSE2 baseline) while still
# dispatching to AVX2/AVX-512 kernels at runtime when the CPU supports them
# (MLAS CPUID dispatch + onnxruntime_ENABLE_CPUINFO).
#
# Why: the official prebuilt ONNX Runtime binaries are compiled with AVX2 and
# SIGILL-crash on pre-Haswell CPUs (e.g. Sandy Bridge). basemind links ORT via
# the `ort` crate, so an AVX2-less CPU needs a custom ORT + basemind built with
# the `ort-dynamic` feature against it.
#
# ORT version MUST match what ort-sys expects (see ort-sys's build/download/
# dist.tsv for the x86_64-unknown-linux-gnu row; e.g. 1.28.0 for
# ort 2.0.0-rc.13). A mismatched C ABI makes ort panic at startup
# ("unsupported version of ONNX Runtime").
#
# Usage:
#   ORT_VERSION=1.28.0 ./scripts/build-ort-noavx2.sh <source-dir> <build-dir> <install-prefix>
#
# In CI (manylinux_2_28 container) this also keeps the glibc floor at 2.28.
set -euo pipefail

if [ $# -ne 3 ]; then
  echo "Usage: $0 <source-dir> <build-dir> <install-prefix>" >&2
  exit 1
fi

SRC="$1"
BUILD="$2"
PREFIX="$3"
ORT_VERSION="${ORT_VERSION:-1.28.0}"

if [ ! -d "$SRC" ]; then
  git clone --depth 1 --branch "v${ORT_VERSION}" --recursive --shallow-submodules \
    https://github.com/microsoft/onnxruntime.git "$SRC"
fi

cd "$SRC"
# Fail on a stale or unrelated reuse of $SRC: require the ORT repo, make
# fetch/checkout fatal, and verify HEAD matches the requested tag before
# building (a silent fallback would ship an unverified ORT).
git remote get-url origin | grep -q "onnxruntime" || {
  echo "unexpected git remote in $SRC (expected onnxruntime)" >&2
  exit 1
}
git fetch --depth 1 origin "refs/tags/v${ORT_VERSION}:refs/tags/v${ORT_VERSION}"
git checkout "v${ORT_VERSION}"
[ "$(git rev-parse HEAD)" = "$(git rev-list -n 1 "v${ORT_VERSION}")" ] || {
  echo "HEAD does not match v${ORT_VERSION} in $SRC" >&2
  exit 1
}

# CMAKE_DISABLE_FIND_PACKAGE_flatbuffers: FetchContent prefers any system
# flatbuffers >= 23.5.9 over fetching v23.5.26 — dev machines with Android SDK
# installed resolve the emulator's flatbuffers v25.1, which breaks the
# checked-in v23-generated headers (static_assert failure).
python3 tools/ci_build/build.py \
  --build_dir "$BUILD" \
  --config Release \
  --build_shared_lib \
  --parallel "$(nproc)" \
  --skip_tests \
  --cmake_extra_defines \
    onnxruntime_ENABLE_CPUINFO=ON \
    onnxruntime_USE_AVX=OFF \
    onnxruntime_BUILD_FOR_NATIVE_MACHINE=OFF \
    CMAKE_DISABLE_FIND_PACKAGE_flatbuffers=TRUE \
    'CMAKE_C_FLAGS=-mno-avx -mno-avx2 -mno-fma -mno-avx512f' \
    'CMAKE_CXX_FLAGS=-mno-avx -mno-avx2 -mno-fma -mno-avx512f'

SO="$BUILD/Release/libonnxruntime.so.${ORT_VERSION}"
[ -f "$SO" ] || {
  echo "expected shared library not found: $SO" >&2
  exit 1
}

mkdir -p "$PREFIX/lib"
cp "$SO" "$PREFIX/lib/"
ln -sf "libonnxruntime.so.${ORT_VERSION}" "$PREFIX/lib/libonnxruntime.so.1"
ln -sf libonnxruntime.so.1 "$PREFIX/lib/libonnxruntime.so"

echo "✓ ONNX Runtime ${ORT_VERSION} (no AVX) installed to $PREFIX/lib"
ls -la "$PREFIX/lib/" | grep onnxruntime
