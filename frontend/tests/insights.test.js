// The Insights charts, run against the real source in index.html.
//
// What goes wrong in a chart builder is never "it throws". It is a bar drawn
// past its axis, a label silently clipped to a different word, a percentage
// taken over the wrong denominator, or — the one that only shows up on someone
// else's machine — an SVG that depends on this app's stylesheet and therefore
// rasterises to a blank rectangle the moment it is copied into an email. None
// of those look like failures on screen; all of them are arithmetic over a
// string, which is exactly what a test can hold.
const fs = require("fs")
const path = require("path")
const assert = require("assert")

// The whole Insights block, not function by function.
//
// It is built around two palette objects and a handful of layout constants, and
// a harness that redefined those would be testing its own copy of them — the
// first thing to drift, and the thing most worth pinning. The block is
// contiguous and has no top-level side effects, so it can simply be evaluated.
const src = fs.readFileSync(path.join(__dirname, "..", "index.html"), "utf8")
const from = src.indexOf("// ══ INSIGHTS ══")
const to = src.indexOf("// ── Wiring ──", from)
assert.ok(from > 0 && to > from, "could not find the Insights block in index.html")

// `esc` cannot come through `extract`: its body contains the regex literal
// `/"/g`, and the extractor is a brace matcher that reads that quote as opening
// a string — documented in extract.js, with "stub it in the harness" as the
// sanctioned answer. Stubbing HTML escaping would defeat the point of the
// well-formedness assertions below, so it is sliced out by indentation instead,
// which still runs the real source.
function sliceByIndent(decl) {
  const at = src.indexOf("      " + decl)
  assert.ok(at > 0, "could not find " + decl)
  const end = src.indexOf("\n      }\n", at)
  assert.ok(end > at, "could not find the end of " + decl)
  return src.slice(at, end + "\n      }".length)
}

// `new Function` rather than `vm.runInContext`, for two reasons that both bite
// silently: a top-level `const` never reaches a vm context object (only `var`
// does), and values built inside a vm realm fail `deepStrictEqual` against
// values built out here because their prototypes are different objects.
const EXPORTS = [
  "INS_THEME_SCREEN",
  "INS_THEME_EXPORT",
  "insRenderChart",
  "insChartTsv",
  "insFitText",
  "insNiceTicks",
  "insTextWidth",
  "insPct",
  "insRampColor",
  "insFillDays",
  "insFeedbackSpec",
  "insTagSpec",
  "insTagNote",
  "insUnitLine",
  "insWords",
  "insBuildCards",
  "insTiles",
  "insSearchSummary",
  "INS_SECTIONS",
  "INS_READ_SECTIONS",
]
const {
  INS_THEME_SCREEN,
  INS_THEME_EXPORT,
  insRenderChart,
  insChartTsv,
  insFitText,
  insNiceTicks,
  insTextWidth,
  insPct,
  insRampColor,
  insFillDays,
  insFeedbackSpec,
  insTagSpec,
  insTagNote,
  insUnitLine,
  insWords,
  insBuildCards,
  insTiles,
  insSearchSummary,
  INS_SECTIONS,
  INS_READ_SECTIONS,
} = new Function(
  sliceByIndent("function esc(s) {") +
    "\n" +
    src.slice(from, to) +
    "\nreturn {" +
    EXPORTS.join(",") +
    "}",
)()

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

