# Флейк для сборки switch-migrator под NixOS / Nix.
#
# Команды:
#   nix run .                             — запустить TUI
#   nix run . -- ls                       — список накопителей
#   nix develop                           — dev-shell c cargo/clippy
#   nix build .                           — собрать пакет
{
  description = "switch-migrator — перенос данных Nintendo Switch на новую SD-карту";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;

      # Системные утилиты, вызываемые из PATH (никаких зашитых путей).
      # Все они — runtime-зависимости пакета.
      runtimeTools = pkgs: [
        pkgs.util-linux      # sfdisk, wipefs, partprobe, partx, blockdev
        pkgs.dosfstools      # mkfs.vfat
        pkgs.parted          # parted (fallback для разметки)
        pkgs.zstd            # zstd CLI (резервный путь сжатия)
        pkgs.coreutils       # dd, sync
      ] ++ pkgs.lib.optionals (pkgs.stdenv.hostPlatform.isLinux) [
        pkgs.which
      ];

      # Основной Rust-пакет.
      rustPackage = system:
        let pkgs = nixpkgs.legacyPackages.${system}; in
        pkgs.rustPlatform.buildRustPackage {
          pname = "switch-migrator";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;

          # Лечит периодический SIGSEGV rustc при параллельной сборке
          # тяжёлых proc-macro зависимостей (syn и др.) в песочнице Nix.
          RUST_MIN_STACK = "33554432";

          nativeBuildInputs = with pkgs; [ pkg-config ];
          buildInputs = with pkgs; [ zlib openssl ];

          # Юнит-тесты не требуют реальных дисков; в Nix-build запускать
          # необязательно (могут собираться дольше).
          doCheck = true;

          meta = with pkgs.lib; {
            description = "Инструмент миграции SD-карт Nintendo Switch в два клика";
            homepage = "https://gitlab.com/dark_siders/switch-migrator";
            license = licenses.mit;
            mainProgram = "switch-migrator";
            platforms = platforms.linux;
          };
        };

      # Итоговый пакет: бинарь + runtime-утилиты в одном окружении (PATH).
      fullPackage = system:
        let pkgs = nixpkgs.legacyPackages.${system}; in
        pkgs.symlinkJoin {
          name = "switch-migrator-env";
          paths = [ (rustPackage system) ] ++ runtimeTools pkgs;
        };

      forEachSystem = f: nixpkgs.lib.genAttrs systems f;

    in
    {
      packages = forEachSystem (system: {
        default = fullPackage system;
      });

      apps = forEachSystem (system: {
        default = {
          type = "app";
          program = "${fullPackage system}/bin/switch-migrator";
        };
      });

      devShells = forEachSystem (system:
        let pkgs = nixpkgs.legacyPackages.${system}; in
        {
          default = pkgs.mkShell {
            packages = [ pkgs.cargo pkgs.rustc pkgs.clippy pkgs.rustfmt pkgs.pkg-config ]
              ++ runtimeTools pkgs;
            RUST_SRC_PATH = "${pkgs.rustc}/lib/rustlib/src/rust/library";
            shellHook = ''
              echo "dev-shell switch-migrator: cargo build / cargo clippy / cargo test"
            '';
          };
        });
    };
}