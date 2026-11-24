// settings/snippet-editor.mjs — the two-pane snippet editor (form + live preview), its rich
// toolbar, inline images and format conversion. Loaded the first time someone opens a
// snippet, so a Settings window that only changes a toggle never parses it (PERF-05).

import { icon } from '../shared/icons.js';
import { escapeHtml, escapeAttr, mdToHtml, sanitizeSnippetHtml } from '../shared/markdown.js';
import { state, FORMATS, KINDS, KIND_HINTS } from './model.mjs';
import { el, openModal, openPopover, setStatus } from './dom.mjs';

const { invoke } = window.__TAURI__.core;

// --- Editor (two-pane modal: form + live preview) ---------------------------

const FMT_HINTS = {
  plain: 'Inserted exactly as typed.',
  html: 'Rich text — pastes with formatting. Select an image, then use the resize button in the toolbar.',
  markdown: 'Markdown source, pasted as rich text.',
};

export function openEditor(existing) {
  const isEdit = Boolean(existing);
  let format = existing?.format || 'plain';
  let kind = existing?.kind || 'text';
  let htmlView = 'rich'; // for format=html: 'rich' (WYSIWYG) | 'source' (raw HTML)
  let savedRange = null; // caret saved before a toolbar popover steals focus
  let selectionHandler = null;
  // Form-kind field builder state ({name,label,type,options}); stored as variables.fields.
  let formFields = (kind === 'form' && Array.isArray(existing?.variables?.fields))
    ? existing.variables.fields.map((f) => ({ ...f }))
    : [];

  const groupOptions = `<option value="">Ungrouped</option>` +
    state.groups.map((g) => `<option value="${g.id}">${escapeHtml(g.name)}</option>`).join('');

  const { modal, close, $ } = openModal(`
    <div class="modal-head">
      <h3>${isEdit ? 'Edit snippet' : 'New snippet'}</h3>
      <button class="icon-btn" id="e-close" title="Close (Esc)" aria-label="Close">${icon('x', 15)}</button>
    </div>
    <div class="editor-panes">
      <div class="editor-form">
        <div class="mfield">
          <label for="e-trigger">Trigger</label>
          <input id="e-trigger" type="text" placeholder=":sig" autocomplete="off" spellcheck="false" autocapitalize="off" />
          <p class="fmt-hint">Lowercase, no spaces — you type this inline to expand.</p>
          <p class="field-error" id="e-trigger-err"></p>
        </div>
        <div class="mfield">
          <label>Type</label>
          <div class="seg" id="e-kind"></div>
          <p class="fmt-hint" id="e-kind-hint"></p>
        </div>
        <div class="mfield">
          <label for="e-group">Group</label>
          <select id="e-group">${groupOptions}</select>
          <p class="fmt-hint" id="e-group-hint" style="display:none">Command snippets stay personal — they never sync, even inside a shared group.</p>
        </div>
        <div class="mfield" id="e-fmt-row">
          <label>Format</label>
          <div class="seg" id="e-fmt"></div>
          <p class="fmt-hint" id="e-fmt-hint"></p>
        </div>
        <div class="mfield" id="e-shell-row" style="display:none">
          <label for="e-shell">Shell</label>
          <select id="e-shell">
            <option value="sh">sh (default)</option>
            <option value="bash">bash</option>
            <option value="zsh">zsh</option>
          </select>
        </div>
        <div class="mfield" id="e-fields-row" style="display:none">
          <label>Form fields</label>
          <p class="adv-hint">Reference a field as <code>{{name}}</code> — double braces, the name not the label. No spaces in names. With no fields, each <code>{{placeholder}}</code> becomes a text input.</p>
          <div id="e-fields"></div>
          <button type="button" class="secondary sm" id="e-field-add">Add field</button>
          <p class="field-error" id="e-fields-err"></p>
        </div>
        <div class="mfield content-field">
          <div class="content-head">
            <label>Content</label>
            <div class="seg seg-mini" id="e-htmlview" style="display:none">
              <button type="button" class="seg-opt" data-view="rich">Rich</button>
              <button type="button" class="seg-opt" data-view="source">HTML</button>
            </div>
          </div>
          <div class="rich-toolbar" id="e-rich-toolbar"></div>
          <div id="e-body-wrap"></div>
          <p class="field-error" id="e-body-err"></p>
        </div>
        <details class="adv" id="e-adv">
          <summary>Variables <span class="adv-sub">— dynamic values (advanced)</span></summary>
          <p class="adv-hint">Dynamic values inserted at expansion time. A JSON array; reference them in your content as <code>{{name}}</code>.</p>
          <textarea id="e-vars" class="mono" spellcheck="false" placeholder='[{"name":"date","type":"date","params":{"format":"%Y-%m-%d"}}]'></textarea>
          <p class="field-error" id="e-vars-err"></p>
        </details>
        <details class="adv" id="e-scope-adv">
          <summary>App scope <span class="adv-sub">— limit where this snippet expands</span></summary>
          <p class="adv-hint">Empty = everywhere. An app name matches the app's executable (e.g. <code>Slack</code>); or use <code>exec:&lt;regex&gt;</code> / <code>title:&lt;regex&gt;</code> for full control.</p>
          <input id="e-scope" type="text" autocomplete="off" spellcheck="false" placeholder="e.g. Slack" />
        </details>
      </div>
      <div class="editor-preview">
        <div class="preview-head">Live preview <span class="preview-sub" id="e-preview-fmt"></span></div>
        <div class="preview-body" id="e-preview"></div>
      </div>
    </div>
    <div class="modal-actions">
      <span class="save-hint">⌘↵ save · Esc cancel</span>
      <div class="spacer"></div>
      <button class="secondary" id="e-cancel">Cancel</button>
      <button class="primary" id="e-save">${isEdit ? 'Save' : 'Create'}</button>
    </div>`, { className: 'editor2', onClose: cleanup });

  function cleanup() {
    if (selectionHandler) { document.removeEventListener('selectionchange', selectionHandler); selectionHandler = null; }
  }

  const gsel = $('#e-group');
  let defaultGroup = '';
  if (state.selected.startsWith('team:')) {
    // Creating from a team view: default into that team's first shared group so the new
    // snippet inherits the team and syncs immediately.
    defaultGroup = state.groups.find((g) => g.team === state.selected.slice(5))?.id || '';
  } else if (!['all', 'ungrouped', 'settings', 'history'].includes(state.selected)) {
    defaultGroup = state.selected;
  }
  gsel.value = existing?.groupId || defaultGroup;
  $('#e-trigger').value = existing?.trigger || '';
  $('#e-vars').value = existing?.variables ? JSON.stringify(existing.variables, null, 2) : '';
  $('#e-scope').value = existing?.appScope || '';
  if (existing?.appScope) $('#e-scope-adv').open = true;

  const fmtSeg = $('#e-fmt');
  fmtSeg.innerHTML = FORMATS.map((f) => `<button type="button" class="seg-opt" data-fmt="${f.id}">${f.label}</button>`).join('');
  const kindSeg = $('#e-kind');
  kindSeg.innerHTML = KINDS.map((k) => `<button type="button" class="seg-opt" data-kind="${k.id}">${k.label}</button>`).join('');
  const bodyWrap = $('#e-body-wrap');
  const toolbar = $('#e-rich-toolbar');
  const hint = $('#e-fmt-hint');
  const kindHint = $('#e-kind-hint');
  const preview = $('#e-preview');
  const previewFmt = $('#e-preview-fmt');
  const htmlViewSeg = $('#e-htmlview');

  const getRich = () => bodyWrap.querySelector('.rich-editor');
  const readBody = () => {
    if (kind !== 'command' && format === 'html' && htmlView === 'rich') return getRich().innerHTML.trim();
    return bodyWrap.querySelector('#e-body').value;
  };

  function updatePreview() {
    if (kind === 'command') {
      previewFmt.textContent = 'Command';
      preview.className = 'preview-body pv-plain';
      const cmd = readBody().trim();
      preview.textContent = cmd ? `$ ${cmd}\n→ output is pasted` : '';
      if (!cmd) preview.innerHTML = '<span class="pv-empty">Nothing to preview yet.</span>';
      return;
    }
    previewFmt.textContent = FORMATS.find((f) => f.id === format)?.label || '';
    renderPreview(preview, format, readBody());
  }

  function buildBody(fmt, initial) {
    fmtSeg.querySelectorAll('.seg-opt').forEach((b) => b.classList.toggle('active', b.dataset.fmt === fmt));
    hint.textContent = FMT_HINTS[fmt];
    cleanup();
    htmlViewSeg.style.display = kind !== 'command' && fmt === 'html' ? 'inline-flex' : 'none';
    htmlViewSeg.querySelectorAll('.seg-opt').forEach((b) => b.classList.toggle('active', b.dataset.view === htmlView));
    if (kind === 'command') {
      toolbar.style.display = 'none';
      bodyWrap.innerHTML = `<textarea id="e-body" class="body-area mono" placeholder="e.g. date +%Y-%m-%d" spellcheck="false"></textarea>`;
      const ta = bodyWrap.querySelector('#e-body');
      ta.value = initial || '';
      ta.addEventListener('input', updatePreview);
    } else if (fmt === 'html' && htmlView === 'source') {
      toolbar.style.display = 'none';
      bodyWrap.innerHTML = `<textarea id="e-body" class="body-area mono" placeholder="<p>Type HTML…</p>" spellcheck="false"></textarea>`;
      const ta = bodyWrap.querySelector('#e-body');
      ta.value = initial || '';
      ta.addEventListener('input', updatePreview);
    } else if (fmt === 'html') {
      toolbar.style.display = 'flex';
      bodyWrap.innerHTML = `<div class="rich-editor" contenteditable="true"></div>`;
      const ed = getRich();
      ed.innerHTML = sanitizeSnippetHtml(initial || '');
      const refresh = renderRichToolbar(toolbar, getRich, {
        onChange: updatePreview,
        saveSel: () => { const s = window.getSelection(); savedRange = s.rangeCount ? s.getRangeAt(0).cloneRange() : null; },
        restoreSel: () => { if (savedRange) { const s = window.getSelection(); s.removeAllRanges(); s.addRange(savedRange); } },
      });
      ed.addEventListener('input', updatePreview);
      // Pasting an image file inserts it inline (downscaled data URI) — same as the
      // toolbar's Insert image.
      ed.addEventListener('paste', (e) => {
        const file = [...(e.clipboardData?.files || [])].find((f) => f.type.startsWith('image/'));
        if (!file) return;
        e.preventDefault();
        insertImageFile(file, getRich, updatePreview);
      });
      // Clicking an inline image arms the toolbar's resize button (renderRichToolbar).
      selectionHandler = () => { if (document.activeElement === ed) refresh(); };
      document.addEventListener('selectionchange', selectionHandler);
    } else {
      toolbar.style.display = 'none';
      const ph = kind === 'form'
        ? 'Hi {{name}},\nthanks for reaching out…'
        : (fmt === 'markdown' ? 'Type Markdown…' : 'Type your snippet…');
      bodyWrap.innerHTML = `<textarea id="e-body" class="body-area ${fmt === 'markdown' ? 'mono' : ''}" placeholder="${escapeAttr(ph)}"></textarea>`;
      const ta = bodyWrap.querySelector('#e-body');
      ta.value = initial || '';
      ta.addEventListener('input', updatePreview);
    }
    updatePreview();
  }

  // Show/hide the kind-specific rows and relabel the content field.
  function applyKind(initialBody) {
    kindSeg.querySelectorAll('.seg-opt').forEach((b) => b.classList.toggle('active', b.dataset.kind === kind));
    kindHint.textContent = KIND_HINTS[kind];
    $('#e-fmt-row').style.display = kind === 'command' ? 'none' : '';
    $('#e-shell-row').style.display = kind === 'command' ? '' : 'none';
    $('#e-fields-row').style.display = kind === 'form' ? '' : 'none';
    $('#e-adv').style.display = kind === 'text' ? '' : 'none';
    $('#e-group-hint').style.display = kind === 'command' ? '' : 'none';
    modal.querySelector('.content-field label').textContent = kind === 'command' ? 'Command' : 'Content';
    if (kind === 'form') renderFieldBuilder();
    buildBody(format, initialBody);
  }

  // --- form-kind field builder ----------------------------------------------
  function renderFieldBuilder() {
    const box = $('#e-fields');
    box.innerHTML = '';
    formFields.forEach((f, i) => {
      const row = el('div', { className: 'field-row' });
      row.innerHTML = `
        <input type="text" data-k="name" placeholder="firstname" spellcheck="false" />
        <input type="text" data-k="label" placeholder="Label (optional)" />
        <select data-k="type">
          <option value="text">Text</option>
          <option value="multiline">Multiline</option>
          <option value="select">Choices</option>
        </select>
        <input type="text" data-k="options" placeholder="a, b, c" style="display:none" />
        <button type="button" class="ghost sm" data-rm title="Remove field">✕</button>`;
      row.querySelector('[data-k="name"]').value = f.name || '';
      row.querySelector('[data-k="label"]').value = f.label || '';
      row.querySelector('[data-k="type"]').value = f.type || 'text';
      const optInput = row.querySelector('[data-k="options"]');
      optInput.value = Array.isArray(f.options) ? f.options.join(', ') : '';
      optInput.style.display = (f.type === 'select') ? '' : 'none';
      // Refuse the characters a {{placeholder}} can't contain rather than accept them and
      // fail at expansion time. Attached first so the handler below reads the cleaned value.
      const nameInput = row.querySelector('[data-k="name"]');
      nameInput.addEventListener('input', () => {
        const typed = nameInput.value;
        const legal = typed.replace(/[^\w.-]/g, '');
        if (legal === typed) return;
        const caret = (nameInput.selectionStart ?? legal.length) - (typed.length - legal.length);
        nameInput.value = legal;
        nameInput.setSelectionRange(caret, caret);
      });
      row.querySelectorAll('[data-k]').forEach((input) => input.addEventListener('input', () => {
        f.name = row.querySelector('[data-k="name"]').value.trim();
        f.label = row.querySelector('[data-k="label"]').value.trim();
        f.type = row.querySelector('[data-k="type"]').value;
        f.options = optInput.value.split(',').map((s) => s.trim()).filter(Boolean);
        optInput.style.display = (f.type === 'select') ? '' : 'none';
      }));
      row.querySelector('[data-rm]').addEventListener('click', () => {
        formFields.splice(i, 1);
        renderFieldBuilder();
      });
      box.appendChild(row);
    });
  }
  $('#e-field-add').addEventListener('click', () => {
    formFields.push({ name: '', label: '', type: 'text', options: [] });
    renderFieldBuilder();
    $('#e-fields .field-row:last-child [data-k="name"]')?.focus();
  });

  if (kind === 'command' && existing?.variables?.shell) {
    $('#e-shell').value = existing.variables.shell;
  }

  applyKind(existing?.replacement || '');

  fmtSeg.querySelectorAll('.seg-opt').forEach((b) => b.addEventListener('click', () => {
    if (b.dataset.fmt === format) return;
    const converted = convertContent(readBody(), format, b.dataset.fmt);
    format = b.dataset.fmt;
    buildBody(format, converted);
  }));

  kindSeg.querySelectorAll('.seg-opt').forEach((b) => b.addEventListener('click', () => {
    if (b.dataset.kind === kind) return;
    const body = readBody();
    kind = b.dataset.kind;
    applyKind(body);
  }));

  htmlViewSeg.querySelectorAll('.seg-opt').forEach((b) => b.addEventListener('click', () => {
    if (b.dataset.view === htmlView) return;
    const content = readBody();
    htmlView = b.dataset.view;
    buildBody(format, content);
  }));

  // Trigger validation (required + duplicate detection). Triggers are canonically
  // lowercase with no spaces (they're typed inline; the engine matches contiguous
  // keystrokes) — normalise live so what the user sees is what gets saved.
  const triggerInput = $('#e-trigger');
  const triggerErr = $('#e-trigger-err');
  function checkTrigger() {
    const raw = triggerInput.value;
    const normalized = raw.replace(/\s+/g, '').toLowerCase();
    if (raw !== normalized) {
      const pos = triggerInput.selectionStart;
      triggerInput.value = normalized;
      const drop = raw.length - normalized.length;
      triggerInput.setSelectionRange(Math.max(0, pos - drop), Math.max(0, pos - drop));
    }
    const t = normalized;
    const conflict = t && state.snippets.find((s) => s.id !== existing?.id && s.trigger === t);
    triggerErr.textContent = conflict ? 'Another snippet already uses this trigger.' : '';
    return !conflict;
  }
  triggerInput.addEventListener('input', checkTrigger);

  async function save() {
    checkTrigger(); // normalise even if the user never typed after paste/prefill
    const trigger = triggerInput.value.trim();
    const replacement = readBody();
    const bodyErr = $('#e-body-err');
    const varsErr = $('#e-vars-err');
    const fieldsErr = $('#e-fields-err');
    triggerErr.textContent = ''; bodyErr.textContent = ''; varsErr.textContent = '';
    fieldsErr.textContent = '';
    let ok = true;
    if (!trigger) { triggerErr.textContent = 'Trigger is required.'; ok = false; }
    else if (!checkTrigger()) { ok = false; }
    if (!replacement.trim()) {
      bodyErr.textContent = kind === 'command' ? 'Command is required.' : 'Content is required.';
      ok = false;
    }
    let variables = null;
    if (kind === 'text') {
      const vr = $('#e-vars').value.trim();
      if (vr) {
        try { variables = JSON.parse(vr); }
        catch { varsErr.textContent = 'Variables must be valid JSON.'; $('#e-adv').open = true; ok = false; }
      }
    } else if (kind === 'form') {
      const problems = formFieldProblems(formFields, replacement);
      if (problems.length) {
        fieldsErr.textContent = problems[0];
        if (problems.length > 1) fieldsErr.textContent += ` (+${problems.length - 1} more)`;
        ok = false;
      }
      const cleaned = formFields
        .filter((f) => f.name)
        .map((f) => ({
          name: f.name,
          ...(f.label ? { label: f.label } : {}),
          type: f.type || 'text',
          ...(f.type === 'select' && f.options?.length ? { options: f.options } : {}),
        }));
      if (cleaned.length) variables = { fields: cleaned };
    } else if (kind === 'command') {
      const sh = $('#e-shell').value;
      if (sh && sh !== 'sh') variables = { shell: sh };
    }
    if (!ok) return;
    const payload = {
      trigger, replacement, variables, kind,
      format: kind === 'command' ? 'plain' : format,
      enabled: existing ? existing.enabled !== false : true,
      groupId: gsel.value || null,
      appScope: $('#e-scope').value.trim() || null,
      // Preserve sync scope on edit (team assignment is managed by the Sync section).
      // The store forces command snippets personal regardless.
      team: existing?.team || null,
    };
    try {
      if (isEdit) await invoke('update_snippet', { id: existing.id, patch: payload });
      else await invoke('create_snippet', { snippet: payload });
      close();
      setStatus(isEdit ? 'Snippet saved.' : 'Snippet created.', 'ok');
    } catch (e) { setStatus(String(e), 'err'); }
  }

  $('#e-save').addEventListener('click', save);
  $('#e-cancel').addEventListener('click', close);
  $('#e-close').addEventListener('click', close);
  modal.addEventListener('keydown', (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') { e.preventDefault(); save(); }
  });
  triggerInput.focus();
  checkTrigger();
}

