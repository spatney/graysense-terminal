# Changelog

Each version's section is its release notes: on GitHub, and in Graysense's "What's new" when an
update is ready.

## 0.4.0 — 2026-11-15

### Undo, edit, retry and fork

- **Undo from here**: hover one of your messages and undo it. The conversation goes back to just
  before it, the message returns to the message box, and the files the agent changed since are put
  back as they were (files you changed by hand since are left alone). A dialog lists what goes
  first. Graysense now records the files agents change so it can do this (**Settings › Agents ›
  Track file changes for undo**, on by default); turns from before this version undo the
  conversation only.
- **Edit and send again**: the edit button on one of your messages now takes the conversation back
  to it, after the same check, so you can change it and send it again. Before, it only copied the
  message to the message box.
- **Retry** the last answer, on the same model or another one.
- **Fork a conversation** into a new session: all of it from the bay's ⋯ menu, or from any of your
  messages (the copy stops just before it and puts it in the message box). The fork keeps the
  model, thinking level, context window and approval choice.

### Agents side by side

- **Worktree sessions**: **New agent in a worktree** (the ⌄ menu beside +, the command palette, or
  a mission's launch options) gives the session a branch and a folder of its own beside the
  repository (`<repo>.worktrees\<slug>`), made with its first message and named after it.
  **Fork into a worktree** takes the uncommitted changes along. When the work is done, click the
  branch chip to review it, then merge it or open a pull request. Merging into a branch that's
  checked out updates its files, and a conflict changes nothing. Closing a session with work in its
  worktree asks whether to keep it.
- **A warning when agents share a folder**: start a session where another agent is changing files
  (the same repository, or the same folder outside one) and it says who. Before its first message,
  **Start in a worktree instead** swaps it for a worktree session, keeping its model, mode,
  attachments and the message you were writing.

### Memory

- **See what an agent was given, and correct it**: the "Used 3 memories" chip under an answer
  opens into those memories, with Show, Edit, Pin and **Wrong or outdated**. That forgets the
  memory (with Undo), stops it being learned again, and tells the agents that were given it.
- **Learns how failures were fixed**: the curator now sees why a step failed (one line, with
  secrets removed) and saves the fix as a how-to when the conversation shows one.
- **Finds memories by other names**: learning gives each memory a few hidden search words
  (synonyms, the tool or area it's about), so asking about *the bundler* finds *"Start Vite
  with…"*. They sync too, and older versions don't wipe them.
- **Where you left off**: when a session's work isn't finished, learning notes the next step.
  Resuming the session (or reopening one left for more than 12 hours) shows a card with the
  summary and that step, and **Continue** puts the step in the message box. New agents in the
  project are told what was left to do.
- **Memory tidies itself**: about once a week, or with **Tidy up now** in Memory, the curator
  merges saved preferences, facts and how-tos that say the same thing. What you wrote or pinned is
  only proposed for change, and anything dropped waits in Deleted for 30 days.
- **Don't learn from a project**: from a session's folder menu, or in Memory. Nothing said there
  is learned or saved, and agents there still get your memories.
- **Cut cleanly**: long memories end after a whole word (with "…"), and what happened in a
  session is kept to one short sentence.

### Everyday

- **Find in a conversation** (Ctrl+F, or the search button in a bay's header): matches reach turns
  that aren't on screen yet, opening the work log or step they're in, and a match can run across
  bold text or code. Enter and Shift+Enter step through them; Esc closes.
- **Unsent messages are kept** across restarts and updates, and come back when you reopen the
  session.
- **Windows notifications** when an agent finishes, asks a question or needs approval while
  Graysense is in the background (several at once are combined, questions first). Click one to go
  to that agent. Incognito sessions only say what happened. Turn them off in **Settings ›
  Appearance › Desktop notifications**.
- **Premium requests left** show in the status bar (it warns under 10%), with the reset date and
  what the focused session has used.

### Fixes

- **A damaged settings, memory or missions file is never written over**: it's moved aside and the
  last good copy is restored, with a message saying so. Saves reach the disk before they replace
  the old file, and one that fails is tried again, then reported. Messages raised before the window
  opens (such as "Graysense updated") now show once it does.
- **The Changes tab lists the files patches wrote** (how GPT models edit), not only files edited
  one at a time.

### Known limitations

- Undoing a turn doesn't take back what was learned from it.
- Retrying or editing a question you asked about your notes sends it without the notes.
- A fork works in the same folder as the original unless you fork into a worktree; a worktree fork
  from an earlier message takes the files as they are now.
- Worktrees don't get the files Git ignores (such as `node_modules` or `.env`): install
  dependencies in them. Merging into a branch that isn't checked out needs Git 2.38 or newer.
  Deleting a session that isn't open leaves its worktree.
- The warning about agents sharing a folder is given when a session starts or moves, not when
  another agent moves in later.
- Hidden search words help agents find memories, not the Memory panel's search. The projects
  Graysense doesn't learn from are kept per PC, and unsent messages in incognito sessions aren't
  kept.

## 0.3.2 — 2026-11-14

- **Changes shown inline**: an agent's edits now read like a code review. Open an edit or a new
  file in the work log to see the change itself, with line numbers, the code in color and the
  added and removed lines marked, and every step says how many lines it added and removed without
  being opened. A long change shows its first lines and **Show all** for the rest; you can wrap
  long lines, copy the diff or open the file. Approving an edit shows the same diff first.
- **Changed files, folded**: the files a turn changed are listed under its answer with their
  counts, and past four the rest fold into "+N more" (hover it to see them). Click a file to see
  everything the turn did to it as one diff, even when it was edited several times; when later
  edits undo earlier ones, it says there's no net change. A change too big to show whole still
  lists every file it touched, with its counts.

## 0.3.1 — 2026-11-14

- **A calmer look, without the glow**: the soft halos around the message box, text boxes,
  approval cards and the focused session are gone (a plain border shows where you're typing), and
  so are the light behind the logo, the tinted backdrops on the start page and in About, and the
  gradients: the accent is one flat color. Busy and waiting dots pulse gently instead of sending
  out rings, and the outline that shows keyboard focus is a crisp neutral line, so it no longer
  looks like a glow or like the accent's "selected".
- **"Thinking" shows once**: when an agent starts on your message, what it's doing and for how
  long appear on one line until its first step, not on two.

## 0.3.0 — 2026-11-14

- **Keep your memory in a GitHub repository**: in **Settings › Sync**, connect Graysense to a
  private repository (it can create one for you) and your memories, notes, projects, missions and
  a short list of preferences are saved there and shared by every PC you connect. It uses the Git
  and `gh` sign-in you already have; Graysense never asks for a token. Sync runs on its own after
  a change, when it starts and every so often, and **Sync now** does it on request. Each part can
  be switched off, and the status bar shows when everything is up to date.
- **Merge with AI**: connect a repository that already holds Graysense data and the two sets are
  reconciled instead of one replacing the other. Anything only one side has is kept, an item both
  sides changed keeps the version with your own words (or else the newer one), and duplicates and
  contradictions are sorted out by your curator model. Memories you wrote or pinned are never
  changed behind your back: the AI's suggestions for them wait in **Review**. Notes are never
  rewritten. A copy of what was on this PC is kept first, and **Use the repository's copy** starts
  over from the repository instead.
- **Made to be safe**: only private repositories are used unless you confirm otherwise, approval
  rules, shell access, Approve all and anything that looks like a secret are never synced, and
  what comes back from the repository is checked before it is used: network paths are never
  opened, and a file that a note names on another PC is never handed to an agent. A sync that
  would remove most of what it keeps in step, because the repository was deleted and made again
  or this PC's memory file wouldn't load, stops and asks first. A git step that stops answering
  is stopped, with everything it started.

## 0.2.5 — 2026-11-14

- **Rows stay put on hover**: pointing at an installed plugin no longer makes its row taller (and
  everything below it jump) as its Update and Uninstall buttons appear. No list with buttons that
  appear on hover changes height any more.

## 0.2.4 — 2026-11-14

- **Pick up where you left off**: close Graysense (or restart it to update) while an agent is
  working, and when it opens again the agent carries on with its task. Messages it hadn't read
  yet are sent again, and reopened sessions keep their names and their Approve all setting.
- **Reopened sessions stay put**: closing Graysense again while a session is still reopening (or
  after you cleared it with `/clear`) no longer swaps it for a new, empty one the next time.
  Steps that were cut off show as interrupted instead of running forever, and the first message
  you send to a reopened session no longer renames it.
- **"Approving all" stays on one line**: in a narrow message box it shrinks to its lit shield
  (hover it for what it means), leaving room for the model's name.

## 0.2.3 — 2026-11-14

- **Recent folders, a hover away**: in the ⌄ menu beside +, **New agent in folder** and **New
  terminal in folder** now list the folders you've worked in lately, with **Choose a folder…**
  for any other.
- **Move a session to another folder**: click the folder beside a session's title and choose
  **Change folder**, type `/cd <folder>` (`/cd` alone lets you pick one), or use the command
  palette. The agent's commands, file lookups, permissions, project instructions and memory follow
  it there. A new session that hasn't started yet can start in a folder you choose instead of
  getting its own.
- **Image previews**: attached images show as thumbnails in the message box, in your messages and
  in the Files panel. Click one to see it large, with buttons to open it, show it in File Explorer
  or copy its path.

## 0.2.2 — 2026-11-13

- **Choose the context window**: for models that have a long window (such as 1M tokens on Claude
  Opus), the model picker now has a Context window row beside Thinking: Auto, the standard size
  or the long one. It applies mid-session, the picker says when the long window costs more per
  token, and the chip shows the size you chose. Set what new sessions start with in Settings >
  Agents, or type `/context 1m` (`/context` alone shows how full the window is).
- **No more stacks of "Context compacted"**: when a long turn compacts its context several times,
  the notice shows once with a count (×3). A compaction that fails now says so instead of
  claiming it freed space.

## 0.2.1 — 2026-11-13

- **Open in VS Code from the Files panel**: a button beside Refresh opens the folder you're
  looking at in VS Code, and the folders in its path now have the same right-click menu as the
  files and folders below them.
- **No more text spilling out of the work log**: a long answer to an agent's question or form now
  ends with "…" at the edge of the conversation; hover it to read the whole answer. In narrow
  panels, a long step name at the top of a running work log is shortened the same way, so the
  timer and step count stay on one line.

## 0.2.0 — 2026-11-13

- **Steer Copilot while it works**: press Enter while an agent is busy and Copilot reads your
  message at its next step, without waiting for the turn to finish. Ctrl+Enter (or Ctrl+click on
  send) queues it for after the turn instead. Waiting messages show below the turn.
- **Mention files with `@`**: type `@` in the message box to autocomplete paths from the session
  folder. Tab or Enter picks a file or opens a folder; mentioned files are attached when you send.
- **Paste images**: Ctrl+V a screenshot or a copied picture into the message box to attach it.
  Pasting on the home screen starts a session.
- **Approve all tool calls by default**: a new setting (Settings > Agents) lets new sessions run
  tools without asking. It's off by default, and each session keeps its own approval toggle.

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
