import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import { openUrl } from '@tauri-apps/plugin-opener';
import { applyUpdate, focusComposer, openOverlay, toast, useUi } from './store';
import type { Action, BayId, DirEntry, PanelId, SearchHit, UiUpdate } from './types';

/** Run a core action. Errors are shown as a toast and resolve to `undefined`. */
export async function act<T = unknown>(action: Action): Promise<T | undefined> {
  try {
    return (await invoke('act', { action })) as T;
  } catch (e) {
    toast('error', String(e));
    return undefined;
  }
}

/** Run a core action, throwing its error (for callers that show it themselves). */
export function tryAct<T = unknown>(action: Action): Promise<T> {
  return invoke('act', { action }) as Promise<T>;
}

/** Subscribe to the core's updates, then load the whole state. */
export async function connect() {
  const early: UiUpdate[] = [];
  let synced = false;
  await listen<UiUpdate>('ui', (e) => {
    if (!synced) early.push(e.payload);
    else if (e.payload.seq > useUi.getState().seq) applyUpdate(e.payload);
  });
  const snap = await invoke<UiUpdate>('snapshot');
  applyUpdate(snap);
  for (const u of early) if (u.seq > snap.seq) applyUpdate(u);
  synced = true;
}

export const search = (query: string, limit = 40) => invoke<SearchHit[]>('search', { query, limit });
export const listDir = (path: string) => invoke<DirEntry[]>('list_dir', { path });
export const readText = (path: string) => invoke<string | null>('read_text', { path });

/** Run what a toast's button asks for (an action, or a link). */
export function runToastAction(action: any) {
  if (!action) return;
  if (action.type === 'openUrl') void openUrl(action.url);
  else if (action.type === 'focusBay') {
    void act({ type: 'focusBay', bay: action.bay });
    focusComposer(action.bay);
  } else if (action.type === 'updateRestart') void restartToUpdate();
  else if (action.type === 'revealMemory') revealMemory(action.id);
  else void act(action as Action);
}

export async function restartToUpdate() {
  const r = await act<{ working: number }>({ type: 'updateRestart', confirmed: false });
  if (r && r.working > 0) {
    const n = r.working;
    openOverlay({
      kind: 'confirm',
      title: `Restart to install Graysense ${useUi.getState().update?.nextVersion ?? ''}?`,
      body: `${n === 1 ? 'An agent is' : `${n} agents are`} still working. Restarting stops ${n === 1 ? 'it' : 'them'}; your sessions reopen when Graysense is back.`,
      confirm: 'Restart now',
      onConfirm: () => void act({ type: 'updateRestart', confirmed: true }),
    });
  }
}

/** Open Memory on one memory or note, scrolled to it and highlighted. */
export function revealMemory(id: number) {
  void setSide('memory');
  useUi.setState({ revealMemory: id });
}

/** Save something to notes from anywhere: a small capture window, prefilled. */
export function captureNote(prefill: { text?: string; files?: string[]; session?: string | null; title?: string | null } = {}) {
  openOverlay({ kind: 'note', ...prefill });
}

export function setSide(panel: PanelId | 'hidden') {
  return act({ type: 'setSide', panel });
}

/** Open or close a side panel, VS Code style: clicking the open one hides it. */
export function toggleSide(panel: PanelId) {
  const open = useUi.getState().settings?.side_panel;
  return setSide(open === panel ? 'hidden' : panel);
}

export async function pickFolder(title = 'Choose a folder', defaultPath?: string): Promise<string | null> {
  const r = await openDialog({ directory: true, multiple: false, title, defaultPath });
  return typeof r === 'string' ? r : null;
}

export async function pickFiles(title = 'Attach files', defaultPath?: string): Promise<string[]> {
  const r = await openDialog({ multiple: true, title, defaultPath });
  return Array.isArray(r) ? r : r ? [r] : [];
}

export async function pickSave(title: string, defaultPath: string): Promise<string | null> {
  return (await saveDialog({ title, defaultPath })) ?? null;
}

/** New session (deferred: its folder is made with the first message). */
export async function newSession(prompt?: string, incognito = false) {
  const bay = await act<BayId>({ type: 'newSession', prompt: prompt ?? null, incognito });
  if (bay != null) focusComposer(bay);
  return bay;
}

export async function newAgentIn() {
  const dir = await pickFolder('Open an agent in a folder', useUi.getState().app?.defaultDir);
  if (!dir) return;
  const bay = await act<BayId>({ type: 'newAgent', dir });
  if (bay != null) focusComposer(bay);
}

export async function newShell(dir?: string | null) {
  return act<BayId>({ type: 'newShell', dir: dir ?? null });
}

export async function newShellIn() {
  const dir = await pickFolder('Open a terminal in a folder', useUi.getState().app?.defaultDir);
  if (dir) await newShell(dir);
}

export function openLink(url: string) {
  if (/^https?:\/\//i.test(url)) void openUrl(url);
}
