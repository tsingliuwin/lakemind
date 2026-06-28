// One-off sanity check for src/lib/metrics.ts pure functions.
// Run: npx tsx scripts/verify-metrics.ts
import { mergeUsage, derivePanelMetrics, fmtCap, fmtNum, fmtPct } from "../src/lib/metrics";
import type { TokenUsage } from "../src/lib/types";

let pass = 0, fail = 0;
function assert(cond: boolean, msg: string) {
  if (cond) { pass++; } else { fail++; console.error("FAIL:", msg); }
}

// --- mergeUsage: estimate first message (no prev) ---
let u = mergeUsage(null, { isEstimate: true, promptTokens: 1200, completionTokens: 0, estPreambleRaw: 900, estToolsRaw: 80 });
assert(u.isEstimate === true, "first estimate isEstimate");
assert(u.promptTokens === 1200, "first estimate prompt");
assert(u._calibK === 1, "first estimate calibK=1");

// --- mergeUsage: real per-call event (OpenAI-like) ---
// runCompletionTokens = cumulative run output so far (call 1 = 200).
u = mergeUsage(u, { isEstimate: false, promptTokens: 1000, completionTokens: 200, runCompletionTokens: 200, cacheReadTokens: 800, cacheCreationTokens: 0, freshInputTokens: 200, estPreambleRaw: 900, estToolsRaw: 80, kSample: 1000 / 1200 });
assert(u.isEstimate === false, "real isEstimate=false");
assert(u.promptTokens === 1000, "real prompt");
assert(u._totalPrompt === 1000, "cumulative prompt after 1 call");
assert(u._totalCompletion === 200, "cumulative completion");
assert(u.completionTokens === 200, "perTurn completion = cumulative run (call 1)");
assert(u._totalCacheRead === 800, "cumulative cacheRead");
assert(u._llmCallCount === 1, "llmCallCount=1");
assert(u._peakPromptTokens === 1000, "peak=1000");
assert(Math.abs((u._calibK ?? 1) - (0.6 * 1 + 0.4 * (1000 / 1200))) < 1e-9, "calibK EMA fit");

// --- second real call (multi-turn within a run): no kSample (not first) ---
// runCompletionTokens = 500 (call1 200 + call2 300) — cumulative, no drop.
u = mergeUsage(u, { isEstimate: false, promptTokens: 1500, completionTokens: 300, runCompletionTokens: 500, cacheReadTokens: 1200, cacheCreationTokens: 0, freshInputTokens: 300, estPreambleRaw: 900, estToolsRaw: 80 });
assert(u._totalPrompt === 2500, "cumulative prompt after 2 calls");
assert(u._totalCompletion === 500, "cumulative completion after 2 calls");
assert(u.completionTokens === 500, "perTurn completion = cumulative run total (no drop between calls)");
assert(u._llmCallCount === 2, "llmCallCount=2");
assert(u._peakPromptTokens === 1500, "peak=1500");

// --- estimate mid-run should not drop below the real cumulative ---
// Simulate call-3 streaming: completionTokens (est) = run_output(500) + est(40) = 540.
u = mergeUsage(u, { isEstimate: true, promptTokens: 1600, completionTokens: 540, estPreambleRaw: 900, estToolsRaw: 80 });
assert(u.completionTokens === 540, "estimate completion = prior real + current est (>= 500, no drop)");
assert(u.isEstimate === true, "estimate sets isEstimate");

// --- run summary (turn complete) ---
u = mergeUsage(u, { isEstimate: false, turnComplete: true, runOutputTokens: 580, runElapsedMs: 10000, tokPerSec: 58 });
assert(u._turnCount === 1, "turnCount=1 after run summary");
assert(u._lastTokPerSec === 58, "lastTokPerSec=58");
assert(u._totalPrompt === 2500, "run summary does not accumulate prompt");
assert(u.completionTokens === 580, "perTurn completion = final run total after summary");

