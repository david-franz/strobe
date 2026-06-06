# Phase 2.02 — Mini-notation parser (`mini.ts`)

## Purpose

`mini.ts` turns a mini-notation **string** — the tiny DSL from [pattern-model.md](../../pattern-model.md#mini-notation) — into a `Pattern` built from the core constructors of [01-pattern-core.md](01-pattern-core.md). It is what makes `sound("bd(3,8), ~ hh ~ hh")` work: the string is the *what and when*, parsed once when the song line runs, into a pure pattern the scheduler can query forever.

It implements the **full** table from the model, nothing added, nothing dropped:

| Syntax | Meaning | Builds |
| --- | --- | --- |
| `a b c` | sequence — split the cycle evenly | `fastcat` |
| `~` | rest (silence) | `silence` |
| `a*2` | repeat/speed up within slot | `fast` on that step |
| `a/2` | slow down (every other cycle) | `slow` on that step |
| `[a b]` | subdivide — group into one slot | nested `fastcat` |
| `<a b>` | alternate — one per cycle, cycling | `slowcat` (cycle-indexed) |
| `a,b` | stack — layers at once | `stack` |
| `a(3,8)` | Euclidean rhythm | `euclid` |
| `a!3` | replicate | three steps |
| `a?` | randomly drop (degrade) | `degrade` |

`fast`/`slow`/`fastcat`/`slowcat`/`stack`/`euclid`/`degrade` are the [03-combinators.md](03-combinators.md) functions; the parser is purely *front-end*. It never defines new semantics — it maps syntax onto the combinator vocabulary.

## Location

```
packages/tekno/src/
  mini.ts          # tokenizer + parser + AST→Pattern compiler
  mini.test.ts     # queryArc tests per construct + error-reporting tests
```

`mini.ts` imports from `pattern.ts` ([01](01-pattern-core.md)) and `combinators.ts` ([03](03-combinators.md)). It is imported by `sound.ts`/`instrument.ts` ([04](04-sound-note-api.md)): `sound("...")`, `note("...")`, `freq("...")` call `parseMini` then map values.

## Design

The pipeline is the classic three stages: **tokenize → parse to AST → compile to `Pattern`**. Keeping the AST explicit (rather than building patterns directly during parse) makes error reporting precise and the construct-to-combinator mapping testable in isolation.

```ts
export function parseMini(src: string): Pat<string> {
  const tokens = tokenize(src);
  const ast = new Parser(tokens, src).parseSequence();   // top level is a sequence (with , = stack)
  return compile(ast);
}
```

`parseMini` always yields `Pat<string>` (raw tokens like `"bd"`, `"c3"`, `"0.6"`). The *caller* decides interpretation: `sound` keeps the string as `{ s }`, `note` parses it to a frequency, `freq` to a number ([04](04-sound-note-api.md)). The parser is value-agnostic.

### Tokens

```ts
type Token =
  | { t: "word"; v: string; pos: number }     // bd, c#3, 0.6, hh
  | { t: "~"; pos: number }
  | { t: "*"; pos: number } | { t: "/"; pos: number }
  | { t: "!"; pos: number } | { t: "?"; pos: number }
  | { t: "["; pos: number } | { t: "]"; pos: number }
  | { t: "<"; pos: number } | { t: ">"; pos: number }
  | { t: "("; pos: number } | { t: ")"; pos: number }
  | { t: ","; pos: number };
```

Tokenizer rules: whitespace separates words and is otherwise insignificant; a `word` is a maximal run of `[A-Za-z0-9.#-]` (covers `bd`, `c#3`, `eb2`, `0.6`, `-7` for negative semitone offsets); every structural character is its own token; `pos` (byte offset) is carried for error messages. Numbers inside `(n,k)` are just `word` tokens parsed as ints at AST time.

### AST

```ts
type Ast =
  | { k: "atom"; v: string; pos: number }
  | { k: "rest"; pos: number }
  | { k: "seq"; steps: Ast[] }                  // a b c   → fastcat
  | { k: "stack"; layers: Ast[] }               // a,b     → stack
  | { k: "group"; inner: Ast }                  // [ ... ] → one slot
  | { k: "alt"; options: Ast[] }                // < ... > → slowcat
  | { k: "fast"; inner: Ast; n: Ast }           // a*2     (n is itself an Ast → "a*<2 3>")
  | { k: "slow"; inner: Ast; n: Ast }           // a/2
  | { k: "replicate"; inner: Ast; count: number } // a!3
  | { k: "degrade"; inner: Ast }                // a?  (optional ?p handled as ?word)
  | { k: "euclid"; inner: Ast; pulses: Ast; steps: Ast; rotation?: Ast }; // a(3,8) / a(3,8,2)
```

Modifiers (`*`, `/`, `!`, `?`, `(…)`) are **postfix** and bind to the immediately preceding primary, and can chain: `bd*2!3`, `hh(3,8)?`. The grammar encodes that as a `primary` followed by a run of postfix operators.

### Grammar (EBNF-ish)

```
top        = stack ;
stack      = sequence ( "," sequence )* ;        // top-level and inside [ ] / < >
sequence   = step* ;                              // whitespace-separated
step       = primary postfix* ;
primary    = atom | rest | group | alt ;
atom       = WORD ;
rest       = "~" ;
group      = "[" stack "]" ;
alt        = "<" stack ">" ;                      // each comma-layer alternates together
postfix    = "*" factor
           | "/" factor
           | "!" INT
           | "?"  [ WORD ]                         // optional probability, e.g. ?0.3
           | "(" arg "," arg [ "," arg ] ")" ;     // Euclid (pulses,steps[,rotation])
factor     = WORD | group | alt ;                  // a*2, a*[2 3], a*<2 3>
arg        = WORD | group | alt ;                  // Euclid params may themselves be patterns
```

Notes:
- `sequence` containing a single step is unwrapped (so `[bd]` == `bd`).
- A `factor`/`arg` can be a pattern (`a*<2 3>` alternates the speed per cycle); we keep it as an `Ast` and compile to a *patterned parameter* (see Compile).
- `<a b c>` is sugar: it is `slowcat` of its options — one per cycle (whereas a bare `sequence` is `fastcat` — all in one cycle).

### Parser (recursive descent)

```ts
class Parser {
  constructor(private toks: Token[], private src: string) {}
  private i = 0;
  private peek() { return this.toks[this.i]; }
  private next() { return this.toks[this.i++]; }
  private expect(t: Token["t"]): Token { /* throw MiniError if mismatch */ }

  parseSequence(): Ast { /* top-level stack of sequences */ }
  private parseStack(stop: Token["t"][]): Ast { /* sequences split on "," */ }
  private parseStep(): Ast { /* primary then postfix* */ }
  private parsePrimary(): Ast { /* atom | rest | "[" stack "]" | "<" stack ">" */ }
  private parsePostfix(base: Ast): Ast { /* fold *, /, !, ?, (...) onto base */ }
}
```

### Compile: AST → `Pattern`

Each AST node maps to exactly one combinator from [03](03-combinators.md):

```ts
function compile(ast: Ast): Pat<string> {
  switch (ast.k) {
    case "atom":   return pure(ast.v);                       // [01]
    case "rest":   return silence;                           // [01]
    case "group":  return compile(ast.inner);
    case "seq":    return fastcat(...ast.steps.map(compile)); // squeeze all steps into one cycle
    case "stack":  return stack(...ast.layers.map(compile));
    case "alt":    return slowcat(...ast.options.map(compile)); // one option per cycle
    case "fast":   return fast(asNumPat(ast.n), compile(ast.inner));
    case "slow":   return slow(asNumPat(ast.n), compile(ast.inner));
    case "replicate": return fastcat(...Array(ast.count).fill(0).map(() => compile(ast.inner)));
    case "degrade":   return degrade(compile(ast.inner));
    case "euclid":    return euclid(intOf(ast.pulses), intOf(ast.steps),
                                    ast.rotation ? intOf(ast.rotation) : 0, compile(ast.inner));
  }
}
```

`asNumPat` compiles a factor that may be a pattern (`*<2 3>`) into a `Pat<number>`; `fast`/`slow`/`euclid` accept either a number or a number-pattern (the *patternified-argument* convention defined in [03](03-combinators.md)).

#### Why `fastcat` for sequences

A bare sequence `"a b c"` must place all three events inside **one** cycle, evenly. That is precisely `fastcat` (= "squeeze all patterns into one cycle"; [pattern-model.md layering table](../../pattern-model.md#layering)). Each child is itself a one-cycle pattern; `fastcat` of N children compresses child *i* into `[i/N, (i+1)/N)`. Nesting falls out for free: `"bd [sn sn]"` is `fastcat(pure("bd"), fastcat(pure("sn"), pure("sn")))`, so the second slot subdivides.

### Worked examples

**`"bd ~ sn ~"`** → `fastcat(pure("bd"), silence, pure("sn"), silence)`.

```
queryArc(parseMini("bd ~ sn ~"), 0, 1)
→ [ { whole:{0,0.25},  part:{0,0.25},  value:"bd" },
    { whole:{0.5,0.75},part:{0.5,0.75},value:"sn" } ]
```

(matches the canonical example in [pattern-model.md](../../pattern-model.md#testing-patterns)).

**`"bd*2 sn"`** → `fastcat(fast(2, pure("bd")), pure("sn"))`.

```
queryArc(_, 0, 1)
→ [ {whole:{0,0.25},   part:{0,0.25},   value:"bd"},
    {whole:{0.25,0.5}, part:{0.25,0.5}, value:"bd"},
    {whole:{0.5,1},    part:{0.5,1},    value:"sn"} ]
```

**`"bd <sn cp>"`** → `fastcat(pure("bd"), slowcat(pure("sn"), pure("cp")))`. The second slot alternates per cycle:

```
queryArc(_, 0, 1) → [ {.. "bd" 0..0.5}, {.. "sn" 0.5..1} ]
queryArc(_, 1, 2) → [ {.. "bd" 1..1.5}, {.. "cp" 1.5..2} ]
```

**`"bd(3,8)"`** → `euclid(3, 8, 0, pure("bd"))`. Euclid spreads 3 pulses over 8 even steps using Bjorklund: `[x . . x . . x .]`, i.e. onsets at steps 0,3,6:

```
queryArc(_, 0, 1)
→ haps at part begins 0, 3/8, 6/8 (each whole one step = 1/8 wide)
```

`euclid`'s Bjorklund generator lives in [03](03-combinators.md); the parser only forwards `(pulses, steps, rotation)`.

**`"[bd*4, hh*8]"`** → `stack(fast(4,pure("bd")), fast(8,pure("hh")))` — both layers in the same cycle.

### Error reporting

Parse errors throw a `MiniError` carrying the byte offset and a caret-underlined snippet, so a typo in a live song line gives a useful message instead of silent silence:

```ts
export class MiniError extends Error {
  constructor(msg: string, src: string, pos: number) {
    super(`${msg}\n  ${src}\n  ${" ".repeat(pos)}^`);
  }
}
```

Caught cases: unbalanced `[` `]` / `<` `>` / `(` `)`; `*`/`/`/`!`/`(` with no preceding primary or no operand; non-integer `!`/Euclid counts; empty groups `[]`; trailing/leading `,`. The live runtime ([phase-5]) will catch `MiniError` and keep the previous pattern playing rather than crash the session.

## Interfaces

- **Builds on [01-pattern-core.md](01-pattern-core.md):** `pure`, `silence`.
- **Builds on [03-combinators.md](03-combinators.md):** `fastcat`, `slowcat`, `stack`, `fast`, `slow`, `euclid`, `degrade`. The parser introduces **no** time math of its own; correctness of timing is the combinators' responsibility.
- **Consumed by [04-sound-note-api.md](04-sound-note-api.md):** `sound`/`note`/`freq` call `parseMini` and then `.fmap` the resulting `Pat<string>` to the right value type (string id, frequency number, control map).
- **Patternified args:** a `factor`/`arg` that is itself a pattern (`*<2 3>`, `(<3 5>,8)`) compiles to a `Pat<number>` and is handed to the combinator's patternified-argument path ([03](03-combinators.md)).

## Edge cases & risks

- **Purity:** the parser runs once per song-line evaluation, not per query. The produced pattern is pure; `?` (degrade) defers its randomness to query time via the cycle-seeded RNG of [03](03-combinators.md), so `parseMini` itself stays deterministic.
- **Fractional precision:** sequence slot boundaries (`i/N`) and Euclid step boundaries (`j/steps`) are computed with the exact-rational helper (`fraction.ts`, [01](01-pattern-core.md)) and converted to `number` only at `Hap` edges, so `"a b c"` onsets are `sameTime` to `0, 1/3, 2/3`.
- **Continuous patterns:** mini-notation produces only **discrete** patterns; there is no syntax for a continuous signal (those come from `sine`/`saw`/… in [05](05-continuous-signals.md)). A mini string can be *used as* a parameter pattern (`.gain("0.6 0.9")`), which is fine — it is discrete and sampled per note ([05](05-continuous-signals.md), [06](06-scheduler-integration.md)).
- **Replicate vs. fast:** `a!3` produces **three separate onsets** (three steps), whereas `a*3` produces three by *speeding one slot* — both end up three events in this case, but they compose differently with neighbors (`a!3 b` = 4 slots; `a*3 b` = 2 slots). The compiler keeps them distinct.
- **`?` probability:** bare `?` degrades at the default 0.5; `?0.3` parses the trailing word as a float probability and forwards it to `degradeBy`.
- **Performance:** parsing is O(length of string), done once; querying cost is entirely in the combinators. No risk on the control plane.

## Acceptance criteria

- `queryArc(parseMini("bd ~ sn ~"), 0, 1)` equals the canonical two-hap result (`bd` at `{0,0.25}`, `sn` at `{0.5,0.75}`).
- `queryArc(parseMini("bd*2 sn"), 0, 1)` yields onsets at `0, 0.25, 0.5` with values `bd, bd, sn`.
- `queryArc(parseMini("bd <sn cp>"), 0, 1)` and `(…, 1, 2)` show the second slot alternating `sn`→`cp` per cycle.
- `queryArc(parseMini("bd(3,8)"), 0, 1)` yields 3 onsets at part-begins `0, 3/8, 6/8` (Bjorklund verified).
- `queryArc(parseMini("[bd*4, hh*8]"), 0, 1)` yields 4 `bd` + 8 `hh` haps stacked in one cycle.
- `queryArc(parseMini("bd!3 sn"), 0, 1)` yields 4 evenly-spaced onsets `bd,bd,bd,sn`.
- `parseMini("[bd sn")` throws a `MiniError` whose message points at the unclosed `[`.
- `parseMini("*2")` (postfix with no primary) throws a `MiniError`.
- A round-trip purity check: `queryArc(parseMini("hh*8?"), 0, 1)` is deep-equal across two calls (deterministic degrade).
