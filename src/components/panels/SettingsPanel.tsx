import { invoke } from '@tauri-apps/api/core';
import { type CSSProperties, type KeyboardEvent, type ReactNode, useEffect, useRef, useState } from 'react';
import { act, openLink, pickFolder, restartToUpdate, setSide } from '../../lib/api';
import { agoLong, effortLabel, modelLabel, plural } from '../../lib/format';
import { openOverlay, toast, useUi } from '../../lib/store';
import { THEMES } from '../../lib/theme';
import type { AppDto, MemoryDto, ModelDto, Settings, TerminalShell, ThemeId, UpdateStatus } from '../../lib/types';
import { Markdown } from '../Markdown';
import { Icon, Toggle, copyText } from '../ui';
import '../../styles/panels/settings.css';

// ------------------------------------------------------------------ plumbing

/** Defaults from src-tauri/src/settings.rs, for the reset buttons. */
const DEFAULT_ZOOM = 1;
const DEFAULT_FONT_SIZE = 13.5;

/** Change settings under the keys agents use (`graysense_settings`). The core echoes them back. */
function change(changes: Partial<Record<keyof Settings, unknown>>) {
  return act<string[]>({ type: 'setSettings', changes });
}

/** Unix seconds, refreshed every `ms` (for "checked 5 minutes ago"). */
function useNow(ms: number): number {
  const [now, setNow] = useState(() => Date.now() / 1000);
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now() / 1000), ms);
    return () => window.clearInterval(t);
  }, [ms]);
  return now;
}

let shellCache: TerminalShell[] | null = null;

/** The shells on this computer (a custom one from Settings first), fetched again when it changes. */
function useShells(custom: string | null): TerminalShell[] | null {
  const [shells, setShells] = useState(shellCache);
  useEffect(() => {
    let live = true;
    invoke<TerminalShell[]>('terminal_shells')
      .then((list) => {
        shellCache = list;
        if (live) setShells(list);
      })
      .catch(() => {
        if (live) setShells((s) => s ?? []);
      });
    return () => {
      live = false;
    };
  }, [custom]);
  return shells;
}

const countActive = (list: MemoryDto[]) => list.reduce((n, m) => n + (m.state === 'active' ? 1 : 0), 0);
/** What the Memory panel asks the user to look at (counted as the rail counts it). */
const countToReview = (list: MemoryDto[]) => list.reduce((n, m) => n + (m.kind !== 'note' && (m.state === 'pending' || m.proposal != null) ? 1 : 0), 0);

/** The home folder as "~", only at a folder boundary ("C:\Users\Meg" isn't inside "C:\Users\Me"). */
function tilde(path: string, home: string): string {
  const h = home.replace(/[\\/]+$/, '');
  if (!h || path.slice(0, h.length).toLowerCase() !== h.toLowerCase()) return path;
  const rest = path.slice(h.length);
  return rest === '' || /^[\\/]/.test(rest) ? `~${rest}` : path;
}

const normPath = (p: string) => p.replace(/\//g, '\\').replace(/\\+$/, '').toLowerCase();

function joinPath(dir: string, name: string): string {
  const sep = dir.includes('/') && !dir.includes('\\') ? '/' : '\\';
  return `${dir.replace(/[\\/]+$/, '')}${sep}${name}`;
}

const round3 = (n: number) => Math.round(n * 1000) / 1000;
const snap = (n: number, step: number) => round3(Math.round(n / step) * step);

/** The next value on the `step` grid above or below `v` (also from a value off the grid). */
function stepFrom(v: number, step: number, dir: 1 | -1): number {
  const k = v / step;
  return round3((dir > 0 ? Math.floor(k + 1e-6) + 1 : Math.ceil(k - 1e-6) - 1) * step);
}

const formatZoom = (v: number) => `${Math.round(v * 100)}%`;

/** "120", "120%" and "1.2" all mean 120%. */
function parseZoom(text: string): number | null {
  const n = parseFloat(text.replace('%', ''));
  if (!Number.isFinite(n)) return null;
  return n > 4 ? n / 100 : n;
}

const formatPoints = (v: number) => String(Number(v.toFixed(1)));

function parsePoints(text: string): number | null {
  const n = parseFloat(text.replace(/pt/i, ''));
  return Number.isFinite(n) ? n : null;
}

/** The program a command line runs ("pwsh -NoProfile" → "pwsh"). */
function programOf(command: string): string {
  const c = command.trim();
  return (c.startsWith('"') ? c.slice(1).split('"')[0] : c.split(/\s+/)[0]) || c;
}

const multiplier = (n: number) => `${Number(n.toFixed(2))}×`;

function isLight(hex: string): boolean {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex);
  if (!m) return false;
  const n = parseInt(m[1], 16);
  return 0.2126 * ((n >> 16) & 255) + 0.7152 * ((n >> 8) & 255) + 0.0722 * (n & 255) > 140;
}

