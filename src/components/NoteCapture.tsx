import { useEffect, useRef, useState } from 'react';
import { tryAct, pickFiles } from '../lib/api';
import { baseName } from '../lib/format';
import { fileIcon, linksIn } from '../lib/notes';
import { toast } from '../lib/store';
import { Icon, useAutosize } from './ui';

export interface NoteDraft {
  text?: string;
  files?: string[];
  session?: string | null;
  title?: string | null;
}

/** Files dropped while a capture box is open come here (see lib/drop.ts). */
export const NOTE_DROP = 'gs-note-drop';

/**
 * Save something for later: what happened, plus the files and links that go with it. Links
 * typed in the text are kept too.
 */
export function NoteCapture({ initial, onSaved, onCancel, autoFocus, focusTick, dropTarget = 'panel' }: { initial?: NoteDraft; onSaved?: (id: number) => void; onCancel?: () => void; autoFocus?: boolean; focusTick?: number; dropTarget?: 'panel' | 'overlay' }) {
  const [text, setText] = useState(initial?.text ?? '');
  const [files, setFiles] = useState<string[]>(initial?.files ?? []);
  const [links, setLinks] = useState<string[]>([]);
  const [linkDraft, setLinkDraft] = useState<string | null>(null);
  const [source, setSource] = useState(initial?.session ? { session: initial.session, title: initial.title ?? null } : null);
  const [saving, setSaving] = useState(false);
  const [dropping, setDropping] = useState(false);
  const ref = useRef<HTMLTextAreaElement>(null);
  useAutosize(ref, text, 260);

  useEffect(() => {
    if (autoFocus || focusTick) requestAnimationFrame(() => ref.current?.focus());
  }, [autoFocus, focusTick]);

  useEffect(() => {
    const onDrop = (e: Event) => {
      const { paths, target } = (e as CustomEvent<{ paths: string[]; target: string }>).detail;
      if (target !== dropTarget) return;
      setFiles((f) => [...f, ...paths.filter((p) => !f.includes(p))]);
      setDropping(false);
      ref.current?.focus();
    };
    const onOver = (e: Event) => {
      const { on, target } = (e as CustomEvent<{ on: boolean; target: string }>).detail;
      setDropping(on && target === dropTarget);
    };
    window.addEventListener(NOTE_DROP, onDrop);
    window.addEventListener(`${NOTE_DROP}-over`, onOver);
    return () => {
      window.removeEventListener(NOTE_DROP, onDrop);
      window.removeEventListener(`${NOTE_DROP}-over`, onOver);
    };
  }, [dropTarget]);

  const typed = linksIn(text).filter((l) => !links.includes(l));
  const empty = !text.trim() && !files.length && !links.length;

  const save = async () => {
    if (empty || saving) return;
    setSaving(true);
    try {
      const id = await tryAct<number>({ type: 'noteAdd', text, files, links, session: source?.session ?? null, title: source?.title ?? null });
      setText('');
      setFiles([]);
      setLinks([]);
      setLinkDraft(null);
      setSource(null);
      onSaved?.(id);
    } catch (e) {
      toast('error', String(e));
    } finally {
      setSaving(false);
    }
  };

  const addLink = () => {
    const url = (linkDraft ?? '').trim();
    if (!url) return setLinkDraft(null);
    const full = /^https?:\/\//i.test(url) ? url : `https://${url}`;
    if (!links.includes(full)) setLinks([...links, full]);
    setLinkDraft(null);
    ref.current?.focus();
  };

  return (
    <div className={`nts-capture ${dropping ? 'dropping' : ''}`} data-note-drop={dropTarget}>
      <textarea
        ref={ref}
        rows={2}
        value={text}
        placeholder="Save something for later: what happened, who it was for, what you used…"
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
            e.preventDefault();
            void save();
          } else if (e.key === 'Escape' && onCancel) {
            e.preventDefault();
            onCancel();
          }
        }}
      />
      {(files.length > 0 || links.length > 0 || typed.length > 0 || source) && (
        <div className="nts-chips">
          {source && (
            <span className="nts-chip source" data-tip="Saved with a link back to this session">
              <Icon name="comment-discussion" />
              <span className="ellipsis">{source.title || 'This session'}</span>
              <button type="button" className="icon-btn sm" aria-label="Don't link the session" onClick={() => setSource(null)}>
                <Icon name="close" />
              </button>
            </span>
          )}
          {files.map((f) => (
            <span key={f} className="nts-chip" data-tip={f}>
              <Icon name={fileIcon(f, /\.[^\\/]+$/.test(f) ? 'file' : 'folder')} />
              <span className="ellipsis">{baseName(f)}</span>
              <button type="button" className="icon-btn sm" aria-label="Remove" onClick={() => setFiles(files.filter((x) => x !== f))}>
                <Icon name="close" />
              </button>
            </span>
          ))}
          {links.map((l) => (
            <span key={l} className="nts-chip" data-tip={l}>
              <Icon name="link" />
              <span className="ellipsis">{l.replace(/^https?:\/\/(www\.)?/, '')}</span>
              <button type="button" className="icon-btn sm" aria-label="Remove" onClick={() => setLinks(links.filter((x) => x !== l))}>
                <Icon name="close" />
              </button>
            </span>
          ))}
          {typed.map((l) => (
            <span key={l} className="nts-chip auto" data-tip="Saved with the note (it's in the text)">
              <Icon name="link" />
              <span className="ellipsis">{l.replace(/^https?:\/\/(www\.)?/, '')}</span>
            </span>
          ))}
        </div>
      )}
      {linkDraft != null && (
        <div className="nts-link-row">
          <Icon name="link" className="faint" />
          <input
            className="input"
            autoFocus
            placeholder="Paste a link"
            value={linkDraft}
            onChange={(e) => setLinkDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                addLink();
              }
              if (e.key === 'Escape') setLinkDraft(null);
            }}
            onBlur={addLink}
          />
        </div>
      )}
      <div className="nts-capture-bar">
        <button
          type="button"
          className="btn ghost sm"
          data-tip="Add files (Graysense keeps a copy)"
          onClick={() =>
            void pickFiles('Add files to the note').then((picked) => {
              if (picked.length) setFiles((f) => [...f, ...picked.filter((p) => !f.includes(p))]);
            })
          }
        >
          <Icon name="attach" /> Files
        </button>
        <button type="button" className="btn ghost sm" onClick={() => setLinkDraft('')}>
          <Icon name="link" /> Link
        </button>
        <span className="grow" />
        {onCancel && (
          <button type="button" className="btn ghost sm" onClick={onCancel}>
            Cancel
          </button>
        )}
        <button type="button" className="btn primary sm" disabled={empty || saving} data-tip="Save" data-keys="Ctrl+Enter" onClick={() => void save()}>
          {saving ? <span className="spinner sm" /> : <Icon name="bookmark" />} Save
        </button>
      </div>
      {dropping && (
        <div className="nts-drop">
          <Icon name="cloud-upload" /> Drop to add to this note
        </div>
      )}
    </div>
  );
}
