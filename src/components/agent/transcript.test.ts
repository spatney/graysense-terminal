import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { createElement } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act } from '../../lib/api';
import { applyUpdate, useUi } from '../../lib/store';
import type { AgentDto, BayDto, ItemDto, MemoryDto, ToolDto } from '../../lib/types';
import { Transcript, group } from './Transcript';

vi.mock('../../lib/api', async (original) => ({ ...(await original<typeof import('../../lib/api')>()), act: vi.fn() }));

const tool = (label: string, state: ToolDto['state'] = 'ok'): ToolDto => ({
  type: 'tool',
  name: 'powershell',
  label,
  verb: 'Run command',
  target: null,
  family: 'shell',
  state,
  progress: null,
  output: '',
  result: null,
  error: null,
  files: [],
  elapsed: 1,
  started: 0,
  mcpServer: null,
  hasApp: false,
  args: {},
  diff: null,
});

describe('transcript turns', () => {
  it('folds steps into a work log and keeps the last text as the answer', () => {
    const items: ItemDto[] = [
      { type: 'user', text: 'fix the build', files: [] },
      { type: 'assistant', text: 'Let me look.', done: true },
      tool('Run the build'),
      { type: 'assistant', text: 'Fixed: the import was wrong.', done: true },
    ];
    const blocks = group(items, false, true);
    expect(blocks.map((b) => b.kind)).toEqual(['user', 'agent']);
    const turn = blocks[1];
    if (turn.kind !== 'agent') throw new Error('expected an agent turn');
    expect(turn.answer?.item.text).toBe('Fixed: the import was wrong.');
    // The text written before the step is narration inside the log.
    expect(turn.parts.map((p) => p.t)).toEqual(['note', 'step']);
    expect(turn.live).toBe(false);
  });

  it('shows a pending approval as a card, and a live turn before anything arrives', () => {
    const items: ItemDto[] = [
      { type: 'user', text: 'deploy', files: [] },
      { type: 'permission', kind: 'commands', what: 'Run a command', summary: 'npm run deploy', headline: null, intention: null, command: 'npm run deploy', warning: null, fileName: null, diff: null, diffTruncated: false, paths: [], url: null, server: null, tool: null, toolDescription: null, args: null, canSession: true, pending: true, outcome: null },
    ];
    const blocks = group(items, true, true);
    const turn = blocks[1];
    if (turn.kind !== 'agent') throw new Error('expected an agent turn');
    expect(turn.parts[0].t).toBe('prompt');
    expect(turn.live).toBe(true);

    const waiting = group([{ type: 'user', text: 'hi', files: [] }], true, true);
    expect(waiting).toHaveLength(2);
    expect(waiting[1].kind === 'agent' && waiting[1].live).toBe(true);
  });

  it('counts a notice that comes again in a turn instead of repeating it', () => {
    const compacted: ItemDto = { type: 'notice', level: 'info', text: 'Context compacted to free up space' };
    const items: ItemDto[] = [
      { type: 'user', text: 'refactor it all', files: [] },
      compacted,
      tool('Run the build'),
      compacted,
      compacted,
      { type: 'notice', level: 'warn', text: 'Something else' },
      compacted,
    ];
    const turn = group(items, false, true)[1];
    if (turn.kind !== 'agent') throw new Error('expected an agent turn');
    const notices = turn.parts.flatMap((p) => (p.item.type === 'notice' ? [[p.item.text, p.n ?? 1]] : []));
    expect(notices).toEqual([
      ['Context compacted to free up space', 3],
      ['Something else', 1],
      ['Context compacted to free up space', 1],
    ]);
  });

  it('hides reasoning when it is turned off', () => {
    const items: ItemDto[] = [
      { type: 'user', text: 'q', files: [] },
      { type: 'reasoning', text: 'thinking…', done: true },
      { type: 'assistant', text: 'a', done: true },
    ];
    const turn = group(items, false, false)[1];
    expect(turn.kind === 'agent' && turn.parts.length).toBe(0);
  });

  it('times a turn by the whole of the work on its message, not just its steps', () => {
    const items: ItemDto[] = [
      { type: 'user', text: 'write tidy.py', files: [], took: 14_000 },
      tool('Create tidy.py'),
      { type: 'assistant', text: 'Done.', done: true },
      { type: 'user', text: 'thanks', files: [] },
      { type: 'assistant', text: 'Any time.', done: true },
    ];
    const blocks = group(items, false, true);
    expect(blocks.map((b) => (b.kind === 'agent' ? b.took : 'user'))).toEqual(['user', 14_000, 'user', null]);
  });
});

