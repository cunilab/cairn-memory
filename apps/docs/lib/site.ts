export const basePath = process.env.NEXT_PUBLIC_BASE_PATH ?? '/cairn-memory';
export const origin = process.env.NEXT_PUBLIC_SITE_ORIGIN ?? 'https://cunilab.github.io';
export const repository = 'https://github.com/cunilab/cairn-memory';

export function canonicalUrl(path: string) {
  return `${origin}${basePath}${path.endsWith('/') ? path : path + '/'}`;
}
