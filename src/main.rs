mod commands;
mod ipc;
mod proc;
mod service;
mod settings;
mod state;
mod telegram;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use clap::{Parser, Subcommand};

use crate::ipc::{AskChoice, Request, Response};
use crate::service::App;
use crate::settings::Settings;
use crate::state::Shared;
use crate::telegram::Telegram;

#[derive(Debug, Parser)]
#[command(
    name = "telegram-bots",
    about = "Nix-configured Telegram bots (notify / ask / optional commands)"
)]
struct Cli {
    /// Instance id (matches services.telegram-bots.instances.<name>).
    #[arg(long, env = "TELEGRAM_BOTS_INSTANCE")]
    instance: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Long-poll Telegram and serve the control socket (systemd default).
    Daemon,
    /// Fire-and-forget notice.
    Notify {
        /// Role tag shown as [role] in the message.
        #[arg(long, default_value = "agent")]
        role: String,
        /// Message body. If omitted, read stdin.
        text: Option<String>,
    },
    /// Ask a question and block until a Telegram reply (or timeout).
    Ask {
        #[arg(long, default_value = "agent")]
        role: String,
        #[arg(long, default_value_t = 600)]
        timeout: u64,
        /// Override destination chat (default: settings chatIds[0]).
        #[arg(long)]
        chat_id: Option<i64>,
        /// Inline-keyboard choice as `id=Label` (repeatable). Answer is the id.
        #[arg(long = "choice", value_name = "ID=LABEL", value_parser = parse_choice)]
        choices: Vec<AskChoice>,
        /// Guest query id for answerGuestQuery (default: TELEGRAM_BOTS_GUEST_QUERY_ID).
        #[arg(long, env = "TELEGRAM_BOTS_GUEST_QUERY_ID")]
        guest_query_id: Option<String>,
        text: Option<String>,
    },
    /// Check the daemon control socket.
    Ping,
}

fn parse_choice(raw: &str) -> Result<AskChoice, String> {
    let (id, label) = raw
        .split_once('=')
        .ok_or_else(|| String::from("expected id=Label"))?;
    let id = id.trim();
    let label = label.trim();
    if id.is_empty() || label.is_empty() {
        return Err("expected non-empty id=Label".into());
    }
    if id.contains(':') {
        return Err("choice id must not contain ':'".into());
    }
    Ok(AskChoice {
        id: id.to_string(),
        label: label.to_string(),
    })
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    let instance = cli.instance.clone();
    match cli.command.unwrap_or(Command::Daemon) {
        Command::Daemon => run_daemon(instance),
        Command::Notify { role, text } => {
            let text = message_text(text)?;
            client_call(
                instance.as_deref(),
                Request::Notify {
                    text,
                    role: Some(role),
                },
            )
        }
        Command::Ask {
            role,
            timeout,
            chat_id,
            choices,
            guest_query_id,
            text,
        } => {
            let text = message_text(text)?;
            let response = raw_call(
                instance.as_deref(),
                Request::Ask {
                    text,
                    role: Some(role),
                    timeout_sec: timeout,
                    choices: if choices.is_empty() {
                        None
                    } else {
                        Some(choices)
                    },
                    chat_id,
                    guest_query_id,
                },
            )?;
            if !response.ok {
                return Err(response.error.unwrap_or_else(|| "ask failed".into()));
            }
            let answer = response.answer.unwrap_or_default();
            println!("{answer}");
            Ok(())
        }
        Command::Ping => client_call(instance.as_deref(), Request::Ping),
    }
}

fn message_text(arg: Option<String>) -> Result<String, String> {
    if let Some(text) = arg {
        if text.trim().is_empty() {
            return Err("message text is empty".into());
        }
        return Ok(text);
    }
    let mut buf = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
        .map_err(|err| format!("reading stdin: {err}"))?;
    let text = buf.trim_end().to_string();
    if text.is_empty() {
        return Err("message text is empty".into());
    }
    Ok(text)
}

