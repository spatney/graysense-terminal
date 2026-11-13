import { describe, expect, it } from 'vitest';
import { activeMention, completionQuery, insertMention, mentionedFiles } from './mentions';

describe('file mentions', () => {
  it('finds the token at the caret without treating emails or selections as mentions', () => {
    expect(activeMention('Read @src\\Co', 12)).toEqual({ start: 5, end: 12, query: 'src\\Co' });
    expect(activeMention('a@b.com', 7)).toBeNull();
    expect(activeMention('Read @src', 9, 5)).toBeNull();
    expect(activeMention('Read @"C:\\My Files\\a', 20)?.query).toBe('C:\\My Files\\a');
    expect(activeMention('Read @"C:\\a.txt" ', 17)).toBeNull();
  });

  it('resolves relative, parent, absolute, UNC and home paths', () => {
    const query = (s: string) => completionQuery(s, 'C:\\repo', 'C:\\Users\\Me');
    expect(query('')).toEqual({ directory: 'C:\\repo\\', prefix: '' });
    expect(query('src/Co')).toEqual({ directory: 'C:\\repo\\src\\', prefix: 'Co' });
    expect(query('../README')).toEqual({ directory: 'C:\\repo\\..\\', prefix: 'README' });
    expect(query('D:\\Docs\\a')).toEqual({ directory: 'D:\\Docs\\', prefix: 'a' });
    expect(query('D:')).toEqual({ directory: 'D:\\', prefix: '' });
    expect(query('/Docs/a')).toEqual({ directory: 'C:\\Docs\\', prefix: 'a' });
    expect(query('\\\\server\\share\\a')).toEqual({ directory: '\\\\server\\share\\', prefix: 'a' });
    expect(query('~/a')).toEqual({ directory: 'C:\\Users\\Me\\', prefix: 'a' });
    expect(completionQuery('src/a', '/repo', '/home/me')).toEqual({ directory: '/repo/src/', prefix: 'a' });
  });

  it('replaces the full token when completing in the middle and keeps surrounding text', () => {
    const text = 'Read @src\\old then explain';
    const mention = activeMention(text, 11)!;
    const next = insertMention(text, mention, { path: 'C:\\repo\\My File.ts', isDir: false });
    expect(next.text).toBe('Read @"C:\\repo\\My File.ts"  then explain');
    expect(next.text.slice(0, next.caret)).toBe('Read @"C:\\repo\\My File.ts" ');
    expect(mentionedFiles(next.text)).toEqual(['C:\\repo\\My File.ts']);
  });

  it('continues completing inside a selected folder with spaces', () => {
    const text = 'Read @My then explain';
    const folder = insertMention(text, activeMention(text, 8)!, { path: 'C:\\My Files', isDir: true });
    expect(activeMention(folder.text, folder.caret)?.query).toBe('C:\\My Files\\');
    expect(mentionedFiles(folder.text)).toEqual([]);
    const file = insertMention(folder.text, activeMention(folder.text, folder.caret)!, { path: 'C:\\My Files\\a.txt', isDir: false });
    expect(mentionedFiles(file.text)).toEqual(['C:\\My Files\\a.txt']);
    expect(file.text).toBe('Read @"C:\\My Files\\a.txt"  then explain');
  });

  it('only attaches completed absolute mentions and deduplicates them', () => {
    expect(mentionedFiles('@"C:\\a.txt" @"C:\\a.txt" @person a@b.com @"relative.txt" @"C:\\unfinished')).toEqual(['C:\\a.txt']);
    expect(mentionedFiles('@"C:\\a.txt"')).toEqual(['C:\\a.txt']);
    expect(mentionedFiles('No mention left')).toEqual([]);
  });
});
