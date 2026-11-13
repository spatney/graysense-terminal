/** "just now", "5m", "3h", "Yesterday", "Mon", "Nov 10" — for lists. `ts` in Unix seconds. */
export function ago(ts: number, now = Date.now() / 1000): string {
  const s = Math.max(0, now - ts);
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  const d = new Date(ts * 1000);
  if (s < 2 * 86400) return 'Yesterday';
  if (s < 6 * 86400) return d.toLocaleDateString(undefined, { weekday: 'short' });
  const sameYear = d.getFullYear() === new Date(now * 1000).getFullYear();
  return d.toLocaleDateString(undefined, sameYear ? { month: 'short', day: 'numeric' } : { month: 'short', day: 'numeric', year: 'numeric' });
}

/** "5 minutes ago", "3 hours ago", "on Nov 10". */
export function agoLong(ts: number, now = Date.now() / 1000): string {
  const s = Math.max(0, now - ts);
  const n = (v: number, unit: string) => `${v} ${unit}${v === 1 ? '' : 's'} ago`;
  if (s < 45) return 'just now';
  if (s < 3600) return n(Math.max(1, Math.round(s / 60)), 'minute');
  if (s < 86400) return n(Math.round(s / 3600), 'hour');
  if (s < 7 * 86400) return n(Math.round(s / 86400), 'day');
  return `on ${new Date(ts * 1000).toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' })}`;
}

/** "0.4s", "45s", "3m 12s", "1h 4m". */
export function duration(ms: number): string {
  const s = ms / 1000;
  if (s < 10) return `${s.toFixed(1)}s`;
  if (s < 60) return `${Math.round(s)}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${Math.round(s % 60)}s`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

/** 1.2k, 3.4M. */
export function human(n: number): string {
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)}k`;
  return String(n);
}

export function bytes(n: number): string {
  if (n >= 1 << 30) return `${(n / (1 << 30)).toFixed(1)} GB`;
  if (n >= 1 << 20) return `${(n / (1 << 20)).toFixed(1)} MB`;
  if (n >= 1 << 10) return `${Math.round(n / (1 << 10))} KB`;
  return `${n} B`;
}

/** Date groups for history lists, like chat apps: Today, Yesterday, Previous 7 days, … */
export function dateGroup(ts: number, now = new Date()): string {
  const d = new Date(ts * 1000);
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((day(now) - day(d)) / 86400000);
  if (days <= 0) return 'Today';
  if (days === 1) return 'Yesterday';
  if (days < 7) return 'Previous 7 days';
  if (days < 30) return 'Previous 30 days';
  return d.toLocaleDateString(undefined, d.getFullYear() === now.getFullYear() ? { month: 'long' } : { month: 'long', year: 'numeric' });
}

/** The last part of a path ("C:\\code\\app" → "app"). */
export function baseName(path: string): string {
  const parts = path.replace(/[\\/]+$/, '').split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/** The folder a path is in. */
export function dirName(path: string): string {
  const i = Math.max(path.lastIndexOf('\\'), path.lastIndexOf('/'));
  return i > 0 ? path.slice(0, i) : path;
}

/** Home shortened to "~". */
export function shortPath(path: string, home?: string | null): string {
  if (!home) return path;
  const h = home.replace(/[\\/]+$/, '');
  const next = path.charAt(h.length);
  if (path.toLowerCase().startsWith(h.toLowerCase()) && (next === '' || next === '\\' || next === '/')) return `~${path.slice(h.length)}`;
  return path;
}

export function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}

const EFFORTS: Record<string, [label: string, short: string]> = {
  none: ['Off', 'Off'],
  minimal: ['Minimal', 'Min'],
  low: ['Low', 'Low'],
  medium: ['Medium', 'Med'],
  high: ['High', 'High'],
  xhigh: ['Extra high', 'X-High'],
  max: ['Max', 'Max'],
};

/** A thinking effort level as people say it ("xhigh" → "Extra high"). */
export function effortLabel(effort: string, short = false): string {
  const known = EFFORTS[effort.toLowerCase()];
  if (known) return known[short ? 1 : 0];
  return effort.charAt(0).toUpperCase() + effort.slice(1);
}

/** A model id as people say it ("claude-sonnet-4.5" → "Claude Sonnet 4.5"). */
export function modelLabel(id: string | null | undefined, models?: { id: string; name: string }[]): string {
  if (!id) return 'Default model';
  const known = models?.find((m) => m.id === id);
  if (known?.name) return known.name;
  return id
    .split('-')
    .map((w) => (/^gpt$/i.test(w) ? 'GPT' : w.charAt(0).toUpperCase() + w.slice(1)))
    .join(' ')
    .replace(/^GPT /, 'GPT-');
}
