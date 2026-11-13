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

export type MenuItem =
  | { label: string; icon?: string; hint?: string; danger?: boolean; checked?: boolean; disabled?: boolean; onSelect: () => void }
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

export function MenuHost() {
  const menu = useMenuStore((s) => s.menu);
  const ref = useRef<HTMLDivElement>(null);
  const [hot, setHot] = useState(-1);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  useLayoutEffect(() => {
    setHot(-1);
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
  useEffect(() => {
    if (!menu) return;
    const down = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) closeMenu();
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
  const actionable = menu.items.map((it, i) => ('onSelect' in it && !it.disabled ? i : -1)).filter((i) => i >= 0);
  const pick = (i: number) => {
    const it = menu.items[i];
    if (it && 'onSelect' in it && !it.disabled) {
      closeMenu();
      it.onSelect();
    }
  };
  const key = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.stopPropagation();
      closeMenu();
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const at = actionable.indexOf(hot);
      const next = e.key === 'ArrowDown' ? actionable[(at + 1) % actionable.length] : actionable[(at - 1 + actionable.length) % actionable.length];
      setHot(next ?? -1);
    } else if (e.key === 'Enter' && hot >= 0) {
      e.preventDefault();
      pick(hot);
    }
  };
  return createPortal(
    <div ref={ref} className="menu" role="menu" tabIndex={-1} onKeyDown={key} style={pos ?? { left: -9999, top: -9999 }} onContextMenu={(e) => e.preventDefault()}>
      {menu.items.map((it, i) => {
        if ('sep' in it) return <div key={i} className="menu-sep" />;
        if ('section' in it) return <div key={i} className="menu-label">{it.section}</div>;
        return (
          <button key={i} type="button" role="menuitem" className={`menu-item ${it.danger ? 'danger' : ''} ${hot === i ? 'hot' : ''} ${it.checked ? 'checked' : ''}`} disabled={it.disabled} onPointerEnter={() => setHot(i)} onClick={() => pick(i)}>
            <Icon name={it.checked ? 'check' : (it.icon ?? 'blank')} />
            <span className="ellipsis">{it.label}</span>
            {it.hint && <span className="hint">{it.hint}</span>}
          </button>
        );
      })}
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

export function Modal({ onClose, children, width = 520, className = '', labelledBy }: { onClose: () => void; children: ReactNode; width?: number; className?: string; labelledBy?: string }) {
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
