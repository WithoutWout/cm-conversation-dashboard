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
  "insFeedbackNote",
  "insAnswers",
  "insTagSpec",
  "insTagNote",
  "insUnitLine",
  "insWords",
  "insBuildCards",
  "insTiles",
  "insSearchSummary",
  "INS_SECTIONS",
  "INS_READ_SECTIONS",
  "insTimeSeries",
  "insSetDisplayZone",
  "insZone",
  "insZoneLabel",
  "insZoneDayBounds",
  "insZoneStampToUtc",
  "insZoneDayOf",
  "insCacheDecide",
  "insLinkBuckets",
  "insParseContentLabel",
  "INS_CAP_MAX_LINES",
  "INS_CAP_MAX_ROWS",
  "insChartMarkdown",
  "insChartCsv",
  "insCaptionLines",
  "insCaptionLayout",
  "insCacheKey",
  "insSegmentTable",
  "insTableTsv",
  "insTsvToCsv",
  "insTsvToMarkdown",
  "insSegPct",
  "insSegShare",
  "insSegmentTableHtml",
  "insReportTableHtml",
  "insCardTsv",
  "INS_SEGMENT_COLUMNS",
  "INS_SEGMENT_MAX_KEYS",
  "insSegRowId",
  "insSegLabelOf",
  "insSegNaturalLayout",
  "insSegNormalizeLayout",
  "insSegNormalizePresets",
  "insSegResolve",
  "insSegDropIndex",
  "insExportTheme",
  "insNormalizePalette",
  "insNormalizeChartTypes",
  "insNormalizeLabelOpts",
  "insApplyChartKind",
  "insVolumeSpec",
  "insFmtDay",
  "insFmtHour",
  "insWithLang",
  "insT",
  "INS_I18N",
  "INS_CHART_KINDS",
  "INS_LABEL_DEFAULTS",
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
  insFeedbackNote,
  insAnswers,
  insTagSpec,
  insTagNote,
  insUnitLine,
  insWords,
  insBuildCards,
  insTiles,
  insSearchSummary,
  INS_SECTIONS,
  INS_READ_SECTIONS,
  insTimeSeries,
  insSetDisplayZone,
  insZone,
  insZoneLabel,
  insZoneDayBounds,
  insZoneStampToUtc,
  insZoneDayOf,
  insCacheDecide,
  insLinkBuckets,
  insParseContentLabel,
  INS_CAP_MAX_LINES,
  INS_CAP_MAX_ROWS,
  insChartMarkdown,
  insChartCsv,
  insCaptionLines,
  insCaptionLayout,
  insCacheKey,
  insSegmentTable,
  insTableTsv,
  insTsvToCsv,
  insTsvToMarkdown,
  insSegPct,
  insSegShare,
  insSegmentTableHtml,
  insReportTableHtml,
  insCardTsv,
  INS_SEGMENT_COLUMNS,
  INS_SEGMENT_MAX_KEYS,
  insSegRowId,
  insSegLabelOf,
  insSegNaturalLayout,
  insSegNormalizeLayout,
  insSegNormalizePresets,
  insSegResolve,
  insSegDropIndex,
  insExportTheme,
  insNormalizePalette,
  insNormalizeChartTypes,
  insNormalizeLabelOpts,
  insApplyChartKind,
  insVolumeSpec,
  insFmtDay,
  insFmtHour,
  insWithLang,
  insT,
  INS_I18N,
  INS_CHART_KINDS,
  INS_LABEL_DEFAULTS,
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
    { kind: "line", data: bars(30, 40) },
    { kind: "donut", data: bars(5), total: 40 },
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
    // A rotated tick carries its origin in `transform="translate(x,y) rotate()"`
    // and has no `x` at all. Reading `.exec(...)[1]` off it threw — which never
    // fired only because `rotateLabels` needs 12–62 days and the fixture spans
    // seven. Measured at its translate origin instead: rotated by -40° about
    // that point and anchored at the end, it extends up and to the left of it,
    // so the origin is its rightmost extent.
    const rot = /transform="translate\((-?[\d.]+),(-?[\d.]+)\)/.exec(attrs)
    const xm = /\bx="(-?[\d.]+)"/.exec(attrs)
    if (!xm && !rot) throw new Error(label + ": a <text> with no position at all")
    const x = Number(xm ? xm[1] : rot[1])
    const anchor = (/text-anchor="(\w+)"/.exec(attrs) || [])[1] || "start"
    const size = Number((/font-size="([\d.]+)"/.exec(attrs) || [])[1] || 11)
    // Rotated by 40°, a label's horizontal extent is its length × cos 40°.
    const tw = insTextWidth(m[2], size) * (rot ? Math.cos((40 * Math.PI) / 180) : 1)
    const left = anchor === "end" ? x - tw : anchor === "middle" ? x - tw / 2 : x
    assert.ok(left >= -0.5, label + ': "' + m[2] + '" starts left of the canvas')
    assert.ok(left + tw <= width + 0.5, label + ': "' + m[2] + '" runs past the right edge')
  }
}

