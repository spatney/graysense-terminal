import { cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, toggleSide } from './api';
import { openOverlay, useUi } from './store';
import { useShortcuts } from './shortcuts';

const platform = vi.hoisted(() => ({ mac: false }));
vi.mock('./platform', () => ({
  get isMac() { return platform.mac; },
  primaryModifier: (e: KeyboardEvent) => platform.mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey,
  bayNumber: (e: KeyboardEvent) => /^Digit([1-9])$/.exec(e.code)?.[1] ? Number(e.code.slice(-1)) : /^[1-9]$/.test(e.key) ? Number(e.key) : null,
}));
vi.mock('./api', () => ({
  act: vi.fn(), captureNote: vi.fn(), closeBay: vi.fn(), newSession: vi.fn(), newShell: vi.fn(), setSide: vi.fn(), toggleSide: vi.fn(),
}));
vi.mock('../components/Overlays', () => ({ captureIdea: vi.fn(), toggleSidePanel: vi.fn(), zoomBy: vi.fn() }));

function Harness() {
  useShortcuts();
  return <><textarea aria-label="Message" /><div className="xterm"><textarea aria-label="Terminal" /></div></>;
}

beforeEach(() => {
  vi.clearAllMocks();
  useUi.setState({ overlay: null, bays: [] });
});
afterEach(cleanup);

describe.each([true, false])('app shortcuts (Mac=%s)', (mac) => {
  it('opens search with the native modifier while leaving text editing and terminal controls alone', () => {
    platform.mac = mac;
    const { getByLabelText } = render(<Harness />);
    const field = getByLabelText('Message');
    fireEvent.keyDown(field, { key: 'k', ...(mac ? { metaKey: true } : { ctrlKey: true }) });
    expect(useUi.getState().overlay).toEqual({ kind: 'search' });
    openOverlay(null);
    for (const key of ['c', 'v', 'a', 'z']) {
      expect(fireEvent.keyDown(field, { key, ...(mac ? { metaKey: true } : { ctrlKey: true }) })).toBe(true);
    }
    fireEvent.keyDown(getByLabelText('Terminal'), { key: 'k', ctrlKey: true });
    expect(useUi.getState().overlay).toBeNull();
    expect(act).not.toHaveBeenCalled();
  });

  it('uses a non-conflicting Mac history shortcut', () => {
    platform.mac = mac;
    render(<Harness />);
    fireEvent.keyDown(window, { key: mac ? 'y' : 'h', shiftKey: true, ...(mac ? { metaKey: true } : { ctrlKey: true }) });
    expect(toggleSide).toHaveBeenCalledWith('sessions');
    if (mac) {
      vi.mocked(toggleSide).mockClear();
      fireEvent.keyDown(window, { key: 'h', shiftKey: true, metaKey: true });
      expect(toggleSide).not.toHaveBeenCalled();
    }
  });
});
