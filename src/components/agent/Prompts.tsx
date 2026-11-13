import { useState } from 'react';
import { act, openLink } from '../../lib/api';
import type { AskItem, ElicitItem, FormField, ItemDto, PermissionDto, PlanItem, Reply, SwitchItem } from '../../lib/types';
import { Markdown } from '../Markdown';
import { Icon, Toggle } from '../ui';
import { UnifiedDiff } from './Transcript';

export function isPrompt(it: ItemDto): it is PermissionDto | AskItem | ElicitItem | PlanItem | SwitchItem {
  return it.type === 'permission' || it.type === 'ask' || it.type === 'elicit' || it.type === 'plan' || it.type === 'switch';
}

function reply(bay: number, index: number, r: Reply) {
  return act({ type: 'reply', bay, index, reply: r });
}

function Permission({ bay, index, p }: { bay: number; index: number; p: PermissionDto }) {
  const [feedback, setFeedback] = useState('');
  const [busy, setBusy] = useState(false);
  const decide = async (decision: 'once' | 'session' | 'reject') => {
    setBusy(true);
    await reply(bay, index, { kind: 'permission', decision, feedback: decision === 'reject' ? feedback : null });
    setBusy(false);
  };
  return (
    <div className="qcard attention" role="group" aria-label="Approval needed">
      <div className="qcard-head">
        <Icon name="shield" />
        <span>
          Approval needed <span className="faint">· {p.what}</span>
        </span>
      </div>
      <div className="qcard-body">
        {p.headline && <b>{p.headline}</b>}
        {p.intention && <div className="muted">{p.intention}</div>}
        {p.command && (
          <div className="well">
            <pre className="cmd">{p.command}</pre>
          </div>
        )}
        {p.warning && (
          <div className="notice warn" style={{ alignSelf: 'flex-start' }}>
            <Icon name="warning" />
            <span>{p.warning}</span>
          </div>
        )}
        {p.fileName && <div className="mono" style={{ fontSize: 12 }}>{p.fileName}</div>}
        {p.diff && <UnifiedDiff text={p.diff} />}
        {p.paths.map((path) => (
          <div key={path} className="mono selectable" style={{ fontSize: 12 }}>
            {path}
          </div>
        ))}
        {p.url && (
          <a className="mono" style={{ fontSize: 12 }} onClick={() => openLink(p.url!)}>
            {p.url}
          </a>
        )}
        {(p.server || p.tool) && p.kind === 'mcp' && (
          <div className="mono" style={{ fontSize: 12 }}>
            {p.server} › {p.tool}
          </div>
        )}
        {p.toolDescription && <div className="faint">{p.toolDescription}</div>}
        {p.args && (
          <div className="well">
            <pre>{p.args}</pre>
          </div>
        )}
      </div>
      <div className="qcard-foot">
        <button type="button" className="btn primary" disabled={busy} onClick={() => void decide('once')} autoFocus>
          <Icon name="check" /> Allow
        </button>
        {p.canSession && (
          <button type="button" className="btn" disabled={busy} data-tip="Don't ask again for this kind of action in this session" onClick={() => void decide('session')}>
            Allow for session
          </button>
        )}
        <input
          className="input"
          placeholder="Or tell Copilot what to do instead…"
          value={feedback}
          onChange={(e) => setFeedback(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && feedback.trim()) void decide('reject');
          }}
        />
        <button type="button" className="btn danger" disabled={busy} onClick={() => void decide('reject')}>
          {feedback.trim() ? 'Deny and tell' : 'Deny'}
        </button>
      </div>
    </div>
  );
}

