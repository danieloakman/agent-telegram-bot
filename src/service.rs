use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use frankenstein::types::{CallbackQuery, Message};
use frankenstein::updates::{Update, UpdateContent};
use uuid::Uuid;

use crate::commands;
use crate::ipc::AskChoice;
use crate::proc::{self, ProcOutput};
use crate::settings::{RunnableCommand, Settings};
use crate::state::{PendingAsk, Shared};
use crate::telegram::{self, Telegram};

pub struct App {
    settings: Settings,
    telegram: Telegram,
}

/// Options for a blocking ask (free-text or inline-keyboard choices).
pub struct AskOpts<'a> {
    pub role: Option<&'a str>,
    pub text: &'a str,
    pub timeout: Duration,
    pub chat_id: Option<i64>,
    pub choices: &'a [AskChoice],
    pub guest_query_id: Option<&'a str>,
}

struct CommandCtx<'a> {
    chat_id: i64,
    user_id: Option<i64>,
    guest_query_id: Option<&'a str>,
}

impl App {
    pub fn new(settings: Settings, telegram: Telegram) -> Self {
        Self { settings, telegram }
    }

    /// Long-poll Telegram. Takes `Arc<Self>` so slash/default commands can run on a
    /// background thread while this loop keeps handling ask callbacks.
    pub fn run_poll(self: &Arc<Self>, state: Arc<Shared>) -> Result<(), String> {
        log::info!(
            "polling telegram updates instance={} guest_mode={}",
            self.settings.instance,
            self.settings.guest_mode
        );
        loop {
            if let Err(err) = self.poll_once(&state) {
                log::error!("poll failed: {err}");
                thread::sleep(Duration::from_secs(5));
            }
        }
    }

    fn poll_once(self: &Arc<Self>, state: &Arc<Shared>) -> Result<(), String> {
        let updates = self
            .telegram
            .updates(state.offset(), self.settings.guest_mode)?;
        for update in updates {
            let next = i64::from(update.update_id) + 1;
            if let Err(err) = self.handle(update, state) {
                log::error!("update failed: {err}");
            }
            state.set_offset(next)?;
        }
        Ok(())
    }

    fn handle(self: &Arc<Self>, update: Update, state: &Arc<Shared>) -> Result<(), String> {
        match update.content {
            UpdateContent::Message(message) => {
                self.handle_message(*message, state, /* guest_query_id */ None)
            }
            UpdateContent::GuestMessage(message) => {
                if !self.settings.guest_mode {
                    return Ok(());
                }
                let guest_query_id = message.guest_query_id.clone();
                if guest_query_id.is_none() {
                    log::info!("guest message without guest_query_id");
                    return Ok(());
                }
                self.handle_message(*message, state, guest_query_id)
            }
            UpdateContent::CallbackQuery(callback) => self.handle_callback(*callback, state),
            _ => Ok(()),
        }
    }

