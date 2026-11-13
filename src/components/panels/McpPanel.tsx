import { type ReactNode, useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { act, pickFolder, setSide } from '../../lib/api';
import { plural } from '../../lib/format';
import { dismissToast, type LocalToast, openOverlay, useUi } from '../../lib/store';
import type { Action, McpServerDto, McpToolDto, Result } from '../../lib/types';
import { copyText, Icon, type MenuItem, Modal, Seg, showMenu, Toggle, useAutosize } from '../ui';
import '../../styles/panels/mcp.css';

type Json = Record<string, unknown>;
type Ui = ReturnType<typeof useUi.getState>;
type Kind = 'local' | 'http' | 'sse';
type Tone = 'ok' | 'busy' | 'warn' | 'danger' | 'off' | 'unknown';
type Op = 'restart' | 'stop' | 'start';

const isObj = (v: unknown): v is Json => typeof v === 'object' && v !== null && !Array.isArray(v);
const strList = (v: unknown): string[] => (Array.isArray(v) ? v.map((x) => (typeof x === 'string' ? x : JSON.stringify(x))) : []);
const pretty = (v: unknown) => JSON.stringify(v, null, 2);

function humanize(s: string): string {
  const t = s.replace(/[-_]+/g, ' ').trim();
  return t ? t[0].toUpperCase() + t.slice(1) : s;
}

const SOURCES: Record<string, { label: string; tip: string }> = {
  user: { label: 'User', tip: 'In your Copilot config, for every folder' },
  workspace: { label: 'Workspace', tip: "From the open folder's MCP config" },
  plugin: { label: 'Plugins', tip: 'Come with plugins you installed' },
  account: { label: 'Account', tip: 'Come with your GitHub account' },
  managed: { label: 'Managed', tip: 'Set up by your organization' },
  builtin: { label: 'Built in', tip: 'Come with Copilot' },
  session: { label: 'Session only', tip: "Running in the agent's session, not in a config" },
};
const ORDER = Object.keys(SOURCES);
const rank = (src: string) => (ORDER.includes(src) ? ORDER.indexOf(src) : ORDER.length);
const sourceLabel = (src: string) => SOURCES[src]?.label ?? humanize(src);

const KIND_TIP: Record<Kind, string> = {
  local: 'Runs a command on this computer',
  http: 'Remote, over HTTP',
  sse: 'Remote, over server-sent events',
};

const DOT: Record<Tone, string> = { ok: 'idle', busy: 'busy', warn: 'mcp-dot-warn', danger: 'error', off: 'offline', unknown: 'mcp-dot-unknown' };

const OPS: Record<Op, { label: string; icon: string; busy: string; action: (server: string) => Action }> = {
  restart: { label: 'Restart', icon: 'debug-restart', busy: 'Restarting…', action: (server) => ({ type: 'mcpRestart', server }) },
  stop: { label: 'Stop', icon: 'debug-stop', busy: 'Stopping…', action: (server) => ({ type: 'mcpStop', server }) },
  start: { label: 'Start', icon: 'debug-start', busy: 'Starting…', action: (server) => ({ type: 'mcpStart', server }) },
};
const REMOVING = 'Removing…';
const TOOLS_SHOWN = 8;

// ------------------------------------------------------------------ status

/** How a server looks in its row: the dot's tone and a few words. */
function look(s: McpServerDto, live: boolean, enabled: boolean, busy?: string): { tone: Tone; text: string | null } {
  if (busy) return { tone: 'busy', text: busy };
  const status = live ? s.status : null;
  const error = live ? s.error : null;
  if (!enabled) return { tone: 'off', text: status === 'connected' ? 'Off for new sessions' : 'Off' };
  switch (status) {
    case null:
      return { tone: 'unknown', text: live ? 'Starts with new sessions' : null };
    case 'connected':
      return { tone: 'ok', text: 'Connected' };
    case 'pending':
      return { tone: 'busy', text: 'Connecting…' };
    case 'failed':
      return { tone: 'danger', text: error || "Couldn't start" };
    case 'needs-auth':
      return { tone: 'warn', text: 'Needs sign-in' };
    case 'stopped':
      return { tone: 'off', text: 'Stopped' };
    case 'disabled':
      return { tone: 'off', text: 'Off in this session' };
    case 'not_configured':
      return { tone: 'off', text: 'Not in this session' };
    default:
      return error ? { tone: 'danger', text: error } : { tone: 'off', text: humanize(status) };
  }
}

function kindOf(cfg: unknown): Kind | null {
  if (!isObj(cfg)) return null;
  const t = typeof cfg.type === 'string' ? cfg.type.toLowerCase() : '';
  if (t === 'http' || t === 'sse') return t;
  if (t === 'local' || t === 'stdio') return 'local';
  if (t) return null;
  if (typeof cfg.command === 'string') return 'local';
  if (typeof cfg.url === 'string') return 'http';
  return null;
}

/** The command line a local server runs, or a remote one's URL. */
function summaryOf(cfg: unknown): string | null {
  const kind = kindOf(cfg);
  if (!isObj(cfg) || !kind) return null;
  if (kind !== 'local') return typeof cfg.url === 'string' ? cfg.url : null;
  const argv = typeof cfg.command === 'string' ? [cfg.command, ...strList(cfg.args)] : strList(cfg.args);
  return argv.length ? (joinLine(argv) ?? argv.join(' ')) : null;
}

function opsFor(status: string | null): Op[] {
  if (status === 'connected' || status === 'pending') return ['restart', 'stop'];
  if (status === 'failed' || status === 'needs-auth') return ['restart'];
  return ['start'];
}

/** Only servers in the user's own Copilot config can be edited or removed. */
const canEdit = (s: McpServerDto) => isObj(s.config) && !s.plugin;

function lockedWhy(s: McpServerDto): string | null {
  if (canEdit(s)) return null;
  if (s.plugin) return `It comes with the ${s.plugin} plugin, so you can't edit or remove it here.`;
  switch (s.source) {
    case 'workspace':
      return "It's set up in this folder's MCP config. Edit that file to change it.";
    case 'builtin':
      return 'It comes with Copilot.';
    case 'managed':
      return 'Your organization manages it.';
    case 'account':
      return 'It comes with your GitHub account.';
    case 'session':
      return "It's running in the agent's session only, so there's no config to edit.";
    default:
      return "It isn't in your Copilot config, so you can't edit it here.";
  }
}

// ------------------------------------------------------------------ helpers

/**
 * Follow an action the core runs in the background: it toasts the outcome, then refreshes.
 * Resolves with that toast once `settled` sees the refresh (at once for an error), or with
 * whatever it heard after `ms`.
 */
function follow(settled: (s: Ui, prev: Ui) => boolean, ms: number) {
  let finish: (t: LocalToast | null) => void = () => {};
  const promise = new Promise<LocalToast | null>((resolve) => {
    let heard: LocalToast | null = null;
    const unsub = useUi.subscribe((s, prev) => {
      if (!heard && s.toasts !== prev.toasts) {
        const old = new Set(prev.toasts.map((t) => t.id));
        heard = s.toasts.find((t) => t.id > 0 && !old.has(t.id)) ?? null;
        if (heard?.level === 'error') return finish(heard);
      }
      if (heard && settled(s, prev)) finish(heard);
    });
    const timer = window.setTimeout(() => finish(heard), ms);
    finish = (t) => {
      window.clearTimeout(timer);
      unsub();
      resolve(t);
    };
  });
  return { promise, cancel: () => finish(null) };
}

const mcpSettled = (s: Ui, prev: Ui) => s.mcp !== prev.mcp && !!s.mcp && !s.mcp.loading;

function omit<T>(rec: Record<string, T>, key: string): Record<string, T> {
  if (!(key in rec)) return rec;
  const next = { ...rec };
  delete next[key];
  return next;
}

function toggled(set: ReadonlySet<string>, key: string): ReadonlySet<string> {
  const next = new Set(set);
  if (!next.delete(key)) next.add(key);
  return next;
}

function without(set: ReadonlySet<string>, key: string): ReadonlySet<string> {
  if (!set.has(key)) return set;
  const next = new Set(set);
  next.delete(key);
  return next;
}

function useAlive() {
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  return alive;
}

/** A clickable block that isn't a `<button>` (so text inside it can clamp). */
function pressable(onPress: () => void) {
  return {
    role: 'button',
    tabIndex: 0,
    onClick: onPress,
    onKeyDown: (e: React.KeyboardEvent) => {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        onPress();
      }
    },
  };
}

