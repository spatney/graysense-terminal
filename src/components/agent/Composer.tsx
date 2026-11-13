import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { act, completePath, newSession, newShell, pickFiles, pickFolder, savePastedImage } from '../../lib/api';
import { effortLabel, human } from '../../lib/format';
import { activeMention, completionQuery, insertMention, mentionedFiles } from '../../lib/mentions';
import { fileIcon } from '../../lib/notes';
import { setDraft, toast, useUi } from '../../lib/store';
import type { BayDto, BayId, DirEntry, ModeId, ModelDto } from '../../lib/types';
import { Icon, Seg, showMenu, useAutosize } from '../ui';
import { ModelPicker, useModelChoice } from './ModelPicker';

const NO_MODELS: ModelDto[] = [];

const MODES: { value: ModeId; label: string; tip: string; icon: string }[] = [
  { value: 'interactive', label: 'Ask', tip: 'Ask before acting (Shift+Tab to switch)', icon: 'comment' },
  { value: 'plan', label: 'Plan', tip: 'Plan first, then build when you approve', icon: 'checklist' },
  { value: 'autopilot', label: 'Autopilot', tip: 'Work through the task on its own', icon: 'rocket' },
];

interface Slash {
  name: string;
  args: string;
  desc: string;
}

const SLASH: Slash[] = [
  { name: 'model', args: '<name>', desc: 'Switch the model' },
  { name: 'effort', args: '<level>', desc: 'How hard the agent thinks (low, medium, high…)' },
  { name: 'mode', args: 'ask | plan | autopilot', desc: 'Change how much the agent does alone' },
  { name: 'attach', args: '[path]', desc: 'Attach files to the next message' },
  { name: 'rename', args: '<name>', desc: 'Name this session' },
  { name: 'learn', args: '', desc: 'Learn from this conversation now' },
  { name: 'clear', args: '', desc: 'Clear this transcript' },
  { name: 'stop', args: '', desc: 'Stop the current turn' },
  { name: 'terminal', args: '', desc: 'Open a terminal in this folder' },
  { name: 'new', args: '', desc: 'Start a new session' },
];

/** How full the context window is, as a slim meter (a ring reads like a spinner at this size). */
function Ring({ used, limit }: { used: number; limit: number }) {
  const pct = Math.min(1, used / limit);
  const level = pct > 0.9 ? 'danger' : pct > 0.75 ? 'warn' : '';
  return (
    <span className={`ctx-meter ${level}`} data-tip={`Context ${Math.round(pct * 100)}% full · ${human(used)} of ${human(limit)} tokens`}>
      <i style={{ width: `${Math.max(4, pct * 100)}%` }} />
    </span>
  );
}

/**
 * Images on the clipboard: a screenshot, or a copied picture or image file. A copy that carries
 * text too (Office copies cells and slides as text and a picture) pastes as text.
 */
function pastedImages(data: DataTransfer): File[] {
  if (data.getData('text/plain').trim()) return [];
  const files = data.files.length ? Array.from(data.files) : Array.from(data.items, (item) => item.getAsFile());
  return files.filter((f): f is File => !!f && f.type.startsWith('image/'));
}

/**
 * The message box. With a bay it talks to that agent; without one (the home screen) sending
 * starts a new session with the message.
 */