/** A theme's swatch as the colours its preview card paints with. */
function swatchStyle([bg, surface, accent]: [string, string, string]): CSSProperties {
  return { '--sw-bg': bg, '--sw-surface': surface, '--sw-accent': accent, '--sw-ink': isLight(bg) ? '#000' : '#fff' } as CSSProperties;
}

// ------------------------------------------------------------------ rows, sections and search

interface RowDef {
  id: string;
  label: string;
  /** What it does, in a line. */
  desc?: string;
  /** Shown in place of `desc`, such as a live status. */
  status?: ReactNode;
  /** Back to the default, offered beside the label while the value differs from it. */
  reset?: { tip: string; run: () => void };
  /** More words that find this row. */
  keywords?: string;
  /** The control at the end of the row. */
  control?: ReactNode;
  /** A wide control, under the label. */
  below?: ReactNode;
  /** A switch: the whole row flips it. */
  toggle?: { on: boolean; onChange: (on: boolean) => void; disabled?: boolean };
  /** The row opens something else. */
  open?: () => void;
  /** Free-form content instead of a label and a control. */
  custom?: ReactNode;
}

interface SectionDef {
  id: string;
  title: string;
  rows: RowDef[];
}

const NOT_WORD = /[^\p{L}\p{N}]+/gu;
const terms = (query: string) => query.toLowerCase().split(NOT_WORD).filter(Boolean);
const isWordChar = (c: string | undefined) => !!c && /[\p{L}\p{N}]/u.test(c);

/** Rows where every term starts a word ("up" finds "Update", not "support"). A section whose
 * title has every term keeps all its rows. */
function filterSections(sections: SectionDef[], words: string[]): SectionDef[] {
  if (words.length === 0) return sections;
  const has = (text: string) => {
    const t = ` ${text.toLowerCase().replace(NOT_WORD, ' ')}`;
    return words.every((w) => t.includes(` ${w}`));
  };
  return sections
    .map((s) => ({ ...s, rows: has(s.title) ? s.rows : s.rows.filter((r) => has(`${s.title} ${r.label} ${r.desc ?? ''} ${r.keywords ?? ''}`)) }))
    .filter((s) => s.rows.length > 0);
}

function highlight(text: string, words: string[]): ReactNode {
  if (words.length === 0) return text;
  const lower = text.toLowerCase();
  const spans: [number, number][] = [];
  for (const w of words) {
    for (let i = lower.indexOf(w); i >= 0; i = lower.indexOf(w, i + 1)) {
      if (!isWordChar(lower[i - 1])) spans.push([i, i + w.length]);
    }
  }
  if (spans.length === 0) return text;
  spans.sort((a, b) => a[0] - b[0]);
  const out: ReactNode[] = [];
  let at = 0;
  for (const [a, b] of spans) {
    const from = Math.max(a, at);
    if (b <= from) continue;
    if (from > at) out.push(text.slice(at, from));
    out.push(
      <mark key={from} className="set-hit">
        {text.slice(from, b)}
      </mark>,
    );
    at = b;
  }
  if (at < text.length) out.push(text.slice(at));
  return out;
}

function RowView({ row, words }: { row: RowDef; words: string[] }) {
  if (row.custom) return <div className="set-row set-row-custom">{row.custom}</div>;
  const text = (
    <span className="set-row-text">
      <span className="set-row-label">
        {highlight(row.label, words)}
        {row.reset && (
          <button type="button" className="set-reset" data-tip={row.reset.tip} aria-label={`${row.label}: ${row.reset.tip.toLowerCase()}`} onClick={row.reset.run}>
            <Icon name="discard" />
          </button>
        )}
      </span>
      {row.status ?? (row.desc ? <span className="set-row-desc">{row.desc}</span> : null)}
    </span>
  );
  if (row.toggle) {
    const t = row.toggle;
    return (
      <label className={`set-row ${t.disabled ? 'disabled' : ''}`}>
        {text}
        <Toggle on={t.on} onChange={t.onChange} disabled={t.disabled} label={row.label} />
      </label>
    );
  }
  if (row.open) {
    return (
      <button type="button" className="set-row set-row-link" onClick={row.open}>
        {text}
        <Icon name="chevron-right" className="set-row-chevron" />
      </button>
    );
  }
  return (
    <div className="set-row">
      {text}
      {row.control && <div className="set-row-control">{row.control}</div>}
      {row.below && <div className="set-row-below">{row.below}</div>}
    </div>
  );
}

function SectionView({ section, words }: { section: SectionDef; words: string[] }) {
  const titleId = `settings-${section.id}-title`;
  return (
    <section className="set-section" id={`settings-${section.id}`} aria-labelledby={titleId}>
      <h3 className="section-label set-section-title" id={titleId}>
        {section.title}
      </h3>
      <div className="set-card">
        {section.rows.map((r) => (
          <RowView key={r.id} row={r} words={words} />
        ))}
      </div>
    </section>
  );
}