// ── A minimal XML well-formedness check ──────────────────────────────
//
// The exported chart is loaded through `new Image()` from a data URI, which
// parses it as XML: one unescaped `&` in an entity name and the whole picture
// fails to render, silently, on the machine it was pasted into. `esc()` is
// supposed to prevent that, so this asserts the property rather than trusting it.
function assertWellFormed(svg) {
  const stack = []
  let i = 0
  while (i < svg.length) {
    const lt = svg.indexOf("<", i)
    const text = svg.slice(i, lt === -1 ? svg.length : lt)
    const bad = text.indexOf("&")
    if (bad !== -1 && !/^&(amp|lt|gt|quot|#\d+|#x[0-9a-f]+);/i.test(text.slice(bad))) {
      throw new Error("raw & in text content near: " + text.slice(bad, bad + 24))
    }
    if (lt === -1) break
    const gt = svg.indexOf(">", lt)
    if (gt === -1) throw new Error("unterminated tag")
    const tag = svg.slice(lt + 1, gt)
    if (tag.startsWith("/")) {
      const open = stack.pop()
      if (open !== tag.slice(1)) throw new Error("closed <" + tag.slice(1) + "> inside <" + open + ">")
    } else if (!tag.endsWith("/")) {
      stack.push(tag.split(/[\s>]/)[0])
    }
    i = gt + 1
  }
  if (stack.length) throw new Error("unclosed: " + stack.join(", "))
}

const bars = (n, base) => {
  const out = []
  for (let i = 0; i < n; i++) out.push({ label: "item " + i, value: (base || 10) - i })
  return out
}

console.log("Insights charts")

// The single property the entire copy-to-email feature rests on. A chart that
// needs a stylesheet renders as a blank box inside an <img>, and it does so
// only after it has been pasted somewhere — never here.
test("an exported chart depends on nothing outside itself", () => {
  const specs = [
    { kind: "columns", data: bars(6) },
    { kind: "bars", data: bars(5), total: 40 },
    { kind: "stack", data: bars(3), total: 27 },
    { kind: "heatmap", grid: Array.from({ length: 168 }, (_, i) => i % 9) },
    { kind: "area", data: bars(80, 90) },
  ]
  for (const spec of specs) {
    const { svg } = insRenderChart(spec, INS_THEME_EXPORT)
    assertWellFormed(svg)
    assert.ok(!/\sclass=/.test(svg), spec.kind + ": carries a class attribute")
    assert.ok(!/<style/.test(svg), spec.kind + ": carries a <style> block")
    assert.ok(!/url\(|href=|xlink/.test(svg), spec.kind + ": references something external")
    // Every drawn element states its own colour.
    for (const m of svg.match(/<(rect|path|line|circle|text)\b[^>]*>/g) || []) {
      assert.ok(
        /fill="|stroke="/.test(m),
        spec.kind + ": an element with no colour of its own — " + m.slice(0, 60),
      )
    }
    for (const m of svg.match(/<text\b[^>]*>/g) || []) {
      assert.ok(/font-family="/.test(m), spec.kind + ": text with no font of its own")
      assert.ok(!/-apple-system/.test(m), spec.kind + ": a font the rasteriser may not have")
    }
  }
})

// The two themes are selected for their own surfaces, not flipped. The export
// one is the one that matters: a dark chart in a white email is a black box.
test("the exported theme paints a light surface, the screen one a dark surface", () => {
  const spec = { kind: "columns", data: bars(4) }
  const dark = insRenderChart(spec, INS_THEME_SCREEN).svg
  const light = insRenderChart(spec, INS_THEME_EXPORT).svg
  assert.ok(dark.includes(INS_THEME_SCREEN.surface), "screen surface missing")
  assert.ok(light.includes(INS_THEME_EXPORT.surface), "export surface missing")
  assert.notStrictEqual(dark, light, "both themes produced the same picture")
  assert.strictEqual(INS_THEME_EXPORT.surface, "#ffffff")
})

// Nothing outside the viewBox is cropped by a scrollbar or a container — it is
// simply gone, with no sign that it was ever there. The tip label on a bar
// chart is the case that actually bit: `3,140 · 50%` is half again as wide as
// `3,140`, and the space for it used to be a constant.
// Nothing outside the viewBox is cropped by a container — it is simply gone,
// with no sign it was ever there.
function assertInsideCanvas(spec, label) {
  const { svg, width, height } = insRenderChart(spec, INS_THEME_SCREEN)
  for (const m of svg.matchAll(/<rect x="(-?[\d.]+)" y="(-?[\d.]+)" width="([\d.]+)" height="([\d.]+)"/g)) {
    const [x, y, w, h] = m.slice(1).map(Number)
    assert.ok(x >= -0.01 && y >= -0.01, label + ": a rect starts outside the canvas")
    assert.ok(x + w <= width + 0.01, label + ": a rect runs past the right edge")
    assert.ok(y + h <= height + 0.01, label + ": a rect runs past the bottom edge")
  }
  // The whole tag, then its attributes — a lazy scan with an optional
  // `text-anchor` group prefers to match nothing, so every label would be
  // measured as left-anchored and a right-aligned one would look overflowing.
  for (const m of svg.matchAll(/<text ([^>]*)>([^<]*)</g)) {
    const attrs = m[1]
    const x = Number(/\bx="(-?[\d.]+)"/.exec(attrs)[1])
    const anchor = (/text-anchor="(\w+)"/.exec(attrs) || [])[1] || "start"
    const size = Number((/font-size="([\d.]+)"/.exec(attrs) || [])[1] || 11)
    const tw = insTextWidth(m[2], size)
    const left = anchor === "end" ? x - tw : anchor === "middle" ? x - tw / 2 : x
    assert.ok(left >= -0.5, label + ': "' + m[2] + '" starts left of the canvas')
    assert.ok(left + tw <= width + 0.5, label + ': "' + m[2] + '" runs past the right edge')
  }
}

test("no mark and no label is drawn outside the chart it belongs to", () => {
  const specs = [
    { kind: "columns", data: [{ label: "a", value: 7 }, { label: "b", value: 1 }] },
    // The widest tip label this dashboard can produce: a six-figure count with
    // a share beside it, on the bar that reaches the full width of the plot.
    {
      kind: "bars",
      total: 250000,
      data: [{ label: "Article", value: 148820 }, { label: "None", value: 24 }],
    },
    { kind: "bars", data: [{ label: "EndedOrInProgress", value: 3140 }], total: 6284 },
    { kind: "stack", total: 100, data: [{ label: "all of it", value: 100 }] },
  ]
  for (const spec of specs) assertInsideCanvas(spec, spec.kind)
})

// A clipped label is worse than a shorter one: `Stoppen_Faciliteitenkaart` and
// `Stoppen_Faciliteitenpas` differ in exactly the part that gets cut off.
test("a label too long for its gutter is shortened, never clipped", () => {
  const long = "Stoppen_Faciliteitenkaart_Aanvraag_Afgebroken_Door_Gebruiker"
  const spec = { kind: "bars", data: [{ label: long, value: 5 }], total: 5 }
  const { svg } = insRenderChart(spec, INS_THEME_SCREEN)
  // Drawn text only: the full string legitimately appears in the tooltip below.
  const drawn = (svg.match(/<text\b[^>]*>([^<]*)</g) || []).join("|")
  assert.ok(!drawn.includes(long), "the whole label was drawn into the gutter")
  assert.ok(drawn.includes(long.slice(0, 12)), "the label was dropped rather than shortened")
  assert.ok(drawn.includes("…"), "no ellipsis marks what was dropped")
  // The full text stays reachable — the tooltip and the copied table both have it.
  assert.ok(svg.includes("<title>" + long), "the full label is not in the tooltip")
  assert.ok(insChartTsv(spec).includes(long), "the full label is not in the table")
  // And a label that fits is left alone.
  assert.strictEqual(insFitText("nl-NL", 200, 11), "nl-NL")
})

test("a stacked segment is labelled inside only when the label fits", () => {
  const theme = INS_THEME_SCREEN
  const spec = {
    kind: "stack",
    total: 1000,
    data: [
      { label: "big", value: 970, color: theme.good },
      { label: "sliver", value: 30, color: theme.critical },
    ],
  }
  const { svg } = insRenderChart(spec, theme)
  assert.ok(svg.includes(">97%<"), "the segment with room lost its label")
  assert.ok(!svg.includes(">3%<"), "a label was drawn into a segment too small for it")
  // Nothing is lost: the legend under the bar carries every segment.
  assert.ok(svg.includes("sliver"), "the sliver is missing from the legend")
  assert.ok(insChartTsv(spec).includes("3%"), "the sliver's share is missing from the table")
})

// An hour with nothing in it must read as empty — giving zero the first
// coloured step turns a quiet night into "a little activity" across the grid —
// but it still has to read as a *cell*, or the grid dissolves wherever the data
// is quiet and "no conversations" looks like "no square here".
test("an empty heatmap cell is visible, and never mistaken for a value", () => {
  for (const t of [INS_THEME_SCREEN, INS_THEME_EXPORT]) {
    const empty = insRampColor(0, 50, t)
    assert.strictEqual(empty, t.ramp[0])
    assert.notStrictEqual(empty, t.surface, "an empty cell is invisible on the card")
    assert.notStrictEqual(insRampColor(1, 50, t), empty, "one conversation reads as none")
    assert.strictEqual(insRampColor(50, 50, t), t.ramp[t.ramp.length - 1])
  }
  const t = INS_THEME_SCREEN
  // The scale legend keys every step, the empty one included: it is the colour
  // a reader meets most often and the one a partial key would leave unexplained.
  const { svg } = insRenderChart({ kind: "heatmap", grid: new Array(168).fill(0) }, t)
  for (const step of t.ramp) {
    assert.ok(svg.includes(step), "the scale legend omits " + step)
  }
  // Monotone: more is never lighter than less.
  let prev = -1
  for (const v of [1, 10, 20, 30, 40, 50]) {
    const idx = t.ramp.indexOf(insRampColor(v, 50, t))
    assert.ok(idx >= prev, "the ramp goes backwards at " + v)
    prev = idx
  }
})

test("axis ticks land on round numbers and always cover the data", () => {
  for (const max of [1, 7, 23, 99, 137, 1001, 48213]) {
    const ticks = insNiceTicks(max, 4)
    assert.strictEqual(ticks[0], 0, "ticks do not start at zero")
    assert.ok(ticks[ticks.length - 1] >= max, "the tallest bar is off the top of the axis")
    const step = ticks[1] - ticks[0]
    const mantissa = step / Math.pow(10, Math.floor(Math.log10(step)))
    assert.ok(
      [1, 2, 5, 10].some((n) => Math.abs(mantissa - n) < 1e-9),
      "step " + step + " is not 1/2/5 × 10ⁿ",
    )
  }
})

console.log("Insights data")

const PAYLOAD = {
  unit: "conversations",
  matchedInteractions: 0,
  matchesAreNarrowed: false,
  sessionCount: 100,
  interactionCount: 412,
  medianInteractions: 3,
  firstTs: "2026-06-01T08:00:00",
  lastTs: "2026-06-07T22:00:00",
  genaiSessions: 12,
  posFeedbackSessions: 8,
  negFeedbackSessions: 6,
  mixedFeedbackSessions: 1,
  zeroRecogSessions: 9,
  lowRecogSessions: 21,
  lowRecogThreshold: 60,
  byDay: [{ label: "2026-06-01", count: 60 }, { label: "2026-06-07", count: 40 }],
  // Zero-padded, as the backend emits them — `9:00 UTC` in a tooltip and a
  // ragged `9` on the axis are both wrong.
  byHour: Array.from({ length: 24 }, (_, h) => ({
    label: String(h).padStart(2, "0"),
    count: h,
  })),
  hourWeekday: Array.from({ length: 168 }, (_, i) => i % 5),
  lengthBuckets: [{ label: "21+", count: 4 }, { label: "1", count: 60 }, { label: "3–5", count: 36 }],
  recognitionBands: [
    { label: "70–100%", count: 70 },
    { label: "Zero", count: 9 },
    { label: "Under 40%", count: 21 },
  ],
  cultures: [{ label: "nl-NL", count: 100 }],
  recognitionTypes: [{ label: "Article", count: 300 }],
  dialogStatus: [{ label: "EndedOrInProgress", count: 40 }],
  contexts: [{ name: "channel", withKey: 90, distinctValues: 2, values: [{ label: "web", count: 90 }] }],
  metadata: [{ name: "nochat", withKey: 10, distinctValues: 1, values: [{ label: "true", count: 10 }] }],
  entities: [{ label: "WIJN", count: 12 }],
  articles: [{ label: "qa-101", count: 30 }],
  dialogNodes: [],
  dialogs: [{ label: "6391", count: 25 }],
  firstMessages: [{ label: "openingstijden", count: 15 }],
}

// The same result set, read as the matching turns rather than as the
// conversations holding them. `matchedInteractions` is the denominator of
// everything in this reading; `sessionCount` is still the conversations they
// came from.
const CONVS = PAYLOAD
const TURNS = {
  ...PAYLOAD,
  unit: "interactions",
  matchedInteractions: 260,
  matchesAreNarrowed: true,
  firstMessages: [],
}


// The two feedback flags are independent — one conversation can carry a thumbs
// up and a thumbs down — so `none` is not `total − up − down`. Getting this
// wrong over-counts the overlap and silently under-states how many
// conversations nobody rated at all.
test("the feedback split is disjoint and accounts for every conversation", () => {
  const spec = insFeedbackSpec(
    {
      sessionCount: 100,
      posFeedbackSessions: 30,
      negFeedbackSessions: 20,
      mixedFeedbackSessions: 5,
    },
    INS_THEME_SCREEN,
  )
  const at = (l) => spec.data.find((d) => d.label === l).value
  assert.strictEqual(at("Thumbs up"), 25)
  assert.strictEqual(at("Thumbs down"), 15)
  assert.strictEqual(at("Both"), 5)
  assert.strictEqual(at("No feedback given"), 55)
  assert.strictEqual(
    spec.data.reduce((a, d) => a + d.value, 0),
    100,
    "the segments do not add up to the result set",
  )
})

// A quiet week has to look quiet. The query only returns days that have
// conversations, so without this the series closes up and a gap reads as
// uninterrupted activity at a steady rate.
test("days with no conversations are drawn as gaps, not skipped", () => {
  const filled = insFillDays([
    { label: "2026-06-01", count: 12 },
    { label: "2026-06-05", count: 4 },
  ])
  assert.deepStrictEqual(
    filled.map((d) => d.label),
    ["2026-06-01", "2026-06-02", "2026-06-03", "2026-06-04", "2026-06-05"],
  )
  assert.deepStrictEqual(filled.map((d) => d.value), [12, 0, 0, 0, 4])
  assert.deepStrictEqual(insFillDays([]), [])
  // Purely string arithmetic on UTC dates — a month boundary is not special,
  // and no Date-with-local-offset can creep in.
  const across = insFillDays([
    { label: "2026-02-27", count: 1 },
    { label: "2026-03-02", count: 1 },
  ])
  assert.deepStrictEqual(
    across.map((d) => d.label),
    ["2026-02-27", "2026-02-28", "2026-03-01", "2026-03-02"],
  )
})

// A value's share is of the conversations that set the key. Against the whole
// result set every value of a rarely-set key reads as negligible — a statement
// about the key, not about the value.
test("a tag value's share is of the conversations that set its key", () => {
  const group = {
    name: "channel",
    withKey: 80,
    distinctValues: 14,
    values: [
      { label: "web", count: 50 },
      { label: "app", count: 20 },
    ],
  }
  const words = insWords(CONVS)
  const spec = insTagSpec(group, words)
  assert.strictEqual(spec.total, 80, "the share is taken over the wrong denominator")
  assert.strictEqual(insPct(50, spec.total), "63%")
  const note = insTagNote(group, { ...words, total: 200 })
  assert.ok(note.includes("120"), "the note does not say how many never set the key")
  assert.ok(note.includes("top 2 of 14"), "the note does not say what was folded away: " + note)
  // No synthetic tail bar — see insTagSpec. The chart shows the values it has.
  assert.deepStrictEqual(spec.data.map((d) => d.label), ["web", "app"])
})

// A conversation whose context changed mid-way carries both values, so the bars
// can legitimately add up to more than the conversations that set the key. The
// chart used to derive an "Other" bar as `withKey − shown`, which goes
// *negative* exactly then — found against the real Interaction Log, where a
// context key really does take two values within one conversation.
test("a key one conversation set twice does not produce a negative remainder", () => {
  const group = {
    name: "Channel",
    withKey: 100,
    distinctValues: 2,
    values: [
      { label: "web", count: 90 },
      { label: "app", count: 40 },
    ],
  }
  const words = insWords(CONVS)
  const spec = insTagSpec(group, words)
  assert.ok(spec.data.every((d) => d.value > 0), "a bar came out negative or empty")
  assert.strictEqual(spec.data.length, 2, "a phantom remainder bar was added")
  const note = insTagNote(group, { ...words, total: 100 })
  assert.ok(
    note.includes("more than one value"),
    "the chart adds up to more than its denominator and does not say so: " + note,
  )
  // …and it is not said where it is not true.
  assert.ok(
    !insTagNote(
      { name: "x", withKey: 100, distinctValues: 1, values: [{ label: "a", count: 60 }] },
      { ...insWords(CONVS), total: 100 },
    ).includes("more than one value"),
  )
})

// Recognition bands are an ordered scale: swapping two of them changes what the
// chart says. Sorting them by count — the obvious thing for every other chart
// here — would put "Zero" wherever it happened to land.
test("ordered scales keep their order however few land in each band", () => {
  const cards = insBuildCards(CONVS, INS_THEME_SCREEN)
  const byId = new Map(cards.map((c) => [c.id, c]))
  // The fixture has nothing in the 40–69% band; the bands either side keep
  // their places rather than closing up around it.
  assert.deepStrictEqual(
    byId.get("recognition").spec.data.map((d) => d.label),
    ["Zero", "Under 40%", "70–100%"],
  )
  assert.deepStrictEqual(
    byId.get("length").spec.data.map((d) => d.label),
    ["1", "3–5", "21+"],
  )
  // The band colours are the reserved status scale, never a series hue.
  const colors = byId.get("recognition").spec.data.map((d) => d.color)
  assert.deepStrictEqual(colors, [
    INS_THEME_SCREEN.critical,
    INS_THEME_SCREEN.serious,
    INS_THEME_SCREEN.good,
  ])
  assert.ok(!colors.includes(INS_THEME_SCREEN.series), "a status band wears a series colour")
})

// A card with nothing in it is not an empty chart, it is a chart that should
// not be there — an axis with no marks reads as a rendering fault.
test("a card with no data is not rendered at all", () => {
  const cards = insBuildCards(CONVS, INS_THEME_SCREEN)
  const ids = cards.map((c) => c.id)
  assert.ok(!ids.includes("dialogNodes"), "an empty series produced a card")
  assert.ok(!ids.includes("cultures"), "one culture is not a distribution")
  assert.ok(ids.includes("articles") && ids.includes("heat"))
  // Sections come out in reading order and nothing lands outside one.
  const sections = [...new Set(cards.map((c) => c.section))]
  assert.deepStrictEqual(sections, ["Volume", "Quality", "Context", "Metadata", "Content"])
})

// The chooser and the card builder name the same five sections, and they have
// to keep naming them the same way: the dashboard decides what to draw by
// looking a card's section up in the chosen set. Rename one on either side and
// every card in it silently disappears — no error, no empty state, just a
// missing section on a screen that has no other way of saying so.
test("the chooser and the cards agree on what a section is", () => {
  const offered = INS_SECTIONS.map((sec) => sec.key)
  assert.deepStrictEqual(offered, ["volume", "quality", "context", "metadata", "content"])
  const drawn = [...new Set(insBuildCards(CONVS, INS_THEME_SCREEN).map((c) => c.section))]
  for (const name of drawn) {
    assert.ok(
      offered.includes(name.toLowerCase()),
      "the card builder draws a section the chooser cannot offer: " + name,
    )
  }
  // Context and Metadata come back from their own call, so they are never sent
  // as part of the dashboard read.
  assert.deepStrictEqual(INS_READ_SECTIONS, ["volume", "quality", "content"])
  for (const key of INS_READ_SECTIONS) {
    const sec = INS_SECTIONS.find((x) => x.key === key)
    assert.ok(sec.fields && sec.fields.length, key + " has no fields to merge when added")
  }
  // Every field a section claims is a field the payload actually carries, or
  // adding that section later would merge nothing into the dashboard.
  for (const sec of INS_SECTIONS) {
    for (const f of sec.fields || []) {
      assert.ok(f in CONVS, sec.key + " claims a field the payload has no such key for: " + f)
    }
  }
})

// A section left out of the chooser must contribute no card at all — including
// the one card that is not built from that section's own query.
//
// Feedback comes off the headline counters, which are read whatever was chosen,
// so an unchosen Quality section would otherwise leave exactly one Quality
// chart stranded on the dashboard under a heading for a section that was never
// read. Filtering by emptiness instead of by the choice is what lets that
// through, which is why the renderer filters by the choice.
test("a section that was not chosen contributes no card, not even a derived one", () => {
  const chosen = { volume: true, quality: false, context: false, metadata: false, content: false }
  // The payload as the backend returns it with only Volume asked for: every
  // Quality and Content array empty, every headline counter still populated.
  const partial = { ...CONVS }
  for (const sec of INS_SECTIONS) {
    if (chosen[sec.key]) continue
    for (const f of sec.fields || []) partial[f] = []
  }
  const all = insBuildCards(partial, INS_THEME_SCREEN)
  assert.ok(
    all.some((c) => c.section === "Quality"),
    "the fixture must still produce a derived Quality card, or this proves nothing",
  )
  const kept = all.filter((c) => chosen[c.section.toLowerCase()])
  assert.deepStrictEqual([...new Set(kept.map((c) => c.section))], ["Volume"])
  assert.ok(kept.length, "Volume was chosen and drew nothing")
})

// Every value on this dashboard is reachable without looking at a colour. The
// copied table is that twin, and it is also the fallback when an image cannot
// reach the clipboard.
test("every chart has a table twin carrying all of its values", () => {
  for (const d of [CONVS, TURNS]) {
    for (const card of insBuildCards(d, INS_THEME_SCREEN)) {
      const tsv = insChartTsv(card.spec)
      const rows = tsv.split("\n")
      assert.ok(rows.length > 1, card.id + ": empty table")
      if (card.spec.kind === "heatmap") {
        assert.strictEqual(rows.length, 8, "a heatmap table is a header and seven days")
        assert.ok(rows[1].startsWith("Mon\t"), "the week does not start on Monday")
        assert.strictEqual(rows[1].split("\t").length, 25, "a day row is not 24 hours")
        continue
      }
      // A pasted spreadsheet column has to say what it counted; a bare number
      // in a cell has no other way of telling the reader which reading it is.
      // Located by name rather than by position: a chart may carry an extra
      // descriptive column (the day series carries the weekday), and which
      // column the count lands in is not what this is about.
      const head = rows[0].split("\t").map((h) => h.toLowerCase())
      assert.ok(
        head.includes(card.spec.unit),
        card.id + ": the table header does not name the unit — " + rows[0],
      )
      // The weekend is a band and a tick colour on the picture, and a
      // spreadsheet keeps neither — so the day series has to say it in words.
      if (card.spec.weekdayColumn) {
        assert.ok(head.includes("weekday"), card.id + ": no weekday column")
        const at = head.indexOf("weekday")
        for (const r of rows.slice(1)) {
          assert.ok(
            /^(Mon|Tue|Wed|Thu|Fri|Sat|Sun)$/.test(r.split("\t")[at]),
            card.id + ": a day row carries no weekday — " + r,
          )
        }
      }
      for (const b of card.spec.data) {
        assert.ok(
          rows.some((r) => r.startsWith(b.label + "\t") && r.includes("\t" + b.value + "\t")),
          card.id + ": " + b.label + " (" + b.value + ") is not in the table",
        )
      }
    }
  }
})

// A percentage means nothing without the slice it was taken over, and that
// sentence has to survive into a pasted email.
test("the slice is always stated, including when nothing was filtered", () => {
  assert.deepStrictEqual(insSearchSummary({}), [
    { label: "Scope", value: "Everything in the database" },
  ])
  assert.deepStrictEqual(insSearchSummary(null), [
    { label: "Scope", value: "Everything in the database" },
  ])
  const described = insSearchSummary({
    query: "parkeren",
    queryScope: "user",
    queryEntities: true,
    filter: "neg_feedback",
    dateFrom: "2026-06-01",
    contextFilters: [{ name: "park", value: "__not_set__" }],
    metadataFilters: [{ name: "nochat", value: "true" }],
  })
  const text = described.map((c) => c.label + "=" + c.value).join(" | ")
  assert.ok(text.includes("parkeren · user + entities"), text)
  assert.ok(text.includes("Thumbs down only"), text)
  assert.ok(text.includes("2026-06-01 → latest"), text)
  assert.ok(text.includes("park not set"), text)
  assert.ok(text.includes("nochat = true"), text)
})

// A weekend is not a property of the numbers, so nothing in the data says
// where one is — but "why is Tuesday always low?" is unanswerable without it.
test("the weekend is visible in the picture and named in the table", () => {
  const volume = insBuildCards(CONVS, INS_THEME_SCREEN).find((c) => c.id === "volume")
  assert.ok(volume, "no volume card")
  const at = new Map(volume.spec.data.map((d) => [d.label, d]))
  // 2026-06-01 is a Monday, 06 a Saturday, 07 a Sunday.
  assert.strictEqual(at.get("2026-06-06").band, "weekend", "Saturday is not banded")
  assert.strictEqual(at.get("2026-06-07").band, "weekend", "Sunday is not banded")
  assert.strictEqual(at.get("2026-06-01").band, undefined, "Monday is banded")

  // A datum names a palette token and never a colour. One spec object is drawn
  // in both palettes — `insCardHtml` in the screen one, `insChartCanvas` in the
  // export one — so a literal would paint the dark wash onto a white surface.
  for (const d of volume.spec.data) {
    assert.ok(!/^#/.test(d.band || ""), d.label + ": a literal band colour")
    assert.ok(!/^#/.test(d.tickColor || ""), d.label + ": a literal tick colour")
  }
  const dark = insRenderChart(volume.spec, INS_THEME_SCREEN).svg
  const light = insRenderChart(volume.spec, INS_THEME_EXPORT).svg
  assert.notStrictEqual(INS_THEME_SCREEN.weekend, INS_THEME_EXPORT.weekend)
  assert.ok(dark.includes(INS_THEME_SCREEN.weekend), "no weekend band on screen")
  assert.ok(light.includes(INS_THEME_EXPORT.weekend), "no weekend band in the export")

  // The band and the tick colour both vanish into a spreadsheet and into a
  // screen reader, so the day says which one it is in words too.
  assert.ok(at.get("2026-06-07").tip.endsWith("Sun"), at.get("2026-06-07").tip)
  assert.ok(insChartTsv(volume.spec).includes("2026-06-07\tSun\t"))
})

// The bounds the picker sends are always midnight and one second to midnight,
// which is the one part of that chip carrying no information.
test("the dates chip names days, and keeps the exact bounds for the hover", () => {
  const [chip] = insSearchSummary({
    dateFrom: "2026-06-01T00:00:00",
    dateTo: "2026-06-30T23:59:59",
  })
  assert.strictEqual(chip.value, "2026-06-01 → 2026-06-30")
  assert.strictEqual(chip.title, "2026-06-01T00:00:00 → 2026-06-30T23:59:59")
  // A bound with nothing to trim carries no hover: a tooltip repeating the text
  // underneath it is a tooltip that says nothing.
  const [plain] = insSearchSummary({ dateFrom: "2026-06-01" })
  assert.strictEqual(plain.value, "2026-06-01 → latest")
  assert.strictEqual(plain.title, undefined)
})

// The timezone is stated once, not on every card.
//
// It used to appear in ten places — both time axes, three tooltips, two card
// notes, the header, the footer and the copied table header — all saying the
// same unvarying thing about the whole screen. What is left is the header's own
// badge, which no chart builder can produce, and the hour axis, which is a
// label rather than a disclaimer: an exported hour chart carries nothing else
// that could say what `09` means.
test("the timezone is a header badge, not a caption on every chart", () => {
  const offenders = []
  for (const d of [CONVS, TURNS]) {
    for (const card of insBuildCards(d, INS_THEME_SCREEN)) {
      if (/UTC/.test(card.note || "")) offenders.push(card.id + " note")
      if (/UTC/.test(card.title)) offenders.push(card.id + " title")
      const svg = insRenderChart(card.spec, INS_THEME_SCREEN).svg
      const texts = [...svg.matchAll(/<(?:text|title)[^>]*>([^<]*)</g)].map((m) => m[1])
      for (const t of texts) {
        // The hour axis is the one place it belongs, and it is an axis label.
        if (/UTC/.test(t) && t !== "Hour (UTC)") offenders.push(card.id + ": " + t)
      }
      // Same exemption in the copied table: the hour column is named after
      // its axis, and a spreadsheet column called "Hour" alone is ambiguous.
      const header = insChartTsv(card.spec).split("\n")[0]
      if (/UTC/.test(header) && !header.startsWith("Hour (UTC)\t")) {
        offenders.push(card.id + " table header: " + header)
      }
    }
  }
  assert.deepStrictEqual(offenders, [])
  // …and the one place it does belong is still there, so this cannot pass by
  // the disclaimer having been deleted outright.
  const hour = insBuildCards(CONVS, INS_THEME_SCREEN).find((c) => c.id === "hour")
  assert.strictEqual(hour.spec.axisLabel, "Hour (UTC)")
})

// Every tooltip on this dashboard ends in a counted noun.
test("a tooltip counting one thing does not say “1 conversations”", () => {
  const tips = (spec) =>
    [...insRenderChart(spec, INS_THEME_SCREEN).svg.matchAll(/<title>([^<]*)</g)].map((m) => m[1])
  for (const spec of [
    { kind: "columns", data: [{ label: "a", value: 1 }, { label: "b", value: 4 }] },
    { kind: "bars", data: [{ label: "a", value: 1 }, { label: "b", value: 4 }] },
    { kind: "area", data: [{ label: "d1", value: 1 }, { label: "d2", value: 4 }] },
    { kind: "heatmap", grid: [1, 4].concat(new Array(166).fill(0)) },
    { kind: "stack", total: 5, data: [{ label: "a", value: 1 }, { label: "b", value: 4 }] },
  ]) {
    const all = tips(spec).join(" | ")
    assert.ok(!/\b1 conversations\b/.test(all), spec.kind + ": " + all)
    assert.ok(/\b1 conversation\b/.test(all), spec.kind + " never counted one: " + all)
    assert.ok(/\b4 conversations\b/.test(all), spec.kind + " never counted four: " + all)
  }
  // The tooltip may say more than the axis has room for.
  const hourCard = insBuildCards(CONVS, INS_THEME_SCREEN).find((c) => c.id === "hour")
  const hourTips = tips(hourCard.spec)
  assert.ok(hourTips.some((t) => t.startsWith("09:00:")), hourTips.slice(0, 3).join(" | "))
  assert.ok(
    hourCard.spec.data.every((d) => /^\d\d$/.test(d.label)),
    "the axis label grew along with the tooltip",
  )
})

// ── The two readings ────────────────────────────────────────────────
//
// The same result set can be read as the conversations that matched or as the
// interactions that did. Both are right; the failure mode is a chart drawn in
// one and labelled in the other, which is invisible on screen and permanent
// once pasted into an email.

test("every card, tile and tooltip in one reading names that reading", () => {
  for (const [d, noun, wrong] of [
    [CONVS, "conversations", "interactions"],
    [TURNS, "interactions", "conversations"],
  ]) {
    for (const card of insBuildCards(d, INS_THEME_SCREEN)) {
      // The length chart is the deliberate exception: it bins conversations in
      // both readings, and says so in its own note.
      if (card.id === "length") {
        assert.strictEqual(card.spec.unit, "conversations", "length changed its unit")
        continue
      }
      // "How the answer was found" counts interactions in both readings — in
      // one it is every interaction of the matched conversations, in the other
      // only the matching ones.
      const expected = card.id === "recogType" ? "interactions" : noun
      assert.strictEqual(card.spec.unit, expected, card.id + " counts the wrong thing")
      const tips = [
        ...insRenderChart(card.spec, INS_THEME_SCREEN).svg.matchAll(/<title>([^<]*)</g),
      ].map((m) => m[1])
      if (card.spec.kind !== "heatmap" && expected !== "interactions") {
        assert.ok(
          tips.every((t) => !t.includes(wrong)),
          card.id + " tooltip says “" + wrong + "”: " + tips[0],
        )
      }
    }
  }
})

test("the hero is the number the unit names, and says which", () => {
  const convHero = insTiles(CONVS).find((t) => t.hero)
  assert.strictEqual(convHero.value, "100")
  assert.strictEqual(convHero.label, "Conversations")

  const turnHero = insTiles(TURNS).find((t) => t.hero)
  assert.strictEqual(turnHero.value, "260", "the hero still counts conversations")
  assert.strictEqual(turnHero.label, "Matching interactions")
  assert.strictEqual(
    insTiles({ ...TURNS, matchesAreNarrowed: false }).find((t) => t.hero).label,
    "Interactions",
    "nothing narrowed the search, so nothing may be called “matching”",
  )
  // Exactly one hero either way, and the conversations behind the turns stay
  // reachable so the two readings can be related to each other.
  assert.strictEqual(insTiles(TURNS).filter((t) => t.hero).length, 1)
  const conv = insTiles(TURNS).find((t) => t.label === "Conversations")
  assert.strictEqual(conv.value, "100")

  // Percentages are taken over the unit being counted, never over the other.
  const zero = insTiles(TURNS).find((t) => t.label === "Zero recognition")
  assert.strictEqual(zero.value, insPct(TURNS.zeroRecogSessions, 260))
  assert.ok(zero.sub.endsWith("interactions"), zero.sub)
})

// A pasted number with two possible meanings is worse than no number.
test("both readings state the unit in words, in the header and the report", () => {
  assert.strictEqual(insUnitLine(CONVS), "100 conversations · 412 interactions")
  const turns = insUnitLine(TURNS)
  assert.ok(turns.startsWith("260 matching interactions in 100 conversations"), turns)
  assert.ok(turns.includes("412"), "the report does not say what 260 is out of: " + turns)
  const wide = insUnitLine({ ...TURNS, matchesAreNarrowed: false })
  assert.ok(wide.startsWith("260 interactions in 100 conversations"), wide)
  assert.ok(!wide.includes("matching"), "nothing narrowed it: " + wide)

  // The header form drops the leading count, which the hero already shows —
  // and drops nothing else, so the two can never describe different readings.
  for (const d of [CONVS, TURNS]) {
    const full = insUnitLine(d)
    const short = insUnitLine(d, true)
    assert.ok(!short.startsWith(insNumLike(d)), "the header restates the hero: " + short)
    assert.ok(full.endsWith(short), "the short form says something the full one does not: " + short)
  }
})

/// The hero figure as the line would render it — the count the short form must
/// not begin by repeating.
function insNumLike(d) {
  return (d.unit === "interactions" ? d.matchedInteractions : d.sessionCount).toLocaleString()
}

// Both are properties of a conversation, not of a turn that happened to match.
// Rendering them in the interactions reading would invite the exact misreading
// the reading exists to prevent.
test("feedback and opening questions are left out of the interactions reading", () => {
  const convIds = insBuildCards(CONVS, INS_THEME_SCREEN).map((c) => c.id)
  const turnIds = insBuildCards(TURNS, INS_THEME_SCREEN).map((c) => c.id)
  assert.ok(convIds.includes("feedback") && convIds.includes("questions"))
  assert.ok(!turnIds.includes("feedback"), "a conversation-level rating in a per-turn reading")
  assert.ok(!turnIds.includes("questions"))
  assert.ok(!insTiles(TURNS).some((t) => t.label === "Thumbs down"))
  assert.ok(insTiles(CONVS).some((t) => t.label === "Thumbs down"))
  // Everything else survives the switch — the reading is not a smaller feature.
  for (const id of convIds) {
    if (id === "feedback" || id === "questions") continue
    assert.ok(turnIds.includes(id), id + " disappeared in the interactions reading")
  }
})

test("a tag card says that context is recorded per conversation, not per turn", () => {
  const group = { name: "channel", withKey: 80, distinctValues: 3, values: [{ label: "web", count: 80 }] }
  const convNote = insTagNote(group, insWords(CONVS))
  const turnNote = insTagNote(group, insWords(TURNS))
  assert.ok(!convNote.includes("recorded per conversation"), convNote)
  assert.ok(
    turnNote.includes("recorded per conversation"),
    "an interaction counted under its conversation's context, with no caveat: " + turnNote,
  )
  assert.ok(turnNote.includes("80 interactions that set this key"), turnNote)
  assert.strictEqual(insTagSpec(group, insWords(TURNS)).unit, "interactions")
})

test("the tiles lead with one hero figure and state their own denominator", () => {
  const tiles = insTiles(CONVS)
  assert.strictEqual(tiles.filter((t) => t.hero).length, 1, "not exactly one hero figure")
  assert.strictEqual(tiles[0].value, "100")
  const zero = tiles.find((t) => t.label === "Zero recognition")
  assert.strictEqual(zero.value, "9%")
  assert.strictEqual(zero.sub, "9 conversations")
  assert.ok(tiles.some((t) => t.label === "Under 60%"), "the threshold tile does not name its threshold")
  // An empty result must not produce percentages over nothing.
  assert.strictEqual(insTiles({ sessionCount: 0, interactionCount: 0 }).length, 3)
})

// Every card the dashboard renders is also a card the clipboard can carry.
test("every rendered card produces a well-formed chart in both themes", () => {
  for (const card of insBuildCards(CONVS, INS_THEME_SCREEN)) {
    for (const theme of [INS_THEME_SCREEN, INS_THEME_EXPORT]) {
      const out = insRenderChart(card.spec, theme)
      assertWellFormed(out.svg)
      assert.ok(out.width > 0 && out.height > 0, card.id + ": zero-sized chart")
      assert.ok(
        out.svg.startsWith('<svg xmlns="http://www.w3.org/2000/svg"'),
        card.id + ": no XML namespace — it will not load in an <img>",
      )
    }
    assertInsideCanvas(card.spec, card.id)
    {
    }
  }
})

// The one place user data reaches the SVG. An entity or context value is
// arbitrary text from a customer's own configuration.
test("a label full of markup cannot break the picture", () => {
  const nasty = 'A & B <script>"x"</script> >'
  const spec = { kind: "bars", data: [{ label: nasty, value: 3 }], total: 3 }
  const { svg } = insRenderChart(spec, INS_THEME_EXPORT)
  assertWellFormed(svg)
  assert.ok(!svg.includes("<script"), "raw markup reached the SVG")
  assert.ok(svg.includes("&amp;"), "an ampersand was not escaped")
})

console.log(failures ? "\n" + failures + " failing" : "\nall insights tests passed")
process.exit(failures ? 1 : 0)
