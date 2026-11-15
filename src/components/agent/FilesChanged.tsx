import { useEffect, useId, useRef, useState } from 'react';
import { act } from '../../lib/api';
import { baseName, plural } from '../../lib/format';
import { Icon } from '../ui';
import { DiffView, Stats } from './DiffView';
import { displayPath, type FileChange } from './diff';

/** Chips shown before "+N more". */
const CHIP_LIMIT = 4;

const STATUS_ICON = { created: 'new-file', edited: 'edit', deleted: 'trash' } as const;
const STATUS_WORD = { created: 'Created', edited: 'Edited', deleted: 'Deleted' } as const;

/** The hidden files, for the "+N more" chip's tooltip. */
function moreTip(rest: FileChange[], cwd: string | null): string {
  const MAX = 12;
  const lines = rest.slice(0, MAX).map((f) => displayPath(f.path, cwd));
  if (rest.length > MAX) lines.push(`and ${rest.length - MAX} more`);
  return lines.join('\n');
}

/** What a turn did to one file, opened from its chip. */
function FileDiff({ file, id, cwd }: { file: FileChange; id: string; cwd: string | null }) {
  const ref = useRef<HTMLDivElement>(null);
  // On the newest turn the diff opens below the fold.
  useEffect(() => {
    ref.current?.scrollIntoView?.({ block: 'nearest', behavior: 'smooth' });
  }, []);
  const note = file.partial ? `Some of the changes to this file aren't shown${file.status === 'deleted' ? '.' : ' — open it to see everything.'}` : file.stepwise ? "Shown edit by edit — these edits couldn't be combined into one diff." : undefined;
  return (
    <div ref={ref} id={id} className="file-diff" role="region" aria-label={`Changes to ${displayPath(file.path, cwd)}`}>
      {file.diff ? (
        <DiffView diff={file.diff} cwd={cwd} note={note} />
      ) : (
        <div className="file-diff-empty">
          <span>No net change — later edits undid the earlier ones.</span>
          {file.status !== 'deleted' && (
            <button type="button" className="icon-btn sm" data-tip={`Open ${baseName(file.path)}`} aria-label={`Open ${baseName(file.path)}`} onClick={() => void act({ type: 'openPath', path: file.path })}>
              <Icon name="go-to-file" />
            </button>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * The files a turn changed, as chips after a "Changed" label, the first few then "+N more". A chip
 * shows what the turn did to its file below the row, or opens the file when there's no diff of it.
 */
export function FilesChanged({ files, cwd }: { files: FileChange[]; cwd: string | null }) {
  const [all, setAll] = useState(false);
  const [shown, setShown] = useState<string | null>(null);
  const panel = useId();
  if (!files.length) return null;
  const visible = all ? files : files.slice(0, CHIP_LIMIT);
  const open = files.find((f) => f.path === shown && f.diff != null);
  return (
    <>
      <div className="files-changed" role="group" aria-label={`Changed ${plural(files.length, 'file')}`}>
        <span className="files-changed-label" aria-hidden="true">
          Changed
        </span>
        {visible.map((f) => {
          const hasDiff = f.diff != null;
          const isOpen = open === f;
          const what = `${STATUS_WORD[f.status]} ${displayPath(f.path, cwd)}`;
          const openable = f.status !== 'deleted';
          return (
            <button
              key={f.path}
              type="button"
              className={`fc-chip ${f.status} ${isOpen ? 'open' : ''}`}
              disabled={!hasDiff && !openable}
              aria-expanded={hasDiff ? isOpen : undefined}
              aria-controls={isOpen ? panel : undefined}
              data-tip={hasDiff ? `${what} — ${isOpen ? 'hide' : 'show'} the changes` : openable ? `${what} — open it` : what}
              onClick={() => (hasDiff ? setShown(isOpen ? null : f.path) : void act({ type: 'openPath', path: f.path }))}
            >
              <Icon name={STATUS_ICON[f.status]} />
              <span className="fc-chip-name">{baseName(f.path)}</span>
              <Stats added={f.added} removed={f.removed} />
            </button>
          );
        })}
        {!all && files.length > CHIP_LIMIT && (
          <button type="button" className="fc-chip more" data-tip={moreTip(files.slice(CHIP_LIMIT), cwd)} onClick={() => setAll(true)}>
            +{files.length - CHIP_LIMIT} more
          </button>
        )}
      </div>
      {open && <FileDiff key={open.path} file={open} id={panel} cwd={cwd} />}
    </>
  );
}
