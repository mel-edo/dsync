# dsync - Zero-Config P2P File Sync with Ephemeral Trust

**A decentralized file synchronization tool that establishes trust automatically without pre-configuration, cloud services, or persistent credentials.**

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)

---

## What Makes dsync Different?

Most sync tools require one of:
- **Manual setup** (Syncthing: copy device IDs, approve peers)
- **Cloud accounts** (Dropbox, Google Drive, iCloud)
- **Pre-shared secrets** (Resilio Sync)
- **Certificate authorities** (Traditional TLS/SSL)

**dsync does none of this.**

1. Start dsync → Generates temporary cryptographic identity
2. Automatic discovery → Finds peers via mDNS (like AirDrop)
3. Zero-config handshake → Verifies identity via challenge-response
4. Sync begins → No setup, no configuration, no cloud
5. Exit dsync → Identity destroyed, trust relationships gone

---

## Features

### Security
- **Ed25519 cryptographic handshake** - Challenge-response authentication
- **Ephemeral identities** - New keys generated on each startup
- **Zero trust by default** - No pre-configured peers, no persistent credentials
- **Local-only** - Never touches the internet, all traffic stays on LAN

### Performance  
- **Real-time sync** - File changes detected and synced instantly
- **Chunked transfers** - Large files streamed efficiently
- **Blake3 hashing** - Fast file integrity verification
- **Async I/O** - Powered by Tokio for high concurrency

### Network
- **Automatic peer discovery** - mDNS/Bonjour (zero manual configuration)
- **Cross-platform** - Works on Linux, macOS, Windows
- **Multi-peer mesh** - Sync across 2+ devices simultaneously
- **Resilient** - Handles network interruptions gracefully

### Privacy
- **No cloud** - Files never leave your local network
- **No telemetry** - Zero data collection
- **No accounts** - No registration, no tracking
- **Decentralized** - No single point of failure or surveillance

---

## Installation

### Prerequisites
- [Rust toolchain](https://rustup.rs/) (1.70+)

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
dsync -d ~/Documents -p 9000 -n laptop

# On Machine 2
dsync -d ~/Documents -p 9000 -n desktop

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
dsync -d ~/Documents -p 9000 -n laptop -v
```

**Disable auto-discovery (manual peer specification):**
```bash
dsync -d ~/sync -p 9000 --no-discovery -a 192.168.1.100:9000
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

### 3. Trust Establishment (The Novel Part!)
```
Client → Server: [TCP Connect]
Server → Client: Challenge (32 random bytes)
Client → Server: {PublicKey, Signature(Challenge)}
Server: Verify signature matches public key from mDNS
Server → Client: [Accept/Reject]

If signature valid → Trusted connection established
If signature invalid → Connection dropped
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
├─ Broadcast to all trusted peers
└─ Peers apply changes locally
```

---

**Components:**
- **File Watcher** - Monitors directory for changes (using `notify` crate)
- **mDNS Discovery** - Broadcasts/discovers peers (using `mdns-sd` crate)
- **Connection Pool** - Manages TCP connections to peers
- **Handshake Module** - Ephemeral trust establishment (Ed25519)
- **Sync Engine** - Handles file transfers and conflict resolution

---

## File Ignore Rules

Create a `.dsyncignore` file in your sync directory:

```gitignore
# System files
.DS_Store
Thumbs.db

# Build artifacts
target/
node_modules/
*.o
*.so

# Temporary files
*.tmp
*.swp
*~

# Version control
.git/
.svn/
```

**Default ignores** (always applied):
- `.git/`
- `*.part` (partial downloads)
- `.DS_Store`
- `node_modules/`

---

## Security Considerations

### What dsync DOES provide:
**Authentication** - Peers prove ownership of advertised public key  
**Integrity** - Blake3 hashing detects corruption  
**Local trust** - Only peers on same mDNS domain can discover  
**Ephemeral identity** - No persistent credentials to steal  

### What dsync does NOT provide (yet):
**Encryption** - Traffic is NOT encrypted (planned feature)  
**Authorization** - Any peer on LAN can sync (by design)  
**Anonymity** - Hostnames/IPs are visible  

### Threat Model

**Protected against:**
- Unauthorized peers (must complete handshake)
- Replay attacks (challenge-response prevents this)
- Identity theft (keys never saved, can't be stolen)

**NOT protected against:**
- Network sniffing (no encryption yet)
- Malicious peer on your LAN (assumes trusted network)
- Man-in-the-middle (mDNS can be spoofed on hostile networks)

**Recommendation:** Use on trusted networks (home/office LAN, VPN). For hostile networks, encryption will be added in future releases.

---

## Roadmap

### Completed (v0.1)
- [x] Ephemeral Ed25519 identity
- [x] mDNS peer discovery
- [x] Challenge-response handshake
- [x] Real-time file watching
- [x] Chunked file transfers
- [x] Progress indicators
- [x] Better error messages
- [x] Graceful shutdown

### In Progress
- [ ] Multi-peer support
- [ ] Cross-platform support
- [ ] End-to-end encryption (ChaCha20-Poly1305)
- [ ] `.dsyncignore` improvements

### 🔮 Planned Features
- [ ] Compression (LZ4)
- [ ] Bandwidth throttling

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
4. Verify disk space on receiving end

---

## Contributing

Contributions welcome! Please:

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

---

## License

This project is licensed under the GNU General Public License v3.0 - see the [LICENSE](LICENSE) file for details.