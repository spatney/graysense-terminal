import { fromMarkdown } from 'mdast-util-from-markdown';
import { gfmFromMarkdown } from 'mdast-util-gfm';
import { gfm } from 'micromark-extension-gfm';
import type { ItemDto, ToolDto } from '../../lib/types';
import { diffText } from './diff';
import type { Block } from './Transcript';

// Find in a conversation: matches come from the items, so a find reaches turns that aren't
// on screen yet; the page then shows the one you're on (its turn, work log and step opened)
// and paints what's on screen.

/** Where in an item a match is: a work-log step's row, what the row opens to, or its text. */
export type FindPart = 'row' | 'detail' | 'text';

export interface Hit {
  /** The turn it's in. */
  block: number;
  /** The item. */
  i: number;
  part: FindPart;
  /** Which match in that part. */
  n: number;
}

/** A literal, case-insensitive search for `query` (none for blank). */
export function matcher(query: string): RegExp | null {
  if (!query.trim()) return null;
  return new RegExp(query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'giu');
}

function count(text: string, re: RegExp): number {
  re.lastIndex = 0;
  let n = 0;
  while (re.exec(text)) n++;
  return n;
}

interface MdNode {
  type: string;
  value?: string;
  alt?: string | null;
  children?: MdNode[];
}

/** What holds blocks, each on its own line on the page (a table's cells too). */
const BLOCKS = new Set(['root', 'blockquote', 'list', 'listItem', 'table', 'tableRow']);

/** Text the page shows for a piece of markdown. */
function textOf(n: MdNode): string {
  switch (n.type) {
    case 'text':
    case 'inlineCode':
    case 'code':
    case 'html':
      return n.value ?? '';
    case 'image':
    case 'imageReference':
      return `[${n.alt || 'image'}]`;
    case 'break':
      return '\n';
    case 'definition':
    case 'thematicBreak':
    case 'footnoteReference':
    case 'footnoteDefinition':
      return '';
  }
  return (n.children ?? []).map(textOf).join(BLOCKS.has(n.type) ? '\n' : '');
}

