#!/usr/bin/env sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
binary_source="$repo_root/target/release/kclient"
install_dir="${HOME}/.local/bin"
install_path="$install_dir/kclient"

cd "$repo_root"
cargo build --release --bin kclient

if [ ! -f "$binary_source" ]; then
  printf 'Built binary not found at %s\n' "$binary_source" >&2
  exit 1
fi

mkdir -p "$install_dir"
install -m 755 "$binary_source" "$install_path"

printf 'Installed %s\n' "$install_path"
printf "If '%s' is not on PATH yet, add it and open a new terminal.\n" "$install_dir"
