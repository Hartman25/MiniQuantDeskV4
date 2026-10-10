/** Only the latest requested observation may publish, including failure state. */
export class LatestRequest {
  private generation = 0;
  private pending = false;
  get busy() { return this.pending; }
  invalidate() { this.generation++; this.pending = false; }
  async run<T>(load: () => Promise<T>, accept: (value: T) => void, fail: (error: unknown) => void) {
    const generation = ++this.generation;
    this.pending = true;
    try {
      const value = await load();
      if (generation === this.generation) accept(value);
    } catch (error) {
      if (generation === this.generation) fail(error);
    } finally {
      if (generation === this.generation) this.pending = false;
    }
  }
}
