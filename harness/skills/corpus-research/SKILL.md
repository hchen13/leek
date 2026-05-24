---
name: corpus-research
description: 用 corpus_search + corpus_read 做投研主题深度研究的方法。当问题落在已有 knowledge base（行业、公司、品类、宏观主题）且需要 ≥ 2 篇 doc 才能讲清楚时使用。
allowed-tools: [corpus_search, corpus_read, web_search, web_fetch]
---

# corpus-research — 投研主题深度研究

适用：用户问的话题在 corpus 已有覆盖（你已在 corpus orientation 里看到的轴/主题），且光靠一篇 doc 答不全。

## 步骤

1. **从查询开始扩散**
   - 第一发 `corpus_search`：用用户原话当 query。
   - 看 hits（每条带 path + 短 snippet）。**先扫一遍 paths 的目录结构**——同一个目录下的 doc 往往配套，可以一起打开。
   - 命中少 / 不相关 → 改 query 重试。用同义词（中文 / 英文都试）、不同抽象层（"半导体" → "晶圆代工" / "封测"）、明确技术词。

2. **打开关键 doc**
   - 用 `corpus_read` 一次开 1-3 篇,**不要一次性 batch read 全部 hits**——大文件吃 context。
   - 每读完一篇做 30 字心里 summary,记下 path + 关键论点。

3. **交叉验证**
   - corpus 是用户自己整理的 knowledge,**写法可能带主观偏好**。重要事实(数字 / 公司动作 / 时间)**对照 web_search 求证**(对齐求证纪律)。
   - 矛盾 → 显式 surface,不要默选 corpus 那一边。

4. **回答时引用**
   - 任何来自 corpus 的论点都带 path,格式:`(来自 corpus: <path>)`。
   - 用户能凭 path 自己回去看,这是 corpus 的契约。

## 反模式

- 一次 read 10 篇 → context 爆炸,模型记不住。
- 只信 corpus,不 web_search 求证 → 用户写错了你也写错。
- 拿一篇 doc 当全部 → 一篇不够还说"corpus 里就这么写的"。
