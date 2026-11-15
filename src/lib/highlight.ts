import hljs from 'highlight.js/lib/common';

/** `code` as highlighted HTML, or null to show it as plain text. Without a known language, a long enough snippet is recognized. */
export function highlight(code: string, lang: string | undefined): string | null {
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

const EXT_LANG: Record<string, string> = {
  ts: 'typescript',
  tsx: 'typescript',
  mts: 'typescript',
  cts: 'typescript',
  js: 'javascript',
  jsx: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  json: 'json',
  jsonc: 'json',
  rs: 'rust',
  py: 'python',
  ps1: 'powershell',
  sh: 'bash',
  bash: 'bash',
  md: 'markdown',
  yml: 'yaml',
  yaml: 'yaml',
  toml: 'ini',
  ini: 'ini',
  cs: 'csharp',
  go: 'go',
  java: 'java',
  kt: 'kotlin',
  c: 'c',
  h: 'c',
  cpp: 'cpp',
  hpp: 'cpp',
  html: 'xml',
  xml: 'xml',
  svg: 'xml',
  vue: 'xml',
  css: 'css',
  scss: 'scss',
  less: 'less',
  sql: 'sql',
};

/** The highlight.js language for a file, from its extension. */
export function langOf(path: string | undefined): string | undefined {
  const ext = path?.split('.').pop()?.toLowerCase();
  return ext ? EXT_LANG[ext] : undefined;
}

const TOKEN = /<span[^>]*>|<\/span>|\n|[^<\n]+|</g;

/**
 * `lines` highlighted as one block (so a string or comment that spans lines comes out right), then
 * split back into a line each, closing and reopening the spans a line break cuts. Null when the
 * language isn't known.
 */
export function highlightLines(lines: readonly string[], lang: string | undefined): string[] | null {
  if (!lines.length) return [];
  if (!lang || !hljs.getLanguage(lang)) return null;
  let html: string;
  try {
    html = hljs.highlight(lines.join('\n'), { language: lang, ignoreIllegals: true }).value;
  } catch {
    return null;
  }
  const out: string[] = [];
  const open: string[] = [];
  let cur = '';
  for (const token of html.match(TOKEN) ?? []) {
    if (token === '\n') {
      out.push(cur + '</span>'.repeat(open.length));
      cur = open.join('');
      continue;
    }
    if (token.startsWith('<span')) open.push(token);
    else if (token === '</span>') open.pop();
    cur += token;
  }
  out.push(cur + '</span>'.repeat(open.length));
  return out.length === lines.length ? out : null;
}
