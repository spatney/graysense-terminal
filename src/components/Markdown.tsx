import hljs from 'highlight.js/lib/common';
import { memo, useMemo, useState } from 'react';
import ReactMarkdown, { type Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { openLink } from '../lib/api';
import { Icon, copyText } from './ui';

function highlight(code: string, lang: string | undefined): string | null {
  try {
    if (lang && hljs.getLanguage(lang)) return hljs.highlight(code, { language: lang, ignoreIllegals: true }).value;
    if (code.length < 20_000) {
      const r = hljs.highlightAuto(code);
      if (r.relevance >= 5) return r.value;
    }
  } catch {
    /* plain text */
  }
  return null;
}

const LANG_NAMES: Record<string, string> = {
  ts: 'TypeScript',
  tsx: 'TSX',
  js: 'JavaScript',
  jsx: 'JSX',
  rs: 'Rust',
  rust: 'Rust',
  py: 'Python',
  python: 'Python',
  ps1: 'PowerShell',
  powershell: 'PowerShell',
  pwsh: 'PowerShell',
  sh: 'Shell',
  bash: 'Bash',
  json: 'JSON',
  yaml: 'YAML',
  yml: 'YAML',
  toml: 'TOML',
  md: 'Markdown',
  html: 'HTML',
  css: 'CSS',
  sql: 'SQL',
  diff: 'Diff',
  cs: 'C#',
  csharp: 'C#',
  go: 'Go',
  java: 'Java',
  cpp: 'C++',
  c: 'C',
};

export function CodeBlock({ code, lang, maxHeight }: { code: string; lang?: string; maxHeight?: number }) {
  const html = useMemo(() => highlight(code, lang), [code, lang]);
  const [wrap, setWrap] = useState(false);
  return (
    <div className="codeblock">
      <div className="codeblock-head">
        <span>{lang ? (LANG_NAMES[lang.toLowerCase()] ?? lang) : 'Text'}</span>
        <span className="grow" />
        <button type="button" className="icon-btn sm" data-tip={wrap ? 'No wrap' : 'Wrap lines'} onClick={() => setWrap(!wrap)}>
          <Icon name="word-wrap" />
        </button>
        <button type="button" className="icon-btn sm" data-tip="Copy" onClick={() => void copyText(code)}>
          <Icon name="copy" />
        </button>
      </div>
      <pre className={`selectable hljs ${wrap ? 'wrap' : ''}`} style={maxHeight ? { maxHeight } : undefined}>
        {html != null ? <code dangerouslySetInnerHTML={{ __html: html }} /> : <code>{code}</code>}
      </pre>
    </div>
  );
}

const components: Components = {
  a: ({ href, children }) => (
    <a
      href={href}
      onClick={(e) => {
        e.preventDefault();
        if (href) openLink(href);
      }}
      data-tip={href}
    >
      {children}
    </a>
  ),
  pre: ({ children }) => <>{children}</>,
  code: ({ className, children }) => {
    const text = String(children ?? '');
    const lang = /language-([\w+#-]+)/.exec(className ?? '')?.[1];
    if (lang || text.includes('\n')) return <CodeBlock code={text.replace(/\n$/, '')} lang={lang} />;
    return <code>{children}</code>;
  },
  table: ({ children }) => (
    <div className="md-table">
      <table>{children}</table>
    </div>
  ),
  img: ({ alt }) => <span className="faint">[{alt || 'image'}]</span>,
};

/** Markdown as the transcript shows it: GitHub-flavoured, with highlighted, copyable code. */
export const Markdown = memo(function Markdown({ text, className = '' }: { text: string; className?: string }) {
  return (
    <div className={`md selectable ${className}`}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
});
