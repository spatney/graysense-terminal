import { useEffect, useMemo, useRef, useState } from 'react';
import { act, captureNote, newAgentIn, newSession, newShell, newShellIn, restartToUpdate, revealMemory, search, setSide, toggleSide } from '../lib/api';
import { rankNotes } from '../lib/notes';
import { NoteCapture } from './NoteCapture';
import { ago, baseName, contextLabel, effortLabel, plural, shortPath } from '../lib/format';
import { focusComposer, openOverlay, type Overlay, toast, useUi } from '../lib/store';
import { THEMES } from '../lib/theme';
import type { SearchHit } from '../lib/types';
import { Icon, Kbd, Modal } from './ui';

export function Overlays() {
  const overlay = useUi((s) => s.overlay);
  if (!overlay) return null;
  const close = () => openOverlay(null);
  switch (overlay.kind) {
    case 'search':
      return <SearchOverlay initial={overlay.query ?? ''} onClose={close} />;
    case 'palette':
      return <Palette onClose={close} />;
    case 'shortcuts':
      return <Shortcuts onClose={close} />;
    case 'confirm':
      return <Confirm o={overlay} onClose={close} />;
    case 'note':
      return <NoteOverlay o={overlay} onClose={close} />;
  }
}

/** Ctrl+Shift+S: save a note from anywhere. Files dropped on the window while it's open go in it. */
function NoteOverlay({ o, onClose }: { o: Extract<Overlay, { kind: 'note' }>; onClose: () => void }) {
  return (
    <Modal onClose={onClose} width={540} labelledBy="note-title">
      <div className="nts-modal-head">
        <Icon name="bookmark" />
        <h3 id="note-title">Save a note</h3>
        <button type="button" className="icon-btn" aria-label="Close" onClick={onClose}>
          <Icon name="close" />
        </button>
      </div>
      <div className="nts-modal-body">
        <NoteCapture initial={o} autoFocus dropTarget="overlay" onSaved={onClose} onCancel={onClose} />
        <div className="nts-modal-hint">Write what happened and who it was for, then drop in the files or links you used. Ask about it later in Memory › Notes, or just ask an agent.</div>
      </div>
    </Modal>
  );
}

/** Text with highlighted UTF-16 ranges. */
function Marked({ text, ranges }: { text: string; ranges: [number, number][] }) {
  if (!ranges.length) return <>{text}</>;
  const out: React.ReactNode[] = [];
  let at = 0;
  ranges.forEach(([a, b], i) => {
    if (a > at) out.push(text.slice(at, a));
    out.push(<mark key={i}>{text.slice(a, b)}</mark>);
    at = b;
  });
  if (at < text.length) out.push(text.slice(at));
  return <>{out}</>;
}

/** Arrow keys move through `count` items; Enter runs one. */
function useListKeys(count: number, run: (i: number, e: React.KeyboardEvent) => void) {
  const [hot, setHot] = useState(0);
  useEffect(() => setHot((h) => Math.min(h, Math.max(0, count - 1))), [count]);
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setHot((h) => (count ? (h + 1) % count : 0));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setHot((h) => (count ? (h - 1 + count) % count : 0));
    } else if (e.key === 'Enter' && count) {
      e.preventDefault();
      run(hot, e);
    }
  };
  return { hot, setHot, onKeyDown };
}

function scrollHot(list: HTMLElement | null) {
  list?.querySelector('.finder-item.hot')?.scrollIntoView({ block: 'nearest' });
}

// ------------------------------------------------------------------ search

