import { useEffect, useState } from 'react';
import { readImage } from './api';

const MIME = new Map([
  ['png', 'image/png'],
  ['jpg', 'image/jpeg'],
  ['jpeg', 'image/jpeg'],
  ['gif', 'image/gif'],
  ['webp', 'image/webp'],
  ['bmp', 'image/bmp'],
  ['ico', 'image/x-icon'],
  ['avif', 'image/avif'],
  ['svg', 'image/svg+xml'],
]);

const mimeOf = (path: string) => MIME.get(/\.([a-z0-9]+)$/i.exec(path)?.[1].toLowerCase() ?? '');

/** A full path to an image Graysense can preview (by its extension). */
export function isImagePath(path: string): boolean {
  return !!mimeOf(path) && /^(?:[a-zA-Z]:[\\/]|\\\\|\/)/.test(path);
}

/**
 * A local image as an object URL for as long as the component shows it: `undefined` while it
 * loads, `null` when it can't be shown (gone, or not an image).
 */
export function useImageUrl(path: string | null): string | null | undefined {
  const [loaded, setLoaded] = useState<{ path: string; url: string | null } | null>(null);
  useEffect(() => {
    if (!path) return;
    let live = true;
    let url: string | null = null;
    readImage(path)
      .then((bytes) => {
        if (!live) return;
        url = URL.createObjectURL(new Blob([bytes instanceof ArrayBuffer ? bytes : new Uint8Array(bytes)], { type: mimeOf(path) }));
        setLoaded({ path, url });
      })
      .catch(() => live && setLoaded({ path, url: null }));
    return () => {
      live = false;
      if (url) URL.revokeObjectURL(url);
    };
  }, [path]);
  return path && loaded?.path === path ? loaded.url : undefined;
}
