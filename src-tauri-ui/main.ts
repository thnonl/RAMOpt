import './style.css';

type AppCloseEvent = { preventDefault: () => void };
type Unlisten = () => void;
declare global {
  interface Window {
    __TAURI__: {
      core: { invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> };
      event: { listen<T>(name: string, handler: (event: { payload: T }) => void): Promise<Unlisten> };
      window: { getCurrentWindow(): { onCloseRequested(handler: (event: AppCloseEvent) => void | Promise<void>): Promise<Unlisten>; setSize(size: unknown): Promise<void> } };
      dpi?: { LogicalSize: new (width: number, height: number) => unknown };
    };
  }
}
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

type Settings = {
  auto_clean: boolean; interval_minutes: number; hotkey: string; clean_temp: boolean;
  trim_background_apps: boolean; start_with_windows: boolean; close_to_tray: boolean;
  dark_mode: boolean; auto_threshold: boolean; threshold_percent: number;
};
type AppState = {
  settings: Settings;
  memory: { available: boolean; used_gb: number; total_gb: number; percent: number };
  status: string; logs: string[]; update_version: string | null; current_version: string; cleaning: boolean;
};
type ToggleKey = 'clean_temp' | 'trim_background_apps' | 'start_with_windows' | 'close_to_tray' | 'dark_mode';
// Scheduled and threshold cleanup exclude each other, so the UI models them as one choice.
type Mode = 'off' | 'schedule' | 'threshold';

const DEFAULT_HOTKEY = 'ctrl+alt+KeyR';
const HOTKEYS: ReadonlyArray<readonly [ui: string, backend: string]> = [
  ['Ctrl+Alt+R', 'ctrl+alt+KeyR'], ['Ctrl+Alt+M', 'ctrl+alt+KeyM'],
  ['Ctrl+Shift+R', 'ctrl+shift+KeyR'], ['Ctrl+Shift+M', 'ctrl+shift+KeyM'],
  ['Alt+R', 'alt+KeyR'], ['Alt+M', 'alt+KeyM'],
];
const MODES: ReadonlyArray<readonly [Mode, string]> = [['off', 'Off'], ['schedule', 'Scheduled'], ['threshold', 'RAM high']];