// ------------------------------------------------------------------ command lines and config

/** Split a command line into words. Quotes group words; backslashes stay as typed, for Windows paths. */
function splitLine(line: string): { words: string[]; open: boolean } {
  const words: string[] = [];
  let word = '';
  let quote = '';
  let started = false;
  for (const c of line) {
    if (quote) {
      if (c === quote) quote = '';
      else word += c;
    } else if (c === '"' || c === "'") {
      quote = c;
      started = true;
    } else if (/\s/.test(c)) {
      if (started) words.push(word);
      word = '';
      started = false;
    } else {
      word += c;
      started = true;
    }
  }
  if (started) words.push(word);
  return { words, open: quote !== '' };
}

/** Words as one line that `splitLine` reads back, or null if a word has both kinds of quote. */
function joinLine(words: string[]): string | null {
  const out: string[] = [];
  for (const w of words) {
    if (w !== '' && !/[\s"']/.test(w)) out.push(w);
    else if (!w.includes('"')) out.push(`"${w}"`);
    else if (!w.includes("'")) out.push(`'${w}'`);
    else return null;
  }
  return out.join(' ');
}

/** `KEY=value` (or `Name: value`) lines, and the number of the first line that isn't one. */
function readPairs(text: string, sep: '=' | ':'): { map: Record<string, string>; bad: number } {
  const map: Record<string, string> = {};
  let bad = 0;
  text.split(/\r?\n/).forEach((raw, i) => {
    const line = raw.trim();
    if (!line || line.startsWith('#')) return;
    const at = line.indexOf(sep);
    const key = at > 0 ? line.slice(0, at).trim() : '';
    if (key) map[key] = line.slice(at + 1).trim();
    else if (!bad) bad = i + 1;
  });
  return { map, bad };
}

const writePairs = (v: unknown, sep: string) =>
  isObj(v)
    ? Object.entries(v)
        .map(([k, x]) => `${k}${sep}${typeof x === 'string' ? x : JSON.stringify(x)}`)
        .join('\n')
    : '';

interface Draft {
  name: string;
  kind: 'local' | 'remote';
  line: string;
  env: string;
  cwd: string;
  url: string;
  sse: boolean;
  /** SSE was chosen (or loaded), so typing the URL no longer decides it. */
  sseSet: boolean;
  headers: string;
  tools: string;
  timeout: string;
}

/** Config keys the form edits. Anything else in a server's config is kept as it is. */
const FORM_KEYS = new Set(['type', 'command', 'args', 'env', 'cwd', 'url', 'headers', 'tools', 'timeout']);
/** What the runtime records about a config as it loads it; not settings. */
const META_KEYS = new Set(['source', 'sourcePath', 'sourcePlugin', 'sourcePluginSpec', 'sourcePluginVersion', 'configWarnings']);

const clean = (cfg: unknown): Json => (isObj(cfg) ? Object.fromEntries(Object.entries(cfg).filter(([k]) => !META_KEYS.has(k))) : {});

/** Values the line format can hold: strings without line breaks. */
const linesOk = (v: unknown) => v === undefined || (isObj(v) && Object.values(v).every((x) => typeof x === 'string' && !/[\r\n]/.test(x)));

/** The form for a config, and whether the form can show all of it (otherwise it's edited as JSON). */
function draftOf(name: string, cfg: Json): { draft: Draft; formOk: boolean } {
  const kind = kindOf(cfg);
  const argv = typeof cfg.command === 'string' ? [cfg.command, ...strList(cfg.args)] : strList(cfg.args);
  const line = argv.length ? joinLine(argv) : '';
  return {
    draft: {
      name,
      kind: kind === 'http' || kind === 'sse' ? 'remote' : 'local',
      line: line ?? argv.join(' '),
      env: writePairs(cfg.env, '='),
      cwd: typeof cfg.cwd === 'string' ? cfg.cwd : '',
      url: typeof cfg.url === 'string' ? cfg.url : '',
      sse: kind === 'sse',
      sseSet: kind === 'http' || kind === 'sse',
      headers: writePairs(cfg.headers, ': '),
      tools: strList(cfg.tools)
        .filter((t) => t !== '*')
        .join(', '),
      timeout: typeof cfg.timeout === 'number' ? String(cfg.timeout) : '',
    },
    formOk:
      line !== null &&
      linesOk(cfg.env) &&
      linesOk(cfg.headers) &&
      (cfg.type === undefined || kind !== null) &&
      (cfg.args === undefined || Array.isArray(cfg.args)) &&
      (cfg.tools === undefined || Array.isArray(cfg.tools)) &&
      (cfg.timeout === undefined || typeof cfg.timeout === 'number'),
  };
}

/** The config to save: the form's fields, then everything else the server had. */
function buildConfig(d: Draft, base: Json): Json {
  const cfg: Json = {};
  if (d.kind === 'local') {
    const [command = '', ...args] = splitLine(d.line).words;
    cfg.type = base.type === 'stdio' ? 'stdio' : 'local';
    cfg.command = command;
    cfg.args = args;
    const env = readPairs(d.env, '=').map;
    if (Object.keys(env).length) cfg.env = env;
    if (d.cwd.trim()) cfg.cwd = d.cwd.trim();
  } else {
    cfg.type = d.sse ? 'sse' : 'http';
    cfg.url = d.url.trim();
    const headers = readPairs(d.headers, ':').map;
    if (Object.keys(headers).length) cfg.headers = headers;
  }
  const tools = d.tools
    .split(',')
    .map((t) => t.trim())
    .filter(Boolean);
  cfg.tools = tools.length ? tools : ['*'];
  if (/^\d+$/.test(d.timeout.trim())) cfg.timeout = Number(d.timeout.trim());
  for (const [k, v] of Object.entries(base)) if (!FORM_KEYS.has(k) && !META_KEYS.has(k)) cfg[k] = v;
  return cfg;
}

function isHttpUrl(s: string): boolean {
  try {
    const u = new URL(s);
    return u.protocol === 'https:' || u.protocol === 'http:';
  } catch {
    return false;
  }
}

type Problems = Partial<Record<'name' | 'line' | 'env' | 'url' | 'headers' | 'timeout', string>>;

function nameProblem(name: string, taken: ReadonlySet<string>, original: string | null): string | undefined {
  if (!name) return 'Give the server a name.';
  if (/\s/.test(name)) return 'Use a name without spaces, like my-server.';
  if (name !== original && taken.has(name)) return `There's already a server named ${name}.`;
  return undefined;
}

function problems(d: Draft, taken: ReadonlySet<string>, original: string | null): Problems {
  const p: Problems = {};
  const name = nameProblem(d.name.trim(), taken, original);
  if (name) p.name = name;
  if (d.kind === 'local') {
    const { words, open } = splitLine(d.line);
    if (!words[0]) p.line = 'Enter the command that starts the server.';
    else if (open) p.line = "A quote isn't closed.";
    const env = readPairs(d.env, '=').bad;
    if (env) p.env = `Line ${env} isn't KEY=value.`;
  } else {
    if (!isHttpUrl(d.url.trim())) p.url = 'Enter a URL that starts with https:// or http://.';
    const headers = readPairs(d.headers, ':').bad;
    if (headers) p.headers = `Line ${headers} isn't Name: value.`;
  }
  const timeout = d.timeout.trim();
  if (timeout && !/^\d+$/.test(timeout)) p.timeout = 'Use a whole number of milliseconds.';
  return p;
}

/** Drop comments and trailing commas outside strings: READMEs often show configs that way. */
function stripJsonc(text: string): string {
  let out = '';
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (c === '"') {
      const start = i;
      for (i++; i < text.length && text[i] !== '"'; i++) if (text[i] === '\\') i++;
      out += text.slice(start, i + 1);
    } else if (c === '/' && text[i + 1] === '/') {
      while (i < text.length && text[i] !== '\n') i++;
      out += '\n';
    } else if (c === '/' && text[i + 1] === '*') {
      const end = text.indexOf('*/', i + 2);
      i = end < 0 ? text.length : end + 1;
    } else if (!(c === ',' && /^\s*[}\]]/.test(text.slice(i + 1)))) {
      out += c;
    }
  }
  return out;
}