const MARKUP = /[*_`[\]<>&\\|#~!]|^\s*(?:[-+=]|\d+[.)])/m;
const PARSE = { extensions: [gfm()], mdastExtensions: [gfmFromMarkdown()] };
const plainCache = new Map<string, string>();

/** Markdown as it reads on the page: no markup, and links show their text, not where they go. */
export function plain(md: string): string {
  if (!MARKUP.test(md)) return md;
  let out = plainCache.get(md);
  if (out == null) {
    out = textOf(fromMarkdown(md, PARSE) as MdNode);
    if (plainCache.size >= 2000) plainCache.clear();
    plainCache.set(md, out);
  }
  return out;
}

/** A work-log step's row: what it did, and to what. */
export function stepRow(it: ItemDto): { label: string; target: string | null } {
  switch (it.type) {
    case 'tool':
      return { label: it.label, target: it.target };
    case 'reasoning':
      return { label: it.done ? 'Thought it through' : 'Thinking', target: it.text.split('\n').find((l) => l.trim())?.replace(/^[#*\s]+/, '').slice(0, 120) ?? null };
    case 'subagent':
      return { label: it.name, target: it.description || null };
    case 'permission':
      return { label: it.summary, target: null };
    case 'ask':
      return { label: it.question.split('\n')[0], target: null };
    case 'elicit':
      return { label: it.message.split('\n')[0], target: null };
    case 'plan':
      return { label: 'Plan', target: null };
    case 'switch':
      return { label: 'Model fallback', target: null };
    default:
      return { label: '', target: null };
  }
}

/** What a tool's step opens to (in step with ToolDetail). */
function toolDetail(t: ToolDto, cwd: string | null): string {
  const a = t.args ?? {};
  const output = t.output || t.result || '';
  const out: string[] = [];
  switch (t.family) {
    case 'shell':
      if (a.command) out.push(String(a.command));
      if (output) out.push(output);
      break;
    case 'edit':
      if (t.diff) out.push(diffText(t.diff.text, cwd));
      else if (a.old_str != null || a.new_str != null) out.push(String(a.old_str ?? ''), String(a.new_str ?? ''));
      else if (a.input || a.patch) out.push(String(a.input ?? a.patch));
      if (t.state === 'failed' && output) out.push(output);
      break;
    case 'create':
      if (t.diff) out.push(diffText(t.diff.text, cwd));
      else if (a.file_text != null) out.push(String(a.file_text));
      break;
    case 'task':
      if (a.prompt) out.push(plain(String(a.prompt)));
      if (output) out.push(output);
      break;
    default:
      if ((t.family === 'other' || t.family === 'graysense' || t.family === 'canvas' || t.family === 'sql') && Object.keys(a).length) out.push(JSON.stringify(a, null, 2));
      if (output) out.push(output);
  }
  if (t.progress && t.state === 'running') out.push(t.progress);
  if (t.error) out.push(t.error);
  return out.join('\n');
}

/** What a step's row opens to. */
function stepDetail(it: ItemDto, cwd: string | null): string {
  switch (it.type) {
    case 'tool':
      return toolDetail(it, cwd);
    case 'reasoning':
      return plain(it.text);
    case 'plan':
      return plain(it.plan ?? it.summary);
    default:
      return '';
  }
}

/** A question or approval waiting for an answer, as its card shows it (in step with PromptCard). */
function promptText(it: ItemDto, cwd: string | null): string {
  const lines = (xs: (string | null | undefined | false)[]) => xs.filter(Boolean).join('\n');
  switch (it.type) {
    case 'permission':
      return lines([
        it.headline,
        it.intention,
        it.command,
        it.warning,
        !it.diff && it.fileName,
        it.diff && diffText(it.diff, cwd),
        ...it.paths,
        it.url,
        it.kind === 'mcp' && (it.server || it.tool) && `${it.server ?? ''} › ${it.tool ?? ''}`,
        it.toolDescription,
        it.args,
      ]);
    case 'ask':
      return lines([plain(it.question), ...it.choices]);
    case 'elicit':
      return lines([
        plain(it.message),
        it.url,
        ...(it.url
          ? []
          : it.fields.flatMap((f) => [f.title, f.description, ...((f.kind.type === 'choice' && f.kind.options.length <= 5) || f.kind.type === 'multi' ? f.kind.options.map((o) => o.label) : [])])),
      ]);
    case 'plan':
      return plain(it.summary);
    case 'switch':
      return plain(it.text);
    default:
      return '';
  }
}

/** Every match in the conversation, top to bottom as it shows: in a turn, the work log, then
 *  what's waiting for you, the answer, notices and what was remembered. */
export function findHits(blocks: Block[], query: string, cwd: string | null = null): Hit[] {
  const re = matcher(query);
  const hits: Hit[] = [];
  if (!re) return hits;
  const add = (block: number, i: number, part: FindPart, text: string) => {
    for (let n = 0, c = text ? count(text, re) : 0; n < c; n++) hits.push({ block, i, part, n });
  };
  blocks.forEach((b, bi) => {
    if (b.kind === 'user') {
      add(bi, b.i, 'text', b.item.text);
      return;
    }
    for (const p of b.parts) {
      if (p.t === 'note' && p.item.type === 'assistant') add(bi, p.i, 'text', plain(p.item.text));
      else if (p.t === 'step') {
        const { label, target } = stepRow(p.item);
        add(bi, p.i, 'row', [label, target].filter(Boolean).join('\n'));
        add(bi, p.i, 'detail', stepDetail(p.item, cwd));
      }
    }
    for (const p of b.parts) if (p.t === 'prompt') add(bi, p.i, 'text', promptText(p.item, cwd));
    if (b.answer) add(bi, b.answer.i, 'text', plain(b.answer.item.text));
    for (const p of b.parts) {
      if (p.t === 'notice' && p.item.type === 'notice') add(bi, p.i, 'text', p.item.text);
      else if (p.t === 'memory' && p.item.type === 'memory') add(bi, p.i, 'text', `${p.item.learned ? 'Learned' : 'Remembered'} ${p.item.text}`);
    }
  });
  return hits;
}

// ------------------------------------------------------------------ on the page

/** Text around what was said: line numbers, timings, counts, captions, buttons. */
const SKIP =
  '.diff-no, .diff-sign, .diff-stats, .diff-status, .diff-hunk, .diff-foot, .step-meta, .step-sub, .notice-count, .well-head, .codeblock-head, .memory-chip > .btn, .qcard-head, .qcard-foot, .link-btn, select, .no-find';

/** What starts a new line on the page: a match can't run from one into the next. */
const LINE = 'p, li, tr, td, th, pre, h1, h2, h3, h4, h5, h6, blockquote, div, label, button, .step-label, .step-target';

/** Every match under `root`, in order (a match can cross formatting, like **bold**). */
export function rangesIn(root: Node, re: RegExp): Range[] {
  const nodes: Text[] = [];
  const starts: number[] = [];
  let text = '';
  let line: Element | null | undefined;
  const walk = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: (n) => (n.parentElement?.closest(SKIP) ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
  });
  for (let n = walk.nextNode(); n; n = walk.nextNode()) {
    const at = n.parentElement?.closest(LINE) ?? null;
    if (line !== undefined && at !== line) text += '\n';
    line = at;
    nodes.push(n as Text);
    starts.push(text.length);
    text += n.nodeValue ?? '';
  }
  const out: Range[] = [];
  re.lastIndex = 0;
  let k = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    const start = m.index;
    const end = start + m[0].length;
    while (k + 1 < nodes.length && starts[k + 1] <= start) k++;
    let j = k;
    while (j + 1 < nodes.length && starts[j + 1] < end) j++;
    const r = document.createRange();
    r.setStart(nodes[k], start - starts[k]);
    r.setEnd(nodes[j], end - starts[j]);
    out.push(r);
  }
  return out;
}

/** Bring a match into view: inside a long output or diff, then in the conversation. */
export function reveal(what: Range | Element, scroller: HTMLElement) {
  const from = what instanceof Range ? what.startContainer.parentElement : what.parentElement;
  for (let el = from; el && el !== scroller && scroller.contains(el); el = el.parentElement) {
    const style = getComputedStyle(el);
    const r = what.getBoundingClientRect();
    const box = el.getBoundingClientRect();
    if (el.scrollHeight > el.clientHeight + 1 && /auto|scroll/.test(style.overflowY) && (r.top < box.top || r.bottom > box.bottom)) el.scrollTop += r.top - box.top - (box.height - r.height) / 2;
    if (el.scrollWidth > el.clientWidth + 1 && /auto|scroll/.test(style.overflowX) && (r.left < box.left || r.right > box.right)) el.scrollLeft += r.left - box.left - (box.width - r.width) / 2;
  }
  const r = what.getBoundingClientRect();
  const box = scroller.getBoundingClientRect();
  if (r.top < box.top + 48 || r.bottom > box.bottom - 48) scroller.scrollTop += r.top - box.top - box.height / 3;
}

const painted = new Map<number, { all: Range[]; now: Range | null }>();

/** Paint a bay's matches, and the one it's on (none clears them). */
export function paint(bay: number, all: Range[] | null, now: Range | null = null) {
  if (all) painted.set(bay, { all, now });
  else if (!painted.delete(bay)) return;
  if (typeof CSS === 'undefined' || !CSS.highlights || typeof Highlight === 'undefined') return;
  const every = [...painted.values()];
  CSS.highlights.set('gs-find', new Highlight(...every.flatMap((p) => p.all)));
  const current = new Highlight(...every.flatMap((p) => (p.now ? [p.now] : [])));
  current.priority = 1;
  CSS.highlights.set('gs-find-now', current);
}
