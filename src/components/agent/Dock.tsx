import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useEffect, useMemo, useRef, useState } from 'react';
import { act, listDir, openLink, readText } from '../../lib/api';
import { ago, baseName, bytes } from '../../lib/format';
import { focusComposer, setDock, toast, useUi } from '../../lib/store';
import { cssVar } from '../../lib/theme';
import type { BayDto, CanvasDto, DirEntry } from '../../lib/types';
import { CodeBlock, Markdown } from '../Markdown';
import { Icon, type MenuItem, copyText, showMenu } from '../ui';

// ------------------------------------------------------------------ canvases

function TableCanvas({ c }: { c: CanvasDto }) {
  const cols: string[] = c.state?.columns ?? [];
  const rows: any[][] = c.state?.rows ?? [];
  const [sort, setSort] = useState<{ col: number; dir: 1 | -1 } | null>(null);
  const sorted = useMemo(() => {
    if (!sort) return rows;
    const cell = (r: any[]) => r[sort.col];
    return [...rows].sort((a, b) => {
      const x = cell(a);
      const y = cell(b);
      const nx = Number(x);
      const ny = Number(y);
      const cmp = !Number.isNaN(nx) && !Number.isNaN(ny) ? nx - ny : String(x ?? '').localeCompare(String(y ?? ''));
      return cmp * sort.dir;
    });
  }, [rows, sort]);
  return (
    <div className="cv-table">
      <table>
        <thead>
          <tr>
            {cols.map((h, i) => (
              <th key={i} onClick={() => setSort(sort?.col === i ? { col: i, dir: sort.dir === 1 ? -1 : 1 } : { col: i, dir: 1 })}>
                {h} {sort?.col === i && <Icon name={sort.dir === 1 ? 'arrow-up' : 'arrow-down'} />}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {sorted.map((r, i) => (
            <tr key={i}>
              {cols.map((_, j) => (
                <td key={j}>{r[j] == null ? '' : typeof r[j] === 'object' ? JSON.stringify(r[j]) : String(r[j])}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function ChecklistCanvas({ bay, c }: { bay: number; c: CanvasDto }) {
  const items: { text: string; done: boolean; note?: string }[] = c.state?.items ?? [];
  const done = items.filter((i) => i.done).length;
  return (
    <div>
      <div className="row faint" style={{ fontSize: 12, marginBottom: 4 }}>
        <span className="grow">
          {done} of {items.length} done
        </span>
      </div>
      <div className="cv-progress">
        <div style={{ width: `${items.length ? (done / items.length) * 100 : 0}%` }} />
      </div>
      {items.map((it, i) => (
        <button key={i} type="button" className={`cv-check ${it.done ? 'done' : ''}`} onClick={() => void act({ type: 'canvasEdit', bay, instance: c.instanceId, action: 'set_done', input: { index: i, done: !it.done } })}>
          <span className="cv-box">{it.done && <Icon name="check" />}</span>
          <span className="col" style={{ gap: 1 }}>
            <span className="cv-text">{it.text}</span>
            {it.note && <span className="faint" style={{ fontSize: 12 }}>{it.note}</span>}
          </span>
        </button>
      ))}
    </div>
  );
}

const SERIES = ['var(--accent)', 'var(--accent-2)', 'var(--ok)', 'var(--warn)', 'var(--danger)'];

function Chart({ chart }: { chart: any }) {
  const series: { name: string; values: number[] }[] = chart.series ?? [];
  const all = series.flatMap((s) => s.values);
  const max = Math.max(1e-9, ...all);
  const min = Math.min(0, ...all);
  const n = Math.max(1, ...series.map((s) => s.values.length));
  const W = 320;
  const H = 120;
  const y = (v: number) => H - ((v - min) / (max - min || 1)) * (H - 8) - 4;
  return (
    <div className="cv-chart">
      <h4>{chart.title}</h4>
      <svg viewBox={`0 0 ${W} ${H}`} width="100%" height={H} preserveAspectRatio="none">
        {chart.kind === 'bar'
          ? series.map((s, si) =>
              s.values.map((v, i) => {
                const bw = W / n / series.length;
                return <rect key={`${si}-${i}`} x={i * (W / n) + si * bw + 2} y={y(v)} width={Math.max(2, bw - 4)} height={H - y(v)} rx="2" fill={SERIES[si % SERIES.length]} opacity="0.85" />;
              }),
            )
          : series.map((s, si) => (
              <polyline key={si} fill="none" stroke={SERIES[si % SERIES.length]} strokeWidth="2" strokeLinejoin="round" points={s.values.map((v, i) => `${(i / Math.max(1, n - 1)) * W},${y(v)}`).join(' ')} vectorEffect="non-scaling-stroke" />
            ))}
      </svg>
      {series.length > 1 && (
        <div className="cv-legend">
          {series.map((s, i) => (
            <span key={s.name}>
              <i style={{ background: SERIES[i % SERIES.length] }} />
              {s.name}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

function DashboardCanvas({ c }: { c: CanvasDto }) {
  const metrics: any[] = c.state?.metrics ?? [];
  const charts: any[] = c.state?.charts ?? [];
  return (
    <div>
      {metrics.length > 0 && (
        <div className="cv-metrics">
          {metrics.map((m, i) => (
            <div key={i} className="cv-metric">
              <div className="cv-metric-label">{m.label}</div>
              <div className="cv-metric-value">{m.value}</div>
              {m.delta && <div className={`cv-metric-delta ${m.trend ?? ''}`}>{m.delta}</div>}
            </div>
          ))}
        </div>
      )}
      {charts.map((ch, i) => (
        <Chart key={i} chart={ch} />
      ))}
    </div>
  );
}

function CanvasView({ bay, c }: { bay: number; c: CanvasDto }) {
  return (
    <div className="dock-body">
      <div className="dock-head">
        <h3>{c.title}</h3>
        <button type="button" className="btn sm" data-tip="Tell the agent about your changes" onClick={() => void act({ type: 'canvasSend', bay, instance: c.instanceId })}>
          <Icon name="send" /> Send to agent
        </button>
        <button type="button" className="icon-btn sm" data-tip="Close canvas" onClick={() => void act({ type: 'closeCanvas', bay, instance: c.instanceId })}>
          <Icon name="close" />
        </button>
      </div>
      {c.canvasId === 'table' && <TableCanvas c={c} />}
      {c.canvasId === 'checklist' && <ChecklistCanvas bay={bay} c={c} />}
      {c.canvasId === 'dashboard' && <DashboardCanvas c={c} />}
      {c.canvasId === 'document' && <Markdown text={c.state?.markdown ?? ''} />}
    </div>
  );
}

// ------------------------------------------------------------------ MCP apps

interface AppViewData {
  id: number;
  bay: number;
  title: string;
  server: string;
  tool: string;
  toolCallId: string;
  html: string;
  meta: any;
  args: any;
  result: any;
}

function domains(list: unknown): string {
  return Array.isArray(list) ? list.filter((s): s is string => typeof s === 'string' && !/[;'"\s]/.test(s)).join(' ') : '';
}

/** The page an MCP App shipped, with a content security policy from its metadata. */
function appDocument(app: AppViewData): string {
  const csp = app.meta?.ui?.csp ?? {};
  const res = domains(csp.resourceDomains);
  const connect = domains(csp.connectDomains);
  const frames = domains(csp.frameDomains);
  const policy = [
    "default-src 'none'",
    `script-src 'unsafe-inline' 'unsafe-eval' blob: ${res}`,
    `style-src 'unsafe-inline' ${res}`,
    `img-src data: blob: ${res}`,
    `font-src data: ${res}`,
    `media-src data: blob: ${res}`,
    `connect-src ${connect || "'none'"}`,
    `frame-src ${frames || "'none'"}`,
    "base-uri 'none'",
    "form-action 'none'",
  ].join('; ');
  const meta = `<meta http-equiv="Content-Security-Policy" content="${policy.replace(/"/g, '&quot;')}">`;
  return /<head[^>]*>/i.test(app.html) ? app.html.replace(/<head[^>]*>/i, (m) => `${m}${meta}`) : `<!doctype html><html><head>${meta}</head><body>${app.html}</body></html>`;
}

function hostStyles(): Record<string, string> {
  return {
    '--color-background-primary': cssVar('--elev'),
    '--color-background-secondary': cssVar('--elev-2'),
    '--color-background-tertiary': cssVar('--elev-3'),
    '--color-text-primary': cssVar('--text'),
    '--color-text-secondary': cssVar('--text-dim'),
    '--color-text-tertiary': cssVar('--text-faint'),
    '--color-border-primary': cssVar('--border-strong'),
    '--color-border-secondary': cssVar('--border'),
    '--color-ring-primary': cssVar('--accent'),
    '--color-text-info': cssVar('--accent'),
    '--color-text-success': cssVar('--ok'),
    '--color-text-warning': cssVar('--warn'),
    '--color-text-danger': cssVar('--danger'),
    '--font-sans': "'Segoe UI Variable Text', 'Segoe UI', system-ui, sans-serif",
    '--font-mono': "'Cascadia Code', Consolas, monospace",
    '--border-radius-md': '8px',
  };
}

/**
 * An MCP App (SEP-1865) in a sandboxed frame: no same-origin access, its own CSP, and JSON-RPC
 * over postMessage. Tool calls and resource reads go to the agent's MCP server through Graysense.
 */
function AppFrame({ bay, view }: { bay: number; view: number }) {
  const [app, setApp] = useState<AppViewData | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const frame = useRef<HTMLIFrameElement>(null);
  const theme = useUi((s) => s.settings?.theme);
  const version = useUi((s) => s.app?.version ?? '0');

  useEffect(() => {
    invoke<AppViewData>('app_view', { view })
      .then(setApp)
      .catch((e) => setFailed(String(e)));
  }, [view]);

  const srcDoc = useMemo(() => (app ? appDocument(app) : ''), [app]);

  useEffect(() => {
    if (!app) return;
    const post = (msg: unknown) => frame.current?.contentWindow?.postMessage(msg, '*');
    const result = (id: unknown, r: unknown) => post({ jsonrpc: '2.0', id, result: r });
    const error = (id: unknown, message: string, code = -32601) => post({ jsonrpc: '2.0', id, error: { code, message } });
    const onMessage = (e: MessageEvent) => {
      if (e.source !== frame.current?.contentWindow) return;
      const m = e.data;
      if (!m || m.jsonrpc !== '2.0') return;
      const { id, method, params } = m;
      switch (method) {
        case 'ui/initialize':
          result(id, {
            protocolVersion: params?.protocolVersion ?? '2026-01-26',
            hostInfo: { name: 'Graysense', version },
            hostCapabilities: { openLinks: {}, serverTools: {}, serverResources: {}, logging: {}, message: { text: {} }, updateModelContext: { text: {} } },
            hostContext: {
              theme: document.documentElement.dataset.theme === 'paper' ? 'light' : 'dark',
              platform: 'desktop',
              displayMode: 'inline',
              availableDisplayModes: ['inline'],
              locale: navigator.language,
              userAgent: `Graysense/${version}`,
              toolInfo: { id: app.toolCallId, tool: { name: app.tool, inputSchema: { type: 'object' } } },
              styles: { variables: hostStyles() },
            },
          });
          break;
        case 'ui/notifications/initialized':
          post({ jsonrpc: '2.0', method: 'ui/notifications/tool-input', params: { arguments: app.args } });
          post({ jsonrpc: '2.0', method: 'ui/notifications/tool-result', params: app.result });
          break;
        case 'tools/call':
          void act({ type: 'appCallTool', view, reqId: id, tool: String(params?.name ?? ''), args: params?.arguments ?? null });
          break;
        case 'resources/read':
          void act({ type: 'appReadResource', view, reqId: id, uri: String(params?.uri ?? '') });
          break;
        case 'ui/message': {
          const blocks = Array.isArray(params?.content) ? params.content : params?.content ? [params.content] : [];
          const text = blocks
            .map((b: any) => b?.text)
            .filter(Boolean)
            .join('\n');
          if (text.trim()) void act({ type: 'appMessage', view, text });
          result(id, {});
          break;
        }
        case 'ui/open-link':
          if (typeof params?.url === 'string' && /^https?:\/\//.test(params.url)) {
            openLink(params.url);
            result(id, {});
          } else error(id, 'only http(s) links are allowed', -32000);
          break;
        case 'ui/request-display-mode':
          result(id, { mode: 'inline' });
          break;
        case 'ui/update-model-context':
        case 'ping':
          result(id, {});
          break;
        case 'notifications/message':
        case 'ui/notifications/size-changed':
        case 'ui/notifications/request-teardown':
          break;
        default:
          if (id !== undefined && method) error(id, `method not supported: ${method}`);
      }
    };
    window.addEventListener('message', onMessage);
    let unlisten: (() => void) | undefined;
    void listen<{ view: number; message: unknown }>('app-reply', (e) => {
      if (e.payload.view === view) post(e.payload.message);
    }).then((u) => (unlisten = u));
    return () => {
      window.removeEventListener('message', onMessage);
      unlisten?.();
      post({ jsonrpc: '2.0', id: 'teardown', method: 'ui/resource-teardown', params: {} });
    };
  }, [app, view, version]);

  useEffect(() => {
    // Tell the app when the theme changes.
    if (!theme || !app) return;
    requestAnimationFrame(() =>
      frame.current?.contentWindow?.postMessage(
        { jsonrpc: '2.0', method: 'ui/notifications/host-context-changed', params: { theme: theme === 'paper' ? 'light' : 'dark', styles: { variables: hostStyles() } } },
        '*',
      ),
    );
  }, [theme, app]);

  if (failed) return <div className="app-failed">This app couldn’t be shown: {failed}</div>;
  if (!app) return <div className="app-failed"><span className="spinner" /></div>;
  return (
    <div className="dock-body flush" style={{ flexDirection: 'column' }}>
      <div className="dock-head" style={{ padding: '8px 12px 0', marginBottom: 8 }}>
        <h3>{app.title}</h3>
        <button type="button" className="icon-btn sm" data-tip="Close app" onClick={() => void act({ type: 'closeApp', bay, view })}>
          <Icon name="close" />
        </button>
      </div>
      <iframe ref={frame} className="app-frame" title={app.title} sandbox="allow-scripts allow-forms" srcDoc={srcDoc} />
    </div>
  );
}

// ------------------------------------------------------------------ files

function attachToBay(bay: number, path: string) {
  void act({ type: 'attach', bay, paths: [path] }).then(() => {
    focusComposer(bay);
    toast('ok', `Attached ${baseName(path)}`);
  });
}

function fileMenu(e: React.MouseEvent, bay: number, path: string, isDir: boolean) {
  e.preventDefault();
  const items: MenuItem[] = [
    { label: 'Open', icon: 'go-to-file', onSelect: () => void act({ type: 'openPath', path }) },
    { label: 'Reveal in File Explorer', icon: 'folder-opened', onSelect: () => void act({ type: 'revealPath', path }) },
    { label: 'Open in VS Code', icon: 'vscode', onSelect: () => void act({ type: 'openInVsCode', path }) },
    { label: 'Attach to message', icon: 'attach', onSelect: () => attachToBay(bay, path) },
    { label: 'Copy path', icon: 'copy', onSelect: () => void copyText(path, 'Path copied') },
  ];
  if (isDir) items.splice(2, 0, { label: 'New terminal here', icon: 'terminal', onSelect: () => void act({ type: 'newShell', dir: path }) });
  showMenu(e, items);
}

function FilesView({ bay }: { bay: BayDto }) {
  const root = bay.cwd;
  const [dir, setDir] = useState(root);
  const [entries, setEntries] = useState<DirEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [preview, setPreview] = useState<{ path: string; text: string | null } | null>(null);
  const [tick, setTick] = useState(0);
  const busy = bay.agent?.status.busy;

  useEffect(() => setDir(root), [root]);
  useEffect(() => {
    let live = true;
    listDir(dir)
      .then((e) => {
        if (!live) return;
        setEntries(e);
        setError(null);
      })
      .catch((e) => live && setError(String(e)));
    return () => {
      live = false;
    };
  }, [dir, tick, busy]);

  const rel = dir.length > root.length ? dir.slice(root.length).split(/[\\/]/).filter(Boolean) : [];
  const show = async (path: string) => {
    const text = await readText(path).catch(() => null);
    setPreview({ path, text: text ?? null });
  };
  return (
    <div className="dock-body">
      <div className="files-crumbs">
        <button type="button" onClick={() => setDir(root)} onContextMenu={(ev) => fileMenu(ev, bay.id, root, true)}>
          <Icon name="root-folder" /> {baseName(root)}
        </button>
        {rel.map((part, i) => {
          const at = `${root}\\${rel.slice(0, i + 1).join('\\')}`;
          return (
            <span key={i} className="row" style={{ gap: 2 }}>
              <Icon name="chevron-right" className="faint" />
              <button type="button" onClick={() => setDir(at)} onContextMenu={(ev) => fileMenu(ev, bay.id, at, true)}>
                {part}
              </button>
            </span>
          );
        })}
        <span className="grow" />
        <button type="button" data-tip="Open this folder in VS Code" aria-label="Open this folder in VS Code" onClick={() => void act({ type: 'openInVsCode', path: dir })}>
          <Icon name="vscode" />
        </button>
        <button type="button" data-tip="Refresh" onClick={() => setTick((t) => t + 1)}>
          <Icon name="refresh" />
        </button>
      </div>
      {error && <div className="faint">{error}</div>}
      {entries?.length === 0 && <div className="empty"><Icon name="folder" /><p>This folder is empty. Files the agent makes show up here.</p></div>}
      {entries?.map((e) => (
        <button
          key={e.path}
          type="button"
          className={`file-row ${preview?.path === e.path ? 'on' : ''}`}
          onClick={() => (e.isDir ? setDir(e.path) : void show(e.path))}
          onDoubleClick={() => !e.isDir && void act({ type: 'openPath', path: e.path })}
          onContextMenu={(ev) => fileMenu(ev, bay.id, e.path, e.isDir)}
        >
          <Icon name={e.isDir ? 'folder' : 'file'} />
          <span className="ellipsis">{e.name}</span>
          <span className="meta">{e.isDir ? '' : `${bytes(e.size)} · `}{ago(e.modified)}</span>
        </button>
      ))}
      {preview && (
        <div className="file-preview">
          <div className="dock-head">
            <h3>{baseName(preview.path)}</h3>
            <button type="button" className="icon-btn sm" data-tip="Attach to message" onClick={() => attachToBay(bay.id, preview.path)}>
              <Icon name="attach" />
            </button>
            <button type="button" className="icon-btn sm" data-tip="Open" onClick={() => void act({ type: 'openPath', path: preview.path })}>
              <Icon name="go-to-file" />
            </button>
            <button type="button" className="icon-btn sm" aria-label="Close preview" onClick={() => setPreview(null)}>
              <Icon name="close" />
            </button>
          </div>
          {preview.text == null ? <div className="faint">No preview for this file.</div> : /\.md$/i.test(preview.path) ? <Markdown text={preview.text} /> : <CodeBlock code={preview.text} lang={preview.path.split('.').pop()} maxHeight={420} />}
        </div>
      )}
    </div>
  );
}

function ChangesView({ bay }: { bay: BayDto }) {
  const files = bay.agent?.changedFiles ?? [];
  if (!files.length) return <div className="empty"><Icon name="git-compare" /><h4>No changes yet</h4><p>Files the agent edits or creates in this session are listed here.</p></div>;
  // Where each file is, from the session's folder (files outside it keep their full path).
  const root = bay.cwd.replace(/[\\/]+$/, '');
  const where = (f: string) => {
    const dir = f.slice(0, f.length - baseName(f).length).replace(/[\\/]+$/, '');
    if (dir.toLowerCase() === root.toLowerCase()) return '';
    return dir.toLowerCase().startsWith(`${root.toLowerCase()}\\`) || dir.toLowerCase().startsWith(`${root.toLowerCase()}/`) ? dir.slice(root.length + 1) : dir;
  };
  return (
    <div className="dock-body">
      {files.map((f) => (
        <button key={f} type="button" className="file-row" onClick={() => void act({ type: 'openPath', path: f })} onContextMenu={(e) => fileMenu(e, bay.id, f, false)} data-tip={f}>
          <Icon name="diff" />
          <span className="ellipsis">{baseName(f)}</span>
          <span className="meta ellipsis" style={{ maxWidth: '50%' }}>{where(f)}</span>
        </button>
      ))}
    </div>
  );
}

// ------------------------------------------------------------------ dock

export function Dock({ bay }: { bay: BayDto }) {
  const agent = bay.agent!;
  const dock = useUi((s) => s.dock[bay.id]) ?? { open: true, tab: 'files', width: 420 };
  const [width, setWidth] = useState(dock.width);
  const start = useRef<{ x: number; w: number } | null>(null);
  const tabs = [
    ...agent.canvases.map((c) => ({ id: `canvas:${c.instanceId}`, icon: c.canvasId === 'table' ? 'table' : c.canvasId === 'checklist' ? 'checklist' : c.canvasId === 'dashboard' ? 'dashboard' : 'book', label: c.title })),
    ...agent.apps.map((a) => ({ id: `app:${a.view}`, icon: 'window', label: a.title })),
    { id: 'changes', icon: 'git-compare', label: agent.changedFiles.length ? `Changes ${agent.changedFiles.length}` : 'Changes' },
    { id: 'files', icon: 'files', label: 'Files' },
  ];
  const tab = tabs.some((t) => t.id === dock.tab) ? dock.tab : tabs[0].id;
  const canvas = tab.startsWith('canvas:') ? agent.canvases.find((c) => `canvas:${c.instanceId}` === tab) : null;
  const app = tab.startsWith('app:') ? Number(tab.slice(4)) : null;
  return (
    <aside className="dock" style={{ width }}>
      <div
        className="dock-resize"
        onPointerDown={(e) => {
          (e.target as HTMLElement).setPointerCapture(e.pointerId);
          start.current = { x: e.clientX, w: width };
        }}
        onPointerMove={(e) => start.current && setWidth(Math.min(900, Math.max(280, start.current.w - (e.clientX - start.current.x))))}
        onPointerUp={() => {
          start.current = null;
          setDock(bay.id, { width });
        }}
      />
      <div className="dock-tabs" role="tablist">
        {tabs.map((t) => (
          <button key={t.id} type="button" role="tab" aria-selected={t.id === tab} className={`dock-tab ${t.id === tab ? 'on' : ''}`} data-tip={t.label} onClick={() => setDock(bay.id, { tab: t.id })}>
            <Icon name={t.icon} />
            <span className="ellipsis">{t.label}</span>
          </button>
        ))}
        <span className="grow" />
        <button type="button" className="icon-btn sm" data-tip="Hide panel" onClick={() => setDock(bay.id, { open: false })}>
          <Icon name="layout-sidebar-right-off" />
        </button>
      </div>
      {canvas && <CanvasView bay={bay.id} c={canvas} />}
      {app != null && <AppFrame key={app} bay={bay.id} view={app} />}
      {tab === 'changes' && <ChangesView bay={bay} />}
      {tab === 'files' && <FilesView bay={bay} />}
    </aside>
  );
}
