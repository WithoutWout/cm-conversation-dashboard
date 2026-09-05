// The conversations search bar: what the type-ahead offers, how it ranks, and
// what the field turns into on the wire.
//
// The wire format is the interesting half. The whole field is one boolean
// expression written into the existing `query` string, so these assertions are
// the renderer's side of `parse_search_expr` in lib.rs — the two have to agree
// about what `( qa-1418 OR dn-1604 ) AND entity:camper` means, or a search the
// bar drew as valid comes back meaning something else. The strings asserted
// here are quoted verbatim by `the_renderer_and_this_parser_read_the_same_strings`
// on the Rust side; that fixture is the only thing holding the two halves of one
// grammar together across two languages.
//
// Runs the real functions out of index.html via ./extract, so none of this can
// drift from the source it is asserting about.
const { extract } = require("./extract")
const vm = require("vm")

const NAMES = [
  "convContentIndex",
  "convEntityIndex",
  "_convWordStart",
  "_convSuggestRank",
  "_convNodeSuggest",
  "convTokenNodeRows",
  "_convSuggestVia",
  "convParseIdToken",
  "_convSyntheticIdRows",
  "convSuggestFor",
  "_convMarkSuggest",
  "_convIsOperand",
  "_convTokenKeys",
  "_convAppendOperand",
  "_convDedupeOperands",
  "normalizeConvExpr",
  "_convEscapeText",
  "_convEntityToken",
  "convExprToQuery",
  "convScanExprText",
  "_convUnquote",
  "convOpenGroupDepth",
  "convTextTerms",
  "_convChatQuery",
]

const ctx = vm.createContext({ console })
vm.runInContext(
  `
  // Plain state the extracted functions read. Nothing here decides anything —
  // \`extract\` pulls named function declarations only, so the module-level
  // caches and the search-bar state have to be restated.
  let _convContentIndex = null
  let _convEntityIndex = null
  let _convEntityIndexSrc = null
  let convExpr = []
  let convSearchEntities = true
  let convSearchRegex = false
  let allArticles = []
  let allDialogsCombined = []
  let entityOptions = []
  let entityMap = new Map()
  let dialogMap = new Map()
  const CONV_SUGGEST_MAX = 8
  // The type-ahead asks for a label when it reads an id out of typed text.
  function _convLabelForId(idText) {
    const hit = convContentIndex().find((c) => c.idText === idText)
    return hit ? hit.label : ""
  }
  function esc(s) {
    return String(s == null ? "" : s)
      .replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
  }
  ${NAMES.map(extract).join("\n")}
  function setUp(o) {
    allArticles = o.articles || []
    allDialogsCombined = o.dialogs || []
    dialogMap = new Map(allDialogsCombined.map((d) => [d.id, d]))
    entityOptions = o.entities || []
    entityMap = new Map(
      (o.exported || []).map((e) => [e.name.toUpperCase(), e]),
    )
    convExpr = o.expr || []
    convSearchEntities = o.entitiesOn !== false
    convSearchRegex = !!o.regex
    _convContentIndex = null
    _convEntityIndex = null
    _convEntityIndexSrc = null
    normalizeConvExpr()
  }
  function labelsFor(q) { return convSuggestFor(q).map((r) => r.idText || r.value) }
  function query() { return convExprToQuery() }
  function items() { return convExpr }
  function ids() { return convExpr.filter((i) => i.t === "id").map((i) => i.idText) }
  // \`setUp\` then serialise, so a fixture reads as the field it describes.
  function queryOf(expr) { convExpr = expr; normalizeConvExpr(); return convExprToQuery() }
  // Round-trip: what the field writes, read back, written again.
  function reparse(s) { convExpr = convScanExprText(s, convSearchRegex); normalizeConvExpr(); return convExprToQuery() }
  function append(item) { _convAppendOperand(item); normalizeConvExpr(); return convExprToQuery() }
  // The real replaceConvToken also closes the popover and re-runs the search;
  // this is the array surgery, which is the part that can be wrong.
  function replaceToken(i, idText) {
    const cand = convTokenNodeRows(convExpr[i]).find((r) => r.idText === idText)
    convExpr[i] = { t: "id", kind: cand.kind, idText: cand.idText, label: cand.label || "" }
    _convDedupeOperands()
  }
`,
  ctx,
)

