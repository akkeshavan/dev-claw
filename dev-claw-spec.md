# Product Specification: Dev-Claw (The AI-Powered Developer Daemon)

**Version:** 1.1.0-draft  
**Author:** Anand Kumar Keshavan  
**Status:** Open-Source Core (BYOK) Spec — Cross-Platform with Freemium Tier  

---

## 1. Executive Summary

**Dev-Claw** is an open-source, local background CLI daemon (`dev-clawd`) and terminal frontend (`dev-claw`) engineered to automate routine Software Development Lifecycle (SDLC) drudgery.

Unlike generalist AI coding agents that consume heavy context to generate full features, Dev-Claw operates as a surgical, low-latency "utility layer." It handles terminal error diagnostics, Git pre-push audits, `.env` secret validation, legacy code forensics, and automated local/cloud staging workflows using a **Bring Your Own Key (BYOK)** model.

Dev-Claw runs natively on **macOS, Windows, and Linux** with platform-specific installers for each. The initial release ships as a **free tier** with a generous monthly usage quota (configurable per deployment). A **Pro tier** with unlimited usage and team features is on the roadmap.

---

## 2. Core Architecture & Security Model

Dev-Claw operates entirely on the developer's local machine with zero mandatory external SaaS dependencies for core execution.

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           Developer's Machine                           │
│                                                                         │
│   ┌──────────────────┐               ┌──────────────────────────────┐   │
│   │ CLI / Background ├──────────────►│  OS Native Keychain          │   │
│   │ Daemon (dev-claw)│               │  (MacOS Keychain / Secret    │   │
│   └────────┬─────────┘               │   Service / Credential Mgr)  │   │
│            │                         └──────────────┬───────────────┘   │
│            │                                        │                   │
│            │ Direct HTTPS (User's API Key)          │ Local Token Read  │
│            ▼                                        ▼                   │
│   ┌──────────────────┐               ┌──────────────────────────────┐   │
│   │ LLM API          │               │ Cloud Providers              │   │
│   │ (DeepSeek /      │               │ (Hetzner / DigitalOcean)     │   │
│   │  OpenAI / Claude)│               └──────────────────────────────┘   │
│   └──────────────────┘                                                  │
└─────────────────────────────────────────────────────────────────────────┘
```

### 2.1 Principles

1. **Local Security:** API keys (`sk-deepseek-...`, `sk-proj-...`) and Cloud Personal Access Tokens are stored exclusively in the host OS Keychain using native bindings (`keyring`). Keys are never transmitted to any third-party telemetry server.
2. **Zero Runtime Dependencies:** The CLI binary is compiled natively (Rust or Go) to ensure sub-100ms startup times and zero runtime overhead (no Node.js/Python environment required).
3. **Pluggable Providers:** Out-of-the-box support for DeepSeek (V4 Flash / V4 Pro), Anthropic (Claude 3.5 Haiku/Sonnet), OpenAI (GPT-4o-mini), and local offline models via Ollama (`http://localhost:11434`).

### 2.2 Cross-Platform Support

Dev-Claw is compiled as a single static binary with no runtime dependencies on all three major platforms.

| Platform | Keychain Backend | Config Path | Shell Integration |
|---|---|---|---|
| macOS | macOS Keychain (`security` CLI) | `~/.config/dev-claw/` | zsh / bash / fish |
| Linux | libsecret / KWallet / plain-file fallback | `~/.config/dev-claw/` | bash / zsh / fish |
| Windows | Windows Credential Manager (`wincred`) | `%APPDATA%\dev-claw\` | PowerShell / cmd / Git Bash |

**Platform-Specific Installers:**

- **macOS:** Homebrew formula (`brew install dev-claw`) + `.pkg` installer + 1-line `curl` shell script.
- **Linux:** `.deb` (Debian/Ubuntu), `.rpm` (Fedora/RHEL), `.tar.gz` (universal), AUR package (Arch), 1-line `curl` shell script.
- **Windows:** `.msi` installer (via WiX Toolset), `winget` package (`winget install dev-claw`), `scoop` bucket entry.

All installers add `dev-claw` to `PATH` automatically and register the `dev-clawd` background daemon as an OS service (launchd on macOS, systemd on Linux, Windows Service on Windows).

---

## 3. Command Suite & Feature Matrix

Dev-Claw organizes its capabilities into specialized functional modules.

### 3.1 `dev-claw doctor` (Terminal Error & Crash Explainer)

- **Execution:** Accepts piped `stderr` output (e.g., `npm run dev | dev-claw doctor` or `cargo build | dev-claw doctor`).
- **Function:** Strips stack-trace noise, identifies the exact failing source code line, and outputs a 2-line root cause with a suggested terminal command or code patch.

### 3.2 `dev-claw git` (Pre-Commit & PR Guardian)

- **Execution:** Invoked directly or via Git hooks (`.git/hooks/pre-commit`).
- **Function:**
  - Scans uncommitted diffs for leftover debug markers (`console.log`, `print()`, `debugger`, `TODO`).
  - Generates conventional commit messages (`feat:`, `fix:`, `chore:`) from staged code.
  - Auto-drafts structured Markdown PR specs grouped by features, breaking changes, and tested cases.

### 3.3 `dev-claw env` (Config & Secret Shield)

- **Execution:** Triggers automatically on branch switch or repo initialization.
- **Function:** Compares local `.env` files against repository templates (`.env.example`), identifies missing variables, validates type mismatches, and prevents accidental commits containing API keys or credentials.

### 3.4 `dev-claw forensic` (Legacy Code Explainer)

- **Execution:** `dev-claw forensic <file>:<line-range>`.
- **Function:** Integrates local `git blame` history with file context to output a concise 3-bullet breakdown: **Why** the code was written, **what** edge case it addresses, and **risks** involved in refactoring it.

### 3.5 `dev-claw mock` (Synthetic Fixture Generator)

- **Execution:** `dev-claw mock --schema src/types/user.ts --count 20 > users.json`.
- **Function:** Reads TypeScript interfaces, GraphQL schemas, or SQL files and generates valid, mock JSON or SQL seed data instantly.

### 3.6 `dev-claw cloud` (Infrastructure Orchestration)

- **Execution:** `dev-claw cloud spin --provider hetzner --ttl 2h`.
- **Function:** Uses local DigitalOcean or Hetzner API tokens to provision ephemeral staging VPS instances, run remote test suites, return logs, and automatically execute teardown on expiration.

---

## 4. Stack Customization & Configuration (`.devclawrc`)

Stack profiles auto-initialize via `dev-claw init`. The CLI auto-detects repository indicators (`package.json`, `Cargo.toml`, `go.mod`, `requirements.txt`) and applies contextual LLM system prompts and linter constraints.

```toml
# Example .devclawrc (TOML Format)

[stack]
name = "typescript-nextjs"
package_manager = "pnpm"

[standards]
coding_style = """
- Always use functional components with explicitly typed props interfaces.
- Do not use 'any' under any circumstances; prefer 'unknown'.
- Use Tailwind CSS classes for styling.
- Ensure all asynchronous API calls include explicit try-catch error handling.
"""

[git]
commit_style = "conventional"
block_on_keywords = ["console.log", "debugger", "FIXME", "INTERNAL_SECRET"]

[doctor]
auto_ignore_patterns = ["node_modules", ".next", "dist"]
max_log_lines = 100

[usage]
# Free tier monthly limits — configurable per self-hosted deployment.
# Defaults are intentionally generous; set to 0 for unlimited (Pro/BYOK mode).
monthly_limit         = 200   # total AI invocations per calendar month
doctor_limit          = 75    # dev-claw doctor calls
git_limit             = 60    # dev-claw git calls (commit gen + PR draft)
env_limit             = 20    # dev-claw env scans
forensic_limit        = 25    # dev-claw forensic calls
mock_limit            = 15    # dev-claw mock generations
cloud_limit           = 5     # dev-claw cloud spin operations
warn_at_percent       = 80    # show warning banner when this % of quota is used
reset_day             = 1     # day of month quotas reset (1 = first of month)
```

---

## 5. Product & Business Roadmap

```
Phase 1: Open Core (Free) ──► Phase 2: Community Scale ──► Phase 3: Pro/Team Monetization
```

### Phase 1: Free Tier Launch (Current)

- **Price:** $0
- **Model:** BYOK — users supply their own LLM API keys (DeepSeek, OpenAI, Ollama). Dev-Claw handles the plumbing.
- **Usage Quota:** 200 AI invocations per month across all commands (see `[usage]` config block). Defaults are generous. Self-hosters can set `monthly_limit = 0` for unlimited.
- **Quota Enforcement:** Tracked locally in `~/.config/dev-claw/usage.db` (SQLite). No phone-home. When a user hits 80% of quota, a non-blocking banner appears in the terminal. At 100%, commands soft-fail with a clear message and a link to the download/upgrade page.
- **Distribution:**
  - macOS: Homebrew, `.pkg`, `curl` script
  - Linux: `.deb`, `.rpm`, `.tar.gz`, AUR, `curl` script
  - Windows: `.msi`, `winget`, `scoop`
  - Source: MIT-licensed GitHub repository

### Phase 2: Community & Plugin Ecosystem

- **Features:** Community-submitted stack profiles, custom team skill definitions, and local IDE extensions (VS Code / JetBrains terminal bindings).

### Phase 3: Pro Tier & SaaS Monetization (Future)

- **Pro Tier ($20/year or $2.25/month):**
  - Unlimited AI invocations (no monthly quota).
  - Automated cross-machine rule and skill sync via Cloudflare Workers.
  - Optional managed backend proxy — bundled LLM access with no personal API key required.
  - Priority model access (latest DeepSeek / Claude versions).

- **Payment Gateway — Regional Routing:**
  - **Outside India:** [**Stripe**](https://stripe.com) — handles USD, EUR, GBP, and all major international cards. Stripe Tax manages US sales tax and EU VAT automatically.
  - **India:** **UPI** (via Razorpay or Cashfree) — supports all UPI apps (GPay, PhonePe, Paytm, BHIM). INR pricing. GST at 18% (OIDAR) for domestic customers; 0% GST via LUT for export invoices.
  - The checkout page auto-detects the user's country via IP geolocation and renders the appropriate payment flow. Users can manually switch region if needed.

- **Merchant of Record:** Payments processed via **Polar.sh** or **Lemon Squeezy** for international sales (handles VAT/GST remittance). Indian UPI payments handled directly via Razorpay with GST invoicing.

---

## 6. Installation & Quickstart Specification

### macOS

```bash
# Homebrew (recommended)
brew install dev-claw

# Or via curl
curl -fsSL https://get.dev-claw.dev/install.sh | sh
```

Also available as a signed `.pkg` installer from the download page.

### Linux

```bash
# Universal curl installer (auto-detects distro)
curl -fsSL https://get.dev-claw.dev/install.sh | sh

# Debian / Ubuntu
sudo dpkg -i dev-claw_1.0.0_amd64.deb

# Fedora / RHEL
sudo rpm -i dev-claw-1.0.0.x86_64.rpm

# Arch (AUR)
yay -S dev-claw
```

### Windows

```powershell
# winget
winget install dev-claw.dev-claw

# scoop
scoop bucket add dev-claw https://github.com/your-org/scoop-dev-claw
scoop install dev-claw
```

Also available as a signed `.msi` installer from the download page.

### Quickstart (all platforms)

```bash
# 1. Configure your LLM provider key
dev-claw config set-key --provider deepseek

# 2. Initialize repository (auto-detects stack)
dev-claw init

# 3. Pipe a build error for instant diagnosis
npm run dev | dev-claw doctor

# 4. Check remaining free-tier quota
dev-claw usage
```

---

## 7. Download Page Specification

The public download page (`https://dev-claw.dev/download`) is the primary acquisition surface. It must be self-contained — a developer landing on it should understand the product, download it, and know what they're getting, all without reading docs.

### 7.1 Page Structure & Copy

**Hero Section**

```
Dev-Claw
The AI-Powered Developer Daemon

Fix errors faster. Ship cleaner code. Zero SaaS overhead.
Dev-Claw is a local CLI tool that handles the SDLC tasks you hate —
terminal crash diagnosis, commit hygiene, .env auditing, and more.
Runs on your machine. Uses your API key. Free to start.

[ Download for macOS ]  [ Download for Linux ]  [ Download for Windows ]
```

**Free Tier Callout Banner**

```
✦ Free tier — generous by design

Dev-Claw is free with 200 AI invocations per month across all commands.
That's roughly:
  • 75 error diagnoses  (dev-claw doctor)
  • 60 commit / PR generations  (dev-claw git)
  • 25 code archaeology lookups  (dev-claw forensic)
  • 20 .env audits  (dev-claw env)
  • 15 mock data generations  (dev-claw mock)
  •  5 ephemeral cloud spins  (dev-claw cloud)

Quotas reset on the 1st of each month.
You can adjust per-command limits in .devclawrc.
Running your own infra? Set monthly_limit = 0 for unlimited.
```

**Use Cases Section**

| Command | What it does | Example |
|---|---|---|
| `doctor` | Pipes your `stderr` to an LLM. Get a 2-line root cause + a fix. No more googling stack traces. | `cargo build \| dev-claw doctor` |
| `git` | Scans your diff for debug leftovers, generates a conventional commit message, and drafts a structured PR description. | `dev-claw git commit` |
| `env` | Diffs your `.env` against `.env.example`, flags missing keys and type mismatches, blocks accidental secret commits. | `dev-claw env check` |
| `forensic` | Combines `git blame` + file context to explain *why* a gnarly block of code exists and what breaks if you touch it. | `dev-claw forensic src/auth.ts:42-80` |
| `mock` | Reads a TypeScript interface, GraphQL schema, or SQL table and generates N rows of realistic test data. | `dev-claw mock --schema types/user.ts --count 50` |
| `cloud` | Spins an ephemeral Hetzner / DigitalOcean VPS, runs your test suite remotely, streams logs, and tears it down automatically. | `dev-claw cloud spin --provider hetzner --ttl 2h` |

**Configuration Options Section**

```
Customize Dev-Claw via .devclawrc in your project root.

  Choose your LLM provider:       DeepSeek · Claude · GPT-4o-mini · Ollama (local/offline)
  Set your coding standards:      Paste your team's style guide — it's injected as system context.
  Block commit keywords:          console.log, debugger, FIXME, your own patterns.
  Tune your quota limits:         Raise, lower, or disable per-command monthly caps.
  Auto-detect your stack:         dev-claw init reads package.json / Cargo.toml / go.mod / requirements.txt.
```

**Pro Version Teaser**

```
─────────────────────────────────────────────
  Dev-Claw Pro — Coming Soon

  Unlimited invocations · No API key needed
  Cross-machine config sync · Team profiles
  Priority model access

  [ Join the waitlist ]       🔒 Coming Soon
─────────────────────────────────────────────
```

**Download Grid (per-platform)**

```
macOS
  • brew install dev-claw          (Homebrew)
  • dev-claw-1.0.0.pkg             (Signed installer, Apple Silicon + Intel)
  • curl install script

Linux
  • dev-claw_1.0.0_amd64.deb      (Debian / Ubuntu)
  • dev-claw-1.0.0.x86_64.rpm     (Fedora / RHEL / openSUSE)
  • dev-claw-1.0.0-linux.tar.gz   (Universal binary)
  • AUR: yay -S dev-claw           (Arch Linux)
  • curl install script

Windows
  • dev-claw-1.0.0-setup.msi      (Signed installer)
  • winget install dev-claw.dev-claw
  • scoop install dev-claw
```

**Footer Trust Signals**

```
MIT Licensed · No telemetry · Keys stored in your OS keychain only
Source on GitHub · Runs 100% locally
```

### 7.2 Page Requirements

- Single-page, no JS framework required — fast load on mobile/spotty dev WiFi.
- OS auto-detection: the primary download button highlights the correct platform on page load.
- Country auto-detection for the Pro waitlist form: show INR pricing for Indian IPs, USD otherwise.
- All installer file links are GitHub Releases assets — no proprietary CDN required for core distribution.