// ------------------------------------------------------------------ controls

const THEME_KEYS: Partial<Record<string, number>> = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: 2, ArrowUp: -2 };

/** Theme cards, 2 × 2, each a miniature of the window in that theme's colours. */
function ThemePicker({ value }: { value: ThemeId }) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const at = THEMES.findIndex((t) => t.id === value);
  const pick = (i: number) => {
    const t = THEMES[i];
    if (!t) return;
    refs.current[i]?.focus();
    if (t.id !== value) void change({ theme: t.id });
  };
  const key = (e: KeyboardEvent<HTMLDivElement>) => {
    const n = THEMES.length;
    const step = THEME_KEYS[e.key];
    let next: number | null = null;
    if (step !== undefined) next = (((Math.max(at, 0) + step) % n) + n) % n;
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = n - 1;
    if (next === null) return;
    e.preventDefault();
    pick(next);
  };
  return (
    <div className="set-themes" role="radiogroup" aria-label="Theme" onKeyDown={key}>
      {THEMES.map((t, i) => {
        const on = t.id === value;
        return (
          <button
            key={t.id}
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={on}
            tabIndex={on || (at < 0 && i === 0) ? 0 : -1}
            className={`set-theme ${on ? 'on' : ''}`}
            onClick={() => pick(i)}
          >
            <span className="set-theme-preview" style={swatchStyle(t.swatch)} aria-hidden>
              <span className="set-tp-rail" />
              <span className="set-tp-card">
                <span className="set-tp-line" />
                <span className="set-tp-line short" />
                <span className="set-tp-accent" />
              </span>
              {on && (
                <span className="set-theme-check">
                  <Icon name="check" />
                </span>
              )}
            </span>
            <span className="set-theme-name">{t.name}</span>
            <span className="set-theme-note">{t.note}</span>
          </button>
        );
      })}
    </div>
  );
}

/** − value +. The value can also be typed, or stepped with the arrow keys. */
function Stepper({
  label,
  value,
  min,
  max,
  step,
  format,
  parse,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  format: (v: number) => string;
  parse: (text: string) => number | null;
  onChange: (v: number) => Promise<unknown>;
}) {
  // The value just asked for, until the core answers (so quick clicks add up).
  const [pending, setPending] = useState<number | null>(null);
  const [draft, setDraft] = useState<string | null>(null);
  const gen = useRef(0);
  const cancel = useRef(false);
  const shown = pending ?? value;
  const eps = step / 100;

  const commit = (n: number) => {
    const next = Math.min(max, Math.max(min, snap(n, step)));
    if (Math.abs(next - shown) < eps) return;
    const g = ++gen.current;
    setPending(next);
    void onChange(next).finally(() => {
      if (gen.current === g) setPending(null);
    });
  };
  const bounded = (n: number) => Math.min(max, Math.max(min, n));
  const what = label.toLowerCase();

  return (
    <div className="set-stepper" role="group" aria-label={label}>
      <button type="button" className="set-step" aria-label={`Decrease ${what}`} disabled={shown <= min + eps} onClick={() => commit(stepFrom(shown, step, -1))}>
        <Icon name="remove" />
      </button>
      <input
        className="set-step-value"
        value={draft ?? format(shown)}
        inputMode="decimal"
        spellCheck={false}
        aria-label={label}
        onFocus={(e) => {
          setDraft(format(shown));
          e.currentTarget.select();
        }}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => {
          const text = draft;
          setDraft(null);
          if (cancel.current) {
            cancel.current = false;
            return;
          }
          const n = text === null ? null : parse(text);
          if (n !== null) commit(n);
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') e.currentTarget.blur();
          else if (e.key === 'Escape') {
            e.stopPropagation();
            cancel.current = true;
            e.currentTarget.blur();
          } else if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
            e.preventDefault();
            const next = bounded(stepFrom((draft === null ? null : parse(draft)) ?? shown, step, e.key === 'ArrowUp' ? 1 : -1));
            setDraft(format(next));
            commit(next);
          }
        }}
      />
      <button type="button" className="set-step" aria-label={`Increase ${what}`} disabled={shown >= max - eps} onClick={() => commit(stepFrom(shown, step, 1))}>
        <Icon name="add" />
      </button>
    </div>
  );
}

function ModelSelect({ value, models }: { value: string | null; models: ModelDto[] }) {
  const known = value === null || models.some((m) => m.id === value);
  return (
    <select className="select" aria-label="Default model" value={value ?? ''} onChange={(e) => void change({ default_model: e.target.value || null })}>
      <option value="">Automatic</option>
      {models.map((m) => (
        <option key={m.id} value={m.id}>
          {m.multiplier === null ? m.name : `${m.name} · ${multiplier(m.multiplier)}`}
        </option>
      ))}
      {!known && value !== null && <option value={value}>{modelLabel(value)}</option>}
    </select>
  );
}

