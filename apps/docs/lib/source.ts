import { loader } from 'fumadocs-core/source';
import { docs } from '@/.source/server';

export const source = loader({
  baseUrl: '/docs',
  url: (slugs) => (slugs.length ? `/docs/${slugs.join('/')}/` : '/'),
  source: docs.toFumadocsSource(),
});
