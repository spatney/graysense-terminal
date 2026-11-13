import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { act, captureNote, revealMemory } from '../../lib/api';
import { baseName, duration, plural } from '../../lib/format';
import { setDraft, focusComposer, useUi } from '../../lib/store';
import type { AgentDto, AssistantItem, BayDto, ItemDto, MemoryItem, NoticeItem, QueuedMessage, RecallItem, ToolDto, UserItem } from '../../lib/types';
import { CodeBlock, Markdown } from '../Markdown';
import { Icon, copyText } from '../ui';
import { PromptCard, isPrompt } from './Prompts';

// ------------------------------------------------------------------ grouping

type Part = { t: 'step' | 'note' | 'prompt' | 'notice' | 'memory'; i: number; item: ItemDto };

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
      case 'notice':
        t.parts.push({ t: 'notice', i, item });
        return;
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

function langOf(path: string | undefined): string | undefined {
  const ext = path?.split('.').pop()?.toLowerCase();
  const map: Record<string, string> = { ts: 'typescript', tsx: 'typescript', js: 'javascript', jsx: 'javascript', rs: 'rust', py: 'python', ps1: 'powershell', sh: 'bash', md: 'markdown', json: 'json', yml: 'yaml', yaml: 'yaml', toml: 'ini', cs: 'csharp', go: 'go', html: 'xml', css: 'css', sql: 'sql', java: 'java' };
  return ext ? map[ext] : undefined;
}

