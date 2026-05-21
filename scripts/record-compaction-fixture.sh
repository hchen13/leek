#!/usr/bin/env bash
# M2.5 step 1 (v2) — record a real multi-turn long session as a permanent
# compaction regression fixture (docs/dispatches/M2.5-compaction-fixture.md).
#
# Drives a running leek backend through the v2 调淡 30-prompt sequence on
# Buffett value-investing topics that the corpus covers, watches per-turn
# `turn_metrics_recorded` events, and stops when:
#   - total turns ≥ MAX_TURNS (default 30), or
#   - cumulative cost ≥ COST_CAP_USD (default 200), or
#   - the prompt list runs out, or
#   - codex returns context_too_large (recorded as a useful boundary), or
#   - a turn times out > TURN_TIMEOUT_SEC (default 600), or
#   - a POST /messages 5xx fails twice in a row.
#
# v1 had an `INPUT_TOKEN_STOP` knob — REMOVED in v2 because the per-turn
# `input_tokens` metric is an iteration-sum, not a context-window proxy
# (see compaction-single-turn-burst/README.md for the morphology that
# mistake produced). v2 lets the prompt list and cost cap define the run.
#
# Exports to OUTPUT_DIR:
#   metadata.json, session.json, events.json,
#   transcripts/turn-<N>-iter-<M>-request.json,
#   transcripts/turn-<N>-iter-<M>-response.txt
#
# Requires:  curl, jq.  Assumes leek backend on http://127.0.0.1:8964.
# This script is one-shot. It MUST NOT be re-run mid-recording — the
# leek backend session is the source of truth.

set -euo pipefail

API="${LEEK_API:-http://127.0.0.1:8964}"
MAX_TURNS="${MAX_TURNS:-30}"
COST_CAP_USD="${COST_CAP_USD:-200}"
TURN_TIMEOUT_SEC="${TURN_TIMEOUT_SEC:-600}"
POLL_SEC="${POLL_SEC:-3}"
OUTPUT_DIR="${OUTPUT_DIR:-crates/gateway/tests/fixtures/compaction-multi-turn-accumulation}"

# Resume support — when set, the script reuses an existing session and
# skips the first N prompts (which have already been posted to the
# backend in a prior run). The script still polls the backend for those
# N turns' metrics (read from the persisted event log) so the in-memory
# `turn_ids[]` and `total_cost_bc` line up with prior state, then posts
# from prompt N+1 onward.
RESUME_SESSION_ID="${RESUME_SESSION_ID:-}"
RESUME_FROM_PROMPT="${RESUME_FROM_PROMPT:-1}"

# v2 prompt sequence (spec §3, 30 entries — "调淡版"). Each prompt
# explicitly bounds research depth (length + corpus_read count) so the
# per-turn iter count converges to 5-10. If a turn nonetheless blows past
# 15 iter, the next prompt can be tightened (e.g. add "only corpus, no
# web_search") — but the already-posted prompt is never edited.
PROMPTS=(
  "从 corpus 找 1-2 段关于'安全边际'的经典定义（Buffett 原话），200 字摘录就行。"
  "上面这段话的历史背景，简述 200 字。"
  "Buffett 在 1965-1975 之间对这个概念的态度，corpus 找 2-3 处变化引述。"
  "Munger 在 Poor Charlie's Almanack 对这个概念的看法，引一段原文即可。"
  "一句话对比 Buffett 和 Munger 在这点上的差异。"
  "换个话题：'护城河'在 corpus 里的权威定义，找 1 处，300 字以内。"
  "护城河有哪些类型？简单列 3-5 个，每个一句话定义。"
  "茅台符合哪种护城河？100 字判断。"
  "恒瑞医药呢？100 字判断。"
  "'能力圈'是什么？Buffett 怎么定义？引一句原话。"
  "你自己定义的能力圈在哪？用消费类股票举例，200 字。"
  "'内在价值'怎么估？Buffett 用的公式简述。"
  "DCF 模型和 Buffett 估值法的差异，100 字。"
  "Buffett 1989 致股东信里有句关于'好公司比好价格更重要'的话，找出来。"
  "这句话和早期 Graham 派的'低 P/E'方法冲突在哪？"
  "Munger 怎么影响了 Buffett 从 Graham 派转向 quality compounder？"
  "举一个 Buffett 持有 10 年以上的标的，200 字介绍。"
  "为什么 Buffett 长期持有可口可乐？简述 3 点原因。"
  "Buffett 卖出航空股的逻辑是什么？2020 年那次。"
  "Buffett 对科技股的态度变化：从早期回避到买苹果，关键转折点。"
  "Berkshire 现金储备策略，Buffett 的 own words。"
  "Buffett 对回购的看法，corpus 里有没有他的态度引述。"
  "保险浮存金是什么？Buffett 怎么用它。"
  "GEICO 之于 Berkshire 的战略意义，简述。"
  "Buffett 2008 金融危机时的几个经典决策，各 100 字。"
  "你刚刚说的这些决策，哪一个最能体现'恐惧时贪婪'？"
  "如果让你给 2026 年的散户一句巴菲特最重要的建议，会是什么？"
  "Munger 会怎么补充这句建议？"
  "整理上面所有内容，做一份 500 字的极简学习笔记摘要。"
  "如果这份摘要要送给一个 20 岁的投资新手，你会怎么调整措辞？"
)

