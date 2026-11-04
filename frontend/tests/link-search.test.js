// A Response's URLs are searched: the export keeps them beside the text, which
// holds only `%{Link(1)}` and the label — so searching a URL found 666 of the
// 3557 places one sits in the real export. Runs the worker's real source, and
// the renderer's real `hlVar`, since a hit has to show as a link on the card.
const fs = require("fs")
const path = require("path")
const assert = require("assert")
const vm = require("vm")
const { extract } = require("./extract")

// A URL is no id, so the id reader this calls has nothing to say about one.
const _convEscapeText = vm.runInNewContext("(" + extract("_convEscapeText") + ")", {
  convParseIdToken: () => null,
})

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

const URL_A = "https://www.efteling.com/nl/specials/reserveerlijst-verblijf"
const ARTICLES = [
  {
    Id: 1,
    _kind: "article",
    Culture: "nl",
    Questions: [{ Text: "oud en nieuw", IsFaq: true }],
    Categories: [],
    Outputs: [
      {
        Type: "Answer",
        IsDefault: true,
        Text: "%{Link(1)} voor het feest.",
        Links: [{ TagId: 1, Label: "Schrijf je in", URL: URL_A, Target: 0 }],
        Images: [],
        Videos: [],
        ContextVariables: [],
      },
    ],
  },
  {
    Id: 2,
    _kind: "article",
    Culture: "nl",
    Questions: [{ Text: "plattegrond", IsFaq: true }],
    Categories: [],
    Outputs: [
      {
        Type: "Answer",
        IsDefault: true,
        Text: "Kijk hier %{Image(1)} %{Video(1)}",
        Links: [],
        Images: [{ TagId: 1, Name: "https://cdn.example.com/plattegrond-kaart.png" }],
        Videos: [{ TagId: 1, Url: "https://video.example.com/rondleiding" }],
        ContextVariables: [],
      },
    ],
  },
  {
    Id: 3,
    _kind: "article",
    Culture: "nl",
    Questions: [{ Text: "los antwoord", IsFaq: true }],
    Categories: [],
    Outputs: [
      { Type: "Answer", IsDefault: true, Text: "Zie " + URL_A, Links: [], Images: [], Videos: [], ContextVariables: [] },
    ],
  },
]
const answer = (data) => ({ type: "Answer", isDefault: true, data, contextVariables: [], metadata: {} })
const DIALOGS = [
  {
    id: 100,
    _kind: "dialog",
    name: "Verblijf",
    nodes: [
      {
        id: 1,
        type: "Output",
        name: "Inschrijven",
        output: {
          items: [
            answer({
              text: "%{Link(1)} nu.",
              hyperlinks: [{ id: 1, label: "Aanmelden", urlTemplate: URL_A, target: 3 }],
              images: [{ id: 1, name: "https://cdn.example.com/hotel-foto.jpg" }],
              videos: [{ id: 1, url: "https://video.example.com/hotelkamer" }],
            }),
          ],
        },
        links: [],
      },
      {
        id: 2,
        type: "Output",
        name: "Anders",
        output: {
          items: [
            answer({
              text: "%{Link(1)}",
              hyperlinks: [
                { id: 1, label: "Boek", urlTemplate: "https://www.efteling.com/boek?g=%{ConversationVariable(7)}", target: 3 },
              ],
            }),
          ],
        },
        links: [],
      },
    ],
  },
]

const ctx = { postMessage: () => {}, onmessage: null }
new Function("self", "postMessage", src)(ctx, () => {})
let out = []
ctx.postMessage = (m) => out.push(m)
ctx.onmessage({
  data: {
    type: "init",
    json: JSON.stringify({
      articles: ARTICLES,
      dialogs: DIALOGS,
      entities: [],
      convVars: [{ id: 7, name: "gasten" }],
      ctxVars: [],
    }),
  },
})
const ALL = ARTICLES.concat(DIALOGS)
function search(query, extra) {
  out = []
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
  return { ids: [...r.filteredAllIdx].map((i) => ALL[i].Id || ALL[i].id).sort((a, b) => a - b), r }
}
const ids = (q, extra) => search(q, extra).ids

test("a whole URL finds it as a link, in the text and in a Dialog node", () => {
  assert.deepStrictEqual(ids(URL_A), [1, 3, 100])
})
test("so does a part of one", () => {
  assert.deepStrictEqual(ids("reserveerlijst"), [1, 3, 100])
  assert.deepStrictEqual(ids("specials/reserveerlijst"), [1, 3, 100])
  assert.deepStrictEqual(ids("efteling.com/nl/specials"), [1, 3, 100])
})
test("as a chip in an expression too, serialised as the bar serialises it", () => {
  const chip = _convEscapeText(URL_A)
  assert.deepStrictEqual(ids("", { expr: chip + " AND NOT feest" }), [3, 100])
  assert.deepStrictEqual(ids("", { expr: chip + " OR rondleiding" }), [1, 2, 3, 100])
  assert.deepStrictEqual(ids("", { expr: _convEscapeText("reserveerlijst") + " AND NOT feest" }), [3, 100])
})
test("image and video URLs are found, on Articles and on Dialog nodes", () => {
  assert.deepStrictEqual(ids("plattegrond-kaart.png"), [2])
  assert.deepStrictEqual(ids("rondleiding"), [2])
  assert.deepStrictEqual(ids("hotel-foto"), [100])
  assert.deepStrictEqual(ids("hotelkamer"), [100])
})
test("a URL belongs to the Response, so ¬T keeps it", () => {
  assert.deepStrictEqual(ids("reserveerlijst", { searchContent: true }), [1, 3, 100])
})
test("the counted part is the node that links, not the whole Dialog", () => {
  const { r } = search("reserveerlijst")
  const info = new Map(r.matchInfo).get(ALL.indexOf(DIALOGS[0]))
  assert.deepStrictEqual(info.n, [0])
})
test("a variable in a URL reads as its name, and its placeholder is not a word", () => {
  assert.deepStrictEqual(ids("gasten"), [100])
  assert.deepStrictEqual(ids("ConversationVariable"), [])
})
test("a term in the URL and a term in the text of one Response meet", () => {
  assert.deepStrictEqual(ids("reserveerlijst feest"), [1])
})

// The card draws what the worker found. A search for a whole URL marks it from
// its first character, and `(<mark>https…` used to fall out of the link pattern.
// Sliced by indentation: `extract` cannot read this one's regex literal.
const html = fs.readFileSync(path.join(__dirname, "..", "index.html"), "utf8")
const hlAt = html.indexOf("      function hlVar(")
const hlVar = vm.runInNewContext("(" + html.slice(hlAt, html.indexOf("\n      }\n", hlAt) + 8) + ")")
test("a URL marked from its first character is still a link", () => {
  const out = hlVar("[Label](<mark>https://www.efteling.com/nl</mark>)")
  assert.ok(out.includes("previewUrl('https://www.efteling.com/nl')"), out)
  assert.ok(out.includes("<mark>https://www.efteling.com/nl</mark></a>"), out)
})
test("a URL marked in the middle still is, too", () => {
  const out = hlVar("[Label](https://www.<mark>efteling</mark>.com/nl)")
  assert.ok(out.includes("previewUrl('https://www.efteling.com/nl')"), out)
})

if (failures) {
  console.log("\n" + failures + " failed")
  process.exit(1)
}
console.log("\nlink-search: all passed")
