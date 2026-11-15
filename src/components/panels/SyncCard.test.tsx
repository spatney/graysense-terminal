import { act as reactAct, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, openLink, revealMemory, setSide } from '../../lib/api';
import { useUi } from '../../lib/store';
import type { MemoryDto, SyncDto, SyncPending } from '../../lib/types';
import { SyncCard, SyncPointer } from './SyncCard';

vi.mock('../../lib/api', () => ({ act: vi.fn(), openLink: vi.fn(), revealMemory: vi.fn(), setSide: vi.fn() }));

const ago = (seconds: number) => Math.floor(Date.now() / 1000) - seconds;
const idle: SyncDto = {
  git: true, repo: null, url: null, account: null, device: 'DESKTOP-4F2K', phase: 'idle', busy: false, lastSync: null, lastError: null,
  report: null, pending: null, auto: true, files: true, settings: true, missions: true, changes: false,
};
const connected: SyncDto = { ...idle, repo: 'octocat/memory', url: 'https://github.com/octocat/memory', account: 'GitHub CLI (octocat)', lastSync: ago(300) };
const ask = (patch: Partial<Extract<SyncPending, { kind: 'merge' }>> = {}): SyncDto => ({
  ...idle,
  repo: 'octocat/memory',
  pending: { kind: 'merge', repo: 'octocat/memory', remote: { memories: 42, notes: 7, missions: 1 }, local: { memories: 18, notes: 2, missions: 0 }, from: 'LAPTOP-9QX', at: ago(3 * 86400), ...patch },
});
const merge = ask();

/** A memory the AI left for review: it proposes dropping it. */
const proposed = (id: number): MemoryDto => ({
  id, kind: 'preference', kindLabel: 'Preference', project: null, projectName: null, text: 'Prefers tabs', state: 'active', origin: 'user', confidence: 1, pinned: false, proposal: '',
  created: 1, updated: 1, lastUsed: null, uses: 0, reinforced: 0, sources: [], history: [], commands: [], stale: false, userOwned: true, archivedReason: null, attachments: [], votes: 0,
  opens: 0, lastOpened: null, relevance: 0,
});

function show(sync: SyncDto | null) {
  useUi.setState({ sync, overlay: null });
  return render(<SyncCard />);
}

/** A click, and the answer to it settling (the card holds its buttons until then). */
const click = (el: HTMLElement) =>
  reactAct(async () => {
    fireEvent.click(el);
  });

function confirmation() {
  const o = useUi.getState().overlay;
  if (o?.kind !== 'confirm') throw new Error('no confirmation is open');
  return o;
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(act).mockResolvedValue(undefined);
  vi.mocked(setSide).mockResolvedValue(undefined);
  useUi.setState({ memory: { memories: [], lastRun: null, learning: 0, tidying: false, lastTidy: null } });
});
afterEach(cleanup);

describe('connecting', () => {
  it('creates a new repository under the typed name', async () => {
    show(idle);
    const name = screen.getByRole<HTMLInputElement>('textbox', { name: 'New repository name' });
    expect(name.value).toBe('graysense-memory');
    fireEvent.change(name, { target: { value: ' notes-sync ' } });
    await click(screen.getByRole('button', { name: 'Create and sync' }));
    expect(act).toHaveBeenCalledTimes(1);
    expect(act).toHaveBeenCalledWith({ type: 'syncConnect', repo: 'notes-sync', create: true });
  });

  it('connects to an existing repository, remembers what was typed for each choice, and never submits nothing', async () => {
    show(idle);
    fireEvent.click(screen.getByRole('radio', { name: 'Use an existing repo' }));
    const repo = screen.getByRole<HTMLInputElement>('textbox', { name: 'Repository to connect' });
    const connect = screen.getByRole<HTMLButtonElement>('button', { name: 'Connect and sync' });
    expect(repo.placeholder).toBe('owner/name or https://github.com/owner/name');
    expect(connect.disabled).toBe(true);
    // Enter in the box submits the form.
    fireEvent.submit(screen.getByRole('form', { name: 'Connect to GitHub' }));
    expect(act).not.toHaveBeenCalled();

    fireEvent.change(repo, { target: { value: 'octocat/notes' } });
    fireEvent.click(screen.getByRole('radio', { name: 'Create a new repo' }));
    expect(screen.getByRole<HTMLInputElement>('textbox', { name: 'New repository name' }).value).toBe('graysense-memory');
    fireEvent.click(screen.getByRole('radio', { name: 'Use an existing repo' }));
    expect(screen.getByRole<HTMLInputElement>('textbox', { name: 'Repository to connect' }).value).toBe('octocat/notes');
    await reactAct(async () => {
      fireEvent.submit(screen.getByRole('form', { name: 'Connect to GitHub' }));
    });
    expect(act).toHaveBeenCalledWith({ type: 'syncConnect', repo: 'octocat/notes', create: false });
  });

  it('shows why it failed as the core said it, and holds the form while it works', () => {
    const why = "GitHub can't find octocat/nope. Check the name, or sign in with an account that can (gh auth login).";
    show({ ...idle, lastError: why });
    expect(screen.getByRole('alert').textContent).toBe(why);
    reactAct(() => useUi.setState({ sync: { ...idle, lastError: why, busy: true, phase: 'connecting' } }));
    expect(screen.getByRole('status').textContent).toBe('Connecting to GitHub…');
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.getByRole<HTMLButtonElement>('button', { name: 'Create and sync' }).disabled).toBe(true);
    expect(screen.getByRole<HTMLInputElement>('textbox', { name: 'New repository name' }).disabled).toBe(true);
  });
});

