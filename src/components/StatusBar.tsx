import { act, restartToUpdate, setSide } from '../lib/api';
import { agoLong, bytes, human, modelLabel } from '../lib/format';
import { quotaStatus } from '../lib/quota';
import { fileManager } from '../lib/platform';
import { focusComposer, openOverlay, useUi } from '../lib/store';
import type { QuotaDto } from '../lib/types';
import { showSyncSettings } from './panels/SyncCard';
import { Icon } from './ui';

function Spark({ values }: { values: number[] }) {
  if (values.length < 2) return null;
  const w = 36;
  const h = 12;
  const pts = values.map((v, i) => `${(i / (values.length - 1)) * w},${h - (Math.min(100, v) / 100) * (h - 1) - 0.5}`).join(' ');
  return (
    <svg className="spark" width={w} height={h} viewBox={`0 0 ${w} ${h}`} aria-hidden>
      <polyline points={pts} fill="none" stroke="var(--accent)" strokeWidth="1.2" strokeLinejoin="round" strokeLinecap="round" />
    </svg>
  );
}

function Quota({ quota, cost }: { quota: QuotaDto; cost?: number | null }) {
  const s = quotaStatus(quota, cost);
  return (
    <span className={`status-item ${s.low ? 'warn' : ''}`} data-tip={s.tip}>
      <Icon name="dashboard" /> {s.text}
    </span>
  );
}

export function StatusBar() {
  const app = useUi((s) => s.app);
  const settings = useUi((s) => s.settings);
  const bays = useUi((s) => s.bays);
  const update = useUi((s) => s.update);
  const stats = useUi((s) => s.stats);
  const cpu = useUi((s) => s.cpuHistory);
  const learning = useUi((s) => s.memory.learning);
  const sync = useUi((s) => s.sync);
  if (!app || !settings) return null;
  const focused = bays.find((b) => b.id === app.focused);
  const working = bays.filter((b) => b.agent?.status.busy);
  const waiting = bays.filter((b) => b.agent?.status.attention);
  const agent = focused?.agent;

  const client =
    app.client === 'ready' ? (
      <span className="status-item" data-tip="The Copilot runtime is running">
        <Icon name="copilot" /> Copilot
      </span>
    ) : app.client === 'failed' ? (
      <span className="status-item danger" data-tip={app.clientError ?? 'The Copilot runtime failed to start'}>
        <Icon name="error" /> Copilot unavailable
      </span>
    ) : (
      <span className="status-item">
        <span className="spinner sm" /> Starting Copilot…
      </span>
    );

  return (
    <footer className="status">
      {client}
      {waiting.length > 0 ? (
        <button
          type="button"
          className="status-item warn"
          data-tip="Show the agent that needs you"
          onClick={() => {
            void act({ type: 'focusBay', bay: waiting[0].id });
            focusComposer(waiting[0].id);
          }}
        >
          <span className="dot attention" /> {waiting.length === 1 ? `${waiting[0].title} needs you` : `${waiting.length} agents need you`}
        </button>
      ) : working.length > 0 ? (
        <span className="status-item accent">
          <span className="dot busy" /> {working.length === 1 ? '1 agent working' : `${working.length} agents working`}
        </span>
      ) : null}
      {focused && (
        <button type="button" className="status-item" data-tip={`${focused.cwd} · reveal in ${fileManager}`} onClick={() => void act({ type: 'revealPath', path: focused.cwd })}>
          <Icon name="folder" /> {focused.folder}
        </button>
      )}
      {agent && !agent.deferred && (
        <>
          <span className="status-item" data-tip="Model">
            <Icon name="sparkle" /> {modelLabel(agent.model, app.models)}
          </span>
          {(agent.tokensIn > 0 || agent.tokensOut > 0) && (
            <span className="status-item" data-tip="Tokens in · out this session">
              <Icon name="arrow-swap" /> {human(agent.tokensIn)} · {human(agent.tokensOut)}
            </span>
          )}
          {agent.context && agent.context[1] > 0 && (
            <span className="status-item" data-tip={`Context window: ${human(agent.context[0])} of ${human(agent.context[1])} tokens`}>
              <Icon name="layers" /> {Math.round((agent.context[0] / agent.context[1]) * 100)}%
            </span>
          )}
        </>
      )}
      <span className="status-spacer" />
      {app.quota && <Quota quota={app.quota} cost={agent?.premiumCost} />}
      {sync?.repo && (
        <button
          type="button"
          className={`status-item ${sync.lastError || sync.pending ? 'warn' : ''}`}
          data-tip={sync.pending ? 'GitHub sync is waiting for you' : (sync.lastError ?? (sync.busy ? 'Syncing with GitHub…' : `Synced with ${sync.repo}${sync.lastSync ? `. Last sync ${agoLong(sync.lastSync)}` : ''}`))}
          onClick={() => void showSyncSettings()}
        >
          {sync.busy ? <span className="spinner sm" /> : <Icon name={sync.lastError || sync.pending ? 'warning' : 'cloud'} />}
          {sync.busy ? <span className="shimmer">Syncing…</span> : sync.pending ? 'Sync needs you' : sync.lastError ? 'Sync failed' : sync.changes ? 'Sync pending' : 'Synced'}
        </button>
      )}
      <button type="button" className={`status-item ${settings['memory.enabled'] ? '' : 'warn'}`} data-tip={settings['memory.enabled'] ? 'Memory is on' : 'Memory is off'} onClick={() => void setSide('memory')}>
        <Icon name="lightbulb-sparkle" />
        {learning > 0 ? <span className="shimmer">Learning…</span> : settings['memory.enabled'] ? 'Memory' : 'Memory off'}
      </button>
      {update?.phase === 'ready' && (
        <button type="button" className="status-item accent" data-tip="Restart to install the update" onClick={() => void restartToUpdate()}>
          <Icon name="cloud-download" /> Restart to update
        </button>
      )}
      {update?.phase === 'downloading' && (
        <button type="button" className="status-item" onClick={() => void setSide('settings')}>
          <Icon name="cloud-download" /> Updating {Math.round((update.progress ?? 0) * 100)}%
        </button>
      )}
      {settings.performance_hud && stats && (
        <span className="status-item" data-tip={`CPU ${stats.cpu.toFixed(0)}% · memory ${bytes(stats.memUsed)} of ${bytes(stats.memTotal)} · Graysense ${bytes(stats.appMem)}`}>
          <Spark values={cpu} /> {stats.cpu.toFixed(0)}% · {bytes(stats.appMem)}
        </span>
      )}
      <button type="button" className="status-item" data-tip="Keyboard shortcuts" data-keys="Ctrl+/" onClick={() => openOverlay({ kind: 'shortcuts' })}>
        <Icon name="keyboard" />
      </button>
      <button
        type="button"
        className="status-item"
        data-tip="About Graysense"
        onClick={() =>
          void setSide('settings').then(() => window.setTimeout(() => document.getElementById('settings-updates')?.scrollIntoView({ behavior: 'smooth', block: 'start' }), 90))
        }
      >
        v{app.version}
      </button>
    </footer>
  );
}