// What a form field's name may contain — the same characters `form.js` will substitute for.
// Anything else (a space, most often) can never match a {{placeholder}}, so the form would
// collect the value and then quietly drop it.
const FIELD_NAME_RE = /^[\w.-]+$/;

/// Everything wrong with a form snippet's fields, in the order worth fixing.
///
/// A form whose fields are never referenced is not a subtle problem — it collects input and
/// pastes the template untouched — but it fails at expansion time, in a window that has
/// already closed, so nothing points at the cause. These are the checks that would have.
function formFieldProblems(fields, body) {
    const text = String(body || '');
    const problems = [];
    const named = fields.filter((f) => f.name);

    for (const f of named) {
        if (!FIELD_NAME_RE.test(f.name)) {
            problems.push(
                `Field name “${f.name}” can't be used: names take letters, digits, _ . and - ` +
                `(no spaces). Try “${f.name.replace(/[^\w.-]+/g, '')}”.`,
            );
        }
    }
    if (problems.length) return problems; // fix the names before judging the content

    const used = new Set(
        [...text.matchAll(/\{\{\s*([\w.-]+)\s*\}\}/g)].map((m) => m[1]),
    );
    const missing = named.filter((f) => !used.has(f.name));
    for (const f of missing) {
        // The single-brace slip is worth calling out by name: it looks right, and the label
        // is the obvious thing to reach for even though the name is what substitutes.
        const singles = [f.name, f.label].filter(Boolean)
            .filter((s) => text.includes(`{${s}}`));
        problems.push(
            singles.length
                ? `Content has “{${singles[0]}}” — placeholders need double braces. ` +
                  `Write “{{${f.name}}}”.`
                : `Field “${f.name}” isn't used in the content — add “{{${f.name}}}”, ` +
                  `or remove the field.`,
        );
    }
    return problems;
}

