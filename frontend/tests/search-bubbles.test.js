// The conversations search bar's bubbles: what the type-ahead offers, how it
// ranks, and what a bubble turns into on the wire.
//
// The wire format is the interesting half. An `#ID` bubble is written in the
// app's own vocabulary into the existing `query` string, so these assertions
// are the renderer's side of `IdTarget::parse_all` in lib.rs — the two have to
// agree about what `qa-1418 | dn-6391-4` means or a bubble the bar drew as
// valid comes back as "matches nothing".
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
  "_convSuggestVia",
  "convSuggestFor",
  "_convMarkSuggest",
  "_convTokenKeys",
  "convIdQueryString",
  "convIdSegmentValid",
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
  let convIdTokens = []
  let convEntityFilters = []
  let convSearchIds = false
  let convSearchEntities = true
  let convQuery = ""
  let allArticles = []
  let allDialogsCombined = []
  let entityOptions = []
  let entityMap = new Map()
  let dialogMap = new Map()
  const CONV_SUGGEST_MAX = 8
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
    convIdTokens = o.idTokens || []
    convEntityFilters = o.entityFilters || []
    convSearchIds = !!o.idMode
    convSearchEntities = o.entitiesOn !== false
    convQuery = o.query || ""
    _convContentIndex = null
    _convEntityIndex = null
    _convEntityIndexSrc = null
  }
  function labelsFor(q) { return convSuggestFor(q).map((r) => r.idText || r.value) }
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

// ── Ranking ──────────────────────────────────────────────────────────────────
// The whole point of the list is that the row you meant is the one Enter takes.
console.log("What the type-ahead offers:")
ctx.setUp({ articles: ARTICLES, dialogs: DIALOGS, entities: ENTITIES })

eq(
  "an entity whose word starts with the query outranks one that merely contains it",
  ctx.labelsFor("park"),
  ["parkeren", "parkeerplaats_hotel", "zonnepark"],
)
eq(
  "a whole-word match outranks a prefix",
  ctx.labelsFor("wijn"),
  ["wijn"],
)
ok(
  "an entity buried mid-word is still offered, just last",
  ctx.labelsFor("park").indexOf("zonnepark") === 2,
)
eq(
  "every typed token has to hit, so a second word narrows",
  ctx.labelsFor("park hotel"),
  ["parkeerplaats_hotel"],
)
eq("nothing typed offers nothing", ctx.labelsFor("   "), [])

// E is what says entities are part of this search at all.
ctx.setUp({ entities: ENTITIES, entitiesOn: false })
eq("with E off no entity is offered", ctx.labelsFor("park"), [])

// ── Content: found by id *and* by title ──────────────────────────────────────
console.log("\nArticles and Dialogs by name:")
ctx.setUp({ articles: ARTICLES, dialogs: DIALOGS, idMode: true })

eq(
  "a title finds the Article whose id you don't know",
  ctx.labelsFor("park"),
  ["dn-6391", "qa-1418", "qa-1419"],
)
eq("an exact id wins outright", ctx.labelsFor("1418"), ["qa-1418"])
eq("a prefixed id finds itself", ctx.labelsFor("qa-233"), ["qa-233"])
eq(
  "a Transactional Dialog is found and kept distinct",
  ctx.convSuggestFor("ticket").map((r) => r.kind + ":" + r.idText),
  ["tdialog:dn-5803"],
)
eq(
  "a bubble already up is not offered again",
  (ctx.setUp({
    articles: ARTICLES,
    dialogs: DIALOGS,
    idMode: true,
    idTokens: [{ kind: "article", idText: "qa-1418", label: "" }],
  }),
  ctx.labelsFor("park")),
  ["dn-6391", "qa-1419"],
)
// The one path that must not throw: no content export has been loaded.
ctx.setUp({ idMode: true })
eq("with no content export loaded nothing is offered", ctx.labelsFor("park"), [])

// ── Nodes ────────────────────────────────────────────────────────────────────
// The node half of "Dialog / Node ID" was previously reachable only by already
// knowing the number.
console.log("\nDialog nodes:")
ctx.setUp({ articles: ARTICLES, dialogs: DIALOGS, idMode: true })
eq(
  "a trailing dash lists the dialog's nodes, the whole dialog still first",
  ctx.labelsFor("6391-"),
  ["dn-6391", "dn-6391-2", "dn-6391-4", "dn-6391-15"],
)
eq("a node can be found by its name", ctx.labelsFor("6391-park"), ["dn-6391", "dn-6391-15"])
eq("an exact node number wins outright", ctx.labelsFor("6391-4"), ["dn-6391-4", "dn-6391"])
eq("the prefixed spelling reads the same", ctx.labelsFor("dn-6391-4"), ["dn-6391-4", "dn-6391"])
eq("a dialog that does not exist offers nothing", ctx.labelsFor("9999-2"), [])

