# dsync - A cross platform LAN file synchronization tool

---

## Goal

Building a lightweight, rust based daemon that allows for real time file synchronization over LAN across multiple devices without cloud or centralized servers, ensuring fast and private folder mirroring.

- Lightweight & efficient → small resource footprint, suitable for always-on background use.

- LAN-only → no internet/cloud reliance, purely local.

- Automatic sync → creation, modification, and deletion events reflected across devices.

- Decentralized → no single server, all peers participate equally.

- Privacy-focused → stays entirely within the local network.

---

## Installation

### Prerequisites

- [Rust toolchain](https://rustup.rs/)

### Build from Source

1. Clone the repository:
```bash
git clone https://github.com/mel-edo/dsync.git
cd dsync
```

2. Build release binary:
```bash
cargo build --release
```

3. Binary will be located at 'target/release/dsync'

4. Add to local/bin
```bash
cp target/release/dsync ~/.local/bin/
```
---

## Usage

```bash
dsync -d [file_path] -p [port_number] -n [machine_name]
```

Example:
```bash
dsync -d ~/test_folder -p 9000 -n machine1
```

---

### License

Licensed under [GPL-3.0](LICENSE)
