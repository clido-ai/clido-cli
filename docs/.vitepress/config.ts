import { defineConfig } from 'vitepress'

export default defineConfig({
  title: 'clido',
  description: 'AI coding agent for your terminal',

  head: [
    ['link', { rel: 'icon', type: 'image/svg+xml', href: '/logo.svg' }],
  ],

  themeConfig: {
    logo: '/logo.svg',

    nav: [
      { text: 'Guide', link: '/guide/introduction', activeMatch: '/guide/' },
      { text: 'Reference', link: '/reference/cli', activeMatch: '/reference/' },
      { text: 'Developer', link: '/developer/architecture', activeMatch: '/developer/' },
    ],

    sidebar: {
      '/guide/': [
        {
          text: 'Getting Started',
          items: [
            { text: 'Introduction', link: '/guide/introduction' },
            { text: 'Installation', link: '/guide/installation' },
            { text: 'Quick Start', link: '/guide/quick-start' },
            { text: 'First Run', link: '/guide/first-run' },
          ],
        },
        {
          text: 'Using clido',
          items: [
            { text: 'Interactive TUI', link: '/guide/tui' },
            { text: 'Running Prompts', link: '/guide/running-prompts' },
            { text: 'Session Management', link: '/guide/sessions' },
            { text: 'Configuration', link: '/guide/configuration' },
          ],
        },
        {
          text: 'Features',
          items: [
            { text: 'Providers & Models', link: '/guide/providers' },
            { text: 'Skills', link: '/guide/skills' },
            { text: 'Memory', link: '/guide/memory' },
            { text: 'Repository Index', link: '/guide/index-search' },
            { text: 'Workflows', link: '/guide/workflows' },
            { text: 'MCP Servers', link: '/guide/mcp' },
            { text: 'Planner (experimental)', link: '/guide/planner' },
            { text: 'Harness mode', link: '/guide/harness' },
            { text: 'Audit Log', link: '/guide/audit' },
          ],
        },
      ],

      '/reference/': [
        {
          text: 'CLI Reference',
          items: [
            { text: 'All Commands', link: '/reference/cli' },
            { text: 'All Flags', link: '/reference/flags' },
            { text: 'Output Formats', link: '/reference/output-formats' },
            { text: 'Exit Codes', link: '/reference/exit-codes' },
          ],
        },
        {
          text: 'Configuration Reference',
          items: [
            { text: 'config.toml', link: '/reference/config' },
            { text: 'Environment Variables', link: '/reference/env-vars' },
          ],
        },
        {
          text: 'TUI Reference',
          items: [
            { text: 'Slash Commands', link: '/reference/slash-commands' },
            { text: 'Key Bindings', link: '/reference/key-bindings' },
          ],
        },
      ],

      '/developer/': [
        {
          text: 'Internals',
          items: [
            { text: 'Architecture', link: '/developer/architecture' },
            { text: 'Crate Overview', link: '/developer/crates' },
            { text: 'Building & Testing', link: '/developer/building' },
          ],
        },
        {
          text: 'Extending clido',
          items: [
            { text: 'Adding Tools', link: '/developer/adding-tools' },
            { text: 'Adding Providers', link: '/developer/adding-providers' },
          ],
        },
        {
          text: 'Data Formats',
          items: [
            { text: 'Session Format', link: '/developer/session-format' },
          ],
        },
        {
          text: 'Community',
          items: [
            { text: 'Contributing', link: '/developer/contributing' },
          ],
        },
      ],
    },

    socialLinks: [
      { icon: 'github', link: 'https://github.com/kurtbuilds/clido' },
    ],

    search: {
      provider: 'local',
    },

    footer: {
      message: 'Released under the MIT License.',
      copyright: 'Copyright © 2025-present clido contributors',
    },

    editLink: {
      pattern: 'https://github.com/kurtbuilds/clido/edit/master/docs/:path',
      text: 'Edit this page on GitHub',
    },
  },
})
