# dev-claw

**AI-powered CLI daemon for SDLC automation.**  
Stop context-switching to do the non-coding parts of coding.

dev-claw automates the chores that surround writing software — commit messages, code review, dependency audits, standup updates, release notes, cloud VMs, and more — using whatever AI provider you already have a key for.

- **BYOK** — bring your own API key (DeepSeek, OpenAI, Claude, Sarvam, Mistral, Ollama). Keys are stored in `~/dev-claw/creds/` as AES-256-GCM encrypted files, never in plain config files.
- **Local-first** — no account, no telemetry, no backend. Usage quota tracked in a local SQLite database.
- **Composable** — chain commands into shareable workflows.

---

## Install

**From source** (requires Rust 1.75+):

```sh
git clone https://github.com/yourusername/dev-claw
cd dev-claw
cargo install --path .
```

Pre-built binaries for macOS, Linux, and Windows coming soon.

---

## Quick start

```sh
# 1. Store an API key (encrypted in ~/dev-claw/creds/ — prompts for a master passphrase on first use)
dev-claw config set-key --provider deepseek

# 2. Detect your stack and generate a .devclawrc
dev-claw init

# 3. Done. Try something:
dev-claw standup
```

---

## Commands

### `dev-claw doctor`
Diagnose build errors. Pipe stderr from any tool and get an explanation + fix.

```sh
cargo build 2>&1 | dev-claw doctor
npm run build 2>&1 | dev-claw doctor
```

---

### `dev-claw git`

**`commit`** — scan staged changes and generate a Conventional Commits message.

```sh
git add -p
dev-claw git commit
```

**`pr`** — draft a pull request description from commits since `main`.

```sh
dev-claw git pr
dev-claw git pr --base develop
```

**`hook`** — install a `prepare-commit-msg` hook so commit message generation runs automatically.

```sh
dev-claw git hook
```

---

### `dev-claw review`

AI code review with structured output: Summary → Findings (🔴 Critical / 🟠 Major / 🟡 Minor / ⚪ Nitpick) → Positives.

```sh
dev-claw review diff                          # review all uncommitted changes
dev-claw review diff --staged                 # review only staged changes
dev-claw review diff --focus security         # security lens (OWASP Top 10)
dev-claw review diff --focus performance      # performance lens
dev-claw review pr 142                        # review GitHub PR #142 (needs gh CLI)
dev-claw review pr 142 --focus style
```

---

### `dev-claw forensic`

Explain why code exists using `git blame` and LLM analysis.

```sh
dev-claw forensic explain src/auth/token.rs          # explain entire file
dev-claw forensic explain src/auth/token.rs --lines 40-80  # specific line range
dev-claw forensic blame src/auth/token.rs            # show annotated blame summary
```

---

### `dev-claw env`

**`check`** — diff `.env` against `.env.example` and report missing, empty, or extra keys.

```sh
dev-claw env check
```

**`guard`** — scan staged changes for secrets about to be committed. Blocks commit if found.

```sh
dev-claw env guard
```

**`hook`** — install pre-commit (`env guard`) and post-checkout (`env check`) hooks.

```sh
dev-claw env hook
```

---

### `dev-claw deps`

**`audit`** — run security audits across all detected package managers (`cargo audit`, `npm audit`, `govulncheck`, `pip-audit`). Add `--triage` to get an LLM risk ranking.

```sh
dev-claw deps audit
dev-claw deps audit --triage
```

**`outdated`** — check for outdated packages across all detected package managers.

```sh
dev-claw deps outdated
```

Auto-detects package managers from: `Cargo.toml`, `package.json`, `go.mod`, `pyproject.toml`, `requirements.txt`.

---

### `dev-claw mock`

Generate realistic fixture data from any schema.

**`gen`** — generate test records from a SQL DDL, TypeScript types, JSON Schema, Protobuf, or any schema file.

```sh
dev-claw mock gen schema.sql --count 20 --format sql
dev-claw mock gen types.ts --format json --out fixtures.json
dev-claw mock gen schema.proto --format csv
```

Supported formats: `json`, `ts`, `sql`, `csv`.

**`factory`** — generate TypeScript factory functions from a types file.

```sh
dev-claw mock factory src/types/user.ts --out src/__tests__/factories.ts
```

---

### `dev-claw standup`

Generate a daily standup update from your recent git commits and active branches.

```sh
dev-claw standup
dev-claw standup --since "2 days ago"
dev-claw standup --format slack       # emoji bullets, *bold* headers
dev-claw standup --format markdown    # ## sections
dev-claw standup --format plain       # default, no markup
```

---

### `dev-claw release`

**`notes`** — draft polished, user-facing release notes from commits since the last tag.

```sh
dev-claw release notes
dev-claw release notes --since v1.1.0   # compare from a specific ref
```

**`cut`** — suggest a semver bump, draft release notes, prepend `CHANGELOG.md`, and create an annotated git tag.

```sh
dev-claw release cut
dev-claw release cut --version v2.0.0   # override the suggested version
dev-claw release cut --dry-run          # preview without writing anything
```

---

### `dev-claw cloud`

Provision and destroy ephemeral VMs across cloud providers.

**`up`** — spin up a VM.

```sh
dev-claw cloud up --provider do                        # DigitalOcean
dev-claw cloud up --provider hetzner --size cx21
dev-claw cloud up --provider aws --region us-west-2
dev-claw cloud up --provider azure
dev-claw cloud up --provider gcp --region us-central1-a
```

