// Unified diffs for the transcript: reading the shapes the Copilot tools write (`diff --git a/C:/… b/C:/…`
// headers, `--- a/dev/null` for a new file, a multi-file diff from `apply_patch`), combining one file's
// edits in a turn into one diff, and writing diffs back out as text.

import type { FileEdit, ToolDto } from '../../lib/types';

export type DiffLineKind = 'add' | 'del' | 'ctx' | 'meta';

export interface DiffLine {
  kind: DiffLineKind;
  text: string;
  oldNo?: number;
  newNo?: number;
}

export interface DiffHunk {
  oldStart: number;
  newStart: number;
  /** What follows the `@@ … @@` marker (often a function's name). */
  section: string;
  lines: DiffLine[];
}

export type DiffFileStatus = 'added' | 'deleted' | 'modified' | 'renamed';

export interface DiffFile {
  /** The new path, or the old one for a deleted file. */
  path: string;
  oldPath: string | null;
  newPath: string | null;
  status: DiffFileStatus;
  hunks: DiffHunk[];
  added: number;
  removed: number;
  /** Git only says a binary file differs. */
  binary?: boolean;
}

const HUNK_RE = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@ ?(.*)$/;

/** `a/src/x.ts` → `src/x.ts`; `/dev/null` (or the tools' `a/dev/null`) → null. */
function headerPath(raw: string): string | null {
  const p = raw.replace(/\t.*$/, '').trim();
  const stripped = /^[ab]\//.test(p) ? p.slice(2) : p;
  return stripped === '/dev/null' || stripped === 'dev/null' ? null : stripped;
}

/**
 * A unified diff's files and hunks. A hunk's lines are read by the counts in its `@@` header, so
 * content such as a removed `-- comment` line (`--- comment`) is never taken for a file header.
 */
export function parseUnifiedDiff(diff: string): DiffFile[] {
  const files: DiffFile[] = [];
  let file: DiffFile | null = null;
  let hunk: DiffHunk | null = null;
  let oldNo = 0;
  let newNo = 0;
  let oldLeft = 0;
  let newLeft = 0;
  const startFile = (): DiffFile => {
    const f: DiffFile = { path: '', oldPath: null, newPath: null, status: 'modified', hunks: [], added: 0, removed: 0 };
    files.push(f);
    return f;
  };

  for (const line of diff.replace(/\r\n?/g, '\n').split('\n')) {
    if (hunk && file && (oldLeft > 0 || newLeft > 0)) {
      const first = line[0];
      if (first === '+') {
        hunk.lines.push({ kind: 'add', text: line.slice(1), newNo: newNo++ });
        file.added++;
        newLeft--;
        continue;
      }
      if (first === '-') {
        hunk.lines.push({ kind: 'del', text: line.slice(1), oldNo: oldNo++ });
        file.removed++;
        oldLeft--;
        continue;
      }
      // The tools trim trailing spaces, so an empty context line can come without its space.
      if (first === ' ' || line === '') {
        hunk.lines.push({ kind: 'ctx', text: line.slice(1), oldNo: oldNo++, newNo: newNo++ });
        oldLeft--;
        newLeft--;
        continue;
      }
    }
    if (line.startsWith('\\') && hunk) {
      // "\ No newline at end of file" follows the line it is about.
      hunk.lines.push({ kind: 'meta', text: line.slice(1).trim() });
      continue;
    }
    const m = HUNK_RE.exec(line);
    if (m) {
      if (!file) file = startFile();
      oldNo = Number(m[1]);
      newNo = Number(m[3]);
      oldLeft = m[2] === undefined ? 1 : Number(m[2]);
      newLeft = m[4] === undefined ? 1 : Number(m[4]);
      hunk = { oldStart: oldNo, newStart: newNo, section: m[5].trim(), lines: [] };
      file.hunks.push(hunk);
      continue;
    }
    if (line.startsWith('diff --git ')) {
      file = startFile();
      hunk = null;
      continue;
    }
    if (line.startsWith('--- ')) {
      // A `---` header after hunks starts a file that had no `diff --git` line.
      if (!file || file.hunks.length) file = startFile();
      hunk = null;
      file.oldPath = headerPath(line.slice(4));
      continue;
    }
    if (!file || file.hunks.length) continue;
    if (line.startsWith('+++ ')) file.newPath = headerPath(line.slice(4));
    else if (line.startsWith('new file mode') || line.startsWith('create file mode')) file.status = 'added';
    else if (line.startsWith('deleted file mode')) file.status = 'deleted';
    else if (line.startsWith('Binary files ')) {
      const b = /^Binary files (.+?) and ((?:b\/|\/dev\/null).*) differ$/.exec(line);
      if (b) {
        file.oldPath = headerPath(b[1]);
        file.newPath = headerPath(b[2]);
        file.binary = true;
      }
    } else if (line.startsWith('rename from ')) {
      file.status = 'renamed';
      file.oldPath = line.slice('rename from '.length).trim();
    } else if (line.startsWith('rename to ')) file.newPath = line.slice('rename to '.length).trim();
  }

  for (const f of files) {
    if (f.status === 'modified') {
      if (f.oldPath == null && f.newPath != null) f.status = 'added';
      else if (f.newPath == null && f.oldPath != null) f.status = 'deleted';
    }
    f.path = f.newPath ?? f.oldPath ?? '';
  }
  return files.filter((f) => f.hunks.length > 0 || f.path);
}

