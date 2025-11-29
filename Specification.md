# Technical Specification: High-Volume Streaming Sync Tool

## Problem Statement

### Objective

Reliably migrate approximately **200TB** of data between two network-mounted storage systems (NFS/SMB) over a shared Gigabit Ethernet link, ensuring minimal impact on concurrent workloads.

### Current Limitations

The standard tools `rsync` is unsuitable for this environment because it:

1. **Wastes Bandwidth:** Operate synchronously (read-then-write) on mounted shares, resulting in inefficient "half-duplex" throughput.
2. **Lacks Control:** Fail to strictly enforce bandwidth limits on kernel-buffered mounts, causing latency spikes for other users.
3. **Stalls on Resume:** Require prohibitive "pre-scan" times to identify changed files before transferring data, making resumption slow and inefficient.

### Environment

- **Network:** Gigabit Ethernet, shared with other workloads.
- **Access:** Source and Destination are mounted network shares (NFS/SMB). No shell access to the servers.

### Solution

A custom **Producer-Consumer** application is required to:

- **Maximize Throughput:** Decouple read and write operations via a **FIFO queue** to ensure full-duplex streaming.
- **Stateful Efficiency:** Utilize a **local SQLite database** to track file state, enabling immediate startup (no pre-scan) and efficient resumption.
- **Audit & Sync:** Perform on-the-fly checksum calculation for auditing and synchronize file timestamps (`mtime`/`atime`/`ctime`) while bypassing strict permission enforcement to avoid network share errors.

### Core Constraints

- Must utilize full-duplex network capabilities (streaming).
- Must operate efficiently on a mix of file sizes (megabytes to gigabytes).
- Must be resumable over a multi-week process.

---

## 2. Architectural Design

The application will utilize a **Producer-Consumer (FIFO Queue)** architecture to decouple read operations from write operations.

### 2.1. The Pipeline (Queue)

- **Structure:** A fixed-size FIFO queue (e.g., 20 slots).
- **Block Definition:** Each entry in the queue contains:

| Field             | Description                                              |
|-------------------|----------------------------------------------------------|
| `Data`            | Byte buffer (Maximum **5MB**)                            |
| `Offset`          | Integer indicating the write position in the destination file |
| `DestinationPath` | Full path string                                         |
| `Timestamps`      | Source `atime`, `mtime`, `ctime`                         |
| `Permissions`     | Source mode/ownership bits (for logging purposes)        |
| `IsLastBlock`     | Boolean flag                                             |
| `FileHash`        | String (Populated only if `IsLastBlock` is `True`)       |

### 2.2. The Reader (Producer)

- **Discovery:** Iterates through the Source directory structure.
- **Skipping Logic:** Checks if the file exists at the Destination.
  - If **Exists** AND **Mtime matches Source**: Skip file (do not read, do not add to queue).
  - Otherwise: Begin processing.
- **Processing:**
  - Reads the file in blocks (max 5MB).
  - Calculates the checksum incrementally while reading.
  - Feeds blocks into the queue.
  - On the final block of a file: Sets `IsLastBlock = True` and attaches the calculated `FileHash`.

### 2.3. The Writer (Consumer)

- **Operation:** Pops blocks from the queue.
- **File Handling:**
  - If `Offset == 0`: Opens/creates the file at `DestinationPath`.
  - Writes `Data` to the file.
  - If `IsLastBlock == True`:
    1. Closes the file.
    2. **Metadata Sync:** Attempts to apply source `mtime` (Modification Time), `atime` (Access Time), and `ctime` (Change Time) to the destination file on disk. **Does not** apply permissions.
    3. **Persistence:** Writes record to local database.
    4. **Audit:** Writes entry to log file.

---

## 3. Functional Requirements

### 3.1. Transfer & Integrity

- **Streaming:** Read and write operations must occur concurrently via the queue to ensure pipeline efficiency.
- **Bandwidth Limiting:** The tool must enforce a configurable speed limit (e.g., 20MB/s).
- **Checksums:**
  - Computed on-the-fly during the read phase.
  - **Algorithm:** User-configurable (MD5, SHA1, SHA256).
  - **Usage:** Stored in database/log for audit. **No read-back verification** is performed after writing.

### 3.2. File Skipping & Overwrite Strategy

- **Skip Criteria:** Destination file exists **AND** destination `mtime` == source `mtime`.
- **Overwrite:** If a file is not skipped, it is overwritten entirely.
- **Partial Files:** No support for resuming mid-file. If a transfer is interrupted, the specific file being transferred is restarted from offset 0 on the next run.

### 3.3. State Management (Resumability)

- **Database:** SQLite
- **Location:** Database file path must be user-configurable.
- **Schema:**

| Field          | Description                               |
|----------------|-------------------------------------------|
| Source Path    | Full source path                          |
| Dest Path      | Full destination path                     |
| Created Date   | Source value                              |
| Changed Date   | Source value                              |
| Modified Date  | Source value                              |
| Permissions    | Source values (stored for record only)    |
| Hash           | Checksum                                  |
| Size           | File size in bytes                        |

- **Skipped Files:** Database record is updated, but `Hash` field is left **null/blank**.

### 3.4. Mirroring (Cleanup)

- **Default State:** **Disabled** (copy/overwrite only).
- **Execution Phase:** Cleanup runs only after the main transfer loop has finished scanning/copying.
- **Safety Check:** Before deleting any file at the destination that appears to be "extra":
  - The tool must perform a **live check** against the source filesystem to confirm the file is truly absent.
  - Only if the live check confirms absence is the file deleted.

---

## 4. User Interface & Reporting

### 4.1. Console Output

- **Start-up:** Immediate execution (no pre-scan phase).
- **Metrics:**
  - Current file name
  - Current bandwidth usage
  - ETA for the *current file*
  - Total data copied (session/global)

### 4.2. Logging

- **Audit Log:** Plain text or structured log file.
- **Configuration:** Log file path must be user-configurable.
- **Content:** Success/failure status, source path, destination path, checksum (if transferred), timestamp.

---

## 5. Configuration Summary

The tool must accept arguments/config for:

| Option             | Description                                  | Example                |
|--------------------|----------------------------------------------|------------------------|
| Source Path        | Path to source directory                     |                        |
| Destination Path   | Path to destination directory                |                        |
| Database Path      | Local database file path                     |                        |
| Log File Path      | Audit log file path                          |                        |
| Bandwidth Limit    | Maximum transfer speed                       | `--bwlimit 20M`        |
| Hash Algorithm     | Checksum algorithm to use                    | `--checksum sha256`    |
| Mirror Mode        | Enable deletion of extra files (default: off)| `--delete-extras`      |
| Rescan             | Force database update                        |                        |
