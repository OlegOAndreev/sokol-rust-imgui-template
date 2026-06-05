#!/bin/sh

set -eu

if [ -z "${EMSDK:-}" ]; then
    echo "Error: no EMSDK found. Did you forget to install and activate Emscripten SDK? Run 'source emsdk_env.sh' first."
    exit 1
fi

PROFILE=$1
shift

cd `dirname $0`/..

case $PROFILE in
  "dev")
    export EMCC_CFLAGS="-sASSERTIONS=1 -sSTACK_OVERFLOW_CHECK=2"
    cargo build --target wasm32-unknown-emscripten $@
    cp emscripten/index.html target/wasm32-unknown-emscripten/debug/
    ;;
  "release")
    export EMCC_CFLAGS="-sSTACK_OVERFLOW_CHECK=1"
    cargo build --target wasm32-unknown-emscripten --release $@
    cp emscripten/index.html target/wasm32-unknown-emscripten/release/
    ;;
  "dist")
    # Build for static pages
    rustup show
    rustup target add wasm32-unknown-emscripten

    export EMCC_CFLAGS="-sSTACK_OVERFLOW_CHECK=1"
    cargo build --target wasm32-unknown-emscripten --release $@

    mkdir -p dist
    cp target/wasm32-unknown-emscripten/release/sokol-rust-imgui-template.js dist/
    cp target/wasm32-unknown-emscripten/release/sokol_rust_imgui_template.wasm dist/
    cp emscripten/index.html dist/
    ;;
  *)
    echo "Usage: $0 (dev|release|dist) ..."
    exit 1;;
esac
