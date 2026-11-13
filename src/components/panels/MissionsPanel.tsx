import { type ReactNode, useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { act, pickFolder } from '../../lib/api';
import { startMissionDrag, useDrag } from '../../lib/drag';
import { ago, baseName } from '../../lib/format';
import { focusComposer, useUi } from '../../lib/store';
import type { BayDto, BayId, MissionDto, MissionStatus } from '../../lib/types';
import { Icon, type MenuItem, showMenu, useAutosize } from '../ui';
import '../../styles/panels/missions.css';

const SECTIONS: { status: MissionStatus; label: string }[] = [
  { status: 'active', label: 'In progress' },
  { status: 'review', label: 'Ready for review' },
  { status: 'idea', label: 'Ideas' },
  { status: 'done', label: 'Done' },
];

const MARKS: { status: MissionStatus; label: string; icon: string }[] = [
  { status: 'idea', label: 'Mark as idea', icon: 'lightbulb' },
  { status: 'active', label: 'Mark as in progress', icon: 'play-circle' },
  { status: 'review', label: 'Mark as ready for review', icon: 'eye' },
  { status: 'done', label: 'Mark as done', icon: 'pass' },
];

const HINT_KEY = 'gs-missions-drag-hint';
const COLLAPSED_KEY = 'gs-missions-collapsed';

/** The capture request last handled. Module-level: Ctrl+Shift+I asks before this panel has mounted. */
let seenCaptureTick = useUi.getState().captureTick;

function readPref(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writePref(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Storage unavailable: the preference just isn't kept.
  }
}

function loadCollapsed(): Set<MissionStatus> {
  try {
    const v: unknown = JSON.parse(readPref(COLLAPSED_KEY) ?? 'null');
    if (Array.isArray(v)) return new Set(SECTIONS.map((s) => s.status).filter((s) => v.includes(s)));
  } catch {
    // A malformed value falls back to the default.
  }
  return new Set<MissionStatus>(['done']);
}

function dotClass(b: BayDto): string {
  const s = b.agent?.status;
  if (!s) return 'idle';
  if (s.attention) return 'attention';
  if (s.kind === 'error') return 'error';
  if (s.kind === 'offline' || s.kind === 'booting') return s.kind;
  return s.busy ? 'busy' : 'idle';
}

/** Whether a bay's title just repeats the mission's (sessions are named after their first prompt). */
function sameTitle(a: string, b: string): boolean {
  const norm = (s: string) => s.toLowerCase().replace(/\s+/g, ' ').replace(/[\s.…]+$/u, '').trim();
  const x = norm(a);
  const y = norm(b);
  if (!x || !y) return false;
  if (x === y) return true;
  const [short, long] = x.length < y.length ? [x, y] : [y, x];
  return short.length >= 16 && long.startsWith(short);
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
 * Keeps a card highlighted while its menu is open. Menus don't report closing, so the next
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

function goToBay(bay: BayId) {
  void act({ type: 'focusBay', bay });
  focusComposer(bay);
}

async function openSession(id: string) {
  const bay = await act<BayId>({ type: 'resumeSession', id });
  if (typeof bay === 'number') focusComposer(bay);
}

async function chooseFolder(m: MissionDto) {
  const dir = await pickFolder('Choose a folder for this mission', m.dir ?? useUi.getState().app?.defaultDir);
  if (dir) void act({ type: 'missionSetDir', id: m.id, dir });
}

function setStatus(m: MissionDto, status: MissionStatus) {
  if (m.status !== status) void act({ type: 'missionStatus', id: m.id, status });
}

const cardsIn = (root: Element | null) => Array.from(root?.querySelectorAll<HTMLElement>('[data-msn-card][tabindex]') ?? []);

/** The card to focus once this one is gone. */
function neighbour(el: HTMLElement): HTMLElement | null {
  const cards = cardsIn(el.closest('.msn-body'));
  const i = cards.indexOf(el);
  return cards[i + 1] ?? cards[i - 1] ?? null;
}

// ------------------------------------------------------------------ capture

function Capture({ onAdded }: { onAdded: (id: number) => void }) {
  const [text, setText] = useState('');
  const [dir, setDir] = useState<string | null>(null);
  const ref = useRef<HTMLTextAreaElement>(null);
  const tick = useUi((s) => s.captureTick);
  useAutosize(ref, text, 220);

  useEffect(() => {
    if (tick === seenCaptureTick) return;
    seenCaptureTick = tick;
    ref.current?.focus();
  }, [tick]);

  const submit = async () => {
    const value = text.trim();
    if (!value) return;
    setText('');
    const id = await act<number>({ type: 'missionAdd', text: value });
    if (typeof id !== 'number') {
      // The error is toasted; give the idea back.
      setText((t) => t || value);
      return;
    }
    if (dir) void act({ type: 'missionSetDir', id, dir });
    onAdded(id);
  };

  const pick = async () => {
    const d = await pickFolder('Choose a folder for new ideas', dir ?? useUi.getState().app?.defaultDir);
    if (d) setDir(d);
    ref.current?.focus();
  };

  const keys = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      void submit();
    } else if (e.key === 'Escape') {
      e.currentTarget.blur();
    }
  };

  return (
    <div
      className="msn-capture"
      onMouseDown={(e) => {
        // Clicks on the card's padding land in the text box.
        if (!(e.target as Element).closest('button, textarea')) {
          e.preventDefault();
          ref.current?.focus();
        }
      }}
    >
      <textarea
        ref={ref}
        className="msn-capture-input"
        value={text}
        rows={2}
        placeholder="Capture an idea…"
        aria-label="Capture an idea"
        onChange={(e) => setText(e.target.value)}
        onKeyDown={keys}
      />
      <div className="msn-capture-foot">
        <span className={`msn-folder${dir ? ' on' : ''}`}>
          <button type="button" className="msn-folder-pick" data-tip={dir ?? 'Pick a folder for new ideas (optional)'} onClick={() => void pick()}>
            <Icon name="folder" />
            <span className="ellipsis">{dir ? baseName(dir) : 'Folder'}</span>
          </button>
          {dir && (
            <button type="button" className="msn-folder-clear" aria-label="Clear folder" data-tip="Clear folder" onClick={() => setDir(null)}>
              <Icon name="close" />
            </button>
          )}
        </span>
        {text.trim() && <span className="msn-keyhint">Shift+Enter adds notes</span>}
        <button type="button" className="msn-add" aria-label="Add idea" data-tip="Add idea" data-keys="Enter" disabled={!text.trim()} onClick={() => void submit()}>
          <Icon name="arrow-up" />
        </button>
      </div>
    </div>
  );
}

