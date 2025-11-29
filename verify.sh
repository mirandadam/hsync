#!/bin/bash
set -e

# Setup
TEST_DB="test_hsync.db"
TEST_LOG="test_hsync.log"

rm -rf test_source test_dest "$TEST_DB" "$TEST_LOG"
mkdir -p test_source
echo "Hello World" > test_source/file1.txt
dd if=/dev/urandom of=test_source/large_file.bin bs=1M count=10

# Run hsync
echo "Running hsync..."
cargo run -- --source $(pwd)/test_source --dest $(pwd)/test_dest --bwlimit 10000000 --db "$TEST_DB" --log "$TEST_LOG"

# Verify
echo "Verifying..."
if [ -f test_dest/file1.txt ]; then
    echo "file1.txt exists"
else
    echo "file1.txt missing"
    exit 1
fi

if [ -f test_dest/large_file.bin ]; then
    echo "large_file.bin exists"
else
    echo "large_file.bin missing"
    exit 1
fi

# Check content
diff test_source/file1.txt test_dest/file1.txt
cmp test_source/large_file.bin test_dest/large_file.bin

# Check DB
if [ -f "$TEST_DB" ]; then
    echo "Database exists"
else
    echo "Database missing"
    exit 1
fi

# Check Log
if [ -f "$TEST_LOG" ]; then
    echo "Log exists"
else
    echo "Log missing"
    exit 1
fi

echo "Verification Passed!"
