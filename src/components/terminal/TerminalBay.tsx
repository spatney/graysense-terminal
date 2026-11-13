import { Channel, invoke } from '@tauri-apps/api/core';
import { FitAddon } from '@xterm/addon-fit';
import { SearchAddon } from '@xterm/addon-search';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import { WebLinksAddon } from '@xterm/addon-web-links';
import { WebglAddon } from '@xterm/addon-webgl';
import { Terminal } from '@xterm/xterm';
import { useEffect, useRef, useState } from 'react';
import { act, newShell, openLink } from '../../lib/api';
import { useUi } from '../../lib/store';
import { terminalTheme } from '../../lib/theme';
import type { BayDto, TerminalEvent, TerminalInfo, TerminalShell } from '../../lib/types';
import { Icon, type MenuItem, showMenu } from '../ui';

const HIGH_WATER = 256 * 1024;
const LOW_WATER = 32 * 1024;
// Cascadia first; the Nerd Fonts after it supply the icons prompts like oh-my-posh and Starship draw
// (fallback is per glyph, so only the icons come from them).
const FONT =
  "'Cascadia Code', 'Cascadia Mono', 'CaskaydiaCove Nerd Font Mono', 'CaskaydiaMono Nerd Font Mono', 'Cascadia Code NF', 'Cascadia Mono NF', 'MesloLGM Nerd Font Mono', 'MesloLGS Nerd Font Mono', 'JetBrainsMono Nerd Font Mono', 'FiraCode Nerd Font Mono', 'Hack Nerd Font Mono', 'Symbols Nerd Font Mono', Consolas, 'Courier New', monospace";

/** One shell in a pseudoterminal, shown with xterm.js. */
class Session {
  term: Terminal;
  fit = new FitAddon();
  search = new SearchAddon({ highlightLimit: 1000 });
  private webgl: WebglAddon | null = null;
  private running = false;
  private pending = 0;
  private paused = false;
  private outbox = '';
  private sending = false;
  private resizeTimer = 0;
  private generation = 0;
  disposed = false;
  onExit: (code: number | null) => void = () => {};
  onTitle: (title: string) => void = () => {};

  constructor(
    readonly bay: number,
    host: HTMLElement,
    fontSize: number,
    blink: boolean,
  ) {
    const light = document.documentElement.dataset.theme === 'paper';
    this.term = new Terminal({
      allowProposedApi: true,
      fontFamily: FONT,
      fontSize,
      lineHeight: 1.2,
      cursorBlink: blink,
      cursorStyle: 'bar',
      cursorWidth: 2,
      scrollback: 10_000,
      minimumContrastRatio: light ? 4.5 : 1,
      theme: terminalTheme(),
      drawBoldTextInBrightColors: false,
    });
    this.term.loadAddon(this.fit);
    this.term.loadAddon(this.search);
    this.term.loadAddon(new Unicode11Addon());
    this.term.unicode.activeVersion = '11';
    this.term.loadAddon(new WebLinksAddon((e, uri) => (e.ctrlKey ? openLink(uri) : undefined)));
    this.term.open(host);
    try {
      this.webgl = new WebglAddon();
      this.webgl.onContextLoss(() => {
        this.webgl?.dispose();
        this.webgl = null;
      });
      this.term.loadAddon(this.webgl);
    } catch {
      this.webgl = null;
    }
    this.term.onData((d) => this.input(d));
    this.term.onTitleChange((t) => this.onTitle(t));
    this.term.onResize(({ cols, rows }) => this.resized(cols, rows));
    this.term.attachCustomKeyEventHandler((e) => this.key(e));
  }

  async start() {
    const gen = ++this.generation;
    this.running = false;
    this.fitNow();
    const channel = new Channel<TerminalEvent>();
    channel.onmessage = (ev) => {
      if (gen !== this.generation || this.disposed) return;
      if (ev.kind === 'output') this.output(ev.data);
      else {
        this.running = false;
        this.onExit(ev.code);
      }
    };
    try {
      const info = await invoke<TerminalInfo>('terminal_spawn', { bay: this.bay, cols: this.term.cols, rows: this.term.rows, onEvent: channel });
      if (gen !== this.generation || this.disposed) return;
      if (info.windowsBuild) this.term.options.windowsPty = { backend: 'conpty', buildNumber: info.windowsBuild };
      this.running = true;
      void this.send();
    } catch (e) {
      this.term.write(`\x1b[31m${String(e)}\x1b[0m\r\n`);
      this.onExit(null);
    }
  }

  restart() {
    this.term.reset();
    void this.start();
  }

  private output(data: string) {
    const size = data.length;
    this.pending += size;
    this.term.write(data, () => {
      this.pending -= size;
      if (this.paused && this.pending <= LOW_WATER) {
        this.paused = false;
        void invoke('terminal_pause', { bay: this.bay, paused: false }).catch(() => {});
      }
    });
    if (!this.paused && this.pending > HIGH_WATER) {
      this.paused = true;
      void invoke('terminal_pause', { bay: this.bay, paused: true }).catch(() => {});
    }
  }