// Rich-text toolbar. Returns a `refresh()` that syncs button active-states to the current selection.
function renderRichToolbar(bar, getEditor, { onChange, saveSel, restoreSel }) {
  bar.innerHTML = '';
  const stateBtns = [];
  const exec = (command, value) => { getEditor().focus(); document.execCommand(command, false, value); onChange(); refresh(); };
  const mk = (label, title, onClick, cmdName) => {
    const b = el('button', { type: 'button', className: 'tb-btn', title, innerHTML: label });
    if (cmdName) { b.dataset.cmd = cmdName; stateBtns.push(b); }
    b.addEventListener('mousedown', (e) => e.preventDefault()); // keep the editor selection
    b.addEventListener('click', (e) => { e.preventDefault(); onClick(b); });
    bar.append(b);
    return b;
  };

  mk('<b>B</b>', 'Bold (⌘B)', () => exec('bold'), 'bold');
  mk('<i>I</i>', 'Italic (⌘I)', () => exec('italic'), 'italic');
  mk('<u>U</u>', 'Underline (⌘U)', () => exec('underline'), 'underline');
  mk('H', 'Heading', () => exec('formatBlock', 'H3'));
  mk(icon('list'), 'Bullet list', () => exec('insertUnorderedList'), 'insertUnorderedList');
  mk(icon('listOrdered'), 'Numbered list', () => exec('insertOrderedList'), 'insertOrderedList');
  mk(icon('code'), 'Code block', () => exec('formatBlock', 'PRE'));
  mk(icon('link'), 'Insert link', (b) => openLinkPopover(b));
  mk(icon('table'), 'Insert table', (b) => openTablePopover(b));
  mk(icon('image'), 'Insert image', () => pickImage());
  const sizeBtn = mk(icon('resize'), 'Resize image — select one first', (b) => {
    if (selectedImage()) openImageSizePopover(selectedImage(), onChange, b);
  });
  mk(icon('eraser'), 'Clear formatting', () => exec('removeFormat'));

  // Image sizing belongs here, alongside every other formatting control. It used to be a
  // pill that appeared only while hovering the image, which people didn't find at all.
  // Clicking an image arms this button; the editor's hover outline hints it's selectable.
  let picked = null;
  function selectedImage() {
    // The image may have been deleted since it was picked.
    return picked && getEditor().contains(picked) ? picked : null;
  }
  function armSizeButton(img) {
    picked = img;
    const on = !!selectedImage();
    sizeBtn.disabled = !on;
    sizeBtn.classList.toggle('active', on);
    sizeBtn.title = on ? 'Resize this image' : 'Resize image — select one first';
  }
  armSizeButton(null);
  {
    const ed = getEditor();
    ed.addEventListener('click', (e) => armSizeButton(e.target.closest('img')));
    // Typing moves the point of interest away from the image.
    ed.addEventListener('input', () => armSizeButton(null));
  }

  function pickImage() {
    const input = el('input', { type: 'file', accept: 'image/*' });
    input.addEventListener('change', () => {
      const file = input.files?.[0];
      if (file) insertImageFile(file, getEditor, onChange);
    });
    input.click();
  }

  function openLinkPopover(anchor) {
    saveSel();
    openPopover(anchor, `
      <div class="pop-row"><input type="url" data-url placeholder="https://…" /></div>
      <div class="pop-actions"><button class="secondary sm" data-cancel>Cancel</button><button class="primary sm" data-ok>Insert</button></div>`,
      ({ root, close }) => {
        const url = root.querySelector('[data-url]');
        url.focus();
        const insert = () => {
          const v = url.value.trim();
          if (!v) { url.focus(); return; }
          restoreSel(); getEditor().focus();
          const sel = window.getSelection();
          if (sel && sel.toString()) exec('createLink', v);
          else exec('insertHTML', `<a href="${escapeAttr(v)}">${escapeHtml(v)}</a>`);
          close();
        };
        root.querySelector('[data-ok]').addEventListener('click', insert);
        root.querySelector('[data-cancel]').addEventListener('click', close);
        url.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); insert(); } });
      });
  }

  function openTablePopover(anchor) {
    saveSel();
    openPopover(anchor, `
      <div class="pop-row pop-grid">
        <label>Rows<input type="number" data-rows min="1" max="20" value="2" /></label>
        <label>Cols<input type="number" data-cols min="1" max="20" value="2" /></label>
      </div>
      <div class="pop-actions"><button class="secondary sm" data-cancel>Cancel</button><button class="primary sm" data-ok>Insert</button></div>`,
      ({ root, close }) => {
        const clamp = (n) => Math.max(1, Math.min(20, parseInt(n, 10) || 1));
        const insert = () => {
          const r = clamp(root.querySelector('[data-rows]').value);
          const c = clamp(root.querySelector('[data-cols]').value);
          restoreSel(); getEditor().focus();
          exec('insertHTML', tableHtml(r, c));
          close();
        };
        root.querySelector('[data-ok]').addEventListener('click', insert);
        root.querySelector('[data-cancel]').addEventListener('click', close);
        root.querySelector('[data-rows]').focus();
      });
  }

  function refresh() {
    for (const b of stateBtns) {
      let on = false;
      try { on = document.queryCommandState(b.dataset.cmd); } catch { /* unsupported */ }
      b.classList.toggle('active', on);
    }
  }
  return refresh;
}

