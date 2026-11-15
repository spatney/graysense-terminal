import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { act, captureNote, revealMemory } from '../../lib/api';
import { agoLong, baseName, duration, plural } from '../../lib/format';
import { langOf } from '../../lib/highlight';
import { isImagePath } from '../../lib/images';
import { setDraft, fillDraft, focusComposer, useUi } from '../../lib/store';
import type { AgentDto, AssistantItem, BayDto, FileEdit, HandoffDto, ItemDto, MemoryItem, NoticeItem, QueuedMessage, RecallItem, ToolDto, UserItem } from '../../lib/types';
import { ImageThumb } from '../ImageThumb';
import { CodeBlock, Markdown } from '../Markdown';
import { Icon, copyText, showMenu } from '../ui';
import { filesChanged, missingFrom } from './diff';
import { DiffView, Stats } from './DiffView';
import { FilesChanged } from './FilesChanged';
import { PromptCard, isPrompt } from './Prompts';

// ------------------------------------------------------------------ grouping

type Part = {
  t: 'step' | 'note' | 'prompt' | 'notice' | 'memory';
  i: number;
  item: ItemDto;
  /** How many times a notice came in a row (it shows once, counted). */
  n?: number;
};

interface AgentTurn {
  kind: 'agent';
  key: string;
  parts: Part[];
  answer: { i: number; item: AssistantItem } | null;
  live: boolean;
  /** How long the agent worked on the message before it (ms), when known. */
  took: number | null;
}

interface UserTurn {
  kind: 'user';
  key: string;
  i: number;
  item: UserItem;
}

type Block = AgentTurn | UserTurn;

/** Fold a transcript into turns: your message, then the agent's steps, prompts and answer. */
export function group(items: ItemDto[], busy: boolean, showReasoning: boolean): Block[] {
  const blocks: Block[] = [];
  let cur: AgentTurn | null = null;
  let took: number | null = null;
  const turn = (i: number): AgentTurn => {
    if (!cur) {
      cur = { kind: 'agent', key: `a${i}`, parts: [], answer: null, live: false, took };
      blocks.push(cur);
    }
    return cur;
  };
  // Text the agent wrote before more steps followed is narration, not the answer.
  const narrate = (t: AgentTurn) => {
    if (t.answer) {
      t.parts.push({ t: 'note', i: t.answer.i, item: t.answer.item });
      t.answer = null;
    }
  };
  items.forEach((item, i) => {
    if (item.type === 'user') {
      blocks.push({ kind: 'user', key: `u${i}`, i, item });
      cur = null;
      took = item.took ?? null;
      return;
    }
    const t = turn(i);
    switch (item.type) {
      case 'assistant':
        if (!item.text.trim()) return;
        narrate(t);
        t.answer = { i, item };
        return;
      case 'reasoning':
        if (!showReasoning || !item.text.trim()) return;
        narrate(t);
        t.parts.push({ t: 'step', i, item });
        return;
      case 'tool':
      case 'subagent':
        narrate(t);
        t.parts.push({ t: 'step', i, item });
        return;
      case 'notice': {
        // A long turn can compact its context several times: a notice that comes again is counted, not repeated.
        const prev = t.parts.findLast((p) => p.t === 'notice');
        if (prev?.item.type === 'notice' && prev.item.text === item.text && prev.item.level === item.level) prev.n = (prev.n ?? 1) + 1;
        else t.parts.push({ t: 'notice', i, item });
        return;
      }
      case 'memory':
      case 'recall':
        t.parts.push({ t: 'memory', i, item });
        return;
      default:
        if (isPrompt(item)) {
          // An approval that went through is the step itself; only a denial is worth a row.
          if (item.type === 'permission' && !item.pending && item.outcome !== 'denied') return;
          narrate(t);
          t.parts.push({ t: item.pending ? 'prompt' : 'step', i, item });
        }
    }
  });
  const last = blocks[blocks.length - 1];
  if (busy) {
    if (last?.kind === 'agent') last.live = true;
    else blocks.push({ kind: 'agent', key: `a${items.length}`, parts: [], answer: null, live: true, took: null });
  }
  return blocks;
}

// ------------------------------------------------------------------ small parts

/** Re-render every second while `on`. */
function useNow(on: boolean): number {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!on) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [on]);
  return now;
}

