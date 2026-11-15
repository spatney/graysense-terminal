import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act } from '../../lib/api';
import { useUi } from '../../lib/store';
import type { Settings } from '../../lib/types';
import { MemoryPanel } from './MemoryPanel';

vi.mock('../../lib/api', async (original) => ({ ...(await original<typeof import('../../lib/api')>()), act: vi.fn() }));

const settings = { 'memory.enabled': true, 'memory.learn': true, 'memory.share': true, 'memory.review_all': false, 'memory.learning_model': null, 'memory.tidy': true } as unknown as Settings;

function show(tidying: boolean, lastTidy: number | null = null) {
  useUi.setState({ settings, memory: { memories: [], lastRun: null, learning: 0, tidying, lastTidy } });
  render(<MemoryPanel />);
  fireEvent.click(screen.getByRole('button', { name: 'Memory settings' }));
}

describe('tidying memory', () => {
  beforeEach(() => {
    vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
    Element.prototype.scrollTo ??= () => {};
    localStorage.setItem('gs-memory-tab', 'learned');
    vi.mocked(act).mockReset().mockResolvedValue(null);
  });
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it('tidies on request and can be switched off', () => {
    show(false, Math.floor(Date.now() / 1000) - 3 * 86400);
    expect(screen.getByText('Last tidied 3 days ago')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Tidy up now' }));
    expect(act).toHaveBeenCalledWith({ type: 'memoryTidy' });
    fireEvent.click(screen.getByRole('switch', { name: 'Tidy up every week' }));
    expect(act).toHaveBeenCalledWith({ type: 'setSettings', changes: { 'memory.tidy': false } });
  });

  it('shows it working, and is not asked twice', () => {
    show(true);
    expect(screen.getByRole('status').textContent).toContain('Tidying up');
    const busy = screen.getByRole<HTMLButtonElement>('button', { name: 'Tidying…' });
    expect(busy.disabled).toBe(true);
  });
});