log() { printf '%s [rec] %s\n' "$(date '+%Y-%m-%dT%H:%M:%S')" "$*"; }
die() { printf '%s [rec] FATAL %s\n' "$(date '+%Y-%m-%dT%H:%M:%S')" "$*" >&2; exit 1; }

# --- 0. preflight -----------------------------------------------------------
log "checking backend at $API"
curl -sf "$API/api/v1/health" >/dev/null || die "backend not reachable at $API"
log "backend ok"

mkdir -p "$OUTPUT_DIR/transcripts"
TURNS_LOG="$OUTPUT_DIR/.turns.jsonl"
: > "$TURNS_LOG"

# --- 1. session create or resume --------------------------------------------
DAY="$(date '+%Y-%m-%d')"
TITLE="M2.5 compaction fixture v2 $DAY"
if [[ -n "$RESUME_SESSION_ID" ]]; then
  SESSION_ID="$RESUME_SESSION_ID"
  log "resuming session $SESSION_ID from prompt $RESUME_FROM_PROMPT"
  # Look the row up to confirm it exists + pull created_at.
  SESSION_ROW="$(curl -sf "$API/api/v1/sessions" | jq --arg id "$SESSION_ID" '.items[] | select(.id == $id)')"
  [[ -n "$SESSION_ROW" ]] || die "resume session $SESSION_ID not found in backend"
  SESSION_CREATED_AT="$(echo "$SESSION_ROW" | jq -r '.created_at')"
  TITLE="$(echo "$SESSION_ROW" | jq -r '.title // ""')"
  [[ -z "$TITLE" || "$TITLE" == "null" ]] && TITLE="M2.5 compaction fixture v2 $DAY"
else
  SESSION_JSON="$(curl -sf -X POST "$API/api/v1/sessions" \
    -H 'content-type: application/json' \
    --data "$(jq -n --arg t "$TITLE" '{title:$t}')")" || die "session create failed"
  SESSION_ID="$(echo "$SESSION_JSON" | jq -r '.id')"
  [[ -n "$SESSION_ID" && "$SESSION_ID" != "null" ]] || die "session id missing"
  log "session id=$SESSION_ID title=\"$TITLE\""
  SESSION_CREATED_AT="$(echo "$SESSION_JSON" | jq -r '.created_at')"
fi

RECORDED_AT_ISO="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
LEEK_GIT_REV="$(git rev-parse HEAD)"
CORPUS_GIT_REV="$(git rev-parse HEAD)"   # corpus lives in the same repo

# Submit POST /messages with one retry on transport/5xx — spec §7.
post_message_once() {
  local prompt="$1"
  curl -sf -X POST "$API/api/v1/sessions/$SESSION_ID/messages" \
    -H 'content-type: application/json' \
    --data "$(jq -n --arg c "$prompt" '{content:$c}')"
}

# --- 2. drive prompts -------------------------------------------------------
since_seq=0
total_cost_bc="0"            # bc-arithmetic float
turn_index=0
stopped_by="prompts_exhausted"
turn_ids=()
max_iter_seen=0

