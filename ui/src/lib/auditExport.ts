/**
 * Page through the audit log for an export, and say whether it got to the end.
 *
 * The loop used to stop after a fixed number of pages and report however many rows it had as
 * a success. The file is handed on as evidence, so a log cut short has to be recognisable as
 * one: `complete` is false when the page limit ran out before the server said there was no
 * more.
 */
export async function collectAudit<R>(
  fetchPage: (before?: number) => Promise<{ rows: R[]; next_before: number | null }>,
  maxPages: number,
): Promise<{ rows: R[]; complete: boolean }> {
  const rows: R[] = [];
  let before: number | undefined;
  for (let i = 0; i < maxPages; i++) {
    const page = await fetchPage(before);
    rows.push(...page.rows);
    if (page.next_before === null) return { rows, complete: true };
    before = page.next_before;
  }
  return { rows, complete: false };
}
