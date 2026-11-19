use std::fs;
use std::path::Path;

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Destination chats for send/ask (usually a single private chat with the bot).
    pub chat_ids: Vec<i64>,
    /// Users allowed to answer asks. Empty allows every user in those chats.
    #[serde(default)]
    pub allowed_user_ids: Vec<i64>,
}

impl Settings {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|err| format!("reading settings: {err}"))?;
        let settings: Self =
            serde_json::from_str(&text).map_err(|err| format!("parsing settings: {err}"))?;
        if settings.chat_ids.is_empty() {
            return Err("settings.chatIds must be non-empty".into());
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
        let path = std::env::temp_dir().join(format!("agent-telegram-bot-settings-{nanos}.json"));
        fs::write(&path, contents).expect("write temp settings");
        path
    }

    #[test]
    fn load_parses_camel_case() {
        let path = temp_settings(r#"{"chatIds":[1,2],"allowedUserIds":[9]}"#);
        let settings = Settings::load(&path).unwrap();
        let _ = fs::remove_file(&path);
        assert_eq!(settings.chat_ids, vec![1, 2]);
        assert_eq!(settings.allowed_user_ids, vec![9]);
    }

    #[test]
    fn load_rejects_empty_chat_ids() {
        let path = temp_settings(r#"{"chatIds":[]}"#);
        let err = Settings::load(&path).unwrap_err();
        let _ = fs::remove_file(&path);
        assert!(err.contains("non-empty"));
    }

    #[test]
    fn allows_user_empty_list_allows_all() {
        let settings = Settings {
            chat_ids: vec![1],
            allowed_user_ids: vec![],
        };
        assert!(settings.allows_user(Some(42)));
        assert!(settings.allows_user(None));
    }

    #[test]
    fn allows_user_filters_when_configured() {
        let settings = Settings {
            chat_ids: vec![1],
            allowed_user_ids: vec![7, 8],
        };
        assert!(settings.allows_user(Some(7)));
        assert!(!settings.allows_user(Some(9)));
        assert!(!settings.allows_user(None));
    }
}