const isServer = (v: unknown): boolean => isObj(v) && (typeof v.command === 'string' || typeof v.url === 'string');

/** Pasted JSON: one server's config, an `mcpServers` (or `servers`) block with one server, or a `"name": {…}` entry. */
function parseJson(text: string): { config: Json; name?: string } | { error: string } {
  const t = text.trim();
  if (!t) return { error: "Paste the server's JSON config." };
  let v: unknown;
  try {
    v = JSON.parse(stripJsonc(t));
  } catch (e) {
    try {
      v = JSON.parse(stripJsonc(`{${t}}`));
    } catch {
      return { error: `That isn't valid JSON. ${e instanceof Error ? e.message : ''}`.trim() };
    }
  }
  if (!isObj(v)) return { error: 'A server config is a JSON object, like { "command": "npx", "args": [] }.' };
  if (isServer(v)) return { config: clean(v) };
  const box = [v.mcpServers, v.servers, isObj(v.mcp) ? v.mcp.servers : undefined, v].find((m): m is Json => isObj(m) && Object.values(m).some(isServer));
  if (!box) return { config: clean(v) };
  const found = Object.entries(box).filter((e): e is [string, Json] => isServer(e[1]));
  if (found.length > 1) return { error: `That has ${found.length} servers (${found.map(([n]) => n).join(', ')}). Paste one at a time.` };
  return { name: found[0][0], config: clean(found[0][1]) };
}

/** Why a config can't be saved, if it can't. Types Graysense doesn't know are left to the runtime. */
function configProblem(cfg: Json): string | null {
  const kind = kindOf(cfg);
  if (kind === 'http' || kind === 'sse') return isHttpUrl(String(cfg.url ?? '').trim()) ? null : 'A remote server needs a "url" that starts with https:// or http://.';
  if (kind === 'local') return typeof cfg.command === 'string' && cfg.command.trim() ? null : 'A local server needs a "command" to run.';
  if (typeof cfg.type === 'string') return null;
  return 'A server needs a "command" to run or a "url" to connect to.';
}

