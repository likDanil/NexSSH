// Small fuzzy matcher for search and the command palette (no dependency needed).
// Scores subsequence matches, preferring word starts and consecutive characters.

export function fuzzyScore(query: string, text: string): number {
  const q = query.trim().toLowerCase();
  if (!q) return 1;
  const t = text.toLowerCase();
  const direct = t.indexOf(q);
  if (direct !== -1) {
    // Substring matches win; earlier and word-start matches win more.
    const wordStart = direct === 0 || /[\s._\-@:/]/.test(t[direct - 1]);
    return 1000 - direct + (wordStart ? 200 : 0) - t.length * 0.5;
  }
  let score = 0;
  let ti = 0;
  let streak = 0;
  for (const ch of q) {
    if (ch === ' ') continue;
    const found = t.indexOf(ch, ti);
    if (found === -1) return 0;
    const wordStart = found === 0 || /[\s._\-@:/]/.test(t[found - 1]);
    streak = found === ti ? streak + 1 : 0;
    score += 10 + streak * 6 + (wordStart ? 12 : 0) - Math.min(found - ti, 10);
    ti = found + 1;
  }
  return Math.max(score - t.length * 0.2, 1);
}

/** Best score over several fields (e.g. name, host, group). */
export function fuzzyBest(query: string, ...fields: (string | undefined)[]): number {
  let best = 0;
  for (const f of fields) {
    if (f) best = Math.max(best, fuzzyScore(query, f));
  }
  return best;
}
