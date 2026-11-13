import { create } from 'zustand';
import type { BayId } from './types';

/** A mission being dragged onto a bay (pointer events: native drag-and-drop is taken by file drops). */
export interface MissionDrag {
  id: number;
  title: string;
  x: number;
  y: number;
  /** The agent bay under the pointer, if any. */
  overBay: BayId | null;
}

export const useDrag = create<{ mission: MissionDrag | null }>(() => ({ mission: null }));

/** The bay under a screen point (bays mark their drop zone with `data-bay-drop`). */
export function bayAt(x: number, y: number): BayId | null {
  const el = document.elementFromPoint(x, y)?.closest('[data-bay-drop]');
  const id = el?.getAttribute('data-bay-drop');
  return id ? Number(id) : null;
}

/**
 * Start dragging a mission from a pointerdown. The drag begins once the pointer moves a few
 * pixels (so clicks still click); `onDrop` gets the bay it was released over.
 */
export function startMissionDrag(e: React.PointerEvent, id: number, title: string, onDrop: (bay: BayId) => void) {
  if (e.button !== 0) return;
  const sx = e.clientX;
  const sy = e.clientY;
  let dragging = false;
  const move = (ev: PointerEvent) => {
    if (!dragging && Math.hypot(ev.clientX - sx, ev.clientY - sy) < 5) return;
    dragging = true;
    document.body.classList.add('dragging-mission');
    useDrag.setState({ mission: { id, title, x: ev.clientX, y: ev.clientY, overBay: bayAt(ev.clientX, ev.clientY) } });
  };
  const end = (ev: PointerEvent) => {
    window.removeEventListener('pointermove', move);
    window.removeEventListener('pointerup', end);
    window.removeEventListener('pointercancel', cancel);
    document.body.classList.remove('dragging-mission');
    const bay = dragging ? bayAt(ev.clientX, ev.clientY) : null;
    useDrag.setState({ mission: null });
    if (dragging) {
      // Swallow the click that follows a drag (if any: none fires when released elsewhere).
      const swallow = (c: MouseEvent) => {
        c.stopPropagation();
        c.preventDefault();
      };
      window.addEventListener('click', swallow, true);
      window.setTimeout(() => window.removeEventListener('click', swallow, true), 0);
    }
    if (bay != null) onDrop(bay);
  };
  const cancel = () => {
    window.removeEventListener('pointermove', move);
    window.removeEventListener('pointerup', end);
    window.removeEventListener('pointercancel', cancel);
    document.body.classList.remove('dragging-mission');
    useDrag.setState({ mission: null });
  };
  window.addEventListener('pointermove', move);
  window.addEventListener('pointerup', end);
  window.addEventListener('pointercancel', cancel);
}