/** Unchanged lines kept around each change in a combined diff. */
const CONTEXT = 3;

/** Where a hunk side starts, as a 1-based line: the tools number an empty side by where its lines would go (`@@ -1,0 +1,3 @@` makes a file), and a deleted file's new side `+0,0`. */
const sideStart = (start: number): number => Math.max(1, start);

function pushAt(map: Map<number, string[]>, at: number, text: string): void {
  const list = map.get(at);
  if (list) list.push(text);
  else map.set(at, [text]);
}

/** The text two diffs show for the same line, or null when they disagree. The tools trim trailing spaces off a diff's lines, so that alone is no disagreement. */
function sameLine(a: string, b: string): string | null {
  if (a === b) return a;
  if (a.trimEnd() !== b.trimEnd()) return null;
  return a.length >= b.length ? a : b;
}

/**
 * `first` (A→B) and `next` (B→C) as one diff, A→C. Both are read on B: `first`'s new side and
 * `next`'s old side; a line neither shows was unchanged by both. `whole` says `first` shows every
 * line of B. Null when the two disagree about B, as when something else changed the file between them.
 */
function composePair(first: readonly DiffHunk[], whole: boolean, next: DiffFile): DiffHunk[] | null {
  // B's lines as `first` shows them (`fromA` is false for lines it added), and the A lines it
  // removed, under the B line they came before.
  const mid = new Map<number, { text: string; fromA: boolean }>();
  const removed = new Map<number, string[]>();
  let end = 0;
  for (const h of first) {
    let at = sideStart(h.newStart);
    if (at < end) return null;
    for (const l of h.lines) {
      if (l.kind === 'del') pushAt(removed, at, l.text);
      else if (l.kind !== 'meta') mid.set(at++, { text: l.text, fromA: l.kind === 'ctx' });
    }
    end = at;
  }
  // B's lines as `next` shows them (`kept` is false for lines it removed), and the C lines it added,
  // under the B line they come before.
  const seen = new Map<number, { text: string; kept: boolean }>();
  const added = new Map<number, string[]>();
  end = 0;
  for (const h of next.hunks) {
    let at = sideStart(h.oldStart);
    if (at < end) return null;
    for (const l of h.lines) {
      if (l.kind === 'add') pushAt(added, at, l.text);
      else if (l.kind !== 'meta') {
        const known = mid.get(at);
        const text = known ? sameLine(known.text, l.text) : l.text;
        if (text == null) return null;
        seen.set(at++, { text, kept: l.kind === 'ctx' });
      }
    }
    end = at;
  }
  // A file is only made where there is none, and deleting one removes every line; when `first`
  // shows all of B, `next` can't touch lines past its end.
  if (next.status === 'added' && mid.size > 0) return null;
  if (next.status === 'deleted' && [...mid.keys()].some((at) => !seen.has(at))) return null;
  if (whole && ([...seen.keys()].some((at) => !mid.has(at)) || [...added.keys()].some((at) => at > mid.size + 1))) return null;

  const positions = [...new Set([...mid.keys(), ...seen.keys(), ...removed.keys(), ...added.keys()])].sort((x, y) => x - y);
  const hunks: DiffHunk[] = [];
  let hunk: DiffHunk | null = null;
  let oldNo = 0; // A lines so far
  let newNo = 0; // C lines so far
  let passed = 0; // B lines so far
  for (const at of positions) {
    // B lines neither diff shows are the same in A, B and C.
    const hidden = at - 1 - passed;
    if (!hunk || hidden > 0) {
      oldNo += hidden;
      newNo += hidden;
      hunk = { oldStart: oldNo + 1, newStart: newNo + 1, section: '', lines: [] };
      hunks.push(hunk);
    }
    passed = at - 1;
    for (const text of removed.get(at) ?? []) hunk.lines.push({ kind: 'del', text, oldNo: ++oldNo });
    for (const text of added.get(at) ?? []) hunk.lines.push({ kind: 'add', text, newNo: ++newNo });
    const before = mid.get(at);
    const after = seen.get(at);
    const line = after ?? before;
    if (!line) continue;
    passed = at;
    const fromA = before?.fromA ?? true;
    const kept = after?.kept ?? true;
    if (fromA && kept) hunk.lines.push({ kind: 'ctx', text: line.text, oldNo: ++oldNo, newNo: ++newNo });
    else if (fromA) hunk.lines.push({ kind: 'del', text: line.text, oldNo: ++oldNo });
    else if (kept) hunk.lines.push({ kind: 'add', text: line.text, newNo: ++newNo });
  }
  return hunks;
}

