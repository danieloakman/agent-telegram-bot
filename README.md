# agent-telegram-bot

Notify/ask bridge for agents via Telegram: a long-poll daemon plus a unix control socket, with a small CLI for fire-and-forget notices and blocking questions.

## CLI

```bash
# Long-poll daemon (systemd default)
agent-telegram-bot daemon

# Fire-and-forget notice
agent-telegram-bot send --role vuln-monitor "build finished"

# Block until a Telegram reply (or timeout)
agent-telegram-bot ask --role agent --timeout 600 "Ship it?"

# Check the control socket
agent-telegram-bot ping
```

Message body can be a positional argument or stdin.

## Daemon environment

| Variable | Purpose |
|----------|---------|
| `AGENT_TELEGRAM_BOT_TOKEN_FILE` | Path to Bot API token file |
| `AGENT_TELEGRAM_BOT_SETTINGS` | JSON settings: `{ "chatIds": [...], "allowedUserIds": [...] }` |
| `STATE_DIRECTORY` | Daemon only: state dir (socket + persisted state); set by systemd `StateDirectory=` |
| `AGENT_TELEGRAM_BOT_SOCKET` | Optional override for the control socket path |

Daemon socket: `$STATE_DIRECTORY/agent-telegram-bot.sock` (or `AGENT_TELEGRAM_BOT_SOCKET`).
CLI send/ask/ping ignore `STATE_DIRECTORY` and use `AGENT_TELEGRAM_BOT_SOCKET` or `~/.local/state/agent-telegram-bot/agent-telegram-bot.sock`, so a foreign systemd unit's `STATE_DIRECTORY` cannot poison the client.

## Nix

```bash
nix build
nix develop
```

### Flake input + Home Manager

```nix
{
  inputs.agent-telegram-bot.url = "github:danieloakman/agent-telegram-bot";

  # In a home-manager module / user config:
  imports = [ inputs.agent-telegram-bot.homeManagerModules.default ];

  services.agent-telegram-bot = {
    enable = true;
    chatIds = [ 123456789 ];
    allowedUserIds = [ 123456789 ];
    tokenFile = config.sops.secrets.agent_telegram_bot_token.path; # or any token file
    # optional:
    # socketPath = "/run/user/1000/agent-telegram-bot.sock";
    # extraEnvironment = { RUST_LOG = "info"; };
  };
}
```

`package` defaults to this flake’s build; override if needed. Secrets (sops/agenix) stay in the consumer flake. Overlay: `overlays.default` exposes `pkgs.agent-telegram-bot`.

## Develop

```bash
nix develop   # or: direnv allow
just check
just test
just clippy
just fmt
```

## License

MIT