// ------------------------------------------------------------------ cards

function ActionButton({ icon, tip, onClick }: { icon: string; tip: string; onClick: () => void }) {
  return (
    <button type="button" className="icon-btn sm" aria-label={tip} data-tip={tip} onClick={onClick}>
      <Icon name={icon} />
    </button>
  );
}

function BayChip({ bay, mission }: { bay: BayDto; mission: string }) {
  const s = bay.agent?.status;
  const named = !s || !sameTitle(bay.title, mission);
  return (
    <button type="button" className="msn-chip msn-bay" data-tip={`Go to ${bay.title}`} onClick={() => goToBay(bay.id)}>
      <span className={`dot ${dotClass(bay)}`} />
      {named && <span className="msn-chip-label">{bay.title}</span>}
      {s && <span className={`msn-chip-status${s.busy ? ' shimmer' : ''}${s.attention ? ' attention' : ''}`}>{s.label}</span>}
    </button>
  );
}

interface CardProps {
  m: MissionDto;
  /** The open agent bay working on it. */
  bay: BayDto | null;
  /** The focused agent, when the mission can be sent there. */
  sendTo: BayDto | null;
  fresh: boolean;
  onDelegated: () => void;
}

function MissionCard({ m, bay, sendTo, fresh, onDelegated }: CardProps) {
  const [editing, setEditing] = useState(false);
  const [menu, setMenu] = useMenuOwner<true>();
  const dragging = useDrag((s) => s.mission?.id === m.id);
  const ref = useRef<HTMLDivElement>(null);

  if (editing) {
    return (
      <MissionEditor
        m={m}
        onClose={() => {
          setEditing(false);
          requestAnimationFrame(() => ref.current?.focus({ preventScroll: true }));
        }}
      />
    );
  }

  const launch = () => void act({ type: 'missionLaunch', id: m.id });
  const remove = () => void act({ type: 'missionRemove', id: m.id });
  const clearDir = () => void act({ type: 'missionSetDir', id: m.id, dir: null });

  const openMenu = (at: Element | { clientX: number; clientY: number }, items: MenuItem[], alignRight = false) => {
    setMenu(true);
    showMenu(at, items, { alignRight });
  };

  const items = (): MenuItem[] => {
    const list: MenuItem[] = [];
    if (bay) list.push({ label: 'Go to agent', icon: 'arrow-right', onSelect: () => goToBay(bay.id) });
    else if (m.sessionId) {
      const id = m.sessionId;
      list.push({ label: 'Open session', icon: 'history', onSelect: () => void openSession(id) });
    }
    list.push({ label: 'Launch in new session', icon: 'play', onSelect: launch });
    if (sendTo) list.push({ label: `Send to ${sendTo.title}`, icon: 'send', onSelect: () => void act({ type: 'missionDelegate', id: m.id, bay: sendTo.id }) });
    list.push({ sep: true });
    if (m.dir) {
      list.push({ label: 'Change folder…', icon: 'folder', onSelect: () => void chooseFolder(m) });
      list.push({ label: 'Clear folder', icon: 'close', onSelect: clearDir });
    } else {
      list.push({ label: 'Set folder…', icon: 'folder', onSelect: () => void chooseFolder(m) });
    }
    list.push({ sep: true });
    for (const k of MARKS) list.push({ label: k.label, icon: k.icon, checked: m.status === k.status, onSelect: () => setStatus(m, k.status) });
    list.push({ sep: true });
    list.push({ label: 'Edit', icon: 'edit', hint: 'Enter', onSelect: () => setEditing(true) });
    list.push({ label: 'Delete', icon: 'trash', hint: 'Del', danger: true, onSelect: remove });
    return list;
  };

  const folderMenu = (el: Element, dir: string) =>
    openMenu(el, [
      { label: 'Reveal in File Explorer', icon: 'folder-opened', onSelect: () => void act({ type: 'revealPath', path: dir }) },
      { label: 'Change folder…', icon: 'folder', onSelect: () => void chooseFolder(m) },
      { label: 'Clear folder', icon: 'close', onSelect: clearDir },
    ]);

  let primary: ReactNode;
  if (m.status === 'review') primary = <ActionButton icon="check" tip="Mark done" onClick={() => setStatus(m, 'done')} />;
  else if (m.status === 'done') primary = <ActionButton icon="issue-reopened" tip="Reopen as an idea" onClick={() => setStatus(m, 'idea')} />;
  else if (bay) primary = <ActionButton icon="arrow-right" tip="Go to agent" onClick={() => goToBay(bay.id)} />;
  else if (m.status === 'active' && m.sessionId) {
    const id = m.sessionId;
    primary = <ActionButton icon="history" tip="Open session" onClick={() => void openSession(id)} />;
  } else primary = <ActionButton icon="play" tip="Launch in a new session" onClick={launch} />;

  const down = (e: React.PointerEvent<HTMLDivElement>) => {
    if ((e.target as Element).closest('button, a, input, textarea')) return;
    startMissionDrag(e, m.id, m.title, (target) => {
      onDelegated();
      void act({ type: 'missionDelegate', id: m.id, bay: target });
    });
  };

  const keys = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.target !== e.currentTarget) return;
    if (e.key === 'Enter' || e.key === 'F2') {
      e.preventDefault();
      setEditing(true);
    } else if (e.key === 'Delete') {
      e.preventDefault();
      const next = neighbour(e.currentTarget);
      remove();
      next?.focus();
    }
  };

  const working = !!bay?.agent?.status.busy;
  const cls = ['msn-card', m.status, working && 'working', fresh && 'fresh', dragging && 'dragging', menu && 'menu'].filter(Boolean).join(' ');

  return (
    <div
      ref={ref}
      className={cls}
      tabIndex={0}
      role="group"
      aria-label={m.title}
      data-msn-card={m.id}
      onPointerDown={down}
      onDoubleClick={(e) => {
        if (!(e.target as Element).closest('button')) setEditing(true);
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        openMenu(e.clientX || e.clientY ? e : e.currentTarget, items());
      }}
      onKeyDown={keys}
    >
      <Icon name="gripper" className="msn-grip" />
      <div className="msn-title">
        {m.status === 'done' && <Icon name="pass-filled" className="msn-check" />}
        <span className="msn-title-text">{m.title}</span>
      </div>
      {m.notes && <div className="msn-notes">{m.notes}</div>}
      <div className="msn-meta">
        <div className="msn-tags">
          {bay ? (
            <BayChip bay={bay} mission={m.title} />
          ) : (
            m.sessionId && (
              <button type="button" className="msn-chip" data-tip="Open the session that worked on this" onClick={() => void openSession(m.sessionId!)}>
                <Icon name="history" />
                <span className="msn-chip-label">Session</span>
              </button>
            )
          )}
          {m.dir && (
            <button type="button" className="msn-chip" data-tip={m.dir} onClick={(e) => folderMenu(e.currentTarget, m.dir!)}>
              <Icon name="folder" />
              <span className="msn-chip-label">{baseName(m.dir)}</span>
            </button>
          )}
          {m.fromAgent && (
            <span className="badge msn-badge" data-tip="An agent suggested this">
              From agent
            </span>
          )}
        </div>
        <span className="msn-time">{ago(m.created)}</span>
        <div className="msn-actions">
          {primary}
          <button type="button" className="icon-btn sm" aria-label="More actions" data-tip="More" onClick={(e) => openMenu(e.currentTarget, items(), true)}>
            <Icon name="kebab-vertical" />
          </button>
        </div>
      </div>
    </div>
  );
}

