import '../../styles/panels/memory.css';
import { memo, useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { act, pickFiles, pickSave, tryAct } from '../../lib/api';
import { ago, agoLong, baseName, modelLabel, plural } from '../../lib/format';
import { openOverlay, toast, useUi } from '../../lib/store';
import type { MemoryDto, MemoryKind, MemoryStateDto, ModelDto, Settings } from '../../lib/types';
import { Icon, type MenuItem, Seg, Toggle, copyText, showMenu, useAutosize } from '../ui';
import { NotesView } from './NotesView';

type Tab = 'notes' | 'learned';
type Filter = 'review' | 'all' | 'you' | 'project' | 'sessions' | 'rules' | 'deleted';
type CardAction = 'toggle' | 'copy' | 'pin' | 'edit' | 'delete' | 'restore' | 'purge' | 'accept' | 'dismiss';
type UiState = ReturnType<typeof useUi.getState>;
type Source = MemoryDto['sources'][number];
type SettingChanges = Partial<Record<keyof Settings, unknown>>;

interface Project {
  key: string;
  name: string;
}

/** The new-memory or edit form. Kept by the panel so it survives switching filters and tabs. */
interface Draft {
  /** The memory being edited, or null for a new one. */
  id: number | null;
  text: string;
  kind: MemoryKind;
  /** Project key, or null for everywhere. */
  scope: string | null;
  /** The user picked a scope, so it stops following the kind. */
  scopeTouched: boolean;
  error: string | null;
  saving: boolean;
}

interface Chip {
  id: Filter;
  label: string;
  tip: string;
  icon?: string;
}

const TAB_KEY = 'gs-memory-tab';
const MAX_LEN = 280;
const FILTERS: Filter[] = ['review', 'all', 'you', 'project', 'sessions', 'rules', 'deleted'];

const KIND_ICON: Record<MemoryKind, string> = {
  preference: 'heart',
  fact: 'info',
  procedure: 'checklist',
  episode: 'history',
  rule: 'shield',
  note: 'note',
};

const KINDS: { value: MemoryKind; label: string; tip: string }[] = [
  { value: 'preference', label: 'Preference', tip: 'How you like to work' },
  { value: 'fact', label: 'Fact', tip: 'Something true about a project or your setup' },
  { value: 'procedure', label: 'How-to', tip: 'Commands, conventions and gotchas' },
];

const PLACEHOLDER: Partial<Record<MemoryKind, string>> = {
  preference: 'e.g. I prefer pnpm over npm',
  fact: 'e.g. The API server runs on port 8080',
  procedure: 'e.g. Run npm test before every commit',
  episode: 'What happened in this session',
};

const ORIGIN: Record<MemoryDto['origin'], { label: string; long: string }> = {
  user: { label: 'You', long: 'Added by you' },
  learned: { label: 'Learned', long: 'Learned from a conversation' },
  agent: { label: 'Agent', long: 'Saved by an agent' },
};

const VIEW_NOTE: Partial<Record<Filter, string>> = {
  review: 'Things your agents picked up that need a quick yes or no.',
  rules: 'Commands agents may run without asking, learned from your approvals.',
  deleted: "Kept for 30 days so learning doesn't bring them back.",
};

const EMPTY: Record<Filter, (project: Project | null) => { icon: string; title: string; body: string }> = {
  review: () => ({ icon: 'check-all', title: 'All caught up', body: "When your agents pick up something they're not sure about, it waits here for a quick yes or no." }),
  all: () => ({ icon: 'lightbulb-sparkle', title: 'Nothing remembered yet', body: 'Graysense learns from your conversations, or say “remember that …” to an agent.' }),
  you: () => ({ icon: 'account', title: 'Nothing about you yet', body: 'Your preferences, like favorite tools and how you like answers, show up here.' }),
  project: (p) => ({ icon: 'folder', title: `Nothing about ${p?.name ?? 'this project'} yet`, body: 'Facts, conventions and how-tos for this project show up here as you work in it.' }),
  sessions: () => ({ icon: 'history', title: 'No past work yet', body: 'Short summaries of what your sessions did show up here and help new agents pick up where you left off.' }),
  rules: () => ({ icon: 'shield', title: 'No approval rules', body: 'After you approve the same command a few times, Graysense offers to let agents run it without asking in that project.' }),
  deleted: () => ({ icon: 'trash', title: 'Nothing deleted', body: "Deleted memories stay here for 30 days, so learning doesn't bring them back." }),
};

/** The filter to come back to when the panel reopens. */
let lastFilter: Filter = 'all';

// ------------------------------------------------------------------ helpers

const focusedAgent = (s: UiState) => s.bays.find((b) => b.id === s.app?.focused)?.agent ?? null;
const isReview = (m: MemoryDto) => m.state === 'pending' || (m.state === 'active' && m.proposal != null);
const isKnowledge = (m: MemoryDto) => m.kind === 'preference' || m.kind === 'fact' || m.kind === 'procedure';
const canPin = (m: MemoryDto) => m.state === 'active' && m.kind !== 'rule';
const canEdit = (m: MemoryDto) => m.state !== 'archived' && m.kind !== 'rule';
const when = (ts: number, now: number) => agoLong(ts, now).replace(/^on /, '');
const fullDate = (ts: number) => new Date(ts * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
const saveSettings = (changes: SettingChanges) => act({ type: 'setSettings', changes });

/** Notes live on their own tab, so every Learned view leaves them out. */
function inFilter(m: MemoryDto, f: Filter, projectKey: string | null): boolean {
  if (m.kind === 'note') return false;
  const active = m.state === 'active';
  switch (f) {
    case 'review':
      return isReview(m);
    case 'all':
      return active && isKnowledge(m);
    case 'you':
      return active && isKnowledge(m) && (m.project == null || m.kind === 'preference');
    case 'project':
      return active && isKnowledge(m) && projectKey != null && m.project === projectKey;
    case 'sessions':
      return active && m.kind === 'episode';
    case 'rules':
      return active && m.kind === 'rule';
    case 'deleted':
      return m.state === 'archived';
  }
}

/** Where a memory is listed. */
function homeFilter(m: MemoryDto): Filter {
  if (isReview(m)) return 'review';
  if (m.state === 'archived') return 'deleted';
  if (m.kind === 'episode') return 'sessions';
  if (m.kind === 'rule') return 'rules';
  return 'all';
}

/** Newest first; pinned memories lead the lists they're kept in. */
function sortFor(f: Filter, list: MemoryDto[]): MemoryDto[] {
  if (f === 'review' || f === 'deleted') return list.sort((a, b) => b.updated - a.updated || b.id - a.id);
  return list.sort((a, b) => Number(b.pinned) - Number(a.pinned) || b.updated - a.updated || b.id - a.id);
}

function searchText(m: MemoryDto): string {
  return [m.text, m.proposal ?? '', m.projectName ?? '', ...m.sources.map((s) => s.title ?? ''), ...m.commands].join('\n').toLowerCase();
}

function cleanQuote(q: string | null | undefined): string | null {
  const t = (q ?? '')
    .trim()
    .replace(/^user input:\s*/i, '')
    .replace(/^["“]+|["”]+$/g, '')
    .trim();
  return t || null;
}

const lastQuote = (m: MemoryDto) => cleanQuote(m.sources[m.sources.length - 1]?.quote);

function originText(m: MemoryDto): string {
  const base = ORIGIN[m.origin].long;
  return m.origin !== 'user' && m.history.some((r) => r.why === 'edited') ? `${base}, then edited by you` : base;
}

function whyLabel(why: string): string {
  switch (why) {
    case 'edited':
      return 'Edited by you';
    case 'updated':
      return 'Updated';
    case 'restored':
      return 'Restored';
    case 'deleted':
      return 'Deleted';
    case 'undone':
      return 'Undone';
    case 'dismissed':
      return 'Dismissed';
    case 'removed':
      return 'Removed';
    default:
      return `Removed by learning: ${why}`;
  }
}

interface ReviewInfo {
  flag: string;
  icon: string;
  warn: boolean;
  yes: string;
  yesTip: string;
  yesDanger: boolean;
  no: string;
  noTip: string;
}

function reviewInfo(m: MemoryDto): ReviewInfo {
  if (m.proposal === '') {
    return { flag: 'Looks outdated', icon: 'warning', warn: true, yes: 'Delete', yesTip: 'Delete this memory', yesDanger: true, no: 'Keep', noTip: 'Keep it as it is' };
  }
  if (m.proposal != null) {
    return { flag: 'Update suggested', icon: 'sparkle', warn: false, yes: 'Accept', yesTip: 'Use the suggested wording', yesDanger: false, no: 'Keep current', noTip: 'Keep the current wording' };
  }
  if (m.kind === 'rule') {
    const where = m.projectName ? ` in ${m.projectName}` : '';
    return { flag: 'Suggested rule', icon: 'shield', warn: false, yes: 'Allow', yesTip: `Let agents run this without asking${where}`, yesDanger: false, no: 'Not now', noTip: 'Dismiss this suggestion' };
  }
  return {
    flag: `Suggested · ${Math.round(m.confidence * 100)}% sure`,
    icon: 'sparkle',
    warn: false,
    yes: 'Accept',
    yesTip: 'Keep this memory',
    yesDanger: false,
    no: 'Dismiss',
    noTip: "Forget it. Learning won't suggest it again.",
  };
}

function newDraft(view: Filter, project: Project | null): Draft {
  const key = view === 'project' ? (project?.key ?? null) : null;
  return { id: null, text: '', kind: key ? 'fact' : 'preference', scope: key, scopeTouched: false, error: null, saving: false };
}

function readTab(): Tab {
  try {
    return localStorage.getItem(TAB_KEY) === 'learned' ? 'learned' : 'notes';
  } catch {
    return 'notes';
  }
}

/** The clock for relative times, ticking only while it's on screen. */
function useNow(active: boolean, ms = 30_000): number {
  const [now, setNow] = useState(() => Date.now() / 1000);
  useEffect(() => {
    if (!active) return;
    setNow(Date.now() / 1000);
    const t = window.setInterval(() => setNow(Date.now() / 1000), ms);
    return () => window.clearInterval(t);
  }, [active, ms]);
  return now;
}

/** The card next to this one in the list (for arrow keys). */
function neighbor(from: HTMLElement, dir: 1 | -1): HTMLElement | null {
  const list = from.closest('.mem-list');
  if (!list) return null;
  const all = Array.from(list.querySelectorAll<HTMLElement>('[data-mem-toggle]'));
  return all[all.indexOf(from) + dir] ?? null;
}

async function exportMemory(markdown: boolean) {
  try {
    const path = await pickSave('Export memories', markdown ? 'graysense-memory.md' : 'graysense-memory.json');
    if (path) await act({ type: 'memoryExport', path, markdown });
  } catch (e) {
    toast('error', String(e));
  }
}

async function importMemory(): Promise<number> {
  try {
    const [path] = await pickFiles('Import memories');
    if (!path) return 0;
    return (await act<number>({ type: 'memoryImport', path })) ?? 0;
  } catch (e) {
    toast('error', String(e));
    return 0;
  }
}

function confirmClearAll() {
  openOverlay({
    kind: 'confirm',
    title: 'Forget everything learned?',
    body: 'This permanently deletes everything Graysense has learned, including suggestions and approval rules. Your notes are kept. Export first if you might want any of it back.',
    confirm: 'Forget everything',
    danger: true,
    onConfirm: () => void act({ type: 'memoryClearAll' }),
  });
}

function cardMenu(m: MemoryDto, expanded: boolean, selection: string, run: (a: CardAction) => void): MenuItem[] {
  const items: MenuItem[] = [
    { label: expanded ? 'Hide details' : 'Show details', icon: expanded ? 'chevron-up' : 'chevron-down', hint: 'Enter', onSelect: () => run('toggle') },
    ...(selection ? [{ label: 'Copy selection', icon: 'copy', onSelect: () => void copyText(selection, 'Copied') }] : []),
    { label: 'Copy text', icon: selection ? 'blank' : 'copy', onSelect: () => run('copy') },
  ];
  if (m.state === 'archived') {
    items.push({ sep: true }, { label: 'Restore', icon: 'discard', hint: 'R', onSelect: () => run('restore') }, { label: 'Delete forever', icon: 'trash', danger: true, onSelect: () => run('purge') });
    return items;
  }
  if (isReview(m)) {
    const info = reviewInfo(m);
    items.push({ sep: true }, { label: info.yes, icon: info.yesDanger ? 'trash' : 'check', onSelect: () => run('accept') }, { label: info.no, icon: 'close', onSelect: () => run('dismiss') });
  }
  items.push({ sep: true });
  if (canPin(m)) items.push({ label: m.pinned ? 'Unpin' : 'Pin', icon: m.pinned ? 'pinned' : 'pin', hint: 'P', onSelect: () => run('pin') });
  if (canEdit(m)) items.push({ label: 'Edit', icon: 'edit', hint: 'E', onSelect: () => run('edit') });
  items.push({ label: m.kind === 'rule' ? 'Revoke rule' : 'Delete', icon: 'trash', hint: 'Del', danger: true, onSelect: () => run('delete') });
  return items;
}

const Sep = () => (
  <span className="mem-sep" aria-hidden>
    ·
  </span>
);

// ------------------------------------------------------------------ panel

/** Memory: the Notes shelf, and everything Graysense learned (review inbox, search, editing, settings). */
export function MemoryPanel() {
  const mem = useUi((s) => s.memory);
  const settings = useUi((s) => s.settings);
  const models = useUi((s) => s.app?.models);
  const reveal = useUi((s) => s.revealMemory);
  const projectKey = useUi((s) => focusedAgent(s)?.projectKey || null);
  const projectName = useUi((s) => focusedAgent(s)?.project || null);
  const project = useMemo<Project | null>(() => (projectKey ? { key: projectKey, name: projectName || baseName(projectKey) } : null), [projectKey, projectName]);
  const enabled = settings?.['memory.enabled'] ?? true;

  const [tab, setTabState] = useState<Tab>(readTab);
  const [filter, setFilterState] = useState<Filter>(() => lastFilter);
  const [query, setQuery] = useState('');
  const [showSettings, setShowSettings] = useState(false);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [focusTick, setFocusTick] = useState(0);
  const [expanded, setExpanded] = useState<ReadonlySet<number>>(() => new Set<number>());
  const [flash, setFlash] = useState<{ id: number; n: number } | null>(null);
  const now = useNow(tab === 'learned');

  const bodyRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const settingsBtn = useRef<HTMLButtonElement>(null);
  const noteReveal = useRef<number | null>(null);
  const draftRef = useRef(draft);
  useLayoutEffect(() => {
    draftRef.current = draft;
  });

  const setTab = useCallback((t: Tab) => {
    setTabState(t);
    try {
      localStorage.setItem(TAB_KEY, t);
    } catch {
      /* storage unavailable: the tab just isn't remembered */
    }
  }, []);
  const setFilter = useCallback((f: Filter) => {
    lastFilter = f;
    setFilterState(f);
  }, []);
  const view: Filter = filter === 'project' && !project ? 'all' : filter;

  const haystacks = useMemo(() => new Map(mem.memories.map((m) => [m.id, searchText(m)] as const)), [mem.memories]);
  const terms = useMemo(() => query.toLowerCase().split(/\s+/).filter(Boolean), [query]);
  const matches = useCallback((m: MemoryDto) => terms.every((t) => haystacks.get(m.id)?.includes(t) ?? false), [terms, haystacks]);
  const searching = terms.length > 0;

  const inbox = useMemo(() => mem.memories.reduce((n, m) => n + (inFilter(m, 'review', null) ? 1 : 0), 0), [mem.memories]);
  const counts = useMemo(() => {
    const c: Record<Filter, number> = { review: 0, all: 0, you: 0, project: 0, sessions: 0, rules: 0, deleted: 0 };
    for (const m of mem.memories) {
      if (!matches(m)) continue;
      for (const f of FILTERS) if (inFilter(m, f, projectKey)) c[f] += 1;
    }
    return c;
  }, [mem.memories, matches, projectKey]);
  const items = useMemo(() => sortFor(view, mem.memories.filter((m) => inFilter(m, view, projectKey) && matches(m))), [mem.memories, view, projectKey, matches]);

  const chips = useMemo(() => {
    const list: Chip[] = [];
    if (inbox > 0 || view === 'review') list.push({ id: 'review', label: 'Review', tip: 'Suggestions waiting for a yes or no' });
    list.push({ id: 'all', label: 'All', tip: 'Preferences, facts and how-tos' }, { id: 'you', label: 'You', tip: 'About you: your preferences and anything that applies everywhere' });
    if (project) list.push({ id: 'project', label: project.name, icon: 'folder', tip: `Only ${project.name}, the focused agent's project` });
    list.push({ id: 'sessions', label: 'Past work', tip: 'Summaries of what past sessions did' }, { id: 'rules', label: 'Rules', tip: 'Commands agents may run without asking' }, { id: 'deleted', label: 'Deleted', tip: 'Recently deleted, kept for 30 days' });
    return list;
  }, [inbox, view, project]);

  useEffect(() => {
    if (filter === 'project' && !project) setFilter('all');
  }, [filter, project, setFilter]);

  // The memory being edited went away (deleted elsewhere, or memory was cleared).
  useEffect(() => {
    if (draft?.id != null && !mem.memories.some((m) => m.id === draft.id)) setDraft(null);
  }, [draft, mem.memories]);

  useEffect(() => {
    bodyRef.current?.scrollTo({ top: 0 });
  }, [view]);

  // Reveal a memory: notes go to the Notes tab (NotesView highlights them and clears the request);
  // anything else is shown on the Learned tab, in a view that lists it.
  useEffect(() => {
    if (reveal == null) {
      noteReveal.current = null;
      return;
    }
    const m = mem.memories.find((x) => x.id === reveal);
    if (!m) {
      // It may not have reached the panel yet; stop waiting after a moment.
      const t = window.setTimeout(() => {
        if (useUi.getState().revealMemory === reveal) useUi.setState({ revealMemory: null });
      }, 4000);
      return () => window.clearTimeout(t);
    }
    if (m.kind === 'note') {
      if (noteReveal.current !== reveal) {
        noteReveal.current = reveal;
        setTab('notes');
      }
      return;
    }
    setTab('learned');
    if (!inFilter(m, view, projectKey) || !matches(m)) {
      setQuery('');
      setFilter(homeFilter(m));
    }
    setFlash((f) => ({ id: m.id, n: (f?.n ?? 0) + 1 }));
    useUi.setState({ revealMemory: null });
  }, [reveal, mem.memories]);

  // Scroll a flashed memory into view (below the toolbar), and let the highlight fade.
  useEffect(() => {
    if (!flash) return;
    const body = bodyRef.current;
    const el = body?.querySelector<HTMLElement>(`[data-mem-id="${flash.id}"]`);
    if (body && el) {
      const r = el.getBoundingClientRect();
      const b = body.getBoundingClientRect();
      if (r.top < b.top || r.bottom > b.bottom) body.scrollBy({ top: r.top - b.top - Math.max(8, (b.height - r.height) / 2), behavior: 'smooth' });
    }
    const t = window.setTimeout(() => setFlash(null), 1600);
    return () => window.clearTimeout(t);
  }, [flash]);

  const startAdd = useCallback(() => {
    setTab('learned');
    setDraft((d) => (d && d.id == null ? d : newDraft(view, project)));
    setFocusTick((n) => n + 1);
  }, [setTab, view, project]);

  const patchDraft = useCallback((patch: Partial<Draft>) => setDraft((d) => (d ? { ...d, ...patch } : d)), []);

  // Closing an edit hands the keyboard back to its card.
  const closeDraft = useCallback(() => {
    const id = draftRef.current?.id;
    setDraft(null);
    if (id != null) requestAnimationFrame(() => bodyRef.current?.querySelector<HTMLElement>(`[data-mem-id="${id}"] [data-mem-toggle]`)?.focus({ preventScroll: true }));
  }, []);

  const saveDraft = async () => {
    const d = draft;
    if (!d || d.saving) return;
    const text = d.text.trim();
    if (!text) return patchDraft({ error: 'Write something to remember first.' });
    if (d.id == null && text.length < 4) return patchDraft({ error: "That's a little short to remember." });
    patchDraft({ saving: true, error: null });
    try {
      if (d.id == null) {
        const id = await tryAct<number>({ type: 'memoryAdd', text, kind: d.kind, project: d.scope });
        setDraft(null);
        if (typeof id === 'number') useUi.setState({ revealMemory: id });
      } else {
        const id = d.id;
        await tryAct({ type: 'memoryEdit', id, text, kind: d.kind, project: d.scope });
        closeDraft();
        setFlash((f) => ({ id, n: (f?.n ?? 0) + 1 }));
      }
    } catch (e) {
      patchDraft({ saving: false, error: String(e) });
    }
  };

  const onAction = useCallback((m: MemoryDto, a: CardAction) => {
    switch (a) {
      case 'toggle':
        setExpanded((s) => {
          const next = new Set(s);
          if (!next.delete(m.id)) next.add(m.id);
          return next;
        });
        return;
      case 'copy':
        void copyText(m.text, 'Copied');
        return;
      case 'pin':
        void act({ type: 'memoryPin', id: m.id, pinned: !m.pinned });
        return;
      case 'edit':
        // A suggested rewording is what you'd want to tweak; a suggested deletion keeps the text.
        setDraft({ id: m.id, text: m.proposal || m.text, kind: m.kind, scope: m.project, scopeTouched: true, error: null, saving: false });
        return;
      case 'delete':
        void act({ type: 'memoryArchive', id: m.id });
        return;
      case 'restore':
        void act({ type: 'memoryRestore', id: m.id }).then((r) => {
          if (r !== undefined) toast('ok', 'Memory restored');
        });
        return;
      case 'purge':
        void act({ type: 'memoryPurge', id: m.id });
        return;
      case 'accept':
        void act({ type: 'memoryAccept', id: m.id });
        return;
      case 'dismiss':
        void act({ type: 'memoryDismiss', id: m.id });
        return;
    }
  }, []);

  // Accept what's listed (only the matches while searching, and never notes).
  const acceptAll = () => {
    const ids = items.map((m) => m.id);
    void Promise.all(ids.map((id) => act({ type: 'memoryAccept', id }))).then(() => toast('ok', `Accepted ${plural(ids.length, 'suggestion')}`));
  };

  const onImported = (n: number) => {
    if (n > 0 && tab === 'learned') setFilter('review');
  };

  const panelKeys = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (tab === 'learned' && (e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'f') {
      e.preventDefault();
      searchRef.current?.focus();
      searchRef.current?.select();
    }
  };

  const searchKeys = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Escape' && query) {
      e.preventDefault();
      e.stopPropagation();
      setQuery('');
    } else if (e.key === 'ArrowDown') {
      const first = bodyRef.current?.querySelector<HTMLElement>('[data-mem-toggle]');
      if (first) {
        e.preventDefault();
        first.focus();
      }
    }
  };

  const adding = draft != null && draft.id == null;
  const elsewhere = searching && items.length === 0 ? chips.filter((c) => c.id !== view && counts[c.id] > 0) : [];
  const note = VIEW_NOTE[view];
  const editorFor = (m: MemoryDto | null) =>
    draft && (
      <MemoryEditor
        draft={draft}
        memory={m}
        project={project}
        focusTick={focusTick}
        onChange={patchDraft}
        onSave={() => void saveDraft()}
        onCancel={closeDraft}
      />
    );

  return (
    <div className="panel mem-panel" onKeyDown={panelKeys}>
      <div className="panel-head mem-head">
        <h2>Memory</h2>
        <Seg
          size="sm"
          value={tab}
          onChange={setTab}
          options={[
            { value: 'notes', label: 'Notes' },
            {
              value: 'learned',
              label: (
                <>
                  Learned
                  {inbox > 0 && <span className="count">{inbox}</span>}
                </>
              ),
              tip: inbox > 0 ? `${plural(inbox, 'suggestion')} to review` : undefined,
            },
          ]}
        />
        <span className="grow" />
        {tab === 'learned' && (
          <button type="button" className="icon-btn mem-head-btn" data-tip="Add a memory" aria-label="Add a memory" onClick={startAdd}>
            <Icon name="add" />
          </button>
        )}
        <span className="mem-master" data-tip={enabled ? 'Turn memory off' : 'Turn memory on'}>
          <Toggle on={enabled} disabled={!settings} label="Memory" onChange={(on) => void saveSettings({ 'memory.enabled': on })} />
        </span>
        <button
          ref={settingsBtn}
          type="button"
          className={`icon-btn mem-head-btn${showSettings ? ' active' : ''}`}
          data-tip="Memory settings"
          aria-label="Memory settings"
          aria-expanded={showSettings}
          onClick={() => setShowSettings((v) => !v)}
        >
          <Icon name="settings-gear" />
        </button>
      </div>

      {showSettings && (
        <SettingsCard
          settings={settings}
          models={models}
          onImported={onImported}
          onClose={() => {
            setShowSettings(false);
            settingsBtn.current?.focus();
          }}
        />
      )}

      {tab === 'notes' ? (
        <NotesView />
      ) : (
        <div className="mem-learned">
          <div className="panel-toolbar mem-toolbar">
            <div className="search-field">
              <Icon name="search" />
              <input
                ref={searchRef}
                className="input mem-search"
                placeholder="Search memories"
                aria-label="Search memories"
                spellCheck={false}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                onKeyDown={searchKeys}
              />
              {query && (
                <button
                  type="button"
                  className="icon-btn sm mem-search-clear"
                  data-tip="Clear search"
                  aria-label="Clear search"
                  onClick={() => {
                    setQuery('');
                    searchRef.current?.focus();
                  }}
                >
                  <Icon name="close" />
                </button>
              )}
            </div>
            <FilterChips chips={chips} value={view} counts={counts} searching={searching} onChange={setFilter} />
            {enabled && <StatusLine mem={mem} learnOn={settings?.['memory.learn'] ?? true} now={now} onTurnOn={() => void saveSettings({ 'memory.learn': true })} />}
          </div>

          <div ref={bodyRef} className={`panel-body mem-body${enabled ? '' : ' mem-dim'}`}>
            {!enabled && <OffCard onTurnOn={() => void saveSettings({ 'memory.enabled': true })} />}
            {adding && <div className="mem-new">{editorFor(null)}</div>}
            {view === 'all' && inbox > 0 && !searching && (
              <button type="button" className="mem-banner" onClick={() => setFilter('review')}>
                <Icon name="sparkle" />
                <span className="grow">{inbox === 1 ? '1 suggestion to review' : `${inbox} suggestions to review`}</span>
                <Icon name="chevron-right" />
              </button>
            )}
            {note && items.length > 0 && (
              <div className="mem-viewhead">
                <p>{note}</p>
                {view === 'review' && (
                  <button type="button" className="btn sm" data-tip={searching ? 'Accept the suggestions shown' : 'Accept every suggestion'} onClick={acceptAll}>
                    <Icon name="check-all" />
                    Accept all
                  </button>
                )}
              </div>
            )}
            {items.length > 0 && (
              <div className="mem-list" role="list" aria-label="Memories">
                {items.map((m) =>
                  draft?.id === m.id ? (
                    <div key={m.id} role="listitem" data-mem-id={m.id}>
                      {editorFor(m)}
                    </div>
                  ) : (
                    <MemoryCard key={m.id} m={m} expanded={expanded.has(m.id)} flash={flash?.id === m.id ? flash.n : 0} now={now} onAction={onAction} />
                  ),
                )}
              </div>
            )}
            {items.length === 0 && !adding && (enabled || searching) && (
              <EmptyState view={view} project={project} query={query} elsewhere={elsewhere} counts={counts} onPick={setFilter} onAdd={startAdd} />
            )}
          </div>
        </div>
      )}
    </div>
  );
}