function SearchOverlay({ initial, onClose }: { initial: string; onClose: () => void }) {
  const [query, setQuery] = useState(initial);
  const [hits, setHits] = useState<SearchHit[]>([]);
  const status = useUi((s) => s.app?.search);
  const home = useUi((s) => s.app?.home);
  const focusedAgent = useUi((s) => s.bays.find((b) => b.id === s.app?.focused && b.kind === 'agent')?.id ?? null);
  const list = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let live = true;
    const t = window.setTimeout(
      () =>
        void search(query, 50)
          .then((h) => live && setHits(h))
          .catch(() => {}),
      query ? 70 : 0,
    );
    return () => {
      live = false;
      window.clearTimeout(t);
    };
  }, [query, status?.sessions, status?.files]);

  const open = async (h: SearchHit, e?: { ctrlKey?: boolean; shiftKey?: boolean }) => {
    onClose();
    if (h.kind === 'session' && h.sessionId) {
      const bay = await act<number>({ type: 'resumeSession', id: h.sessionId });
      if (bay != null) focusComposer(bay);
    } else if (h.path) {
      if (e?.shiftKey && focusedAgent != null) {
        await act({ type: 'attach', bay: focusedAgent, paths: [h.path] });
        focusComposer(focusedAgent);
        toast('ok', `Attached ${baseName(h.path)}`);
      } else if (e?.ctrlKey) void act({ type: 'revealPath', path: h.path });
      else void act({ type: 'openPath', path: h.path });
    }
  };
  const memories = useUi((s) => s.memory.memories);
  const notes = useMemo(() => (query.trim() ? rankNotes(memories.filter((m) => m.kind === 'note' && m.state === 'active'), query).slice(0, 4) : []), [memories, query]);
  const openNote = (id: number, e?: { ctrlKey?: boolean }) => {
    onClose();
    if (e?.ctrlKey) void act<number>({ type: 'noteAsk', question: query.trim(), ids: [id] }).then((b) => b != null && focusComposer(b));
    else revealMemory(id);
  };
  const total = notes.length + hits.length;
  const { hot, setHot, onKeyDown } = useListKeys(total, (i, e) => {
    if (i < notes.length) openNote(notes[i].note.id, e);
    else void open(hits[i - notes.length], e);
  });
  useEffect(() => scrollHot(list.current), [hot]);

  const sessions = hits.filter((h) => h.kind === 'session');
  const files = hits.filter((h) => h.kind === 'file');
  const row = (h: SearchHit) => {
    const i = hits.indexOf(h) + notes.length;
    return (
      <button key={`${h.kind}:${h.sessionId ?? h.path}`} type="button" className={`finder-item ${i === hot ? 'hot' : ''}`} onPointerMove={() => setHot(i)} onClick={(e) => void open(h, e)}>
        <Icon name={h.kind === 'session' ? 'comment-discussion' : 'file'} />
        <span className="finder-item-main">
          <span className="finder-item-title">
            <Marked text={h.title} ranges={h.titleRanges} />
            {h.line != null && <span className="faint"> :{h.line}</span>}
          </span>
          {h.snippet ? (
            <span className="finder-item-sub">
              <Marked text={h.snippet} ranges={h.snippetRanges} />
            </span>
          ) : (
            h.location && <span className="finder-item-sub">{shortPath(h.location, home)}</span>
          )}
        </span>
        <span className="finder-item-meta">{h.modified ? ago(h.modified) : ''}</span>
      </button>
    );
  };

  return (
    <Modal onClose={onClose} className="finder" width={660}>
      <div className="finder-input">
        <Icon name="search" />
        <input autoFocus value={query} placeholder="Search notes, sessions, conversations and files" onChange={(e) => setQuery(e.target.value)} onKeyDown={onKeyDown} spellCheck={false} />
        {status?.building && <span className="spinner sm" data-tip="Updating the index" />}
      </div>
      <div className="finder-list" ref={list}>
        {total === 0 && <div className="finder-empty">{status?.built ? (query ? 'Nothing matches.' : 'No sessions or files yet.') : 'Indexing sessions and files…'}</div>}
        {notes.length > 0 && <div className="finder-group section-label">Notes</div>}
        {notes.map(({ note }, i) => (
          <button key={`note:${note.id}`} type="button" className={`finder-item ${i === hot ? 'hot' : ''}`} onPointerMove={() => setHot(i)} onClick={(e) => openNote(note.id, e)}>
            <Icon name="bookmark" />
            <span className="finder-item-main">
              <span className="finder-item-title">{note.text.split('\n')[0]}</span>
              {note.attachments.length > 0 && <span className="finder-item-sub">{note.attachments.map((a) => a.name).join(' · ')}</span>}
            </span>
            <span className="finder-item-meta">{ago(note.created)}</span>
          </button>
        ))}
        {sessions.length > 0 && <div className="finder-group section-label">Sessions</div>}
        {sessions.map(row)}
        {files.length > 0 && <div className="finder-group section-label">Files in sessions</div>}
        {files.map(row)}
      </div>
      <div className="finder-foot">
        <span>
          <Kbd keys={['↵']} /> Open
        </span>
        <span>
          <Kbd keys={['Ctrl', '↵']} /> {hot < notes.length ? 'Ask Copilot' : 'Reveal file'}
        </span>
        {focusedAgent != null && hot >= notes.length && (
          <span>
            <Kbd keys={['Shift', '↵']} /> Attach file
          </span>
        )}
        <span className="grow" />
        {status && <span>{status.built ? `${plural(status.sessions, 'session')}${status.files ? ` · ${plural(status.files, 'file')}` : ''}` : 'Indexing…'}</span>}
      </div>
    </Modal>
  );
}

