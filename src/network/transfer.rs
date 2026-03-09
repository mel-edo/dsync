use std::{fs, io::Read, path::PathBuf};
use tokio::io::AsyncWriteExt;
use anyhow::bail;
use chacha20poly1305::{ChaCha20Poly1305, aead::{Aead, AeadCore, OsRng as AeadOsRng}};
use lz4_flex::block::compress_prepend_size;

use crate::core::event::FileChunk;
use crate::core::protocol::TransferMsg;

pub(super) const CHUNK_SIZE: usize = 1024 * 1024;

// helpers

// helper to chunk and stream a file
pub(super) async fn stream_file_to_writer(writer: &mut tokio::net::tcp::OwnedWriteHalf, root: &PathBuf, rel_path: &PathBuf, progress: Option<indicatif::ProgressBar>, cipher: &ChaCha20Poly1305) -> anyhow::Result<()> {
    let full_path = root.join(rel_path);

    if let Ok(mut file) = fs::File::open(&full_path) {
        let file_size = file.metadata()?.len();

        if let Some(ref pb) = progress {
            pb.set_length(file_size);
        }

        if file_size == 0 {
            let chunk = FileChunk {
                rel_path: rel_path.clone(),
                offset: 0,
                data: vec![],
                is_last: true,
                compressed: false,
            };
            send_encrypted(writer, cipher, &TransferMsg::Chunk(chunk)).await?;
            if let Some(pb) = progress {
                pb.finish_with_message("✓ Sent".to_string());
            }
            return Ok(());
        }

        let mut buffer = vec![0u8; CHUNK_SIZE];
        let mut total_sent = 0u64;

        loop {
            let bytes_read = file.read(&mut buffer)?;
            if bytes_read == 0 { break; }

            total_sent += bytes_read as u64;
            let is_last = total_sent >= file_size;

            let raw_data = &buffer[..bytes_read];
            let (data, compressed) = if should_compress(rel_path) {
                let c = compress_prepend_size(raw_data);
                if c.len() < raw_data.len() {
                    (c, true)
                } else {
                    (raw_data.to_vec(), false)
                }
            } else {
                (raw_data.to_vec(), false)
            };

            let chunk = FileChunk {
                rel_path: rel_path.clone(),
                offset: total_sent - bytes_read as u64,
                data,
                is_last,
                compressed,
            };
            let chunk_msg = TransferMsg::Chunk(chunk);
            send_encrypted(writer, cipher, &chunk_msg).await?;

            if let Some(ref pb) = progress {
                pb.set_position(total_sent);
            }
        }
        if let Some(pb) = progress {
            pb.finish_with_message("✓ Sent".to_string());
        }
    }

    Ok(())
}

pub(super) fn encrypt_msg(cipher: &ChaCha20Poly1305, msg: &TransferMsg) -> anyhow::Result<Vec<u8>> {
    let plaintext = bincode::serialize(msg)?;
    let nonce = ChaCha20Poly1305::generate_nonce(&mut AeadOsRng);
    let ciphertext = cipher.encrypt(&nonce, plaintext.as_ref())
        .map_err(|e| anyhow::anyhow!("Encryption failed: {:?}", e))?;
    // prepend nonce to ciphertext so receiver can use it
    let mut result = nonce.to_vec();
    result.extend(ciphertext);
    Ok(result)
}

pub(super) fn decrypt_msg(cipher: &ChaCha20Poly1305, data: &[u8]) -> anyhow::Result<TransferMsg> {
    if data.len() < 12 {
        bail!("Message too short to contain nonce");
    }
    let (nonce_bytes, ciphertext) = data.split_at(12);
    let nonce = chacha20poly1305::Nonce::from_slice(nonce_bytes);
    let plaintext = cipher.decrypt(nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("Decryption failed: {:?}", e))?;
    Ok(bincode::deserialize(&plaintext)?)
}

pub(super) async fn send_encrypted(writer: &mut tokio::net::tcp::OwnedWriteHalf, cipher: &ChaCha20Poly1305, msg: &TransferMsg) -> anyhow::Result<()> {
    let encrypted = encrypt_msg(cipher, msg)?;
    writer.write_all(&(encrypted.len() as u32).to_be_bytes()).await?;
    writer.write_all(&encrypted).await?;
    Ok(())
}

fn should_compress(path: &PathBuf) -> bool {
    const INCOMPRESSIBLE: &[&str] = &[
        "jpg", "jpeg", "png", "gif", "webp", "avif",
        "mp4", "mkv", "mov", "avi", "webm",
        "mp3", "aac", "flac", "ogg",
        "zip", "gz", "xz", "zst", "bz2", "7z", "rar",
        "pdf",
    ];
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => !INCOMPRESSIBLE.contains(&ext.to_lowercase().as_str()),
        None => true,
    }
}