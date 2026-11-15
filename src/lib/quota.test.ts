import { describe, expect, it } from 'vitest';
import { quotaStatus } from './quota';
import type { QuotaDto } from './types';

const base: QuotaDto = { entitlement: 300, used: 120, remainingPercent: 60, unlimited: false, overage: 0, resetDate: null };

describe('quotaStatus', () => {
  it('counts the requests left and what the session used', () => {
    const s = quotaStatus(base, 4.5);
    expect(s.text).toBe('180 left');
    expect(s.tip).toBe('180 of 300 premium requests left. This session used 4.5.');
    expect(s.low).toBe(false);
  });

  it('warns when under a tenth is left, and when the allowance is spent', () => {
    expect(quotaStatus({ ...base, used: 280 }).low).toBe(true);
    const over = quotaStatus({ ...base, used: 300, overage: 12.33 });
    expect(over.text).toBe('12.33 over');
    expect(over.low).toBe(true);
  });

  it('falls back to the percentage, and says when there is no limit', () => {
    expect(quotaStatus({ ...base, entitlement: 0, used: 0, remainingPercent: 7.6 })).toMatchObject({ text: '8% left', low: true });
    expect(quotaStatus({ ...base, unlimited: true }, 0)).toEqual({ text: 'Unlimited', tip: 'Premium requests are unlimited.', low: false });
  });
});
