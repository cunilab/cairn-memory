import { DocsLayout } from 'fumadocs-ui/layouts/docs';
import { navigation } from '@/lib/navigation';
import { source } from '@/lib/source';

export default function Layout({ children }: { children: React.ReactNode }) {
  return <DocsLayout tree={source.getPageTree()} tabs={false} {...navigation}>{children}</DocsLayout>;
}