function MissionEditor({ m, onClose }: { m: MissionDto; onClose: () => void }) {
  const [title, setTitle] = useState(m.title);
  const [notes, setNotes] = useState(m.notes);
  const titleRef = useRef<HTMLTextAreaElement>(null);
  const notesRef = useRef<HTMLTextAreaElement>(null);
  useAutosize(titleRef, title, 120);
  useAutosize(notesRef, notes, 260);

  useEffect(() => {
    const el = titleRef.current;
    if (!el) return;
    el.focus();
    el.setSelectionRange(el.value.length, el.value.length);
  }, []);

  const save = () => {
    const t = title.replace(/\s+/g, ' ').trim();
    if (!t) return;
    const n = notes.trim();
    if (t !== m.title || n !== m.notes) void act({ type: 'missionEdit', id: m.id, title: t, notes: n });
    onClose();
  };

  return (
    <div
      className="msn-card editing"
      data-msn-card={m.id}
      onKeyDown={(e) => {
        if (e.isDefaultPrevented()) return;
        if (e.key === 'Escape') {
          e.preventDefault();
          e.stopPropagation();
          onClose();
        } else if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
          e.preventDefault();
          save();
        }
      }}
    >
      <textarea
        ref={titleRef}
        className="msn-edit-title"
        value={title}
        rows={1}
        placeholder="Title"
        aria-label="Title"
        onChange={(e) => setTitle(e.target.value.replace(/\r?\n/g, ' '))}
        onKeyDown={(e) => {
          if (e.key !== 'Enter' || e.nativeEvent.isComposing || e.ctrlKey || e.metaKey) return;
          e.preventDefault();
          // Shift+Enter moves on to the notes, as it does when capturing.
          if (e.shiftKey) notesRef.current?.focus();
          else save();
        }}
      />
      <textarea
        ref={notesRef}
        className="msn-edit-notes"
        value={notes}
        rows={2}
        placeholder="Notes, context, acceptance criteria…"
        aria-label="Notes"
        onChange={(e) => setNotes(e.target.value)}
      />
      <div className="msn-edit-foot">
        <span className="msn-edit-hint">Ctrl+Enter to save</span>
        <button type="button" className="btn ghost sm" onClick={onClose}>
          Cancel
        </button>
        <button type="button" className="btn primary sm" disabled={!title.trim()} onClick={save}>
          Save
        </button>
      </div>
    </div>
  );
}

