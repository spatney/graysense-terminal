import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { act, newSession } from '../../lib/api';
import { ago, agoLong, baseName, dateGroup, shortPath } from '../../lib/format';
import { focusComposer, openOverlay, useUi } from '../../lib/store';
import type { BayDto, BayId, Level, PastSessionDto } from '../../lib/types';
import { copyText, Icon, type MenuItem, showMenu } from '../ui';
import '../../styles/panels/sessions.css';

const LEVEL_ICON: Record<Level, string> = { info: 'circle-small-filled', ok: 'pass', warn: 'warning', error: 'error' };
const MAX_ACTIVITY = 100;
/** Refetch when the window comes back (sessions started in the Copilot CLI appear), at most this often. */
const FOCUS_REFRESH_MS = 20_000;
/** Session rows drawn at first, and added each time the list is scrolled near its end. */
const PAGE_ROWS = 150;

type Entry =
  | { kind: 'head'; key: string; label: string; count?: number }
  | { kind: 'open'; key: string; bay: BayDto; session: PastSessionDto | null }
  | { kind: 'past'; key: string; session: PastSessionDto }
  | { kind: 'search'; key: string };

type MenuAt = Element | { clientX: number; clientY: number };

function dotClass(b: BayDto): string {
  const s = b.agent?.status;
  if (!s) return 'idle';
  if (s.attention) return 'attention';
  if (s.kind === 'error') return 'error';
  if (s.kind === 'offline' || s.kind === 'booting') return s.kind;
  return s.busy ? 'busy' : 'idle';
}

function clip(s: string, n: number): string {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}

/** Re-render now and then so relative times stay fresh. */
function useTick(ms: number) {
  const [, setN] = useState(0);
  useEffect(() => {
    const t = window.setInterval(() => setN((n) => n + 1), ms);
    return () => window.clearInterval(t);
  }, [ms]);
}

/**
 * Keeps a row highlighted while its menu is open. Menus don't report closing, so the next
 * click anywhere, Escape/Enter/Tab, or the window losing focus ends it.
 */
function useMenuOwner<T>(): [T | null, (owner: T) => void] {
  const [owner, setOwner] = useState<T | null>(null);
  useEffect(() => {
    if (owner === null) return;
    const end = () => setOwner(null);
    const key = (e: KeyboardEvent) => {
      if (e.key === 'Escape' || e.key === 'Enter' || e.key === 'Tab') end();
    };
    window.addEventListener('pointerdown', end, true);
    window.addEventListener('keydown', key, true);
    window.addEventListener('blur', end);
    window.addEventListener('resize', end);
    return () => {
      window.removeEventListener('pointerdown', end, true);
      window.removeEventListener('keydown', key, true);
      window.removeEventListener('blur', end);
      window.removeEventListener('resize', end);
    };
  }, [owner]);
  return [owner, setOwner];
}

function focusBay(bay: BayId) {
  void act({ type: 'focusBay', bay });
  focusComposer(bay);
}

async function resume(id: string) {
  const bay = await act<BayId>({ type: 'resumeSession', id });
  if (typeof bay === 'number') focusComposer(bay);
}

function confirmDelete(id: string, title: string) {
  openOverlay({
    kind: 'confirm',
    title: `Delete “${clip(title, 60)}”?`,
    body: 'Its conversation is deleted; files in its folder are kept.',
    confirm: 'Delete',
    danger: true,
    onConfirm: () => void act({ type: 'deleteSession', id }),
  });
}

