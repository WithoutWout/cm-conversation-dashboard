// Import and Stored data work in days of the display timezone; the database,
// the API windows and the coverage masks stay UTC. These are the conversions
// between the two — see docs/import.md → "Calendars are in your timezone;
// storage and requests are UTC". The real functions run against a pinned zone.
const assert = require("assert")
const vm = require("vm")
const { extract } = require("./extract")

const ctx = vm.createContext({})
vm.runInContext(
  `
  const insTzFormatters = new Map()
  const IMP_RETENTION_DAYS = 90
  let _impDbHours = new Map()
  ${[
    "insOffsetAt",
    "insUtcDayKey",
    "insShiftDay",
    "insZoneStampToUtc",
    "insZoneDayBounds",
    "zoneDayKey",
    "zoneInstant",
    "zoneDayCounts",
    "zoneDayCoverage",
    "_impIsoZ",
    "_impWindowCovered",
    "buildImportQueue",
    "missingImportDays",
  ]
    .map(extract)
    .join("\n")}
  globalThis.api = {
    buildImportQueue, zoneDayCounts, zoneInstant, zoneDayCoverage, _impWindowCovered, missingImportDays,
    setHours: (m) => { _impDbHours = m },
  }
  `,
  ctx,
)
const api = ctx.api
const AMS = "Europe/Amsterdam"
const ALL = 2 ** 24 - 1
const plain = (v) => JSON.parse(JSON.stringify(v))

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

const queue = (fromDay, toDay, zone, opts) =>
  plain(
    api.buildImportQueue(
      api.zoneInstant(fromDay, "00:00", zone),
      new Date(api.zoneInstant(toDay, "23:59", zone).getTime() + 59000),
      { zone, ...(opts || {}) },
    ),
  )

test("a picked local day is one item of two UTC windows, and nothing is missed", () => {
  api.setHours(new Map())
  const q = queue("2026-09-01", "2026-09-02", AMS)
  assert.deepStrictEqual(q.map((i) => i.key), ["2026-09-01", "2026-09-02"])
  assert.deepStrictEqual(q[0].windows, [
    { startUtc: "2026-08-31T22:00:00Z", endUtc: "2026-08-31T23:59:59Z" },
    { startUtc: "2026-09-01T00:00:00Z", endUtc: "2026-09-01T21:59:59Z" },
  ])
  // Consecutive, no gap and no overlap across the day boundary.
  assert.strictEqual(q[1].windows[0].startUtc, "2026-09-01T22:00:00Z")
  assert.strictEqual(q[1].windows[1].endUtc, "2026-09-02T21:59:59Z")
})

test("in UTC a day is one window, as it always was", () => {
  api.setHours(new Map())
  const q = queue("2026-09-01", "2026-09-01", "UTC")
  assert.deepStrictEqual(q[0].windows, [{ startUtc: "2026-09-01T00:00:00Z", endUtc: "2026-09-01T23:59:59Z" }])
})

test("a DST day's windows cover its 23 or 25 hours", () => {
  api.setHours(new Map())
  const [spring] = queue("2026-03-29", "2026-03-29", AMS)
  assert.strictEqual(spring.windows[0].startUtc, "2026-03-28T23:00:00Z")
  assert.strictEqual(spring.windows[spring.windows.length - 1].endUtc, "2026-03-29T21:59:59Z")
  const [autumn] = queue("2026-10-25", "2026-10-25", AMS)
  assert.strictEqual(autumn.windows[0].startUtc, "2026-10-24T22:00:00Z")
  assert.strictEqual(autumn.windows[autumn.windows.length - 1].endUtc, "2026-10-25T22:59:59Z")
})

test("a local day is 'already imported' only when both of its UTC halves are", () => {
  api.setHours(new Map([["2026-09-01", ALL]]))
  // 1 Sep local needs 31 Aug 22–23Z too.
  assert.strictEqual(queue("2026-09-01", "2026-09-01", AMS)[0].existing, false)
  api.setHours(new Map([["2026-08-31", ALL], ["2026-09-01", ALL]]))
  assert.strictEqual(queue("2026-09-01", "2026-09-01", AMS)[0].existing, true)
  assert.strictEqual(queue("2026-09-01", "2026-09-01", AMS, { skipExisting: true }).length, 0)
})

