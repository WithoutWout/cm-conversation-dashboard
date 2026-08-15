// Conversations-side search behaviour that lives in the renderer: diacritic
// folding (so the chat agrees with the FTS-backed session list), entity-aware
// turn matching, and the entity → Articles/Dialogs cross-reference.
//
// Runs the real functions out of index.html via ./extract, so none of this can
// drift from the source it is asserting about.
const { extract } = require("./extract")
const vm = require("vm")

const NAMES = [
  "foldDiacritics",
  "_tokenizeSegment",
  "createChatSearchPlan",
  "currentChatSearchPlan",
  "chatQueryMatchesText",
  "_chatMarkSegment",
  "rowEntityFields",
  "getEntityForChip",
  "_resolveEntityForChip",
  "entityRefIndex",
  "entityArticleXrefs",
  "entityDialogXrefs",
]

const ctx = vm.createContext({ console })
vm.runInContext(
  `
  // Pure caches and plain state the extracted functions read. Nothing here
  // carries behaviour — anything that decides something is extracted above.
  // \`extract\` pulls named function declarations only, so a module-level
  // const the real source hoists out of a hot function has to be restated.
  const _CHIP_TOKEN_SPLIT = /[\\s\\-_,;.!?]+/
  const _foldCache = new Map()
  let _chatPlanCache = { key: null, plan: null }
  let _chipEntityCache = new Map()
  let _entityRefIndex = null
  let chatQuery = ""
  let chatSearchRegex = false
  let entityMap = new Map()
  let entityWordMap = new Map()
  let allArticles = []
  let allDialogsCombined = []
  function parseJsonSafe(s) {
    if (!s) return null
    try { return JSON.parse(s) } catch { return null }
  }
  ${NAMES.map(extract).join("\n")}
  function setChatQuery(q, regex) {
    chatQuery = q
    chatSearchRegex = !!regex
    _chatPlanCache = { key: null, plan: null }
  }
  function setContent(entities, articles, dialogs) {
    entityMap = new Map(entities.map((e) => [e.name.toUpperCase(), e]))
    entityWordMap = new Map()
    for (const e of entities) {
      for (const w of e.words) {
        const key = w.text.toLowerCase()
        if (!entityWordMap.has(key)) entityWordMap.set(key, e)
      }
    }
    allArticles = articles
    allDialogsCombined = dialogs
    _chipEntityCache = new Map()
    _entityRefIndex = null
  }
  function entityByName(name) { return entityMap.get(name.toUpperCase()) }
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

// ── Diacritic folding ────────────────────────────────────────────────────────
// The session list is backed by an FTS index built with remove_diacritics, so
// "cafe" finds "café" there. Opening one of those results and being told no
// message matches reads as the chat being broken.
const fold = (s) => ctx.foldDiacritics(s)
eq("folds accented Latin to its base letters", fold("Café Konditorei"), "Cafe Konditorei")
eq("leaves plain ASCII untouched", fold("plain ascii"), "plain ascii")
ok(
  "folding never changes the length — highlight offsets depend on it",
  ["café", "Ångström", "señor", "ıi", "😀 café", "é"].every(
    (s) => fold(s).length === s.length,
  ),
)
ok("a surrogate pair survives folding intact", fold("😀 café") === "😀 cafe")

console.log("Diacritic-insensitive chat matching:")
ctx.setChatQuery("cafe")
ok("unaccented query matches accented text", ctx.chatQueryMatchesText("het café is open"))
ctx.setChatQuery("café")
ok("accented query matches unaccented text", ctx.chatQueryMatchesText("het cafe is open"))
ctx.setChatQuery("openingstijden")
ok("a non-match is still a non-match", !ctx.chatQueryMatchesText("het cafe is open"))
ctx.setChatQuery('"cafe konditorei"')
ok("quoted phrases fold too", ctx.chatQueryMatchesText("ons Café Konditorei"))

// ── Highlighting lands on the original text ──────────────────────────────────
console.log("Highlight placement:")
const mark = (seg, q) => {
  ctx.setChatQuery(q)
  return ctx
    ._chatMarkSegment(seg, ctx.currentChatSearchPlan().highlightRegexes)
    .replace(/\x00MARK\x00/g, "[")
    .replace(/\x00\/MARK\x00/g, "]")
}
eq("marks the accented word, keeping its accent", mark("het café is open", "cafe"), "het [café] is open")
eq("marks a plain word", mark("het cafe is open", "cafe"), "het [cafe] is open")
eq("marks every occurrence", mark("cafe en café", "cafe"), "[cafe] en [café]")
eq("no match leaves the text alone", mark("het cafe is open", "hotel"), "het cafe is open")
// Two terms covering the same span must not nest <mark> inside <mark>.
eq("overlapping terms produce one mark", mark("openingstijden", "opening | openingstijden"), "[openingstijden]")
eq("adjacent terms both mark", mark("cafe konditorei", "cafe | konditorei"), "[cafe] [konditorei]")

// ── Entity fields on a chat row ──────────────────────────────────────────────
console.log("Entity-aware turn matching:")
const row = {
  interactionValue: "mag ik een fles rood meenemen",
  outputText: "Dat mag helaas niet.",
  recognitionDetails: JSON.stringify({
    entityMatches: [
      { name: "FLES_2", match: "wijn", entityId: 2611, displayName: "WIJN" },
    ],
  }),
}
const fields = ctx.rowEntityFields(row)
ok("display name is searchable", fields.includes("WIJN"))
ok("internal name is searchable", fields.includes("FLES_2"))
ok("the matched text is searchable", fields.includes("wijn"))
ok("the entity id is searchable", fields.includes("2611"))
ok("fields are cached on the row", ctx.rowEntityFields(row) === fields)
eq("a row with no recognition details yields nothing", ctx.rowEntityFields({}), [])
ctx.setChatQuery("wijn")
ok(
  "the entity is what matches, not the message text",
  fields.some((f) => ctx.chatQueryMatchesText(f)) &&
    !ctx.chatQueryMatchesText(row.interactionValue) &&
    !ctx.chatQueryMatchesText(row.outputText),
)

// ── Entity → Articles / Dialogs ──────────────────────────────────────────────
// The chip on an Article says "Entity: WIJN" when the phrase resolves by word
// or token, but the entity's own view used to list only Articles whose phrase
// was verbatim the entity name — so most of what the chips claimed was missing.
console.log("Entity cross-references:")
const entities = [
  { name: "WIJN", words: [{ text: "wijn" }, { text: "rode wijn" }] },
  { name: "SOUVENIR", words: [{ text: "souvenir" }, { text: "aandenken" }] },
  { name: "PARKEREN", words: [{ text: "parkeren" }] },
]
const articles = [
  { Id: 1, Questions: [{ Text: "WIJN" }] }, // verbatim name
  { Id: 2, Questions: [{ Text: "rode wijn" }] }, // entity word
  { Id: 3, Questions: [{ Text: "souvenir kapot" }] }, // token inside a phrase
  { Id: 4, Questions: [{ Text: "iets heel anders" }] }, // no entity at all
  { Id: 5, Questions: [{ Text: "wijn" }, { Text: "rode wijn" }] }, // same entity twice
]
const dialogs = [
  {
    id: 50,
    _kind: "dialog",
    nodes: [
      {
        id: 1,
        links: [
          { condition: { data: { questions: [{ text: "parkeren" }] } } },
          { condition: { data: { isFallback: true, questions: [{ text: "wijn" }] } } },
        ],
      },
    ],
  },
]
ctx.setContent(entities, articles, dialogs)
const artIds = (name) => ctx.entityArticleXrefs(ctx.entityByName(name)).map((a) => a.Id)
eq("verbatim name, entity word and token all attach", artIds("WIJN"), [1, 2, 5])
eq("a token inside a longer phrase attaches", artIds("SOUVENIR"), [3])
eq("an unrelated Article attaches to nothing", artIds("PARKEREN"), [])
ok(
  "an Article is listed once even when several phrases resolve to it",
  artIds("WIJN").filter((id) => id === 5).length === 1,
)
eq(
  "a Recognition link attaches its Dialog",
  ctx.entityDialogXrefs(ctx.entityByName("PARKEREN")).map((d) => d.id),
  [50],
)
eq(
  "a fallback link is not a reference",
  ctx.entityDialogXrefs(ctx.entityByName("WIJN")).map((d) => d.id),
  [],
)
// The forward direction (chip label) and the reverse (entity view) must agree.
const disagreements = articles.filter((a) => {
  const resolved = new Set(
    (a.Questions || []).map((q) => ctx.getEntityForChip(q.Text)).filter(Boolean).map((e) => e.name),
  )
  return [...resolved].some((name) => !artIds(name).includes(a.Id))
})
eq("every chip's entity lists the Article it labels", disagreements.map((a) => a.Id), [])

console.log(out.join("\n"))
if (failed) {
  console.error(`\nConversations search: ${failed} check(s) failed`)
  process.exit(1)
}
console.log("\nConversations search: all checks passed")