    fn handle_message(
        self: &Arc<Self>,
        message: Message,
        state: &Arc<Shared>,
        guest_query_id: Option<String>,
    ) -> Result<(), String> {
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

        // Prefer reply-to an ask message (free-text asks only).
        if let Some(reply) = message.reply_to_message.as_ref() {
            if let Some(ask_id) = state.ask_id_for_message(reply.message_id) {
                if let Some(ask) = state.pending_ask(&ask_id) {
                    if ask.choice_ids.is_empty() {
                        let answer = strip_answer_prefix(text).to_string();
                        if state.complete_ask(&ask_id, Ok(answer.clone())) {
                            let _ = self.telegram.send(
                                chat_id,
                                &format!("✅ recorded answer for `{ask_id}`"),
                                None,
                            );
                        }
                        return Ok(());
                    }
                }
            }
        }

        // /a <answer> or /answer <answer> against the latest open free-text ask.
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
                let _ =
                    self.telegram
                        .send(chat_id, "Usage: /a <answer> (or reply to the ask)", None);
                return Ok(());
            }
            let Some(ask_id) = state.latest_ask_id() else {
                let _ = self.telegram.send(chat_id, "No open ask to answer.", None);
                return Ok(());
            };
            if state.complete_ask(&ask_id, Ok(answer.to_string())) {
                let _ = self.telegram.send(
                    chat_id,
                    &format!("✅ recorded answer for `{ask_id}`"),
                    None,
                );
            }
            return Ok(());
        }

        let uid = user_id.map(|id| id as i64);
        let guest = guest_query_id.as_deref();

        if let Some((name, args)) = commands::split_slash(text) {
            if name == "help" && self.settings.features.builtin_help {
                self.deliver_text(chat_id, guest, &self.help_text(), state);
                return Ok(());
            }
            if let Some((primary, command)) = commands::resolve_command(&self.settings, &name) {
                // Must not block the poll loop: scripts often call `telegram-bots ask`,
                // which needs CallbackQuery handling on this thread.
                self.spawn_command(
                    primary,
                    &command.run,
                    &args,
                    CommandCtx {
                        chat_id,
                        user_id: uid,
                        guest_query_id: guest,
                    },
                    state,
                );
                return Ok(());
            }
            self.deliver_text(
                chat_id,
                guest,
                &format!("Unknown command /{name}. Try /help."),
                state,
            );
            return Ok(());
        }

        // Free-form message (no slash): optional defaultCommand.
        if let Some(default) = &self.settings.default_command {
            self.spawn_command(
                "default",
                default,
                text,
                CommandCtx {
                    chat_id,
                    user_id: uid,
                    guest_query_id: guest,
                },
                state,
            );
        }

        Ok(())
    }

    fn handle_callback(&self, callback: CallbackQuery, state: &Shared) -> Result<(), String> {
        let data = callback.data.clone().unwrap_or_default();
        let Some((ask_id, choice_id)) = telegram::parse_choice_callback(&data) else {
            let _ = self.telegram.answer_callback(&callback.id, None);
            return Ok(());
        };
        let Some(ask) = state.pending_ask(&ask_id) else {
            let _ = self
                .telegram
                .answer_callback(&callback.id, Some("That ask expired."));
            return Ok(());
        };
        if !ask.choice_ids.iter().any(|id| id == &choice_id) {
            let _ = self
                .telegram
                .answer_callback(&callback.id, Some("Unknown choice."));
            return Ok(());
        }
        let user_id = Some(callback.from.id);
        if !self.settings.allows_user(user_id) {
            let _ = self
                .telegram
                .answer_callback(&callback.id, Some("Not allowed."));
            return Ok(());
        }
        if state.complete_ask(&ask_id, Ok(choice_id.clone())) {
            let _ = self
                .telegram
                .answer_callback(&callback.id, Some(&choice_id));
        } else {
            let _ = self
                .telegram
                .answer_callback(&callback.id, Some("That ask expired."));
        }
        Ok(())
    }

    fn spawn_command(
        self: &Arc<Self>,
        label: &str,
        command: &RunnableCommand,
        args: &str,
        ctx: CommandCtx<'_>,
        state: &Arc<Shared>,
    ) {
        let argv = commands::split_args(args);
        log::info!("running {label} command={} args={argv:?}", command.command);
        let mut cmd = Command::new(&command.command);
        cmd.args(&argv);
        cmd.env("TELEGRAM_BOTS_INSTANCE", &self.settings.instance);
        cmd.env("TELEGRAM_BOTS_COMMAND", label);
        cmd.env("TELEGRAM_BOTS_CHAT_ID", ctx.chat_id.to_string());
        cmd.env("TELEGRAM_BOTS_TIMEZONE", &self.settings.timezone);
        if let Some(uid) = ctx.user_id {
            cmd.env("TELEGRAM_BOTS_USER_ID", uid.to_string());
        }
        if let Some(gq) = ctx.guest_query_id {
            cmd.env("TELEGRAM_BOTS_GUEST_QUERY_ID", gq);
        }
        let context = serde_json::to_string(&self.settings.context).unwrap_or_else(|_| "{}".into());
        cmd.env("TELEGRAM_BOTS_CONTEXT", context);
        for (key, value) in &command.env {
            cmd.env(key, value);
        }

        let app = Arc::clone(self);
        let state = Arc::clone(state);
        let label = label.to_string();
        let timeout = Duration::from_secs(command.timeout_sec);
        let chat_id = ctx.chat_id;
        let guest = ctx.guest_query_id.map(str::to_string);
        thread::spawn(move || {
            let follow = command_follow_up(&label, proc::run(cmd, timeout));
            if let Some(reply) = follow {
                app.deliver_text(chat_id, guest.as_deref(), &reply, &state);
            }
        });
    }

    /// Send text via answerGuestQuery when the guest id is still unused; else sendMessage.
    fn deliver_text(&self, chat_id: i64, guest_query_id: Option<&str>, text: &str, state: &Shared) {
        if let Some(qid) = guest_query_id {
            if state.consume_guest_query(qid) {
                if let Err(err) = self.telegram.answer_guest(qid, text, None) {
                    log::error!("answerGuestQuery failed: {err}");
                }
                return;
            }
        }
        if let Err(err) = self.telegram.send(chat_id, text, None) {
            log::error!("send failed: {err}");
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
        let body = format_message(role, text, None, false);
        let chat_id = self.primary_chat()?;
        self.telegram.send(chat_id, &body, None)
    }

    pub fn ask(&self, state: &Shared, opts: AskOpts<'_>) -> Result<(String, String), String> {
        let ask_id = short_id();
        let has_choices = !opts.choices.is_empty();
        let body = format_message(opts.role, opts.text, Some(&ask_id), has_choices);
        let chat_id = match opts.chat_id {
            Some(id) => id,
            None => self.primary_chat()?,
        };
        let markup = if has_choices {
            Some(telegram::choice_keyboard(&ask_id, opts.choices))
        } else {
            None
        };

        let (message_id, inline_message_id) =
            self.send_ask(chat_id, opts.guest_query_id, &body, markup, state)?;

        let ask = PendingAsk {
            id: ask_id.clone(),
            role: opts.role.unwrap_or("agent").to_string(),
            chat_id,
            message_id,
            inline_message_id,
            choice_ids: opts.choices.iter().map(|c| c.id.clone()).collect(),
            created: now_unix_secs(),
        };
        let waiter = state.register_ask(ask);
        match Shared::wait_for_answer(&waiter, opts.timeout) {
            Ok(answer) => Ok((ask_id, answer)),
            Err(err) => {
                state.complete_ask(&ask_id, Err(err.clone()));
                Err(err)
            }
        }
    }

    fn send_ask(
        &self,
        chat_id: i64,
        guest_query_id: Option<&str>,
        body: &str,
        markup: Option<frankenstein::types::InlineKeyboardMarkup>,
        state: &Shared,
    ) -> Result<(Option<i32>, Option<String>), String> {
        if let Some(qid) = guest_query_id {
            if state.consume_guest_query(qid) {
                let inline_id = self.telegram.answer_guest(qid, body, markup)?;
                return Ok((None, Some(inline_id)));
            }
        }
        let message_id = self.telegram.send(chat_id, body, markup)?;
        Ok((Some(message_id), None))
    }

    fn primary_chat(&self) -> Result<i64, String> {
        self.settings
            .chat_ids
            .first()
            .copied()
            .ok_or_else(|| "no chat ids configured".into())
    }
}