let state: AppState = { settings: { auto_clean: true, interval_minutes: 15, hotkey: 'Ctrl+Alt+R', clean_temp: true, trim_background_apps: false, start_with_windows: false, close_to_tray: true, dark_mode: false, auto_threshold: false, threshold_percent: 75 }, memory: { available: false, used_gb: 0, total_gb: 0, percent: 0 }, status: 'Starting RAMOpt…', logs: [], update_version: null, current_version: 'v0.3.1', cleaning: false };
let revision = 0;
let saveTimer = 0;
let settingsDirty = false;
let updating = false;
let settingsOpen = false;
let unlistenState: Unlisten | undefined;
let unlistenSettings: Unlisten | undefined;
let closeRequestedHandler: Unlisten | undefined;
const root = document.querySelector<HTMLDivElement>('#app')!;
const esc = (s: string): string => s.replace(/[&<>"']/g, c => ({ '&':'&amp;', '<':'&lt;', '>':'&gt;', '"':'&quot;', "'":'&#39;' })[c]!);
const modeOf = (s: Settings): Mode => s.auto_clean ? 'schedule' : s.auto_threshold ? 'threshold' : 'off';
const thresholdOf = (s: Settings): number => Math.max(1, Math.min(100, s.threshold_percent));

function toggleRow(key: ToggleKey, title: string, hint: string, on: boolean): string {
  return `<label class="row"><span class="row-text"><strong>${title}</strong>${hint ? `<small>${hint}</small>` : ''}</span><input class="toggle" type="checkbox" data-setting="${key}" ${on ? 'checked' : ''}></label>`;
}

function modePanel(s: Settings): string {
  switch (modeOf(s)) {
    case 'schedule':
      return `<div class="mode-line"><label for="interval">Clean every</label><span class="input-suffix"><input id="interval" type="number" min="1" max="1440" value="${s.interval_minutes}"><span>minutes</span></span></div>
        <p class="mode-hint">Runs on this timer, whatever the RAM usage.</p>`;
    case 'threshold':
      return `<div class="mode-line"><label for="threshold">Clean when RAM reaches</label><b id="threshold-value" class="value-pill">${thresholdOf(s)}%</b></div>
        <input id="threshold" type="range" min="1" max="100" value="${thresholdOf(s)}">
        <p class="mode-hint" title="At most one cleanup per minute">Checked every minute.</p>`;
    default:
      return `<p class="mode-hint">Only cleans when you press <strong>Clean RAM now</strong> or the hotkey.</p>`;
  }
}

function autoCard(s: Settings): string {
  const mode = modeOf(s);
  return `<h2 class="card-title">Automatic cleanup</h2>
    <div class="segmented" role="radiogroup" aria-label="Automatic cleanup mode">${MODES.map(([value, label]) =>
      `<label class="seg"><input type="radio" name="mode" value="${value}" ${mode === value ? 'checked' : ''}><span>${label}</span></label>`).join('')}</div>
    <div class="mode-panel">${modePanel(s)}</div>`;
}

function cleanOptionsCard(s: Settings): string {
  const current = HOTKEYS.find(([ui]) => ui === s.hotkey)?.[0] ?? HOTKEYS[0][0];
  return `<h2 class="card-title">Cleaning</h2>
    ${toggleRow('clean_temp', 'Clean temp files', 'Removes files older than 24 hours', s.clean_temp)}
    ${toggleRow('trim_background_apps', 'Close background apps', 'Skips services and protected apps', s.trim_background_apps)}
    <label class="row"><span class="row-text"><strong>Quick-clean hotkey</strong><small>Works from any app</small></span><select id="hotkey" aria-label="Quick-clean hotkey">${HOTKEYS.map(([ui]) => `<option value="${ui}" ${current === ui ? 'selected' : ''}>${ui.replaceAll('+', ' + ')}</option>`).join('')}</select></label>`;
}

function settingsModal(s: Settings): string {
  return `<div class="modal-card" role="dialog" aria-modal="true" aria-labelledby="modal-title">
    <div class="modal-head"><h2 id="modal-title">App settings</h2><button class="icon-button" data-action="close-settings" aria-label="Close settings" title="Close">✕</button></div>
    ${toggleRow('start_with_windows', 'Start with Windows', 'Launches RAMOpt after you sign in', s.start_with_windows)}
    ${toggleRow('close_to_tray', 'Close to tray', 'Keeps RAMOpt running when the window closes', s.close_to_tray)}
    ${toggleRow('dark_mode', 'Dark mode', 'Uses the dark theme', s.dark_mode)}
  </div>`;
}

function summaryOf(s: Settings): string {
  switch (modeOf(s)) {
    case 'schedule': return `Cleans every ${s.interval_minutes} min`;
    case 'threshold': return `Cleans at ${thresholdOf(s)}% RAM`;
    default: return 'Automatic cleanup off';
  }
}

function memoryCard(): string {
  const s = state.settings;
  const m = state.memory;
  const pct = Math.max(0, Math.min(100, m.percent));
  return `<div class="memory-top"><span class="eyebrow">SYSTEM MEMORY</span><span class="memory-mode">${m.available ? summaryOf(s) : 'Waiting for memory data'}</span></div>
    <div class="memory-head"><strong class="memory-value">${m.available ? `${m.used_gb.toFixed(1)} / ${m.total_gb.toFixed(1)} GB` : 'Memory data unavailable'}</strong><strong class="memory-percent">${m.available ? `${pct.toFixed(0)}%` : '—'}</strong></div>
    <div class="meter" role="progressbar" aria-label="Memory usage" aria-valuenow="${pct.toFixed(0)}" aria-valuemin="0" aria-valuemax="100"><span style="width:${pct}%"></span>${modeOf(s) === 'threshold' && m.available ? `<i title="Cleanup threshold" style="left:${thresholdOf(s)}%"></i>` : ''}</div>`;
}

const cache = new Map<string, string>();
// Skip identical markup so a periodic refresh never resets a field the user is editing.
function setHtml(id: string, html: string): void {
  if (cache.get(id) === html) return;
  cache.set(id, html);
  document.getElementById(id)!.innerHTML = html;
}

function renderSettings(): void {
  const s = state.settings;
  document.documentElement.dataset.theme = s.dark_mode ? 'dark' : 'light';
  setHtml('auto', autoCard(s));
  setHtml('clean-options', cleanOptionsCard(s));
}

function renderModal(): void {
  setHtml('modal', settingsOpen ? settingsModal(state.settings) : '');
  byId('modal').hidden = !settingsOpen;
  (root.firstElementChild as HTMLElement).inert = settingsOpen;
}

function renderLive(): void {
  byId('version').textContent = state.current_version;
  setHtml('banner', state.update_version ? `<section class="update-banner"><div><strong>Update available</strong><span>${esc(state.update_version)} is ready · you have ${esc(state.current_version)}</span></div><button class="button button-update" data-action="install-update" ${updating ? 'disabled' : ''}>${updating ? 'Starting…' : 'Install update'}</button></section>` : '');
  setHtml('memory', memoryCard());
  setHtml('activity', `<div class="activity-head"><h2>Recent activity</h2><span class="activity-status" id="status" role="status"><span class="status-text" title="${esc(state.status)}">${esc(state.status)}</span><span class="live-dot ${state.cleaning ? 'busy' : ''}" title="${state.cleaning ? 'Cleanup in progress' : 'Ready'}"></span></span></div><ol class="log-list">${state.logs.slice(-1).map(x => `<li>${esc(x)}</li>`).join('') || '<li class="empty-log">No cleanup logs yet.</li>'}</ol>`);
  setHtml('actions', `<button class="button button-secondary" data-action="restore-defaults">Restore defaults</button><span class="footer-spacer"></span><button class="button button-primary" data-action="clean-now" ${state.cleaning ? 'disabled' : ''}><span class="button-icon">✦</span>${state.cleaning ? 'Cleaning…' : 'Clean RAM now'}</button>`);
}

let lastFitHeight = 0;
let fitTimer = 0;
// Size the window to its content so the UI never needs a scrollbar; it grows or shrinks per mode and banner.
// A timer is used instead of requestAnimationFrame, which is paused while the window is hidden in the tray.
function fitWindow(): void {
  window.clearTimeout(fitTimer);
  fitTimer = window.setTimeout(() => {
    const shell = root.firstElementChild as HTMLElement | null;
    const dpi = window.__TAURI__.dpi;
    if (!shell || !dpi) return;
    const limit = Math.max(400, screen.availHeight - 48);
    const height = Math.min(Math.ceil(shell.getBoundingClientRect().height), limit);
    if (height === lastFitHeight) return;
    lastFitHeight = height;
    getCurrentWindow().setSize(new dpi.LogicalSize(window.innerWidth, height)).catch(() => { lastFitHeight = 0; });
  }, 30);
}

function render(): void {
  renderSettings();
  renderLive();
  renderModal();
}

const GEAR = '<svg viewBox="0 0 24 24" width="19" height="19" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/></svg>';

function buildShell(): void {
  root.innerHTML = `<main class="shell">
    <header class="topbar"><div class="brand-row"><div class="brand-mark">R</div><div class="brand-text"><div class="title-line"><h1>RAMOpt</h1><span class="version" id="version"></span></div><p>Lightweight RAM maintenance</p></div></div><button class="icon-button" data-action="open-settings" aria-label="App settings" title="App settings">${GEAR}</button></header>
    <div id="banner"></div>
    <section class="card memory-card" id="memory"></section>
    <div class="option-grid"><section class="card" id="auto"></section><section class="card" id="clean-options"></section></div>
    <section class="card activity-card" id="activity"></section>
    <footer class="actions" id="actions"></footer>
  </main>
  <div class="modal-backdrop" id="modal" hidden></div>`;
}

function toBackendSettings(settings: Settings): Settings {
  return { ...settings, hotkey: HOTKEYS.find(([ui]) => ui === settings.hotkey)?.[1] ?? DEFAULT_HOTKEY };
}
function fromBackendSettings(settings: Settings): Settings {
  return { ...settings, hotkey: HOTKEYS.find(([, backend]) => backend === settings.hotkey)?.[0] ?? HOTKEYS[0][0] };
}

function setMode(mode: Mode): void {
  patch({ auto_clean: mode === 'schedule', auto_threshold: mode === 'threshold' });
}

function bindControls(): void {
  root.addEventListener('change', event => {
    const el = event.target as HTMLInputElement | HTMLSelectElement;
    if (el.name === 'mode') setMode(el.value as Mode);
    else if (el.id === 'interval') patch({ interval_minutes: Math.max(1, Math.min(1440, Math.round(Number(el.value)) || 15)) });
    else if (el.id === 'threshold') patch({ threshold_percent: Number(el.value) });
    else if (el.id === 'hotkey') patch({ hotkey: el.value });
    else if (el.dataset.setting) patch({ [el.dataset.setting as ToggleKey]: (el as HTMLInputElement).checked });
  });
  root.addEventListener('input', event => {
    const el = event.target as HTMLInputElement;
    if (el.id !== 'threshold') return;
    state.settings.threshold_percent = Number(el.value);
    document.getElementById('threshold-value')!.textContent = `${thresholdOf(state.settings)}%`;
    renderLive();
  });
  document.addEventListener('keydown', event => { if (event.key === 'Escape') closeSettings(); });
  root.addEventListener('click', event => {
    const action = (event.target as HTMLElement).closest<HTMLElement>('[data-action]')?.dataset.action;
    if ((event.target as HTMLElement).id === 'modal') closeSettings();
    else if (action === 'open-settings') openSettings();
    else if (action === 'close-settings') closeSettings();
    else if (action === 'restore-defaults') void restoreDefaults();
    else if (action === 'clean-now') void run(async () => { await invoke('clean_now'); await refresh(); });
    else if (action === 'install-update') void installUpdate();
  });
}
function openSettings(): void {
  if (settingsOpen) return;
  settingsOpen = true;
  renderModal();
  document.querySelector<HTMLElement>('.modal-card [data-action=close-settings]')?.focus();
}
function closeSettings(): void {
  if (!settingsOpen) return;
  settingsOpen = false;
  renderModal();
  document.querySelector<HTMLElement>('[data-action=open-settings]')?.focus();
}
async function run(task: () => Promise<unknown>): Promise<void> { try { await task(); } catch (e) { showError(e); } }
async function installUpdate(): Promise<void> {
  updating = true;
  state.status = 'Starting updater…';
  renderLive();
  try { await invoke('install_update'); } catch (e) { updating = false; showError(e); }
}
async function restoreDefaults(): Promise<void> {
  await run(async () => {
    state.settings = fromBackendSettings(await invoke<Settings>('restore_defaults'));
    settingsDirty = false; revision++;
    await refresh();
  });
}
function byId<T extends HTMLElement>(id: string): T { return document.getElementById(id) as T; }
function patch(changes: Partial<Settings>, persist = true): void {
  state.settings = { ...state.settings, ...changes };
  if (state.settings.auto_clean) state.settings.auto_threshold = false;
  revision++; settingsDirty = true; render();
  if (!persist) return;
  window.clearTimeout(saveTimer);
  saveTimer = window.setTimeout(async () => {
    const current = revision;
    const payload = toBackendSettings(state.settings);
    try {
      const saved = fromBackendSettings(await invoke<Settings>('save_settings', { settings: payload }));
      if (current === revision) { state.settings = saved; settingsDirty = false; render(); }
    } catch (e) {
      if (current === revision) { showError(e); settingsDirty = false; await refresh(); }
    }
  }, 220);
}
async function refresh(): Promise<void> {
  try {
    const latest = await invoke<AppState>('get_app_state');
    latest.settings = settingsDirty ? state.settings : fromBackendSettings(latest.settings);
    state = latest;
    render();
  } catch (e) { showError(e); }
}
function showError(e: unknown): void {
  state.status = typeof e === 'string' ? e : e instanceof Error ? e.message : 'RAMOpt operation failed.';
  state.cleaning = false;
  render();
}
async function initialize(): Promise<void> {
  try {
    await refresh();
    unlistenState = await listen('app-state-changed', () => void refresh());
    unlistenSettings = await listen('open-settings', openSettings);
    closeRequestedHandler = await getCurrentWindow().onCloseRequested(async event => {
      if (state.settings.close_to_tray) {
        event.preventDefault();
        try { await invoke('hide_window'); } catch (error) { showError(error); }
      }
    });
    window.setInterval(() => void refresh(), 5000);
  } catch (e) { showError(e); }
}
window.addEventListener('beforeunload', () => { unlistenState?.(); unlistenSettings?.(); closeRequestedHandler?.(); });
buildShell(); bindControls(); render(); void initialize();
// Content height drives the window height, so any layout change (mode, banner, font load) re-fits it.
new ResizeObserver(fitWindow).observe(root.firstElementChild!);
