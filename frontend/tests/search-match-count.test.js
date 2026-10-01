// Where a result matched — the worker's `matchInfo`, run against its real
// source. A card says "3 of 40 nodes match" and the info modal's "Matches only"
// shows those nodes, so the count has to be the search's own reading: a part
// counts when every word of the search sits in it, not when any one word does.
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

const answer = (text, isDefault = true) => ({
  type: "Answer",
  isDefault,
  data: { text, hyperlinks: [] },
  contextVariables: [],
  metadata: {},
})
const node = (id, name, items, links, ref) => ({
  id,
  type: items.length || ref ? "Output" : "Recognition",
  name,
  output: { items, kbaIdReference: ref || null },
  links: links || [],
})
const says = (childNodeId, ...texts) => ({
  childNodeId,
  condition: { type: "Recognition", data: { isFallback: false, questions: texts.map((text) => ({ text })) } },
})

const ARTICLES = [
  {
    Id: 10,
    _kind: "article",
    Culture: "nl",
    Questions: [
      { Text: "korting voor kinderen", IsFaq: true },
      { Text: "kinderen gratis", IsFaq: false },
      { Text: "parkeren", IsFaq: false },
    ],
    Outputs: [
      { Type: "Answer", IsDefault: true, Text: "Kinderen tot 3 jaar zijn gratis.", Links: [], ContextVariables: [] },
      {
        Type: "Answer",
        IsDefault: false,
        Text: "Leden krijgen korting voor kinderen.",
        Links: [],
        ContextVariables: [{ Id: 1, Values: ["leden"] }],
      },
      { Type: "Answer", IsDefault: false, Text: "Geen korting vandaag.", Links: [], ContextVariables: [{ Id: 1, Values: ["gast"] }] },
    ],
    Categories: [],
  },
  // The words only meet across two questions — the questions are one part.
  {
    Id: 11,
    _kind: "article",
    Culture: "nl",
    Questions: [
      { Text: "hotel boeken", IsFaq: true },
      { Text: "annuleren", IsFaq: false },
      { Text: "parkeren", IsFaq: false },
    ],
    Outputs: [{ Type: "Answer", IsDefault: true, Text: "Zie de website.", Links: [], ContextVariables: [] }],
    Categories: [],
  },
]

const DIALOGS = [
  {
    id: 100,
    name: "Tarieven",
    description: "",
    _kind: "dialog",
    nodes: [
      node(1, "Menu", [answer("Waar gaat het om?")], [says(2, "korting kinderen"), says(3, "jaarkaart")]),
      node(2, "Kinderen", [answer("Kinderen betalen minder. Vraag naar korting!")]),
      node(3, "Jaarkaart", [answer("Met korting op de jaarkaart.")]),
      node(4, "Groepen", [answer("Kinderen in groepen.")]),
      node(5, "Kinderen korting", [], [], 10), // shows Article 10: a reference
    ],
  },
  // Tags: node 1 is tagged nochat and says korting; node 2 is conditioned on
  // DeviceType — stored, as the export does, one entry per value; node 3 routes
  // on it; node 4 has neither.
  {
    id: 200,
    name: "Apparaat",
    description: "",
    _kind: "dialog",
    nodes: [
      node(1, "Geen chat", [{ ...answer("Geen korting via chat."), metadata: { nochat: "true" } }]),
      node(2, "Mobiel", [
        {
          ...answer("Op je telefoon.", false),
          contextVariables: [{ id: 2, value: "Tablet" }, { id: 2, value: "Mobile" }, { id: 2, value: "Unknown" }],
        },
      ]),
      node(3, "Doorsturen", [
        { type: "DialogStart", isDefault: true, data: { dialogId: 100 }, contextVariables: [{ id: 2, value: "Mobile" }], metadata: {} },
      ]),
      node(4, "Korting", [answer("Korting voor iedereen.")]),
    ],
  },
]

