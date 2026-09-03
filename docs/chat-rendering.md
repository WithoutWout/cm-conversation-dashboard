# Chat rendering

Turning raw interaction rows into a readable thread: CM output formatting, redacted values, per-message metadata.

_Split out of `CLAUDE.md`. Read this before changing anything it covers._

---

The chat view turns raw `interactions` rows into turns (`buildChatTurns`) and renders each row's `output_text` through `parseCmOutput`. Both have had bugs that read as "the chat is broken" rather than "the renderer is wrong", so the rules below are load-bearing.

## CM.com output formatting (`parseCmOutput`)

- **`_` is a line break, never emphasis.** A single `_` is `<br>`, a run of two or more (`__`, or `_  _`) is `<br><br>`. **`**text**` is the only bold marker.**
- **Do not reintroduce `__text__` → `<strong>`.** That rule existed, ran *before* the line-break rules, and therefore bolded whole paragraphs *and* swallowed the two breaks on either side — so answers rendered bold and run together with no spaces. It fired on **93 of 400** distinct sampled answers. `_` marking a break and `__` marking bold cannot both be true; the break wins.
- **List markers.** `*` followed by whitespace becomes `• `; `**` is never a list marker (both the leading and trailing star are guarded with a lookaround). The `*` that prefixes each `%{DialogOption(...)}` / `%{Image(n)}` token is consumed *with* the token, so what survives to the bullet step is a real content bullet. Deleting all markers outright — the old behaviour — flattened genuine lists into unmarked lines.
- **Anchors and `{{variable}}` chips are placeholder-protected** before the underscore pass, or a `_` inside a href or inside `{{opening_hours_to}}` becomes a `<br>` and breaks the element.
- Order inside the function matters and is: card.ask → CTA → DialogOption → other `%{}` tokens → bullets → markdown links → `<a>` links + HTML escaping → (botMode) protect → bold → line breaks → restore.

## Metadata on one message

The tag filter popover aggregates `OutputMetadata` across the whole database; `#msgMetaModal` shows the same data on a single message. They are the two halves of one question — "why is this conversation in my results?" against "what does this answer actually carry?" — and the second had no answer at all before: the column reached the renderer and nothing read it.

- **It is a speech bubble anchored to the tag button, not a centred dialog**, and that is about what the data means rather than about decoration. The values belong to *one message*; a modal in the middle of the screen severs them from the only thing that gives them meaning, and on a long thread you lose which bubble you were reading the moment it opens. Anchored, the answer stays beside the question. It closes on a click outside, on ✕, and on Escape — all three, because a popover with only one way out reads as stuck.
  - **There is deliberately no backdrop**, unlike `.ctx-modal-backdrop`. One would catch outside clicks for free, but it also swallows the wheel, and the thread has to stay scrollable with the bubble riding along on its tag. `_msgMetaOnScroll` re-places it on every scroll — **capture phase, because a scrolling `.chat-thread` does not bubble its scroll event to the document** — and closes it once the anchor leaves that thread. That last part is the one that matters: the tag scrolls under the chat header long before it leaves the window, and a bubble left pointing at the header points at nothing.
  - Outside clicks are a document listener instead, exempting `#msgMetaPop` and `.bubble-meta-btn`. With no backdrop covering it, the tag stays clickable, so **the toggle has to be explicit** — and it lives in `openMessageMetadata` rather than in the listener because the tag stops the click propagating (the bubble underneath toggles its details), so the listener never sees it.
  - **A wheel over the bubble scrolls the thread.** Removing the backdrop made the thread scrollable again *around* the bubble, but the bubble is a fixed-position island: the wheel lands on it, its own list is usually too short to scroll, and it is not a descendant of `.chat-thread`, so there is nothing for the scroll to chain into — leaving the one place the pointer naturally rests as the one place scrolling died. Its list gets first refusal and only while it can actually move; at either end the delta goes to the thread instead of stopping dead. `deltaMode` is normalised because a line- or page-mode wheel would otherwise move the thread three pixels a notch.
  - **The exit is `pop-out`, the entrance reversed**, held on its last frame by `forwards` while `MSG_META_OUT_MS` runs. A bubble that grew out of the tag and then simply vanished left you wondering where it went. `_msgMetaIsOpen` is "open **and not closing**", so a bubble mid-exit is never re-placed, toggled against, or closed twice.
  - **`msgMetaPlacement` is split out as pure arithmetic over three rects**, and `frontend/tests/msg-meta-place.test.js` is the reason: what goes wrong here is never "it fails to open", it is the bubble landing somewhere plausible with its tail pointing at nothing. Every case that produces it — a message at the bottom of the thread, an anchor against the window edge, a bubble too tall for either side — is one no manual pass reliably reaches.
  - **The tail is positioned from the *anchor's* centre, never from the middle of the box.** The box is clamped to the window, so a bubble pushed sideways by the edge would otherwise point at empty space. It is held `MSG_META_TAIL_INSET` clear of the rounded corners, where it would read as detached.
  - **`.msg-meta-pop` deliberately has no `overflow: hidden`** — it would clip the tail off. The rounded corners still hold because only `.msg-meta-body` can overflow and its own `overflow-y: auto` clips it.
  - Placement needs the bubble's real size, so `openMessageMetadata` hides it, shows the overlay, measures, places, and reveals — all synchronously, so the intermediate position is never painted and `pop-in` still plays from the right spot. `transform-origin` follows `--tail-x`, so it grows out of the tag rather than out of its own middle.