/** The MCP Apps sample server: one tool that shows an interactive app. */
function demoConfig(): Json {
  const args = ['-y', '@modelcontextprotocol/server-basic-vanillajs', '--stdio'];
  // npx is a .cmd script on Windows, which only starts through a shell.
  if (navigator.userAgent.includes('Windows')) return { type: 'local', command: 'cmd', args: ['/c', 'npx', ...args], tools: ['*'] };
  return { type: 'local', command: 'npx', args, tools: ['*'] };
}

// ------------------------------------------------------------------ panel

interface EditorInit {
  /** The server being edited; null adds one. */
  original: string | null;
  name: string;
  config: Json;
}

const NEW_SERVER: EditorInit = { original: null, name: '', config: {} };

interface Ctl {
  toggleOpen: (name: string) => void;
  setEnabled: (name: string, on: boolean) => void;
  loadTools: (name: string) => void;
  lifecycle: (name: string, op: Op) => void;
  edit: (s: McpServerDto) => void;
  menu: (s: McpServerDto) => MenuItem[];
}

/** The agent whose session MCP actions use: the focused one, else the first (as the core picks). */
function targetAgent(s: Ui): number | null {
  const agents = s.bays.filter((b) => b.kind === 'agent');
  return (agents.find((b) => b.id === s.app?.focused) ?? agents[0])?.id ?? null;
}

export function McpPanel() {
  const mcp = useUi((s) => s.mcp);
  const target = useUi(targetAgent);
  const liveTitle = useUi((s) => (s.mcp?.liveBay == null ? null : (s.bays.find((b) => b.id === s.mcp?.liveBay)?.title ?? null)));
  const hasAgent = target !== null;
  const live = liveTitle !== null;
  const [open, setOpen] = useState<ReadonlySet<string>>(() => new Set());
  const [asked, setAsked] = useState<ReadonlySet<string>>(() => new Set());
  const [flip, setFlip] = useState<Record<string, boolean>>({});
  const [busy, setBusy] = useState<Record<string, string>>({});
  const [editor, setEditor] = useState<EditorInit | null>(null);
  const alive = useAlive();

  const refresh = () => void act({ type: 'mcpRefresh' });

  useEffect(() => {
    if (!useUi.getState().mcp?.loading) refresh();
  }, []);

  // Live status comes from one agent's session. Follow the focused agent, and refresh again when a
  // change made elsewhere refreshed without one (the list then has no status).
  const seen = useRef({ target, live: mcp?.liveBay ?? null });
  useEffect(() => {
    if (!mcp || mcp.loading) return;
    const prev = seen.current;
    seen.current = { target, live: mcp.liveBay };
    const moved = prev.target !== target && target !== mcp.liveBay;
    const dropped = prev.live !== null && mcp.liveBay === null && target !== null;
    if (moved || dropped) refresh();
  }, [mcp, target]);

  const servers = mcp?.servers ?? null;
  const list = servers && 'Ok' in servers ? servers.Ok : null;
  const tools = useMemo(() => new Map(mcp?.tools ?? []), [mcp?.tools]);
  const taken = useMemo(() => new Set((list ?? []).map((s) => s.name)), [list]);
  const groups = useMemo(() => {
    const by = new Map<string, McpServerDto[]>();
    for (const s of list ?? []) by.set(s.source, [...(by.get(s.source) ?? []), s]);
    return [...by].sort(([a], [b]) => rank(a) - rank(b) || a.localeCompare(b));
  }, [list]);

  const run = async (name: string, label: string, action: Action) => {
    setBusy((b) => ({ ...b, [name]: label }));
    const w = follow(mcpSettled, 30000);
    if ((await act(action)) === undefined) w.cancel();
    await w.promise;
    if (alive.current) setBusy((b) => omit(b, name));
  };

  const setEnabled = async (name: string, on: boolean) => {
    setFlip((f) => ({ ...f, [name]: on }));
    const w = follow(mcpSettled, 20000);
    if ((await act({ type: 'mcpSetEnabled', name, enabled: on })) === undefined) w.cancel();
    await w.promise;
    if (alive.current) setFlip((f) => omit(f, name));
  };

  const remove = (s: McpServerDto) =>
    openOverlay({
      kind: 'confirm',
      title: `Remove ${s.name}?`,
      body: "It comes out of your Copilot config, so new sessions won't have its tools. You can add it again later.",
      confirm: 'Remove',
      danger: true,
      onConfirm: () => void run(s.name, REMOVING, { type: 'mcpRemove', name: s.name }),
    });

  const ctl: Ctl = {
    toggleOpen: (name) => setOpen((o) => toggled(o, name)),
    setEnabled: (name, on) => void setEnabled(name, on),
    loadTools: (name) => {
      setAsked((a) => new Set(a).add(name));
      void act({ type: 'mcpTools', server: name }).then((r) => {
        if (r === undefined && alive.current) setAsked((a) => without(a, name));
      });
    },
    lifecycle: (name, op) => void run(name, OPS[op].busy, OPS[op].action(name)),
    edit: (s) => setEditor({ original: s.name, name: s.name, config: clean(s.config) }),
    menu: (s) => {
      const on = flip[s.name] ?? s.enabled;
      const groups: MenuItem[][] = [];
      if (hasAgent && on) groups.push(opsFor(live ? s.status : null).map((op) => ({ label: OPS[op].label, icon: OPS[op].icon, disabled: !!busy[s.name], onSelect: () => ctl.lifecycle(s.name, op) })));
      const manage: MenuItem[] = [];
      if (s.source !== 'session') manage.push({ label: on ? 'Turn off' : 'Turn on', icon: on ? 'circle-slash' : 'pass', onSelect: () => ctl.setEnabled(s.name, !on) });
      if (canEdit(s)) manage.push({ label: 'Edit…', icon: 'edit', onSelect: () => ctl.edit(s) });
      if (isObj(s.config)) manage.push({ label: 'Copy config JSON', icon: 'copy', onSelect: () => void copyText(pretty({ mcpServers: { [s.name]: clean(s.config) } }), 'Copied the config') });
      groups.push(manage);
      if (canEdit(s)) groups.push([{ label: 'Remove…', icon: 'trash', danger: true, onSelect: () => remove(s) }]);
      return groups.filter((g) => g.length).flatMap<MenuItem>((g, i) => (i ? [{ sep: true }, ...g] : g));
    },
  };

  const closeEditor = useCallback(() => setEditor(null), []);

  let body: ReactNode;
  if (!servers) {
    body = (
      <div className="mcp-loading" role="status">
        <span className="spinner sm" />
        Loading servers…
      </div>
    );
  } else if ('Err' in servers) {
    body = <Problem title="Couldn't load MCP servers" detail={servers.Err} onRetry={refresh} />;
  } else if (!servers.Ok.length) {
    body = (
      <div className="empty">
        <Icon name="server-process" />
        <h4>No MCP servers yet</h4>
        <p>MCP servers give agents more tools, like searching docs or reading an issue tracker. Some show interactive apps right in the chat.</p>
        <div className="mcp-empty-actions">
          <button type="button" className="btn primary sm" onClick={() => setEditor(NEW_SERVER)}>
            <Icon name="add" />
            Add server
          </button>
          <button type="button" className="btn ghost sm" onClick={() => setEditor({ original: null, name: 'mcp-apps-demo', config: demoConfig() })}>
            Try the MCP Apps demo
          </button>
        </div>
      </div>
    );
  } else {
    body = groups.map(([source, items]) => (
      <section key={source} aria-label={sourceLabel(source)}>
        <div className="panel-section">
          <span className="section-label">
            <span data-tip={SOURCES[source]?.tip}>{sourceLabel(source)}</span>
          </span>
          <span className="mcp-count">{items.length}</span>
        </div>
        {items.map((s) => (
          <ServerItem key={s.name} s={s} open={open.has(s.name)} live={live} hasAgent={hasAgent} enabled={flip[s.name] ?? s.enabled} busy={busy[s.name]} tools={tools.get(s.name)} asked={asked.has(s.name)} ctl={ctl} />
        ))}
      </section>
    ));
  }

  const hint = !hasAgent ? 'Open an agent to see live status and tools.' : live ? `Live status from “${liveTitle}”.` : mcp?.loading ? 'Checking live status…' : 'Live status shows once the agent is ready.';

  return (
    <div className="panel mcp">
      <div className="panel-head">
        <h2>MCP servers</h2>
        <button type="button" className="icon-btn" data-tip="Refresh" aria-label="Refresh" aria-busy={!!mcp?.loading} onClick={refresh}>
          {mcp?.loading ? <span className="spinner sm" /> : <Icon name="refresh" />}
        </button>
        <button type="button" className="icon-btn" data-tip="Add server" aria-label="Add server" onClick={() => setEditor(NEW_SERVER)}>
          <Icon name="add" />
        </button>
      </div>
      {!!list?.length && (
        <div className="panel-toolbar">
          <p className={`mcp-hint ${live ? 'live' : ''}`}>
            <Icon name={live ? 'pulse' : 'info'} />
            <span>{hint} Changes apply to new sessions.</span>
          </p>
        </div>
      )}
      <div className="panel-body mcp-list">
        {body}
        <p className="mcp-foot">
          <Icon name="sparkle" />
          <span>Agents can add and manage servers too — just ask.</span>
        </p>
      </div>
      {editor && <ServerEditor init={editor} taken={taken} onClose={closeEditor} />}
    </div>
  );
}

