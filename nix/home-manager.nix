# Home Manager module: user systemd daemon + CLI package.
#
# Token is supplied as a file path so consumers can wire sops, agenix, etc.
# Prefer importing via the flake (`homeManagerModules.default`) so `package`
# is defaulted; if you import this file directly, set `package` yourself.
{ config, lib, pkgs, ... }:
let
  cfg = config.services.agent-telegram-bot;
  settingsFile = pkgs.writeText "agent-telegram-bot-settings.json" (builtins.toJSON {
    chatIds = cfg.chatIds;
    allowedUserIds = cfg.allowedUserIds;
  });
in
{
  options.services.agent-telegram-bot = {
    enable = lib.mkEnableOption ''
      Agent Telegram bridge (notify + ask): long-poll daemon and unix control socket.
    '';

    package = lib.mkOption {
      type = lib.types.package;
      description = "agent-telegram-bot package.";
    };

    chatIds = lib.mkOption {
      type = lib.types.listOf lib.types.int;
      default = [ ];
      example = [ 8348310754 ];
      description = ''
        Destination Telegram chat ids for send/ask (usually your private chat
        with the bot). First id is used for outbound messages.
      '';
    };

    allowedUserIds = lib.mkOption {
      type = lib.types.listOf lib.types.int;
      default = [ ];
      description = ''
        Telegram user ids allowed to answer asks. Empty allows any user in the
        configured chats.
      '';
    };

    tokenFile = lib.mkOption {
      type = lib.types.path;
      description = ''
        Path to a file containing the Bot API token (one line). Typically a
        sops-nix or agenix secret path.
      '';
    };

    socketPath = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/run/user/1000/agent-telegram-bot.sock";
      description = ''
        Override the control socket path (`AGENT_TELEGRAM_BOT_SOCKET`).
        When null, the daemon uses `$STATE_DIRECTORY/agent-telegram-bot.sock`.
      '';
    };

    extraEnvironment = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = { };
      example = {
        RUST_LOG = "info";
      };
      description = "Extra environment variables for the daemon unit.";
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.chatIds != [ ];
        message = "services.agent-telegram-bot.chatIds must be non-empty.";
      }
    ];

    home.packages = [ cfg.package ];

    systemd.user.services.agent-telegram-bot = {
      Unit = {
        Description = "Agent Telegram bridge (notify + ask)";
        After = [ "network-online.target" ];
        Wants = [ "network-online.target" ];
      };
      Service = {
        ExecStart = "${lib.getExe cfg.package} daemon";
        Restart = "on-failure";
        RestartSec = 5;
        StateDirectory = "agent-telegram-bot";
        Environment = [
          "AGENT_TELEGRAM_BOT_TOKEN_FILE=${cfg.tokenFile}"
          "AGENT_TELEGRAM_BOT_SETTINGS=${settingsFile}"
        ]
        ++ lib.optionals (cfg.socketPath != null) [
          "AGENT_TELEGRAM_BOT_SOCKET=${cfg.socketPath}"
        ]
        ++ lib.mapAttrsToList (name: value: "${name}=${value}") cfg.extraEnvironment;

        # Mild hardening that stays compatible with StateDirectory + secret files.
        NoNewPrivileges = true;
        PrivateTmp = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        RestrictRealtime = true;
        SystemCallArchitectures = "native";
      };
      Install.WantedBy = [ "default.target" ];
    };
  };
}
