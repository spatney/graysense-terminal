import { act as reactAct, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, completePath, newSession, savePastedImage } from '../../lib/api';
import { useUi } from '../../lib/store';
import type { BayDto, DirEntry } from '../../lib/types';
import { Composer } from './Composer';

vi.mock('../../lib/api', () => ({
  act: vi.fn(),
  completePath: vi.fn(),
  newSession: vi.fn(),
  newShell: vi.fn(),
  pickFiles: vi.fn(),
  pickFolder: vi.fn(),
  savePastedImage: vi.fn(),
}));
vi.mock('./ModelPicker', () => ({
  ModelPicker: () => null,
  useModelChoice: () => ({ name: 'Auto', label: 'Auto', model: null, effort: null }),
}));
vi.mock('../ui', () => ({
  Icon: () => null,
  Seg: () => null,
  showMenu: vi.fn(),
  useAutosize: vi.fn(),
}));

const bay: BayDto = {
  id: 1, kind: 'agent', title: 'Test', cwd: 'C:\\repo', folder: 'repo', shell: null,
  agent: {
    sessionId: 'test', model: null, effort: null, contextTier: null, mode: 'interactive',
    status: { kind: 'thinking', label: 'Thinking', detail: null, busy: true, attention: false },
    intent: null, name: null, deferred: false, incognito: false, tokensIn: 0, tokensOut: 0,
    context: null, toolCount: 0, turns: 0, changedFiles: [], busySince: null, mission: null,
    canvases: [], apps: [], attachments: [], autoApprove: false, pending: 0, memoryUsed: 0,
    subagentsActive: 0, project: 'repo', projectKey: 'repo', sentHistory: [], queued: [],
  },
};
const entry = (name: string): DirEntry => ({ name, path: `C:\\repo\\${name}`, isDir: false, size: 0, modified: 0 });

function type(text: string) {
  const input = screen.getByRole<HTMLTextAreaElement>('combobox', { name: 'Message' });
  fireEvent.focus(input);
  fireEvent.change(input, { target: { value: text, selectionStart: text.length, selectionEnd: text.length } });
  return input;
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(act).mockResolvedValue(null);
  vi.mocked(completePath).mockResolvedValue([]);
  vi.mocked(newSession).mockResolvedValue(2);
  useUi.setState({ drafts: {}, app: null, focusTick: {}, toasts: [] });
  Element.prototype.scrollIntoView = vi.fn();
});
afterEach(cleanup);

describe('composer file completion', () => {
  it('selects with the keyboard without sending, then attaches the file before sending', async () => {
    vi.mocked(completePath).mockResolvedValue([entry('a.ts'), entry('My File.ts')]);
    render(<Composer bay={bay} />);
    const input = type('Read @');
    await screen.findByRole('option', { name: 'a.ts' });
    expect(completePath).toHaveBeenCalledWith('C:\\repo\\', '');
    fireEvent.keyDown(input, { key: 'ArrowDown' });
    fireEvent.keyDown(input, { key: 'Enter', isComposing: true });
    expect(useUi.getState().drafts[1]).toBe('Read @');
    fireEvent.keyDown(input, { key: 'Tab' });
    await waitFor(() => expect(input.value).toBe('Read @"C:\\repo\\My File.ts" '));
    expect(act).toHaveBeenCalledTimes(0);
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(act).toHaveBeenCalledTimes(2));
    expect(vi.mocked(act).mock.calls.map(([action]) => action)).toEqual([
      { type: 'attach', bay: 1, paths: ['C:\\repo\\My File.ts'] },
      { type: 'send', bay: 1, text: 'Read @"C:\\repo\\My File.ts"', queue: false },
    ]);
  });

  it('dismisses the picker without stopping a running agent or losing the draft', async () => {
    vi.mocked(completePath).mockResolvedValue([entry('a.ts')]);
    render(<Composer bay={bay} />);
    const input = type('@a');
    await screen.findByRole('option');
    fireEvent.keyDown(input, { key: 'Escape' });
    expect(screen.queryByRole('listbox')).toBeNull();
    expect(useUi.getState().drafts[1]).toBe('@a');
    expect(act).toHaveBeenCalledTimes(0);
  });

  it('ignores stale requests and shows directory errors instead of stale matches', async () => {
    let finishOld!: (entries: DirEntry[]) => void;
    vi.mocked(completePath).mockImplementation((_dir, prefix) => {
      if (prefix === 'old') return new Promise((resolve) => { finishOld = resolve; });
      if (prefix === 'bad') return Promise.reject(new Error('Access denied'));
      return Promise.resolve([entry('new.ts')]);
    });
    render(<Composer bay={bay} />);
    type('@old');
    await waitFor(() => expect(completePath).toHaveBeenCalledWith('C:\\repo\\', 'old'));
    type('@new');
    await screen.findByRole('option', { name: 'new.ts' });
    await reactAct(async () => finishOld([entry('old.ts')]));
    expect(screen.queryByRole('option', { name: 'old.ts' })).toBeNull();
    type('@bad');
    await screen.findByText('Error: Access denied');
    expect(screen.queryByRole('option')).toBeNull();
  });

  it('keeps the draft and does not send if a mentioned file cannot be attached', async () => {
    vi.mocked(act).mockResolvedValue(undefined);
    render(<Composer bay={bay} />);
    const text = 'Read @"C:\\repo\\missing.ts"';
    fireEvent.keyDown(type(text), { key: 'Enter' });
    await waitFor(() => expect(act).toHaveBeenCalledTimes(1));
    expect(vi.mocked(act).mock.calls[0][0].type).toBe('attach');
    expect(useUi.getState().drafts[1]).toBe(text);
  });

  it('carries home-screen file mentions into the new session before sending', async () => {
    render(<Composer bay={null} />);
    const text = 'Read @"C:\\repo\\a.ts"';
    fireEvent.keyDown(type(text), { key: 'Enter' });
    await waitFor(() => expect(act).toHaveBeenCalledTimes(2));
    expect(newSession).toHaveBeenCalledWith();
    expect(vi.mocked(act).mock.calls.map(([action]) => action)).toEqual([
      { type: 'attach', bay: 2, paths: ['C:\\repo\\a.ts'] },
      { type: 'send', bay: 2, text, queue: false },
    ]);
    expect(useUi.getState().drafts[0]).toBe('');
    expect(useUi.getState().drafts[2]).toBe('');
  });
});