  input(data: string) {
    if (this.disposed) return;
    this.outbox += data;
    void this.send();
  }

  /** One write at a time, so keystrokes arrive in order; typed-ahead text waits for the shell. */
  private async send() {
    if (this.sending) return;
    this.sending = true;
    try {
      while (this.outbox && this.running) {
        const data = this.outbox;
        this.outbox = '';
        await invoke('terminal_write', { bay: this.bay, data }).catch(() => {});
      }
    } finally {
      this.sending = false;
    }
  }

  private resized(cols: number, rows: number) {
    window.clearTimeout(this.resizeTimer);
    this.resizeTimer = window.setTimeout(() => {
      if (this.running) void invoke('terminal_resize', { bay: this.bay, cols, rows }).catch(() => {});
    }, 60);
  }

  fitNow() {
    try {
      const el = this.term.element;
      if (el && el.offsetWidth > 0 && el.offsetHeight > 0) this.fit.fit();
    } catch {
      /* not laid out yet */
    }
  }

  copy() {
    const text = this.term.getSelection();
    if (text) void navigator.clipboard.writeText(text).catch(() => {});
    this.term.clearSelection();
  }

  paste() {
    void navigator.clipboard
      .readText()
      .then((t) => t && this.term.paste(t))
      .catch(() => {});
  }

  private key(e: KeyboardEvent): boolean {
    if (e.type !== 'keydown') return true;
    const k = e.key.toLowerCase();
    if (e.ctrlKey && !e.metaKey) {
      if (!e.altKey && k === 'c' && (e.shiftKey || this.term.hasSelection())) {
        e.preventDefault();
        this.copy();
        return false;
      }
      // The browser pastes and xterm.js types it from the paste event.
      if (!e.altKey && k === 'v') return false;
      if (!e.altKey && !e.shiftKey && k === 'f') {
        e.preventDefault();
        window.dispatchEvent(new CustomEvent('gs-term-find', { detail: this.bay }));
        return false;
      }
      // App shortcuts reach the app.
      if (!e.altKey && e.shiftKey && /^[a-z]$/.test(k)) return false;
      if (!e.altKey && ['tab', '=', '+', '-', '0', ',', '/'].includes(k)) return false;
      if (e.altKey && /^[1-9]$/.test(e.key)) return false;
    }
    return true;
  }

  dispose() {
    this.disposed = true;
    this.generation++;
    window.clearTimeout(this.resizeTimer);
    this.webgl?.dispose();
    this.term.dispose();
  }
}

