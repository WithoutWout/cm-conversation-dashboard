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
  const strip = (t) => t // its regexes are beyond extract.js; no markup in these rows
  ${["parseJsonSafe", "gapPlain", "gapFbRowList", "gapFbNegShareReaches", "gapFbItems", "insOffsetAt", "insUtcDayKey", "insZoneStampToUtc", "insZoneDayBounds", "zoneDayCoverage", "gapRangeLabel", "gapExportTitle"].map(extract).join("\n")}
  globalThis.api = { gapFbRowList, gapFbNegShareReaches, gapFbItems, zoneDayCoverage, gapRangeLabel, gapExportTitle }
  `,
  ctx,
)
const { gapFbRowList, gapFbNegShareReaches, gapFbItems, zoneDayCoverage, gapRangeLabel, gapExportTitle } = ctx.api

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

test("an item is fixed once every thumbs down on it is", () => {
  const rows = [
    { logId: 1, score: -1, articleIds: '["qa-7"]', fixedAt: "2026-09-20T10:00:00" },
    { logId: 2, score: -1, articleIds: '["qa-7"]', fixedAt: null },
    { logId: 3, score: 1, articleIds: '["qa-7"]', fixedAt: null },
    { logId: 4, score: -1, articleIds: '["qa-8"]', fixedAt: "2026-09-21T09:00:00" },
    { logId: 5, score: 1, articleIds: '["qa-9"]', fixedAt: null },
  ]
  const by = (items) => Object.fromEntries(items.map((it) => [it.key, it]))
  let items = by(gapFbItems(rows, "answer"))
  // One of two thumbs down fixed: partly, not fixed.
  assert.strictEqual(items["qa-7"].negFixed, 1)
  assert.strictEqual(items["qa-7"].fixed, false)
  assert.strictEqual(items["qa-7"].fixedAt, null)
  assert.strictEqual(items["qa-8"].fixed, true)
  assert.strictEqual(items["qa-8"].fixedAt, "2026-09-21T09:00:00")
  // Nothing rated down is nothing to fix.
  assert.strictEqual(items["qa-9"].fixed, false)
  rows[1].fixedAt = "2026-09-22T08:00:00"
  items = by(gapFbItems(rows, "answer"))
  assert.strictEqual(items["qa-7"].fixed, true)
  assert.strictEqual(items["qa-7"].fixedAt, "2026-09-22T08:00:00", "the latest fix")
  // A new thumbs down after the fix reopens it.
  rows.push({ logId: 6, score: -1, articleIds: '["qa-7"]', fixedAt: null })
  assert.strictEqual(by(gapFbItems(rows, "answer"))["qa-7"].fixed, false)
})

test("the thumbs-down share filter keeps what reaches it", () => {
  const reaches = gapFbNegShareReaches
  assert.strictEqual(reaches({ neg: 1, rated: 4 }, 25), true, "exactly on the boundary stays")
  assert.strictEqual(reaches({ neg: 1, rated: 4 }, 26), false)
  assert.strictEqual(reaches({ neg: 1, rated: 3 }, 33), true)
  assert.strictEqual(reaches({ neg: 1, rated: 3 }, 34), false)
  // Off by default: nothing rated down still lists.
  assert.strictEqual(reaches({ neg: 0, rated: 9 }, 0), true)
  assert.strictEqual(reaches({ neg: 0, rated: 9 }, 1), false)
  assert.strictEqual(reaches({ neg: 3, rated: 3 }, 100), true)
})

test("All ratings lists every rated answer once, whatever its ids", () => {
  const rows = [
    { logId: 1, score: -1, question: "a", response: "x", articleIds: '["qa-1","dn-2-3"]', timestamp: "2026-09-01T10:00:00", fixedAt: null },
    { logId: 2, score: 1, question: "b", response: "y", articleIds: "", timestamp: "2026-09-03T10:00:00", fixedAt: null },
    { logId: 3, score: -1, question: "c", response: "**Parkeren** kan", articleIds: '["qa-7"]', timestamp: "2026-09-02T10:00:00", fixedAt: "2026-09-04T10:00:00" },
  ]
  const ids = (opts) =>
    [...gapFbRowList(rows, { query: "", rating: "all", status: "all", sort: { key: "time", dir: -1 }, ...opts })].map((r) => r.logId).join()
  assert.strictEqual(ids({}), "2,3,1", "newest first")
  assert.strictEqual(ids({ rating: "up" }), "2")
  assert.strictEqual(ids({ rating: "down" }), "3,1")
  // A thumbs up is nothing to fix: it is neither open nor fixed.
  assert.strictEqual(ids({ status: "open" }), "1")
  assert.strictEqual(ids({ status: "fixed" }), "3")
  // Ties in a column fall back to newest first.
  assert.strictEqual(ids({ sort: { key: "rating", dir: 1 } }), "3,1,2", "thumbs down first")
  assert.strictEqual(ids({ sort: { key: "question", dir: -1 } }), "3,2,1")
  // The filter reads the question, the plain response and the ids.
  assert.strictEqual(ids({ query: "parkeren kan" }), "3")
  assert.strictEqual(ids({ query: "qa-1" }), "1")
  assert.strictEqual(ids({ query: "nothing" }), "")
})

test("an export is named for its analysis and its date range", () => {
  const r = (from, to) => ({ from, to })
  assert.strictEqual(gapRangeLabel(r("2026-09-19", "2026-09-19")), "19 Sep 2026")
  assert.strictEqual(gapRangeLabel(r("2026-09-14", "2026-09-20")), "14–20 Sep 2026")
  assert.strictEqual(gapRangeLabel(r("2026-08-28", "2026-09-03")), "28 Aug – 3 Sep 2026")
  assert.strictEqual(gapRangeLabel(r("2025-12-28", "2026-01-03")), "28 Dec 2025 – 3 Jan 2026")
  assert.strictEqual(gapRangeLabel(null), "")
  const t = gapExportTitle("Recognition", r("2026-09-14", "2026-09-20"))
  assert.strictEqual(t.file, "Recognition analysis 14–20 Sep 2026.xlsx")
  assert.strictEqual(t.sheet, "Recognition 14–20 Sep 2026")
  // Excel allows 31 characters in a sheet name; past that the sheet is the type.
  const long = gapExportTitle("Recognition", r("2025-12-28", "2026-01-03"))
  assert.strictEqual(long.file, "Recognition analysis 28 Dec 2025 – 3 Jan 2026.xlsx")
  assert.strictEqual(long.sheet, "Recognition")
  assert.strictEqual(gapExportTitle("GenAI", null).file, "GenAI analysis.xlsx")
})

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