// ------------------------------------------------------------------ server rows

interface ItemProps {
  s: McpServerDto;
  open: boolean;
  live: boolean;
  hasAgent: boolean;
  /** Enabled, as the user last set it (before the core confirms). */
  enabled: boolean;
  busy: string | undefined;
  tools: Result<McpToolDto[]> | undefined;
  /** Its tools were asked for. */
  asked: boolean;
  ctl: Ctl;
}

function ServerItem(props: ItemProps) {
  const { s, open, live, enabled, busy, tools, ctl } = props;
  const st = look(s, live, enabled, busy);
  const kind = kindOf(s.config);
  const count = st.tone === 'ok' && tools && 'Ok' in tools ? tools.Ok.length : null;
  const summary = st.text ? null : s.plugin ? `From ${s.plugin}` : summaryOf(s.config);
  const sub = count !== null ? `${st.text} · ${plural(count, 'tool')}` : (st.text ?? summary);
  const tone = st.tone === 'danger' || st.tone === 'warn' ? st.tone : '';
  return (
    <div className={`mcp-server ${open ? 'open' : ''} ${enabled ? '' : 'off'} ${busy === REMOVING ? 'removing' : ''}`}>
      <div
        className="list-row mcp-row"
        onContextMenu={(e) => {
          e.preventDefault();
          showMenu(e, ctl.menu(s));
        }}
      >
        <button type="button" className="mcp-hit" aria-expanded={open} onClick={() => ctl.toggleOpen(s.name)}>
          <Icon name="chevron-right" className="mcp-chev" />
          <span className={`dot ${DOT[st.tone]}`} />
          <span className="mcp-main">
            <span className="mcp-title">
              <span className="mcp-name ellipsis">{s.name}</span>
              {kind && (
                <span className="mcp-kind" data-tip={KIND_TIP[kind]}>
                  {kind}
                </span>
              )}
            </span>
            {sub && <span className={`mcp-sub ellipsis ${tone} ${summary && !s.plugin ? 'mono' : ''}`}>{sub}</span>}
          </span>
        </button>
        {s.source !== 'session' && (
          <span className="mcp-switch" data-tip={enabled ? 'Turn off for new sessions' : 'Turn on for new sessions'}>
            <Toggle on={enabled} onChange={(on) => ctl.setEnabled(s.name, on)} label={s.name} disabled={busy === REMOVING} />
          </span>
        )}
      </div>
      {open && <Details {...props} />}
    </div>
  );
}