function Ask({ bay, index, a }: { bay: number; index: number; a: AskItem }) {
  const [answer, setAnswer] = useState('');
  const send = (text: string, freeform: boolean) => text.trim() && void reply(bay, index, { kind: 'ask', answer: text.trim(), freeform });
  return (
    <div className="qcard ask" role="group" aria-label="Copilot is asking">
      <div className="qcard-head">
        <Icon name="question" />
        <span>Copilot is asking</span>
      </div>
      <div className="qcard-body">
        <Markdown text={a.question} />
        {a.choices.length > 0 && (
          <div className="qcard-choices">
            {a.choices.map((c, i) => (
              <button key={c} type="button" className="qcard-choice" autoFocus={i === 0 && !a.freeform} onClick={() => send(c, false)}>
                {c}
              </button>
            ))}
          </div>
        )}
      </div>
      {a.freeform && (
        <div className="qcard-foot">
          <input
            className="input"
            autoFocus
            placeholder={a.choices.length ? 'Or type your own answer…' : 'Type your answer…'}
            value={answer}
            onChange={(e) => setAnswer(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && send(answer, true)}
          />
          <button type="button" className="btn primary" disabled={!answer.trim()} onClick={() => send(answer, true)}>
            Send
          </button>
        </div>
      )}
    </div>
  );
}

function initialValue(f: FormField): any {
  if (f.default !== undefined && f.default !== null) return f.default;
  switch (f.kind.type) {
    case 'bool':
      return false;
    case 'multi':
      return [];
    case 'number':
      return '';
    default:
      return '';
  }
}

function FieldInput({ f, value, set }: { f: FormField; value: any; set: (v: any) => void }) {
  const k = f.kind;
  switch (k.type) {
    case 'bool':
      return (
        <label className="form-check">
          <Toggle on={!!value} onChange={set} label={f.title} /> {f.title}
        </label>
      );
    case 'choice':
      return k.options.length <= 5 ? (
        <div className="qcard-choices">
          {k.options.map((o) => (
            <button key={String(o.value)} type="button" className={`qcard-choice ${value === o.value ? 'on' : ''}`} style={value === o.value ? { borderColor: 'var(--accent)', background: 'var(--accent-soft)' } : undefined} onClick={() => set(o.value)}>
              {o.label}
            </button>
          ))}
        </div>
      ) : (
        <select className="select" value={String(value ?? '')} onChange={(e) => set(k.options.find((o) => String(o.value) === e.target.value)?.value ?? e.target.value)}>
          <option value="">Choose…</option>
          {k.options.map((o) => (
            <option key={String(o.value)} value={String(o.value)}>
              {o.label}
            </option>
          ))}
        </select>
      );
    case 'multi':
      return (
        <div className="qcard-choices">
          {k.options.map((o) => {
            const on = Array.isArray(value) && value.includes(o.value);
            return (
              <button key={String(o.value)} type="button" className="qcard-choice" style={on ? { borderColor: 'var(--accent)', background: 'var(--accent-soft)' } : undefined} onClick={() => set(on ? value.filter((v: any) => v !== o.value) : [...(value ?? []), o.value])}>
                {on && <Icon name="check" />} {o.label}
              </button>
            );
          })}
        </div>
      );
    case 'number':
      return <input className="input" type="number" min={k.min ?? undefined} max={k.max ?? undefined} step={k.integer ? 1 : 'any'} value={value} onChange={(e) => set(e.target.value)} />;
    default:
      return k.multiline ? <textarea className="textarea" rows={3} value={value} onChange={(e) => set(e.target.value)} /> : <input className="input" value={value} onChange={(e) => set(e.target.value)} />;
  }
}

function Elicit({ bay, index, e }: { bay: number; index: number; e: ElicitItem }) {
  const [values, setValues] = useState<Record<string, any>>(() => Object.fromEntries(e.fields.map((f) => [f.key, initialValue(f)])));
  const [error, setError] = useState<string | null>(null);
  const submit = () => {
    const content: Record<string, any> = {};
    for (const f of e.fields) {
      let v = values[f.key];
      if (f.kind.type === 'number') v = v === '' ? undefined : f.kind.integer ? parseInt(v, 10) : parseFloat(v);
      if (f.required && (v === undefined || v === '' || (Array.isArray(v) && !v.length))) return setError(`${f.title} is required.`);
      if (v !== undefined && v !== '') content[f.key] = v;
    }
    void reply(bay, index, { kind: 'elicit', action: 'accept', content });
  };
  return (
    <div className="qcard ask" role="group" aria-label="Input requested">
      <div className="qcard-head">
        <Icon name="edit" />
        <span>
          Input requested{e.source && <span className="faint"> · {e.source}</span>}
        </span>
      </div>
      <div className="qcard-body">
        <Markdown text={e.message} />
        {e.url ? (
          <a className="mono" style={{ fontSize: 12 }} onClick={() => openLink(e.url!)}>
            {e.url}
          </a>
        ) : (
          e.fields.map((f) => (
            <div key={f.key} className="form-field">
              {f.kind.type !== 'bool' && (
                <label>
                  {f.title}
                  {f.required && <span className="faint"> *</span>}
                </label>
              )}
              {f.description && <span className="faint">{f.description}</span>}
              <FieldInput f={f} value={values[f.key]} set={(v) => setValues({ ...values, [f.key]: v })} />
            </div>
          ))
        )}
        {error && <div className="step-error">{error}</div>}
      </div>
      <div className="qcard-foot">
        {e.url ? (
          <>
            <button type="button" className="btn primary" onClick={() => openLink(e.url!)}>
              <Icon name="link-external" /> Open link
            </button>
            <button type="button" className="btn" onClick={() => void reply(bay, index, { kind: 'elicit', action: 'accept' })}>
              Done
            </button>
            <button type="button" className="btn ghost" onClick={() => void reply(bay, index, { kind: 'elicit', action: 'cancel' })}>
              Cancel
            </button>
          </>
        ) : (
          <>
            <button type="button" className="btn primary" onClick={submit}>
              Submit
            </button>
            <button type="button" className="btn ghost" onClick={() => void reply(bay, index, { kind: 'elicit', action: 'decline' })}>
              Decline
            </button>
          </>
        )}
      </div>
    </div>
  );
}

const ACTION_LABEL: Record<string, string> = {
  interactive: 'Build it step by step',
  autopilot: 'Build it on autopilot',
  autopilot_fleet: 'Build it with a fleet',
};

function Plan({ bay, index, p }: { bay: number; index: number; p: PlanItem }) {
  const [feedback, setFeedback] = useState('');
  const [full, setFull] = useState(false);
  return (
    <div className="qcard ask" role="group" aria-label="Plan ready">
      <div className="qcard-head">
        <Icon name="checklist" />
        <span>Plan ready for review</span>
      </div>
      <div className="qcard-body">
        <Markdown text={full && p.plan ? p.plan : p.summary} />
        {p.plan && (
          <button type="button" className="link-btn" style={{ alignSelf: 'flex-start', fontSize: 12 }} onClick={() => setFull(!full)}>
            {full ? 'Show the summary' : 'Read the full plan'}
          </button>
        )}
        <textarea className="textarea" rows={2} placeholder="Feedback, or what to change (optional)" value={feedback} onChange={(e) => setFeedback(e.target.value)} />
      </div>
      <div className="qcard-foot">
        {p.actions.map((a) => (
          <button key={a} type="button" className={`btn ${a === p.recommended || (!p.recommended && a === p.actions[0]) ? 'primary' : ''}`} onClick={() => void reply(bay, index, { kind: 'plan', approved: true, action: a, feedback })}>
            {ACTION_LABEL[a] ?? `Approve · ${a}`}
          </button>
        ))}
        <button type="button" className="btn ghost" onClick={() => void reply(bay, index, { kind: 'plan', approved: false, action: null, feedback })}>
          Keep planning
        </button>
      </div>
    </div>
  );
}

function Switch({ bay, index, s }: { bay: number; index: number; s: SwitchItem }) {
  return (
    <div className="qcard attention" role="group" aria-label="Model unavailable">
      <div className="qcard-head">
        <Icon name="arrow-swap" />
        <span>Switch models?</span>
      </div>
      <div className="qcard-body">
        <Markdown text={s.text} />
      </div>
      <div className="qcard-foot">
        <button type="button" className="btn primary" onClick={() => void reply(bay, index, { kind: 'switch', choice: 'yes' })}>
          Switch to Auto
        </button>
        <button type="button" className="btn" onClick={() => void reply(bay, index, { kind: 'switch', choice: 'always' })}>
          Always switch
        </button>
        <button type="button" className="btn ghost" onClick={() => void reply(bay, index, { kind: 'switch', choice: 'no' })}>
          No
        </button>
      </div>
    </div>
  );
}

/** A prompt waiting for you, inline in the transcript. */
export function PromptCard({ bay, index, item }: { bay: number; index: number; item: ItemDto }) {
  switch (item.type) {
    case 'permission':
      return <Permission bay={bay} index={index} p={item} />;
    case 'ask':
      return <Ask bay={bay} index={index} a={item} />;
    case 'elicit':
      return <Elicit bay={bay} index={index} e={item} />;
    case 'plan':
      return <Plan bay={bay} index={index} p={item} />;
    case 'switch':
      return <Switch bay={bay} index={index} s={item} />;
    default:
      return null;
  }
}