/** Reasoning levels, least to most; the select offers the ones some model has. */
const EFFORT_ORDER = ['minimal', 'low', 'medium', 'high', 'xhigh', 'max'];

function EffortSelect({ value, models }: { value: string | null; models: ModelDto[] }) {
  const offered = new Set(models.flatMap((m) => m.efforts));
  const levels = offered.size ? EFFORT_ORDER.filter((e) => offered.has(e)) : ['low', 'medium', 'high'];
  if (value && !levels.includes(value)) levels.push(value);
  return (
    <select className="select" aria-label="Thinking effort" value={value ?? ''} onChange={(e) => void change({ default_effort: e.target.value || null })}>
      <option value="">Each model’s default</option>
      {levels.map((e) => (
        <option key={e} value={e}>
          {effortLabel(e)}
        </option>
      ))}
    </select>
  );
}

/** The shells found here (saved by id, so each keeps its own arguments), or a command line of your own ("Custom…"). */
function ShellPicker({ value, shells }: { value: string | null; shells: TerminalShell[] | null }) {
  const current = value?.trim() || null;
  const detected = (shells ?? []).filter((s) => s.id !== 'custom');
  const resolved = shells?.find((s) => s.id === 'custom') ?? null;
  const matched = current === null ? null : (detected.find((s) => s.id === current) ?? null);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(current ?? '');
  const inputRef = useRef<HTMLInputElement>(null);
  const cancel = useRef(false);

  // Follow the setting (an agent may change it too).
  useEffect(() => {
    setEditing(false);
    setDraft(current ?? '');
  }, [current]);

  useEffect(() => {
    if (editing) inputRef.current?.focus();
  }, [editing]);

  if (shells === null) {
    return (
      <select className="select" aria-label="Shell" disabled>
        <option>Finding shells…</option>
      </select>
    );
  }

  const custom = editing || (current !== null && matched === null);
  const choice = custom ? 'custom' : matched ? `shell:${matched.id}` : 'auto';
  // Without a chosen shell the list is in the core's order, default first: what "Automatic" runs.
  const fallback = detected[0]?.name;
  const autoLabel = matched === null && fallback ? `Automatic (${fallback})` : 'Automatic';

  const pick = (v: string) => {
    if (v === 'custom') {
      setDraft(current ?? '');
      setEditing(true);
      return;
    }
    setEditing(false);
    if (v === 'auto') {
      if (current !== null) void change({ shell: null });
      return;
    }
    const s = detected.find((x) => `shell:${x.id}` === v);
    if (s) void change({ shell: s.id });
  };

  const commit = () => {
    if (cancel.current) {
      cancel.current = false;
      return;
    }
    const t = draft.trim();
    if (t === (current ?? '')) setEditing(false);
    else void change({ shell: t || null });
  };

  return (
    <div className="set-stack">
      <select className="select" aria-label="Shell" value={choice} onChange={(e) => pick(e.target.value)}>
        <option value="auto">{autoLabel}</option>
        {detected.map((s) => (
          <option key={s.id} value={`shell:${s.id}`}>
            {s.name}
          </option>
        ))}
        <option value="custom">Custom…</option>
      </select>
      {custom && (
        <>
          <input
            ref={inputRef}
            className="input mono"
            value={draft}
            placeholder="pwsh -NoProfile"
            spellCheck={false}
            aria-label="Shell command"
            onChange={(e) => {
              setDraft(e.target.value);
              setEditing(true);
            }}
            onBlur={commit}
            onKeyDown={(e) => {
              if (e.key === 'Enter') e.currentTarget.blur();
              else if (e.key === 'Escape') {
                e.stopPropagation();
                cancel.current = true;
                setDraft(current ?? '');
                setEditing(false);
                e.currentTarget.blur();
              }
            }}
          />
          {editing || current === null ? (
            <span className="set-hint">A program and its arguments. Quote paths that have spaces. Leave empty for automatic.</span>
          ) : resolved ? (
            <span className="set-hint">
              Runs <span className="mono selectable">{resolved.path}</span>
            </span>
          ) : (
            <span className="set-hint warn">
              <Icon name="warning" />
              <span>
                Can't find {programOf(current)}. New terminals use {fallback ?? 'the default shell'}.
              </span>
            </span>
          )}
        </>
      )}
    </div>
  );
}

