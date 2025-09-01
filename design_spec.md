## Phase 1

### Scope & Constraints

Scope: LAN-only file synchronization (no WAN, no cloud, no external servers).

Constraints:

- Works on Linux, Windows, macOS.
- One shared folder per device (configurable and *later expand to as many folders as the users want*)
- Must detect: create, modify, delete.
- Sync should be real-time, not periodic.

### What's an event

- Should have an operation (create, modify, delete)
- file path
- hash (for integrity check)
- timestamp (for last writer's win condition -> Conflict resolution)
- probably a struct

### Networking portion

Discovery:

- UDP broadcast every x seconds
- mDNS and however that works (automatic service discovery)

Transport:

- TCP (reliable, easy)
- QUIC (faster, complex)

Syncing:

- full rescan + send hashes and sync differences

### Crates & Tools to Use

File watching: notify crate.

Networking: tokio (for async IO), possibly tokio::net for TCP/UDP.

Serialization: serde + bincode (binary serialization for speed).

Hashing: blake3 (fast, secure hash).
