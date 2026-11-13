import { useEffect, useState } from 'react';
import { runToastAction } from '../lib/api';
import { dismissToast, type LocalToast, useUi } from '../lib/store';
import { Icon } from './ui';

const ICONS = { info: 'info', ok: 'pass', warn: 'warning', error: 'error' } as const;

function Toast({ t }: { t: LocalToast }) {
  const [hover, setHover] = useState(false);
  useEffect(() => {
    if (hover) return;
    const life = t.level === 'error' ? 9000 : t.action ? 9000 : 5500;
    const left = Math.max(1500, life - (Date.now() - t.born));
    const timer = window.setTimeout(() => dismissToast(t.id), left);
    return () => window.clearTimeout(timer);
  }, [hover, t]);
  return (
    <div className={`toast ${t.level}`} role={t.level === 'error' ? 'alert' : 'status'} onPointerEnter={() => setHover(true)} onPointerLeave={() => setHover(false)}>
      <Icon name={ICONS[t.level]} />
      <div className="toast-text">{t.text}</div>
      {t.action && (
        <button
          type="button"
          className="btn sm"
          onClick={() => {
            runToastAction(t.action![1]);
            dismissToast(t.id);
          }}
        >
          {t.action[0]}
        </button>
      )}
      <button type="button" className="icon-btn sm" aria-label="Dismiss" onClick={() => dismissToast(t.id)}>
        <Icon name="close" />
      </button>
    </div>
  );
}

export function Toasts() {
  const toasts = useUi((s) => s.toasts);
  return (
    <div className="toasts" aria-live="polite">
      {toasts.map((t) => (
        <Toast key={t.id} t={t} />
      ))}
    </div>
  );
}
