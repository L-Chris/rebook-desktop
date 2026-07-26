//! Built-in reader plugins. Plugins consume publication semantics and return
//! stable source-backed results; none of them depend on Xilem or the renderer.

mod ai;
mod commands;
mod rewrite;
mod search;
mod translation;

use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::persistence::write_json_atomic;

pub use ai::{ChatResponse, ChatRole, ChatTurn, chat_with_book, translate_blocks};
pub use commands::{
    ChatCommand, ChatCommandResolution, chat_command_suggestions, resolve_chat_command,
};
pub use rewrite::RewriteBookSource;
pub use search::{BookSearchResult, search_book};
pub use translation::{BlockTranslation, TranslationBlockInput, TranslationBookSource};

const SETTINGS_FILE: &str = "plugins.json";
const DEFAULT_PROVIDER_ID: &str = "openai";
const DEFAULT_MODEL: &str = "gpt-4o-mini";

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
pub struct AiProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub models: Vec<String>,
    /// Secrets are deliberately session-only. The initial value can be supplied
    /// through `REBOOK_AI_API_KEY`, and edits are never serialized to disk.
    #[serde(skip)]
    pub api_key: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranslationMode {
    Replace,
    #[default]
    Bilingual,
}

impl Default for AiProvider {
    fn default() -> Self {
        Self {
            id: DEFAULT_PROVIDER_ID.into(),
            name: "OpenAI".into(),
            base_url: "https://api.openai.com/v1".into(),
            models: vec![DEFAULT_MODEL.into()],
            api_key: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginSettings {
    pub providers: Vec<AiProvider>,
    pub chat_provider: String,
    pub chat_model: String,
    pub translation_provider: String,
    pub translation_model: String,
    pub target_language: String,
    pub translation_mode: TranslationMode,
    #[serde(default, rename = "base_url", skip_serializing)]
    legacy_base_url: Option<String>,
    #[serde(default, rename = "api_key", skip_serializing)]
    legacy_api_key: Option<String>,
}

impl Default for PluginSettings {
    fn default() -> Self {
        Self {
            providers: vec![AiProvider::default()],
            chat_provider: DEFAULT_PROVIDER_ID.into(),
            chat_model: DEFAULT_MODEL.into(),
            translation_provider: DEFAULT_PROVIDER_ID.into(),
            translation_model: DEFAULT_MODEL.into(),
            target_language: "简体中文".into(),
            translation_mode: TranslationMode::Bilingual,
            legacy_base_url: None,
            legacy_api_key: None,
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
        settings.migrate_legacy();
        if let Ok(value) = env::var("REBOOK_AI_BASE_URL")
            && !value.trim().is_empty()
            && let Some(provider) = settings.providers.first_mut()
        {
            provider.base_url = value;
        }
        if let Ok(value) = env::var("REBOOK_AI_MODEL")
            && !value.trim().is_empty()
        {
            if let Some(provider) = settings.providers.first_mut()
                && !provider.models.iter().any(|model| model == &value)
            {
                provider.models.push(value.clone());
            }
            if let Some(provider) = settings.providers.first() {
                settings.chat_provider.clone_from(&provider.id);
                settings.translation_provider.clone_from(&provider.id);
            }
            settings.chat_model.clone_from(&value);
            settings.translation_model = value;
        }
        if let Ok(value) = env::var("REBOOK_AI_API_KEY")
            && let Some(provider) = settings.providers.first_mut()
        {
            provider.api_key = value;
        }
        settings.normalize();
        Ok(settings)
    }

    pub fn save_default(&self) -> io::Result<()> {
        let mut settings = self.clone();
        settings.normalize();
        let path = settings_path()?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "插件设置路径没有父目录"))?;
        fs::create_dir_all(parent)?;
        write_json_atomic(&path, &settings)
    }

    pub fn normalize(&mut self) {
        self.migrate_legacy();
        if self.providers.is_empty() {
            self.providers.push(AiProvider::default());
        }

        let mut ids = std::collections::HashSet::new();
        for (index, provider) in self.providers.iter_mut().enumerate() {
            let fallback_id = format!("provider-{}", index + 1);
            if provider.id.trim().is_empty() || !ids.insert(provider.id.clone()) {
                provider.id = fallback_id;
                while !ids.insert(provider.id.clone()) {
                    provider.id.push('-');
                }
            }
            if provider.name.trim().is_empty() {
                provider.name = format!("Provider {}", index + 1);
            }
            provider.models = normalized_models(std::mem::take(&mut provider.models));
        }

        normalize_selection(
            &self.providers,
            &mut self.chat_provider,
            &mut self.chat_model,
        );
        normalize_selection(
            &self.providers,
            &mut self.translation_provider,
            &mut self.translation_model,
        );
        if self.target_language.trim().is_empty() {
            self.target_language = "简体中文".into();
        }
    }

    pub fn add_provider(&mut self) {
        let mut suffix = self.providers.len() + 1;
        let id = loop {
            let candidate = format!("provider-{suffix}");
            if self
                .providers
                .iter()
                .all(|provider| provider.id != candidate)
            {
                break candidate;
            }
            suffix += 1;
        };
        self.providers.push(AiProvider {
            id,
            name: format!("Provider {suffix}"),
            base_url: String::new(),
            models: vec![DEFAULT_MODEL.into()],
            api_key: String::new(),
        });
    }

    pub fn remove_provider(&mut self, index: usize) {
        if self.providers.len() <= 1 || index >= self.providers.len() {
            return;
        }
        self.providers.remove(index);
        normalize_selection(
            &self.providers,
            &mut self.chat_provider,
            &mut self.chat_model,
        );
        normalize_selection(
            &self.providers,
            &mut self.translation_provider,
            &mut self.translation_model,
        );
    }

    pub fn remove_model(&mut self, provider_index: usize, model_index: usize) {
        let Some(provider) = self.providers.get_mut(provider_index) else {
            return;
        };
        if provider.models.len() <= 1 || model_index >= provider.models.len() {
            return;
        }
        provider.models.remove(model_index);
        normalize_selection(
            &self.providers,
            &mut self.chat_provider,
            &mut self.chat_model,
        );
        normalize_selection(
            &self.providers,
            &mut self.translation_provider,
            &mut self.translation_model,
        );
    }

    pub fn chat_endpoint(&self) -> Result<(&AiProvider, &str), String> {
        self.endpoint(&self.chat_provider, &self.chat_model, "AI Chat")
    }

    pub fn translation_endpoint(&self) -> Result<(&AiProvider, &str), String> {
        self.endpoint(&self.translation_provider, &self.translation_model, "翻译")
    }

    fn endpoint<'a>(
        &'a self,
        provider_id: &str,
        model: &'a str,
        feature: &str,
    ) -> Result<(&'a AiProvider, &'a str), String> {
        let provider = self
            .providers
            .iter()
            .find(|provider| provider.id == provider_id)
            .ok_or_else(|| format!("请先在“设置 → {feature}”中选择 Provider"))?;
        if provider.api_key.trim().is_empty() {
            return Err(format!(
                "请先在“设置 → AI”中填写 {} 的 API Key",
                provider.name
            ));
        }
        let base_url = provider.base_url.trim();
        if base_url.is_empty() {
            return Err(format!("{} 的 API 地址不能为空", provider.name));
        }
        if !base_url.starts_with("https://") && !base_url.starts_with("http://") {
            return Err(format!(
                "{} 的 API 地址必须使用 http:// 或 https://",
                provider.name
            ));
        }
        let model = model.trim();
        if model.is_empty() || !provider.models.iter().any(|candidate| candidate == model) {
            return Err(format!(
                "请先在“设置 → {feature}”中选择 {} 下的模型",
                provider.name
            ));
        }
        Ok((provider, model))
    }

    fn migrate_legacy(&mut self) {
        let Some(base_url) = self.legacy_base_url.take() else {
            return;
        };
        if self.providers.is_empty() {
            self.providers.push(AiProvider::default());
        }
        if let Some(provider) = self.providers.first_mut() {
            if !base_url.trim().is_empty() {
                provider.base_url = base_url;
            }
            if let Some(api_key) = self.legacy_api_key.take() {
                provider.api_key = api_key;
            }
            for model in [&self.chat_model, &self.translation_model] {
                if !model.trim().is_empty() && !provider.models.contains(model) {
                    provider.models.push(model.clone());
                }
            }
            self.chat_provider.clone_from(&provider.id);
            self.translation_provider.clone_from(&provider.id);
        }
    }
}

fn normalized_models(models: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut models = models
        .into_iter()
        .map(|model| model.trim().to_owned())
        .filter(|model| !model.is_empty() && seen.insert(model.clone()))
        .collect::<Vec<_>>();
    if models.is_empty() {
        models.push(DEFAULT_MODEL.into());
    }
    models
}

fn normalize_selection(providers: &[AiProvider], provider_id: &mut String, model: &mut String) {
    let provider = providers
        .iter()
        .find(|provider| provider.id == *provider_id)
        .unwrap_or(&providers[0]);
    provider_id.clone_from(&provider.id);
    if !provider.models.iter().any(|candidate| candidate == model) {
        model.clone_from(&provider.models[0]);
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
        let mut settings = PluginSettings::default();
        settings.providers[0].api_key = "top-secret".into();
        let json = serde_json::to_string(&settings).unwrap();

        assert!(!json.contains("top-secret"));
        assert!(!json.contains("api_key"));
    }

    #[test]
    fn legacy_single_provider_settings_migrate_without_losing_models() {
        let json = r#"{
            "base_url": "http://localhost:11434/v1",
            "chat_model": "qwen-chat",
            "translation_model": "qwen-translate",
            "target_language": "English"
        }"#;
        let mut settings: PluginSettings = serde_json::from_str(json).unwrap();
        settings.normalize();

        assert_eq!(settings.providers.len(), 1);
        assert_eq!(settings.providers[0].base_url, "http://localhost:11434/v1");
        assert!(settings.providers[0].models.contains(&"qwen-chat".into()));
        assert!(
            settings.providers[0]
                .models
                .contains(&"qwen-translate".into())
        );
        assert_eq!(settings.chat_model, "qwen-chat");
        assert_eq!(settings.translation_model, "qwen-translate");
        assert_eq!(settings.target_language, "English");
        assert_eq!(settings.translation_mode, TranslationMode::Bilingual);
    }

    #[test]
    fn translation_mode_round_trips_through_settings_json() {
        let settings = PluginSettings {
            translation_mode: TranslationMode::Replace,
            ..PluginSettings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        let restored: PluginSettings = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.translation_mode, TranslationMode::Replace);
    }

    #[test]
    fn removing_a_selected_provider_repairs_both_feature_selections() {
        let mut settings = PluginSettings::default();
        settings.add_provider();
        let second = settings.providers[1].id.clone();
        settings.chat_provider.clone_from(&second);
        settings.translation_provider = second;

        settings.remove_provider(1);

        assert_eq!(settings.chat_provider, DEFAULT_PROVIDER_ID);
        assert_eq!(settings.translation_provider, DEFAULT_PROVIDER_ID);
        assert_eq!(settings.chat_model, DEFAULT_MODEL);
        assert_eq!(settings.translation_model, DEFAULT_MODEL);
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
