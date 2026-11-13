import { invoke } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { useEffect } from 'react';
import { create } from 'zustand';
import { act } from './api';
import { bayAt } from './drag';
import { dirName } from './format';
import { focusComposer, toast, useUi } from './store';
import type { BayId } from './types';

/** Event names the note capture boxes listen to (see components/NoteCapture). */
const NOTE_DROP = 'gs-note-drop';

/** A note capture box under the pointer (the quick-capture window takes every drop while open). */
function noteTarget(position: { x: number; y: number }): string | null {
  if (useUi.getState().overlay?.kind === 'note') return 'overlay';
  const dpr = window.devicePixelRatio || 1;
  const el = document.elementFromPoint(position.x / dpr, position.y / dpr)?.closest('[data-note-drop]');
  return el ? el.getAttribute('data-note-drop') || 'panel' : null;
}

function noteOver(target: string | null) {
  window.dispatchEvent(new CustomEvent(`${NOTE_DROP}-over`, { detail: { on: target != null, target } }));
}

/** Files being dragged in from File Explorer: how many, and the bay under the pointer. */
export const useFileDrag = create<{ active: boolean; count: number; over: BayId | null }>(() => ({ active: false, count: 0, over: null }));

/** Quote a path for pasting into a shell prompt. */
export function shellQuote(path: string): string {
  return /[\s&()';,$`]/.test(path) ? `"${path}"` : path;
}

function overBay(position: { x: number; y: number }): BayId | null {
  const dpr = window.devicePixelRatio || 1;
  return bayAt(position.x / dpr, position.y / dpr);
}

/** OS file drops: onto an agent they're attached, onto a terminal their paths are typed. */
export function useFileDrop() {
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let alive = true;
    void getCurrentWebview()
      .onDragDropEvent((e) => {
        const p = e.payload;
        if (p.type === 'enter' || p.type === 'over') {
          const note = noteTarget(p.position);
          noteOver(note);
          const over = note ? null : overBay(p.position);
          useFileDrag.setState(p.type === 'enter' ? { active: true, count: p.paths.length, over } : { over });
        } else if (p.type === 'leave') {
          noteOver(null);
          useFileDrag.setState({ active: false, over: null, count: 0 });
        } else if (p.type === 'drop') {
          const note = noteTarget(p.position);
          noteOver(null);
          const bay = overBay(p.position);
          useFileDrag.setState({ active: false, over: null, count: 0 });
          if (note) window.dispatchEvent(new CustomEvent(NOTE_DROP, { detail: { paths: p.paths, target: note } }));
          else void dropped(p.paths, bay);
        }
      })
      .then((u) => {
        if (alive) unlisten = u;
        else u();
      });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);
}

async function dropped(paths: string[], bay: BayId | null) {
  if (!paths.length) return;
  const s = useUi.getState();
  const target = s.bays.find((b) => b.id === bay);
  if (target?.kind === 'shell') {
    await invoke('terminal_write', { bay: target.id, data: paths.map(shellQuote).join(' ') + ' ' }).catch(() => {});
    void act({ type: 'focusBay', bay: target.id });
    return;
  }
  if (target?.kind === 'agent') {
    await act({ type: 'attach', bay: target.id, paths });
    void act({ type: 'focusBay', bay: target.id });
    focusComposer(target.id);
    return;
  }
  // Dropped outside any bay: start an agent where the files are, with them attached.
  const dir = dirName(paths[0]);
  const id = await act<number>({ type: 'newAgent', dir: paths.length === 1 && !/\.[^\\/]+$/.test(paths[0]) ? paths[0] : dir });
  if (id == null) return;
  await act({ type: 'attach', bay: id, paths: paths.filter((p) => /\.[^\\/]+$/.test(p)) });
  focusComposer(id);
  toast('ok', `Opened an agent in ${dir}`);
}
