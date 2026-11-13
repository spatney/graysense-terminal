import { useUi } from '../lib/store';
import { useDrag } from '../lib/drag';
import { useFileDrag } from '../lib/drop';
import { act } from '../lib/api';
import type { BayDto } from '../lib/types';
import { AgentBay } from './agent/AgentBay';
import { Hero } from './agent/Hero';
import { TerminalBay } from './terminal/TerminalBay';
import { Boundary, Icon } from './ui';

function DropHint({ bay }: { bay: BayDto }) {
  const files = useFileDrag((s) => (s.over === bay.id ? s.count : 0));
  const mission = useDrag((s) => (s.mission?.overBay === bay.id ? s.mission : null));
  if (!files && !mission) return null;
  const busy = bay.agent?.status.busy && !bay.agent.deferred;
  const label = mission
    ? bay.kind === 'agent'
      ? busy
        ? 'Busy: drop it on an idle agent'
        : 'Delegate to this agent'
      : 'Missions go to agents'
    : bay.kind === 'agent'
      ? `Attach ${files === 1 ? 'file' : `${files} files`}`
      : 'Paste path into terminal';
  return (
    <div className={`bay-drop${mission && (busy || bay.kind !== 'agent') ? ' no' : ''}`}>
      <div>
        <Icon name={mission ? (busy ? 'watch' : bay.kind === 'agent' ? 'rocket' : 'circle-slash') : bay.kind === 'agent' ? 'attach' : 'terminal'} />
        {label}
      </div>
    </div>
  );
}

function BayFrame({ bay, focused, hidden, span }: { bay: BayDto; focused: boolean; hidden: boolean; span: number }) {
  return (
    <div
      className={`bay ${focused ? 'focused' : ''} ${hidden ? 'hidden' : ''}`}
      style={span > 1 ? { gridColumn: `span ${span}` } : undefined}
      data-bay-drop={bay.id}
      onPointerDownCapture={() => {
        if (!focused) void act({ type: 'focusBay', bay: bay.id });
      }}
    >
      <Boundary label={bay.kind === 'agent' ? 'This session' : 'This terminal'}>{bay.kind === 'agent' ? <AgentBay bay={bay} visible={!hidden} /> : <TerminalBay bay={bay} visible={!hidden} focused={focused} />}</Boundary>
      <DropHint bay={bay} />
    </div>
  );
}

export function Workspace() {
  const bays = useUi((s) => s.bays);
  const focused = useUi((s) => s.app?.focused ?? null);
  const maximized = useUi((s) => s.app?.maximized ?? false);
  const layout = useUi((s) => s.settings?.layout ?? 'grid');
  if (!bays.length) return <Welcome />;
  const current = bays.find((b) => b.id === focused) ?? bays[0];
  const solo = layout === 'focus' || maximized || bays.length === 1;
  const visible = solo ? [current] : bays;
  const n = visible.length;
  let cols = 1;
  let rows = 1;
  if (layout === 'columns') cols = n;
  else if (layout === 'rows') rows = n;
  else {
    cols = Math.ceil(Math.sqrt(n));
    rows = Math.ceil(n / cols);
  }
  const spare = cols * rows - n;
  return (
    <section className={`bays ${solo ? 'single' : ''}`} style={{ gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`, gridTemplateRows: `repeat(${rows}, minmax(0, 1fr))` }}>
      {bays.map((b) => {
        const shown = visible.includes(b);
        const last = shown && visible[visible.length - 1] === b;
        return <BayFrame key={b.id} bay={b} focused={b.id === current.id} hidden={!shown} span={last && layout === 'grid' ? spare + 1 : 1} />;
      })}
    </section>
  );
}

/** No bays open: the home screen, a composer that starts a new session. */
function Welcome() {
  return (
    <section className="bays single">
      <div className="bay" data-bay-drop="">
        <Hero bay={null} />
      </div>
    </section>
  );
}