function Details({ s, live, hasAgent, enabled, busy, tools, asked, ctl }: ItemProps) {
  const cfg = isObj(s.config) ? s.config : null;
  const kind = kindOf(cfg);
  const status = live ? s.status : null;
  const loaded = tools && 'Ok' in tools ? tools.Ok : null;
  const failed = tools && 'Err' in tools ? tools.Err : null;
  const loading = asked && !tools;
  const canLoad = hasAgent && live && enabled && status === 'connected';
  const [all, setAll] = useState(false);
  const tried = useRef(false);

  useEffect(() => {
    if (canLoad && !tools && !asked && !tried.current) {
      tried.current = true;
      ctl.loadTools(s.name);
    }
  }, [canLoad, tools, asked]);

  const envKeys = cfg && isObj(cfg.env) ? Object.keys(cfg.env) : [];
  const headerKeys = cfg && isObj(cfg.headers) ? Object.keys(cfg.headers) : [];
  const allow = cfg ? strList(cfg.tools).filter((t) => t !== '*') : [];
  const ops = hasAgent && enabled ? opsFor(status) : [];
  const why = lockedWhy(s);
  const shown = loaded ? (all ? loaded : loaded.slice(0, TOOLS_SHOWN)) : [];

  let toolsBody: ReactNode;
  if (loading) {
    toolsBody = (
      <div className="mcp-quiet">
        <span className="spinner sm" />
        Loading tools…
      </div>
    );
  } else if (failed !== null) {
    toolsBody = <div className="mcp-quiet danger selectable">{failed}</div>;
  } else if (loaded) {
    toolsBody = loaded.length ? (
      <>
        <ul className="mcp-tool-list">
          {shown.map((t) => (
            <ToolRow key={t.name} t={t} />
          ))}
        </ul>
        {loaded.length > shown.length && (
          <button type="button" className="link-btn mcp-all-tools" onClick={() => setAll(true)}>
            Show all {loaded.length}
          </button>
        )}
      </>
    ) : (
      <div className="mcp-quiet">It doesn't offer any tools.</div>
    );
  } else {
    const text = !hasAgent ? 'Open an agent to see its tools.' : !enabled ? 'Turn it on to see its tools.' : status === 'pending' ? 'Tools show once it connects.' : "Tools show once it's running.";
    toolsBody = <div className="mcp-quiet">{text}</div>;
  }

  return (
    <div className="mcp-details">
      {live && s.error && (
        <div className="mcp-note danger selectable" role="alert">
          <Icon name="error" />
          <span>{s.error}</span>
        </div>
      )}
      {status === 'needs-auth' && (
        <div className="mcp-note warn">
          <Icon name="key" />
          <span>It needs you to sign in before agents can use it.</span>
        </div>
      )}
      {!enabled && status === 'connected' && (
        <div className="mcp-note">
          <Icon name="info" />
          <span>New sessions skip it. This agent keeps it until its session ends.</span>
        </div>
      )}
      {enabled && live && status === null && !busy && (
        <div className="mcp-note">
          <Icon name="info" />
          <span>It isn't running in this agent. New sessions load it.</span>
        </div>
      )}

      {cfg && kind && (
        <dl className="mcp-props">
          {kind === 'local' ? (
            <>
              <dt>Command</dt>
              <dd className="mono selectable">{summaryOf(cfg)}</dd>
              {typeof cfg.cwd === 'string' && cfg.cwd !== '' && (
                <>
                  <dt>Folder</dt>
                  <dd className="mono selectable">{cfg.cwd}</dd>
                </>
              )}
              {envKeys.length > 0 && (
                <>
                  <dt>Env</dt>
                  <dd>
                    <Secrets keys={envKeys} />
                  </dd>
                </>
              )}
            </>
          ) : (
            <>
              <dt>URL</dt>
              <dd className="mono selectable">{String(cfg.url ?? '')}</dd>
              {headerKeys.length > 0 && (
                <>
                  <dt>Headers</dt>
                  <dd>
                    <Secrets keys={headerKeys} />
                  </dd>
                </>
              )}
            </>
          )}
          {allow.length > 0 && (
            <>
              <dt>Allows</dt>
              <dd className="mono">{allow.join(', ')}</dd>
            </>
          )}
          {typeof cfg.timeout === 'number' && (
            <>
              <dt>Timeout</dt>
              <dd>{cfg.timeout} ms</dd>
            </>
          )}
        </dl>
      )}

      <div className="mcp-actions">
        {ops.map((op) => (
          <button key={op} type="button" className="btn sm" disabled={!!busy} onClick={() => ctl.lifecycle(s.name, op)}>
            <Icon name={OPS[op].icon} />
            {OPS[op].label}
          </button>
        ))}
        {canEdit(s) && (
          <button type="button" className="btn sm" disabled={!!busy} onClick={() => ctl.edit(s)}>
            <Icon name="edit" />
            Edit
          </button>
        )}
        <button type="button" className="icon-btn sm mcp-more" aria-label={`More actions for ${s.name}`} data-tip="More actions" onClick={(e) => showMenu(e.currentTarget, ctl.menu(s), { alignRight: true })}>
          <Icon name="ellipsis" />
        </button>
      </div>

      {why && (
        <div className="mcp-note">
          <Icon name={s.plugin ? 'extensions' : 'lock'} />
          <span>
            {why}
            {s.plugin && (
              <>
                {' '}
                <button type="button" className="link-btn" onClick={() => void setSide('plugins')}>
                  Manage plugins
                </button>
              </>
            )}
          </span>
        </div>
      )}

      <div className="mcp-tools">
        <div className="mcp-tools-head">
          <span className="section-label">Tools</span>
          {loaded && <span className="mcp-count">{loaded.length}</span>}
          <span className="grow" />
          {hasAgent && enabled && (
            <button type="button" className="icon-btn sm" data-tip="Reload tools" aria-label={`Reload tools of ${s.name}`} disabled={loading} onClick={() => ctl.loadTools(s.name)}>
              <Icon name="refresh" />
            </button>
          )}
        </div>
        {toolsBody}
      </div>
    </div>
  );
}

function ToolRow({ t }: { t: McpToolDto }) {
  const [full, setFull] = useState(false);
  return (
    <li className={`mcp-tool ${full ? 'full' : ''}`}>
      <div className="mcp-tool-hit" aria-expanded={full} {...pressable(() => setFull(!full))}>
        <span className="mcp-tool-title">
          <span className="mcp-tool-name ellipsis">{t.name}</span>
          {t.hasUi && (
            <span className="badge accent" data-tip="Shows an interactive app in the chat">
              UI
            </span>
          )}
        </span>
        {t.description && <span className="mcp-tool-desc">{t.description}</span>}
      </div>
    </li>
  );
}

