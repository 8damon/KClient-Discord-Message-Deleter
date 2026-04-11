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

append_once() {
  file=$1
  line=$2
  mkdir -p "$(dirname -- "$file")"
  touch "$file"
  if ! grep -Fqx "$line" "$file"; then
    printf '%s\n' "$line" >> "$file"
  fi
}

install_linux_secret_service() {
  if command -v apt-get >/dev/null 2>&1; then
    sudo apt-get update
    sudo apt-get install -y libsecret-tools gnome-keyring dbus-user-session
    return 0
  fi
  if command -v dnf >/dev/null 2>&1; then
    sudo dnf install -y libsecret gnome-keyring dbus-daemon
    return 0
  fi
  if command -v pacman >/dev/null 2>&1; then
    sudo pacman -Sy --noconfirm libsecret gnome-keyring dbus
    return 0
  fi
  if command -v zypper >/dev/null 2>&1; then
    sudo zypper --non-interactive install libsecret-tools gnome-keyring dbus-1
    return 0
  fi
  if command -v apk >/dev/null 2>&1; then
    sudo apk add libsecret gnome-keyring dbus
    return 0
  fi

  printf 'Unsupported Linux package manager. Install libsecret-tools, gnome-keyring, and dbus-user-session manually.\n' >&2
  exit 1
}

configure_shell_path() {
  rc_file=$1
  shell_name=$(basename -- "$shell_path")

  case "$shell_name" in
    fish)
      append_once "$rc_file" "fish_add_path $install_dir"
      ;;
    *)
      append_once "$rc_file" "export PATH=\"$install_dir:\$PATH\""
      ;;
  esac
}

configure_linux_secret_service_startup() {
  rc_file=$1
  shell_name=$(basename -- "$shell_path")

  case "$shell_name" in
    fish)
      append_once "$rc_file" "if type -q gnome-keyring-daemon; and not set -q GNOME_KEYRING_CONTROL; eval (gnome-keyring-daemon --start --components=secrets | string replace -a ';' '' | string replace 'export ' 'set -gx '); end"
      ;;
    *)
      append_once "$rc_file" "if command -v gnome-keyring-daemon >/dev/null 2>&1 && [ -z \"\${GNOME_KEYRING_CONTROL:-}\" ]; then"
      append_once "$rc_file" "  eval \"\$(gnome-keyring-daemon --start --components=secrets)\" >/dev/null"
      append_once "$rc_file" "fi"
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
    install_linux_secret_service
    configure_linux_secret_service_startup "$rc_file"
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
