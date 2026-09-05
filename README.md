# s3-tui

A terminal UI manager for AWS S3. Browse buckets, navigate folders, upload and download files with metadata, and manage objects — all from your terminal.

**License:** MIT

---

## Features

- **Multi-panel navigation** — buckets list, object browser, detail/preview pane.
- **File picker** — select one or many files locally; add S3 metadata (key/value pairs) per file before uploading.
- **Batch operations** — download and delete multiple objects at once.
- **Metadata editor** — define custom key/value pairs in a spreadsheet-style editor; rows chain automatically so several metadata can be typed in sequence.
- **Object info** — inspect size, storage class, last modified, ETag, content type, and custom metadata.
- **SQLite cache** — listings are cached locally; subsequent openings are instant; only changed listings are refreshed from S3.
- **Filter** — type-ahead filter for the current object list.
- **LocalStack / S3-compatible** — connect to any S3-compatible endpoint (LocalStack, MinIO, MiniStack etc.) with path-style or virtual-hosted-style URLs.

---

## Installation

### From source (requires Rust 1.85+)

```bash
cargo install --path .
```

### Build manually

```bash
git clone https://github.com/idcmardelplata/s3_tui && cd s3-tui
cargo build --release
# binary is at ./target/release/s3-tui
```

---

## Quick start

```bash
# Using static credentials against a LocalStack endpoint
s3-tui --region us-east-1 \
       --endpoint http://localhost:4566 \
       --access-key test --secret-key test

# Using the standard AWS credential chain (env, ~/.aws/credentials, IAM role)
s3-tui --region us-east-1

# Using a named profile
s3-tui --profile my-profile
```

On first run a default config file is created at `$HOME/.config/s3-tui/config.toml`.

---

## Configuration

Config file location: `$HOME/.config/s3-tui/config.toml`

```toml
# AWS region (override: AWS_REGION env var)
region = "us-east-1"

# S3 endpoint (override: S3_ENDPOINT env var)
# Most useful with LocalStack / MinIO / any S3-compatible service
endpoint = "http://pi:4566"

# Path-style URLs (bucket in path, not subdomain).
# Defaults to true when an endpoint is set; set false for virtual-hosted-style.
force_path_style = true

# Named profile from ~/.aws/credentials (override: AWS_PROFILE env var)
#profile = "localstack"

# Static credentials (override: AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY env vars)
# When omitted, the standard AWS credential chain is used.
# [credentials]
# access_key_id = "test"
# secret_access_key = "test"
# session_token = ""

[ui]
# Theme ID — override at runtime with S3TUI_THEME; NO_COLOR disables colours entirely.
# Built-in themes: catppuccin, dracula, gruvbox, nord, one-dark, solarized,
#                  tailwind, tokyo-night, rose-pine, terminal
theme = "catppuccin"
```

An example config is also shipped as `config.toml.example`.

### Configuration precedence

```
CLI flags > environment variables > config file > defaults
```

---

## CLI options

```
s3-tui [OPTIONS]
```

| Flag | Env var | Description |
| --- | --- | --- |
| `--region <REGION>` | `AWS_REGION`, `AWS_DEFAULT_REGION` | AWS region |
| `--endpoint <URL>` | `S3_ENDPOINT` | S3 endpoint URL |
| `--profile <NAME>` | `AWS_PROFILE` | Named AWS profile |
| `--access-key <ACCESS_KEY>` | `AWS_ACCESS_KEY_ID` | Static access key |
| `--secret-key <SECRET_KEY>` | `AWS_SECRET_ACCESS_KEY` | Static secret key |
| `--session-token <TOKEN>` | `AWS_SESSION_TOKEN` | Session token for static credentials |
| `--force-path-style` | | Force path-style URLs |
| `--no-path-style` | | Disable path-style URLs |

---

## Key bindings

### Main panels (Buckets / Objects)

| Key | Action |
| --- | --- |
| `↑` / `k` | Move up |
| `↓` / `j` | Move down |
| `Enter` | Open bucket / enter folder |
| `←` / `h` | Go back to parent |
| `r` | Refresh (re-list from S3) |
| `/` | Filter objects |
| `i` | Object info |
| `u` | Upload files (opens file picker) |
| `g` | Download object or selected objects |
| `d` | Delete object or selected objects |
| `Space` | Toggle selection |
| `a` | Select all visible objects |
| `c` | Clear selection |
| `?` | Show help |
| `q` | Quit |

### File picker

| Key | Action |
| --- | --- |
| `↑` / `k` / `↓` / `j` | Navigate |
| `Enter` / `→` | Enter folder or toggle selection on a file |
| `←` / `Backspace` | Go to parent directory |
| `Space` | Toggle file selection |
| `a` | Select all visible |
| `c` | Clear selection |
| `u` | Proceed to metadata editor |
| Any char | Type to filter |
| `Esc` | Cancel (clear filter first if active) |

### Metadata editor

| Key | Action |
| --- | --- |
| `↑` / `↓` | Move between rows |
| `Tab` | Switch between key/value field |
| `Enter` | Commit current cell; move to next field or next row |
| `A` | Append new empty row |
| `D` / `Delete` | Delete current row |
| `N` / `BackTab` | Switch to next file |
| `P` | Switch to previous file |
| `U` | Upload all files with their metadata |
| `Esc` | Back to file picker (clear buffer first if typing) |
| Any char | Type into current cell |

### Confirmation prompt

| Key | Action |
| --- | --- |
| `y` | Confirm |
| `n` / `N` / `Esc` | Cancel |

---

## Theme

Set via the `[ui] theme` field in the config file, or at runtime with:

```bash
S3TUI_THEME=dracula s3-tui
```

Disable colours entirely:

```bash
NO_COLOR=1 s3-tui
```

Built-in themes: `catppuccin`, `dracula`, `gruvbox`, `nord`, `one-dark`, `solarized`, `tailwind`, `tokyo-night`, `rose-pine`, `terminal`.

---

## SQLite cache

Listings are cached in an SQLite database at `$HOME/.cache/s3-tui/cache.db`. On the first open of a folder the listing is fetched from S3; subsequent opens load from the local cache instantly. Press `r` to force a refresh from S3 — the cache is updated only when the remote listing differs. No internet connection is needed after the initial load of a folder you've already visited.

---

## License

MIT — see [LICENSE](LICENSE) (if present) or the `license` field in `Cargo.toml`.
