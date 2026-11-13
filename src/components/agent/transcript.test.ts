import { describe, expect, it } from 'vitest';
import type { ItemDto, ToolDto } from '../../lib/types';
import { group } from './Transcript';

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
});

describe('transcript turns', () => {
  it('folds steps into a work log and keeps the last text as the answer', () => {
    const items: ItemDto[] = [
      { type: 'user', text: 'fix the build', queued: false, files: [] },
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
      { type: 'user', text: 'deploy', queued: false, files: [] },
      { type: 'permission', kind: 'commands', what: 'Run a command', summary: 'npm run deploy', headline: null, intention: null, command: 'npm run deploy', warning: null, fileName: null, diff: null, paths: [], url: null, server: null, tool: null, toolDescription: null, args: null, canSession: true, pending: true, outcome: null },
    ];
    const blocks = group(items, true, true);
    const turn = blocks[1];
    if (turn.kind !== 'agent') throw new Error('expected an agent turn');
    expect(turn.parts[0].t).toBe('prompt');
    expect(turn.live).toBe(true);

    const waiting = group([{ type: 'user', text: 'hi', queued: false, files: [] }], true, true);
    expect(waiting).toHaveLength(2);
    expect(waiting[1].kind === 'agent' && waiting[1].live).toBe(true);
  });

  it('hides reasoning when it is turned off', () => {
    const items: ItemDto[] = [
      { type: 'user', text: 'q', queued: false, files: [] },
      { type: 'reasoning', text: 'thinking…', done: true },
      { type: 'assistant', text: 'a', done: true },
    ];
    const turn = group(items, false, false)[1];
    expect(turn.kind === 'agent' && turn.parts.length).toBe(0);
  });

  it('times a turn by the whole of the work on its message, not just its steps', () => {
    const items: ItemDto[] = [
      { type: 'user', text: 'write tidy.py', queued: false, files: [], took: 14_000 },
      tool('Create tidy.py'),
      { type: 'assistant', text: 'Done.', done: true },
      { type: 'user', text: 'thanks', queued: false, files: [] },
      { type: 'assistant', text: 'Any time.', done: true },
    ];
    const blocks = group(items, false, true);
    expect(blocks.map((b) => (b.kind === 'agent' ? b.took : 'user'))).toEqual(['user', 14_000, 'user', null]);
  });
});
