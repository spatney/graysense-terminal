import { afterEach, describe, expect, it, vi } from 'vitest';
import { draftKeys, installDrafts, keptDrafts } from './drafts';
import { useUi } from './store';
import type { BayDto } from './types';

const agent = (id: number, cwd: string, a: { sessionId?: string; resumable?: boolean; incognito?: boolean } = {}) =>
  ({ id, kind: 'agent', cwd, agent: { sessionId: a.sessionId ?? null, resumable: !!a.resumable, incognito: !!a.incognito } }) as unknown as BayDto;
const shell = (id: number) => ({ id, kind: 'shell', cwd: 'C:\\code', agent: null }) as unknown as BayDto;
const DAY = 24 * 3600 * 1000;

afterEach(() => vi.useRealTimers());

describe('where drafts are kept', () => {
  it('keys a session that reopens by its id, and a new one by its folder', () => {
    const keys = draftKeys([
      agent(1, 'C:\\code\\api', { sessionId: 'abc', resumable: true }),
      agent(2, 'C:/Code/App/', { sessionId: 'new' }),
      agent(3, 'c:\\code\\app'),
      agent(4, 'C:\\secret', { incognito: true }),
      shell(5),
    ]);
    expect([...keys]).toEqual([
      [0, 'home'],
      [1, 's:abc'],
      [2, 'f:c:\\code\\app'],
      [3, 'f:c:\\code\\app#2'],
    ]);
  });

  it("keeps open boxes' drafts and closed sessions', dropping sent ones and the oldest", () => {
    const now = 100 * DAY;
    const saved = {
      's:closed': { text: 'come back to this', at: now - DAY },
      's:stale': { text: 'old', at: now - 31 * DAY },
      'f:c:\\gone': { text: 'its bay was closed', at: now - DAY },
      's:sent': { text: 'was sent', at: now - DAY },
      's:same': { text: 'unchanged', at: now - 2 * DAY },
    };
    const keys = new Map([
      [0, 'home'],
      [1, 's:sent'],
      [2, 's:same'],
      [3, 'f:c:\\new'],
    ]);
    const kept = keptDrafts(saved, keys, { 0: '  ', 1: '', 2: 'unchanged', 3: 'fresh' }, now);
    expect(kept).toEqual({ 'f:c:\\new': { text: 'fresh', at: now }, 's:closed': { text: 'come back to this', at: now - DAY }, 's:same': { text: 'unchanged', at: now - 2 * DAY } });
    const many = Object.fromEntries(Array.from({ length: 120 }, (_, i) => [`s:${i}`, { text: 't', at: now - i * 1000 }]));
    expect(Object.keys(keptDrafts(many, new Map(), {}, now))).toHaveLength(100);
    expect(keptDrafts(many, new Map(), {}, now)['s:99']).toBeDefined();
  });
});

describe('drafts across restarts', () => {
  it('puts drafts back in the restored bays and a reopened session, and saves new ones', () => {
    vi.useFakeTimers();
    const now = Date.now();
    localStorage.setItem(
      'gs-drafts',
      JSON.stringify({
        home: { text: 'start something', at: now },
        's:abc': { text: 'half a thought', at: now },
        'f:c:\\code\\app': { text: 'set up the project', at: now },
        's:later': { text: 'for when it reopens', at: now },
      }),
    );
    useUi.setState({ ready: false, bays: [], drafts: {} });
    installDrafts();
    expect(useUi.getState().drafts[0]).toBe('start something');

    useUi.setState({ ready: true, bays: [agent(1, 'C:\\code\\api', { sessionId: 'abc', resumable: true }), agent(2, 'C:\\code\\app')] });
    expect(useUi.getState().drafts).toMatchObject({ 1: 'half a thought', 2: 'set up the project' });

    // A new bay in a folder that had a draft doesn't take it; a reopened session gets its own.
    useUi.setState((s) => ({ bays: [agent(3, 'C:\\code\\app'), ...s.bays, agent(4, 'C:\\x', { sessionId: 'later', resumable: true })] }));
    expect(useUi.getState().drafts[3]).toBeUndefined();
    expect(useUi.getState().drafts[4]).toBe('for when it reopens');

    useUi.setState((s) => ({ drafts: { ...s.drafts, 1: '', 4: 'for when it reopens, edited' } }));
    vi.advanceTimersByTime(600);
    const saved = JSON.parse(localStorage.getItem('gs-drafts')!);
    expect(Object.keys(saved).sort()).toEqual(['f:c:\\code\\app#2', 'home', 's:later']);
    expect(saved['s:later'].text).toBe('for when it reopens, edited');
    expect(saved['f:c:\\code\\app#2'].text).toBe('set up the project');
  });
});