const out = []
let failed = 0
const ok = (n, c) => {
  if (!c) failed++
  out.push((c ? "  PASS  " : "  FAIL  ") + n)
}
const eq = (n, a, b) => {
  const same = JSON.stringify(a) === JSON.stringify(b)
  if (!same) out.push(`         got ${JSON.stringify(a)} want ${JSON.stringify(b)}`)
  ok(n, same)
}

const ARTICLES = [
  { Id: 1418, _faqQ: "Waar kan ik parkeren?" },
  { Id: 1419, _faqQ: "Wat kost parkeren bij het hotel op zondag?" },
  { Id: 233, _faqQ: "Openingstijden vandaag" },
]
const DIALOGS = [
  {
    id: 6391,
    name: "Parkeren en bereikbaarheid",
    _kind: "dialog",
    nodes: [
      { id: 2, name: "Start" },
      { id: 15, name: "Parkeerplaats kiezen" },
      { id: 4, name: "Prijs tonen" },
    ],
  },
  { id: 5803, name: "Ticket omruilen", _kind: "tdialog" },
]
// The export's entities carry the words that fire them. These are the real
// CAMPER / CAMPERPLAATS rows from the Efteling export, trimmed.
const EXPORTED = [
  {
    name: "CAMPER",
    words: [
      { text: "camper" },
      { text: "caravan" },
      { text: "stacaravan" },
      { text: "kampeerwagen" },
      { text: "mobilhome" },
    ],
  },
  {
    name: "CAMPERPLAATS",
    words: [{ text: "caravanterrein" }, { text: "caravanstalling" }],
  },
  { name: "PARKEREN", words: [{ text: "parkeren" }, { text: "auto kwijt" }] },
]

const ENTITIES = [
  { name: "Entities", value: "parkeren", count: 812 },
  { name: "Entities", value: "parkeerplaats_hotel", count: 41 },
  { name: "Entities", value: "zonnepark", count: 7 },
  { name: "Entities", value: "wijn", count: 133 },
]

const TEXT = (v) => ({ t: "text", value: v })
const ENT = (v) => ({ t: "entity", value: v, label: v })
const ID = (idText, kind) => ({ t: "id", kind, idText, label: "" })
const OP = (op) => ({ t: "op", op })

// ── Plain typing is untouched ────────────────────────────────────────────────
// By far the most common thing anyone does with this field, and the one thing
// the grammar must not have made more expensive.
console.log("A sentence is still a sentence:")
ctx.setUp({})
eq("a plain sentence is one condition", ctx.queryOf([TEXT("parkeren tarief")]), "parkeren tarief")
eq(
  "the older term grammar lives inside a chip, a layer below the operators",
  ctx.queryOf([TEXT("a b | c d")]),
  "a b | c d",
)
eq("a quoted phrase survives", ctx.queryOf([TEXT('kaart "op zondag"')]), 'kaart "op zondag"')
eq(
  "typing a sentence produces exactly one condition",
  ctx.convScanExprText("brood and boter", false),
  [TEXT("brood and boter")],
)
ok(
  "…because the keywords are upper-case only",
  ctx.convScanExprText("brood AND boter", false).length === 3,
)

