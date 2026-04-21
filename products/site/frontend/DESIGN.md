# Design System — GitIM Website

## Product Context
- **What this is:** AI Agent 团队协作工具的产品主页 (gitim.io)
- **Who it's for:** 开发者、AI agent 运维人员，对 agent 团队协作有需求的技术用户
- **Space/industry:** 开发者工具 / AI Agent 编排
- **Project type:** Product landing page + early access gate
- **Product architecture:** Web app (app.gitim.io) 是"皮儿"，所有运行逻辑和数据保存在用户本地。用户需本地安装一个包，web app 连接该本地包。白名单邀请制，首批约 50 名额。

## Aesthetic Direction
- **Direction:** Quiet Futurism — 安静的确信，不是赛博朋克的"酷炫未来"。极致克制，大面积负空间，产品截图做说服。设计在说："我们已经在这里了，你来不来？"
- **Decoration level:** Minimal — 排版和空间做所有工作。唯一允许的装饰是产品截图周围极微弱的 glow，暗示技术的存在。没有网格线背景，没有渐变 blob，没有粒子效果。
- **Mood:** 未来感。不是 hacker/cyberpunk，是"这就是 AI 时代协作该有的样子"。安静、精确、自信。
- **Key principle:** 克制即力量。少即是多。安静比喧嚣更有说服力。

## Typography
- **Display/Hero:** Space Grotesk 700 — 几何、精确、微妙的字母差异(g, a, 数字形状)创造辨识度。大部分 dev tool 用 Inter 或系统字体，Space Grotesk 立刻创造视觉差异。
- **Body:** Plus Jakarta Sans 400/500 — 与 app.gitim.io 产品端保持一致。温暖但不随意，长文本可读性优秀。
- **UI/Labels:** Plus Jakarta Sans 500
- **Data/Tables:** JetBrains Mono 400 — 支持 tabular-nums，agent ID、路径、技术值用 mono
- **Code:** JetBrains Mono 400
- **Loading:** Google Fonts CDN
  ```html
  <link href="https://fonts.googleapis.com/css2?family=Space+Grotesk:wght@400;500;600;700&family=Plus+Jakarta+Sans:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500&display=swap" rel="stylesheet">
  ```
- **Scale:**
  - xs: 11px / 0.6875rem
  - sm: 13px / 0.8125rem
  - base: 16px / 1rem
  - lg: 18px / 1.125rem
  - xl: 22px / 1.375rem
  - 2xl: 28px / 1.75rem
  - 3xl: 36px / 2.25rem
  - hero: 56px / 3.5rem (letter-spacing: -0.025em)

## Color
- **Approach:** Restrained — 一个强调色，一个意思。颜色稀少且有意义。
- **Background:** `#09090B` (zinc-950, 近黑微暖)
- **Surface:** `#18181B` (zinc-900, 卡片、代码块)
- **Surface hover:** `#27272A` (zinc-800)
- **Border:** `#27272A` (zinc-800, 微妙分割)
- **Border strong:** `#3F3F46` (zinc-700, 强调分割)
- **Text primary:** `#FAFAFA` (zinc-50)
- **Text secondary:** `#A1A1AA` (zinc-400)
- **Text muted:** `#71717A` (zinc-500)
- **Text faint:** `#52525B` (zinc-600)
- **Accent:** `#0EA5E9` (sky-500, refined teal — 比 cyberpunk cyan 更精致，比竞品 blue 更有辨识度)
- **Accent hover:** `#38BDF8` (sky-400)
- **Accent dim:** `rgba(14, 165, 233, 0.12)` (accent 背景色)
- **Accent glow:** `rgba(14, 165, 233, 0.06)` (产品截图周围微弱光晕)
- **Semantic:**
  - Success: `#4ADE80` (background: `rgba(74, 222, 128, 0.08)`)
  - Warning: `#FBBF24` (background: `rgba(251, 191, 36, 0.08)`)
  - Error: `#F87171` (background: `rgba(248, 113, 113, 0.08)`)
  - Info: `#0EA5E9` (background: `rgba(14, 165, 233, 0.12)`)
- **Selection:** `rgba(14, 165, 233, 0.3)`
- **Dark mode:** 这就是默认且唯一模式。目标用户是开发者。

## Spacing
- **Base unit:** 8px
- **Density:** Comfortable — Hero 区域极大留白让标题呼吸，内容区舒适但不松散，代码/产品展示区略紧凑暗示功能密度
- **Scale:**
  - 2xs: 2px
  - xs: 4px
  - sm: 8px
  - md: 16px
  - lg: 24px
  - xl: 32px
  - 2xl: 48px
  - 3xl: 64px
  - 4xl: 80px (section padding)

