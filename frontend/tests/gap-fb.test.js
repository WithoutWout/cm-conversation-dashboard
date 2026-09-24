// GAP's feedback list: the rated answers of a range, grouped per Article and
// Dialog node (or per Dialog) and ranked by the share of positive ratings. The
// real function runs against rows shaped like `get_gap_feedback`'s — see
// docs/gap.md → "Feedback".
const assert = require("assert")
const vm = require("vm")
const { extract } = require("./extract")

const ctx = vm.createContext({})
vm.runInContext(
  `
  const insTzFormatters = new Map()
  ${["parseJsonSafe", "gapFbItems", "insOffsetAt", "insUtcDayKey", "insZoneStampToUtc", "insZoneDayBounds", "zoneDayCoverage"].map(extract).join("\n")}
  globalThis.api = { gapFbItems, zoneDayCoverage }
  `,
  ctx,
)
const { gapFbItems, zoneDayCoverage } = ctx.api

let failed = 0
const test = (name, fn) => {
  try {
    fn()
    console.log("  ok   " + name)
  } catch (e) {
    failed++
    console.log("  FAIL " + name + "\n       " + e.message)
  }
}

const ROWS = [
  { logId: 1, score: -1, articleIds: '["qa-7"]' },
  { logId: 2, score: 1, articleIds: '["qa-7"]' },
  { logId: 3, score: 1, articleIds: '["qa-7"]' },
  // A Dialog node that answered with an Article's text is about both.
  { logId: 4, score: -1, articleIds: '["dn-9-2","qa-7"]' },
  { logId: 5, score: 1, articleIds: '["dn-9-3"]' },
  // The fallback, GenAI: nothing answered that can be opened.
  { logId: 6, score: -1, articleIds: "" },
  { logId: 7, score: -1, articleIds: "[]" },
]
const byKey = (items) => Object.fromEntries(items.map((it) => [it.key, it]))

test("an answer is counted under every id it carries", () => {
  const it = byKey(gapFbItems(ROWS, "answer"))
  assert.deepStrictEqual(Object.keys(it).sort(), ["", "dn-9-2", "dn-9-3", "qa-7"])
  assert.strictEqual(it["qa-7"].rated, 4)
  assert.strictEqual(it["qa-7"].neg, 2)
  assert.strictEqual(it["qa-7"].share, 0.5)
  assert.strictEqual(it["dn-9-2"].rated, 1)
  assert.strictEqual(it["dn-9-2"].share, 0)
  assert.strictEqual(it[""].rated, 2, "no id at all is its own line, not dropped")
})

test("per Dialog, the nodes add up into their Dialog", () => {
  const it = byKey(gapFbItems(ROWS, "content"))
  assert.deepStrictEqual(Object.keys(it).sort(), ["", "dn-9", "qa-7"])
  assert.strictEqual(it["dn-9"].rated, 2)
  assert.strictEqual(it["dn-9"].neg, 1)
  assert.strictEqual(it["dn-9"].rows.length, 2)
})

test("nothing rated is nothing listed", () => {
  assert.strictEqual(gapFbItems([], "answer").length, 0)
  assert.strictEqual(gapFbItems(null, "content").length, 0)
})

// The calendar's "which days hold data": a display-timezone day spans parts of
// two UTC days, and coverage is stored per UTC day.
const ALL = 2 ** 24 - 1
test("a local day is imported only when every one of its UTC hours is", () => {
  const cov = new Map([["2026-06-01", ALL]])
  // Amsterdam in June is UTC+2: 1 June local is 31 May 22:00 → 1 June 21:59 UTC.
  // 31 May is not imported, so two of its hours are missing.
  const c = zoneDayCoverage("2026-06-01", cov, "Europe/Amsterdam")
  assert.strictEqual(c.total, 24)
  assert.strictEqual(c.covered, 22)
  cov.set("2026-05-31", ALL)
  assert.strictEqual(zoneDayCoverage("2026-06-01", cov, "Europe/Amsterdam").covered, 24)
  // In UTC the day is its own.
  assert.strictEqual(zoneDayCoverage("2026-06-02", new Map([["2026-06-02", ALL]]), "UTC").covered, 24)
})

test("a DST day has 23 or 25 hours, and nothing imported is nothing", () => {
  assert.strictEqual(zoneDayCoverage("2026-03-29", new Map(), "Europe/Amsterdam").total, 23)
  assert.strictEqual(zoneDayCoverage("2026-10-25", new Map(), "Europe/Amsterdam").total, 25)
  assert.strictEqual(zoneDayCoverage("2026-10-25", new Map(), "Europe/Amsterdam").covered, 0)
})

if (failed) {
  console.log("\n" + failed + " gap feedback test(s) failed")
  process.exit(1)
}
console.log("\nall gap feedback tests passed")
