import { defineConfig } from 'vitepress'

// https://vitepress.dev/reference/site-config
export default defineConfig({
  title: "LakeMind",
  description: "本地优先的 AI 数据分析 Agent 工作台",
  head: [
    ['link', { rel: 'icon', type: 'image/x-icon', href: '/favicon.ico' }],
    ['link', { rel: 'apple-touch-icon', href: '/favicon.ico' }],
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
      { text: '技术架构', link: '/guide/architecture' },
      { text: '变更日志', link: '/changelog' }
    ],

    sidebar: [
      {
        text: '产品介绍',
        items: [
          { text: '关于 LakeMind', link: '/guide/' },
          { text: '产品设计与范式思考', link: '/guide/dilemmas-and-paradigm' },
          { text: '版本变更日志', link: '/changelog' }
        ]
      },
      {
        text: 'Agent 核心指南',
        items: [
          { text: '快速入门', link: '/guide/getting-started' },
          { text: 'Agent 核心工具', link: '/guide/agent-skills' },
          { text: '谷歌 OKF 统一标准', link: '/guide/okf-knowledge' },
          { text: '可进化的分析准则', link: '/guide/evolving-tenets' }
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
      message: '本网站内容版权归 LakeMind 所有，未经授权请勿转载。',
      copyright: 'Copyright © 2026-present LakeMind Authors'
    }
  }
})

