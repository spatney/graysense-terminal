import '../../styles/panels/sync.css';
import { type ReactNode, useEffect, useId, useState } from 'react';
import { act, openLink, revealMemory, setSide } from '../../lib/api';
import { agoLong, plural } from '../../lib/format';
import { openOverlay, useUi } from '../../lib/store';
import type { Action, SyncCounts, SyncDto, SyncPending, SyncReport } from '../../lib/types';
import { Icon, Seg, Toggle } from '../ui';

type Mode = 'create' | 'existing';
type Choice = Extract<Action, { type: 'syncAnswer' }>['choice'];

const GIT_URL = 'https://git-scm.com/download/win';
const DEFAULT_NAME = 'graysense-memory';

const PHASE: Record<SyncDto['phase'], string> = {
  idle: 'Working…',
  connecting: 'Connecting to GitHub…',
  fetching: 'Checking the repository…',
  merging: 'Merging…',
  reconciling: 'Merging with AI…',
  pushing: 'Saving to GitHub…',
};

/** Unix seconds, refreshed every `ms` (for "synced 5 minutes ago"). */
function useNow(ms = 30_000): number {
  const [now, setNow] = useState(() => Date.now() / 1000);
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now() / 1000), ms);
    return () => window.clearInterval(t);
  }, [ms]);
  return now;
}

const upper = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/** Send an action, holding the button that sent it until the core has taken it (its `busy` follows). */
function useSend() {
  const [sending, setSending] = useState(false);
  const send = async (action: Action) => {
    setSending(true);
    try {
      await act(action);
    } finally {
      setSending(false);
    }
  };
  return [sending, send] as const;
}

/** What a sync did, in a line (what it left for review is shown apart, as a link). */
function summary(r: SyncReport): string {
  const parts: string[] = [];
  if (r.pulled) parts.push(`brought in ${r.pulled}`);
  if (r.removed) parts.push(`removed ${r.removed}`);
  if (r.pushed) parts.push(`saved ${plural(r.pushed, 'file')} to GitHub`);
  if (r.combined) parts.push(`combined ${plural(r.combined, 'duplicate')}`);
  if (r.conflicts) parts.push(`settled ${plural(r.conflicts, 'conflict')}`);
  if (r.tidied) parts.push(`AI merged ${r.tidied}`);
  return upper(parts.join(' · '));
}

/** Opens Memory at the first thing waiting for review (or just Memory, when nothing is). */
function useOpenReview() {
  const target = useUi((s) => s.memory.memories.find((m) => m.kind !== 'note' && (m.state === 'pending' || (m.state === 'active' && m.proposal != null))));
  return () => (target ? revealMemory(target.id) : void setSide('memory'));
}

// ------------------------------------------------------------------ the card

/** GitHub sync, as Settings shows it. Everything comes from the store: the core reports each step. */
export function SyncCard() {
  const sync = useUi((s) => s.sync);
  // The form's text is kept here, so a question that interrupts it doesn't lose what was typed.
  const [mode, setMode] = useState<Mode>('create');
  const [names, setNames] = useState<Record<Mode, string>>({ create: DEFAULT_NAME, existing: '' });

  if (!sync) {
    return (
      <div className="syn syn-wait" role="status" aria-label="Loading GitHub sync">
        <span className="spinner" />
      </div>
    );
  }
  if (sync.pending) return <Question key={sync.pending.kind} sync={sync} pending={sync.pending} />;
  if (!sync.git) return <NoGit />;
  if (sync.repo == null) {
    return <Connect sync={sync} mode={mode} name={names[mode]} onMode={setMode} onName={(name) => setNames((n) => ({ ...n, [mode]: name }))} />;
  }
  return <Connected sync={sync} repo={sync.repo} />;
}

function Busy({ phase }: { phase: SyncDto['phase'] }) {
  return (
    <div className="syn-busy" role="status">
      <span className="spinner sm" />
      <span className="shimmer ellipsis">{PHASE[phase]}</span>
    </div>
  );
}

