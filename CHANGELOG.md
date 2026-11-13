# Changelog

Each version's section is its release notes: on GitHub, and in Graysense's "What's new" when an
update is ready.

## 0.1.0 — 2026-11-13

The first release of Graysense: a futuristic workbench for GitHub Copilot agents. A Rust core on the
Copilot SDK, with a fast, flat, modern interface (Tauri and React).

- **Copilot agents, Claude-style**: a centered conversation with streaming answers, a work log of
  every step (commands, edits with diffs, reads, searches, subagents) that folds away when the
  turn is done, inline approvals, questions, forms and plan reviews, Ask / Plan / Autopilot modes,
  a context meter, slash commands, attachments and queued messages.
- **Pick a model and how hard it thinks**: models grouped into Powerful, Versatile and Fast, with
  Auto at the top, and thinking levels (Auto, Low, Medium, High, Extra high, Max, as each model
  offers them) beside them. Change them mid-session, set what new sessions start with, or use
  `/model` and `/effort` from the message box.
- **Notes: a bucket for context you'll need later**. Save what happened ("Sent Charles the Rayfin
  logo for the partner deck") with the files and links that go with it, from anywhere
  (Ctrl+Shift+S, or the bookmark on any message). Search or just ask: "What did I give Charles?"
  answers from your notes, with their files attached. Notes rise with your votes and use, fade
  when they're not touched, and agents find them when they're relevant. Clean up the stale ones in
  one click.
- **Memory you can see**: agents learn your preferences and each project's conventions. Review
  what they learned, edit it, pin it, or turn memory off.
- **Sessions**: every session gets its own folder in your sessions folder (you choose where). A
  history sidebar with pinning, renaming, date groups and an activity feed, and Ctrl+K search
  across notes, conversations and session files.
- **Bays**: run several agents and terminals side by side (grid, columns, rows or one at a time),
  drag tabs to reorder, drop files onto an agent to attach them.
- **Real terminals**: PowerShell, Command Prompt, Git Bash or WSL through ConPTY, with WebGL
  rendering, find, and flow control for floods of output.
- **Mission board**: capture ideas, then drag one onto an agent (or launch it) to delegate it.
- **Canvases and MCP Apps**: checklists, tables, dashboards and documents beside the conversation,
  and sandboxed interactive UIs from MCP servers.
- **MCP servers and plugins**: add, configure and inspect servers and their tools; browse and
  install plugins from marketplaces.
- **Ask agents to set things up**: they can change any setting, MCP server or plugin for you.
- **Four themes** (Graphite, Carbon, Slate and Paper), interface zoom, a command palette
  (Ctrl+Shift+P) and keyboard shortcuts for everything (Ctrl+/).
- **Signed installer and automatic updates**: a one-click, code-signed installer for your account.
  New versions download in the background and install when you restart Graysense.