// ------------------------------------------------------------------ palette

interface Command {
  id: string;
  label: string;
  icon: string;
  group: string;
  keys?: string[];
  run: () => void;
}

/** Fuzzy subsequence score (higher is better), or -1. */
export function fuzzy(query: string, text: string): number {
  const q = query.toLowerCase().trim();
  if (!q) return 0;
  const t = text.toLowerCase();
  const direct = t.indexOf(q);
  if (direct >= 0) return 100 - direct + (direct === 0 || t[direct - 1] === ' ' ? 20 : 0);
  let ti = 0;
  let score = 0;
  for (const ch of q) {
    if (ch === ' ') continue;
    const at = t.indexOf(ch, ti);
    if (at < 0) return -1;
    score += at === ti ? 3 : 1;
    ti = at + 1;
  }
  return score;
}

function useCommands(): Command[] {
  const s = useUi.getState();
  const focused = s.bays.find((b) => b.id === s.app?.focused) ?? null;
  const cmds: Command[] = [
    { id: 'new', label: 'New session', icon: 'comment-discussion', group: 'Start', keys: ['Ctrl', 'Shift', 'N'], run: () => void newSession() },
    { id: 'new-incognito', label: 'New incognito session (no memory)', icon: 'eye-closed', group: 'Start', run: () => void newSession(undefined, true) },
    { id: 'new-in', label: 'New agent in folder…', icon: 'folder-opened', group: 'Start', run: () => void newAgentIn() },
    { id: 'term', label: 'New terminal', icon: 'terminal', group: 'Start', keys: ['Ctrl', 'Shift', 'T'], run: () => void newShell() },
    { id: 'term-in', label: 'New terminal in folder…', icon: 'folder', group: 'Start', run: () => void newShellIn() },
    { id: 'idea', label: 'Capture an idea', icon: 'rocket', group: 'Start', keys: ['Ctrl', 'Shift', 'I'], run: () => captureIdea() },
    { id: 'note', label: 'Save a note', icon: 'bookmark', group: 'Start', keys: ['Ctrl', 'Shift', 'S'], run: () => setTimeout(() => captureNote()) },
    { id: 'p-notes', label: 'Show notes', icon: 'archive', group: 'Go', run: () => { localStorage.setItem('gs-memory-tab', 'notes'); void setSide('memory'); } },
    { id: 'search', label: 'Search sessions and files', icon: 'search', group: 'Go', keys: ['Ctrl', 'K'], run: () => setTimeout(() => openOverlay({ kind: 'search' })) },
    { id: 'p-sessions', label: 'Show sessions', icon: 'comment-discussion', group: 'Go', keys: ['Ctrl', 'Shift', 'H'], run: () => void setSide('sessions') },
    { id: 'p-missions', label: 'Show missions', icon: 'rocket', group: 'Go', run: () => void setSide('missions') },
    { id: 'p-memory', label: 'Show memory', icon: 'lightbulb-sparkle', group: 'Go', keys: ['Ctrl', 'Shift', 'M'], run: () => void setSide('memory') },
    { id: 'p-mcp', label: 'Show MCP servers', icon: 'server-process', group: 'Go', run: () => void setSide('mcp') },
    { id: 'p-plugins', label: 'Show plugins', icon: 'extensions', group: 'Go', run: () => void setSide('plugins') },
    { id: 'p-settings', label: 'Open settings', icon: 'settings-gear', group: 'Go', keys: ['Ctrl', ','], run: () => void setSide('settings') },
    { id: 'p-toggle', label: 'Toggle side panel', icon: 'layout-sidebar-left', group: 'View', keys: ['Ctrl', 'Shift', 'B'], run: () => toggleSidePanel() },
    ...THEMES.map((t) => ({ id: `theme-${t.id}`, label: `Theme: ${t.name}`, icon: 'symbol-color', group: 'View', run: () => void act({ type: 'setSettings', changes: { theme: t.id } }) })),
    ...(['grid', 'focus', 'columns', 'rows'] as const).map((l) => ({ id: `layout-${l}`, label: `Layout: ${{ grid: 'Grid', focus: 'One at a time', columns: 'Columns', rows: 'Rows' }[l]}`, icon: 'layout', group: 'View', run: () => void act({ type: 'setSettings', changes: { layout: l } }) })),
    { id: 'zoom-in', label: 'Zoom in', icon: 'zoom-in', group: 'View', keys: ['Ctrl', '='], run: () => zoomBy(0.1) },
    { id: 'zoom-out', label: 'Zoom out', icon: 'zoom-out', group: 'View', keys: ['Ctrl', '-'], run: () => zoomBy(-0.1) },
    { id: 'zoom-reset', label: 'Reset zoom', icon: 'screen-normal', group: 'View', keys: ['Ctrl', '0'], run: () => void act({ type: 'setSettings', changes: { zoom: 1 } }) },
    {
      id: 'memory-toggle',
      label: s.settings?.['memory.enabled'] ? 'Turn memory off' : 'Turn memory on',
      icon: 'lightbulb-sparkle',
      group: 'Memory',
      run: () => void act({ type: 'setSettings', changes: { 'memory.enabled': !s.settings?.['memory.enabled'] } }),
    },
    { id: 'updates', label: 'Check for updates', icon: 'cloud-download', group: 'Graysense', run: () => void act({ type: 'updateCheck' }) },
    ...(s.update?.phase === 'ready' ? [{ id: 'restart', label: `Restart to update to ${s.update.nextVersion ?? 'the new version'}`, icon: 'debug-restart', group: 'Graysense', run: () => void restartToUpdate() }] : []),
    { id: 'shortcuts', label: 'Keyboard shortcuts', icon: 'keyboard', group: 'Graysense', keys: ['Ctrl', '/'], run: () => setTimeout(() => openOverlay({ kind: 'shortcuts' })) },
    { id: 'config', label: 'Reveal Graysense data folder', icon: 'folder-library', group: 'Graysense', run: () => s.app && void act({ type: 'revealPath', path: s.app.configDir }) },
  ];
  if (focused) {
    cmds.push(
      { id: 'close', label: `Close “${focused.title}”`, icon: 'close', group: 'Bay', keys: ['Ctrl', 'Shift', 'W'], run: () => void act({ type: 'closeBay', bay: focused.id }) },
      { id: 'max', label: 'Maximize bay', icon: 'screen-full', group: 'Bay', keys: ['Ctrl', 'Shift', '↵'], run: () => void act({ type: 'maximizeBay', bay: focused.id }) },
      { id: 'reveal', label: 'Reveal folder in File Explorer', icon: 'folder-opened', group: 'Bay', run: () => void act({ type: 'revealPath', path: focused.cwd }) },
      { id: 'vscode', label: 'Open folder in VS Code', icon: 'vscode', group: 'Bay', run: () => void act({ type: 'openInVsCode', path: focused.cwd }) },
    );
    if (focused.agent) {
      const agent = focused.agent;
      cmds.push(
        { id: 'learn', label: 'Learn from this conversation now', icon: 'lightbulb-sparkle', group: 'Bay', run: () => void act({ type: 'learnNow', bay: focused.id }) },
        { id: 'clear', label: 'Clear transcript', icon: 'clear-all', group: 'Bay', run: () => void act({ type: 'clearTranscript', bay: focused.id }) },
      );
      const models = s.app?.models ?? [];
      const current = models.find((m) => m.id === (agent.model ?? s.settings?.default_model));
      for (const e of current?.efforts.length ? ['auto', ...current.efforts] : []) {
        const name = e === 'auto' ? 'Auto' : effortLabel(e);
        cmds.push({ id: `effort-${e}`, label: `Thinking: ${name}${(agent.effort ?? 'auto') === e ? ' (current)' : ''}`, icon: 'lightbulb', group: 'Model', run: () => void act({ type: 'setModel', bay: focused.id, model: current!.id, effort: e }) });
      }
      for (const t of current?.contexts.length ? ['auto', ...current.contexts.map((c) => c.tier)] : []) {
        const size = current!.contexts.find((c) => c.tier === t);
        cmds.push({ id: `context-${t}`, label: `Context window: ${size ? contextLabel(size) : 'Auto'}${(agent.contextTier ?? 'auto') === t ? ' (current)' : ''}`, icon: 'layers', group: 'Model', run: () => void act({ type: 'setModel', bay: focused.id, model: current!.id, context: t }) });
      }
      for (const m of models) {
        if (m.id === current?.id) continue;
        cmds.push({ id: `model-${m.id}`, label: `Switch to ${m.name || m.id}`, icon: 'sparkle', group: 'Model', run: () => void act({ type: 'setModel', bay: focused.id, model: m.id }) });
      }
    }
  }
  for (const dir of (s.settings?.recent_dirs ?? []).slice(0, 8)) {
    cmds.push({ id: `dir-${dir}`, label: `Open agent in ${shortPath(dir, s.app?.home)}`, icon: 'folder', group: 'Recent folders', run: () => void act({ type: 'newAgent', dir }).then((b) => typeof b === 'number' && focusComposer(b)) });
  }
  for (const ses of s.sessions.items.slice(0, 60)) {
    cmds.push({ id: `ses-${ses.id}`, label: `Resume: ${ses.title}`, icon: 'history', group: 'Sessions', run: () => void act<number>({ type: 'resumeSession', id: ses.id }).then((b) => b != null && focusComposer(b)) });
  }
  return cmds;
}

