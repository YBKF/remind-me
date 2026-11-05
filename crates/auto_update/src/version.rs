use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct VersionJson {
    pub version: String,
    pub notes: String,
    pub pub_date: String,
    pub platforms: Platforms,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Platforms {
    #[serde(rename = "windows-x86_64")]
    pub windows_x86_64: DownloadFile,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DownloadFile {
    pub signature: String,
    pub url: String,
}

impl VersionJson {
    pub fn get_semver(&self) -> Result<semver::Version> {
        let version = self.version.strip_prefix('v').unwrap_or(&self.version);
        let version = semver::Version::parse(version)?;
        Ok(version)
    }

    /// Returns the download URL for the current platform's installer.
    pub fn download_url(&self) -> &str {
        &self.platforms.windows_x86_64.url
    }

    /// Returns the minisign signature for the current platform's installer.
    pub fn signature(&self) -> &str {
        &self.platforms.windows_x86_64.signature
    }
}
