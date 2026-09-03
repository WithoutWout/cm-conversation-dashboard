# Loading states and motion

Why every loading affordance is gated, and the entrance/exit motion rules that go with them.

_Split out of `CLAUDE.md`. Read this before changing anything it covers._

---

**A blank pane is not a neutral state — it looks exactly like a finished, empty one.** On a small database the reads below flash by; on a large one they are real seconds, and every one of them used to show nothing at all. Each now paints a spinner *before* it awaits and marks the controls that act on the not-yet-loaded thing inert.

**Every indicator is gated behind `gateLoading`, and the flicker it prevents is the reason all of them exist in the first place.** Nearly every read here finishes in tens of milliseconds — a folder already cached, a small database, a worker search over an indexed export — and a spinner that appears and vanishes inside that window is not feedback. The eye registers that *something* happened without ever resolving what, which reads as the interface glitching rather than working. So an indicator is asked for the moment work starts and only shown if that work is still running `LOADING_SHOW_DELAY_MS` (500ms) later; anything faster never paints one.

- **`LOADING_MIN_VISIBLE_MS` (300ms) is the other half, and without it the problem only moves.** Work finishing a little past the delay would put a spinner up and take it straight back down — the same flicker, rarer and harder to reproduce. It costs a short wait on results landing inside that window, which is the price of never flickering.
- **`gateLoading(key, wanted, apply)` calls `apply` only when the answer actually changes**, so a caller may ask for the same state repeatedly at no cost, and the overwhelmingly common path — fast work — never touches the DOM at all.
- **The constants live at the top of the script beside `yieldToPaint`**, for the same reason it does: the bootstrap `loadData()` runs during script evaluation and reaches `setAppLoadingState`, so a `const` further down would still be in its temporal dead zone.
- **`fadeIndicator` exists because all four indicators are `display: none` when hidden**, so there is nothing for a transition to animate — the element leaves the layout on the same frame. The exit goes through `.leaving`, the idiom the toasts and the metadata bubble already use: keep it displayed, run `fade-out`, drop both classes on a timer. `LOADING_FADE_MS` must stay in step with those rules, and they are declared *after* the entrance rules so they win on equal specificity while both classes are briefly present.
- **The two inline indicators restate `spin` alongside the fade** (`animation: spin …, fade-in …`). A second `animation` declaration replaces rather than adds, so omitting it freezes the spinner mid-fade — and under `prefers-reduced-motion` the rotation is kept while only the fade is dropped, because the rotation is what says "working".
- **`setPaneLoading` is the one that needs pairing.** It replaces its container's contents and the caller writes the result into the same element, so it has no minimum visible time to enforce — but every call **must** be matched with `clearPaneLoading`, or a spinner scheduled while the read was still running lands on top of the finished result. Verified to actually happen, not merely feared.
- **`setBusy` is deliberately *not* gated.** `.is-busy` is an input interlock (`pointer-events: none`), not an indicator; delaying it by half a second would leave controls live over data that hasn't loaded. Its existing `transition: opacity` already softens the visual half.
- `frontend/tests/loading-gate.test.js` runs the real `gateLoading` against a virtual clock. The interesting cases are all unreachable by hand — work landing a millisecond either side of the threshold, a burst of fast reads, a second read starting before the first one's spinner was due — and "it looked fine" cannot tell a correctly suppressed spinner from one suppressed for the wrong reason.

- `paneLoadingHtml(label, note)` / `setPaneLoading(id, …)` render the shared `.pane-loading` block. The `note` line says *why* something is slow ("Counting interactions per day across the database"), which is the difference between waiting and wondering.
- `setBusy(ids, on)` toggles `.is-busy` — `opacity` plus `pointer-events: none`. A class rather than `disabled` so it covers non-form elements (the chat filter row, the calendar) with one rule. A control that looks live but does nothing reads as a bug.
- `setConvOverlay(visible, label, cancellable)` owns the sessions-pane overlay. It existed for search; opening a database reuses it with its own sentence and **no Cancel button**, because there is nothing safe to cancel halfway through a migration. Every caller goes through this function, so the label can't be left reading the previous operation's.

Where they are wired, and why each one mattered:

