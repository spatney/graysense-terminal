import { useEffect, useRef, useState } from 'react';
import { act } from '../lib/api';
import { useUi } from '../lib/store';
import { McpPanel } from './panels/McpPanel';
import { MemoryPanel } from './panels/MemoryPanel';
import { MissionsPanel } from './panels/MissionsPanel';
import { PluginsPanel } from './panels/PluginsPanel';
import { SessionsPanel } from './panels/SessionsPanel';
import { SettingsPanel } from './panels/SettingsPanel';
import { Boundary } from './ui';

const MIN = 260;
const MAX = 620;

export function SidePanel() {
  const panel = useUi((s) => s.settings?.side_panel ?? 'hidden');
  const saved = useUi((s) => s.settings?.side_panel_width ?? 320);
  const [width, setWidth] = useState(saved);
  const [dragging, setDragging] = useState(false);
  const start = useRef<{ x: number; w: number } | null>(null);

  useEffect(() => {
    if (!dragging) setWidth(saved);
  }, [saved, dragging]);

  useEffect(() => {
    if (panel !== 'hidden') localStorage.setItem('gs-last-panel', panel);
  }, [panel]);

  if (panel === 'hidden') return null;

  const down = (e: React.PointerEvent) => {
    e.preventDefault();
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    start.current = { x: e.clientX, w: width };
    setDragging(true);
  };
  const move = (e: React.PointerEvent) => {
    if (!start.current) return;
    setWidth(Math.round(Math.min(MAX, Math.max(MIN, start.current.w + e.clientX - start.current.x))));
  };
  const up = () => {
    if (!start.current) return;
    start.current = null;
    setDragging(false);
    void act({ type: 'setSettings', changes: { side_panel_width: width } });
  };

  return (
    <aside className="side" style={{ width }} aria-label="Side panel">
      <Boundary key={panel} label="This panel">
        {panel === 'sessions' && <SessionsPanel />}
        {panel === 'missions' && <MissionsPanel />}
        {panel === 'memory' && <MemoryPanel />}
        {panel === 'mcp' && <McpPanel />}
        {panel === 'plugins' && <PluginsPanel />}
        {panel === 'settings' && <SettingsPanel />}
      </Boundary>
      <div
        className={`side-resize ${dragging ? 'dragging' : ''}`}
        onPointerDown={down}
        onPointerMove={move}
        onPointerUp={up}
        onPointerCancel={up}
        onDoubleClick={() => void act({ type: 'setSettings', changes: { side_panel_width: 320 } })}
      />
    </aside>
  );
}
