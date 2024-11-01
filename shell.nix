{ mkShellNoCC, protobuf, cargo-release }:

mkShellNoCC {
  packages = [ protobuf cargo-release ];
}
