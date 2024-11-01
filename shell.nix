{ mkShellNoCC, protobuf }:

mkShellNoCC {
  packages = [ protobuf ];
}
