// A loading indicator that appears and vanishes inside a tenth of a second
// reads as the interface glitching. `gateLoading` is what stops that: it holds
// the indicator back until the work has genuinely been slow, and then holds it
// *on* long enough to be read.
//
// The whole thing is timing, and timing is exactly what nobody verifies by
// hand — "it looked fine" cannot distinguish a spinner that was correctly
// suppressed from one that was suppressed for the wrong reason, and the
// interesting cases (work finishing a few milliseconds either side of the
// threshold) are unreachable by clicking. So the gate runs here against a
// virtual clock instead.
const { extract } = require("./extract")
const vm = require("vm")

const DELAY = 500 // LOADING_SHOW_DELAY_MS
const MIN = 300 // LOADING_MIN_VISIBLE_MS

// ── A virtual clock, so a test that spans ten seconds costs nothing ──
let now = 0
let timers = []
let nextId = 1
const setTimeoutFake = (fn, ms) => {
  const id = nextId++
  timers.push({ id, at: now + (ms || 0), fn })
  return id
}
const clearTimeoutFake = (id) => {
  timers = timers.filter((t) => t.id !== id)
}
/// Advance the clock, firing anything due — in time order, as a real loop does.
const advance = (ms) => {
  const until = now + ms
  for (;;) {
    const due = timers
      .filter((t) => t.at <= until)
      .sort((a, b) => a.at - b.at)[0]
    if (!due) break
    timers = timers.filter((t) => t !== due)
    now = due.at
    due.fn()
  }
  now = until
}
const resetClock = () => {
  now = 0
  timers = []
  ctx._loadingGates.clear()
}

const ctx = vm.createContext({
  console,
  Math,
  Map,
  Date: { now: () => now },
  setTimeout: setTimeoutFake,
  clearTimeout: clearTimeoutFake,
})
vm.runInContext(
  `
  const LOADING_SHOW_DELAY_MS = ${DELAY}
  const LOADING_MIN_VISIBLE_MS = ${MIN}
  // \`var\`, not \`const\`: only var reaches the context object, and the
  // harness has to clear the gate map between cases.
  var _loadingGates = new Map()
  ${extract("gateLoading")}
`,
  ctx,
)
const gate = ctx.gateLoading

const out = []
let failed = 0
const ok = (n, c) => {
  if (!c) failed++
  out.push((c ? "  PASS  " : "  FAIL  ") + n)
}

/// A recorder standing in for the DOM change.
const rec = () => {
  const calls = []
  const fn = (on) => calls.push({ on, at: now })
  fn.calls = calls
  return fn
}

// ── The case this exists for: fast work shows nothing at all ────────
{
  resetClock()
  const a = rec()
  gate("k", true, a)
  advance(80) // a cached read
  gate("k", false, a)
  advance(5000) // and nothing appears later, either
  ok("work faster than the delay never shows an indicator", a.calls.length === 0)
}
{
  resetClock()
  const a = rec()
  gate("k", true, a)
  advance(DELAY - 1)
  gate("k", false, a)
  advance(5000)
  ok("finishing one millisecond before the delay shows nothing", a.calls.length === 0)
}

// ── Slow work does show, once, on time ──────────────────────────────
{
  resetClock()
  const a = rec()
  gate("k", true, a)
  advance(2000)
  ok("slow work shows the indicator", a.calls.length === 1 && a.calls[0].on)
  ok("shows exactly on the delay", a.calls[0].at === DELAY)
  gate("k", false, a)
  ok("hides immediately once the minimum is long past", a.calls.length === 2 && !a.calls[1].on)
  ok("hide is not deferred", a.calls[1].at === 2000)
}

// ── The other half: what stops the flicker merely moving ────────────
{
  resetClock()
  const a = rec()
  gate("k", true, a)
  advance(DELAY + 5) // shown, then work lands almost at once
  gate("k", false, a)
  ok("still visible right after being shown", a.calls.length === 1)
  advance(MIN)
  ok("stays up for the minimum", a.calls.length === 2 && !a.calls[1].on)
  ok(
    "hidden exactly a minimum after it appeared",
    a.calls[1].at === DELAY + MIN,
  )
}
{
  resetClock()
  const a = rec()
  gate("k", true, a)
  advance(DELAY + MIN) // visible for exactly the minimum
  gate("k", false, a)
  ok("no extra hold once the minimum is met to the millisecond", a.calls.length === 2)
}

// ── Repeat and interleaved calls ────────────────────────────────────
{
  resetClock()
  const a = rec()
  gate("k", true, a)
  gate("k", true, a)
  gate("k", true, a)
  advance(2000)
  ok("asking repeatedly shows it once", a.calls.length === 1)
}
{
  resetClock()
  const a = rec()
  gate("k", true, a)
  advance(2000)
  gate("k", false, a)
  gate("k", false, a)
  ok("hiding twice applies once", a.calls.length === 2)
}
{
  // Two quick reads back to back — each finishing well inside the delay.
  resetClock()
  const a = rec()
  for (let i = 0; i < 5; i++) {
    gate("k", true, a)
    advance(50)
    gate("k", false, a)
    advance(10)
  }
  advance(5000)
  ok("a burst of fast reads never shows anything", a.calls.length === 0)
}
{
  // A second read starting before the first one's spinner was due must not
  // reset the clock — the user has been waiting the whole time.
  resetClock()
  const a = rec()
  gate("k", true, a)
  advance(400)
  gate("k", false, a) // first read done
  gate("k", true, a) // second starts at once
  advance(200) // 600ms total, but only 200ms into this read
  ok("the delay is per request, not cumulative", a.calls.length === 0)
  advance(DELAY)
  ok("the second read shows on its own delay", a.calls.length === 1 && a.calls[0].on)
}

// ── Keys are independent ────────────────────────────────────────────
{
  resetClock()
  const a = rec()
  const b = rec()
  gate("one", true, a)
  gate("two", true, b)
  advance(100)
  gate("one", false, a) // fast
  advance(2000) // two is still going
  ok("a fast indicator stays hidden", a.calls.length === 0)
  ok("while a slow one on another key shows", b.calls.length === 1 && b.calls[0].on)
}

// ── A hide that arrives while the show is still pending ─────────────
{
  resetClock()
  const a = rec()
  gate("k", true, a)
  advance(200)
  gate("k", false, a)
  advance(DELAY * 3)
  ok("cancelling a pending show leaves no stray timer", a.calls.length === 0)
  // And the gate must still work afterwards rather than being wedged.
  gate("k", true, a)
  advance(2000)
  ok("the gate still works after a cancelled show", a.calls.length === 1)
}

console.log(out.join("\n"))
console.log(
  failed
    ? `\nLoading gate: ${failed} FAILED`
    : "\nLoading gate: all checks passed",
)
process.exit(failed ? 1 : 0)