// ── The wire format ──────────────────────────────────────────────────────────
// This is the half lib.rs has to agree with: `IdTarget::parse_all` reads what
// `convIdQueryString` writes, and `convIdSegmentValid` mirrors what it accepts.
console.log("\nWhat a bubble becomes on the wire:")
ctx.setUp({
  idMode: true,
  idTokens: [
    { kind: "article", idText: "qa-1418", label: "" },
    { kind: "dialog", idText: "dn-6391", label: "" },
    { kind: "node", idText: "dn-6391-4", label: "" },
  ],
})
eq(
  "bubbles are `|`-separated, exactly as OR groups already are",
  ctx.convIdQueryString(),
  "qa-1418 | dn-6391 | dn-6391-4",
)
ok(
  "every segment it writes is a segment it accepts",
  ctx
    .convIdQueryString()
    .split("|")
    .every((seg) => ctx.convIdSegmentValid(seg, "article")),
)
// A raw number typed without picking anything still means what the pill row says.
ok("a bare number is an Article id under the Article pill", ctx.convIdSegmentValid("1234", "article"))
ok("a bare dialog-node is valid under the Dialog pill", ctx.convIdSegmentValid("6391-4", "dialog"))
ok("…and a typo under the Article pill is not", !ctx.convIdSegmentValid("1234-5", "article"))
ok("a prefix outranks the pill in either direction", ctx.convIdSegmentValid("dn-6391-4", "article"))
ok("a word is never an id", !ctx.convIdSegmentValid("parkeren", "dialog"))

ctx.setUp({ idMode: true, idTokens: [{ kind: "article", idText: "qa-1418", label: "" }], query: "233" })
eq(
  "text still typed in the box joins the bubbles rather than replacing them",
  ctx.convIdQueryString(),
  "qa-1418 | 233",
)

// ── What the opened chat searches for ────────────────────────────────────────
// The chat matches by substring over the turn's own fields, so an id has to
// reach it in the shape those fields are in — a Dialog's evidence in
// `dialogPaths` is a bare `6391:2/15`, never `dn-6391`.
console.log("\nThe chat mirror:")
eq("id bubbles lose their prefix, as the raw box always did", ctx._convChatQuery(), "1418 | 233")
ctx.setUp({
  entityFilters: [{ name: "Entities", value: "wijn" }, { name: "Entities", value: "parkeren" }],
})
eq(
  "entity bubbles are the chat's query when nothing was typed",
  ctx._convChatQuery(),
  "wijn | parkeren",
)
ctx.setUp({ entityFilters: [{ name: "Entities", value: "wijn" }], query: "terras" })
eq("…but typed text still wins, because it is what was asked", ctx._convChatQuery(), "terras")

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

eq(
  "a word inside an entity finds it",
  ctx.labelsFor("caravan"),
  ["camper", "camperplaats"],
)
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
eq(
  "an exact word beats a partial one",
  ctx.labelsFor("caravanterrein"),
  ["camperplaats"],
)
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

// ── Marking ──────────────────────────────────────────────────────────────────
console.log("\nHighlighting the typed run:")
eq(
  "the match is marked and everything is escaped",
  ctx._convMarkSuggest("Waar kan ik parkeren?", "park"),
  "Waar kan ik <mark>park</mark>eren?",
)
eq(
  "a hostile name cannot open a tag",
  ctx._convMarkSuggest('<img src=x onerror=alert(1)>', "img"),
  "&lt;<mark>img</mark> src=x onerror=alert(1)&gt;",
)
// An index into the lowercased string is only an index into the original while
// the two are the same length — the trap foldDiacritics documents.
eq(
  "a name that grows when lowercased is left unmarked rather than mis-marked",
  ctx._convMarkSuggest("İstanbul", "i"),
  "İstanbul",
)
eq("no query marks nothing", ctx._convMarkSuggest("parkeren", ""), "parkeren")

console.log(out.join("\n"))
if (failed) {
  console.error(`\nSearch-bar bubbles: ${failed} check(s) failed`)
  process.exit(1)
}
console.log("\nSearch-bar bubbles: all checks passed")
