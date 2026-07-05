import { defineConfig } from 'vitepress'

// https://vitepress.dev/reference/site-config
export default defineConfig({
  title: "LakeMind",
  description: "本地优先的 AI 数据分析 Agent 工作台",
  head: [
    ['link', { rel: 'icon', type: 'image/png', href: '/logo.png' }],
    ['link', { rel: 'preconnect', href: 'https://fonts.googleapis.com' }],
    ['link', { rel: 'preconnect', href: 'https://fonts.gstatic.com', crossorigin: '' }],
    ['link', { rel: 'stylesheet', href: 'https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=Outfit:wght@400;500;600;700;800&display=swap' }]
  ],
  themeConfig: {
    logo: {
      light: '/logo.png',
      dark: '/logo_white.png'
    },
    // https://vitepress.dev/reference/default-theme-config
    nav: [
      { text: '首页', link: '/' },
      { text: '产品手册', link: '/guide/' },
      { text: '技术架构', link: '/guide/architecture' }
    ],

    sidebar: [
      {
        text: '产品介绍',
        items: [
          { text: '关于 LakeMind', link: '/guide/' },
          { text: '产品设计与范式思考', link: '/guide/dilemmas-and-paradigm' }
        ]
      },
      {
        text: 'Agent 核心指南',
        items: [
          { text: '快速入门 (LLM配置)', link: '/guide/getting-started' },
          { text: 'Agent 核心工具', link: '/guide/agent-skills' },
          { text: '谷歌 OKF 统一标准', link: '/guide/okf-knowledge' }
        ]
      },
      {
        text: '技术底层',
        items: [
          { text: '系统架构', link: '/guide/architecture' }
        ]
      }
    ],

    socialLinks: [
      { icon: 'github', link: 'https://github.com/tsingliuwin/lakemind' }
    ],
    
    footer: {
      message: 'Released under the MIT License.',
      copyright: 'Copyright © 2026-present LakeMind Authors'
    }
  }
})