- **`selectSession`** — the thread kept showing the *previous* conversation until the interactions arrived, with the new card already highlighted. It now clears to a spinner first, and a second click while the first is in flight can't paint over the newer selection (`activeSessionUuid !== uuid` guard, with the busy flags released in a `finally` that covers that early return).
- **`openDbAtPath`** — the longest call in the app and the one that said nothing: `set_db_path` applies schema migrations, repairs the FTS index if stale, and runs the one-time entity and metadata backfills.
- **The data modal's Import and Stored data tabs** — both open on aggregate queries. The import calendar in particular would have rendered as "nothing imported", which is a *wrong* answer rather than a missing one.

**`yieldToPaint()` is a race, and the timeout half is the point.** WebKit will not repaint between a class change and a Tauri `invoke` that occupies the IPC channel, so the loading state needs two frames before the call goes out. But `requestAnimationFrame` **does not fire in a window that isn't being composited** — minimised, occluded, or on another space — so awaiting it bare parks the caller forever behind a spinner that never resolves. On screen the frames win and the paint happens; off screen the timer wins after `PAINT_YIELD_MS` and the work proceeds without one, which is correct: nobody is watching. `loadSessions` awaited the bare frames before this, so a minimised window could hang it indefinitely.

- **There is one such helper, and awaiting a bare frame anywhere is the bug.** `waitForNextPaint` was a second, unbounded copy — a plain double `requestAnimationFrame` — and it survived the `loadSessions` fix untouched, so the hang simply moved: `loadData` awaited it before `getData`, and a launch or refresh with the window minimised, occluded or on another space parked it with `dataLoadInFlight` left `true`. That flag is the early-return guard, so every later `loadData()` call silently did nothing and the refresh button was dead until restart. It is gone; its two call sites go through `yieldToPaint`.
- **`yieldToPaint` is declared at the top of the script, beside `setAppLoadingState`, and not down here with the rest of the loading states.** The bootstrap `loadData()` at the end of the script runs during evaluation and awaits it *before* execution reaches this section, so a `const` declared here would still be in its temporal dead zone — swapping the call sites without moving the definition trades the hang for a `ReferenceError` that takes out startup entirely.
- Every other `requestAnimationFrame` in the file schedules a callback and is never awaited (chunked chat rendering, the scroll-into-view calls, the content-preview reposition). A window that is not being composited defers those until it is, which is correct; only an awaited frame can turn into a hang.

**`animateModalResize(box, mutate)`** grows a modal between two heights instead of snapping. The data modal's three tabs are a two-month calendar, a spinner and a column of settings — very different heights — so switching jerked the whole dialog under the pointer and took the tab bar with it.

- Everything between the two measurements is synchronous, so the intermediate layout is never painted. `height: auto` before measuring the target is what makes the box report its *natural* height when a previous animation still has an explicit one set; `max-height: 88vh` still applies, so an over-tall tab measures already clamped.
- **The explicit height is released on a timer, not `transitionend`.** A tab switched again mid-animation cancels the event, and a box left pinned would clip everything taller. `MODAL_RESIZE_MS` must stay in step with the `.modal-box.resizing` transition.
- Each tab's prepare paints its loading state *synchronously* before awaiting, so it is part of the height being animated to rather than a second jump straight after; the later content render wraps its own `_cdataResize`.
- `prefers-reduced-motion` skips the animation **and clears any height left pinned by an earlier one**, so the escape hatch can't strand the box.
- The Settings modal uses the same helper. Its tab handler is scoped to `#settingsModal` — the data modal's panels carry the same `.settings-tab-panel` class, and the unscoped query was clearing their active state.
- **The Import tab's own source tabs (Analytics API / CSV file) animate too**, and they are a separate mechanism: they re-render `#convImportBody` directly rather than toggling a panel class, so they never passed through `_cdataResize`. It is the biggest height change in the modal — a full calendar and date pickers against two paragraphs.
  - `_impSetupHtml` emits the tab strip and then `_impSourcePanelHtml()` inside a `.import-source-panel` wrapper. The split exists so the fade lands on the panel and **not** on the tab strip: the control you just clicked should stay put.
  - **`panel-in` is added by `impSetSource` after the render, never baked into the markup.** `_impRenderModal` also runs on every time-input change, every calendar click and the skip checkbox; a class in the HTML would replay the animation on all of them. No cleanup is needed because the next render replaces the element outright.
  - Clicking the already-active source returns early — it used to re-render for nothing.

