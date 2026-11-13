# Graysense

A futuristic workbench for **GitHub Copilot agents**: a Rust core on the
[GitHub Copilot SDK](https://crates.io/crates/github-copilot-sdk) with a fast, flat, modern
interface (Tauri + React). Run agents and terminals side by side, keep the context you'll need
later, and let Copilot set everything up for you.

![Graysense](docs/images/hero.png)

## Install

Download **Graysense-win-Setup.exe** from the [latest release](https://github.com/spatney/graysense-terminal/releases/latest)
and run it. It installs for your account (no admin rights), adds a Start menu shortcut and opens.
The installer and app are code-signed. Installed copies update themselves: new versions download
in the background and install when you restart or close Graysense (the repository is private, so
updates sign in with your GitHub CLI login, `gh auth login`).

Needs Windows 10 or 11 (x64) and the [GitHub Copilot CLI](https://github.com/github/copilot-cli),
signed in (`copilot login`).

## Highlights

**Copilot agents, Claude-style**
- A centered conversation with streaming markdown answers (highlighted, copyable code; tables;
  task lists). While the agent works, a live **work log** shows every step — commands with their
  output, edits as diffs, reads, searches, web fetches, subagents — and folds into
  "Worked for 2m · 14 steps" when the turn is done.
- Inline cards for **approvals** (*Allow*, *Allow for session*, *Deny* with feedback, showing the
  exact command, diff, path or URL), **questions**, **forms** and **plan reviews**.
- **Settings > Agents > Approve all tool calls by default** lets new sessions run tools without
  asking, including commands and file changes. Off by default; existing and resumed sessions
  are unchanged. Each session still has its own approval toggle.
- **Ask / Plan / Autopilot** modes (Shift+Tab), a context meter, Esc to stop, ↑ for earlier
  messages, and slash commands (`/model`, `/effort`, `/mode`, `/attach`, `/rename`, `/learn`,
  `/clear`, `/stop`, `/terminal`, `/new`).
- **Steer the agent while it works**: Enter sends a message that Copilot reads at its next step,
  without waiting for the turn to finish. Ctrl+Enter queues it for after the turn instead.
- **A model picker with thinking levels**: models grouped into Powerful, Versatile and Fast, with
  Auto at the top, and how hard the model thinks (Auto, Low, Medium, High, Extra high, Max, as
  each model offers them). Change them mid-session, or make them what new sessions start with.
- Attach files by dropping them on an agent, with the + button, or `/attach <path>`.
  Type **@** in a message to autocomplete paths from the session folder (the default folder
  before a session has its own folder). Use Up/Down and Tab/Enter to select, navigate folders,
  or Esc to dismiss.
  Absolute paths, `..`, `~`, and quoted paths with spaces work too. Selected files are attached
  when you send; deleting a mention before sending removes it from the message's attachments.
  **Paste an image** (Ctrl+V a screenshot or a copied PNG, JPEG, GIF or WebP picture) into the
  message box to attach it to the next message; pasting on the home screen starts a session.
  Pasted images are kept in `%APPDATA%\Graysense\pasted-images`; ones older than 30 days are
  deleted the next time you paste.

**Notes: a bucket for context you'll need later**
- Save what happened — *"When Charles asked for the Rayfin logo, I sent him the dark version"* —
  with the files and links that go with it. Capture from anywhere with **Ctrl+Shift+S**, the
  bookmark on any message, or by dropping files on Memory › Notes. Graysense keeps a copy of each
  file, so the note still works if the original moves.
- **Search or ask**: type *"what did I give Charles?"* and Copilot answers from your notes, with
  their files attached. Notes also show up in Ctrl+K search, and agents look them up on their own
  when they're relevant (`graysense_recall`), or save one when you ask (`graysense_save_note`).
- **Relevance that maintains itself**: upvote or downvote any note; opening its files, copying it
  and agents using it raise it, and notes nobody touches fade (half-life of about four months).
  Rarely useful notes collect in one place to clean up in a click; deleted notes wait in the
  trash for 30 days.

**Memory you can see and control**
- Agents learn your preferences and each project's conventions in the background (a cheap model
  reads finished conversations); new agents start with what's relevant. Review suggestions,
  edit, pin or delete anything, see where each memory came from, export or import it, and turn
  learning — or all of memory — off. Incognito sessions never read or write memory.

**Sessions**
- Every new session gets its own folder, named after your first message, in a sessions folder you
  choose (`~\Graysense\Sessions` by default). A history sidebar (Ctrl+Shift+H): open sessions,
  pinned, then Today / Yesterday / Previous 7 days…, with rename, pin, reveal and delete, plus an
  activity feed. Sessions started in the Copilot CLI are listed too.
- **Search everything** (Ctrl+K): notes, session names and conversations, and the contents of every
  file in your sessions folder, with highlighted snippets and line numbers.
- Each agent has a side panel with its **files** (open the folder in VS Code from there), the
  **changes** it made, and any canvases or apps.

**Bays and terminals**
- Tile agents and terminals in a grid, columns, rows or one at a time; drag tabs to reorder;
  maximize with Ctrl+Shift+Enter.
- Real terminals (PowerShell, Command Prompt, Git Bash or WSL) through ConPTY with xterm.js and
  WebGL: find, copy and paste, clickable links, and flow control so floods of output never freeze
  the app. Drop files on a terminal to type their paths.

**Mission board**
- Capture ideas (Ctrl+Shift+I), then launch one as a new session or drag its card onto an agent to
  delegate it. Missions follow their agent: in progress → ready for review → done. Agents can add
  missions too.

**Canvases, MCP Apps, MCP servers and plugins**
- Agents open native **canvases** next to the conversation: checklists you can tick, sortable
  tables, dashboards with charts, and documents.
- **MCP Apps** (SEP-1865) render in a sandboxed frame with their own content security policy.
- Manage **MCP servers** (local commands or remote URLs, status, tools, restart) and **plugins**
  (installed, marketplaces, browse and install).

**Ask agents to set things up**
- Everything you can configure, agents can configure for you: *"switch to the light theme"*,
  *"make new sessions go in D:\Work"*, *"add the GitHub MCP server"*. Appearance changes apply
  right away; anything else shows the exact change and asks first.

**Look and feel**
- Four themes — Graphite, Carbon, Slate and Paper (light) — with the native title bar painted to
  match, interface zoom (Ctrl+= / Ctrl+-), a command palette (Ctrl+Shift+P) and every shortcut on
  one sheet (Ctrl+/).

## How it's built

```
src-tauri/          Rust: the Graysense core
  src/agent/        Copilot SDK actor (sessions, tools, permissions, canvases, MCP, plugins)
  src/engine/       UI-agnostic core: bays, actions, learning, and dirty-tracked UI updates
  src/memory.rs     memory and notes (ranking, relevance, curation, attachments)
  src/terminal/     PTY service (ConPTY via portable-pty)
  src/search.rs     search index over sessions and session files
  src/update.rs     Velopack updater (draft-aware GitHub releases source)
src/                React + TypeScript UI (Vite)
  components/       shell, agent bay (transcript, composer, prompts, dock), terminal, panels
  lib/              store (applies core updates), API, notes ranking, theming
scripts/            release.ps1 (build, sign, package, publish), render-assets.ps1 (icons)
```

The core runs on its own thread: backend events in, batched UI updates out (only what changed;
transcripts as per-item patches). The UI calls one `act` command with typed actions.

## Develop

Needs Node.js 20+, Rust 1.94+ and the Copilot CLI.

```powershell
npm install
npm run tauri dev           # Vite dev server + the debug app with hot reload
npm test                    # frontend tests (vitest)
npm run typecheck
cd src-tauri; cargo test    # core tests
```

`GRAYSENSE_CONFIG_DIR` points the app at another profile (settings, memory, workspace), handy for
trying things without touching your own. `GRAYSENSE_LOG=debug` turns up logging
(`%APPDATA%\Graysense\graysense.log`).

## Release

```powershell
pwsh scripts\release.ps1          # build, sign, package and publish v<version>
pwsh scripts\release.ps1 -Draft   # publish as a draft to look over first
```

The version comes from `src-tauri/Cargo.toml` (keep `package.json` and `tauri.conf.json` in step)
and the notes from its `CHANGELOG.md` section. Packages are made with [Velopack](https://velopack.io)
and signed with Azure Artifact Signing using your Azure CLI sign-in (`az login`).