/** Keep `context` unchanged lines around the changes, splitting hunks where more lie between. */
function trimContext(hunks: readonly DiffHunk[], context = CONTEXT): DiffHunk[] {
  const out: DiffHunk[] = [];
  for (const h of hunks) {
    // The line numbers at each line, so any line can start a hunk.
    const starts: [number, number][] = [];
    const changes: number[] = [];
    let oldNo = h.oldStart;
    let newNo = h.newStart;
    h.lines.forEach((l, i) => {
      starts.push([oldNo, newNo]);
      if (l.kind === 'add' || l.kind === 'del') changes.push(i);
      if (l.kind === 'ctx' || l.kind === 'del') oldNo++;
      if (l.kind === 'ctx' || l.kind === 'add') newNo++;
    });
    let i = 0;
    while (i < changes.length) {
      let j = i;
      while (j + 1 < changes.length && changes[j + 1] - changes[j] - 1 <= 2 * context) j++;
      const from = Math.max(0, changes[i] - context);
      const to = Math.min(h.lines.length, changes[j] + context + 1);
      const [oldStart, newStart] = starts[from];
      out.push({ oldStart, newStart, section: '', lines: h.lines.slice(from, to) });
      i = j + 1;
    }
  }
  return out;
}

/**
 * Lines a block of changes removes and adds back the same, as unchanged lines: a line one edit
 * changed and a later one changed back is no change. Within a block, removed lines go first.
 */
