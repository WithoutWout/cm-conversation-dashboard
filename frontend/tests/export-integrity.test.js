// End-to-end: build the real export rows, serialize them exactly as
// exportCollection() does, and scan the resulting BYTES for anything that
// should never have reached the file. Every excluded string below is unique and
// tagged, so a leak is unambiguous rather than a judgement call.
const vm = require("vm")
const { extract } = require("./extract")

const NAMES = [
  "_parseCollectionItemKey","_rowContextText","_prepareExclusionPatterns","_rowMatchingPatterns",
  "_articleExportRows","_colRouteIndex","_articlesRoutingIntoDialog","_dialogExportRows",
  "_mergeRowsByContent","_itemExportRows","_itemExportRowCount","_colBuildSignature",
  "_colExcludedItems","_colExcludedContent","_colDisabledFilters","_colEffectivePatterns",
  "buildCollectionExportRows","_buildCollectionExportRows","invalidateCollectionCaches",
  "articleAnswerHasContext","dialogAnswerHasContext","_defaultAnswerAmong",
  "defaultArticleAnswer","defaultDialogAnswerItem",
]
const ctx = vm.createContext({ console })
vm.runInContext(`
  let articleMap = new Map(), dialogMap = new Map(), ctxVarMap = new Map()
  let cmExportFilters = []
  let cmExportKeepUnreachable = false
  let _colRouteIndexCache = null
  const _colItemRowsCache = new Map(), _colBuildCache = new Map()
  const UNREACHABLE_REASON = "Not reachable: non-default response with no context"
  const MANUAL_ROW_REASON = "Removed by hand"
  const MANUAL_ITEM_REASON = "Removed by hand: whole item"
  function stripDisplay(t) { return (t || "").replace(/<[^>]*>/g, "").trim() }
  ${NAMES.map(extract).join("\n")}
  function setup(a, d, c, f) { articleMap = a; dialogMap = d; ctxVarMap = c; cmExportFilters = f; invalidateCollectionCaches() }
`, ctx)

// ── Fixture ──────────────────────────────────────────────────────────────────
// Every string that must NOT appear in the file is prefixed LEAK_<mechanism>.
const A = (Id, Questions, Outputs) => [Id, { Id, Questions, Outputs }]
const ans = (Text, extra) => Object.assign({ Type: "Answer", Text }, extra || {})

const articles = new Map([
  A(1, [{ Text: "openingstijden" }, { Text: "SHARED_ARTICLE_PHRASE" }], [
    ans("Wij zijn open van 10 tot 18 uur.", { IsDefault: true }),
    ans("LEAK_UNREACHABLE nondefault zonder context."),
    ans("LEAK_CTXANY nondefault met context any.", { ContextVariables: [{ Id: 9, Values: ["any"] }] }),
    ans("LEAK_PATTERN_CONTENT interne notitie.", { ContextVariables: [{ Id: 9, Values: ["winter"] }] }),
    ans("Reachable zomer-antwoord.", { ContextVariables: [{ Id: 9, Values: ["zomer"] }] }),
    ans("Antwoord met LEAK_PATTERN_CONTEXT erin.", { ContextVariables: [{ Id: 9, Values: ["geheim"] }] }),
  ]),
  // Every phrase on this Article matches an entity pattern, so nothing it makes
  // may appear — neither its content nor its trigger text.
  A(2, [{ Text: "LEAK_PATTERN_ENTITY parkeren" }], [
    ans("Parkeren kost 15 euro.", { IsDefault: true }),
  ]),
  // Hand-removed single response.
  A(3, [{ Text: "wifi" }], [
    ans("LEAK_HANDROW wifi is gratis in het park.", { IsDefault: true }),
    ans("Wifi werkt ook op de camping.", { ContextVariables: [{ Id: 9, Values: ["camping"] }] }),
  ]),
  // Whole item held out.
  A(4, [{ Text: "LEAK_HELDITEM_TRIGGER kluisjes" }], [
    ans("LEAK_HELDITEM kluisjes kosten 5 euro.", { IsDefault: true }),
  ]),
  // Shares its default content with Article 5 below, to prove merge cannot pull
  // an excluded row's phrases onto a surviving row.
  A(5, [{ Text: "LEAK_MERGE_PHRASE_FROM_HELD" }], [
    ans("Gedeelde tekst die wel geexporteerd wordt.", { IsDefault: true }),
  ]),
  A(6, [{ Text: "gedeeld" }], [
    ans("Gedeelde tekst die wel geexporteerd wordt.", { IsDefault: true }),
  ]),
])
const dialogs = new Map([
  [50, { id: 50, name: "D", nodes: [{ id: 100, output: { items: [
    { type: "Answer", data: { text: "Dialoog standaardantwoord." }, isDefault: true },
    { type: "Answer", data: { text: "LEAK_DLG_UNREACHABLE zonder context." } },
    { type: "Answer", data: { text: "LEAK_DLG_CTXANY met context any." }, contextVariables: [{ id: 9, value: "any" }] },
  ] } }] }],
])
// Article 7 routes into dialog 50 so the dialog has a trigger.
articles.set(7, { Id: 7, Questions: [{ Text: "dialoogvraag" }], Outputs: [
  { Type: "DialogStart", DialogId: 50, DialogStartNodeId: 100, IsDefault: true },
] })

