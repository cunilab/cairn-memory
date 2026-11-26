import type { BaseLayoutProps } from 'fumadocs-ui/layouts/shared';
import { basePath, repository } from './site';

export const navigation = {
  nav: {
    url: '/',
    title: (
      <span className="flex items-center gap-2.5 font-semibold">
        <img src={`${basePath}/cairn-logo.svg`} alt="" width={28} height={28} />
        Cairn Memory
      </span>
    ),
  },
  links: [
    { text: 'Users', url: '/docs/users/', active: 'nested-url' },
    { text: 'Technical', url: '/docs/technical/', active: 'nested-url' },
    { text: 'GitHub', url: repository, external: true },
  ],
} satisfies BaseLayoutProps;
