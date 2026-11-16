export const isMac = /Mac|iPhone|iPad|iPod/.test(navigator.platform);
export const fileManager = isMac ? 'Finder' : 'File Explorer';

export function primaryModifier(e: { ctrlKey?: boolean; metaKey?: boolean }): boolean {
  return isMac ? !!e.metaKey && !e.ctrlKey : !!e.ctrlKey && !e.metaKey;
}

/** Source shortcuts use Ctrl for the platform modifier; Control means the literal terminal key. */
export function shortcutLabel(keys: string): string {
  if (!isMac) return keys.replace(/\bControl\b/g, 'Ctrl');
  return keys.replace(/\bCtrl\+Shift\+H\b/g, 'Cmd+Shift+Y').replace(/\bCtrl\b(?!\+(?:Shift\+)?Tab)/g, 'Cmd').replace(/\bAlt\b/g, 'Option').replace(/\bControl\b/g, 'Ctrl');
}

export function bayNumber(e: { key: string; code: string }): number | null {
  const digit = /^Digit([1-9])$/.exec(e.code)?.[1] ?? (/^[1-9]$/.test(e.key) ? e.key : null);
  return digit ? Number(digit) : null;
}
