import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { readImage } from '../lib/api';
import { isImagePath } from '../lib/images';
import { useUi } from '../lib/store';
import { ImageThumb } from './ImageThumb';

vi.mock('../lib/api', () => ({ readImage: vi.fn() }));

beforeEach(() => {
  vi.clearAllMocks();
  useUi.setState({ overlay: null });
  URL.createObjectURL = vi.fn(() => 'blob:shot');
  URL.revokeObjectURL = vi.fn();
});
afterEach(cleanup);

describe('image thumbnails', () => {
  it('shows a local image, opens it large, and lets go of it when it leaves', async () => {
    vi.mocked(readImage).mockResolvedValue(new Uint8Array([0x89, 0x50, 0x4e, 0x47]).buffer);
    const { unmount } = render(<ImageThumb path="C:\shots\error.png" fallback={<span>error.png</span>} />);
    const img = await screen.findByRole('img', { name: 'error.png' });
    expect(img.getAttribute('src')).toBe('blob:shot');
    expect(readImage).toHaveBeenCalledWith('C:\\shots\\error.png');
    fireEvent.click(screen.getByRole('button', { name: 'Preview error.png' }));
    expect(useUi.getState().overlay).toEqual({ kind: 'image', path: 'C:\\shots\\error.png' });
    unmount();
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:shot');
  });

  it('shows the fallback when the image is gone', async () => {
    vi.mocked(readImage).mockRejectedValue("Couldn't open C:\\shots\\old.png");
    render(<ImageThumb path="C:\shots\old.png" fallback={<span>old.png (file)</span>} />);
    await screen.findByText('old.png (file)');
    expect(screen.queryByRole('img')).toBeNull();
  });

  it('previews only images with a full path', () => {
    expect(isImagePath('C:\\shots\\Error.PNG')).toBe(true);
    expect(isImagePath('\\\\server\\share\\logo.svg')).toBe(true);
    // A name without a path (from a session started elsewhere) can't be read.
    expect(isImagePath('error.png')).toBe(false);
    expect(isImagePath('C:\\notes\\todo.txt')).toBe(false);
  });
});
