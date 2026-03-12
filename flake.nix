{
  description = "Small but mighty dynamic function row daemon";
  inputs = { nixpkgs.url = "github:nixos/nixpkgs/nixos-25.11"; };
  outputs = { self, nixpkgs }:
    let
      supportedSystems = [ "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
      pkgsFor = forAllSystems (system: import nixpkgs { inherit system; });
    in rec {
      packages = forAllSystems (system:
        let pkgs = pkgsFor.${system};
        in {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "smol-dfr";
            version = "0.1.0";
            src = ./.;
            cargoLock = { lockFile = ./Cargo.lock; };
            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [
              pkgs.libinput
              pkgs.fontconfig
              pkgs.libpulseaudio
            ];

            postConfigure = ''
              substituteInPlace etc/systemd/system/smol-dfr.service \
                  --replace-fail /usr/bin $out/bin
              substituteInPlace src/*.rs --replace-quiet /usr/share $out/share
            '';

            postInstall = ''
              cp -R etc $out/lib
              cp -R share $out
            '';
          };
        });

      devShells = forAllSystems (system:
        let pkgs = pkgsFor.${system};
        in {
          default = pkgs.mkShell {
            inputsFrom = [ packages.${system}.default ];
            packages = [ pkgs.rustfmt pkgs.rust-analyzer ];
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          };
        });
    };
}
