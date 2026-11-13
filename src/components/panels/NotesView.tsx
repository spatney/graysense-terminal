import '../../styles/panels/notes.css';
import { openUrl } from '@tauri-apps/plugin-opener';
import { useEffect, useMemo, useRef, useState } from 'react';
import { act, pickFiles } from '../../lib/api';
import { agoLong, ago, bytes, plural } from '../../lib/format';
import { fileIcon, looksLikeQuestion, matchRanges, rankNotes, relevanceLevel } from '../../lib/notes';
import { focusComposer, openOverlay, toast, useUi } from '../../lib/store';
import type { MemoryDto, NoteAttachmentDto } from '../../lib/types';
import { NoteCapture } from '../NoteCapture';
import { Icon, type MenuItem, Seg, copyText, showMenu, useAutosize } from '../ui';

type Sort = 'relevant' | 'recent';

function Highlighted({ text, query }: { text: string; query: string }) {
  const ranges = query ? matchRanges(text, query) : [];
  if (!ranges.length) return <>{text}</>;
  const out: React.ReactNode[] = [];
  let at = 0;
  ranges.forEach(([a, b], i) => {
    if (a > at) out.push(text.slice(at, a));
    out.push(<mark key={i}>{text.slice(a, b)}</mark>);
    at = b;
  });
  out.push(text.slice(at));
  return <>{out}</>;
}

function Meter({ note }: { note: MemoryDto }) {
  const level = relevanceLevel(note.relevance);
  const why = [
    note.votes > 0 ? `${plural(note.votes, 'upvote')}` : note.votes < 0 ? `${plural(-note.votes, 'downvote')}` : null,
    note.opens ? `opened ${plural(note.opens, 'time')}` : null,
    note.uses ? `used by agents ${plural(note.uses, 'time')}` : null,
    note.lastUsed || note.lastOpened ? `last used ${agoLong(Math.max(note.lastUsed ?? 0, note.lastOpened ?? 0))}` : 'not used yet',
  ].filter(Boolean);
  const fresh = !note.votes && !note.opens && !note.uses && Date.now() / 1000 - note.created < 7 * 86400;
  return (
    <span className={`nts-meter l${level}`} data-tip={`Relevance ${fresh ? 'new' : ['', 'low', 'fading', 'steady', 'high', 'top'][level]} · ${why.join(' · ')}`}>
      {[1, 2, 3, 4, 5].map((i) => (
        <i key={i} className={i <= level ? 'on' : ''} />
      ))}
    </span>
  );
}

function attachTo(path: string) {
  const s = useUi.getState();
  const bay = s.bays.find((b) => b.id === s.app?.focused && b.kind === 'agent') ?? s.bays.find((b) => b.kind === 'agent');
  if (!bay) return toast('info', 'Open an agent first, then attach this to it.');
  void act({ type: 'attach', bay: bay.id, paths: [path] }).then(() => {
    void act({ type: 'focusBay', bay: bay.id });
    focusComposer(bay.id);
    toast('ok', `Attached to ${bay.title}`);
  });
}

async function openAttachment(note: MemoryDto, index: number) {
  const r = await act<{ url?: string } | null>({ type: 'noteOpen', id: note.id, index });
  if (r && typeof r === 'object' && r.url) void openUrl(r.url);
}

