//! 版本清单（`version.json`）的数据结构与平台解析。
//!
//! 清单格式与 Zed / Tauri 的 updater 兼容：
//!
//! ```json
//! {
//!   "version": "v1.2.3",
//!   "notes": "修复了一些问题",
//!   "pub_date": "2026-11-06T17:02:20+08:00",
//!   "platforms": {
//!     "windows-x86_64": { "url": "https://.../Setup.exe", "signature": "..." }
//!   }
//! }
//! ```

use std::collections::BTreeMap;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::AutoUpdateError;

/// 远端发布的版本清单。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseManifest {
    /// 版本号，允许带 `v` 前缀。
    pub version: String,
    /// 更新说明（通常是 Markdown）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// 发布时间，原样保留，不做解析。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pub_date: Option<String>,
    /// 平台标识 -> 产物。
    #[serde(default)]
    pub platforms: BTreeMap<String, ReleaseAsset>,
}

/// 某个平台对应的更新产物。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseAsset {
    /// 安装包下载地址。
    pub url: String,
    /// minisign 签名内容（`minisign -S` 生成的文件内容）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl ReleaseManifest {
    /// 解析 [`ReleaseManifest::version`] 为 semver。
    pub fn semver(&self) -> Result<Version, AutoUpdateError> {
        parse_version(&self.version)
    }

    /// 按平台标识查找产物。
    ///
    /// 先精确匹配 `windows-x86_64` 这样的键，再退化为 `windows` / `darwin`
    /// 这类系统级键，方便清单只用系统级键发布。
    pub fn asset_for(&self, platform: &str) -> Option<&ReleaseAsset> {
        platform_candidates(platform)
            .into_iter()
            .find_map(|key| self.platforms.get(key))
    }

    /// 清单中包含的所有平台键，用于错误提示。
    pub fn available_platforms(&self) -> String {
        if self.platforms.is_empty() {
            return "none".to_string();
        }
        self.platforms
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// 解析版本号，容忍 `v` / `V` 前缀和首尾空白。
pub fn parse_version(raw: &str) -> Result<Version, AutoUpdateError> {
    let trimmed = raw.trim();
    let trimmed = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    Version::parse(trimmed).map_err(|source| AutoUpdateError::Version {
        value: raw.to_string(),
        source,
    })
}

/// 当前编译目标对应的平台标识。
///
/// 命名与 Zed / Tauri 的清单保持一致（如 `darwin-aarch64`）。
pub fn platform_key() -> &'static str {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "windows-x86_64"
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        "windows-aarch64"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "darwin-aarch64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "darwin-x86_64"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux-x86_64"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "linux-aarch64"
    } else {
        "unknown"
    }
}

fn platform_candidates(platform: &str) -> Vec<&str> {
    let mut candidates = vec![platform];
    if let Some((os, _arch)) = platform.split_once('-') {
        candidates.push(os);
        match os {
            "darwin" => candidates.push("macos"),
            "macos" => candidates.push("darwin"),
            _ => {}
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_version_with_prefix() {
        assert_eq!(parse_version("v1.2.3").unwrap(), Version::new(1, 2, 3));
        assert_eq!(parse_version(" 1.2.3 ").unwrap(), Version::new(1, 2, 3));
        assert!(parse_version("not-a-version").is_err());
    }

    #[test]
    fn falls_back_to_os_level_key() {
        let manifest: ReleaseManifest = serde_json::from_str(
            r#"{
                "version": "1.0.0",
                "platforms": { "windows": { "url": "https://example.com/a.exe" } }
            }"#,
        )
        .unwrap();
        assert!(manifest.asset_for("windows-x86_64").is_some());
        assert!(manifest.asset_for("linux-x86_64").is_none());
    }

    #[test]
    fn prefers_exact_platform_key() {
        let manifest: ReleaseManifest = serde_json::from_str(
            r#"{
                "version": "1.0.0",
                "platforms": {
                    "windows": { "url": "https://example.com/generic.exe" },
                    "windows-x86_64": { "url": "https://example.com/x64.exe" }
                }
            }"#,
        )
        .unwrap();
        assert_eq!(
            manifest.asset_for("windows-x86_64").unwrap().url,
            "https://example.com/x64.exe"
        );
    }

    #[test]
    fn missing_optional_fields_deserializes() {
        let manifest: ReleaseManifest =
            serde_json::from_str(r#"{"version":"v0.1.0","platforms":{}}"#).unwrap();
        assert!(manifest.notes.is_none());
        assert!(manifest.pub_date.is_none());
    }
}
