# Technical Specification: High-Volume Streaming Sync Tool

## Problem Statement

### Objective

Reliably migrate approximately **200TB** of data between two network-mounted storage systems (NFS/SMB) over a shared Gigabit Ethernet link, ensuring minimal impact on concurrent workloads.

### Current Limitations

The standard tools `rsync` is unsuitable for this environment because it:

1. **Wastes Bandwidth:** Operate synchronously (read-then-write) on mounted shares, resulting in inefficient "half-duplex" throughput.
2. **Lacks Control:** Fail to strictly enforce bandwidth limits on kernel-buffered mounts, causing latency spikes for other users.
3. **Blocks on Scan:** Require a full sequential scan before any transfer can begin.

### Environment

- **Network:** Gigabit Ethernet, shared with other workloads.
- **Access:** Source and Destination are mounted network shares (NFS/SMB). No shell access to the servers.

### Solution

A custom **Producer-Consumer** application is required to:

- **Maximize Throughput:** Decouple read and write operations via a **FIFO queue** to ensure full-duplex streaming.
- **Stateful Efficiency:** Utilize a **local SQLite database** to track file state and maintain a transfer backlog, enabling efficient resumption without re-scanning.
- **Parallel Scanning:** Scan source and destination directories in parallel to build the initial backlog quickly.
- **Audit & Sync:** Perform on-the-fly checksum calculation for auditing and synchronize file timestamps (`mtime`/`atime`/`ctime`) while bypassing strict permission enforcement to avoid network share errors.

### Core Constraints

- Must utilize full-duplex network capabilities (streaming).
- Must operate efficiently on a mix of file sizes (megabytes to gigabytes).
- Must be resumable over a multi-week process.

---

## 2. Architectural Design

The application operates in two phases: a **Scan Phase** followed by a **Transfer Phase** using a **Producer-Consumer (FIFO Queue)** architecture.

### 2.1. Startup Behavior

The application automatically determines its mode based on database state:

1. **If database does not exist OR has no pending files:** Run full scan phase, then transfer phase.
2. **If database exists AND has pending files:** Skip scan, resume transfer phase from backlog.

After all transfers complete (backlog empty), the next run will perform a full rescan to detect new or changed files.

### 2.2. Scan Phase

- **Parallel Execution:** Source and destination directories are scanned in parallel (independent threads, not synchronized to each other).
- **Progress Display:** Show scan progress with file counts and aggregate size (human-readable) during this phase.
- **Database Population:**
  - For each source file, record its metadata and determine if transfer is needed.
  - Compare against destination filesystem to determine sync status.
  - Files needing transfer are marked as `pending` in the database.
  - Files already synced (destination exists with matching mtime) are marked as `synced`.
- **Hash Preservation:** When updating file records, existing hash values are preserved if the file hasn't changed.

### 2.3. The Pipeline (Queue)

- **Structure:** A FIFO queue (default 20 slots, configurable).
- **Block Definition:** Each entry in the queue contains:

| Field             | Description                                              |
|-------------------|----------------------------------------------------------|
| `Data`            | Byte buffer (Default **5MB**, configurable)              |
| `Offset`          | Integer indicating the write position in the destination file |
| `DestinationPath` | Full path string                                         |
| `Timestamps`      | Source `atime`, `mtime`, `ctime`                         |
| `Permissions`     | Source mode/ownership bits (for logging purposes)        |
| `IsLastBlock`     | Boolean flag                                             |
| `FileHash`        | String (Populated only if `IsLastBlock` is `True`)       |

### 2.4. The Reader (Producer)

- **Source:** Reads files from the pending backlog in the database (not filesystem scan).
- **Processing:**
  - Reads each pending file in blocks (default 5MB).
  - Calculates the checksum incrementally while reading.
  - Feeds blocks into the queue.
  - On the final block of a file: Sets `IsLastBlock = True` and attaches the calculated `FileHash`.

### 2.5. The Writer (Consumer)

- **Operation:** Pops blocks from the queue.
- **File Handling:**
  - If `Offset == 0`: Opens/creates the file at `DestinationPath`.
  - Writes `Data` to the file.
  - If `IsLastBlock == True`:
    1. Closes the file.
    2. **Metadata Sync:** Attempts to apply source `mtime` (Modification Time), `atime` (Access Time), and `ctime` (Change Time) to the destination file on disk. **Does not** apply permissions.
    3. **Persistence:** Updates database record, marking file as `synced` and storing the hash.
    4. **Audit:** Writes entry to log file.

