import { useUi } from './store';
import type { BayDto, BayId } from './types';

/** Unsent messages, kept across restarts and updates. */
const STORE = 'gs-drafts';
const KEEP = 100;
const MAX_AGE = 30 * 24 * 3600 * 1000;
/** The home screen's message box (bay id 0). */
const HOME = 'home';

export interface SavedDraft {
  text: string;
  /** When it last changed (Unix ms). */
  at: number;
}

export type SavedDrafts = Record<string, SavedDraft>;

const folderKey = (dir: string) => dir.replace(/\//g, '\\').replace(/\\+$/, '').toLowerCase();

/**
 * Where each message box's draft is kept: under its session once the session is reopened after a
 * restart, otherwise under its folder (an empty session opens there afresh). Incognito drafts aren't kept.
 */
export function draftKeys(bays: BayDto[]): Map<BayId, string> {
  const keys = new Map<BayId, string>([[0, HOME]]);
  const folders = new Map<string, number>();
  for (const b of bays) {
    const a = b.agent;
    if (!a || a.incognito) continue;
    if (a.sessionId && a.resumable) {
      keys.set(b.id, `s:${a.sessionId}`);
      continue;
    }
    const folder = folderKey(b.cwd);
    const n = (folders.get(folder) ?? 0) + 1;
    folders.set(folder, n);
    keys.set(b.id, n > 1 ? `f:${folder}#${n}` : `f:${folder}`);
  }
  return keys;
}

/**
 * The drafts to keep: the open message boxes', and those left in closed sessions (for when they're
 * reopened). Newest first, at most 100, none older than 30 days.
 */
export function keptDrafts(saved: SavedDrafts, keys: Map<BayId, string>, drafts: Record<BayId, string>, now: number): SavedDrafts {
  const open = new Set(keys.values());
  const out: SavedDrafts = {};
  for (const [k, v] of Object.entries(saved)) if (k.startsWith('s:') && !open.has(k)) out[k] = v;
  for (const [id, k] of keys) {
    const text = drafts[id] ?? '';
    if (text.trim()) out[k] = saved[k]?.text === text ? saved[k] : { text, at: now };
  }
  return Object.fromEntries(
    Object.entries(out)
      .filter(([, v]) => now - v.at < MAX_AGE)
      .sort((a, b) => b[1].at - a[1].at)
      .slice(0, KEEP),
  );
}

function load(): SavedDrafts {
  try {
    const v = JSON.parse(localStorage.getItem(STORE) ?? '{}');
    return v && typeof v === 'object' && !Array.isArray(v) ? v : {};
  } catch {
    return {};
  }
}

let installed = false;

/** Keep message boxes' drafts in local storage, and put them back when their bays open again. */
export function installDrafts() {
  if (installed) return;
  installed = true;
  let saved = load();
  let written = JSON.stringify(saved);
  const seen = new Set<BayId>();
  let restored = false;
  let keys = '';
  let timer: number | undefined;

  const save = () => {
    timer = undefined;
    const s = useUi.getState();
    if (!s.ready) return;
    saved = keptDrafts(saved, draftKeys(s.bays), s.drafts, Date.now());
    const json = JSON.stringify(saved);
    if (json === written) return;
    try {
      localStorage.setItem(STORE, json);
      written = json;
    } catch {
      // Storage full or unavailable: drafts just aren't kept.
    }
  };

  // The workspace's bays come back with their drafts. Later, a session reopened gets its own back;
  // a new bay in a folder doesn't take another's.
  const seed = (bays: BayDto[]) => {
    const all = !restored;
    restored = true;
    const fills: Record<BayId, string> = {};
    for (const [id, k] of draftKeys(bays)) {
      if (seen.has(id)) continue;
      seen.add(id);
      const text = (all || k.startsWith('s:')) && saved[k]?.text;
      if (text && !useUi.getState().drafts[id]) fills[id] = text;
    }
    if (Object.keys(fills).length) useUi.setState((s) => ({ drafts: { ...s.drafts, ...fills } }));
  };

  if (saved[HOME] && !useUi.getState().drafts[0]) useUi.setState((s) => ({ drafts: { ...s.drafts, 0: saved[HOME].text } }));
  seen.add(0);
  if (useUi.getState().ready) seed(useUi.getState().bays);

  useUi.subscribe((s, prev) => {
    if (!s.ready) return;
    if (s.bays !== prev.bays || !prev.ready) seed(s.bays);
    let changed = s.drafts !== prev.drafts;
    // Bays change all the time while agents work; only a draft moving to another key matters.
    if (s.bays !== prev.bays || !prev.ready) {
      const now = JSON.stringify([...draftKeys(s.bays)]);
      changed ||= now !== keys;
      keys = now;
    }
    if (changed && timer === undefined) timer = window.setTimeout(save, 500);
  });
  window.addEventListener('beforeunload', () => {
    if (timer === undefined) return;
    window.clearTimeout(timer);
    save();
  });
}
