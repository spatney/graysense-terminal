import type { MemoryDto } from './types';

const STOP = new Set(
  'a an and are as at be but by did do does for from had has have he her him his how i if in into is it its me my of on or our she so that the their them they this to us was we were what when where which who why will with you your give gave given send sent show find get got about'.split(
    ' ',
  ),
);

export function terms(text: string): string[] {
  return text
    .toLowerCase()
    .split(/[^\p{L}\p{N}._-]+/u)
    .map((t) => t.replace(/^[._-]+|[._-]+$/g, ''))
    .filter((t) => t.length > 1 && !STOP.has(t));
}

/** Text a note is found by: what it says, plus its files' names and links. */
function haystack(m: MemoryDto): string {
  return `${m.text} ${m.attachments.map((a) => `${a.name} ${a.target}`).join(' ')} ${m.sources.map((s) => s.title ?? '').join(' ')}`.toLowerCase();
}

export interface NoteHit {
  note: MemoryDto;
  score: number;
}

/** Rank notes for a query: matching words (rarer ones count more), then how useful each has been. */
export function rankNotes(notes: MemoryDto[], query: string): NoteHit[] {
  const q = terms(query);
  if (!q.length) return [];
  const docs = notes.map((n) => haystack(n));
  const idf = q.map((t) => {
    const df = docs.filter((d) => d.includes(t)).length;
    return Math.log(1 + (notes.length - df + 0.5) / (df + 0.5));
  });
  const phrase = query.trim().toLowerCase();
  const hits: NoteHit[] = [];
  notes.forEach((note, i) => {
    const d = docs[i];
    let s = 0;
    let matched = 0;
    q.forEach((t, k) => {
      let count = 0;
      let at = d.indexOf(t);
      while (at >= 0 && count < 10) {
        count++;
        at = d.indexOf(t, at + t.length);
      }
      // Prefix matches ("logo" finds "logos") count a little less.
      if (!count && t.length > 3 && new RegExp(`\\b${t.slice(0, -1).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`).test(d)) count = 0.6;
      if (count) {
        matched++;
        s += idf[k] * (1 + Math.log(1 + count));
      }
    });
    if (!matched) return;
    if (phrase.length > 4 && d.includes(phrase)) s *= 1.5;
    s *= 0.4 + 0.6 * (matched / q.length);
    s *= 0.65 + 0.35 * note.relevance + (note.pinned ? 0.15 : 0);
    hits.push({ note, score: s });
  });
  return hits.sort((a, b) => b.score - a.score);
}

/** A question to answer from notes rather than a few words to filter by. */
export function looksLikeQuestion(query: string): boolean {
  const q = query.trim().toLowerCase();
  return q.endsWith('?') || /^(who|what|when|where|why|how|which|whom|whose|did|do|does|is|are|was|were|can|could|should|have|has|find|show|tell|remind|list)\b/.test(q);
}

/** Ranges of `text` that match the query's words, for highlighting. */
export function matchRanges(text: string, query: string): [number, number][] {
  const q = terms(query);
  if (!q.length) return [];
  const lower = text.toLowerCase();
  const ranges: [number, number][] = [];
  for (const t of q) {
    let at = lower.indexOf(t);
    while (at >= 0) {
      ranges.push([at, at + t.length]);
      at = lower.indexOf(t, at + t.length);
    }
  }
  ranges.sort((a, b) => a[0] - b[0]);
  const merged: [number, number][] = [];
  for (const r of ranges) {
    const last = merged[merged.length - 1];
    if (last && r[0] <= last[1]) last[1] = Math.max(last[1], r[1]);
    else merged.push([...r]);
  }
  return merged;
}

/** Five-step relevance for the meter. */
export function relevanceLevel(r: number): number {
  return r >= 0.8 ? 5 : r >= 0.6 ? 4 : r >= 0.4 ? 3 : r >= 0.2 ? 2 : 1;
}

export function fileIcon(name: string, kind: 'file' | 'folder' | 'link'): string {
  if (kind === 'folder') return 'folder';
  if (kind === 'link') return 'link';
  const ext = name.split('.').pop()?.toLowerCase() ?? '';
  if (['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'bmp', 'ico'].includes(ext)) return 'file-media';
  if (['pdf'].includes(ext)) return 'file-pdf';
  if (['zip', '7z', 'rar', 'gz', 'tar'].includes(ext)) return 'file-zip';
  if (['md', 'txt', 'doc', 'docx', 'rtf'].includes(ext)) return 'note';
  if (['xls', 'xlsx', 'csv'].includes(ext)) return 'table';
  if (['ppt', 'pptx', 'key'].includes(ext)) return 'preview';
  if (['ts', 'tsx', 'js', 'jsx', 'rs', 'py', 'cs', 'go', 'java', 'json', 'yaml', 'yml', 'toml', 'html', 'css', 'ps1', 'sh'].includes(ext)) return 'file-code';
  return 'file';
}

/** URLs typed into a note (saved as links too). */
export function linksIn(text: string): string[] {
  const out: string[] = [];
  for (const m of text.matchAll(/https?:\/\/[^\s<>"()]+/g)) {
    const url = m[0].replace(/[.,;:!?']+$/, '');
    if (!out.includes(url)) out.push(url);
  }
  return out;
}
