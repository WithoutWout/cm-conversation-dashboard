// GAP's "Fix this" panel: what to change in CM.com for one low- or
// zero-recognition turn, read from the turn's own recognition details and the
// turns after it. The real functions run against a fixture shaped like the
// Interaction Log — see docs/gap.md → "Fix this".
const assert = require("assert")
const vm = require("vm")
const { extract } = require("./extract")

const BASE = "https://cm.example/app"
const ctx = vm.createContext({})
vm.runInContext(
  `
  const cmBaseUrl = ${JSON.stringify(BASE)}
  const lowRecogThreshold = 60
  const art = (Id, q) => ({ Id, Questions: [{ Text: q, IsFaq: true }] })
  let allArticles = [art(1203, "Wat betekent een woord"), art(4410, "betekenis"), art(8927, "buiten")]
  const articleMap = new Map(allArticles.map((a) => [a.Id, a]))
  const dialogMap = new Map([[5793, { id: 5793, name: "Fallback", nodes: [{ id: 6193, name: "Niet begrepen" }] }]])
  const tDialogMap = new Map()
  let allEntities = [
    { name: "BETEKENEN", words: [{ text: "betekenen" }, { text: "betekend" }] },
    { name: "MOED", words: [{ text: "moedig" }, { text: "onversaagdheid" }] },
    { name: "BUITEN", words: [{ text: "uit" }] },
  ]
  // What the Entities cards resolve: BETEKENEN is used by two Articles.
  function entityRefIndex() {
    return new Map([["BETEKENEN", { articles: [articleMap.get(1203), articleMap.get(4410)], dialogs: [] }]])
  }
  function entityIdFor(name) { return { moed: 77 }[String(name).toLowerCase()] ?? null }
  ${["aFaqQ", "parseJsonSafe", "gapIsZero", "gapContentRef", "gapFixModel", "gapClosestEntities"].map(extract).join("\n")}
  globalThis.api = { gapContentRef, gapFixModel, gapClosestEntities }
  `,
  ctx,
)
const { gapContentRef, gapFixModel, gapClosestEntities } = ctx.api
const plain = (v) => JSON.parse(JSON.stringify(v))

let failed = 0
const test = (name, fn) => {
  try {
    fn()
    console.log("  PASS  " + name)
  } catch (e) {
    failed++
    console.log("  FAIL  " + name + "\n        " + e.message)
  }
}

const details = (d) => JSON.stringify(d)
const ZERO = { logId: 11, recognition: 0, articleIds: '["dn-5793-6193","e-1"]' }
const chat = [
  { logId: 10, recognitionType: "Entity Recognition", recognitionQuality: 95, articleIds: '["qa-8927"]', interactionValue: "before" },
  {
    logId: 11,
    recognitionType: "No Recognition",
    recognitionQuality: 0,
    articleIds: '["dn-5793-6193","e-1"]',
    recognitionDetails: details({
      missingWords: "onversaagd, onversaagd",
      entityMatches: [{ name: "BETEKENEN_1", match: "betekend", entityId: 496, displayName: "BETEKENEN" }],
      missingArticle: "true",
    }),
  },
  { logId: 12, recognitionType: "GenerativeAI", recognitionQuality: 0, articleIds: "[]", interactionValue: "genai" },
  { logId: 13, recognitionType: "Entity Recognition", recognitionQuality: 40, articleIds: '["qa-4410"]', interactionValue: "still low" },
  { logId: 14, recognitionType: "Entity Recognition", recognitionQuality: 88, articleIds: '["qa-1203"]', interactionValue: "wat is de betekenis" },
]

test("an Article, a Dialog node and an unknown id become links, or nothing", () => {
  assert.deepStrictEqual(plain(gapContentRef("qa-1203")), {
    kind: "article",
    label: "qa-1203",
    title: "Wat betekent een woord",
    url: BASE + "/articles/1203",
  })
  const node = plain(gapContentRef("dn-5793-6193"))
  assert.strictEqual(node.url, BASE + "/dialogs/5793?currentNode=6193")
  assert.strictEqual(node.title, "Fallback · Niet begrepen")
  assert.strictEqual(gapContentRef("e-1"), null)
})

test("a zero turn: unknown words, what it was recognised as, the Articles for that pattern, and the rephrase", () => {
  const m = plain(gapFixModel(ZERO, chat))
  assert.strictEqual(m.zero, true)
  assert.deepStrictEqual(m.unknown, ["onversaagd"], "missingWords, de-duplicated")
  assert.deepStrictEqual(m.recognised, [{ name: "BETEKENEN", match: "betekend", id: 496 }], "the entity's own id")
  assert.strictEqual(m.missingArticle, true)
  assert.deepStrictEqual(m.candidates.map((c) => c.label), ["qa-1203", "qa-4410"])
  // Next: skips the GenAI turn and the one still under the threshold.
  assert.deepStrictEqual(m.next.refs.map((r) => r.label), ["qa-1203"])
  assert.strictEqual(m.next.question, "wat is de betekenis")
})

test("a low turn lists what answered, and does not offer it again as a candidate", () => {
  const low = { logId: 21, recognition: 45, articleIds: '["qa-1203"]' }
  const rows = [
    {
      logId: 21,
      recognitionType: "Entity Recognition",
      recognitionQuality: 45,
      articleIds: '["qa-1203"]',
      recognitionDetails: details({ entityMatches: [{ match: "betekend", displayName: "BETEKENEN", entityId: 496 }] }),
    },
  ]
  const m = plain(gapFixModel(low, rows))
  assert.strictEqual(m.zero, false)
  assert.deepStrictEqual(m.answered.map((r) => r.label), ["qa-1203"])
  assert.deepStrictEqual(m.candidates.map((c) => c.label), ["qa-4410"])
  assert.strictEqual(m.next, null, "nothing after it")
})

test("a turn with no recognition details says so rather than inventing a diagnosis", () => {
  const m = plain(gapFixModel({ logId: 31, recognition: 0 }, [{ logId: 31 }]))
  assert.strictEqual(m.hasDetails, false)
  assert.deepStrictEqual([m.unknown, m.recognised, m.candidates], [[], [], []])
})

test("the closest entity to an unknown word shares its stem", () => {
  const close = plain(gapClosestEntities("onversaagd"))
  assert.deepStrictEqual(close.map((c) => [c.name, c.via]), [["MOED", "onversaagdheid"]])
  assert.deepStrictEqual(plain(gapClosestEntities("zz")), [], "too short to guess from")
})

if (failed) {
  console.error("GAP fix: " + failed + " check(s) failed")
  process.exit(1)
}
console.log("GAP fix: all checks passed")
