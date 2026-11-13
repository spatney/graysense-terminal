import { useMemo } from 'react';
import { act, setSide } from '../../lib/api';
import { ago, shortPath } from '../../lib/format';
import { focusComposer, setDraft, useUi } from '../../lib/store';
import type { BayDto } from '../../lib/types';
import { Icon } from '../ui';
import { Composer } from './Composer';

const SUGGESTIONS: { icon: string; label: string; prompt: string }[] = [
  { icon: 'search', label: 'Explain a codebase', prompt: 'Walk me through how this project is organized and how the main pieces fit together.' },
  { icon: 'bug', label: 'Find and fix a bug', prompt: 'Help me find and fix a bug: ' },
  { icon: 'beaker', label: 'Write tests', prompt: 'Add focused tests for the most important behavior that is not covered yet.' },
  { icon: 'checklist', label: 'Plan a feature', prompt: 'Plan this feature with me before writing code: ' },
  { icon: 'rocket', label: 'Prototype an app', prompt: 'Build a small, polished prototype of ' },
];

function greeting(): string {
  const h = new Date().getHours();
  if (h < 5) return 'Up late';
  if (h < 12) return 'Good morning';
  if (h < 18) return 'Good afternoon';
  return 'Good evening';
}

/** An empty session (or the home screen when `bay` is null): a greeting and the message box. */
export function Hero({ bay }: { bay: BayDto | null }) {
  const root = useUi((s) => s.app?.sessionsRoot);
  const home = useUi((s) => s.app?.home);
  const folders = useUi((s) => s.settings?.session_folders ?? true);
  const sessions = useUi((s) => s.sessions.items);
  const recent = useMemo(() => sessions.slice(0, 4), [sessions]);
  const agent = bay?.agent ?? null;
  const id = bay?.id ?? 0;
  const pick = (prompt: string) => {
    setDraft(id, prompt);
    focusComposer(id);
  };
  const where = agent && !agent.deferred ? { lead: 'Working in', path: bay!.cwd } : folders && root ? { lead: 'New sessions get their own folder in', path: root } : null;
  return (
    <div className="chat fresh">
      <div className="hero">
        <div className="hero-mark">
          <img src="/logo-128.png" alt="" draggable={false} />
        </div>
        <h1>
          {greeting()}. <span className="grad">What shall we build?</span>
        </h1>
        <p>{agent?.incognito ? 'Incognito: this session won’t use or add to memory.' : 'Copilot can plan, write and run code, and remember what you teach it.'}</p>
        {where && (
          <button type="button" className="bay-crumb hero-folder" data-tip={`${where.path} · Change in Settings`} onClick={() => void setSide('settings')}>
            <Icon name="folder" />
            <span className="ellipsis">
              {where.lead} {shortPath(where.path, home)}
            </span>
          </button>
        )}
      </div>
      <Composer bay={bay} autoFocus />
      <div className="suggestions">
        {SUGGESTIONS.map((s) => (
          <button key={s.label} type="button" className="suggestion" onClick={() => pick(s.prompt)}>
            <Icon name={s.icon} /> {s.label}
          </button>
        ))}
      </div>
      {!bay && recent.length > 0 && (
        <div className="welcome-recent" style={{ alignSelf: 'center' }}>
          <div className="panel-section">
            <span className="section-label">Recent sessions</span>
            <button type="button" className="link-btn" style={{ fontSize: 12 }} onClick={() => void setSide('sessions')}>
              Show all
            </button>
          </div>
          {recent.map((s) => (
            <button
              key={s.id}
              type="button"
              className="list-row"
              onClick={() =>
                void act<number>({ type: 'resumeSession', id: s.id }).then((b) => {
                  if (b != null) focusComposer(b);
                })
              }
            >
              <Icon name="history" className="faint" />
              <span className="ellipsis grow">{s.title}</span>
              <span className="row-meta">{ago(s.modifiedTs)}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
