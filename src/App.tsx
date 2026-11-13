import { invoke } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { useEffect, useRef } from 'react';
import { Overlays } from './components/Overlays';
import { MissionDragGhost } from './components/panels/MissionsPanel';
import { Rail } from './components/Rail';
import { SidePanel } from './components/SidePanel';
import { StatusBar } from './components/StatusBar';
import { Strip } from './components/Strip';
import { Toasts } from './components/Toasts';
import { Boundary, MenuHost, Tooltips } from './components/ui';
import { Workspace } from './components/Workspace';
import { useFileDrop } from './lib/drop';
import { useShortcuts } from './lib/shortcuts';
import { useUi } from './lib/store';
import { applyTheme, cssVar } from './lib/theme';

export function App() {
  const ready = useUi((s) => s.ready);
  const theme = useUi((s) => s.settings?.theme);
  const zoom = useUi((s) => s.settings?.zoom);
  const revealed = useRef(false);

  useEffect(() => {
    if (!theme) return;
    applyTheme(theme);
    localStorage.setItem('gs-theme', theme);
  }, [theme]);

  useEffect(() => {
    if (zoom) void getCurrentWebview().setZoom(zoom).catch(() => {});
  }, [zoom]);

  useEffect(() => {
    if (!ready || revealed.current) return;
    revealed.current = true;
    // Show the window once the first real frame is on screen.
    requestAnimationFrame(() => requestAnimationFrame(() => void invoke('window_reveal', { background: cssVar('--bg') }).catch(() => {})));
  }, [ready]);

  useShortcuts();
  useFileDrop();

  if (!ready) {
    return (
      <div className="app boot">
        <div className="spinner" />
      </div>
    );
  }
  return (
    <div className="app">
      <Rail />
      <SidePanel />
      <main className="main">
        <Strip />
        <Workspace />
      </main>
      <StatusBar />
      <Boundary label="This window">
        <Overlays />
      </Boundary>
      <Toasts />
      <Tooltips />
      <MenuHost />
      <MissionDragGhost />
    </div>
  );
}
