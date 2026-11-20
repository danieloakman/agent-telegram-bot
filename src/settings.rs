use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Instance id (attrs key under services.telegram-bots.instances).
    pub instance: String,
    /// Destination chats for notify/ask (usually a single private chat with the bot).
    pub chat_ids: Vec<i64>,
    /// Users allowed to answer asks / run commands. Empty allows every user in allowed chats.
    #[serde(default)]
    pub allowed_user_ids: Vec<i64>,
    /// Chats allowed for inbound commands/guest traffic. Empty allows all chats.
    #[serde(default)]
    pub allowed_chat_ids: Vec<i64>,
    /// Reserved: guest mention handling (answerGuestQuery).
    #[serde(default)]
    #[allow(dead_code)]
    pub guest_mode: bool,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    /// Opaque JSON available to command scripts via TELEGRAM_BOTS_CONTEXT.
    #[serde(default)]
    pub context: Value,
    #[serde(default)]
    pub features: Features,
    /// Runs for free-form messages (no slash command). Same runner shape as commands.
    #[serde(default)]
    pub default_command: Option<RunnableCommand>,
    #[serde(default)]
    pub commands: BTreeMap<String, CommandSettings>,
}

fn default_timezone() -> String {
    "UTC".into()
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Features {
    #[serde(default = "default_true")]
    pub builtin_help: bool,
}

impl Default for Features {
    fn default() -> Self {
        Self { builtin_help: true }
    }
}

fn default_true() -> bool {
    true
}

/// Shared executable + timeout + env for slash commands and defaultCommand.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnableCommand {
    /// Absolute path to the executable (e.g. lib.getExe of writeShellScriptBin).
    pub command: String,
    #[serde(default = "default_timeout")]
    pub timeout_sec: u64,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandSettings {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(flatten)]
    pub run: RunnableCommand,
}

fn default_timeout() -> u64 {
    60
}

impl Settings {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|err| format!("reading settings: {err}"))?;
        let settings: Self =
            serde_json::from_str(&text).map_err(|err| format!("parsing settings: {err}"))?;
        if settings.instance.trim().is_empty() {
            return Err("settings.instance must be non-empty".into());
        }
        if settings.chat_ids.is_empty() {
            return Err("settings.chatIds must be non-empty".into());
        }
        if let Some(default) = &settings.default_command {
            if default.command.trim().is_empty() {
                return Err("defaultCommand.command must be non-empty".into());
            }
        }
        for (name, command) in &settings.commands {
            if command.run.command.trim().is_empty() {
                return Err(format!("commands.{name}.command must be non-empty"));
            }
        }
        Ok(settings)
    }

    pub fn allows_user(&self, user_id: Option<u64>) -> bool {
        if self.allowed_user_ids.is_empty() {
            return true;
        }
        let Some(uid) = user_id else {
            return false;
        };
        self.allowed_user_ids.contains(&(uid as i64))
    }

    pub fn allows_chat(&self, chat_id: i64) -> bool {
        self.allowed_chat_ids.is_empty() || self.allowed_chat_ids.contains(&chat_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_settings(contents: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("telegram-bots-settings-{nanos}.json"));
        fs::write(&path, contents).expect("write temp settings");
        path
    }

    #[test]
    fn load_parses_camel_case() {
        let path = temp_settings(
            r#"{"instance":"agent","chatIds":[1,2],"allowedUserIds":[9],"guestMode":true}"#,
        );
        let settings = Settings::load(&path).unwrap();
        let _ = fs::remove_file(&path);
        assert_eq!(settings.instance, "agent");
        assert_eq!(settings.chat_ids, vec![1, 2]);
        assert_eq!(settings.allowed_user_ids, vec![9]);
        assert!(settings.guest_mode);
        assert!(settings.features.builtin_help);
        assert_eq!(settings.timezone, "UTC");
        assert!(settings.default_command.is_none());
    }

    #[test]
    fn load_parses_default_command() {
        let path = temp_settings(
            r#"{"instance":"fam","chatIds":[1],"defaultCommand":{"command":"/bin/echo","timeoutSec":30}}"#,
        );
        let settings = Settings::load(&path).unwrap();
        let _ = fs::remove_file(&path);
        let default = settings.default_command.unwrap();
        assert_eq!(default.command, "/bin/echo");
        assert_eq!(default.timeout_sec, 30);
    }

    #[test]
    fn load_rejects_empty_chat_ids() {
        let path = temp_settings(r#"{"instance":"agent","chatIds":[]}"#);
        let err = Settings::load(&path).unwrap_err();
        let _ = fs::remove_file(&path);
        assert!(err.contains("non-empty"));
    }

    #[test]
    fn load_rejects_empty_command_path() {
        let path = temp_settings(
            r#"{"instance":"agent","chatIds":[1],"commands":{"expense":{"command":""}}}"#,
        );
        let err = Settings::load(&path).unwrap_err();
        let _ = fs::remove_file(&path);
        assert!(err.contains("command must be non-empty"));
    }

    #[test]
    fn allows_user_empty_list_allows_all() {
        let settings = Settings {
            instance: "agent".into(),
            chat_ids: vec![1],
            allowed_user_ids: vec![],
            allowed_chat_ids: vec![],
            guest_mode: false,
            timezone: "UTC".into(),
            context: Value::Null,
            features: Features::default(),
            default_command: None,
            commands: BTreeMap::new(),
        };
        assert!(settings.allows_user(Some(42)));
        assert!(settings.allows_user(None));
        assert!(settings.allows_chat(99));
    }

    #[test]
    fn allows_user_filters_when_configured() {
        let settings = Settings {
            instance: "agent".into(),
            chat_ids: vec![1],
            allowed_user_ids: vec![7, 8],
            allowed_chat_ids: vec![1],
            guest_mode: false,
            timezone: "UTC".into(),
            context: Value::Null,
            features: Features::default(),
            default_command: None,
            commands: BTreeMap::new(),
        };
        assert!(settings.allows_user(Some(7)));
        assert!(!settings.allows_user(Some(9)));
        assert!(!settings.allows_user(None));
        assert!(settings.allows_chat(1));
        assert!(!settings.allows_chat(2));
    }
}