// --- derivePanelMetrics: composition sums to totalPrompt ---
const m = derivePanelMetrics(u, { contextWindow: 128000 });
assert(m != null, "metrics non-null");
const comp = m!.composition;
const sumPct = comp.preamble.pct + comp.tools.pct + comp.messages.pct;
assert(Math.abs(sumPct - 100) < 0.01, `composition pct sums to 100 (got ${sumPct})`);
const sumTok = comp.preamble.tokens + comp.tools.tokens + comp.messages.tokens;
assert(sumTok === 2500, `composition tokens sum to totalPrompt (got ${sumTok})`);
// preamble/tools = calls(2) * k*raw. messages = remainder.
assert(comp.preamble.tokens === 2 * Math.round((u._calibK ?? 1) * 900), "cumPreamble = calls * k*raw");
assert(m!.cumulative.hitRate === 2000 / 2500 * 100, "hitRate = totalCacheRead/totalPrompt");
assert(m!.cumulative.turnCount === 1, "metrics turnCount=1");
assert(m!.perTurn.completion === 580, "panel perTurn.completion = run total (includes reasoning)");
assert(m!.capacity.peak === 1600, "capacity peak=1600 (estimate raised it)");
assert(m!.tokPerSec === 58, "tokPerSec from last run (run total / elapsed)");

// --- cache hit rate never exceeds 100% (Anthropic shape, the old >100% bug) ---
let au: TokenUsage | null = null;
// Anthropic: input=20 (fresh), creation=50, cached=80 -> prompt=150, hit=80/150=53%
au = mergeUsage(au, { isEstimate: false, promptTokens: 150, completionTokens: 10, cacheReadTokens: 80, cacheCreationTokens: 50, freshInputTokens: 20, estPreambleRaw: 100, estToolsRaw: 10, kSample: 1 });
const am = derivePanelMetrics(au, { contextWindow: 128000 })!;
assert(am.cumulative.hitRate <= 100, `hitRate <= 100 (got ${am.cumulative.hitRate})`);
assert(Math.abs(am.cumulative.hitRate - 80 / 150 * 100) < 0.01, `anthropic hitRate=53.3 (got ${am.cumulative.hitRate})`);

// --- formatters ---
assert(fmtCap(128000) === "12.8万", `fmtCap(128000) (got ${fmtCap(128000)})`);
assert(fmtCap(82000) === "8.2万", `fmtCap(82000) (got ${fmtCap(82000)})`);
assert(fmtNum(24103) === "24,103", `fmtNum(24103) (got ${fmtNum(24103)})`);
assert(fmtPct(79.95) === "80%", `fmtPct(79.95) (got ${fmtPct(79.95)})`);
assert(fmtPct(58.3) === "58.3%", `fmtPct(58.3) (got ${fmtPct(58.3)})`);

// --- legacy seed: old-shape persisted data migrates cumulative totals ---
const legacy: TokenUsage = { inputTokens: 5000, outputTokens: 800, totalTokens: 5800, cachedInputTokens: 3000, messagesTokens: 0, toolsTokens: 0, preambleTokens: 0, cacheHitRate: 60, _totalInputAllTurns: 5000, _totalCachedAllTurns: 3000, _peakInputTokens: 5000 };
const lu = mergeUsage(legacy, { isEstimate: false, promptTokens: 1000, completionTokens: 100, cacheReadTokens: 900, cacheCreationTokens: 0, freshInputTokens: 100, estPreambleRaw: 900, estToolsRaw: 80 });
assert(lu._totalPrompt === 6000, `legacy seed cumulative (got ${lu._totalPrompt})`);
assert(lu._peakPromptTokens === 5000, `legacy seed peak preserved (got ${lu._peakPromptTokens})`);

// --- k convergence: many kSample=2.0 events → _calibK converges to 2.0 ---
let cu: TokenUsage | null = null;
for (let i = 0; i < 40; i++) {
  cu = mergeUsage(cu, { isEstimate: false, promptTokens: 2000, completionTokens: 1, cacheReadTokens: 0, estPreambleRaw: 100, estToolsRaw: 10, kSample: 2.0 });
}
assert(Math.abs((cu._calibK ?? 1) - 2.0) < 0.05, `k converges to 2.0 (got ${cu._calibK})`);

console.log(`\n${pass} passed, ${fail} failed`);
if (fail > 0) process.exit(1);