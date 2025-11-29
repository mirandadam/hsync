#!/bin/bash
set -e

# Ensure cargo-llvm-cov is installed
if ! cargo llvm-cov --version &> /dev/null; then
    echo "cargo-llvm-cov not found. Installing..."
    cargo install cargo-llvm-cov
fi

echo "Running tests with coverage..."
cargo llvm-cov --html --output-dir target/llvm-cov

echo "Coverage report generated at target/llvm-cov/index.html"
