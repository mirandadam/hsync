# hsync: High-Volume Streaming Sync Tool

A proof-of-concept application designed to reliably migrate large datasets between network-mounted storage systems (NFS/SMB).

## Features

- **Producer-Consumer Architecture**: Decouples read and write operations for full-duplex throughput.
- **Resumable**: Uses a local SQLite database to track file state, enabling immediate startup and efficient resumption.
- **Bandwidth Limiting**: Configurable transfer speed limit.
- **Integrity**: On-the-fly checksum calculation and metadata synchronization (mtime, atime).

## Usage

```bash
cargo run --release -- \
  --source /path/to/source \
  --dest /path/to/destination \
  --bwlimit 20000000
```

### Arguments

- `--source`: Path to source directory.
- `--dest`: Path to destination directory.
- `--db`: Local database file path (default: `hsync.db`).
- `--log`: Audit log file path (default: `hsync.log`).
- `--bwlimit`: Maximum transfer speed in bytes per second.

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
