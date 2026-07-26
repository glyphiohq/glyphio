// shared/banner.js — banner composition shared by the editor window and the history list.
// Stored capture PNGs are content-only; the banner (timestamp + window title + note) is
// composited at view/export time, always from the row's immutable capturedAt — so the
// banner can be added or removed later without ever losing the original capture time.

import { config } from '../config.js';

function showFrameUrlLine(settings, meta) {
  return settings.showTargetFrameUrl
    && meta.targetFrameUrl
    && meta.targetFrameUrl !== meta.url;
}

// A capture with no window title (a snipped region is the common one) has nothing to put on
// the URL line — reserving it would leave an empty band under the timestamp.
function showUrlLine(settings, meta) {
  return Boolean(settings.showUrl && (meta.url || '').trim());
}

export function hasAnyBannerContent(settings, meta, note) {
  return Boolean(settings.showTimestamp || showUrlLine(settings, meta) || showFrameUrlLine(settings, meta) || note);
}

export function computeBannerCssHeight(settings, meta, note) {
  if (!hasAnyBannerContent(settings, meta, note)) return 0;
  const b = config.banner;
  const url = showUrlLine(settings, meta);
  const frame = showFrameUrlLine(settings, meta);
  let h = b.paddingPx;
  if (settings.showTimestamp) h += b.timestampFontPx;
  if (url) {
    if (settings.showTimestamp) h += b.lineGapPx;
    h += b.urlFontPx;
  }
  if (frame) {
    if (settings.showTimestamp || url) h += b.lineGapPx;
    h += b.urlFontPx;
  }
  if (note) {
    if (settings.showTimestamp || url || frame) h += b.lineGapPx;
    h += b.noteFontPx;
  }
  h += b.paddingPx;
  return h;
}

/**
 * Is this something `Intl` will actually accept? A timezone or locale it rejects throws out
 * of the formatter, and that used to take the whole banner down with it — one typo in the
 * timezone box and captures came out with no timestamp, no title, nothing. Settings offers a
 * list rather than a text box now, but a value can still arrive from an older install or a
 * hand-edited settings.json, so nothing here is trusted.
 */
const intlSupport = new Map();
function accepts(key, build) {
  if (intlSupport.has(key)) return intlSupport.get(key);
  let ok = true;
  try { build(); } catch { ok = false; }
  intlSupport.set(key, ok);
  return ok;
}

export function isSupportedTimezone(tz) {
  if (!tz || tz === 'device') return true;
  return accepts(`tz:${tz}`, () => new Intl.DateTimeFormat('en-GB', { timeZone: tz }).format(new Date()));
}

export function isSupportedLocale(locale) {
  if (!locale || locale === 'device') return true;
  return accepts(`loc:${locale}`, () => new Intl.DateTimeFormat(locale).format(new Date()));
}

/** The locale/timezone to actually format with — `undefined` means "whatever the Mac uses". */
function intlPrefs(settings) {
  const locale = settings.locale && settings.locale !== 'device' && isSupportedLocale(settings.locale)
    ? settings.locale : undefined;
  const timeZone = settings.timezone && settings.timezone !== 'device' && isSupportedTimezone(settings.timezone)
    ? settings.timezone : undefined;
  return { locale, timeZone };
}

export function formatTimestamp(iso, settings) {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '';
  const { locale, timeZone: tz } = intlPrefs(settings);

  switch (settings.timestampFormat) {
    case 'iso-8601':
      if (!tz) return d.toISOString().replace('T', ' ').replace(/\.\d+Z$/, 'Z');
      return formatIsoInTz(d, tz);
    case 'utc-human':
      return new Intl.DateTimeFormat('en-GB', {
        dateStyle: 'medium', timeStyle: 'long', timeZone: 'UTC'
      }).format(d);
    case 'device-locale':
    default:
      return new Intl.DateTimeFormat(locale, {
        dateStyle: 'medium', timeStyle: 'long', timeZone: tz
      }).format(d);
  }
}

/**
 * The timestamp at decreasing lengths, longest first. A narrow capture — a snipped region is
 * usually the narrowest thing anyone captures — sheds precision rather than the date itself:
 * losing the timezone or the seconds still leaves a legible record, whereas the fixed-size
 * text simply ran off the edge and showed nothing usable.
 */
function timestampVariants(iso, settings) {
  const d = new Date(iso);
  const { locale, timeZone: tz } = intlPrefs(settings);
  const full = formatTimestamp(iso, settings);
  const fmt = (opts, loc = locale) => {
    try {
      return new Intl.DateTimeFormat(loc, { timeZone: tz, ...opts }).format(d);
    } catch {
      return full;
    }
  };
  switch (settings.timestampFormat) {
    case 'iso-8601':
      return [full, full.replace(/:\d{2}(Z|\s|$)/, '$1'), full.slice(0, 16)];
    case 'utc-human':
      return [
        full,
        new Intl.DateTimeFormat('en-GB', { dateStyle: 'medium', timeStyle: 'short', timeZone: 'UTC' }).format(d) + ' UTC',
        new Intl.DateTimeFormat('en-GB', { dateStyle: 'short', timeStyle: 'short', timeZone: 'UTC' }).format(d) + ' UTC',
      ];
    case 'device-locale':
    default:
      return [
        full,
        fmt({ dateStyle: 'medium', timeStyle: 'short' }),
        fmt({ dateStyle: 'short', timeStyle: 'short' }),
      ];
  }
}