const FAMILY_ICON: Record<ToolDto['family'], string> = {
  read: 'eye',
  edit: 'edit',
  create: 'new-file',
  shell: 'terminal',
  search: 'search',
  web: 'globe',
  task: 'hubot',
  canvas: 'layout',
  memory: 'lightbulb-sparkle',
  skill: 'mortar-board',
  sql: 'database',
  graysense: 'settings-gear',
  other: 'tools',
};

/** old → new as removed and added lines (before the edit reports its diff). */
function Diff({ before, after }: { before: string; after: string }) {
  const del = before.split('\n');
  const add = after.split('\n');
  return (
    <div className="well">
      <div className="line-diff selectable">
        {del.map((l, i) => (
          <div key={`d${i}`} className="del">
            - {l}
          </div>
        ))}
        {add.map((l, i) => (
          <div key={`a${i}`} className="add">
            + {l}
          </div>
        ))}
      </div>
    </div>
  );
}

function Well({ title, text, cmd, actions }: { title?: string; text: string; cmd?: boolean; actions?: React.ReactNode }) {
  return (
    <div className="well">
      {(title || actions) && (
        <div className="well-head">
          <span className="ellipsis grow">{title}</span>
          {actions}
          <button type="button" className="icon-btn sm" data-tip="Copy" onClick={() => void copyText(text)}>
            <Icon name="copy" />
          </button>
        </div>
      )}
      <pre className={cmd ? 'cmd' : ''}>{text}</pre>
    </div>
  );
}

function FileChips({ files }: { files: FileEdit[] }) {
  if (!files.length) return null;
  return (
    <div className="changed-files">
      {files.map(({ path, kind }) =>
        kind === 'delete' ? (
          <button key={path} type="button" className="file-chip" disabled data-tip={`${path} · deleted`}>
            <Icon name="trash" /> <span className="ellipsis">{baseName(path)}</span>
          </button>
        ) : (
          <button key={path} type="button" className="file-chip" data-tip={`${path} · open`} onClick={() => void act({ type: 'openPath', path })}>
            <Icon name="file" /> <span className="ellipsis">{baseName(path)}</span>
          </button>
        ),
      )}
    </div>
  );
}

function ToolDetail({ tool, cwd }: { tool: ToolDto; cwd: string | null }) {
  const a = tool.args ?? {};
  const output = tool.output || tool.result || '';
  // A diff names its files, each with a button to open it; the rest get chips (files past where a long diff was cut).
  const chips = useMemo(() => {
    const files: FileEdit[] = tool.files.length ? tool.files : (tool.family === 'edit' || tool.family === 'create' || tool.family === 'read') && a.path ? [{ path: String(a.path), kind: tool.family === 'create' ? 'create' : 'edit' }] : [];
    return tool.diff ? missingFrom(tool.diff.text, files, cwd) : files;
  }, [tool.files, tool.family, tool.diff, a.path, cwd]);
  const blocks: React.ReactNode[] = [];
  switch (tool.family) {
    case 'shell':
      if (a.command) blocks.push(<Well key="cmd" title={a.description && a.description !== tool.label ? a.description : 'Command'} text={String(a.command)} cmd />);
      if (output) blocks.push(<Well key="out" title="Output" text={output} />);
      break;
    case 'edit':
      if (tool.diff) blocks.push(<DiffView key="diff" diff={tool.diff.text} truncated={tool.diff.truncated} cwd={cwd} />);
      else if (a.old_str != null || a.new_str != null) blocks.push(<Diff key="diff" before={String(a.old_str ?? '')} after={String(a.new_str ?? '')} />);
      else if (a.input || a.patch) blocks.push(<Well key="patch" title="Patch" text={String(a.input ?? a.patch)} />);
      if (tool.state === 'failed' && output) blocks.push(<Well key="out" title="Result" text={output} />);
      break;
    case 'create':
      if (tool.diff) blocks.push(<DiffView key="diff" diff={tool.diff.text} truncated={tool.diff.truncated} cwd={cwd} />);
      else if (a.file_text != null) blocks.push(<CodeBlock key="file" code={String(a.file_text)} lang={langOf(a.path)} maxHeight={320} />);
      break;
    case 'task':
      if (a.prompt) blocks.push(<Markdown key="prompt" className="worklog-note" text={String(a.prompt)} />);
      if (output) blocks.push(<Well key="out" title="Result" text={output} />);
      break;
    default:
      if (tool.family === 'other' || tool.family === 'graysense' || tool.family === 'canvas' || tool.family === 'sql') {
        const shown = Object.keys(a).length ? JSON.stringify(a, null, 2) : '';
        if (shown) blocks.push(<Well key="args" title={tool.mcpServer ? `${tool.mcpServer} · arguments` : 'Arguments'} text={shown} />);
      }
      if (output) blocks.push(<Well key="out" title={tool.family === 'read' ? (a.path ? baseName(String(a.path)) : 'Contents') : 'Result'} text={output} />);
  }
  if (tool.progress && tool.state === 'running') blocks.push(<div key="prog" className="faint">{tool.progress}</div>);
  if (tool.error) blocks.push(<div key="err" className="step-error selectable">{tool.error}</div>);
  if (chips.length) blocks.push(<FileChips key="files" files={chips} />);
  if (!blocks.length) blocks.push(<div key="none" className="faint">No details.</div>);
  return <div className="step-detail">{blocks}</div>;
}

