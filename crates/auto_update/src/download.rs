//! 安装包下载（带进度）与下载后处理。
//!
//! 下载先写入 `<file>.part`，校验通过后才重命名为最终文件，因此失败或取消
//! 都不会留下看起来可用的半成品。

use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use semver::Version;

use crate::{error::AutoUpdateError, verify};

/// 下载进度。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DownloadProgress {
    /// 已下载字节数。
    pub downloaded: u64,
    /// 总字节数；服务端未提供 `Content-Length` 时为 `None`。
    pub total: Option<u64>,
}

impl DownloadProgress {
    /// 进度比例 `0.0..=1.0`；总长度未知时为 `None`。
    pub fn fraction(&self) -> Option<f32> {
        match self.total {
            Some(total) if total > 0 => {
                Some((self.downloaded as f32 / total as f32).clamp(0.0, 1.0))
            }
            _ => None,
        }
    }
}

/// 一次下载所需的所有参数。
pub(crate) struct DownloadRequest {
    pub client: ureq::Agent,
    pub url: String,
    pub part_path: PathBuf,
    pub final_path: PathBuf,
    /// 为 `Some` 时强制校验签名。
    pub public_key: Option<String>,
    pub signature: Option<String>,
}

/// 进度上报通道；阻塞线程通过 `try_send` 推送给 GPUI 前台。
pub(crate) type ProgressSender = smol::channel::Sender<DownloadProgress>;

/// 阻塞式下载 + 校验，返回最终的安装包路径。
///
/// 只应在后台线程（`BackgroundExecutor`）上调用。
pub(crate) fn download_and_verify(
    request: &DownloadRequest,
    progress: &ProgressSender,
    cancel: &AtomicBool,
) -> Result<PathBuf, AutoUpdateError> {
    if let Some(parent) = request.part_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let response = request.client.get(&request.url).call()?;
    let status = response.status();
    if !(200..300).contains(&status) {
        return Err(AutoUpdateError::Http {
            status,
            url: request.url.clone(),
        });
    }
    let total = response
        .header("content-length")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|total| *total > 0);

    if let Err(error) = write_stream(response, &request.part_path, total, progress, cancel) {
        let _ = std::fs::remove_file(&request.part_path);
        return Err(error);
    }

    if let Some(public_key) = request.public_key.as_deref() {
        let Some(signature) = request.signature.as_deref() else {
            let _ = std::fs::remove_file(&request.part_path);
            return Err(AutoUpdateError::MissingSignature(request.url.clone()));
        };
        if let Err(error) = verify::verify_file(&request.part_path, public_key, signature) {
            let _ = std::fs::remove_file(&request.part_path);
            return Err(error);
        }
    }

    // 删除旧文件
    if request.final_path.exists() {
        let _ = std::fs::remove_file(&request.final_path);
    }
    std::fs::rename(&request.part_path, &request.final_path)?;

    Ok(request.final_path.clone())
}

fn write_stream(
    response: ureq::Response,
    part_path: &Path,
    total: Option<u64>,
    progress: &ProgressSender,
    cancel: &AtomicBool,
) -> Result<(), AutoUpdateError> {
    let mut file = File::create(part_path)?;
    let mut reader = response.into_reader();

    let mut buf = [0u8; 64 * 1024];
    let mut downloaded = 0u64;
    let mut last_report = Instant::now();
    let mut last_percent: Option<u8> = None;

    let _ = progress.try_send(DownloadProgress {
        downloaded: 0,
        total,
    });

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(AutoUpdateError::Cancelled);
        }

        let read = reader.read(&mut buf)?;
        if read == 0 {
            break;
        }
        file.write_all(&buf[..read])?;
        downloaded += read as u64;

        let percent =
            total.map(|total| ((downloaded as f32 / total as f32) * 100.0).clamp(0.0, 100.0) as u8);
        // 1% 变化或每 100ms 上报一次，避免刷爆前台事件队列。
        let should_report =
            percent != last_percent || last_report.elapsed() >= Duration::from_millis(100);
        if should_report {
            last_percent = percent;
            last_report = Instant::now();
            let _ = progress.try_send(DownloadProgress { downloaded, total });
        }
    }

    file.flush()?;
    let _ = progress.try_send(DownloadProgress { downloaded, total });

    Ok(())
}

/// 从下载地址推断文件名；推断失败时退回 `update-<version><ext>`。
pub(crate) fn file_name_for(url: &str, version: &Version) -> String {
    extract_file_name(url).unwrap_or_else(|| format!("update-{version}{}", default_extension()))
}

/// 取 URL 最后一段作为文件名，要求带扩展名且不含路径穿越字符。
pub(crate) fn extract_file_name(url: &str) -> Option<String> {
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

    /// 起一个只回一次响应的本地 HTTP 服务，返回地址。
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

    fn test_request(url: String, dir: &Path) -> DownloadRequest {
        DownloadRequest {
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

        let request = test_request(url, &dir);
        let (sender, receiver) = smol::channel::unbounded::<DownloadProgress>();
        let path = download_and_verify(&request, &sender, &AtomicBool::new(false)).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), BODY);
        assert!(!request.part_path.exists());
        assert!(
            receiver.try_recv().is_ok(),
            "expected at least one progress event"
        );

        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_removes_partial_file() {
        let (url, server) = serve_once(b"payload");
        let dir = std::env::temp_dir().join(format!("auto-update-cancel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let request = test_request(url, &dir);
        let (sender, _receiver) = smol::channel::unbounded::<DownloadProgress>();
        let result = download_and_verify(&request, &sender, &AtomicBool::new(true));

        assert!(matches!(result, Err(AutoUpdateError::Cancelled)));
        assert!(!request.part_path.exists());
        assert!(!request.final_path.exists());

        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
