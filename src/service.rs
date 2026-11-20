use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use frankenstein::types::Message;
use frankenstein::updates::{Update, UpdateContent};
use uuid::Uuid;

use crate::commands;
use crate::proc;
use crate::settings::{RunnableCommand, Settings};
use crate::state::{PendingAsk, Shared};
use crate::telegram::Telegram;

pub struct App {
    settings: Settings,
    telegram: Telegram,
}

impl App {
    pub fn new(settings: Settings, telegram: Telegram) -> Self {
        Self { settings, telegram }
    }

    pub fn run_poll(&self, state: Arc<Shared>) -> Result<(), String> {
        log::info!(
            "polling telegram updates instance={}",
            self.settings.instance
        );
        loop {
            if let Err(err) = self.poll_once(&state) {
                log::error!("poll failed: {err}");
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    }

    fn poll_once(&self, state: &Shared) -> Result<(), String> {
        let updates = self.telegram.updates(state.offset())?;
        for update in updates {
            let next = i64::from(update.update_id) + 1;
            if let Err(err) = self.handle(update, state) {
                log::error!("update failed: {err}");
            }
            state.set_offset(next)?;
        }
        Ok(())
    }

    fn handle(&self, update: Update, state: &Shared) -> Result<(), String> {
        match update.content {
            UpdateContent::Message(message) => self.handle_message(*message, state),
            _ => Ok(()),
        }
    }

    fn handle_message(&self, message: Message, state: &Shared) -> Result<(), String> {
        let chat_id = message.chat.id;
        if !self.settings.allows_chat(chat_id) && !self.settings.chat_ids.contains(&chat_id) {
            return Ok(());
        }
        let user_id = message.from.as_ref().map(|u| u.id);
        if !self.settings.allows_user(user_id) {
            log::info!("ignored message from disallowed user_id={user_id:?}");
            return Ok(());
        }
        let Some(text) = message.text.as_deref().map(str::trim) else {
            return Ok(());
        };
        if text.is_empty() {
            return Ok(());
        }

        // Prefer reply-to an ask message.
        if let Some(reply) = message.reply_to_message.as_ref() {
            if let Some(ask_id) = state.ask_id_for_message(reply.message_id) {
                let answer = strip_answer_prefix(text).to_string();
                if state.complete_ask(&ask_id, Ok(answer.clone())) {
                    let _ = self
                        .telegram
                        .send(chat_id, &format!("✅ recorded answer for `{ask_id}`"));
                }
                return Ok(());
            }
        }

        // /a <answer> or /answer <answer> against the latest open ask.
        if let Some(rest) = text
            .strip_prefix("/a ")
            .or_else(|| text.strip_prefix("/answer "))
            .or_else(|| {
                if text == "/a" || text == "/answer" {
                    Some("")
                } else {
                    None
                }
            })
        {
            let answer = rest.trim();
            if answer.is_empty() {
                let _ = self
                    .telegram
                    .send(chat_id, "Usage: /a <answer> (or reply to the ask)");
                return Ok(());
            }
            let Some(ask_id) = state.latest_ask_id() else {
                let _ = self.telegram.send(chat_id, "No open ask to answer.");
                return Ok(());
            };
            if state.complete_ask(&ask_id, Ok(answer.to_string())) {
                let _ = self
                    .telegram
                    .send(chat_id, &format!("✅ recorded answer for `{ask_id}`"));
            }
            return Ok(());
        }

        let uid = user_id.map(|id| id as i64);

        if let Some((name, args)) = commands::split_slash(text) {
            if name == "help" && self.settings.features.builtin_help {
                let _ = self.telegram.send(chat_id, &self.help_text());
                return Ok(());
            }
            if let Some((primary, command)) = commands::resolve_command(&self.settings, &name) {
                let reply = self.run_command(primary, &command.run, &args, chat_id, uid);
                let _ = self.telegram.send(chat_id, &reply);
                return Ok(());
            }
            let _ = self
                .telegram
                .send(chat_id, &format!("Unknown command /{name}. Try /help."));
            return Ok(());
        }

        // Free-form message (no slash): optional defaultCommand.
        if let Some(default) = &self.settings.default_command {
            let reply = self.run_command("default", default, text, chat_id, uid);
            let _ = self.telegram.send(chat_id, &reply);
        }

        Ok(())
    }

    fn run_command(
        &self,
        label: &str,
        command: &RunnableCommand,
        args: &str,
        chat_id: i64,
        user_id: Option<i64>,
    ) -> String {
        let argv = commands::split_args(args);
        log::info!("running {label} command={} args={argv:?}", command.command);
        let mut cmd = Command::new(&command.command);
        cmd.args(&argv);
        cmd.env("TELEGRAM_BOTS_INSTANCE", &self.settings.instance);
        cmd.env("TELEGRAM_BOTS_COMMAND", label);
        cmd.env("TELEGRAM_BOTS_CHAT_ID", chat_id.to_string());
        cmd.env("TELEGRAM_BOTS_TIMEZONE", &self.settings.timezone);
        if let Some(uid) = user_id {
            cmd.env("TELEGRAM_BOTS_USER_ID", uid.to_string());
        }
        let context = serde_json::to_string(&self.settings.context).unwrap_or_else(|_| "{}".into());
        cmd.env("TELEGRAM_BOTS_CONTEXT", context);
        for (key, value) in &command.env {
            cmd.env(key, value);
        }
        match proc::run(cmd, Duration::from_secs(command.timeout_sec)) {
            Ok(output) => {
                let text = output.stdout.trim();
                if text.is_empty() {
                    format!("✅ {label} done.")
                } else {
                    text.to_string()
                }
            }
            Err(err) => format!("❌ {label} failed: {err}"),
        }
    }

    fn help_text(&self) -> String {
        let mut lines = vec![
            format!("Telegram bot `{}`.", self.settings.instance),
            "Automation can notify and ask here.".into(),
            "Reply to an ask, or send `/a <answer>`.".into(),
        ];
        if !self.settings.commands.is_empty() {
            lines.push(String::new());
            lines.push("Commands:".into());
            for (name, command) in &self.settings.commands {
                if command.description.is_empty() {
                    lines.push(format!("/{name}"));
                } else {
                    lines.push(format!("/{name} — {}", command.description));
                }
            }
        }
        if self.settings.default_command.is_some() {
            lines.push(String::new());
            lines.push("Free-form messages are also handled.".into());
        }
        lines.join("\n")
    }

    pub fn notify(&self, role: Option<&str>, text: &str) -> Result<i32, String> {
        let body = format_message(role, text, None);
        let chat_id = self.primary_chat()?;
        self.telegram.send(chat_id, &body)
    }

    pub fn ask(
        &self,
        role: Option<&str>,
        text: &str,
        timeout: Duration,
        state: &Shared,
    ) -> Result<(String, String), String> {
        let ask_id = short_id();
        let body = format_message(role, text, Some(&ask_id));
        let chat_id = self.primary_chat()?;
        let message_id = self.telegram.send(chat_id, &body)?;
        let ask = PendingAsk {
            id: ask_id.clone(),
            role: role.unwrap_or("agent").to_string(),
            chat_id,
            message_id,
            created: now_unix_secs(),
        };
        let waiter = state.register_ask(ask);
        match Shared::wait_for_answer(&waiter, timeout) {
            Ok(answer) => Ok((ask_id, answer)),
            Err(err) => {
                state.complete_ask(&ask_id, Err(err.clone()));
                Err(err)
            }
        }
    }

    fn primary_chat(&self) -> Result<i64, String> {
        self.settings
            .chat_ids
            .first()
            .copied()
            .ok_or_else(|| "no chat ids configured".into())
    }
}

fn format_message(role: Option<&str>, text: &str, ask_id: Option<&str>) -> String {
    let role = role.unwrap_or("agent");
    match ask_id {
        Some(id) => {
            format!("[{role}] ask `{id}`\n\n{text}\n\nReply to this message, or `/a <answer>`.")
        }
        None => format!("[{role}]\n{text}"),
    }
}

fn strip_answer_prefix(text: &str) -> &str {
    text.strip_prefix("/a ")
        .or_else(|| text.strip_prefix("/answer "))
        .unwrap_or(text)
        .trim()
}

fn short_id() -> String {
    let uuid = Uuid::new_v4().to_string();
    uuid[..8].to_string()
}

fn now_unix_secs() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_message_notice() {
        assert_eq!(
            format_message(Some("vuln-monitor"), "build finished", None),
            "[vuln-monitor]\nbuild finished"
        );
    }

    #[test]
    fn format_message_ask_includes_id_and_hint() {
        let body = format_message(Some("agent"), "Ship it?", Some("abcd1234"));
        assert!(body.starts_with("[agent] ask `abcd1234`"));
        assert!(body.contains("Ship it?"));
        assert!(body.contains("/a <answer>"));
    }

    #[test]
    fn format_message_default_role() {
        assert_eq!(format_message(None, "hi", None), "[agent]\nhi");
    }

    #[test]
    fn strip_answer_prefix_variants() {
        assert_eq!(strip_answer_prefix("/a yes"), "yes");
        assert_eq!(strip_answer_prefix("/answer yes"), "yes");
        assert_eq!(strip_answer_prefix("plain"), "plain");
    }
}
