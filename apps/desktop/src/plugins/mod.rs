//! Built-in reader plugins. Plugins consume publication semantics and return
//! stable source-backed results; none of them depend on Xilem or the renderer.

mod ai;
mod commands;
mod rewrite;
mod search;

use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

pub use ai::{ChatResponse, ChatRole, ChatTurn, chat_with_book, translate_text};
pub use commands::{
    ChatCommand, ChatCommandResolution, chat_command_suggestions, resolve_chat_command,
};
pub use rewrite::RewriteBookSource;
pub use search::{BookSearchResult, search_book};

const SETTINGS_FILE: &str = "plugins.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginManifest {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
}

pub const BUILTIN_PLUGINS: [PluginManifest; 3] = [
    PluginManifest {
        id: "rebook.search",
        name: "全文搜索",
        description: "按语义正文搜索并跳转到原文",
    },
    PluginManifest {
        id: "rebook.ai-chat",
        name: "AI 对话",
        description: "围绕当前书籍检索、解释和问答",
    },
    PluginManifest {
        id: "rebook.translation",
        name: "翻译",
        description: "翻译选中文字并保留原文锚点",
    },
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginSettings {
    pub base_url: String,
    pub chat_model: String,
    pub translation_model: String,
    pub target_language: String,
    /// Secrets are deliberately session-only. The initial value can be supplied
    /// through `REBOOK_AI_API_KEY`, and edits are never serialized to disk.
    #[serde(skip)]
    pub api_key: String,
}

impl Default for PluginSettings {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".into(),
            chat_model: "gpt-4o-mini".into(),
            translation_model: "gpt-4o-mini".into(),
            target_language: "简体中文".into(),
            api_key: String::new(),
        }
    }
}

impl PluginSettings {
    pub fn load_default() -> io::Result<Self> {
        let path = settings_path()?;
        let mut settings = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(error) => return Err(error),
        };
        if let Ok(value) = env::var("REBOOK_AI_BASE_URL")
            && !value.trim().is_empty()
        {
            settings.base_url = value;
        }
        if let Ok(value) = env::var("REBOOK_AI_MODEL")
            && !value.trim().is_empty()
        {
            settings.chat_model.clone_from(&value);
            settings.translation_model = value;
        }
        if let Ok(value) = env::var("REBOOK_AI_API_KEY") {
            settings.api_key = value;
        }
        Ok(settings)
    }

    pub fn save_default(&self) -> io::Result<()> {
        let path = settings_path()?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "插件设置路径没有父目录"))?;
        fs::create_dir_all(parent)?;
        let temporary = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        fs::write(&temporary, bytes)?;
        if path.exists() {
            fs::remove_file(&path)?;
        }
        fs::rename(temporary, path)
    }

    pub fn validate_ai(&self) -> Result<(), String> {
        if self.api_key.trim().is_empty() {
            return Err("请先在“设置 → 插件”中填写 API Key".into());
        }
        if self.base_url.trim().is_empty() {
            return Err("API 地址不能为空".into());
        }
        if !self.base_url.starts_with("https://") && !self.base_url.starts_with("http://") {
            return Err("API 地址必须使用 http:// 或 https://".into());
        }
        Ok(())
    }
}

fn settings_path() -> io::Result<PathBuf> {
    let project = ProjectDirs::from("com", "Rebook", "Rebook")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "无法确定插件配置目录"))?;
    Ok(project.config_dir().join(SETTINGS_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialized_plugin_settings_never_contain_the_api_key() {
        let settings = PluginSettings {
            api_key: "top-secret".into(),
            ..PluginSettings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();

        assert!(!json.contains("top-secret"));
        assert!(!json.contains("api_key"));
    }

    #[test]
    fn builtin_plugin_ids_are_stable_and_unique() {
        let ids = BUILTIN_PLUGINS
            .iter()
            .map(|plugin| plugin.id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), BUILTIN_PLUGINS.len());
        assert!(ids.contains("rebook.search"));
        assert!(ids.contains("rebook.ai-chat"));
        assert!(ids.contains("rebook.translation"));
    }
}
