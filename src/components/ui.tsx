import { Component, type ReactNode, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { create } from 'zustand';
import { toast } from '../lib/store';

export function Icon({ name, className = '', style }: { name: string; className?: string; style?: React.CSSProperties }) {
  return <i className={`codicon codicon-${name} ${className}`} style={style} aria-hidden />;
}

export function Kbd({ keys }: { keys: string[] }) {
  return (
    <span className="kbd">
      {keys.map((k) => (
        <kbd key={k}>{k}</kbd>
      ))}
    </span>
  );
}

export function Toggle({ on, onChange, disabled, label }: { on: boolean; onChange: (on: boolean) => void; disabled?: boolean; label?: string }) {
  return <button type="button" role="switch" aria-checked={on} aria-label={label} className={`toggle ${on ? 'on' : ''}`} disabled={disabled} onClick={() => onChange(!on)} />;
}

export function Seg<T extends string>({ value, options, onChange, size }: { value: T; options: { value: T; label: ReactNode; tip?: string }[]; onChange: (v: T) => void; size?: 'sm' }) {
  return (
    <div className={`seg ${size ?? ''}`} role="radiogroup">
      {options.map((o) => (
        <button key={o.value} type="button" role="radio" aria-checked={value === o.value} className={value === o.value ? 'on' : ''} data-tip={o.tip} onClick={() => onChange(o.value)}>
          {o.label}
        </button>
      ))}
    </div>
  );
}

// ------------------------------------------------------------------ tooltips

/** Tooltips for any element with `data-tip` (and optional `data-keys="Ctrl+K"`). */
export function Tooltips() {
  const [tip, setTip] = useState<{ text: string; keys: string[]; x: number; y: number; below: boolean } | null>(null);
  const timer = useRef<number>(0);
  const target = useRef<Element | null>(null);
  useEffect(() => {
    const hide = () => {
      window.clearTimeout(timer.current);
      target.current = null;
      setTip(null);
    };
    const over = (e: PointerEvent) => {
      const el = (e.target as Element | null)?.closest?.('[data-tip]');
      if (el === target.current) return;
      hide();
      if (!el) return;
      const text = el.getAttribute('data-tip');
      if (!text) return;
      target.current = el;
      timer.current = window.setTimeout(() => {
        if (!el.isConnected) return;
        const r = el.getBoundingClientRect();
        const below = r.top < 48;
        setTip({ text, keys: (el.getAttribute('data-keys') ?? '').split('+').filter(Boolean), x: r.left + r.width / 2, y: below ? r.bottom + 6 : r.top - 6, below });
      }, 420);
    };
    window.addEventListener('pointerover', over);
    window.addEventListener('pointerdown', hide, true);
    window.addEventListener('keydown', hide, true);
    window.addEventListener('wheel', hide, true);
    return () => {
      window.removeEventListener('pointerover', over);
      window.removeEventListener('pointerdown', hide, true);
      window.removeEventListener('keydown', hide, true);
      window.removeEventListener('wheel', hide, true);
    };
  }, []);
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  useLayoutEffect(() => {
    if (!tip || !ref.current) return setPos(null);
    const w = ref.current.offsetWidth;
    const h = ref.current.offsetHeight;
    const left = Math.min(Math.max(6, tip.x - w / 2), window.innerWidth - w - 6);
    setPos({ left, top: tip.below ? tip.y : tip.y - h });
  }, [tip]);
  if (!tip) return null;
  return createPortal(
    <div ref={ref} className="tooltip" style={pos ? { left: pos.left, top: pos.top } : { visibility: 'hidden', left: 0, top: 0 }}>
      <span>{tip.text}</span>
      {tip.keys.length > 0 && <Kbd keys={tip.keys} />}
    </div>,
    document.body,
  );
}

// ------------------------------------------------------------------ menus

/**
 * A menu row. `detail` is dim text after the label that gives way first when space is short (a
 * folder's location); `tip` is a tooltip. A `submenu` opens beside its row (one level deep).
 */
export type MenuItem =
  | { label: string; icon?: string; hint?: string; detail?: string; tip?: string; danger?: boolean; checked?: boolean; disabled?: boolean; onSelect: () => void }
  | { label: string; icon?: string; tip?: string; disabled?: boolean; submenu: MenuItem[] }
  | { sep: true }
  | { section: string };

interface MenuState {
  items: MenuItem[];
  x: number;
  y: number;
  /** Align the menu's right edge with x. */
  alignRight?: boolean;
  /** Open above y instead of below. */
  above?: boolean;
}

const useMenuStore = create<{ menu: MenuState | null }>(() => ({ menu: null }));

/** Open a menu at the pointer (`MouseEvent`) or under an element. */
export function showMenu(at: { clientX: number; clientY: number } | Element, items: MenuItem[], opts: { alignRight?: boolean; above?: boolean } = {}) {
  if (at instanceof Element) {
    const r = at.getBoundingClientRect();
    useMenuStore.setState({ menu: { items, x: opts.alignRight ? r.right : r.left, y: opts.above ? r.top - 4 : r.bottom + 4, ...opts } });
  } else {
    useMenuStore.setState({ menu: { items, x: at.clientX, y: at.clientY, ...opts } });
  }
}

export function closeMenu() {
  useMenuStore.setState({ menu: null });
}

/** Rows that can be highlighted: actions and submenus that aren't disabled. */
function selectable(items: MenuItem[]): number[] {
  return items.flatMap((it, i) => (('onSelect' in it || 'submenu' in it) && !it.disabled ? [i] : []));
}

/** The selectable row after (or before) `from`, wrapping around; from none, the first (or last). */
function stepRow(rows: number[], from: number, down: boolean): number {
  if (!rows.length) return -1;
  const at = rows.indexOf(from);
  if (at < 0) return down ? rows[0] : rows[rows.length - 1];
  return rows[(at + (down ? 1 : -1) + rows.length) % rows.length];
}

function MenuRows({ items, hot, open, onHover, onPick }: { items: MenuItem[]; hot: number; open?: number; onHover: (i: number) => void; onPick: (i: number) => void }) {
  return (
    <>
      {items.map((it, i) => {
        if ('sep' in it) return <div key={i} className="menu-sep" />;
        if ('section' in it) return <div key={i} className="menu-label">{it.section}</div>;
        if ('submenu' in it) {
          return (
            <button key={i} type="button" role="menuitem" aria-haspopup="menu" aria-expanded={open === i} data-row={i} data-tip={it.tip} className={`menu-item ${hot === i || open === i ? 'hot' : ''}`} disabled={it.disabled} onPointerEnter={() => onHover(i)} onClick={() => onPick(i)}>
              <Icon name={it.icon ?? 'blank'} />
              <span className="ellipsis">{it.label}</span>
              <Icon name="chevron-right" className="menu-more" />
            </button>
          );
        }
        return (
          <button key={i} type="button" role="menuitem" data-row={i} data-tip={it.tip} className={`menu-item ${it.danger ? 'danger' : ''} ${hot === i ? 'hot' : ''} ${it.checked ? 'checked' : ''}`} disabled={it.disabled} onPointerEnter={() => onHover(i)} onClick={() => onPick(i)}>
            <Icon name={it.checked ? 'check' : (it.icon ?? 'blank')} />
            <span className="ellipsis">{it.label}</span>
            {it.detail && <span className="menu-detail">{it.detail}</span>}
            {it.hint && <span className="hint">{it.hint}</span>}
          </button>
        );
      })}
    </>
  );
}

/** How long the pointer rests on a row before its submenu opens. */
const SUBMENU_OPEN_MS = 90;
/** Longer while another submenu is open, so moving diagonally toward that one doesn't switch it. */
const SUBMENU_SWITCH_MS = 220;

export function MenuHost() {
  const menu = useMenuStore((s) => s.menu);
  const host = useRef<HTMLDivElement>(null);
  const ref = useRef<HTMLDivElement>(null);
  const subRef = useRef<HTMLDivElement>(null);
  const timer = useRef(0);
  const [hot, setHot] = useState(-1);
  // The open submenu: its row in the menu, and its own highlighted row (-1 until the keyboard or pointer is in it).
  const [sub, setSub] = useState<{ at: number; hot: number } | null>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const [subPos, setSubPos] = useState<{ left: number; top: number } | null>(null);
  const subAt = sub?.at ?? -1;
  useLayoutEffect(() => {
    setHot(-1);
    setSub(null);
    window.clearTimeout(timer.current);
    if (!menu || !ref.current) return setPos(null);
    const w = ref.current.offsetWidth;
    const h = ref.current.offsetHeight;
    let left = menu.alignRight ? menu.x - w : menu.x;
    let top = menu.above ? menu.y - h : menu.y;
    left = Math.min(Math.max(6, left), window.innerWidth - w - 6);
    if (top + h > window.innerHeight - 6) top = Math.max(6, menu.y - h - 8);
    top = Math.max(6, top);
    setPos({ left, top });
    ref.current.focus();
  }, [menu]);
  useLayoutEffect(() => {
    const row = ref.current?.querySelector(`[data-row="${subAt}"]`);
    if (!row || !ref.current || !subRef.current) return setSubPos(null);
    const r = row.getBoundingClientRect();
    const box = ref.current.getBoundingClientRect();
    const w = subRef.current.offsetWidth;
    const h = subRef.current.offsetHeight;
    // Beside the menu, over its border; on the left when the right has no room.
    let left = box.right - 3;
    if (left + w > window.innerWidth - 6) left = Math.max(6, box.left - w + 3);
    setSubPos({ left, top: Math.max(6, Math.min(r.top - 6, window.innerHeight - h - 6)) });
  }, [subAt, pos]);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  useEffect(() => {
    if (!menu) return;
    const down = (e: PointerEvent) => {
      if (!host.current?.contains(e.target as Node)) closeMenu();
    };
    const blur = () => closeMenu();
    window.addEventListener('pointerdown', down, true);
    window.addEventListener('blur', blur);
    window.addEventListener('resize', blur);
    return () => {
      window.removeEventListener('pointerdown', down, true);
      window.removeEventListener('blur', blur);
      window.removeEventListener('resize', blur);
    };
  }, [menu]);
  if (!menu) return null;
  const parent = sub ? menu.items[sub.at] : undefined;
  const subItems = parent && 'submenu' in parent ? parent.submenu : null;
  const later = (fn: () => void, ms: number) => {
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(fn, ms);
  };
  const openSub = (i: number, keyboard: boolean) => {
    const it = menu.items[i];
    if (!it || !('submenu' in it) || it.disabled) return;
    window.clearTimeout(timer.current);
    setHot(i);
    setSub({ at: i, hot: keyboard ? stepRow(selectable(it.submenu), -1, true) : -1 });
  };
  const closeSub = () => {
    window.clearTimeout(timer.current);
    setSub(null);
    ref.current?.focus();
  };
  const hover = (i: number) => {
    setHot(i);
    const it = menu.items[i];
    if (sub?.at === i) window.clearTimeout(timer.current);
    else if (it && 'submenu' in it && !it.disabled) later(() => openSub(i, false), sub ? SUBMENU_SWITCH_MS : SUBMENU_OPEN_MS);
    else if (sub) later(closeSub, SUBMENU_SWITCH_MS);
    else window.clearTimeout(timer.current);
  };
  const run = (it: MenuItem | undefined) => {
    if (it && 'onSelect' in it && !it.disabled) {
      closeMenu();
      it.onSelect();
    }
  };
  const pick = (i: number) => {
    const it = menu.items[i];
    if (it && 'submenu' in it) openSub(i, false);
    else run(it);
  };
  const key = (e: React.KeyboardEvent) => {
    const down = e.key === 'ArrowDown';
    if (sub && sub.hot >= 0 && subItems) {
      if (down || e.key === 'ArrowUp') {
        e.preventDefault();
        setSub({ ...sub, hot: stepRow(selectable(subItems), sub.hot, down) });
      } else if (e.key === 'ArrowLeft' || e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        closeSub();
      } else if (e.key === 'Enter') {
        e.preventDefault();
        run(subItems[sub.hot]);
      }
      return;
    }
    if (e.key === 'Escape') {
      e.stopPropagation();
      if (sub) closeSub();
      else closeMenu();
    } else if (down || e.key === 'ArrowUp') {
      e.preventDefault();
      if (sub) closeSub();
      setHot(stepRow(selectable(menu.items), hot, down));
    } else if (e.key === 'ArrowRight' || e.key === 'Enter') {
      const it = menu.items[hot];
      if (it && 'submenu' in it) {
        e.preventDefault();
        openSub(hot, true);
      } else if (e.key === 'Enter' && hot >= 0) {
        e.preventDefault();
        run(it);
      }
    }
  };
  return createPortal(
    <div ref={host} className="menu-host" onKeyDown={key} onContextMenu={(e) => e.preventDefault()}>
      <div ref={ref} className="menu" role="menu" tabIndex={-1} style={pos ?? { left: -9999, top: -9999 }}>
        <MenuRows items={menu.items} hot={hot} open={sub?.at} onHover={hover} onPick={pick} />
      </div>
      {sub && subItems && (
        <div
          ref={subRef}
          className="menu menu-sub"
          role="menu"
          tabIndex={-1}
          style={subPos ?? { left: -9999, top: -9999 }}
          onPointerEnter={() => {
            window.clearTimeout(timer.current);
            setHot(sub.at);
          }}
        >
          <MenuRows items={subItems} hot={sub.hot} onHover={(j) => setSub((s) => (s ? { ...s, hot: j } : s))} onPick={(j) => run(subItems[j])} />
        </div>
      )}
    </div>,
    document.body,
  );
}

// ------------------------------------------------------------------ error boundary

interface BoundaryState {
  error: Error | null;
}

/** Keeps a crash in one part of the window (a panel, a bay) from taking down the rest. */
export class Boundary extends Component<{ children: ReactNode; label: string }, BoundaryState> {
  state: BoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): BoundaryState {
    return { error };
  }

  componentDidCatch(error: Error) {
    console.error(`${this.props.label} crashed`, error);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="empty" style={{ flex: 1 }}>
        <Icon name="warning" />
        <h4>{this.props.label} ran into a problem</h4>
        <p className="selectable">{this.state.error.message}</p>
        <button type="button" className="btn sm" onClick={() => this.setState({ error: null })}>
          Try again
        </button>
      </div>
    );
  }
}