## Entrance motion

Everything under `/* Shared entrance motion */` animates an **arrival** — something that previously appeared between one frame and the next. The rule is that none of it may gate a click, a render or a request: the element is laid out and interactive on the first frame and the animation only softens how it lands. Anything that would make the user wait for motion to finish does not belong here.

Three keyframes carry all of it, alongside the existing `tab-panel-in`:

| | Used for |
| --- | --- |
| `fade-in` | main tabs, the Content/Conversations/Flagged switch, loading affordances, the copy confirmation |
| `pop-in` | the tag filter and Add to Collection popovers, and the message-metadata bubble — scale + rise, so a menu grows out of the control that opened it |
| `pop-out` | the exit `pop-in` never had, a touch quicker: an arrival is worth watching, a dismissal is not |
| `toast-out` | the exit that `toast-in` never had |

- **The big containers get opacity only, never a transform.** A `.panel` can hold thousands of nodes, and a transform would additionally make it the containing block for anything `position: fixed` inside it. `pop-in` is fine on a popover because both `.ctx-modal-box` call sites compute their position from the *button's* rect and write it inline **before** the box is shown, so the transform has nothing to disturb.
- **The animations restart because the elements are `display: none` in between**, not because anything re-adds a class. That is what keeps a search, a sort or a pagination click from replaying the panel fade — `.active` never leaves. Verified by `getAnimations()`: no animation on the hidden element, a fresh `running` one at `currentTime: 0` the moment it is shown.
- **An expanded card body and the chips behind a "+N more" fade, but the box still snaps to its full height.** Animating the height would mean measuring content that is only sized once revealed, and the page would visibly settle afterwards. `.chip-overflow` is `display: contents`, so the animation has to go on `> *` — the parent has no box to animate.
- **A toast only fades out when nothing is replacing it.** Every toast occupies the same corner, so animating one out while a successor appears leaves two messages stacked. `_dismissToast` removes the toast outright when another is already in the DOM, and `showImportToast` keeps removing its predecessor instantly; the fade is for the last toast on screen — the 6-second auto-dismiss and `clearProgressToast` when it stands alone. `TOAST_OUT_MS` must stay in step with `.import-toast.leaving`.
- **`_showCopyFeedback` restarts its fade explicitly** (`classList.remove` → forced reflow → `add`). Copying the same thing twice in a row otherwise confirms itself silently, because the text does not change and a running animation does not replay.
- **A fade on a loading affordance is not a delay.** `.pane-loading` and both overlays are in the DOM on exactly the frame they always were; the 140 ms only takes the flash off a fast read.
- `prefers-reduced-motion` disables every one of these in the single media query at the end of the block — including `toast-in`, which predates it. `.import-toast.leaving` gets `opacity: 0` rather than nothing, because the timer that removes it does not know about the media query.

## No scrollbar may arrive mid-animation

A scrollbar that appears and disappears during an animation is worse than no animation: where scrollbars take real width — Windows, and macOS set to always show them — every element inside the container reflows narrower and then jumps back, so the motion reads as a glitch. Two rules keep that from happening, and both are invisible on a Mac with overlay scrollbars, which is why this is easy to ship broken.

- **A box animating its height clips the body inside it for the duration.** `.modal-box.resizing`'s own `overflow: hidden` covers the *box*; the element that actually scrolls is `.modal-body`, and for the length of the animation the box is deliberately shorter than its content — so the body became scrollable and then wasn't. `.modal-box.resizing .modal-body` clips instead; the content is growing into place anyway. The rule is duplicated under `#settingsModal` purely to outrank `#settingsModal .modal-body`'s ID specificity — check that with `getComputedStyle`, not by eye, if you touch either.
- **Containers that cross the scrollable threshold reserve the gutter** (`scrollbar-gutter: stable`, as `.chat-thread` already did): both height-animated modal bodies, `.list-wrap` (one per content tab — whether it scrolls depends on the result count, so switching tabs or just searching shifted every card sideways), `.sessions-list` (spinner ↔ a full page of results), and `.ctx-modal-body` (the chip list is fetched *after* the popover opens, so the scrollbar used to land mid-pop-in).
- Transforms are exempt by construction — they do not affect layout, so `pop-in`, `toast-out` and `tab-panel-in` cannot produce a scrollbar. Only the height animation and genuine content swaps can.