function AttachmentChip({ note, a, index }: { note: MemoryDto; a: NoteAttachmentDto; index: number }) {
  const path = a.stored ?? a.target;
  const menu = (e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const items: MenuItem[] = [{ label: a.kind === 'link' ? 'Open link' : 'Open', icon: a.kind === 'link' ? 'link-external' : 'go-to-file', onSelect: () => void openAttachment(note, index) }];
    if (a.kind !== 'link') {
      items.push(
        { label: 'Reveal in File Explorer', icon: 'folder-opened', disabled: !a.exists, onSelect: () => void act({ type: 'noteReveal', id: note.id, index }) },
        { label: 'Attach to an agent', icon: 'attach', disabled: !a.exists, onSelect: () => attachTo(path) },
        { label: 'Copy path', icon: 'copy', onSelect: () => void copyText(path, 'Path copied') },
      );
    } else items.push({ label: 'Copy link', icon: 'copy', onSelect: () => void copyText(a.target, 'Link copied') });
    items.push({ sep: true }, { label: 'Remove from note', icon: 'close', danger: true, onSelect: () => void act({ type: 'noteDetach', id: note.id, index }) });
    showMenu(e, items);
  };
  const tip = a.kind === 'link' ? a.target : a.exists ? `${a.target}${a.stored ? ' · Graysense keeps a copy' : ''}${a.size ? ` · ${bytes(a.size)}` : ''}` : `${a.target} · missing`;
  return (
    <button
      type="button"
      className={`nts-file ${a.kind} ${a.exists ? '' : 'missing'}`}
      data-tip={tip}
      onClick={(e) => {
        e.stopPropagation();
        void openAttachment(note, index);
      }}
      onContextMenu={menu}
    >
      <Icon name={fileIcon(a.name, a.kind)} />
      <span className="ellipsis">{a.name}</span>
      {a.kind === 'link' && <Icon name="link-external" className="nts-file-ext" />}
    </button>
  );
}

