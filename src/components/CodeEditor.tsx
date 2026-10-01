// The text editor behind manual edits (CodeMirror 6). Loaded on demand by FileEditor, so the
// viewer and the diff don't carry it. Highlighting uses the same token classes as the read-only
// views (lib/languages.ts, styled in index.css), so both themes and accents apply.
import { type Ref, useEffect, useImperativeHandle, useRef } from "react";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching, indentOnInput, LanguageDescription, syntaxHighlighting } from "@codemirror/language";
import { Compartment, EditorState } from "@codemirror/state";
import { EditorView, drawSelection, highlightActiveLine, highlightActiveLineGutter, keymap, lineNumbers } from "@codemirror/view";
import { highlighter, known } from "@/lib/languages";

export interface CodeEditorHandle {
  /** The text as edited, with the file's own line endings. */
  getText: () => string;
  focus: () => void;
}

const theme = EditorView.theme({
  "&": { color: "var(--card-foreground)", backgroundColor: "transparent", fontSize: "12px", maxHeight: "inherit", height: "100%" },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": { fontFamily: "inherit", lineHeight: "20px" },
  ".cm-content": { caretColor: "var(--foreground)", padding: "8px 0" },
  ".cm-line": { padding: "0 16px 0 12px" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--foreground)" },
  ".cm-gutters": { backgroundColor: "var(--card)", color: "color-mix(in oklab, var(--muted-foreground) 70%, transparent)", border: "none", borderRight: "1px solid color-mix(in oklab, var(--border) 60%, transparent)" },
  ".cm-lineNumbers .cm-gutterElement": { padding: "0 8px 0 12px", minWidth: "2.5rem" },
  ".cm-activeLine": { backgroundColor: "color-mix(in oklab, var(--primary) 8%, transparent)" },
  ".cm-activeLineGutter": { backgroundColor: "color-mix(in oklab, var(--primary) 12%, transparent)", color: "var(--foreground)" },
  "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground": { backgroundColor: "color-mix(in oklab, var(--primary) 28%, transparent)" },
  ".cm-matchingBracket": { backgroundColor: "color-mix(in oklab, var(--primary) 25%, transparent)", outline: "none" },
});

/**
 * Edits `text`, the language of `path` highlighted. The text is only the starting point: read the
 * result with `getText`, and mount afresh (a new `key`) to load other text. `line` (1-based)
 * is put in view, with the cursor on it. Mod-S calls `onSave`.
 */
export function CodeEditor({
  path,
  text,
  line,
  onChange,
  onSave,
  onUnmount,
  ref,
}: {
  path: string;
  text: string;
  line?: number;
  /** Called on every edit. */
  onChange: () => void;
  onSave: () => void;
  /** Called with the text as edited when the editor goes away (closed, or remounted). */
  onUnmount?: (text: string) => void;
  ref?: Ref<CodeEditorHandle>;
}) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView>(null);
  // The callbacks change with every render; the editor is built once.
  const callbacks = useRef({ onChange, onSave, onUnmount });
  callbacks.current = { onChange, onSave, onUnmount };

  useImperativeHandle(ref, () => ({
    getText: () => (view.current ? view.current.state.doc.sliceString(0, undefined, view.current.state.lineBreak) : text),
    focus: () => view.current?.focus(),
  }));

  useEffect(() => {
    const language = new Compartment();
    // Keep a file's CRLF line endings instead of turning every line into a change.
    const lineSeparator = text.includes("\r\n") && !/(^|[^\r])\n/.test(text) ? "\r\n" : undefined;
    const state = EditorState.create({
      doc: text,
      extensions: [
        lineSeparator ? EditorState.lineSeparator.of(lineSeparator) : [],
        EditorState.tabSize.of(4),
        lineNumbers(),
        highlightActiveLineGutter(),
        highlightActiveLine(),
        history(),
        drawSelection(),
        indentOnInput(),
        bracketMatching(),
        syntaxHighlighting(highlighter),
        language.of([]),
        keymap.of([{ key: "Mod-s", preventDefault: true, run: () => (callbacks.current.onSave(), true) }, indentWithTab, ...defaultKeymap, ...historyKeymap]),
        theme,
        EditorView.updateListener.of((u) => {
          if (u.docChanged) callbacks.current.onChange();
        }),
      ],
    });
    const v = new EditorView({ state, parent: host.current! });
    view.current = v;

    if (line && line > 0) {
      const at = v.state.doc.line(Math.min(line, v.state.doc.lines));
      v.dispatch({ selection: { anchor: at.from }, effects: EditorView.scrollIntoView(at.from, { y: "center" }) });
    }
    v.focus();

    // The language arrives a moment later; the text is editable meanwhile.
    let live = true;
    const desc = LanguageDescription.matchFilename(known, path.slice(path.lastIndexOf("/") + 1));
    desc?.load().then(
      (support) => live && v.dispatch({ effects: language.reconfigure(support) }),
      () => {},
    );
    return () => {
      live = false;
      callbacks.current.onUnmount?.(v.state.doc.sliceString(0, undefined, v.state.lineBreak));
      v.destroy();
      view.current = null;
    };
  }, [path, text, line]);

  return <div ref={host} className="h-full max-h-[inherit] min-h-0 [&>.cm-editor]:h-full" />;
}