test("no mark and no label is drawn outside the chart it belongs to", () => {
  const specs = [
    // A rotated axis, which needs 12-62 days to appear at all — so no fixture
    // on this dashboard reached it, and `assertInsideCanvas` threw on the
    // first `<text>` that carried a transform instead of an `x`.
    {
      kind: "columns",
      rotateLabels: true,
      data: Array.from({ length: 20 }, (_, i) => ({
        label: "2026-06-" + String(i + 1).padStart(2, "0"),
        value: 40 - i,
      })),
    },
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
  // Per answer, read with Quality: 300 answers, 40 of them rated, 30 positively.
  answerCount: 300,
  ratedAnswers: 40,
  positiveAnswers: 30,
  // The chart's GenerativeAI bar and its total, read with Quality.
  typedQuestions: 200,
  genaiAnswers: 13,
  zeroRecogSessions: 9,
  lowRecogSessions: 21,
  lowRecogThreshold: 60,
  // One row per UTC day per UTC hour, exactly as `InsightDayHours` crosses the
  // bridge. The renderer folds this into the day series, the hour histogram and
  // the heatmap, in the display timezone — nothing arrives pre-folded.
  //
  // 2026-06-01 is a Monday and 2026-06-07 a Sunday, and every hour of both is
  // occupied, so a shift in either direction has somewhere to land.
  dayHours: [
    { day: "2026-06-01", hours: Array.from({ length: 24 }, (_, h) => h) },
    { day: "2026-06-07", hours: Array.from({ length: 24 }, (_, h) => 23 - h) },
  ],
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


// Feedback is reported as the positive share of the *rated* answers, and the
// share of answers rated at all. The bar is the rated answers, so its green part
// is the headline percentage; unrated answers are not a slice of it, or the
// positive share would shrink every time fewer people bothered to rate.
test("feedback is the positive share of the rated answers", () => {
  const spec = insFeedbackSpec({ answerCount: 300, ratedAnswers: 40, positiveAnswers: 30 })
  const at = (l) => spec.data.find((d) => d.label === l).value
  assert.strictEqual(spec.total, 40)
  assert.strictEqual(spec.unit, "answers")
  assert.strictEqual(at("Thumbs up"), 30)
  assert.strictEqual(at("Thumbs down"), 10)
  assert.strictEqual(spec.data.length, 2, "unrated answers are not part of the bar")
  const note = insFeedbackNote({ answerCount: 300, ratedAnswers: 40, positiveAnswers: 30 })
  assert.ok(note.startsWith("75% of the 40 rated answers were positive."), note)
  assert.ok(note.includes("13% of all 300 answers were rated."), note)
  const tiles = insTiles(PAYLOAD)
  const tile = (l) => tiles.find((t) => t.label === l)
  assert.strictEqual(tile("Positive feedback").value, "75%")
  assert.strictEqual(tile("Positive feedback").sub, "of 40 rated")
  assert.strictEqual(tile("Answers rated").value, "13%")
  assert.strictEqual(tile("Answers rated").sub, "40 of 300 answers")
  // Leads the rates: before GenAI and recognition.
  assert.ok(tiles.indexOf(tile("Positive feedback")) < tiles.findIndex((t) => t.label === "GenAI answers"))

  // Nothing rated is a dash, never "0% positive" — a claim about ratings nobody left.
  const none = insTiles({ ...PAYLOAD, ratedAnswers: 0, positiveAnswers: 0 })
  assert.strictEqual(none.find((t) => t.label === "Positive feedback").value, "—")
  // Quality not read: no feedback tiles at all, rather than a false 0%.
  const noQuality = insTiles({ ...PAYLOAD, answerCount: 0, ratedAnswers: 0, positiveAnswers: 0 })
  assert.ok(!noQuality.some((t) => t.label === "Positive feedback" || t.label === "Answers rated"))
  // In German too, with the slots filled.
  const de = insWithLang("de", () => insFeedbackNote({ answerCount: 300, ratedAnswers: 40, positiveAnswers: 30 }))
  assert.ok(de.startsWith("75 % der 40 bewerteten Antworten waren positiv."), de)
})

// The GenAI tile is the GenerativeAI share of *How the answer was found* —
// the same rows and the same denominator — so the headline and the chart can
// never show two different GenAI percentages again.
test("the GenAI tile is the chart's own GenAI share", () => {
  const d = { ...PAYLOAD, recognitionTypes: [{ label: "Entity Recognition", count: 187 }, { label: "GenerativeAI", count: 13 }] }
  const tile = insTiles(d).find((t) => t.label === "GenAI answers")
  assert.strictEqual(tile.value, "6.5%")
  assert.strictEqual(tile.value, insPct(13, 187 + 13), "the tile and the chart disagree")
  assert.strictEqual(tile.sub, "13 answers · in 12 conversations")
  assert.strictEqual(insTiles(TURNS).find((t) => t.label === "GenAI answers").sub, "13 of 200 typed questions")
  // …and the chart's GenAI bar says the same, in both readings: its share is
  // of the typed questions, never of every matching answer.
  for (const base of [d, { ...TURNS, recognitionTypes: d.recognitionTypes }]) {
    const card = insBuildCards(base, INS_THEME_SCREEN).find((c) => c.id === "recogType")
    assert.strictEqual(card.spec.total, 200)
    const svg = insRenderChart(card.spec, INS_THEME_SCREEN).svg
    assert.ok(svg.includes(insPct(13, 200)), "the GenAI bar is not the tile's share")
  }
  // Quality not read: no GenAI tile rather than a 0%.
  assert.ok(!insTiles({ ...PAYLOAD, typedQuestions: undefined, genaiAnswers: undefined }).some((t) => t.label === "GenAI answers"))
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
  // The band colours are the reserved status scale, never a series hue — and
  // tokens, resolved per palette, so an export palette recolours them.
  const colors = byId.get("recognition").spec.data.map((d) => d.color)
  assert.deepStrictEqual(colors, ["critical", "serious", "good"])
  for (const theme of [INS_THEME_SCREEN, INS_THEME_EXPORT]) {
    const svg = insRenderChart(byId.get("recognition").spec, theme).svg
    assert.ok(svg.includes(theme.critical) && svg.includes(theme.good), theme.name)
  }
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
  assert.deepStrictEqual(offered, [
    "volume",
    "quality",
    "context",
    "metadata",
    "content",
    "segments",
  ])
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

// ── The Segments table ───────────────────────────────────────────────
//
// Five measures, four of which are ratios whose denominator is the whole
// question. Nothing here fails loudly when one is taken over the wrong set: a
// recognition rate computed over every interaction instead of the scored ones
// is simply a smaller plausible percentage, and only known answers tell them
// apart.

// One payload, counted by hand. `total` deliberately does not equal the sum of
// the `channel` rows — a context key is not a partition — and `culture` does.
const SEGMENTS = {
  total: {
    label: "",
    sessions: 100,
    interactions: 412,
    feedback: 40,
    feedbackPos: 23,
    recognized: 300,
    unrecognized: 20,
    qualitySum: 24000,
  },
  groups: [
    {
      kind: "culture",
      name: "",
      foldedValues: 0,
      overlapping: false,
      rows: [
        { label: "nl", sessions: 80, interactions: 330, feedback: 34, feedbackPos: 20, recognized: 250, unrecognized: 15, qualitySum: 20000 },
        { label: "en", sessions: 20, interactions: 82, feedback: 6, feedbackPos: 3, recognized: 50, unrecognized: 5, qualitySum: 4000 },
      ],
    },
    {
      kind: "context",
      name: "chan, nel",
      foldedValues: 3,
      overlapping: true,
      rows: [
        { label: "web", sessions: 60, interactions: 240, feedback: 0, feedbackPos: 0, recognized: 200, unrecognized: 10, qualitySum: 16000 },
        { label: "app | phone", sessions: 30, interactions: 120, feedback: 4, feedbackPos: 4, recognized: 0, unrecognized: 0, qualitySum: 0 },
      ],
    },
  ],
  contextKeys: [{ name: "chan, nel", sessions: 90, distinctValues: 5 }],
  metadataKeys: [],
  cultureValues: 2,
}

test("a segment row's shares are of the rows that could have them", () => {
  const table = insSegmentTable(SEGMENTS)
  const row = (label) => table.rows.find((r) => r.label === label)
  const col = (label) => table.headers.indexOf(label) - 1

  // 23 of 40 rated turns, to two decimals — the column someone compares
  // against last month's figure, where insPct's whole-number rounding above
  // 10% throws away exactly the difference they are looking for.
  assert.strictEqual(row("Total").cells[col("Positive feedback")], "57.50%")
  // 300 of the 320 turns the recognizer scored — *not* of 412 interactions.
  assert.strictEqual(row("Total").cells[col("Recognition rate")], "93.75%")
  // The mean over every turn the recognizer attempted, zeros included — the
  // portal's definition: 24000 / 320, not 24000 / 300 (which read 80.00%).
  assert.strictEqual(row("Total").cells[col("Recognition quality")], "75.00%")
  assert.strictEqual(row("Total").cells[col("Interactions")], insNumOf(412))
  assert.strictEqual(row("Total").cells[col("Conversations")], insNumOf(100))

  // A share of nothing is a dash, never 0% — "0% positive" is a claim about
  // ratings nobody left, and it is exactly the row people ask about.
  assert.strictEqual(row("chan, nel: web").cells[col("Positive feedback")], "—")
  assert.strictEqual(row("chan, nel: app | phone").cells[col("Recognition rate")], "—")
  assert.strictEqual(row("chan, nel: app | phone").cells[col("Recognition quality")], "—")
  assert.strictEqual(insSegShare(0, 0), "—")
  assert.strictEqual(insSegPct(93.754), "93.75%")

  // Culture partitions the result set; the table has to still add up.
  const cultureRows = ["Culture: nl", "Culture: en"].map(row)
  assert.strictEqual(
    cultureRows.reduce((a, r) => a + Number(r.data[col("Interactions")]), 0),
    SEGMENTS.total.interactions,
  )
})

// A locale that groups thousands is what a spreadsheet has to parse rather than
// read, so the two forms of a cell differ — and only in that.
function insNumOf(n) {
  return Number(n).toLocaleString()
}

test("a pasted cell carries the figure without the thousands separators", () => {
  const table = insSegmentTable(SEGMENTS)
  const at = table.headers.indexOf("Interactions") - 1
  const total = table.rows.find((r) => r.label === "Total")
  assert.strictEqual(total.data[at], "412")
  assert.strictEqual(
    total.cells.length,
    total.data.length,
    "the two renderings of one row disagree about how many columns it has",
  )
  // Every non-heading row, every column.
  for (const r of table.rows) {
    if (r.kind === "head") continue
    assert.strictEqual(r.cells.length, table.headers.length - 1, r.label)
    assert.strictEqual(r.data.length, table.headers.length - 1, r.label)
  }
})

test("a breakdown that hides rows or double-counts them says so", () => {
  const table = insSegmentTable(SEGMENTS)
  const head = table.rows.find((r) => r.kind === "head" && r.label.includes("chan"))
  assert.ok(head, "the context block has no heading")
  assert.strictEqual(head.label, "Context · chan, nel")
  assert.ok(head.note.includes("3 more values"), head.note)
  assert.ok(head.note.includes("overlap"), head.note)
  // Culture here hides nothing and overlaps nothing, so it claims neither.
  const culture = table.rows.find((r) => r.kind === "head" && r.label === "Culture")
  assert.strictEqual(culture.note, "")
})

test("the table's three text twins carry every value it shows", () => {
  const table = insSegmentTable(SEGMENTS)
  const tsv = insTableTsv(table)
  const lines = tsv.split("\n")
  assert.strictEqual(lines[0], table.headers.join("\t"))
  for (const r of table.rows) {
    if (r.kind === "head") continue
    for (const v of r.data) {
      assert.ok(tsv.includes(v), "the TSV is missing " + v + " from " + r.label)
    }
  }
  // A heading keeps a line of its own with a blank one above it — the shape the
  // hand-built spreadsheet already has.
  const headAt = lines.indexOf("Culture")
  assert.ok(headAt > 0, "the heading is not on a line of its own")
  assert.strictEqual(lines[headAt - 1], "")

  // A label holding a comma has to be quoted in the CSV and left alone in the
  // TSV; one holding a pipe has to be escaped in the Markdown or it ends its
  // own cell. Both are real context values.
  const csv = insTsvToCsv(tsv)
  assert.ok(csv.includes('"Context · chan, nel"'), csv.slice(0, 400))
  assert.ok(!tsv.includes('"Context'), "the TSV never needed quoting")
  const md = insTsvToMarkdown(tsv)
  assert.ok(md.includes("app \\| phone"), "a pipe inside a cell was not escaped")
  // Every row of a pipe table is the width of its header, including the short
  // heading rows — one narrow row and it stops rendering as a table at all.
  const width = table.headers.length
  for (const line of md.split("\n")) {
    if (!line.startsWith("|")) continue
    // The escaped pipes are cell *content*, not separators — counting them is
    // how the escaping the line above asserts would read as a wider row.
    assert.strictEqual(
      line.replace(/\\\|/g, "").split("|").length - 2,
      width,
      "a Markdown row is not the table's width: " + line,
    )
  }
})

test("the Segments card is a table, and every export path reaches it", () => {
  const cards = insBuildCards({ ...CONVS, segments: SEGMENTS }, INS_THEME_SCREEN)
  const card = cards.find((c) => c.section === "Segments")
  assert.ok(card, "no Segments card was built")
  assert.ok(card.table && !card.spec, "the Segments card is not a chart")
  assert.strictEqual(insCardTsv(card), insTableTsv(card.table))
  // The chart cards still go the other way through the same function.
  const chart = cards.find((c) => c.section === "Volume")
  assert.strictEqual(insCardTsv(chart), insChartTsv(chart.spec))

  // Both HTML renderings are well-formed and hold every value on screen.
  for (const html of [insSegmentTableHtml(card.table), insReportTableHtml(card.table)]) {
    assertWellFormed("<svg>" + html + "</svg>")
    for (const r of card.table.rows) {
      if (r.kind === "head") continue
      for (const v of r.cells) {
        assert.ok(
          html.includes(v.replace(/&/g, "&amp;")),
          "a value on screen is missing from an export: " + v,
        )
      }
    }
  }
  // The mail report is styled inline throughout — a class or a <style> block
  // is stripped by every mail client, and the table would arrive unformatted.
  const mail = insReportTableHtml(card.table)
  assert.ok(!/class=/.test(mail), "the mail table carries a class")
  assert.ok(!/<style/.test(mail), "the mail table carries a stylesheet")
})

// ── The arrangement ──────────────────────────────────────────────────
//
// A preset is applied to *next month's* numbers, which is the case none of the
// above covers: rows that no longer come back, rows that were not there when it
// was saved, and rows somebody removed on purpose. All three produce a
// plausible-looking table if they are got wrong — one that is quietly shorter,
// or quietly missing the value that mattered.

test("a row id survives a value that contains the obvious separators", () => {
  for (const value of ["web", "Stoppen, Faciliteitenkaart", "a|b", "x:y", ""]) {
    const id = insSegRowId("context", "chan|nel", value)
    assert.strictEqual(insSegLabelOf(id), value.toLowerCase(), "round-trip failed for " + value)
  }
})

test("an arrangement keeps rows the data lost and appends ones it gained", () => {
  const saved = insSegNaturalLayout(SEGMENTS)
  // Renamed, reordered and one row taken out by hand.
  saved.entries = saved.entries.filter((e) => e.t !== "head")
  const nl = saved.entries.find((e) => e.t === "row" && e.id.endsWith("nl"))
  nl.label = "NL"
  saved.dropped = [insSegRowId("culture", "", "en")]
  saved.entries = saved.entries.filter((e) => e.id !== saved.dropped[0])

  // Next month: `web` is gone, `kiosk` is new, everything else moved.
  const next = JSON.parse(JSON.stringify(SEGMENTS))
  next.groups[1].rows = [
    next.groups[1].rows[1],
    { label: "kiosk", sessions: 5, interactions: 20, feedback: 1, feedbackPos: 1, recognized: 18, unrecognized: 1, qualitySum: 1500 },
  ]

  const table = insSegmentTable(next, saved)
  const labels = table.rows.map((r) => r.label)
  assert.deepStrictEqual(labels, ["Total", "NL", "chan, nel: web", "chan, nel: app | phone", "chan, nel: kiosk"])

  // The order the arrangement chose is untouched, and the rename with it.
  const row = (l) => table.rows.find((r) => r.label === l)
  assert.strictEqual(row("NL").missing, false, "nl still has numbers")
  // `web` no longer comes back: kept, dashed, and marked rather than dropped —
  // a report that silently gets shorter is the failure this prevents.
  assert.strictEqual(row("chan, nel: web").missing, true)
  assert.deepStrictEqual(row("chan, nel: web").cells, ["—", "—", "—", "—", "—", "—"])
  assert.deepStrictEqual(row("chan, nel: web").data, ["", "", "", "", "", ""])
  // `kiosk` was not in the arrangement: appended at the end and marked, where
  // it is obvious, rather than slotted into a block nobody put it in.
  assert.strictEqual(row("chan, nel: kiosk").isNew, true)
  assert.strictEqual(table.added, 1)
  // …and the row this fixture dropped by hand is simply not there.
  assert.strictEqual(row("Culture: en"), undefined)
})

test("a value is one row whatever its case, and is named with its key", () => {
  // The backend folds `True` and `true`; the label it sends is whichever
  // spelling is commoner, and that can change month to month. The id cannot.
  assert.strictEqual(
    insSegRowId("context", "Verblijf", "True"),
    insSegRowId("context", "Verblijf", "true"),
  )
  // An arrangement saved before the fold still finds its row, and two
  // spellings saved as two rows become one.
  const layout = insSegNormalizeLayout({
    entries: [
      { t: "row", id: "context\x1fVerblijf\x1fTrue", label: null },
      { t: "row", id: "context\x1fVerblijf\x1ftrue", label: "Stays" },
    ],
    dropped: ["context\x1fVerblijf\x1fFALSE", "context\x1fVerblijf\x1ffalse"],
  })
  assert.deepStrictEqual(layout.entries.map((e) => e.id), [
    insSegRowId("context", "Verblijf", "true"),
  ])
  assert.deepStrictEqual(layout.dropped, [insSegRowId("context", "Verblijf", "false")])

  // Named `Key: value` in every rendering, so a row moved out from under its
  // heading is still some key's `true` and not an anonymous one.
  const seg = {
    ...SEGMENTS,
    groups: [
      {
        kind: "context",
        name: "Verblijf",
        foldedValues: 0,
        overlapping: false,
        rows: [{ ...SEGMENTS.groups[0].rows[0], label: "True" }],
      },
    ],
  }
  const table = insSegmentTable(seg)
  const value = table.rows.find((r) => r.kind === "value")
  assert.strictEqual(value.label, "Verblijf: True")
  assert.ok(insTableTsv(table).includes("Verblijf: True\t"))
  const html = insSegmentTableHtml(table, false)
  assert.ok(html.includes('<span class="ins-seg-key">Verblijf:</span> True'), html)
  // A renamed row keeps its own name, key and all.
  const renamed = insSegmentTable(seg, {
    entries: [{ t: "row", id: insSegRowId("context", "Verblijf", "true"), label: "Stays over" }],
    dropped: [],
  })
  assert.strictEqual(renamed.rows[0].label, "Stays over")
})

test("a row removed by hand stays removed on the next read", () => {
  const saved = insSegNaturalLayout(SEGMENTS)
  const id = insSegRowId("culture", "", "en")
  saved.entries = saved.entries.filter((e) => e.id !== id)
  saved.dropped = [id]
  const table = insSegmentTable(SEGMENTS, saved)
  assert.ok(
    !table.rows.some((r) => r.label === "Culture: en"),
    "a row taken out came back on the next read",
  )
  assert.strictEqual(table.added, 0, "and it was not counted as a new value either")
})

test("the arrangement decides the export, line for line", () => {
  const saved = {
    entries: [
      { t: "total", label: "Total (nochat false)" },
      { t: "gap", label: null },
      { t: "head", gid: null, label: "Channels" },
      { t: "row", id: insSegRowId("context", "chan, nel", "web"), label: "Web" },
    ],
    dropped: [
      insSegRowId("context", "chan, nel", "app | phone"),
      insSegRowId("culture", "", "nl"),
      insSegRowId("culture", "", "en"),
    ],
  }
  const table = insSegmentTable(SEGMENTS, saved)
  const tsv = insTableTsv(table)
  const lines = tsv.split("\n")
  assert.strictEqual(lines[1], [
    "Total (nochat false)", "40", "57.50%", "93.75%", "75.00%", "412", "100",
  ].join("\t"))
  // A blank line asked for is a blank row in the sheet, and a heading with no
  // group behind it keeps the name it was given.
  assert.strictEqual(lines[2], "")
  assert.ok(lines.includes("Channels"), tsv)
  assert.ok(tsv.includes("Web\t"), tsv)
  assert.ok(!tsv.includes("app | phone"), "a dropped row reached the export")
  // Every text twin renders the same lines.
  assert.ok(insTsvToCsv(tsv).includes('"Total (nochat false)"') || insTsvToCsv(tsv).includes("Total (nochat false)"))
  const width = table.headers.length
  for (const line of insTsvToMarkdown(tsv).split("\n")) {
    if (!line.startsWith("|")) continue
    assert.strictEqual(line.replace(/\\\|/g, "").split("|").length - 2, width, line)
  }
})

// Dropping a line removes it before re-inserting it, which shifts every index
// below the one it came from. Get that wrong and every downward drag lands one
// row late — which still looks like a working drag, and is only visible by
// counting.
test("a line lands where it was dropped, in both directions", () => {
  const lines = ["a", "b", "c", "d"]
  const move = (from, at, after) => {
    const out = lines.slice()
    const [x] = out.splice(from, 1)
    out.splice(insSegDropIndex(from, at, after), 0, x)
    return out.join("")
  }
  // Upward: onto the top half of `a` is before it; onto its bottom half is
  // after it.
  assert.strictEqual(move(3, 0, false), "dabc")
  assert.strictEqual(move(3, 0, true), "adbc")
  // Downward, where the shift bites.
  assert.strictEqual(move(0, 3, true), "bcda", "the bottom half of the last line is the end")
  assert.strictEqual(move(0, 3, false), "bcad")
  assert.strictEqual(move(0, 1, true), "bacd", "one step down is one step, not two")
  // Dropping a line onto itself is a no-op either way round.
  assert.strictEqual(move(1, 1, false), "abcd")
  assert.strictEqual(move(1, 1, true), "abcd")
})

test("a stored arrangement or preset cannot introduce something undrawable", () => {
  assert.strictEqual(insSegNormalizeLayout(null), null)
  assert.strictEqual(insSegNormalizeLayout({ entries: "nope" }), null)
  assert.strictEqual(insSegNormalizeLayout({ entries: [] }), null)
  // Unknown line kinds and rows with no id are dropped, not drawn.
  const layout = insSegNormalizeLayout({
    entries: [
      { t: "spell" },
      { t: "row" },
      { t: "row", id: "contextkv", label: 7 },
      { t: "head" },
      { t: "gap" },
    ],
    dropped: "not an array",
  })
  assert.deepStrictEqual(layout.entries.map((e) => e.t), ["row", "head", "gap"])
  assert.strictEqual(layout.entries[0].label, "7", "a label is coerced to text")
  assert.deepStrictEqual(layout.dropped, [])

  assert.deepStrictEqual(insSegNormalizePresets(null), [])
  const presets = insSegNormalizePresets([
    { name: "  " },
    null,
    {
      id: "p1",
      name: "Monthly",
      breakdowns: [
        { kind: "culture" },
        { kind: "sorcery", name: "x" },
        { kind: "context", name: "" },
        { kind: "context", name: "channel" },
        { kind: "context", name: "channel" },
      ],
      layout: { entries: [{ t: "total" }] },
    },
  ])
  assert.strictEqual(presets.length, 1, "an unnamed preset is not a preset")
  assert.deepStrictEqual(presets[0].breakdowns, [
    { kind: "culture", name: "" },
    { kind: "context", name: "channel" },
  ])
  assert.strictEqual(presets[0].layout.entries.length, 1)
})

test("no Segments card is built when the section was not read", () => {
  assert.strictEqual(
    insBuildCards(CONVS, INS_THEME_SCREEN).some((c) => c.section === "Segments"),
    false,
  )
  // …nor from a payload whose result set is empty, where every share would be
  // a dash and the table would say nothing at all.
  const empty = { ...SEGMENTS, total: { ...SEGMENTS.total, interactions: 0 }, groups: [] }
  assert.strictEqual(
    insBuildCards({ ...CONVS, segments: empty }, INS_THEME_SCREEN).some(
      (c) => c.section === "Segments",
    ),
    false,
  )
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
  // Feedback used to be the derived card this guarded against — drawn from the
  // always-read headline flags under a section nobody chose. It is counted per
  // answer now, from Quality's own fields, so an unchosen Quality has nothing
  // to draw it from: no card, and no feedback tile claiming 0%.
  assert.ok(!all.some((c) => c.section === "Quality"), "a Quality card with Quality not read")
  assert.ok(!insTiles(partial).some((t) => t.label === "Positive feedback"))
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
  // Pinned to UTC: the fixture's day keys are UTC days, and in another zone
  // the series legitimately starts a day earlier or later — which would make
  // this test a statement about the machine running it.
  const volume = insBuildCards(CONVS, INS_THEME_SCREEN, "UTC").find((c) => c.id === "volume")
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

// The bounds the picker sends are instants, not days — a day picked in a zone
// east of Greenwich is stored as the evening before. The chip has to name the
// day that was clicked, and it is the one place that can.
test("the dates chip names the days that were picked, in the display zone", () => {
  insSetDisplayZone("Europe/Amsterdam")
  try {
    const b = insZoneDayBounds("2026-06-01", "2026-06-30", "Europe/Amsterdam")
    assert.strictEqual(b.from, "2026-05-31T22:00:00", "the bound was not shifted")
    const [chip] = insSearchSummary({ dateFrom: b.from, dateTo: b.to })
    assert.strictEqual(chip.value, "2026-06-01 → 2026-06-30")
    // The exact bound is what the query compared against, so it stays one
    // hover away and goes into both reports, which travel without a hover.
    assert.strictEqual(chip.title, b.from + " → " + b.to)

    // A date-only bound has nothing to shift and nothing to trim, so it
    // carries no hover: a tooltip repeating the text under it says nothing.
    const [plain] = insSearchSummary({ dateFrom: "2026-06-01" })
    assert.strictEqual(plain.value, "2026-06-01 → latest")
    assert.strictEqual(plain.title, undefined)
  } finally {
    insSetDisplayZone("")
  }
})

// ── Reading the day×hour table in a timezone ───────────────────────────────
//
// The backend counts UTC days and UTC hours and knows nothing else; every one
// of these is arithmetic the renderer alone performs, and every one of them is
// wrong in a way that still produces a plausible chart.
const ZONES = [
  "UTC",
  "Europe/Amsterdam",
  "America/New_York",
  "Asia/Kolkata", // +05:30 — a half-hour offset
  "Pacific/Chatham", // +12:45 / +13:45 — a quarter-hour offset, and DST
]
const dayOf = (hours) => ({ day: "2026-06-30", hours })
const oneAt = (day, hour, n) => [
  { day, hours: Array.from({ length: 24 }, (_, h) => (h === hour ? n || 1 : 0)) },
]

test("a bucket lands on the day and hour it was in the chosen zone", () => {
  const d = { dayHours: oneAt("2026-06-30", 23) }
  const ams = insTimeSeries(d, "Europe/Amsterdam")
  assert.deepStrictEqual(ams.byDay, [{ label: "2026-07-01", count: 1 }])
  assert.strictEqual(ams.byHour[1].count, 1, "23:00Z is 01:00 in Amsterdam")

  // And the other way: the same instant is still 30 June in New York.
  const ny = insTimeSeries({ dayHours: oneAt("2026-06-30", 23) }, "America/New_York")
  assert.deepStrictEqual(ny.byDay, [{ label: "2026-06-30", count: 1 }])
  assert.strictEqual(ny.byHour[19].count, 1)
})

test("a spring-forward day loses its missing hour and keeps every conversation", () => {
  // Amsterdam, 29 March 2026: 02:00 local never happens. 00:00Z is 01:00
  // local, 01:00Z is 03:00, 02:00Z is 04:00.
  //
  // Only the first three hours are occupied, so nothing from the far end of
  // the day can roll forward into the early hours and mask the gap.
  const hours = Array.from({ length: 24 }, (_, h) => (h < 3 ? 1 : 0))
  const s = insTimeSeries({ dayHours: [{ day: "2026-03-29", hours }] }, "Europe/Amsterdam")
  assert.strictEqual(s.byHour[1].count, 1)
  assert.strictEqual(s.byHour[2].count, 0, "an hour that did not happen holds something")
  assert.strictEqual(s.byHour[3].count, 1)
  assert.strictEqual(s.byHour[4].count, 1)
  assert.strictEqual(
    s.byHour.reduce((a, b) => a + b.count, 0),
    3,
    "a conversation was lost to the clock change",
  )
})

test("an autumn-back day counts the hour that happened twice, once each", () => {
  // Amsterdam, 25 October 2026: 00:00Z and 01:00Z are both 02:00 local.
  const hours = Array.from({ length: 24 }, (_, h) => (h === 0 || h === 1 ? 5 : 0))
  const s = insTimeSeries({ dayHours: [{ day: "2026-10-25", hours }] }, "Europe/Amsterdam")
  assert.strictEqual(s.byHour[2].count, 10, "the repeated hour is not the sum of both")
  assert.strictEqual(
    s.byHour.reduce((a, b) => a + b.count, 0),
    10,
  )
})

test("every bucket is counted exactly once, in every zone", () => {
  const src = [
    dayOf(Array.from({ length: 24 }, (_, h) => h + 1)),
    { day: "2026-07-01", hours: Array.from({ length: 24 }, (_, h) => 24 - h) },
  ]
  const total = src.reduce((a, d) => a + d.hours.reduce((x, y) => x + y, 0), 0)
  for (const zone of ZONES) {
    const s = insTimeSeries({ dayHours: src }, zone)
    const sum = (xs) => xs.reduce((a, b) => a + (b.count === undefined ? b : b.count), 0)
    assert.strictEqual(sum(s.byDay), total, zone + ": byDay")
    assert.strictEqual(sum(s.byHour), total, zone + ": byHour")
    assert.strictEqual(sum(s.hourWeekday), total, zone + ": hourWeekday")
    assert.strictEqual(s.byHour.length, 24, zone)
    assert.strictEqual(s.hourWeekday.length, 7 * 24, zone)
    // The day series has to stay in order, since `insFillDays` walks it.
    const labels = s.byDay.map((b) => b.label)
    assert.deepStrictEqual(labels, [...labels].sort(), zone + ": days out of order")
  }
})

test("the heatmap week still starts on Monday after the shift", () => {
  // 2026-06-07 is a Sunday. At 12:00Z it is Sunday in every zone here.
  const s = insTimeSeries({ dayHours: oneAt("2026-06-07", 12) }, "Europe/Amsterdam")
  assert.strictEqual(s.hourWeekday[6 * 24 + 14], 1, "Sunday is not the last row")
  // …and 2026-06-01 is a Monday, the first.
  const m = insTimeSeries({ dayHours: oneAt("2026-06-01", 12) }, "Europe/Amsterdam")
  assert.strictEqual(m.hourWeekday[0 * 24 + 14], 1, "Monday is not the first row")
})

test("a local day becomes the instants that bound it, in the column's own shape", () => {
  const shape = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}$/
  for (const zone of ZONES) {
    const b = insZoneDayBounds("2026-07-01", "2026-07-31", zone)
    // The comparison in `build_session_filter_query` is a plain string compare
    // against an indexed column. A bound of any other shape stops being one.
    assert.ok(shape.test(b.from), zone + ": " + b.from)
    assert.ok(shape.test(b.to), zone + ": " + b.to)
    assert.ok(b.from < b.to, zone)
    // And it round-trips: the day that was picked is the day that comes back.
    assert.strictEqual(insZoneDayOf(b.from, zone), "2026-07-01", zone)
    assert.strictEqual(insZoneDayOf(b.to, zone), "2026-07-31", zone)
  }
  assert.deepStrictEqual(insZoneDayBounds("2026-07-01", "2026-07-01", "UTC"), {
    from: "2026-07-01T00:00:00",
    to: "2026-07-01T23:59:59",
  })
  // A bound landing on a clock change takes the offset in force at the bound
  // — which is the whole reason the resolve is two passes and not one.
  assert.strictEqual(
    insZoneDayBounds("2026-03-29", "2026-03-29", "Europe/Amsterdam").from,
    "2026-03-28T23:00:00",
  )
})

test("the zone label is a place, never an offset", () => {
  assert.strictEqual(insZoneLabel("Europe/Amsterdam"), "Amsterdam")
  assert.strictEqual(insZoneLabel("America/New_York"), "New York")
  assert.strictEqual(insZoneLabel("UTC"), "UTC")
  // An offset would be right for half the year and wrong for the other half,
  // and a chart spanning both has no single correct one to print.
  assert.ok(!/[+-]\d/.test(insZoneLabel("Europe/Amsterdam")))
})

// ── Content charts point at the things they name ───────────────────────────
//
// `qa-1234` is an identifier, not an answer to "which Article?". The parsing
// is the fragile half — `dn-` prefixes a Dialog node and `qa-` an Article, and
// reading one as the other still produces a plausible number.
test("a bucket label is parsed as the thing it actually names", () => {
  assert.deepStrictEqual(insParseContentLabel("articles", "qa-1234"), {
    kind: "article",
    id: 1234,
  })
  assert.deepStrictEqual(insParseContentLabel("dialogNodes", "dn-6391-4"), {
    kind: "dialogNode",
    id: 6391,
    node: 4,
  })
  assert.deepStrictEqual(insParseContentLabel("dialogs", "6391"), {
    kind: "dialog",
    id: 6391,
  })
  assert.deepStrictEqual(insParseContentLabel("entities", "parkeren"), {
    kind: "entity",
    name: "parkeren",
  })

  // A `dn-` id must never read as an Article, in either direction.
  assert.strictEqual(insParseContentLabel("articles", "dn-6391-4"), null)
  assert.strictEqual(insParseContentLabel("dialogNodes", "qa-1234"), null)
  // A Dialog id is a bare number; a prefixed one is a different kind of thing.
  assert.strictEqual(insParseContentLabel("dialogs", "dn-6391-4"), null)
  assert.strictEqual(insParseContentLabel("dialogNodes", "dn-6391"), null)

  // The backend's fallback labels name no thing and must not resolve to one.
  for (const empty of ["(unnamed)", "(unknown)", "(none)", "(empty)", ""]) {
    for (const kind of ["entities", "articles", "dialogs", "dialogNodes"]) {
      assert.strictEqual(insParseContentLabel(kind, empty), null, kind + " " + empty)
    }
  }
})

test("with no content export loaded, a bar keeps its id and is not clickable", () => {
  // The normal case when the app is used for conversations alone. Nothing
  // resolves, so nothing claims to.
  const data = insLinkBuckets("articles", [{ label: "qa-1234", count: 9 }])
  assert.deepStrictEqual(data, [{ label: "qa-1234", value: 9 }])
  assert.strictEqual(data[0].nav, undefined)
})

test("a destination is drawn for the screen and never for an export", () => {
  const spec = {
    kind: "bars",
    unit: "conversations",
    total: 10,
    data: [{ label: "qa-1234", value: 9, tip: "qa-1234 · Openingstijden", nav: "article:1234" }],
  }
  const screen = insRenderChart(spec, INS_THEME_SCREEN).svg
  const exported = insRenderChart(spec, INS_THEME_EXPORT).svg
  assert.ok(screen.includes('data-ins-nav="article:1234"'), "no destination on screen")
  // An exported SVG carries none: inert inside an `<img>`, dead weight in a
  // file, and one fewer thing for the standalone assertion to police.
  assert.ok(!/data-ins-nav/.test(exported), "a destination leaked into the export")
  assert.ok(!/cursor:pointer/.test(exported), "a cursor style leaked into the export")
  // The resolved name reaches the tooltip and the table either way.
  assert.ok(screen.includes("qa-1234 · Openingstijden"))
})

// ── The caption block ──────────────────────────────────────────────────────
//
// An exported chart travels alone. A bar reading 43% is alarming or
// unremarkable entirely depending on what was searched for, and the header
// saying so is on a screen the recipient never saw.
const CAPTIONED = (spec, lines) => ({
  ...spec,
  caption: lines || [
    { text: "Conversations per day", style: "title" },
    { text: "Counted on the day the conversation started.", style: "note" },
    { text: "Search: parkeren · user", style: "meta" },
  ],
})

test("a captioned chart still depends on nothing outside itself", () => {
  // The same battery as the un-captioned one. A caption drawn as HTML, or
  // referencing a stylesheet, would render as a blank box — and only after it
  // had been pasted somewhere.
  const specs = [
    CAPTIONED({ kind: "columns", data: bars(6) }),
    CAPTIONED({ kind: "bars", data: bars(5), total: 40 }),
    CAPTIONED({ kind: "stack", data: bars(3), total: 27 }),
    CAPTIONED({ kind: "heatmap", grid: Array.from({ length: 168 }, (_, i) => i % 9) }),
    CAPTIONED({ kind: "area", data: bars(80, 90) }),
  ]
  for (const spec of specs) {
    const { svg } = insRenderChart(spec, INS_THEME_EXPORT)
    assertWellFormed(svg)
    assert.ok(!/\sclass=/.test(svg), spec.kind + ": carries a class attribute")
    assert.ok(!/<style/.test(svg), spec.kind + ": carries a <style> block")
    assert.ok(!/url\(|href=|xlink/.test(svg), spec.kind + ": references something external")
    for (const m of svg.match(/<(rect|path|line|circle|text)\b[^>]*>/g) || []) {
      assert.ok(/fill="|stroke="/.test(m), spec.kind + ": an element with no colour")
    }
    for (const m of svg.match(/<text\b[^>]*>/g) || []) {
      assert.ok(/font-family="/.test(m), spec.kind + ": text with no font of its own")
    }
    // The caption text actually reached the picture.
    assert.ok(svg.includes("Conversations per day"), spec.kind + ": no caption drawn")
  }
})

test("a caption grows the chart instead of drawing over it", () => {
  for (const kind of ["columns", "bars", "stack", "heatmap", "area"]) {
    const base =
      kind === "heatmap"
        ? { kind, grid: Array.from({ length: 168 }, (_, i) => i % 9) }
        : { kind, data: bars(5), total: 40 }
    const plain = insRenderChart(base, INS_THEME_EXPORT)
    const withCap = insRenderChart(CAPTIONED(base), INS_THEME_EXPORT)
    assert.ok(withCap.height > plain.height, kind + ": the caption took no space")
    assert.strictEqual(withCap.width, plain.width, kind + ": the caption changed the width")
    // Nothing the builder drew moved: every mark keeps the y it had before.
    const ys = (svg) =>
      [...svg.matchAll(/<rect x="[-\d.]+" y="([-\d.]+)"/g)].map((m) => m[1]).join(",")
    assert.strictEqual(ys(withCap.svg), ys(plain.svg), kind + ": a mark moved")
    // …and every caption row is below where the plain chart ended.
    const capTexts = [...withCap.svg.matchAll(/<text[^>]*\by="([\d.]+)"[^>]*>([^<]*)</g)]
      .filter((m) => m[2].includes("Conversations per day"))
    assert.ok(capTexts.length, kind + ": caption title not found")
    for (const m of capTexts) {
      assert.ok(Number(m[1]) > plain.height, kind + ": the caption is drawn over the chart")
    }
  }
})

test("nothing in a caption is drawn outside the canvas", () => {
  for (const d of [CONVS, TURNS]) {
    for (const card of insBuildCards(d, INS_THEME_SCREEN, "UTC")) {
      const lines = insCaptionLines(card, d, { query: "parkeren" }, "UTC")
      assertInsideCanvas(CAPTIONED(card.spec, lines), "captioned " + card.id)
    }
  }
})

test("a caption line too long for the chart is wrapped, never clipped", () => {
  const long = "Stoppen ".repeat(60).trim()
  const layout = insCaptionLayout([{ text: long, style: "note" }], 400)
  assert.ok(layout.rows.length > 1, "a 480-character note produced one row")
  assert.ok(layout.rows.length <= INS_CAP_MAX_LINES, "an entry outgrew its cap")
  for (const r of layout.rows) {
    assert.ok(
      insTextWidth(r.text, r.style.size) <= 400 + 0.5,
      "a wrapped row is wider than the chart: " + r.text,
    )
  }
  // A single word with nothing to wrap on is shortened rather than allowed to
  // run off the canvas — the same rule every label here follows.
  const oneWord = insCaptionLayout([{ text: "x".repeat(400), style: "note" }], 200)
  assert.strictEqual(oneWord.rows.length, 1)
  assert.ok(oneWord.rows[0].text.endsWith("…"), oneWord.rows[0].text)
  // And a pathological set of entries cannot grow a chart without limit.
  const many = insCaptionLayout(
    Array.from({ length: 40 }, (_, i) => ({ text: "line " + i, style: "meta" })),
    400,
  )
  assert.strictEqual(many.rows.length, INS_CAP_MAX_ROWS)
})

test("a caption full of markup cannot break the picture", () => {
  const nasty = '</text><script>alert(1)</script> & < > "'
  const spec = CAPTIONED({ kind: "columns", data: bars(4) }, [
    { text: nasty, style: "title" },
    { text: nasty, style: "meta" },
  ])
  const { svg } = insRenderChart(spec, INS_THEME_EXPORT)
  assertWellFormed(svg)
  assert.ok(!/<script/.test(svg), "a script tag survived into the picture")
})

test("every chart builder closes its own tag exactly once", () => {
  // `insRenderChart` trims the trailing `</svg>` to append the caption. That
  // is only safe while every builder ends with exactly one.
  const specs = [
    { kind: "columns", data: bars(4) },
    { kind: "bars", data: bars(4), total: 40 },
    { kind: "stack", data: bars(3), total: 27 },
    { kind: "heatmap", grid: Array.from({ length: 168 }, (_, i) => i % 9) },
    { kind: "area", data: bars(80, 90) },
    { kind: "line", data: bars(20, 30) },
    { kind: "donut", data: bars(12), total: 60 },
  ]
  for (const spec of specs) {
    for (const s of [spec, CAPTIONED(spec)]) {
      const { svg } = insRenderChart(s, INS_THEME_EXPORT)
      assert.strictEqual((svg.match(/<svg\b/g) || []).length, 1, spec.kind)
      assert.strictEqual((svg.match(/<\/svg>/g) || []).length, 1, spec.kind)
      assert.ok(svg.endsWith("</svg>"), spec.kind + ": does not end with its own close")
    }
  }
})

test("the caption carries the slice the chart was made from", () => {
  const args = {
    query: "parkeren",
    queryScope: "user",
    filter: "neg_feedback",
    dateFrom: "2026-06-01",
  }
  const card = insBuildCards(CONVS, INS_THEME_SCREEN, "UTC").find((c) => c.id === "volume")
  const lines = insCaptionLines(card, CONVS, args, "Europe/Amsterdam")
  const text = lines.map((l) => l.text).join(" | ")
  assert.ok(text.includes(card.title), "the caption does not name its own chart")
  assert.ok(text.includes(card.note), "the caption drops the chart's note")
  // Built *through* `insSearchSummary`, so a captioned chart and a pasted
  // report cannot end up describing different slices.
  for (const c of insSearchSummary(args)) {
    assert.ok(text.includes(c.label + ": " + c.value), c.label + " is missing")
  }
  // The trimmed form, not the exact bound: the caption is read by a person
  // looking at a picture. The exact bound stays in the report, which is where
  // a machine-checkable boundary actually matters.
  const bounds = insZoneDayBounds("2026-06-01", "2026-06-30", "Europe/Amsterdam")
  const dated = insCaptionLines(card, CONVS, { dateFrom: bounds.from, dateTo: bounds.to }, "UTC")
    .map((l) => l.text)
    .join(" | ")
  assert.ok(dated.includes("Dates: 2026-06-01 → 2026-06-30"), dated)
  assert.ok(!dated.includes("T22:00:00"), "the caption printed a raw bound")
  assert.ok(text.includes("Amsterdam"), "the caption does not say which zone it is in")
})

// ── The table twins ────────────────────────────────────────────────────────

test("the CSV and Markdown twins carry every value the TSV does", () => {
  for (const d of [CONVS, TURNS]) {
    for (const card of insBuildCards(d, INS_THEME_SCREEN, "UTC")) {
      const tsv = insChartTsv(card.spec).split("\n")
      assert.strictEqual(insChartCsv(card.spec).split("\n").length, tsv.length, card.id)
      // Markdown adds exactly one row: the header separator.
      assert.strictEqual(
        insChartMarkdown(card.spec).split("\n").length,
        tsv.length + 1,
        card.id,
      )
    }
  }
})

test("a CSV cell containing a comma is quoted, and the TSV is untouched", () => {
  const spec = {
    kind: "bars",
    axisLabel: "Entity",
    unit: "conversations",
    total: 100,
    data: [
      { label: "Stoppen, Faciliteitenkaart", value: 60 },
      { label: 'a "quoted" one', value: 40 },
    ],
  }
  const csv = insChartCsv(spec).split("\n")
  assert.strictEqual(csv[1], '"Stoppen, Faciliteitenkaart",60,60%')
  assert.strictEqual(csv[2], '"a ""quoted"" one",40,40%')
  // The tab-separated twin never needed quoting and must not start.
  assert.ok(insChartTsv(spec).includes("Stoppen, Faciliteitenkaart\t60\t"))
  // A pipe inside a Markdown cell would end the cell.
  assert.ok(insChartMarkdown({ ...spec, data: [{ label: "a | b", value: 1 }] })
    .includes("a \\| b"))
})

// ── Switching the unit ─────────────────────────────────────────────────────
//
// The two readings are two questions about one result set, and the answer to
// the one you switched away from does not go stale while you are looking at
// the other. A miss where there should be a hit is a wasted read; a hit where
// there should be a miss draws numbers that are no longer the answer, with
// nothing on screen to say so.
const ALL_READ = { volume: true, quality: true, content: true }

test("a payload already read is a redraw, not a read", () => {
  const entry = { payload: {}, read: { ...ALL_READ } }
  assert.deepStrictEqual(insCacheDecide(entry, ALL_READ), { hit: true, missing: [] })
  // Asking for less than was read is still a hit: the extra sections are
  // simply not rendered.
  assert.deepStrictEqual(insCacheDecide(entry, { volume: true }), {
    hit: true,
    missing: [],
  })
})

test("a payload missing a section is topped up, never re-read whole", () => {
  const entry = { payload: {}, read: { volume: true, quality: true, content: false } }
  const d = insCacheDecide(entry, ALL_READ)
  assert.strictEqual(d.hit, false)
  // Only the missing one. Re-reading the two already in hand is the wait this
  // whole path exists to remove.
  assert.deepStrictEqual(d.missing, ["content"])
})

test("nothing read is a full read, and every chosen section is asked for", () => {
  const d = insCacheDecide(undefined, ALL_READ)
  assert.strictEqual(d.hit, false)
  assert.deepStrictEqual(d.missing, INS_READ_SECTIONS.filter((k) => ALL_READ[k]))
  // A section that was not chosen is never asked for, cached or not.
  assert.deepStrictEqual(insCacheDecide(undefined, { volume: true }).missing, ["volume"])
})

test("the two readings of one search are two cache entries", () => {
  const sig = JSON.stringify({ query: "parkeren", filter: "all" })
  assert.notStrictEqual(
    insCacheKey("conversations", sig),
    insCacheKey("interactions", sig),
  )
  // …and the same reading of a different search is a third.
  assert.notStrictEqual(
    insCacheKey("conversations", sig),
    insCacheKey("conversations", JSON.stringify({ query: "fiets", filter: "all" })),
  )
  // The key is stable across two builds of the same args object, which is what
  // the whole cache rests on.
  const again = JSON.stringify({ query: "parkeren", filter: "all" })
  assert.strictEqual(insCacheKey("conversations", sig), insCacheKey("conversations", again))
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
  const ZONE = "Europe/Amsterdam"
  const NAMED = /\bUTC\b|Amsterdam/
  const offenders = []
  for (const d of [CONVS, TURNS]) {
    for (const card of insBuildCards(d, INS_THEME_SCREEN, ZONE)) {
      if (NAMED.test(card.note || "")) offenders.push(card.id + " note")
      if (NAMED.test(card.title)) offenders.push(card.id + " title")
      const svg = insRenderChart(card.spec, INS_THEME_SCREEN).svg
      const texts = [...svg.matchAll(/<(?:text|title)[^>]*>([^<]*)</g)].map((m) => m[1])
      for (const t of texts) {
        // The hour axis is the one place it belongs, and it is an axis label.
        if (NAMED.test(t) && !/^Hour \(.+\)$/.test(t)) offenders.push(card.id + ": " + t)
      }
      // Same exemption in the copied table: the hour column is named after
      // its axis, and a spreadsheet column called "Hour" alone is ambiguous.
      const header = insChartTsv(card.spec).split("\n")[0]
      if (NAMED.test(header) && !/^Hour \(.+\)\t/.test(header)) {
        offenders.push(card.id + " table header: " + header)
      }
    }
  }
  assert.deepStrictEqual(offenders, [])
  // …and the one place it does belong is still there, so this cannot pass by
  // the disclaimer having been deleted outright — and it names the zone the
  // chart was actually drawn in, not a constant.
  const named = (z) =>
    insBuildCards(CONVS, INS_THEME_SCREEN, z).find((c) => c.id === "hour").spec.axisLabel
  assert.strictEqual(named(ZONE), "Hour (Amsterdam)")
  assert.strictEqual(named("UTC"), "Hour (UTC)")
  assert.strictEqual(named("America/New_York"), "Hour (New York)")
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
      // only the matching ones. Feedback counts answers in both, the same way.
      const expected = card.id === "recogType" ? "interactions" : card.id === "feedback" ? "answers" : noun
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
  // Answers, not every row: 300 of the 412 rows in the fixture are answers.
  assert.strictEqual(insUnitLine(CONVS), "100 conversations · 300 interactions")
  assert.strictEqual(insUnitLine({ ...CONVS, answerCount: undefined }), "100 conversations · 412 interactions")
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

// The opening question is a property of a conversation, not of a turn that
// happened to match, and has no per-turn reading. Feedback is counted per
// answer, so unlike the old per-conversation flags it reads in both.
test("opening questions are left out of the interactions reading; feedback is not", () => {
  const convIds = insBuildCards(CONVS, INS_THEME_SCREEN).map((c) => c.id)
  const turnIds = insBuildCards(TURNS, INS_THEME_SCREEN).map((c) => c.id)
  assert.ok(convIds.includes("feedback") && convIds.includes("questions"))
  assert.ok(turnIds.includes("feedback"), "per-answer feedback is a per-turn fact")
  assert.ok(!turnIds.includes("questions"))
  assert.ok(insBuildCards(TURNS, INS_THEME_SCREEN).find((c) => c.id === "feedback").note.includes("matching answers"))
  assert.ok(insTiles(TURNS).some((t) => t.label === "Positive feedback"))
  assert.ok(!insTiles(CONVS).some((t) => t.label === "Thumbs down"), "the old conversation tile is gone")
  // Everything else survives the switch — the reading is not a smaller feature.
  for (const id of convIds) {
    if (id === "questions") continue
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


// ── Export settings: palette, language, labels, chart types ─────────────

test("a custom export palette recolours every mark, and leaks nothing from the screen", () => {
  const palette = {
    surface: "#101820",
    ink: "#fafafa",
    accents: ["#ff6600", "#00aa88", "#aa00ff"],
    good: "#11aa11",
    warning: "#eeaa00",
    critical: "#cc0000",
  }
  const theme = insExportTheme(palette)
  assert.strictEqual(theme.surface, "#101820")
  assert.strictEqual(theme.series, "#ff6600")
  for (const card of insBuildCards(CONVS, INS_THEME_SCREEN)) {
    if (!card.spec) continue
    const { svg } = insRenderChart(card.spec, theme)
    assertWellFormed(svg)
    // Nothing chosen at build time in the screen palette survives into a
    // picture drawn in another one.
    for (const k of ["surface", "series", "neutral", "ink", "grid"]) {
      assert.ok(
        !svg.toLowerCase().includes(INS_THEME_SCREEN[k].toLowerCase()),
        card.id + ": the screen palette's " + k + " leaked into an export",
      )
    }
  }
  const fb = insBuildCards(CONVS, INS_THEME_SCREEN).find((c) => c.id === "feedback")
  const svg = insRenderChart(fb.spec, theme).svg
  assert.ok(svg.includes("#11aa11") && svg.includes("#cc0000"), "status colours were not applied")
})

test("the default palette is the validated export theme itself", () => {
  assert.strictEqual(insExportTheme(null), INS_THEME_EXPORT)
  assert.strictEqual(insExportTheme(insNormalizePalette({ surface: "nope" })), INS_THEME_EXPORT)
  // A stored palette cannot put a non-colour into an attribute.
  const p = insNormalizePalette({ surface: 'red" onload="x', accents: ["#123456", "blue", 7] })
  assert.strictEqual(p.surface, INS_THEME_EXPORT.surface)
  assert.deepStrictEqual(p.accents, ["#123456"])
})

test("every translated phrase exists in both languages", () => {
  const de = Object.keys(INS_I18N.de).sort()
  const nl = Object.keys(INS_I18N.nl).sort()
  assert.deepStrictEqual(de, nl, "German and Dutch cover different phrases")
  for (const lang of ["de", "nl"]) {
    for (const [k, v] of Object.entries(INS_I18N[lang])) {
      // A slot the English has must survive into the translation.
      for (const slot of k.match(/\{\w+\}/g) || []) {
        assert.ok(v.includes(slot), lang + ": " + JSON.stringify(k) + " lost " + slot)
      }
    }
  }
})

test("an export in German is German, and the screen stays English", () => {
  const de = insWithLang("de", () => insBuildCards(CONVS, INS_THEME_SCREEN))
  const volume = de.find((c) => c.id === "volume")
  assert.strictEqual(volume.title, "Gespräche pro Tag")
  const rec = de.find((c) => c.id === "recognition")
  assert.ok(rec.spec.data.some((d) => d.label === "Null"), "the band label was not translated")
  // …and still wears the status colour its English key names.
  assert.strictEqual(rec.spec.data.find((d) => d.label === "Null").color, "critical")
  assert.ok(insWithLang("de", () => insChartTsv(volume.spec)).startsWith("Tag\tWochentag\tGespräche"))
  assert.strictEqual(insBuildCards(CONVS, INS_THEME_SCREEN)[0].title, "Conversations per day")
  // A tag note is built from slots, so the numbers survive translation.
  const nl = insWithLang("nl", () => insBuildCards(CONVS, INS_THEME_SCREEN))
  const ctx = nl.find((c) => c.id === "context")
  if (ctx) assert.ok(/^Aandeel van de [\d.,]+ gesprekken/.test(ctx.note), ctx.note)
  assert.strictEqual(insT("No such phrase", null, "de"), "No such phrase")
})

test("a chart type is only ever one the card's data can honestly take", () => {
  assert.deepStrictEqual(
    insNormalizeChartTypes({ volume: "donut", feedback: "donut", heat: "bars", nope: "bars" }),
    { feedback: "donut" },
  )
  const cards = insBuildCards(CONVS, INS_THEME_SCREEN)
  const fb = cards.find((c) => c.id === "feedback")
  const asDonut = insApplyChartKind("feedback", fb.spec, { feedback: "donut" })
  assert.strictEqual(asDonut.kind, "donut")
  const out = insRenderChart(asDonut, INS_THEME_EXPORT)
  assertWellFormed(out.svg)
  assertInsideCanvas(asDonut, "feedback as donut")
  // Bars turned into columns tilt their names rather than cutting them.
  const ents = cards.find((c) => c.id === "entities")
  if (ents && ents.spec.data.length > 6) {
    assert.strictEqual(insApplyChartKind("entities", ents.spec, { entities: "columns" }).rotateLabels, true)
  }
  const hour = cards.find((c) => c.id === "hour")
  const line = insApplyChartKind("hour", hour.spec, { hour: "line" })
  assert.ok(!insRenderChart(line, INS_THEME_EXPORT).svg.includes('fill-opacity="0.1"'), "a line has no wash")
})

test("a donut folds a long tail into one counted slice", () => {
  const out = insRenderChart({ kind: "donut", data: bars(14, 30), total: 300 }, INS_THEME_EXPORT)
  assertWellFormed(out.svg)
  assert.strictEqual((out.svg.match(/<path /g) || []).length, 8, "eight slices, not fourteen")
  assert.ok(out.svg.includes("Other (7)"), "the folded slice does not say how many it holds")
  // One slice is a whole ring, and still draws.
  const one = insRenderChart({ kind: "donut", data: [{ label: "a", value: 5 }] }, INS_THEME_EXPORT)
  assert.ok(/<path d="M[^"]+A[^"]+A/.test(one.svg), "a single slice drew nothing")
})

test("day labels follow the chosen format, weekday and weekend options", () => {
  assert.strictEqual(insFmtDay("2026-06-01", true, "iso"), "06-01")
  assert.strictEqual(insFmtDay("2026-06-01", false, "iso"), "2026-06-01")
  assert.strictEqual(insFmtDay("2026-06-01", true, "dm"), "01-06")
  assert.strictEqual(insFmtDay("2026-06-01", true, "dmon"), "1 Jun")
  // The month name is the engine's own (`Mär` or `März` depending on its ICU).
  assert.ok(/^5 Mär/.test(insWithLang("de", () => insFmtDay("2026-03-05", false, "dmon"))))
  assert.strictEqual(insFmtHour("00", "ampm"), "12 AM")
  assert.strictEqual(insFmtHour("13", "ampm"), "1 PM")
  assert.strictEqual(insFmtHour("9", "hhmm"), "09:00")
  const byDay = [
    { label: "2026-06-05", count: 3 },
    { label: "2026-06-06", count: 1 },
  ]
  const on = insVolumeSpec({ byDay }, { ...INS_LABEL_DEFAULTS, weekdayTicks: true })
  assert.strictEqual(on.data[0].tick, "Fri 06-05")
  assert.strictEqual(on.data[1].band, "weekend")
  const off = insVolumeSpec({ byDay }, { ...INS_LABEL_DEFAULTS, weekend: false })
  assert.strictEqual(off.data[1].band, undefined)
  assert.strictEqual(off.data[1].tickColor, undefined)
  assert.deepStrictEqual(insNormalizeLabelOpts({ dateFmt: "weird", weekend: "yes" }), INS_LABEL_DEFAULTS)
})

test("a tilted first label is never cut by the canvas edge", () => {
  const data = []
  for (let i = 0; i < 20; i++) data.push({ label: "x" + i, tick: "Wed 12 Sep 2026", value: i + 1 })
  const spec = { kind: "columns", data, rotateLabels: true }
  assertInsideCanvas(spec, "tilted")
})

test("caption parts can each be left out", () => {
  const card = insBuildCards(CONVS, INS_THEME_SCREEN, "UTC").find((c) => c.id === "volume")
  const none = { ...INS_LABEL_DEFAULTS, capTitle: false, capNote: false, capSearch: false, capUnit: false, capZone: false }
  assert.deepStrictEqual(insCaptionLines(card, CONVS, { query: "x" }, "UTC", none), [])
  const onlyZone = { ...none, capZone: true }
  const lines = insCaptionLines(card, CONVS, {}, "Europe/Amsterdam", onlyZone)
  assert.strictEqual(lines.length, 1)
  assert.strictEqual(lines[0].text, "times in Amsterdam")
})

console.log(failures ? "\n" + failures + " failing" : "\nall insights tests passed")
process.exit(failures ? 1 : 0)
