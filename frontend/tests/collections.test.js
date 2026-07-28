const { extract } = require("./extract")
const vm = require("vm")

const NAMES = [
  "_parseCollectionItemKey","_rowContextText","_prepareExclusionPatterns","_rowMatchingPatterns",
  "_articleExportRows","_colRouteIndex","_articlesRoutingIntoDialog","_dialogExportRows",
  "_mergeRowsByContent","_itemExportRows","_itemExportRowCount","_colBuildSignature","_colExcludedItems","_colExcludedContent","_colDisabledFilters","_colEffectivePatterns",
  "buildCollectionExportRows","_buildCollectionExportRows","invalidateCollectionCaches",
  "articleAnswerHasContext","dialogAnswerHasContext","defaultArticleAnswer","defaultDialogAnswerItem",
]
const ctx = vm.createContext({ console })
vm.runInContext(`
  let articleMap = new Map(), dialogMap = new Map(), ctxVarMap = new Map()
  let cmExportFilters = []
  let cmExportKeepUnreachable = false
  const UNREACHABLE_REASON = "Not reachable: non-default response with no context"
  const MANUAL_ROW_REASON = "Removed by hand"
  const MANUAL_ITEM_REASON = "Removed by hand: whole item"
  let _colRouteIndexCache = null
  const _colItemRowsCache = new Map()
  const _colBuildCache = new Map()
  // Not under test here: collapse HTML to its text, which is all the row needs.
  function stripDisplay(t) { return (t || "").replace(/<[^>]*>/g, "").trim() }
  ${NAMES.map(extract).join("\n")}
  function setData(a, d, c) { articleMap = a; dialogMap = d; ctxVarMap = c; invalidateCollectionCaches() }
  function setFilters(f) { cmExportFilters = f }
  function setKeep(k) { cmExportKeepUnreachable = k; invalidateCollectionCaches() }
  function getFilterCount() { return cmExportFilters.length }
`, ctx)

const cmExportFiltersLength = () => ctx.getFilterCount()
const out = []
let failed = 0
const ok = (n, c) => { if (!c) failed++; out.push((c ? "  PASS  " : "  FAIL  ") + n) }

const seed = () => ctx.setData(
  new Map([
    [1, { Id: 1, Questions: [{ Text: "openingstijden" }, { Text: "hoe laat open" }], Outputs: [
      { Type: "Answer", Text: "Open 10:00 - 18:00", IsDefault: true },
      { Type: "Answer", Text: "Winter gesloten", ContextVariables: [{ Id: 9, Values: ["winter"] }] },
      { Type: "Answer", Text: "NONDEFAULT-NOCTX" },
      { Type: "Answer", Text: "ARTICLE-CTX-ANY", ContextVariables: [{ Id: 9, Values: ["any"] }] },
    ] }],
    [2, { Id: 2, Questions: [{ Text: "klacht indienen" }], Outputs: [
      { Type: "DialogStart", DialogId: 50, DialogStartNodeId: 100, IsDefault: true },
    ] }],
  ]),
  new Map([
    [50, { id: 50, name: "Klachten", nodes: [
      { id: 100, type: "Output", output: { items: [
        { type: "Answer", data: { text: "Vul het formulier in" }, isDefault: true },
        { type: "Answer", data: { text: "DIALOG-CTX-ANY" }, contextVariables: [{ id: 9, value: "any" }] },
        { type: "Answer", data: { text: "DIALOG-NONDEFAULT-NOCTX" } },
      ] } },
    ] }],
  ]),
  new Map([[9, "seizoen"]]),
)
seed()
ctx.setFilters([])

const coll = { id: "c1", name: "T", itemKeys: ["article:1", "dialog:50"] }
const r1 = ctx.buildCollectionExportRows(coll)
out.push("Rows produced:")
r1.rows.forEach((r) => out.push("    " + r.trigger + "  =>  " + r.content))
out.push("")

const has = (s) => r1.rows.some((r) => r.content.includes(s))
ok("default article answer exported", has("Open 10:00 - 18:00"))
ok("non-default WITH context exported", has("Winter gesloten"))
ok("non-default, NO context dropped (article)", !has("NONDEFAULT-NOCTX"))
ok("non-default, context='any' dropped (article)", !has("ARTICLE-CTX-ANY"))
ok("dialog default answer exported via routing Article", has("Vul het formulier in"))
ok("non-default, NO context dropped (dialog)", !has("DIALOG-NONDEFAULT-NOCTX"))
ok("non-default, context='any' dropped (dialog) <- asymmetry FIXED", !has("DIALOG-CTX-ANY"))
ok("routing Article contributes 0 rows of its own", ctx._itemExportRowCount("article:2") === 0)
ok("dialog trigger comes from the routing Article", r1.rows.some((r) => r.trigger.includes("klacht indienen")))

