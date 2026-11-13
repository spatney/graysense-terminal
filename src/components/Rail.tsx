import { newSession, toggleSide } from '../lib/api';
import { openOverlay, useUi } from '../lib/store';
import type { PanelId } from '../lib/types';
import { Icon } from './ui';

const ITEMS: { id: PanelId; icon: string; label: string; keys?: string }[] = [
  { id: 'sessions', icon: 'comment-discussion', label: 'Sessions', keys: 'Ctrl+Shift+H' },
  { id: 'missions', icon: 'rocket', label: 'Missions', keys: 'Ctrl+Shift+I' },
  { id: 'memory', icon: 'lightbulb-sparkle', label: 'Memory', keys: 'Ctrl+Shift+M' },
  { id: 'mcp', icon: 'server-process', label: 'MCP servers' },
  { id: 'plugins', icon: 'extensions', label: 'Plugins' },
];

export function Rail() {
  const open = useUi((s) => s.settings?.side_panel);
  const attention = useUi((s) => s.bays.filter((b) => b.agent?.status.attention).length);
  const working = useUi((s) => s.bays.some((b) => b.agent?.status.busy));
  const missions = useUi((s) => s.missions.filter((m) => m.status === 'review').length);
  const review = useUi((s) => s.memory.memories.filter((m) => m.kind !== 'note' && (m.state === 'pending' || m.proposal != null)).length);
  const updateReady = useUi((s) => s.update?.phase === 'ready');
  const badge = (id: PanelId) => {
    if (id === 'sessions' && attention > 0) return <span className="count">{attention}</span>;
    if (id === 'sessions' && working) return <span className="rail-pip" />;
    if (id === 'missions' && missions > 0) return <span className="count">{missions}</span>;
    if (id === 'memory' && review > 0) return <span className="count">{review}</span>;
    return null;
  };
  return (
    <nav className="rail" aria-label="Panels">
      <button type="button" className="rail-logo" data-tip="New session" data-keys="Ctrl+Shift+N" onClick={() => void newSession()}>
        <img src="/logo-64.png" alt="Graysense" draggable={false} />
      </button>
      {ITEMS.map((it) => (
        <button key={it.id} type="button" className={`rail-btn ${open === it.id ? 'on' : ''}`} data-tip={it.label} data-keys={it.keys} aria-pressed={open === it.id} onClick={() => void toggleSide(it.id)}>
          <Icon name={it.icon} />
          {badge(it.id)}
        </button>
      ))}
      <div className="rail-spacer" />
      <button type="button" className="rail-btn" data-tip="Search sessions and files" data-keys="Ctrl+K" onClick={() => openOverlay({ kind: 'search' })}>
        <Icon name="search" />
      </button>
      <button type="button" className={`rail-btn ${open === 'settings' ? 'on' : ''}`} data-tip={updateReady ? 'Settings · update ready' : 'Settings'} data-keys="Ctrl+," onClick={() => void toggleSide('settings')}>
        <Icon name="settings-gear" />
        {updateReady && <span className="rail-pip" />}
      </button>
    </nav>
  );
}
