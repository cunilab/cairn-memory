import type { MetadataRoute } from 'next';
import { canonicalUrl } from '@/lib/site';
import { source } from '@/lib/source';

export const dynamic = 'force-static';

export default function sitemap(): MetadataRoute.Sitemap {
  return source.getPages().map(({ url }) => ({ url: canonicalUrl(url) }));
}