export function captureIdea() {
  void setSide('missions');
  useUi.setState((s) => ({ captureTick: s.captureTick + 1 }));
}

export function toggleSidePanel() {
  const open = useUi.getState().settings?.side_panel;
  if (open && open !== 'hidden') void setSide('hidden');
  else void toggleSide((localStorage.getItem('gs-last-panel') as any) || 'sessions');
}

export function zoomBy(delta: number) {
  const z = useUi.getState().settings?.zoom ?? 1;
  const next = Math.round(Math.min(1.6, Math.max(0.7, z + delta)) * 10) / 10;
  void act({ type: 'setSettings', changes: { zoom: next } });
}

function Palette({ onClose }: { onClose: () => void }) {
  const [query, setQuery] = useState('');
  const all = useCommands();
  const list = useRef<HTMLDivElement>(null);
  const shown = useMemo(() => {
    const q = query.replace(/^>\s*/, '');
    if (!q) return all.filter((c) => c.group !== 'Sessions' && c.group !== 'Recent folders' && c.group !== 'Model').concat(all.filter((c) => c.group === 'Recent folders').slice(0, 4));
    return all
      .map((c) => ({ c, score: fuzzy(q, `${c.label} ${c.group}`) }))
      .filter((x) => x.score >= 0)
      .sort((a, b) => b.score - a.score)
      .slice(0, 40)
      .map((x) => x.c);
  }, [all, query]);
  const run = (i: number) => {
    onClose();
    shown[i]?.run();
  };
  const { hot, setHot, onKeyDown } = useListKeys(shown.length, (i) => run(i));
  useEffect(() => scrollHot(list.current), [hot]);
  let lastGroup = '';
  return (
    <Modal onClose={onClose} className="finder" width={600}>
      <div className="finder-input">
        <Icon name="terminal-cmd" />
        <input autoFocus value={query} placeholder="Type a command, folder or session" onChange={(e) => setQuery(e.target.value)} onKeyDown={onKeyDown} spellCheck={false} />
      </div>
      <div className="finder-list" ref={list}>
        {shown.length === 0 && <div className="finder-empty">No matching commands.</div>}
        {shown.map((c, i) => {
          const head = !query && c.group !== lastGroup ? <div className="finder-group section-label">{c.group}</div> : null;
          lastGroup = c.group;
          return (
            <div key={c.id}>
              {head}
              <button type="button" className={`finder-item ${i === hot ? 'hot' : ''}`} onPointerMove={() => setHot(i)} onClick={() => run(i)}>
                <Icon name={c.icon} />
                <span className="finder-item-main">
                  <span className="finder-item-title">{c.label}</span>
                </span>
                {query && <span className="finder-item-meta">{c.group}</span>}
                {c.keys && <Kbd keys={c.keys} />}
              </button>
            </div>
          );
        })}
      </div>
    </Modal>
  );
}