/// Map command process result to an optional Telegram follow-up.
/// Exit 0 + empty stdout ⇒ `None` (daemon stays silent).
pub fn command_follow_up(label: &str, result: Result<ProcOutput, String>) -> Option<String> {
    match result {
        Ok(output) => {
            let text = output.stdout.trim();
            if text.is_empty() {
                None
            } else {
                Some(text.to_string())
            }
        }
        Err(err) => Some(format!("❌ {label} failed: {err}")),
    }
}

fn format_message(
    role: Option<&str>,
    text: &str,
    ask_id: Option<&str>,
    has_choices: bool,
) -> String {
    let role = role.unwrap_or("agent");
    match ask_id {
        Some(id) if has_choices => {
            format!("[{role}] ask `{id}`\n\n{text}\n\nChoose a button below.")
        }
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
            format_message(Some("vuln-monitor"), "build finished", None, false),
            "[vuln-monitor]\nbuild finished"
        );
    }

    #[test]
    fn format_message_ask_includes_id_and_hint() {
        let body = format_message(Some("agent"), "Ship it?", Some("abcd1234"), false);
        assert!(body.starts_with("[agent] ask `abcd1234`"));
        assert!(body.contains("Ship it?"));
        assert!(body.contains("/a <answer>"));
    }

    #[test]
    fn format_message_choice_ask_hints_buttons() {
        let body = format_message(Some("agent"), "Ship it?", Some("abcd1234"), true);
        assert!(body.contains("Choose a button below."));
        assert!(!body.contains("/a <answer>"));
    }

    #[test]
    fn format_message_default_role() {
        assert_eq!(format_message(None, "hi", None, false), "[agent]\nhi");
    }

    #[test]
    fn strip_answer_prefix_variants() {
        assert_eq!(strip_answer_prefix("/a yes"), "yes");
        assert_eq!(strip_answer_prefix("/answer yes"), "yes");
        assert_eq!(strip_answer_prefix("plain"), "plain");
    }

    #[test]
    fn command_follow_up_silent_on_empty_stdout() {
        assert_eq!(
            command_follow_up(
                "expense",
                Ok(ProcOutput {
                    stdout: String::new()
                })
            ),
            None
        );
        assert_eq!(
            command_follow_up(
                "expense",
                Ok(ProcOutput {
                    stdout: "   \n".into()
                })
            ),
            None
        );
    }

    #[test]
    fn command_follow_up_sends_trimmed_stdout() {
        assert_eq!(
            command_follow_up(
                "expense",
                Ok(ProcOutput {
                    stdout: " logged 15 coffee \n".into()
                })
            )
            .as_deref(),
            Some("logged 15 coffee")
        );
    }

    #[test]
    fn command_follow_up_errors() {
        let msg = command_follow_up("expense", Err("boom".into())).unwrap();
        assert!(msg.contains("❌ expense failed"));
        assert!(msg.contains("boom"));
    }
}
