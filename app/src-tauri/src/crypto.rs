//! 应用层对称加密: AES-256-GCM + HKDF(machine-id) 派生 master key.
//!
//! RULE-D-001 (P1 安全债): provider api_key 不再明文存 DB.
//!
//! 威胁模型: 防"DB 文件 / app_data_dir 整体泄露" —— 攻击者拿到 DB
//! 但没有本机 machine-id, 无法派生 master key, 密文不可解. 明确不防:
//! 本机 root / 运行中进程内存窃取(超出威胁模型).
//!
//! 密文格式: `base64( VERSION(1B) || nonce(12B) || ciphertext || tag(16B) )`
//! - `VERSION` 前缀: 未来换算法/KDF 时老密文可识别, decrypt 拒绝未知版本.
//! - `nonce`: 每条密文 OsRng 随机 96-bit(同明文两次加密 → 不同密文).
//! - `tag`: Poly1305 认证(aead crate 自动 append 到 ciphertext 尾部).
//! - `aad`(关联数据): 调用方传 provider id, 密文与 provider 绑定 ——
//!   把 A 行密文复制到 B 行解密会失败(防 DB 内挪用).
//!
//! 见 `.trellis/tasks/06-24-p1-api-key-encryption/research/app-layer-encryption-rust.md`.

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
    Aes256Gcm, Key, Nonce,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use hkdf::Hkdf;
use sha2::Sha256;

/// 打包格式版本. 换算法/KDF 时 bump, [`decrypt`] 拒绝未知版本.
const VERSION: u8 = 0x01;
/// HKDF salt: 绑应用 + 方案版本.
const SALT: &[u8] = b"everlasting::provider-key::v1";
/// HKDF info: 绑用途(context string).
const INFO: &[u8] = b"aes-256-gcm master key";
/// AES-GCM nonce 长度(96-bit).
const NONCE_LEN: usize = 12;
/// 最小密文长度: VERSION(1) + nonce(12) + 空 ct 的 tag(16).
const MIN_BLOB_LEN: usize = 1 + NONCE_LEN + 16;

/// 从本机 machine-id 派生 32B master key. 启动时调用一次, 缓存进 `AppState`.
///
/// machine-id 来源: Linux `/etc/machine-id` / macOS `gethostuuid` /
/// Windows `MachineGuid`, 由 `machine-uid` crate 跨平台取.
///
/// 机器绑定固有性质: `wsl --unregister` / 重装发行班会重置 machine-id
/// → 旧密文不可解密(非 bug, 解密失败兜底友好提示引导重粘 key).
pub fn derive_master_key() -> anyhow::Result<[u8; 32]> {
    let mid = machine_uid::get().map_err(|e| anyhow::anyhow!("read machine-id: {e}"))?;
    let hk = Hkdf::<Sha256>::new(Some(SALT), mid.as_bytes());
    let mut okm = [0u8; 32];
    hk.expand(INFO, &mut okm)
        .map_err(|e| anyhow::anyhow!("hkdf expand: {e}"))?;
    Ok(okm)
}

/// 加密明文, 返回 base64 密文串. `aad` 通常是 provider id, 绑定密文归属.
///
/// 空 plaintext 原样返回空串(不加密空值, 与 DB `DEFAULT ''` 语义一致).
pub fn encrypt(master_key: &[u8; 32], plaintext: &str, aad: &str) -> anyhow::Result<String> {
    if plaintext.is_empty() {
        return Ok(String::new());
    }
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(master_key));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ct = cipher
        .encrypt(&nonce, Payload { msg: plaintext.as_bytes(), aad: aad.as_bytes() })
        .map_err(|e| anyhow::anyhow!("aes-gcm encrypt: {e}"))?;
    let mut blob = Vec::with_capacity(1 + NONCE_LEN + ct.len());
    blob.push(VERSION);
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&ct);
    Ok(B64.encode(&blob))
}