// ── The wire format ──────────────────────────────────────────────────────────
// This is the half lib.rs has to agree with. Every string below is quoted
// verbatim by `the_renderer_and_this_parser_read_the_same_strings`.
console.log("\nWhat the field becomes on the wire:")
eq(
  "text and an entity join with the operator between them",
  ctx.queryOf([TEXT("boos"), OP("and"), ENT("camper")]),
  "boos AND entity:camper",
)
eq(
  "two conditions with no operator between them join with AND",
  ctx.queryOf([TEXT("boos"), ENT("camper")]),
  "boos AND entity:camper",
)
eq(
  "an exclusion between two conditions is AND NOT",
  ctx.queryOf([ID("dn-2", "dialog"), OP("not"), TEXT("duur")]),
  "dn-2 AND NOT duur",
)
eq(
  "…and one leading the whole search is a bare NOT",
  ctx.queryOf([OP("not"), TEXT("duur")]),
  "NOT duur",
)
eq(
  "a leading AND has nothing to join, so it is dropped",
  ctx.queryOf([OP("and"), TEXT("duur")]),
  "duur",
)
eq(
  "brackets are written explicitly, so the string says what it means",
  ctx.queryOf([
    { t: "(" },
    ID("qa-1418", "article"),
    OP("or"),
    ID("dn-1604", "dialog"),
    { t: ")" },
    OP("and"),
    ENT("camper"),
    OP("not"),
    TEXT("veel te duur"),
  ]),
  "( qa-1418 OR dn-1604 ) AND entity:camper AND NOT veel te duur",
)
eq(
  "an entity name with a space keeps its quotes and loses them again on the way back",
  ctx.queryOf([ENT("polles keuken")]),
  'entity:"polles keuken"',
)
eq(
  "a text chip carrying grammar is quoted, so it comes back as text",
  ctx.queryOf([TEXT("AND dn-6391 entity:y ok")]),
  '"AND" "dn-6391" "entity:y" ok',
)

// ── Round-tripping ───────────────────────────────────────────────────────────
// Every string the field writes has to parse back to the same field. Without
// this the Insights chip, the AI export and the next render could each read one
// search three ways.
console.log("\nEvery string it writes, read back:")
for (const s of [
  "parkeren tarief",
  "boos AND entity:camper",
  "qa-1 OR dn-2",
  "dn-2 AND NOT duur",
  "NOT duur",
  "( qa-1418 OR dn-1604 ) AND entity:camper AND NOT veel te duur",
  'kaart "op zondag"',
  "a b | c d",
  '"AND" "dn-6391" ok',
  'entity:"polles keuken" OR dn-6391-4',
]) {
  eq("round-trips: " + s, ctx.reparse(s), s)
}

// ── Normalising ──────────────────────────────────────────────────────────────
// The field cannot hold an expression nobody can read, which is what lets the
// search run without a validation step in front of it.
console.log("\nThe field cannot hold nonsense:")
eq("a doubled operator keeps the last one", ctx.queryOf([TEXT("a"), OP("and"), OP("or"), TEXT("b")]), "a OR b")
eq("a trailing operator is dropped", ctx.queryOf([TEXT("a"), OP("or")]), "a")
eq("an empty group is removed", ctx.queryOf([TEXT("a"), { t: "(" }, { t: ")" }]), "a")
eq("a closer with no opener is ignored", ctx.queryOf([TEXT("a"), { t: ")" }]), "a")
eq(
  "an operator cannot lead a group either, unless it excludes",
  ctx.queryOf([TEXT("a"), OP("and"), { t: "(" }, OP("or"), TEXT("b"), { t: ")" }]),
  "a AND ( b )",
)
eq(
  "a group still being built closes itself on the way out",
  ctx.queryOf([TEXT("a"), OP("and"), { t: "(" }, TEXT("b")]),
  "a AND ( b )",
)
ok(
  "…and is left open in the field, which is the state you build in",
  (ctx.setUp({ expr: [TEXT("a"), { t: "(" }, TEXT("b")] }), ctx.convOpenGroupDepth() === 1),
)

