// Share Content lists its items by number in every view — Articles, then
// Dialogs, then Transactional Dialogs, each by id — not in the order the search
// ranked them. The real `getExportItemsForCurrentView` is run, because it is
// what both the rows on screen and every copy action read.
const assert = require("assert")
const vm = require("vm")
const { extract } = require("./extract")

const ctx = vm.createContext({})
vm.runInContext(
  `
  let _exportFilter = ""
  let _exportView = "list"
  const _exportDropped = new Set()
  let ITEMS = []
  function getActiveExportItems() { return ITEMS }
  function _exportItemMatches() { return true }
  ${["_exportKey", "exportGroupOrder", "groupedExportItems", "getExportItemsForCurrentView"].map(extract).join("\n")}
  globalThis.run = (items, view) => { ITEMS = items; _exportView = view; return getExportItemsForCurrentView() }
  `,
  ctx,
)

// Search rank order, deliberately scrambled across kinds and numbers.
const ranked = [
  { kind: "dialog", id: 40 },
  { kind: "article", id: 900 },
  { kind: "tdialog", id: 3 },
  { kind: "article", id: 12 },
  { kind: "dialog", id: 7 },
]
const want = ["article:12", "article:900", "dialog:7", "dialog:40", "tdialog:3"]
let failed = 0
for (const view of ["list", "grouped", "table"]) {
  const got = ctx.run(ranked, view).map((i) => i.kind + ":" + i.id)
  try {
    assert.deepStrictEqual(got, want)
    console.log("  PASS  " + view + " is in number order")
  } catch (e) {
    failed++
    console.log("  FAIL  " + view + ": " + got.join(", "))
  }
}
assert.deepStrictEqual(ranked.map((i) => i.id), [40, 900, 3, 12, 7], "the tab's own list was reordered in place")
if (failed) {
  console.error("Share Content order: " + failed + " check(s) failed")
  process.exit(1)
}
console.log("Share Content order: all checks passed")
