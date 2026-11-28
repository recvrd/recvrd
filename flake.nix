{
  description = "recvrd development environment and package";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;

      webFor =
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        pkgs.buildNpmPackage {
          pname = "recvrd-web";
          version = (builtins.fromJSON (builtins.readFile ./crates/recvrd-web/package.json)).version;
          src = ./crates/recvrd-web;
          npmDepsHash = "sha256-jxD0gC08yqwfKCgE9celd0oXB2L1u5lApWR/oQN5UBo=";
          buildPhase = "npm run build";
          installPhase = "cp -r dist $out";
        };

      recvrdFor =
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          inherit (pkgs) lib;
        in
        pkgs.rustPlatform.buildRustPackage {
          pname = "recvrd";
          version = (builtins.fromTOML (builtins.readFile ./crates/recvrd/Cargo.toml)).package.version;
          src = self;

          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = {
              "rusty_ytdl-0.7.4" = "sha256-mCX+vylB4uV8GFhv8vLC69MEszEkfUgA7xXtD1fP6go=";
            };
          };

          cargoBuildFlags = [ "--package" "recvrd" ];
          doCheck = false;

          preBuild = ''
            ${
              lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
                export LIBCLANG_PATH="${pkgs.llvmPackages.libclang.lib}/lib"
                export BINDGEN_EXTRA_CLANG_ARGS="-I${pkgs.glibc.dev}/include"
              ''
            }
            mkdir -p crates/recvrd-web/dist
            cp -r ${webFor system}/. crates/recvrd-web/dist/
          '';

          nativeBuildInputs =
            [ pkgs.makeWrapper pkgs.pkg-config ]
            ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.llvmPackages.libclang ];

          buildInputs =
            with pkgs;
            lib.optionals stdenv.hostPlatform.isLinux [
              pipewire
              dbus
            ];

          postInstall = ''
            wrapProgram $out/bin/recvrd \
              --prefix PATH : ${lib.makeBinPath [ pkgs.yt-dlp pkgs.ffmpeg ]}
            install -Dm644 packaging/systemd/recvrd.service \
              $out/lib/systemd/user/recvrd.service
            install -Dm644 packaging/systemd/recvrd.env.example \
              $out/share/doc/recvrd/recvrd.env.example
            install -Dm644 config.example.toml \
              $out/share/doc/recvrd/config.example.toml
          '';

          meta = {
            description = "Headless YouTube Music daemon with a web UI, MPRIS and MCP";
            license = lib.licenses.asl20;
            mainProgram = "recvrd";
          };
        };

      recvcFor =
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          inherit (pkgs) lib;
        in
        pkgs.rustPlatform.buildRustPackage {
          pname = "recvc";
          version = (builtins.fromTOML (builtins.readFile ./crates/recvc/Cargo.toml)).package.version;
          src = self;

          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = {
              "rusty_ytdl-0.7.4" = "sha256-mCX+vylB4uV8GFhv8vLC69MEszEkfUgA7xXtD1fP6go=";
            };
          };

          cargoBuildFlags = [ "--package" "recvc" ];
          doCheck = false;

          meta = {
            description = "Terminal client for the recvrd daemon";
            license = lib.licenses.asl20;
            mainProgram = "recvc";
          };
        };

      nixosModule =
        { config, lib, pkgs, ... }:
        let
          cfg = config.services.recvrd;
        in
        {
          imports = [
            (lib.mkRenamedOptionModule [ "services" "ytermusicd" ] [ "services" "recvrd" ])
          ];

          options.services.recvrd = {
            enable = lib.mkEnableOption "recvrd headless daemon";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.recvrd;
              description = "recvrd package to use.";
            };
            user = lib.mkOption {
              type = lib.types.str;
              default = "recvrd";
              description = "System user for the service.";
            };
            group = lib.mkOption {
              type = lib.types.str;
              default = "recvrd";
              description = "System group for the service.";
            };
            bind = lib.mkOption {
              type = lib.types.str;
              default = "127.0.0.1";
              description = "HTTP bind address.";
            };
            port = lib.mkOption {
              type = lib.types.port;
              default = 8080;
              description = "HTTP port.";
            };
            environmentFile = lib.mkOption {
              type = lib.types.nullOr lib.types.path;
              default = null;
              description = "Optional systemd EnvironmentFile (headers path, secrets, etc.).";
            };
          };

          config = lib.mkIf cfg.enable {
            users.groups.${cfg.group} = { };
            users.users.${cfg.user} = {
              isSystemUser = true;
              group = cfg.group;
            };

            systemd.services.recvrd-migrate = {
              description = "Migrate ytermusicd state to recvrd";
              before = [ "recvrd.service" ];
              requiredBy = [ "recvrd.service" ];
              path = [ pkgs.coreutils ];
              serviceConfig = {
                Type = "oneshot";
                User = "root";
              };
              script = ''
                if [ -d /var/lib/ytermusicd ]; then
                  mkdir -p /var/lib/recvrd
                  if [ -z "$(ls -A /var/lib/recvrd)" ]; then
                    cp -a /var/lib/ytermusicd/. /var/lib/recvrd/
                    chown -R ${cfg.user}:${cfg.group} /var/lib/recvrd
                    rm -rf /var/lib/ytermusicd
                  fi
                fi
              '';
            };

            systemd.services.recvrd = {
              description = "recvrd headless YouTube Music daemon";
              after = [ "network-online.target" ];
              wants = [ "network-online.target" ];
              wantedBy = [ "multi-user.target" ];
              aliases = [ "ytermusicd.service" ];

              serviceConfig = {
                ExecStart = "${cfg.package}/bin/recvrd serve";
                User = cfg.user;
                Group = cfg.group;
                Restart = "on-failure";
                RestartSec = 5;
                StateDirectory = "recvrd";
                StateDirectoryMode = "0700";
                CacheDirectory = "recvrd";
                NoNewPrivileges = true;
                PrivateTmp = true;
                Environment = [
                  "HOME=/var/lib/recvrd"
                  "RECVRD_BIND=${cfg.bind}"
                  "RECVRD_PORT=${toString cfg.port}"
                ];
              } // lib.optionalAttrs (cfg.environmentFile != null) {
                EnvironmentFile = cfg.environmentFile;
              };
            };
          };
        };

      homeManagerModule =
        { config, lib, pkgs, ... }:
        let
          cfg = config.services.recvrd;
        in
        {
          imports = [
            (lib.mkRenamedOptionModule [ "services" "ytermusicd" ] [ "services" "recvrd" ])
          ];

          options.services.recvrd = {
            enable = lib.mkEnableOption "recvrd user service";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.recvrd;
              description = "recvrd package to use.";
            };
            bind = lib.mkOption {
              type = lib.types.str;
              default = "127.0.0.1";
              description = "HTTP bind address.";
            };
            port = lib.mkOption {
              type = lib.types.port;
              default = 8080;
              description = "HTTP port.";
            };
            environmentFile = lib.mkOption {
              type = lib.types.nullOr lib.types.path;
              default = null;
              description = "Optional systemd user EnvironmentFile.";
            };
          };

          config = lib.mkIf cfg.enable {
            home.packages = [
              cfg.package
              self.packages.${pkgs.stdenv.hostPlatform.system}.recvc
            ];
            home.sessionVariables.RECVRD_URL =
              "http://${if cfg.bind == "0.0.0.0" then "127.0.0.1" else cfg.bind}:${toString cfg.port}";
            systemd.user.services.recvrd = {
              aliases = [ "ytermusicd.service" ];
              Unit = {
                Description = "recvrd headless YouTube Music daemon";
                After = [ "network-online.target" ];
                Wants = [ "network-online.target" ];
              };
              Service = {
                ExecStart = "${cfg.package}/bin/recvrd serve";
                Restart = "on-failure";
                RestartSec = 5;
                Environment = [
                  "RECVRD_BIND=${cfg.bind}"
                  "RECVRD_PORT=${toString cfg.port}"
                ];
              } // lib.optionalAttrs (cfg.environmentFile != null) {
                EnvironmentFile = cfg.environmentFile;
              };
              Install.WantedBy = [ "default.target" ];
            };
          };
        };
    in
    {
      packages = forAllSystems (system: {
        default = recvrdFor system;
        recvrd = recvrdFor system;
        recvc = recvcFor system;
        ytermusicd = recvrdFor system;
        ytmc = recvcFor system;
      });

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.recvrd}/bin/recvrd";
        };
        recvrd = {
          type = "app";
          program = "${self.packages.${system}.recvrd}/bin/recvrd";
        };
        recvc = {
          type = "app";
          program = "${self.packages.${system}.recvc}/bin/recvc";
        };
        ytermusicd = {
          type = "app";
          program = "${self.packages.${system}.recvrd}/bin/ytermusicd";
        };
        ytmc = {
          type = "app";
          program = "${self.packages.${system}.recvc}/bin/ytmc";
        };
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          inherit (pkgs) lib;

          pkgConfigPaths =
            lib.optionals pkgs.stdenv.hostPlatform.isLinux [
              "${pkgs.pipewire.dev}/lib/pkgconfig"
              "${pkgs.dbus.dev}/lib/pkgconfig"
            ];
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              clippy
              curl
              ffmpeg
              just
              nodejs_22
              rustc
              rustfmt
              yt-dlp
            ];

            buildInputs =
              with pkgs;
              lib.optionals stdenv.hostPlatform.isLinux [
                pipewire
                dbus
              ];

            nativeBuildInputs =
              with pkgs;
              [ pkg-config ]
              ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [ llvmPackages.libclang ];

            shellHook = ''
              export PKG_CONFIG_PATH="${lib.concatStringsSep ":" pkgConfigPaths}''${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
              ${
                lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
                  export LIBCLANG_PATH="${pkgs.llvmPackages.libclang.lib}/lib"
                  export BINDGEN_EXTRA_CLANG_ARGS="-I${pkgs.glibc.dev}/include"
                ''
              }
              export RUST_BACKTRACE=1
            '';
          };
        }
      );

      nixosModules.default = nixosModule;
      nixosModules.recvrd = nixosModule;
      nixosModules.ytermusicd = nixosModule;
      homeManagerModules.default = homeManagerModule;
      homeManagerModules.recvrd = homeManagerModule;
      homeManagerModules.ytermusicd = homeManagerModule;
    };
}