function search(query, extra) {
  const ctx = { postMessage: () => {}, onmessage: null }
  new Function("self", "postMessage", src)(ctx, () => {})
  const out = []
  ctx.postMessage = (m) => out.push(m)
  ctx.onmessage({
    data: {
      type: "init",
      json: JSON.stringify({
        articles: ARTICLES,
        dialogs: DIALOGS,
        entities: [],
        convVars: [],
        ctxVars: [{ id: 1, name: "klant" }, { id: 2, name: "DeviceType" }],
      }),
    },
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
  const all = ARTICLES.concat(DIALOGS)
  const byKey = {}
  for (const [gidx, info] of r.matchInfo) {
    const it = all[gidx]
    byKey[it._kind === "article" ? "qa-" + it.Id : "dn-" + it.id] = info
  }
  return byKey
}
const nodeIds = (info, d = DIALOGS[0]) => info.n.map((i) => d.nodes[i].id)
const tagIds = (info) => nodeIds(info, DIALOGS[1])

test("a node counts when it holds every word, not any one", () => {
  // Node 2 says both; node 1's phrasing says both; 3 and 4 hold one each.
  assert.deepStrictEqual(nodeIds(search("kinderen korting")["dn-100"]), [1, 2])
})

test("text chips ANDed together count the same nodes as one chip", () => {
  assert.deepStrictEqual(nodeIds(search("", { expr: "kinderen AND korting" })["dn-100"]), [1, 2])
})

test("OR counts a node holding either", () => {
  assert.deepStrictEqual(nodeIds(search("", { expr: "kinderen OR korting" })["dn-100"]), [1, 2, 3, 4])
})

test("an exclusion is no reason a node matched", () => {
  assert.deepStrictEqual(nodeIds(search("", { expr: "kinderen AND NOT zwembad" })["dn-100"]), [1, 2, 4])
})

test("a phrasing is credited to the node whose link carries it", () => {
  assert.deepStrictEqual(nodeIds(search("jaarkaart")["dn-100"]), [1, 3])
})

test("a reference node counts only under Ref", () => {
  assert.ok(!nodeIds(search("kinderen korting")["dn-100"]).includes(5))
  assert.ok(nodeIds(search("kinderen korting", { searchIncludeRefs: true })["dn-100"]).includes(5))
})

test("under ¬T only Responses count", () => {
  // Node 1 matched by its phrasing, which ¬T leaves out.
  assert.deepStrictEqual(nodeIds(search("kinderen korting", { searchContent: true })["dn-100"]), [2])
})

test("an Article counts its Responses and its questions by index", () => {
  const a = search("korting kinderen")["qa-10"]
  assert.deepStrictEqual(a.r, [1]) // the contextual one says both
  assert.deepStrictEqual(a.q, [0])
})

test("ND leaves the non-default Responses out of the count", () => {
  assert.deepStrictEqual(search("korting", { searchExcludeNonDefault: true })["qa-10"].r, [])
  assert.deepStrictEqual(search("korting")["qa-10"].r, [1, 2])
})

test("under a Context filter only the Responses it lets through count, and questions don't", () => {
  const a = search("korting", { contentContextFilters: [{ name: "klant", value: "leden" }] })["qa-10"]
  assert.deepStrictEqual(a.r, [1])
  assert.deepStrictEqual(a.q, [])
})

test("words spread over an Article's questions still say which questions", () => {
  // Found because the questions are one part; no single question holds both.
  const a = search("hotel annuleren")["qa-11"]
  assert.deepStrictEqual(a.q, [0, 1])
  assert.deepStrictEqual(a.r, [])
})

test("an item found by id alone carries no count", () => {
  assert.deepStrictEqual(search("", { expr: "qa-10 OR dn-100" }), {})
})

test("an item that did not match carries none either", () => {
  const r = search("jaarkaart")
  assert.ok(!("qa-10" in r) && !("qa-11" in r))
})

test("a meta: chip counts the node whose Response carries the tag", () => {
  assert.deepStrictEqual(tagIds(search("", { expr: 'meta:"nochat"="true" AND meta:"nochat"="true"' })["dn-200"]), [1])
})

test("a ctx: chip counts a value before the last of a repeated condition, on a Response or a route", () => {
  assert.deepStrictEqual(tagIds(search("", { expr: 'ctx:"DeviceType"="Mobile" AND ctx:"DeviceType"="Mobile"' })["dn-200"]), [2, 3])
})

test("the panel alone counts the same nodes as the chip", () => {
  assert.deepStrictEqual(tagIds(search("", { contentContextFilters: [{ name: "DeviceType", value: "Mobile" }] })["dn-200"]), [2, 3])
  assert.deepStrictEqual(tagIds(search("", { contentMetadataFilters: [{ name: "nochat", value: "true" }] })["dn-200"]), [1])
})

test("words and a tag count the node holding both", () => {
  // Nodes 1 and 4 say korting; only node 1 is tagged.
  assert.deepStrictEqual(tagIds(search("", { expr: 'korting AND meta:"nochat"="true"' })["dn-200"]), [1])
  assert.deepStrictEqual(tagIds(search("korting", { contentMetadataFilters: [{ name: "nochat", value: "true" }] })["dn-200"]), [1])
})

test("when no node holds both, the words are counted, then the tag", () => {
  // "telefoon" is in node 2; the tag in node 1 — nothing holds both.
  assert.deepStrictEqual(tagIds(search("", { expr: 'telefoon AND meta:"nochat"="true"' })["dn-200"]), [2])
})

test("a chip value is matched trimmed on both sides", () => {
  assert.ok("dn-200" in search("", { expr: 'meta:"nochat"="true " AND meta:"nochat"="true"' }))
})

console.log(failures ? "\n" + failures + " failing" : "Search match counts: all checks passed")
process.exit(failures ? 1 : 0)
