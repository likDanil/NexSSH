// The text of a terminal for an agent (`terminal_read`): the screen, or the last lines of the
// output. Apart from xterm, so that it is tested without a terminal (ui/tests/screen.test.ts).

/** What is read of a terminal's buffer (xterm's `IBuffer` has it). */
export interface Rows {
  readonly length: number;
  getLine(y: number): Row | undefined;
}

export interface Row {
  /** The row continues the one above: the terminal wrapped a long line there. */
  readonly isWrapped: boolean;
  translateToString(trimRight?: boolean): string;
}

export interface RowsText {
  /** The lines; a line the terminal wrapped is one line again. */
  text: string;
  /** The first row given, from 1. */
  from: number;
  /** The last row with something on it, from 1 (0 when nothing is). */
  last: number;
}

/**
 * The rows from `start` to the end of the output, or, with `lines`, that many lines back from
 * the end of the output. The output ends at the last row with something on it, so the empty
 * bottom of a screen (after `clear`, or in a new terminal) is never what gets counted. Lines are
 * counted as they were printed: the rows of a wrapped line count once.
 */
export function readRows(rows: Rows, start: number, lines: number | null): RowsText {
  const row = (y: number) => rows.getLine(y);
  let last = rows.length - 1;
  while (last >= 0 && !row(last)?.translateToString(true).trim()) last--;
  let from = start;
  if (lines !== null) {
    from = last + 1;
    for (let n = 0; n < lines && from > 0; n++) {
      from--;
      // Up to the row the line starts on.
      while (from > 0 && row(from)?.isWrapped) from--;
    }
  }
  const out: string[] = [];
  for (let y = Math.max(0, from); y <= last; y++) {
    const line = row(y);
    if (!line) continue;
    // A row the next one continues is full: spaces at its end are part of the line.
    const text = line.translateToString(!row(y + 1)?.isWrapped);
    if (line.isWrapped && out.length) out[out.length - 1] += text;
    else out.push(text);
  }
  return { text: out.join('\n'), from: from + 1, last: last + 1 };
}
