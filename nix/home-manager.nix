# Home Manager module: one user systemd daemon (+ socket) per instance.
#
# Token is a file path so consumers can wire sops, agenix, etc.
# Prefer importing via the flake (`homeManagerModules.default`) so `package`
# is defaulted; if you import this file directly, set `package` yourself.
{ config, lib, pkgs, ... }:
let
  cfg = config.services.telegram-bots;

  instanceModule = { name, ... }: {
    options = {
      enable = lib.mkEnableOption "this telegram-bots instance";

      tokenFile = lib.mkOption {
        type = lib.types.path;
        description = ''
          Path to a file containing the Bot API token (one line). Typically a
          sops-nix or agenix secret path.
        '';
      };

      chatIds = lib.mkOption {
        type = lib.types.listOf lib.types.int;
        example = [ 8348310754 ];
        description = ''
          Destination Telegram chat ids for notify/ask. First id is used for
          outbound messages. Required non-empty when enabled.
        '';
      };

      allowedUserIds = lib.mkOption {
        type = lib.types.listOf lib.types.int;
        default = [ ];
        description = ''
          Telegram user ids allowed to answer asks / run commands. Empty allows
          any user in allowed chats.
        '';
      };

      allowedChatIds = lib.mkOption {
        type = lib.types.listOf lib.types.int;
        default = [ ];
        description = ''
          Telegram chat ids allowed for inbound traffic. Empty allows every chat.
          Outbound notify/ask still uses chatIds.
        '';
      };

      guestMode = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Handle BotFather guest mentions (answerGuestQuery).";
      };

      timezone = lib.mkOption {
        type = lib.types.str;
        default = "UTC";
        description = "IANA timezone passed to command scripts as TELEGRAM_BOTS_TIMEZONE.";
      };

      socketPath = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/user/1000/telegram-bots-agent.sock";
        description = ''
          Override the control socket path (`TELEGRAM_BOTS_SOCKET`).
          When null, the daemon uses `$STATE_DIRECTORY/telegram-bots.sock`.
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

      context = lib.mkOption {
        type = lib.types.attrs;
        default = { };
        description = ''
          Opaque JSON object exported to command scripts as TELEGRAM_BOTS_CONTEXT
          (payers, calendar ids, …).
        '';
      };

      features.builtinHelp = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Auto-handle /help from command descriptions.";
      };

      defaultCommand = lib.mkOption {
        type = lib.types.nullOr (lib.types.submodule runnableModule);
        default = null;
        description = ''
          Optional runner for free-form messages (no leading /). Same package
          + argv contract as commands; TELEGRAM_BOTS_COMMAND=default.
        '';
      };

      commands = lib.mkOption {
        type = lib.types.attrsOf (lib.types.submodule commandModule);
        default = { };
        description = "Slash commands (key = primary name, no leading /).";
      };
    };
  };

  runnableModule = {
    options = {
      package = lib.mkOption {
        type = lib.types.package;
        example = lib.literalExpression ''
          pkgs.writeShellScriptBin "brown-fam-default" '''
            set -euo pipefail
            # free-form "trotty paid 40 lunch" → "$1"=trotty "$2"=paid …
            echo "got: $*"
          '''
        '';
        description = ''
          Executable (typically writeShellScriptBin). Message words are
          appended as argv.
        '';
      };
      timeoutSec = lib.mkOption {
        type = lib.types.ints.positive;
        default = 60;
        description = "Kill the process group after this many seconds.";
      };
      env = lib.mkOption {
        type = lib.types.attrsOf lib.types.str;
        default = { };
        description = "Extra environment for this runner only.";
      };
    };
  };

  commandModule = {
    options = {
      description = lib.mkOption {
        type = lib.types.str;
        default = "";
        description = "setMyCommands / help text.";
      };
      aliases = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        description = "Alternate slash names (e.g. calendar for cal).";
      };
      package = lib.mkOption {
        type = lib.types.package;
        example = lib.literalExpression ''
          pkgs.writeShellScriptBin "brown-fam-expense" '''
            set -euo pipefail
            # argv from "/expense 15 coffee" → "$1"=15 "$2"=coffee
            echo "logged: $*"
          '''
        '';
        description = ''
          Executable for this slash command (typically writeShellScriptBin).
          Message args after /name are appended as argv.
        '';
      };
      timeoutSec = lib.mkOption {
        type = lib.types.ints.positive;
        default = 60;
        description = "Kill the process group after this many seconds.";
      };
      env = lib.mkOption {
        type = lib.types.attrsOf lib.types.str;
        default = { };
        description = "Extra environment for this command only.";
      };
    };
  };

  runnableSettings = cmd: {
    inherit (cmd) env;
    command = lib.getExe cmd.package;
    timeoutSec = cmd.timeoutSec;
  };

  settingsFor = name: inst: pkgs.writeText "telegram-bots-${name}-settings.json" (builtins.toJSON {
    instance = name;
    chatIds = inst.chatIds;
    allowedUserIds = inst.allowedUserIds;
    allowedChatIds = inst.allowedChatIds;
    guestMode = inst.guestMode;
    timezone = inst.timezone;
    context = inst.context;
    features.builtinHelp = inst.features.builtinHelp;
    defaultCommand =
      if inst.defaultCommand != null then runnableSettings inst.defaultCommand else null;
    commands = lib.mapAttrs (_: cmd: {
      inherit (cmd) description aliases;
    } // runnableSettings cmd) inst.commands;
  });

  enabledInstances = lib.filterAttrs (_: inst: inst.enable) cfg.instances;
in
{
  options.services.telegram-bots = {
    package = lib.mkOption {
      type = lib.types.package;
      description = "telegram-bots package.";
    };

    instances = lib.mkOption {
      type = lib.types.attrsOf (lib.types.submodule instanceModule);
      default = { };
      description = ''
        Telegram bot instances. Attr key is the instance id used with
        `telegram-bots --instance <name> notify|ask|ping`.
      '';
    };
  };

  config = lib.mkIf (enabledInstances != { }) {
    assertions = lib.mapAttrsToList (name: inst: {
      assertion = inst.chatIds != [ ];
      message = "services.telegram-bots.instances.${name}.chatIds must be non-empty.";
    }) enabledInstances;

    home.packages = [ cfg.package ];

    systemd.user.services = lib.mapAttrs' (name: inst:
      lib.nameValuePair "telegram-bots-${name}" {
        Unit = {
          Description = "telegram-bots instance ${name}";
          After = [ "network-online.target" ];
          Wants = [ "network-online.target" ];
        };
        Service = {
          ExecStart = "${lib.getExe cfg.package} --instance ${lib.escapeShellArg name} daemon";
          Restart = "on-failure";
          RestartSec = 5;
          StateDirectory = "telegram-bots-${name}";
          Environment = [
            "TELEGRAM_BOTS_INSTANCE=${name}"
            "TELEGRAM_BOTS_TOKEN_FILE=${inst.tokenFile}"
            "TELEGRAM_BOTS_SETTINGS=${settingsFor name inst}"
          ]
          ++ lib.optionals (inst.socketPath != null) [
            "TELEGRAM_BOTS_SOCKET=${inst.socketPath}"
          ]
          ++ lib.mapAttrsToList (k: v: "${k}=${v}") inst.extraEnvironment;

          NoNewPrivileges = true;
          PrivateTmp = true;
          RestrictSUIDSGID = true;
          LockPersonality = true;
          MemoryDenyWriteExecute = true;
          RestrictRealtime = true;
          SystemCallArchitectures = "native";
        };
        Install.WantedBy = [ "default.target" ];
      }
    ) enabledInstances;
  };
}
