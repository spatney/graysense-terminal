import { useEffect } from 'react';
import { captureIdea, toggleSidePanel, zoomBy } from '../components/Overlays';
import { act, captureNote, newSession, newShell, setSide, toggleSide } from './api';
import { focusComposer, openOverlay, useUi } from './store';
import type { LayoutId } from './types';

const LAYOUTS: LayoutId[] = ['grid', 'focus', 'columns', 'rows'];

function focusBayAt(index: number) {
  const b = useUi.getState().bays[index];
  if (!b) return;
  void act({ type: 'focusBay', bay: b.id });
  if (b.kind === 'agent') focusComposer(b.id);
}

/** App-wide keyboard shortcuts (see the shortcut sheet in Overlays). */
export function useShortcuts() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const s = useUi.getState();
      if (s.overlay || e.defaultPrevented) return;
      const target = e.target as HTMLElement | null;
      const inTerminal = !!target?.closest?.('.xterm');
      const k = e.key.toLowerCase();
      const ctrl = e.ctrlKey && !e.metaKey;
      const focusedIndex = s.bays.findIndex((b) => b.id === s.app?.focused);
      const focused = s.bays[focusedIndex];
      let handled = true;
      if (ctrl && !e.shiftKey && !e.altKey && k === 'k' && !inTerminal) openOverlay({ kind: 'search' });
      else if (ctrl && e.shiftKey && k === 'p') openOverlay({ kind: 'palette' });
      else if (ctrl && !e.shiftKey && (k === '/' || e.code === 'Slash')) openOverlay({ kind: 'shortcuts' });
      else if (ctrl && !e.shiftKey && k === ',') void setSide('settings');
      else if (ctrl && e.shiftKey && k === 'b') toggleSidePanel();
      else if (ctrl && e.shiftKey && k === 'h') void toggleSide('sessions');
      else if (ctrl && e.shiftKey && k === 'm') void toggleSide('memory');
      else if (ctrl && e.shiftKey && k === 'i') captureIdea();
      else if (ctrl && e.shiftKey && k === 's') captureNote();
      else if (ctrl && e.shiftKey && k === 'n') void newSession();
      else if (ctrl && e.shiftKey && k === 't') void newShell(focused && !focused.agent?.deferred ? focused.cwd : null);
      else if (ctrl && e.shiftKey && k === 'w' && focused) void act({ type: 'closeBay', bay: focused.id });
      else if (ctrl && e.shiftKey && k === 'enter' && focused) void act({ type: 'maximizeBay', bay: focused.id });
      else if (ctrl && e.shiftKey && k === 'l') {
        const now = s.settings?.layout ?? 'grid';
        void act({ type: 'setSettings', changes: { layout: LAYOUTS[(LAYOUTS.indexOf(now) + 1) % LAYOUTS.length] } });
      } else if (ctrl && k === 'tab' && s.bays.length > 1) focusBayAt((focusedIndex + (e.shiftKey ? -1 : 1) + s.bays.length) % s.bays.length);
      else if (ctrl && e.altKey && /^[1-9]$/.test(e.key)) focusBayAt(Number(e.key) - 1);
      else if (ctrl && !e.altKey && (k === '=' || k === '+')) {
        if (inTerminal && s.settings) void act({ type: 'setSettings', changes: { terminal_font_size: Math.min(28, s.settings.terminal_font_size + 1) } });
        else zoomBy(0.1);
      } else if (ctrl && !e.altKey && k === '-') {
        if (inTerminal && s.settings) void act({ type: 'setSettings', changes: { terminal_font_size: Math.max(9, s.settings.terminal_font_size - 1) } });
        else zoomBy(-0.1);
      } else if (ctrl && !e.altKey && k === '0') {
        if (inTerminal) void act({ type: 'setSettings', changes: { terminal_font_size: 13.5 } });
        else void act({ type: 'setSettings', changes: { zoom: 1 } });
      } else handled = false;
      if (handled) {
        e.preventDefault();
        e.stopPropagation();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);
}
