# L.E.E.K Frontend Prototype（来自 claude.ai/design）

这是 claude design 在 2026-05-01 产出的 HTML/CSS/JS 高保真静态稿。**不是 production code**，是视觉与交互的设计 fixture。

## 如何查看

打开 `L.E.E.K.html`：

```
cd design/frontend/prototype
open L.E.E.K.html
```

文件用 `<script type="text/babel">` + `@babel/standalone` 在浏览器内编译 JSX，无需任何 build step。

## 包含什么

`L.E.E.K.html` 用 `DesignCanvas` 把 5 个 artboard 排在一起，展示 workbench 的 5 种状态：

| Scene | 内容 |
|--|--|
| A · Idle | 空 canvas，CorpusBrain ambient |
| B · Thinking-shallow | 流式输出 + 3 个 panel 浮现 |
| C · Clarify | agent 反问 horizon / risk |
| D · Deep | 6 panel 装配 + brain 跨层激活 |
| E · Delivered | 决策草稿 + 用户后续追问 |

## 文件清单

| 文件 | 内容 |
|--|--|
| `L.E.E.K.html` | 入口 + DesignCanvas wrapper |
| `styles.css` | Claude-warm dark palette + 全部样式 |
| `corpus-brain.js` | 2D force-directed graph（vanilla canvas）+ 60 个真实 corpus path 节点 + 3 强度激活动效 |
| `design-canvas.jsx` | 设计画布 wrapper（用于陈列多 artboard） |
| `leek-icons.jsx` | 内嵌 SVG 图标组件 |
| `leek-panels.jsx` | Panel + 各模块 renderer（quote / candles / pdf / cmp / cites / valuation / ...） |
| `leek-chat.jsx` | 聊天列（StreamText / NodeRefPill / Composer / 5 个 Transcript） |
| `leek-workbench.jsx` | TopBar / Rail / BrainWidget / CanvasArea / Workbench shell + 5 个 scene 的 canvas layout |

## 与生产实现的关系

- 生产实现位置：`frontend/web/`（SolidJS + Vite）
- 这份 prototype 是**视觉 / 交互参照标准**——port 时按此 1:1 还原，不需要改设计判断
- 用户已声明设计"有不少瑕疵"，最终视觉以生产实现的迭代为准；本 prototype 不再维护

## 设计调性

- **Claude-warm dark palette**：暖 umber 底 + 黏土 / 琥珀色作 accent，不是冷金融蓝
- **CorpusBrain**：Obsidian 风格 force-directed，4 cluster（principles/wikis、principles/sources、knowledge/wikis、knowledge/sources），节点大小按 degree，轻微 idle 漂移
- **Reasoning DAG**：暂未做完整 DAG（prototype 用 panel 直接展示），生产实现按 `frontend/panels.md` §4 落地
- **Composable panel**：agent 通过 `kind + modules[]` 在 runtime 组装 panel
