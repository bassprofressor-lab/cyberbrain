/**
 * The terminal token, taken from the address once and kept in memory.
 *
 * `cyberbrain serve --terminal` prints an address with the token in its *fragment*, which a
 * browser keeps to itself and never sends to a server. The page takes it on arrival and
 * removes it from the address, so it is not left in a bookmark, in the history, or on
 * screen behind somebody.
 *
 * In memory only: not `localStorage`, not `sessionStorage`. A token that outlives the run of
 * `serve` that minted it is a token that is wrong by the next run, and one that survives in
 * storage is one that can be read by anything else served from this origin.
 */
let token: string | null = null;

/** Take it from a route's query if it is there. Returns true when the address should be
 * rewritten without it. */
export function captureToken(query: URLSearchParams): boolean {
    const found = query.get("t");
    if (!found) return false;
    token = found;
    return true;
}

export function terminalToken(): string | null {
    return token;
}
