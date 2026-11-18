# Home Manager module: user systemd daemon + CLI package.
#
# Token is supplied as a file path so consumers can wire sops, agenix, etc.
{ config, lib, pkgs, ... }:
let
  cfg = config.services.agent-telegram-bot;
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
      };
      Service = {
        ExecStart = "${lib.getExe cfg.package} daemon";
        Restart = "on-failure";
        RestartSec = 5;
        StateDirectory = "agent-telegram-bot";
        Environment = [
          "AGENT_TELEGRAM_BOT_TOKEN_FILE=${cfg.tokenFile}"
          "AGENT_TELEGRAM_BOT_SETTINGS=${
            pkgs.writeText "agent-telegram-bot-settings.json" (builtins.toJSON {
              chatIds = cfg.chatIds;
              allowedUserIds = cfg.allowedUserIds;
            })
          }"
        ];
      };
      Install.WantedBy = [ "default.target" ];
    };
  };
}
