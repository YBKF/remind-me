//! 安装包下载与签名校验
//!
//! 下载先写入 `<文件名>.part`，校验通过后重命名为最终文件，失败或取消都不会
//! 留下可用的半成品

use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use minisign_verify::{PublicKey, Signature};
use semver::Version;

use crate::AutoUpdateError;

/// 下载进度
///
/// `DownloadProgress::default()` 的 `downloaded` 为 0，`total` 为 `None`
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DownloadProgress {
    /// 已下载字节数
    pub downloaded: u64,
    /// 总字节数，服务端未提供 `Content-Length` 时为 `None`
    pub total: Option<u64>,
}

impl DownloadProgress {
    /// 进度比例 `0.0..=1.0`，总长度未知时返回 `None`
    pub fn fraction(&self) -> Option<f32> {
        match self.total {
            Some(total) if total > 0 => {
                Some((self.downloaded as f32 / total as f32).clamp(0.0, 1.0))
            }
            _ => None,
        }
    }
}

/// 一次下载所需的全部输入
pub(crate) struct DownloadJob {
    pub client: ureq::Agent,
    pub url: String,
    pub part_path: PathBuf,
    pub final_path: PathBuf,
    /// 为 `Some` 时强制校验签名
    pub public_key: Option<String>,
    pub signature: Option<String>,
}

/// 进度通道，阻塞线程通过 `try_send` 把进度推给前台
pub(crate) type ProgressSender = smol::channel::Sender<DownloadProgress>;

/// 下载 `download_job.url` 到 `download_job.part_path`，校验签名后重命名为
/// `download_job.final_path`
///
/// 进度写入 `progress_sender`；`cancel` 置位后返回
/// [`AutoUpdateError::Cancelled`] 并删除半成品
///
/// 返回最终的安装包路径，内部使用阻塞 IO，只应在后台线程调用
pub(crate) fn download_and_verify(
    download_job: &DownloadJob,
    progress_sender: &ProgressSender,
    cancel: &AtomicBool,
) -> Result<PathBuf, AutoUpdateError> {
    if let Some(dir) = download_job.part_path.parent() {
        std::fs::create_dir_all(dir)?;
    }

    let response = download_job.client.get(&download_job.url).call()?;
    let status = response.status();
    if !(200..300).contains(&status) {
        return Err(AutoUpdateError::Http {
            status,
            url: download_job.url.clone(),
        });
    }

    let total = response
        .header("content-length")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|total| *total > 0);

    let written = write_body(
        response,
        &download_job.part_path,
        total,
        progress_sender,
        cancel,
    );
    if let Err(error) = written {
        let _ = std::fs::remove_file(&download_job.part_path);
        return Err(error);
    }

    if let Some(public_key) = download_job.public_key.as_deref() {
        let Some(signature) = download_job.signature.as_deref() else {
            let _ = std::fs::remove_file(&download_job.part_path);
            return Err(AutoUpdateError::MissingSignature(download_job.url.clone()));
        };
        if let Err(error) = verify_file(&download_job.part_path, public_key, signature) {
            let _ = std::fs::remove_file(&download_job.part_path);
            return Err(error);
        }
    }

    if download_job.final_path.exists() {
        let _ = std::fs::remove_file(&download_job.final_path);
    }
    std::fs::rename(&download_job.part_path, &download_job.final_path)?;

    Ok(download_job.final_path.clone())
}

/// 把 `response` 的响应体写入 `part_path`，按 1% 或 100ms 的间隔向
/// `progress_sender` 上报进度
fn write_body(
    response: ureq::Response,
    part_path: &Path,
    total: Option<u64>,
    progress_sender: &ProgressSender,
    cancel: &AtomicBool,
) -> Result<(), AutoUpdateError> {
    let mut file = File::create(part_path)?;
    let mut body = response.into_reader();
    let mut buffer = [0u8; 64 * 1024];
    let mut downloaded = 0u64;
    let mut last_report = Instant::now();
    let mut last_percent: Option<u8> = None;

    let _ = progress_sender.try_send(DownloadProgress { downloaded, total });

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(AutoUpdateError::Cancelled);
        }

        let read = body.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])?;
        downloaded += read as u64;

        let percent =
            total.map(|total| ((downloaded as f32 / total as f32) * 100.0).clamp(0.0, 100.0) as u8);
        if percent != last_percent || last_report.elapsed() >= Duration::from_millis(100) {
            last_percent = percent;
            last_report = Instant::now();
            let _ = progress_sender.try_send(DownloadProgress { downloaded, total });
        }
    }

    file.flush()?;
    let _ = progress_sender.try_send(DownloadProgress { downloaded, total });

    Ok(())
}

/// 校验 `path` 与 `signature`、`public_key` 是否匹配
///
/// 按 64 KiB 分块流式校验，不把安装包整体读入内存
fn verify_file(path: &Path, public_key: &str, signature: &str) -> Result<(), AutoUpdateError> {
    let public_key = parse_public_key(public_key)?;
    let signature = Signature::decode(signature)?;

    match public_key.verify_stream(&signature) {
        Ok(mut verifier) => {
            let mut file = File::open(path)?;
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                verifier.update(&buffer[..read]);
            }
            verifier.finalize()?;
            Ok(())
        }
        // 旧版 minisign 签名不支持流式校验，退回一次性读取
        Err(minisign_verify::Error::UnsupportedLegacyMode) => {
            let package_bytes = std::fs::read(path)?;
            public_key
                .verify(&package_bytes, &signature, true)
                .map_err(AutoUpdateError::Signature)
        }
        Err(error) => Err(AutoUpdateError::Signature(error)),
    }
}

