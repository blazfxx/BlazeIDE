// Settings UI primitives: row/toggle/inputs + the declarative registry that
// drives rendering, search filtering, and the nav rail.

import type { AppSettings } from '../../shared/types';
import { bus, el, escapeHtml, state } from '../state';

export async function apply(patch: Partial<AppSettings>): Promise<void> {
  await window.void.settingsSet(patch);
  state.settings = await window.void.settingsGet();
  bus.emit('settings-changed', state.settings);
}

export function toggle(checked: boolean, onChange: (v: boolean) => void): HTMLElement {
  const wrap = el('label', `switch${checked ? ' on' : ''}`);
  wrap.appendChild(el('span', 'knob'));
  wrap.addEventListener('click', () => {
    onChange(wrap.classList.toggle('on'));
  });
  return wrap;
}

export function numberInput(
  value: number,
  min: number,
  max: number,
  onChange: (v: number) => void
): HTMLElement {
  const input = el('input', 'setting-number');
  input.type = 'number';
  input.min = String(min);
  input.max = String(max);
  input.value = String(value);
  input.addEventListener('change', () => {
    const v = parseInt(input.value, 10);
    if (Number.isFinite(v) && v >= min && v <= max) onChange(v);
  });
  return input;
}

/** A slider (range) with a live value label. `onChange` fires continuously as
 *  the user drags; optional `onCommit` fires once on release (use it to persist
 *  while onChange does cheap live updates). `format` renders the label. */
export function sliderInput(
  value: number,
  min: number,
  max: number,
  onChange: (v: number) => void,
  format: (v: number) => string = (v) => String(v),
  onCommit?: (v: number) => void
): HTMLElement {
  const wrap = el('div', 'set-slider');
  const input = el('input', 'set-slider-input') as HTMLInputElement;
  input.type = 'range';
  input.min = String(min);
  input.max = String(max);
  input.value = String(value);
  const label = el('span', 'set-slider-value', format(value));
  input.addEventListener('input', () => {
    const v = Number(input.value);
    label.textContent = format(v);
    onChange(v);
  });
  if (onCommit) {
    input.addEventListener('change', () => onCommit(Number(input.value)));
  }
  wrap.append(input, label);
  return wrap;
}

export function selectInput(
  options: Array<{ value: string; label: string }>,
  value: string,
  onChange: (v: string) => void
): HTMLElement {
  const select = el('select', 'setting-select');
  for (const o of options) {
    const opt = el('option', '', escapeHtml(o.label));
    opt.value = o.value;
    select.appendChild(opt);
  }
  select.value = value;
  select.addEventListener('change', () => onChange(select.value));
  return select;
}

/** One setting row: title + description left, control right. */
export function settingRow(title: string, description: string, control: HTMLElement): HTMLElement {
  const r = el('div', 'set-row');
  r.dataset.search = `${title} ${description}`.toLowerCase();
  const left = el('div', 'set-row-text');
  left.appendChild(el('div', 'set-row-title', escapeHtml(title)));
  if (description) left.appendChild(el('div', 'set-row-desc', escapeHtml(description)));
  r.append(left, control);
  return r;
}

/** A full-width block (BYOK manager, keybindings table) inside a section. */
export function settingBlock(title: string, description: string, body: HTMLElement): HTMLElement {
  const r = el('div', 'set-row set-row-block');
  r.dataset.search = `${title} ${description}`.toLowerCase();
  const head = el('div', 'set-row-text');
  head.appendChild(el('div', 'set-row-title', escapeHtml(title)));
  if (description) head.appendChild(el('div', 'set-row-desc', escapeHtml(description)));
  r.append(head, body);
  return r;
}

export interface SettingsSection {
  id: string;
  label: string;
  rows: HTMLElement[];
}

export function filterSections(
  container: HTMLElement,
  nav: HTMLElement,
  query: string
): void {
  const q = query.trim().toLowerCase();
  for (const sectionEl of container.querySelectorAll<HTMLElement>('.set-section')) {
    let visible = 0;
    for (const rowEl of sectionEl.querySelectorAll<HTMLElement>('.set-row')) {
      const match = q === '' || (rowEl.dataset.search ?? '').includes(q);
      rowEl.style.display = match ? '' : 'none';
      if (match) visible++;
    }
    sectionEl.style.display = visible > 0 ? '' : 'none';
    const navItem = nav.querySelector<HTMLElement>(`[data-target="${sectionEl.id}"]`);
    if (navItem) navItem.style.display = visible > 0 ? '' : 'none';
  }
}
