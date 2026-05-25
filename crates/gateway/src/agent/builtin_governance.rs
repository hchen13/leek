//! M3.1 — codex builtin web_search 治理.
//!
//! 背景: leek 的 5 个 guards (idle / wall / iter / doom / cost) 全部对 **codex
//! 服务端 builtin tools** (web_search 内部的 search / open_page / find_in_page)
//! 失效. codex Responses API 是 request → response 单向流, 没有反向通道在 iter
//! 内告诉模型 "stop". M3 deep-review 实测踩雷: 同一 PDF 被 codex 内部 open_page
//! × 9 次, 另一 PDF × 5 次, user 等 12 分钟无反馈无干预手段.
//!
//! 这个模块在 iter 边界做兜底:
//!
//! - **L2 abort**: 同一 `(action_type, url)` 调用次数跨过 abort 阈值, loop 主动
//!   break 当前 iter, `stop_reason = "codex_duplicate_abort"`.
//! - **L3 next-iter inject**: 同一 `(action_type, url)` 调用次数跨过 warn 阈值,
//!   loop 在下一 iter 的 input 拼一条中文 hint, 告诉模型 "你已 open 过这些 URL N
//!   次, 别再 open 了, 直接得结论". 不阻断模型决策, 只是 nudge.
//! - **Warning event**: 每次跨过 warn 阈值 emit 一个 `codex_duplicate_url_warning`
//!   到 canvas surface, frontend 实时展示给 user.
//!
//! 配置: warn 阈值默认 3, abort 阈值默认 7. 走 M2.6 layering (env > config >
//! default). env vars: `LEEK_BUILTIN_URL_WARN_THRESHOLD` /
//! `LEEK_BUILTIN_URL_ABORT_THRESHOLD`. 阈值 0 = 关闭对应 layer.
//!
//! 范围: 只监听 codex builtin web_search 的 `search_lifecycle` event. **不**
//! 监听 leek-side `web_fetch` (doom_loop 已管) , **不**做 leek-side web_search
//! wrapper (单独 milestone).

use std::collections::{HashMap, HashSet};

/// Per-turn tracker for codex builtin web_search 重复 URL.
///
/// 每 turn 创建一个, lifetime 跟着 turn 走. observe() 每个
/// `search_lifecycle` completion event 调一次; drain_for_inject() 在 iter
/// 结束、下个 iter 拼 input 之前调一次.
#[derive(Debug)]
pub struct BuiltinTracker {
    /// `(action_type, url)` → 累计计数.
    url_counts: HashMap<(String, String), u32>,
    /// 已 emit 过 Warn 信号的 key (clear 在每次 drain_for_inject 之后, 这样下
    /// 个 iter 若同 URL 再触发还能再次 inject).
    warned: HashSet<(String, String)>,
    /// 已 abort 过的标记, 防多次触发(同 turn 内 abort 只发生一次).
    aborted: bool,
    /// 跨过该阈值 → emit Warn (≥ 阈值时触发, 仅一次/per drain cycle).
    /// `0` = 关闭 warn.
    warn_threshold: u32,
    /// 跨过该阈值 → emit Abort. `0` = 关闭 abort.
    abort_threshold: u32,
}

/// Tracker observe() 的返回值: 告诉 caller 要 emit 什么动作.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackerSignal {
    /// 某 URL 跨过 warn 阈值. caller 应 emit `codex_duplicate_url_warning`
    /// event 到 canvas surface, 但 iter 继续跑.
    Warn {
        action_type: String,
        url: String,
        count: u32,
    },
    /// 某 URL 跨过 abort 阈值. caller 应 break stream, stop_reason 设为
    /// `codex_duplicate_abort`. abort 优先级高于 warn (一个 observe 只返一个
    /// signal, abort 覆盖 warn).
    Abort {
        action_type: String,
        url: String,
        count: u32,
    },
}