// ------------------------------------------------------------------ toolbar parts

function FilterChips({ chips, value, counts, searching, onChange }: { chips: Chip[]; value: Filter; counts: Record<Filter, number>; searching: boolean; onChange: (f: Filter) => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [fade, setFade] = useState({ left: false, right: false });

  const measure = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    const left = el.scrollLeft > 1;
    const right = el.scrollLeft + el.clientWidth < el.scrollWidth - 1;
    setFade((f) => (f.left === left && f.right === right ? f : { left, right }));
  }, []);

  useLayoutEffect(measure);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [measure]);

  // Keep the selected chip in view.
  useEffect(() => {
    const el = ref.current;
    const on = el?.querySelector<HTMLElement>('[aria-selected="true"]');
    if (!el || !on) return;
    const pad = 24;
    if (on.offsetLeft - pad < el.scrollLeft) el.scrollTo({ left: on.offsetLeft - pad, behavior: 'smooth' });
    else if (on.offsetLeft + on.offsetWidth + pad > el.scrollLeft + el.clientWidth) el.scrollTo({ left: on.offsetLeft + on.offsetWidth + pad - el.clientWidth, behavior: 'smooth' });
  }, [value]);

  const keys = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const i = chips.findIndex((c) => c.id === value);
    const last = chips.length - 1;
    const next = e.key === 'ArrowRight' ? (i + 1) % chips.length : e.key === 'ArrowLeft' ? (i - 1 + chips.length) % chips.length : e.key === 'Home' ? 0 : e.key === 'End' ? last : -1;
    if (next < 0 || !chips[next]) return;
    e.preventDefault();
    onChange(chips[next].id);
    ref.current?.querySelectorAll<HTMLElement>('[role="tab"]')[next]?.focus();
  };

  // A mouse wheel scrolls the row sideways.
  const wheel = (e: React.WheelEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    if (Math.abs(e.deltaY) > Math.abs(e.deltaX) && el.scrollWidth > el.clientWidth) el.scrollLeft += e.deltaY;
  };

  return (
    <div
      ref={ref}
      className={`mem-filters${fade.left ? ' mem-fade-l' : ''}${fade.right ? ' mem-fade-r' : ''}`}
      role="tablist"
      aria-label="Filter memories"
      onScroll={measure}
      onWheel={wheel}
      onKeyDown={keys}
    >
      {chips.map((c) => {
        const on = c.id === value;
        const n = counts[c.id];
        return (
          <button
            key={c.id}
            type="button"
            role="tab"
            aria-selected={on}
            tabIndex={on ? 0 : -1}
            className={`mem-chip${on ? ' mem-on' : ''}${searching && n === 0 ? ' mem-none' : ''}`}
            data-tip={c.tip}
            onClick={() => onChange(c.id)}
          >
            {c.icon && <Icon name={c.icon} />}
            <span className="mem-chip-label">{c.label}</span>
            {n > 0 && (c.id === 'review' && !searching ? <span className="count">{n}</span> : searching && <span className="mem-chip-n">{n}</span>)}
          </button>
        );
      })}
    </div>
  );
}

