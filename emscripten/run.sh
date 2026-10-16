#!/bin/sh

set -e

PROFILE=${1:-dev}
if [ $# -gt 0 ]; then
    shift
fi

cd `dirname $0`/..
./emscripten/build.sh $PROFILE $BUILD_ARGS

case $PROFILE in
  "dev")
    emrun target/wasm32-unknown-emscripten/debug/index.html --serve_after_close $@
    ;;
  "release")
    emrun target/wasm32-unknown-emscripten/release/index.html --serve_after_close $@
    ;;
  *)
    echo "Usage: $0 (dev|release) ..."
    exit 1;;
esac
