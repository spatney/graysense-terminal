import { create } from 'zustand';
import type {
  AppDto,
  BayDto,
  BayId,
  ItemDto,
  LogLineDto,
  McpDto,
  MemoryStateDto,
  MissionDto,
  PluginsDto,
  SessionsDto,
  Settings,
  StatsDto,
  ToastDto,
  UiUpdate,
  UpdateStatus,
} from './types';

export type Overlay =
  | { kind: 'search'; query?: string }
  | { kind: 'palette'; mode?: 'commands' | 'folders-agent' | 'folders-shell' | 'sessions' }
  | { kind: 'shortcuts' }
  | { kind: 'note'; text?: string; files?: string[]; session?: string | null; title?: string | null }
  | { kind: 'confirm'; title: string; body: string; confirm: string; danger?: boolean; onConfirm: () => void };

export interface LocalToast extends ToastDto {
  born: number;
}

export interface DockState {
  open: boolean;
  /** `canvas:<instanceId>`, `app:<view>`, `files` or `changes`. */
  tab: string;
  width: number;
}

interface UiState {
  ready: boolean;
  seq: number;
  app: AppDto | null;
  settings: Settings | null;
  bays: BayDto[];
  transcripts: Record<BayId, ItemDto[]>;
  missions: MissionDto[];
  sessions: SessionsDto;
  memory: MemoryStateDto;
  mcp: McpDto | null;
  plugins: PluginsDto | null;
  update: UpdateStatus | null;
  stats: StatsDto | null;
  cpuHistory: number[];
  activity: LogLineDto[];
  toasts: LocalToast[];

  overlay: Overlay | null;
  /** Unsent text per bay (kept when switching bays). */
  drafts: Record<BayId, string>;
  dock: Record<BayId, DockState>;
  /** Memory to scroll to and highlight in the Memory panel. */
  revealMemory: number | null;
  /** Ask the missions panel to focus its capture box. */
  captureTick: number;
  /** Focus a bay's composer (bumped to re-trigger). */
  focusTick: Record<BayId, number>;
}

const empty = {
  sessions: { loaded: false, items: [] } as SessionsDto,
  memory: { memories: [], lastRun: null, learning: 0 } as MemoryStateDto,
};

export const useUi = create<UiState>(() => ({
  ready: false,
  seq: -1,
  app: null,
  settings: null,
  bays: [],
  transcripts: {},
  missions: [],
  sessions: empty.sessions,
  memory: empty.memory,
  mcp: null,
  plugins: null,
  update: null,
  stats: null,
  cpuHistory: [],
  activity: [],
  toasts: [],
  overlay: null,
  drafts: {},
  dock: {},
  revealMemory: null,
  captureTick: 0,
  focusTick: {},
}));

/** Fold one update from the core into the store. */
export function applyUpdate(u: UiUpdate) {
  useUi.setState((s) => {
    const next: Partial<UiState> = { seq: u.seq, ready: true };
    if (u.app) next.app = u.app;
    if (u.settings) next.settings = u.settings;
    if (u.bays) next.bays = u.bays;
    if (u.missions) next.missions = u.missions;
    if (u.sessions) next.sessions = u.sessions;
    if (u.memory) next.memory = u.memory;
    if (u.mcp) next.mcp = u.mcp;
    if (u.plugins) next.plugins = u.plugins;
    if (u.update) next.update = u.update;
    if (u.stats) {
      next.stats = u.stats;
      next.cpuHistory = [...s.cpuHistory, u.stats.cpu].slice(-40);
    }
    if (u.transcripts?.length || u.full || (u.bays && Object.keys(s.transcripts).some((k) => !u.bays!.some((b) => b.id === Number(k))))) {
      const t: Record<BayId, ItemDto[]> = u.full ? {} : { ...s.transcripts };
      for (const p of u.transcripts ?? []) {
        const arr = (t[p.bay] ?? []).slice(0, p.len);
        for (const [i, item] of p.items) arr[i] = item;
        t[p.bay] = arr;
      }
      if (u.bays) {
        const open = new Set(u.bays.map((b) => b.id));
        for (const k of Object.keys(t)) if (!open.has(Number(k))) delete t[Number(k)];
      }
      next.transcripts = t;
    }
    if (u.full) next.activity = (u.activity ?? []).slice().reverse();
    else if (u.activity?.length) next.activity = [...u.activity.slice().reverse(), ...s.activity].slice(0, 300);
    if (u.toasts?.length) {
      const now = Date.now();
      next.toasts = [...s.toasts, ...u.toasts.map((t) => ({ ...t, born: now }))].slice(-5);
    }
    return next;
  });
}

let localToast = -1;

export function toast(level: ToastDto['level'], text: string, action: ToastDto['action'] = null) {
  useUi.setState((s) => ({ toasts: [...s.toasts, { id: localToast--, level, text, action, born: Date.now() }].slice(-5) }));
}

export function dismissToast(id: number) {
  useUi.setState((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }));
}

export function openOverlay(overlay: Overlay | null) {
  useUi.setState({ overlay });
}

export function setDraft(bay: BayId, text: string) {
  useUi.setState((s) => ({ drafts: { ...s.drafts, [bay]: text } }));
}

export function focusComposer(bay: BayId) {
  useUi.setState((s) => ({ focusTick: { ...s.focusTick, [bay]: (s.focusTick[bay] ?? 0) + 1 } }));
}

export function dockOf(bay: BayId): DockState {
  return useUi.getState().dock[bay] ?? { open: false, tab: 'files', width: 420 };
}

export function setDock(bay: BayId, patch: Partial<DockState>) {
  useUi.setState((s) => ({ dock: { ...s.dock, [bay]: { ...(s.dock[bay] ?? { open: false, tab: 'files', width: 420 }), ...patch } } }));
}

export const bayById = (id: BayId | null | undefined) => useUi.getState().bays.find((b) => b.id === id) ?? null;
