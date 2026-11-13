import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import '@xterm/xterm/css/xterm.css';
import './styles/tokens.css';
import './styles/base.css';
import './styles/app.css';
import './styles/chat.css';
import { App } from './App';
import { connect } from './lib/api';

// Paint the last theme straight away, before the core's settings arrive.
const saved = localStorage.getItem('gs-theme');
if (saved) document.documentElement.dataset.theme = saved;

// The page is the app: no browser context menu, no reload or zoom keys.
window.addEventListener('contextmenu', (e) => {
  const t = e.target as HTMLElement;
  if (!t.closest('input, textarea, .selectable, .xterm')) e.preventDefault();
});
window.addEventListener('keydown', (e) => {
  if ((e.key === 'F5' || (e.ctrlKey && e.key.toLowerCase() === 'r')) && !import.meta.env.DEV) e.preventDefault();
});

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);

void connect().catch((e) => {
  console.error('could not reach the Graysense core', e);
});
