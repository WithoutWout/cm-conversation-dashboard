// condenseEntityPhrases: an Article's phrases read as the Entities they are
// made of, each listed once. Runs the real source out of index.html.
const vm = require("vm")
const { extract, extractConst } = require("./extract")

const NAMES = ["condenseEntityPhrases", "_phraseEntityTags", "_entityAt", "_rowTrigger", "_triggerTexts", "_triggerTerms", "_coverPhrases"]
const ctx = vm.createContext({ console })
vm.runInContext(`
  let entityMap = new Map(), entityWordMap = new Map()
  let _condenseCache = new Map(), _entityWordMaxTokens = 0
  let cmCondenseEntities = true
  let cmTriggerStyle = "names"
  ${extractConst("_CONDENSE_EDGE_PUNCT")}
  ${NAMES.map(extract).join("\n")}
  // Mirrors the load in index.html: name → entity, lowercase word → first entity.
  function setEntities(list) {
    entityMap = new Map(list.map((e) => [e.name.toUpperCase(), e]))
    entityWordMap = new Map()
    for (const e of list) for (const w of e.words) {
      const k = w.text.toLowerCase()
      if (!entityWordMap.has(k)) entityWordMap.set(k, e)
    }
    _condenseCache = new Map()
    _entityWordMaxTokens = 0
  }
  function setCondense(on) { cmCondenseEntities = on }
  function setStyle(v) { cmTriggerStyle = v }
`, ctx)

const out = []
let failed = 0
const ok = (n, c) => { if (!c) failed++; out.push((c ? "  PASS  " : "  FAIL  ") + n) }
const texts = (phrases) => ctx.condenseEntityPhrases(phrases).map((t) => t.text)
const E = (name, ...words) => ({ name, words: words.map((text) => ({ text })) })

out.push("Without an EntitiesExport:")
ctx.setEntities([])
ok("phrases are kept whole, only de-duplicated (case-insensitively)",
   JSON.stringify(texts(["Flight tag kopen", "flight tag kopen", "parkeren"])) ===
   JSON.stringify(["Flight tag kopen", "parkeren"]))

ctx.setEntities([
  E("OVERWINNINGSBANDJE", "overwinningsbandje", "overwinningsbandjes"),
  E("KOPEN", "kopen", "kopen", "aanschaffen"),
  E("PRIJS", "prijs", "kosten", "wat kost"),
  E("FLIGHTTAG", "flight tag", "flighttag"),
  E("AANBOD", "een aanbod"),
  E("IN", "in"),
])

out.push("")
out.push("With Entities:")
ok("the running example condenses to each Entity once, first-seen order",
   JSON.stringify(texts([
     "overwinningsbandje kopen", "overwinningsbandje prijs", "flight tag kopen",
     "flighttag prijs", "overwinningsbandjes",
   ])) === JSON.stringify(["OVERWINNINGSBANDJE", "KOPEN", "PRIJS", "FLIGHTTAG"]))
ok("synonyms collapse into their one Entity",
   JSON.stringify(texts(["aanschaffen", "kopen"])) === JSON.stringify(["KOPEN"]))
ok("a multi-word Entity word is matched before its parts",
   JSON.stringify(texts(["wat kost een aanbod"])) === JSON.stringify(["PRIJS", "AANBOD"]))
ok("a word no Entity claims is kept, lowercased, once",
   JSON.stringify(texts(["Overwinningsbandje Efteling", "efteling kopen"])) ===
   JSON.stringify(["OVERWINNINGSBANDJE", "efteling", "KOPEN"]))
ok("a lone token under three letters never resolves to an Entity",
   JSON.stringify(texts(["kopen in park"])) === JSON.stringify(["KOPEN", "in", "park"]))
ok("edge punctuation does not stop a word from resolving",
   JSON.stringify(texts(["kopen?", "(prijs)"])) === JSON.stringify(["KOPEN", "PRIJS"]))
const counted = ctx.condenseEntityPhrases(["kopen kopen", "kopen prijs", "prijs"])
ok("`from` counts phrases, not occurrences",
   counted[0].from === 2 && counted[1].from === 2)
ok("an Entity tag carries its entity; a kept word carries none",
   ctx.condenseEntityPhrases(["kopen efteling"]).map((t) => !!t.entity).join() === "true,false")

out.push("")
out.push("The export trigger follows the trigger style (Article 10348's real phrases):")
ctx.setEntities([
  E("ENTREETICKET", "ticket", "tickets", "entreebewijs"),
  E("DUBBEL", "dubbel"),
  E("BETALEN", "betaald", "betalen"),
  E("AFSCHRIJVEN", "afgeschreven", "afschrijving"),
  E("NOG", "nog"),
  E("KAART", "kaart"),
])
const art = [
  "ticket dubbel betaald", "ticket dubbel afgeschreven", "ticket dubbel afgeschreven betaald",
  "ticket nog een keer afgeschreven", "kaart dubbel afgeschreven", "kaart dubbel betaald",
  "kaart nog een keer afgeschreven",
]
ctx.setStyle("names")
ok("names: each Entity once, by its CM.com name",
   ctx._rowTrigger(art) === "ENTREETICKET | DUBBEL | BETALEN | AFSCHRIJVEN | NOG | een | keer | KAART")
ctx.setStyle("words")
ok("words: the same Entities, written as the phrases write them",
   ctx._rowTrigger(art) === "ticket | dubbel | betaald | afgeschreven | nog | een | keer | kaart")
ok("words: the form the phrases use most wins",
   ctx._rowTrigger(["tickets kopen", "tickets prijs", "ticket kwijt"]).startsWith("tickets"))
ctx.setStyle("phrases")
ok("phrases: the fewest phrases that hold every Entity and word",
   ctx._rowTrigger(art) === "kaart nog een keer afgeschreven | ticket dubbel betaald")
const cover = ctx._triggerTerms(art)
ok("phrases: every Entity and word of the Article is in the cover",
   new Set(cover.flatMap((t) => t.keys)).size === 8)
ok("phrases: a term's keys are only the ones it added", cover[1].keys.length === 3 && cover[1].from === 3)
ok("phrases: what a row already holds is not covered twice",
   ctx._triggerTerms(art, new Set(cover[0].keys)).map((t) => t.text).join() === "ticket dubbel betaald")
ok("phrases: a tie goes to the shorter phrase",
   ctx._rowTrigger(["ticket dubbel betaald", "dubbel betaald ticket!!"]) === "ticket dubbel betaald")
ctx.setStyle("all")
ok("all: every phrase verbatim", ctx._rowTrigger(art) === art.join(" | "))
ctx.setEntities([])
ctx.setStyle("phrases")
ok("phrases without an EntitiesExport: covers the words",
   ctx._rowTrigger(["dubbel betaald", "betaald dubbel", "dubbel afgeschreven"]) === "dubbel betaald | dubbel afgeschreven")

console.log(out.join("\n"))
console.log("")
console.log(failed ? failed + " FAILED" : "Entity condensing: all checks passed")
process.exit(failed ? 1 : 0)
