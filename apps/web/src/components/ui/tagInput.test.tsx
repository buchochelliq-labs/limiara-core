// @vitest-environment happy-dom
import {act, useState} from 'react';
import {createRoot, type Root} from 'react-dom/client';
import {afterEach, beforeEach, describe, expect, it, vi} from 'vitest';
import {TagInput} from './primitives';

(globalThis as {IS_REACT_ACT_ENVIRONMENT?: boolean}).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;
beforeEach(() => {
  container = document.createElement('div');
  document.body.append(container);
  root = createRoot(container);
});
afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

function Harness({initial = [], max, onSubmit}: {initial?: string[]; max?: number; onSubmit?: () => void}) {
  const [values, setValues] = useState(initial);
  return <form onSubmit={event => { event.preventDefault(); onSubmit?.(); }}>
    <TagInput id="tags" label="Tags" values={values} max={max} maxLength={80} onChange={setValues} />
  </form>;
}

const input = () => container.querySelector<HTMLInputElement>('#tags')!;
const chips = () => [...container.querySelectorAll('.ui-tags__text')].map(item => item.textContent);
const status = () => container.querySelector('[role="status"]')?.textContent;
const type = (text: string) => act(() => {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!;
  setter.call(input(), text);
  input().dispatchEvent(new Event('input', {bubbles: true}));
});
const key = (name: string) => act(() => { input().dispatchEvent(new KeyboardEvent('keydown', {key: name, bubbles: true, cancelable: true})); });

describe('the chip input for text lists (ADR 0035 G7)', () => {
  it('adds what was typed on Enter, without submitting the form', () => {
    const onSubmit = vi.fn();
    act(() => root.render(<Harness onSubmit={onSubmit} />));
    type('  VIP  ');
    key('Enter');
    type('Speaker, keynote');
    key('Enter');
    expect(chips()).toEqual(['VIP', 'Speaker, keynote']);
    expect(input().value).toBe('');
    expect(status()).toBe('Speaker, keynote added.');
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it('labels the input and names each remove button by its value', () => {
    act(() => root.render(<Harness initial={['VIP']} />));
    expect(container.querySelector('label')?.getAttribute('for')).toBe('tags');
    expect(container.querySelector('ul')?.getAttribute('aria-label')).toBe('Tags values');
    expect(input().getAttribute('aria-describedby')).toContain('tags-values');
    expect(container.querySelector('.ui-tags__remove')?.getAttribute('aria-label')).toBe('Remove VIP');
  });

  it('does not add a duplicate, ignoring case', () => {
    act(() => root.render(<Harness initial={['VIP']} />));
    type('vip');
    key('Enter');
    expect(chips()).toEqual(['VIP']);
    expect(status()).toBe('vip is already added.');
  });

  it('removes a chip by its button, or the last one with Backspace in an empty input', () => {
    act(() => root.render(<Harness initial={['VIP', 'Speaker', 'Partner']} />));
    act(() => container.querySelector<HTMLButtonElement>('[aria-label="Remove Speaker"]')!.click());
    expect(chips()).toEqual(['VIP', 'Partner']);
    expect(document.activeElement).toBe(input());
    key('Backspace');
    expect(chips()).toEqual(['VIP']);
    expect(status()).toBe('Partner removed.');
    type('x');
    key('Backspace');
    expect(chips()).toEqual(['VIP']);
  });

  it('keeps typed text when the member leaves the field', () => {
    act(() => root.render(<Harness />));
    type('Newsletter');
    act(() => { input().dispatchEvent(new FocusEvent('focusout', {bubbles: true})); });
    expect(chips()).toEqual(['Newsletter']);
  });

  it('closes once it holds the most values the field takes', () => {
    act(() => root.render(<Harness initial={['a']} max={2} />));
    type('b');
    key('Enter');
    expect(chips()).toEqual(['a', 'b']);
    expect(input().disabled).toBe(true);
    expect(input().placeholder).toBe('Up to 2 values');
  });
});
