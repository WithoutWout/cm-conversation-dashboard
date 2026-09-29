// What a Dialog is searched by, run against the real worker source.
//
// Three misses measured on the 2026-08-15 export, each pinned here:
// - what the user says to move between nodes was not searched at all, so 1500
//   of 2532 distinct phrasings did not find the Dialog they belong to;
// - `%{DialogOptions()}` and friends were searched as words, so "option"
//   matched 489 Dialogs, 487 of them through the placeholder alone;
// - accents had to match exactly: "oke" found 27 Dialogs, "oké" 144.
const fs = require("fs")
const path = require("path")
const assert = require("assert")

const src = fs.readFileSync(path.join(__dirname, "..", "search-worker.js"), "utf8")

let failures = 0
function test(name, fn) {
  try {
    fn()
    console.log("  ok   " + name)
  } catch (e) {
    failures++
    console.log("  FAIL " + name + "\n       " + e.message)
  }
}

const answer = (text) => ({
  type: "Answer",
  isDefault: true,
  data: { text, hyperlinks: [] },
  contextVariables: [],
  metadata: {},
})
const node = (id, name, items, links) => ({
  id,
  type: items.length ? "Output" : "Recognition",
  name,
  output: { items, kbaIdReference: null },
  links: links || [],
})
const says = (childNodeId, ...texts) => ({
  childNodeId,
  condition: { type: "Recognition", data: { isFallback: false, questions: texts.map((text) => ({ text })) } },
})
const fallback = (childNodeId, text) => ({
  childNodeId,
  condition: { type: "Recognition", data: { isFallback: true, questions: [{ text }] } },
})

const DIALOGS = [
  {
    id: 1025,
    name: "Afgeschreven",
    description: "",
    _kind: "dialog",
    nodes: [
      node(145, "Start", [answer("Waar gaat het om? %{DialogOptions()}")], [says(146, "Dubbel betaald"), fallback(147, "noodgeval")]),
      node(146, "Dubbel", [answer("Dat lossen we op.")]),
      node(147, "Anders", [answer("Vertel het me.")]),
    ],
  },
  {
    id: 2000,
    name: "Groeten",
    description: "",
    _kind: "dialog",
    nodes: [node(1, "Hallo", [answer("Oké, tot ziens bij Krümel!")])],
  },
  { id: 3000, name: "Oud & Nieuw", description: "", _kind: "dialog", nodes: [node(1, "Start", [answer("Vier het mee!")])] },
  { id: 3001, name: "Feest", description: "", _kind: "dialog", nodes: [node(1, "Start", [answer("Kijk op /oud-en-nieuw voor tickets.")])] },
  { id: 3002, name: "Oudjaar", description: "", _kind: "dialog", nodes: [node(1, "Start", [answer("Het oude jaar en het nieuwe.")])] },
  { id: 5000, name: "Leden", description: "", _kind: "dialog", nodes: [node(1, "Start", [answer("Wij houden van onze abonnementhouders.")])] },
  { id: 5001, name: "Feest", description: "", _kind: "dialog", nodes: [node(1, "Start", [answer("Vraag naar de verjaardagskorting!")])] },
  // "hotel" in a phrasing, "annuleren" in the description: two parts.
  {
    id: 4000,
    name: "Reserveren",
    description: "Wijzigen of annuleren",
    _kind: "dialog",
    nodes: [
      node(1, "Menu", [answer("Waarmee kan ik helpen?")], [says(2, "hotel boeken")]),
      node(2, "Boeken", [answer("Kies een datum.")]),
      node(3, "Kosten", [answer("Een hotel annuleren kost niets.")]),
    ],
  },
]

function search(query, extra) {
  const ctx = { postMessage: () => {}, onmessage: null }
  new Function("self", "postMessage", src)(ctx, () => {})
  const out = []
  ctx.postMessage = (m) => out.push(m)
  ctx.onmessage({
    data: { type: "init", json: JSON.stringify({ articles: [], dialogs: DIALOGS, entities: [], convVars: [], ctxVars: [] }) },
  })
  ctx.onmessage({
    data: {
      type: "search", id: 1, query,
      allFilterPill: "all", aFilter: "all", dFilter: "all", eFilter: "all",
      searchCase: false, searchWord: false, searchRegex: false, searchContent: false,
      searchExcludeNonDefault: false, allSort: "id-asc", aSort: "id-asc", dSort: "id-asc", eSort: "name-asc",
      contentContextFilters: [], contentMetadataFilters: [],
      ...extra,
    },
  })
  const r = out.find((m) => m.type === "results")
  return Array.from(r.filteredDialogsIdx).map((i) => DIALOGS[i].id)
}

