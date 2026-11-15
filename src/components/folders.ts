import { act, changeFolder, learnsIn, newAgent, newShell, setProjectLearning, setSide } from '../lib/api';
import { baseName, shortPath } from '../lib/format';
import { openOverlay, useUi } from '../lib/store';
import type { BayDto } from '../lib/types';
import { type MenuItem, copyText } from './ui';

/** How many recent folders a menu lists. */
const SHOWN = 8;

const pathKey = (p: string) => p.replace(/\//g, '\\').replace(/\\+$/, '').toLowerCase();

/** The folder `dir` is in ("C:\\code\\app" → "C:\\code"); a drive's root has none. */
function parentOf(dir: string): string | null {
  const t = dir.replace(/[\\/]+$/, '');
  const i = Math.max(t.lastIndexOf('\\'), t.lastIndexOf('/'));
  if (i <= 0) return null;
  const p = t.slice(0, i);
  return /^[a-zA-Z]:$/.test(p) ? `${p}\\` : p;
}

/** The recent folders, newest first, as menu rows (name, then where it is), leaving out `except`. */
export function recentFolderItems(open: (dir: string) => void, except?: string | null): MenuItem[] {
  const s = useUi.getState();
  return (s.settings?.recent_dirs ?? [])
    .filter((d) => !except || pathKey(d) !== pathKey(except))
    .slice(0, SHOWN)
    .map((dir) => {
      const parent = parentOf(dir);
      return { label: baseName(dir), detail: parent ? shortPath(parent, s.app?.home) : undefined, tip: dir, icon: 'folder', onSelect: () => open(dir) };
    });
}

/**
 * `label ›`: the recent folders, then "Choose a folder…". Before there are any recent folders,
 * just `label…`, which goes straight to the folder picker.
 */
export function folderSubmenu(label: string, icon: string, open: (dir: string) => void, choose: () => void, except?: string | null): MenuItem {
  const recent = recentFolderItems(open, except);
  if (!recent.length) return { label: `${label}…`, icon, onSelect: choose };
  return { label, icon, submenu: [...recent, { sep: true }, { label: 'Choose a folder…', icon: 'folder-opened', onSelect: choose }] };
}

/** An agent's folder menu: move the session to another folder, and the usual folder actions. */
export function agentFolderItems(bay: BayDto): MenuItem[] {
  const move = (dir: string) => void changeFolder(bay.id, dir);
  const pick = () => void changeFolder(bay.id);
  const w = bay.agent?.worktree;
  if (bay.agent?.deferred && w?.pending) {
    return [
      { label: 'Start without a worktree', icon: 'folder', tip: `Work in ${w.repo} itself`, onSelect: () => move(w.repo) },
      folderSubmenu('Start in a folder', 'arrow-swap', move, pick),
      { sep: true },
      { label: 'Reveal repository in File Explorer', icon: 'folder-opened', onSelect: () => void act({ type: 'revealPath', path: w.repo }) },
    ];
  }
  if (bay.agent?.deferred) {
    return [
      folderSubmenu('Start in a folder', 'arrow-swap', move, pick),
      { sep: true },
      { label: 'Sessions folder settings', icon: 'settings-gear', onSelect: () => void setSide('settings') },
    ];
  }
  const worktree: MenuItem[] = w && !w.pending ? [{ label: 'Merge or open a pull request…', icon: 'git-merge', tip: `The branch ${w.branch}`, onSelect: () => openOverlay({ kind: 'worktree', bay: bay.id }) }, { sep: true }] : [];
  return [
    ...worktree,
    folderSubmenu('Change folder', 'arrow-swap', move, pick, bay.cwd),
    { sep: true },
    { label: 'Reveal in File Explorer', icon: 'folder-opened', onSelect: () => void act({ type: 'revealPath', path: bay.cwd }) },
    { label: 'Open in VS Code', icon: 'vscode', onSelect: () => void act({ type: 'openInVsCode', path: bay.cwd }) },
    { label: 'New terminal here', icon: 'terminal', onSelect: () => void newShell(bay.cwd) },
    { label: 'New agent here', icon: 'add', onSelect: () => void act({ type: 'newAgent', dir: bay.cwd }) },
    { label: 'New agent in a worktree', icon: 'git-branch', tip: 'A new session with a worktree of its own, made from this folder’s repository', onSelect: () => void newAgent(bay.cwd, true) },
    { sep: true },
    { label: 'Copy path', icon: 'copy', onSelect: () => void copyText(bay.cwd, 'Path copied') },
    ...projectLearningItems(bay),
  ];
}

/** "Don't learn from <project>" (or "Learn from <project> again"), while memory is on. */
function projectLearningItems(bay: BayDto): MenuItem[] {
  const settings = useUi.getState().settings;
  const a = bay.agent;
  if (!a?.projectKey || a.incognito || !(settings?.['memory.enabled'] ?? true)) return [];
  const key = a.projectKey;
  const name = a.project || baseName(key);
  const item: MenuItem = learnsIn(settings, key)
    ? { label: `Don't learn from ${name}`, icon: 'circle-slash', tip: 'Nothing said in this project is learned or saved to memory. Agents here still get your memories.', onSelect: () => void setProjectLearning(key, false, name) }
    : { label: `Learn from ${name} again`, icon: 'lightbulb-sparkle', tip: 'Learning is off in this project', onSelect: () => void setProjectLearning(key, true, name) };
  return [{ sep: true }, item];
}