- **The button is only rendered when the message has metadata** (`bubbleMetaButtonHtml`). Plenty of rows carry none, and a button that opens an empty dialog on half the messages is worse than no button. Its tooltip states the count, so the bubble is never a surprise.
- **It shares the bottom-right corner with the "▾ details" hint, not the opposite one.** A one-line answer is short enough that top-right and bottom-right are the same place, and the two overlapped — verified by rect, not by eye. `.bubble:has(.bubble-meta-btn)` shifts the hint left by exactly the button's width, so they cannot collide at any bubble height.
- **`.bubble-gutter` is what keeps the answer out from under both of them.** They are absolutely positioned, so the corner is free space right up until the last line of text reaches it and then runs underneath — measured at **26 of 48** message lengths on a realistic sentence. The gutter is an inline spacer at the end of the text, as wide as the controls reach into the content box: while the last line has room it sits there and costs nothing, and when it doesn't it wraps, taking one short line with it and leaving the corner clear.
  - **Reserving the space in the flow instead — a footer row, or padding on the bubble — would have added that height to every message**, including the overwhelming majority that never needed it. Over the same 48 cases only **5** bubbles grow, all by one line, and every one of them was broken before; the other 43 are identical in height. Most of the fix costs nothing at all, because a bubble shrink-wraps its text and simply gets wider.
  - It is hidden while the details are open (`.bubble:has(.bubble-detail.open)`) — the controls then hang over the bottom of the expanded block, which reserves the corner itself through `.bubble-detail`'s `padding-bottom`, and a spacer stranded beside the text would just be a hole in the middle of the bubble.

**The details expand and collapse instead of jumping.** `.bubble-detail` was `display: none` ↔ `block`, so the bubble changed height between one frame and the next and took the whole thread under it along. It is now the `grid-template-rows: 0fr → 1fr` trick, which animates to the content's natural height without anyone measuring it — necessary here, since the detail is built per row and there is nothing sensible to hard-code. Both directions animate, because it is a transition rather than an animation.

- **The animated grid item carries no padding or border**, and that is not a style preference. `min-height: 0` zeroes an item's *content* box, but padding and border sit outside it, so the row's automatic minimum stays at their total — a collapsed detail kept **23px** of height and every bot bubble in the thread grew by that much. They live on `.bubble-detail` itself and animate alongside the row, which also gives the separator a width to wipe in from instead of appearing at full strength.
- `overflow: hidden` on the container is what clips the content on the way; the item needs its own for the `0fr` collapse.
- **Nested values are split into sub-rows** using the same `_flattenMetaEntry` the chips use, so a value reads the same way in both places and **Copy** produces the `key.subkey` names you would search for. An unexpanded blob here would be the same unreadable line the chips used to be.
- **Nothing is hidden.** `conversation_id` and `CURRENT_DATETIME` are excluded from the *filter index* because a chip matching one message is useless; on the message itself they are exactly what someone is looking for.
- `_msgMetaById` is keyed by `logId` and filled during render rather than read off a data attribute, because the two chat paths hold their rows differently (`activeInteractions` for the main view, a local array for Flagged) and a JSON blob per bubble would bloat the DOM of a long conversation.

**With a tag filter on, the chat says which message carries it.** `chatMetaFilters` mirrors the applied metadata filters into the opened chat exactly as `chatMatchEntities` mirrors the E toggle, and the matching bubble gets `.meta-match-highlight`, its tag button turns accent and stops being quiet, and the popover marks the value that did it. Without this a tag filter was the one filter whose reason was invisible: the conversation was in the list and nothing in it said why.

