{
  description = "Flake for cd-webhooks-forwarder";

  outputs = { nixpkgs, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        pkgsCross = pkgs.pkgsCross;
        dev-shell-path = ./shell.nix;

        # A more minimal shell for building in CI
        build-only-shell = { mkShellNoCC, protobuf }:
          mkShellNoCC { packages = [ protobuf ]; };

      in {
        devShells = {
          default = pkgs.callPackage build-only-shell { };
          aarch64-unknown-linux-musl =
            pkgsCross.aarch64-multiplatform-musl.callPackage build-only-shell
            { };

          full-dev-shell = pkgs.callPackage dev-shell-path { };
        };
      });
}