/** Open agents first, then pinned sessions, then the rest by date. Each session shows up once. */
function buildEntries(items: PastSessionDto[], agents: BayDto[], q: string, now: Date): Entry[] {
  // Folders match by name: their full paths share segments ("C:\Users\…\code") that would match everything.
  const match = (title: string, ...folders: (string | null | undefined)[]) =>
    !q || title.toLowerCase().includes(q) || folders.some((f) => !!f && baseName(f).toLowerCase().includes(q));
  const byId = new Map(items.map((s) => [s.id, s]));
  const openBays = new Set(agents.map((b) => b.id));
  const openIds = new Set<string>();
  for (const b of agents) if (b.agent?.sessionId) openIds.add(b.agent.sessionId);
  const out: Entry[] = [];

  const live = agents
    .map((bay) => {
      const sid = bay.agent?.sessionId;
      const session = (sid ? byId.get(sid) : undefined) ?? items.find((s) => s.openBay === bay.id) ?? null;
      return { bay, session };
    })
    .filter(({ bay, session }) => match(bay.title, bay.cwd, session?.folder) || (!!session && match(session.title)));
  if (live.length) {
    out.push({ kind: 'head', key: 'h:open', label: 'Open now', count: live.length });
    for (const l of live) out.push({ kind: 'open', key: `b:${l.bay.id}`, bay: l.bay, session: l.session });
  }

  const past = items
    .filter((s) => !openIds.has(s.id) && !(s.openBay != null && openBays.has(s.openBay)))
    .filter((s) => match(s.title, s.folder))
    .sort((a, b) => b.modifiedTs - a.modifiedTs);
  const pinned = past.filter((s) => s.pinned);
  if (pinned.length) {
    out.push({ kind: 'head', key: 'h:pinned', label: 'Pinned' });
    for (const s of pinned) out.push({ kind: 'past', key: `s:${s.id}`, session: s });
  }
  let group = '';
  for (const s of past) {
    if (s.pinned) continue;
    const g = dateGroup(s.modifiedTs, now);
    if (g !== group) {
      group = g;
      out.push({ kind: 'head', key: `h:${g}`, label: g });
    }
    out.push({ kind: 'past', key: `s:${s.id}`, session: s });
  }
  if (q) out.push({ kind: 'search', key: 'search' });
  return out;
}

// ------------------------------------------------------------------ rows

function ActionButton({ icon, tip, onClick }: { icon: string; tip: string; onClick: () => void }) {
  return (
    <button
      type="button"
      className="icon-btn sm"
      aria-label={tip}
      data-tip={tip}
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
    >
      <Icon name={icon} />
    </button>
  );
}

function RenameInput({ initial, onDone }: { initial: string; onDone: (name: string | null, byKeyboard: boolean) => void }) {
  const [value, setValue] = useState(initial);
  const ref = useRef<HTMLInputElement>(null);
  const done = useRef(false);
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);
  const finish = (name: string | null, byKeyboard: boolean) => {
    if (done.current) return;
    done.current = true;
    onDone(name, byKeyboard);
  };
  return (
    <input
      ref={ref}
      className="input ses-rename"
      value={value}
      placeholder="Automatic name"
      aria-label="Session name"
      spellCheck={false}
      onChange={(e) => setValue(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === 'Enter') {
          e.preventDefault();
          finish(value, true);
        } else if (e.key === 'Escape') {
          e.preventDefault();
          e.stopPropagation();
          finish(null, true);
        }
      }}
      onBlur={() => finish(value, false)}
    />
  );
}

interface RowProps {
  rowKey: string;
  title: string;
  tip?: string;
  /** The row Tab lands on (roving tab stop). */
  tabStop: boolean;
  className?: string;
  lead?: ReactNode;
  after?: ReactNode;
  meta?: ReactNode;
  actions?: ReactNode;
  menuOpen: boolean;
  renaming: boolean;
  onOpen: () => void;
  onMenu: (at: MenuAt, alignRight: boolean) => void;
  onRename: () => void;
  onRenamed: (name: string | null, byKeyboard: boolean) => void;
  onDelete?: () => void;
  onFocus: () => void;
}

function SessionRow(p: RowProps) {
  if (p.renaming) {
    return (
      <div className="list-row ses-row renaming" data-ses-key={p.rowKey}>
        <RenameInput initial={p.title} onDone={p.onRenamed} />
      </div>
    );
  }
  return (
    <div
      role="button"
      tabIndex={p.tabStop ? 0 : -1}
      data-ses-row=""
      data-ses-key={p.rowKey}
      className={`list-row ses-row${p.className ? ` ${p.className}` : ''}${p.menuOpen ? ' menu' : ''}`}
      onClick={p.onOpen}
      onFocus={p.onFocus}
      onContextMenu={(e) => {
        e.preventDefault();
        p.onMenu(e.clientX || e.clientY ? e : e.currentTarget, false);
      }}
      onKeyDown={(e) => {
        if (e.target !== e.currentTarget) return;
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          p.onOpen();
        } else if (e.key === 'F2') {
          e.preventDefault();
          p.onRename();
        } else if (e.key === 'Delete' && p.onDelete) {
          e.preventDefault();
          p.onDelete();
        }
      }}
    >
      {p.lead}
      <span className="ses-title" data-tip={p.tip}>
        {p.title}
      </span>
      {p.after}
      {p.meta}
      <span className="row-actions">
        {p.actions}
        <button
          type="button"
          className="icon-btn sm"
          aria-label="More actions"
          data-tip="More"
          onClick={(e) => {
            e.stopPropagation();
            p.onMenu(e.currentTarget, true);
          }}
        >
          <Icon name="ellipsis" />
        </button>
      </span>
    </div>
  );
}

