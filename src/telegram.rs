use std::time::Duration;

use frankenstein::client_ureq::Bot;
use frankenstein::inline_mode::{
    InlineQueryResult, InlineQueryResultArticle, InputTextMessageContent,
};
use frankenstein::methods::{
    AnswerCallbackQueryParams, AnswerGuestQueryParams, EditMessageTextParams, GetUpdatesParams,
    SendMessageParams,
};
use frankenstein::types::{AllowedUpdate, InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup};
use frankenstein::updates::Update;
use frankenstein::TelegramApi;

use crate::ipc::AskChoice;

const TEXT_LIMIT: usize = 3500;
const HTTP_TIMEOUT: Duration = Duration::from_secs(70);

pub struct Telegram {
    bot: Bot,
}

/// Where an ask message lives (private chat or guest/inline).
#[derive(Clone, Debug)]
pub enum EditTarget {
    Chat { chat_id: i64, message_id: i32 },
    Inline(String),
}

impl Telegram {
    pub fn new(token: &str) -> Self {
        let agent = ureq::Agent::new_with_config(
            ureq::config::Config::builder()
                .http_status_as_error(false)
                .timeout_global(Some(HTTP_TIMEOUT))
                .build(),
        );
        Self {
            bot: Bot::builder()
                .api_url(format!("https://api.telegram.org/bot{token}"))
                .request_agent(agent)
                .build(),
        }
    }

    pub fn updates(&self, offset: Option<i64>, guest_mode: bool) -> Result<Vec<Update>, String> {
        let mut allowed = vec![AllowedUpdate::Message, AllowedUpdate::CallbackQuery];
        if guest_mode {
            allowed.push(AllowedUpdate::GuestMessage);
        }
        let params = GetUpdatesParams::builder()
            .maybe_offset(offset)
            .timeout(50u32)
            .allowed_updates(allowed)
            .build();
        self.bot
            .get_updates(&params)
            .map(|response| response.result)
            .map_err(|err| err.to_string())
    }

    pub fn send(
        &self,
        chat_id: i64,
        text: &str,
        markup: Option<InlineKeyboardMarkup>,
    ) -> Result<i32, String> {
        let params = SendMessageParams::builder()
            .chat_id(chat_id)
            .text(clip(text))
            .maybe_reply_markup(markup.map(ReplyMarkup::InlineKeyboardMarkup))
            .build();
        self.bot
            .send_message(&params)
            .map(|response| response.result.message_id)
            .map_err(|err| err.to_string())
    }

    pub fn answer_guest(
        &self,
        query_id: &str,
        text: &str,
        markup: Option<InlineKeyboardMarkup>,
    ) -> Result<String, String> {
        let article = InlineQueryResultArticle::builder()
            .id("1")
            .title("telegram-bots")
            .input_message_content(
                InputTextMessageContent::builder()
                    .message_text(clip(text))
                    .build(),
            )
            .maybe_reply_markup(markup)
            .build();
        let params = AnswerGuestQueryParams::builder()
            .guest_query_id(query_id)
            .result(InlineQueryResult::from(article))
            .build();
        self.bot
            .answer_guest_query(&params)
            .map(|response| response.result.inline_message_id)
            .map_err(|err| err.to_string())
    }

    pub fn answer_callback(&self, callback_id: &str, text: Option<&str>) -> Result<(), String> {
        let params = AnswerCallbackQueryParams::builder()
            .callback_query_id(callback_id)
            .maybe_text(text.map(str::to_string))
            .build();
        self.bot
            .answer_callback_query(&params)
            .map(|_| ())
            .map_err(|err| err.to_string())
    }

    pub fn edit(
        &self,
        target: &EditTarget,
        text: &str,
        markup: Option<InlineKeyboardMarkup>,
    ) -> Result<(), String> {
        let params = match target {
            EditTarget::Inline(id) => EditMessageTextParams::builder()
                .text(clip(text))
                .maybe_reply_markup(markup.clone())
                .inline_message_id(id)
                .build(),
            EditTarget::Chat {
                chat_id,
                message_id,
            } => EditMessageTextParams::builder()
                .text(clip(text))
                .maybe_reply_markup(markup)
                .chat_id(*chat_id)
                .message_id(*message_id)
                .build(),
        };
        self.bot
            .edit_message_text(&params)
            .map(|_| ())
            .map_err(|err| err.to_string())
    }
}

pub fn empty_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(Vec::new())
        .build()
}

pub fn choice_keyboard(ask_id: &str, choices: &[AskChoice]) -> InlineKeyboardMarkup {
    let row = choices
        .iter()
        .map(|choice| {
            InlineKeyboardButton::builder()
                .text(choice.label.clone())
                .callback_data(callback_data(ask_id, &choice.id))
                .build()
        })
        .collect();
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![row])
        .build()
}

pub fn callback_data(ask_id: &str, choice_id: &str) -> String {
    format!("a:{ask_id}:{choice_id}")
}

/// Parse `a:{ask_id}:{choice_id}` callback payloads.
pub fn parse_choice_callback(data: &str) -> Option<(String, String)> {
    let rest = data.strip_prefix("a:")?;
    let (ask_id, choice_id) = rest.split_once(':')?;
    if ask_id.is_empty() || choice_id.is_empty() {
        return None;
    }
    Some((ask_id.to_string(), choice_id.to_string()))
}

fn clip(text: &str) -> String {
    let mut clipped: String = text.chars().take(TEXT_LIMIT).collect();
    if text.chars().count() > TEXT_LIMIT {
        clipped.push('…');
    }
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_leaves_short_text() {
        assert_eq!(clip("hello"), "hello");
    }

    #[test]
    fn clip_truncates_long_text() {
        let long: String = "あ".repeat(TEXT_LIMIT + 10);
        let clipped = clip(&long);
        assert_eq!(clipped.chars().count(), TEXT_LIMIT + 1);
        assert!(clipped.ends_with('…'));
    }

    #[test]
    fn choice_callback_round_trip() {
        let data = callback_data("abcd1234", "confirm");
        assert_eq!(
            parse_choice_callback(&data),
            Some(("abcd1234".into(), "confirm".into()))
        );
        assert!(parse_choice_callback("nope").is_none());
        assert!(parse_choice_callback("a:only").is_none());
    }

    #[test]
    fn choice_keyboard_encodes_ids() {
        let markup = choice_keyboard(
            "abcd1234",
            &[
                AskChoice {
                    id: "confirm".into(),
                    label: "Confirm".into(),
                },
                AskChoice {
                    id: "cancel".into(),
                    label: "Cancel".into(),
                },
            ],
        );
        assert_eq!(markup.inline_keyboard.len(), 1);
        assert_eq!(markup.inline_keyboard[0].len(), 2);
        assert_eq!(
            markup.inline_keyboard[0][0].callback_data.as_deref(),
            Some("a:abcd1234:confirm")
        );
        assert_eq!(markup.inline_keyboard[0][1].text, "Cancel");
    }
}