function Warn({ text }: { text: string }) {
  return (
    <div className="syn-warn" role="alert">
      <Icon name="warning" />
      <span className="selectable">{text}</span>
    </div>
  );
}

// ------------------------------------------------------------------ not set up

function NoGit() {
  return (
    <div className="syn">
      <div className="syn-block">
        <div className="syn-intro">
          <span className="syn-tile warn">
            <Icon name="warning" />
          </span>
          <h4 className="syn-title">Sync needs Git</h4>
        </div>
        <p className="syn-text">Graysense uses Git to keep your memory in a GitHub repository, and Git isn't installed on this PC. Install Git for Windows; Graysense notices as soon as it's there.</p>
        <div className="syn-actions">
          <button type="button" className="btn sm" onClick={() => openLink(GIT_URL)}>
            <Icon name="link-external" />
            Get Git for Windows
          </button>
        </div>
      </div>
    </div>
  );
}

function Connect({ sync, mode, name, onMode, onName }: { sync: SyncDto; mode: Mode; name: string; onMode: (mode: Mode) => void; onName: (name: string) => void }) {
  const id = useId();
  const [sending, send] = useSend();
  const create = mode === 'create';
  const repo = name.trim();
  const busy = sync.busy;
  const submit = () => {
    if (repo && !busy && !sending) void send({ type: 'syncConnect', repo, create });
  };
  return (
    <div className="syn">
      <div className="syn-block">
        <div className="syn-intro">
          <span className="syn-tile">
            <Icon name="cloud-upload" />
          </span>
          <h4 className="syn-title">Back up and sync with GitHub</h4>
        </div>
        <p className="syn-text">
          Keeps your memories, notes (with their attached files), missions and a few preferences in a private GitHub repository, so they survive a reinstall and follow you between PCs. Keep the repository private.
        </p>
      </div>
      <form
        className="syn-block syn-form"
        aria-label="Connect to GitHub"
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <div className="syn-mode" role="group" aria-label="Repository" inert={busy}>
          <Seg<Mode>
            value={mode}
            onChange={onMode}
            options={[
              { value: 'create', label: 'Create a new repo' },
              { value: 'existing', label: 'Use an existing repo' },
            ]}
          />
        </div>
        <input
          className="input"
          value={name}
          placeholder={create ? DEFAULT_NAME : 'owner/name or https://github.com/owner/name'}
          aria-label={create ? 'New repository name' : 'Repository to connect'}
          aria-describedby={`${id}-hint`}
          spellCheck={false}
          autoComplete="off"
          disabled={busy}
          onChange={(e) => onName(e.target.value)}
        />
        <p className="syn-hint" id={`${id}-hint`}>
          <Icon name={create ? 'lock' : 'info'} />
          {create ? 'Made private on your GitHub account.' : 'An empty repository, or one Graysense made on another PC.'}
        </p>
        {busy ? <Busy phase={sync.phase} /> : sync.lastError && <Warn text={sync.lastError} />}
        <div className="syn-actions">
          <button type="submit" className="btn sm primary" disabled={busy || sending || !repo}>
            <Icon name={create ? 'repo-create' : 'repo'} />
            {create ? 'Create and sync' : 'Connect and sync'}
          </button>
        </div>
      </form>
      <p className="syn-foot">
        <Icon name="lock" />
        Uses your GitHub CLI sign-in (gh auth login). Approval rules, folders and the shell stay on this PC.
      </p>
    </div>
  );
}

// ------------------------------------------------------------------ a question

function Counts({ title, counts }: { title: string; counts: SyncCounts }) {
  return (
    <div className="syn-count">
      <dt>{title}</dt>
      <dd>{plural(counts.memories, 'memory', 'memories')}</dd>
      <dd>{plural(counts.notes, 'note')}</dd>
      <dd>{plural(counts.missions, 'mission')}</dd>
    </div>
  );
}

