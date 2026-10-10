#!/bin/sh

set -e

PROFILE=${1:-dev}
if [ $# -gt 0 ]; then
    shift
fi

cd `dirname $0`/..
./emscripten/build.sh $PROFILE

case $PROFILE in
  "dev")
    emrun target/wasm32-unknown-emscripten/debug/index.html $@
    ;;
  "release")
    emrun target/wasm32-unknown-emscripten/release/index.html $@
    ;;
  *)
    echo "Usage: $0 (dev|release) ..."
    exit 1;;
esac
