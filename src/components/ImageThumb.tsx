import type { ReactNode } from 'react';
import { baseName } from '../lib/format';
import { useImageUrl } from '../lib/images';
import { openOverlay } from '../lib/store';
import { Icon } from './ui';

/**
 * A local image's thumbnail, which opens large when clicked. `fallback` shows instead when the
 * image can't be (it was deleted, or isn't really an image).
 */
export function ImageThumb({ path, className = '', fallback, onRemove }: { path: string; className?: string; fallback: ReactNode; onRemove?: () => void }) {
  const url = useImageUrl(path);
  const name = baseName(path);
  if (url === null) return <>{fallback}</>;
  return (
    <span className={`img-thumb ${className} ${url ? '' : 'loading'}`} data-tip={name}>
      <button type="button" className="img-thumb-open" aria-label={`Preview ${name}`} onClick={() => openOverlay({ kind: 'image', path })}>
        {url && <img src={url} alt={name} draggable={false} />}
      </button>
      {onRemove && (
        <button type="button" className="img-thumb-remove" aria-label={`Remove ${name}`} onClick={onRemove}>
          <Icon name="close" />
        </button>
      )}
    </span>
  );
}
