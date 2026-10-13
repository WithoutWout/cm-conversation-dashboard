// End-to-end: build the real export rows, serialize them exactly as
// exportCollection() does, and scan the resulting BYTES for anything that
// should never have reached the file. Every excluded string below is unique and
// tagged, so a leak is unambiguous rather than a judgement call.
const vm = require("vm")
const { extract, extractConst } = require("./extract")

const NAMES = [
  "_parseCollectionItemKey","_ctxAdd","_rowContextText","_prepareExclusionPatterns","_rowMatchingPatterns",
  "_articleExportRows","_pushArticleAnswerRows","_dialogEntryNodeId","_nodeLandsOn","nodeIsRef","_recognitionPhrases","_dialogWaysIn","_pushNodeAnswerRows","_contextTrigger","_textGrams","_entityInGrams","_dialogContentIsShared","_colRouteIndex","_articlesRoutingIntoDialog","_dialogExportRows",
  "_mergeRowsByContent","_itemExportRows","_itemExportRowCount","_colBuildSignature",
  "_colExcludedItems","_colExcludedContent","_colDisabledFilters","_colEffectivePatterns",
  "buildCollectionExportRows","_buildCollectionExportRows","invalidateCollectionCaches",
  "articleAnswerHasContext","dialogAnswerHasContext","_defaultAnswerAmong",
  "defaultArticleAnswer","defaultDialogAnswerItem",
  "_cutSentences","_sentencePatterns","_handRemovedKey","_rowTrigger","_triggerTexts",
  "condenseEntityPhrases","_phraseEntityTags","_entityAt",
]
const ctx = vm.createContext({ console })
vm.runInContext(`
  let articleMap = new Map(), dialogMap = new Map(), ctxVarMap = new Map()
  let cmExportFilters = []
  let cmExportKeepUnreachable = false
  let cmCondenseEntities = true
  let entityMap = new Map(), entityWordMap = new Map()
  let _condenseCache = new Map(), _entityWordMaxTokens = 0
  ${extractConst("_SENTENCE_RE")}
  ${extractConst("_CONDENSE_EDGE_PUNCT")}
  let _colRouteIndexCache = null
  const _dialogNodesById = new WeakMap()
  let _colSharedContentCache = null
  const _entityGramWords = new WeakMap()
  let cmDialogContext = true, cmDialogContextCap = 30
  const DIALOG_CONTEXT_DEFAULT_CAP = 30, DIALOG_CONTEXT_MAX_STEPS = 3, DIALOG_CONTEXT_SHARED = 10
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

// Sentence filters: cut a sentence, keep the rest of the response.
articles.set(8, { Id: 8, Questions: [{ Text: "plattegrond" }], Outputs: [
  // Flat, as stripDisplay leaves it: the placeholder runs into the next sentence.
  ans("Kijk op de kaart. [Image] Bekijk LEAK_SENTENCE_ANNOUNCE de afbeelding hieronder. Het park opent om 10.30 uur.", { IsDefault: true }),
] })
// Nothing but an image: every sentence is cut, so the row has nothing left.
articles.set(9, { Id: 9, Questions: [{ Text: "LEAK_EMPTIED_TRIGGER foto" }], Outputs: [
  ans("[Image] Bekijk LEAK_EMPTIED_IMG de afbeelding.", { IsDefault: true }),
] })
// Identical once cut, so they merge into one row.
articles.set(10, { Id: 10, Questions: [{ Text: "knip een" }], Outputs: [
  ans("Gedeeld na knippen. LEAK_CUTMERGE zie afbeelding.", { IsDefault: true }),
] })
articles.set(11, { Id: 11, Questions: [{ Text: "knip twee" }], Outputs: [
  ans("Gedeeld na knippen.", { IsDefault: true }),
] })
// Hand-removed by the text the preview showed — i.e. after the cut.
articles.set(12, { Id: 12, Questions: [{ Text: "handtrim" }], Outputs: [
  ans("LEAK_HANDTRIM bewaar dit. Zie de afbeelding.", { IsDefault: true }),
] })
// A sentence filter matching a trigger phrase must not drop the row: it is
// not an Entity filter.
articles.set(13, { Id: 13, Questions: [{ Text: "afbeelding vraag" }], Outputs: [
  ans("Antwoord over een vraag met een trefwoord.", { IsDefault: true }),
] })

const filters = [
  { id: "p1", field: "content", pattern: "LEAK_PATTERN_CONTENT", enabled: true },
  { id: "p2", field: "entity",  pattern: "LEAK_PATTERN_ENTITY",  enabled: true },
  { id: "p3", field: "context", pattern: "seizoen:geheim",       enabled: true },
  { id: "p4", field: "content", pattern: "NEVER_MATCHES_ANYTHING", enabled: false },
  { id: "p5", field: "content", pattern: "Reachable zomer",      enabled: true },
  { id: "s1", field: "sentence", pattern: "[Image]",              enabled: true },
  { id: "s2", field: "sentence", pattern: "afbeelding",           enabled: true },
]
ctx.setup(articles, dialogs, new Map([[9, "seizoen"]]), filters)

const coll = {
  id: "x", name: "Export integrity",
  itemKeys: ["article:1","article:2","article:3","article:4","article:5","article:6","article:7","dialog:50",
    "article:8","article:9","article:10","article:11","article:12","article:13"],
  excludedContent: ["LEAK_HANDROW wifi is gratis in het park.", "LEAK_HANDTRIM bewaar dit."],
  excludedItemKeys: ["article:4", "article:5"],
  // p5 is globally enabled but switched off for THIS collection, so the zomer
  // answer must survive here — the mirror image of the leak checks.
  disabledFilterIds: ["p5"],
}

const { rows, excludedRows, cutSentences, cutByContent } = ctx.buildCollectionExportRows(coll)
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
  ["image placeholder (sentence filter)",   "[Image]"],
  ["sentence announcing the image",         "LEAK_SENTENCE_ANNOUNCE"],
  ["response emptied by sentence filters",  "LEAK_EMPTIED_IMG"],
  ["trigger of an emptied response",        "LEAK_EMPTIED_TRIGGER"],
  ["cut sentence of a merged row",          "LEAK_CUTMERGE"],
  ["hand-removed after a cut",              "LEAK_HANDTRIM"],
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

ok("a cut response keeps its other sentences",
   rows.some((r) => r.content === "Kijk op de kaart. Het park opent om 10.30 uur."))
ok("responses identical after the cut merge into one row",
   rows.filter((r) => r.content === "Gedeeld na knippen.").length === 1 &&
   rows.find((r) => r.content === "Gedeeld na knippen.").trigger === "knip een | knip twee")
ok("a sentence filter never drops a row on its trigger phrases",
   json.includes("Antwoord over een vraag met een trefwoord."))
ok("the emptied response is reported, with its sentence filter as the reason",
   excludedRows.some((r) => r.emptied && r.matchedFields.includes("sentence: [Image]")))
ok("every cut sentence is reported with its pattern",
   cutSentences.some((r) => r.content === "[Image]" && r.matchedFields[0] === "sentence: [Image]") &&
   cutSentences.some((r) => r.content.includes("LEAK_SENTENCE_ANNOUNCE") && r.matchedFields[0] === "sentence: afbeelding"))
ok("a hand-removal keyed by the cut text is restorable by that same key",
   excludedRows.some((r) => r.manual && r.content === "LEAK_HANDTRIM bewaar dit."))
ok("the preview knows what was cut from a surviving row",
   (cutByContent.get("Kijk op de kaart. Het park opent om 10.30 uur.") || [])
     .map((c) => c.reason).join() === "sentence: [Image],sentence: afbeelding")

out.push("")
out.push("_cutSentences on its own:")
const prep = (...ps) => ctx._prepareExclusionPatterns(ps.map((pattern) => ({ field: "sentence", pattern })))
const cutText = (t, ...ps) => ctx._cutSentences(t, prep(...ps)).text
const untouched = "Open om 10.30 uur. Zie [de kaart](https://efteling.com/a.b/kaart.pdf)!  Tot dan."
ok("nothing matched → the very same string, whitespace and all", cutText(untouched, "zzz") === untouched)
ok("times and URLs with dots are not sentence ends",
   cutText("Open om 10.30 uur. Zie [de kaart](https://efteling.com/a.b/kaart.pdf)! Tot dan.", "tot dan") ===
   "Open om 10.30 uur. Zie [de kaart](https://efteling.com/a.b/kaart.pdf)!")
// The real shape: stripDisplay flattens whitespace, so the placeholder sits
// flush against the next sentence. Cutting it must not take that sentence.
ok("[Image] is a segment of its own, not part of the sentence after it",
   cutText("[Image] ’s Nachts, als de maan schijnt, let dan goed op. Als de heks begint te zingen!", "[Image]") ===
   "’s Nachts, als de maan schijnt, let dan goed op. Als de heks begint te zingen!")
ok("…nor of the sentence before it",
   cutText("Zie hieronder.[Image] Klaar.", "[Image]") === "Zie hieronder. Klaar.")
ok("a sentence in the middle of a line is cut, its neighbours kept",
   cutText("Een. Twee afbeelding? Drie!", "afbeelding") === "Een. Drie!")
ok("a regex pattern cuts by regex",
   ctx._cutSentences("Zie foto 1. Klaar.", ctx._prepareExclusionPatterns([{ field: "sentence", pattern: "foto \\d", isRegex: true }])).text === "Klaar.")

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