/** Environment variable or header names; their values stay hidden. */
function Secrets({ keys }: { keys: string[] }) {
  return (
    <span className="mcp-secrets">
      {keys.map((k) => (
        <span key={k} className="mcp-secret">
          <span className="ellipsis">{k}</span>
          <span className="mcp-mask" aria-label="value hidden">
            ••••
          </span>
        </span>
      ))}
    </span>
  );
}

function Problem({ title, detail, onRetry }: { title: string; detail: string; onRetry: () => void }) {
  return (
    <div className="mcp-problem" role="alert">
      <Icon name="warning" />
      <div className="grow">
        <div className="mcp-problem-title">{title}</div>
        <p className="selectable">{detail}</p>
        <button type="button" className="btn sm" onClick={onRetry}>
          <Icon name="refresh" />
          Retry
        </button>
      </div>
    </div>
  );
}

// ------------------------------------------------------------------ add / edit

function Field({ id, label, optional, hint, error, children }: { id?: string; label: string; optional?: boolean; hint?: ReactNode; error?: string | null; children: ReactNode }) {
  const text = (
    <>
      {label}
      {optional && <span className="faint"> (optional)</span>}
    </>
  );
  return (
    <div className={`mcp-field ${error ? 'invalid' : ''}`}>
      {id ? (
        <label className="mcp-label" htmlFor={id}>
          {text}
        </label>
      ) : (
        <span className="mcp-label">{text}</span>
      )}
      {children}
      {error ? (
        <div className="mcp-field-error" role="alert">
          <Icon name="error" />
          <span>{error}</span>
        </div>
      ) : (
        hint && <div className="mcp-field-hint">{hint}</div>
      )}
    </div>
  );
}

const JSON_PLACEHOLDER = '{\n  "type": "local",\n  "command": "npx",\n  "args": ["-y", "@scope/server"],\n  "tools": ["*"]\n}';

