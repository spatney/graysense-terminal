import { describe, expect, it } from 'vitest';
import { isImagePath } from './images';

describe('local image paths', () => {
  it('recognizes Unix, Windows and UNC images for native previews', () => {
    expect(isImagePath('/Users/test/My Pictures/shot.PNG')).toBe(true);
    expect(isImagePath('/shot.webp')).toBe(true);
    expect(isImagePath('C:\\Pictures\\shot.png')).toBe(true);
    expect(isImagePath('\\\\server\\share\\shot.jpg')).toBe(true);
  });

  it('does not treat relative paths, remote URLs or non-images as local images', () => {
    expect(isImagePath('Pictures/shot.png')).toBe(false);
    expect(isImagePath('https://example.com/shot.png')).toBe(false);
    expect(isImagePath('/Users/test/notes.txt')).toBe(false);
  });
});
