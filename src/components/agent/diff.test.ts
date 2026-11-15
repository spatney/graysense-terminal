import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { createElement } from 'react';
import { afterEach, describe, expect, it } from 'vitest';
import { highlightLines } from '../../lib/highlight';
import type { ToolDto } from '../../lib/types';
import { composeDiffs, filesChanged, missingFrom, parseUnifiedDiff, shownPath } from './diff';
import { FilesChanged } from './FilesChanged';

/** A diff the way the edit tools write one: git headers with the absolute path, forward slashes. */
const edit = (path: string, hunk: string) => `diff --git a/${path} b/${path}\nindex 0000000..0000000 100644\n--- a/${path}\n+++ b/${path}\n${hunk}`;

const tool = (family: 'edit' | 'create', diff: string | null, more: Partial<ToolDto> = {}): ToolDto => ({
  type: 'tool',
  name: family,
  label: family === 'edit' ? 'Edit' : 'Create',
  verb: family,
  target: null,
  family,
  state: 'ok',
  progress: null,
  output: '',
  result: null,
  error: null,
  files: [],
  elapsed: 0,
  started: 0,
  mcpServer: null,
  hasApp: false,
  args: {},
  diff: diff == null ? null : { text: diff, truncated: false, added: 0, removed: 0 },
  ...more,
});

// l1…l10, with l3 changed to L3 and then l8 to L8.
const first = edit('C:/app/src/x.ts', `@@ -1,6 +1,6 @@\n l1\n l2\n-l3\n+L3\n l4\n l5\n l6`);
const second = edit('C:/app/src/x.ts', `@@ -5,6 +5,6 @@\n l5\n l6\n l7\n-l8\n+L8\n l9\n l10`);

describe('reading diffs', () => {
  it('reads an edit by its hunk counts, so a removed "-- note" line is no file header', () => {
    const [f, ...rest] = parseUnifiedDiff(`\n${edit('C:/app/q.sql', '@@ -1,3 +1,3 @@\n select 1;\n--- note\n+-- a better note\n select 2;')}`);
    expect(rest).toEqual([]);
    expect(f).toMatchObject({ path: 'C:/app/q.sql', status: 'modified', added: 1, removed: 1 });
    expect(f.hunks[0].lines.map((l) => [l.kind, l.text, l.oldNo ?? null, l.newNo ?? null])).toEqual([
      ['ctx', 'select 1;', 1, 1],
      ['del', '-- note', 2, null],
      ['add', '-- a better note', null, 2],
      ['ctx', 'select 2;', 3, 3],
    ]);
  });

  it('reads a new file, and each file of a patch', () => {
    const [made] = parseUnifiedDiff('diff --git a/C:/app/new.py b/C:/app/new.py\ncreate file mode 100644\nindex 0000000..0000000\n--- a/dev/null\n+++ b/C:/app/new.py\n@@ -1,0 +1,2 @@\n+import os\n+print(os.name)');
    expect(made).toMatchObject({ path: 'C:/app/new.py', status: 'added', added: 2, removed: 0 });
    const patch = parseUnifiedDiff(`${edit('C:/app/a.ts', '@@ -1 +1 @@\n-a\n+A')}\n${edit('C:/app/b.ts', '@@ -2,0 +3 @@\n+b')}`);
    expect(patch.map((f) => [f.path, f.added, f.removed])).toEqual([
      ['C:/app/a.ts', 1, 1],
      ['C:/app/b.ts', 1, 0],
    ]);
  });

  it('reads binary files git only says differ', () => {
    const files = parseUnifiedDiff(
      'diff --git a/art/logo and icon.png b/art/logo and icon.png\nnew file mode 100644\nindex 0000000..1234567\nBinary files /dev/null and b/art/logo and icon.png differ\n' +
        'diff --git a/old.bin b/old.bin\ndeleted file mode 100644\nindex 1234567..0000000\nBinary files a/old.bin and /dev/null differ\n' +
        edit('src/x.ts', '@@ -1 +1 @@\n-a\n+A'),
    );
    expect(files.map((f) => [f.path, f.status, !!f.binary, f.hunks.length])).toEqual([
      ['art/logo and icon.png', 'added', true, 0],
      ['old.bin', 'deleted', true, 0],
      ['src/x.ts', 'modified', false, 1],
    ]);
  });
});

