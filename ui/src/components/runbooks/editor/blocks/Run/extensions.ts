// Based on the basicSetup extension, as suggested by the source. Customized for Atuin.

import {
  KeyBinding,
  lineNumbers,
  highlightActiveLineGutter,
  highlightSpecialChars,
  drawSelection,
  dropCursor,
  rectangularSelection,
  crosshairCursor,
  highlightActiveLine,
  keymap,
} from "@codemirror/view";
import { EditorState, Extension } from "@codemirror/state";
import { history, defaultKeymap, historyKeymap } from "@codemirror/commands";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";

import {
  closeBrackets,
  autocompletion,
  closeBracketsKeymap,
  completionKeymap,
  CompletionContext,
} from "@codemirror/autocomplete";

import {
  foldGutter,
  indentOnInput,
  syntaxHighlighting,
  defaultHighlightStyle,
  bracketMatching,
  indentUnit,
  foldKeymap,
} from "@codemirror/language";

import { lintKeymap } from "@codemirror/lint";
import { invoke } from "@tauri-apps/api/core";

export interface MinimalSetupOptions {
  highlightSpecialChars?: boolean;
  history?: boolean;
  drawSelection?: boolean;
  syntaxHighlighting?: boolean;

  defaultKeymap?: boolean;
  historyKeymap?: boolean;
}

export interface BasicSetupOptions extends MinimalSetupOptions {
  lineNumbers?: boolean;
  highlightActiveLineGutter?: boolean;
  foldGutter?: boolean;
  dropCursor?: boolean;
  allowMultipleSelections?: boolean;
  indentOnInput?: boolean;
  bracketMatching?: boolean;
  closeBrackets?: boolean;
  autocompletion?: boolean;
  rectangularSelection?: boolean;
  crosshairCursor?: boolean;
  highlightActiveLine?: boolean;
  highlightSelectionMatches?: boolean;

  closeBracketsKeymap?: boolean;
  searchKeymap?: boolean;
  foldKeymap?: boolean;
  completionKeymap?: boolean;
  lintKeymap?: boolean;
  tabSize?: number;
}

function myCompletions(context: CompletionContext) {
  let word = context.matchBefore(/^.*/);

  if (!word) return null;
  if (word.from == word.to && !context.explicit) return null;

  return invoke("prefix_search", { query: word.text }).then(
    // @ts-ignore
    (results: string[]) => {
      let options = results.map((i) => {
        return { label: i, type: "text" };
      });

      return {
        from: word.from,
        options,
      };
    },
  );
}

const buildAutocomplete = (): Extension => {
  let ac = autocompletion({ override: [myCompletions] });

  return ac;
};

export const extensions = (options: BasicSetupOptions = {}): Extension[] => {
  const { crosshairCursor: initCrosshairCursor = false } = options;

  let keymaps: KeyBinding[] = [];
  if (options.closeBracketsKeymap !== false) {
    keymaps = keymaps.concat(closeBracketsKeymap);
  }
  if (options.defaultKeymap !== false) {
    keymaps = keymaps.concat(defaultKeymap);
  }
  if (options.searchKeymap !== false) {
    keymaps = keymaps.concat(searchKeymap);
  }
  if (options.historyKeymap !== false) {
    keymaps = keymaps.concat(historyKeymap);
  }
  if (options.foldKeymap !== false) {
    keymaps = keymaps.concat(foldKeymap);
  }
  if (options.completionKeymap !== false) {
    keymaps = keymaps.concat(completionKeymap);
  }
  if (options.lintKeymap !== false) {
    keymaps = keymaps.concat(lintKeymap);
  }
  const extensions: Extension[] = [];
  if (options.lineNumbers !== false) extensions.push(lineNumbers());
  if (options.highlightActiveLineGutter !== false)
    extensions.push(highlightActiveLineGutter());
  if (options.highlightSpecialChars !== false)
    extensions.push(highlightSpecialChars());
  if (options.history !== false) extensions.push(history());
  if (options.foldGutter !== false) extensions.push(foldGutter());
  if (options.drawSelection !== false) extensions.push(drawSelection());
  if (options.dropCursor !== false) extensions.push(dropCursor());
  if (options.allowMultipleSelections !== false)
    extensions.push(EditorState.allowMultipleSelections.of(true));
  if (options.indentOnInput !== false) extensions.push(indentOnInput());
  if (options.syntaxHighlighting !== false)
    extensions.push(
      syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
    );

  if (options.bracketMatching !== false) extensions.push(bracketMatching());
  if (options.closeBrackets !== false) extensions.push(closeBrackets());
  if (options.autocompletion !== false) extensions.push(buildAutocomplete());

  if (options.rectangularSelection !== false)
    extensions.push(rectangularSelection());
  if (initCrosshairCursor !== false) extensions.push(crosshairCursor());
  if (options.highlightActiveLine !== false)
    extensions.push(highlightActiveLine());
  if (options.highlightSelectionMatches !== false)
    extensions.push(highlightSelectionMatches());
  if (options.tabSize && typeof options.tabSize === "number")
    extensions.push(indentUnit.of(" ".repeat(options.tabSize)));

  return extensions.concat([keymap.of(keymaps.flat())]).filter(Boolean);
};
