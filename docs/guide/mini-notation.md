[« strobe user guide](../guide.md)

# Mini-notation

The string passed to `sound`/`note`/`freq` is **mini-notation** — a tiny rhythm DSL borrowed
from TidalCycles. The core rule: **a sequence divides its slot of time evenly between its
steps, and every bracket nests another sequence inside one step.** That single rule, applied
recursively, gives you arbitrarily intricate rhythm from short strings.

```text
"bd sn hh cp"     →  4 steps, each 1/4 of the cycle
"bd [sn sn] hh"   →  3 steps; the middle 1/3 is split into two 1/6 hits
"bd*4"            →  one step that repeats 4× inside its slot (here, the whole cycle)
```

| Syntax | Meaning | Example |
| --- | --- | --- |
| `a b c` | sequence — split the cycle evenly between the steps | `"bd sn hh"` |
| `~` | a rest (silence) — holds its slot of time | `"bd ~ sn ~"` |
| `a*n` | speed a step up `n`× *within its slot* (repeats) | `"bd*2 sn"` |
| `a/n` | slow a step down — play it once every `n` cycles | `"bd/2"` |
| `[a b]` | group — subdivide one slot into its own sequence | `"bd [sn sn]"` |
| `<a b>` | alternate — pick one option per cycle, cycling round | `"bd <sn cp>"` |
| `a, b` | stack — layer simultaneously (inside `[]`/`<>` or at top level) | `"[bd*4, hh*8]"` |
| `a(p,s)` | Euclidean rhythm — `p` pulses spread as evenly as possible over `s` steps | `"bd(3,8)"` |
| `a(p,s,r)` | …rotated by `r` steps | `"bd(3,8,2)"` |
| `a!n` | replicate — `n` separate copies as sibling steps (lengthens the sequence) | `"bd!3 sn"` |
| `a?` / `a?p` | randomly drop (degrade) each hit, probability `0.5` (or `p`) | `"hh*8?"` |

**`*` vs `!` vs `[]` — a common confusion.** `bd*3` keeps *one* step slot and crams 3 hits
into it; `bd!3` makes *three* step slots (so `"bd!3 sn"` is a 4-step sequence); `[bd bd bd]`
is the same as `bd*3`. Use `*` for rolls/ratchets inside a step, `!` to lengthen the bar.

**Arguments can themselves be patterns.** Anywhere a number goes, you can put a `<…>`
alternation: `"bd*<2 3>"` plays 2 hits one cycle, 3 the next; `"bd(<3 5>,8)"` alternates the
Euclidean density. This is the same "everything is a pattern" idea reaching into the notation.

**Euclidean rhythms** (`(p,s)`) are worth knowing: they distribute `p` hits over `s` slots as
evenly as possible — the backbone of a huge amount of world and electronic music. `(3,8)` is
the tresillo `x..x..x.`; `(5,8)` is the cinquillo; `(2,5)`, `(7,16)` etc. all sound musical.

---

Prev: [« The mental model](mental-model.md) · Next: [sound / note / freq »](sound-note-freq.md)