function StatusLine({ mem, learnOn, now, onTurnOn }: { mem: MemoryStateDto; learnOn: boolean; now: number; onTurnOn: () => void }) {
  if (mem.learning > 0) {
    return (
      <div className="mem-status" role="status">
        <Icon name="sparkle" className="mem-status-live" />
        <span className="shimmer ellipsis">Learning from {plural(mem.learning, 'conversation')}…</span>
      </div>
    );
  }
  if (!learnOn) {
    return (
      <div className="mem-status">
        <Icon name="debug-pause" />
        <span className="ellipsis">Learning is off</span>
        <button type="button" className="link-btn" data-tip="Learn from conversations again" onClick={onTurnOn}>
          Turn on
        </button>
      </div>
    );
  }
  if (!mem.lastRun) return null;
  const [at, err] = mem.lastRun;
  if (err) {
    return (
      <div className="mem-status mem-warn" data-tip={`Learning failed ${agoLong(at, now)}: ${err}`}>
        <Icon name="warning" />
        <span className="ellipsis">Couldn't learn: {err}</span>
      </div>
    );
  }
  return (
    <div className="mem-status">
      <Icon name="check" />
      <span className="ellipsis">Last learned {agoLong(at, now)}</span>
    </div>
  );
}

function OffCard({ onTurnOn }: { onTurnOn: () => void }) {
  return (
    <div className="mem-off" role="status">
      <span className="mem-off-icon">
        <Icon name="circle-slash" />
      </span>
      <div className="mem-off-body">
        <h3>Memory is off</h3>
        <p>Agents won't learn from your conversations or remember anything. Everything here is kept for when you turn it back on.</p>
        <button type="button" className="btn sm primary" onClick={onTurnOn}>
          Turn memory on
        </button>
      </div>
    </div>
  );
}

