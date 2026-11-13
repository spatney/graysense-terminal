import { type ReactNode, useEffect, useMemo, useRef, useState } from 'react';
import { act, openLink } from '../../lib/api';
import { plural } from '../../lib/format';
import { type LocalToast, openOverlay, useUi } from '../../lib/store';
import type { Action, MarketDto, MarketPluginDto, PluginDto, Result } from '../../lib/types';
import { copyText, Icon, type MenuItem, Seg, showMenu, Toggle } from '../ui';
import '../../styles/panels/plugins.css';

type Tab = 'installed' | 'discover';
type Ui = ReturnType<typeof useUi.getState>;

/** The tab the panel opens on next time. */
let lastTab: Tab = 'installed';

/** Installs and updates fetch from git, so they can take a while. */
const SLOW = 180_000;

// ------------------------------------------------------------------ helpers

/**
 * Follow an action the core runs in the background: it toasts the outcome (unless `toast` is
 * false), then refreshes the plugin lists. Resolves with that toast once the lists change (at
 * once for an error), or with whatever it heard after `ms`.
 */
function follow(ms: number, toast = true) {
  let finish: (t: LocalToast | null) => void = () => {};
  const promise = new Promise<LocalToast | null>((resolve) => {
    let heard: LocalToast | null = null;
    let waiting = toast;
    const unsub = useUi.subscribe((s: Ui, prev: Ui) => {
      if (waiting && s.toasts !== prev.toasts) {
        const old = new Set(prev.toasts.map((t) => t.id));
        heard = s.toasts.find((t) => t.id > 0 && !old.has(t.id)) ?? null;
        if (heard) waiting = false;
        if (heard?.level === 'error') return finish(heard);
      }
      if (!waiting && s.plugins !== prev.plugins) finish(heard);
    });
    const timer = window.setTimeout(() => finish(heard), ms);
    finish = (t) => {
      window.clearTimeout(timer);
      unsub();
      resolve(t);
    };
  });
  return { promise, cancel: () => finish(null) };
}

function omit<T>(rec: Record<string, T>, key: string): Record<string, T> {
  if (!(key in rec)) return rec;
  const next = { ...rec };
  delete next[key];
  return next;
}

function toggled(set: ReadonlySet<string>, key: string): ReadonlySet<string> {
  const next = new Set(set);
  if (!next.delete(key)) next.add(key);
  return next;
}

function useAlive() {
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  return alive;
}

const matches = (q: string, ...texts: (string | null | undefined)[]) => !q || texts.some((t) => !!t && t.toLowerCase().includes(q));

/** A web page for a marketplace's source: its URL, or the GitHub repo for `owner/repo`. */
function sourcePage(src: string): string | null {
  const s = src.trim();
  if (/^https?:\/\//i.test(s)) return s;
  if (/^[\w.-]+\/[\w.-]+$/.test(s)) return `https://github.com/${s}`;
  return null;
}

/** `text` with the (lowercase) query marked. */
function Hl({ text, q }: { text: string; q: string }) {
  if (!q) return <>{text}</>;
  const lower = text.toLowerCase();
  const out: ReactNode[] = [];
  let at = 0;
  for (let i = lower.indexOf(q); i >= 0; i = lower.indexOf(q, at)) {
    if (i > at) out.push(text.slice(at, i));
    out.push(<mark key={i}>{text.slice(i, i + q.length)}</mark>);
    at = i + q.length;
  }
  if (at < text.length) out.push(text.slice(at));
  return <>{out}</>;
}

/** A description clamped to two lines; long ones open on click (and while a search matches them). */
function Desc({ text, q }: { text: string; q: string }) {
  const [full, setFull] = useState(false);
  if (text.length < 90) {
    return (
      <p className="plg-desc">
        <Hl text={text} q={q} />
      </p>
    );
  }
  const open = full || (!!q && text.toLowerCase().includes(q));
  const flip = () => setFull(!open);
  return (
    <p
      className={`plg-desc more ${open ? 'full' : ''}`}
      role="button"
      tabIndex={0}
      aria-expanded={open}
      onClick={flip}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          flip();
        }
      }}
    >
      <Hl text={text} q={q} />
    </p>
  );
}