function settle(hunks: readonly DiffHunk[]): DiffHunk[] {
  return hunks.map((h) => {
    const lines: DiffLine[] = [];
    let block: DiffLine[] = [];
    const same = (d: DiffLine, a: DiffLine): DiffLine => ({ kind: 'ctx', text: d.text, oldNo: d.oldNo, newNo: a.newNo });
    const flush = () => {
      const dels = block.filter((l) => l.kind === 'del');
      const adds = block.filter((l) => l.kind === 'add');
      let p = 0;
      while (p < dels.length && p < adds.length && dels[p].text === adds[p].text) p++;
      let s = 0;
      while (s < dels.length - p && s < adds.length - p && dels[dels.length - 1 - s].text === adds[adds.length - 1 - s].text) s++;
      for (let i = 0; i < p; i++) lines.push(same(dels[i], adds[i]));
      lines.push(...dels.slice(p, dels.length - s), ...adds.slice(p, adds.length - s));
      for (let i = s; i > 0; i--) lines.push(same(dels[dels.length - i], adds[adds.length - i]));
      block = [];
    };
    for (const l of h.lines) {
      if (l.kind === 'del' || l.kind === 'add') block.push(l);
      else {
        flush();
        lines.push(l);
      }
    }
    flush();
    return { ...h, lines };
  });
}

/**
 * One file's diffs, made one after another (each against what the one before left), as the single
 * diff from before the first to after the last. Null when they don't line up, as when something
 * other than these edits changed the file between them.
 */
export function composeDiffs(steps: readonly DiffFile[]): DiffFile | null {
  const [head, ...rest] = steps;
  if (!head) return null;
  if (!rest.length) return head;
  let hunks: DiffHunk[] = head.hunks;
  // True once `hunks` shows every line of the file, as a new file's diff does.
  let whole = head.status === 'added';
  for (const step of rest) {
    const next = composePair(hunks, whole, step);
    if (!next) return null;
    hunks = next;
    whole = whole || step.status === 'added' || step.status === 'deleted';
  }
  hunks = trimContext(settle(hunks));
  const last = rest[rest.length - 1];
  const { oldPath } = head;
  const { newPath } = last;
  const count = (kind: DiffLineKind): number => hunks.reduce((n, h) => n + h.lines.filter((l) => l.kind === kind).length, 0);
  return {
    path: newPath ?? oldPath ?? last.path,
    oldPath,
    newPath,
    status: !oldPath ? 'added' : !newPath ? 'deleted' : oldPath !== newPath ? 'renamed' : 'modified',
    hunks,
    added: count('add'),
    removed: count('del'),
  };
}

const SIGN: Record<DiffLineKind, string> = { add: '+', del: '-', ctx: ' ', meta: '\\ ' };

/** Diff files as unified diff text, the way `parseUnifiedDiff` reads it. */
export function formatUnifiedDiff(files: readonly DiffFile[]): string {
  const out: string[] = [];
  for (const f of files) {
    const oldPath = f.status === 'added' ? null : (f.oldPath ?? f.path);
    const newPath = f.status === 'deleted' ? null : (f.newPath ?? f.path);
    out.push(`diff --git a/${oldPath ?? newPath} b/${newPath ?? oldPath}`);
    if (f.status === 'renamed') out.push(`rename from ${oldPath}`, `rename to ${newPath}`);
    out.push(oldPath ? `--- a/${oldPath}` : '--- /dev/null');
    out.push(newPath ? `+++ b/${newPath}` : '+++ /dev/null');
    for (const h of f.hunks) {
      const oldCount = h.lines.filter((l) => l.kind === 'ctx' || l.kind === 'del').length;
      const newCount = h.lines.filter((l) => l.kind === 'ctx' || l.kind === 'add').length;
      out.push(`@@ -${h.oldStart},${oldCount} +${h.newStart},${newCount} @@${h.section ? ` ${h.section}` : ''}`);
      for (const l of h.lines) out.push(SIGN[l.kind] + l.text);
    }
  }
  return out.join('\n');
}

// ------------------------------------------------------------------ paths

/** Forward slashes, no trailing slash. */
const normalize = (path: string): string => path.trim().replace(/\\/g, '/').replace(/\/+$/, '');
const isWindows = (p: string): boolean => /^[a-z]:(\/|$)/i.test(p) || p.startsWith('//');
const isAbsolute = (p: string): boolean => isWindows(p) || p.startsWith('/');

/** `path` made absolute against `root` when it's relative, with forward slashes. */
function absolute(path: string, root: string | null | undefined): string {
  const p = normalize(path);
  return isAbsolute(p) || !root ? p : `${normalize(root)}/${p.replace(/^\.\//, '')}`;
}

