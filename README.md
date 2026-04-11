# KCLIENT

## OVERVIEW

`kclient` is a CLI for deleting your own Discord messages from a DM, a single channel, or every text channel in a server. It supports stored accounts, checkpointed runs, proxy rotation, rate-limit visibility, and plain command-line workflows without a web UI or `.env` file.

## FEATURES

- CLI-only workflow
- Stored account add and remove commands
- One-off runs with `--token`
- Server-wide discovery with checkpointing
- Timeframe filtering with values like `30m`, `24h`, and `7d`
- Proxy rotation with `proxy.txt`
- Local logs for each run

## PLATFORM SUPPORT

- Windows is the primary verified platform in this workspace.
- Linux and macOS code paths are present for data directories and uninstall scripts.
- Only the Windows Rust target was installed in this environment, so Linux and macOS were reviewed for cfg correctness but not cross-compiled here.

## BUILD

```bash
cargo build --release --bin kclient
```

The release binary is written to `target/release/kclient.exe` on Windows and `target/release/kclient` on Linux and macOS.

## ACCOUNT MANAGEMENT

Store an account:

```bash
kclient --add-account --token TOKEN_HERE
```

Remove an account:

```bash
kclient --remove-account --account myuser
```

Run once without storing the token:

```bash
kclient --token TOKEN_HERE --delete --channel 123456789012345678 --tf 24h
```

## USAGE EXAMPLES

Delete the last 24 hours from one channel:

```bash
kclient --account myuser --delete --channel 680459914828972076 --tf 24h
```

Delete everything from one DM:

```bash
kclient --account myuser --delete --dm 123456789012345678 --all
```

Delete everything you authored across a server:

```bash
kclient --account myuser --delete --server 123456789012345678 --all
```

Start fresh and ignore old checkpoints:

```bash
kclient --account myuser --delete --server 123456789012345678 --tf 24h --reload
```

Use proxies from `proxy.txt`:

```bash
kclient --account myuser --delete --server 123456789012345678 --all --dproxy
```

## PROXY FORMAT

`proxy.txt` expects one proxy per line:

```text
ip:port:username:password
```

Empty lines and lines starting with `#` are ignored.

## DATA AND LOGS

Application data is stored under the `kcordclient` app directory.

- Windows: `%APPDATA%\kcordclient`
- Linux: `$XDG_DATA_HOME/kcordclient` or `~/.local/share/kcordclient`
- macOS: `~/Library/Application Support/kcordclient`

Run logs are written to the `logs` subdirectory inside that app directory.

## TIPS

- `--tf` and `--all` are mutually exclusive.
- `--server`, `--channel`, and `--dm` are mutually exclusive.
- If you do not pass `--token`, `kclient` uses a stored account.
- If multiple accounts are stored, pass `--account`.
- `--reload` clears saved checkpoints before starting a run.

## UNINSTALL

Windows:

```powershell
./scripts/uninstall.ps1
```

Linux or macOS:

```bash
./scripts/uninstall.sh
```