describe('combining a file’s edits', () => {
  it('turns edits made one after another into the one diff they add up to', () => {
    const net = composeDiffs([...parseUnifiedDiff(first), ...parseUnifiedDiff(second)]);
    expect(net && { added: net.added, removed: net.removed, hunks: net.hunks.length }).toEqual({ added: 2, removed: 2, hunks: 1 });
    expect(net?.hunks[0].lines.map((l) => ({ add: '+', del: '-', ctx: ' ', meta: '\\' })[l.kind] + l.text)).toEqual([' l1', ' l2', '-l3', '+L3', ' l4', ' l5', ' l6', ' l7', '-l8', '+L8', ' l9', ' l10']);
  });

  it('finds no change when a later edit puts a line back', () => {
    const back = edit('C:/app/src/x.ts', `@@ -1,6 +1,6 @@\n l1\n l2\n-L3\n+l3\n l4\n l5\n l6`);
    expect(composeDiffs([...parseUnifiedDiff(first), ...parseUnifiedDiff(back)])?.hunks).toEqual([]);
  });

  it("won't combine edits that disagree about the file between them", () => {
    const stale = edit('C:/app/src/x.ts', `@@ -1,4 +1,4 @@\n l1\n l2\n-l3\n+M3\n l4`);
    expect(composeDiffs([...parseUnifiedDiff(first), ...parseUnifiedDiff(stale)])).toBeNull();
  });
});

describe('a turn’s files', () => {
  it('lists each file once, with its net change, however its path was written', () => {
    const made = 'diff --git a/C:/app/new.md b/C:/app/new.md\ncreate file mode 100644\n--- a/dev/null\n+++ b/C:/app/new.md\n@@ -1,0 +1,2 @@\n+# Notes\n+one';
    const grown = edit('c:/APP/new.md', '@@ -1,2 +1,3 @@\n # Notes\n one\n+two');
    const files = filesChanged(
      [
        tool('edit', first),
        tool('create', made),
        tool('edit', second, { state: 'failed' }),
        tool('edit', second),
        tool('edit', grown),
      ],
      'C:\\app',
    );
    expect(files.map((f) => [f.path, f.status, f.added, f.removed, !!f.stepwise, !!f.partial])).toEqual([
      ['C:\\app\\src\\x.ts', 'edited', 2, 2, false, false],
      ['C:\\app\\new.md', 'created', 3, 0, false, false],
    ]);
    expect(parseUnifiedDiff(files[1].diff ?? '')[0].status).toBe('added');
  });

  it('lists edits one by one when they can’t be combined, and says when some went unrecorded', () => {
    const stale = edit('C:/app/src/x.ts', `@@ -1,4 +1,4 @@\n l1\n l2\n-l3\n+M3\n l4`);
    const [stepwise] = filesChanged([tool('edit', first), tool('edit', stale)], 'C:\\app');
    expect(stepwise).toMatchObject({ stepwise: true, added: 2, removed: 2 });
    expect(parseUnifiedDiff(stepwise.diff ?? '')).toHaveLength(2);

    const [partial] = filesChanged([tool('edit', first), tool('edit', null, { files: [{ path: 'C:\\app\\src\\x.ts', kind: 'edit' }] })], 'C:\\app');
    expect(partial).toMatchObject({ partial: true });
    expect(partial.stepwise).toBeUndefined();
    expect(filesChanged([tool('edit', null, { args: { path: 'src\\y.ts' } })], 'C:\\app')).toEqual([{ path: 'C:\\app\\src\\y.ts', status: 'edited', added: undefined, removed: undefined }]);
  });

  it('keeps every file a patch wrote when its diff was cut short, counting what it can', () => {
    const made = (path: string, n: number) => `diff --git a/${path} b/${path}\ncreate file mode 100644\n--- a/dev/null\n+++ b/${path}\n@@ -1,0 +1,${n} @@\n${Array.from({ length: n }, (_, i) => `+line ${i}`).join('\n')}`;
    // The patch changed a.ts in full, then made b.ts (300 lines) — the diff was cut after 40 of them.
    const cutDiff = `${edit('C:/app/a.ts', '@@ -1 +1 @@\n-a\n+A')}\n${made('C:/app/b.ts', 300).split('\n').slice(0, 45).join('\n')}`;
    const patch = (files: ToolDto['files']) => tool('edit', null, { name: 'apply_patch', files, diff: { text: cutDiff, truncated: true, added: 301, removed: 1 } });
    const two = filesChanged([patch([{ path: 'C:\\app\\a.ts', kind: 'edit' }, { path: 'C:\\app\\b.ts', kind: 'create' }])], 'C:\\app');
    expect(two.map((f) => [f.path, f.status, f.added, f.removed, !!f.partial])).toEqual([
      ['C:\\app\\a.ts', 'edited', 1, 1, false],
      // The rest of the count is the cut file's.
      ['C:\\app\\b.ts', 'created', 300, 0, true],
    ]);
    // A file past the cut is kept, as the CLI reported it; the rest of the count is now two files', so neither gets it.
    const three = filesChanged([patch([{ path: 'C:\\app\\a.ts', kind: 'edit' }, { path: 'C:\\app\\b.ts', kind: 'create' }, { path: 'C:\\app\\old.md', kind: 'delete' }])], 'C:\\app');
    expect(three.map((f) => [f.path, f.status, f.added, f.removed, !!f.partial, f.diff != null])).toEqual([
      ['C:\\app\\a.ts', 'edited', 1, 1, false, true],
      ['C:\\app\\b.ts', 'created', undefined, undefined, true, true],
      ['C:\\app\\old.md', 'deleted', undefined, undefined, false, false],
    ]);
    // A step's own diff shows its files; chips are for the ones it doesn't.
    expect(missingFrom(cutDiff, [{ path: 'C:\\app\\B.ts' }, { path: 'old.md' }], 'C:\\app')).toEqual([{ path: 'old.md' }]);
  });

  it("leaves a file's count unknown once one of its edits is of unknown size", () => {
    const grown = edit('C:/app/src/x.ts', '@@ -10,2 +10,3 @@\n l10\n+l11\n l12');
    const [x] = filesChanged([tool('edit', first), tool('edit', null, { files: [{ path: 'C:\\app\\src\\x.ts', kind: 'edit' }] }), tool('edit', grown)], 'C:\\app');
    expect(x).toMatchObject({ partial: true, added: undefined, removed: undefined });
  });

  it('shows a path in the folder by its place in it, and one outside by its name', () => {
    expect(shownPath('C:/app/src/x.ts', 'C:\\App')).toEqual({ dir: 'src\\', base: 'x.ts', full: 'C:\\app\\src\\x.ts' });
    expect(shownPath('D:/notes/plan.md', 'C:\\app')).toEqual({ dir: '', base: 'plan.md', full: 'D:\\notes\\plan.md' });
  });
});

