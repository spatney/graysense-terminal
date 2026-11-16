import { cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { useUi } from '../../lib/store';
import type { BayDto } from '../../lib/types';
import { showMenu } from '../ui';
import { TerminalBay } from './TerminalBay';

const terminal = vi.hoisted(() => ({
  unicode: { activeVersion: '' }, options: {}, cols: 80, rows: 24,
  loadAddon: vi.fn(), open: vi.fn(), onData: vi.fn(), onTitleChange: vi.fn(), onResize: vi.fn(),
  attachCustomKeyEventHandler: vi.fn<(handler: (e: KeyboardEvent) => boolean) => void>(),
  hasSelection: vi.fn(() => true), getSelection: vi.fn(() => 'synthetic terminal selection'),
  clearSelection: vi.fn(), selectAll: vi.fn(), paste: vi.fn(), write: vi.fn(), focus: vi.fn(), dispose: vi.fn(),
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock('@xterm/xterm', () => ({ Terminal: class { constructor() { return terminal; } } }));
vi.mock('@xterm/addon-fit', () => ({ FitAddon: class { fit() {} } }));
vi.mock('@xterm/addon-search', () => ({ SearchAddon: class {} }));
vi.mock('@xterm/addon-unicode11', () => ({ Unicode11Addon: class {} }));
vi.mock('@xterm/addon-web-links', () => ({ WebLinksAddon: class {} }));
vi.mock('@xterm/addon-webgl', () => ({ WebglAddon: class { onContextLoss() {} dispose() {} } }));
vi.mock('../../lib/theme', () => ({ terminalTheme: vi.fn(() => ({})) }));
vi.mock('../../lib/platform', async (original) => ({
  ...await original<typeof import('../../lib/platform')>(),
  isMac: true,
  primaryModifier: (e: KeyboardEvent) => e.metaKey && !e.ctrlKey,
}));
vi.mock('../ui', () => ({ Icon: () => null, showMenu: vi.fn() }));

const bay: BayDto = { id: 1, kind: 'shell', title: 'zsh', cwd: '/Users/developer', folder: 'developer', shell: 'zsh', agent: null };
const writeText = vi.fn();
const readText = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { callback(0); return 0; });
  vi.stubGlobal('navigator', Object.create(navigator));
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText, readText } });
  writeText.mockResolvedValue(undefined);
  readText.mockResolvedValue('synthetic paste');
  vi.mocked(invoke).mockReset().mockResolvedValueOnce({ windowsBuild: null }).mockResolvedValue([]);
  useUi.setState({ settings: null, toasts: [] });
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function open() {
  const view = render(<TerminalBay bay={bay} visible focused />);
  const key = terminal.attachCustomKeyEventHandler.mock.calls[0][0];
  return { ...view, key };
}

describe('Mac terminal clipboard', () => {
  it('copies with Command without stealing Control+C, and leaves Command+V for the native paste event', async () => {
    const { key } = open();
    expect(key(new KeyboardEvent('keydown', { key: 'c', ctrlKey: true }))).toBe(true);
    expect(writeText).not.toHaveBeenCalled();
    const copy = new KeyboardEvent('keydown', { key: 'c', metaKey: true, cancelable: true });
    expect(key(copy)).toBe(false);
    expect(copy.defaultPrevented).toBe(true);
    await waitFor(() => expect(terminal.clearSelection).toHaveBeenCalledTimes(1));
    expect(writeText).toHaveBeenCalledWith('synthetic terminal selection');
    const paste = new KeyboardEvent('keydown', { key: 'v', metaKey: true, cancelable: true });
    expect(key(paste)).toBe(false);
    expect(paste.defaultPrevented).toBe(false);
    expect(readText).not.toHaveBeenCalled();
  });

  it('keeps the selection and reports failed clipboard writes', async () => {
    writeText.mockRejectedValue(new Error('Clipboard denied'));
    const { key } = open();
    key(new KeyboardEvent('keydown', { key: 'c', metaKey: true }));
    await waitFor(() => expect(useUi.getState().toasts.some((t) => t.text === "Couldn't copy to the clipboard")).toBe(true));
    expect(terminal.clearSelection).not.toHaveBeenCalled();
  });

  it('pastes from the menu and reports clipboard read failures', async () => {
    const { container } = open();
    const host = container.querySelector('.term');
    if (!host) throw new Error('terminal host missing');
    fireEvent.contextMenu(host);
    const item = vi.mocked(showMenu).mock.calls[0][1].find((item) => 'label' in item && item.label === 'Paste');
    if (!item || !('onSelect' in item)) throw new Error('Paste menu item missing');
    await item.onSelect();
    expect(terminal.paste).toHaveBeenCalledWith('synthetic paste');
    readText.mockRejectedValue(new Error('Clipboard denied'));
    await item.onSelect();
    expect(useUi.getState().toasts.some((t) => t.text === "Couldn't paste from the clipboard")).toBe(true);
    expect(terminal.paste).toHaveBeenCalledTimes(1);
  });
});
