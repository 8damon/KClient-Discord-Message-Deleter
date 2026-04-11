#!/usr/bin/env sh
set -eu

if [ -n "${XDG_DATA_HOME:-}" ]; then
  app_dir="$XDG_DATA_HOME/kcordclient"
elif [ -n "${HOME:-}" ]; then
  app_dir="$HOME/.local/share/kcordclient"
else
  app_dir="./kcordclient"
fi

if [ -d "$app_dir" ]; then
  rm -rf "$app_dir"
  printf 'Removed %s\n' "$app_dir"
else
  printf 'No local kcordclient data found at %s\n' "$app_dir"
fi

printf 'Delete kcordclient / kclient from their install location if you no longer want the binaries.\n'