// --- Inline images (data URIs) ----------------------------------------------
// Images live inside the HTML body as data URIs — they sync/export with the snippet and
// render in the popup/form windows and on rich paste. Downscaled on insert so a snippet
// stays well under the 1 MB sync cap.

const IMG_MAX_EDGE = 1600;
const IMG_TARGET_BYTES = 300 * 1024;

async function insertImageFile(file, getEditor, onChange) {
  try {
    const dataUrl = await downscaleImageToDataUrl(file);
    const ed = getEditor();
    ed.focus();
    const marker = `img-${Date.now()}`;
    document.execCommand('insertHTML', false, `<img src="${dataUrl}" alt="" data-new="${marker}">`);
    onChange?.();
    // Open the size popover right away — the moment of insertion is when the user is
    // thinking about size, and it teaches that images are click-to-resize.
    const img = ed.querySelector(`img[data-new="${marker}"]`);
    if (img) {
      img.removeAttribute('data-new'); // keep saved HTML clean
      const openPop = () => openImageSizePopover(img, onChange);
      if (img.complete) openPop();
      else img.addEventListener('load', openPop, { once: true });
    }
  } catch (e) {
    setStatus(`Could not insert image: ${e.message || e}`, 'err');
  }
}

/// Resize an inline snippet image: a popover with a width slider + presets, anchored to the
/// toolbar button that opened it (or to the image itself, right after inserting one). Width
/// is stored as an absolute `width` attribute (height auto-follows), which survives
/// sanitization, the popup/form surfaces, and rich paste into target apps.
function openImageSizePopover(img, onChange, anchor = img) {
  const natural = img.naturalWidth || parseInt(img.getAttribute('width'), 10) || 0;
  if (!natural) return;
  const current = parseInt(img.getAttribute('width'), 10) || natural;
  const pct = Math.max(10, Math.min(100, Math.round((current / natural) * 100)));
  openPopover(anchor, `
    <div class="pop-row img-size-row">
      <input type="range" data-size min="10" max="100" step="5" value="${pct}" aria-label="Image width" />
      <span class="img-size-val" data-val>${pct}%</span>
    </div>
    <div class="pop-actions">
      <button class="secondary sm" data-preset="25" type="button">S</button>
      <button class="secondary sm" data-preset="50" type="button">M</button>
      <button class="secondary sm" data-preset="75" type="button">L</button>
      <button class="secondary sm" data-preset="100" type="button">Full</button>
    </div>`,
    ({ root }) => {
      const slider = root.querySelector('[data-size]');
      const val = root.querySelector('[data-val]');
      const apply = (p) => {
        slider.value = String(p);
        val.textContent = `${p}%`;
        if (p >= 100) { img.removeAttribute('width'); }
        else { img.setAttribute('width', Math.max(1, Math.round((natural * p) / 100))); }
        img.removeAttribute('height'); // keep aspect ratio
        onChange?.();
      };
      slider.addEventListener('input', () => apply(parseInt(slider.value, 10)));
      root.querySelectorAll('[data-preset]').forEach((b) =>
        b.addEventListener('click', () => apply(parseInt(b.dataset.preset, 10))));
    });
}

