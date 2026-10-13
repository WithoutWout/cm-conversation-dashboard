const { extract, extractConst } = require("./extract")
const vm = require("vm")

const NAMES = [
  "_parseCollectionItemKey","_ctxAdd","_rowContextText","_prepareExclusionPatterns","_rowMatchingPatterns",
  "_articleExportRows","_pushArticleAnswerRows","_dialogEntryNodeId","_nodeLandsOn","nodeIsRef","_recognitionPhrases","_dialogWaysIn","_pushNodeAnswerRows","_contextTrigger","_compactPaths","_textGrams","_entityInGrams","_dialogContentIsShared","_colRouteIndex","_articlesRoutingIntoDialog","_dialogExportRows",
  "_mergeRowsByContent","_itemExportRows","_itemExportRowCount","_colBuildSignature","_colExcludedItems","_colExcludedContent","_colDisabledFilters","_colEffectivePatterns",
  "buildCollectionExportRows","_buildCollectionExportRows","invalidateCollectionCaches",
  "articleAnswerHasContext","dialogAnswerHasContext","_defaultAnswerAmong",
  "defaultArticleAnswer","defaultDialogAnswerItem",
  "_cutSentences","_sentencePatterns","_handRemovedKey","_rowTrigger","_triggerTexts","_triggerTerms","_coverPhrases",
  "condenseEntityPhrases","_phraseEntityTags","_entityAt",
]
const ctx = vm.createContext({ console })
vm.runInContext(`
  let articleMap = new Map(), dialogMap = new Map(), ctxVarMap = new Map()
  let cmExportFilters = []
  let cmExportKeepUnreachable = false
  let cmCondenseEntities = true
  let cmTriggerStyle = "names"
  let entityMap = new Map(), entityWordMap = new Map()
  let _condenseCache = new Map(), _entityWordMaxTokens = 0
  ${extractConst("_SENTENCE_RE")}
  ${extractConst("_CONDENSE_EDGE_PUNCT")}
  const UNREACHABLE_REASON = "Not reachable: non-default response with no context"
  const MANUAL_ROW_REASON = "Removed by hand"
  const MANUAL_ITEM_REASON = "Removed by hand: whole item"
  let _colRouteIndexCache = null
  const _dialogNodesById = new WeakMap()
  let _colSharedContentCache = null
  const _entityGramWords = new WeakMap()
  let cmDialogContext = true, cmDialogContextCap = 30
  const DIALOG_CONTEXT_DEFAULT_CAP = 30, DIALOG_CONTEXT_MAX_STEPS = 3, DIALOG_CONTEXT_SHARED = 10
  const _colItemRowsCache = new Map()
  const _colBuildCache = new Map()
  // Not under test here: collapse HTML to its text, which is all the row needs.
  function stripDisplay(t) { return (t || "").replace(/<[^>]*>/g, "").trim() }
  ${NAMES.map(extract).join("\n")}
  function setData(a, d, c) { articleMap = a; dialogMap = d; ctxVarMap = c; invalidateCollectionCaches() }
  function setFilters(f) { cmExportFilters = f }
  function setKeep(k) { cmExportKeepUnreachable = k; invalidateCollectionCaches() }
  function setContext(on, cap) { cmDialogContext = on; cmDialogContextCap = cap || 30 }
  function setStyle(v) { cmTriggerStyle = v }
  function setEntities(list) {
    entityMap = new Map(list.map((e) => [e.name.toUpperCase(), e]))
    entityWordMap = new Map()
    for (const e of list) for (const w of e.words) entityWordMap.set(w.text.toLowerCase(), e)
    _condenseCache = new Map(); _entityWordMaxTokens = 0; invalidateCollectionCaches()
  }
  function getFilterCount() { return cmExportFilters.length }
  function articleById(id) { return articleMap.get(id) }
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


out.push("")
out.push("Reachability: an Article that routes into a Dialog (real shape of Article 4937):")
// Its default Output is a DialogStart, so NONE of its Answers is the default.
// All three carry OutputMetaData.escalationGroup and no ContextVariables, which
// is exactly the combination that used to make them look reachable twice over.
ctx.setData(
  new Map([[4937, { Id: 4937, Questions: [{ Text: "foutmelding reserveren" }], Outputs: [
    { Type: "DialogStart", DialogId: 4066, DialogStartNodeId: 1339, IsDefault: true, ContextVariables: [] },
    { Type: "Answer", Text: "Spiegeltje spiegeltje", IsDefault: false, ContextVariables: [],
      OutputMetaData: { escalationGroup: "attractiepark", hideFeedback: "true" } },
    { Type: "Answer", Text: "Reserveringssysteem vervangen", IsDefault: false, ContextVariables: [],
      OutputMetaData: { escalationGroup: "attractiepark", hideFeedback: "true" } },
    { Type: "Answer", Text: "Geen verblijf reserveren", IsDefault: false, ContextVariables: [],
      OutputMetaData: { escalationGroup: "attractiepark", hideFeedback: "true" } },
  ] }]]),
  new Map(), new Map(),
)
ctx.setFilters([]); ctx.setKeep(false)
const routing = { id: "r", name: "R", itemKeys: ["article:4937"] }
const rr = ctx.buildCollectionExportRows(routing)
ok("no Answer is promoted to default when the default Output is a DialogStart",
   ctx.defaultArticleAnswer(ctx.articleById(4937)) === null)
ok("escalationGroup metadata alone is NOT a context condition",
   ctx.articleAnswerHasContext({ ContextVariables: [], OutputMetaData: { escalationGroup: "attractiepark" } }) === false)
ok("a real escalationGroup condition still counts",
   ctx.articleAnswerHasContext({ ContextVariables: [{ Id: 5, Values: ["attractiepark"] }] }) === true)
ok("...but not when its value is 'any'",
   ctx.articleAnswerHasContext({ ContextVariables: [{ Id: 5, Values: ["any"] }] }) === false)
ok("all three responses are unreachable", rr.unreachableCount === 3)
ok("nothing from this article reaches the export", rr.rows.length === 0)

// The fallback survives only where nothing at all is flagged.
ok("first Answer is still the default when NO output is flagged",
   ctx._defaultAnswerAmong([{ a: 1 }, { a: 2 }], [{ a: 1 }, { a: 2 }], () => false).a === 1)

// Dialog side: same two rules.
ok("dialog metadata.escalationGroup alone is not a condition",
   ctx.dialogAnswerHasContext({ contextVariables: [], metadata: { escalationGroup: "attractiepark" } }) === false)
ok("dialog contextVariables condition still counts",
   ctx.dialogAnswerHasContext({ contextVariables: [{ id: 5, value: "attractiepark" }] }) === true)
ok("no dialog Answer is default when the default item is not an Answer",
   ctx.defaultDialogAnswerItem([
     { type: "DialogStart", isDefault: true },
     { type: "Answer", data: { text: "x" } },
   ]) === null)

out.push("")
out.push("Dialog Links enter through entryPoints; GoTo and Article references are followed:")
// Real shape of Dialog 6401: the Article's DialogStartNodeId 13580 is an entry
// point id, mapped to node 2. Node 3 is there to catch a lookup that still
// reads the entry id as a node id: here entry 3 maps to node 5, not node 3.
ctx.setData(
  new Map([
    [10, { Id: 10, Questions: [{ Text: "zwembad openingstijden" }], Outputs: [
      { Type: "DialogStart", DialogId: 6401, DialogStartNodeId: 13580, IsDefault: true },
    ] }],
    [11, { Id: 11, Questions: [{ Text: "badhuys" }], Outputs: [
      { Type: "DialogStart", DialogId: 6401, DialogStartNodeId: 3, IsDefault: true },
    ] }],
    [12, { Id: 12, Questions: [{ Text: "zwemkleding" }], Outputs: [
      { Type: "Answer", Text: "REFERENCED-DEFAULT", IsDefault: true },
      { Type: "Answer", Text: "REFERENCED-NOCTX" },
    ] }],
  ]),
  new Map([[6401, { id: 6401, name: "Badhuys", entryPoints: [{ id: 13580, nodeId: 2 }, { id: 3, nodeId: 5 }], nodes: [
    { id: 2, type: "Recognition", output: { items: [{ type: "Answer", data: { text: "ENTRY-NODE" }, isDefault: true }] },
      links: [
        { childNodeId: 6, condition: { type: "Recognition", data: { questions: [{ text: "Tickets" }] } } },
        { childNodeId: 8, condition: { type: "Recognition", data: { questions: [{ text: "Kleding" }] } } },
        { childNodeId: 9, condition: { type: "Recognition", data: { questions: [{ text: "Via goto" }] } } },
      ] },
    { id: 3, type: "Output", output: { items: [{ type: "Answer", data: { text: "WRONG-NODE" }, isDefault: true }] } },
    { id: 5, type: "Output", output: { items: [{ type: "Answer", data: { text: "SECOND-ENTRY" }, isDefault: true }] } },
    { id: 6, type: "GoTo", links: [{ childNodeId: 7, condition: { type: "GoTo" } }] },
    { id: 7, type: "Output", output: { items: [{ type: "Answer", data: { text: "BEHIND-GOTO" }, isDefault: true }] } },
    { id: 8, type: "Output", output: { kbaIdReference: 12, items: [] } },
    { id: 9, type: "GoTo", links: [{ childNodeId: 8, condition: { type: "GoTo" } }] },
  ] }]]),
  new Map(),
)
ctx.setFilters([]); ctx.setKeep(false)
// These pin *which* responses are exported; the trigger context is tested below.
ctx.setContext(false)
const ep = ctx.buildCollectionExportRows({ id: "ep", itemKeys: ["dialog:6401"] })
ctx.setContext(true)
const epRow = (s) => ep.rows.find((r) => r.content.includes(s))
ok("an entry point id is resolved to its node", !!epRow("ENTRY-NODE") && epRow("ENTRY-NODE").trigger.includes("zwembad openingstijden"))
ok("an entry id that is also some node's id still goes through entryPoints",
   !epRow("WRONG-NODE") && !!epRow("SECOND-ENTRY") && epRow("SECOND-ENTRY").trigger.includes("badhuys"))
ok("a Dialog without entryPoints keeps reading it as a node id",
   ctx._dialogEntryNodeId({ nodes: [] }, 100) === 100 && ctx._dialogEntryNodeId(null, 100) === 100)
ok("a link through a GoTo exports the node it lands on", !!epRow("BEHIND-GOTO") && epRow("BEHIND-GOTO").trigger === "Tickets")
ok("a link onto an Article reference exports that Article's Response",
   !!epRow("REFERENCED-DEFAULT") && epRow("REFERENCED-DEFAULT").trigger.split(" | ").sort().join(",") === "Kleding,Via goto")
ok("...the referenced Article's reachability rule still applies", !epRow("REFERENCED-NOCTX") && ep.unreachableCount === 2)
ok("the Article's own row is unchanged", ctx._itemExportRows("article:12").length === 2)


out.push("")
out.push("Dialog button context: a button-reached response carries the question that led in:")
// Modelled on "Afgeschreven/dubbel betaald": the Article leads into a question
// node, the information sits one button further.
ctx.setEntities([
  { name: "DUBBEL", words: [{ text: "dubbel" }] },
  { name: "BETALEN", words: [{ text: "betaald" }, { text: "betalen" }] },
  { name: "AFSCHRIJVEN", words: [{ text: "afgeschreven" }] },
  { name: "TICKET", words: [{ text: "ticket" }] },
])
const btn = (text, to) => ({ childNodeId: to, condition: { type: "Recognition", data: { questions: [{ text }] } } })
const ans = (text) => ({ items: [{ type: "Answer", data: { text }, isDefault: true }] })
const dubbelDialog = (extraArticles) => ctx.setData(
  new Map([
    [20, { Id: 20, Questions: [{ Text: "dubbel betaald" }, { Text: "dubbel afgeschreven" }, { Text: "twee keer betaald" }], Outputs: [
      { Type: "DialogStart", DialogId: 70, DialogStartNodeId: 900, IsDefault: true },
    ] }],
    ...(extraArticles || []),
  ]),
  new Map([[70, { id: 70, name: "Afgeschreven/dubbel betaald", entryPoints: [{ id: 900, nodeId: 1 }, { id: 901, nodeId: 2 }], nodes: [
    { id: 1, type: "Output", output: ans("ENTRY Waar gaat het om?"), links: [btn("Tickets", 2), btn("Abonnement", 3)] },
    { id: 2, type: "Output", output: ans("Is het bedrag van je tickets 2 keer afgeschreven?"), links: [btn("Ja, graag", 4)] },
    { id: 3, type: "Output", output: ans("ABONNEMENT-DUBBEL") },
    { id: 4, type: "Output", output: ans("COLLEGA") },
    { id: 6, type: "Output", output: ans("NOBODY-GETS-HERE"), links: [btn("Los", 5)] },
    { id: 5, type: "Output", output: ans("ORPHAN") },
  ] }]]),
  new Map(),
)
dubbelDialog()
ctx.setFilters([]); ctx.setKeep(false); ctx.setContext(true)
const dd = () => ctx.buildCollectionExportRows({ id: "dd", itemKeys: ["dialog:70"] })
const trig = (r, s) => r.rows.find((x) => x.content.includes(s)).trigger
// Today's plain trigger is condensed, which lower-cases a word no Entity claims.
const plainIs = (t, p) => t.toLowerCase() === p.toLowerCase()
let cr = dd()
const t2 = trig(cr, "tickets 2 keer").split(" | ")
ok("a button-reached response carries the leading Article's Entities", t2.includes("DUBBEL") && t2.includes("BETALEN"))
ok("...and the button as its own path line, not a bare phrase", t2[t2.length - 1] === "→ Tickets" && !t2.includes("Tickets"))
ok("an Entity found in the response comes first", t2[0] === "AFSCHRIJVEN")
ok("leftover words come after every Entity", t2.indexOf("twee") > t2.indexOf("BETALEN") && t2.indexOf("keer") > t2.indexOf("BETALEN"))
ok("a deeper response carries the whole run of buttons", trig(cr, "COLLEGA").endsWith("→ Tickets → Ja, graag"))
ok("a response no Article leads to keeps its plain button trigger", plainIs(trig(cr, "ORPHAN"), "Los"))
ok("the response an Article enters on keeps today's trigger", !trig(cr, "ENTRY").includes("→"))
ok("the build reports how many rows got context", cr.contextRows === 3 && cr.contextCapped === 0)

ctx.setContext(false)
ok("with the setting off, the button alone is the trigger again", plainIs(trig(dd(), "tickets 2 keer"), "Tickets"))
ctx.setContext(true, 2)
cr = dd()
ok("the cap limits the context terms, in rank order",
   trig(cr, "tickets 2 keer") === "AFSCHRIJVEN | DUBBEL | → Tickets" && cr.contextCapped === 3)
ctx.setContext(true)

ctx.setFilters([{ id: "fe", field: "entity", pattern: "dubbel", enabled: true }])
ok("an Entity smart filter still tests the button phrases only", dd().rows.some((r) => r.content.includes("tickets 2 keer")))
ctx.setFilters([])

// An Article routing straight into the same node: its phrases are the row's
// own, and the cap never removes them.
dubbelDialog([[21, { Id: 21, Questions: [{ Text: "ticket dubbel betaald" }], Outputs: [
  { Type: "DialogStart", DialogId: 70, DialogStartNodeId: 901, IsDefault: true },
] }]])
ctx.setContext(true, 1)
const own = trig(dd(), "tickets 2 keer").split(" | ")
ok("the row's own Article Entities survive any cap", ["TICKET", "DUBBEL", "BETALEN"].every((t) => own.includes(t)))
ok("...while context beyond the cap is dropped", !own.includes("AFSCHRIJVEN") && own.includes("→ Tickets"))
ctx.setContext(true)

// A route the reachability rule rejects is not a way in.
ctx.setData(
  new Map([[22, { Id: 22, Questions: [{ Text: "dubbel betaald" }], Outputs: [
    { Type: "Answer", Text: "ART-22", IsDefault: true },
    { Type: "DialogStart", DialogId: 71, DialogStartNodeId: 1 },
  ] }]]),
  new Map([[71, { id: 71, nodes: [
    { id: 1, output: ans("START"), links: [btn("Tickets", 2)] },
    { id: 2, output: ans("BEHIND-UNREACHABLE-ROUTE") },
  ] }]]),
  new Map(),
)
ok("an unreachable route adds no context",
   plainIs(trig(ctx.buildCollectionExportRows({ id: "ur", itemKeys: ["dialog:71"] }), "BEHIND-UNREACHABLE"), "Tickets"))

// A reply reached from 10 or more Dialogs keeps its buttons only.
const sharedData = (n) => {
  const arts = new Map(), dls = new Map()
  for (let i = 0; i < n; i++) {
    arts.set(100 + i, { Id: 100 + i, Questions: [{ Text: "onderwerp" + i }], Outputs: [
      { Type: "DialogStart", DialogId: 200 + i, DialogStartNodeId: 1, IsDefault: true },
    ] })
    dls.set(200 + i, { id: 200 + i, nodes: [
      { id: 1, output: ans("Q" + i), links: [btn("Nee", 2)] },
      { id: 2, output: ans("SHARED-REPLY") },
    ] })
  }
  ctx.setData(arts, dls, new Map())
  return ctx.buildCollectionExportRows({ id: "sh" + n, itemKeys: [...dls.keys()].map((id) => "dialog:" + id) })
}
ok("a reply reached from 10 Dialogs keeps its buttons only", plainIs(trig(sharedData(10), "SHARED-REPLY"), "Nee"))
ok("...from 9 it still gets context", trig(sharedData(9), "SHARED-REPLY").includes("→ Nee"))

// Each button's phrases are written once per row; later it is named by its
// first phrase. The real case: the Monorail reply repeated
// "Monorail / Volk van Laaf" eight times.
const mono = [["Een andere dag", "morgen"], ["Monorail", "Volk van Laaf"]]
const cp = ctx._compactPaths([mono, [["Monorail", "Volk van Laaf"]], mono, [["Ja, graag", "Morgen"], ["Monorail", "Volk van Laaf"]]])
ok("a button repeated across routes is written in full once", cp.filter((l) => l.includes("Volk van Laaf")).length === 1)
ok("...and by its first phrase after that", cp[1] === "→ Monorail" && cp[2] === "→ Ja, graag / Morgen → Monorail")
ok("an identical route is written once, not twice in two forms", cp.length === 3)
// [J / G] seen, so route 2 compacts to "→ J" — the very line route 3 writes.
ok("two routes that compact to the same line are written once",
   JSON.stringify(ctx._compactPaths([[["J", "G"], ["X"]], [["J", "G"]], [["J"]]])) === JSON.stringify(["→ J / G → X", "→ J"]))
// End to end: two Articles reach one response by routes sharing their last button.
ctx.setData(
  new Map([
    [30, { Id: 30, Questions: [{ Text: "monorail" }], Outputs: [{ Type: "DialogStart", DialogId: 80, DialogStartNodeId: 1, IsDefault: true }] }],
    [31, { Id: 31, Questions: [{ Text: "attractie dicht" }], Outputs: [{ Type: "DialogStart", DialogId: 80, DialogStartNodeId: 2, IsDefault: true }] }],
  ]),
  new Map([[80, { id: 80, nodes: [
    { id: 1, output: ans("WELKE DAG"), links: [{ childNodeId: 2, condition: { type: "Recognition", data: { questions: [{ text: "Een andere dag" }, { text: "morgen" }] } } }] },
    { id: 2, output: ans("WELKE ATTRACTIE"), links: [{ childNodeId: 3, condition: { type: "Recognition", data: { questions: [{ text: "Monorail" }, { text: "Volk van Laaf" }] } } }] },
    { id: 3, output: ans("MONORAIL-OPEN") },
  ] }]]),
  new Map(),
)
const mt = trig(ctx.buildCollectionExportRows({ id: "mono", itemKeys: ["dialog:80"] }), "MONORAIL-OPEN").split(" | ")
ok("in an export row the shared last button is written in full once",
   mt.includes("→ Een andere dag / morgen → Monorail / Volk van Laaf") && mt.includes("→ Monorail"))
ctx.setEntities([])

out.push("")
out.push("Trigger styles:")
// Migration: a stored style wins; condensing switched off meant every phrase
// verbatim, so it stays that; everyone else gets phrases.
{
  const store = new Map()
  const lctx = vm.createContext({ localStorage: { getItem: (k) => (store.has(k) ? store.get(k) : null) } })
  vm.runInContext(extractConst("TRIGGER_STYLES") + "\n" + extract("loadTriggerStyle"), lctx)
  const load = () => vm.runInContext("loadTriggerStyle()", lctx)
  ok("a fresh install writes phrases", load() === "phrases")
  store.set("cm-condense-entities", "1")
  ok("condensing on (the old default) moves to phrases", load() === "phrases")
  store.set("cm-condense-entities", "0")
  ok("condensing off keeps exporting every phrase", load() === "all")
  store.set("cm-trigger-style", "words")
  ok("a chosen style wins", load() === "words")
  store.set("cm-trigger-style", "bogus")
  ok("an unknown stored style falls back", load() === "all")
}
// The cap counts Entities in every style: a context phrase costs the Entities
// it adds. Reuses the "Afgeschreven/dubbel betaald" fixture.
ctx.setEntities([
  { name: "DUBBEL", words: [{ text: "dubbel" }] },
  { name: "BETALEN", words: [{ text: "betaald" }, { text: "betalen" }] },
  { name: "AFSCHRIJVEN", words: [{ text: "afgeschreven" }] },
])
dubbelDialog()
ctx.setFilters([]); ctx.setKeep(false)
ctx.setStyle("phrases"); ctx.setContext(true)
let ps = trig(dd(), "tickets 2 keer").split(" | ")
// Cover of "dubbel betaald | dubbel afgeschreven | twee keer betaald": the last
// holds 3 keys, then "dubbel afgeschreven" adds DUBBEL and AFSCHRIJVEN, and
// "dubbel betaald" adds nothing. AFSCHRIJVEN is in the response, so it ranks first.
ok("phrases: a button-reached row carries the leading Article's cover phrases",
   JSON.stringify(ps) === JSON.stringify(["dubbel afgeschreven", "twee keer betaald", "→ Tickets"]))
ok("phrases: a phrase that adds nothing is not written", !ps.includes("dubbel betaald"))
ctx.setContext(true, 2)
ps = trig(dd(), "tickets 2 keer").split(" | ")
// "twee keer betaald" was chosen first by the cover (3 keys) but ranks second;
// once "dubbel afgeschreven" is kept it would still add 3, which no longer fits.
ok("phrases: the cap counts Entities, so a 2-Entity phrase fills a cap of 2",
   ps.filter((p) => !p.startsWith("→")).length === 1 && ps.includes("→ Tickets"))
// "dubbel afgeschreven" comes second in the cover, where it adds only
// AFSCHRIJVEN; ranked first (AFSCHRIJVEN is in the response) it adds DUBBEL too.
dubbelDialog([[20, { Id: 20, Questions: [{ Text: "twee keer dubbel betaald" }, { Text: "dubbel afgeschreven" }], Outputs: [
  { Type: "DialogStart", DialogId: 70, DialogStartNodeId: 900, IsDefault: true },
] }]])
ctx.setContext(true, 1)
ok("phrases: a phrase is charged what it adds where it is kept, not where the cover chose it",
   trig(dd(), "tickets 2 keer") === "→ Tickets")
ctx.setContext(true, 2)
ok("...and fits once the cap allows both Entities", trig(dd(), "tickets 2 keer") === "dubbel afgeschreven | → Tickets")
dubbelDialog()
ctx.setContext(true)
ctx.setStyle("words")
ps = trig(dd(), "tickets 2 keer").split(" | ")
ok("words: context is written as user words", ps.includes("afgeschreven") && ps.includes("dubbel") && !ps.includes("AFSCHRIJVEN"))
ctx.setStyle("all")
ps = trig(dd(), "tickets 2 keer").split(" | ")
ok("all: context is every leading phrase", ["dubbel betaald", "dubbel afgeschreven", "twee keer betaald"].every((p) => ps.includes(p)))
ctx.setStyle("names")
ctx.setEntities([])

console.log(out.join("\n"))
console.log(failed ? "\n" + failed + " FAILED" : "\nAll checks passed")
process.exit(failed ? 1 : 0)
