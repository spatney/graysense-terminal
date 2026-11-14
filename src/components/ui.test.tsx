import { act as reactAct, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MenuHost, type MenuItem, closeMenu, showMenu } from './ui';

afterEach(() => {
  reactAct(() => closeMenu());
  cleanup();
  vi.useRealTimers();
});

function open(items: MenuItem[]) {
  render(<MenuHost />);
  reactAct(() => showMenu({ clientX: 10, clientY: 10 }, items));
  return screen.getByRole('menu');
}

function folders(pickApp: () => void, choose: () => void, other: () => void): MenuItem[] {
  return [
    { label: 'New session', onSelect: other },
    { label: 'New agent in folder', submenu: [{ label: 'app', detail: '~\\code', onSelect: pickApp }, { sep: true }, { label: 'Choose a folder…', onSelect: choose }] },
    { label: 'New terminal', onSelect: other },
  ];
}

describe('menus with a submenu', () => {
  it('opens it from the keyboard, steps through it, and runs the chosen item', () => {
    const [pickApp, choose, other] = [vi.fn(), vi.fn(), vi.fn()];
    const menu = open(folders(pickApp, choose, other));
    fireEvent.keyDown(menu, { key: 'ArrowDown' });
    fireEvent.keyDown(menu, { key: 'ArrowDown' });
    fireEvent.keyDown(menu, { key: 'ArrowRight' });
    expect(screen.getAllByRole('menu')).toHaveLength(2);
    // The separator is skipped both ways.
    fireEvent.keyDown(menu, { key: 'ArrowDown' });
    fireEvent.keyDown(menu, { key: 'ArrowDown' });
    fireEvent.keyDown(menu, { key: 'Enter' });
    expect(pickApp).toHaveBeenCalledTimes(1);
    expect(choose).not.toHaveBeenCalled();
    expect(other).not.toHaveBeenCalled();
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('goes back to the menu with ArrowLeft, and Escape closes one level at a time', () => {
    const [pickApp, choose, other] = [vi.fn(), vi.fn(), vi.fn()];
    const menu = open(folders(pickApp, choose, other));
    fireEvent.keyDown(menu, { key: 'ArrowDown' });
    fireEvent.keyDown(menu, { key: 'ArrowDown' });
    fireEvent.keyDown(menu, { key: 'Enter' });
    expect(screen.getAllByRole('menu')).toHaveLength(2);
    fireEvent.keyDown(menu, { key: 'ArrowLeft' });
    expect(screen.getAllByRole('menu')).toHaveLength(1);
    fireEvent.keyDown(menu, { key: 'ArrowDown' });
    fireEvent.keyDown(menu, { key: 'Enter' });
    expect(other).toHaveBeenCalledTimes(1);

    const again = folders(pickApp, choose, other);
    reactAct(() => showMenu({ clientX: 10, clientY: 10 }, again));
    fireEvent.click(screen.getByRole('menuitem', { name: /New agent in folder/ }));
    const root = screen.getAllByRole('menu')[0];
    fireEvent.keyDown(root, { key: 'Escape' });
    expect(screen.getAllByRole('menu')).toHaveLength(1);
    fireEvent.keyDown(root, { key: 'Escape' });
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('opens when the pointer rests on its row and closes when it rests on another', () => {
    vi.useFakeTimers();
    const [pickApp, choose, other] = [vi.fn(), vi.fn(), vi.fn()];
    open(folders(pickApp, choose, other));
    const row = screen.getByRole('menuitem', { name: /New agent in folder/ });
    fireEvent.pointerEnter(row);
    expect(screen.getAllByRole('menu')).toHaveLength(1);
    reactAct(() => vi.advanceTimersByTime(100));
    expect(screen.getAllByRole('menu')).toHaveLength(2);
    expect(row.getAttribute('aria-expanded')).toBe('true');

    // Passing over another row on the way to the submenu doesn't close it...
    fireEvent.pointerEnter(screen.getByRole('menuitem', { name: 'New terminal' }));
    reactAct(() => vi.advanceTimersByTime(100));
    fireEvent.pointerEnter(screen.getAllByRole('menu')[1]);
    reactAct(() => vi.advanceTimersByTime(400));
    expect(screen.getAllByRole('menu')).toHaveLength(2);
    // ...resting there does.
    fireEvent.pointerEnter(screen.getByRole('menuitem', { name: 'New session' }));
    reactAct(() => vi.advanceTimersByTime(400));
    expect(screen.getAllByRole('menu')).toHaveLength(1);

    fireEvent.click(row);
    fireEvent.click(screen.getByRole('menuitem', { name: /Choose a folder/ }));
    expect(choose).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('menu')).toBeNull();
  });
});
