# dev-claw

**AI-powered CLI daemon for SDLC automation.**

Stop context-switching to do the non-coding parts of coding. dev-claw brings AI into every phase of your workflow — from a broken build to a shipped release — using whatever LLM provider you already have a key for.

[![CI](https://github.com/akkeshavan/dev-claw/actions/workflows/ci.yml/badge.svg)](https://github.com/akkeshavan/dev-claw/actions/workflows/ci.yml)
[![Latest Release](https://img.shields.io/github/v/release/akkeshavan/dev-claw)](https://github.com/akkeshavan/dev-claw/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

---

## Features

- **14 commands** — doctor, git commit/pr/hook, review diff/pr, forensic explain/blame, env check/guard/hook, deps audit/outdated, mock gen/factory, standup, release notes/cut, cloud up/down/ls/ssh, workflow, memory, config
- **BYOK** — bring your own API key. Supports OpenAI, Anthropic, DeepSeek, Groq, Mistral, Ollama, OpenRouter, and any OpenAI-compatible endpoint
- **Encrypted credentials** — keys stored with AES-256-GCM + Argon2id, never in plain text
- **Per-project memory** — learns your stack and preferences across sessions for better responses
- **Safety-first** — never deletes anything without confirmation; file writes restricted to the current working directory
- **Local-first** — no account, no telemetry, no backend. Usage quota tracked in a local SQLite database

---

## Install

### curl (recommended — macOS and Linux)

```sh
curl -fsSL https://akkeshavan.github.io/dev-claw/install.sh | sh
```

Auto-detects your OS and architecture, verifies SHA-256, installs to `~/.local/bin`.

### Download binary

Download the pre-built binary for your platform from [Releases](https://github.com/akkeshavan/dev-claw/releases/latest):

| Platform | File |
|---|---|
| macOS — Apple Silicon | `dev-claw-vX.Y.Z-aarch64-apple-darwin.tar.gz` |
| macOS — Intel | `dev-claw-vX.Y.Z-x86_64-apple-darwin.tar.gz` |
| Linux x86_64 (static) | `dev-claw-vX.Y.Z-x86_64-unknown-linux-musl.tar.gz` |
| Linux ARM64 (static) | `dev-claw-vX.Y.Z-aarch64-unknown-linux-musl.tar.gz` |
| Windows x86_64 | `dev-claw-vX.Y.Z-x86_64-pc-windows-msvc.zip` |

Extract and copy the `dev-claw` binary to any directory in your `$PATH`.

### Build from source

**Prerequisites:** Rust 1.75+ — install from [rustup.rs](https://rustup.rs)

No other system dependencies are needed. The build uses `rustls` (pure-Rust TLS) and bundles SQLite, so `openssl` and `libsqlite3-dev` are not required.

> **Note:** `dev-claw git commit` and `dev-claw git pr` generate messages using an LLM but delegate the actual git operations to your existing `git` installation. Git must already be configured with your identity and credentials:
> ```sh
> git config --global user.name "Your Name"
> git config --global user.email "you@example.com"
> # For GitHub push: gh auth login   (or SSH keys / osxkeychain)
> ```

```sh
git clone https://github.com/akkeshavan/dev-claw
cd dev-claw
cargo build --release
# Binary is at: target/release/dev-claw
```

To install to `~/.cargo/bin`:

```sh
cargo install --path .
```

To run the test suite:

```sh
cargo test        # 275 tests
cargo clippy      # zero warnings enforced
cargo fmt --check
```

---

## Quick Start

```sh
# 1. Set an API key — pick any provider
dev-claw config set-key --provider deepseek   # cheapest
dev-claw config set-key --provider openai     # most capable
dev-claw config set-key --provider ollama     # local, no key needed

# 2. Initialise a project (generates .devclawrc)
dev-claw init

# 3. Start automating
cargo build 2>&1 | dev-claw doctor
dev-claw git commit
dev-claw standup
```

---

## Commands

### `dev-claw doctor`

Pipe any build log, compiler error, or crash dump. Get root cause + a copy-pasteable fix. No preamble.

```sh
cargo build 2>&1 | dev-claw doctor
npm run build 2>&1 | dev-claw doctor
cat ci-failure.log | dev-claw doctor
```

---

### `dev-claw git`

> dev-claw generates the message; your existing git credentials handle the actual commit and push. Git must be configured with `user.name`/`user.email` before use.

**`commit`** — generate a Conventional Commits message from staged diff.

```sh
git add -p
dev-claw git commit
```

**`pr`** — draft a pull request description from commits since `main`.

```sh
dev-claw git pr
dev-claw git pr --base develop
```

**`hook`** — install a `prepare-commit-msg` hook so commit generation runs automatically.

```sh
dev-claw git hook
```

---

### `dev-claw review`

AI code review: Summary → Findings (Critical / Major / Minor / Nitpick) → Positives.

```sh
dev-claw review diff                      # all uncommitted changes
dev-claw review diff --staged             # only staged changes
dev-claw review diff --focus security     # OWASP Top 10 lens
dev-claw review pr 142                    # GitHub PR by number (needs gh CLI)
```

---

### `dev-claw forensic`

Explain why code exists using `git blame` and LLM analysis.

```sh
dev-claw forensic explain src/auth/token.rs          # explain entire file
dev-claw forensic explain src/auth/token.rs --lines 40-80
dev-claw forensic blame src/auth/token.rs            # annotated blame summary
```

---

### `dev-claw env`

**`check`** — diff `.env` against `.env.example`; report missing, empty, or extra keys.

```sh
dev-claw env check
```

**`guard`** — scan staged changes for secrets about to be committed.

```sh
dev-claw env guard
```

**`hook`** — install pre-commit (`env guard`) and post-checkout (`env check`) git hooks.

```sh
dev-claw env hook
```

---

### `dev-claw deps`

Security audits and outdated-package checks across all detected package managers (Cargo, npm, Go, pip) in one shot.

```sh
dev-claw deps audit              # run cargo audit, npm audit, govulncheck, pip-audit
dev-claw deps audit --triage     # + AI risk ranking: fix now / later / ignore
dev-claw deps outdated           # check for newer versions
```

Auto-detects package managers from: `Cargo.toml`, `package.json`, `go.mod`, `pyproject.toml`, `requirements.txt`.

---

### `dev-claw mock`

Generate realistic fixture data from any schema.

```sh
dev-claw mock gen schema.sql --count 20 --format sql
dev-claw mock gen types.ts --format json --out fixtures.json
dev-claw mock factory src/types/user.ts --out src/__tests__/factories.ts
```

---

### `dev-claw standup`

Generate a daily standup update from git history.

```sh
dev-claw standup
dev-claw standup --since "2 days ago"
dev-claw standup --format slack     # emoji bullets, *bold* headers
dev-claw standup --format markdown
```

---

### `dev-claw release`

**`notes`** — draft release notes from commits since the last tag.

```sh
dev-claw release notes
dev-claw release notes --since v1.1.0
```

**`cut`** — write `CHANGELOG.md` and create an annotated git tag. Prompts for confirmation before writing.

```sh
dev-claw release cut v2.0.0
```

---

### `dev-claw cloud`

Provision and destroy ephemeral VMs across cloud providers.

```sh
dev-claw cloud up --provider aws --region us-east-1 --size small
dev-claw cloud ls
dev-claw cloud ssh my-vm
dev-claw cloud down my-vm        # prompts for confirmation before destroying
```

Supported providers: AWS, GCP, Azure, Fly.io. AWS/GCP/Azure delegate to their CLIs (`aws`, `gcloud`, `az`) — any existing auth (SSO, IAM roles, service accounts) works automatically.

---

### `dev-claw workflow`

Define multi-step pipelines in `.devclawrc`, run them locally, and share via GitHub Gist.

```sh
dev-claw workflow ls
dev-claw workflow run pre-push
dev-claw workflow publish pre-push
dev-claw workflow import gist:abc123def456
```

---

### `dev-claw memory`

Per-project memory that persists across sessions. Context is injected into every LLM call for better answers over time.

```sh
dev-claw memory note "using postgres 16 with pgvector"
dev-claw memory feedback "prefer concise responses"
dev-claw memory ls
dev-claw memory clear            # clears this project only
dev-claw memory clear --global   # clears global SQLite store
```

---

### `dev-claw config` / `dev-claw usage`

```sh
dev-claw config set-key --provider deepseek
dev-claw config list-keys
dev-claw usage                   # token quota for this month
```

---

## Configuration

`dev-claw init` generates a `.devclawrc` starter. Or write one manually:

```toml
[stack]
name            = "typescript-nextjs"
package_manager = "pnpm"

[usage]
monthly_limit   = 200
warn_at_percent = 80

[doctor]
max_log_lines        = 100
auto_ignore_patterns = ["node_modules", ".next", "dist"]

[[workflows]]
name  = "pre-push"
steps = ["env check", "deps audit", "review diff --staged --focus security"]

[[workflows]]
name  = "ship"
steps = ["deps audit --triage", "release notes", "release cut"]
```

dev-claw searches upward from the current directory for `.devclawrc`.

---

## API Providers

dev-claw never stores keys in plain text. All keys are encrypted with AES-256-GCM + Argon2id and saved to `~/dev-claw/creds/` — one file per provider (e.g. `deepseek.enc`, `openai.enc`). On macOS/Linux the directory is locked to `700` and each file to `600` so only your user can read them. You set a master passphrase on first use; subsequent commands prompt for it once per session.

### Supported providers

| Provider | Key name | Best for |
|---|---|---|
| OpenAI | `openai` | Best overall quality |
| Anthropic | `anthropic` | Nuanced reasoning, long context |
| DeepSeek | `deepseek` | Best cost / quality ratio |
| Groq | `groq` | Fastest inference |
| Mistral | `mistral` | EU data residency |
| Ollama | `ollama` | Local inference, no API key needed |
| OpenRouter | `openrouter` | Unified access to 200+ models |
| Custom | `openai-compat` | Self-hosted LLMs, vLLM, LM Studio |

### Per-provider setup

**OpenAI**
1. Get your key at https://platform.openai.com/api-keys
2. `dev-claw config set-key --provider openai`

**Anthropic**
1. Get your key at https://console.anthropic.com/settings/keys
2. `dev-claw config set-key --provider anthropic`

**DeepSeek** *(recommended — best cost/quality)*
1. Get your key at https://platform.deepseek.com/api_keys
2. `dev-claw config set-key --provider deepseek`

**Groq** *(fastest)*
1. Get your key at https://console.groq.com/keys
2. `dev-claw config set-key --provider groq`

**Mistral**
1. Get your key at https://console.mistral.ai/api-keys
2. `dev-claw config set-key --provider mistral`

**OpenRouter** *(access 200+ models with one key)*
1. Get your key at https://openrouter.ai/keys
2. `dev-claw config set-key --provider openrouter`

**Ollama** *(local, no API key required)*
1. Install Ollama from https://ollama.com
2. Pull a model: `ollama pull llama3.2`
3. Set the provider — no key needed:
   ```sh
   DEV_CLAW_PROVIDER=ollama dev-claw doctor
   ```
   Or set it as the default in `.devclawrc`:
   ```toml
   [stack]
   provider = "ollama"
   ```

**Custom / OpenAI-compatible endpoint** *(vLLM, LM Studio, etc.)*
```sh
DEV_CLAW_PROVIDER=openai-compat \
DEV_CLAW_BASE_URL=http://localhost:8000/v1 \
dev-claw git commit
```

### Verify your setup

```sh
dev-claw config list-keys    # show which providers are configured
dev-claw usage               # confirm a call went through this month
```

### Switch provider per command

```sh
DEV_CLAW_PROVIDER=anthropic dev-claw review diff --focus security
DEV_CLAW_PROVIDER=groq dev-claw git commit
```

### CI / scripting

Set `DEV_CLAW_MASTER` to skip the interactive passphrase prompt:

```sh
export DEV_CLAW_MASTER="your master passphrase"
dev-claw standup --format slack
```

---

## Contributing

```sh
git clone https://github.com/akkeshavan/dev-claw
cd dev-claw
cargo build
cargo test        # 275 tests
cargo clippy      # zero warnings enforced
cargo fmt
```

Please keep functions under 40 lines, max 2 levels of nesting, and add tests for new behaviour.

---

## License

[MIT](LICENSE) — Copyright (c) 2024 Anand Keshavan
