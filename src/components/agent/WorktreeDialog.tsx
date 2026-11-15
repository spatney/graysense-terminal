import { useEffect, useState } from 'react';
import { act, hasWork, worktreeReview, workSummary } from '../../lib/api';
import { plural, shortPath } from '../../lib/format';
import { useUi } from '../../lib/store';
import type { BayId, WorktreeReview } from '../../lib/types';
import { Icon, Modal, Toggle } from '../ui';
import { DiffView, Stats } from './DiffView';

/** A session's worktree: what its branch changes, and merging it, opening a pull request from it, or deleting it. */
export function WorktreeDialog({ bay, onClose }: { bay: BayId; onClose: () => void }) {
  const b = useUi((s) => s.bays.find((x) => x.id === bay) ?? null);
  const home = useUi((s) => s.app?.home);
  const w = b?.agent?.worktree ?? null;
  const path = w && !w.pending ? w.path : null;
  const [review, setReview] = useState<WorktreeReview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const [message, setMessage] = useState('');
  const [close, setClose] = useState(true);
  const [deleting, setDeleting] = useState(false);

  useEffect(() => {
    if (!path) return;
    let live = true;
    setError(null);
    worktreeReview(path, true).then(
      (r) => live && setReview(r),
      (e) => live && setError(String(e)),
    );
    return () => {
      live = false;
    };
  }, [path, nonce]);

  // The session closed, or no longer works in a worktree of its own.
  useEffect(() => {
    if (!path) onClose();
  }, [path, onClose]);
  if (!b?.agent || !w || !path) return null;

  const status = b.agent.status;
  const agentBusy = status.busy || status.attention;
  const busy = w.busy;
  const loading = !review && !error;
  const work = review ? hasWork(review) : false;
  const base = review ? review.base : w.base;
  const wait = busy ? `${busy} Wait for it to finish.` : agentBusy ? 'Wait for the agent to finish (or stop it) first: it may still be changing files.' : null;

  const merge = () => {
    void act({ type: 'worktreeMerge', bay, message, close });
    onClose();
  };
  const publish = () => {
    void act({ type: 'worktreePublish', bay, message });
    onClose();
  };
  const discard = () => {
    void act({ type: 'closeBay', bay, worktree: 'discard' });
    onClose();
  };

  if (deleting) {
    return (
      <Modal onClose={() => setDeleting(false)} width={460} labelledBy="worktree-title">
        <div className="modal-body">
          <h3 id="worktree-title">Delete this worktree?</h3>
          <p>
            The session closes, and {shortPath(w.path, home)} and its branch {w.branch} are deleted
            {review && work ? `, with everything on it that isn't merged (${workSummary(review)})` : ''}. This can't be undone.
          </p>
        </div>
        <div className="modal-foot">
          <button type="button" className="btn ghost" autoFocus onClick={() => setDeleting(false)}>
            Keep it
          </button>
          <button type="button" className="btn danger" disabled={!!busy} onClick={discard}>
            Delete worktree and branch
          </button>
        </div>
      </Modal>
    );
  }

  return (
    <Modal onClose={onClose} width={760} labelledBy="worktree-title">
      <div className="modal-body worktree">
        <div className="worktree-head">
          <h3 id="worktree-title">
            <Icon name="git-branch" /> {w.branch}
          </h3>
          <span className="grow" />
          <button type="button" className="icon-btn sm" data-tip="Look again" aria-label="Look again" disabled={loading} onClick={() => setNonce((n) => n + 1)}>
            <Icon name="refresh" />
          </button>
        </div>
        <p>
          The session works in {shortPath(w.path, home)}, a worktree of {shortPath(w.repo, home)}
          {w.base ? <> made from {w.base}</> : null}. Merge its branch back when you're happy with it, or push it and open a pull request.
        </p>
        {loading ? (
          <div className="rewind-note faint">
            <span className="spinner sm" /> Looking at what changed…
          </div>
        ) : error ? (
          <div className="rewind-note worktree-error">
            <Icon name="warning" /> {error}
          </div>
        ) : review && !work ? (
          <div className="rewind-note faint">Nothing to merge yet: the branch is the same as {base ?? 'where it started'}.</div>
        ) : review ? (
          <>
            <div className="worktree-summary">
              <span>{workSummary(review)}</span>
              <Stats added={review.added} removed={review.removed} />
              {review.dirty && <span className="worktree-tag" data-tip="Merging or opening a pull request commits them first">Not committed yet</span>}
              {review.behind > 0 && base && (
                <span className="worktree-tag warn" data-tip="Merging may need conflicts fixed">
                  {base} has {plural(review.behind, 'new commit')}
                </span>
              )}
            </div>
            {review.diff ? <DiffView diff={review.diff} cwd={w.path} truncated={review.truncated} /> : null}
          </>
        ) : null}
        {review?.dirty && (
          <label className="worktree-field">
            <span>Commit message</span>
            <input className="input" value={message} placeholder={b.title} spellCheck autoComplete="off" onChange={(e) => setMessage(e.target.value)} />
          </label>
        )}
        <label className="worktree-close" data-tip="After merging, close the session and delete the worktree and its branch">
          <Toggle on={close} onChange={setClose} label="Close the session and remove the worktree after merging" />
          <span>Close the session and remove the worktree after merging</span>
        </label>
        {wait && (
          <div className="rewind-note faint">
            {busy && <span className="spinner sm" />} {wait}
          </div>
        )}
      </div>
      <div className="modal-foot">
        <button type="button" className="btn ghost worktree-delete" disabled={!!busy} onClick={() => setDeleting(true)}>
          <Icon name="trash" /> Delete worktree…
        </button>
        <span className="grow" />
        <button type="button" className="btn ghost" disabled={!!wait || loading || (!!review && !work)} data-tip={review && !work ? 'Nothing to open a pull request for yet' : 'Commit what isn’t committed, push the branch and open a pull request'} onClick={publish}>
          <Icon name="git-pull-request" /> Open pull request
        </button>
        <button
          type="button"
          className="btn primary"
          autoFocus
          disabled={!!wait || loading || !base}
          data-tip={!base ? "This worktree didn't start from a branch: open a pull request instead" : undefined}
          onClick={merge}
        >
          <Icon name="git-merge" /> {base ? `Merge into ${base}` : 'Merge'}
        </button>
      </div>
    </Modal>
  );
}