describe('a live turn', () => {
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it('says what the agent is doing once while it has no steps yet', () => {
    vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
    const bay: BayDto = {
      id: 1, kind: 'agent', title: 'Test', cwd: 'C:\\repo', folder: 'repo', shell: null,
      agent: {
        sessionId: 'test', model: null, effort: null, contextTier: null, mode: 'interactive',
        status: { kind: 'thinking', label: 'Thinking', detail: null, busy: true, attention: false },
        intent: null, name: null, deferred: false, incognito: false, tokensIn: 0, tokensOut: 0,
        context: null, toolCount: 0, turns: 0, changedFiles: [], busySince: Date.now(), mission: null,
        canvases: [], apps: [], attachments: [], autoApprove: false, pending: 0, memoryUsed: 0,
        subagentsActive: 0, project: 'repo', projectKey: 'repo', sentHistory: [], queued: [], canRewind: false, rewind: null, handoff: null, resumable: true,
      },
    };
    render(createElement(Transcript, { bay, items: [{ type: 'user', text: 'fix the flaky test', files: [] }], visible: true }));
    expect(screen.getAllByText('Thinking')).toHaveLength(1);
  });
});

describe('rewinding', () => {
  const settled = (canRewind: boolean): BayDto => ({
    id: 1, kind: 'agent', title: 'Test', cwd: 'C:\\repo', folder: 'repo', shell: null,
    agent: {
      sessionId: 'test', model: null, effort: null, contextTier: null, mode: 'interactive',
      status: { kind: 'idle', label: 'Ready', detail: null, busy: false, attention: false },
      intent: null, name: null, deferred: false, incognito: false, tokensIn: 0, tokensOut: 0,
      context: null, toolCount: 0, turns: 2, changedFiles: [], busySince: null, mission: null,
      canvases: [], apps: [], attachments: [], autoApprove: false, pending: 0, memoryUsed: 0,
      subagentsActive: 0, project: 'repo', projectKey: 'repo', sentHistory: [], queued: [], canRewind, rewind: null, handoff: null, resumable: true,
    } satisfies AgentDto,
  });
  const items: ItemDto[] = [
    { type: 'user', text: 'first', files: [], eventId: 'e1' },
    { type: 'assistant', text: 'one', done: true },
    { type: 'user', text: 'second', files: [], eventId: 'e2' },
    { type: 'assistant', text: 'two', done: true },
  ];
  const tips = (tip: string) => Array.from(document.querySelectorAll<HTMLElement>(`[data-tip="${tip}"]`));

  beforeEach(() => {
    vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
    vi.mocked(act).mockReset().mockResolvedValue(null);
    useUi.setState({ drafts: {}, focusTick: {} });
  });
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it('offers to undo any message and retry the last once the agent has settled', () => {
    render(createElement(Transcript, { bay: settled(true), items, visible: true }));
    expect(tips('Undo from here')).toHaveLength(2);
    expect(tips('Retry')).toHaveLength(1);
    fireEvent.click(tips('Retry')[0]);
    expect(act).toHaveBeenCalledWith({ type: 'rewindStart', bay: 1, index: 2, then: 'retry' });
    fireEvent.click(tips('Edit and send again')[0]);
    expect(act).toHaveBeenCalledWith({ type: 'rewindStart', bay: 1, index: 0, then: 'edit' });
  });

  it('only copies a message to the message box when the conversation can’t be rewound', () => {
    render(createElement(Transcript, { bay: settled(false), items, visible: true }));
    expect(tips('Undo from here')).toHaveLength(0);
    expect(tips('Retry')).toHaveLength(0);
    fireEvent.click(tips('Copy to the message box')[1]);
    expect(act).not.toHaveBeenCalled();
    expect(useUi.getState().drafts[1]).toBe('second');
  });

  it('forks from any message the runtime has, even while the agent works', () => {
    render(createElement(Transcript, { bay: settled(false), items: [...items, { type: 'user', text: 'third', files: [] }], visible: true }));
    expect(tips('Fork a new session from here')).toHaveLength(2);
    fireEvent.click(tips('Fork a new session from here')[1]);
    expect(act).toHaveBeenCalledWith({ type: 'fork', bay: 1, index: 2 });
  });

  it('puts an undone message above what is being written', () => {
    useUi.setState({ drafts: { 1: 'half a thought', 2: 'redo this' } });
    applyUpdate({ seq: 1, full: false, fills: [{ bay: 1, text: 'redo this' }, { bay: 2, text: 'redo this' }, { bay: 3, text: 'new' }] });
    const { drafts, focusTick } = useUi.getState();
    expect(drafts).toEqual({ 1: 'redo this\n\nhalf a thought', 2: 'redo this', 3: 'new' });
    expect(focusTick[1]).toBe(1);
  });
});

