# Design System — GitIM

## Product Context
- **What this is:** AI Agent 团队协作平台，Git 作为消息总线
- **Who it's for:** 开发者和 AI agent 运维人员
- **Space/industry:** 开发者工具 / Agent 编排
- **Project type:** Web app (管理层 + 聊天层)

## Aesthetic Direction
- **Direction:** Clean/Friendly — 温暖的深色主题，简洁不冷淡
- **Decoration level:** Minimal — 排版和留白做所有工作
- **Mood:** 安静、专注、长时间使用不疲劳。像一个精心设计的阅读环境，不是闪烁的控制台
- **Key principle:** 柔和 > 锐利。圆角 > 直角。舒适间距 > 极致密度

## Typography
- **Display/Hero:** Plus Jakarta Sans 700 — 温暖、微圆、现代，比 Inter/Roboto 多一份亲和力
- **Body:** Plus Jakarta Sans 400/500 — 同家族，各字号可读性优秀，line-height 1.5-1.6
- **UI/Labels:** Plus Jakarta Sans 500 — 略加粗用于导航和标签
- **Data/Tables:** JetBrains Mono 400 — Agent ID、路径、session 等技术值用 mono
- **Code:** JetBrains Mono 400
- **Loading:** Google Fonts CDN
  ```html
  <link href="https://fonts.googleapis.com/css2?family=Plus+Jakarta+Sans:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500&display=swap" rel="stylesheet">
  ```
- **Scale:**
  - xs: 11px / 0.6875rem
  - sm: 13px / 0.8125rem
  - base: 15px / 0.9375rem
  - lg: 18px / 1.125rem
  - xl: 22px / 1.375rem
  - 2xl: 28px / 1.75rem
  - 3xl: 36px / 2.25rem

## Color
- **Approach:** Restrained — 以中性色为主，accent 用于焦点和交互
- **Background:** #1c1c1e (温暖深灰，非纯黑)
- **Surface:** #232326 (卡片、侧边栏、输入框)
- **Surface hover:** #2a2a2e
- **Border:** #2c2c30 (微妙分割)
- **Border strong:** #3c3c40
- **Text primary:** #e4e4e7
- **Text secondary:** #a1a1aa
- **Text muted:** #71717a
- **Text faint:** #52525b
- **Accent:** #60a5fa (soft blue — 柔和、不刺眼、长时间看不疲劳)
- **Accent hover:** #3b82f6
- **Accent muted:** #60a5fa18 (用于 active 状态背景)
- **Semantic:**
  - Success: #4ade80 (background: #4ade8018)
  - Warning: #fbbf24 (background: #fbbf2418)
  - Error: #f87171 (background: #f8717118)
  - Info: #60a5fa (background: #60a5fa18)
- **Dark mode:** 这就是默认模式。不提供 light mode（目标用户是开发者）

## Spacing
- **Base unit:** 4px
- **Density:** Comfortable — 不像 Slack 那么松散，也不像 terminal 那么紧凑。介于两者之间
- **Scale:**
  - 2xs: 2px
  - xs: 4px
  - sm: 8px
  - md: 16px
  - lg: 24px
  - xl: 32px
  - 2xl: 48px
  - 3xl: 64px

## Layout
- **Approach:** Grid-disciplined
- **Grid:** 聊天层三栏 (sidebar 240px + main flex-1 + thread 320px conditional)
- **Max content width:** 1440px (管理层卡片网格)
- **Border radius:**
  - sm: 4px (badge, 小元素)
  - md: 8px (input, button, card)
  - lg: 12px (dialog, panel, app shell 外边框)
  - full: 9999px (avatar, pill badge)

## Motion
- **Approach:** Minimal-functional — 只做辅助理解的过渡，零装饰性动画
- **Easing:** enter(ease-out) exit(ease-in) move(ease-in-out)
- **Duration:**
  - micro: 75ms (hover state, badge)
  - short: 150ms (button, tab switch)
  - medium: 250ms (panel open/close, page transition)
- **Rules:**
  - Tab 切换: 无动画，instant
  - Thread panel: width transition 250ms ease-in-out
  - Hover: background-color 75ms
  - Message highlight: background fade 1500ms

## Decisions Log
| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-04-10 | Direction D: Clean/Friendly | 长时间使用舒适度优先，柔和 > 锐利 |
| 2026-04-10 | Plus Jakarta Sans | 温暖圆润，比 Geist/Inter 更亲和 |
| 2026-04-10 | Soft blue accent #60a5fa | 不刺眼，区别于 Slack 紫/Discord blurple |
| 2026-04-10 | 非纯黑背景 #1c1c1e | 降低对比度，减少视觉疲劳 |
| 2026-04-10 | Comfortable spacing | 平衡信息密度和阅读舒适度 |