// ------------------------------------------------------------------ shortcuts

const SHEET: [string, [string[], string][]][] = [
  [
    'General',
    [
      [['Ctrl', 'K'], 'Search sessions and files'],
      [['Ctrl', 'Shift', 'P'], 'Command palette'],
      [['Ctrl', '/'], 'Keyboard shortcuts'],
      [['Ctrl', 'Shift', 'B'], 'Toggle the side panel'],
      [['Ctrl', 'Shift', 'H'], 'Sessions'],
      [['Ctrl', 'Shift', 'M'], 'Memory'],
      [['Ctrl', 'Shift', 'I'], 'Capture an idea'],
      [['Ctrl', 'Shift', 'S'], 'Save a note'],
      [['Ctrl', ','], 'Settings'],
      [['Ctrl', '=', '/', '-'], 'Zoom in / out'],
    ],
  ],
  [
    'Sessions and bays',
    [
      [['Ctrl', 'Shift', 'N'], 'New session'],
      [['Ctrl', 'Shift', 'T'], 'New terminal in this folder'],
      [['Ctrl', 'Shift', 'W'], 'Close bay'],
      [['Ctrl', 'Tab'], 'Next bay'],
      [['Ctrl', 'Alt', '1–9'], 'Go to bay'],
      [['Ctrl', 'Shift', '↵'], 'Maximize bay'],
      [['Ctrl', 'Shift', 'L'], 'Cycle layout'],
    ],
  ],
  [
    'Message box',
    [
      [['↵'], 'Send'],
      [['Shift', '↵'], 'New line'],
      [['Esc'], 'Stop the agent'],
      [['↑'], 'Previous message'],
      [['/'], 'Commands'],
      [['Shift', 'Tab'], 'Next mode (Ask, Plan, Autopilot)'],
    ],
  ],
  [
    'Terminal',
    [
      [['Ctrl', 'C'], 'Copy selection (or interrupt)'],
      [['Ctrl', 'V'], 'Paste'],
      [['Ctrl', 'F'], 'Find'],
      [['Ctrl', '=', '/', '-'], 'Font size'],
    ],
  ],
];