test("per-hour counts fold into local days", () => {
  const hours = [
    { date: "2026-08-31", hour: 21, count: 5 }, // 23:00 local, 31 Aug
    { date: "2026-08-31", hour: 22, count: 7 }, // 00:00 local, 1 Sep
    { date: "2026-09-01", hour: 10, count: 3 },
  ]
  assert.deepStrictEqual(plain([...api.zoneDayCounts(hours, AMS)]), [["2026-08-31", 5], ["2026-09-01", 10]])
  assert.deepStrictEqual(plain([...api.zoneDayCounts(hours, "UTC")]), [["2026-08-31", 12], ["2026-09-01", 3]])
})

test("a window is covered by the hours it fetches whole, not the ones it touches", () => {
  const hours = (list) => list.reduce((m, h) => m | (1 << h), 0)
  // The retention floor's first window starts mid-hour; 10:00 can never be
  // recorded, so it must not keep the day from counting as imported.
  api.setHours(new Map([["2026-09-01", hours([11, 12, 13])]]))
  assert.strictEqual(api._impWindowCovered("2026-09-01T10:31:00Z", "2026-09-01T13:59:59Z"), true)
  // A range ending at 14:20 needs 13:00 — the last whole hour — but not 14:00.
  assert.strictEqual(api._impWindowCovered("2026-09-01T11:00:00Z", "2026-09-01T14:20:59Z"), true)
  api.setHours(new Map([["2026-09-01", hours([11, 12])]]))
  assert.strictEqual(api._impWindowCovered("2026-09-01T11:00:00Z", "2026-09-01T14:20:59Z"), false)
  // A window inside one hour falls back to that hour.
  assert.strictEqual(api._impWindowCovered("2026-09-01T14:00:00Z", "2026-09-01T14:20:59Z"), false)
  assert.strictEqual(api._impWindowCovered("2026-09-01T12:00:00Z", "2026-09-01T12:20:59Z"), true)
  // A full UTC day still needs all 24.
  api.setHours(new Map([["2026-09-01", ALL]]))
  assert.strictEqual(api._impWindowCovered("2026-09-01T00:00:00Z", "2026-09-01T23:59:59Z"), true)
  api.setHours(new Map([["2026-09-01", ALL & ~(1 << 23)]]))
  assert.strictEqual(api._impWindowCovered("2026-09-01T00:00:00Z", "2026-09-01T23:59:59Z"), false)
})

test("the daily check lists incomplete days from the oldest one stored, never today", () => {
  const full = (from, to) => {
    const m = new Map()
    for (let d = Date.parse(from + "T00:00:00Z"); d <= Date.parse(to + "T00:00:00Z"); d += 864e5)
      m.set(new Date(d).toISOString().slice(0, 10), ALL)
    return m
  }
  // Stored 5–12 Sep in full (UTC), with 8 Sep missing and 10 Sep half done.
  const cov = full("2026-09-05", "2026-09-12")
  cov.delete("2026-09-08")
  cov.set("2026-09-10", (1 << 12) - 1)
  assert.deepStrictEqual(
    plain(api.missingImportDays(cov, "2026-09-13", 90, "UTC")),
    ["2026-09-08", "2026-09-10"],
  )
  // Days since the last import count; today does not.
  assert.deepStrictEqual(plain(api.missingImportDays(cov, "2026-09-15", 90, "UTC")), [
    "2026-09-08", "2026-09-10", "2026-09-13", "2026-09-14",
  ])
  // A retention setting shorter than the gap moves the floor past it.
  assert.deepStrictEqual(plain(api.missingImportDays(cov, "2026-09-15", 4, "UTC")), [
    "2026-09-13", "2026-09-14",
  ])
  // Nothing stored: nothing to say.
  assert.deepStrictEqual(plain(api.missingImportDays(new Map(), "2026-09-15", 90, "UTC")), [])
  // In Amsterdam the first stored local day lacks its first two hours (they
  // are on UTC 4 Sep), so it is listed too.
  assert.strictEqual(api.missingImportDays(cov, "2026-09-13", 90, AMS)[0], "2026-09-05")
})

if (failed) {
  console.log("\n" + failed + " import-day test(s) failed")
  process.exit(1)
}
console.log("\nImport days: all checks passed")