- **Read from `lastConvSearchArgs`, not the live popover state.** `convMetadataFilters` changes as soon as a chip is clicked, so the chat would mark messages against a filter that had not been searched with yet.
- **`__not_set__` is dropped, and that is the point.** It matched the session because *no* message carries the key, so there is no message to point at; marking every message would be noise dressed up as an answer. `setChatMetaFilters` filters it out rather than every call site remembering to.
- **Any one filter is enough to mark a message.** The backend ANDs across names at the *session* level, so two different messages can legitimately satisfy two different names.
- **Marked at leaf level in the popover.** A nested value's sub-keys are separate filters, so `abortTransactionAction.topicName` highlights that sub-row and not the whole `abortTransactionAction` block.
- The accent is deliberate: `<mark>`, the entity `.is-hit` chips and this all mean "here is your match", where the recognition and feedback highlights use their own colours for their own meanings.
- `rowMetaSet` caches on the row like `rowEntityFields` does — a long chat re-renders on every filter change and this parses JSON per message. Safe to cache because the set is derived from the row alone, never from the filters.

## `{{variable}}` is a redacted bot value

The bot side of the same story as `#Variable#` below: the answer was personalised for the user, but the log keeps the template (`Hoi {{name}}!`, `{{attraction\_name}}`, `{{emailAddress}}`, `{{opening_hours_from}}`). `templateVarChip(name)` renders each one as an inline `.tpl-var` chip instead of raw braces, and normalises the export's escaped `\_` back to `_`.

- **`.tpl-var` is deliberately near-padding-free** (2px, no margin, dotted underline rather than a bordered pill). A wider chip pushes the following punctuation away from it — `Hoi {{name}} !` — which reads as a typo. Verified visually before settling on it.
- `rawName` arrives already HTML-escaped, so the fallback branch (an unparseable token) must not escape it a second time. The sanitised branch is restricted to `[\w .-]`, where `esc()` is a no-op.
- `.tpl-var` and `.redacted-value` share one visual language on purpose — both mean "the log never stored this value" — but differ in weight, because one sits mid-sentence and the other is a whole message.

## Search highlighting never enters a tag

`chatHl` splits `body` on `/(<[^>]+>)/` and highlights only the text segments. `body` carries anchors (`href`, `onclick="previewUrl('…')"`) and `.tpl-var` chips with title text, so matching across the raw HTML injects `<mark>` into an attribute and breaks the element — searching `efteling` used to corrupt every link in the answer.

## `#Variable#` is a redacted user turn, not an internal value

CM logs a user's typed value as the variable name it was stored in (`#Voornaam#`, `#E-mailadres#`, `#Toelichting klacht#`, `#Vraag#`) whenever the field is PII. `isInternalValue` used to classify these as internal, which did two things:

1. dropped the user turn entirely, and
2. — the damaging one — stopped the `botRows` loop from breaking, so **every following bot row was absorbed into the previous turn**.

On the facility-card transactional dialog that produced a turn reading `User: "Ja"` followed by six consecutive bot bubbles, with the bot appearing to ask for a first name and then immediately a last name and no user input between them. It looked like message ordering was broken. 2,953 of 18,295 sessions in a real database were affected.

- `isInternalValue` covers only `continue`, `dialogId:nodeId`, and empty — genuine system values.
- `redactedUserLabel(v)` extracts the field name; `userBubbleBody(value, plan)` renders it as a lock chip (`.redacted-value`) instead of a text bubble, and is used by **both** chat render paths (`renderChatThread` and `renderFlaggedThread`) so they cannot drift.
- The **User** chat filter pill now includes these turns, which is correct — a user turn did happen.

## GenAI bubbles show no recognition data

`renderBubbleDetail` suppresses **recognition quality**, **entity matches**, and **dialogs** when the row is GenAI (`chatRowIsGenAi(row) || recognitionType === "GenerativeAI"`). A GenAI answer did not come from Conversational AI Cloud recognition, so those fields describe something else — rows with `all_interaction_types = ["QA","GenerativeAI"]` carry a populated `entityMatches`, `recognition_quality: 0.0`, and a `dialog_paths` of `{"DropOut": "…"}` (the dialog the user dropped *out of*), all of which read as an explanation of the answer. **GenAI source articles** (`faqs_found`) and **Recognition type** stay — those are genuinely about the GenAI answer. When nothing remains the empty state says so explicitly.

The low/zero-recognition bubble highlighting already excluded GenAI rows the same way; keep the two checks in agreement.
