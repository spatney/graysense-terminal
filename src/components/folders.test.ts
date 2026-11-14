import { afterEach, describe, expect, it, vi } from 'vitest';
import { useUi } from '../lib/store';
import type { AppDto, Settings } from '../lib/types';
import { folderSubmenu } from './folders';
import type { MenuItem } from './ui';

vi.mock('../lib/api', () => ({ act: vi.fn(), changeFolder: vi.fn(), newShell: vi.fn(), setSide: vi.fn() }));

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