describe('what an agent was given from memory', () => {
  const memory = (id: number, text: string, patch: Partial<MemoryDto> = {}): MemoryDto => ({
    id, kind: 'fact', kindLabel: 'Fact', project: null, projectName: null, text, state: 'active', origin: 'learned',
    confidence: 0.9, pinned: false, proposal: null, created: 0, updated: 0, lastUsed: null, uses: 1, reinforced: 0,
    sources: [], history: [], commands: [], stale: false, userOwned: false, archivedReason: null, attachments: [],
    votes: 0, opens: 0, lastOpened: null, relevance: 0, ...patch,
  });
  const bay: BayDto = {
    id: 1, kind: 'agent', title: 'Test', cwd: 'C:\\repo', folder: 'repo', shell: null,
    agent: {
      sessionId: 'test', model: null, effort: null, contextTier: null, mode: 'interactive',
      status: { kind: 'idle', label: 'Ready', detail: null, busy: false, attention: false },
      intent: null, name: null, deferred: false, incognito: false, tokensIn: 0, tokensOut: 0,
      context: null, toolCount: 0, turns: 1, changedFiles: [], busySince: null, mission: null,
      canvases: [], apps: [], attachments: [], autoApprove: false, pending: 0, memoryUsed: 0,
      subagentsActive: 0, project: 'repo', projectKey: 'repo', sentHistory: [], queued: [], canRewind: false, rewind: null, handoff: null, resumable: true,
    },
  };
  const items: ItemDto[] = [
    { type: 'user', text: 'start the api', files: [] },
    { type: 'recall', entries: [{ id: 1, text: 'The API listens on port 8080' }, { id: 2, text: 'Tests need Docker' }, { id: 3, text: 'Uses yarn' }] },
    { type: 'assistant', text: 'Started.', done: true },
  ];
  const tips = (tip: string) => Array.from(document.querySelectorAll<HTMLElement>(`[data-tip="${tip}"]`));

  beforeEach(() => {
    vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
    vi.mocked(act).mockReset().mockResolvedValue(null);
    useUi.setState({
      memory: {
        memories: [memory(1, 'The API listens on port 8080'), memory(2, 'Tests need Docker', { pinned: true }), memory(3, 'Uses yarn', { state: 'archived', archivedReason: 'Marked wrong' })],
        lastRun: null,
        learning: 0,
        tidying: false,
        lastTidy: null,
      },
    });
  });
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it('lists them on demand, with a way to say one is wrong', () => {
    render(createElement(Transcript, { bay, items, visible: true }));
    expect(screen.queryByText('Tests need Docker')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /Used 3 memories/ }));
    expect(screen.getByText('Tests need Docker')).toBeTruthy();
    // Gone ones say why, and only a memory still saved can be flagged.
    expect(screen.getByText('Marked wrong')).toBeTruthy();
    expect(tips('Wrong or outdated: forget it, and tell the agent')).toHaveLength(2);
    fireEvent.click(tips('Wrong or outdated: forget it, and tell the agent')[0]);
    expect(act).toHaveBeenCalledWith({ type: 'memoryFlag', id: 1 });
    fireEvent.click(tips('Unpin')[0]);
    expect(act).toHaveBeenCalledWith({ type: 'memoryPin', id: 2, pinned: false });
    fireEvent.click(screen.getByRole('button', { name: 'Undo' }));
    expect(act).toHaveBeenCalledWith({ type: 'memoryRestore', id: 3 });
  });
});