/** What sync is waiting on the user for: the repository is public, or both it and this PC hold data. */
function Question({ sync, pending }: { sync: SyncDto; pending: SyncPending }) {
  const titleId = useId();
  const [sending, send] = useSend();
  const off = sync.busy || sending;
  const answer = (choice: Choice) => send({ type: 'syncAnswer', choice });

  if (pending.kind === 'public') {
    return (
      <div className="syn syn-ask" role="group" aria-labelledby={titleId}>
        <div className="syn-block">
          <div className="syn-intro">
            <span className="syn-tile warn">
              <Icon name="warning" />
            </span>
            <h4 className="syn-title" id={titleId}>
              This repository is public
            </h4>
          </div>
          <p className="syn-text">
            <span className="syn-name">{pending.repo}</span> is public, so everything Graysense syncs, including your notes, would be visible to anyone. To keep it private, cancel and change its visibility on GitHub, or choose another repository.
          </p>
          <div className="syn-actions">
            <button type="button" className="btn sm danger" disabled={off} onClick={() => void answer('public')}>
              Use it anyway
            </button>
            <button type="button" className="btn sm ghost" disabled={off} onClick={() => void answer('cancel')}>
              Cancel
            </button>
          </div>
        </div>
      </div>
    );
  }

  if (pending.kind === 'removals') {
    const { repo, here, there, known } = pending;
    const n = here + there;
    // Which side lost it decides what the choices do.
    const words = there === 0 ? { keep: 'Save them to GitHub again', remove: 'Remove them here' } : here === 0 ? { keep: 'Restore them here', remove: 'Delete them from GitHub' } : { keep: 'Keep them', remove: 'Remove them' };
    const text =
      there === 0
        ? {
            what: `${repo} no longer has ${n} of the ${known} items it held at the last sync, so syncing would remove them from this PC too.`,
            unless: "If you didn't do that (the repository was deleted and made again, say), keep them and they're saved to GitHub again.",
          }
        : here === 0
          ? {
              what: `This PC no longer has ${n} of the ${known} items in ${repo}, so syncing would delete them from GitHub and from your other PCs.`,
              unless: "If you didn't do that (a file that wouldn't load, say), restore them here from GitHub.",
            }
          : {
              what: `Syncing would remove ${here} items from this PC and ${there} from ${repo} (and so from your other PCs): ${n} of the ${known} kept in step.`,
              unless: 'Keep them unless you meant to.',
            };
    const remove = () =>
      openOverlay({
        kind: 'confirm',
        title: `${words.remove}?`,
        body: text.what,
        confirm: words.remove,
        danger: true,
        onConfirm: () => void answer('remove'),
      });
    return (
      <div className="syn syn-ask" role="group" aria-labelledby={titleId}>
        <div className="syn-block">
          <div className="syn-intro">
            <span className="syn-tile warn">
              <Icon name="warning" />
            </span>
            <h4 className="syn-title" id={titleId}>
              Sync would remove most of your memory
            </h4>
          </div>
          <p className="syn-text">
            {text.what} {text.unless}
          </p>
          <p className="syn-hint">Nothing has changed yet.</p>
          <div className="syn-actions">
            <button type="button" className="btn sm primary" disabled={off} onClick={() => void answer('keep')}>
              {words.keep}
            </button>
            <button type="button" className="btn sm danger" disabled={off} onClick={remove}>
              {words.remove}
            </button>
            <button type="button" className="btn sm ghost" disabled={off} onClick={() => void answer('cancel')}>
              Not now
            </button>
          </div>
        </div>
      </div>
    );
  }

  const { repo, remote, local, from, at } = pending;
  // Saved under this PC's own name: most likely from before a reinstall.
  const who = from === sync.device ? `this PC (${from})` : from;
  const saved = who ? `Last saved by ${who}${at != null ? ` ${agoLong(at)}` : ''}` : at != null ? `Last saved ${agoLong(at)}` : null;
  const useRepoCopy = () =>
    openOverlay({
      kind: 'confirm',
      title: "Use the repository's copy?",
      body: `This PC's memories, notes and missions are replaced by what's in ${repo}. Anything here that isn't in the repository is removed. A backup of this PC's memory is kept first.`,
      confirm: "Replace this PC's data",
      danger: true,
      onConfirm: () => void answer('replace'),
    });
  return (
    <div className="syn syn-ask" role="group" aria-labelledby={titleId}>
      <div className="syn-block">
        <div className="syn-intro">
          <span className="syn-tile">
            <Icon name="git-merge" />
          </span>
          <h4 className="syn-title" id={titleId}>
            <span className="syn-name">{repo}</span> already has Graysense data
          </h4>
        </div>
        <dl className="syn-counts">
          <Counts title="Repository" counts={remote} />
          <Counts title="This PC" counts={local} />
        </dl>
        {saved && <p className="syn-hint">{saved}</p>}
        <div className="syn-choice">
          <button type="button" className="btn sm primary" disabled={off} onClick={() => void answer('merge')}>
            <Icon name="sparkle" />
            Merge with AI
          </button>
          <p className="syn-text">
            Combines both: duplicates are merged and conflicts settled by rules and an AI pass. Anything you wrote waits in Memory for your review, and a backup of this PC's memory is kept first.
          </p>
        </div>
        <div className="syn-actions">
          <button type="button" className="btn sm" disabled={off} data-tip="Replace this PC's data with what's in the repository" onClick={useRepoCopy}>
            Use the repository's copy
          </button>
          <button type="button" className="btn sm ghost" disabled={off} onClick={() => void answer('cancel')}>
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}

// ------------------------------------------------------------------ connected

function Connected({ sync, repo }: { sync: SyncDto; repo: string }) {
  const now = useNow();
  const [sending, send] = useSend();
  const { busy, lastSync, lastError, report, changes, url } = sync;
  const status = `${lastSync != null ? `Synced ${agoLong(lastSync, now)}` : 'Not synced yet'}${changes ? ' · changes not on GitHub yet' : ''}`;
  const disconnect = () =>
    openOverlay({
      kind: 'confirm',
      title: 'Disconnect from GitHub?',
      body: `Graysense stops syncing with ${repo}. The repository and the memories, notes and missions on this PC are left exactly as they are.`,
      confirm: 'Disconnect',
      onConfirm: () => void act({ type: 'syncDisconnect' }),
    });
  return (
    <div className="syn">
      <div className="syn-block">
        <div className="syn-id">
          <span className="syn-tile">
            <Icon name="github" />
          </span>
          <div className="syn-who">
            {url ? (
              <button type="button" className="link-btn syn-repo" data-tip="Open on GitHub" aria-label={`Open ${repo} on GitHub`} onClick={() => openLink(url)}>
                <span className="ellipsis">{repo}</span>
                <Icon name="link-external" />
              </button>
            ) : (
              <span className="syn-name syn-repo">{repo}</span>
            )}
            {sync.account && <span className="syn-sub ellipsis">Signed in as {sync.account}</span>}
          </div>
        </div>
        {busy ? (
          <Busy phase={sync.phase} />
        ) : (
          <div className="syn-state">
            <span className={`dot${lastSync != null && !changes && !lastError ? ' idle' : ''}`} aria-hidden />
            <span>{status}</span>
          </div>
        )}
        {!busy && lastError && <Warn text={lastError} />}
        <div className="syn-actions">
          <button type="button" className="btn sm primary" disabled={busy || sending} onClick={() => void send({ type: 'syncNow' })}>
            <Icon name="sync" />
            Sync now
          </button>
          <button type="button" className="btn sm" disabled={busy} onClick={disconnect}>
            <Icon name="debug-disconnect" />
            Disconnect…
          </button>
        </div>
      </div>
      {report && <Report report={report} />}
      <div className="syn-opts">
        <SwitchRow label="Sync automatically" desc="After changes, when Graysense opens, and every so often." on={sync.auto} onChange={(auto) => void act({ type: 'syncOptions', auto })} />
        <SwitchRow
          label="Files attached to notes"
          desc="Keep the files you attach to notes in the repository too. Very large files stay on this PC."
          on={sync.files}
          onChange={(files) => void act({ type: 'syncOptions', files })}
        />
        <SwitchRow label="Missions" desc="Your missions follow you between PCs." on={sync.missions} onChange={(missions) => void act({ type: 'syncOptions', missions })} />
        <SwitchRow
          label="Preferences"
          desc="Theme, default model and a few more. Never your shell, folders or approval settings."
          on={sync.settings}
          onChange={(settings) => void act({ type: 'syncOptions', settings })}
        />
      </div>
      <p className="syn-foot">
        <Icon name="lock" />
        Only connect repositories you control. Approval rules stay on this PC.
      </p>
    </div>
  );
}

/** What the last sync did, and what it wants the user to know. Nothing when it changed nothing. */
function Report({ report }: { report: SyncReport }) {
  const [open, setOpen] = useState(false);
  const openReview = useOpenReview();
  const text = summary(report);
  const notes = report.notes.length;
  if (!text && report.review === 0 && notes === 0) return null;
  return (
    <div className="syn-block syn-sum">
      {(text || report.review > 0) && (
        <p className="syn-text">
          {text}
          {text && report.review > 0 && ' · '}
          {report.review > 0 && (
            <button type="button" className="link-btn" data-tip="Open Memory to look at them" onClick={openReview}>
              {report.review} waiting for your review
            </button>
          )}
        </p>
      )}
      {notes > 0 && (
        <div>
          <button type="button" className="syn-more-btn" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
            <Icon name="chevron-right" className="syn-caret" />
            {`Details (${notes})`}
          </button>
          {open && (
            <ul className="syn-list">
              {report.notes.map((n, i) => (
                <li key={i}>{n}</li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}

function SwitchRow({ label, desc, on, onChange }: { label: string; desc: string; on: boolean; onChange: (on: boolean) => void }) {
  return (
    <label className="set-row">
      <span className="set-row-text">
        <span className="set-row-label">{label}</span>
        <span className="set-row-desc">{desc}</span>
      </span>
      <Toggle on={on} label={label} onChange={onChange} />
    </label>
  );
}

// ------------------------------------------------------------------ pointer from Memory

/** Open Settings at the Sync section, once the panel has mounted. */
export async function showSyncSettings() {
  await setSide('settings');
  for (let tries = 0; tries < 30; tries++) {
    const section = document.getElementById('settings-sync');
    if (section) {
      section.scrollIntoView({ block: 'start' });
      return;
    }
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
  }
}

/** A row for Memory's settings: where GitHub sync stands, and the way to Settings to change it. */
export function SyncPointer() {
  const sync = useUi((s) => s.sync);
  const now = useNow();
  if (!sync) return null;
  const { repo, busy, lastSync, lastError, pending } = sync;
  let title = 'Back up and sync with GitHub';
  let desc: ReactNode = 'Keeps your memory, notes and missions in a private repository.';
  let warn = false;
  if (pending) {
    title = 'GitHub sync is waiting for you';
    desc = pending.kind === 'public' ? `${pending.repo} is public` : pending.kind === 'removals' ? 'Most of what was synced would be removed' : `${pending.repo} already has Graysense data`;
  } else if (repo) {
    title = lastSync != null ? `Synced with ${repo}` : `Connected to ${repo}`;
    if (busy) {
      desc = (
        <>
          <span className="spinner sm" />
          Syncing…
        </>
      );
    } else if (lastError) {
      warn = true;
      desc = (
        <>
          <Icon name="warning" />
          <span className="ellipsis">{lastError}</span>
        </>
      );
    } else {
      desc = lastSync != null ? agoLong(lastSync, now) : 'Not synced yet';
    }
  }
  const connected = pending != null || repo != null;
  return (
    <div className="syn-ptr">
      <span className="syn-ptr-text">
        <span className="syn-ptr-title">{title}</span>
        <span className={`syn-ptr-desc${warn ? ' warn' : ''}`} data-tip={warn && lastError ? lastError : undefined}>
          {desc}
        </span>
      </span>
      <button type="button" className="btn sm" aria-label={connected ? 'Open GitHub sync settings' : 'Set up GitHub sync'} onClick={() => void showSyncSettings()}>
        {connected ? 'Open' : 'Set up'}
      </button>
    </div>
  );
}
