#!/usr/bin/env sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
binary_source="$repo_root/target/release/kclient"
home_dir="${HOME:-}"
install_dir="$home_dir/.local/bin"
install_path="$install_dir/kclient"
shell_path="${SHELL:-/bin/sh}"

detect_rc_file() {
  shell_name=$(basename -- "$shell_path")
  case "$shell_name" in
    zsh) printf '%s\n' "$home_dir/.zshrc" ;;
    bash) printf '%s\n' "$home_dir/.bashrc" ;;
    fish) printf '%s\n' "$home_dir/.config/fish/config.fish" ;;
    *) printf '%s\n' "$home_dir/.profile" ;;
  esac
}

replace_block() {
  file=$1
  marker=$2
  content=$3
  tmp_file="${file}.kclient.tmp"
  start="# >>> kclient $marker >>>"
  end="# <<< kclient $marker <<<"

  mkdir -p "$(dirname -- "$file")"
  touch "$file"

  awk -v start="$start" -v end="$end" '
    $0 == start { skip = 1; next }
    $0 == end { skip = 0; next }
    !skip { print }
  ' "$file" > "$tmp_file"

  {
    cat "$tmp_file"
    printf '%s\n' "$start"
    printf '%s\n' "$content"
    printf '%s\n' "$end"
  } > "$file"

  rm -f "$tmp_file"
}

configure_shell_path() {
  rc_file=$1
  shell_name=$(basename -- "$shell_path")

  case "$shell_name" in
    fish)
      replace_block "$rc_file" "path" "fish_add_path $install_dir"
      ;;
    *)
      replace_block "$rc_file" "path" "export PATH=\"$install_dir:\$PATH\""
      ;;
  esac
}

reload_shell() {
  if [ -t 0 ] && [ -t 1 ] && [ -n "${SHELL:-}" ]; then
    printf 'Reloading %s as a login shell so PATH changes apply now...\n' "$SHELL"
    exec "$SHELL" -l
  fi
}

if [ -z "$home_dir" ]; then
  printf 'HOME is not set.\n' >&2
  exit 1
fi

rc_file=$(detect_rc_file)

case "$(uname -s)" in
  Linux)
    printf 'Linux install uses local token storage with filesystem permissions only.\n'
    ;;
  Darwin)
    printf 'macOS secure storage uses the built-in Keychain command-line tools.\n'
    ;;
  *)
    printf 'install.sh only supports Linux and macOS.\n' >&2
    exit 1
    ;;
esac

configure_shell_path "$rc_file"

cd "$repo_root"
cargo build --release --bin kclient

if [ ! -f "$binary_source" ]; then
  printf 'Built binary not found at %s\n' "$binary_source" >&2
  exit 1
fi

mkdir -p "$install_dir"
install -m 755 "$binary_source" "$install_path"

printf 'Installed %s\n' "$install_path"
printf 'Updated shell config: %s\n' "$rc_file"
reload_shell
