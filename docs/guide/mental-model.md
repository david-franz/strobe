[« strobe user guide](../guide.md)

# The mental model

Everything in strobe is built from one idea: **a pattern is a pure function from a span of
time to the events that happen in it.** You don't build a list of notes; you describe a
function, and the engine *queries* it for whatever slice of time it needs to play next.

Three consequences follow, and they explain why the rest of the API looks the way it does:

- **Patterns are infinite and lazy.** `sound("bd*4")` isn't four events — it's a recipe that,
  asked for cycle 9, returns cycle 9's four kicks. Nothing is precomputed, so a pattern can
  loop forever and combinators can transform *time itself* (play it twice as fast, reversed,
  every third cycle) just by composing functions.
- **They compose freely.** Because every combinator is `pattern → pattern`, you can stack
  transforms in any order and they always produce another valid pattern. There are no
  "invalid states" to guard against.
- **Hot-swapping is seamless.** Reassigning a track replaces one pure function with another.
  The engine simply queries the new function from the next cycle boundary on — no notes are
  "in flight" to clean up, so there's no click or dropout.

## The vocabulary

| Term | What it is |
| --- | --- |
| **Cycle** | The unit of musical time. A pattern is defined over one cycle (`0 → 1`) and repeats. Think of it as one bar/loop. |
| **cps** | **Cycles per second** — the tempo. `setbpm(130)` is just `setcps(130/60/4)`. |
| **Hap** | One event the pattern emits: a value plus *when* it happens (`whole` = its full timespan, `part` = the slice you queried). An onset is a hap whose `whole` starts inside the queried span. |
| **Pattern** | The pure time→haps function. `Pat<T>` is the chainable wrapper you actually use. |
| **Control map** | The value most patterns carry: `{ s, note, gain, lpf, … }` — *what to play and how it sounds.* |
| **Track** | A named slot (`p1`…`p8`) holding one pattern. Re-running `pN(...)` swaps it live. |

## Tempo

| Function | Does |
| --- | --- |
| `setcps(x)` | set tempo directly, in cycles per second |
| `setbpm(n)` | set tempo in BPM (assumes 4 beats per cycle: `cps = bpm/60/4`) |

Tempo is global and applies to every track. Changing it live re-anchors the clock smoothly
(the cycle you're in keeps its phase), so you can `setcps` from the REPL without a glitch.

---

Next: [Mini-notation »](mini-notation.md)