---

## 3. Functional Requirements

### 3.1. Transfer & Integrity

- **Streaming:** Read and write operations must occur concurrently via the queue to ensure pipeline efficiency.
- **Bandwidth Limiting:** The tool must enforce a configurable speed limit (e.g., 20MB/s).
- **Checksums:**
  - Computed on-the-fly during the read phase.
  - **Algorithm:** User-configurable (MD5, SHA1, SHA256, BLAKE2b).
  - **Usage:** Stored in database/log for audit. **No read-back verification** is performed after writing.

### 3.2. File Skipping & Overwrite Strategy

- **Skip Criteria:** Destination file exists **AND** destination `mtime` == source `mtime` **AND** destination `size` == source `size`.
- **Overwrite:** If a file is not skipped, it is overwritten entirely.
- **Partial Files:** No support for resuming mid-file. If a transfer is interrupted, the specific file being transferred is restarted from offset 0 on the next run.

### 3.3. State Management (Resumability)

- **Database:** SQLite
- **Location:** Database file path must be user-configurable.
- **Schema:**

| Field          | Description                                      |
|----------------|--------------------------------------------------|
| Source Path    | Full source path (primary key)                   |
| Dest Path      | Full destination path                            |
| Created Date   | Source value                                     |
| Changed Date   | Source value                                     |
| Modified Date  | Source value                                     |
| Permissions    | Source values (stored for record only)           |
| Hash           | Checksum (null until file is transferred)        |
| Size           | File size in bytes                               |
| Status         | `pending` (needs transfer) or `synced` (complete)|

- **Resume Logic:** On startup, if the database contains `pending` files, resume transferring from the backlog without re-scanning.
- **Rescan Trigger:** If no `pending` files exist, perform a full scan to detect new or changed files.
- **Hash Preservation:** When rescanning, preserve existing hash values for files that haven't changed.

### 3.4. Mirroring (Cleanup)

- **Default State:** **Disabled** (copy/overwrite only).
- **Execution Phase:** Cleanup runs only after the main transfer loop has finished scanning/copying.
- **Safety Check:** Before deleting any file at the destination that appears to be "extra":
  - The tool must perform a **live check** against the source filesystem to confirm the file is truly absent.
  - Only if the live check confirms absence is the file deleted.

---

## 4. User Interface & Reporting

### 4.1. Console Output

- **Scan Phase:** Display progress showing files scanned and pending count.
- **Transfer Phase Metrics:**
  - Current file name
  - Current bandwidth usage
  - ETA for the *current file*
  - ETA for the *entire backlog* (based on total pending bytes from scan)
  - Total data copied (session/global)
- **Resume Feedback:** Indicate when resuming from existing backlog vs. performing fresh scan.

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
| Rescan             | Force full rescan, ignoring existing backlog | `--rescan`             |
| Block Size         | Size of transfer blocks (default: 5MB)       | `--block-size 1M`      |
| Queue Capacity     | Size of the block queue (default: 20)        | `--queue-capacity 50`  |

---

## 6. Quality Assurance & Tooling

### 6.1. Automated Testing
- The project must include **automated integration tests** to verify core functionality, including:
  - Full synchronization flow.
  - File skipping logic.
  - File truncation/update handling.
  - Mirroring/Cleanup functionality.
  - Bandwidth limiting enforcement.

### 6.2. Code Coverage
- The project must include tooling to measure and report **code coverage**.
- A script (e.g., `coverage.sh`) must be provided to easily generate these reports.

### 6.3. Benchmarking
- The project must include **throughput benchmarks** for the supported hash algorithms (MD5, SHA1, SHA256, BLAKE2b).
- Benchmarks are implemented using Criterion and are run with `cargo bench`.
- Benchmarks are separate from the production binary and do not affect the final tool.

### 6.4. Verification
- A standalone verification script (e.g., `verify.sh`) must be provided to demonstrate the tool's functionality in a clean environment, performing:
  - Setup of test data.
  - Execution of the sync tool.
  - Validation of transferred files (existence and content).
  - Validation of database and log file creation.
