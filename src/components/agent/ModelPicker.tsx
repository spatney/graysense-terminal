import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { act } from '../../lib/api';
import { effortLabel, modelLabel } from '../../lib/format';
import { useUi } from '../../lib/store';
import type { BayDto, ModelCategory, ModelDto } from '../../lib/types';
import { Icon } from '../ui';

const NO_MODELS: ModelDto[] = [];

const GROUPS: { id: ModelCategory | 'other'; label: string; tip: string }[] = [
  { id: 'powerful', label: 'Powerful', tip: 'For complex, multi-step work' },
  { id: 'versatile', label: 'Versatile', tip: 'A good fit for most tasks' },
  { id: 'lightweight', label: 'Fast', tip: 'Quick answers that cost less' },
  { id: 'other', label: 'More', tip: '' },
];

/** The effort a session thinks at: the level chosen when the model offers it, else its default. */
export function effortOf(model: ModelDto | undefined, chosen: string | null): string | null {
  if (!model?.efforts.length) return null;
  return chosen && model.efforts.includes(chosen) ? chosen : (model.defaultEffort ?? null);
}

/** What a bay's model chip says, and what the picker edits (the defaults, on the home screen). */
export function useModelChoice(bay: BayDto | null) {
  const models = useUi((s) => s.app?.models ?? NO_MODELS);
  const defaultModel = useUi((s) => s.settings?.default_model ?? null);
  const defaultEffort = useUi((s) => s.settings?.default_effort ?? null);
  const runtimeModel = useUi((s) => s.app?.runtimeModel ?? null);
  const agent = bay?.agent ?? null;
  // Nothing chosen: sessions start with Copilot's own default, once Graysense has seen which it is.
  const id = (bay ? agent?.model : null) ?? defaultModel ?? runtimeModel;
  const model = models.find((m) => m.id === id);
  const effort = effortOf(model, bay ? (agent?.effort ?? null) : defaultEffort);
  const name = modelLabel(id, models);
  // The chip is tight: "Sonnet 5.5" says it (the picker shows full names).
  return { models, id, model, effort, name, label: name.replace(/^Claude (?=\S)/, '') };
}

/**
 * Pick a model and how hard it thinks. For a bay it changes that session; on the home screen
 * it changes what new sessions start with.
 */