/** A path with "~" for home. When it's too long the start gives way, so the last folders stay readable. */
function PathField({ path, home, onOpen }: { path: string; home: string; onOpen?: () => void }) {
  const shown = tilde(path, home);
  const trimmed = shown.length > 3 ? shown.replace(/[\\/]+$/, '') : shown;
  const cut = Math.max(trimmed.lastIndexOf('\\'), trimmed.lastIndexOf('/'));
  const head = cut > 0 ? trimmed.slice(0, cut) : '';
  const tail = cut > 0 ? trimmed.slice(cut) : trimmed;
  return (
    <div className="set-path">
      <span className="set-path-text selectable" data-tip={path}>
        <bdi dir="ltr">
          {head && <span className="set-path-head">{head}</span>}
          <span className="set-path-tail">{tail}</span>
        </bdi>
      </span>
      {onOpen && (
        <button type="button" className="icon-btn sm" data-tip="Open" aria-label={`Open ${tail.replace(/^[\\/]/, '')}`} onClick={onOpen}>
          <Icon name="go-to-file" />
        </button>
      )}
      <button type="button" className="icon-btn sm" data-tip="Show in File Explorer" aria-label="Show in File Explorer" onClick={() => void act({ type: 'revealPath', path })}>
        <Icon name="folder-opened" />
      </button>
    </div>
  );
}

function SessionsFolder({ path, home }: { path: string; home: string }) {
  const fallback = home ? joinPath(home, 'Graysense\\Sessions') : null;
  const isDefault = fallback === null || normPath(path) === normPath(fallback);
  const browse = async () => {
    try {
      const dir = await pickFolder('Choose where new sessions go', path);
      if (dir) await change({ sessions_folder: dir });
    } catch (e) {
      toast('error', String(e));
    }
  };
  return (
    <div className="set-stack">
      <PathField path={path} home={home} />
      <div className="set-actions">
        <button type="button" className="btn sm" onClick={() => void browse()}>
          <Icon name="folder" />
          Browse…
        </button>
        {!isDefault && fallback && (
          <button type="button" className="btn sm ghost" data-tip={tilde(fallback, home)} onClick={() => void change({ sessions_folder: null })}>
            <Icon name="discard" />
            Reset to default
          </button>
        )}
      </div>
    </div>
  );
}

function MemoryStatus({ on, count, review }: { on: boolean; count: number; review: number }) {
  const parts = on ? ['On', count ? plural(count, 'memory', 'memories') : 'no memories yet', ...(review ? [`${review} to review`] : [])] : ['Off', 'nothing is learned or shared'];
  return (
    <span className="set-row-desc set-memory-status">
      <span className={`dot ${on ? 'idle' : ''}`} />
      {parts.join(' · ')}
    </span>
  );
}

// ------------------------------------------------------------------ updates and about

const NOTES_PHASES: UpdateStatus['phase'][] = ['available', 'downloading', 'ready', 'applying'];

/** How an update check signed in to GitHub (" as GitHub CLI (octocat)"). */
function viaPhrase(via: string | null): string {
  if (!via) return '';
  if (via === 'no sign-in') return ' without signing in';
  if (/^[A-Z][A-Z0-9_]*$/.test(via)) return ` with ${via}`;
  if (via.startsWith('GitHub CLI')) return ` as ${via}`;
  return ` from ${via}`;
}

const releasesUrl = (notesUrl: string) => notesUrl.replace(/\/tag\/[^/]*$/, '');

function About({ app, update, auto }: { app: AppDto; update: UpdateStatus | null; auto: boolean }) {
  return (
    <>
      <div className="set-about">
        <img className="set-about-logo" src="/logo-128.png" alt="" width={44} height={44} draggable={false} />
        <div className="set-about-text">
          <div className="set-about-title">
            <span className="set-about-name">Graysense</span>
            <span className="set-about-version selectable" data-tip={update?.phase === 'unsupported' ? 'Built from source' : undefined}>
              {update?.version ?? app.version}
            </span>
          </div>
          <span className="set-about-tagline">A workbench for Copilot agents</span>
        </div>
      </div>
      {update && <UpdateBlock update={update} auto={auto} />}
    </>
  );
}