async function downscaleImageToDataUrl(blob) {
  const bmp = await createImageBitmap(blob);
  const scale = Math.min(1, IMG_MAX_EDGE / Math.max(bmp.width, bmp.height));
  const w = Math.max(1, Math.round(bmp.width * scale));
  const h = Math.max(1, Math.round(bmp.height * scale));
  const canvas = el('canvas', { width: w, height: h });
  canvas.getContext('2d').drawImage(bmp, 0, 0, w, h);
  bmp.close?.();
  // PNG keeps screenshots/text crisp (and transparency); step down through JPEG
  // qualities only when the PNG would blow past the target size.
  let out = canvas.toDataURL('image/png');
  const budget = IMG_TARGET_BYTES * (4 / 3); // data-URI base64 overhead
  if (out.length > budget) {
    for (const q of [0.85, 0.7, 0.55]) {
      out = canvas.toDataURL('image/jpeg', q);
      if (out.length <= budget) break;
    }
  }
  return out;
}

function tableHtml(rows, cols) {
  const cell = 'style="border:1px solid #888;padding:4px 8px"';
  let out = '<table style="border-collapse:collapse"><tbody>';
  for (let r = 0; r < rows; r++) {
    out += '<tr>';
    for (let c = 0; c < cols; c++) out += `<td ${cell}>&nbsp;</td>`;
    out += '</tr>';
  }
  return out + '</tbody></table><p></p>';
}