/// 解析 minisign 公钥，支持完整 `minisign.pub` 内容或其中的 base64 行
fn parse_public_key(raw: &str) -> Result<PublicKey, AutoUpdateError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AutoUpdateError::Signature(
            minisign_verify::Error::InvalidEncoding,
        ));
    }

    if trimmed.lines().count() > 1 {
        PublicKey::decode(trimmed).map_err(AutoUpdateError::Signature)
    } else {
        PublicKey::from_base64(trimmed).map_err(AutoUpdateError::Signature)
    }
}

/// 由下载地址推断安装包文件名，无法推断时退回 `update-<version><扩展名>`
pub(crate) fn file_name_for(url: &str, version: &Version) -> String {
    extract_file_name(url).unwrap_or_else(|| format!("update-{version}{}", default_extension()))
}

/// 取 URL 最后一段作为文件名，要求包含扩展名且不含路径穿越字符
fn extract_file_name(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let segment = path.rsplit('/').next()?.trim();
    if segment.is_empty() || !segment.contains('.') {
        return None;
    }
    if segment == "." || segment == ".." || segment.contains('\\') || segment.contains(':') {
        return None;
    }
    Some(segment.to_string())
}

fn default_extension() -> &'static str {
    if cfg!(target_os = "windows") {
        ".exe"
    } else if cfg!(target_os = "macos") {
        ".dmg"
    } else {
        ".AppImage"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_file_name() {
        assert_eq!(
            extract_file_name("https://example.com/a/b/Setup.exe").as_deref(),
            Some("Setup.exe")
        );
        assert_eq!(
            extract_file_name("https://example.com/Setup.exe?token=1#frag").as_deref(),
            Some("Setup.exe")
        );
    }

    #[test]
    fn rejects_unusable_names() {
        assert!(extract_file_name("https://example.com/download").is_none());
        assert!(extract_file_name("https://example.com/").is_none());
        assert!(extract_file_name("https://example.com/..").is_none());
    }

    #[test]
    fn falls_back_to_versioned_name() {
        let name = file_name_for("https://example.com/download", &Version::new(1, 2, 3));
        assert!(name.starts_with("update-1.2.3"));
    }

    #[test]
    fn fraction_is_clamped() {
        let progress = DownloadProgress {
            downloaded: 150,
            total: Some(100),
        };
        assert_eq!(progress.fraction(), Some(1.0));
        assert_eq!(
            DownloadProgress {
                downloaded: 50,
                total: None
            }
            .fraction(),
            None
        );
    }

    #[test]
    fn rejects_empty_public_key() {
        assert!(parse_public_key("   ").is_err());
    }

    #[test]
    fn accepts_bare_base64_key() {
        // 42 字节：算法标记 Ed + 8 字节 key id + 32 字节密钥
        assert!(parse_public_key(&format!("RWQA{}", "A".repeat(52))).is_ok());
    }

    #[test]
    fn accepts_key_with_comment_line() {
        let key = format!(
            "untrusted comment: minisign public key\nRWQA{}",
            "A".repeat(52)
        );
        assert!(parse_public_key(&key).is_ok());
    }

    /// 起一个只回一次响应的本地 HTTP 服务
    fn serve_once(body: &'static [u8]) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(body);
            let _ = stream.flush();
        });
        (format!("http://{address}/Setup.exe"), handle)
    }

    fn test_job(url: String, dir: &Path) -> DownloadJob {
        DownloadJob {
            client: ureq::AgentBuilder::new().build(),
            url,
            part_path: dir.join("Setup.exe.part"),
            final_path: dir.join("Setup.exe"),
            public_key: None,
            signature: None,
        }
    }

    #[test]
    fn downloads_and_renames_part_file() {
        const BODY: &[u8] = b"hello update payload";
        let (url, server) = serve_once(BODY);
        let dir = std::env::temp_dir().join(format!("auto-update-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let download_job = test_job(url, &dir);
        let (progress_sender, progress_receiver) = smol::channel::unbounded::<DownloadProgress>();
        let path =
            download_and_verify(&download_job, &progress_sender, &AtomicBool::new(false)).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), BODY);
        assert!(!download_job.part_path.exists());
        assert!(progress_receiver.try_recv().is_ok());

        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_removes_partial_file() {
        let (url, server) = serve_once(b"payload");
        let dir = std::env::temp_dir().join(format!("auto-update-cancel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let download_job = test_job(url, &dir);
        let (progress_sender, _progress_receiver) = smol::channel::unbounded::<DownloadProgress>();
        let result = download_and_verify(&download_job, &progress_sender, &AtomicBool::new(true));

        assert!(matches!(result, Err(AutoUpdateError::Cancelled)));
        assert!(!download_job.part_path.exists());
        assert!(!download_job.final_path.exists());

        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