// ------------------------------------------------------------------ panel

export function MissionsPanel() {
  useTick(30_000);
  const missions = useUi((s) => s.missions);
  const bays = useUi((s) => s.bays);
  const focusedId = useUi((s) => s.app?.focused ?? null);
  const [collapsed, setCollapsed] = useState(loadCollapsed);
  const [hintSeen, setHintSeen] = useState(() => readPref(HINT_KEY) === '1');
  const [fresh, setFresh] = useState<number | null>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const scrolledTo = useRef<number | null>(null);

  const groups = useMemo(() => {
    const g: Record<MissionStatus, MissionDto[]> = { active: [], review: [], idea: [], done: [] };
    for (const m of missions) ((g[m.status] as MissionDto[] | undefined) ?? g.idea).push(m);
    return g;
  }, [missions]);
  const bayMap = useMemo(() => new Map(bays.map((b) => [b.id, b])), [bays]);
  const focusedAgent = bays.find((b) => b.id === focusedId && b.kind === 'agent' && !b.agent?.status.busy) ?? null;

  useEffect(() => writePref(COLLAPSED_KEY, JSON.stringify([...collapsed])), [collapsed]);

  useEffect(() => {
    if (fresh == null) return;
    const t = window.setTimeout(() => setFresh(null), 1600);
    return () => window.clearTimeout(t);
  }, [fresh]);

  // Bring a just-captured idea into view once it arrives from the core.
  useEffect(() => {
    if (fresh == null || scrolledTo.current === fresh) return;
    const el = bodyRef.current?.querySelector(`[data-msn-card="${fresh}"]`);
    if (!el) return;
    scrolledTo.current = fresh;
    el.scrollIntoView({ block: 'nearest' });
  }, [fresh, missions]);

  const added = (id: number) => {
    setFresh(id);
    setCollapsed((c) => {
      if (!c.has('idea')) return c;
      const n = new Set(c);
      n.delete('idea');
      return n;
    });
  };

  const toggle = (status: MissionStatus) =>
    setCollapsed((c) => {
      const n = new Set(c);
      if (n.has(status)) n.delete(status);
      else n.add(status);
      return n;
    });

  const seeHint = () => {
    setHintSeen(true);
    writePref(HINT_KEY, '1');
  };

  const navigate = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    const el = e.target as HTMLElement;
    if (!el.matches('[data-msn-card][tabindex]')) return;
    const cards = cardsIn(e.currentTarget);
    const next = cards[cards.indexOf(el) + (e.key === 'ArrowDown' ? 1 : -1)];
    if (next) {
      e.preventDefault();
      next.focus();
    }
  };

  const open = missions.length - groups.done.length;
  const rows: ReactNode[] = [];
  for (const sec of SECTIONS) {
    const list = groups[sec.status];
    if (!list.length) continue;
    const shut = collapsed.has(sec.status);
    rows.push(
      <button key={`sec-${sec.status}`} type="button" className="panel-section msn-sec" aria-expanded={!shut} onClick={() => toggle(sec.status)}>
        <Icon name="chevron-right" className={`msn-sec-chev${shut ? '' : ' open'}`} />
        <span className={`msn-sec-dot ${sec.status}`} />
        <span className="section-label">{sec.label}</span>
        <span className="msn-sec-count">{list.length}</span>
      </button>,
    );
    if (shut) continue;
    for (const m of list) {
      const linked = m.bay != null ? bayMap.get(m.bay) : undefined;
      rows.push(
        <MissionCard
          key={m.id}
          m={m}
          bay={linked?.kind === 'agent' ? linked : null}
          sendTo={focusedAgent && focusedAgent.id !== m.bay ? focusedAgent : null}
          fresh={fresh === m.id}
          onDelegated={seeHint}
        />,
      );
    }
  }

  return (
    <div className="panel msn-panel">
      <div className="panel-head">
        <h2>Missions</h2>
        {open > 0 && <span className="msn-head-count">{open} open</span>}
      </div>
      <div className="panel-toolbar msn-toolbar">
        <Capture onAdded={added} />
        {groups.idea.length > 0 && !hintSeen && (
          <div className="msn-hint">
            <Icon name="info" />
            <span className="grow">Drag an idea onto an agent to delegate it</span>
            <button type="button" className="icon-btn sm" aria-label="Dismiss hint" data-tip="Got it" onClick={seeHint}>
              <Icon name="close" />
            </button>
          </div>
        )}
      </div>
      <div ref={bodyRef} className="panel-body msn-body" onKeyDown={navigate}>
        {missions.length === 0 ? (
          <div className="empty msn-empty">
            <span className="msn-empty-icon">
              <Icon name="rocket" />
            </span>
            <h4>Your idea board is empty</h4>
            <p>Capture ideas as they come. Launch one in a new session, or drag it onto an agent to delegate it.</p>
          </div>
        ) : (
          <>
            {open === 0 && (
              <div className="msn-clear">
                <Icon name="pass" />
                <span>All caught up. Capture the next idea above.</span>
              </div>
            )}
            {rows}
          </>
        )}
      </div>
    </div>
  );
}