// --- Live preview + format conversion --------------------------------------

function renderPreview(container, fmt, content) {
  if (!content || !content.trim()) {
    container.className = 'preview-body';
    container.innerHTML = '<span class="pv-empty">Nothing to preview yet.</span>';
    return;
  }
  if (fmt === 'plain') {
    container.className = 'preview-body pv-plain';
    container.textContent = content;
  } else {
    container.className = 'preview-body pv-rich';
    // Sanitized: html bodies can arrive from teammates via sync, and this webview holds IPC.
    container.innerHTML = fmt === 'markdown' ? mdToHtml(content) : sanitizeSnippetHtml(content);
  }
}

// Convert content when the user switches format, so switching never leaves raw markup behind.
function convertContent(content, from, to) {
  if (from === to || !content) return content;
  if (from === 'plain' && to === 'html') return textToHtml(content);
  if (from === 'plain' && to === 'markdown') return content;
  if (from === 'markdown' && to === 'plain') return mdToText(content);
  if (from === 'markdown' && to === 'html') return mdToHtml(content);
  if (from === 'html' && to === 'plain') return htmlToText(content);
  if (from === 'html' && to === 'markdown') return htmlToMarkdown(content);
  return content;
}

function textToHtml(t) { return escapeHtml(t).replace(/\n/g, '<br>'); }

