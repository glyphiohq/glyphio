// The editor's durable state: content pixels, immutable capture metadata, and banner state.
// Tool interactions remain page concerns; each accepted edit replaces `content` here.
export class EditableCaptureArtifact {
  #content;
  #metadata;
  #banner;

  constructor({ content, metadata, banner = {} }) {
    this.#content = content;
    this.#metadata = { ...metadata };
    this.#banner = {
      note: banner.note || '',
      enabled: banner.enabled !== false,
      baked: Boolean(banner.baked),
    };
  }

  replaceContent(content) {
    this.#content = content;
  }

  setBanner({ note = this.#banner.note, enabled = this.#banner.enabled, baked = this.#banner.baked }) {
    this.#banner = { note, enabled, baked };
  }

  render(renderer) {
    return renderer(this.#snapshot());
  }

  persistence() {
    return this.#snapshot();
  }

  #snapshot() {
    return {
      content: this.#content,
      metadata: { ...this.#metadata },
      banner: { ...this.#banner },
    };
  }
}
