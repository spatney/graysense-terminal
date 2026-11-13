import { invoke } from '@tauri-apps/api/core';
import type { ITheme } from '@xterm/xterm';
import type { ThemeId } from './types';

export const THEMES: { id: ThemeId; name: string; note: string; swatch: [string, string, string] }[] = [
  { id: 'graphite', name: 'Graphite', note: 'Neutral greys, blue-violet', swatch: ['#0e0f13', '#14161b', '#7c8cff'] },
  { id: 'carbon', name: 'Carbon', note: 'Near-black, electric cyan', swatch: ['#08090b', '#0f1013', '#3fc6ff'] },
  { id: 'slate', name: 'Slate', note: 'Blue-grey, azure and mint', swatch: ['#0b0e14', '#12161f', '#35a3ea'] },
  { id: 'paper', name: 'Paper', note: 'Light', swatch: ['#f1f2f5', '#ffffff', '#4f5bd5'] },
];

export function cssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

/** Switch the page's palette and paint the native title bar to match. */
export function applyTheme(theme: ThemeId) {
  document.documentElement.dataset.theme = theme;
  void invoke('window_theme', { caption: cssVar('--chrome'), text: cssVar('--text'), border: cssVar('--border'), dark: theme !== 'paper' }).catch(() => {});
}

const DARK_ANSI = {
  black: '#1b1d23',
  red: '#ff6b6b',
  green: '#4fd68b',
  yellow: '#f2c35b',
  blue: '#6f9bff',
  magenta: '#c08cff',
  cyan: '#4fd0e0',
  white: '#d5d9e2',
  brightBlack: '#5c6370',
  brightRed: '#ff8a85',
  brightGreen: '#7ee6a8',
  brightYellow: '#ffd77a',
  brightBlue: '#94b4ff',
  brightMagenta: '#d6afff',
  brightCyan: '#7ce3ee',
  brightWhite: '#f4f6fa',
};

const LIGHT_ANSI = {
  black: '#1f2328',
  red: '#cf222e',
  green: '#116329',
  yellow: '#8a5a00',
  blue: '#0550ae',
  magenta: '#8250df',
  cyan: '#1b7c83',
  white: '#6e7781',
  brightBlack: '#57606a',
  brightRed: '#a40e26',
  brightGreen: '#1a7f37',
  brightYellow: '#633c01',
  brightBlue: '#218bff',
  brightMagenta: '#a475f9',
  brightCyan: '#3192aa',
  brightWhite: '#8c959f',
};

/** xterm.js colours from the current theme. */
export function terminalTheme(): ITheme {
  const light = document.documentElement.dataset.theme === 'paper';
  const accent = cssVar('--accent');
  return {
    background: cssVar('--code-bg'),
    foreground: cssVar('--code-text'),
    cursor: accent,
    cursorAccent: cssVar('--code-bg'),
    selectionBackground: `${accent}55`,
    selectionInactiveBackground: `${accent}30`,
    scrollbarSliderBackground: light ? '#00000022' : '#ffffff1c',
    scrollbarSliderHoverBackground: light ? '#00000035' : '#ffffff30',
    scrollbarSliderActiveBackground: light ? '#00000045' : '#ffffff40',
    ...(light ? LIGHT_ANSI : DARK_ANSI),
  };
}
