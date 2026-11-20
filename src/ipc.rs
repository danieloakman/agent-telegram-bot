use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::service::App;
use crate::state::Shared;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    #[serde(alias = "send")]
    Notify {
        text: String,
        #[serde(default)]
        role: Option<String>,
    },
    Ask {
        text: String,
        #[serde(default)]
        role: Option<String>,
        #[serde(default = "default_timeout")]
        timeout_sec: u64,
        /// Multiple-choice options. Empty/omitted ⇒ free-text ask.
        #[serde(default)]
        choices: Option<Vec<AskChoice>>,
    },
    Ping,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskChoice {
    pub id: String,
    pub label: String,
}

fn default_timeout() -> u64 {
    600
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

impl Response {
    fn ok_notify(message_id: i32) -> Self {
        Self {
            ok: true,
            error: None,
            message_id: Some(message_id),
            ask_id: None,
            answer: None,
        }
    }

    fn ok_ask(ask_id: String, answer: String) -> Self {
        Self {
            ok: true,
            error: None,
            message_id: None,
            ask_id: Some(ask_id),
            answer: Some(answer),
        }
    }

    fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(msg.into()),
            message_id: None,
            ask_id: None,
            answer: None,
        }
    }
}

pub fn listen(socket: &Path, app: Arc<App>, state: Arc<Shared>) -> Result<(), String> {
    if socket.exists() {
        std::fs::remove_file(socket).map_err(|err| format!("removing old socket: {err}"))?;
    }
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("creating socket dir: {err}"))?;
    }
    let listener =
        UnixListener::bind(socket).map_err(|err| format!("binding control socket: {err}"))?;
    // Allow the same user (and root via runuser) to connect.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o660));
    }
    log::info!("control socket listening on {}", socket.display());

    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let app = Arc::clone(&app);
                let state = Arc::clone(&state);
                thread::spawn(move || {
                    if let Err(err) = handle_client(stream, &app, &state) {
                        log::error!("ipc client failed: {err}");
                    }
                });
            }
            Err(err) => {
                log::error!("accept failed: {err}");
                thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

fn handle_client(stream: UnixStream, app: &App, state: &Shared) -> Result<(), String> {
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|err| format!("reading request: {err}"))?;
    if line.trim().is_empty() {
        return Ok(());
    }
    let request: Request =
        serde_json::from_str(line.trim()).map_err(|err| format!("parsing request: {err}"))?;
    let response = match request {
        Request::Ping => Response {
            ok: true,
            error: None,
            message_id: None,
            ask_id: None,
            answer: None,
        },
        Request::Notify { text, role } => match app.notify(role.as_deref(), &text) {
            Ok(message_id) => Response::ok_notify(message_id),
            Err(err) => Response::err(err),
        },
        Request::Ask {
            text,
            role,
            timeout_sec,
            choices,
        } => {
            let choices = choices.unwrap_or_default();
            if !choices.is_empty() {
                // Inline-keyboard asks land in a follow-up change; reject clearly for now.
                Response::err("ask choices are not implemented yet")
            } else {
                match app.ask(
                    role.as_deref(),
                    &text,
                    Duration::from_secs(timeout_sec),
                    state,
                ) {
                    Ok((ask_id, answer)) => Response::ok_ask(ask_id, answer),
                    Err(err) => Response::err(err),
                }
            }
        }
    };
    let mut writer = stream;
    let body = serde_json::to_string(&response).map_err(|e| e.to_string())?;
    writeln!(writer, "{body}").map_err(|err| format!("writing response: {err}"))?;
    Ok(())
}

pub fn call(socket: &Path, request: &Request) -> Result<Response, String> {
    let mut stream = UnixStream::connect(socket)
        .map_err(|err| format!("connecting to {}: {err}", socket.display()))?;
    let body = serde_json::to_string(request).map_err(|e| e.to_string())?;
    writeln!(stream, "{body}").map_err(|err| format!("writing request: {err}"))?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|err| format!("reading response: {err}"))?;
    serde_json::from_str(line.trim()).map_err(|err| format!("parsing response: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_notify_round_trip() {
        let request = Request::Notify {
            text: "hello".into(),
            role: Some("agent".into()),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains(r#""cmd":"notify""#));
        let parsed: Request = serde_json::from_str(&json).unwrap();
        match parsed {
            Request::Notify { text, role } => {
                assert_eq!(text, "hello");
                assert_eq!(role.as_deref(), Some("agent"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn request_accepts_legacy_send_alias() {
        let parsed: Request =
            serde_json::from_str(r#"{"cmd":"send","text":"hi","role":"agent"}"#).unwrap();
        match parsed {
            Request::Notify { text, .. } => assert_eq!(text, "hi"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn request_ask_default_timeout() {
        let parsed: Request =
            serde_json::from_str(r#"{"cmd":"ask","text":"Ship it?","role":"agent"}"#).unwrap();
        match parsed {
            Request::Ask {
                text,
                role,
                timeout_sec,
                choices,
            } => {
                assert_eq!(text, "Ship it?");
                assert_eq!(role.as_deref(), Some("agent"));
                assert_eq!(timeout_sec, 600);
                assert!(choices.is_none());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn response_ok_ask_serializes_camel_case() {
        let response = Response::ok_ask("abcd1234".into(), "yes".into());
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains(r#""askId":"abcd1234""#));
        assert!(json.contains(r#""answer":"yes""#));
        assert!(!json.contains("message_id"));
    }

    #[test]
    fn response_err_omits_optional_fields() {
        let response = Response::err("nope");
        let json = serde_json::to_string(&response).unwrap();
        assert_eq!(json, r#"{"ok":false,"error":"nope"}"#);
    }
}