function htmlToText(html) {
  const tmp = el('div');
  tmp.innerHTML = html
    .replace(/<\s*br\s*\/?>/gi, '\n')
    .replace(/<li[^>]*>/gi, '• ')
    .replace(/<\/(p|div|h[1-6]|li|tr)>/gi, '\n');
  return (tmp.textContent || '').replace(/\n{3,}/g, '\n\n').trim();
}

// Markdown rendering, HTML escaping, and snippet-HTML sanitization live in
// ../shared/markdown.js (shared with the popup/form surfaces).

function mdToText(md) {
  return md
    .replace(/^#{1,6}\s+/gm, '')
    .replace(/\*\*([^*]+)\*\*/g, '$1')
    .replace(/\*([^*]+)\*/g, '$1')
    .replace(/`([^`]+)`/g, '$1')
    .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')
    .replace(/^\s*[-*]\s+/gm, '• ')
    .trim();
}

function htmlToMarkdown(html) {
  const s = html
    .replace(/<\s*br\s*\/?>/gi, '\n')
    .replace(/<(strong|b)>([\s\S]*?)<\/\1>/gi, '**$2**')
    .replace(/<(em|i)>([\s\S]*?)<\/\1>/gi, '*$2*')
    .replace(/<code>([\s\S]*?)<\/code>/gi, '`$1`')
    .replace(/<a[^>]*href="([^"]*)"[^>]*>([\s\S]*?)<\/a>/gi, '[$2]($1)')
    .replace(/<h([1-6])>([\s\S]*?)<\/h\1>/gi, (m, l, t) => `\n${'#'.repeat(+l)} ${t}\n`)
    .replace(/<li[^>]*>([\s\S]*?)<\/li>/gi, '- $1\n')
    .replace(/<\/(p|div|ul|ol)>/gi, '\n');
  const tmp = el('div');
  tmp.innerHTML = s;
  return (tmp.textContent || '').replace(/\n{3,}/g, '\n\n').trim();
}
