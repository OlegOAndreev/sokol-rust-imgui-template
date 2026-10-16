# sokol-rust + imgui template

A small example app which integrates sokol-rust with imgui (and optionally egui) and supports both native builds and emscripten.

## Building and running

* After checkout run `git submodule update --init --recursive` once to checkout sokol-tools-bin.
* The shaders are recompiled automatically using sokol-shdc (see build.rs for details).
* For native targets, simply run `cargo run`
* For emscripten
  * run `rustup target add wasm32-unknown-emscripten`
  * [install emscripten SDK](https://emscripten.org/docs/getting_started/downloads.html)
  * activate emscripten SDK using `emsdk_env.sh`
  * run `./emscripten/run.sh (dev|release)`

## Cargo features

* `egui` - renders the same UI a second time with [egui](https://github.com/emilk/egui) next to the imgui one.

  Enable it by appending `--features egui` to the commands above, e.g. `cargo run --features egui`.