# Resume — replay prior turns' state from the backend event log so the
# turn_ids[], total_cost_bc, and turn_index counters line up.
if [[ -n "$RESUME_SESSION_ID" ]]; then
  log "  replaying prior turns from backend event log…"
  events_page="$(curl -sf "$API/api/v1/sessions/$SESSION_ID/events?since=0&limit=500")"
  resume_metrics="$(echo "$events_page" | jq -c '[.items[] | select(.kind=="turn_metrics_recorded") | .payload]')"
  prior_n="$(echo "$resume_metrics" | jq 'length')"
  log "    found $prior_n prior turn_metrics_recorded event(s)"
  for i in $(seq 0 $((prior_n - 1))); do
    p="$(echo "$resume_metrics" | jq -c ".[$i]")"
    pid="$(echo "$p" | jq -r '.turn_id')"
    pin="$(echo "$p" | jq -r '.input_tokens')"
    pout="$(echo "$p" | jq -r '.output_tokens')"
    pcu="$(echo "$p" | jq -r '.cost_usd')"
    pcc="$(echo "$p" | jq -r '.compaction_count')"
    pit="$(echo "$p" | jq -r '.iteration_count')"
    ptc="$(echo "$p" | jq -r '.tool_call_count')"
    pwm="$(echo "$p" | jq -r '.wall_clock_ms')"
    psr="$(echo "$p" | jq -r '.stop_reason')"
    turn_ids+=("$pid")
    turn_index=$((turn_index + 1))
    if (( pit > max_iter_seen )); then max_iter_seen=$pit; fi
    total_cost_bc="$(printf '%s + %s\n' "$total_cost_bc" "$pcu" | bc -l)"
    # The Nth user message_created is the Nth turn's prompt preview.
    pmsg_preview="$(echo "$events_page" \
      | jq -r '[.items[] | select(.kind=="message_created" and .payload.role=="user")]
               | .['$i'].payload.content // ""' \
      | head -c 200)"
    jq -n --arg tid "$pid" --argjson idx "$turn_index" \
          --arg preview "$pmsg_preview" \
          --argjson in "$pin" --argjson out "$pout" \
          --argjson tc "$ptc" --argjson it "$pit" \
          --argjson wm "$pwm" --argjson cu "$pcu" \
          --argjson cc "$pcc" --arg sr "$psr" \
       '{turn_id:$tid, turn_index:$idx, user_message_preview:$preview,
         input_tokens:$in, output_tokens:$out, tool_call_count:$tc,
         iteration_count:$it, wall_clock_ms:$wm, cost_usd:$cu,
         compaction_count:$cc, stop_reason:$sr}' >> "$TURNS_LOG"
    log "    replayed turn $turn_index ($pid): in=$pin iters=$pit cost=\$$pcu"
  done
  # Advance event cursor past everything already replayed.
  since_seq="$(echo "$events_page" | jq '[.items[].seq] | max // 0')"
  log "  resume state: turn_index=$turn_index cum_cost=\$$total_cost_bc since_seq=$since_seq"
fi