describe('composer steering', () => {
  it('steers a working agent with Enter and queues with Ctrl+Enter', async () => {
    render(<Composer bay={bay} />);
    fireEvent.keyDown(type('Use tabs'), { key: 'Enter' });
    await waitFor(() => expect(act).toHaveBeenCalledWith({ type: 'send', bay: 1, text: 'Use tabs', queue: false }));
    fireEvent.keyDown(type('Then run it'), { key: 'Enter', ctrlKey: true });
    await waitFor(() => expect(act).toHaveBeenCalledWith({ type: 'send', bay: 1, text: 'Then run it', queue: true }));
  });
});

describe('composer image paste', () => {
  const shot = 'C:\\data\\pasted-images\\pasted-image-1.png';
  const image = (type = 'image/png') => new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], 'image.png', { type });
  // jsdom has no DataTransfer; React reads the clipboardData the event is given.
  const paste = (files: File[], text = '') =>
    fireEvent.paste(screen.getByRole('combobox', { name: 'Message' }), { clipboardData: { getData: () => text, files, items: [] } });

  it('keeps a pasted screenshot and attaches it to the next message', async () => {
    vi.mocked(savePastedImage).mockResolvedValue(shot);
    render(<Composer bay={bay} />);
    expect(paste([image()])).toBe(false);
    await waitFor(() => expect(act).toHaveBeenCalledWith({ type: 'attach', bay: 1, paths: [shot] }));
    expect(Array.from(vi.mocked(savePastedImage).mock.calls[0][0])).toEqual([0x89, 0x50, 0x4e, 0x47]);
  });

  it('pastes text as text, even with a picture alongside', () => {
    render(<Composer bay={bay} />);
    expect(paste([image()], 'A1\tB1')).toBe(true);
    expect(savePastedImage).toHaveBeenCalledTimes(0);
  });

  it('opens one session for images pasted on the home screen and carries the draft over', async () => {
    vi.mocked(savePastedImage).mockResolvedValue(shot);
    render(<Composer bay={null} />);
    type('What is wrong here?');
    paste([image()]);
    paste([image()]);
    await waitFor(() => expect(act).toHaveBeenCalledTimes(2));
    expect(newSession).toHaveBeenCalledTimes(1);
    for (const [action] of vi.mocked(act).mock.calls) expect(action).toEqual({ type: 'attach', bay: 2, paths: [shot] });
    expect(useUi.getState().drafts).toEqual({ 0: '', 2: 'What is wrong here?' });
  });

  it('reports an image it cannot keep without opening a session', async () => {
    vi.mocked(savePastedImage).mockRejectedValue('Graysense can paste PNG, JPEG, GIF and WebP images.');
    render(<Composer bay={null} />);
    paste([image('image/svg+xml')]);
    await waitFor(() => expect(useUi.getState().toasts.map((t) => t.text)).toEqual(['Graysense can paste PNG, JPEG, GIF and WebP images.']));
    expect(newSession).toHaveBeenCalledTimes(0);
    expect(act).toHaveBeenCalledTimes(0);
  });
});