describe('questions', () => {
  it('shows what the repository and this PC hold, and sends the answer chosen', async () => {
    show(merge);
    expect(screen.getByRole('heading', { name: 'octocat/memory already has Graysense data' })).toBeTruthy();
    for (const counts of ['42 memories', '7 notes', '1 mission', '18 memories', '2 notes', '0 missions']) expect(screen.getByText(counts)).toBeTruthy();
    expect(screen.getByText('Last saved by LAPTOP-9QX 3 days ago')).toBeTruthy();

    await click(screen.getByRole('button', { name: 'Merge with AI' }));
    expect(act).toHaveBeenLastCalledWith({ type: 'syncAnswer', choice: 'merge' });
    await click(screen.getByRole('button', { name: 'Cancel' }));
    expect(act).toHaveBeenLastCalledWith({ type: 'syncAnswer', choice: 'cancel' });
    expect(act).toHaveBeenCalledTimes(2);
  });

  it("says so when the repository was last saved under this PC's own name, and says nothing when it can't tell", () => {
    show(ask({ from: 'DESKTOP-4F2K' }));
    expect(screen.getByText('Last saved by this PC (DESKTOP-4F2K) 3 days ago')).toBeTruthy();
    cleanup();
    show(ask({ from: null, at: null }));
    expect(screen.queryByText(/Last saved/)).toBeNull();
  });

  it("replaces this PC's data with the repository's only after confirming", async () => {
    show(merge);
    await click(screen.getByRole('button', { name: "Use the repository's copy" }));
    expect(act).not.toHaveBeenCalled();
    const ask = confirmation();
    expect(ask.danger).toBe(true);
    expect(ask.body).toContain('octocat/memory');
    expect(ask.body).toContain('backup');
    await reactAct(async () => ask.onConfirm());
    expect(act).toHaveBeenCalledWith({ type: 'syncAnswer', choice: 'replace' });
  });

  it('warns that a public repository would show everything, and lets the user go ahead or stop', async () => {
    show({ ...idle, repo: 'octocat/notes', pending: { kind: 'public', repo: 'octocat/notes' } });
    expect(screen.getByText('octocat/notes')).toBeTruthy();
    expect(screen.getByText(/is public, so everything Graysense syncs, including your notes, would be visible to anyone/)).toBeTruthy();
    await click(screen.getByRole('button', { name: 'Use it anyway' }));
    expect(act).toHaveBeenLastCalledWith({ type: 'syncAnswer', choice: 'public' });
    await click(screen.getByRole('button', { name: 'Cancel' }));
    expect(act).toHaveBeenLastCalledWith({ type: 'syncAnswer', choice: 'cancel' });
  });

  it('keeps the answers from being sent while sync is busy', () => {
    show({ ...merge, busy: true, phase: 'fetching' });
    for (const name of ['Merge with AI', "Use the repository's copy", 'Cancel']) expect(screen.getByRole<HTMLButtonElement>('button', { name }).disabled).toBe(true);
  });

  describe('when most of what was synced would be removed', () => {
    const removals = (here: number, there: number): SyncDto => ({ ...connected, pending: { kind: 'removals', repo: 'octocat/memory', here, there, known: 40 } });

    it('says the repository lost it, and keeps by saving it again', async () => {
      show(removals(37, 0));
      expect(screen.getByRole('heading', { name: 'Sync would remove most of your memory' })).toBeTruthy();
      expect(screen.getByText(/octocat\/memory no longer has 37 of the 40 items it held at the last sync, so syncing would remove them from this PC too/)).toBeTruthy();
      expect(screen.getByText(/Nothing has changed yet/)).toBeTruthy();
      await click(screen.getByRole('button', { name: 'Save them to GitHub again' }));
      expect(act).toHaveBeenLastCalledWith({ type: 'syncAnswer', choice: 'keep' });
      await click(screen.getByRole('button', { name: 'Not now' }));
      expect(act).toHaveBeenLastCalledWith({ type: 'syncAnswer', choice: 'cancel' });
    });

    it('removing it asks first, and says what would go', async () => {
      show(removals(37, 0));
      await click(screen.getByRole('button', { name: 'Remove them here' }));
      expect(act).not.toHaveBeenCalled();
      const ask = confirmation();
      expect(ask.danger).toBe(true);
      expect(ask.body).toContain('37 of the 40');
      await reactAct(async () => ask.onConfirm());
      expect(act).toHaveBeenCalledWith({ type: 'syncAnswer', choice: 'remove' });
    });

    it('says this PC lost it, and keeps by bringing it back', async () => {
      show(removals(0, 37));
      expect(screen.getByText(/This PC no longer has 37 of the 40 items in octocat\/memory, so syncing would delete them from GitHub and from your other PCs/)).toBeTruthy();
      await click(screen.getByRole('button', { name: 'Restore them here' }));
      expect(act).toHaveBeenLastCalledWith({ type: 'syncAnswer', choice: 'keep' });
      expect(screen.getByRole('button', { name: 'Delete them from GitHub' })).toBeTruthy();
    });

    it('is shown in the status line of Memory too', () => {
      useUi.setState({ sync: removals(0, 37), memory: { memories: [], lastRun: null, learning: 0, tidying: false, lastTidy: null } });
      render(<SyncPointer />);
      expect(screen.getByText('GitHub sync is waiting for you')).toBeTruthy();
      expect(screen.getByText('Most of what was synced would be removed')).toBeTruthy();
    });
  });
});