// ── The operator a new chip arrives with ─────────────────────────────────────
// Guessing is worth doing because it is one click to change.
console.log("\nThe operator a new chip guesses at:")
ctx.setUp({ expr: [ID("qa-1", "article")] })
eq("two Articles almost always mean OR", ctx.append(ID("qa-2", "article")), "qa-1 OR qa-2")
ctx.setUp({ expr: [ENT("camper")] })
eq("two entities too", ctx.append(ENT("caravan")), "entity:camper OR entity:caravan")
ctx.setUp({ expr: [ID("dn-6391", "dialog")] })
eq("a Dialog and a word almost always mean AND", ctx.append(TEXT("boos")), "dn-6391 AND boos")
ctx.setUp({ expr: [TEXT("boos")] })
eq("and so do two runs of text", ctx.append(TEXT("duur")), "boos AND duur")

// ── Ranking ──────────────────────────────────────────────────────────────────
// The whole point of the list is that the row you meant is the one you take.
console.log("\nWhat the type-ahead offers:")
ctx.setUp({ articles: ARTICLES, dialogs: DIALOGS, entities: ENTITIES })

eq(
  "both pools are searched at once, and ranked together",
  ctx.labelsFor("park"),
  // Two entities and a Dialog all begin with it, so the conversation counts
  // break the tie — which is the ordering that already governed the entity
  // list on its own.
  ["parkeren", "parkeerplaats_hotel", "dn-6391", "qa-1418", "qa-1419", "zonnepark"],
)
eq("a whole-word match outranks a prefix", ctx.labelsFor("wijn"), ["wijn"])
eq(
  "every typed token has to hit, so a second word narrows",
  ctx.labelsFor("park hotel"),
  ["parkeerplaats_hotel", "qa-1419"],
)
eq("nothing typed offers nothing", ctx.labelsFor("   "), [])
eq("an exact id wins outright", ctx.labelsFor("qa-233"), ["qa-233"])
eq(
  "a Transactional Dialog is found and kept distinct",
  ctx.convSuggestFor("ticket").map((r) => r.kind + ":" + r.idText),
  ["tdialog:dn-5803"],
)
eq(
  "a chip already up is not offered again",
  (ctx.setUp({
    articles: ARTICLES,
    dialogs: DIALOGS,
    expr: [ID("qa-1418", "article")],
  }),
  ctx.labelsFor("park")),
  ["dn-6391", "qa-1419"],
)

// E governs whether a *typed word* also matches the entity fields. An
// `entity:` condition is exact and does not depend on it, so the suggestions
// must not either — gating them left the field unable to offer a chip that
// would have worked perfectly well once placed.
ctx.setUp({ entities: ENTITIES, entitiesOn: false })
eq(
  "an entity is still offered with E off, because the chip does not need E",
  ctx.labelsFor("park"),
  ["parkeren", "parkeerplaats_hotel", "zonnepark"],
)

// ── Searching by id with no content export ───────────────────────────────────
// Removing `#ID` would otherwise have taken away the only way to do it: only
// the *label* needs the export.
console.log("\nIds without a content export:")
ctx.setUp({})
eq("a fully spelled id is offered on its own", ctx.labelsFor("qa-1418"), ["qa-1418"])
eq("a bare number offers both readings", ctx.labelsFor("1418"), ["qa-1418", "dn-1418"])
eq("…and a word is still not an id", ctx.labelsFor("parkeren"), [])
ctx.setUp({ articles: ARTICLES, dialogs: DIALOGS })
eq(
  "with an export loaded the row that has a title behind it comes first",
  ctx.labelsFor("1418"),
  ["qa-1418", "dn-1418"],
)
eq(
  "an id the export does not know is still offered, spelled out",
  ctx.labelsFor("dn-9999"),
  ["dn-9999"],
)

// ── Nodes ────────────────────────────────────────────────────────────────────
console.log("\nDialog nodes:")
ctx.setUp({ articles: ARTICLES, dialogs: DIALOGS })
eq(
  "a trailing dash lists the dialog's nodes, the whole dialog still first",
  ctx.labelsFor("dn-6391-"),
  ["dn-6391", "dn-6391-2", "dn-6391-4", "dn-6391-15"],
)
eq("a node can be found by its name", ctx.labelsFor("dn-6391-park"), ["dn-6391", "dn-6391-15"])
eq("an exact node number wins outright", ctx.labelsFor("dn-6391-4"), ["dn-6391-4", "dn-6391"])
// A node of a dialog the export does not hold is still a legitimate search —
// only its *name* needed the export.
eq("a node of an unknown dialog is still offered, spelled out", ctx.labelsFor("dn-9999-2"), [
  "dn-9999-2",
])

