// The Import tab's progress view, once a run has ended.
//
// The current operation is only ever overwritten by the next one, and the last
// one of every run is "Finishing up…". Rendered past the end of the run, it sat
// above "Import complete" with the Close button already showing, and a user
// watched it for twenty minutes wondering whether the import was still going.
const { extract } = require("./extract")
const vm = require("vm")

const out = []
let failed = 0
const ok = (label, cond) => {
  out.push(`  ${cond ? "ok  " : "FAIL"} ${label}`)
  if (!cond) failed++
}

function render(phase, outcome) {
  const ctx = {
    _impPhase: phase,
    _impOutcome: outcome,
    _impOpText: "Finishing up…",
    _impOpDetail: "Updating conversation summaries",
    _impQueue: [{ key: "2026-11-03", status: "done", detail: "18825 new" }],
    _impDoneCount: 1,
    _impTotals: { inserted: 18825, skipped: 0, days: 1 },
    _impLog: ["Queue complete"],
    _result: "",
    esc: (s) => String(s),
  }
  vm.createContext(ctx)
  vm.runInContext(`${extract("_impProgressHtml")}\n_result = _impProgressHtml()`, ctx)
  return ctx._result
}

const running = render("running", null)
ok("a running import shows its current operation", running.includes("Finishing up…"))
ok("a running import shows what that operation is doing", running.includes("Updating conversation summaries"))
ok("a running import has a spinner", running.includes("import-spinner"))

const outcomes = [
  ["done", { type: "done" }, "Import complete"],
  ["cancelled", { type: "cancelled" }, "Import cancelled"],
  ["failed", { type: "failed", index: 0, error: { message: "HTTP 500" } }, "Import stopped"],
]
for (const [name, outcome, banner] of outcomes) {
  const html = render("finished", outcome)
  ok(`${name}: the banner says how it ended`, html.includes(banner))
  ok(`${name}: no operation is left on screen`, !html.includes("Finishing up…"))
  ok(`${name}: nor what it was doing`, !html.includes("Updating conversation summaries"))
  ok(`${name}: no spinner`, !html.includes("import-spinner"))
  ok(`${name}: the day list and log stay`, html.includes("import-day-list") && html.includes("Queue complete"))
}

console.log("import progress")
console.log(out.join("\n"))
if (failed) {
  console.error(`${failed} failed`)
  process.exit(1)
}