export function Composer({ bay, autoFocus, placeholder }: { bay: BayDto | null; autoFocus?: boolean; placeholder?: string }) {
  const id = bay?.id ?? 0;
  const agent = bay?.agent ?? null;
  const text = useUi((s) => s.drafts[id] ?? '');
  const focusTick = useUi((s) => s.focusTick[id] ?? 0);
  const models = useUi((s) => s.app?.models ?? NO_MODELS);
  const defaultDir = useUi((s) => s.app?.defaultDir ?? '');
  const home = useUi((s) => s.app?.home ?? '');
  const choice = useModelChoice(bay);
  const [picker, setPicker] = useState<HTMLElement | null>(null);
  const closePicker = useCallback(() => setPicker(null), []);
  const ref = useRef<HTMLTextAreaElement>(null);
  const [hot, setHot] = useState(0);
  const [history, setHistory] = useState<number | null>(null);
  const [selection, setSelection] = useState({ start: 0, end: 0 });
  const [focused, setFocused] = useState(false);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const [fileHot, setFileHot] = useState(0);
  const [completion, setCompletion] = useState<{ key: string; entries: DirEntry[]; error: string | null } | null>(null);
  const fileMenu = useRef<HTMLDivElement>(null);
  const submitting = useRef(false);
  // On the home screen a pasted image starts a session; pastes made while it opens join it.
  const homeSession = useRef<Promise<BayId | undefined> | null>(null);
  useAutosize(ref, text, 300);

  const mention = focused ? activeMention(text, selection.start, selection.end) : null;
  const mentionKey = JSON.stringify([id, text, selection.start, selection.end]);
  const root = agent?.deferred ? defaultDir : bay?.cwd ?? defaultDir;
  const fileOpen = !!mention && !!root && dismissed !== mentionKey;
  const query = fileOpen ? completionQuery(mention!.query, root, home) : null;
  const directory = query?.directory;
  const prefix = query?.prefix;
  const queryKey = query ? JSON.stringify([directory, prefix]) : '';
  const results = completion?.key === queryKey ? completion : null;
  const files = results?.entries ?? [];

  useEffect(() => {
    setFileHot(0);
    if (directory === undefined || prefix === undefined) return;
    let live = true;
    const timer = window.setTimeout(() => {
      completePath(directory, prefix)
        .then((entries) => live && setCompletion({ key: queryKey, entries, error: null }))
        .catch((error) => live && setCompletion({ key: queryKey, entries: [], error: String(error) }));
    }, 100);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [directory, prefix, queryKey]);

  useEffect(() => {
    fileMenu.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' });
  }, [fileHot, results]);

  useEffect(() => {
    if (autoFocus || focusTick) requestAnimationFrame(() => ref.current?.focus());
  }, [focusTick, autoFocus]);

  const busy = !!agent?.status.busy;
  const slash = useMemo(() => {
    const m = /^\/(\w*)$/.exec(text);
    if (!m) return [];
    return SLASH.filter((c) => c.name.startsWith(m[1].toLowerCase()));
  }, [text]);
  useEffect(() => setHot(0), [slash.length]);

  const set = (v: string) => setDraft(id, v);

  const chooseFile = (entry: DirEntry) => {
    if (!mention) return;
    const next = insertMention(text, mention, entry);
    set(next.text);
    setSelection({ start: next.caret, end: next.caret });
    setHistory(null);
    requestAnimationFrame(() => {
      ref.current?.focus();
      ref.current?.setSelectionRange(next.caret, next.caret);
    });
  };

  const runSlash = async (line: string): Promise<boolean> => {
    const m = /^\/(\w+)\s*(.*)$/s.exec(line.trim());
    if (!m || !bay) return false;
    const [, cmd, rest] = m;
    const arg = rest.trim();
    switch (cmd.toLowerCase()) {
      case 'model': {
        if (!arg) {
          toast('info', `Model: ${choice.name}. Use /model <name>.`);
          return true;
        }
        const q = arg.toLowerCase();
        const pick = models.find((x) => x.id.toLowerCase() === q) ?? models.find((x) => x.id.toLowerCase().includes(q) || x.name.toLowerCase().includes(q));
        if (!pick) toast('warn', `No model matches “${arg}”.`);
        else await act({ type: 'setModel', bay: bay.id, model: pick.id });
        return true;
      }
      case 'effort':
      case 'think': {
        const m = choice.model;
        if (!m?.efforts.length) {
          toast('info', `${choice.label} doesn’t have thinking levels.`);
          return true;
        }
        const levels = m.efforts.map((e) => effortLabel(e).toLowerCase());
        if (!arg) {
          toast('info', `Thinking: ${choice.effort ? effortLabel(choice.effort) : 'default'}. Use /effort ${m.efforts.join(' | ')}.`);
          return true;
        }
        const a = arg.toLowerCase();
        const want = m.efforts.find((e, i) => e === a || levels[i] === a || effortLabel(e, true).toLowerCase() === a);
        if (!want) toast('warn', `${choice.label} thinks at ${m.efforts.map((e) => effortLabel(e)).join(', ')}.`);
        else await act({ type: 'setModel', bay: bay.id, model: m.id, effort: want });
        return true;
      }
      case 'mode':
        await act({ type: 'setMode', bay: bay.id, mode: arg || 'interactive' });
        return true;
      case 'attach': {
        if (!arg) {
          const files = await pickFiles('Attach files', bay.cwd);
          if (files.length) await act({ type: 'attach', bay: bay.id, paths: files });
        } else {
          const p = arg.replace(/^"|"$/g, '');
          const full = /^[a-zA-Z]:[\\/]|^\\\\/.test(p) ? p : `${bay.cwd}\\${p}`;
          await act({ type: 'attach', bay: bay.id, paths: [full] });
        }
        return true;
      }
      case 'rename':
        await act({ type: 'rename', bay: bay.id, name: arg });
        return true;
      case 'learn':
        await act({ type: 'learnNow', bay: bay.id });
        return true;
      case 'clear':
        await act({ type: 'clearTranscript', bay: bay.id });
        return true;
      case 'stop':
      case 'abort':
        await act({ type: 'abort', bay: bay.id });
        return true;
      case 'terminal':
        await newShell(bay.cwd);
        return true;
      case 'new':
        await newSession();
        return true;
      default:
        return false;
    }
  };

  /** `queue`: if the agent is mid-turn, wait for the turn to end instead of steering it. */
  const submit = async (queue = false) => {
    const value = text.trim();
    if (!value || submitting.current) return;
    submitting.current = true;
    try {
      setHistory(null);
      const paths = mentionedFiles(value);
      if (!bay && !paths.length) {
        set('');
        const created = await newSession(value);
        if (created === undefined && !useUi.getState().drafts[id]) set(value);
        return;
      }
      if (value.startsWith('/') && (await runSlash(value))) {
        set('');
        return;
      }
      const target = bay?.id ?? await newSession();
      if (target === undefined) return;
      if (!bay) {
        setDraft(target, value);
        if (useUi.getState().drafts[id] === text) set('');
      }
      if (paths.length && await act({ type: 'attach', bay: target, paths }) === undefined) return;
      if (useUi.getState().drafts[target] === (bay ? text : value)) setDraft(target, '');
      window.dispatchEvent(new CustomEvent('gs-sent', { detail: target }));
      const ok = await act({ type: 'send', bay: target, text: value, queue });
      // Sending failed (the error is shown): give the text back.
      if (ok === undefined && !useUi.getState().drafts[target]) setDraft(target, value);
    } finally {
      submitting.current = false;
    }
  };

  const openHomeSession = async () => {
    const created = await newSession();
    if (created === undefined) homeSession.current = null;
    else {
      setDraft(created, useUi.getState().drafts[id] ?? '');
      set('');
    }
    return created;
  };

  /** Keep pasted images and attach them to the next message. */
  const attachPasted = async (images: File[]) => {
    const paths: string[] = [];
    for (const image of images) {
      try {
        paths.push(await savePastedImage(new Uint8Array(await image.arrayBuffer())));
      } catch (e) {
        toast('error', String(e));
      }
    }
    if (!paths.length) return;
    const target = bay?.id ?? (await (homeSession.current ??= openHomeSession()));
    if (target !== undefined) await act({ type: 'attach', bay: target, paths });
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.nativeEvent.isComposing) return;
    if (fileOpen && !e.shiftKey) {
      if (e.key === 'Escape') {
        e.preventDefault();
        setDismissed(mentionKey);
        return;
      }
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        if (files.length) setFileHot((h) => (h + (e.key === 'ArrowDown' ? 1 : files.length - 1)) % files.length);
        return;
      }
      if (e.key === 'Enter' || e.key === 'Tab' && files.length > 0) {
        e.preventDefault();
        if (files[fileHot]) chooseFile(files[fileHot]);
        return;
      }
    }
    if (slash.length && !e.shiftKey) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        setHot((h) => (h + (e.key === 'ArrowDown' ? 1 : slash.length - 1)) % slash.length);
        return;
      }
      if (e.key === 'Tab' || (e.key === 'Enter' && slash[hot] && `/${slash[hot].name}` !== text.trim())) {
        e.preventDefault();
        const c = slash[hot];
        set(`/${c.name}${c.args ? ' ' : ''}`);
        if (!c.args) void runSlash(`/${c.name}`).then((done) => done && set(''));
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        set('');
        return;
      }
    }
    if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      void submit(e.ctrlKey || e.metaKey);
      return;
    }
    if (e.key === 'Escape') {
      if (busy && bay) {
        e.preventDefault();
        void act({ type: 'abort', bay: bay.id });
      } else ref.current?.blur();
      return;
    }
    if (e.key === 'Tab' && e.shiftKey && bay && agent) {
      e.preventDefault();
      const i = MODES.findIndex((m) => m.value === agent.mode);
      void act({ type: 'setMode', bay: bay.id, mode: MODES[(i + 1) % MODES.length].value });
      return;
    }
    const sent = agent?.sentHistory ?? [];
    const el = e.currentTarget;
    if (e.key === 'ArrowUp' && sent.length && el.selectionStart === 0 && el.selectionEnd === 0 && (text === '' || history != null)) {
      e.preventDefault();
      const next = history == null ? sent.length - 1 : Math.max(0, history - 1);
      setHistory(next);
      set(sent[next]);
      return;
    }
    if (e.key === 'ArrowDown' && history != null && el.selectionStart === el.value.length) {
      e.preventDefault();
      const next = history + 1;
      if (next >= sent.length) {
        setHistory(null);
        set('');
      } else {
        setHistory(next);
        set(sent[next]);
      }
    }
  };

  const attachMenu = (anchor: Element) => {
    if (!bay) return;
    showMenu(
      anchor,
      [
        {
          label: 'Attach files…',
          icon: 'file',
          onSelect: () =>
            void pickFiles('Attach files', bay.cwd).then((files) => {
              if (files.length) void act({ type: 'attach', bay: bay.id, paths: files });
            }),
        },
        {
          label: 'Attach a folder…',
          icon: 'folder',
          onSelect: () =>
            void pickFolder('Attach a folder', bay.cwd).then((dir) => {
              if (dir) void act({ type: 'attach', bay: bay.id, paths: [dir] });
            }),
        },
      ],
      { above: true },
    );
  };

  const canSend = text.trim().length > 0;
  // Mid-turn, a message steers the agent (it starts with the turn while the session boots).
  const steering = busy && agent?.status.kind !== 'booting';
  const hint = placeholder ?? (bay ? (steering ? 'Steer Copilot, Ctrl+Enter to queue, Esc to stop' : busy ? 'Add to the queue, or press Esc to stop' : agent?.deferred ? 'What should we work on?' : 'Reply to Copilot') : 'Ask Copilot to build, fix or explain anything');

  return (
    <div className="composer-wrap">
      <div className="composer">
        {fileOpen && (
          <div className="slash-menu file-mention-menu">
            <div className="file-mention-heading ellipsis" title={directory}>{directory}</div>
            <div ref={fileMenu} className="file-mention-results" id={`file-mentions-${id}`} role="listbox" aria-label="File paths">
              {files.map((entry, i) => (
                <button
                  key={entry.path}
                  id={`file-mention-${id}-${i}`}
                  type="button"
                  role="option"
                  aria-selected={i === fileHot}
                  className={`slash-item ${i === fileHot ? 'hot' : ''}`}
                  onPointerMove={() => setFileHot(i)}
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => chooseFile(entry)}
                  title={entry.path}
                >
                  <Icon name={entry.isDir ? 'folder' : 'file'} />
                  <b className="ellipsis">{entry.name}</b>
                  {entry.isDir && <Icon name="chevron-right" className="desc" />}
                </button>
              ))}
            </div>
            {!files.length && (
              <div className="file-mention-status" role="status">{results?.error ?? (results ? 'No matching paths' : 'Loading paths...')}</div>
            )}
            <div className="file-mention-heading">Up/Down to choose · Tab/Enter to select · Esc to dismiss</div>
          </div>
        )}
        {slash.length > 0 && (
          <div className="slash-menu" role="listbox">
            {slash.map((c, i) => (
              <button
                key={c.name}
                type="button"
                className={`slash-item ${i === hot ? 'hot' : ''}`}
                onPointerMove={() => setHot(i)}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => {
                  set(`/${c.name}${c.args ? ' ' : ''}`);
                  if (!c.args) void runSlash(`/${c.name}`).then((done) => done && set(''));
                  ref.current?.focus();
                }}
              >
                <b>/{c.name}</b>
                {c.args && <span className="faint">{c.args}</span>}
                <span className="desc">{c.desc}</span>
              </button>
            ))}
          </div>
        )}
        {agent && agent.attachments.length > 0 && (
          <div className="composer-attachments">
            {agent.attachments.map((a) => (
              <span key={a.path} className="attach-chip" data-tip={a.path}>
                <Icon name={fileIcon(a.name, a.isDir ? 'folder' : 'file')} />
                <span className="ellipsis">{a.name}</span>
                <button type="button" className="icon-btn sm" aria-label={`Remove ${a.name}`} onClick={() => void act({ type: 'detach', bay: bay!.id, path: a.path })}>
                  <Icon name="close" />
                </button>
              </span>
            ))}
          </div>
        )}
        <textarea
          ref={ref}
          rows={1}
          value={text}
          placeholder={hint}
          spellCheck
          role="combobox"
          aria-label="Message"
          aria-autocomplete="list"
          aria-expanded={fileOpen}
          aria-controls={fileOpen ? `file-mentions-${id}` : undefined}
          aria-activedescendant={fileOpen && files[fileHot] ? `file-mention-${id}-${fileHot}` : undefined}
          onFocus={() => {
            setFocused(true);
            setDismissed(null);
          }}
          onBlur={() => setFocused(false)}
          onSelect={(e) => setSelection({ start: e.currentTarget.selectionStart, end: e.currentTarget.selectionEnd })}
          onChange={(e) => {
            set(e.target.value);
            setDismissed(null);
            setSelection({ start: e.target.selectionStart, end: e.target.selectionEnd });
            setHistory(null);
          }}
          onKeyDown={onKeyDown}
          onPaste={(e) => {
            const images = pastedImages(e.clipboardData);
            if (!images.length) return;
            e.preventDefault();
            void attachPasted(images);
          }}
        />
        <div className="composer-bar">
          {bay && (
            <button type="button" className="icon-btn" data-tip="Attach files, type @ to mention a path, or paste an image" onClick={(e) => attachMenu(e.currentTarget)}>
              <Icon name="add" />
            </button>
          )}
          {agent && (
            <Seg
              value={agent.mode}
              onChange={(m) => void act({ type: 'setMode', bay: bay!.id, mode: m })}
              options={MODES.map((m) => ({ value: m.value, label: m.label, tip: m.tip }))}
            />
          )}
          {agent && (
            <button
              type="button"
              className={`auto-chip ${agent.autoApprove ? 'on' : ''}`}
              data-tip={agent.autoApprove ? 'Approving every tool call in this session. Click to ask again.' : 'Approve every tool call in this session without asking'}
              onClick={() => void act({ type: 'setAutoApprove', bay: bay!.id, on: !agent.autoApprove })}
            >
              <Icon name={agent.autoApprove ? 'shield' : 'workspace-trusted'} />
              {agent.autoApprove ? 'Approving all' : null}
            </button>
          )}
          <span className="grow" />
          <button
            type="button"
            className={`model-chip${picker ? ' open' : ''}`}
            data-tip={picker ? undefined : `${choice.name}${choice.effort ? ` · thinking ${effortLabel(choice.effort).toLowerCase()}` : ''}${bay ? '' : ' (for new sessions)'}`}
            aria-haspopup="dialog"
            aria-expanded={!!picker}
            onClick={(e) => setPicker(picker ? null : e.currentTarget)}
          >
            <span className="ellipsis">{choice.label}</span>
            {choice.effort && <span className="model-effort">{effortLabel(choice.effort, true)}</span>}
            <Icon name="chevron-down" />
          </button>
          {picker && <ModelPicker anchor={picker} bay={bay} onClose={closePicker} />}
          {agent?.context && agent.context[1] > 0 && <Ring used={agent.context[0]} limit={agent.context[1]} />}
          {busy && !canSend ? (
            <button type="button" className="send-btn stop" data-tip="Stop" data-keys="Esc" onClick={() => void act({ type: 'abort', bay: bay!.id })}>
              <Icon name="debug-stop" />
            </button>
          ) : (
            <button type="button" className="send-btn" disabled={!canSend} data-tip={steering ? 'Steer Copilot (Ctrl+click to queue)' : busy ? 'Queue message' : 'Send'} data-keys="Enter" onClick={(e) => void submit(e.ctrlKey || e.metaKey)}>
              <Icon name="arrow-up" />
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
