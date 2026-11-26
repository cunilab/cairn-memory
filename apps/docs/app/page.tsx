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
    <>
      <a href="#nd-page" className="not-focus-visible:sr-only fixed start-2 top-2 z-50 rounded-lg bg-fd-background px-3 py-2 shadow-lg">Skip to content</a>
      <HomeLayout {...navigation}>
        <div id="nd-page" tabIndex={-1}>
          <section className="home-hero" aria-labelledby={titleId}>
            <div className="mx-auto grid w-full max-w-6xl gap-10 px-5 py-12 sm:px-10 sm:py-20 lg:grid-cols-[1.28fr_0.72fr] lg:items-center">
              <div>
                <h1 id={titleId} className="max-w-xl text-[clamp(2.6rem,5.6vw,4.25rem)] leading-[1.04] font-semibold tracking-[-0.06em]">{page.data.title}</h1>
                <p className="mt-5 max-w-lg text-lg leading-relaxed text-fd-muted-foreground">{page.data.description}</p>
                <div className="mt-7 flex flex-wrap items-center gap-x-6 gap-y-3 text-sm font-semibold">
                  <Link href="/docs/users/" className="inline-flex min-h-11 items-center rounded-lg bg-(--hero-amber) px-5 text-[#231605] hover:brightness-95">Quickstart</Link>
                  <Link href="/docs/technical/deployment/" className="inline-flex min-h-11 items-center text-fd-primary hover:underline">Deployment guide</Link>
                </div>
              </div>
              <figure className="home-example p-4 sm:p-6">
                <figcaption className="border-b border-fd-border pb-3 text-sm font-semibold">A decision across two sessions</figcaption>
                <dl className="mt-4 grid gap-3 text-sm leading-relaxed">
                  <div className="px-3">
                    <dt>Session 1 · Save</dt>
                    <dd>“Give every database test suite a disposable schema.”</dd>
                  </div>
                  <div className="home-example-recall px-3 py-3">
                    <dt>Session 2 · Recall</dt>
                    <dd>The next agent sees the decision before writing a test.</dd>
                  </div>
                </dl>
              </figure>
            </div>
          </section>
          <div className="mx-auto w-full max-w-6xl px-5 sm:px-10">
            <Content components={components(page)} />
          </div>
        </div>
      </HomeLayout>
    </>
  );
}