function NoteCard({ note, query, flash, ask }: { note: MemoryDto; query: string; flash: boolean; ask: (q: string, ids: number[]) => void }) {
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState<string | null>(null);
  const editRef = useRef<HTMLTextAreaElement>(null);
  const ref = useRef<HTMLDivElement>(null);
  useAutosize(editRef, editing ?? '', 280);
  useEffect(() => {
    if (flash) ref.current?.scrollIntoView({ block: 'center', behavior: 'smooth' });
  }, [flash]);
  const source = note.sources.find((s) => s.title || s.session);
  const long = note.text.length > 220 || note.text.split('\n').length > 4;

  const saveEdit = () => {
    if (editing == null) return;
    if (editing.trim() && editing.trim() !== note.text) void act({ type: 'memoryEdit', id: note.id, text: editing, kind: 'note', project: note.project });
    setEditing(null);
  };

  const more = (anchor: Element | React.MouseEvent) => {
    const items: MenuItem[] = [
      { label: 'Ask Copilot about this', icon: 'sparkle', onSelect: () => ask(`Tell me about this note and what I might need from it: ${note.text.split('\n')[0]}`, [note.id]) },
      {
        label: 'Copy text',
        icon: 'copy',
        onSelect: () => {
          void copyText(note.text);
          void act({ type: 'noteTouched', id: note.id });
        },
      },
      { label: 'Edit', icon: 'edit', onSelect: () => setEditing(note.text) },
      { label: note.pinned ? 'Unpin' : 'Pin to top', icon: note.pinned ? 'pinned' : 'pin', onSelect: () => void act({ type: 'memoryPin', id: note.id, pinned: !note.pinned }) },
      {
        label: 'Add files…',
        icon: 'attach',
        onSelect: () =>
          void pickFiles('Add files to the note').then((files) => {
            if (files.length) void act({ type: 'noteAttach', id: note.id, files });
          }),
      },
      { sep: true },
      { label: 'Delete', icon: 'trash', danger: true, onSelect: () => void act({ type: 'memoryArchive', id: note.id }) },
    ];
    if (anchor instanceof Element) showMenu(anchor, items, { alignRight: true });
    else {
      anchor.preventDefault();
      showMenu(anchor, items);
    }
  };

  return (
    <div ref={ref} className={`nts-card ${note.pinned ? 'pinned' : ''} ${flash ? 'flash' : ''} ${note.relevance < 0.12 ? 'faded' : ''}`} onContextMenu={(e) => more(e)}>
      <div className="nts-vote">
        <button type="button" className={`nts-vote-btn ${note.votes > 0 ? 'on' : ''}`} data-tip="More relevant" aria-label="Upvote" onClick={() => void act({ type: 'noteVote', id: note.id, delta: 1 })}>
          <Icon name="triangle-up" />
        </button>
        <span className="nts-vote-n">{note.votes}</span>
        <button type="button" className={`nts-vote-btn down ${note.votes < 0 ? 'on' : ''}`} data-tip="Less relevant" aria-label="Downvote" onClick={() => void act({ type: 'noteVote', id: note.id, delta: -1 })}>
          <Icon name="triangle-down" />
        </button>
      </div>
      <div className="nts-main">
        {editing != null ? (
          <div className="nts-edit">
            <textarea
              ref={editRef}
              className="textarea"
              autoFocus
              value={editing}
              onChange={(e) => setEditing(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
                  e.preventDefault();
                  saveEdit();
                }
                if (e.key === 'Escape') setEditing(null);
              }}
            />
            <div className="row">
              <span className="faint grow" style={{ fontSize: 11.5 }}>
                Ctrl+Enter to save
              </span>
              <button type="button" className="btn ghost sm" onClick={() => setEditing(null)}>
                Cancel
              </button>
              <button type="button" className="btn primary sm" onClick={saveEdit}>
                Save
              </button>
            </div>
          </div>
        ) : (
          <div className={`nts-text selectable ${open || !long ? 'open' : ''}`} onDoubleClick={() => setEditing(note.text)} onClick={() => long && setOpen(!open)}>
            <Highlighted text={note.text} query={query} />
          </div>
        )}
        {note.attachments.length > 0 && (
          <div className="nts-files">
            {note.attachments.map((a, i) => (
              <AttachmentChip key={`${a.target}-${i}`} note={note} a={a} index={i} />
            ))}
          </div>
        )}
        <div className="nts-meta">
          <Meter note={note} />
          {note.pinned && <Icon name="pinned" className="nts-pin" />}
          <span className="nts-when" data-tip={new Date(note.created * 1000).toLocaleString()}>{ago(note.created)}</span>
          {source && (
            <button
              type="button"
              className="nts-source"
              data-tip="Open the session it came from"
              onClick={() => source.session && void act<number>({ type: 'resumeSession', id: source.session }).then((b) => b != null && focusComposer(b))}
              disabled={!source.session}
            >
              <Icon name="comment-discussion" />
              <span className="ellipsis">{source.title ?? 'Session'}</span>
            </button>
          )}
          {note.uses > 0 && (
            <span className="nts-used" data-tip="Times agents used this note">
              <Icon name="copilot" /> {note.uses}
            </span>
          )}
          <span className="grow" />
          <span className="nts-actions">
            <button type="button" className="icon-btn sm" data-tip="Ask Copilot about this" onClick={() => ask(`Tell me about this note and what I might need from it: ${note.text.split('\n')[0]}`, [note.id])}>
              <Icon name="sparkle" />
            </button>
            <button type="button" className="icon-btn sm" data-tip="Edit" onClick={() => setEditing(note.text)}>
              <Icon name="edit" />
            </button>
            <button type="button" className="icon-btn sm" data-tip="More" onClick={(e) => more(e.currentTarget)}>
              <Icon name="kebab-vertical" />
            </button>
          </span>
        </div>
      </div>
    </div>
  );
}