function Skeleton() {
  return (
    <div className="ses-skeleton" aria-hidden>
      <span className="ses-skel-head" />
      {[72, 54, 64, 46, 58, 38].map((w, i) => (
        <span key={i} className="ses-skel-row">
          <span style={{ width: `${w}%` }} />
        </span>
      ))}
    </div>
  );
}

// ------------------------------------------------------------------ activity

function Activity() {
  const activity = useUi((s) => s.activity);
  const bays = useUi((s) => s.bays);
  const [open, setOpen] = useState(false);
  const titles = useMemo(() => new Map(bays.map((b) => [b.id, b.title])), [bays]);
  const latest = activity[0];

  const lines: ReactNode[] = [];
  if (open) {
    const seen = new Map<string, number>();
    for (const l of activity.slice(0, MAX_ACTIVITY)) {
      const base = `${l.at}:${l.bay}:${l.text}`;
      const n = seen.get(base) ?? 0;
      seen.set(base, n + 1);
      const title = titles.get(l.bay);
      lines.push(
        <button
          key={n ? `${base}#${n}` : base}
          type="button"
          className={`ses-act-line${title == null ? ' closed' : ''}`}
          aria-disabled={title == null}
          data-tip={title != null ? `${l.text} · ${title}` : l.text}
          onClick={() => {
            if (title != null) void act({ type: 'focusBay', bay: l.bay });
          }}
        >
          <Icon name={LEVEL_ICON[l.level] ?? 'circle-small-filled'} className={`ses-lvl ${l.level}`} />
          <span className="ses-act-text">{l.text}</span>
          {title != null && <span className="ses-act-bay">{title}</span>}
          <span className="ses-act-time">{ago(l.at / 1000)}</span>
        </button>,
      );
    }
  }

  return (
    <div className={`ses-activity${open ? ' open' : ''}`}>
      <button type="button" className="ses-activity-head" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        <Icon name="chevron-right" className="ses-activity-chev" />
        <span className="section-label">Activity</span>
        {!open && latest ? (
          <>
            <span className="ses-activity-peek">
              <Icon name={LEVEL_ICON[latest.level] ?? 'circle-small-filled'} className={`ses-lvl ${latest.level}`} />
              <span className="ellipsis">{latest.text}</span>
            </span>
            <span className="ses-act-time">{ago(latest.at / 1000)}</span>
          </>
        ) : (
          <span className="grow" />
        )}
      </button>
      {open && (
        <div className="ses-activity-list" role="log" aria-label="Agent activity">
          {lines.length ? lines : <div className="ses-activity-empty">What your agents do shows up here.</div>}
        </div>
      )}
    </div>
  );
}

// ------------------------------------------------------------------ panel

