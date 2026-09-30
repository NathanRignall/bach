// Syntax highlighting off the main thread: parses documents with CodeMirror's Lezer parsers
// (loaded per language on first use) and sends back each line's styled ranges.
import { LanguageDescription, type Language } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { highlightTree, tagHighlighter, tags as t } from "@lezer/highlight";
import type { HighlightRequest, HighlightResponse } from "./highlight";

/** Token classes, styled in index.css. Kept few, so the colours stay calm. */
const highlighter = tagHighlighter([
  { tag: [t.keyword, t.modifier, t.controlKeyword, t.operatorKeyword, t.definitionKeyword, t.moduleKeyword, t.self], class: "tok-keyword" },
  { tag: [t.string, t.special(t.string), t.character, t.docString], class: "tok-string" },
  { tag: [t.regexp, t.escape], class: "tok-escape" },
  { tag: t.comment, class: "tok-comment" },
  { tag: [t.number, t.bool, t.null, t.atom, t.unit, t.constant(t.variableName), t.standard(t.variableName)], class: "tok-constant" },
  { tag: [t.typeName, t.className, t.namespace, t.standard(t.typeName)], class: "tok-type" },
  { tag: [t.function(t.variableName), t.function(t.propertyName), t.macroName], class: "tok-function" },
  { tag: [t.propertyName, t.attributeName, t.labelName], class: "tok-property" },
  { tag: [t.tagName, t.angleBracket], class: "tok-tag" },
  { tag: [t.meta, t.annotation, t.processingInstruction, t.documentMeta], class: "tok-meta" },
  { tag: t.heading, class: "tok-heading" },
  { tag: t.emphasis, class: "tok-emphasis" },
  { tag: t.strong, class: "tok-strong" },
  { tag: [t.link, t.url], class: "tok-link" },
  { tag: t.inserted, class: "tok-inserted" },
  { tag: t.deleted, class: "tok-deleted" },
  { tag: t.invalid, class: "tok-invalid" },
]);

const known = [
  ...languages,
  // Not in @codemirror/language-data.
  LanguageDescription.of({
    name: "Nix",
    extensions: ["nix"],
    load: () => import("@replit/codemirror-lang-nix").then((m) => m.nix()),
  }),
];

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