**`ls`** — list all VMs provisioned by dev-claw.

```sh
dev-claw cloud ls
```

**`ssh`** — SSH into a VM by name or ID.

```sh
dev-claw cloud ssh claw-a3f7c2
```

**`down`** — destroy a VM.

```sh
dev-claw cloud down claw-a3f7c2
```

DigitalOcean and Hetzner use API tokens stored in the encrypted credential vault (`do-token` and `hetzner-token`). AWS, Azure, and GCP delegate to their CLIs (`aws`, `az`, `gcloud`) — any existing auth method (SSO, IAM roles, service accounts) works automatically.

---

### `dev-claw workflow`

Define multi-step pipelines in `.devclawrc`, run them locally, and share them as GitHub Gists.

**`ls`** — list all workflows (project + global).

```sh
dev-claw workflow ls
```

**`run`** — execute a workflow step by step. Stops on the first failure.

```sh
dev-claw workflow run pre-push
dev-claw workflow run pre-push --dry-run   # preview steps without running
```

**`publish`** — publish a workflow as a public GitHub Gist (requires `gh` CLI).

```sh
dev-claw workflow publish pre-push
# → dev-claw workflow import https://gist.github.com/...
```

**`import`** — import a workflow from a GitHub Gist URL or any raw TOML URL.

```sh
dev-claw workflow import https://gist.github.com/user/abc123
```

---

### `dev-claw usage`

Show how much of the free-tier quota has been used this month.

```sh
dev-claw usage
```

---

## Configuration

Run `dev-claw init` to generate a `.devclawrc` starter, or write one manually:

```toml
[stack]
name            = "typescript-nextjs"
package_manager = "pnpm"

[usage]
monthly_limit   = 200   # total LLM calls per month
warn_at_percent = 80    # warn when 80% used

[doctor]
max_log_lines          = 100
auto_ignore_patterns   = ["node_modules", ".next", "dist"]

[[workflows]]
name        = "pre-push"
description = "Security gates before pushing"
steps = [
  "env check",
  "deps audit",
  "review diff --staged --focus security",
]

[[workflows]]
name  = "ship"
steps = ["deps audit --triage", "release notes", "release cut"]
```

dev-claw searches upward from the current directory for `.devclawrc`, so a repo-root file covers the whole project.

---

## API providers

### Supported providers

| Provider | Notes |
|---|---|
| `deepseek` | Cheapest per-token, recommended default |
| `openai` | GPT-4o-mini |
| `claude` | Claude Haiku (Anthropic) |
| `sarvam` | Indian-language specialist (Sarvam AI) |
| `mistral` | mistral-small-latest |
| `ollama` | Local inference, fully offline, no key needed |

### Credential store

Keys are stored in **`~/dev-claw/creds/`** as AES-256-GCM encrypted files — one file per provider. No OS keychain is used; encryption relies entirely on a master passphrase you set on first use.

**Key derivation**: Argon2id (64 MB memory, 3 iterations) — deliberately slow to resist brute-force if the files are ever exfiltrated.

**File layout:**
```
~/dev-claw/creds/
  .salt           # 32-byte random salt, written once
  deepseek.enc    # 12-byte nonce + AES-256-GCM ciphertext
  openai.enc
  claude.enc
  sarvam.enc
  mistral.enc
```

### Setup

```sh
# First call creates the vault and prompts to set a master passphrase
dev-claw config set-key --provider deepseek

# Add more providers — prompts for master passphrase once per session
dev-claw config set-key --provider mistral
dev-claw config set-key --provider sarvam

# See what's stored
dev-claw config list-keys
```

On first run, you are prompted to create and confirm a master passphrase. Subsequent commands prompt for it once per process invocation — the derived key is cached in memory for the session.

### CI / scripting

Set `DEV_CLAW_MASTER` to skip the interactive prompt:

```sh
export DEV_CLAW_MASTER="my passphrase"
dev-claw standup --format slack
```

### Per-command provider override

```sh
DEV_CLAW_PROVIDER=claude dev-claw review diff --focus security
DEV_CLAW_PROVIDER=mistral dev-claw git commit
```

### Cloud provider tokens

DigitalOcean and Hetzner API tokens are stored in the same encrypted vault:

```sh
dev-claw config set-key --provider do-token
dev-claw config set-key --provider hetzner-token
```

AWS, Azure, and GCP delegate to their CLIs (`aws`, `az`, `gcloud`) — any existing auth (SSO, IAM roles, service accounts) works automatically.

---

## Workflow recipes

**Pre-push gate:**
```toml
[[workflows]]
name  = "pre-push"
steps = ["env check", "deps audit", "review diff --staged --focus security"]
```

**Daily routine:**
```toml
[[workflows]]
name  = "morning"
steps = ["standup --format slack", "deps outdated"]
```

**Release pipeline:**
```toml
[[workflows]]
name  = "release"
steps = ["deps audit --triage", "release cut"]
```

---

## Contributing

```sh
git clone https://github.com/yourusername/dev-claw
cd dev-claw
cargo build
cargo test       # 247 tests
cargo clippy     # zero warnings enforced
```

Pull requests welcome. Please keep functions under 40 lines and add tests for new behaviour.

---

## License

MIT