out.push("")
ok("build memoized (same object)", ctx.buildCollectionExportRows(coll) === r1)
ok("item rows memoized", ctx._itemExportRows("dialog:50") === ctx._itemExportRows("dialog:50"))
ok("route index memoized", ctx._colRouteIndex() === ctx._colRouteIndex())
ok("route index resolves dialog 50", ctx._articlesRoutingIntoDialog(50).length === 1)
ok("unknown dialog resolves to []", ctx._articlesRoutingIntoDialog(999).length === 0)
ok("stale key yields no rows", ctx._itemExportRowCount("article:9999") === 0)
ok("malformed key yields no rows", ctx._itemExportRowCount("nonsense") === 0)

ctx.setFilters([{ id: "f1", field: "content", pattern: "formulier", enabled: true }])
const r2 = ctx.buildCollectionExportRows(coll)
ok("enabling a filter busts the cache", r2 !== r1)
const patternDrops = (r) => r.excludedRows.filter((x) => !x.unreachable)
ok("filter excludes the row", patternDrops(r2).length === 1 && !r2.rows.some((r) => r.content.includes("formulier")))
ok("excluded row names its pattern", patternDrops(r2)[0].matchedFields[0] === "content: formulier")

coll.itemKeys = ["article:1"]
const r3 = ctx.buildCollectionExportRows(coll)
ok("changing itemKeys busts the cache", r3 !== r2 && r3.totalCandidates < r2.totalCandidates)

ctx.setFilters([
  { id: "a", field: "entity", pattern: "^openings", isRegex: true, enabled: true },
  { id: "b", field: "content", pattern: "10:00", enabled: false },
  { id: "c", field: "entity", pattern: "[", isRegex: true, enabled: true },
])
const r4 = ctx.buildCollectionExportRows(coll)
ok("regex applies / disabled ignored / bad regex inert", patternDrops(r4).length === 2 && r4.rows.length === 0)

ctx.setFilters([{ id: "d", field: "context", pattern: "seizoen:winter", enabled: true }])
const r5 = ctx.buildCollectionExportRows(coll)
ok("context field matches the flattened ctx string", patternDrops(r5).length === 1)

ctx.setFilters([])
coll.itemKeys = ["article:1", "dialog:50"]
const before = ctx.buildCollectionExportRows(coll)
ctx.setData(new Map(), new Map(), new Map())
const after = ctx.buildCollectionExportRows(coll)
ok("data reload invalidates everything derived", after !== before && after.rows.length === 0)


out.push("")
out.push("Reachability rule as a setting:")
seed()
ctx.setFilters([])
coll.itemKeys = ["article:1", "dialog:50"]
ctx.setKeep(false)
const on = ctx.buildCollectionExportRows(coll)
ok("rule ON: unreachable rows are reported, not silent", on.unreachableCount === 4)
ok("rule ON: they appear in excludedRows", on.excludedRows.length === 4 && on.excludedRows.every((r) => r.unreachable))
ok("rule ON: each names the reason", on.excludedRows[0].matchedFields[0].startsWith("Not reachable"))
ok("rule ON: they are not exported", !on.rows.some((r) => /NONDEFAULT-NOCTX|CTX-ANY/.test(r.content)))

ctx.setKeep(true)
const off = ctx.buildCollectionExportRows(coll)
ok("rule OFF: unreachable rows are exported", off.rows.some((r) => r.content.includes("NONDEFAULT-NOCTX")) && off.rows.some((r) => r.content.includes("ARTICLE-CTX-ANY")) && off.rows.some((r) => r.content.includes("DIALOG-CTX-ANY")))
ok("rule OFF: nothing is excluded", off.excludedCount === 0)
ok("rule OFF: count still reported for the impact readout", off.unreachableCount === 4)
ok("rule OFF: item row count grows", ctx._itemExportRowCount("article:1") === 4)
ctx.setKeep(false)
ok("rule ON: item row count shrinks", ctx._itemExportRowCount("article:1") === 2)