// ------------------------------------------------------------------ drag ghost

/** The chip that follows the pointer while a mission is dragged onto a bay (mounted once, app-wide). */
export function MissionDragGhost() {
  const drag = useDrag((s) => s.mission);
  const over = useUi((s) => (drag?.overBay != null ? (s.bays.find((b) => b.id === drag.overBay) ?? null) : null));
  if (!drag) return null;
  const agent = over?.kind === 'agent' ? over.agent : null;
  const busy = !!agent && agent.status.busy && !agent.deferred;
  const flipX = drag.x > window.innerWidth - 320;
  const flipY = drag.y > window.innerHeight - 56;
  return createPortal(
    <div
      className={`msn-ghost${agent ? (busy ? ' busy' : ' over') : ''}`}
      style={{
        left: flipX ? drag.x - 12 : drag.x + 14,
        top: flipY ? drag.y - 10 : drag.y + 16,
        translate: `${flipX ? '-100%' : '0'} ${flipY ? '-100%' : '0'}`,
      }}
      aria-hidden
    >
      <Icon name={busy ? 'watch' : agent ? 'send' : 'rocket'} />
      <span className="ellipsis">{drag.title}</span>
      {busy && <span className="msn-ghost-note">Busy</span>}
    </div>,
    document.body,
  );
}
