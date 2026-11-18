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
| `STATE_DIRECTORY` | State dir (socket + persisted state); set by systemd `StateDirectory=` |
| `AGENT_TELEGRAM_BOT_SOCKET` | Optional override for the control socket path |

Default socket: `$STATE_DIRECTORY/agent-telegram-bot.sock`, or `~/.local/state/agent-telegram-bot/agent-telegram-bot.sock`.

## Nix

```bash
nix build
nix develop
```

As a flake input:

```nix
{
  inputs.agent-telegram-bot.url = "github:danieloakman/agent-telegram-bot";
  # packages.${system}.default
}
```

## Develop

```bash
just build
just test
just fmt
```

## License

MIT
