// The mini-notation parser (pattern-model.md "mini-notation"): turn a string like
// "bd(3,8), ~ hh ~ hh" into a Pattern via the core constructors and combinators. The
// pipeline is tokenize → parse to AST → compile. The parser is purely front-end: it adds
// no semantics, only maps syntax onto the combinator vocabulary.

import { Pat, pure, silence } from "./pattern";
import { fastcat, slowcat, stack, fast, slow, euclid, degrade, degradeBy, type NumArg } from "./combinators";

export class MiniError extends Error {
  constructor(msg: string, src: string, pos: number) {
    super(`${msg}\n  ${src}\n  ${" ".repeat(Math.max(0, pos))}^`);
    this.name = "MiniError";
  }
}

// ── Tokens ─────────────────────────────────────────────────────────────────────

type Struct = "~" | "*" | "/" | "!" | "?" | "[" | "]" | "<" | ">" | "(" | ")" | ",";
type Token =
  | { t: "word"; v: string; pos: number }
  | { t: Struct; pos: number };

const WORD_CHAR = /[A-Za-z0-9.#-]/;
const STRUCT = "~*/!?[]<>(),";

function tokenize(src: string): Token[] {
  const toks: Token[] = [];
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (/\s/.test(c)) {
      i++;
    } else if (WORD_CHAR.test(c)) {
      let j = i;
      while (j < src.length && WORD_CHAR.test(src[j])) j++;
      toks.push({ t: "word", v: src.slice(i, j), pos: i });
      i = j;
    } else if (STRUCT.includes(c)) {
      toks.push({ t: c as Struct, pos: i });
      i++;
    } else {
      throw new MiniError(`unexpected character '${c}'`, src, i);
    }
  }
  return toks;
}

// ── AST ──────────────────────────────────────────────────────────────────────

type Ast =
  | { k: "atom"; v: string }
  | { k: "rest" }
  | { k: "seq"; steps: Ast[] }
  | { k: "stack"; layers: Ast[] }
  | { k: "alt"; options: Ast[] }
  | { k: "fast"; inner: Ast; n: Ast }
  | { k: "slow"; inner: Ast; n: Ast }
  | { k: "replicate"; inner: Ast; count: number }
  | { k: "degrade"; inner: Ast; prob?: number }
  | { k: "euclid"; inner: Ast; pulses: Ast; steps: Ast; rotation?: Ast };

const SEQ_STOP = new Set<string>([",", "]", ">", ")"]);

class Parser {
  private i = 0;
  constructor(
    private toks: Token[],
    private src: string,
  ) {}

  private peek(): Token | undefined {
    return this.toks[this.i];
  }
  private next(): Token | undefined {
    return this.toks[this.i++];
  }
  private expect(t: Struct): Token {
    const tok = this.peek();
    if (!tok || tok.t !== t) {
      throw new MiniError(`expected '${t}'`, this.src, tok ? tok.pos : this.src.length);
    }
    return this.next()!;
  }

  /** top = stack */
  parse(): Ast {
    const ast = this.parseStack();
    if (this.peek()) {
      throw new MiniError(`unexpected '${tokenText(this.peek()!)}'`, this.src, this.peek()!.pos);
    }
    return ast;
  }

  /** stack = sequence ( "," sequence )* */
  private parseStack(): Ast {
    const layers: Ast[] = [this.parseSequence()];
    while (this.peek()?.t === ",") {
      this.next();
      layers.push(this.parseSequence());
    }
    return layers.length === 1 ? layers[0] : { k: "stack", layers };
  }

  /** sequence = step* ; a `!n` replicate expands into n sibling steps here. */
  private parseSequence(): Ast {
    const steps: Ast[] = [];
    while (this.peek() && !SEQ_STOP.has(this.peek()!.t)) {
      const step = this.parseStep();
      if (step.k === "replicate") {
        for (let r = 0; r < step.count; r++) steps.push(step.inner);
      } else {
        steps.push(step);
      }
    }
    return steps.length === 1 ? steps[0] : { k: "seq", steps };
  }

  /** step = primary postfix* */
  private parseStep(): Ast {
    let ast = this.parsePrimary();
    for (;;) {
      const t = this.peek()?.t;
      if (t === "*" || t === "/" || t === "!" || t === "?" || t === "(") {
        ast = this.parsePostfix(ast);
      } else {
        break;
      }
    }
    return ast;
  }