const filters = [
  { id: "p1", field: "content", pattern: "LEAK_PATTERN_CONTENT", enabled: true },
  { id: "p2", field: "entity",  pattern: "LEAK_PATTERN_ENTITY",  enabled: true },
  { id: "p3", field: "context", pattern: "seizoen:geheim",       enabled: true },
  { id: "p4", field: "content", pattern: "NEVER_MATCHES_ANYTHING", enabled: false },
  { id: "p5", field: "content", pattern: "Reachable zomer",      enabled: true },
]
ctx.setup(articles, dialogs, new Map([[9, "seizoen"]]), filters)

const coll = {
  id: "x", name: "Export integrity",
  itemKeys: ["article:1","article:2","article:3","article:4","article:5","article:6","article:7","dialog:50"],
  excludedContent: ["LEAK_HANDROW wifi is gratis in het park."],
  excludedItemKeys: ["article:4", "article:5"],
  // p5 is globally enabled but switched off for THIS collection, so the zomer
  // answer must survive here — the mirror image of the leak checks.
  disabledFilterIds: ["p5"],
}

const { rows, excludedRows } = ctx.buildCollectionExportRows(coll)
const json = JSON.stringify(rows, null, 2)   // byte-for-byte what exportCollection writes

// ── Assertions ───────────────────────────────────────────────────────────────
const out = []
let failed = 0
const ok = (n, c) => { if (!c) failed++; out.push((c ? "  PASS  " : "  FAIL  ") + n) }

const MUST_NOT_APPEAR = [
  ["unreachable article response",          "LEAK_UNREACHABLE"],
  ["article response with context 'any'",   "LEAK_CTXANY"],
  ["unreachable dialog response",           "LEAK_DLG_UNREACHABLE"],
  ["dialog response with context 'any'",    "LEAK_DLG_CTXANY"],
  ["smart filter on content",               "LEAK_PATTERN_CONTENT"],
  ["smart filter on entity (content side)", "Parkeren kost 15 euro"],
  ["smart filter on entity (trigger side)", "LEAK_PATTERN_ENTITY"],
  ["smart filter on context",               "LEAK_PATTERN_CONTEXT"],
  ["hand-removed response",                 "LEAK_HANDROW"],
  ["held-out item content",                 "LEAK_HELDITEM"],
  ["held-out item trigger",                 "LEAK_HELDITEM_TRIGGER"],
  ["phrase merged in from a held-out item", "LEAK_MERGE_PHRASE_FROM_HELD"],
]
out.push("Scanning " + json.length + " bytes of export JSON (" + rows.length + " rows):")
for (const [label, needle] of MUST_NOT_APPEAR)
  ok("no leak — " + label, !json.includes(needle))

out.push("")
out.push("Things that MUST survive (so the checks above aren't passing vacuously):")
ok("default article response exported", json.includes("Wij zijn open van 10 tot 18 uur."))
ok("reachable contextual response exported", json.includes("Wifi werkt ook op de camping."))
ok("dialog response exported via its routing Article", json.includes("Dialoog standaardantwoord."))
ok("shared content exported once, from the item that isn't held out",
   json.includes("Gedeelde tekst die wel geexporteerd wordt.") &&
   rows.filter((r) => r.content === "Gedeelde tekst die wel geexporteerd wordt.").length === 1)
ok("collection-disabled filter did NOT remove its rows here", json.includes("Reachable zomer-antwoord."))
// An Article's Questions trigger every one of its responses, so a phrase is
// article-wide by definition — it survives as long as any row of that article
// does. Asserted so the scan above is not mistaken for the opposite rule.
ok("an Article phrase rides every surviving row of that Article", json.includes("SHARED_ARTICLE_PHRASE"))

out.push("")
out.push("File shape:")
ok("rows carry exactly {trigger, content} — no internal fields leak",
   rows.every((r) => Object.keys(r).sort().join(",") === "content,trigger"))
ok("no unreachable/manual/ctxText/phrases keys in the JSON",
   !/"(unreachable|manual|ctxText|phrases|itemKey|matchedFields)"/.test(json))
ok("every excluded row is accounted for with a reason",
   excludedRows.every((r) => Array.isArray(r.matchedFields) && r.matchedFields.length > 0))
ok("no row has empty content", rows.every((r) => r.content && r.content.trim()))

console.log(out.join("\n"))
console.log("")
console.log(failed ? failed + " FAILED" : "Export integrity: all checks passed")
if (!failed) {
  console.log("\n--- exported JSON ---")
  console.log(json)
}
process.exit(failed ? 1 : 0)
