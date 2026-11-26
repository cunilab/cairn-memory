import posix from 'node:path/posix';
import Link from 'next/link';
import { useId } from 'react';
import defaultComponents from 'fumadocs-ui/mdx';
import { Tab, Tabs } from 'fumadocs-ui/components/tabs';
import { Steps, Step } from 'fumadocs-ui/components/steps';
import type { MDXComponents } from 'mdx/types';
import { render } from 'pele';
import { source } from './source';
import { basePath, repository } from './site';

function Mermaid({ chart }: { chart: string }) {
  const id = useId();
  const wide = render(chart, { idPrefix: id });
  const narrow = render(chart, { idPrefix: `${id}-small`, maxWidth: 320 });
  return (
    <div className="@container my-6" aria-hidden="true">
      <div className="hidden @min-[560px]:block" dangerouslySetInnerHTML={{ __html: wide.svg }} />
      <div className="@min-[560px]:hidden" dangerouslySetInnerHTML={{ __html: narrow.svg }} />
    </div>
  );
}

export function components(page: NonNullable<ReturnType<typeof source.getPage>>): MDXComponents {
  return {
    ...defaultComponents,
    Mermaid,
    Steps,
    Step,
    Tab,
    Tabs,
    // Keep headings in source for GitHub. Layouts render the page title.
    h1: () => null,
    a: ({ href, children, ...props }) => {
      if (!href || /^(?:[a-z]+:|#|\/\/)/i.test(href)) {
        return <a href={href} {...props}>{children}</a>;
      }

      const [target, fragment] = href.split('#', 2);
      const suffix = fragment ? `#${fragment}` : '';
      const path = posix.normalize(posix.join(posix.dirname(page.path), target));
      const linked = source.getPages().find((candidate) => candidate.path === path);
      if (linked) return <Link href={linked.url + suffix} {...props}>{children}</Link>;

      const file = posix.normalize(posix.join('apps/docs/content', path));
      if (file.startsWith('apps/docs/public/')) {
        return <a href={`${basePath}/${file.slice('apps/docs/public/'.length)}${suffix}`} {...props}>{children}</a>;
      }

      return <a href={`${repository}/blob/main/${file}${suffix}`} {...props}>{children}</a>;
    },
  };
}