  private parsePrimary(): Ast {
    const tok = this.peek();
    if (!tok) throw new MiniError("unexpected end of input", this.src, this.src.length);
    switch (tok.t) {
      case "word":
        this.next();
        return { k: "atom", v: tok.v };
      case "~":
        this.next();
        return { k: "rest" };
      case "[": {
        this.next();
        const inner = this.parseStack();
        this.expect("]");
        return inner;
      }
      case "<": {
        this.next();
        const inner = this.parseStack();
        this.expect(">");
        return { k: "alt", options: altOptions(inner) };
      }
      default:
        throw new MiniError(`unexpected '${tokenText(tok)}'`, this.src, tok.pos);
    }
  }

  private parsePostfix(base: Ast): Ast {
    const tok = this.next()!;
    switch (tok.t) {
      case "*":
        return { k: "fast", inner: base, n: this.parseFactor() };
      case "/":
        return { k: "slow", inner: base, n: this.parseFactor() };
      case "!": {
        const w = this.expectWord();
        const count = parseInt(w.v, 10);
        if (!Number.isInteger(count) || count < 1) {
          throw new MiniError("'!' needs a positive integer", this.src, w.pos);
        }
        return { k: "replicate", inner: base, count };
      }
      case "?": {
        const nxt = this.peek();
        if (nxt?.t === "word" && /^\d*\.?\d+$/.test(nxt.v)) {
          this.next();
          return { k: "degrade", inner: base, prob: parseFloat(nxt.v) };
        }
        return { k: "degrade", inner: base };
      }
      case "(": {
        const pulses = this.parseFactor();
        this.expect(",");
        const steps = this.parseFactor();
        let rotation: Ast | undefined;
        if (this.peek()?.t === ",") {
          this.next();
          rotation = this.parseFactor();
        }
        this.expect(")");
        return { k: "euclid", inner: base, pulses, steps, rotation };
      }
      default:
        throw new MiniError(`unexpected '${tokenText(tok)}'`, this.src, tok.pos);
    }
  }

  /** factor = WORD | group | alt */
  private parseFactor(): Ast {
    const tok = this.peek();
    if (tok?.t === "[" || tok?.t === "<") return this.parsePrimary();
    return { k: "atom", v: this.expectWord().v };
  }

  private expectWord(): Extract<Token, { t: "word" }> {
    const tok = this.peek();
    if (!tok || tok.t !== "word") {
      throw new MiniError("expected a value", this.src, tok ? tok.pos : this.src.length);
    }
    this.next();
    return tok;
  }
}

function altOptions(inner: Ast): Ast[] {
  if (inner.k === "seq") return inner.steps;
  if (inner.k === "stack") return inner.layers;
  return [inner];
}

function tokenText(tok: Token): string {
  return tok.t === "word" ? tok.v : tok.t;
}

// ── Compile: AST → Pattern ───────────────────────────────────────────────────

function asNumPat(ast: Ast): NumArg {
  if (ast.k === "atom" && /^-?\d*\.?\d+$/.test(ast.v)) return parseFloat(ast.v);
  return compile(ast).fmap((s) => parseFloat(s));
}

function intOf(ast: Ast): number {
  if (ast.k === "atom" && /^-?\d+$/.test(ast.v)) return parseInt(ast.v, 10);
  throw new Error("Euclid arguments must be integers");
}

function compile(ast: Ast): Pat<string> {
  switch (ast.k) {
    case "atom":
      return pure(ast.v);
    case "rest":
      return silence;
    case "seq":
      return ast.steps.length === 1 ? compile(ast.steps[0]) : fastcat(...ast.steps.map(compile));
    case "stack":
      return stack(...ast.layers.map(compile));
    case "alt":
      return slowcat(...ast.options.map(compile));
    case "fast":
      return fast(asNumPat(ast.n), compile(ast.inner));
    case "slow":
      return slow(asNumPat(ast.n), compile(ast.inner));
    case "replicate":
      return fastcat(...Array.from({ length: ast.count }, () => compile(ast.inner)));
    case "degrade":
      return ast.prob !== undefined ? degradeBy(ast.prob, compile(ast.inner)) : degrade(compile(ast.inner));
    case "euclid":
      return euclid(intOf(ast.pulses), intOf(ast.steps), ast.rotation ? intOf(ast.rotation) : 0, compile(ast.inner));
  }
}

/** Parse a mini-notation string into a `Pat<string>` of raw tokens. The caller (sound/
 *  note/freq) decides interpretation. */
export function parseMini(src: string): Pat<string> {
  const ast = new Parser(tokenize(src), src).parse();
  return compile(ast);
}
