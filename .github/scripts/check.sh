#!/usr/bin/env bash
set -euo pipefail

# Test with the workspace's minimum Rust version, including its minimum patch.
minimum=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["rust-version"])')
case "$minimum" in
    *.*.*) RUSTUP_TOOLCHAIN="$minimum" ;;
    *) RUSTUP_TOOLCHAIN="$minimum.0" ;;
esac
export RUSTUP_TOOLCHAIN
rustup toolchain install "$RUSTUP_TOOLCHAIN" --profile minimal

# Exercise the recording sink and the example apps with and without their hooks.
sudo apt-get update
sudo apt-get install -y ffmpeg
# Keep the many separately linked test binaries within the hosted runner's disk budget.
export CARGO_PROFILE_DEV_DEBUG=0
cargo build --workspace --locked
cargo test --workspace --all-targets --all-features --locked
cargo test --workspace --doc --all-features --locked
cargo doc --workspace --no-deps --all-features --locked
cargo package -p uiprobe-hooks --list --locked
cargo package -p uiprobe --locked
