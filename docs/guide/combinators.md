[« strobe user guide](../guide.md)

# Combinators

Pure `pattern → pattern` transforms — the verbs of the language. They *reshape time and
structure* without you ever touching individual events.

Most are available **both ways**: as a standalone function `fast(2, p)` and as a chainable
method `p.fast(2)`. In a song the **method form reads best** (`p.fast(2).rev()`). Two notes
on naming: **`every` is method-only** — call `p.every(4, rev)`, not `every(...)`, because the
bare `every` name is the Phase-1 builder; and **Euclidean rhythm is reached through
mini-notation** `"bd(3,8)"` rather than a combinator.

The `fn` argument some combinators take is itself a `pattern → pattern` function — usually
another combinator or a small arrow: `every(4, x => x.fast(2).rev(), p)`.

## Time

| Combinator | Effect | Reach for it when… |
| --- | --- | --- |
| `fast(n, p)` / `slow(n, p)` | compress / stretch in time | you want a part to run at a different rate from the cycle |
| `rev(p)` | reverse within each cycle (still loops) | a fill, or the right side of a `jux` |
| `hurry(n, p)` | `fast(n)` **and** raise speed/pitch by `n` | sped-up *and* pitched-up stabs (sampler-flavored) |
| `early(t, p)` / `late(t, p)` | nudge earlier / later by `t` cycles | push/pull feel, swing, pre-delay |
| `off(t, fn, p)` | overlay a copy shifted by `t`, transformed by `fn` | echoes, octave stabs, call-and-response |

`off` is the workhorse for thickening: `off(0.125, x => x.gain(0.5), p)` adds a quieter
eighth-note-late echo. `early`/`late` take *fractions of a cycle*, so `late(1/16, p)` is a
sixteenth.

## Structure & repetition

| Combinator | Effect |
| --- | --- |
| `every(n, fn, p)` | apply `fn` only on every `n`th cycle (cycle 0, n, 2n, …) |
| `whenmod(a, b, fn, p)` | apply `fn` whenever `cycle % a >= b` (e.g. "last 2 of every 8") |
| `iter(n, p)` | rotate the start point by `1/n` each cycle (a cycling phase shift) |
| `chunk(n, fn, p)` | apply `fn` to a different `1/n` slice of the cycle each cycle — a rotating fill |

`every` is how you get variation over time without writing it out: `p.every(4, rev)` keeps
things fresh every fourth bar. `chunk(4, x => x.fast(2), p)` "walks" a ratchet across the bar,
one quarter at a time — great for evolving hi-hats and breaks.

## Layering

| Combinator | Effect |
| --- | --- |
| `stack(...ps)` | play several patterns at once (a chord, or a full kit on one track) |
| `cat(...ps)` / `slowcat(...ps)` | one whole pattern per cycle, in turn |
| `fastcat(...ps)` | squeeze all the patterns into a single cycle (sequence builder) |
| `superimpose(fn, p)` | overlay `p` with an `fn`-transformed copy of itself |
| `jux(fn, p)` | split to stereo: original in the left ear, `fn`-transformed in the right |

`stack` layers; `cat` sequences across cycles; `fastcat` sequences within one cycle.
`superimpose(x => x.add?…)`-style overlays and `jux(rev)` (reversed in one ear) are instant
width. Note `jux` lives on `sound`/`note` patterns and pans the two copies hard L/R.

## Chance (deterministic)

| Combinator | Effect |
| --- | --- |
| `degrade(p)` / `degradeBy(prob, p)` | randomly drop hits (`0.5` / `prob`) |
| `sometimes(fn, p)` / `sometimesBy(prob, fn, p)` | apply `fn` to a random subset of hits |
| `somecycles(fn, p)` | apply `fn` on random whole cycles |

**Randomness here is deterministic** — a hash of cycle number + position, not a live RNG. So
a `?`/`degrade` pattern is *reproducible* (the same bar always plays the same), yet it
*realises differently at different cycle numbers*. That's the point: you get evolving,
non-repeating movement that is still exactly the same every time you run the song.
`sometimesBy(0.3, x => x.fast(2), p)` ratchets ~30% of hits; `degrade` thins a busy hat.

```ts
sound("bd*4").every(4, rev).off(0.125, x => x.gain(0.5)).jux(rev)
```

---

Prev: [« sound / note / freq](sound-note-freq.md) · Next: [Signals »](signals.md)
