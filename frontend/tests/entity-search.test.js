// Entity matching on the Content tab, run against the real worker source.
//
// The bug this file exists for is not a crash: it is two parts of one screen
// disagreeing about the same relationship. An Article card resolves each
// question phrase to an entity by name, then by word, then by longest token,
// and draws a chip saying so — while the *search* behind it required the
// phrase to be that entity's name verbatim. So a card could read
// "Entity: PARKEREN" and a search for one of PARKEREN's other words would not
// return the Article the chip was on. Nothing looks wrong in either half.
const fs = require("fs")
const path = require("path")
const assert = require("assert")

const src = fs.readFileSync(
  path.join(__dirname, "..", "search-worker.js"),
  "utf8",
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

// A fresh worker per call: `init` is once-only and the module holds state.
function worker() {
  const ctx = { postMessage: () => {}, onmessage: null }
  new Function("self", "postMessage", src)(ctx, () => {})
  return ctx
}

const SEARCH_DEFAULTS = {
  type: "search",
  id: 1,
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
}

function run({ articles = [], entities = [], phraseEntity, query }) {
  const ctx = worker()
  const out = []
  ctx.postMessage = (m) => out.push(m)
  ctx.onmessage({
    data: {
      type: "init",
      json: JSON.stringify({
        articles,
        dialogs: [],
        entities,
        convVars: [],
        ctxVars: [],
        entityArticleNames: [],
        entityDialogNames: [],
        ...(phraseEntity ? { phraseEntity } : {}),
      }),
    },
  })
  ctx.onmessage({ data: { ...SEARCH_DEFAULTS, query } })
  const r = out.find((m) => m.type === "results")
  return {
    articles: r ? Array.from(r.filteredArticlesIdx || []).map((i) => articles[i].Id) : [],
    entities: r ? Array.from(r.filteredEntitiesIdx || []).map((i) => entities[i].name) : [],
  }
}

const PARKEREN = {
  name: "PARKEREN",
  type: "Regex",
  description: "Alles over parkeerplaatsen en de garage",
  words: [
    { text: "parkeerplaats", wordInBetween: "", expression: "" },
    { text: "parkeergarage", wordInBetween: "", expression: "" },
  ],
}
const ARTICLES = [
  {
    Id: 1,
    _kind: "article",
    Questions: [{ Text: "waar is de parkeergarage" }],
    Outputs: [{ Type: "Answer", Text: "Bij de ingang." }],
    Categories: [],
  },
  {
    Id: 2,
    _kind: "article",
    Questions: [{ Text: "hoe laat open" }],
    Outputs: [{ Type: "Answer", Text: "Om tien uur." }],
    Categories: [],
  },
]

console.log("Entity search")

test("an Article is found through the entity its question phrase resolves to", () => {
  // The phrase is not the entity's name, so this is exactly the case the card
  // already handled and the search did not.
  const found = run({
    articles: ARTICLES,
    entities: [PARKEREN],
    phraseEntity: ["WAAR IS DE PARKEERGARAGE", "PARKEREN"],
    query: "parkeerplaats",
  })
  assert.deepStrictEqual(found.articles, [1])

  // …and without the resolution shipped from the main thread it is not — which
  // is what makes the assertion above about the fix rather than about the
  // Article's own text.
  assert.deepStrictEqual(
    run({ articles: ARTICLES, entities: [PARKEREN], query: "parkeerplaats" }).articles,
    [],
  )
})

test("an unrelated Article is not dragged in by the enrichment", () => {
  const found = run({
    articles: ARTICLES,
    entities: [PARKEREN],
    phraseEntity: ["WAAR IS DE PARKEERGARAGE", "PARKEREN"],
    query: "parkeerplaats",
  })
  assert.ok(!found.articles.includes(2), "an Article with no matching entity matched")
})

test("an entity is findable by its type and its description", () => {
  const byType = run({ entities: [PARKEREN], query: "Regex" })
  assert.deepStrictEqual(byType.entities, ["PARKEREN"], "type is not searchable")
  const byDesc = run({ entities: [PARKEREN], query: "garage" })
  assert.deepStrictEqual(byDesc.entities, ["PARKEREN"], "description is not searchable")
  // The description is the one field saying what an entity is *for*, and it
  // was parsed and discarded by the extractor for as long as it existed.
  const miss = run({ entities: [PARKEREN], query: "souvenirs" })
  assert.deepStrictEqual(miss.entities, [])
})

test("an entity is findable by the texts it actually matches on", () => {
  // `wordInBetween` and `expression` are what an entity matches on at runtime,
  // so an entity findable in CM.com by one of them must be findable here.
  const ent = {
    name: "KAARTJE",
    type: "Standard",
    description: "",
    words: [{ text: "ticket", wordInBetween: "entree", expression: "kaart(je)?" }],
  }
  assert.deepStrictEqual(run({ entities: [ent], query: "entree" }).entities, ["KAARTJE"])
  assert.deepStrictEqual(run({ entities: [ent], query: "kaart(je)?" }).entities, ["KAARTJE"])
})

test("a regex source does not drag an Article into the results", () => {
  // The enrichment reads trigger words only. An entity whose *expression*
  // happens to contain the search term has not been "found in" an Article —
  // that would be a match on a pattern nobody wrote as content.
  const ent = {
    name: "KAARTJE",
    type: "Standard",
    description: "",
    words: [{ text: "ticket", wordInBetween: "", expression: "parkeerplaats|kaartje" }],
  }
  const found = run({
    articles: ARTICLES,
    entities: [ent],
    phraseEntity: ["WAAR IS DE PARKEERGARAGE", "KAARTJE"],
    query: "parkeerplaats",
  })
  assert.deepStrictEqual(found.articles, [], "a regex source enriched an Article")
  // The entity itself is still findable by it, which is the Entities tab's job.
  assert.deepStrictEqual(found.entities, ["KAARTJE"])
})

console.log(
  failures ? `\nEntity search: ${failures} FAILED` : "\nEntity search: all checks passed",
)
process.exit(failures ? 1 : 0)
