// Syntax highlighting off the main thread: parses documents with CodeMirror's Lezer parsers
// (loaded per language on first use) and sends back each line's styled ranges.
import { LanguageDescription, type Language } from "@codemirror/language";
import { highlightTree } from "@lezer/highlight";
import type { HighlightRequest, HighlightResponse } from "./highlight";
import { highlighter, known } from "./languages";

/** Past this, a document isn't worth parsing (and the diff or file is capped well below it anyway). */
const MAX_CHARS = 5_000_000;

const loaded = new Map<string, Promise<Language | undefined>>();

function languageFor(path: string): { name: string; language: Promise<Language | undefined> } | undefined {
  const name = path.slice(path.lastIndexOf("/") + 1);
  const desc = LanguageDescription.matchFilename(known, name);
  if (!desc) return undefined;
  let language = loaded.get(desc.name);
  if (!language) {
    language = desc.load().then(
      (s) => s.language,
      () => undefined,
    );
    loaded.set(desc.name, language);
  }
  return { name: desc.name, language };
}

/** `[line, from, to, class]` per styled range, `from`/`to` relative to the line's start. */
function highlightDoc(language: Language, text: string, classes: Map<string, number>): Uint32Array {
  const out: number[] = [];
  if (text.length > MAX_CHARS) return new Uint32Array();
  const tree = language.parser.parse(text);
  let line = 0;
  let lineStart = 0;
  let nextBreak = text.indexOf("\n");
  highlightTree(tree, highlighter, (from, to, cls) => {
    let c = classes.get(cls);
    if (c === undefined) classes.set(cls, (c = classes.size));
    // A range can span lines (block comments, template strings): split it at each line break.
    while (from < to) {
      while (nextBreak !== -1 && nextBreak < from) {
        line++;
        lineStart = nextBreak + 1;
        nextBreak = text.indexOf("\n", lineStart);
      }
      const end = nextBreak === -1 ? to : Math.min(to, nextBreak);
      if (end > from) out.push(line, from - lineStart, end - lineStart, c);
      from = end === nextBreak ? end + 1 : end;
    }
  });
  return Uint32Array.from(out);
}

self.onmessage = async ({ data }: MessageEvent<HighlightRequest>) => {
  const reply = (r: HighlightResponse, transfer: Transferable[] = []) => self.postMessage(r, { transfer });
  const lang = languageFor(data.path);
  const language = await lang?.language;
  if (!lang || !language) return reply({ id: data.id });
  const classes = new Map<string, number>();
  try {
    const docs = data.docs.map((d) => highlightDoc(language, d, classes));
    reply({ id: data.id, language: lang.name, classes: [...classes.keys()], docs }, docs.map((d) => d.buffer));
  } catch (e) {
    console.error(`highlighting ${data.path}:`, e);
    reply({ id: data.id, language: lang.name });
  }
};