function UpdateBlock({ update, auto }: { update: UpdateStatus; auto: boolean }) {
  const now = useNow(30_000);
  const v = update.nextVersion;
  const checked = update.checked !== null ? `Checked ${agoLong(update.checked, now)}${viaPhrase(update.via)}` : null;
  const check = () => void act({ type: 'updateCheck' });
  const checkButton = (busy: boolean) => (
    <button type="button" className="btn sm" disabled={busy} onClick={check}>
      <Icon name="sync" />
      Check for updates
    </button>
  );

  let tone: 'ok' | 'accent' | 'warn' | 'faint' = 'faint';
  let icon: string | null = 'info';
  let title = '';
  let detail: ReactNode = null;
  let action: ReactNode = null;
  switch (update.phase) {
    case 'unsupported':
      title = 'Updates are available in installed copies of Graysense.';
      break;
    case 'idle':
      icon = 'sync';
      title = 'Not checked yet';
      detail = auto ? 'Graysense checks for updates on its own.' : null;
      action = checkButton(false);
      break;
    case 'checking':
      icon = null;
      title = 'Checking for updates…';
      action = checkButton(true);
      break;
    case 'upToDate':
      tone = 'ok';
      icon = 'pass-filled';
      title = "You're up to date";
      detail = checked;
      action = checkButton(false);
      break;
    case 'available':
      tone = 'accent';
      icon = 'cloud-download';
      title = v ? `Version ${v} is available` : 'An update is available';
      detail = checked;
      action = auto ? null : (
        <button type="button" className="btn sm primary" onClick={() => void act({ type: 'updateDownload' })}>
          <Icon name="cloud-download" />
          Download
        </button>
      );
      break;
    case 'downloading': {
      const pct = Math.round(Math.min(1, Math.max(0, update.progress ?? 0)) * 100);
      tone = 'accent';
      icon = 'cloud-download';
      title = v ? `Downloading version ${v}` : 'Downloading the update';
      detail = (
        <span className="set-progress-row">
          <span className="set-progress" role="progressbar" aria-label="Download progress" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct}>
            <span style={{ width: `${pct}%` }} />
          </span>
          <span className="set-progress-pct">{pct}%</span>
        </span>
      );
      break;
    }
    case 'ready':
      tone = 'accent';
      icon = 'arrow-circle-up';
      title = v ? `Version ${v} is ready` : 'The update is ready';
      detail = 'It installs when you restart or close Graysense.';
      action = (
        <button type="button" className="btn sm primary" onClick={() => void restartToUpdate()}>
          <Icon name="debug-restart" />
          Restart to update
        </button>
      );
      break;
    case 'applying':
      icon = null;
      title = 'Restarting to update…';
      detail = 'Graysense reopens in a moment.';
      break;
    case 'failed':
      tone = 'warn';
      icon = 'warning';
      title = update.error ?? "Couldn't check for updates.";
      action = (
        <button type="button" className="btn sm" onClick={check}>
          <Icon name="refresh" />
          Try again
        </button>
      );
      break;
  }

  const installed = update.phase !== 'unsupported';
  const notes = update.nextNotes && NOTES_PHASES.includes(update.phase) ? update.nextNotes : null;
  return (
    <div className="set-update">
      <div className={`set-update-status ${tone}`}>
        <span className="set-update-icon">{icon ? <Icon name={icon} /> : <span className="spinner sm" />}</span>
        <span className="set-update-text">
          <span className="set-update-title" role="status">
            {title}
          </span>
          {detail && <span className="set-update-detail">{detail}</span>}
        </span>
      </div>
      {notes && <Notes version={v} notes={notes} />}
      <div className="set-update-actions">
        {action}
        <span className="grow" />
        <button type="button" className="link-btn set-link" onClick={() => openLink(installed ? update.notesUrl : releasesUrl(update.notesUrl))}>
          {installed ? 'Release notes' : 'Releases'}
          <Icon name="link-external" />
        </button>
      </div>
    </div>
  );
}

function Notes({ version, notes }: { version: string | null; notes: string }) {
  const [open, setOpen] = useState(false);
  return (
    <div className={`set-notes ${open ? 'open' : ''}`}>
      <button type="button" className="set-notes-head" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        <Icon name="chevron-right" className="set-notes-caret" />
        <span className="ellipsis">{version ? `What's new in ${version}` : "What's new"}</span>
      </button>
      {open && (
        <div className="set-notes-body">
          <Markdown text={notes} />
        </div>
      )}
    </div>
  );
}

/** What a bug report needs: version, connection, updates and settings (home as "~", no recent folders). */
function diagnostics(app: AppDto, settings: Settings, update: UpdateStatus | null): string {
  const tidy = (p: string) => tilde(p, app.home);
  const { recent_dirs, ...rest } = settings;
  const values = { ...rest, sessions_folder: tidy(rest.sessions_folder), recent_dirs: recent_dirs.length };
  const updates = update
    ? [update.phase, update.error, update.nextVersion && `next ${update.nextVersion}`, update.checked !== null && `checked ${new Date(update.checked * 1000).toISOString()}`, update.via && `via ${update.via}`]
        .filter(Boolean)
        .join(', ')
    : 'unknown';
  return [
    `Graysense ${update?.version ?? app.version}${update?.phase === 'unsupported' ? ' (built from source)' : ''}`,
    `Copilot: ${app.client}${app.clientError ? ` (${app.clientError})` : ''}, ${plural(app.models.length, 'model')}`,
    `Updates: ${updates}`,
    `Config: ${tidy(app.configDir)}`,
    `WebView: ${navigator.userAgent}`,
    '',
    'Settings:',
    JSON.stringify(values, null, 2),
  ].join('\n');
}

// ------------------------------------------------------------------ the panel

