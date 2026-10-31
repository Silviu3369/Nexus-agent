# Nexus

A local-first personal AI agent for Windows. You give it tasks in a desktop app; it works with the AI model you choose, reads what you share with it, keeps track of your work, and asks before it changes anything outside itself.

Nexus is built for one Windows user and runs on your own PC. Its state lives in a local SQLite database; API keys live in Windows Credential Manager; nothing runs in the cloud except the AI provider calls you set up.

**Stack:** Rust (domain core, a Host process, Windows integration), TypeScript + React (dashboard), Tauri (desktop shell), SQLite (state), Three.js (an optional animated view).

## What it does today

- **Tasks with an AI model.** Every task shows its steps live: model calls, pages read, documents read, tools used. Providers: Google Gemini (tested against the real API), OpenAI, Anthropic and DeepSeek (adapters tested against fakes). You can set a backup model for outages, a daily token budget and cost estimates.
- **Approvals you can trust.** Anything that changes something outside Nexus is proposed first, with the exact content and destination, and confirmed in a native Windows dialog, not just in the web view. Approvals are bound to a hash of the exact action, used once, and expire.
- **An independent Host.** The agent runs in its own process. Closing the window does not stop accepted work; a crash or restart does not repeat an effect: intent is stored before the effect and reconciled after an interruption.
- **Your files and documents.** Share folders with Nexus; it reads text, Word, PDF and Excel files (long ones in parts) and can write Word documents into a folder you allow, after your approval.
- **Registers.** Tables Nexus keeps for you, such as job applications, with history, undo, Excel import and export, and measured results.
- **An inbox.** Put a document in a folder and Nexus records it on its own, as a task you can follow, under stricter rules than your own tasks.
- **Memory and projects.** Facts the agent proposes are kept only after you approve them, per project.
- **Skills.** Import skills in the open [Agent Skills](https://agentskills.io/specification) format (`SKILL.md`, from a folder or a `.zip`). A skill starts off; you read it, then turn it on. Its scripts never run, and it grants no permissions.
- **Agents.** Create specialised agents, each with its own instructions, skills, tools, model and project. An agent can only use less than your permissions, never more.
- **Packs.** Built-in, declarative bundles for a workflow. The Jobs pack brings the job applications register and the instructions for new job ads in the inbox.
- **MCP.** Connect remote MCP servers over HTTPS. Read-only tools run directly; tools that change things ask for approval on every call.
- **Automations.** Scheduled instructions that run as ordinary tasks.
- **Permissions and data rules.** Turn any tool off; decide how many register changes run without asking. Each AI connection states what data it may receive (public, your own, other people's), and the Host enforces it before every call.
- **Backup.** A consistent backup, restorable only into a new profile, where nothing runs until you check it.

## What it does not do (yet)

- No installer: you build it from source (below).
- No operations on your PC beyond the files you share, no browser control, no code execution. Skill scripts never run.
- No email or chat channels, and no local models.
- Windows only, one user.

## Build and run

Requirements on Windows: Rust stable (`x86_64-pc-windows-msvc`), MSVC Build Tools 2022, WebView2, Node.js LTS and pnpm.

```powershell
pnpm install

# Release executables (no installer): target\release\desktop.exe and nexus-host.exe
cargo build -p nexus-host --release --locked
pnpm --filter @nexus/desktop tauri build --no-bundle
```

Start `target\release\desktop.exe` and press **Start Host**. In **Models**, add an AI connection with your own API key (for example a free Google AI Studio key) and choose a model. Then give Nexus a task from the command bar.

For development:

```powershell
# Dashboard in development mode; the Host is built next to it so that
# "Start Host" finds it.
cargo build -p nexus-host
pnpm --filter @nexus/desktop tauri dev

# Checks
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
pnpm --filter @nexus/desktop build

# A running Host, checked from outside
cargo run -p nexus-client --example health

# The real window, end to end, in a temporary profile, against test fakes
# (no real AI provider or MCP server is called). -Bin <folder> uses another
# folder with desktop.exe and nexus-host.exe.
cargo build -p nexus-host --locked
cargo build -p nexus-host --example fake_gemini_server --locked
powershell -ExecutionPolicy Bypass -File scripts\m1-ui-check.ps1
powershell -ExecutionPolicy Bypass -File scripts\m2-ui-check.ps1

# Restore a backup into a NEW profile (offline; the current profile is untouched)
target\release\nexus-host.exe restore <backup-folder> <new-profile-folder>
```

A restored profile starts paused: you can read its history, but nothing runs. To activate it after checking it, delete `restored.json` from it.

`NEXUS_PROFILE_DIR` moves the profile (database, notes, pairing token, log) to another folder, with its own pipe, leaving `%LOCALAPPDATA%\Nexus` untouched. The tests and the scripts above use it.

The TypeScript types in `packages/contracts` are generated from Rust. After a contract change: `$env:NEXUS_UPDATE_BINDINGS="1"; cargo test -p desktop`. The test fails if the generated file is behind.

`NEXUS_GEMINI_BASE_URL` and `NEXUS_ALLOW_LOOPBACK_MCP=1` point a debug Host at local test servers; release builds ignore both and talk only to the real HTTPS endpoints.

Local data (database, pairing token, Host log in `logs\nexus-host.log`) stays in `%LOCALAPPDATA%\Nexus`, never in the repo. API keys stay in Credential Manager under `Nexus/<profile-hash>/…`, never in files.

## Documents

The design documents are in Romanian.

| Document | Content |
| --- | --- |
| [AGENTS.md](AGENTS.md) | Rules for the coding agent working in the project. |
| [Plan](docs/PLAN_NEXUS_RO.md) | Milestones, current step and the progress journal. |
| [Architecture](docs/ARHITECTURA_NEXUS_RO.md) | Components, processes, data and the decisions (ADRs). |
| [Contracts](docs/CONTRACTE_NEXUS_RO.md) | Types, operations, states, compatibility and errors. |
| [Security and validation](docs/SECURITATE_SI_VALIDARE_RO.md) | Trust boundaries and the required checks. |
| [Product review](docs/REVIZUIRE_PRODUS_RO.md) | The product review the plan adopted. |
| [Handoff](docs/PREDARE_RO.md) and [start prompt](docs/PROMPT_START_RO.md) | How the implementation was started. |
