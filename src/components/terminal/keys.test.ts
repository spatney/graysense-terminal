import { afterEach, describe, expect, it, vi } from 'vitest';

afterEach(() => {
  vi.restoreAllMocks();
  vi.resetModules();
});

describe.each(['MacIntel', 'Win32'])('terminal keys on %s', (platform) => {
  it('keeps shell control keys separate from copy, paste, find and app shortcuts', async () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue(platform);
    const { terminalKeyAction } = await import('./keys');
    const mac = platform === 'MacIntel';
    const action = (key: string, modifiers: KeyboardEventInit, selected = false) =>
      terminalKeyAction(new KeyboardEvent('keydown', { key, ...modifiers }), selected);
    const primary = mac ? { metaKey: true } : { ctrlKey: true };
    expect(action('c', { ctrlKey: true })).toBeNull();
    expect(action('c', { ctrlKey: true }, true)).toBe(mac ? null : 'copy');
    expect(action('c', { ...primary, shiftKey: true }, true)).toBe('copy');
    expect(action('c', primary, true)).toBe('copy');
    expect(action('v', primary)).toBe('paste');
    expect(action('f', primary)).toBe('find');
    for (const key of ['a', 'd', 'k', 'z']) expect(action(key, { ctrlKey: true })).toBeNull();
    expect(action('a', { metaKey: true })).toBe(mac ? 'selectAll' : null);
    expect(action('Tab', { ctrlKey: true })).toBe('app');
    expect(action('Enter', { ...primary, shiftKey: true })).toBe('app');
    expect(action('T', { ...primary, shiftKey: true })).toBe('app');
    expect(action('1', { ...primary, altKey: true })).toBe('app');
    expect(action('f', { altKey: true })).toBeNull();
    expect(terminalKeyAction(new KeyboardEvent('keyup', { key: 'c', ...primary }), true)).toBeNull();
  });
});