export function TerminalBay({ bay, visible, focused }: { bay: BayDto; visible: boolean; focused: boolean }) {
  const host = useRef<HTMLDivElement>(null);
  const session = useRef<Session | null>(null);
  const [exit, setExit] = useState<{ code: number | null } | null>(null);
  const [find, setFind] = useState<string | null>(null);
  const fontSize = useUi((s) => s.settings?.terminal_font_size ?? 13.5);
  const blink = useUi((s) => s.settings?.cursor_blink ?? true);
  const theme = useUi((s) => s.settings?.theme);
  const [shellName, setShellName] = useState<string | null>(null);

  useEffect(() => {
    if (!host.current) return;
    const st = useUi.getState().settings;
    const s = new Session(bay.id, host.current, st?.terminal_font_size ?? 13.5, st?.cursor_blink ?? true);
    session.current = s;
    let titleTimer = 0;
    s.onExit = (code) => setExit({ code });
    s.onTitle = (t) => {
      window.clearTimeout(titleTimer);
      titleTimer = window.setTimeout(() => void act({ type: 'shellTitle', bay: bay.id, title: t }), 300);
    };
    void s.start();
    const ro = new ResizeObserver(() => s.fitNow());
    ro.observe(host.current);
    void invoke<TerminalShell[]>('terminal_shells')
      .then((list) => setShellName((list.find((x) => x.id === bay.shell) ?? list[0])?.name ?? null))
      .catch(() => {});
    return () => {
      ro.disconnect();
      window.clearTimeout(titleTimer);
      s.dispose();
      session.current = null;
    };
  }, [bay.id, bay.shell]);

  useEffect(() => {
    const s = session.current;
    if (!s) return;
    s.term.options.fontSize = fontSize;
    s.term.options.cursorBlink = blink;
    s.fitNow();
  }, [fontSize, blink]);

  useEffect(() => {
    const s = session.current;
    if (!s || !theme) return;
    // The theme's CSS variables apply first.
    requestAnimationFrame(() => {
      s.term.options.theme = terminalTheme();
      s.term.options.minimumContrastRatio = theme === 'paper' ? 4.5 : 1;
    });
  }, [theme]);

  useEffect(() => {
    if (!visible) return;
    const s = session.current;
    requestAnimationFrame(() => {
      s?.fitNow();
      if (focused) s?.term.focus();
    });
  }, [visible, focused]);

  useEffect(() => {
    const onFind = (e: Event) => {
      if ((e as CustomEvent).detail === bay.id) setFind('');
    };
    window.addEventListener('gs-term-find', onFind);
    return () => window.removeEventListener('gs-term-find', onFind);
  }, [bay.id]);

  const restart = () => {
    setExit(null);
    session.current?.restart();
    session.current?.term.focus();
  };

  const menu = (e: React.MouseEvent) => {
    e.preventDefault();
    const s = session.current;
    if (!s) return;
    const items: MenuItem[] = [
      { label: 'Copy', icon: 'copy', hint: 'Ctrl+C', disabled: !s.term.hasSelection(), onSelect: () => s.copy() },
      { label: 'Paste', icon: 'clippy', hint: 'Ctrl+V', onSelect: () => s.paste() },
      { label: 'Select all', icon: 'list-selection', onSelect: () => s.term.selectAll() },
      { label: 'Find', icon: 'search', hint: 'Ctrl+F', onSelect: () => setFind('') },
      { label: 'Clear', icon: 'clear-all', onSelect: () => s.term.clear() },
      { sep: true },
      { label: 'Restart shell', icon: 'debug-restart', onSelect: restart },
      { label: 'Ask an agent here', icon: 'copilot', onSelect: () => void act({ type: 'newAgent', dir: bay.cwd }) },
    ];
    showMenu(e, items);
  };

  const runFind = (text: string, back = false) => {
    const s = session.current;
    if (!s || !text) return;
    if (back) s.search.findPrevious(text);
    else s.search.findNext(text);
  };
  const closeFind = () => {
    setFind(null);
    session.current?.search.clearDecorations();
    session.current?.term.focus();
  };

  return (
    <>
      <div className="bay-head">
        <Icon name="terminal" className="faint" />
        <span className="bay-title">{bay.title}</span>
        <button type="button" className="bay-crumb" data-tip={`${bay.cwd} · reveal`} onClick={() => void act({ type: 'revealPath', path: bay.cwd })}>
          <Icon name="folder" /> <span className="ellipsis">{bay.folder}</span>
        </button>
        {shellName && shellName.toLowerCase() !== bay.title.toLowerCase() && <span className="badge">{shellName}</span>}
        <div className="bay-head-actions">
          <button type="button" className="icon-btn sm" data-tip="Find" data-keys="Ctrl+F" onClick={() => setFind('')}>
            <Icon name="search" />
          </button>
          <button type="button" className="icon-btn sm" data-tip="New terminal here" onClick={() => void newShell(bay.cwd)}>
            <Icon name="split-horizontal" />
          </button>
          <button type="button" className="icon-btn sm" data-tip="Restart shell" onClick={restart}>
            <Icon name="debug-restart" />
          </button>
          <button type="button" className="icon-btn sm" data-tip="Close" data-keys="Ctrl+Shift+W" onClick={() => void act({ type: 'closeBay', bay: bay.id })}>
            <Icon name="close" />
          </button>
        </div>
      </div>
      <div className="term" onContextMenu={menu} onMouseDown={() => session.current?.term.focus()}>
        <div className="term-host" ref={host} />
        {find != null && (
          <div className="term-find" onMouseDown={(e) => e.stopPropagation()}>
            <input
              className="input"
              autoFocus
              placeholder="Find"
              value={find}
              onChange={(e) => {
                setFind(e.target.value);
                runFind(e.target.value);
              }}
              onKeyDown={(e) => {
                if (e.key === 'Enter') runFind(find, e.shiftKey);
                if (e.key === 'Escape') closeFind();
              }}
            />
            <button type="button" className="icon-btn sm" data-tip="Previous" onClick={() => runFind(find, true)}>
              <Icon name="arrow-up" />
            </button>
            <button type="button" className="icon-btn sm" data-tip="Next" onClick={() => runFind(find)}>
              <Icon name="arrow-down" />
            </button>
            <button type="button" className="icon-btn sm" aria-label="Close" onClick={closeFind}>
              <Icon name="close" />
            </button>
          </div>
        )}
        {exit && (
          <div className="term-exited">
            <span className="muted">{exit.code == null ? 'The shell ended.' : `The shell exited with code ${exit.code}.`}</span>
            <button type="button" className="btn sm primary" onClick={restart}>
              <Icon name="debug-restart" /> Restart
            </button>
            <button type="button" className="btn sm ghost" onClick={() => void act({ type: 'closeBay', bay: bay.id })}>
              Close
            </button>
          </div>
        )}
      </div>
    </>
  );
}
