// Tracks the files an agent run touches, so we can show a changes summary and
// offer a one-click "restore to before this run" (per-task undo). A run is one
// user turn through the agent loop; the changeset accumulates every write the
// agent makes, keyed by absolute path, remembering the FIRST before-content
// seen (so restoring reverts the whole run, not just the last edit).

export interface FileChange {
  abs: string;
  rel: string;
  before: string | null; // null = file did not exist before the run
  after: string;          // latest content the agent wrote
}

export interface ChangeStats {
  files: number;
  added: number;   // lines added (net per file, summed)
  removed: number; // lines removed
}

export class RunChanges {
  private map = new Map<string, FileChange>();

  /** Record one write. `before` is captured only the first time a path is seen
   *  this run (its true pre-run state); later writes only update `after`. */
  record(abs: string, rel: string, before: string | null, after: string): void {
    const existing = this.map.get(abs);
    if (existing) {
      existing.after = after;
    } else {
      this.map.set(abs, { abs, rel, before, after });
    }
  }

  clear(): void { this.map.clear(); }
  get size(): number { return this.map.size; }
  list(): FileChange[] { return [...this.map.values()]; }

  /** Net line add/remove counts across all changed files (rough, line-based). */
  stats(): ChangeStats {
    let added = 0, removed = 0;
    for (const c of this.map.values()) {
      const b = c.before === null ? [] : c.before.split('\n');
      const a = c.after.split('\n');
      const d = a.length - b.length;
      if (d > 0) added += d; else removed += -d;
      // Count changed lines within the common range as add+remove too, cheaply.
      const common = Math.min(b.length, a.length);
      for (let i = 0; i < common; i++) {
        if (b[i] !== a[i]) { added += 1; removed += 1; }
      }
    }
    return { files: this.map.size, added, removed };
  }
}
