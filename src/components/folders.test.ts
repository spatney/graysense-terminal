import { afterEach, describe, expect, it, vi } from 'vitest';
import { setProjectLearning } from '../lib/api';
import { useUi } from '../lib/store';
import type { AppDto, BayDto, Settings } from '../lib/types';
import { agentFolderItems, folderSubmenu } from './folders';
import type { MenuItem } from './ui';

vi.mock('../lib/api', async (original) => ({
  ...(await original<typeof import('../lib/api')>()),
  act: vi.fn(),
  changeFolder: vi.fn(),
  newShell: vi.fn(),
  setSide: vi.fn(),
  setProjectLearning: vi.fn(),
}));

const recent = (dirs: string[]) => useUi.setState({ app: { home: 'C:\\Users\\me' } as AppDto, settings: { recent_dirs: dirs } as Settings });

afterEach(() => useUi.setState({ app: null, settings: null }));

describe('recent folder menus', () => {
  it('lists recent folders by name and place, newest first, leaving out the current one', () => {
    recent(['C:\\Users\\me\\code\\app', 'D:\\', 'C:\\work\\api']);
    const open = vi.fn();
    const item = folderSubmenu('Change folder', 'arrow-swap', open, vi.fn(), 'c:\\WORK\\api\\');
    const rows = 'submenu' in item ? item.submenu : [];
    const shown = rows.map((r: MenuItem) => ('label' in r ? [r.label, 'detail' in r ? r.detail : undefined] : 'sep'));
    expect(shown).toEqual([['app', '~\\code'], ['D:', undefined], 'sep', ['Choose a folder…', undefined]]);
    const first = rows[0];
    if ('onSelect' in first) first.onSelect();
    expect(open).toHaveBeenCalledWith('C:\\Users\\me\\code\\app');
  });

  it('goes straight to the folder picker before there are recent folders', () => {
    recent([]);
    const choose = vi.fn();
    const item = folderSubmenu('New terminal in folder', 'folder', vi.fn(), choose);
    expect(item).toMatchObject({ label: 'New terminal in folder…' });
    if ('onSelect' in item) item.onSelect();
    expect(choose).toHaveBeenCalledTimes(1);
  });
});

describe('learning from a folder', () => {
  const bay = (incognito = false) => ({ id: 1, kind: 'agent', cwd: 'C:\\code\\app\\src', agent: { deferred: false, incognito, project: 'app', projectKey: 'c:\\code\\app' } }) as unknown as BayDto;
  const memory = (enabled: boolean, off: string[]) => useUi.setState({ settings: { recent_dirs: [], 'memory.enabled': enabled, 'memory.no_learn_projects': off } as unknown as Settings });
  const learning = (items: MenuItem[]) => items.filter((i) => 'label' in i && /learn/i.test(i.label));

  it("offers to stop learning from the agent's project, then to learn there again", () => {
    memory(true, []);
    const [item] = learning(agentFolderItems(bay()));
    expect(item).toMatchObject({ label: "Don't learn from app" });
    if ('onSelect' in item) item.onSelect();
    expect(setProjectLearning).toHaveBeenCalledWith('c:\\code\\app', false, 'app');
    memory(true, ['c:\\code\\app']);
    expect(learning(agentFolderItems(bay()))).toMatchObject([{ label: 'Learn from app again' }]);
  });

  it('leaves it out for incognito sessions and while memory is off', () => {
    memory(true, []);
    expect(learning(agentFolderItems(bay(true)))).toEqual([]);
    memory(false, []);
    expect(learning(agentFolderItems(bay()))).toEqual([]);
  });
});