function Shortcuts({ onClose }: { onClose: () => void }) {
  return (
    <Modal onClose={onClose} width={720} labelledBy="shortcuts-title">
      <div className="modal-body row" style={{ paddingBottom: 10 }}>
        <h3 id="shortcuts-title" className="grow" style={{ margin: 0 }}>
          Keyboard shortcuts
        </h3>
        <button type="button" className="icon-btn" aria-label="Close" onClick={onClose}>
          <Icon name="close" />
        </button>
      </div>
      <div className="shortcuts">
        {SHEET.map(([group, rows]) => (
          <div key={group} className="col" style={{ gap: 0 }}>
            <div className="section-label" style={{ padding: '10px 0 4px' }}>
              {group}
            </div>
            {rows.map(([keys, what]) => (
              <div key={what} className="shortcut-row">
                <span>{what}</span>
                <Kbd keys={keys} />
              </div>
            ))}
          </div>
        ))}
      </div>
    </Modal>
  );
}

function Confirm({ o, onClose }: { o: Extract<Overlay, { kind: 'confirm' }>; onClose: () => void }) {
  return (
    <Modal onClose={onClose} width={420} labelledBy="confirm-title">
      <div className="modal-body">
        <h3 id="confirm-title">{o.title}</h3>
        <p>{o.body}</p>
      </div>
      <div className="modal-foot">
        <button type="button" className="btn ghost" onClick={onClose}>
          Cancel
        </button>
        <button
          type="button"
          autoFocus
          className={`btn ${o.danger ? 'danger' : 'primary'}`}
          onClick={() => {
            onClose();
            o.onConfirm();
          }}
        >
          {o.confirm}
        </button>
      </div>
    </Modal>
  );
}
