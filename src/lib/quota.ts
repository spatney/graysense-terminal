import type { QuotaDto } from './types';

/** Premium requests can be fractional (models cost 0.33×, 1×, 10×…). */
function count(n: number): string {
  return String(+n.toFixed(2));
}

function resets(date: string | null): string {
  if (!date) return '';
  const d = new Date(date);
  return Number.isNaN(d.getTime()) ? '' : ` Resets ${d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })}.`;
}

/** The status bar's view of the premium allowance, plus what the focused session has used. */
export function quotaStatus(q: QuotaDto, sessionCost?: number | null): { text: string; tip: string; low: boolean } {
  const session = sessionCost ? ` This session used ${count(sessionCost)}.` : '';
  if (q.unlimited) {
    return { text: 'Unlimited', tip: `Premium requests are unlimited.${session}`, low: false };
  }
  if (q.entitlement > 0) {
    const left = Math.max(0, q.entitlement - q.used);
    if (left === 0 && q.overage > 0) {
      return { text: `${count(q.overage)} over`, tip: `All ${q.entitlement} premium requests are used, and ${count(q.overage)} more were billed as extra.${resets(q.resetDate)}${session}`, low: true };
    }
    return {
      text: `${count(left)} left`,
      tip: `${count(left)} of ${q.entitlement} premium requests left.${resets(q.resetDate)}${session}`,
      low: left < q.entitlement * 0.1,
    };
  }
  const pct = Math.round(q.remainingPercent);
  return { text: `${pct}% left`, tip: `${pct}% of your premium allowance is left.${resets(q.resetDate)}${session}`, low: q.remainingPercent < 10 };
}