// A response behind an unreachable route is itself unreachable.
ctx.setData(
  new Map([[7, { Id: 7, Questions: [{ Text: "route" }], Outputs: [
    { Type: "DialogStart", DialogId: 60, DialogStartNodeId: 999, IsDefault: true },
    { Type: "DialogStart", DialogId: 60, DialogStartNodeId: 200 },
  ] }]]),
  new Map([[60, { id: 60, name: "D", nodes: [
    { id: 200, output: { items: [{ type: "Answer", data: { text: "BEHIND-BAD-ROUTE" }, isDefault: true }] } },
  ] }]]),
  new Map(),
)
ctx.setFilters([])
const routed = ctx.buildCollectionExportRows({ id: "c2", itemKeys: ["dialog:60"] })
ok("unreachable route taints the rows behind it", routed.rows.length === 0 && routed.unreachableCount === 1)
ctx.setKeep(true)
ok("...and rule OFF exports them again", ctx.buildCollectionExportRows({ id: "c3", itemKeys: ["dialog:60"] }).rows.length === 1)
ctx.setKeep(false)


out.push("")
out.push("Hand-curation and per-collection filters:")
seed()
ctx.setFilters([])
ctx.setKeep(false)

// Remove one response by hand.
const cur = { id: "cur", name: "Cur", itemKeys: ["article:1", "dialog:50"] }
const base = ctx.buildCollectionExportRows(cur)
const target = base.rows.find((r) => r.content.includes("Winter gesloten")).content
cur.excludedContent = [target]
const afterRow = ctx.buildCollectionExportRows(cur)
ok("hand-removed response leaves the export", !afterRow.rows.some((r) => r.content === target))
ok("hand-removed response is listed as such", afterRow.excludedRows.some((r) => r.manual && r.content === target && r.matchedFields[0] === "Removed by hand"))
ok("hand-removal is counted", afterRow.manualCount === 1)
ok("restoring it puts it back", (() => { cur.excludedContent = []; return ctx.buildCollectionExportRows(cur).rows.some((r) => r.content === target) })())

// Hold out a whole item.
cur.excludedItemKeys = ["article:1"]
const heldOut = ctx.buildCollectionExportRows(cur)
ok("held-out item contributes nothing", !heldOut.rows.some((r) => r.trigger.includes("openingstijden")))
ok("its rows are listed as a whole-item removal", heldOut.excludedRows.every((r) => !r.trigger.includes("openingstijden") || (r.manual && r.itemKey === "article:1")))
ok("held-out item reports 0 rows in the Items tab", ctx._itemExportRowCount("article:1", cur) === 0)
ok("...but still reports rows outside the collection", ctx._itemExportRowCount("article:1") === 2)

// The sticky requirement: removing and re-adding the item must not resurrect it.
cur.itemKeys = ["dialog:50"]
cur.itemKeys = ["dialog:50", "article:1"]
ok("re-adding a held-out item keeps it held out", !ctx.buildCollectionExportRows(cur).rows.some((r) => r.trigger.includes("openingstijden")))
cur.excludedItemKeys = []
ok("restoring the item brings it back", ctx.buildCollectionExportRows(cur).rows.some((r) => r.trigger.includes("openingstijden")))

// A hand-removed response survives its item being removed and re-added too.
cur.excludedContent = [target]
cur.itemKeys = ["dialog:50"]
cur.itemKeys = ["dialog:50", "article:1"]
ok("re-adding an item keeps its hand-removed response out", !ctx.buildCollectionExportRows(cur).rows.some((r) => r.content === target))
cur.excludedContent = []

// Per-collection smart filter switching.
ctx.setFilters([
  { id: "g1", field: "content", pattern: "formulier", enabled: true },
  { id: "g2", field: "entity", pattern: "openingstijden", enabled: true },
])
const bothOn = ctx.buildCollectionExportRows(cur)
ok("both global filters apply by default", ctx._colEffectivePatterns(cur).length === 2)
cur.disabledFilterIds = ["g2"]
const oneOff = ctx.buildCollectionExportRows(cur)
ok("a filter switched off for this collection stops applying", ctx._colEffectivePatterns(cur).length === 1 && oneOff.rows.length > bothOn.rows.length)
ok("switching it off busts the cache", oneOff !== bothOn)
ok("the global filter list is untouched", cmExportFiltersLength() === 2)
cur.disabledFilterIds = []
ok("switching it back on restores the old result", ctx.buildCollectionExportRows(cur).rows.length === bothOn.rows.length)

console.log(out.join("\n"))
console.log(failed ? "\n" + failed + " FAILED" : "\nAll checks passed")
process.exit(failed ? 1 : 0)