describe('connected', () => {
  it('shows the repository, who is signed in, and when it last synced', () => {
    show({ ...connected, changes: true });
    fireEvent.click(screen.getByRole('button', { name: 'Open octocat/memory on GitHub' }));
    expect(openLink).toHaveBeenCalledWith('https://github.com/octocat/memory');
    expect(screen.getByText('Signed in as GitHub CLI (octocat)')).toBeTruthy();
    expect(screen.getByText('Synced 5 minutes ago · changes not on GitHub yet')).toBeTruthy();
    cleanup();
    show({ ...connected, lastSync: null });
    expect(screen.getByText('Not synced yet')).toBeTruthy();
  });

  it('shows a failed sync as the core said it, and Sync now tries again', async () => {
    const why = "Couldn't reach GitHub. Check your connection and try again.";
    show({ ...connected, lastError: why });
    expect(screen.getByRole('alert').textContent).toBe(why);
    await click(screen.getByRole('button', { name: 'Sync now' }));
    expect(act).toHaveBeenCalledWith({ type: 'syncNow' });
  });

  it('sends a double click once, holding the button until the core has taken it', async () => {
    let taken!: () => void;
    vi.mocked(act).mockReturnValue(new Promise((resolve) => (taken = () => resolve(undefined))));
    show(connected);
    const syncNow = screen.getByRole<HTMLButtonElement>('button', { name: 'Sync now' });
    fireEvent.click(syncNow);
    fireEvent.click(syncNow);
    expect(act).toHaveBeenCalledTimes(1);
    expect(syncNow.disabled).toBe(true);
    await reactAct(async () => taken());
    expect(syncNow.disabled).toBe(false);
  });

  it.each([
    ['connecting', 'Connecting to GitHub…'],
    ['fetching', 'Checking the repository…'],
    ['merging', 'Merging…'],
    ['reconciling', 'Merging with AI…'],
    ['pushing', 'Saving to GitHub…'],
  ] as const)('says what it is doing while %s, and holds the buttons', (phase, text) => {
    show({ ...connected, busy: true, phase });
    expect(screen.getByRole('status').textContent).toBe(text);
    expect(screen.queryByText(/^Synced/)).toBeNull();
    expect(screen.getByRole<HTMLButtonElement>('button', { name: 'Sync now' }).disabled).toBe(true);
    expect(screen.getByRole<HTMLButtonElement>('button', { name: 'Disconnect…' }).disabled).toBe(true);
  });

  it('asks before disconnecting, and says nothing is deleted', async () => {
    show(connected);
    await click(screen.getByRole('button', { name: 'Disconnect…' }));
    expect(act).not.toHaveBeenCalled();
    const ask = confirmation();
    expect(ask.body).toContain('octocat/memory');
    expect(ask.body).toContain('left exactly as they are');
    await reactAct(async () => ask.onConfirm());
    expect(act).toHaveBeenCalledWith({ type: 'syncDisconnect' });
  });

  it('summarises the last sync without the zeros, opens what is waiting for review, and lists what it noted', () => {
    useUi.setState({ memory: { memories: [{ ...proposed(9), kind: 'note' }, proposed(12)], lastRun: null, learning: 0, tidying: false, lastTidy: null } });
    show({ ...connected, report: { at: ago(300), pulled: 3, removed: 1, pushed: 2, combined: 0, conflicts: 0, tidied: 2, review: 1, notes: ['2 files over 25 MB stayed on this PC.'] } });
    expect(screen.getByText('Brought in 3 · removed 1 · saved 2 files to GitHub · AI merged 2 ·')).toBeTruthy();
    expect(screen.queryByText(/combined/)).toBeNull();
    // It goes to the memory that waits for review (not to a note), where Memory shows the Review view.
    fireEvent.click(screen.getByRole('button', { name: '1 waiting for your review' }));
    expect(revealMemory).toHaveBeenCalledWith(12);
    expect(setSide).not.toHaveBeenCalled();

    expect(screen.queryByText('2 files over 25 MB stayed on this PC.')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Details (1)' }));
    expect(screen.getByText('2 files over 25 MB stayed on this PC.')).toBeTruthy();
  });

  it('opens Memory when nothing it can point at is waiting for review any more', () => {
    show({ ...connected, report: { at: ago(300), pulled: 0, removed: 0, pushed: 1, combined: 0, conflicts: 0, tidied: 0, review: 2, notes: [] } });
    fireEvent.click(screen.getByRole('button', { name: '2 waiting for your review' }));
    expect(setSide).toHaveBeenCalledWith('memory');
    expect(revealMemory).not.toHaveBeenCalled();
  });

  it('shows nothing about a sync that changed nothing', () => {
    show({ ...connected, report: { at: ago(300), pulled: 0, removed: 0, pushed: 0, combined: 0, conflicts: 0, tidied: 0, review: 0, notes: [] } });
    expect(screen.queryByText(/Brought in|waiting for your review|Details/)).toBeNull();
  });

  it('turns each option on or off', async () => {
    show({ ...connected, settings: false });
    const options = [
      ['Sync automatically', true, { auto: false }],
      ['Files attached to notes', true, { files: false }],
      ['Missions', true, { missions: false }],
      ['Preferences', false, { settings: true }],
    ] as const;
    for (const [name, on, change] of options) {
      const toggle = screen.getByRole('switch', { name });
      expect(toggle.getAttribute('aria-checked')).toBe(String(on));
      await click(toggle);
      expect(act).toHaveBeenLastCalledWith({ type: 'syncOptions', ...change });
    }
    expect(act).toHaveBeenCalledTimes(4);
  });
});

describe('before there is anything to show', () => {
  it('shows a spinner until the core reports', () => {
    show(null);
    expect(screen.getByRole('status', { name: 'Loading GitHub sync' })).toBeTruthy();
    expect(screen.queryByRole('form')).toBeNull();
  });

  it('asks for Git when it is missing, with a way to get it', () => {
    show({ ...idle, git: false });
    expect(screen.getByRole('heading', { name: 'Sync needs Git' })).toBeTruthy();
    expect(screen.queryByRole('form')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Get Git for Windows' }));
    expect(openLink).toHaveBeenCalledWith('https://git-scm.com/download/win');
  });
});

describe('the pointer in Memory settings', () => {
  const pointer = (sync: SyncDto | null) => {
    useUi.setState({ sync });
    return render(<SyncPointer />);
  };

  it('offers to set sync up, or shows where it stands', () => {
    pointer(idle);
    expect(screen.getByText('Back up and sync with GitHub')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Set up GitHub sync' }).textContent).toBe('Set up');
    cleanup();

    pointer(connected);
    expect(screen.getByText('Synced with octocat/memory')).toBeTruthy();
    expect(screen.getByText('5 minutes ago')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Open GitHub sync settings' }).textContent).toBe('Open');
    cleanup();

    pointer({ ...connected, busy: true, phase: 'pushing' });
    expect(screen.getByText('Syncing…')).toBeTruthy();
    cleanup();

    pointer({ ...connected, lastError: "Couldn't reach GitHub." });
    expect(screen.getByText("Couldn't reach GitHub.")).toBeTruthy();
    cleanup();

    const { container } = pointer(null);
    expect(container.firstChild).toBeNull();
  });

  it('opens Settings and scrolls to the Sync section once it is there', async () => {
    const section = document.createElement('section');
    section.id = 'settings-sync';
    section.scrollIntoView = vi.fn();
    document.body.append(section);
    try {
      pointer(idle);
      fireEvent.click(screen.getByRole('button', { name: 'Set up GitHub sync' }));
      await waitFor(() => expect(section.scrollIntoView).toHaveBeenCalledWith({ block: 'start' }));
      expect(setSide).toHaveBeenCalledWith('settings');
    } finally {
      section.remove();
    }
  });
});
