import type { Metadata } from 'next';
import { GeistSans } from 'geist/font/sans';
import { GeistMono } from 'geist/font/mono';
import { RootProvider } from 'fumadocs-ui/provider/next';
import { basePath } from '@/lib/site';
import './globals.css';

export const metadata: Metadata = {
  title: { default: 'Cairn Memory', template: '%s | Cairn Memory' },
  description: 'Shared memory for coding agents and the people who run them.',
  icons: `${basePath}/cairn-logo.svg`,
};

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en" className={`${GeistSans.variable} ${GeistMono.variable}`} suppressHydrationWarning>
      <body>
        <RootProvider search={{ options: {
          type: 'static',
          api: `${basePath}/search-index.json`,
        } }}>
          {children}
        </RootProvider>
      </body>
    </html>
  );
}
