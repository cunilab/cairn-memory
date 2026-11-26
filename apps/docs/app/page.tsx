import { HomeLayout } from 'fumadocs-ui/layouts/home';
import Link from 'next/link';
import { navigation } from '@/lib/navigation';
import { components } from '@/lib/mdx';
import { canonicalUrl } from '@/lib/site';
import { source } from '@/lib/source';

export const metadata = { alternates: { canonical: canonicalUrl('/') } };

export default function Home() {
  const page = source.getPage([])!;
  const Content = page.data.body;
  const titleId = page.data.toc.find(({ depth }) => depth === 1)?.url.slice(1);

  return (
    <HomeLayout {...navigation}>
      <main id="nd-page">
        <section className="mx-auto grid w-full max-w-6xl gap-10 px-6 py-16 sm:px-10 sm:py-24 lg:grid-cols-[1.25fr_1fr] lg:items-center">
          <div>
            <h1 id={titleId} className="max-w-2xl text-4xl font-semibold tracking-tight sm:text-6xl">{page.data.title}</h1>
            <p className="mt-5 max-w-xl text-lg text-fd-muted-foreground">{page.data.description}</p>
            <div className="mt-8 flex flex-wrap items-center gap-x-6 gap-y-4 text-sm font-semibold">
              <Link href="/docs/users/" className="rounded-lg bg-(--hero-amber) px-5 py-3 text-[#231605] hover:brightness-95">Quickstart</Link>
              <Link href="/docs/users/agents/" className="text-fd-primary hover:underline">Choose an agent</Link>
              <Link href="/docs/technical/" className="text-fd-primary hover:underline">Architecture</Link>
            </div>
          </div>
          <div className="rounded-2xl border border-fd-border bg-fd-card px-6 py-2 sm:px-8">
            <div className="prose"><Content components={components(page)} /></div>
          </div>
        </section>
      </main>
    </HomeLayout>
  );
}