function EmptyState({
  view,
  project,
  query,
  elsewhere,
  counts,
  onPick,
  onAdd,
}: {
  view: Filter;
  project: Project | null;
  query: string;
  elsewhere: Chip[];
  counts: Record<Filter, number>;
  onPick: (f: Filter) => void;
  onAdd: () => void;
}) {
  if (query.trim()) {
    return (
      <div className="empty mem-empty">
        <Icon name="search" />
        <h4>No matches</h4>
        <p>Nothing here matches “{query.trim()}”.</p>
        {elsewhere.length > 0 && (
          <div className="mem-elsewhere">
            <span className="faint">Found in</span>
            {elsewhere.map((c) => (
              <button key={c.id} type="button" className="mem-chip" onClick={() => onPick(c.id)}>
                {c.icon && <Icon name={c.icon} />}
                <span className="mem-chip-label">{c.label}</span>
                <span className="mem-chip-n">{counts[c.id]}</span>
              </button>
            ))}
          </div>
        )}
      </div>
    );
  }
  const e = EMPTY[view](project);
  return (
    <div className="empty mem-empty">
      <Icon name={e.icon} />
      <h4>{e.title}</h4>
      <p>{e.body}</p>
      {(view === 'all' || view === 'you' || view === 'project') && (
        <button type="button" className="btn sm" onClick={onAdd}>
          <Icon name="add" />
          Add a memory
        </button>
      )}
    </div>
  );
}