export function SessionsPanel() {
  useTick(30_000);
  const sessions = useUi((s) => s.sessions);
  const bays = useUi((s) => s.bays);
  const focused = useUi((s) => s.app?.focused ?? null);
  const home = useUi((s) => s.app?.home ?? null);
  const [filter, setFilter] = useState('');
  const [renaming, setRenaming] = useState<string | null>(null);
  const [menuFor, setMenuFor] = useMenuOwner<string>();
  const [current, setCurrent] = useState<string | null>(null);
  const [spinning, setSpinning] = useState(false);
  const listRef = useRef<HTMLDivElement>(null);
  const filterRef = useRef<HTMLInputElement>(null);
  const asked = useRef(false);

  const agents = useMemo(() => bays.filter((b) => b.kind === 'agent'), [bays]);
  const q = filter.trim().toLowerCase();
  const day = new Date().toDateString();
  const entries = useMemo(() => buildEntries(sessions.items, agents, q, new Date()), [sessions.items, agents, q, day]);
  // A long history draws a page at a time, adding more as the list nears its end.
  const [shown, setShown] = useState(PAGE_ROWS);
  useEffect(() => setShown(PAGE_ROWS), [q]);
  const sentinel = useCallback((el: HTMLDivElement | null) => {
    if (!el) return;
    const io = new IntersectionObserver((seen) => seen.some((e) => e.isIntersecting) && setShown((n) => n + PAGE_ROWS), { root: el.parentElement, rootMargin: '600px 0px' });
    io.observe(el);
    return () => io.disconnect();
  }, []);

  useEffect(() => {
    let last = 0;
    const refresh = () => {
      if (Date.now() - last < FOCUS_REFRESH_MS) return;
      last = Date.now();
      void act({ type: 'refreshSessions' });
    };
    if (!asked.current) {
      asked.current = true;
      refresh();
    } else last = Date.now();
    window.addEventListener('focus', refresh);
    return () => window.removeEventListener('focus', refresh);
  }, []);

  useEffect(() => {
    if (!spinning) return;
    const t = window.setTimeout(() => setSpinning(false), 900);
    return () => window.clearTimeout(t);
  }, [spinning]);

  const refresh = () => {
    setSpinning(true);
    void act({ type: 'refreshSessions' });
  };

  const rowEls = () => Array.from(listRef.current?.querySelectorAll<HTMLElement>('[data-ses-row]') ?? []);
  const focusEl = (el: HTMLElement | null | undefined) => {
    if (!el) return;
    el.focus({ preventScroll: true });
    el.scrollIntoView({ block: 'nearest' });
  };
  const focusKey = (key: string) => focusEl(listRef.current?.querySelector<HTMLElement>(`[data-ses-row][data-ses-key="${CSS.escape(key)}"]`));

  const openEntry = (en: Entry) => {
    if (en.kind === 'open') focusBay(en.bay.id);
    else if (en.kind === 'past') void resume(en.session.id);
    else if (en.kind === 'search') openOverlay({ kind: 'search', query: filter.trim() });
  };

  const menu = (key: string, at: MenuAt, alignRight: boolean, items: MenuItem[]) => {
    setMenuFor(key);
    showMenu(at, items, { alignRight });
  };

  /** Finish an inline rename: an empty name goes back to the automatic one. */
  const renamed = (key: string, title: string, target: { id: string } | { bay: BayId }) => (name: string | null, byKeyboard: boolean) => {
    setRenaming(null);
    if (byKeyboard) requestAnimationFrame(() => focusKey(key));
    if (name == null) return;
    const next = name.trim();
    if (next === title.trim()) return;
    if ('id' in target) void act({ type: 'renameSession', id: target.id, name: next });
    else void act({ type: 'rename', bay: target.bay, name: next });
  };

  const stops = entries.filter((e) => e.kind !== 'head').map((e) => e.key);
  const tabStop = current && stops.includes(current) ? current : (stops[0] ?? null);

  const openRow = (en: Extract<Entry, { kind: 'open' }>) => {
    const b = en.bay;
    const a = b.agent;
    const s = en.session;
    const sid = s?.id ?? a?.sessionId ?? null;
    const st = a?.status;
    const pinned = !!s?.pinned;
    const items = (): MenuItem[] => {
      const list: MenuItem[] = [{ label: 'Rename', icon: 'edit', hint: 'F2', onSelect: () => setRenaming(en.key) }];
      if (sid) list.push({ label: pinned ? 'Unpin' : 'Pin', icon: pinned ? 'pinned' : 'pin', onSelect: () => void act({ type: 'pinSession', id: sid, pinned: !pinned }) });
      if (!a?.deferred) list.push({ label: 'Reveal folder', icon: 'folder-opened', onSelect: () => void act({ type: 'revealPath', path: b.cwd }) });
      if (sid) list.push({ label: 'Copy session id', icon: 'copy', onSelect: () => void copyText(sid, 'Session id copied') });
      list.push({ sep: true }, { label: 'Close', icon: 'close', onSelect: () => void act({ type: 'closeBay', bay: b.id }) });
      if (sid) list.push({ label: 'Delete…', icon: 'trash', hint: 'Del', danger: true, onSelect: () => confirmDelete(sid, b.title) });
      return list;
    };
    const tip = [b.title, st?.label, !a?.deferred && shortPath(b.cwd, home)].filter(Boolean).join(' · ');
    return (
      <SessionRow
        key={en.key}
        rowKey={en.key}
        title={b.title}
        tip={tip}
        tabStop={tabStop === en.key}
        className={`ses-live${b.id === focused ? ' on' : ''}`}
        lead={<span className={`dot ${dotClass(b)}`} />}
        after={
          <>
            {a?.incognito && (
              <span className="ses-mark" data-tip="Incognito">
                <Icon name="eye-closed" />
              </span>
            )}
            {pinned && (
              <span className="ses-mark" data-tip="Pinned">
                <Icon name="pinned" />
              </span>
            )}
            {!!a?.pending && <span className="count">{a.pending}</span>}
          </>
        }
        meta={st && <span className={`row-meta ses-status${st.busy ? ' shimmer' : ''}${st.attention ? ' attention' : ''}${st.kind === 'error' ? ' error' : ''}`}>{st.label}</span>}
        actions={<ActionButton icon="close" tip="Close" onClick={() => void act({ type: 'closeBay', bay: b.id })} />}
        menuOpen={menuFor === en.key}
        renaming={renaming === en.key}
        onOpen={() => focusBay(b.id)}
        onMenu={(at, right) => menu(en.key, at, right, items())}
        onRename={() => setRenaming(en.key)}
        onRenamed={renamed(en.key, b.title, sid ? { id: sid } : { bay: b.id })}
        onDelete={sid ? () => confirmDelete(sid, b.title) : undefined}
        onFocus={() => setCurrent(en.key)}
      />
    );
  };

  const pastRow = (en: Extract<Entry, { kind: 'past' }>) => {
    const s = en.session;
    const title = s.title || 'Untitled session';
    const pin = () => void act({ type: 'pinSession', id: s.id, pinned: !s.pinned });
    const items = (): MenuItem[] => {
      const list: MenuItem[] = [
        { label: 'Rename', icon: 'edit', hint: 'F2', onSelect: () => setRenaming(en.key) },
        { label: s.pinned ? 'Unpin' : 'Pin', icon: s.pinned ? 'pinned' : 'pin', onSelect: pin },
      ];
      if (s.folder) {
        const folder = s.folder;
        list.push({ label: 'Reveal folder', icon: 'folder-opened', onSelect: () => void act({ type: 'revealPath', path: folder }) });
      }
      list.push({ label: 'Copy session id', icon: 'copy', onSelect: () => void copyText(s.id, 'Session id copied') });
      list.push({ sep: true }, { label: 'Delete…', icon: 'trash', hint: 'Del', danger: true, onSelect: () => confirmDelete(s.id, title) });
      return list;
    };
    const tip = [title, s.folder && shortPath(s.folder, home), agoLong(s.modifiedTs)].filter(Boolean).join(' · ');
    return (
      <SessionRow
        key={en.key}
        rowKey={en.key}
        title={title}
        tip={tip}
        tabStop={tabStop === en.key}
        className="ses-past"
        after={
          s.external && (
            <span className="badge ses-cli" data-tip="Started in the Copilot CLI">
              CLI
            </span>
          )
        }
        meta={<span className="row-meta">{ago(s.modifiedTs)}</span>}
        actions={<ActionButton icon={s.pinned ? 'pinned' : 'pin'} tip={s.pinned ? 'Unpin' : 'Pin'} onClick={pin} />}
        menuOpen={menuFor === en.key}
        renaming={renaming === en.key}
        onOpen={() => void resume(s.id)}
        onMenu={(at, right) => menu(en.key, at, right, items())}
        onRename={() => setRenaming(en.key)}
        onRenamed={renamed(en.key, title, { id: s.id })}
        onDelete={() => confirmDelete(s.id, title)}
        onFocus={() => setCurrent(en.key)}
      />
    );
  };

  const rows = entries.slice(0, shown).map((en) => {
    if (en.kind === 'head') {
      return (
        <div key={en.key} className="panel-section ses-sec">
          <span className="section-label">{en.label}</span>
          {en.count != null && <span className="ses-sec-count">{en.count}</span>}
        </div>
      );
    }
    if (en.kind === 'search') {
      return (
        <button
          key={en.key}
          type="button"
          data-ses-row=""
          data-ses-key={en.key}
          tabIndex={tabStop === en.key ? 0 : -1}
          className="list-row ses-row ses-search"
          onFocus={() => setCurrent(en.key)}
          onClick={() => openEntry(en)}
        >
          <Icon name="search" />
          <span className="ses-title">Search inside sessions for “{filter.trim()}”</span>
        </button>
      );
    }
    return en.kind === 'open' ? openRow(en) : pastRow(en);
  });

  const navigate = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const t = e.target as HTMLElement;
    if (t.closest('input, textarea')) return;
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp' && e.key !== 'Home' && e.key !== 'End') return;
    const els = rowEls();
    if (!els.length) return;
    e.preventDefault();
    const at = els.indexOf(t.closest<HTMLElement>('[data-ses-row]') as HTMLElement);
    if (e.key === 'ArrowUp' && at <= 0) return filterRef.current?.focus();
    const next = e.key === 'Home' ? 0 : e.key === 'End' ? els.length - 1 : e.key === 'ArrowDown' ? Math.min(els.length - 1, at + 1) : at - 1;
    focusEl(els[next]);
  };

  const filterKeys = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      focusEl(rowEls()[0]);
    } else if (e.key === 'Enter' && q) {
      e.preventDefault();
      const first = entries.find((x) => x.kind === 'open' || x.kind === 'past') ?? entries.find((x) => x.kind === 'search');
      if (first) openEntry(first);
    } else if (e.key === 'Escape') {
      if (filter) {
        e.preventDefault();
        e.stopPropagation();
        setFilter('');
      } else e.currentTarget.blur();
    }
  };

  const loading = !sessions.loaded;
  const matched = entries.some((x) => x.kind === 'open' || x.kind === 'past');
  const nothing = !loading && !q && sessions.items.length === 0 && agents.length === 0;

  return (
    <div className="panel ses-panel">
      <div className="panel-head">
        <h2>Sessions</h2>
        <button type="button" className="icon-btn" aria-label="New session" data-tip="New session" data-keys="Ctrl+Shift+N" onClick={() => void newSession()}>
          <Icon name="add" />
        </button>
        <button type="button" className="icon-btn" aria-label="Refresh" data-tip="Refresh" onClick={refresh}>
          <Icon name="refresh" className={spinning ? 'codicon-modifier-spin' : ''} />
        </button>
      </div>
      <div className="panel-toolbar">
        <div className="search-field ses-filter">
          <Icon name="search" />
          <input
            ref={filterRef}
            className="input"
            value={filter}
            placeholder="Filter sessions"
            aria-label="Filter sessions"
            spellCheck={false}
            onChange={(e) => setFilter(e.target.value)}
            onKeyDown={filterKeys}
          />
          {filter && (
            <button
              type="button"
              className="icon-btn sm ses-filter-clear"
              aria-label="Clear filter"
              data-tip="Clear"
              onClick={() => {
                setFilter('');
                filterRef.current?.focus();
              }}
            >
              <Icon name="close" />
            </button>
          )}
        </div>
      </div>
      <div ref={listRef} className="panel-body ses-list" aria-busy={loading} onKeyDown={navigate}>
        {nothing ? (
          <div className="empty ses-empty">
            <span className="ses-empty-icon">
              <Icon name="comment-discussion" />
            </span>
            <h4>No sessions yet</h4>
            <p>Conversations with your agents show up here, including ones started in the Copilot CLI.</p>
            <button type="button" className="btn primary sm" onClick={() => void newSession()}>
              <Icon name="add" />
              New session
            </button>
          </div>
        ) : (
          <>
            {q && !matched && !loading && <div className="ses-none">No sessions match “{filter.trim()}”.</div>}
            {rows}
            {entries.length > shown && <div ref={sentinel} className="ses-more" aria-hidden />}
            {loading && <Skeleton />}
          </>
        )}
      </div>
      <Activity />
    </div>
  );
}
