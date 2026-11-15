import { invoke } from '@tauri-apps/api/core';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { learnsIn, setProjectLearning, startInWorktree } from './api';
import { useUi } from './store';
import type { Settings } from './types';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));

const off = (keys: string[]) => useUi.setState({ settings: { 'memory.no_learn_projects': keys } as Settings, toasts: [] });
const sent = () => vi.mocked(invoke).mock.calls.map((c) => (c[1] as { action: unknown }).action);

afterEach(() => {
  useUi.setState({ settings: null, toasts: [] });
  vi.mocked(invoke).mockClear();
});

describe('learning per project', () => {
  it('turns learning off in one project, keeping the others, and offers to undo it', async () => {
    off(['c:\\client']);
    expect(learnsIn(useUi.getState().settings, 'c:\\code\\app')).toBe(true);
    await setProjectLearning('c:\\code\\app', false, 'app');
    expect(sent()).toEqual([{ type: 'setSettings', changes: { 'memory.no_learn_projects': ['c:\\client', 'c:\\code\\app'] } }]);
    const [t] = useUi.getState().toasts;
    expect(t.text).toContain("won't learn from app");
    expect(t.action).toEqual(['Undo', { type: 'setSettings', changes: { 'memory.no_learn_projects': ['c:\\client'] } }]);
  });

  it('turns it back on quietly from the panel', async () => {
    off(['c:\\client', 'c:\\code\\app']);
    expect(learnsIn(useUi.getState().settings, 'c:\\code\\app')).toBe(false);
    await setProjectLearning('c:\\code\\app', true);
    expect(sent()).toEqual([{ type: 'setSettings', changes: { 'memory.no_learn_projects': ['c:\\client'] } }]);
    expect(useUi.getState().toasts).toEqual([]);
  });
});

describe('starting over in a worktree', () => {
  it('gives the message being written to the session that takes its place', async () => {
    useUi.setState({ drafts: { 4: 'fix the login flow', 5: 'something else' }, focusTick: {} });
    vi.mocked(invoke).mockResolvedValueOnce(9);
    await startInWorktree(4);
    expect(sent()).toEqual([{ type: 'startInWorktree', bay: 4 }]);
    expect(useUi.getState().drafts).toEqual({ 5: 'something else', 9: 'fix the login flow' });
    expect(useUi.getState().focusTick).toEqual({ 9: 1 });
  });

  it('keeps it where it is when the session can’t start over', async () => {
    useUi.setState({ drafts: { 4: 'fix the login flow' }, focusTick: {} });
    vi.mocked(invoke).mockRejectedValueOnce('This session has started already: fork it into a worktree instead.');
    await startInWorktree(4);
    expect(useUi.getState().drafts).toEqual({ 4: 'fix the login flow' });
    expect(useUi.getState().toasts.map((t) => t.text)).toEqual(['This session has started already: fork it into a worktree instead.']);
  });
});
