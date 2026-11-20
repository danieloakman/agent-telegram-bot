use crate::settings::{CommandSettings, Settings};

/// Split `/name@bot args` → `(name, args)`.
pub fn split_slash(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    let rest = text.strip_prefix('/')?;
    if rest.is_empty() {
        return None;
    }
    let (head, args) = match rest.split_once(char::is_whitespace) {
        Some((head, args)) => (head, args.trim()),
        None => (rest, ""),
    };
    let name = head.split('@').next().unwrap_or(head).to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }
    Some((name, args.to_string()))
}

/// Whitespace-split args for argv (simple; no shell quoting).
pub fn split_args(args: &str) -> Vec<String> {
    args.split_whitespace().map(str::to_string).collect()
}

pub fn resolve_command<'a>(
    settings: &'a Settings,
    name: &str,
) -> Option<(&'a str, &'a CommandSettings)> {
    let needle = name.to_ascii_lowercase();
    if let Some((primary, command)) = settings.commands.get_key_value(&needle) {
        return Some((primary.as_str(), command));
    }
    for (primary, command) in &settings.commands {
        if command
            .aliases
            .iter()
            .any(|alias| alias.eq_ignore_ascii_case(&needle))
        {
            return Some((primary.as_str(), command));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Features, RunnableCommand, Settings};
    use serde_json::Value;
    use std::collections::BTreeMap;

    #[test]
    fn split_slash_strips_bot_suffix_and_args() {
        assert_eq!(
            split_slash("/expense@BrownFamBot 15 coffee"),
            Some(("expense".into(), "15 coffee".into()))
        );
        assert_eq!(split_slash("/help"), Some(("help".into(), "".into())));
        assert_eq!(split_slash("not a slash"), None);
    }

    #[test]
    fn split_args_on_whitespace() {
        assert_eq!(split_args("15 coffee"), vec!["15", "coffee"]);
        assert_eq!(split_args("  "), Vec::<String>::new());
    }

    #[test]
    fn resolve_command_matches_alias() {
        let mut commands = BTreeMap::new();
        commands.insert(
            "cal".into(),
            CommandSettings {
                description: "calendar".into(),
                aliases: vec!["calendar".into()],
                run: RunnableCommand {
                    command: "/bin/true".into(),
                    timeout_sec: 60,
                    env: BTreeMap::new(),
                },
            },
        );
        let settings = Settings {
            instance: "bot".into(),
            chat_ids: vec![1],
            allowed_user_ids: vec![],
            allowed_chat_ids: vec![],
            guest_mode: false,
            timezone: "UTC".into(),
            context: Value::Null,
            features: Features::default(),
            default_command: None,
            commands,
        };
        assert_eq!(
            resolve_command(&settings, "calendar").map(|(n, _)| n),
            Some("cal")
        );
        assert_eq!(
            resolve_command(&settings, "cal").map(|(n, _)| n),
            Some("cal")
        );
    }
}