fn client_call(instance: Option<&str>, request: Request) -> Result<(), String> {
    let response = raw_call(instance, request)?;
    if !response.ok {
        return Err(response.error.unwrap_or_else(|| "request failed".into()));
    }
    if let Some(answer) = response.answer {
        println!("{answer}");
    } else if let Some(message_id) = response.message_id {
        log::info!("sent message_id={message_id}");
    }
    Ok(())
}

fn raw_call(instance: Option<&str>, request: Request) -> Result<Response, String> {
    // CLI must not inherit a foreign STATE_DIRECTORY (e.g. another systemd unit).
    let socket = socket_path(instance, /* prefer_state_directory */ false)?;
    ipc::call(&socket, &request)
}

fn run_daemon(instance: Option<String>) -> Result<(), String> {
    let instance = instance
        .or_else(|| std::env::var("TELEGRAM_BOTS_INSTANCE").ok())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| String::from("daemon requires --instance or TELEGRAM_BOTS_INSTANCE"))?;
    let token_path = env_var("TELEGRAM_BOTS_TOKEN_FILE")?;
    let token = std::fs::read_to_string(&token_path)
        .map_err(|err| format!("reading telegram token: {err}"))?;
    let token = token.trim();
    if token.is_empty() || token == "REPLACE_ME" {
        return Err("telegram bot token is missing (set tokenFile / sops secret)".into());
    }
    let settings = Settings::load(Path::new(&env_var("TELEGRAM_BOTS_SETTINGS")?))?;
    if settings.instance != instance {
        log::warn!(
            "TELEGRAM_BOTS_INSTANCE={instance} differs from settings.instance={}",
            settings.instance
        );
    }
    let state_dir = env_var("STATE_DIRECTORY")?;
    let state = Shared::load(Path::new(&state_dir))?;
    let socket = socket_path(Some(&instance), /* prefer_state_directory */ true)?;
    let app = Arc::new(App::new(settings, Telegram::new(token)));

    let app_ipc = Arc::clone(&app);
    let state_ipc = Arc::clone(&state);
    let socket_ipc = socket.clone();
    thread::spawn(move || {
        if let Err(err) = ipc::listen(&socket_ipc, app_ipc, state_ipc) {
            log::error!("ipc listener exited: {err}");
            std::process::exit(1);
        }
    });

    log::info!(
        "starting daemon instance={instance} socket={}",
        socket.display()
    );
    app.run_poll(state)
}

fn socket_path(instance: Option<&str>, prefer_state_directory: bool) -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("TELEGRAM_BOTS_SOCKET") {
        return Ok(PathBuf::from(path));
    }
    if prefer_state_directory {
        if let Ok(state) = std::env::var("STATE_DIRECTORY") {
            return Ok(PathBuf::from(state).join("telegram-bots.sock"));
        }
    }
    let instance = instance.ok_or_else(|| {
        String::from("CLI requires --instance (or TELEGRAM_BOTS_INSTANCE / TELEGRAM_BOTS_SOCKET)")
    })?;
    let home = std::env::var("HOME").map_err(|_| String::from("HOME is not set"))?;
    let state_home =
        std::env::var("XDG_STATE_HOME").unwrap_or_else(|_| format!("{home}/.local/state"));
    Ok(PathBuf::from(state_home).join(format!("telegram-bots-{instance}/telegram-bots.sock")))
}

fn env_var(key: &str) -> Result<String, String> {
    std::env::var(key).map_err(|_| format!("{key} is not set"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_choice_id_label() {
        let choice = parse_choice("confirm=Confirm").unwrap();
        assert_eq!(choice.id, "confirm");
        assert_eq!(choice.label, "Confirm");
        assert!(parse_choice("nocolon").is_err());
        assert!(parse_choice("=Label").is_err());
        assert!(parse_choice("a:b=Label").is_err());
    }
}
