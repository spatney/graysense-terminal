import { useRef, useState } from 'react';
import { act } from '../../lib/api';
import { baseName, plural } from '../../lib/format';
import { useUi } from '../../lib/store';
import type { BayDto, RewindDto, RewindFileDto } from '../../lib/types';
import { Icon, Modal, Toggle, useAutosize } from '../ui';
import { Stats } from './DiffView';

const CHANGE_ICON: Record<RewindFileDto['change'], string> = {
  created: 'diff-added',
  deleted: 'diff-removed',
  modified: 'diff-modified',
  changed: 'diff-modified',
};

function FileRow({ f }: { f: RewindFileDto }) {
  return (
    <li className="rewind-file" data-tip={f.path}>
      <Icon name={CHANGE_ICON[f.change]} className={`rewind-change rewind-change--${f.change}`} />
      <span className="ellipsis">{baseName(f.path)}</span>
      <Stats added={f.added} removed={f.removed} />
    </li>
  );
}

/** Confirm undoing, editing or retrying a message: what goes, and which files are put back. */
export function RewindDialog({ bay, rewind }: { bay: BayDto; rewind: RewindDto }) {
  const [restore, setRestore] = useState(true);
  const [text, setText] = useState(rewind.text);
  const box = useRef<HTMLTextAreaElement>(null);
  useAutosize(box, text, 260);
  const modelName = useUi((s) => s.app?.models.find((m) => m.id === rewind.model)?.name ?? rewind.model);

  // A retry that changed no files goes ahead without asking.
  if (rewind.then === 'retry' && (rewind.loading || !rewind.files.length)) return null;

  const close = () => {
    if (!rewind.running) void act({ type: 'rewindCancel', bay: bay.id });
  };
  const confirm = () => {
    if (rewind.loading || rewind.running) return;
    void act({ type: 'rewindConfirm', bay: bay.id, restoreFiles: restore && rewind.canRestore, text: rewind.then === 'edit' ? text : undefined });
  };

  const title = rewind.then === 'undo' ? 'Undo from here?' : rewind.then === 'edit' ? 'Edit message' : 'Retry this message?';
  const later = rewind.later ? ` ${plural(rewind.later, 'later message')} ${rewind.later === 1 ? 'goes' : 'go'} too.` : '';
  const lead =
    rewind.then === 'undo'
      ? `This message and everything after it come out of the conversation, and it goes back in the message box.${later}`
      : rewind.then === 'edit'
        ? `Change the message and send it again. What came after it comes out of the conversation.${later}`
        : `The answer comes out of the conversation and the message is sent again${modelName ? ` on ${modelName}` : ''}.`;
  const action = rewind.then === 'undo' ? 'Undo' : rewind.then === 'edit' ? 'Send' : 'Retry';

  return (
    <Modal onClose={close} width={500} labelledBy="rewind-title">
      <div className="modal-body rewind">
        <h3 id="rewind-title">{title}</h3>
        <p>{lead}</p>
        {rewind.then === 'edit' && (
          <textarea
            ref={box}
            className="textarea rewind-text"
            autoFocus
            value={text}
            disabled={rewind.running}
            onChange={(e) => setText(e.target.value)}
            onFocus={(e) => e.currentTarget.setSelectionRange(e.currentTarget.value.length, e.currentTarget.value.length)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
                e.preventDefault();
                confirm();
              }
            }}
          />
        )}
        {rewind.loading ? (
          <div className="rewind-note faint">
            <span className="spinner sm" /> Checking which files the agent changed…
          </div>
        ) : rewind.files.length ? (
          <div className="rewind-files">
            <div className="rewind-files-head">
              <span>{`The agent changed ${plural(rewind.files.length, 'file')} since`}</span>
              <label className="rewind-restore" data-tip={rewind.canRestore ? 'Put the files back as they were before this message' : (rewind.reason ?? undefined)}>
                Put them back
                <Toggle on={restore && rewind.canRestore} disabled={!rewind.canRestore || rewind.running} onChange={setRestore} label="Put the files back" />
              </label>
            </div>
            <ul>
              {rewind.files.map((f) => (
                <FileRow key={f.path} f={f} />
              ))}
            </ul>
            {!rewind.canRestore && rewind.reason && <div className="rewind-note faint">{rewind.reason} They stay as they are.</div>}
            {rewind.canRestore && restore && <div className="rewind-note faint">Files you changed yourself since are left alone.</div>}
          </div>
        ) : (
          rewind.reason && <div className="rewind-note faint">{rewind.reason} Files the agent changed stay as they are.</div>
        )}
      </div>
      <div className="modal-foot">
        <button type="button" className="btn ghost" disabled={rewind.running} onClick={close}>
          Cancel
        </button>
        <button
          type="button"
          autoFocus={rewind.then !== 'edit'}
          className={`btn ${rewind.then === 'undo' ? 'danger' : 'primary'}`}
          disabled={rewind.loading || rewind.running || (rewind.then === 'edit' && !text.trim())}
          onClick={confirm}
        >
          {rewind.running && <span className="spinner sm" />}
          {action}
        </button>
      </div>
    </Modal>
  );
}
