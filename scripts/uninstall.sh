#!/usr/bin/env sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
home_dir="${HOME:-}"
install_dir="$home_dir/.local/bin"
removed_any=0

run_cleanup() {
  candidate=$1
  if [ -x "$candidate" ]; then
    "$candidate" --uninstall || printf 'Warning: uninstall cleanup via %s failed\n' "$candidate" >&2
    return 0
  fi
  return 1
}

remove_path() {
  path=$1
  if [ -e "$path" ]; then
    rm -rf "$path"
    printf 'Removed %s\n' "$path"
    removed_any=1
  fi
}

run_cleanup "$install_dir/kclient" || run_cleanup "$repo_root/target/release/kclient" || true

remove_path "$repo_root/kcordclient"

case "$(uname -s)" in
  Darwin)
    if [ -n "$home_dir" ]; then
      remove_path "$home_dir/Library/Application Support/kcordclient"
    fi
    ;;
  *)
    if [ -n "${XDG_DATA_HOME:-}" ]; then
      remove_path "$XDG_DATA_HOME/kcordclient"
    elif [ -n "$home_dir" ]; then
      remove_path "$home_dir/.local/share/kcordclient"
    fi
    ;;
esac

for binary in \
  "$install_dir/kclient" \
  "$install_dir/kcordclient" \
  "$install_dir/keroklient"
do
  if [ -e "$binary" ]; then
    rm -f "$binary"
    printf 'Removed %s\n' "$binary"
    removed_any=1
  fi
done

if [ "$removed_any" -eq 0 ]; then
  printf 'No local kcordclient data or installed binaries were found.\n'
fi
