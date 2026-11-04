import {useEffect, useState} from 'react';

export const responsiveQueries = {
  compactContent: '(max-width: 48rem)',
  compactCollection: '(max-width: 62rem)',
} as const;

export function useResponsiveFormFactor(query: string = responsiveQueries.compactContent): 'wide' | 'compact' {
  const [compact, setCompact] = useState(() => typeof window !== 'undefined' && window.matchMedia(query).matches);

  useEffect(() => {
    const media = window.matchMedia(query);
    const update = () => setCompact(media.matches);
    update();
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  }, [query]);

  return compact ? 'compact' : 'wide';
}
