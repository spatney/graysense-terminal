import { describe, expect, it } from 'vitest';
import { linksIn, looksLikeQuestion, matchRanges, rankNotes } from './notes';
import type { MemoryDto } from './types';

function note(id: number, text: string, extra: Partial<MemoryDto> = {}): MemoryDto {
  return {
    id,
    kind: 'note',
    kindLabel: 'Note',
    project: null,
    projectName: null,
    text,
    state: 'active',
    origin: 'user',
    confidence: 1,
    pinned: false,
    proposal: null,
    created: 1_700_000_000,
    updated: 1_700_000_000,
    lastUsed: null,
    uses: 0,
    reinforced: 0,
    sources: [],
    history: [],
    commands: [],
    stale: false,
    userOwned: true,
    archivedReason: null,
    attachments: [],
    votes: 0,
    opens: 0,
    lastOpened: null,
    relevance: 0.5,
    ...extra,
  };
}

describe('notes search', () => {
  const notes = [
    note(1, 'When Charles asked me for the Rayfin logo, I sent him the dark version', {
      attachments: [{ kind: 'file', name: 'rayfin-logo-dark.png', target: 'C:\\art\\rayfin-logo-dark.png', stored: null, size: 1200, exists: true }],
    }),
    note(2, 'Rayfin launch checklist lives in the planning folder'),
    note(3, 'Gave Priya the Q3 budget numbers'),
  ];

  it('answers a question by who and what, not by its filler words', () => {
    const hits = rankNotes(notes, 'What did I give Charles for the Rayfin logo?');
    expect(hits[0].note.id).toBe(1);
    expect(hits.map((h) => h.note.id)).not.toContain(3);
  });

  it('finds notes by their files', () => {
    expect(rankNotes(notes, 'logo-dark')[0].note.id).toBe(1);
  });

  it('ranks more useful notes higher when the text matches equally', () => {
    const a = note(10, 'Partner deck draft', { relevance: 0.1 });
    const b = note(11, 'Partner deck final', { relevance: 0.9 });
    expect(rankNotes([a, b], 'partner deck')[0].note.id).toBe(11);
  });

  it('tells questions from filters, and finds links and matches', () => {
    expect(looksLikeQuestion('what did I send Charles')).toBe(true);
    expect(looksLikeQuestion('rayfin logo')).toBe(false);
    expect(linksIn('see https://rayfin.ai/brand. and (https://x.dev/a)')).toEqual(['https://rayfin.ai/brand', 'https://x.dev/a']);
    expect(matchRanges('Rayfin logo for Rayfin', 'rayfin')).toEqual([
      [0, 6],
      [16, 22],
    ]);
  });
});
