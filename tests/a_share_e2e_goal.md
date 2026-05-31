# A Share E2E Goal

目标：清空测试 session 后，按 `tests/a_share_e2e_cases.md` 重跑全部 A 股 E2E case，记录所有 harness / frontend / agent-quality 问题，集中修复可编码问题，并循环验证，直到所有 harness 和前端问题都解决，且剩余 agent 表现问题有清楚归因。

## 执行原则

- 全程使用前端界面操作：创建新 session、重命名 session、发送 prompt、多轮追问、观察 canvas / artifact / retry / trace / streaming / session list。
- 每个 case 由独立 subagent 串行执行，保持上下文隔离。
- 每个执行测试的 subagent 必须先加载 Harness Engineering skill：`/Users/ethan/.codex/skills/harness-engineering/SKILL.md`。
- 主 session 负责 PM / integrator：分配 case、收集问题、分类、集中修复、组织回归。
- 不把一次失败立刻改成代码。先完成一轮 case 执行，形成问题清单，再集中判断哪些是编码问题。
- 不要求 agent 在最终答复中向用户说明 corpus 覆盖情况。用户只需要看到可用的研究框架、事实、判断和边界。

## 测试前准备

1. 确认当前工作树状态，避免覆盖用户未提交改动。
2. 启动 gateway 和 web 前端，确认浏览器可访问。
3. 清空测试 session 记录。
   - 只清理本轮测试用 session。
   - 不删除 vault 文件，不做递归删除，不使用危险通配符。
   - 若已有安全 API / UI / 脚本，优先使用项目内现成入口。
   - 若没有明确安全入口，先定位测试 session 列表和存储方式，再做最小范围清理。
4. 打开前端，从空 session 列表开始测试。

## Case 设计

前 9 个 case 以 `tests/a_share_e2e_cases.md` 为准。

第 10 个 case 改成长程任务，不提前写死完整 prompt，只定义话题方向和测试目标：

话题方向：围绕一个复杂 A 股长期投资决策做至少 10 轮对话，从研究框架、事实缺口、行业与宏观、公司质量、估值、反方观点、组合约束、风险触发条件、行动方案和复盘机制逐步推进。

建议标的：贵州茅台 `600519.SH`、宁德时代 `300750.SZ`、中国平安 `601318.SH` 三选一，或由测试 driver 根据当时工具表现选择一个更适合长链路验证的标的。

最低要求：

- 至少 10 轮用户- agent 交互。
- driver 不提前写死所有 prompt，而是根据 leek agent 每轮回复动态追问。
- 每轮追问都应围绕上一轮暴露出的事实缺口、逻辑跳跃、工具结果、用户约束或行动条件。
- 必须覆盖：研究框架、关键事实补齐、反方验证、估值或价格位置、风险边界、组合约束、最终动作、后续触发条件。
- 重点观察长链路稳定性：是否重复开同一 PDF / URL，是否重复拉同参数据，是否能复用已有证据，是否出现 provider retry UI 脱节，是否出现 trace / tool artifact 顺序错乱，是否因长回复造成前端刷屏或卡顿。

## 问题分类

每个问题记录为以下三类之一：

1. harness 问题
   - agent loop、context、compaction、tool schema、tool description、tool output、retry/recovery、预算、session state、prompt caching、subagent orchestration 等问题。

2. 前端问题
   - canvas / artifact 显示不合理。
   - markdown、JSON、表格、K 线、资金流、研报、trace 等没有以合适形式渲染。
   - provider retry / recovered / error 状态显示与真实执行状态脱节。
   - session 自动命名、session list 更新时间、streaming 频率、排序、时序、布局、移动端/桌面端显示问题。

3. agent 表现问题
   - 推理框架浅、事实堆砌、追问方向差、没有形成任务专属 working model、误用证据、结论跳跃。
   - 通常不直接当作编码 bug 修复，先分析是否由工具、上下文、提示、检索、UI 反馈或缺少观测面导致。

## 每个 case 的记录格式

每个 subagent 完成 case 后，向主 session 汇报：

- case 编号和 session id。
- 执行轮数。
- 用户 prompt 摘要。
- agent 最终结果质量评估。
- 实际调用的主要工具和明显失败工具。
- 是否出现 provider error / retry / recovered。
- 是否出现 frontend artifact / canvas / streaming / session naming 问题。
- 问题列表，按 harness / 前端 / agent 表现分类。
- 可复现步骤。
- 证据位置：session id、事件、tool run、截图路径或 transcript 摘要。

## 修复循环

每一轮按这个顺序执行：

1. 跑完全部 10 个 case。
2. 汇总问题清单，去重，按严重度排序。
3. 区分编码问题和 agent 表现问题。
4. 对 harness 和前端问题集中修复。
5. 为修复点补最小必要测试。
6. 跑后端单测和前端构建。
7. 清空测试 session 后重跑相关 case。
8. 若仍有 harness / 前端问题，继续下一轮。

## 停止条件

只有同时满足以下条件才停止：

- 所有已发现 harness 问题已修复或有明确非编码原因。
- 所有已发现前端问题已修复。
- provider retry / recovered / error 在 UI 上不会长期残留或误导用户。
- reasoning trace、tool artifact、session 自动命名、长回复 streaming 在长程 case 中表现稳定。
- 第 10 个至少 10 轮长程任务完成，并且没有暴露新的 blocker。
- 剩余 agent 表现问题已经完成归因，且没有明显可通过简单编码修复的问题。

## 最终交付

最终汇报需包含：

- 跑过的 case 和 session id 列表。
- 每轮发现的问题和修复摘要。
- 最终剩余问题及归因。
- 已运行的验证命令。
- 如果有未解决问题，说明为什么不属于本轮编码修复范围。
