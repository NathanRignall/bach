// The languages and token classes shared by the highlight worker (diff and file viewer) and the
// editor, so code looks the same whether it is read or edited.
import { LanguageDescription } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { tagHighlighter, tags as t } from "@lezer/highlight";

/** Token classes, styled in index.css. Kept few, so the colours stay calm. */
export const highlighter = tagHighlighter([
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

export const known = [
  ...languages,
  // Not in @codemirror/language-data.
  LanguageDescription.of({
    name: "Nix",
    extensions: ["nix"],
    load: () => import("@replit/codemirror-lang-nix").then((m) => m.nix()),
  }),
];
