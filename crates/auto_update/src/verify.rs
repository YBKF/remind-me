//! minisign 签名校验。
//!
//! 校验过程完全流式：安装包从磁盘按 64 KiB 分块喂给
//! [`minisign_verify::StreamVerifier`]，不会把整个安装包读进内存。
//! 对于 minisign 的 legacy（非 prehashed）签名，才会回退到一次性读取。

use std::{
    fs::File,
    io::Read,
    path::Path,
};

use minisign_verify::{PublicKey, Signature};

use crate::AutoUpdateError;

/// 解析 minisign 公钥。
///
/// 既支持只包含 base64 的裸公钥，也支持完整的 `minisign.pub` 文件内容
/// （第一行注释 + 第二行公钥）。
pub fn parse_public_key(raw: &str) -> Result<PublicKey, AutoUpdateError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AutoUpdateError::PublicKey(
            minisign_verify::Error::InvalidEncoding,
        ));
    }

    if trimmed.lines().count() > 1 {
        PublicKey::decode(trimmed).map_err(AutoUpdateError::PublicKey)
    } else {
        PublicKey::from_base64(trimmed).map_err(AutoUpdateError::PublicKey)
    }
}

/// 校验 `path` 上的文件是否与 `signature`、`public_key` 匹配。
pub fn verify_file(
    path: &Path,
    public_key: &str,
    signature: &str,
) -> Result<(), AutoUpdateError> {
    let public_key = parse_public_key(public_key)?;
    let signature = Signature::decode(signature)?;

    match public_key.verify_stream(&signature) {
        Ok(mut verifier) => {
            let mut file = File::open(path)?;
            let mut buf = [0u8; 64 * 1024];
            loop {
                let read = file.read(&mut buf)?;
                if read == 0 {
                    break;
                }
                verifier.update(&buf[..read]);
            }
            verifier.finalize()?;
            Ok(())
        }
        // 旧版 minisign 生成的签名不支持流式校验，退回一次性读取。
        Err(minisign_verify::Error::UnsupportedLegacyMode) => {
            let data = std::fs::read(path)?;
            public_key
                .verify(&data, &signature, true)
                .map_err(AutoUpdateError::Signature)
        }
        Err(error) => Err(AutoUpdateError::Signature(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_public_key() {
        assert!(matches!(
            parse_public_key("   "),
            Err(AutoUpdateError::PublicKey(_))
        ));
    }

    #[test]
    fn accepts_bare_base64_key() {
        // 42 字节：算法标记 `Ed` + 8 字节 key id + 32 字节密钥。
        let key = format!("RWQA{}", "A".repeat(52));
        assert!(parse_public_key(&key).is_ok());
    }

    #[test]
    fn accepts_key_with_comment_line() {
        let key = format!("untrusted comment: minisign public key\nRWQA{}", "A".repeat(52));
        assert!(parse_public_key(&key).is_ok());
    }
}