describe('where a resumed session left off', () => {
  const resumed = (handoff: AgentDto['handoff'], busy = false): BayDto => ({
    id: 4, kind: 'agent', title: 'Login', cwd: 'C:\\repo', folder: 'repo', shell: null,
    agent: {
      sessionId: 'abc', model: null, effort: null, contextTier: null, mode: 'interactive',
      status: { kind: busy ? 'thinking' : 'idle', label: busy ? 'Thinking' : 'Ready', detail: null, busy, attention: false },
      intent: null, name: null, deferred: false, incognito: false, tokensIn: 0, tokensOut: 0,
      context: null, toolCount: 0, turns: 1, changedFiles: [], busySince: null, mission: null,
      canvases: [], apps: [], attachments: [], autoApprove: false, pending: 0, memoryUsed: 0,
      subagentsActive: 0, project: 'repo', projectKey: 'repo', sentHistory: [], queued: [], canRewind: false, rewind: null, handoff, resumable: true,
    },
  });
  const items: ItemDto[] = [
    { type: 'user', text: 'move login to device codes', files: [] },
    { type: 'assistant', text: 'Started on it.', done: true },
  ];
  const handoff = { summary: 'Started moving the login flow to OAuth device codes.', next: 'Add a regression test for the token refresh', at: Date.now() - 2 * 86_400_000 };

  beforeEach(() => {
    vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
    vi.mocked(act).mockReset().mockResolvedValue(null);
    useUi.setState({ drafts: { 4: 'and keep the old flow' }, focusTick: {} });
  });
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it('offers the next step, above what is being written', () => {
    render(createElement(Transcript, { bay: resumed(handoff), items, visible: true }));
    expect(screen.getByText('Where you left off')).toBeTruthy();
    expect(screen.getByText('2 days ago')).toBeTruthy();
    expect(screen.getByText(handoff.summary)).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    expect(useUi.getState().drafts[4]).toBe('Add a regression test for the token refresh\n\nand keep the old flow');
    expect(useUi.getState().focusTick[4]).toBe(1);
    expect(act).toHaveBeenCalledWith({ type: 'handoffDismiss', bay: 4 });
  });

  it('only summarises finished work, and stays out of the way while the agent works', () => {
    render(createElement(Transcript, { bay: resumed({ ...handoff, next: null }), items, visible: true }));
    expect(screen.queryByRole('button', { name: 'Continue' })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(act).toHaveBeenCalledWith({ type: 'handoffDismiss', bay: 4 });
    cleanup();
    render(createElement(Transcript, { bay: resumed(handoff, true), items, visible: true }));
    expect(screen.queryByText('Where you left off')).toBeNull();
  });
});