// ------------------------------------------------------------------ settings

function SettingsCard({ settings, models, onImported, onClose }: { settings: Settings | null; models: ModelDto[] | undefined; onImported: (n: number) => void; onClose: () => void }) {
  const id = useId();
  const off = !settings || !settings['memory.enabled'];
  const model = settings?.['memory.learning_model'] ?? null;
  const list = models ?? [];
  const keys = (e: React.KeyboardEvent<HTMLElement>) => {
    if (e.key === 'Escape' && !(e.target instanceof HTMLSelectElement)) {
      e.stopPropagation();
      onClose();
    }
  };
  return (
    <section className="mem-settings" aria-label="Memory settings" onKeyDown={keys}>
      <SettingRow
        title="Learn from conversations"
        desc="Picks up your preferences and each project's conventions in the background."
        on={settings?.['memory.learn'] ?? true}
        disabled={off}
        onChange={(v) => void saveSettings({ 'memory.learn': v })}
      />
      <SettingRow
        title="Share with agents"
        desc="Gives each agent the memories that matter for its work."
        on={settings?.['memory.share'] ?? true}
        disabled={off}
        onChange={(v) => void saveSettings({ 'memory.share': v })}
      />
      <SettingRow
        title="Review everything learned"
        desc="Nothing learned is kept until you accept it."
        on={settings?.['memory.review_all'] ?? false}
        disabled={off}
        onChange={(v) => void saveSettings({ 'memory.review_all': v })}
      />
      <div className={`mem-set-field${off ? ' mem-disabled' : ''}`}>
        <label className="mem-set-title" htmlFor={`${id}-model`}>
          Learning model
        </label>
        <select id={`${id}-model`} className="select" value={model ?? ''} disabled={off} onChange={(e) => void saveSettings({ 'memory.learning_model': e.target.value || null })}>
          <option value="">Lowest cost (automatic)</option>
          {model != null && !list.some((m) => m.id === model) && <option value={model}>{modelLabel(model)}</option>}
          {list.map((m) => (
            <option key={m.id} value={m.id}>
              {m.multiplier != null ? `${m.name} · ${m.multiplier}×` : m.name}
            </option>
          ))}
        </select>
        <span className="mem-set-desc">Reads conversations in the background and never uses tools.</span>
      </div>
      <div className="mem-set-data">
        <div className="mem-set-actions">
          <button type="button" className="btn sm" data-tip="Save your memories as a JSON file" onClick={() => void exportMemory(false)}>
            <Icon name="export" />
            Export JSON
          </button>
          <button type="button" className="btn sm" data-tip="Save a readable copy" onClick={() => void exportMemory(true)}>
            <Icon name="markdown" />
            Export Markdown
          </button>
          <button type="button" className="btn sm" data-tip="Add memories from a Graysense export. They wait for your review." onClick={() => void importMemory().then(onImported)}>
            <Icon name="folder-opened" />
            Import…
          </button>
        </div>
        <button type="button" className="btn sm danger" data-tip="Permanently delete everything in memory" onClick={confirmClearAll}>
          <Icon name="trash" />
          Forget everything learned…
        </button>
      </div>
      <p className="mem-set-note">
        <Icon name="lock" />
        Stored only on this PC. Secrets are never saved.
      </p>
    </section>
  );
}

