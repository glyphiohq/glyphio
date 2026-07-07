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

export function hasAnyBannerContent(settings, meta, note) {
  return Boolean(settings.showTimestamp || settings.showUrl || showFrameUrlLine(settings, meta) || note);
}

export function computeBannerCssHeight(settings, meta, note) {
  if (!hasAnyBannerContent(settings, meta, note)) return 0;
  const b = config.banner;
  let h = b.paddingPx;
  if (settings.showTimestamp) h += b.timestampFontPx;
  if (settings.showUrl) {
    if (settings.showTimestamp) h += b.lineGapPx;
    h += b.urlFontPx;
  }
  if (showFrameUrlLine(settings, meta)) {
    if (settings.showTimestamp || settings.showUrl) h += b.lineGapPx;
    h += b.urlFontPx;
  }
  if (note) {
    if (settings.showTimestamp || settings.showUrl || showFrameUrlLine(settings, meta)) h += b.lineGapPx;
    h += b.noteFontPx;
  }
  h += b.paddingPx;
  return h;
}

export function formatTimestamp(iso, settings) {
  const d = new Date(iso);
  const locale = settings.locale === 'device' ? undefined : settings.locale;
  const tz = settings.timezone === 'device' ? undefined : settings.timezone;

  switch (settings.timestampFormat) {
    case 'iso-8601':
      if (settings.timezone === 'device') {
        return d.toISOString().replace('T', ' ').replace(/\.\d+Z$/, 'Z');
      }
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

  if (settings.showTimestamp) {
    ctx.font = `bold ${b.timestampFontPx * scale}px ${b.fontFamily}`;
    ctx.fillStyle = settings.bannerFg;
    ctx.fillText(formatTimestamp(meta.capturedAt, settings), b.paddingPx * scale, y);
    y += b.timestampFontPx * scale;
    prevLineExists = true;
  }

  if (settings.showUrl) {
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
    drawBannerLines(ctx, target.width, scale, settings, meta, note);
  }
  ctx.drawImage(content, 0, bannerPxH);
  return bannerPxH;
}
