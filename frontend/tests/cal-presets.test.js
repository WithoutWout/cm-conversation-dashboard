// The quick ranges every day calendar offers (CAL_PRESETS / calPresetRange).
const vm = require("vm")
const { extract, extractConst } = require("./extract")

const ctx = vm.createContext({ console })
vm.runInContext(
  `${extractConst("CAL_PRESETS")}
  ${["insUtcDayKey", "insShiftDay", "calPresetRange"].map(extract).join("\n")}`,
  ctx,
)
let failed = 0
const ok = (n, c) => {
  if (!c) failed++
  console.log((c ? "  PASS  " : "  FAIL  ") + n)
}
const r = (k, today) => JSON.stringify(ctx.calPresetRange(k, today))

// 2026-11-19 is a Thursday.
ok("last 7 days ends today", r("7", "2026-11-19") === '{"from":"2026-11-13","to":"2026-11-19"}')
ok("last week is the previous Monday–Sunday", r("last-week", "2026-11-19") === '{"from":"2026-11-09","to":"2026-11-15"}')
ok("last week, asked on a Monday", r("last-week", "2026-11-16") === '{"from":"2026-11-09","to":"2026-11-15"}')
ok("last week, asked on a Sunday", r("last-week", "2026-11-22") === '{"from":"2026-11-09","to":"2026-11-15"}')
ok("last week across a year", r("last-week", "2027-01-01") === '{"from":"2026-12-21","to":"2026-12-27"}')
ok("last month", r("last-month", "2026-03-10") === '{"from":"2026-02-01","to":"2026-02-28"}')
ok("last month in January", r("last-month", "2027-01-05") === '{"from":"2026-12-01","to":"2026-12-31"}')
ok("this month", r("month", "2026-11-19") === '{"from":"2026-11-01","to":"2026-11-19"}')
ok("every preset resolves", vm.runInContext("CAL_PRESETS", ctx).every(([k]) => /^\{"from":"\d{4}-\d\d-\d\d","to":/.test(r(k, "2026-11-19"))))

console.log(failed ? "\n" + failed + " FAILED" : "\nAll checks passed")
process.exit(failed ? 1 : 0)
