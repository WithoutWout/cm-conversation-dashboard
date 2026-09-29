// A Dialog matches on its own content, not on the Articles it references.
//
// A Dialog node can show an Article instead of a Response of its own
// (`output.kbaIdReference`, with no output items). On the real export 2639
// nodes do. Such a node's name is a label for the reference — "Telefoonnummer"
// on a node pointing at the telefoonnummer Article — so a search for that
// Article's subject used to return every Dialog that happened to reference it.
// The Ref toggle (`searchIncludeRefs`) is the way back to that reading, and
// with it on the referenced Article's text counts too.
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

const ARTICLES = [
  {
    Id: 1764,
    Culture: "nl",
    Questions: [{ Text: "telefoonnummer klantenservice", IsFaq: true }],
    Outputs: [
      { Type: "Answer", Text: "Bel ons op 0416 288 111.", IsDefault: true, Links: [], ContextVariables: [] },
    ],
  },
]

const node = (id, name, extra) => ({
  id,
  type: "Output",
  name,
  output: { items: [], kbaIdReference: null, ...extra },
  links: [],
})
const answer = (text) => ({
  type: "Answer",
  isDefault: true,
  data: { text, hyperlinks: [] },
  contextVariables: [],
  metadata: {},
})

const DIALOGS = [
  // Only references the Article, through a node named after it.
  { id: 642, name: "Contact", description: "", _kind: "dialog", nodes: [node(1, "Telefoonnummer", { kbaIdReference: 1764 })] },
  // Says it itself.
  { id: 700, name: "Bellen", description: "", _kind: "dialog", nodes: [node(2, "Nummer", { items: [answer("Ons telefoonnummer is 0416 288 111.")] })] },
  // Reaches the reference through what the user says — the Dialog's own — and
  // through a GoTo named after it, which is part of the reference.
  {
    id: 900,
    name: "Contact menu",
    description: "",
    _kind: "dialog",
    nodes: [
      {
        id: 1,
        type: "Recognition",
        name: "Menu",
        output: { items: [answer("Waarmee kan ik helpen?")], kbaIdReference: null },
        links: [
          { childNodeId: 2, condition: { type: "Recognition", data: { isFallback: false, questions: [{ text: "Telefoonnummer klantenservice" }] } } },
          { childNodeId: 4, condition: { type: "Recognition", data: { isFallback: false, questions: [{ text: "Openingstijden kantoor" }] } } },
        ],
      },
      node(2, "Bellen", { kbaIdReference: 1764 }),
      { id: 3, type: "GoTo", name: "GoTo - Belhulp", output: { items: [], kbaIdReference: null }, links: [{ childNodeId: 2, condition: { type: "GoTo", data: {} } }] },
      node(4, "Kantoor", { items: [answer("Van negen tot vijf.")] }),
    ],
  },
  // A node of its own that happens to carry the word in its name.
  { id: 800, name: "Service", description: "", _kind: "dialog", nodes: [node(3, "Telefoonnummer vragen", { items: [answer("Waarvoor wil je bellen?")] })] },
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
        articles: ARTICLES.map((a) => ({ ...a, _kind: "article" })),
        dialogs: DIALOGS,
        entities: [],
        convVars: [],
        ctxVars: [],
      }),
    },
  })
  ctx.onmessage({
    data: {
      type: "search",
      id: 1,
      query,
      allFilterPill: "all",
      aFilter: "all",
      dFilter: "all",
      eFilter: "all",
      searchCase: false,
      searchWord: false,
      searchRegex: false,
      searchContent: false,
      searchExcludeNonDefault: false,
      allSort: "id-asc",
      aSort: "id-asc",
      dSort: "id-asc",
      eSort: "name-asc",
      contentContextFilters: [],
      contentMetadataFilters: [],
      ...extra,
    },
  })
  const r = out.find((m) => m.type === "results")
  return Array.from(r.filteredDialogsIdx).map((i) => DIALOGS[i].id)
}

test("a reference node's name does not make its Dialog match", () => {
  // 642 only has the reference node; 900 is here for a phrasing of its own.
  assert.ok(!search("telefoonnummer").includes(642))
})

test("the referenced Article's text does not either", () => {
  assert.deepStrictEqual(search("288"), [700])
})

test("a node of the Dialog's own still matches by name", () => {
  assert.deepStrictEqual(search("vragen"), [800])
})

test("what the user says counts, even when it routes to a reference", () => {
  assert.deepStrictEqual(search("klantenservice"), [900])
  assert.deepStrictEqual(search("telefoonnummer"), [700, 800, 900])
  assert.deepStrictEqual(search("kantoor"), [900])
})

test("a GoTo named after a reference is the reference", () => {
  assert.deepStrictEqual(search("belhulp"), [])
  assert.deepStrictEqual(search("belhulp", { searchIncludeRefs: true }), [900])
})

test("the Ref toggle lets both count again", () => {
  assert.deepStrictEqual(search("telefoonnummer", { searchIncludeRefs: true }), [642, 700, 800, 900])
  assert.deepStrictEqual(search("288", { searchIncludeRefs: true }), [642, 700, 900])
})

test("Responses only (¬T) never read a reference's name, Ref or not", () => {
  assert.deepStrictEqual(search("belhulp", { searchContent: true, searchIncludeRefs: true }), [])
})

console.log(failures ? "\n" + failures + " failing" : "Content references: all checks passed")
process.exit(failures ? 1 : 0)