/** The same key for every way of writing a file's path (Windows paths ignore case). */
const fileKey = (abs: string): string => (isWindows(abs) ? abs.toLowerCase() : abs);

/** A path as the system writes it: backslashes on Windows. */
const systemPath = (abs: string): string => (isWindows(abs) ? abs.replace(/\//g, '\\') : abs);

/** `path` relative to `root`, with forward slashes: '' for `root` itself, null when it's outside. */
export function relativePath(path: string, root: string | null | undefined): string | null {
  const p = normalize(path);
  if (!isAbsolute(p)) return p.replace(/^\.\//, '');
  const r = root ? normalize(root) : '';
  if (!r) return null;
  const fold = isWindows(r) || isWindows(p);
  const [a, b] = fold ? [p.toLowerCase(), r.toLowerCase()] : [p, r];
  if (a === b) return '';
  return a.startsWith(`${b}/`) ? p.slice(r.length + 1) : null;
}

/** A file's path for showing in full: relative to `root` when it's in it, else whole, as the system writes it. */
export function displayPath(path: string, root: string | null | undefined): string {
  const abs = absolute(path, root);
  const rel = relativePath(abs, root);
  return rel ? rel.replace(/\//g, isWindows(abs) ? '\\' : '/') : systemPath(abs);
}

export interface ShownPath {
  /** The folders, relative to `root` when the file is in it, ending in a separator; '' outside it (the tooltip has them). */
  dir: string;
  base: string;
  /** The whole path, as the system writes it. */
  full: string;
}

/** A file's path for showing: its folders (dim) and its name. */
export function shownPath(path: string, root: string | null | undefined): ShownPath {
  const abs = absolute(path, root);
  const full = systemPath(abs);
  const rel = relativePath(abs, root);
  const sep = isWindows(abs) ? '\\' : '/';
  const parts = (rel || abs).split('/');
  const base = parts.pop() || full;
  return { dir: rel && parts.length ? parts.join(sep) + sep : '', base, full };
}

/** Lines a diff shows before "Show all". */
export const DIFF_PREVIEW = 60;

/** The text a diff shows (its files' paths and lines, up to the preview), to find in. */
export function diffText(diff: string, root: string | null | undefined): string {
  const files = parseUnifiedDiff(diff);
  if (!files.length) return diff;
  let budget = DIFF_PREVIEW;
  const out: string[] = [];
  for (const f of files) {
    const p = shownPath(f.path, root);
    out.push(p.dir + p.base);
    for (const h of f.hunks) {
      if (budget <= 0) break;
      const lines = h.lines.slice(0, budget);
      budget -= lines.length;
      for (const l of lines) out.push(l.text);
    }
  }
  return out.join('\n');
}

// ------------------------------------------------------------------ a turn's files

export type FileChangeStatus = 'created' | 'edited' | 'deleted';

export interface FileChange {
  /** The file's whole path, as the system writes it. */
  path: string;
  status: FileChangeStatus;
  added?: number;
  removed?: number;
  /**
   * What the turn did to the file, as a unified diff: its net change when every edit's diff came
   * whole and they line up, else each edit in turn. Empty when the edits cancel out; absent when no
   * edit came with a diff.
   */
  diff?: string;
  /** `diff` lists the edits one by one: they couldn't be combined. */
  stepwise?: boolean;
  /** Some edits to the file came without a diff, or with only part of one. */
  partial?: boolean;
}

interface Entry {
  change: FileChange;
  /** The file's part of each edit's diff, in order. */
  diffs: DiffFile[];
  /** Some edit to the file has no whole diff. */
  gaps: boolean;
}

function summarize({ change, diffs, gaps }: Entry): FileChange {
  if (!diffs.length) return change;
  const net = gaps ? null : composeDiffs(diffs);
  if (net) return { ...change, added: net.added, removed: net.removed, diff: net.hunks.length ? formatUnifiedDiff([net]) : '' };
  return { ...change, diff: formatUnifiedDiff(diffs), ...(diffs.length > 1 && { stepwise: true }), ...(gaps && { partial: true }) };
}

interface LineCounts {
  added: number;
  removed: number;
}

const KIND_STATUS: Record<FileEdit['kind'], FileChangeStatus> = { create: 'created', edit: 'edited', delete: 'deleted' };
const partStatus = (f: DiffFile): FileChangeStatus => (f.status === 'added' ? 'created' : f.status === 'deleted' ? 'deleted' : 'edited');

/** The files an edit tool wrote: as the CLI reports them, else the one its arguments name. */
function written(t: ToolDto): FileEdit[] {
  if (t.files.length) return t.files;
  return t.args?.path ? [{ path: String(t.args.path), kind: t.family === 'create' ? 'create' : 'edit' }] : [];
}

/** The files a diff doesn't show (the CLI cut them off, or left them out). */
export function missingFrom<T extends { path: string }>(diff: string, files: readonly T[], cwd: string | null): T[] {
  const shown = new Set(parseUnifiedDiff(diff).filter((f) => f.path).map((f) => fileKey(absolute(f.path, cwd))));
  return files.filter((f) => !shown.has(fileKey(absolute(f.path, cwd))));
}

/**
 * The files a turn's edits changed, in the order it first touched them, each with what the turn did
 * to it. `cwd` (the session's folder) places relative paths.
 */
export function filesChanged(tools: readonly ToolDto[], cwd: string | null): FileChange[] {
  const files = new Map<string, Entry>();
  const keyOf = (path: string): string => fileKey(absolute(path, cwd));
  /** Records an edit to a file, with the lines it added and removed when they're known. */
  const touch = (raw: string, status: FileChangeStatus, lines: LineCounts | undefined): Entry => {
    const abs = absolute(raw, cwd);
    const key = fileKey(abs);
    const prev = files.get(key);
    if (!prev) {
      const entry: Entry = { change: { path: systemPath(abs), status, added: lines?.added, removed: lines?.removed }, diffs: [], gaps: false };
      files.set(key, entry);
      return entry;
    }
    const c = prev.change;
    // One edit of unknown size leaves the file's count unknown.
    c.added = c.added != null && lines ? c.added + lines.added : undefined;
    c.removed = c.removed != null && lines ? c.removed + lines.removed : undefined;
    if (status === 'deleted') c.status = 'deleted';
    else if (c.status === 'deleted') c.status = status;
    return prev;
  };
  for (const t of tools) {
    if (t.state !== 'ok' || (t.family !== 'edit' && t.family !== 'create')) continue;
    const parts = t.diff ? parseUnifiedDiff(t.diff.text).filter((f) => f.path) : [];
    // A diff cut short ends partway through its last file; the files after it aren't in it at all.
    const cut = t.diff?.truncated ? parts.pop() : undefined;
    const seen = new Set(parts.map((f) => keyOf(f.path)));
    if (cut) seen.add(keyOf(cut.path));
    const unseen: FileEdit[] = [];
    for (const f of written(t)) {
      const key = keyOf(f.path);
      if (seen.has(key)) continue;
      seen.add(key);
      unseen.push(f);
    }
    // The diff's count is the tool's whole change: what its whole files don't account for is the
    // rest's, known file by file only when the rest is one file.
    let rest: LineCounts | undefined;
    if (t.diff?.truncated && (cut ? 1 : 0) + unseen.length === 1) {
      const added = t.diff.added - parts.reduce((n, f) => n + f.added, 0);
      const removed = t.diff.removed - parts.reduce((n, f) => n + f.removed, 0);
      if (added >= 0 && removed >= 0) rest = { added, removed };
    }
    for (const f of parts) touch(f.path, partStatus(f), f).diffs.push(f);
    if (cut) {
      const entry = touch(cut.path, partStatus(cut), rest);
      entry.diffs.push(cut);
      entry.gaps = true;
    }
    for (const f of unseen) touch(f.path, KIND_STATUS[f.kind], rest).gaps = true;
  }
  return [...files.values()].map(summarize);
}
