import type { DirEntry } from './types';

export interface Mention {
  start: number;
  end: number;
  query: string;
}

const absolute = /^(?:[a-zA-Z]:[\\/]|\\\\|\/)/;

/** Only a standalone @ at the caret opens completion, never an email address. */
export function activeMention(text: string, caret: number, selectionEnd = caret): Mention | null {
  if (caret !== selectionEnd) return null;
  const match = /(?:^|\s)@(?:"([^"\r\n]*)|([^\s"]*))$/.exec(text.slice(0, caret));
  if (!match) return null;
  const quoted = match[1] !== undefined;
  const tail = text.slice(caret);
  const stop = quoted ? tail.search(/["\r\n]/) : tail.search(/[\s"]/);
  const end = stop < 0 ? text.length : caret + stop + (quoted && tail[stop] === '"' ? 1 : 0);
  return { start: match.index + match[0].indexOf('@'), end, query: match[1] ?? match[2] };
}

export function completionQuery(query: string, cwd: string, home: string): { directory: string; prefix: string } {
  const sep = cwd.includes('\\') ? '\\' : '/';
  let path = query.replace(/[\\/]/g, sep);
  if (path === '~' || path.startsWith(`~${sep}`)) path = home + path.slice(1) + (path === '~' ? sep : '');
  if (/^[a-zA-Z]:$/.test(path)) path += '\\';
  if (sep === '\\' && /^\\(?!\\)/.test(path) && /^[a-zA-Z]:/.test(cwd)) path = cwd.slice(0, 2) + path;
  if (!absolute.test(path)) path = `${cwd.replace(/[\\/]$/, '')}${sep}${path}`;
  const split = Math.max(path.lastIndexOf('\\'), path.lastIndexOf('/'));
  return { directory: path.slice(0, split + 1), prefix: path.slice(split + 1) };
}

export function insertMention(text: string, mention: Mention, entry: Pick<DirEntry, 'path' | 'isDir'>): { text: string; caret: number } {
  const sep = entry.path.includes('\\') ? '\\' : '/';
  // Keep the caret inside the quotes while navigating, without swallowing following text.
  const token = entry.isDir ? `@"${entry.path.replace(/[\\/]$/, '')}${sep}"` : `@"${entry.path}" `;
  return {
    text: text.slice(0, mention.start) + token + text.slice(mention.end),
    caret: mention.start + token.length - (entry.isDir ? 1 : 0),
  };
}

/** Completion inserts quoted absolute paths so they remain valid in a new session's folder. */
export function mentionedFiles(text: string): string[] {
  return [...new Set([...text.matchAll(/(?:^|\s)@"([^"\r\n]+)"(?=\s|$)/g)].map((m) => m[1]).filter((p) => absolute.test(p) && !/[\\/]$/.test(p)))];
}
