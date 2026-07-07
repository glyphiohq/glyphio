// shared/markdown.js — HTML escaping, minimal Markdown rendering, and HTML sanitization
// shared by the snippet manager (live preview) and the popup/form surfaces.

export function escapeHtml(s) {
  return String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
}

export function escapeAttr(s) {
  return escapeHtml(s).replace(/'/g, '&#39;');
}

// Minimal, approximate Markdown → HTML — for previews and popup rendering only. The engine
// itself does the real (pulldown-cmark) conversion at expansion time.
export function mdToHtml(md) {
  const lines = md.replace(/\r\n?/g, '\n').split('\n');
  const inline = (s) => escapeHtml(s)
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
    .replace(/(^|[^*])\*([^*]+)\*/g, '$1<em>$2</em>')
    .replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2">$1</a>');
  let html = '', inUl = false, inOl = false, inCode = false;
  const closeLists = () => { if (inUl) { html += '</ul>'; inUl = false; } if (inOl) { html += '</ol>'; inOl = false; } };
  for (const line of lines) {
    if (/^```/.test(line)) { closeLists(); inCode = !inCode; html += inCode ? '<pre><code>' : '</code></pre>'; continue; }
    if (inCode) { html += escapeHtml(line) + '\n'; continue; }
    const h = line.match(/^(#{1,6})\s+(.*)$/);
    if (h) { closeLists(); html += `<h${h[1].length}>${inline(h[2])}</h${h[1].length}>`; continue; }
    const ul = line.match(/^\s*[-*]\s+(.*)$/);
    if (ul) { if (!inUl) { closeLists(); html += '<ul>'; inUl = true; } html += `<li>${inline(ul[1])}</li>`; continue; }
    const ol = line.match(/^\s*\d+\.\s+(.*)$/);
    if (ol) { if (!inOl) { closeLists(); html += '<ol>'; inOl = true; } html += `<li>${inline(ol[1])}</li>`; continue; }
    if (line.trim() === '') { closeLists(); continue; }
    closeLists(); html += `<p>${inline(line)}</p>`;
  }
  closeLists(); if (inCode) html += '</code></pre>';
  return html;
}

/**
 * Sanitize snippet HTML before it lands in a live webview via innerHTML. Snippet bodies can
 * come from teammates over sync, and Glyphio webviews hold Tauri IPC access — so scripts,
 * event handlers, and frame-ish elements are stripped. `data:` image sources stay (that's
 * how snippet images ship); `javascript:` URLs do not.
 */
export function sanitizeSnippetHtml(html) {
  const doc = new DOMParser().parseFromString(html, 'text/html');
  for (const node of doc.querySelectorAll('script, iframe, object, embed, link, meta, base, form')) {
    node.remove();
  }
  for (const node of doc.body ? doc.body.querySelectorAll('*') : []) {
    for (const attr of [...node.attributes]) {
      const name = attr.name.toLowerCase();
      const value = attr.value.trim().toLowerCase();
      if (name.startsWith('on')) node.removeAttribute(attr.name);
      else if ((name === 'href' || name === 'src' || name === 'xlink:href') && value.startsWith('javascript:')) {
        node.removeAttribute(attr.name);
      }
    }
  }
  return doc.body ? doc.body.innerHTML : '';
}
