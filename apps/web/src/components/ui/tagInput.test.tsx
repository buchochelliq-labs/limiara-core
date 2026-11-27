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

/** A form that holds the typed text itself, as record forms do for drafts. */
function HeldText({onDraft}: {onDraft: (values: string[], text: string) => void}) {
  const [draft, setDraft] = useState<{values: string[]; text: string}>({values: ['VIP'], text: ''});
  return <TagInput id="tags" label="Tags" values={draft.values} text={draft.text} onChange={(values, text) => { setDraft({values, text}); onDraft(values, text); }} />;
}

const input = () => container.querySelector<HTMLInputElement>('#tags')!;
const chips = () => [...container.querySelectorAll('.ui-tags__text')].map(item => item.textContent);
const status = () => container.querySelector('[role="status"]')?.textContent;
const type = (text: string) => act(() => {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!;
  setter.call(input(), text);
  input().dispatchEvent(new Event('input', {bubbles: true}));
});
const key = (name: string, init: KeyboardEventInit = {}) => act(() => { input().dispatchEvent(new KeyboardEvent('keydown', {key: name, bubbles: true, cancelable: true, ...init})); });

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

  it('leaves Enter to an input method confirming its candidate', () => {
    act(() => root.render(<Harness />));
    type('とうきょう');
    key('Enter', {isComposing: true});
    expect(chips()).toEqual([]);
    expect(input().value).toBe('とうきょう');
    key('Enter', {keyCode: 229} as KeyboardEventInit);
    expect(chips()).toEqual([]);
    type('東京');
    key('Enter');
    expect(chips()).toEqual(['東京']);
  });

  it('reports typed text to a form that holds it, with the values in one update', () => {
    const onDraft = vi.fn();
    act(() => root.render(<HeldText onDraft={onDraft} />));
    type('Part');
    expect(onDraft).toHaveBeenLastCalledWith(['VIP'], 'Part');
    expect(input().value).toBe('Part');
    key('Enter');
    expect(onDraft).toHaveBeenLastCalledWith(['VIP', 'Part'], '');
    expect(chips()).toEqual(['VIP', 'Part']);
    type('Spe');
    act(() => container.querySelector<HTMLButtonElement>('[aria-label="Remove VIP"]')!.click());
    expect(onDraft).toHaveBeenLastCalledWith(['Part'], 'Spe');
    expect(input().value).toBe('Spe');
  });
});
