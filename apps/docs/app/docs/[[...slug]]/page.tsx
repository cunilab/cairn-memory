import { notFound } from 'next/navigation';
import { DocsBody, DocsDescription, DocsPage, DocsTitle } from 'fumadocs-ui/layouts/docs/page';
import { source } from '@/lib/source';
import { components } from '@/lib/mdx';
import { canonicalUrl } from '@/lib/site';

type Props = { params: Promise<{ slug?: string[] }> };

export const dynamicParams = false;

export function generateStaticParams() {
  return source.generateParams().filter(({ slug }) => slug.length > 0);
}

export async function generateMetadata({ params }: Props) {
  const { slug } = await params;
  const page = slug && source.getPage(slug);
  if (!page) notFound();

  return {
    title: page.data.title,
    description: page.data.description,
    alternates: { canonical: canonicalUrl(page.url) },
  };
}

export default async function Article({ params }: Props) {
  const { slug } = await params;
  const page = slug && source.getPage(slug);
  if (!page) notFound();

  const { body: Content, toc, title, description } = page.data;
  const titleId = toc.find(({ depth }) => depth === 1)?.url.slice(1);
  const headings = toc.filter(({ depth }) => depth > 1 && (page.path !== 'glossary.md' || depth === 2));
  return (
    <DocsPage
      toc={headings}
      tableOfContent={{ container: { role: 'navigation', 'aria-labelledby': 'toc-title' } }}
      tableOfContentPopover={{ container: { role: 'navigation', 'aria-label': 'On this page' } }}
    >
      <DocsTitle id={titleId} className="docs-title">{title}</DocsTitle>
      <DocsDescription className="docs-description">{description}</DocsDescription>
      <DocsBody className="docs-copy"><Content components={components(page)} /></DocsBody>
    </DocsPage>
  );
}
