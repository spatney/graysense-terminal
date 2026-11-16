import { afterEach, describe, expect, it, vi } from 'vitest';

afterEach(() => {
  vi.restoreAllMocks();
  vi.resetModules();
});

describe.each(['MacIntel', 'Win32'])('platform shortcuts on %s', (platform) => {
  it('uses the native modifier and labels without stealing Control+Tab or shell interrupt', async () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue(platform);
    const { bayNumber, primaryModifier, shortcutLabel } = await import('./platform');
    const mac = platform === 'MacIntel';
    expect(primaryModifier({ metaKey: true })).toBe(mac);
    expect(primaryModifier({ ctrlKey: true })).toBe(!mac);
    expect(primaryModifier({ ctrlKey: true, metaKey: true })).toBe(false);
    expect(shortcutLabel('Ctrl+K')).toBe(mac ? 'Cmd+K' : 'Ctrl+K');
    expect(shortcutLabel('Ctrl+Shift+H')).toBe(mac ? 'Cmd+Shift+Y' : 'Ctrl+Shift+H');
    expect(shortcutLabel('Ctrl+Tab')).toBe('Ctrl+Tab');
    expect(shortcutLabel('Ctrl+Shift+Tab')).toBe('Ctrl+Shift+Tab');
    expect(shortcutLabel('Control+C')).toBe('Ctrl+C');
    expect(shortcutLabel('Ctrl+Alt+1')).toBe(mac ? 'Cmd+Option+1' : 'Ctrl+Alt+1');
    expect(bayNumber({ key: '¡', code: 'Digit1' })).toBe(1);
  });
});