// ── Narrowing a Dialog chip to one of its nodes ──────────────────────────────
// A Dialog chip is a question you are half-way through asking. Re-typing the
// whole thing was the only way to add the node.
console.log("\nThe node picker on an existing chip:")
ctx.setUp({
  articles: ARTICLES,
  dialogs: DIALOGS,
  expr: [ID("dn-6391", "dialog")],
})
eq(
  "a Dialog chip offers the whole dialog first, then its nodes",
  ctx.convTokenNodeRows(ctx.items()[0]).map((r) => r.idText),
  ["dn-6391", "dn-6391-2", "dn-6391-15", "dn-6391-4"],
)
eq(
  "…and marks the one the chip already carries",
  ctx.convTokenNodeRows(ctx.items()[0]).filter((r) => r.isCurrent).map((r) => r.idText),
  ["dn-6391"],
)
eq(
  "a chip already narrowed to a node marks that node instead",
  ctx
    .convTokenNodeRows(ID("dn-6391-15", "node"))
    .filter((r) => r.isCurrent)
    .map((r) => r.idText),
  ["dn-6391-15"],
)
// The four cases where the chip must stay a plain, non-clickable one.
ok("an Article chip has no nodes to pick", ctx.convTokenNodeRows(ID("qa-1418", "article")) === null)
ok(
  "a Transactional Dialog carries no nodes in the export",
  ctx.convTokenNodeRows(ID("dn-5803", "tdialog")) === null,
)
ok(
  "a dialog we do not hold offers nothing rather than an empty menu",
  ctx.convTokenNodeRows(ID("dn-9999", "dialog")) === null,
)
ok("a text chip is not a Dialog", ctx.convTokenNodeRows(TEXT("dn-6391")) === null)

// Narrowing, and then widening again from the same menu.
ctx.setUp({ dialogs: DIALOGS, expr: [ID("dn-6391", "dialog")] })
ctx.replaceToken(0, "dn-6391-15")
eq("picking a node narrows the chip in place", ctx.ids(), ["dn-6391-15"])
ctx.replaceToken(0, "dn-6391")
eq("…and 'whole dialog' widens it back", ctx.ids(), ["dn-6391"])
// Two chips converging on the same node would OR with themselves.
ctx.setUp({
  dialogs: DIALOGS,
  expr: [ID("dn-6391-15", "node"), OP("or"), ID("dn-6391", "dialog")],
})
ctx.replaceToken(2, "dn-6391-15")
eq("narrowing onto a node another chip holds leaves one, not two", ctx.query(), "dn-6391-15")

ctx.setUp({})
ok(
  "with no content export loaded a Dialog chip is not clickable",
  ctx.convTokenNodeRows(ID("dn-6391", "dialog")) === null,
)

// ── What the opened chat searches for ────────────────────────────────────────
// The chat matches by substring over the turn's own fields, so an id has to
// reach it in the shape those fields are in — a Dialog's evidence in
// `dialogPaths` is a bare `6391:2/15`, never `dn-6391`.
console.log("\nThe chat mirror:")
ctx.setUp({
  expr: [ID("qa-1418", "article"), OP("or"), ID("dn-6391", "dialog"), OP("and"), TEXT("terras")],
})
eq("id chips lose their prefix, as the raw box always did", ctx._convChatQuery(), "1418 | 6391 | terras")
ctx.setUp({ expr: [ENT("wijn"), OP("or"), ENT("parkeren")] })
eq(
  "entity chips are the chat's query when nothing was typed",
  ctx._convChatQuery(),
  "wijn | parkeren",
)
eq(
  "the union, not the intersection: an AND would open a chat with nothing marked",
  (ctx.setUp({ expr: [ENT("wijn"), OP("and"), TEXT("terras")] }), ctx._convChatQuery()),
  "wijn | terras",
)
eq(
  "an excluded condition is no reason a conversation is here, so it is left out",
  (ctx.setUp({ expr: [TEXT("parkeren"), OP("not"), TEXT("duur")] }), ctx._convChatQuery()),
  "parkeren",
)
eq(
  "the preview highlight is given the words, never the expression",
  (ctx.setUp({
    expr: [ID("qa-1", "article"), OP("and"), TEXT("boos"), OP("or"), TEXT("blij")],
  }),
  ctx.convTextTerms()),
  "boos | blij",
)