function SettingRow({ title, desc, on, disabled, onChange }: { title: string; desc: string; on: boolean; disabled: boolean; onChange: (on: boolean) => void }) {
  return (
    <label className={`mem-set-row${disabled ? ' mem-disabled' : ''}`}>
      <span className="mem-set-text">
        <span className="mem-set-title">{title}</span>
        <span className="mem-set-desc">{desc}</span>
      </span>
      <Toggle on={on} disabled={disabled} label={title} onChange={onChange} />
    </label>
  );
}

// ------------------------------------------------------------------ editor

function MemoryEditor({
  draft,
  memory,
  project,
  focusTick,
  onChange,
  onSave,
  onCancel,
}: {
  draft: Draft;
  memory: MemoryDto | null;
  project: Project | null;
  focusTick: number;
  onChange: (patch: Partial<Draft>) => void;
  onSave: () => void;
  onCancel: () => void;
}) {
  const ref = useRef<HTMLTextAreaElement>(null);
  useAutosize(ref, draft.text, 240);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.focus();
    el.setSelectionRange(el.value.length, el.value.length);
  }, [focusTick]);

  const editing = draft.id != null;
  // Session summaries keep their kind and project; only their wording changes.
  const fixed = !KINDS.some((k) => k.value === draft.kind);
  // An edit keeps the memory's own project; a memory about you can move to the focused project.
  const scopeProject = memory?.project ? { key: memory.project, name: memory.projectName ?? baseName(memory.project) } : project;
  const len = draft.text.trim().length;

  const keys = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      onSave();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      onCancel();
    }
  };

  const pickKind = (kind: MemoryKind) => {
    // Preferences are about you; facts and how-tos are usually about the project you're in.
    const follow = !editing && !draft.scopeTouched && project != null;
    onChange(follow ? { kind, scope: kind === 'preference' ? null : project.key } : { kind });
  };

  const title = !editing ? 'New memory' : memory && isReview(memory) ? 'Edit suggestion' : 'Edit memory';

  return (
    <div className={`mem-editor${draft.error ? ' mem-invalid' : ''}`} onKeyDown={keys}>
      <div className="mem-editor-head">
        <Icon name={KIND_ICON[draft.kind]} />
        <span>{title}</span>
      </div>
      <textarea
        ref={ref}
        className="mem-input"
        rows={2}
        value={draft.text}
        placeholder={PLACEHOLDER[draft.kind] ?? 'What should agents remember?'}
        aria-label={title}
        readOnly={draft.saving}
        onChange={(e) => onChange({ text: e.target.value, error: null })}
      />
      {!fixed && (
        <div className="mem-editor-opts">
          <Seg value={draft.kind} options={KINDS} onChange={pickKind} />
          <Seg
            value={draft.scope ?? ''}
            options={[
              { value: '', label: 'Everywhere', tip: 'Shared with every agent' },
              ...(scopeProject ? [{ value: scopeProject.key, label: <span className="mem-seg-label">{scopeProject.name}</span>, tip: `Only for agents working in ${scopeProject.name}` }] : []),
            ]}
            onChange={(v) => onChange({ scope: v || null, scopeTouched: true })}
          />
        </div>
      )}
      {draft.error && (
        <div className="mem-error" role="alert">
          <Icon name="error" />
          <span>{draft.error}</span>
        </div>
      )}
      <div className="mem-editor-foot">
        {len > MAX_LEN - 40 && (
          <span className={`mem-len${len > MAX_LEN ? ' mem-over' : ''}`} data-tip={len > MAX_LEN ? `Memories are cut to ${MAX_LEN} characters` : undefined}>
            {len}/{MAX_LEN}
          </span>
        )}
        <span className="grow" />
        <button type="button" className="btn sm ghost" data-tip="Cancel" data-keys="Esc" onClick={onCancel}>
          Cancel
        </button>
        <button type="button" className="btn sm primary" data-tip="Save" data-keys="Ctrl+Enter" disabled={!draft.text.trim() || draft.saving} onClick={onSave}>
          <Icon name="check" />
          {draft.saving ? 'Saving…' : 'Save'}
        </button>
      </div>
    </div>
  );
}

