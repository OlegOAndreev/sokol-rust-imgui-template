# sokol-rust + imgui template

A small example app which integrated sokol-rust and imgui and support both native builds and emscripten.

## Building and running

* After checkout run `git submodule update --init --recursive` once to checkout sokol-tools-bin.
* The shaders are recompiled automatically using sokol-shdc (see build.rs for details).
* For native targets, simply run `cargo run`
* For emscripten
  * run `rustup target add wasm32-unknown-emscripten`
  * [install emscripten SDK](https://emscripten.org/docs/getting_started/downloads.html)
  * activate emscripten SDK using `emsdk_env.sh`
  * run `./emscripten/run.sh (dev|release)`