# Build the slice of prompts to actually post — drop the first (RESUME_FROM_PROMPT - 1)
POST_PROMPTS=()
for ((i = RESUME_FROM_PROMPT - 1; i < ${#PROMPTS[@]}; i++)); do
  POST_PROMPTS+=("${PROMPTS[$i]}")
done

for PROMPT in "${POST_PROMPTS[@]}"; do
  turn_index=$((turn_index + 1))
  if (( turn_index > MAX_TURNS )); then
    stopped_by="max_turns"
    log "turn cap reached (MAX_TURNS=$MAX_TURNS), stopping"
    turn_index=$((turn_index - 1))
    break
  fi

  preview="$(printf '%s' "$PROMPT" | head -c 80)"
  log "── turn ${turn_index} ── posting: ${preview}…"

  post_resp="$(post_message_once "$PROMPT")" || {
    log "    POST 5xx / transport error — retrying once after 5s"
    sleep 5
    post_resp="$(post_message_once "$PROMPT")" || {
      stopped_by="post_failed"
      die "POST message failed twice at turn $turn_index — stopping"
    }
  }
  TURN_ID="$(echo "$post_resp" | jq -r '.turn_id')"
  [[ -n "$TURN_ID" && "$TURN_ID" != "null" ]] || die "turn_id missing at turn $turn_index"
  turn_ids+=("$TURN_ID")
  log "    turn_id=$TURN_ID — waiting for turn_metrics_recorded…"

  # Poll the event log until we see turn_metrics_recorded for this turn_id.
  start_epoch=$(date +%s)
  metrics=""
  timed_out="false"
  context_too_large="false"
  while :; do
    page="$(curl -sf "$API/api/v1/sessions/$SESSION_ID/events?since=$since_seq&limit=500" || true)"
    if [[ -n "$page" ]]; then
      last_seq="$(echo "$page" | jq '[.items[].seq] | max // 0')"
      if [[ "$last_seq" -gt "$since_seq" ]]; then
        since_seq="$last_seq"
      fi
      metrics="$(echo "$page" | jq -c --arg tid "$TURN_ID" \
        '.items[] | select(.kind=="turn_metrics_recorded" and .payload.turn_id==$tid) | .payload' \
        | head -n 1 || true)"
      err="$(echo "$page" | jq -c --arg tid "$TURN_ID" \
        '.items[] | select(.kind=="error" and (.payload.turn_id // "") == $tid) | .payload' \
        | head -n 1 || true)"
      if [[ -n "$metrics" ]]; then break; fi
      if [[ -n "$err" ]]; then
        log "    error event: $err"
        # context_too_large is a valid terminal: the agent loop still emits
        # turn_metrics_recorded with fatal_error set, so keep polling.
        if echo "$err" | grep -qi 'context_too_large'; then
          context_too_large="true"
          log "    context_too_large detected — will end after this turn"
        fi
      fi
    fi
    now=$(date +%s)
    elapsed=$(( now - start_epoch ))
    if (( elapsed > TURN_TIMEOUT_SEC )); then
      log "    TIMEOUT after ${elapsed}s — recording timeout entry"
      timed_out="true"
      break
    fi
    sleep "$POLL_SEC"
  done

  if [[ "$timed_out" == "true" ]]; then
    # Record a timeout entry, then per spec §7 SKIP (continue to next
    # prompt) rather than abort — a single hung turn shouldn't lose the
    # rest of the recording. Cumulative cost is unknown for the lost turn.
    jq -n --arg tid "$TURN_ID" --argjson idx "$turn_index" \
          --arg preview "$(printf '%s' "$PROMPT" | head -c 200)" \
          --arg wall_ms "$(( TURN_TIMEOUT_SEC * 1000 ))" \
       '{turn_id:$tid, turn_index:$idx, user_message_preview:$preview,
         input_tokens:0, output_tokens:0, tool_call_count:0, iteration_count:0,
         wall_clock_ms:($wall_ms|tonumber), cost_usd:0, compaction_count:0,
         timeout:true}' >> "$TURNS_LOG"
    continue
  fi

  input_tokens="$(echo "$metrics" | jq -r '.input_tokens')"
  output_tokens="$(echo "$metrics" | jq -r '.output_tokens')"
  cost_usd="$(echo "$metrics" | jq -r '.cost_usd')"
  compaction_count="$(echo "$metrics" | jq -r '.compaction_count')"
  iteration_count="$(echo "$metrics" | jq -r '.iteration_count')"
  tool_call_count="$(echo "$metrics" | jq -r '.tool_call_count')"
  wall_clock_ms="$(echo "$metrics" | jq -r '.wall_clock_ms')"
  stop_reason="$(echo "$metrics" | jq -r '.stop_reason')"

  total_cost_bc="$(printf '%s + %s\n' "$total_cost_bc" "$cost_usd" | bc -l)"
  if (( iteration_count > max_iter_seen )); then
    max_iter_seen=$iteration_count
  fi

  log "    ✓ in=$input_tokens out=$output_tokens iters=$iteration_count tools=$tool_call_count comp=$compaction_count cost=\$$cost_usd wall=${wall_clock_ms}ms stop=$stop_reason"
  log "    progress: turn $turn_index/$MAX_TURNS · cum_cost=\$$total_cost_bc · max_iter_in_turn=$max_iter_seen"

  jq -n --arg tid "$TURN_ID" --argjson idx "$turn_index" \
        --arg preview "$(printf '%s' "$PROMPT" | head -c 200)" \
        --argjson in "$input_tokens" --argjson out "$output_tokens" \
        --argjson tc "$tool_call_count" --argjson it "$iteration_count" \
        --argjson wm "$wall_clock_ms" --argjson cu "$cost_usd" \
        --argjson cc "$compaction_count" --arg sr "$stop_reason" \
     '{turn_id:$tid, turn_index:$idx, user_message_preview:$preview,
       input_tokens:$in, output_tokens:$out, tool_call_count:$tc,
       iteration_count:$it, wall_clock_ms:$wm, cost_usd:$cu,
       compaction_count:$cc, stop_reason:$sr}' >> "$TURNS_LOG"

  # Threshold checks
  cost_cmp="$(printf '%s >= %s\n' "$total_cost_bc" "$COST_CAP_USD" | bc -l)"
  if [[ "$cost_cmp" == "1" ]]; then
    stopped_by="cost_cap"
    log "cost cap reached (\$$total_cost_bc ≥ \$$COST_CAP_USD), stopping"
    break
  fi
  if [[ "$context_too_large" == "true" ]]; then
    stopped_by="context_too_large"
    log "context_too_large was seen — terminating recording per spec §7"
    break
  fi
done

log "── recording complete: $turn_index turn(s), stopped_by=$stopped_by, total_cost=\$$total_cost_bc, max_iter_in_turn=$max_iter_seen"

# --- 3. export fixture ------------------------------------------------------
log "exporting fixture → $OUTPUT_DIR"

# events.json — full persisted event log
EVENTS_TMP="$(mktemp)"
since=0
echo '[]' > "$EVENTS_TMP"
while :; do
  page="$(curl -sf "$API/api/v1/sessions/$SESSION_ID/events?since=$since&limit=500")"
  count="$(echo "$page" | jq '.items | length')"
  if (( count == 0 )); then break; fi
  tmp2="$(mktemp)"
  jq -s '.[0] + .[1].items' "$EVENTS_TMP" <(echo "$page") > "$tmp2"
  mv "$tmp2" "$EVENTS_TMP"
  since="$(echo "$page" | jq '[.items[].seq] | max')"
done
mv "$EVENTS_TMP" "$OUTPUT_DIR/events.json"
events_count="$(jq 'length' "$OUTPUT_DIR/events.json")"
log "  events.json — $events_count events"

# transcripts/ — per (turn, iteration) raw req + SSE response
TRANS_META="$(curl -sf "$API/api/v1/sessions/$SESSION_ID/transcripts")"
echo "$TRANS_META" | jq -c '.items[] | {turn_id, iteration}' | \
while read -r row; do
  tid="$(echo "$row" | jq -r '.turn_id')"
  iter="$(echo "$row" | jq -r '.iteration')"
  # turn-index from our sequence (1-based for human readability)
  tindex=0
  for i in "${!turn_ids[@]}"; do
    if [[ "${turn_ids[$i]}" == "$tid" ]]; then tindex=$((i + 1)); break; fi
  done
  req_path="$OUTPUT_DIR/transcripts/turn-${tindex}-iter-${iter}-request.json"
  resp_path="$OUTPUT_DIR/transcripts/turn-${tindex}-iter-${iter}-response.txt"
  curl -sf "$API/api/v1/sessions/$SESSION_ID/transcripts/$tid/$iter/request"  > "$req_path"  || log "  warn: missing request for $tid/$iter"
  curl -sf "$API/api/v1/sessions/$SESSION_ID/transcripts/$tid/$iter/response" > "$resp_path" || log "  warn: missing response for $tid/$iter"
done
trans_count="$(echo "$TRANS_META" | jq '.items | length')"
log "  transcripts/ — $trans_count (request, response) pairs"

# session.json — session row + extracted system prompt
SESSION_ROW="$(curl -sf "$API/api/v1/sessions" | jq --arg id "$SESSION_ID" '.items[] | select(.id == $id)')"
# Extract system prompt from turn-1 iter-1 request (the `instructions` field is the system prompt)
SYSTEM_PROMPT=""
first_req="$OUTPUT_DIR/transcripts/turn-1-iter-1-request.json"
if [[ -f "$first_req" ]]; then
  SYSTEM_PROMPT="$(jq -r '.instructions // (.input[]? | select(.type == "message" and .role == "system") | (.content[0].text // .content)) // ""' "$first_req" 2>/dev/null || echo '')"
fi
jq -n --arg id "$SESSION_ID" --arg title "$TITLE" --arg created "$SESSION_CREATED_AT" \
      --arg model "gpt-5.5" --arg sys "$SYSTEM_PROMPT" \
      --argjson session "$SESSION_ROW" \
   '{session_id:$id, title:$title, model:$model, created_at:$created,
     system_prompt:$sys, session_row:$session}' \
   > "$OUTPUT_DIR/session.json"