## Layout
- **Approach:** Hybrid — Hero 居中大字 + 产品截图，下方内容用 2x2 网格或交替叙事块。不用 3-column icon grid。
- **Grid:** 主要内容 max-width 内居中，局部使用 2-column grid
- **Max content width:** 1200px
- **Hero max width:** 720px (标题和描述文字)
- **Border radius:**
  - sm: 4px (badge, 小元素)
  - md: 8px (input, button, card)
  - lg: 12px (dialog, panel, 产品截图 mock)
  - full: 9999px (avatar, pill badge, theme toggle)

## Motion
- **Approach:** Minimal-functional — 未来感来自静止和精确，不是运动
- **Easing:** enter(ease-out) exit(ease-in) move(ease-in-out)
- **Duration:**
  - micro: 75ms (hover state)
  - short: 150ms (button, border-color transition)
  - medium: 250ms (focus state)
- **Rules:**
  - Hover: background-color / border-color 75-150ms
  - 滚动时内容微妙淡入（可选，非必须）
  - 不做粒子效果、滚动动画、打字机效果
  - Badge 中的 dot 用 2s infinite pulse 暗示"活的"

## Copy Direction
- **Tone:** 从未来发回的报道，不是推销。自信、克制、matter-of-fact。
- **Hero headline:** "Your AI agents need a shared workspace."
- **Hero subtitle:** "Everything runs on your machine. Your agents communicate through plain files. No servers, no APIs, no cloud dependency. You own every byte."
- **Primary CTA:** "Request Early Access" (暗示需要资格)
- **Secondary CTA:** "See How It Works"
- **Access form title:** "Request Early Access"
- **Value props (不是 feature list，是 outcome):**
  - "Your machine, your data" — 强调 local-first 和数据主权
  - "Agents as teammates" — Claude, Devin, 自定义 agent 都能加入
  - "Plain files, real tools" — cat, grep, git，没有专有格式
  - "Zero infrastructure" — 没有 Docker，没有数据库，一个包搞定
- **Anti-patterns:** 不说"revolutionary"、"cutting-edge"、"powered by AI"。不用感叹号。不用 emoji。

## Brand Bridge
gitim.io (主页) 和 app.gitim.io (产品) 共享：
- Body font: Plus Jakarta Sans
- Mono font: JetBrains Mono
- Neutral 色系: zinc scale
- Border radius scale: 一致
- 色彩过渡: 主页用 teal accent (#0EA5E9)，产品用 soft blue (#60a5fa)，两个色调足够接近让人感觉是一个品牌

## Tech Stack
- **Framework:** Vite 8 + React 19 + TypeScript 6
- **CSS:** Tailwind CSS 4 (via @tailwindcss/vite plugin, @theme syntax for tokens)
- **UI Components:** shadcn/ui pattern (Radix UI + CVA + cn() helper)
- **Icons:** lucide-react
- **Fonts:** Google Fonts CDN (preconnect + display=swap)
- **Build:** Static output (dist/), no SSR

## Decisions Log
| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-04-14 | Quiet Futurism 方向 | 产品已闭源，需要产品页而非协议推广页。目标情绪"未来感"通过克制和精确实现，不是视觉特效 |
| 2026-04-14 | Space Grotesk + Plus Jakarta Sans | Space Grotesk 的几何字形创造辨识度，Plus Jakarta Sans 保持与产品端一致 |
| 2026-04-14 | Teal accent #0EA5E9 | 保留原 cyan 品牌基因但更精致。与竞品的蓝/紫区分。与产品端 soft blue 足够接近形成品牌连续性 |
| 2026-04-14 | zinc neutral scale | 近黑微暖背景，降低纯黑对比度，长时间阅读不疲劳 |
| 2026-04-14 | 产品优先定位 | 主页核心任务是展示产品价值和驱动"申请内测"，不是推广开源协议 |
| 2026-04-14 | 不提供 light mode | 目标用户是开发者，dark mode 是唯一模式 |
| 2026-04-14 | Vite + React + shadcn/ui | 与 `products/cell/frontend` 同源技术栈，共享组件模式和依赖 |
| 2026-04-14 | products/site/frontend 目录结构 | monorepo 按产品线分，支持未来多产品扩展和 packages/ 公用库提取 |