function ServerEditor({ init, taken, onClose }: { init: EditorInit; taken: ReadonlySet<string>; onClose: () => void }) {
  const id = useId();
  const start = useMemo(() => draftOf(init.name, init.config), [init]);
  const [draft, setDraft] = useState<Draft>(start.draft);
  const [base, setBase] = useState<Json>(init.config);
  const [mode, setMode] = useState<'form' | 'json'>(start.formOk ? 'form' : 'json');
  const [json, setJson] = useState(() => pretty(init.config));
  const [jsonError, setJsonError] = useState<string | null>(null);
  const [more, setMore] = useState(Boolean(start.draft.tools || start.draft.timeout));
  const [tried, setTried] = useState(false);
  const [saving, setSaving] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const alive = useAlive();
  const envRef = useRef<HTMLTextAreaElement>(null);
  const headersRef = useRef<HTMLTextAreaElement>(null);
  const jsonRef = useRef<HTMLTextAreaElement>(null);
  useAutosize(envRef, `${draft.kind}${draft.env}`, 150);
  useAutosize(headersRef, `${draft.kind}${draft.headers}`, 150);
  useAutosize(jsonRef, `${mode}${json}`, 340);

  const editing = init.original !== null;
  const errs = tried ? problems(draft, taken, init.original) : {};
  const set = (patch: Partial<Draft>) => {
    setDraft((d) => ({ ...d, ...patch }));
    setFailure(null);
  };

  const toJson = () => {
    setJson(pretty(buildConfig(draft, base)));
    setJsonError(null);
    setMode('json');
  };

  const toForm = () => {
    const r = parseJson(json);
    if ('error' in r) return setJsonError(r.error);
    const next = draftOf(draft.name.trim() || r.name || '', r.config);
    if (!next.formOk) return setJsonError("The form can't show all of this config. Keep editing it as JSON.");
    setDraft({ ...next.draft, name: draft.name.trim() ? draft.name : next.draft.name });
    setBase(r.config);
    setMore(Boolean(next.draft.tools || next.draft.timeout));
    setJsonError(null);
    setMode('form');
  };

  const demo = () => {
    setDraft(draftOf(draft.name.trim() || 'mcp-apps-demo', demoConfig()).draft);
    setBase({});
    setJsonError(null);
    setFailure(null);
    setMode('form');
  };

  const browse = async () => {
    const dir = await pickFolder('Working folder for this server', draft.cwd.trim() || useUi.getState().app?.defaultDir);
    if (dir) set({ cwd: dir });
  };

  const save = async () => {
    if (saving) return;
    setTried(true);
    let name = draft.name.trim();
    let config: Json;
    if (mode === 'json') {
      const r = parseJson(json);
      const problem = 'error' in r ? r.error : configProblem(r.config);
      if (problem || 'error' in r) return setJsonError(problem);
      setJsonError(null);
      if (!name && r.name) {
        name = r.name;
        setDraft((d) => ({ ...d, name }));
      }
      if (nameProblem(name, taken, init.original)) return;
      config = r.config;
    } else {
      if (Object.keys(problems(draft, taken, init.original)).length) return;
      config = buildConfig(draft, base);
    }
    setSaving(true);
    setFailure(null);
    const update = editing && init.original === name;
    const w = follow(mcpSettled, 20000);
    if ((await act({ type: 'mcpSave', name, config, update })) === undefined) {
      w.cancel();
      if (alive.current) setSaving(false);
      return;
    }
    const outcome = await w.promise;
    if (outcome?.level === 'error') {
      if (!alive.current) return;
      dismissToast(outcome.id);
      setFailure(outcome.text);
      setSaving(false);
      return;
    }
    // A new name is a new server: the old one goes once the new one saved.
    if (editing && init.original !== name) void act({ type: 'mcpRemove', name: init.original! });
    if (alive.current) onClose();
  };

  const nameError = errs.name;
  return (
    <Modal onClose={onClose} width={520} className="mcp-ed" labelledBy={`${id}-title`}>
      <div className="mcp-ed-head">
        <div className="grow">
          <h3 id={`${id}-title`}>{editing ? `Edit ${init.original}` : 'Add MCP server'}</h3>
          <p>
            {editing ? (
              'Changes apply to new sessions.'
            ) : (
              <>
                Servers give agents new tools, and some show interactive apps.{' '}
                <button type="button" className="link-btn" onClick={demo}>
                  Try the MCP Apps demo
                </button>
              </>
            )}
          </p>
        </div>
        <button type="button" className={`btn ghost sm ${mode === 'json' ? 'mcp-on' : ''}`} aria-pressed={mode === 'json'} disabled={saving} onClick={mode === 'json' ? toForm : toJson}>
          <Icon name="json" />
          {mode === 'json' ? 'Use the form' : 'Paste JSON'}
        </button>
      </div>
      <form
        id={`${id}-form`}
        className="modal-body mcp-ed-body"
        noValidate
        onSubmit={(e) => {
          e.preventDefault();
          void save();
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
            e.preventDefault();
            void save();
          }
        }}
      >
        <fieldset className="mcp-ed-fields" disabled={saving}>
          <Field id={`${id}-name`} label="Name" error={nameError} hint="No spaces. It prefixes the names of the server's tools.">
            <input id={`${id}-name`} className="input mono" value={draft.name} placeholder="my-server" autoFocus={!editing} spellCheck={false} autoComplete="off" onChange={(e) => set({ name: e.target.value })} />
          </Field>

          {mode === 'form' ? (
            <>
              <Field label="Type">
                <Seg
                  value={draft.kind}
                  options={[
                    { value: 'local', label: 'Local command' },
                    { value: 'remote', label: 'Remote URL' },
                  ]}
                  onChange={(kind) => set({ kind })}
                />
              </Field>
              {draft.kind === 'local' ? (
                <>
                  <Field id={`${id}-line`} label="Command" error={errs.line} hint="The program and its arguments, as you'd type them in a terminal. Quote arguments that have spaces.">
                    <input id={`${id}-line`} className="input mono" value={draft.line} placeholder="npx -y @modelcontextprotocol/server-everything" autoFocus={editing} spellCheck={false} autoComplete="off" onChange={(e) => set({ line: e.target.value })} />
                  </Field>
                  <Field id={`${id}-env`} label="Environment" optional error={errs.env} hint="One KEY=value per line.">
                    <textarea ref={envRef} id={`${id}-env`} className="textarea mono" rows={2} value={draft.env} placeholder="API_KEY=…" spellCheck={false} onChange={(e) => set({ env: e.target.value })} />
                  </Field>
                  <Field id={`${id}-cwd`} label="Working folder" optional>
                    <div className="mcp-inline">
                      <input id={`${id}-cwd`} className="input mono" value={draft.cwd} placeholder="C:\path\to\folder" spellCheck={false} autoComplete="off" onChange={(e) => set({ cwd: e.target.value })} />
                      <button type="button" className="btn" onClick={() => void browse()}>
                        Browse…
                      </button>
                    </div>
                  </Field>
                </>
              ) : (
                <>
                  <Field id={`${id}-url`} label="URL" error={errs.url}>
                    <input
                      id={`${id}-url`}
                      className="input mono"
                      type="url"
                      value={draft.url}
                      placeholder="https://example.com/mcp"
                      autoFocus={editing}
                      spellCheck={false}
                      autoComplete="off"
                      onChange={(e) => {
                        const url = e.target.value;
                        set(draft.sseSet ? { url } : { url, sse: /\/sse\/?$/i.test(url.trim()) });
                      }}
                    />
                  </Field>
                  <label className="mcp-check">
                    <Toggle on={draft.sse} onChange={(sse) => set({ sse, sseSet: true })} label="Uses server-sent events" />
                    <span>
                      Uses server-sent events <span className="faint">(older servers)</span>
                    </span>
                  </label>
                  <Field id={`${id}-headers`} label="Headers" optional error={errs.headers} hint="One Name: value per line.">
                    <textarea ref={headersRef} id={`${id}-headers`} className="textarea mono" rows={2} value={draft.headers} placeholder="Authorization: Bearer …" spellCheck={false} onChange={(e) => set({ headers: e.target.value })} />
                  </Field>
                </>
              )}

              <button type="button" className="mcp-disclose" aria-expanded={more} onClick={() => setMore(!more)}>
                <Icon name="chevron-right" />
                More options
              </button>
              {more && (
                <>
                  <Field id={`${id}-tools`} label="Tools" optional hint="Comma-separated names of the tools agents may use. Leave empty for all.">
                    <input id={`${id}-tools`} className="input mono" value={draft.tools} placeholder="All tools" spellCheck={false} autoComplete="off" onChange={(e) => set({ tools: e.target.value })} />
                  </Field>
                  <Field id={`${id}-timeout`} label="Timeout" optional error={errs.timeout} hint="Milliseconds to wait for its tool lists and calls.">
                    <input id={`${id}-timeout`} className="input mono mcp-short" inputMode="numeric" value={draft.timeout} placeholder="Default" spellCheck={false} autoComplete="off" onChange={(e) => set({ timeout: e.target.value })} />
                  </Field>
                </>
              )}
            </>
          ) : (
            <Field id={`${id}-json`} label="Config JSON" error={jsonError} hint="One server's config, or an mcpServers block with one server in it.">
              <textarea
                ref={jsonRef}
                id={`${id}-json`}
                className="textarea mono mcp-json"
                rows={9}
                value={json}
                placeholder={JSON_PLACEHOLDER}
                spellCheck={false}
                autoFocus={editing}
                onChange={(e) => {
                  setJson(e.target.value);
                  setJsonError(null);
                  setFailure(null);
                }}
              />
            </Field>
          )}

          {failure && (
            <div className="mcp-note danger selectable" role="alert">
              <Icon name="error" />
              <span>Couldn't save it: {failure}</span>
            </div>
          )}
          {!editing && (
            <p className="mcp-trust">
              <Icon name="shield" />
              <span>Servers run with your access to this computer and your accounts. Add only servers you trust.</span>
            </p>
          )}
        </fieldset>
      </form>
      <div className="modal-foot">
        <button type="button" className="btn ghost" onClick={onClose}>
          Cancel
        </button>
        <button type="submit" form={`${id}-form`} className="btn primary" disabled={saving}>
          {saving && <span className="spinner sm" />}
          {saving ? 'Saving…' : 'Save'}
        </button>
      </div>
    </Modal>
  );
}
