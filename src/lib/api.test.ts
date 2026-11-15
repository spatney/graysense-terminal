import { invoke } from '@tauri-apps/api/core';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { learnsIn, setProjectLearning } from './api';
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