/** The Notes shelf of Memory: things you saved, with their files and links, ranked by how useful they've been. */
export function NotesView() {
  const memories = useUi((s) => s.memory.memories);
  const enabled = useUi((s) => s.settings?.['memory.enabled'] ?? true);
  const reveal = useUi((s) => s.revealMemory);
  const [query, setQuery] = useState('');
  const [sort, setSort] = useState<Sort>(() => (localStorage.getItem('gs-notes-sort') as Sort) || 'relevant');
  const [flash, setFlash] = useState<number | null>(null);
  const [showFading, setShowFading] = useState(false);
  const [showTrash, setShowTrash] = useState(false);
  const [captureTick, setCaptureTick] = useState(0);
  const search = useRef<HTMLInputElement>(null);

  const notes = useMemo(() => memories.filter((m) => m.kind === 'note' && m.state === 'active'), [memories]);
  const trash = useMemo(() => memories.filter((m) => m.kind === 'note' && m.state === 'archived').sort((a, b) => b.updated - a.updated), [memories]);
  const hits = useMemo(() => (query.trim() ? rankNotes(notes, query) : null), [notes, query]);
  const now = Date.now() / 1000;

  const ordered = useMemo(() => {
    const list = [...notes];
    if (sort === 'recent') list.sort((a, b) => Number(b.pinned) - Number(a.pinned) || b.created - a.created);
    else list.sort((a, b) => Number(b.pinned) - Number(a.pinned) || b.relevance - a.relevance || b.created - a.created);
    return list;
  }, [notes, sort]);
  const fading = ordered.filter((n) => !n.pinned && n.relevance < 0.12 && now - n.created > 60 * 86400);
  const main = ordered.filter((n) => !fading.includes(n));

  useEffect(() => {
    if (reveal == null) return;
    const target = memories.find((m) => m.id === reveal);
    if (!target || target.kind !== 'note') return;
    setQuery('');
    if (target.state === 'archived') setShowTrash(true);
    else if (!target.pinned && target.relevance < 0.12) setShowFading(true);
    setFlash(target.id);
    useUi.setState({ revealMemory: null });
  }, [reveal, memories]);

  useEffect(() => {
    if (flash == null) return;
    const t = window.setTimeout(() => setFlash(null), 1800);
    return () => window.clearTimeout(t);
  }, [flash]);

  const ask = (question: string, ids: number[]) =>
    act<number>({ type: 'noteAsk', question, ids }).then((bay) => {
      if (bay != null) focusComposer(bay);
      return bay;
    });
  const top = hits?.slice(0, 6) ?? [];
  const askFromSearch = () => {
    const q = query.trim();
    if (!q) return;
    // The question moves to its own session; clear the box for the next one.
    void ask(q, top.map((h) => h.note.id)).then((bay) => bay != null && setQuery(''));
  };
  const question = looksLikeQuestion(query);
  const filesInTop = top.reduce((n, h) => n + h.note.attachments.filter((a) => a.kind !== 'link').length, 0);

  return (
    <div className="nts" data-note-drop="panel">
      <div className="panel-toolbar">
        <NoteCapture focusTick={captureTick} onSaved={() => setSort((s) => s)} />
        <div className="search-field">
          <Icon name={question ? 'sparkle' : 'search'} />
          <input
            ref={search}
            className="input"
            placeholder="Search or ask your notes"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && (e.ctrlKey || question)) {
                e.preventDefault();
                askFromSearch();
              }
              if (e.key === 'Escape') setQuery('');
            }}
          />
        </div>
      </div>
      <div className="panel-body">
        {!enabled && (
          <div className="nts-banner">
            <Icon name="info" /> Memory is off, so agents won’t look at your notes. You can still save and search them here.
          </div>
        )}
        {hits != null ? (
          <>
            <button type="button" className={`nts-ask ${question ? 'hot' : ''}`} onClick={askFromSearch}>
              <span className="nts-ask-orb">
                <Icon name="sparkle" />
              </span>
              <span className="nts-ask-text">
                <b>Ask Copilot</b>
                <span className="ellipsis">“{query.trim()}”</span>
                <span className="faint">{top.length ? `Answers from ${plural(top.length, 'matching note')}${filesInTop ? ` and ${plural(filesInTop, 'file')}` : ''}` : 'No notes match yet; Copilot can still help'}</span>
              </span>
              <span className="nts-ask-key">{question ? '↵' : 'Ctrl ↵'}</span>
            </button>
            <div className="panel-section">
              <span className="section-label">{hits.length ? plural(hits.length, 'match', 'matches') : 'No matches'}</span>
            </div>
            {hits.map((h) => (
              <NoteCard key={h.note.id} note={h.note} query={query} flash={flash === h.note.id} ask={ask} />
            ))}
          </>
        ) : notes.length === 0 ? (
          <div className="empty nts-empty">
            <Icon name="archive" />
            <h4>Keep the context you’ll need later</h4>
            <p>Save what happened, who it was for and the files or links you used. Ask about it any time, and agents find it when it’s relevant.</p>
            <div className="nts-examples">
              {['Sent Charles the Rayfin logo (dark version) for the partner deck', 'Q3 roadmap deck lives in the Planning folder', 'Design review notes from the 12th: ship the compact composer'].map((t) => (
                <button key={t} type="button" className="chip" onClick={() => openOverlay({ kind: 'note', text: t })}>
                  {t}
                </button>
              ))}
            </div>
          </div>
        ) : (
          <>
            <div className="panel-section">
              <span className="section-label">{plural(notes.length, 'note')}</span>
              <Seg
                value={sort}
                onChange={(v) => {
                  setSort(v);
                  localStorage.setItem('gs-notes-sort', v);
                }}
                options={[
                  { value: 'relevant', label: 'Relevant', tip: 'Most useful first: votes, opens and agent use, fading with time' },
                  { value: 'recent', label: 'Recent', tip: 'Newest first' },
                ]}
              />
            </div>
            {main.map((n) => (
              <NoteCard key={n.id} note={n} query="" flash={flash === n.id} ask={ask} />
            ))}
            {fading.length > 0 && (
              <div className="nts-group">
                <button type="button" className="panel-section nts-group-head" onClick={() => setShowFading(!showFading)}>
                  <Icon name={showFading ? 'chevron-down' : 'chevron-right'} />
                  <span className="section-label">Rarely useful lately · {fading.length}</span>
                  <span className="grow" />
                  <span
                    role="button"
                    tabIndex={0}
                    className="link-btn"
                    onClick={(e) => {
                      e.stopPropagation();
                      openOverlay({
                        kind: 'confirm',
                        title: `Clean up ${plural(fading.length, 'note')}?`,
                        body: 'They move to the trash, where you can restore them for 30 days.',
                        confirm: 'Move to trash',
                        onConfirm: () => fading.forEach((n) => void act({ type: 'memoryArchive', id: n.id })),
                      });
                    }}
                  >
                    Clean up
                  </span>
                </button>
                {showFading && fading.map((n) => <NoteCard key={n.id} note={n} query="" flash={flash === n.id} ask={ask} />)}
              </div>
            )}
          </>
        )}
        {trash.length > 0 && !hits && (
          <div className="nts-group">
            <button type="button" className="panel-section nts-group-head" onClick={() => setShowTrash(!showTrash)}>
              <Icon name={showTrash ? 'chevron-down' : 'chevron-right'} />
              <span className="section-label">Trash · {trash.length}</span>
            </button>
            {showTrash &&
              trash.map((n) => (
                <div key={n.id} className={`nts-trash-row ${flash === n.id ? 'flash' : ''}`}>
                  <Icon name="note" className="faint" />
                  <span className="ellipsis grow">{n.text.split('\n')[0]}</span>
                  <button type="button" className="icon-btn sm" data-tip="Restore" onClick={() => void act({ type: 'memoryRestore', id: n.id })}>
                    <Icon name="discard" />
                  </button>
                  <button
                    type="button"
                    className="icon-btn sm"
                    data-tip="Delete forever"
                    onClick={() =>
                      openOverlay({ kind: 'confirm', title: 'Delete this note forever?', body: 'Its saved copies of files are deleted too.', confirm: 'Delete forever', danger: true, onConfirm: () => void act({ type: 'memoryPurge', id: n.id }) })
                    }
                  >
                    <Icon name="trash" />
                  </button>
                </div>
              ))}
          </div>
        )}
        {notes.length > 0 && !hits && (
          <p className="nts-foot">
            Notes rise as you use them and fade when they’re not. Agents look them up when they’re relevant, or ask “what did I send Charles?”.{' '}
            <button type="button" className="link-btn" onClick={() => setCaptureTick((t) => t + 1)}>
              Add a note
            </button>
          </p>
        )}
      </div>
    </div>
  );
}
