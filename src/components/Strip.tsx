import { useRef, useState } from 'react';
import { act, newAgentIn, newSession, newShell, newShellIn } from '../lib/api';
import { focusComposer, openOverlay, useUi } from '../lib/store';
import type { BayDto, LayoutId } from '../lib/types';
import { Icon, Kbd, type MenuItem, showMenu } from './ui';

const LAYOUTS: { id: LayoutId; label: string; icon: string }[] = [
  { id: 'grid', label: 'Grid', icon: 'layout' },
  { id: 'focus', label: 'One at a time', icon: 'screen-full' },
  { id: 'columns', label: 'Columns', icon: 'split-horizontal' },
  { id: 'rows', label: 'Rows', icon: 'split-vertical' },
];

export function statusClass(b: BayDto): string {
  const s = b.agent?.status;
  if (!s) return 'idle';
  if (s.attention) return 'attention';
  if (s.kind === 'error') return 'error';
  if (s.kind === 'offline' || s.kind === 'booting') return s.kind;
  return s.busy ? 'busy' : 'idle';
}

export function newMenu(anchor: Element) {
  const items: MenuItem[] = [
    { label: 'New session', icon: 'comment-discussion', hint: 'Ctrl+Shift+N', onSelect: () => void newSession() },
    { label: 'New incognito session', icon: 'eye-closed', onSelect: () => void newSession(undefined, true) },
    { label: 'New agent in folder…', icon: 'folder-opened', onSelect: () => void newAgentIn() },
    { sep: true },
    { label: 'New terminal', icon: 'terminal', hint: 'Ctrl+Shift+T', onSelect: () => void newShell() },
    { label: 'New terminal in folder…', icon: 'folder', onSelect: () => void newShellIn() },
  ];
  showMenu(anchor, items);
}

function Tab({ b, on, index, onDragStart, dropSide }: { b: BayDto; on: boolean; index: number; onDragStart: (e: React.PointerEvent, b: BayDto) => void; dropSide: 'before' | 'after' | null }) {
  const icon = b.kind === 'shell' ? 'terminal' : b.agent?.incognito ? 'eye-closed' : null;
  const tip = b.kind === 'shell' ? `${b.title} · ${b.folder}` : `${b.title}${b.agent ? ` · ${b.agent.status.label}` : ''}`;
  const menu = (e: React.MouseEvent) => {
    e.preventDefault();
    const items: MenuItem[] = [
      { label: 'Close', icon: 'close', hint: 'Ctrl+Shift+W', onSelect: () => void act({ type: 'closeBay', bay: b.id }) },
      { label: 'Close others', icon: 'close-all', onSelect: () => useUi.getState().bays.filter((x) => x.id !== b.id).forEach((x) => void act({ type: 'closeBay', bay: x.id })) },
      { sep: true },
      { label: 'Maximize', icon: 'screen-full', hint: 'Ctrl+Shift+Enter', onSelect: () => void act({ type: 'maximizeBay', bay: b.id }) },
      { label: 'Reveal folder', icon: 'folder-opened', onSelect: () => void act({ type: 'revealPath', path: b.cwd }) },
      { label: 'New terminal here', icon: 'terminal', onSelect: () => void newShell(b.cwd) },
    ];
    showMenu(e, items);
  };
  return (
    <div
      role="tab"
      aria-selected={on}
      tabIndex={0}
      className={`tab ${on ? 'on' : ''} ${dropSide ? `drop-${dropSide}` : ''}`}
      data-tab-index={index}
      data-tip={tip}
      onPointerDown={(e) => onDragStart(e, b)}
      onClick={() => {
        void act({ type: 'focusBay', bay: b.id });
        if (b.kind === 'agent') focusComposer(b.id);
      }}
      onAuxClick={(e) => e.button === 1 && void act({ type: 'closeBay', bay: b.id })}
      onContextMenu={menu}
    >
      {icon ? <Icon name={icon} /> : <span className={`dot ${statusClass(b)}`} />}
      <span className="tab-title">{b.title}</span>
      {b.agent && b.agent.pending > 0 && <span className="count">{b.agent.pending}</span>}
      <button
        type="button"
        className="icon-btn sm tab-close"
        aria-label="Close"
        onPointerDown={(e) => e.stopPropagation()}
        onClick={(e) => {
          e.stopPropagation();
          void act({ type: 'closeBay', bay: b.id });
        }}
      >
        <Icon name="close" />
      </button>
    </div>
  );
}

export function Strip() {
  const bays = useUi((s) => s.bays);
  const focused = useUi((s) => s.app?.focused);
  const layout = useUi((s) => s.settings?.layout ?? 'grid');
  const [drag, setDrag] = useState<{ id: number; over: number; side: 'before' | 'after' } | null>(null);
  const dragRef = useRef(drag);
  dragRef.current = drag;

  const startDrag = (e: React.PointerEvent, b: BayDto) => {
    if (e.button !== 0) return;
    const sx = e.clientX;
    let moving = false;
    const move = (ev: PointerEvent) => {
      if (!moving && Math.abs(ev.clientX - sx) < 6) return;
      moving = true;
      const el = document.elementFromPoint(ev.clientX, ev.clientY)?.closest('[data-tab-index]') as HTMLElement | null;
      if (!el) return;
      const r = el.getBoundingClientRect();
      setDrag({ id: b.id, over: Number(el.dataset.tabIndex), side: ev.clientX < r.left + r.width / 2 ? 'before' : 'after' });
    };
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      const d = dragRef.current;
      setDrag(null);
      if (!moving || !d) return;
      const from = bays.findIndex((x) => x.id === d.id);
      let to = d.side === 'before' ? d.over : d.over + 1;
      if (from < to) to -= 1;
      if (to !== from) void act({ type: 'moveBay', bay: d.id, to });
      const swallow = (c: MouseEvent) => c.stopPropagation();
      window.addEventListener('click', swallow, true);
      window.setTimeout(() => window.removeEventListener('click', swallow, true), 0);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };

  const layoutMenu = (anchor: Element) =>
    showMenu(
      anchor,
      [
        { section: 'Layout' },
        ...LAYOUTS.map((l) => ({ label: l.label, icon: l.icon, checked: layout === l.id, onSelect: () => void act({ type: 'setSettings', changes: { layout: l.id } }) })),
      ],
      { alignRight: true },
    );

  return (
    <header className="strip">
      <div className="tabs" role="tablist">
        {bays.map((b, i) => (
          <Tab key={b.id} b={b} index={i} on={b.id === focused} onDragStart={startDrag} dropSide={drag && drag.over === i && drag.id !== b.id ? drag.side : null} />
        ))}
      </div>
      <div className="strip-actions">
        <button type="button" className="icon-btn" data-tip="New session" data-keys="Ctrl+Shift+N" onClick={() => void newSession()}>
          <Icon name="add" />
        </button>
        <button type="button" className="icon-btn sm" data-tip="More ways to start" onClick={(e) => newMenu(e.currentTarget)}>
          <Icon name="chevron-down" />
        </button>
      </div>
      <button type="button" className="strip-search" onClick={() => openOverlay({ kind: 'search' })}>
        <Icon name="search" />
        <span>Search sessions and files</span>
        <Kbd keys={['Ctrl', 'K']} />
      </button>
      {bays.length > 1 && (
        <button type="button" className="icon-btn" data-tip="Layout" data-keys="Ctrl+Shift+L" onClick={(e) => layoutMenu(e.currentTarget)}>
          <Icon name={LAYOUTS.find((l) => l.id === layout)?.icon ?? 'layout'} />
        </button>
      )}
    </header>
  );
}