interface Ctx {
  settings: Settings;
  app: AppDto;
  update: UpdateStatus | null;
  shells: TerminalShell[] | null;
  memories: number;
  toReview: number;
}

function modelDesc(app: AppDto): string {
  if (app.models.length > 0 || app.client === 'ready') return 'The model new agents start with.';
  if (app.client === 'failed') return "Copilot isn't connected, so its models can't be listed.";
  return 'The model new agents start with. Models appear once Copilot connects.';
}

/** A row's reset, while `value` isn't the default. */
function resetTo(value: number, fallback: number, format: (v: number) => string, set: (v: number) => unknown): RowDef['reset'] {
  if (Math.abs(value - fallback) < 1e-6) return undefined;
  return { tip: `Reset to ${format(fallback)}`, run: () => void set(fallback) };
}

function buildSections({ settings, app, update, shells, memories, toReview }: Ctx): SectionDef[] {
  const log = joinPath(app.configDir, 'graysense.log');
  return [
    {
      id: 'appearance',
      title: 'Appearance',
      rows: [
        {
          id: 'theme',
          label: 'Theme',
          desc: 'Paper is light; the others are dark.',
          keywords: `colour color dark light mode ${THEMES.map((t) => t.name).join(' ')}`,
          below: <ThemePicker value={settings.theme} />,
        },
        {
          id: 'zoom',
          label: 'Zoom',
          desc: 'Scale the interface.',
          keywords: 'interface ui size scale bigger smaller text',
          reset: resetTo(settings.zoom, DEFAULT_ZOOM, formatZoom, (zoom) => change({ zoom })),
          control: <Stepper label="Zoom" value={settings.zoom} min={0.7} max={1.6} step={0.1} format={formatZoom} parse={parseZoom} onChange={(zoom) => change({ zoom })} />,
        },
        {
          id: 'reasoning',
          label: 'Show agent reasoning',
          desc: "Include agents' thinking in transcripts.",
          keywords: 'thoughts thinking',
          toggle: { on: settings.show_reasoning, onChange: (on) => void change({ show_reasoning: on }) },
        },
        {
          id: 'stats',
          label: 'System stats in status bar',
          desc: 'Show CPU and memory use.',
          keywords: 'performance hud ram usage',
          toggle: { on: settings.performance_hud, onChange: (on) => void change({ performance_hud: on }) },
        },
      ],
    },
    {
      id: 'agents',
      title: 'Agents',
      rows: [
        {
          id: 'model',
          label: 'Default model',
          desc: modelDesc(app),
          keywords: `ai llm ${app.models.map((m) => m.name).join(' ')}`,
          below: <ModelSelect value={settings.default_model} models={app.models} />,
        },
        {
          id: 'effort',
          label: 'Thinking effort',
          desc: 'How hard new agents think. More is slower but handles harder problems; models without the level use their own default.',
          keywords: 'reasoning effort think high low medium xhigh max',
          below: <EffortSelect value={settings.default_effort} models={app.models} />,
        },
        {
          id: 'auto-approve',
          label: 'Approve all tool calls by default',
          desc: 'New sessions run tools without asking, including commands and file changes. Existing and resumed sessions are unchanged. You can turn this off for each session.',
          keywords: 'approval permissions allow automatic new sessions',
          toggle: { on: settings.default_auto_approve, onChange: (on) => void change({ default_auto_approve: on }) },
        },
        {
          id: 'session-folders',
          label: 'Give each session its own folder',
          desc: 'Named after its first message, inside the sessions folder.',
          keywords: 'new directory',
          toggle: { on: settings.session_folders, onChange: (on) => void change({ session_folders: on }) },
        },
        {
          id: 'sessions-folder',
          label: 'Sessions folder',
          desc: 'Where new session folders are made.',
          keywords: 'path location directory browse',
          below: <SessionsFolder path={settings.sessions_folder} home={app.home} />,
        },
      ],
    },
    {
      id: 'terminal',
      title: 'Terminal',
      rows: [
        {
          id: 'font-size',
          label: 'Font size',
          desc: 'Terminal text size.',
          keywords: 'text points zoom',
          reset: resetTo(settings.terminal_font_size, DEFAULT_FONT_SIZE, formatPoints, (size) => change({ terminal_font_size: size })),
          control: (
            <Stepper
              label="Font size"
              value={settings.terminal_font_size}
              min={9}
              max={28}
              step={0.5}
              format={formatPoints}
              parse={parsePoints}
              onChange={(size) => change({ terminal_font_size: size })}
            />
          ),
        },
        {
          id: 'cursor-blink',
          label: 'Blinking cursor',
          desc: 'Blink the cursor in terminals.',
          keywords: 'caret',
          toggle: { on: settings.cursor_blink, onChange: (on) => void change({ cursor_blink: on }) },
        },
        {
          id: 'shell',
          label: 'Shell',
          desc: 'What new terminals run.',
          keywords: `program command line custom powershell pwsh cmd bash wsl ${(shells ?? []).map((s) => s.name).join(' ')}`,
          below: <ShellPicker value={settings.shell} shells={shells} />,
        },
      ],
    },
    {
      id: 'memory',
      title: 'Memory',
      rows: [
        {
          id: 'memory',
          label: 'Memory',
          keywords: "remember recall learn share what's relevant with agents",
          status: <MemoryStatus on={settings['memory.enabled']} count={memories} review={toReview} />,
          toggle: { on: settings['memory.enabled'], onChange: (on) => void change({ 'memory.enabled': on }) },
        },
        {
          id: 'manage-memory',
          label: 'Manage memory',
          desc: 'Review, edit and search what agents remember.',
          keywords: 'memories',
          open: () => void setSide('memory'),
        },
      ],
    },
    {
      id: 'updates',
      title: 'Updates & about',
      rows: [
        {
          id: 'about',
          label: 'About Graysense',
          keywords: "version check download install restart release notes changelog what's new",
          custom: <About app={app} update={update} auto={settings.auto_update} />,
        },
        {
          id: 'auto-update',
          label: 'Update automatically',
          desc: 'Download new versions in the background and install them on restart.',
          keywords: 'auto',
          toggle: { on: settings.auto_update, onChange: (on) => void change({ auto_update: on }), disabled: update?.phase === 'unsupported' },
        },
      ],
    },
    {
      id: 'advanced',
      title: 'Advanced',
      rows: [
        {
          id: 'config',
          label: 'Config folder',
          desc: 'Settings, memories and workspace state.',
          keywords: 'configuration appdata profile files path',
          below: <PathField path={app.configDir} home={app.home} />,
        },
        {
          id: 'log',
          label: 'Log file',
          desc: 'Starts fresh each time Graysense opens.',
          keywords: 'logs logging debug troubleshoot',
          below: <PathField path={log} home={app.home} onOpen={() => void act({ type: 'openPath', path: log })} />,
        },
        {
          id: 'diagnostics',
          label: 'Diagnostics',
          desc: 'Version, connection and settings, for bug reports.',
          keywords: 'copy debug bug report support troubleshoot',
          control: (
            <button type="button" className="btn sm" onClick={() => void copyText(diagnostics(app, settings, update), 'Diagnostics copied')}>
              <Icon name="copy" />
              Copy
            </button>
          ),
        },
      ],
    },
  ];
}

