# dsync - Zero-Config P2P File Sync with Ephemeral Trust

**A peer-to-peer file synchronization tool for local networks that requires no prior configuration or persistent credentials.**

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)
[![Rust](https://img.shields.io/badge/rust-orange.svg)](https://www.rust-lang.org/)

---

## Overview

Most file synchronization tools rely on one of the following:

- Manual device pairing (e.g exchanging device IDs)
- Cloud-based coordination
- Pre-shared secrets or credentials
- Certificate authorities

`dsync` instead uses ephemeral identities and automatic peer discovery.

Typical workflow:

1. Start dsync → generates a temporary cryptographic identity
2. Peers are discovered automatically using mDNS
3. A challenge–response handshake verifies peer identity
4. An encrypted channel is established via X25519 key exchange
5. File synchronization begins
6. When dsync exits, the identity is discarded

---

## Features

### Security
- Ed25519 challenge–response authentication
- X25519 ECDH key exchange per session
- ChaCha20-Poly1305 end-to-end encryption
- Ephemeral identities generated on each startup
- No pre-configured peers or persistent credentials
- Rate limiting to protect against handshake flooding
- Designed for local networks only (no internet communication)

### Performance  
- Real-time synchronization using filesystem notifications
- Chunked file transfers for large files
- Blake3 hashing for fast integrity verification
- Asynchronous I/O using Tokio
- LZ4 compression for compressible files
- Persistent connection pool (eliminates per-transfer handshake overhead)
- Parallel transfers with configurable concurrency

### Network
- Automatic peer discovery using mDNS
- Direct peer-to-peer TCP connections
- Supports multiple peers on the same network
- Handles temporary network interruptions

### Privacy
- No cloud services
- No telemetry or data collection
- No accounts or registration
- Fully peer-to-peer architecture

---

## Installation

### Prerequisites
- [Rust toolchain](https://rustup.rs/) (1.80+)

### Build from Source

```bash
# Clone the repository
git clone https://github.com/mel-edo/dsync.git
cd dsync

# Build release binary
cargo build --release

# Install to system (optional)
sudo cp target/release/dsync /usr/local/bin/
# or for user-only install
cp target/release/dsync ~/.local/bin/
```

### Quick Start

```bash
# On Machine 1
dsync -d ~/path/to/folder

# On Machine 2
dsync -d ~/path/to/folder

# That's it! They'll discover each other and start syncing.
```

---

## Usage

### Basic Command

```bash
dsync -d <directory> -p <port> -n <name>
```

### Options

| Flag | Description | Default | Required |
|------|-------------|---------|----------|
| `-d, --path` | Directory to sync | - | Yes |
| `-p, --port` | TCP port to listen on | 9000 | No |
| `-n, --name` | Friendly name for this instance | `dsync-instance` | No |
| `-a, --peer` | Manually specify peer (format: `ip:port`) | - | No |
| `--no-discovery` | Disable automatic mDNS discovery | false | No |
| `-v, --verbose` | Show detailed logs | false | No |
| `-e, --exclude` | Exclude pattern, repeatable (e.g. `*.log`) | - | No |

### Examples

**Basic sync between two machines:**
```bash
# Machine A
dsync -d ~/shared -p 9000 -n machine-a

# Machine B  
dsync -d ~/shared -p 9000 -n machine-b
```

**Verbose mode (see what's happening):**
```bash
dsync -d ~/Documents -v
```

**Disable auto-discovery (manual peer specification):**
```bash
dsync -d ~/sync -p 9000 --no-discovery -a 192.168.1.100:9000
```

**Exclude patterns:**
```bash
dsync -d ~/code -e "*.log" -e "build/"
```

**Multiple instances on same machine (for testing):**
```bash
# Terminal 1
dsync -d ./test_a -p 9000 -n instance-a -v

# Terminal 2
dsync -d ./test_b -p 9001 -n instance-b -v

# Terminal 3
dsync -d ./test_c -p 9002 -n instance-c -v
```

---

### Configuration

dsync looks for a config file at:

- Linux/macOS: `~/.config/dsync/dsync.toml`
- Windows: `%APPDATA%\dsync\dsync.toml`

All values are optional and overridden by CLI arguments.

```toml
# dsync.toml

# Folder to sync (can be overridden with -d)
path = "/home/user/sync"

# Port to listen on
port = 9000

# Instance name shown during discovery
name = "dsync-instance"

# Static peer to always connect to (in addition to mDNS)
# peer = "192.168.1.100:9000"

# Disable mDNS auto-discovery
# no_discovery = false

# Maximum concurrent file transfers
max_concurrent_transfers = 4
```

## How It Works

### 1. Ephemeral Identity Generation
```
On startup:
├─ Generate Ed25519 keypair (signing key)
├─ Derive peer ID from public key
└─ Identity exists only in memory (never saved to disk)
```

### 2. Peer Discovery
```
mDNS Broadcast:
├─ Service: _dsync._tcp.local
├─ Port: 9000 (configurable)
└─ TXT Record: {id: <public_key_hex>}

Peers discover each other automatically on LAN
```

### 3. Trust Establishment
```
Client → Server: [TCP Connect]
Server → Client: Challenge (32 random bytes)
Client → Server: {PublicKey, Signature(Challenge)}
Server: Verify signature → Send Ack + X25519 public key
Client → Server: X25519 public key
Both: Derive shared secret → ChaCha20-Poly1305 encrypted channel

If signature valid → Encrypted channel established
If signature invalid → Connection dropped immediately
```

### 4. File Synchronization
```
Initial Sync:
├─ Exchange file indexes (path, size, modified time)
├─ Calculate diff (missing/outdated files)
└─ Request missing files

Real-time Sync:
├─ File watcher detects changes
├─ Compute Blake3 hash
├─ Compress with LZ4 (if compressible)
├─ Encrypt and broadcast to all trusted peers
└─ Peers apply changes locally
```

---

## File Ignore Rules

A `.dsyncignore` file is automatically created in your sync directory on first run. Edit it to control what gets synced.

```gitignore
# dsyncignore - files and patterns listed here will not be synced
# Uses glob syntax. Lines starting with # are comments.
# Version control
.git
.git/**

# Dependencies
node_modules
node_modules/**

# OS files
.DS_Store

# Temporary files
*.tmp
*.part
```

**Always ignored**:
- `*.part` - partial downloads in progress
- `.dsyncignore` - the ignore file itself

---

## Security Considerations

### What dsync DOES provide:
**Authentication** - peers prove ownership of their advertised public key
**Encryption** - all traffic is encrypted with ChaCha20-Poly1305
**Integrity** - Blake3 hashing detects corruption and prevents duplicate transfers
**Ephemeral identity** - no persistent credentials to steal or leak
**Local trust** - only peers on the same mDNS domain can discover each other
**DoS protection** - rate limiting on handshake attempts per IP 

### What dsync does Not provide:
**Internet sync** - dsync is designed for trusted local networks only  
**Authorization** - Any peer on LAN that completes the handshake can sync (by design)  
**Anonymity** - Hostnames/IPs are visible on the network

### Threat Model

**Protected against:**
- Unauthorized peers (must complete Ed25519 handshake)
- Replay attacks (challenge-response with fresh random challenge each time)
- Identity theft (keys are never saved to disk)
- Passive eavesdropping (ChaCha20-Poly1305 encryption)
- Handshake flooding (per-IP rate limiting)

**Not protected against:**
- Malicious peers on the same network (dsync assumes a trusted LAN)
- Man-in-the-middle (mDNS can be spoofed on hostile networks)
- Physical access to a running machine (keys exist in memory while running)

**Recommendation:** Use on trusted networks (home/office LAN, VPN). Do not expose port 9000 to the internet.

---

## Roadmap

### Completed
- Ephemeral Ed25519 identity
- mDNS peer discovery
- Challenge-response handshake
- X25519 ECDH + ChaCha20-Poly1305 end-to-end encryption
- Real-time file watching
- Chunked file transfers
- LZ4 compression
- Persistent connection pool
- Parallel transfers
- Progress indicators
- .dsyncignore with auto-generation
- --exclude CLI flag
- Rate limiting (DoS protection)
- Config file support (dsync.toml)
- Graceful shutdown

### In Progress
- [ ] Multi-peer support
- [ ] Cross-platform support

---

## Troubleshooting

### Peers not discovering each other

**Check:**
1. Both machines on same network segment (mDNS doesn't cross VLANs)
2. Firewall allows UDP 5353 (mDNS) and TCP port 9000
3. mDNS/Avahi service running (Linux: `systemctl status avahi-daemon`)

**Workaround:**
```bash
# Manually specify peer IP
dsync -d ~/sync -p 9000 -a 192.168.1.100:9000
```

### Files not syncing

**Check:**
1. File not in `.dsyncignore`
2. Run with `-v` to see events
3. Check file permissions
4. Verify available disk space on receiving end

---

## Contributing

Suggestions, fixes and improvments are welcome. Feel free to open an issue or a PR.

---

## License

This project is licensed under the GNU General Public License v3.0 - see the [LICENSE](LICENSE) file for details.