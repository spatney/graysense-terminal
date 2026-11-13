import { useEffect, useRef, useState } from 'react';
import { act, captureNote, newShell, setSide } from '../../lib/api';
import { setDock, useUi } from '../../lib/store';
import type { BayDto, ItemDto } from '../../lib/types';
import { statusClass } from '../Strip';
import { Icon, type MenuItem, copyText, showMenu } from '../ui';
import { Composer } from './Composer';
import { Dock } from './Dock';
import { Hero } from './Hero';
import { Transcript } from './Transcript';

const EMPTY: ItemDto[] = [];

function BayHead({ bay }: { bay: BayDto }) {
  const agent = bay.agent!;
  const [renaming, setRenaming] = useState<string | null>(null);
  const dockOpen = useUi((s) => s.dock[bay.id]?.open ?? false);
  const missions = useUi((s) => s.missions);
  const mission = agent.mission != null ? missions.find((m) => m.id === agent.mission) : null;
  const extras = agent.canvases.length + agent.apps.length;

  const save = () => {
    if (renaming != null && renaming.trim() !== bay.title) void act({ type: 'rename', bay: bay.id, name: renaming });
    setRenaming(null);
  };

  const folderMenu = (anchor: Element) => {
    const items: MenuItem[] = [
      { label: 'Reveal in File Explorer', icon: 'folder-opened', onSelect: () => void act({ type: 'revealPath', path: bay.cwd }) },
      { label: 'Open in VS Code', icon: 'vscode', onSelect: () => void act({ type: 'openInVsCode', path: bay.cwd }) },
      { label: 'New terminal here', icon: 'terminal', onSelect: () => void newShell(bay.cwd) },
      { label: 'New agent here', icon: 'add', onSelect: () => void act({ type: 'newAgent', dir: bay.cwd }) },
      { sep: true },
      { label: 'Copy path', icon: 'copy', onSelect: () => void copyText(bay.cwd, 'Path copied') },
    ];
    showMenu(anchor, items);
  };

  const moreMenu = (anchor: Element) => {
    const items: MenuItem[] = [
      { label: 'Rename', icon: 'edit', onSelect: () => setRenaming(bay.title) },
      { label: 'Save a note about this session', icon: 'bookmark', hint: 'Ctrl+Shift+S', onSelect: () => captureNote({ session: agent.sessionId, title: bay.title }) },
      { label: 'Learn from this conversation now', icon: 'lightbulb-sparkle', disabled: agent.incognito, onSelect: () => void act({ type: 'learnNow', bay: bay.id }) },
      { label: agent.autoApprove ? 'Ask before running tools' : 'Approve all tool calls', icon: 'shield', onSelect: () => void act({ type: 'setAutoApprove', bay: bay.id, on: !agent.autoApprove }) },
      { label: 'Clear transcript', icon: 'clear-all', onSelect: () => void act({ type: 'clearTranscript', bay: bay.id }) },
      { sep: true },
      { label: 'Maximize', icon: 'screen-full', hint: 'Ctrl+Shift+Enter', onSelect: () => void act({ type: 'maximizeBay', bay: bay.id }) },
      { label: 'Close', icon: 'close', hint: 'Ctrl+Shift+W', onSelect: () => void act({ type: 'closeBay', bay: bay.id }) },
    ];
    showMenu(anchor, items, { alignRight: true });
  };

  return (
    <div className="bay-head">
      <span className={`dot ${statusClass(bay)}`} data-tip={agent.status.detail ? `${agent.status.label}: ${agent.status.detail}` : agent.status.label} />
      {renaming != null ? (
        <input
          className="input bay-title-input"
          autoFocus
          value={renaming}
          onChange={(e) => setRenaming(e.target.value)}
          onBlur={save}
          onKeyDown={(e) => {
            if (e.key === 'Enter') save();
            if (e.key === 'Escape') setRenaming(null);
          }}
        />
      ) : (
        <span className="bay-title" data-tip="Double-click to rename" onDoubleClick={() => setRenaming(bay.title)}>
          {bay.title}
        </span>
      )}
      {agent.incognito && (
        <span className="badge" data-tip="Doesn't use or add to memory">
          Incognito
        </span>
      )}
      {!agent.deferred && (
        <button type="button" className="bay-crumb" data-tip={bay.cwd} onClick={(e) => folderMenu(e.currentTarget)}>
          <Icon name="folder" /> <span className="ellipsis">{bay.folder}</span>
        </button>
      )}
      {mission && (
        <button type="button" className="bay-crumb" data-tip={`Working on mission: ${mission.title}`} onClick={() => void setSide('missions')}>
          <Icon name="rocket" /> <span className="ellipsis">{mission.title}</span>
        </button>
      )}
      <div className="bay-head-actions">
        <button type="button" className={`icon-btn sm ${dockOpen ? 'active' : ''}`} data-tip={dockOpen ? 'Hide side panel' : 'Files, changes and canvases'} onClick={() => setDock(bay.id, { open: !dockOpen })}>
          <Icon name="layout-sidebar-right" />
          {extras > 0 && !dockOpen && <span className="count" style={{ position: 'absolute', transform: 'translate(9px, -8px)', minWidth: 14, height: 14, fontSize: 9 }}>{extras}</span>}
        </button>
        <button type="button" className="icon-btn sm" data-tip="More" onClick={(e) => moreMenu(e.currentTarget)}>
          <Icon name="ellipsis" />
        </button>
      </div>
    </div>
  );
}

export function AgentBay({ bay, visible }: { bay: BayDto; visible: boolean }) {
  const items = useUi((s) => s.transcripts[bay.id]) ?? EMPTY;
  const dockOpen = useUi((s) => s.dock[bay.id]?.open ?? false);
  const agent = bay.agent!;
  const extras = agent.canvases.length + agent.apps.length;
  const seen = useRef(extras);

  // A canvas or app the agent opens shows itself.
  useEffect(() => {
    if (extras > seen.current) {
      const last = agent.apps.length && extras - agent.canvases.length > (seen.current - agent.canvases.length) ? `app:${agent.apps[agent.apps.length - 1].view}` : agent.canvases.length ? `canvas:${agent.canvases[agent.canvases.length - 1].instanceId}` : 'files';
      setDock(bay.id, { open: true, tab: last });
    }
    seen.current = extras;
  }, [extras, agent.apps, agent.canvases, bay.id]);

  const empty = items.length === 0 && !agent.status.busy;
  return (
    <>
      <BayHead bay={bay} />
      <div className="agent">
        {empty ? (
          <Hero bay={bay} />
        ) : (
          <div className="chat">
            <Transcript bay={bay} items={items} visible={visible} />
            <Composer bay={bay} />
          </div>
        )}
        {dockOpen && <Dock bay={bay} />}
      </div>
    </>
  );
}