log "  session.json"

# metadata.json
total_wall="$(jq -s '[.[].wall_clock_ms // 0] | add' "$TURNS_LOG")"
turns_json="$(jq -s '.' "$TURNS_LOG")"
jq -n --arg recorded "$RECORDED_AT_ISO" \
      --arg rev "$LEEK_GIT_REV" --arg corpus_rev "$CORPUS_GIT_REV" \
      --arg model "gpt-5.5" --arg sid "$SESSION_ID" \
      --argjson cost "$(echo "$total_cost_bc" | jq -R 'tonumber')" \
      --argjson wall "$total_wall" --arg stopped "$stopped_by" \
      --argjson turns "$turns_json" \
   '{recorded_at:$recorded, leek_git_rev:$rev, codex_model:$model,
     corpus_git_rev:$corpus_rev, session_id:$sid,
     total_cost_usd:$cost, total_wall_clock_ms:$wall,
     stopped_by:$stopped, turns:$turns}' \
   > "$OUTPUT_DIR/metadata.json"
log "  metadata.json"

rm -f "$TURNS_LOG"

log "fixture complete:"
du -sh "$OUTPUT_DIR"
log "  session_id=$SESSION_ID"
log "  turns=$turn_index"
log "  total_cost=\$$total_cost_bc"
log "  max_iter_in_turn=$max_iter_seen"
log "  stopped_by=$stopped_by"