test("what the user says finds the Dialog", () => {
  assert.deepStrictEqual(search('"dubbel betaald"'), [1025])
})

test("a fallback link's text is not a phrasing", () => {
  assert.deepStrictEqual(search("noodgeval"), [])
})

test("phrasings are off under Responses only (¬T), like an Article's questions", () => {
  assert.deepStrictEqual(search('"dubbel betaald"', { searchContent: true }), [])
})

test("a placeholder is not a word", () => {
  assert.deepStrictEqual(search("options"), [])
  assert.deepStrictEqual(search("dialog"), [])
})

test("accents are ignored in a plain search, both ways", () => {
  assert.deepStrictEqual(search("oke"), [2000])
  assert.deepStrictEqual(search("oké"), [2000])
  assert.deepStrictEqual(search("krumel"), [2000])
})

test("…and in a whole-word search", () => {
  assert.deepStrictEqual(search("oke", { searchWord: true }), [2000])
  assert.deepStrictEqual(search("krum", { searchWord: true }), [])
})

test("a case-sensitive search stays exact", () => {
  assert.deepStrictEqual(search("Oke", { searchCase: true }), [])
  assert.deepStrictEqual(search("Oké", { searchCase: true }), [2000])
})

test("a phrase matches however its words are joined", () => {
  assert.deepStrictEqual(search('"oud en nieuw"'), [3000, 3001])
  assert.deepStrictEqual(search('"Oud & Nieuw"'), [3000, 3001])
  assert.deepStrictEqual(search("oud-en-nieuw"), [3000, 3001])
  assert.deepStrictEqual(search('"oud nieuw"'), [3000, 3001])
})

test("…but its words still have to be next to each other", () => {
  // "oude jaar en het nieuwe" holds both words, far apart: not the phrase.
  assert.ok(!search('"oud en nieuw"').includes(3002))
})

test("…and whole-word still holds at its ends", () => {
  assert.deepStrictEqual(search('"oud en nieuw"', { searchWord: true }), [3000, 3001])
  assert.deepStrictEqual(search('"oud en nieu"', { searchWord: true }), [])
})

test("one chip's words must sit in one part of a Dialog", () => {
  assert.deepStrictEqual(search("hotel boeken"), [4000]) // one phrasing
  assert.deepStrictEqual(search("boeken annuleren"), []) // phrasing + description
})

test("text chips joined by AND must too", () => {
  assert.deepStrictEqual(search("", { expr: "hotel AND annuleren" }), [4000]) // node 3's Response
  assert.deepStrictEqual(search("", { expr: "boeken AND annuleren" }), [])
  assert.deepStrictEqual(search("", { expr: "boeken OR annuleren" }), [4000])
})

test("a short word only matches at the start of a word", () => {
  // "oud" is inside h-oud-en and abonnementh-oud-ers, never a word of its own.
  assert.ok(!search("oud").includes(5000))
  assert.ok(search("oud").includes(3000)) // "Oud & Nieuw"
  assert.deepStrictEqual(search("houd"), [5000]) // …but "houd" starts "houden"
})

test("a longer word still matches inside a compound", () => {
  assert.deepStrictEqual(search("korting"), [5001])
})

test("a phrase is judged by its first word", () => {
  assert.ok(!search('"oud en nieuw"').includes(5000))
  assert.deepStrictEqual(search('"oud en nieuw"'), [3000, 3001])
})

test("a short word inside a compound needs no word-start under .*", () => {
  assert.ok(search("oud", { searchRegex: true }).includes(5000))
})

console.log(failures ? "\n" + failures + " failing" : "Dialog search: all checks passed")
process.exit(failures ? 1 : 0)