// ------------------------------------------------------------------ modal

export function Modal({ onClose, children, width = 520, className = '', labelledBy }: { onClose: () => void; children: ReactNode; width?: number | 'auto'; className?: string; labelledBy?: string }) {
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener('keydown', key, true);
    return () => window.removeEventListener('keydown', key, true);
  }, [onClose]);
  return createPortal(
    <div className="scrim" onPointerDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className={`modal ${className}`} style={{ width }} role="dialog" aria-modal aria-labelledby={labelledBy}>
        {children}
      </div>
    </div>,
    document.body,
  );
}

/** Grow a textarea with its content, up to `max` pixels (re-measured when its width changes). */
export function useAutosize(ref: React.RefObject<HTMLTextAreaElement | null>, value: string, max = 320) {
  const fit = () => {
    const el = ref.current;
    if (!el) return;
    el.style.height = '0px';
    el.style.height = `${Math.min(el.scrollHeight, max)}px`;
    el.style.overflowY = el.scrollHeight > max ? 'auto' : 'hidden';
  };
  useLayoutEffect(fit, [ref, value, max]);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    let width = el.offsetWidth;
    const ro = new ResizeObserver(() => {
      if (el.offsetWidth !== width) {
        width = el.offsetWidth;
        fit();
      }
    });
    ro.observe(el);
    return () => ro.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ref, max]);
}

/** Copy text and say so. */
export async function copyText(text: string, what = 'Copied') {
  try {
    await navigator.clipboard.writeText(text);
    toast('ok', what);
  } catch {
    toast('error', "Couldn't copy to the clipboard");
  }
}