export function SettingsPanel() {
  const settings = useUi((s) => s.settings);
  const app = useUi((s) => s.app);
  const update = useUi((s) => s.update);
  const memories = useUi((s) => countActive(s.memory.memories));
  const toReview = useUi((s) => countToReview(s.memory.memories));
  const shells = useShells(settings?.shell ?? null);
  const [query, setQuery] = useState('');
  const searchRef = useRef<HTMLInputElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bodyRef.current?.scrollTo({ top: 0 });
  }, [query]);

  const head = (
    <div className="panel-head">
      <h2>Settings</h2>
      <button type="button" className="icon-btn" data-tip="Keyboard shortcuts" data-keys="Ctrl+/" aria-label="Keyboard shortcuts" onClick={() => openOverlay({ kind: 'shortcuts' })}>
        <Icon name="keyboard" />
      </button>
    </div>
  );

  if (!settings || !app) {
    return (
      <div className="panel set-panel">
        {head}
        <div className="panel-body set-body">
          <div className="empty">
            <div className="spinner" />
          </div>
        </div>
      </div>
    );
  }

  const words = terms(query);
  const sections = filterSections(buildSections({ settings, app, update, shells, memories, toReview }), words);

  return (
    <div className="panel set-panel">
      {head}
      <div className="panel-toolbar">
        <div className="search-field set-search">
          <Icon name="search" />
          <input
            ref={searchRef}
            className="input"
            value={query}
            placeholder="Search settings"
            spellCheck={false}
            aria-label="Search settings"
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape' && query) {
                e.stopPropagation();
                setQuery('');
              }
            }}
          />
          {query && (
            <button
              type="button"
              className="icon-btn sm set-search-clear"
              data-tip="Clear"
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
      </div>
      <div ref={bodyRef} className="panel-body set-body">
        {words.length === 0 && (
          <div className="set-callout">
            <Icon name="sparkle" />
            <span>Agents can change these settings too — just ask, e.g. “switch to the light theme”.</span>
          </div>
        )}
        {sections.map((s) => (
          <SectionView key={s.id} section={s} words={words} />
        ))}
        {sections.length === 0 && (
          <div className="empty set-empty">
            <Icon name="search" />
            <h4>No matching settings</h4>
            <p>Nothing matches “{query.trim()}”. Try another word, or ask an agent.</p>
          </div>
        )}
      </div>
    </div>
  );
}