// ------------------------------------------------------------------ cards

const MemoryCard = memo(function MemoryCard({ m, expanded, flash, now, onAction }: { m: MemoryDto; expanded: boolean; flash: number; now: number; onAction: (m: MemoryDto, a: CardAction) => void }) {
  const toggleRef = useRef<HTMLDivElement>(null);
  const archived = m.state === 'archived';
  const review = !archived && isReview(m);
  const info = review ? reviewInfo(m) : null;
  const rule = m.kind === 'rule';
  const update = review && !!m.proposal;
  const quote = review || (rule && !archived) ? lastQuote(m) : null;
  const clamp = expanded ? '' : ' mem-clamp';
  const run = (a: CardAction) => onAction(m, a);

  const click = (e: React.MouseEvent<HTMLDivElement>) => {
    if (e.detail > 1) return;
    // Selecting text in an open memory shouldn't close it.
    const sel = window.getSelection();
    if (sel && !sel.isCollapsed && e.currentTarget.contains(sel.anchorNode)) return;
    run('toggle');
  };

  const keys = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.target !== e.currentTarget || e.altKey || e.ctrlKey || e.metaKey) return;
    const el = e.currentTarget;
    const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
    if (key === 'Enter' || key === ' ') run('toggle');
    else if (key === 'ArrowDown') neighbor(el, 1)?.focus();
    else if (key === 'ArrowUp') (neighbor(el, -1) ?? el.closest('.mem-learned')?.querySelector<HTMLElement>('.mem-search'))?.focus();
    else if ((key === 'Delete' && !archived) || (key === 'r' && archived)) {
      (neighbor(el, 1) ?? neighbor(el, -1))?.focus();
      run(archived ? 'restore' : 'delete');
    } else if (key === 'e' && canEdit(m)) run('edit');
    else if (key === 'p' && canPin(m)) run('pin');
    else return;
    e.preventDefault();
  };

  const menu = (e: React.MouseEvent<HTMLDivElement>) => {
    e.preventDefault();
    const sel = window.getSelection();
    const selection = sel && !sel.isCollapsed && e.currentTarget.contains(sel.anchorNode) ? sel.toString().trim() : '';
    // The menu's key hints work on the focused card.
    toggleRef.current?.focus({ preventScroll: true });
    showMenu(e, cardMenu(m, expanded, selection, run));
  };

  const episodeFrom = m.kind === 'episode' ? m.sources[m.sources.length - 1]?.title?.trim() : null;
  const cls = ['mem-card', review && 'mem-review', archived && 'mem-archived', m.pinned && !archived && 'mem-pinned', expanded && 'mem-open', flash > 0 && `mem-flash-${flash % 2}`]
    .filter(Boolean)
    .join(' ');

  return (
    <div role="listitem" className={cls} data-mem-id={m.id} onContextMenu={menu}>
      <span className="mem-kind" data-tip={m.kindLabel}>
        <Icon name={KIND_ICON[m.kind]} />
      </span>
      <div className="mem-main">
        {info && (
          <div className={`mem-flag${info.warn ? ' mem-warn' : ''}`}>
            <Icon name={info.icon} />
            {info.flag}
          </div>
        )}
        <div ref={toggleRef} className="mem-toggle" role="button" tabIndex={0} aria-expanded={expanded} data-mem-toggle="" onClick={click} onKeyDown={keys}>
          {update && <p className={`mem-text mem-old${clamp}`}>{m.text}</p>}
          <p className={`mem-text${clamp}`}>{update ? m.proposal : m.text}</p>
        </div>
        {quote && <blockquote className="mem-quote">{quote}</blockquote>}

        <div className="mem-meta">
          <span className="mem-meta-info">
            {archived ? (
              <>
                <span className="mem-shrink">{m.archivedReason ?? 'Deleted'}</span>
                <Sep />
                <span className="mem-shrink">{m.projectName ?? 'Everywhere'}</span>
              </>
            ) : (
              <>
                {m.pinned && (
                  <span className="mem-pin" data-tip="Pinned: always given to agents">
                    <Icon name="pinned" />
                  </span>
                )}
                <span className="mem-shrink" data-tip={m.project ? `Only for agents in ${m.projectName ?? baseName(m.project)}` : 'Shared with every agent'}>
                  {m.projectName ?? 'Everywhere'}
                </span>
                {m.kind === 'episode' ? (
                  episodeFrom && (
                    <>
                      <Sep />
                      <span className="mem-shrink">{episodeFrom}</span>
                    </>
                  )
                ) : rule ? (
                  m.commands.length > 0 && (
                    <>
                      <Sep />
                      <span>{plural(m.commands.length, 'command')}</span>
                    </>
                  )
                ) : (
                  <>
                    <Sep />
                    <span data-tip={originText(m)}>{ORIGIN[m.origin].label}</span>
                  </>
                )}
                {m.uses > 0 && (
                  <>
                    <Sep />
                    <span data-tip={m.lastUsed != null ? `Last given to an agent ${agoLong(m.lastUsed, now)}` : undefined}>{plural(m.uses, 'use')}</span>
                  </>
                )}
                {m.stale && !m.pinned && m.state === 'active' && (
                  <span className="badge warn mem-stale" data-tip="Not confirmed in a while, so new agents don't get it unless you pin it">
                    Stale
                  </span>
                )}
              </>
            )}
          </span>
          <span className="mem-time" data-tip={fullDate(m.updated)}>
            {ago(m.updated, now)}
          </span>
          {!review && (
            <span className="mem-actions">
              {archived ? (
                <>
                  <button type="button" className="btn sm ghost" data-tip="Bring it back to your memories" onClick={() => run('restore')}>
                    <Icon name="discard" />
                    Restore
                  </button>
                  <button type="button" className="icon-btn sm mem-danger" data-tip="Delete forever. Learning may pick it up again later." aria-label="Delete forever" onClick={() => run('purge')}>
                    <Icon name="trash" />
                  </button>
                </>
              ) : (
                <>
                  {canPin(m) && (
                    <button
                      type="button"
                      className={`icon-btn sm${m.pinned ? ' active' : ''}`}
                      data-tip={m.pinned ? 'Unpin' : 'Pin: always give agents this'}
                      aria-label={m.pinned ? 'Unpin' : 'Pin'}
                      aria-pressed={m.pinned}
                      onClick={() => run('pin')}
                    >
                      <Icon name={m.pinned ? 'pinned' : 'pin'} />
                    </button>
                  )}
                  {canEdit(m) && (
                    <button type="button" className="icon-btn sm" data-tip="Edit" aria-label="Edit" onClick={() => run('edit')}>
                      <Icon name="edit" />
                    </button>
                  )}
                  <button type="button" className="icon-btn sm mem-danger" data-tip={rule ? 'Revoke rule' : 'Delete'} aria-label={rule ? 'Revoke rule' : 'Delete'} onClick={() => run('delete')}>
                    <Icon name="trash" />
                  </button>
                </>
              )}
            </span>
          )}
        </div>

        {expanded && <MemoryDetails m={m} now={now} />}

        {info && (
          <div className="mem-decide">
            <button type="button" className={`btn sm ${info.yesDanger ? 'danger' : 'primary'}`} data-tip={info.yesTip} onClick={() => run('accept')}>
              <Icon name={info.yesDanger ? 'trash' : 'check'} />
              {info.yes}
            </button>
            {canEdit(m) && (
              <button type="button" className="btn sm" data-tip="Change the wording, then save" onClick={() => run('edit')}>
                Edit
              </button>
            )}
            <button type="button" className="btn sm ghost" data-tip={info.noTip} onClick={() => run('dismiss')}>
              {info.no}
            </button>
          </div>
        )}
      </div>
    </div>
  );
});