export function ModelPicker({ anchor, bay, onClose }: { anchor: HTMLElement; bay: BayDto | null; onClose: () => void }) {
  const { models, id, model, effort } = useModelChoice(bay);
  const defaultModel = useUi((s) => s.settings?.default_model ?? s.app?.runtimeModel ?? null);
  const defaultEffort = useUi((s) => s.settings?.default_effort ?? null);
  const [query, setQuery] = useState('');
  const [hot, setHot] = useState(-1);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  const q = query.trim().toLowerCase();
  const auto = models.find((m) => m.id === 'auto' && (!q || 'auto'.includes(q) || m.name.toLowerCase().includes(q)));
  const groups = useMemo(() => {
    const shown = (q ? models.filter((m) => m.name.toLowerCase().includes(q) || m.id.toLowerCase().includes(q)) : models).filter((m) => m.id !== 'auto');
    return GROUPS.map((g) => ({ ...g, models: shown.filter((m) => (m.category ?? 'other') === g.id) })).filter((g) => g.models.length);
  }, [models, q]);
  const flat = useMemo(() => [...(auto ? [auto] : []), ...groups.flatMap((g) => g.models)], [auto, groups]);
  const levels = model?.efforts.length ? ['auto', ...model.efforts] : [];
  // Point at the current model when the list first shows (or after filtering, the first match).
  useEffect(() => {
    byKey.current = true;
    setHot(q ? (flat.length ? 0 : -1) : flat.findIndex((m) => m.id === id));
  }, [q, flat.length]); // eslint-disable-line react-hooks/exhaustive-deps

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = anchor.getBoundingClientRect();
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    const left = Math.min(Math.max(8, r.right - w), window.innerWidth - w - 8);
    const above = r.top - h - 8 >= 8 || r.top > window.innerHeight - r.bottom;
    setPos({ left, top: above ? Math.max(8, r.top - h - 8) : Math.min(r.bottom + 8, window.innerHeight - h - 8) });
  }, [anchor, groups.length, flat.length, !!model?.efforts.length]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    (searchRef.current ?? ref.current)?.focus();
    const down = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node) && !anchor.contains(e.target as Node)) onClose();
    };
    window.addEventListener('pointerdown', down, true);
    window.addEventListener('blur', onClose);
    window.addEventListener('resize', onClose);
    return () => {
      window.removeEventListener('pointerdown', down, true);
      window.removeEventListener('blur', onClose);
      window.removeEventListener('resize', onClose);
    };
  }, [anchor, onClose]);

  // The current model starts in the middle of the list; arrow keys then keep their row in view
  // (hovering never scrolls the list under the pointer).
  const placed = useRef(false);
  const byKey = useRef(false);
  useEffect(() => {
    const row = ref.current?.querySelector('.mp-item.hot');
    if (!row || (placed.current && !byKey.current)) return;
    row.scrollIntoView({ block: placed.current ? 'nearest' : 'center' });
    placed.current = true;
    byKey.current = false;
  }, [hot]);

  const pickModel = (m: ModelDto) => {
    if (bay) void act({ type: 'setModel', bay: bay.id, model: m.id });
    else void act({ type: 'setSettings', changes: { default_model: m.id } });
    onClose();
  };
  const pickEffort = (e: string) => {
    if (!model) return;
    // "auto": no override, so the session thinks at the model's own default.
    if (bay) void act({ type: 'setModel', bay: bay.id, model: model.id, effort: e });
    else void act({ type: 'setSettings', changes: { default_effort: e === 'auto' ? null : e } });
  };
  const isDefault = !!model && model.id === defaultModel && effortOf(model, defaultEffort) === effort;
  const makeDefault = () => {
    if (!model) return;
    void act({ type: 'setSettings', changes: { default_model: model.id, default_effort: effort } });
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      if (query) setQuery('');
      else onClose();
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      if (!flat.length) return;
      const step = e.key === 'ArrowDown' ? 1 : -1;
      byKey.current = true;
      setHot((h) => (h < 0 ? (step > 0 ? 0 : flat.length - 1) : (h + step + flat.length) % flat.length));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (flat[hot]) pickModel(flat[hot]);
    } else if ((e.key === 'ArrowLeft' || e.key === 'ArrowRight') && levels.length && !(e.target instanceof HTMLInputElement && query)) {
      e.preventDefault();
      const at = levels.indexOf(effort ?? 'auto');
      const next = levels[Math.min(levels.length - 1, Math.max(0, at + (e.key === 'ArrowRight' ? 1 : -1)))];
      if (next && next !== (effort ?? 'auto')) pickEffort(next);
    }
  };

  return createPortal(
    <div ref={ref} className="mp" role="dialog" aria-label="Model" tabIndex={-1} onKeyDown={onKey} style={pos ?? { left: -9999, top: -9999 }}>
      {levels.length > 0 && (
        <div className="mp-think">
          <div className="mp-think-head">
            <Icon name="lightbulb" />
            <span>Thinking</span>
            <span className="grow" />
            <span className="mp-think-now">{effort ? effortLabel(effort) : `Auto${model?.defaultEffort ? ` (${effortLabel(model.defaultEffort)})` : ''}`}</span>
          </div>
          <div className="mp-levels" role="radiogroup" aria-label="Thinking effort">
            {levels.map((e, i) => {
              const on = e === (effort ?? 'auto');
              return (
                <button
                  key={e}
                  type="button"
                  role="radio"
                  aria-checked={on}
                  className={`mp-level${on ? ' on' : ''}`}
                  data-tip={e === 'auto' ? 'The model decides how much to think' : effortLabel(e)}
                  onClick={() => pickEffort(e)}
                >
                  {e === 'auto' ? (
                    <Icon name="sparkle" className="mp-auto" />
                  ) : (
                    <span className="mp-bars" aria-hidden>
                      {levels.slice(1).map((_, j) => (
                        <i key={j} className={j < i ? 'lit' : ''} />
                      ))}
                    </span>
                  )}
                  {e === 'auto' ? 'Auto' : effortLabel(e, true)}
                </button>
              );
            })}
          </div>
          <div className="mp-think-hint">More thinking is slower and uses more of the context window, but handles harder problems.</div>
        </div>
      )}
      {models.length > 8 && (
        <div className="mp-search">
          <Icon name="search" />
          <input ref={searchRef} value={query} placeholder="Find a model" spellCheck={false} onChange={(e) => setQuery(e.target.value)} />
        </div>
      )}
      <div className="mp-list" role="listbox" aria-label="Models">
        {!models.length && <div className="mp-empty">Loading models…</div>}
        {models.length > 0 && !flat.length && <div className="mp-empty">No model matches “{query.trim()}”.</div>}
        {auto && (
          <button
            type="button"
            role="option"
            aria-selected={auto.id === id}
            className={`mp-item mp-auto-row${flat.indexOf(auto) === hot ? ' hot' : ''}${auto.id === id ? ' on' : ''}`}
            onPointerMove={() => setHot(flat.indexOf(auto))}
            onClick={() => pickModel(auto)}
          >
            <Icon name={auto.id === id ? 'check' : 'blank'} className="mp-check" />
            <span className="mp-name">
              <span className="ellipsis">{auto.name || 'Auto'}</span>
              <span className="mp-sub ellipsis">Copilot picks a model for each task</span>
            </span>
            {auto.id === defaultModel && <span className="mp-badge">Default</span>}
          </button>
        )}
        {groups.map((g) => (
          <div key={g.id} className="mp-group" role="group" aria-label={g.label}>
            <div className="mp-group-head" data-tip={g.tip || undefined}>
              {g.label}
            </div>
            {g.models.map((m) => {
              const i = flat.indexOf(m);
              return (
                <button
                  key={m.id}
                  type="button"
                  role="option"
                  aria-selected={m.id === id}
                  className={`mp-item${i === hot ? ' hot' : ''}${m.id === id ? ' on' : ''}`}
                  onPointerMove={() => setHot(i)}
                  onClick={() => pickModel(m)}
                >
                  <Icon name={m.id === id ? 'check' : 'blank'} className="mp-check" />
                  <span className="mp-name ellipsis">{m.name || m.id}</span>
                  {m.id === defaultModel && <span className="mp-badge">Default</span>}
                  {m.multiplier != null && (
                    <span className="mp-cost" data-tip="Premium requests per message">
                      {m.multiplier}×
                    </span>
                  )}
                </button>
              );
            })}
          </div>
        ))}
      </div>
      {bay && model && (
        <div className="mp-foot">
          <button type="button" className="mp-item" disabled={isDefault} onClick={makeDefault}>
            <Icon name={isDefault ? 'pass-filled' : 'star-empty'} className="mp-check" />
            <span className="mp-name ellipsis">{isDefault ? 'New sessions start with this' : 'Start new sessions with this'}</span>
          </button>
        </div>
      )}
    </div>,
    document.body,
  );
}