function Loading({ text }: { text: string }) {
  return (
    <div className="plg-loading" role="status">
      <span className="spinner sm" />
      {text}
    </div>
  );
}

function Problem({ title, detail, onRetry }: { title: string; detail: string; onRetry: () => void }) {
  return (
    <div className="plg-problem" role="alert">
      <Icon name="warning" />
      <div className="grow">
        <div className="plg-problem-title">{title}</div>
        <p className="selectable">{detail}</p>
        <button type="button" className="btn sm" onClick={onRetry}>
          <Icon name="refresh" />
          Retry
        </button>
      </div>
    </div>
  );
}

// ------------------------------------------------------------------ panel

export function PluginsPanel() {
  const data = useUi((s) => s.plugins);
  const [tab, setTabState] = useState<Tab>(lastTab);
  const [query, setQuery] = useState('');
  const [refreshing, setRefreshing] = useState(false);
  const [busy, setBusy] = useState<Record<string, true>>({});
  const [flip, setFlip] = useState<Record<string, boolean>>({});
  const [open, setOpen] = useState<ReadonlySet<string>>(() => new Set());
  // Marketplaces collapsed while a search shows them all.
  const [shut, setShut] = useState<ReadonlySet<string>>(() => new Set());
  const [asked, setAsked] = useState<ReadonlySet<string>>(() => new Set());
  const [source, setSource] = useState('');
  const [market, setMarket] = useState('');
  const alive = useAlive();
  const searchRef = useRef<HTMLInputElement>(null);
  const autoOpened = useRef(false);
  const q = query.trim().toLowerCase();

  const installed = data?.installed ?? null;
  const markets = data?.markets ?? null;
  const mine = installed && 'Ok' in installed ? installed.Ok : null;
  const marketList = markets && 'Ok' in markets ? markets.Ok : null;
  const browse = useMemo(() => new Map(data?.browse ?? []), [data?.browse]);
  const have = useMemo(() => new Set((mine ?? []).map((p) => p.name)), [mine]);

  const setTab = (t: Tab) => {
    lastTab = t;
    setTabState(t);
  };

  const refresh = () => {
    setRefreshing(true);
    const w = follow(15_000, false);
    void act({ type: 'pluginsRefresh' }).then((r) => r === undefined && w.cancel());
    void w.promise.then(() => alive.current && setRefreshing(false));
  };

  useEffect(refresh, []);

  /** Run an action, busy under `key` until the core reports back. Resolves false if it failed. */
  const run = async (key: string, action: Action, ms = 30_000): Promise<boolean> => {
    setBusy((b) => ({ ...b, [key]: true }));
    const w = follow(ms);
    const sent = (await act(action)) !== undefined;
    if (!sent) w.cancel();
    const outcome = await w.promise;
    if (alive.current) setBusy((b) => omit(b, key));
    return sent && outcome?.level !== 'error';
  };

  const install = (src: string) => run(`install:${src}`, { type: 'pluginInstall', source: src }, SLOW);

  const installTyped = async () => {
    const src = source.trim();
    if (!src || busy[`install:${src}`]) return;
    if ((await install(src)) && alive.current) setSource((cur) => (cur.trim() === src ? '' : cur));
  };

  const setEnabled = async (name: string, on: boolean) => {
    setFlip((f) => ({ ...f, [name]: on }));
    await run(`toggle:${name}`, { type: 'pluginSetEnabled', name, enabled: on });
    if (alive.current) setFlip((f) => omit(f, name));
  };

  const update = (name: string) => void run(`update:${name}`, { type: 'pluginUpdate', name }, SLOW);

  const uninstall = (p: PluginDto) =>
    openOverlay({
      kind: 'confirm',
      title: `Uninstall ${p.name}?`,
      body: "Its skills, agents and MCP servers won't load in new sessions. You can install it again later.",
      confirm: 'Uninstall',
      danger: true,
      onConfirm: () => void run(`uninstall:${p.name}`, { type: 'pluginUninstall', name: p.name }),
    });

  const browseMarket = (name: string) => {
    setAsked((a) => new Set(a).add(name));
    void act({ type: 'marketBrowse', market: name });
  };

  const clickMarket = (name: string) => {
    if (q) return setShut((s) => toggled(s, name));
    const opening = !open.has(name);
    setOpen((o) => toggled(o, name));
    if (opening && !browse.has(name) && !asked.has(name)) browseMarket(name);
  };

  const addMarket = async () => {
    const src = market.trim();
    if (!src || busy['market-add']) return;
    if ((await run('market-add', { type: 'marketAdd', source: src }, SLOW)) && alive.current) setMarket((cur) => (cur.trim() === src ? '' : cur));
  };

  const removeMarket = (m: MarketDto) =>
    openOverlay({
      kind: 'confirm',
      title: `Remove the ${m.name} marketplace?`,
      body: `Its plugins leave Discover. You can add it again later from ${m.source}.`,
      confirm: 'Remove',
      danger: true,
      onConfirm: () => void run(`market-remove:${m.name}`, { type: 'marketRemove', name: m.name }),
    });

  const pluginMenu = (p: PluginDto, on: boolean): MenuItem[] => [
    { label: 'Update', icon: 'arrow-circle-up', disabled: !!busy[`update:${p.name}`], onSelect: () => update(p.name) },
    { label: on ? 'Turn off' : 'Turn on', icon: on ? 'circle-slash' : 'pass', onSelect: () => void setEnabled(p.name, !on) },
    { label: 'Copy name', icon: 'copy', onSelect: () => void copyText(p.name, 'Copied the name') },
    { sep: true },
    { label: 'Uninstall…', icon: 'trash', danger: true, onSelect: () => uninstall(p) },
  ];

  const marketMenu = (m: MarketDto): MenuItem[] => {
    const page = sourcePage(m.source);
    const items: MenuItem[] = [
      {
        label: 'Browse again',
        icon: 'refresh',
        onSelect: () => {
          setOpen((o) => new Set(o).add(m.name));
          setShut((s) => (s.has(m.name) ? toggled(s, m.name) : s));
          browseMarket(m.name);
        },
      },
    ];
    if (page) items.push({ label: 'Open source page', icon: 'link-external', onSelect: () => openLink(page) });
    items.push({ label: 'Copy source', icon: 'copy', onSelect: () => void copyText(m.source, 'Copied the source') }, { sep: true });
    if (m.isDefault) items.push({ label: 'Remove marketplace', icon: 'trash', hint: 'Default', disabled: true, onSelect: () => {} });
    else items.push({ label: 'Remove marketplace…', icon: 'trash', danger: true, onSelect: () => removeMarket(m) });
    return items;
  };

  // Discover opens on the first (default) marketplace.
  useEffect(() => {
    if (tab !== 'discover' || autoOpened.current || !marketList?.length) return;
    autoOpened.current = true;
    if (open.size) return;
    const first = marketList.find((m) => m.isDefault) ?? marketList[0];
    setOpen(new Set([first.name]));
    if (!browse.has(first.name) && !asked.has(first.name)) browseMarket(first.name);
  }, [tab, marketList]);

  // A search covers every marketplace, so load the ones not loaded yet.
  useEffect(() => {
    if (tab !== 'discover' || !q || !marketList) return;
    for (const m of marketList) if (!browse.has(m.name) && !asked.has(m.name)) browseMarket(m.name);
  }, [tab, q, marketList, browse, asked]);

  useEffect(() => {
    if (!q) setShut((s) => (s.size ? new Set() : s));
  }, [q]);

  // ---------------------------------------------------------------- installed

  const sourceBusy = !!busy[`install:${source.trim()}`];
  const sourceForm = (
    <form
      className="plg-add"
      onSubmit={(e) => {
        e.preventDefault();
        void installTyped();
      }}
    >
      <div className="panel-section">
        <span className="section-label">Install from source</span>
      </div>
      <div className="plg-add-row">
        <input className="input plg-add-input" value={source} placeholder="owner/repo, URL or plugin@marketplace" spellCheck={false} autoComplete="off" aria-label="Plugin source" onChange={(e) => setSource(e.target.value)} />
        <button type="submit" className="btn primary sm" disabled={!source.trim() || sourceBusy}>
          {sourceBusy && <span className="spinner sm" />}
          {sourceBusy ? 'Installing…' : 'Install'}
        </button>
      </div>
    </form>
  );

  const pluginRow = (p: PluginDto) => {
    const on = flip[p.name] ?? p.enabled;
    const leaving = !!busy[`uninstall:${p.name}`];
    const working = leaving ? 'Uninstalling…' : busy[`update:${p.name}`] ? 'Updating…' : null;
    return (
      <div
        key={`${p.marketplace}/${p.name}`}
        className={`list-row plg-row ${on ? '' : 'off'} ${leaving ? 'leaving' : ''}`}
        onContextMenu={(e) => {
          e.preventDefault();
          showMenu(e, pluginMenu(p, on));
        }}
      >
        <Icon name="extensions" className="plg-ico" />
        <div className="plg-main">
          <div className="plg-title">
            <span className="plg-name ellipsis">
              <Hl text={p.name} q={q} />
            </span>
            {p.version && (
              <span className="row-meta plg-ver" data-tip={`Version ${p.version}`}>
                {p.version}
              </span>
            )}
            {!working && (
              <span className="row-actions">
                <button type="button" className="icon-btn sm" data-tip="Update" aria-label={`Update ${p.name}`} onClick={() => update(p.name)}>
                  <Icon name="arrow-circle-up" />
                </button>
                <button type="button" className="icon-btn sm" data-tip="Uninstall" aria-label={`Uninstall ${p.name}`} onClick={() => uninstall(p)}>
                  <Icon name="trash" />
                </button>
              </span>
            )}
            {working && <span className="spinner sm" data-tip={working} aria-label={working} />}
            <span className="plg-switch" data-tip={on ? 'Turn off' : 'Turn on'}>
              <Toggle on={on} onChange={(v) => void setEnabled(p.name, v)} label={p.name} disabled={leaving} />
            </span>
          </div>
          {p.description && <Desc text={p.description} q={q} />}
          {p.marketplace && (
            <div className="plg-meta">
              <Icon name="library" />
              <span className="ellipsis">
                <Hl text={p.marketplace} q={q} />
              </span>
            </div>
          )}
        </div>
      </div>
    );
  };

  let installedView: ReactNode;
  if (!installed) {
    installedView = <Loading text="Loading plugins…" />;
  } else if ('Err' in installed) {
    installedView = (
      <>
        <Problem title="Couldn't load your plugins" detail={installed.Err} onRetry={refresh} />
        {sourceForm}
      </>
    );
  } else if (!installed.Ok.length) {
    installedView = (
      <>
        <div className="empty">
          <Icon name="extensions" />
          <h4>No plugins yet</h4>
          <p>Plugins bundle skills, agents and MCP servers for Copilot. Find some in Discover, or install one from a repo.</p>
          <button type="button" className="btn primary sm plg-empty-btn" onClick={() => setTab('discover')}>
            <Icon name="compass" />
            Discover plugins
          </button>
        </div>
        {sourceForm}
      </>
    );
  } else {
    const list = installed.Ok;
    const shown = list.filter((p) => matches(q, p.name, p.description, p.marketplace));
    const updatingAll = !!busy['update-all'];
    installedView = (
      <>
        <div className="panel-section">
          <span className="section-label">{q ? `${shown.length} of ${list.length}` : `${list.length} installed`}</span>
          <button type="button" className="btn ghost sm plg-section-btn" disabled={updatingAll} onClick={() => void run('update-all', { type: 'pluginUpdateAll' }, SLOW * 2)}>
            {updatingAll ? <span className="spinner sm" /> : <Icon name="arrow-circle-up" />}
            {updatingAll ? 'Updating…' : 'Update all'}
          </button>
        </div>
        {shown.map(pluginRow)}
        {q && !shown.length && (
          <div className="plg-nomatch">
            No installed plugins match “{query.trim()}”.{' '}
            <button type="button" className="link-btn" onClick={() => setTab('discover')}>
              Search Discover
            </button>
          </div>
        )}
        {sourceForm}
      </>
    );
  }

  // ---------------------------------------------------------------- discover

  const marketBusy = !!busy['market-add'];
  const marketForm = (
    <form
      className="plg-add"
      onSubmit={(e) => {
        e.preventDefault();
        void addMarket();
      }}
    >
      <div className="panel-section">
        <span className="section-label">Add a marketplace</span>
      </div>
      <div className="plg-add-row">
        <input className="input plg-add-input" value={market} placeholder="owner/repo or URL" spellCheck={false} autoComplete="off" aria-label="Marketplace source" onChange={(e) => setMarket(e.target.value)} />
        <button type="submit" className="btn sm" disabled={!market.trim() || marketBusy}>
          {marketBusy && <span className="spinner sm" />}
          {marketBusy ? 'Adding…' : 'Add'}
        </button>
      </div>
    </form>
  );

  const card = (m: MarketDto, p: MarketPluginDto) => {
    const src = `${p.name}@${m.name}`;
    return (
      <li key={p.name} className="plg-card">
        <div className="plg-card-head">
          <Icon name="extensions" className="plg-ico" />
          <span className="plg-card-name ellipsis">
            <Hl text={p.name} q={q} />
          </span>
          {have.has(p.name) ? (
            <span className="badge ok">Installed</span>
          ) : busy[`install:${src}`] ? (
            <span className="plg-installing">
              <span className="spinner sm" />
              Installing…
            </span>
          ) : (
            <button type="button" className="btn sm" aria-label={`Install ${p.name}`} onClick={() => void install(src)}>
              Install
            </button>
          )}
        </div>
        {p.description && <Desc text={p.description} q={q} />}
      </li>
    );
  };

  const marketRow = (m: MarketDto) => {
    const entry: Result<MarketPluginDto[]> | undefined = browse.get(m.name);
    const plugins = entry && 'Ok' in entry ? entry.Ok : null;
    const whole = !q || matches(q, m.name, m.source);
    const shown = plugins ? (whole ? plugins : plugins.filter((p) => matches(q, p.name, p.description))) : null;
    if (q && shown && !shown.length) return null;
    const isOpen = q ? !shut.has(m.name) : open.has(m.name);
    const removing = !!busy[`market-remove:${m.name}`];
    let inner: ReactNode = null;
    if (isOpen) {
      if (!entry) inner = <Loading text="Loading plugins…" />;
      else if ('Err' in entry) {
        inner = (
          <div className="plg-quiet danger">
            <span className="selectable">{entry.Err}</span>{' '}
            <button type="button" className="link-btn" onClick={() => browseMarket(m.name)}>
              Retry
            </button>
          </div>
        );
      } else if (shown?.length) inner = <ul className="plg-cards">{shown.map((p) => card(m, p))}</ul>;
      else inner = <div className="plg-quiet">No plugins here yet.</div>;
    }
    return (
      <div key={m.name} className={`plg-market ${isOpen ? 'open' : ''} ${removing ? 'leaving' : ''}`}>
        <div
          className="list-row plg-market-row"
          onContextMenu={(e) => {
            e.preventDefault();
            showMenu(e, marketMenu(m));
          }}
        >
          <button type="button" className="plg-hit" aria-expanded={isOpen} onClick={() => clickMarket(m.name)}>
            <Icon name="chevron-right" className="plg-chev" />
            <span className="plg-market-main">
              <span className="plg-market-title">
                <span className="plg-market-name ellipsis">
                  <Hl text={m.name} q={q} />
                </span>
                {m.isDefault && <span className="badge">Default</span>}
              </span>
              <span className="plg-market-src ellipsis">{m.source}</span>
            </span>
            {plugins && (
              <span className="row-meta" aria-label={plural(plugins.length, 'plugin')}>
                {q && !whole ? `${shown?.length ?? 0}/${plugins.length}` : plugins.length}
              </span>
            )}
          </button>
          {removing ? (
            <span className="spinner sm plg-row-spin" />
          ) : (
            <span className="row-actions">
              <button type="button" className="icon-btn sm" data-tip="More actions" aria-label={`More actions for ${m.name}`} onClick={(e) => showMenu(e.currentTarget, marketMenu(m), { alignRight: true })}>
                <Icon name="ellipsis" />
              </button>
            </span>
          )}
        </div>
        {inner && <div className="plg-market-body">{inner}</div>}
      </div>
    );
  };

  let discoverView: ReactNode;
  if (!markets) {
    discoverView = <Loading text="Loading marketplaces…" />;
  } else if ('Err' in markets) {
    discoverView = (
      <>
        <Problem title="Couldn't load marketplaces" detail={markets.Err} onRetry={refresh} />
        {marketForm}
      </>
    );
  } else if (!markets.Ok.length) {
    discoverView = (
      <>
        <div className="empty">
          <Icon name="library" />
          <h4>No marketplaces</h4>
          <p>Marketplaces list plugins you can install. Add one with its GitHub owner/repo or URL.</p>
        </div>
        {marketForm}
      </>
    );
  } else {
    const rows = markets.Ok.map(marketRow);
    discoverView = (
      <>
        <div className="panel-section">
          <span className="section-label">{plural(markets.Ok.length, 'marketplace')}</span>
        </div>
        {rows}
        {q && !rows.some(Boolean) && <div className="plg-nomatch">No plugins match “{query.trim()}”.</div>}
        {marketForm}
      </>
    );
  }

  return (
    <div className="panel plg">
      <div className="panel-head">
        <h2>Plugins</h2>
        <button type="button" className="icon-btn" data-tip="Refresh" aria-label="Refresh plugins" aria-busy={refreshing} onClick={refresh}>
          {refreshing ? <span className="spinner sm" /> : <Icon name="refresh" />}
        </button>
      </div>
      <div className="panel-toolbar">
        <div className="plg-tabs">
          <Seg
            value={tab}
            onChange={setTab}
            options={[
              {
                value: 'installed',
                label: (
                  <>
                    Installed
                    {!!mine?.length && <span className="plg-tab-count">{mine.length}</span>}
                  </>
                ),
              },
              { value: 'discover', label: 'Discover' },
            ]}
          />
        </div>
        <div className="search-field plg-search">
          <Icon name="search" />
          <input
            ref={searchRef}
            className="input"
            value={query}
            placeholder="Search plugins"
            aria-label="Search plugins"
            spellCheck={false}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape' && query) {
                e.stopPropagation();
                setQuery('');
              }
            }}
          />
          {query && (
            <button
              type="button"
              className="icon-btn sm plg-search-clear"
              aria-label="Clear search"
              onClick={() => {
                setQuery('');
                searchRef.current?.focus();
              }}
            >
              <Icon name="close" />
            </button>
          )}
        </div>
      </div>
      <div className="panel-body plg-body">
        {tab === 'installed' ? installedView : discoverView}
        <p className="plg-foot">
          <Icon name="sparkle" />
          <span>Agents can manage plugins too — just ask.</span>
        </p>
      </div>
    </div>
  );
}