/** old → new as removed and added lines. */
function Diff({ before, after }: { before: string; after: string }) {
  const del = before.split('\n');
  const add = after.split('\n');
  return (
    <div className="well">
      <div className="diff selectable">
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

/** A unified diff (from a permission request). */
export function UnifiedDiff({ text }: { text: string }) {
  return (
    <div className="well">
      <div className="diff selectable">
        {text.split('\n').map((l, i) => (
          <div key={i} className={l.startsWith('+') && !l.startsWith('+++') ? 'add' : l.startsWith('-') && !l.startsWith('---') ? 'del' : l.startsWith('@@') ? 'hunk' : ''}>
            {l || ' '}
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

function FileChips({ files }: { files: string[] }) {
  if (!files.length) return null;
  return (
    <div className="changed-files">
      {files.map((f) => (
        <button key={f} type="button" className="file-chip" data-tip={`${f} · open`} onClick={() => void act({ type: 'openPath', path: f })}>
          <Icon name="file" /> <span className="ellipsis">{baseName(f)}</span>
        </button>
      ))}
    </div>
  );
}

function ToolDetail({ tool }: { tool: ToolDto }) {
  const a = tool.args ?? {};
  const output = tool.output || tool.result || '';
  const blocks: React.ReactNode[] = [];
  switch (tool.family) {
    case 'shell':
      if (a.command) blocks.push(<Well key="cmd" title={a.description && a.description !== tool.label ? a.description : 'Command'} text={String(a.command)} cmd />);
      if (output) blocks.push(<Well key="out" title="Output" text={output} />);
      break;
    case 'edit':
      if (a.old_str != null || a.new_str != null) blocks.push(<Diff key="diff" before={String(a.old_str ?? '')} after={String(a.new_str ?? '')} />);
      else if (a.input || a.patch) blocks.push(<Well key="patch" title="Patch" text={String(a.input ?? a.patch)} />);
      if (tool.state === 'failed' && output) blocks.push(<Well key="out" title="Result" text={output} />);
      break;
    case 'create':
      if (a.file_text != null) blocks.push(<CodeBlock key="file" code={String(a.file_text)} lang={langOf(a.path)} maxHeight={320} />);
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
  const files = tool.files.length ? tool.files : (tool.family === 'edit' || tool.family === 'create' || tool.family === 'read') && a.path ? [String(a.path)] : [];
  if (files.length) blocks.push(<FileChips key="files" files={files} />);
  if (!blocks.length) blocks.push(<div key="none" className="faint">No details.</div>);
  return <div className="step-detail">{blocks}</div>;
}

function Step({ part, now }: { part: Part; now: number }) {
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
      detail = <ToolDetail tool={it} />;
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
      break;
    case 'elicit':
      icon = 'edit';
      label = it.message.split('\n')[0];
      meta = it.outcome;
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
        {meta && <span className="step-meta">{meta}</span>}
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

function WorkLog({ turn, agent, now }: { turn: AgentTurn; agent: AgentDto | null; now: number }) {
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
            <Step key={p.i} part={p} now={now} />
          ))}
        </div>
      )}
    </div>
  );
}

function RecallChip({ item }: { item: RecallItem }) {
  const notes = useUi((s) => item.entries.filter((e) => s.memory.memories.find((m) => m.id === e.id)?.kind === 'note').length);
  const others = item.entries.length - notes;
  const what = [notes ? plural(notes, 'note') : null, others ? plural(others, 'memory', 'memories') : null].filter(Boolean).join(' and ');
  return (
    <button type="button" className="memory-chip" data-tip={item.entries.map((e) => `• ${e.text.split('\n')[0]}`).join('\n')} onClick={() => item.entries[0] && revealMemory(item.entries[0].id)}>
      <Icon name={notes && !others ? 'bookmark' : 'lightbulb-sparkle'} /> Used {what}
    </button>
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

function Notice({ item }: { item: NoticeItem }) {
  const icon = { info: 'info', ok: 'pass', warn: 'warning', error: 'error' }[item.level];
  return (
    <div className={`notice ${item.level} selectable`}>
      <Icon name={icon} />
      <span>{item.text}</span>
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
}

const sameTurn = (a: AgentTurnProps, b: AgentTurnProps) =>
  a.bayId === b.bayId &&
  a.agent === b.agent &&
  a.sessionId === b.sessionId &&
  a.sessionTitle === b.sessionTitle &&
  a.turn.live === b.turn.live &&
  a.turn.took === b.turn.took &&
  a.turn.answer?.item === b.turn.answer?.item &&
  a.turn.parts.length === b.turn.parts.length &&
  a.turn.parts.every((p, i) => p.item === b.turn.parts[i].item && p.t === b.turn.parts[i].t);

const AgentTurnView = memo(function AgentTurnView({ bayId, turn, agent, sessionId, sessionTitle }: AgentTurnProps) {
  const now = useNow(turn.live);
  const prompts = turn.parts.filter((p) => p.t === 'prompt');
  const notices = turn.parts.filter((p) => p.t === 'notice');
  const memories = turn.parts.filter((p) => p.t === 'memory');
  const answer = turn.answer;
  const streaming = answer && !answer.item.done;
  const changed = turn.live
    ? []
    : [
        ...new Set(
          turn.parts
            .map((p) => p.item)
            .filter((it): it is ToolDto => it.type === 'tool' && (it.family === 'edit' || it.family === 'create') && it.state === 'ok')
            .flatMap((t) => (t.files.length ? t.files : t.args?.path ? [String(t.args.path)] : [])),
        ),
      ];
  return (
    <div className="turn">
      <WorkLog turn={turn} agent={agent} now={now} />
      {prompts.map((p) => (
        <PromptCard key={p.i} bay={bayId} index={p.i} item={p.item} />
      ))}
      {answer && (
        <div className="answer">
          <Markdown text={answer.item.text} />
          {streaming && <span className="stream-caret" />}
        </div>
      )}
      {turn.live && agent && !streaming && !prompts.length && !turn.parts.some((p) => p.t === 'step') && (
        <div className="live-line">
          <span className="live-orb" />
          <span className="shimmer">{agent.intent ?? agent.status.label}</span>
          {agent.busySince && <span className="live-time">{duration(now - agent.busySince)}</span>}
        </div>
      )}
      {notices.map((p) => (
        <Notice key={p.i} item={p.item as NoticeItem} />
      ))}
      {changed.length > 0 && <FileChips files={changed} />}
      {memories.map((p) => (
        <MemoryChip key={p.i} bay={bayId} i={p.i} item={p.item as MemoryItem | RecallItem} />
      ))}
      {answer && answer.item.done && (
        <div className="msg-actions">
          <button type="button" className="icon-btn sm" data-tip="Copy" onClick={() => void copyText(answer.item.text)}>
            <Icon name="copy" />
          </button>
          <button type="button" className="icon-btn sm" data-tip="Save to notes" onClick={() => captureNote({ text: answer.item.text, session: sessionId, title: sessionTitle })}>
            <Icon name="bookmark" />
          </button>
        </div>
      )}
    </div>
  );
}, sameTurn);

function UserMessage({ text, files, queued }: { text: string; files: string[]; queued?: boolean }) {
  return (
    <div className="user-msg">
      <div className={`user-bubble ${queued ? 'queued' : ''}`}>{text}</div>
      {files.length > 0 && (
        <div className="user-files">
          {files.map((f) => (
            <span key={f} className="file-chip">
              <Icon name="file" /> <span className="ellipsis">{f}</span>
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

const UserTurnView = memo(function UserTurnView({ bayId, it, sessionId, sessionTitle }: { bayId: number; it: UserItem; sessionId: string | null; sessionTitle: string }) {
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
          data-tip="Edit and send again"
          onClick={() => {
            setDraft(bayId, it.text);
            focusComposer(bayId);
          }}
        >
          <Icon name="edit" />
        </button>
        <button type="button" className="icon-btn sm" data-tip="Save to notes" onClick={() => captureNote({ text: it.text, session: sessionId, title: sessionTitle })}>
          <Icon name="bookmark" />
        </button>
      </div>
    </div>
  );
});

/** A message sent mid-turn, until the agent reads it. */
function QueuedTurnView({ m }: { m: QueuedMessage }) {
  return (
    <div className="turn turn--user">
      <UserMessage text={m.text} files={m.files} queued />
      <span className="turn-queued">{m.steer ? 'Steering: Copilot reads this at its next step' : 'Queued: sends when the agent is ready'}</span>
    </div>
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
              <UserTurnView key={b.key} bayId={bay.id} it={b.item} sessionId={agent.sessionId} sessionTitle={title} />
            ) : (
              <AgentTurnView key={b.key} bayId={bay.id} turn={b} agent={b.live ? agent : null} sessionId={agent.sessionId} sessionTitle={title} />
            ),
          )}
          {agent.queued.map((m, i) => (
            <QueuedTurnView key={`queued-${i}`} m={m} />
          ))}
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