impl BuiltinTracker {
    /// `warn` / `abort` 是 absolute counts (≥ 阈值即触发). `0` 关闭对应判定.
    pub fn new(warn: u32, abort: u32) -> Self {
        Self {
            url_counts: HashMap::new(),
            warned: HashSet::new(),
            aborted: false,
            warn_threshold: warn,
            abort_threshold: abort,
        }
    }

    /// 喂一个 `search_lifecycle` completion event (codex builtin web_search 的
    /// `open_page` / `find_in_page` / `search`). 空 url 视为无效, return None.
    ///
    /// 返回值:
    /// - `None` — 没跨阈值, 或 abort 已 fire 过 (静默).
    /// - `Some(Warn)` — 这个 URL 第一次跨 warn 阈值. 后续同 URL 不再 Warn
    ///   直到 drain_for_inject() 清空 warned 集合.
    /// - `Some(Abort)` — 跨 abort 阈值, 仅返一次(`aborted` 标记后续静默).
    pub fn observe(&mut self, action_type: &str, url: &str) -> Option<TrackerSignal> {
        if url.is_empty() {
            return None;
        }
        if self.aborted {
            // 已经 abort 过, 后续 event 静默 — caller 已经在收尾.
            return None;
        }
        let key = (action_type.to_string(), url.to_string());
        let entry = self.url_counts.entry(key.clone()).or_insert(0);
        *entry = entry.saturating_add(1);
        let count = *entry;

        // Abort 优先: 同一 observe 只返一个 signal, abort 覆盖 warn.
        if self.abort_threshold > 0 && count >= self.abort_threshold {
            self.aborted = true;
            return Some(TrackerSignal::Abort {
                action_type: key.0,
                url: key.1,
                count,
            });
        }
        if self.warn_threshold > 0 && count >= self.warn_threshold && !self.warned.contains(&key) {
            self.warned.insert(key.clone());
            return Some(TrackerSignal::Warn {
                action_type: key.0,
                url: key.1,
                count,
            });
        }
        None
    }

    /// 在拼下个 iter 的 input 之前调一次. 返回当 iter 内 warn 过的 URL 清单,
    /// 渲染成中文 hint 字符串供 inject. 没东西要 inject 返 `None`.
    ///
    /// 调完会 clear `warned` 集合 (`url_counts` 保留, 因为累计计数跨 iter
    /// 仍然有效). 这样下个 iter 若同 URL 再次 open, warn 会再次 fire + 再次
    /// inject, 形成持续 nudge.
    pub fn drain_for_inject(&mut self) -> Option<String> {
        if self.warned.is_empty() {
            return None;
        }
        // 按 count 降序排列, 让消息里最严重的 URL 排在前面.
        let mut items: Vec<((String, String), u32)> = self
            .warned
            .iter()
            .filter_map(|k| self.url_counts.get(k).map(|c| (k.clone(), *c)))
            .collect();
        items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        let mut msg = String::from(
            "⚠ 在前面的 iteration 中, 你重复调用了 codex 内置 web_search 的同一资源 (action_type + URL):\n",
        );
        for ((action_type, url), count) in &items {
            msg.push_str(&format!("- [{action_type}] {url} ({count} 次)\n"));
        }
        msg.push_str(
            "\n这通常意味着你已经从这些资源获取了足够信息 (或它们没有你需要的内容). 请基于已有信息得出结论, 不要再 search 或 open 同一 URL, 进入综合 + 输出阶段.",
        );

        self.warned.clear();
        Some(msg)
    }

