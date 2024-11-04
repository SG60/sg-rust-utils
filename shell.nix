{ mkShellNoCC, protobuf, cargo-release, bacon }:

mkShellNoCC {
  packages = [ protobuf cargo-release bacon ];
}