function MemoryDetails({ m, now }: { m: MemoryDto; now: number }) {
  const pct = Math.round(Math.min(1, Math.max(0, m.confidence)) * 100);
  const sources = m.sources.slice().reverse();
  const history = m.history.slice().reverse();
  return (
    <div className="mem-details">
      <dl className="mem-facts">
        <dt>Kind</dt>
        <dd>{m.kindLabel}</dd>
        <dt>Origin</dt>
        <dd>{originText(m)}</dd>
        <dt>Added</dt>
        <dd>{when(m.created, now)}</dd>
        <dt>Last used</dt>
        <dd>{m.lastUsed != null ? when(m.lastUsed, now) : 'Not yet'}</dd>
        {m.reinforced > 0 && (
          <>
            <dt>Seen again</dt>
            <dd>{plural(m.reinforced, 'time')}</dd>
          </>
        )}
        {m.origin !== 'user' && (
          <>
            <dt>Confidence</dt>
            <dd>
              <span className="mem-meter" aria-hidden>
                <i style={{ width: `${pct}%` }} />
              </span>
              {pct}%
            </dd>
          </>
        )}
      </dl>
      {m.commands.length > 0 && (
        <div className="mem-sec">
          <div className="section-label">{m.kind === 'rule' ? 'Allowed commands' : 'Commands'}</div>
          <div className="mem-cmds">
            {m.commands.map((c) => (
              <code key={c}>{c}</code>
            ))}
          </div>
        </div>
      )}
      {sources.length > 0 && (
        <div className="mem-sec">
          <div className="section-label">{sources.length === 1 ? 'Source' : 'Sources'}</div>
          {sources.map((s, i) => (
            <SourceRow key={`${s.at}-${i}`} s={s} now={now} />
          ))}
        </div>
      )}
      {history.length > 0 && (
        <div className="mem-sec">
          <div className="section-label">History</div>
          {history.map((r, i) => (
            <div key={`${r.at}-${i}`} className="mem-rev">
              <div className="mem-rev-head">
                <span className="mem-shrink">{whyLabel(r.why)}</span>
                <span className="mem-rev-time" data-tip={fullDate(r.at)}>
                  {ago(r.at, now)}
                </span>
              </div>
              {r.text !== m.text && <p className="mem-rev-text">{r.text}</p>}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function SourceRow({ s, now }: { s: Source; now: number }) {
  const title = s.title?.trim() || (s.session ? 'A conversation' : 'Added directly');
  const quote = cleanQuote(s.quote);
  const session = s.session;
  return (
    <div className="mem-src">
      <div className="mem-src-head">
        {session ? (
          <button type="button" className="mem-src-title mem-link" data-tip="Open this conversation" onClick={() => void act({ type: 'resumeSession', id: session })}>
            <Icon name="comment-discussion" />
            <span className="ellipsis">{title}</span>
          </button>
        ) : (
          <span className="mem-src-title">
            <Icon name="comment" />
            <span className="ellipsis">{title}</span>
          </span>
        )}
        <span className="mem-src-time" data-tip={fullDate(s.at)}>
          {ago(s.at, now)}
        </span>
      </div>
      {quote && <blockquote className="mem-quote">{quote}</blockquote>}
    </div>
  );
}
