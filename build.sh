#!/bin/sh
set -eu
cd "$(dirname "$0")"
rustup target add wasm32-unknown-unknown
if ! command -v wasm-bindgen >/dev/null 2>&1; then
  cargo install wasm-bindgen-cli --version 0.2.129 --locked
fi
cargo build --locked -p rustlink-web --target wasm32-unknown-unknown --release
wasm-bindgen target/wasm32-unknown-unknown/release/rustlink_web.wasm --target web --out-dir web/dist/pkg --no-typescript
cargo test --locked --workspace
cargo build --locked --release -p rustlink