describe('highlighting a diff’s lines', () => {
  it('keeps a comment that spans lines coloured on each of them', () => {
    const html = highlightLines(['/* one', 'two */', 'let x = 1;'], 'javascript');
    expect(html).toHaveLength(3);
    expect(html?.[1]).toMatch(/^<span class="hljs-comment">two \*\/<\/span>$/);
    expect(highlightLines(['a'], 'no-such-language')).toBeNull();
  });
});

describe('the changed files row', () => {
  afterEach(cleanup);

  it('shows four files then "+N more", and a file’s changes when its chip is clicked', () => {
    const tools = ['a', 'b', 'c', 'd', 'e', 'f'].map((n) => tool('edit', edit(`C:/app/${n}.ts`, `@@ -1 +1 @@\n-${n}\n+${n.toUpperCase()}`)));
    render(createElement(FilesChanged, { files: filesChanged(tools, 'C:\\app'), cwd: 'C:\\app' }));
    expect(screen.getAllByRole('button').map((b) => b.textContent)).toEqual(['a.ts+1−1', 'b.ts+1−1', 'c.ts+1−1', 'd.ts+1−1', '+2 more']);

    fireEvent.click(screen.getByText('+2 more'));
    expect(screen.queryByText('+2 more')).toBeNull();
    expect(screen.getByText('f.ts')).toBeTruthy();

    const chip = screen.getByText('b.ts').closest('button')!;
    fireEvent.click(chip);
    expect(chip.getAttribute('aria-expanded')).toBe('true');
    const panel = screen.getByRole('region', { name: 'Changes to b.ts' });
    expect(panel.querySelector('.diff-row.del')?.textContent).toContain('b');
    expect(panel.querySelector('.diff-row.add')?.textContent).toContain('B');

    fireEvent.click(chip);
    expect(screen.queryByRole('region')).toBeNull();
  });
});