    /// 是否已 abort 过. 仅在测试用 — production 流程通过 `observe()` 返回的
    /// `Abort` signal 一次性触发 break, 无需轮询.
    #[cfg(test)]
    pub fn aborted(&self) -> bool {
        self.aborted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_url_is_ignored() {
        let mut t = BuiltinTracker::new(2, 5);
        assert!(t.observe("open_page", "").is_none());
        assert!(t.observe("search", "").is_none());
    }

    #[test]
    fn single_url_under_threshold_no_signal() {
        let mut t = BuiltinTracker::new(3, 7);
        assert_eq!(t.observe("open_page", "https://a.com/x"), None);
        assert_eq!(t.observe("open_page", "https://a.com/x"), None);
        // count == 2, threshold == 3 — still under.
    }

    #[test]
    fn single_url_at_warn_threshold_emits_warn() {
        let mut t = BuiltinTracker::new(3, 7);
        assert_eq!(t.observe("open_page", "https://a.com/x"), None);
        assert_eq!(t.observe("open_page", "https://a.com/x"), None);
        let sig = t.observe("open_page", "https://a.com/x").unwrap();
        match sig {
            TrackerSignal::Warn { action_type, url, count } => {
                assert_eq!(action_type, "open_page");
                assert_eq!(url, "https://a.com/x");
                assert_eq!(count, 3);
            }
            other => panic!("expected Warn, got {other:?}"),
        }
    }

    #[test]
    fn single_url_at_warn_threshold_only_once_until_drain() {
        // warn 防重复: 同一 URL 跨阈值后, 后续同 URL observe 不再 Warn,
        // 直到 drain_for_inject() clear warned 集合.
        let mut t = BuiltinTracker::new(2, 10);
        assert!(t.observe("open_page", "https://a.com/x").is_none());
        assert!(matches!(
            t.observe("open_page", "https://a.com/x"),
            Some(TrackerSignal::Warn { .. })
        ));
        // 第 3 次同 URL — warned 已包含 key, 静默.
        assert_eq!(t.observe("open_page", "https://a.com/x"), None);
        assert_eq!(t.observe("open_page", "https://a.com/x"), None);

        // drain 后 warned clear, 同 URL 再次 observe 应该再次 Warn.
        let inject = t.drain_for_inject();
        assert!(inject.is_some());
        let sig = t.observe("open_page", "https://a.com/x");
        // count 已经 5, ≥ warn 阈值 → Warn 又触发.
        assert!(matches!(sig, Some(TrackerSignal::Warn { count: 5, .. })));
    }

    #[test]
    fn single_url_at_abort_threshold_emits_abort() {
        let mut t = BuiltinTracker::new(2, 3);
        assert!(t.observe("open_page", "https://a.com/x").is_none());
        // count == 2 → Warn (而非 Abort, 因为 abort 阈值 3 还没到).
        assert!(matches!(
            t.observe("open_page", "https://a.com/x"),
            Some(TrackerSignal::Warn { .. })
        ));
        // count == 3 → Abort.
        let sig = t.observe("open_page", "https://a.com/x").unwrap();
        match sig {
            TrackerSignal::Abort { action_type, url, count } => {
                assert_eq!(action_type, "open_page");
                assert_eq!(url, "https://a.com/x");
                assert_eq!(count, 3);
            }
            other => panic!("expected Abort, got {other:?}"),
        }
        assert!(t.aborted());
        // 之后 observe 全部静默.
        assert_eq!(t.observe("open_page", "https://a.com/x"), None);
        assert_eq!(t.observe("open_page", "https://b.com/y"), None);
    }

    #[test]
    fn abort_disabled_when_threshold_zero() {
        // 0 abort 阈值 = 关闭 abort, 只 warn 不 abort.
        let mut t = BuiltinTracker::new(2, 0);
        for _ in 0..20 {
            let _ = t.observe("open_page", "https://a.com/x");
        }
        assert!(!t.aborted());
    }

    #[test]
    fn warn_disabled_when_threshold_zero() {
        // 0 warn 阈值 = 关闭 warn. abort 仍可单独生效.
        let mut t = BuiltinTracker::new(0, 3);
        assert_eq!(t.observe("open_page", "https://a.com/x"), None);
        assert_eq!(t.observe("open_page", "https://a.com/x"), None);
        let sig = t.observe("open_page", "https://a.com/x");
        assert!(matches!(sig, Some(TrackerSignal::Abort { .. })));
    }

    #[test]
    fn multiple_urls_independent_counters() {
        let mut t = BuiltinTracker::new(2, 5);
        assert!(t.observe("open_page", "https://a.com/x").is_none());
        assert!(t.observe("open_page", "https://b.com/y").is_none());
        // 两个 URL 各 1 次 — 都没触发.
        assert!(matches!(
            t.observe("open_page", "https://a.com/x"),
            Some(TrackerSignal::Warn { count: 2, .. })
        ));
        // a 触发了 warn; b 继续独立 count.
        assert!(t.observe("open_page", "https://b.com/y").is_some());
    }

    #[test]
    fn different_action_types_are_separate() {
        // (open_page, X) 和 (search, X) 是不同 key, 计数独立.
        let mut t = BuiltinTracker::new(2, 5);
        let _ = t.observe("open_page", "https://a.com/x");
        let _ = t.observe("search", "https://a.com/x");
        // open_page 第 2 次 → warn.
        assert!(matches!(
            t.observe("open_page", "https://a.com/x"),
            Some(TrackerSignal::Warn { action_type, .. }) if action_type == "open_page"
        ));
        // search 第 2 次 → 也 warn (独立计数).
        assert!(matches!(
            t.observe("search", "https://a.com/x"),
            Some(TrackerSignal::Warn { action_type, .. }) if action_type == "search"
        ));
    }

    #[test]
    fn drain_for_inject_returns_none_when_no_warns() {
        let mut t = BuiltinTracker::new(3, 7);
        assert!(t.drain_for_inject().is_none());
        // 1 次调用, 未跨阈值.
        let _ = t.observe("open_page", "https://a.com/x");
        assert!(t.drain_for_inject().is_none());
    }

    #[test]
    fn drain_for_inject_renders_warned_urls_with_counts() {
        let mut t = BuiltinTracker::new(2, 10);
        // 3 个 URL, 都 trigger warn 一次 (count == 2 / 2 / 3).
        let _ = t.observe("open_page", "https://a.com/x");
        let _ = t.observe("open_page", "https://a.com/x");
        let _ = t.observe("open_page", "https://b.com/y");
        let _ = t.observe("open_page", "https://b.com/y");
        let _ = t.observe("open_page", "https://c.com/z");
        let _ = t.observe("open_page", "https://c.com/z");
        let _ = t.observe("open_page", "https://c.com/z");
        let msg = t.drain_for_inject().unwrap();
        assert!(msg.contains("https://a.com/x"));
        assert!(msg.contains("https://b.com/y"));
        assert!(msg.contains("https://c.com/z"));
        assert!(msg.contains("[open_page]"));
        // count 显示在 URL 后.
        assert!(msg.contains("(3 次)"));
        assert!(msg.contains("(2 次)"));
        // 最严重的排前面 (c.com 是 3 次).
        let c_pos = msg.find("https://c.com/z").unwrap();
        let a_pos = msg.find("https://a.com/x").unwrap();
        assert!(c_pos < a_pos, "highest count should sort first");
    }

    #[test]
    fn drain_for_inject_clears_warned_so_next_drain_is_none() {
        let mut t = BuiltinTracker::new(2, 10);
        let _ = t.observe("open_page", "https://a.com/x");
        let _ = t.observe("open_page", "https://a.com/x");
        assert!(t.drain_for_inject().is_some());
        // 第二次 drain — warned 已 clear, 没有新 Warn 触发, 返 None.
        assert!(t.drain_for_inject().is_none());
    }

    #[test]
    fn drain_preserves_url_counts_for_re_trigger() {
        // drain 不 clear url_counts: 下个 iter 若同 URL 再 open, count 累加,
        // 仍 ≥ warn 阈值, 应该再次 Warn (再次 inject).
        let mut t = BuiltinTracker::new(2, 10);
        let _ = t.observe("open_page", "https://a.com/x");
        let _ = t.observe("open_page", "https://a.com/x");
        let _ = t.drain_for_inject(); // 清 warned, 保 counts.
        // 下个 iter 同 URL 再 open 一次 — count → 3, 应该 Warn.
        let sig = t.observe("open_page", "https://a.com/x");
        assert!(matches!(sig, Some(TrackerSignal::Warn { count: 3, .. })));
    }
}
