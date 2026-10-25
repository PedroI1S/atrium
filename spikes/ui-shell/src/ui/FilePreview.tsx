import { FileCode2 } from "lucide-solid";
import { For } from "solid-js";
import { FILES } from "../data/dummy";

const KEYWORDS = new Set(["use", "const", "pub", "struct", "impl", "fn", "if", "let", "return", "mod", "for", "export", "function", "else", "as", "mut", "self", "Self", "new"]);

/** A deliberately tiny highlighter: enough to show `--syntax-*` tokens at work. Shiki replaces it in M1. */
function highlight(line: string): { cls: string; text: string }[] {
  const out: { cls: string; text: string }[] = [];
  const re = /(\/\/.*$)|("(?:[^"\\]|\\.)*")|(\b\d[\d_.]*\b)|(\b[A-Za-z_][A-Za-z0-9_]*\b)(\()?|(\s+)|(.)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(line))) {
    if (m[1]) out.push({ cls: "syntax-comment", text: m[1] });
    else if (m[2]) out.push({ cls: "syntax-string", text: m[2] });
    else if (m[3]) out.push({ cls: "syntax-number", text: m[3] });
    else if (m[4]) {
      const w = m[4];
      const cls = KEYWORDS.has(w) ? "syntax-keyword" : m[5] ? "syntax-function" : /^[A-Z]/.test(w) ? "syntax-type" : "syntax-variable";
      out.push({ cls, text: w });
      if (m[5]) out.push({ cls: "syntax-punctuation", text: "(" });
    } else if (m[6]) out.push({ cls: "", text: m[6] });
    else if (m[7]) out.push({ cls: "syntax-punctuation", text: m[7] });
  }
  return out;
}

export function FilePreview(props: { file: string }) {
  const f = () => FILES[props.file] ?? FILES["session.rs"]!;
  return (
    <section class="pane preview" aria-label="File preview" data-atrium-part="file-preview">
      <header class="pane-head">
        <FileCode2 class="icon-md" />
        <span class="pane-title mono">{f().path}</span>
        <span class="side-grow" />
        <span class="pane-sub">read-only preview</span>
      </header>
      <div class="code-scroll">
        <pre class="code">
          <For each={f().lines}>
            {(line, i) => (
              <div class="code-line">
                <span class="code-no">{i() + 1}</span>
                <span class="code-text">
                  <For each={highlight(line)}>{(t) => <span class={t.cls}>{t.text}</span>}</For>
                </span>
              </div>
            )}
          </For>
        </pre>
      </div>
    </section>
  );
}
