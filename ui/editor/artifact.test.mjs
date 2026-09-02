import assert from 'node:assert/strict';
import test from 'node:test';

import { EditableCaptureArtifact } from './artifact.mjs';

test('renders and persists the same edited artifact state', () => {
  const originalContent = { width: 800, height: 600, label: 'original' };
  const editedContent = { width: 640, height: 480, label: 'cropped' };
  const artifact = new EditableCaptureArtifact({
    content: originalContent,
    metadata: { capturedAt: '2026-09-02T12:00:00.000Z', mode: 'visible' },
    banner: { note: 'before', enabled: true },
  });

  artifact.replaceContent(editedContent);
  artifact.setBanner({ note: 'after', enabled: false });

  const rendered = artifact.render(({ content, metadata, banner }) => ({
    label: content.label,
    timestamp: metadata.capturedAt,
    banner: `${banner.enabled}:${banner.note}`,
  }));

  assert.deepEqual(rendered, {
    label: 'cropped',
    timestamp: '2026-09-02T12:00:00.000Z',
    banner: 'false:after',
  });
  assert.deepEqual(artifact.persistence(), {
    content: editedContent,
    metadata: { capturedAt: '2026-09-02T12:00:00.000Z', mode: 'visible' },
    banner: { note: 'after', enabled: false, baked: false },
  });
});

test('does not let a renderer mutate the persisted banner state', () => {
  const artifact = new EditableCaptureArtifact({
    content: { width: 1, height: 1 },
    metadata: { capturedAt: '2026-09-02T12:00:00.000Z' },
    banner: { note: 'original', enabled: true },
  });

  artifact.render((state) => {
    state.banner.note = 'stale renderer';
  });

  assert.equal(artifact.persistence().banner.note, 'original');
});
