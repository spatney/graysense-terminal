import { memo, useMemo, useState } from 'react';
import { act } from '../../lib/api';
import { highlightLines, langOf } from '../../lib/highlight';
import { Icon, copyText } from '../ui';
import { DIFF_PREVIEW as PREVIEW, parseUnifiedDiff, shownPath, type DiffFile, type DiffHunk } from './diff';

const STATUS: Record<DiffFile['status'], string | null> = { added: 'New file', deleted: 'Deleted', renamed: 'Renamed', modified: null };

/** Lines added and removed: "+12 −3". */
export function Stats({ added, removed }: { added?: number | null; removed?: number | null }) {
  if (!added && !removed) return null;
  return (
    <span className="diff-stats">
      {!!added && <span className="stat-add">+{added}</span>}
      {!!removed && <span className="stat-del">−{removed}</span>}
    </span>
  );
}

/** Each of a hunk's lines as highlighted HTML (null for plain text), read on its own side: the old file for a removed line, the new one otherwise. */
function useHunkHtml(hunk: DiffHunk, lang: string | undefined): (string | null)[] {
  return useMemo(() => {
    const after = highlightLines(hunk.lines.filter((l) => l.kind === 'ctx' || l.kind === 'add').map((l) => l.text), lang);
    const before = highlightLines(hunk.lines.filter((l) => l.kind === 'del').map((l) => l.text), lang);
    let a = 0;
    let b = 0;
    return hunk.lines.map((l) => (l.kind === 'meta' ? null : l.kind === 'del' ? (before?.[b++] ?? null) : (after?.[a++] ?? null)));
  }, [hunk, lang]);
}

function Hunk({ hunk, lang, limit }: { hunk: DiffHunk; lang: string | undefined; limit: number }) {
  const html = useHunkHtml(hunk, lang);
  return (
    <>
      <tr className="diff-hunk">
        <td className="diff-no" />
        <td className="diff-no" />
        <td className="diff-code">
          ⋯ Line {hunk.newStart || hunk.oldStart}
          {hunk.section && <span className="diff-hunk-section"> · {hunk.section}</span>}
        </td>
      </tr>
      {hunk.lines.slice(0, limit).map((l, i) =>
        l.kind === 'meta' ? (
          <tr key={i} className="diff-row meta">
            <td className="diff-no" />
            <td className="diff-no" />
            <td className="diff-code">{l.text}</td>
          </tr>
        ) : (
          <tr key={i} className={`diff-row ${l.kind}`}>
            <td className="diff-no">{l.oldNo}</td>
            <td className="diff-no">{l.newNo}</td>
            <td className="diff-code">
              <span className="diff-sign">{l.kind === 'add' ? '+' : l.kind === 'del' ? '−' : ' '}</span>
              {html[i] != null ? <span dangerouslySetInnerHTML={{ __html: html[i]! }} /> : l.text}
            </td>
          </tr>
        ),
      )}
    </>
  );
}

/** A diff that can't be read into files, line by line with its signs coloured. */
function Lines({ text }: { text: string }) {
  return (
    <div className="well">
      <div className="line-diff selectable">
        {text.split('\n').map((l, i) => (
          <div key={i} className={l.startsWith('+') && !l.startsWith('+++') ? 'add' : l.startsWith('-') && !l.startsWith('---') ? 'del' : l.startsWith('@@') ? 'hunk' : ''}>
            {l || ' '}
          </div>
        ))}
      </div>
    </div>
  );
}

interface DiffViewProps {
  diff: string;
  /** The session's folder: files in it show by their path in it. */
  cwd: string | null;
  /** The diff was cut short. */
  truncated?: boolean;
  /** How complete the diff is, for its foot (in place of the note that it was cut short). */
  note?: string;
  /** Offer to open each file; not while the change is only asked for. */
  openable?: boolean;
}

/** A unified diff, file by file, with line numbers, syntax colours and +/− signs. */
export const DiffView = memo(function DiffView({ diff, cwd, truncated, note, openable = true }: DiffViewProps) {
  const files = useMemo(() => parseUnifiedDiff(diff), [diff]);
  const [all, setAll] = useState(false);
  const [wrap, setWrap] = useState(false);
  if (!files.length) return <Lines text={diff} />;
  const total = files.reduce((n, f) => n + f.hunks.reduce((m, h) => m + h.lines.length, 0), 0);
  let budget = all ? Number.POSITIVE_INFINITY : PREVIEW;
  const remark = note ?? (truncated ? (openable ? 'Diff shortened — open the file to see everything.' : "Diff shortened — the rest isn't shown.") : null);
  return (
    <div className={`diff-view ${wrap ? 'wrap' : ''}`}>
      {files.map((f, fi) => {
        const p = shownPath(f.path, cwd);
        const status = STATUS[f.status];
        const lang = langOf(f.path);
        return (
          <div key={fi} className="diff-file">
            <div className="diff-file-head">
              {status && <span className={`diff-status ${f.status}`}>{status}</span>}
              {f.binary && <span className="diff-status">Binary</span>}
              <span className="diff-path" data-tip={p.full}>
                {p.dir && <span className="diff-dir">{p.dir}</span>}
                <span className="diff-base">{p.base}</span>
              </span>
              <Stats added={f.added} removed={f.removed} />
              <span className="grow" />
              {openable && f.status !== 'deleted' && (
                <button type="button" className="icon-btn sm" data-tip={`Open ${p.base}`} aria-label={`Open ${p.base}`} onClick={() => void act({ type: 'openPath', path: p.full })}>
                  <Icon name="go-to-file" />
                </button>
              )}
            </div>
            {budget > 0 && f.hunks.length > 0 && (
              <div className="diff-scroll">
                <table className="diff-table">
                  <tbody>
                    {f.hunks.map((h, hi) => {
                      if (budget <= 0) return null;
                      const limit = Math.min(h.lines.length, budget);
                      budget -= limit;
                      return <Hunk key={hi} hunk={h} lang={lang} limit={limit} />;
                    })}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        );
      })}
      <div className="diff-foot">
        {!all && total > PREVIEW && (
          <button type="button" className="link-btn" onClick={() => setAll(true)}>
            Show all {total} lines
          </button>
        )}
        {remark && <span className="diff-note">{remark}</span>}
        <span className="grow" />
        <button type="button" className={`icon-btn sm ${wrap ? 'active' : ''}`} data-tip={wrap ? "Don't wrap lines" : 'Wrap lines'} aria-pressed={wrap} onClick={() => setWrap(!wrap)}>
          <Icon name="word-wrap" />
        </button>
        <button type="button" className="icon-btn sm" data-tip="Copy diff" onClick={() => void copyText(diff, 'Diff copied')}>
          <Icon name="copy" />
        </button>
      </div>
    </div>
  );
});
