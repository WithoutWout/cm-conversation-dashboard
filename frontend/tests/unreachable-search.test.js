// An unreachable output is not content: a Response (or a route) that is not
// the default and has no context condition is never shown — CM.com marks it
// "A non-default output requires context variables in order to be
// triggered" — so the app ignores it everywhere. It does that in one place,
// `pruneUnreachable` as the export loads; here its result is fed to the real
// search worker, on fixtures in the export's own shapes.
const fs = require("fs")
const path = require("path")
const vm = require("vm")
const assert = require("assert")
const { extract } = require("./extract")

const src = fs.readFileSync(path.join(__dirname, "..", "search-worker.js"), "utf8")
const page = vm.createContext({})
vm.runInContext(
  ["articleAnswerHasContext", "dialogAnswerHasContext", "pruneUnreachable"].map(extract).join("\n") +
    "\nglobalThis.prune = pruneUnreachable",
  page,
)

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

const CTX = 1
// The Oud & Nieuw Article from CM.com: its default output starts a Dialog,
// and its two Answers have no condition — CM.com shows both as unreachable.
const ARTICLES = [
  {
    Id: 1,
    Culture: "nl",
    Questions: [{ Text: "oud en nieuw", IsFaq: true }],
    Outputs: [
      { Type: "DialogStart", DialogId: 50, IsDefault: true, ContextVariables: [] },
      { Type: "Answer", Text: "Het programma is nog niet bekend.", IsDefault: false, Links: [], ContextVariables: [] },
      { Type: "Answer", Text: "Nog geen tickets te koop.", IsDefault: false, Links: [], ContextVariables: [{ Id: CTX, Values: ["any"] }] },
    ],
  },
  // A contextual Answer is reachable — for users in that context.
  {
    Id: 2,
    Culture: "nl",
    Questions: [{ Text: "kerst", IsFaq: true }],
    Outputs: [
      { Type: "Answer", Text: "Fijne feestdagen.", IsDefault: true, Links: [], ContextVariables: [] },
      { Type: "Answer", Text: "Het programma staat online.", IsDefault: false, Links: [], ContextVariables: [{ Id: CTX, Values: ["app"] }] },
    ],
  },
]
const item = (isDefault, text, cvs = []) => ({
  type: "Answer",
  isDefault,
  data: { text, hyperlinks: [] },
  contextVariables: cvs,
  metadata: { tag: text.split(" ")[0] },
})
const DIALOGS = [
  {
    id: 50,
    name: "Oud en Nieuw",
    description: "",
    _kind: "dialog",
    nodes: [
      {
        id: 1,
        type: "Output",
        name: "Start",
        output: { items: [item(true, "Welkom bij het feest."), item(false, "Vuurwerk om middernacht.")], kbaIdReference: null },
        links: [],
      },
    ],
  },
]

const clone = (v) => JSON.parse(JSON.stringify(v))
let PRUNED = 0
function search(query, extra, { show = false } = {}) {
  const arts = clone(ARTICLES)
  const dlgs = clone(DIALOGS)
  PRUNED = show ? 0 : page.prune(arts, dlgs)
  const ctx = { postMessage: () => {}, onmessage: null }
  new Function("self", "postMessage", src)(ctx, () => {})
  const out = []
  ctx.postMessage = (m) => out.push(m)
  ctx.onmessage({
    data: {
      type: "init",
      json: JSON.stringify({
        articles: arts.map((a) => ({ ...a, _kind: "article" })),
        dialogs: dlgs,
        entities: [],
        convVars: [],
        ctxVars: [{ id: CTX, name: "channel" }],
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
  return {
    articles: Array.from(r.filteredArticlesIdx).map((i) => ARTICLES[i].Id),
    dialogs: Array.from(r.filteredDialogsIdx).map((i) => DIALOGS[i].id),
  }
}

test("an unreachable Response does not match", () => {
  assert.deepStrictEqual(search("programma").articles, [2], "only the contextual Response says it where users see it")
  assert.deepStrictEqual(search("tickets").articles, [], "context \"any\" is no condition")
})

test("nor does one in a Dialog node", () => {
  assert.deepStrictEqual(search("vuurwerk").dialogs, [])
  assert.deepStrictEqual(search("welkom").dialogs, [50])
})

test("nor as part of an expression", () => {
  assert.deepStrictEqual(search("programma", { expr: "programma" }).articles, [2])
})

test("its metadata is not a tag the item carries", () => {
  const meta = (value) => search("", { contentMetadataFilters: [{ name: "tag", value }] }).dialogs
  assert.deepStrictEqual(meta("Welkom"), [50])
  assert.deepStrictEqual(meta("Vuurwerk"), [])
})

test("the question still finds the Article itself", () => {
  assert.deepStrictEqual(search("oud").articles, [1])
})

test("three outputs are left out, the defaults and the contextual one kept", () => {
  search("")
  assert.strictEqual(PRUNED, 3)
  const arts = clone(ARTICLES)
  page.prune(arts, clone(DIALOGS))
  assert.deepStrictEqual(arts[0].Outputs.map((o) => o.Type), ["DialogStart"], "Oud & Nieuw is a Dialog Link and nothing else")
  assert.strictEqual(arts[1].Outputs.length, 2)
})

test("with Show unreachable content on, they are found as before", () => {
  assert.deepStrictEqual(search("programma", {}, { show: true }).articles, [1, 2])
  assert.deepStrictEqual(search("vuurwerk", {}, { show: true }).dialogs, [50])
})

test("nothing is pruned where no output is flagged the default", () => {
  const arts = [{ Id: 9, Outputs: [{ Type: "Answer", IsDefault: false, ContextVariables: [] }, { Type: "Answer", IsDefault: false, ContextVariables: [] }] }]
  assert.strictEqual(page.prune(arts, []), 0)
})

if (failures) {
  console.error("Unreachable search: " + failures + " check(s) failed")
  process.exit(1)
}
console.log("Unreachable search: all checks passed")
