# hsync: High-Volume Streaming Sync Tool

A high-performance application designed to reliably migrate large datasets between network-mounted storage systems (NFS/SMB).

## Features

- **Producer-Consumer Architecture**: Decouples read and write operations for full-duplex throughput.
- **Resumable**: Uses a local SQLite database to track file state, enabling immediate startup and efficient resumption.
- **Bandwidth Limiting**: Configurable transfer speed limit.
- **Integrity**: On-the-fly checksum calculation and metadata synchronization (mtime, atime).
- **Mirroring**: Optional cleanup of extra files at the destination (with safety checks).

## Usage

```bash
cargo run --release -- \
  --source /path/to/source \
  --dest /path/to/destination \
  --bwlimit 20M
```

### Arguments

- `--source`: Path to source directory.
- `--dest`: Path to destination directory.
- `--db`: Local database file path (default: `hsync.db`).
- `--log`: Audit log file path (default: `hsync.log`).
- `--bwlimit`: Maximum transfer speed. Supports human-readable suffixes:
  - `K` or `k`: Kilobytes (×1024)
  - `M` or `m`: Megabytes (×1024²)
  - `G` or `g`: Gigabytes (×1024³)
  - No suffix: raw bytes per second
  - Examples: `20M`, `512K`, `1.5G`, `20000000`
- `--checksum`: Checksum algorithm to use (default: `sha256`). Supported values: `md5`, `sha1`, `sha256`, `blake2b`.
- `--delete-extras`: Enable deletion of extra files in destination.
- `--rescan`: Force a full rescan, ignoring any existing backlog.
- `--block-size`: Block size for file transfer (e.g., `1M`, `512K`). Default: `5MB`.
- `--queue-capacity`: Size of the block queue. Default: `20`.

## Build

```bash
cargo build --release
```

## Specification

This implementation adheres to the requirements defined in [Specification.md](Specification.md).

## License

Licensed under either of

 * Apache License, Version 2.0
   ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license
   ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
