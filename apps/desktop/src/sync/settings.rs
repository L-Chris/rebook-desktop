use std::fs;
use std::io;
use std::path::PathBuf;

use directories::ProjectDirs;
use keyring::Entry;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::persistence::write_json_atomic;

use super::SyncResult;

const SETTINGS_VERSION: u32 = 1;
const SETTINGS_FILE: &str = "webdav-sync.json";
const CREDENTIAL_SERVICE: &str = "Rebook WebDAV";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SyncSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub username: String,
    pub device_id: String,
    pub device_name: String,
    #[serde(default = "default_interval_minutes")]
    pub interval_minutes: u32,
}

#[derive(Serialize, Deserialize)]
struct StoredSettings {
    version: u32,
    settings: SyncSettings,
}

impl SyncSettings {
    pub(crate) fn load_default() -> SyncResult<Self> {
        let path = settings_path()?;
        let mut settings = if path.exists() {
            let stored: StoredSettings = serde_json::from_slice(&fs::read(&path)?)?;
            if stored.version != SETTINGS_VERSION {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("不支持的 WebDAV 同步设置版本：{}", stored.version),
                )
                .into());
            }
            stored.settings
        } else {
            Self::new_device()
        };
        settings.normalize();
        if !path.exists() {
            settings.save_default()?;
        }
        Ok(settings)
    }

    pub(crate) fn new_device() -> Self {
        Self {
            enabled: false,
            base_url: String::new(),
            username: String::new(),
            device_id: Uuid::new_v4().to_string(),
            device_name: std::env::var("COMPUTERNAME")
                .ok()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "Rebook Desktop".into()),
            interval_minutes: default_interval_minutes(),
        }
    }

    pub(crate) fn normalize(&mut self) {
        self.base_url = self.base_url.trim().trim_end_matches('/').to_owned();
        self.username = self.username.trim().to_owned();
        self.device_name = self.device_name.trim().to_owned();
        if self.device_name.is_empty() {
            self.device_name = "Rebook Desktop".into();
        }
        self.interval_minutes = self.interval_minutes.clamp(1, 1_440);
    }

    pub(crate) fn validate(&self) -> SyncResult<()> {
        if self.base_url.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "请输入 WebDAV 地址").into());
        }
        if self.username.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "请输入 WebDAV 用户名").into());
        }
        if Uuid::parse_str(&self.device_id).is_err() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "同步设备标识无效").into());
        }
        let url = Url::parse(&self.base_url)?;
        let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if url.scheme() != "https" && !local {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "WebDAV 同步默认只允许 HTTPS 地址",
            )
            .into());
        }
        Ok(())
    }

    pub(crate) fn save_default(&self) -> SyncResult<()> {
        let path = settings_path()?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "同步设置路径没有父目录"))?;
        fs::create_dir_all(parent)?;
        let stored = StoredSettings {
            version: SETTINGS_VERSION,
            settings: self.clone(),
        };
        write_json_atomic(&path, &stored)?;
        Ok(())
    }

    pub(crate) fn load_password(&self) -> SyncResult<String> {
        match credential_entry(&self.device_id)?.get_password() {
            Ok(password) => Ok(password),
            Err(keyring::Error::NoEntry) => Ok(String::new()),
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn save_password(&self, password: &str) -> SyncResult<()> {
        let entry = credential_entry(&self.device_id)?;
        if password.is_empty() {
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(error) => Err(error.into()),
            }
        } else {
            entry.set_password(password)?;
            Ok(())
        }
    }
}

fn default_interval_minutes() -> u32 {
    5
}

fn credential_entry(device_id: &str) -> SyncResult<Entry> {
    Ok(Entry::new(CREDENTIAL_SERVICE, device_id)?)
}

fn settings_path() -> io::Result<PathBuf> {
    let project = ProjectDirs::from("com", "Rebook", "Rebook")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "无法确定 WebDAV 同步设置目录"))?;
    Ok(project.config_dir().join(SETTINGS_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_keeps_device_identity_and_cleans_endpoint() {
        let mut settings = SyncSettings::new_device();
        settings.base_url = " https://dav.example.test/books/// ".into();
        settings.username = " chris ".into();
        settings.device_name = " ".into();
        settings.interval_minutes = 0;
        let device_id = settings.device_id.clone();

        settings.normalize();

        assert_eq!(settings.base_url, "https://dav.example.test/books");
        assert_eq!(settings.username, "chris");
        assert_eq!(settings.device_name, "Rebook Desktop");
        assert_eq!(settings.interval_minutes, 1);
        assert_eq!(settings.device_id, device_id);
        settings.validate().unwrap();
    }

    #[test]
    fn rejects_plain_http_for_remote_hosts() {
        let mut settings = SyncSettings::new_device();
        settings.base_url = "http://dav.example.test".into();
        settings.username = "reader".into();
        assert!(settings.validate().is_err());

        settings.base_url = "http://127.0.0.1:9080".into();
        assert!(settings.validate().is_ok());
    }
}