function Step({ part, now, cwd }: { part: Part; now: number; cwd: string | null }) {
  const [open, setOpen] = useState(false);
  const it = part.item;
  if (part.t === 'note' && it.type === 'assistant') {
    return (
      <div className="worklog-note">
        <Markdown text={it.text} />
      </div>
    );
  }
  let icon = 'circle-small';
  let label = '';
  let target: string | null = null;
  let meta: string | null = null;
  let metaTip: string | undefined;
  let stats: React.ReactNode = null;
  let state: 'running' | 'ok' | 'failed' = 'ok';
  let detail: React.ReactNode = null;
  let sub: React.ReactNode = null;
  switch (it.type) {
    case 'tool':
      icon = FAMILY_ICON[it.family];
      label = it.label;
      target = it.target;
      state = it.state;
      meta = it.state === 'running' ? duration(now - it.started) : it.state === 'failed' ? 'failed' : it.elapsed != null && it.elapsed >= 1 ? duration(it.elapsed * 1000) : null;
      if (it.diff) stats = <Stats added={it.diff.added} removed={it.diff.removed} />;
      detail = <ToolDetail tool={it} cwd={cwd} />;
      break;
    case 'reasoning':
      icon = 'lightbulb';
      label = it.done ? 'Thought it through' : 'Thinking';
      target = it.text.split('\n').find((l) => l.trim())?.replace(/^[#*\s]+/, '').slice(0, 120) ?? null;
      state = it.done ? 'ok' : 'running';
      detail = (
        <div className="step-detail">
          <Markdown className="worklog-note" text={it.text} />
        </div>
      );
      break;
    case 'subagent':
      icon = 'hubot';
      label = it.name;
      target = it.description || null;
      state = it.done == null ? 'running' : it.done ? 'ok' : 'failed';
      meta = plural(it.tools, 'step');
      if (it.done == null && it.current) sub = <div className="step-sub mono ellipsis">{it.current}</div>;
      break;
    case 'permission':
      icon = 'shield';
      label = it.summary;
      meta = it.outcome;
      state = it.outcome === 'denied' ? 'failed' : 'ok';
      break;
    case 'ask':
      icon = 'question';
      label = it.question.split('\n')[0];
      meta = it.outcome;
      metaTip = it.outcome ?? undefined;
      break;
    case 'elicit':
      icon = 'edit';
      label = it.message.split('\n')[0];
      meta = it.outcome;
      metaTip = it.outcome ?? undefined;
      break;
    case 'plan':
      icon = 'checklist';
      label = 'Plan';
      meta = it.outcome;
      detail = (
        <div className="step-detail">
          <Markdown className="worklog-note" text={it.plan ?? it.summary} />
        </div>
      );
      break;
    case 'switch':
      icon = 'arrow-swap';
      label = 'Model fallback';
      meta = it.outcome;
      break;
  }
  return (
    <div className={`step ${state} ${open ? 'open' : ''}`}>
      <button type="button" className="step-row" onClick={() => detail && setOpen(!open)} aria-expanded={detail ? open : undefined}>
        <span className="step-ico">{state === 'running' ? <span className="spinner sm" /> : <Icon name={state === 'failed' ? 'error' : icon} />}</span>
        <span className="step-label">{label}</span>
        <span className="step-target">{target}</span>
        {stats}
        {meta && <span className="step-meta" data-tip={metaTip}>{meta}</span>}
        {detail && <Icon name="chevron-right" className="step-caret" />}
      </button>
      {sub}
      {open && detail}
    </div>
  );
}

function kinds(steps: Part[]): { icon: string; n: number; tip: string }[] {
  const count = (f: (it: ItemDto) => boolean) => steps.filter((p) => f(p.item)).length;
  const out = [
    { icon: 'edit', n: count((it) => it.type === 'tool' && (it.family === 'edit' || it.family === 'create')), tip: 'files changed' },
    { icon: 'terminal', n: count((it) => it.type === 'tool' && it.family === 'shell'), tip: 'commands' },
    { icon: 'eye', n: count((it) => it.type === 'tool' && (it.family === 'read' || it.family === 'search')), tip: 'reads and searches' },
    { icon: 'globe', n: count((it) => it.type === 'tool' && it.family === 'web'), tip: 'web' },
    { icon: 'hubot', n: count((it) => it.type === 'subagent'), tip: 'subagents' },
  ];
  return out.filter((k) => k.n > 0);
}

function WorkLog({ turn, agent, now, cwd }: { turn: AgentTurn; agent: AgentDto | null; now: number; cwd: string | null }) {
  const [userOpen, setUserOpen] = useState<boolean | null>(null);
  const [all, setAll] = useState(false);
  const steps = turn.parts.filter((p) => p.t === 'step' || p.t === 'note');
  if (!steps.length && !turn.live) return null;
  const open = userOpen ?? turn.live;
  const tools = steps.map((p) => p.item).filter((it): it is ToolDto => it.type === 'tool');
  const startedAt = tools.length ? Math.min(...tools.map((t) => t.started)) : null;
  const endedAt = tools.length ? Math.max(...tools.map((t) => t.started + (t.elapsed ?? 0) * 1000)) : null;
  const running = [...tools].reverse().find((t) => t.state === 'running');
  const waiting = turn.live && !!agent?.status.attention;
  const title = waiting
    ? 'Waiting for you'
    : turn.live
    ? (running?.label ?? agent?.intent ?? agent?.status.label ?? 'Working')
    : turn.took != null && turn.took > 1500
      ? `Worked for ${duration(turn.took)}`
      : startedAt && endedAt && endedAt - startedAt > 1500
        ? `Worked for ${duration(endedAt - startedAt)}`
        : 'Worked';
  const n = steps.filter((p) => p.t === 'step').length;
  const LIMIT = 8;
  const shown = open && !all && turn.live && steps.length > LIMIT ? steps.slice(-LIMIT) : steps;
  return (
    <div className={`worklog ${turn.live ? 'live' : ''} ${open ? 'open' : ''}`}>
      <button type="button" className="worklog-head" onClick={() => setUserOpen(!open)} aria-expanded={open}>
        {waiting ? <Icon name="debug-pause" className="worklog-wait" /> : turn.live ? <span className="spinner sm" /> : <Icon name="pass" />}
        <span className={`worklog-title ${turn.live && !waiting ? 'shimmer' : ''}`}>{title}</span>
        {turn.live && agent?.busySince && <span className="live-time">{duration(now - agent.busySince)}</span>}
        {n > 0 && <span className="worklog-count">{plural(n, 'step')}</span>}
        {!turn.live && (
          <span className="worklog-kinds">
            {kinds(steps).map((k) => (
              <span key={k.icon} data-tip={`${k.n} ${k.tip}`}>
                <Icon name={k.icon} />
                {k.n}
              </span>
            ))}
          </span>
        )}
        {n > 0 && <Icon name="chevron-right" className="worklog-caret" />}
      </button>
      {open && steps.length > 0 && (
        <div className="worklog-body">
          {shown.length < steps.length && (
            <button type="button" className="worklog-earlier" onClick={() => setAll(true)}>
              Show {plural(steps.length - shown.length, 'earlier step')}
            </button>
          )}
          {shown.map((p) => (
            <Step key={p.i} part={p} now={now} cwd={cwd} />
          ))}
        </div>
      )}
    </div>
  );
}

function RecallChip({ item }: { item: RecallItem }) {
  const [open, setOpen] = useState(false);
  const notes = useUi((s) => item.entries.filter((e) => s.memory.memories.find((m) => m.id === e.id)?.kind === 'note').length);
  const others = item.entries.length - notes;
  const what = [notes ? plural(notes, 'note') : null, others ? plural(others, 'memory', 'memories') : null].filter(Boolean).join(' and ');
  return (
    <div className={`recall${open ? ' open' : ''}`}>
      <button type="button" className="memory-chip" aria-expanded={open} data-tip={open ? undefined : 'What the agent was given'} onClick={() => setOpen((o) => !o)}>
        <Icon name={notes && !others ? 'bookmark' : 'lightbulb-sparkle'} /> Used {what}
        <Icon name="chevron-right" className="worklog-caret" />
      </button>
      {open && <RecallList item={item} />}
    </div>
  );
}

/** What an agent was given from memory, each with a way to fix it. */
function RecallList({ item }: { item: RecallItem }) {
  const memories = useUi((s) => s.memory.memories);
  return (
    <ul className="recall-list">
      {item.entries.map((e) => {
        const m = memories.find((x) => x.id === e.id);
        const live = m && m.state !== 'archived';
        const note = m?.kind === 'note';
        return (
          <li key={e.id} className={live ? undefined : 'gone'}>
            <Icon name={note ? 'bookmark' : 'lightbulb'} />
            <span className="recall-text selectable">{live ? m.text : e.text}</span>
            {!live && <span className="recall-status">{m?.archivedReason ?? 'No longer saved'}</span>}
            <span className="recall-actions">
              {m && (
                <button type="button" className="icon-btn sm" data-tip="Show in Memory" aria-label="Show in Memory" onClick={() => revealMemory(e.id)}>
                  <Icon name="go-to-file" />
                </button>
              )}
              {live && !note && (
                <button type="button" className="icon-btn sm" data-tip="Edit" aria-label="Edit" onClick={() => revealMemory(e.id, true)}>
                  <Icon name="edit" />
                </button>
              )}
              {live && m.state === 'active' && m.kind !== 'rule' && (
                <button
                  type="button"
                  className={`icon-btn sm${m.pinned ? ' active' : ''}`}
                  data-tip={m.pinned ? 'Unpin' : 'Pin: always give agents this'}
                  aria-label={m.pinned ? 'Unpin' : 'Pin'}
                  aria-pressed={m.pinned}
                  onClick={() => void act({ type: 'memoryPin', id: e.id, pinned: !m.pinned })}
                >
                  <Icon name={m.pinned ? 'pinned' : 'pin'} />
                </button>
              )}
              {live && !note && (
                <button
                  type="button"
                  className="icon-btn sm"
                  data-tip="Wrong or outdated: forget it, and tell the agent"
                  aria-label="Wrong or outdated"
                  onClick={() => void act({ type: 'memoryFlag', id: e.id })}
                >
                  <Icon name="thumbsdown" />
                </button>
              )}
              {m?.state === 'archived' && m.archivedReason === 'Marked wrong' && (
                <button type="button" className="btn ghost sm" onClick={() => void act({ type: 'memoryRestore', id: e.id })}>
                  Undo
                </button>
              )}
            </span>
          </li>
        );
      })}
    </ul>
  );
}

function MemoryChip({ bay, i, item }: { bay: number; i: number; item: MemoryItem | RecallItem }) {
  if (item.type === 'recall') return <RecallChip item={item} />;
  return (
    <span className={`memory-chip ${item.undone ? 'undone' : ''}`}>
      <Icon name={item.learned ? 'sparkle' : 'lightbulb-sparkle'} />
      <span className="ellipsis">
        <b>{item.learned ? 'Learned' : 'Remembered'}</b> {item.text}
      </span>
      {!item.undone && (
        <>
          <button type="button" className="btn ghost sm" onClick={() => item.ids[0] && revealMemory(item.ids[0])}>
            View
          </button>
          <button type="button" className="btn ghost sm" onClick={() => void act({ type: 'forgetMemories', bay, index: i })}>
            Undo
          </button>
        </>
      )}
    </span>
  );
}

function Notice({ item, count = 1 }: { item: NoticeItem; count?: number }) {
  const icon = { info: 'info', ok: 'pass', warn: 'warning', error: 'error' }[item.level];
  return (
    <div className={`notice ${item.level} selectable`}>
      <Icon name={icon} />
      <span>{item.text}</span>
      {count > 1 && <span className="notice-count">×{count}</span>}
    </div>
  );
}

interface AgentTurnProps {
  bayId: number;
  turn: AgentTurn;
  /** Only for the live turn (its status and timing change constantly). */
  agent: AgentDto | null;
  sessionId: string | null;
  sessionTitle: string;
  /** The session's folder, which file paths show relative to. */
  cwd: string | null;
  /** The message this turn answered, when it can be retried (the last turn, settled). */
  retry: number | null;
}

const sameTurn = (a: AgentTurnProps, b: AgentTurnProps) =>
  a.bayId === b.bayId &&
  a.agent === b.agent &&
  a.sessionId === b.sessionId &&
  a.sessionTitle === b.sessionTitle &&
  a.cwd === b.cwd &&
  a.retry === b.retry &&
  a.turn.live === b.turn.live &&
  a.turn.took === b.turn.took &&
  a.turn.answer?.item === b.turn.answer?.item &&
  a.turn.parts.length === b.turn.parts.length &&
  a.turn.parts.every((p, i) => p.item === b.turn.parts[i].item && p.t === b.turn.parts[i].t && p.n === b.turn.parts[i].n);

const AgentTurnView = memo(function AgentTurnView({ bayId, turn, agent, sessionId, sessionTitle, cwd, retry }: AgentTurnProps) {
  const now = useNow(turn.live);
  const prompts = turn.parts.filter((p) => p.t === 'prompt');
  const notices = turn.parts.filter((p) => p.t === 'notice');
  const memories = turn.parts.filter((p) => p.t === 'memory');
  const answer = turn.answer;
  const streaming = answer && !answer.item.done;
  const changed = useMemo(
    () =>
      turn.live
        ? []
        : filesChanged(
            turn.parts.map((p) => p.item).filter((it): it is ToolDto => it.type === 'tool'),
            cwd,
          ),
    [turn, cwd],
  );
  return (
    <div className="turn">
      <WorkLog turn={turn} agent={agent} now={now} cwd={cwd} />
      {prompts.map((p) => (
        <PromptCard key={p.i} bay={bayId} index={p.i} item={p.item} cwd={cwd} />
      ))}
      {answer && (
        <div className="answer">
          <Markdown text={answer.item.text} />
          {streaming && <span className="stream-caret" />}
        </div>
      )}
      {notices.map((p) => (
        <Notice key={p.i} item={p.item as NoticeItem} count={p.n} />
      ))}
      {changed.length > 0 && <FilesChanged files={changed} cwd={cwd} />}
      {memories.map((p) => (
        <MemoryChip key={p.i} bay={bayId} i={p.i} item={p.item as MemoryItem | RecallItem} />
      ))}
      {(answer?.item.done || retry !== null) && (
        <div className="msg-actions">
          {answer?.item.done && (
            <>
              <button type="button" className="icon-btn sm" data-tip="Copy" onClick={() => void copyText(answer.item.text)}>
                <Icon name="copy" />
              </button>
              <button type="button" className="icon-btn sm" data-tip="Save to notes" onClick={() => captureNote({ text: answer.item.text, session: sessionId, title: sessionTitle })}>
                <Icon name="bookmark" />
              </button>
            </>
          )}
          {retry !== null && (
            <>
              <button type="button" className="icon-btn sm" data-tip="Retry" onClick={() => void act({ type: 'rewindStart', bay: bayId, index: retry, then: 'retry' })}>
                <Icon name="refresh" />
              </button>
              <button type="button" className="icon-btn sm" data-tip="Retry with another model" onClick={(e) => retryMenu(e.currentTarget, bayId, retry)}>
                <Icon name="chevron-down" />
              </button>
            </>
          )}
        </div>
      )}
    </div>
  );
}, sameTurn);

function UserMessage({ text, files, queued }: { text: string; files: string[]; queued?: boolean }) {
  const images = files.filter(isImagePath);
  const others = files.filter((f) => !isImagePath(f));
  const chip = (f: string) => (
    <span key={f} className="file-chip" data-tip={f !== baseName(f) ? f : undefined}>
      <Icon name="file" /> <span className="ellipsis">{baseName(f)}</span>
    </span>
  );
  return (
    <div className="user-msg">
      {images.length > 0 && (
        <div className="user-images">
          {images.map((f) => (
            <ImageThumb key={f} path={f} fallback={chip(f)} />
          ))}
        </div>
      )}
      <div className={`user-bubble ${queued ? 'queued' : ''}`}>{text}</div>
      {others.length > 0 && <div className="user-files">{others.map(chip)}</div>}
    </div>
  );
}

const UserTurnView = memo(function UserTurnView({
  bayId,
  i,
  it,
  canRewind,
  sessionId,
  sessionTitle,
}: {
  bayId: number;
  i: number;
  it: UserItem;
  canRewind: boolean;
  sessionId: string | null;
  sessionTitle: string;
}) {
  const rewindable = canRewind && !!it.eventId;
  return (
    <div className="turn turn--user">
      <UserMessage text={it.text} files={it.files} />
      <div className="msg-actions">
        <button type="button" className="icon-btn sm" data-tip="Copy" onClick={() => void copyText(it.text)}>
          <Icon name="copy" />
        </button>
        <button
          type="button"
          className="icon-btn sm"
          data-tip={rewindable ? 'Edit and send again' : 'Copy to the message box'}
          onClick={() => {
            if (rewindable) {
              void act({ type: 'rewindStart', bay: bayId, index: i, then: 'edit' });
              return;
            }
            setDraft(bayId, it.text);
            focusComposer(bayId);
          }}
        >
          <Icon name="edit" />
        </button>
        {rewindable && (
          <button type="button" className="icon-btn sm" data-tip="Undo from here" onClick={() => void act({ type: 'rewindStart', bay: bayId, index: i, then: 'undo' })}>
            <Icon name="discard" />
          </button>
        )}
        {sessionId && it.eventId && (
          <button type="button" className="icon-btn sm" data-tip="Fork a new session from here" onClick={() => void act({ type: 'fork', bay: bayId, index: i })}>
            <Icon name="repo-forked" />
          </button>
        )}
        <button type="button" className="icon-btn sm" data-tip="Save to notes" onClick={() => captureNote({ text: it.text, session: sessionId, title: sessionTitle })}>
          <Icon name="bookmark" />
        </button>
      </div>
    </div>
  );
});

/** Pick a model to retry a message on. */
function retryMenu(anchor: Element, bay: number, index: number) {
  const { app, bays } = useUi.getState();
  const current = bays.find((b) => b.id === bay)?.agent?.model ?? app?.runtimeModel ?? null;
  const models = app?.models ?? [];
  showMenu(
    anchor,
    [
      { section: 'Retry with' },
      ...(models.length
        ? models.map((m) => ({
            label: m.name,
            detail: m.multiplier != null ? `${Number(m.multiplier.toFixed(2))}×` : undefined,
            checked: m.id === current,
            onSelect: () => void act({ type: 'rewindStart', bay, index, then: 'retry', model: m.id }),
          }))
        : [{ label: 'Loading models…', disabled: true, onSelect: () => {} }]),
    ],
    { above: true },
  );
}

/** A message sent mid-turn, until the agent reads it. */
function QueuedTurnView({ m }: { m: QueuedMessage }) {
  return (
    <div className="turn turn--user">
      <UserMessage text={m.text} files={m.files} queued />
      <span className="turn-queued">{m.steer ? 'Steering: Copilot reads this at its next step' : 'Queued: sends when the agent is ready'}</span>
    </div>
  );
}

/** Where a resumed session left off, from what was learned when it was last used. */
function HandoffCard({ bayId, handoff }: { bayId: number; handoff: HandoffDto }) {
  const dismiss = () => void act({ type: 'handoffDismiss', bay: bayId });
  const next = handoff.next;
  return (
    <section className="handoff" aria-label="Where you left off">
      <div className="handoff-head">
        <Icon name="history" />
        <b>Where you left off</b>
        <span className="faint">{agoLong(handoff.at / 1000)}</span>
        <button type="button" className="icon-btn sm" data-tip="Dismiss" aria-label="Dismiss" onClick={dismiss}>
          <Icon name="close" />
        </button>
      </div>
      <p>{handoff.summary}</p>
      {next && (
        <div className="handoff-next">
          <span>
            <span className="faint">Next: </span>
            {next}
          </span>
          <button
            type="button"
            className="btn sm"
            data-tip="Put this in the message box"
            onClick={() => {
              fillDraft(bayId, next);
              dismiss();
            }}
          >
            Continue
          </button>
        </div>
      )}
    </section>
  );
}

// ------------------------------------------------------------------ transcript

const PAGE = 60;

export function Transcript({ bay, items, visible }: { bay: BayDto; items: ItemDto[]; visible: boolean }) {
  const agent = bay.agent!;
  const showReasoning = useUi((s) => s.settings?.show_reasoning ?? true);
  const live = agent.status.busy || agent.status.attention || agent.status.kind === 'booting';
  const blocks = useMemo(() => group(items, live, showReasoning), [items, live, showReasoning]);
  const [limit, setLimit] = useState(PAGE);
  const scroller = useRef<HTMLDivElement>(null);
  const feed = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const saved = useRef(0);
  const [away, setAway] = useState(false);

  const toBottom = () => {
    const el = scroller.current;
    if (el) el.scrollTop = el.scrollHeight;
  };

  useLayoutEffect(() => {
    if (stick.current) toBottom();
  }, [blocks]);

  useEffect(() => {
    const el = feed.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      if (stick.current) toBottom();
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    const sent = (e: Event) => {
      if ((e as CustomEvent).detail !== bay.id) return;
      stick.current = true;
      setAway(false);
      requestAnimationFrame(toBottom);
    };
    window.addEventListener('gs-sent', sent);
    return () => window.removeEventListener('gs-sent', sent);
  }, [bay.id]);

  useLayoutEffect(() => {
    if (!visible) return;
    const el = scroller.current;
    if (!el) return;
    if (stick.current) toBottom();
    else el.scrollTop = saved.current;
  }, [visible]);

  const onScroll = () => {
    const el = scroller.current;
    if (!el || !visible) return;
    saved.current = el.scrollTop;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
    stick.current = atBottom;
    setAway(!atBottom);
  };

  const shown = blocks.length > limit ? blocks.slice(blocks.length - limit) : blocks;
  const title = bay.title;
  // The last answer can be retried: the message before it is rewound to and sent again.
  const lastBlock = blocks[blocks.length - 1];
  const asked = blocks[blocks.length - 2];
  const retryAt = agent.canRewind && lastBlock?.kind === 'agent' && !lastBlock.live && asked?.kind === 'user' && asked.item.eventId ? asked.i : null;
  return (
    <>
      <div className="chat-scroll" ref={scroller} onScroll={onScroll}>
        <div className="chat-feed" ref={feed}>
          {shown.length < blocks.length && (
            <button type="button" className="btn ghost sm chat-earlier" onClick={() => setLimit((l) => l + PAGE)}>
              Show earlier messages
            </button>
          )}
          {shown.map((b) =>
            b.kind === 'user' ? (
              <UserTurnView key={b.key} bayId={bay.id} i={b.i} it={b.item} canRewind={agent.canRewind} sessionId={agent.sessionId} sessionTitle={title} />
            ) : (
              <AgentTurnView
                key={b.key}
                bayId={bay.id}
                turn={b}
                agent={b.live ? agent : null}
                sessionId={agent.sessionId}
                sessionTitle={title}
                cwd={bay.cwd || null}
                retry={b === lastBlock ? retryAt : null}
              />
            ),
          )}
          {agent.queued.map((m, i) => (
            <QueuedTurnView key={`queued-${i}`} m={m} />
          ))}
          {agent.handoff && !agent.status.busy && <HandoffCard bayId={bay.id} handoff={agent.handoff} />}
        </div>
      </div>
      {away && (
        <button
          type="button"
          className="jump-latest"
          onClick={() => {
            stick.current = true;
            setAway(false);
            toBottom();
          }}
        >
          <Icon name="arrow-down" /> Jump to latest
        </button>
      )}
    </>
  );
}
