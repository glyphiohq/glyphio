// The Settings window keeps its heavy views out of the first load (PERF-05): the snippet
// editor and History are fetched with import() the first time they open. A static import
// would quietly undo that, so this pins it.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const shell = readFileSync(new URL('./settings.js', import.meta.url), 'utf8');
const staticImports = [...shell.matchAll(/^import [^;]*? from '([^']+)'/gms)].map((m) => m[1]);

for (const view of ['./snippet-editor.mjs', './history-view.mjs']) {
  test(`${view} is loaded on demand, never at startup`, () => {
    assert.ok(!staticImports.includes(view), `settings.js imports ${view} statically`);
    assert.ok(shell.includes(`import('${view}')`), `settings.js never loads ${view}`);
  });
}

test('the shared model loads without a window and keeps the snippet vocabulary', async () => {
  const { state, FORMATS, KINDS, KIND_HINTS } = await import('./model.mjs');
  assert.equal(state.selected, 'all');
  assert.deepEqual(FORMATS.map((f) => f.id), ['plain', 'html', 'markdown']);
  assert.deepEqual(KINDS.map((k) => k.id), Object.keys(KIND_HINTS));
});

test('every lazily loaded view exports the entry point settings.js calls', () => {
  for (const [file, entry] of [['snippet-editor.mjs', 'openEditor'], ['history-view.mjs', 'renderHistory']]) {
    const src = readFileSync(new URL(`./${file}`, import.meta.url), 'utf8');
    assert.match(src, new RegExp(`^export (async )?function ${entry}\\b`, 'm'), `${file} lacks ${entry}`);
  }
});