/**
 * Draw `variants[0]`, or the first shorter one that fits, or the shortest shrunk down to
 * `minPx` — and only truncate as a last resort. Returns nothing; sets the font on `ctx`.
 */
function fitTimestamp(ctx, variants, maxW, nominalPx, fontFamily, scale) {
  const minPx = 11 * scale;
  const setFont = (px) => { ctx.font = `bold ${px}px ${fontFamily}`; };
  for (const text of variants) {
    setFont(nominalPx);
    if (ctx.measureText(text).width <= maxW) return text;
  }
  const shortest = variants[variants.length - 1];
  for (let px = nominalPx - scale; px >= minPx; px -= scale) {
    setFont(px);
    if (ctx.measureText(shortest).width <= maxW) return shortest;
  }
  setFont(minPx);
  return truncateToWidth(ctx, shortest, maxW);
}

function formatIsoInTz(d, tz) {
  const parts = new Intl.DateTimeFormat('en-GB', {
    timeZone: tz, year: 'numeric', month: '2-digit', day: '2-digit',
    hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false
  }).formatToParts(d).reduce((acc, p) => (acc[p.type] = p.value, acc), {});
  return `${parts.year}-${parts.month}-${parts.day} ${parts.hour}:${parts.minute}:${parts.second} ${tz}`;
}

function truncateToWidth(ctx, text, maxW) {
  if (ctx.measureText(text).width <= maxW) return text;
  const ellipsis = '…';
  let lo = 0, hi = text.length;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (ctx.measureText(text.slice(0, mid) + ellipsis).width <= maxW) lo = mid;
    else hi = mid - 1;
  }
  return text.slice(0, lo) + ellipsis;
}

function drawBannerLines(ctx, canvasWidth, scale, settings, meta, note) {
  const b = config.banner;
  ctx.textBaseline = 'top';
  let y = b.paddingPx * scale;
  let prevLineExists = false;

  const avail = canvasWidth - 2 * b.paddingPx * scale;

  if (settings.showTimestamp) {
    // fitTimestamp picks the variant/size that fits and leaves it set on the context.
    const text = fitTimestamp(
      ctx,
      timestampVariants(meta.capturedAt, settings),
      avail,
      b.timestampFontPx * scale,
      b.fontFamily,
      scale,
    );
    ctx.fillStyle = settings.bannerFg;
    ctx.fillText(text, b.paddingPx * scale, y);
    y += b.timestampFontPx * scale;
    prevLineExists = true;
  }

  if (showUrlLine(settings, meta)) {
    if (prevLineExists) y += b.lineGapPx * scale;
    ctx.font = `${b.urlFontPx * scale}px ${b.fontFamily}`;
    ctx.fillStyle = settings.bannerMuted;
    ctx.fillText(
      truncateToWidth(ctx, meta.url || '', canvasWidth - 2 * b.paddingPx * scale),
      b.paddingPx * scale,
      y
    );
    y += b.urlFontPx * scale;
    prevLineExists = true;
  }

  if (showFrameUrlLine(settings, meta)) {
    if (prevLineExists) y += b.lineGapPx * scale;
    ctx.font = `${b.urlFontPx * scale}px ${b.fontFamily}`;
    ctx.fillStyle = settings.bannerMuted;
    ctx.fillText(
      `↳ ${truncateToWidth(ctx, meta.targetFrameUrl, canvasWidth - 2 * b.paddingPx * scale - 20)}`,
      b.paddingPx * scale,
      y
    );
    y += b.urlFontPx * scale;
    prevLineExists = true;
  }

  if (note) {
    if (prevLineExists) y += b.lineGapPx * scale;
    ctx.font = `${b.noteFontPx * scale}px ${b.fontFamily}`;
    ctx.fillStyle = settings.bannerFg;
    ctx.fillText(
      truncateToWidth(ctx, note, canvasWidth - 2 * b.paddingPx * scale),
      b.paddingPx * scale,
      y
    );
  }
}

/**
 * Composite `content` (a canvas or ImageBitmap) onto `target`, with the banner above it
 * when `enabled` and there is anything to show. Returns the banner height in pixels.
 * `meta` needs { capturedAt, url, targetFrameUrl?, dpr }.
 */
export function compositeBanner(target, content, { meta, settings, note = '', enabled = true }) {
  const scale = meta.dpr || 1;
  const bannerCssH = enabled ? computeBannerCssHeight(settings, meta, note) : 0;
  const bannerPxH = Math.round(bannerCssH * scale);

  target.width = content.width;
  target.height = bannerPxH + content.height;
  const ctx = target.getContext('2d');

  if (bannerPxH > 0) {
    ctx.fillStyle = settings.bannerBg;
    ctx.fillRect(0, 0, target.width, bannerPxH);
    // Nothing about formatting text is worth losing a capture over: whatever happens up
    // here, the image below still gets drawn.
    try {
      drawBannerLines(ctx, target.width, scale, settings, meta, note);
    } catch (e) {
      console.error('banner text could not be drawn', e);
    }
  }
  ctx.drawImage(content, 0, bannerPxH);
  return bannerPxH;
}
