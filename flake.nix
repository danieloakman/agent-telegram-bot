{
  description = "Agent Telegram bot (notify + ask for automation)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    let
      overlay = final: _prev: {
        agent-telegram-bot = final.callPackage ./nix/package.nix { };
      };
    in
    (flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ overlay ];
        };
        package = pkgs.agent-telegram-bot;
      in
      {
        packages.default = package;

        apps.default = {
          type = "app";
          program = "${package}/bin/agent-telegram-bot";
        };

        checks.package = package;

        formatter = pkgs.nixfmt;

        devShells.default = pkgs.mkShell {
          name = "agent-telegram-bot";
          packages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            rust-analyzer
            just
          ];
          RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
        };
      }
    ))
    // {
      overlays.default = overlay;

      homeManagerModules.default =
        { lib, pkgs, ... }:
        {
          imports = [ ./nix/home-manager.nix ];
          config.services.agent-telegram-bot.package =
            lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.default;
        };
    };
}
