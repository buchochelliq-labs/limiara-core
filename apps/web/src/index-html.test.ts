import {describe, expect, it} from 'vitest';
import html from '../index.html?raw';

// The iOS and Android app shells load this page in a web view, which lays a
// page without a viewport tag out 980 CSS pixels wide: the shell's
// narrow-screen layout never applies and the page overflows the phone.
describe('index.html', () => {
  it('lays the page out at the device width', () => {
    expect(html).toMatch(/<meta name="viewport" content="width=device-width, initial-scale=1">/);
  });

  it('is a standards-mode document with a language and charset', () => {
    expect(html.trimStart().toLowerCase()).toMatch(/^<!doctype html>/);
    expect(html).toMatch(/<html lang="en">/);
    expect(html).toMatch(/<meta charset="utf-8">/);
  });
});