// ── An entity is findable by the words that fire it ──────────────────────────
// `caravan` is not the CAMPER entity's name — it is one of the words the
// recognizer fires it on, and it is what someone types when looking for those
// conversations.
console.log("\nEntities found through their trigger words:")
const CAMPER_ENTITIES = [
  { name: "Entities", value: "camper", count: 300 },
  { name: "Entities", value: "camperplaats", count: 40 },
  { name: "Entities", value: "parkeren", count: 900 },
]
ctx.setUp({ entities: CAMPER_ENTITIES, exported: EXPORTED })

eq("a word inside an entity finds it", ctx.labelsFor("caravan"), ["camper", "camperplaats"])
eq(
  "the name still outranks every word match",
  (ctx.setUp({
    entities: [
      { name: "Entities", value: "camper", count: 1 },
      // Name-matches `camper`; CAMPER only word-matches it.
      { name: "Entities", value: "camperplaats", count: 999 },
    ],
    exported: EXPORTED,
  }),
  ctx.labelsFor("camper")),
  ["camper", "camperplaats"],
)
ctx.setUp({ entities: CAMPER_ENTITIES, exported: EXPORTED })
eq("an exact word beats a partial one", ctx.labelsFor("caravanterrein"), ["camperplaats"])
eq(
  "the row says which word it matched on",
  ctx.convSuggestFor("caravan").map((c) => c.value + " via " + ctx._convSuggestVia(c, "caravan")),
  ["camper via caravan", "camperplaats via caravanterrein"],
)
ok(
  "a name match carries no 'via' — it would only restate the row",
  ctx._convSuggestVia(ctx.convSuggestFor("camper")[0], "camper") === "",
)
// The words come from the content export; without one this must degrade to
// name-only rather than throw.
ctx.setUp({ entities: CAMPER_ENTITIES })
eq("with no content export loaded, names still match", ctx.labelsFor("camper"), ["camper", "camperplaats"])
eq("…and a word-only query finds nothing rather than erroring", ctx.labelsFor("caravan"), [])

// ── Regex mode ───────────────────────────────────────────────────────────────
// `a(b|c)` is a group the *user* wrote, and reading it as ours would silently
// search for two different things.
console.log("\nUnder .* the brackets belong to the pattern:")
ctx.setUp({ regex: true })
eq("a pattern's own brackets stay in it", ctx.convScanExprText("a(b|c)", true), [TEXT("a(b|c)")])
eq("…and the operators still apply beside it", ctx.convScanExprText("dn-6391 AND a(b|c)", true).length, 3)

// ── Marking ──────────────────────────────────────────────────────────────────
console.log("\nHighlighting the typed run:")
eq(
  "the match is marked and everything is escaped",
  ctx._convMarkSuggest("Waar kan ik parkeren?", "park"),
  "Waar kan ik <mark>park</mark>eren?",
)
eq(
  "a name carrying markup is escaped, not rendered",
  ctx._convMarkSuggest("<b>park</b>", "park"),
  "&lt;b&gt;<mark>park</mark>&lt;/b&gt;",
)
eq("nothing typed marks nothing", ctx._convMarkSuggest("parkeren", ""), "parkeren")

console.log(out.join("\n"))
if (failed) {
  console.error(`\n${failed} check(s) failed`)
  process.exit(1)
}
console.log("\nAll search-bar checks passed.")
