import { bayNumber, isMac, primaryModifier } from '../../lib/platform';

export function terminalKeyAction(e: KeyboardEvent, selection: boolean): 'copy' | 'paste' | 'find' | 'selectAll' | 'app' | null {
  if (e.type !== 'keydown') return null;
  const k = e.key.toLowerCase();
  if (e.ctrlKey && !e.metaKey && !e.altKey && k === 'tab') return 'app';
  if (!primaryModifier(e)) return null;
  if (!e.altKey && k === 'c' && (isMac || e.shiftKey || selection)) return 'copy';
  if (!e.altKey && k === 'v') return 'paste';
  if (!e.altKey && !e.shiftKey && k === 'f') return 'find';
  if (isMac && !e.altKey && !e.shiftKey && k === 'a') return 'selectAll';
  if (!e.altKey && e.shiftKey && (/^[a-z]$/.test(k) || k === 'enter')) return 'app';
  if (!e.altKey && (['tab', '=', '+', '-', '0', ',', '/'].includes(k) || e.code === 'Slash')) return 'app';
  if (e.altKey && bayNumber(e) != null) return 'app';
  if (isMac && !e.altKey && k === 'k') return 'app';
  return null;
}
