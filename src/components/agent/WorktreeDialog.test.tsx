import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { createElement } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, worktreeReview } from '../../lib/api';
import { useUi } from '../../lib/store';
import type { BayDto, WorktreeReview } from '../../lib/types';
import { WorktreeDialog } from './WorktreeDialog';

vi.mock('../../lib/api', async (original) => ({ ...(await original<typeof import('../../lib/api')>()), act: vi.fn(), worktreeReview: vi.fn() }));

const bay: BayDto = {
  id: 7, kind: 'agent', title: 'Device-code login', cwd: 'C:\\repo.worktrees\\login', folder: 'login', shell: null,
  agent: {
    sessionId: 's', model: null, effort: null, contextTier: null, mode: 'interactive',
    status: { kind: 'idle', label: 'Ready', detail: null, busy: false, attention: false },
    intent: null, name: null, deferred: false, incognito: false, tokensIn: 0, tokensOut: 0,
    context: null, toolCount: 0, turns: 1, changedFiles: [], busySince: null, mission: null,
    canvases: [], apps: [], attachments: [], autoApprove: false, pending: 0, memoryUsed: 0,
    subagentsActive: 0, project: 'repo', projectKey: 'repo', sentHistory: [], queued: [], canRewind: true, rewind: null, handoff: null, resumable: true, premiumCost: null,
    worktree: { path: 'C:\\repo.worktrees\\login', branch: 'graysense/login', base: 'main', repo: 'C:\\repo', pending: false, busy: null },
  },
};

const review = (patch: Partial<WorktreeReview> = {}): WorktreeReview => ({
  branch: 'graysense/login', base: 'main', added: 3, removed: 1, ahead: 1, behind: 2, dirty: true, truncated: false,
  files: [{ path: 'src/login.ts', added: 3, removed: 1 }],
  diff: 'diff --git a/src/login.ts b/src/login.ts\nindex 1111111..2222222 100644\n--- a/src/login.ts\n+++ b/src/login.ts\n@@ -1 +1,3 @@\n-old\n+new\n+more\n+lines',
  ...patch,
});

describe('a session’s worktree', () => {
  const onClose = vi.fn();
  beforeEach(() => {
    vi.mocked(act).mockReset().mockResolvedValue(null);
    onClose.mockReset();
    useUi.setState({ bays: [bay] });
  });
  afterEach(cleanup);

  it('shows what its branch changes, and merges it with the message given', async () => {
    vi.mocked(worktreeReview).mockResolvedValue(review());
    render(createElement(WorktreeDialog, { bay: 7, onClose }));
    expect(await screen.findByText('1 file changed, 1 commit, some not committed yet')).toBeTruthy();
    expect(screen.getByText('main has 2 new commits')).toBeTruthy();
    expect(screen.getByText('login.ts')).toBeTruthy();
    fireEvent.change(screen.getByPlaceholderText('Device-code login'), { target: { value: 'Log in with device codes' } });
    fireEvent.click(screen.getByRole('switch'));
    fireEvent.click(screen.getByRole('button', { name: /Merge into main/ }));
    expect(act).toHaveBeenCalledWith({ type: 'worktreeMerge', bay: 7, message: 'Log in with device codes', close: false });
    expect(onClose).toHaveBeenCalled();
  });

  it('asks before deleting it, and has nothing to open a pull request for when the branch is unchanged', async () => {
    vi.mocked(worktreeReview).mockResolvedValue(review({ files: [], added: 0, removed: 0, ahead: 0, behind: 0, dirty: false, diff: '' }));
    render(createElement(WorktreeDialog, { bay: 7, onClose }));
    expect(await screen.findByText(/Nothing to merge yet/)).toBeTruthy();
    expect(screen.getByRole('button', { name: /Open pull request/ }).hasAttribute('disabled')).toBe(true);
    fireEvent.click(screen.getByRole('button', { name: /Delete worktree…/ }));
    expect(act).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Delete worktree and branch' }));
    expect(act).toHaveBeenCalledWith({ type: 'closeBay', bay: 7, worktree: 'discard' });
  });

  it('waits while the agent works', async () => {
    vi.mocked(worktreeReview).mockResolvedValue(review());
    useUi.setState({ bays: [{ ...bay, agent: { ...bay.agent!, status: { kind: 'thinking', label: 'Thinking', detail: null, busy: true, attention: false } } }] });
    render(createElement(WorktreeDialog, { bay: 7, onClose }));
    await screen.findByText('1 file changed, 1 commit, some not committed yet');
    expect(screen.getByRole('button', { name: /Merge into main/ }).hasAttribute('disabled')).toBe(true);
    expect(screen.getByText(/Wait for the agent to finish/)).toBeTruthy();
  });
});