/// 解密 [`encrypt`] 的输出. 空串原样返回空.
///
/// AAD 不匹配 / 密文被篡改 / 版本未知 → `Err`(调用方据此降级友好提示,
/// 不 panic).
pub fn decrypt(master_key: &[u8; 32], stored: &str, aad: &str) -> anyhow::Result<String> {
    if stored.is_empty() {
        return Ok(String::new());
    }
    let blob = B64
        .decode(stored)
        .map_err(|e| anyhow::anyhow!("base64 decode: {e}"))?;
    if blob.len() < MIN_BLOB_LEN {
        anyhow::bail!("ciphertext blob too short ({} bytes)", blob.len());
    }
    let (&version, rest) = blob.split_first().ok_or_else(|| anyhow::anyhow!("empty blob"))?;
    if version != VERSION {
        anyhow::bail!(
            "unknown ciphertext version 0x{:02x} (expected 0x{:02x})",
            version,
            VERSION
        );
    }
    let (nonce_bytes, ct) = rest.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(master_key));
    let pt = cipher
        .decrypt(
            Nonce::from_slice(nonce_bytes),
            Payload { msg: ct, aad: aad.as_bytes() },
        )
        .map_err(|e| anyhow::anyhow!("aes-gcm decrypt (aad mismatch or tamper): {e}"))?;
    String::from_utf8(pt).map_err(|e| anyhow::anyhow!("plaintext utf8: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试环境(WSL/CI)须有 machine-id; 本项目实测 `/etc/machine-id` 存在.
    fn mk() -> [u8; 32] {
        derive_master_key().expect("machine-id available in test env")
    }

    #[test]
    fn roundtrip_with_aad() {
        let k = mk();
        let ct = encrypt(&k, "sk-ant-test-12345", "provider-abc").unwrap();
        assert_ne!(ct, "sk-ant-test-12345", "ciphertext must differ from plaintext");
        assert_eq!(decrypt(&k, &ct, "provider-abc").unwrap(), "sk-ant-test-12345");
    }

    #[test]
    fn empty_plaintext_stays_empty() {
        let k = mk();
        assert_eq!(encrypt(&k, "", "p").unwrap(), "");
        assert_eq!(decrypt(&k, "", "p").unwrap(), "");
    }

    #[test]
    fn tamper_ciphertext_fails() {
        let k = mk();
        let ct = encrypt(&k, "secret", "p").unwrap();
        let mut bytes = B64.decode(&ct).unwrap();
        // 翻转 nonce 之后第一个 ciphertext 字节 → Poly1305 tag 校验失败
        bytes[1 + NONCE_LEN] ^= 0xff;
        let tampered = B64.encode(&bytes);
        assert!(
            decrypt(&k, &tampered, "p").is_err(),
            "tampered ciphertext must fail authentication"
        );
    }

    #[test]
    fn aad_mismatch_fails() {
        let k = mk();
        let ct = encrypt(&k, "secret", "provider-a").unwrap();
        // 用 provider-b 的 aad 解密 → 必须失败(密文已绑定 provider-a)
        assert!(
            decrypt(&k, &ct, "provider-b").is_err(),
            "decrypt with wrong aad must fail (provider binding)"
        );
        // 正确 aad 仍可解
        assert_eq!(decrypt(&k, &ct, "provider-a").unwrap(), "secret");
    }

    #[test]
    fn unknown_version_rejected() {
        let k = mk();
        let ct = encrypt(&k, "secret", "p").unwrap();
        let mut bytes = B64.decode(&ct).unwrap();
        bytes[0] = 0x99; // 篡改版本字节
        let bad = B64.encode(&bytes);
        assert!(decrypt(&k, &bad, "p").is_err(), "unknown version must be rejected");
    }

    #[test]
    fn distinct_nonces_per_encrypt() {
        // 同明文两次加密 → 密文不同(nonce 随机), 都可解
        let k = mk();
        let a = encrypt(&k, "same", "p").unwrap();
        let b = encrypt(&k, "same", "p").unwrap();
        assert_ne!(a, b, "random nonce must yield distinct ciphertext");
        assert_eq!(decrypt(&k, &a, "p").unwrap(), "same");
        assert_eq!(decrypt(&k, &b, "p").unwrap(), "same");
    }
}
