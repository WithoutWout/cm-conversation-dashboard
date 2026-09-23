// The Content search bar as an expression: chips joined by AND / OR / NOT and
// grouped with brackets, read by the worker's port of the grammar
// (`parseContentExpr`) and evaluated per item (`evalContentExpr`).
//
// The worker is driven through its real `init` and `search` messages, so the
// precompute pass, the parser and the matcher under test are the ones that run.
// Two things matter most, and both are held below: a query that is one run of
// text must return exactly what it always did, and AND / OR / NOT must combine
// at the item level while a single text leaf keeps its same-answer rule.
const fs = require("fs")
const path = require("path")
const vm = require("vm")

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

const workerSrc = fs.readFileSync(path.join(__dirname, "..", "search-worker.js"), "utf8")
function loadWorker() {
  let last = null
  const self = { postMessage: (m) => (last = m) }
  const wctx = vm.createContext({ console, self })
  vm.runInContext(workerSrc + "\n;globalThis.__parse = parseContentExpr", wctx)
  return {
    parse: (s, re) => JSON.parse(JSON.stringify(wctx.__parse(s, !!re))),
    send: (msg) => {
      self.onmessage({ data: msg })
      return last
    },
  }
}

// ── Fixture ──────────────────────────────────────────────────────────────────
const CHANNEL = 9
const answer = (text, channel, meta = {}) => ({
  Type: "Answer",
  Text: text,
  IsDefault: channel === "any",
  OutputMetaData: meta,
  ContextVariables: [{ Id: CHANNEL, Values: [channel] }],
})
const article = (Id, question, Outputs) => ({
  Id,
  Culture: "nl",
  Questions: [{ Text: question, IsFaq: true }],
  Outputs,
  Categories: [],
})
const ARTS = [
  article(1, "waar kan ik parkeren", [answer("parkeren kost tien euro", "any", { nochat: "true" })]),
  article(2, "openingstijden", [answer("wij zijn open om tien uur", "app")]),
  article(3, "parkeren voor campers", [
    answer("campers parkeren apart", "web"),
    answer("kosten per nacht", "any"),
  ]),
]
const DLGS = [
  {
    id: 50,
    name: "Parkeer dialoog",
    nodes: [{ id: 7, type: "Output", name: "tarief", output: { items: [] }, links: [] }],
  },
]
const { send, parse } = loadWorker()
send({
  type: "init",
  json: JSON.stringify({
    articles: ARTS.map((a) => ({ ...a, _kind: "article" })),
    dialogs: DLGS.map((d) => ({ ...d, _kind: "dialog" })),
    entities: [],
    convVars: [],
    ctxVars: [{ id: CHANNEL, name: "Channel" }],
  }),
})
const keys = [...ARTS.map((a) => "a" + a.Id), ...DLGS.map((d) => "d" + d.id)]
function search(query, expr, extra = {}) {
  const res = send({
    type: "search",
    id: 1,
    query,
    expr: expr === undefined ? null : expr,
    allFilterPill: "all",
    aFilter: "all",
    dFilter: "all",
    eFilter: "all",
    searchCase: false,
    searchWord: false,
    searchRegex: false,
    searchContent: false,
    searchExcludeNonDefault: false,
    contentContextFilters: [],
    contentMetadataFilters: [],
    ...extra,
  })
  if (res.error) return res.error
  return Array.from(res.filteredAllIdx, (i) => keys[i]).sort()
}

// ── The grammar ──────────────────────────────────────────────────────────────
console.log("The worker reads the same grammar as the backend:")
eq("a sentence is one text leaf", parse("parkeren tarief"), { k: "text", raw: "parkeren tarief" })
eq("keywords are upper-case only", parse("brood and boter"), { k: "text", raw: "brood and boter" })
eq("strictly left to right", parse("qa-1 OR qa-2 AND boos").k, "and")
eq(
  "a tag reads its name and value",
  parse('ctx:"Channel"="web"'),
  { k: "tag", kind: "context", name: "Channel", values: ["web"] },
)
eq("a star is any value", parse('meta:"nochat"="*"').values, [])
eq("several values are one tag", parse('ctx:"Channel"="web","a, b"').values, ["web", "a, b"])
eq("a node id is a node", parse("dn-50-7"), { k: "id", kind: "node", id: 50, node: 7 })
eq("NOT NOT is nothing", parse("NOT NOT qa-1"), { k: "id", kind: "article", id: 1 })
eq("an unclosed group closes itself", parse("( qa-1 OR qa-2").k, "or")
eq("under .* the brackets are the pattern's", parse("a(b|c)", true), { k: "text", raw: "a(b|c)" })

// ── What it returns ──────────────────────────────────────────────────────────
console.log("\nWhat an expression returns:")
const plain = search("parkeren")
eq("one run of text is exactly the old search", search("parkeren", "parkeren"), plain)
// The dialog's name says "Parkeer", not "parkeren"; the two articles do.
eq("…which is the two parking articles", plain, ["a1", "a3"])
eq("AND is item-level: both words, possibly in different answers", search("", "campers AND nacht"), ["a3"])
eq("…where one leaf still wants both in the same answer", search("campers nacht"), [])
eq("OR unions", search("", "openingstijden OR campers"), ["a2", "a3"])
eq("AND NOT removes", search("", "parkeren AND NOT campers"), plain.filter((k) => k !== "a3"))
eq("an id is the item itself", search("", "qa-2 OR dn-50"), ["a2", "d50"])
eq("a node id needs the node", search("", "dn-50-8"), [])
eq("a context tag", search("", 'ctx:"Channel"="web"'), ["a3"])
eq("…compares values case-insensitively", search("", 'ctx:"Channel"="WEB"'), ["a3"])
eq("a metadata tag", search("", 'meta:"nochat"="true"'), ["a1"])
eq("a tag with two values matches either", search("", 'ctx:"Channel"="web","APP"'), ["a2", "a3"])
eq("a tag combines with words", search("", 'parkeren AND NOT meta:"nochat"="*"'), plain.filter((k) => k !== "a1"))
eq("a leading NOT excludes", search("", "NOT parkeren").includes("a2"), true)
eq("an invalid pattern in any leaf is reported", search("", "qa-1 OR a(", { searchRegex: true }), "invalid_regex")

console.log(out.join("\n"))
if (failed) {
  console.error(`\nContent expression: ${failed} check(s) failed`)
  process.exit(1)
}
console.log("\nContent expression: all checks passed")
